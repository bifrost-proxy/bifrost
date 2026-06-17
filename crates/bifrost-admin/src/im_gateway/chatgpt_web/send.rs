use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use super::browser::{BrowserSession, CdpClient, CdpEvent, CdpPage};
use super::{
    evaluate_value, seed_chatgpt_cookies, stop_requested, stopped_error, AuthState, RuntimeConfig,
    F_CONVERSATION_URL,
};

const HANDOFF_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const HANDOFF_PAGE_PROBE_TIMEOUT_SECS: u64 = 3;
const TARGET_PAGE_STABLE_MS: u64 = 1_000;
const TARGET_PAGE_POLL_MS: u64 = 250;
const COMPOSER_PASTE_THRESHOLD_CHARS: usize = 120;
const AUTH_FLOW_RECOVERY_TIMEOUT_SECS: u64 = 10 * 60;

pub(in crate::im_gateway::chatgpt_web) async fn send_with_browser(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: Option<&str>,
    text: &str,
    stop_marker_path: &Path,
) -> Result<SendResult, String> {
    // Retry the entire send operation if we hit transient browser UI issues
    // (e.g. send button not found, click didn't trigger POST, CDP timeout).
    // CDP errors get more attempts because the first retry closes the dead
    // pooled tab (if any) and subsequent attempts create a fresh tab.
    const MAX_SEND_ATTEMPTS: usize = 5;
    // After this many consecutive CDP errors on the same pooled tab, evict
    // and close it — the tab is likely dead/frozen.  Before this threshold
    // the tab is kept alive to give it a chance to recover during backoff.
    const CDP_EVICT_THRESHOLD: u32 = 3;
    const MAX_BACKOFF_SECS: u64 = 15;

    let mut last_error = String::new();
    let mut consecutive_cdp_errors: u32 = 0;
    for attempt in 1..=MAX_SEND_ATTEMPTS {
        if attempt > 1 {
            // CDP errors get progressively longer backoff (capped at MAX_BACKOFF_SECS).
            // Non-CDP transient errors get a short 1s backoff.
            let backoff = if is_cdp_transient_error(&last_error) {
                let secs = match consecutive_cdp_errors {
                    1 => 3,
                    2 => 6,
                    3 => 10,
                    _ => MAX_BACKOFF_SECS,
                };
                Duration::from_secs(secs.min(MAX_BACKOFF_SECS))
            } else {
                Duration::from_secs(1)
            };
            warn!(
                attempt,
                MAX_SEND_ATTEMPTS,
                consecutive_cdp_errors,
                cdp_evict_threshold = CDP_EVICT_THRESHOLD,
                backoff_secs = backoff.as_secs(),
                last_error = %last_error,
                "chatgpt_web send: retrying entire send operation"
            );
            sleep(backoff).await;
        }
        match send_with_browser_once(
            config,
            auth,
            conversation_id,
            text,
            stop_marker_path,
            consecutive_cdp_errors,
            CDP_EVICT_THRESHOLD,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(e) if is_retryable_send_error(&e) && attempt < MAX_SEND_ATTEMPTS => {
                if is_cdp_transient_error(&e) {
                    consecutive_cdp_errors += 1;
                } else {
                    consecutive_cdp_errors = 0;
                }
                warn!(
                    attempt,
                    consecutive_cdp_errors,
                    error = %e,
                    "chatgpt_web send: transient send failure, will retry"
                );
                // After evicting the dead tab, reset the counter so the
                // fresh replacement tab starts with normal backoff instead
                // of inheriting the dead tab's penalty.
                if consecutive_cdp_errors >= CDP_EVICT_THRESHOLD {
                    info!(
                        consecutive_cdp_errors,
                        "chatgpt_web send: resetting CDP error counter after tab eviction"
                    );
                    consecutive_cdp_errors = 0;
                }
                last_error = e;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_error)
}

/// Check if a send error is transient and worth retrying the whole operation.
fn is_retryable_send_error(error: &str) -> bool {
    error.contains("browser_send_not_submitted")
        || error.contains("send_button_not_found")
        || error.contains("browser_wrong_page")
        || error.contains("500 Internal Server Error")
        || error.contains("Cannot find default execution context")
        || error.contains("create browser page failed")
        || error.contains("connect CDP websocket failed")
        || is_cdp_transient_error(error)
}

/// CDP-level transient failures — the browser/tab may be temporarily
/// unresponsive (e.g. during internal navigation, GC, or heavy JS execution).
/// These are worth retrying because they often self-resolve within seconds.
fn is_cdp_transient_error(error: &str) -> bool {
    error.contains("CDP command timed out")
        || error.contains("CDP connection closed")
        || error.contains("Inspected target navigated or closed")
        || error.contains("Target closed")
        || error.contains("Session closed")
        || error.contains("CDP event channel closed")
}

/// Normalize whitespace for comparing text inserted into ProseMirror.
/// ProseMirror wraps lines in `<p>` tags, so `innerText` returns double
/// newlines between paragraphs. Collapse all runs of whitespace (including
/// newlines) into single spaces for a lenient equality check.
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::im_gateway::chatgpt_web) enum ComposerTextInjectionMode {
    InsertText,
    NativeClipboardPaste,
}

pub(in crate::im_gateway::chatgpt_web) fn composer_text_injection_mode(
    text: &str,
) -> ComposerTextInjectionMode {
    if text.chars().count() > COMPOSER_PASTE_THRESHOLD_CHARS {
        ComposerTextInjectionMode::NativeClipboardPaste
    } else {
        ComposerTextInjectionMode::InsertText
    }
}

pub(in crate::im_gateway::chatgpt_web) fn send_button_ready_max_wait(text: &str) -> Duration {
    if composer_text_injection_mode(text) == ComposerTextInjectionMode::NativeClipboardPaste {
        let chars = text.chars().count() as u64;
        Duration::from_secs((30 + chars / 10_000).clamp(30, 180))
    } else {
        Duration::from_secs(10)
    }
}

pub(in crate::im_gateway::chatgpt_web) fn send_button_ready_retry_max_wait(text: &str) -> Duration {
    if composer_text_injection_mode(text) == ComposerTextInjectionMode::NativeClipboardPaste {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(15)
    }
}

pub(in crate::im_gateway::chatgpt_web) fn native_clipboard_paste_followup_instruction(
) -> &'static str {
    "\n\n请阅读刚刚粘贴或上传的完整 Markdown 内容，并严格按其中要求直接输出最终结果正文。不要只说明文件内容，也不要输出代码块包装。"
}

async fn send_with_browser_once(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: Option<&str>,
    text: &str,
    stop_marker_path: &Path,
    consecutive_cdp_errors: u32,
    cdp_evict_threshold: u32,
) -> Result<SendResult, String> {
    let headless = config.browser.execution_mode != "headed";
    info!(
        headless,
        conversation_id = ?conversation_id,
        "chatgpt_web send: starting send_with_browser"
    );

    // Reuse existing browser or launch a new one (persistent across calls).
    let mut browser = BrowserSession::get_or_launch(config, headless).await?;
    info!("chatgpt_web send: browser ready");
    if conversation_id.is_none() {
        browser.close_chatgpt_pages_for_fresh_run().await;
    }

    // Each concurrent run gets its own isolated tab.  This allows multiple
    // channels (Feishu, WeChat, CLI, …) to operate the same browser in
    // parallel without CDP / page-state collisions, while still sharing the
    // same user-data-dir (login cookies, profile, etc.).
    let navigate_url = if let Some(cid) = conversation_id {
        format!("{}/c/{}", config.chatgpt.base_url, cid)
    } else {
        new_conversation_url(config)
    };

    let mut target_id = None;
    let mut client: Option<Arc<CdpClient>> = None;
    // Track whether the tab/CDP came from the conversation pool.  Reused
    // tabs MUST NOT be closed on failure (other concurrent runs may also
    // be sharing the same conversation in the future, and even today a
    // failed run does not invalidate the underlying tab).  Newly created
    // tabs that we never registered into the pool MUST be closed to avoid
    // leaks.
    let mut reused_from_pool = false;
    let result = async {
        // Try to reuse a long-lived tab from the per-profile pool.  If the
        // conversation already has a tab, navigate is unnecessary — the
        // composer is right there waiting for input.
        if let Some(cid) = conversation_id {
            if let Some(tab) =
                super::browser::get_conversation_tab(&config.profile_dir, cid)
            {
                info!(
                    conversation_id = %cid,
                    target_id = %tab.target_id,
                    "chatgpt_web send: reusing pooled conversation tab"
                );
                target_id = Some(tab.target_id.clone());
                client = Some(tab.cdp.clone());
                reused_from_pool = true;
            }
        }

        if client.is_none() {
            if let Some(cid) = conversation_id {
                if let Some(page) = browser
                    .find_conversation_page(&config.chatgpt.base_url, cid)
                    .await?
                {
                    let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3)
                        .await?;
                    cdp.enable_domains().await?;
                    let cdp = Arc::new(cdp);
                    if let Some(evicted) = super::browser::register_conversation_tab(
                        &config.profile_dir,
                        cid,
                        page.id.clone(),
                        cdp.clone(),
                    ) {
                        evicted.shutdown(&browser).await;
                    }
                    info!(
                        conversation_id = %cid,
                        target_id = %page.id,
                        page_url = %page.url,
                        "chatgpt_web send: reusing existing browser conversation tab"
                    );
                    target_id = Some(page.id);
                    client = Some(cdp);
                    reused_from_pool = true;
                }
            }
        }

        if client.is_none() {
            // Create a brand-new tab and connect CDP with retry logic.
            // The browser may occasionally return HTTP 500 on the WebSocket
            // endpoint for a freshly-created tab; retrying (with a fresh tab)
            // resolves the transient issue.
            let (page, cdp) = create_tab_with_cdp_retry(&browser, 3).await?;
            info!(
                page_id = %page.id,
                page_url = %page.url,
                "chatgpt_web send: isolated tab created for this run"
            );
            target_id = Some(page.id.clone());
            // Assign CDP client early so diagnostic screenshots can be captured
            // for ANY error that occurs after this point (including browser_wrong_page).
            client = Some(Arc::new(cdp));
        }
        let cdp = client.as_ref().unwrap().clone();
        let cdp = cdp.as_ref();

        if !reused_from_pool {
            // Enable CDP domains for event-driven navigation/network detection.
            cdp.enable_domains().await?;
            cdp.send("Runtime.enable", json!({})).await?;

            // Wait for the about:blank execution context to become available.
            info!("chatgpt_web send: waiting for execution context in new tab");
            wait_for_execution_context(cdp, Duration::from_secs(10)).await?;

            // Seed cookies BEFORE navigating to chatgpt.com so that the first
            // page load is already authenticated.
            seed_chatgpt_cookies(cdp, auth).await?;
        }

        if !headless {
            cdp.send("Page.bringToFront", json!({})).await?;
        }

        if !reused_from_pool {
            // Now navigate the tab to the desired URL (homepage or conversation).
            // Use navigate_and_wait which subscribes to events BEFORE sending
            // Page.navigate, preventing missed load events on fast/cached pages.
            info!(%navigate_url, "chatgpt_web send: navigating isolated tab to target URL");
            cdp.navigate_and_wait(&navigate_url, Duration::from_secs(15))
                .await?;
        } else {
            // Pooled tab is already on the conversation URL.  Verify the
            // conversation id still matches so we don't accidentally inject
            // text into the wrong conversation.
            //
            // CDP operations on a pooled tab may transiently fail if the page
            // is in the middle of heavy JS execution or internal navigation.
            // We give it one retry with a short backoff before propagating.
            match current_conversation_id(cdp).await {
                Ok(Some(actual)) if Some(actual.as_str()) == conversation_id => {
                    info!(
                        conversation_id = ?conversation_id,
                        "chatgpt_web send: pooled tab confirmed on expected conversation"
                    );
                }
                Ok(other) => {
                    warn!(
                        actual = ?other,
                        expected = ?conversation_id,
                        "chatgpt_web send: pooled tab is on a different page, falling back to navigate"
                    );
                    navigate_with_cdp_retry(cdp, &navigate_url).await?;
                }
                Err(e) if is_cdp_transient_error(&e) => {
                    // CDP transient failure — wait and retry once before giving up.
                    warn!(
                        error = %e,
                        "chatgpt_web send: CDP transient error reading pooled tab URL, retrying after backoff"
                    );
                    sleep(Duration::from_secs(2)).await;
                    match current_conversation_id(cdp).await {
                        Ok(Some(actual)) if Some(actual.as_str()) == conversation_id => {
                            info!(
                                conversation_id = ?conversation_id,
                                "chatgpt_web send: pooled tab confirmed after retry"
                            );
                        }
                        _ => {
                            info!("chatgpt_web send: still unable to confirm pooled tab, navigating");
                            navigate_with_cdp_retry(cdp, &navigate_url).await?;
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "chatgpt_web send: failed to read pooled tab URL, navigating to expected URL"
                    );
                    navigate_with_cdp_retry(cdp, &navigate_url).await?;
                }
            }
        }

        // Wait for the page to settle on the expected URL.
        // expected_conversation_id tracks what we expect to see in the URL.
        // For new conversations it's None (homepage); for existing ones it's
        // the conversation_id we navigated to.  If the page doesn't settle on
        // the expected URL, wait_target_page / fallback logic handles it.
        let mut expected_conversation_id = conversation_id;
        let strong_consistency = config.chatgpt.is_strong_consistency();

        recover_visible_auth_flow_if_needed(
            cdp,
            &navigate_url,
            expected_conversation_id,
            Duration::from_secs(AUTH_FLOW_RECOVERY_TIMEOUT_SECS),
        )
        .await?;

        // First wait_target_page: if the conversation URL redirects to the
        // homepage (deleted/expired conversation), fall back based on
        // session_consistency mode.
        if let Err(error) =
            wait_target_page(cdp, expected_conversation_id, Duration::from_secs(30), false).await
        {
            if expected_conversation_id.is_none() {
                warn!(
                    %error,
                    "chatgpt_web send: fresh conversation target was not clean, forcing new chat navigation"
                );
                navigate_to_new_conversation(cdp, config, &error).await?;
            } else if expected_conversation_id.is_some() && should_retry_as_new_conversation(&error) {
                let is_rate_limited = error.contains("body_error=")
                    && (error.contains("429") || error.contains("rate"));

                if strong_consistency && is_rate_limited {
                    // ── Strong consistency 429 recovery ──
                    // (detailed block for 429-specific handling)
                    let retry_secs = config.chatgpt.rate_limit_retry_secs;
                    let max_retries = config.chatgpt.rate_limit_max_retries;
                    let original_conv_url = if let Some(cid) = conversation_id {
                        format!("{}/c/{}", config.chatgpt.base_url, cid)
                    } else {
                        String::new()
                    };
                    let mut recovered = false;
                    for retry in 1..=max_retries {
                        warn!(
                            %error,
                            retry,
                            max_retries,
                            wait_secs = retry_secs,
                            conversation_id = ?conversation_id,
                            "chatgpt_web send [strong]: 429 rate-limit, waiting {retry_secs}s before recovery"
                        );
                        sleep(Duration::from_secs(retry_secs)).await;
                        dismiss_modal_and_wait(cdp).await;

                        // Check if we're still on the right conversation page.
                        if wait_composer(cdp, expected_conversation_id, Duration::from_secs(15)).await.is_ok() {
                            info!(
                                retry,
                                "chatgpt_web send [strong]: composer recovered on original conversation (no navigation needed)"
                            );
                            recovered = true;
                            break;
                        }

                        // Page is NOT on the original conversation — re-navigate.
                        if !original_conv_url.is_empty() {
                            warn!(
                                retry,
                                %original_conv_url,
                                "chatgpt_web send [strong]: page redirected away from conversation, re-navigating to original URL"
                            );
                            dismiss_modal_and_wait(cdp).await;
                            if let Err(nav_err) = cdp.navigate_and_wait(&original_conv_url, Duration::from_secs(20)).await {
                                warn!(
                                    retry,
                                    error = %nav_err,
                                    "chatgpt_web send [strong]: re-navigation failed, will retry"
                                );
                                continue;
                            }
                            sleep(Duration::from_secs(3)).await;
                            dismiss_modal_and_wait(cdp).await;
                            if wait_composer(cdp, expected_conversation_id, Duration::from_secs(20)).await.is_ok() {
                                info!(
                                    retry,
                                    "chatgpt_web send [strong]: composer recovered after re-navigation to original conversation ✓"
                                );
                                recovered = true;
                                break;
                            }
                            warn!(
                                retry,
                                "chatgpt_web send [strong]: re-navigation done but composer not found, will retry"
                            );
                        }
                    }
                    if !recovered {
                        if !original_conv_url.is_empty() {
                            warn!(
                                conversation_id = ?conversation_id,
                                "chatgpt_web send [strong]: final re-navigation attempt to original conversation"
                            );
                            dismiss_modal_and_wait(cdp).await;
                            let _ = cdp.navigate_and_wait(&original_conv_url, Duration::from_secs(20)).await;
                            sleep(Duration::from_secs(3)).await;
                            dismiss_modal_and_wait(cdp).await;
                            if wait_composer(cdp, expected_conversation_id, Duration::from_secs(20)).await.is_ok() {
                                info!(
                                    conversation_id = ?conversation_id,
                                    "chatgpt_web send [strong]: final re-navigation succeeded ✓"
                                );
                                recovered = true;
                            }
                        }
                        if !recovered {
                            return Err(format!(
                                "conversation_lost: 429 recovery failed after {} retries — could not return to original conversation {}",
                                max_retries,
                                conversation_id.unwrap_or("unknown")
                            ));
                        }
                    }
                } else if strong_consistency {
                    // ── Strong consistency: non-429 redirect (page jumped to homepage) ──
                    // The page was redirected away from the conversation (e.g. expired
                    // session, network glitch, ChatGPT internal redirect).
                    // MUST re-navigate back to the original conversation URL.
                    let original_conv_url = if let Some(cid) = conversation_id {
                        format!("{}/c/{}", config.chatgpt.base_url, cid)
                    } else {
                        String::new()
                    };
                    if !original_conv_url.is_empty() {
                        warn!(
                            %error,
                            conversation_id = ?conversation_id,
                            %original_conv_url,
                            "chatgpt_web send [strong]: page redirected away (non-429), re-navigating to original conversation"
                        );
                        dismiss_modal_and_wait(cdp).await;
                        let max_retries = config.chatgpt.rate_limit_max_retries.max(2);
                        let mut recovered = false;
                        for retry in 1..=max_retries {
                            if let Err(nav_err) = cdp.navigate_and_wait(&original_conv_url, Duration::from_secs(20)).await {
                                warn!(
                                    retry,
                                    error = %nav_err,
                                    "chatgpt_web send [strong]: re-navigation failed, waiting 10s before retry"
                                );
                                sleep(Duration::from_secs(10)).await;
                                dismiss_modal_and_wait(cdp).await;
                                continue;
                            }
                            sleep(Duration::from_secs(3)).await;
                            dismiss_modal_and_wait(cdp).await;
                            if wait_composer(cdp, expected_conversation_id, Duration::from_secs(20)).await.is_ok() {
                                info!(
                                    retry,
                                    "chatgpt_web send [strong]: composer recovered after re-navigation ✓"
                                );
                                recovered = true;
                                break;
                            }
                            warn!(retry, "chatgpt_web send [strong]: re-navigation done but composer not found, will retry");
                            sleep(Duration::from_secs(5)).await;
                            dismiss_modal_and_wait(cdp).await;
                        }
                        if !recovered {
                            return Err(format!(
                                "conversation_lost: non-429 redirect recovery failed after {} retries — could not return to conversation {}",
                                max_retries,
                                conversation_id.unwrap_or("unknown")
                            ));
                        }
                    } else {
                        // No conversation_id to navigate back to — fall through
                        warn!(
                            %error,
                            "chatgpt_web send [strong]: page redirected but no conversation_id to navigate back to"
                        );
                        expected_conversation_id = None;
                    }
                } else {
                    // ── Weak consistency mode ──
                    // Accept whatever page we're on. Dismiss modals and proceed.
                    warn!(
                        %error,
                        "chatgpt_web send [weak]: conversation not reachable, using current page as-is (no navigation)"
                    );
                    dismiss_modal_and_wait(cdp).await;
                    expected_conversation_id = None;
                }
            } else {
                return Err(error);
            }
        }
        // Dismiss any rate-limit modal dialog that may be blocking the page
        // (e.g. "请求过于频繁" / "Too many requests" with a "明白了" button).
        // This must happen before wait_composer since the modal overlay blocks
        // composer visibility and interaction.
        dismiss_modal_and_wait(cdp).await;

        info!("chatgpt_web send: waiting for composer");
        let mut composer_result =
            wait_composer(cdp, expected_conversation_id, Duration::from_secs(60)).await;

        // If composer wait failed, it might be because a rate-limit modal is
        // blocking interaction.  Try dismissing it and retry once.
        if composer_result.is_err() && dismiss_rate_limit_modal(cdp).await {
            tracing::info!("chatgpt_web send: dismissed modal blocking composer, retrying wait_composer");
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            composer_result =
                wait_composer(cdp, expected_conversation_id, Duration::from_secs(30)).await;
        }

        // Detect stale conversation (404 / "not found" page) and fall back to new conversation.
        // Only check when we are actually on a conversation page (expected_conversation_id is
        // still set); if the navigation phase already fell back, skip this.
        if expected_conversation_id.is_some() {
            let should_fallback = match &composer_result {
                Err(_) => {
                    // Before treating as stale, check if this is actually a
                    // rate-limit situation in strong consistency mode.
                    let page_state = inspect_page(cdp).await.unwrap_or(json!({}));
                    let on_homepage = page_state
                        .get("pageKind")
                        .and_then(Value::as_str)
                        == Some("new_conversation");
                    if strong_consistency && on_homepage {
                        // Page was redirected to homepage (likely after 429
                        // modal dismiss). Strong mode: RE-NAVIGATE to original
                        // conversation URL to maintain context continuity.
                        warn!(
                            conversation_id = ?conversation_id,
                            "chatgpt_web send [strong]: composer failed, page on homepage — re-navigating to original conversation"
                        );
                        let retry_secs = config.chatgpt.rate_limit_retry_secs;
                        let max_retries = config.chatgpt.rate_limit_max_retries;
                        let original_conv_url = if let Some(cid) = conversation_id {
                            format!("{}/c/{}", config.chatgpt.base_url, cid)
                        } else {
                            String::new()
                        };
                        let mut recovered = false;
                        for retry in 1..=max_retries {
                            warn!(
                                retry, max_retries, wait_secs = retry_secs,
                                "chatgpt_web send [strong]: waiting {retry_secs}s then re-navigating to original conversation"
                            );
                            sleep(Duration::from_secs(retry_secs)).await;
                            dismiss_modal_and_wait(cdp).await;

                            // First check if the page somehow returned to the conversation.
                            if wait_composer(cdp, expected_conversation_id, Duration::from_secs(10)).await.is_ok() {
                                info!(retry, "chatgpt_web send [strong]: composer recovered on original conversation ✓");
                                recovered = true;
                                break;
                            }

                            // Navigate back to original conversation.
                            if !original_conv_url.is_empty() {
                                warn!(retry, %original_conv_url, "chatgpt_web send [strong]: re-navigating to original conversation");
                                dismiss_modal_and_wait(cdp).await;
                                if let Err(nav_err) = cdp.navigate_and_wait(&original_conv_url, Duration::from_secs(20)).await {
                                    warn!(retry, error = %nav_err, "chatgpt_web send [strong]: re-navigation failed, will retry");
                                    continue;
                                }
                                sleep(Duration::from_secs(3)).await;
                                dismiss_modal_and_wait(cdp).await;
                                if wait_composer(cdp, expected_conversation_id, Duration::from_secs(20)).await.is_ok() {
                                    info!(retry, "chatgpt_web send [strong]: composer recovered after re-navigation ✓");
                                    recovered = true;
                                    break;
                                }
                            }
                            dismiss_modal_and_wait(cdp).await;
                        }
                        if recovered {
                            // composer_result is now ok; skip fallback
                            false
                        } else {
                            warn!("chatgpt_web send [strong]: exhausted retries for conversation recovery — will fall back");
                            true
                        }
                    } else {
                        warn!("chatgpt_web send: composer wait failed with conversation_id, likely stale");
                        true
                    }
                }
                Ok(()) => {
                    let page_state = inspect_page(cdp).await.unwrap_or(json!({}));
                    let body = page_state
                        .get("bodyText")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase();
                    let page_url = page_state
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let is_not_found = body.contains("not found")
                        || body.contains("unable to load")
                        || body.contains("something went wrong");
                    if is_not_found {
                        warn!(
                            body_preview = %body.chars().take(200).collect::<String>(),
                            %page_url,
                            "chatgpt_web send: conversation page shows error, likely deleted"
                        );
                    }
                    is_not_found
                }
            };
            if should_fallback {
                if strong_consistency && conversation_id.is_some() {
                    // Strong consistency: DO NOT send on a new conversation without
                    // context.  Return "conversation_lost" so the caller can retry
                    // with conversation history prepended to the message.
                    return Err(format!(
                        "conversation_lost: stale conversation recovery exhausted, original conversation {} unreachable",
                        conversation_id.unwrap_or("unknown")
                    ));
                }
                // Weak consistency: accept current page.
                info!("chatgpt_web send [weak]: fallback — dismissing modals and using current page (no navigation)");
                dismiss_modal_and_wait(cdp).await;
                expected_conversation_id = None;
                wait_composer(cdp, None, Duration::from_secs(60)).await?;
            }
        } else {
            composer_result?;
        }
        let mut retry_as_new_available = expected_conversation_id.is_some();
        let send = loop {
            if let Err(error) =
                wait_target_page(cdp, expected_conversation_id, Duration::from_secs(30), true)
                    .await
            {
                if retry_as_new_available && should_retry_as_new_conversation(&error) {
                    retry_as_new_available = false;
                    // Don't navigate — dismiss modals and use current page.
                    dismiss_modal_and_wait(cdp).await;
                    expected_conversation_id = None;
                    wait_composer(cdp, None, Duration::from_secs(60)).await?;
                    continue;
                }
                return Err(error);
            }

            info!("chatgpt_web send: composer found, typing message");
            // One more check for rate-limit modals that may have appeared
            // after the composer became visible but before we type.
            dismiss_modal_and_wait(cdp).await;
            assert_target_page(cdp, expected_conversation_id, true).await?;

            // Set composer text — retry once after dismissing modal on failure
            // (modal overlay can cause CDP timeout under concurrent load).
            if let Err(e) = set_composer_text(cdp, text).await {
                warn!(error = %e, "chatgpt_web send: set_composer_text failed, dismissing modal and retrying");
                dismiss_modal_and_wait(cdp).await;
                sleep(Duration::from_millis(500)).await;
                set_composer_text(cdp, text).await?;
            }

            let injection_mode = composer_text_injection_mode(text);
            let native_clipboard_paste =
                injection_mode == ComposerTextInjectionMode::NativeClipboardPaste;
            let send_button_max_wait = send_button_ready_max_wait(text);
            let send_button_retry_max_wait = send_button_ready_retry_max_wait(text);
            info!(
                ?injection_mode,
                max_wait_secs = send_button_max_wait.as_secs(),
                retry_max_wait_secs = send_button_retry_max_wait.as_secs(),
                "chatgpt_web send: polling until send button is enabled"
            );
            // Poll until the send button becomes actionable. Native clipboard
            // paste can spend time uploading/parsing large text before the
            // button flips to sendable, but this returns immediately once ready.
            // If still not found, short-text paths can fall through to Enter.
            let send_button_available =
                match wait_send_button_ready(cdp, send_button_max_wait).await {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "chatgpt_web send: send button not ready, dismissing modal and retrying");
                    dismiss_modal_and_wait(cdp).await;
                    sleep(Duration::from_millis(500)).await;
                    match wait_send_button_ready(cdp, send_button_retry_max_wait).await {
                        Ok(()) => true,
                        Err(e2) => {
                            if native_clipboard_paste {
                                return Err(format!(
                                    "browser_ui: send button not actionable after native clipboard paste upload wait: {e2}"
                                ));
                            }
                            warn!(error = %e2, "chatgpt_web send: send button still not found after retry, will use Enter fallback");
                            false
                        }
                    }
                }
            };
            assert_target_page(cdp, expected_conversation_id, true).await?;

            // Subscribe to CDP events before triggering send so we don't miss the
            // f/conversation POST.
            let mut events = cdp.subscribe();
            let before = now_ms();
            let mut request_ids = BTreeSet::new();

            // Primary send: click the send button directly via CDP trusted mouse events.
            // This is more reliable than Enter which can be interpreted as a newline
            // in contenteditable composers on existing conversation pages.
            // If send button was not found during wait phase, skip straight to Enter.
            if send_button_available {
                let btn = locate_send_button(cdp).await.unwrap_or_default();
                let btn_ok = btn.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let btn_disabled = btn.get("disabled").and_then(Value::as_bool).unwrap_or(true);
                if btn_ok && !btn_disabled {
                    let x = btn.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                    let y = btn.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                    cdp_click(cdp, x, y).await?;
                    info!(x, y, "chatgpt_web send: clicked send button (primary send)");
                } else {
                    // Button found but not clickable — Enter fallback
                    info!(?btn, "chatgpt_web send: send button not clickable, falling back to Enter");
                    focus_composer(cdp).await?;
                    sleep(Duration::from_millis(100)).await;
                    dispatch_enter(cdp).await?;
                    info!("chatgpt_web send: Enter dispatched (button not clickable fallback)");
                }
            } else {
                // Send button never found — use Enter directly
                info!("chatgpt_web send: no send button available, using Enter to send");
                focus_composer(cdp).await?;
                sleep(Duration::from_millis(100)).await;
                dispatch_enter(cdp).await?;
                info!("chatgpt_web send: Enter dispatched (no button fallback)");
            }

            // Wait a short window for f/conversation POST to appear.
            let quick_timeout = Duration::from_secs(8);
            let send_result =
                match wait_send_handoff(cdp, &mut browser, &mut events, before, quick_timeout, stop_marker_path, &mut request_ids)
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(_quick_err) => {
                        // Only try secondary send if POST was NOT captured.
                        // If POST was captured (request_ids non-empty), the message was already
                        // sent—just wait longer for the SSE stream to complete.
                        if request_ids.is_empty() {
                            info!(
                                "chatgpt_web send: no POST captured, trying secondary send method"
                            );
                            let secondary_btn = locate_send_button(cdp).await.unwrap_or_default();
                            let secondary_btn_ok = secondary_btn
                                .get("ok")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let secondary_btn_disabled = secondary_btn
                                .get("disabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                            if secondary_btn_ok && !secondary_btn_disabled {
                                let dom_click = click_send_button_dom(cdp).await?;
                                info!(
                                    result = %dom_click,
                                    "chatgpt_web send: DOM button click dispatched (secondary fallback)"
                                );
                                let dom_click_result = wait_send_handoff(
                                    cdp,
                                    &mut browser,
                                    &mut events,
                                    before,
                                    Duration::from_secs(3),
                                    stop_marker_path,
                                    &mut request_ids,
                                )
                                .await;
                                match dom_click_result {
                                    Ok(result) => Ok(result),
                                    Err(error) if request_ids.is_empty() => {
                                        info!(
                                            %error,
                                            "chatgpt_web send: DOM click did not submit, trying Enter fallback"
                                        );
                                        focus_composer(cdp).await?;
                                        sleep(Duration::from_millis(100)).await;
                                        dispatch_enter(cdp).await?;
                                        info!(
                                            "chatgpt_web send: Enter dispatched (tertiary fallback)"
                                        );
                                        let fallback_timeout = quick_timeout;
                                        wait_send_handoff(
                                            cdp,
                                            &mut browser,
                                            &mut events,
                                            before,
                                            fallback_timeout,
                                            stop_marker_path,
                                            &mut request_ids,
                                        )
                                        .await
                                    }
                                    Err(_) => {
                                        info!(
                                            tracked = request_ids.len(),
                                            "chatgpt_web send: POST captured after DOM click, continuing to wait for SSE completion"
                                        );
                                        let fallback_timeout =
                                            Duration::from_secs(config.chatgpt.timeout_secs)
                                                .saturating_sub(quick_timeout);
                                        if fallback_timeout.is_zero() {
                                            Err(
                                                "chatgpt_web send: timeout exhausted after fallback"
                                                    .to_string(),
                                            )
                                        } else {
                                            wait_send_handoff(
                                                cdp,
                                                &mut browser,
                                                &mut events,
                                                before,
                                                fallback_timeout,
                                                stop_marker_path,
                                                &mut request_ids,
                                            )
                                            .await
                                        }
                                    }
                                }
                            } else {
                                // We used Enter but no POST – try clicking the button
                                let btn2 = locate_send_button(cdp).await?;
                                let btn2_ok =
                                    btn2.get("ok").and_then(Value::as_bool).unwrap_or(false);
                                let btn2_disabled = btn2
                                    .get("disabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(true);
                                if btn2_ok && !btn2_disabled {
                                    let x = btn2.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                                    let y = btn2.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                                    cdp_click(cdp, x, y).await?;
                                    info!(
                                        x, y,
                                        "chatgpt_web send: clicked send button (secondary fallback)"
                                    );
                                } else {
                                    info!(
                                        ?btn2,
                                        "chatgpt_web send: send button unavailable in fallback, retrying Enter"
                                    );
                                    focus_composer(cdp).await?;
                                    sleep(Duration::from_millis(200)).await;
                                    dispatch_enter(cdp).await?;
                                }
                                let fallback_timeout = if request_ids.is_empty() {
                                    quick_timeout
                                } else {
                                    Duration::from_secs(config.chatgpt.timeout_secs)
                                        .saturating_sub(quick_timeout)
                                };
                                if fallback_timeout.is_zero() {
                                    Err(
                                        "chatgpt_web send: timeout exhausted after fallback"
                                            .to_string(),
                                    )
                                } else {
                                    wait_send_handoff(
                                        cdp,
                                        &mut browser,
                                        &mut events,
                                        before,
                                        fallback_timeout,
                                        stop_marker_path,
                                        &mut request_ids,
                                    )
                                    .await
                                }
                            }
                        } else {
                            info!(
                                tracked = request_ids.len(),
                                "chatgpt_web send: POST already captured, continuing to wait for SSE completion"
                            );
                            let fallback_timeout = Duration::from_secs(config.chatgpt.timeout_secs)
                                .saturating_sub(quick_timeout);
                            if fallback_timeout.is_zero() {
                                Err("chatgpt_web send: timeout exhausted after fallback".to_string())
                            } else {
                                wait_send_handoff(
                                    cdp,
                                    &mut browser,
                                    &mut events,
                                    before,
                                    fallback_timeout,
                                    stop_marker_path,
                                    &mut request_ids,
                                )
                                .await
                            }
                        }
                    }
                };
            info!(?send_result, "chatgpt_web send: handoff result");
            match send_result {
                Ok(mut send) => {
                    if send.conversation_id.is_none() {
                        send.conversation_id = current_conversation_id(cdp)
                            .await?
                            .or_else(|| conversation_id.map(ToString::to_string));
                    }
                    send.event_types.push("browser_ui".to_string());
                    break send;
                }
                Err(error) if error.starts_with("sse_interrupted") || error.starts_with("timeout") => {
                    let recovered = current_conversation_id(cdp).await?;
                    let mut event_types =
                        vec!["handoff_recovered_after_sse_interrupt".to_string()];
                    if !request_ids.is_empty() {
                        event_types.push("browser_post_captured".to_string());
                    }
                    break SendResult {
                        conversation_id: recovered.or_else(|| conversation_id.map(ToString::to_string)),
                        turn_exchange_id: None,
                        event_types,
                        sse_detail: None,
                    };
                }
                Err(error)
                    if retry_as_new_available
                        && request_ids.is_empty()
                        && should_retry_as_new_conversation(&error) =>
                {
                    retry_as_new_available = false;
                    // Don't navigate — dismiss modals and use current page.
                    dismiss_modal_and_wait(cdp).await;
                    expected_conversation_id = None;
                    wait_composer(cdp, None, Duration::from_secs(60)).await?;
                }
                Err(error) => return Err(error),
            }
        };
        Ok(send)
    }
    .await;

    // On failure, capture a diagnostic screenshot before closing the tab.
    // The screenshot is saved to a temp file and its path is appended to the
    // error string so upstream code can extract and send it via IM.
    let mut screenshot_path = None;
    if result.is_err() {
        if let Some(ref cdp) = client {
            match capture_diagnostic_screenshot(cdp).await {
                Ok(path) => {
                    info!(
                        path = %path.display(),
                        "chatgpt_web send: diagnostic screenshot captured"
                    );
                    screenshot_path = Some(path);
                }
                Err(e) => {
                    warn!(error = %e, "chatgpt_web send: failed to capture diagnostic screenshot");
                }
            }
        }
    }

    // Tab lifecycle decisions:
    //   - On success: register the tab into the per-profile conversation
    //     pool keyed by conversation_id so subsequent runs in the same
    //     conversation can reuse it (no navigate, no relog, instant
    //     composer).  If the pool is full an LRU tab is evicted and we
    //     close it on the browser here.
    //   - On failure of a reused tab: do NOT close — the tab itself is
    //     still healthy; the failure is at most a transient send issue
    //     and the next attempt should keep using the same tab.
    //   - On failure of a fresh (unregistered) tab: close it to avoid
    //     orphan tab leaks.
    match (&result, target_id.as_deref(), client.clone()) {
        (Ok(send), Some(tid), Some(cdp_arc)) => {
            if let Some(cid) = send.conversation_id.as_deref() {
                if let Some(evicted) = super::browser::register_conversation_tab(
                    &config.profile_dir,
                    cid,
                    tid.to_string(),
                    cdp_arc,
                ) {
                    evicted.shutdown(&browser).await;
                }
            } else {
                // No conversation id yet (new conversation that didn't
                // resolve) — the tab is unusable for future routing, so
                // close it to free resources.
                warn!(
                    target_id = %tid,
                    "chatgpt_web send: success without conversation_id, closing orphan tab"
                );
                if let Some(arc) = client.clone() {
                    arc.close();
                }
                if let Err(e) = browser.close_target(tid).await {
                    warn!(error = %e, "chatgpt_web send: failed to close orphan tab (non-fatal)");
                }
            }
        }
        (Err(_), Some(tid), Some(cdp_arc)) if !reused_from_pool => {
            // Fresh tab that never made it into the pool — close to avoid leaks.
            info!(
                target_id = %tid,
                "chatgpt_web send: closing fresh tab after failure"
            );
            cdp_arc.close();
            if let Err(e) = browser.close_target(tid).await {
                warn!(error = %e, "chatgpt_web send: failed to close failed tab (non-fatal)");
            }
        }
        (Err(e), Some(tid), Some(ref cdp_arc)) if reused_from_pool && is_cdp_transient_error(e) => {
            // The pooled tab hit a CDP error.  We track consecutive failures
            // in the outer retry loop.  Only evict the tab once the threshold
            // is reached — before that, keep it alive so the backoff period
            // gives the page a chance to recover.
            //
            // consecutive_cdp_errors is the count BEFORE this failure is
            // added (it's incremented in the outer loop after we return).
            // So `>= threshold - 1` means "this failure is the Nth one".
            let will_evict = consecutive_cdp_errors + 1 >= cdp_evict_threshold;
            if will_evict {
                warn!(
                    target_id = %tid,
                    error = %e,
                    consecutive_cdp_errors = consecutive_cdp_errors + 1,
                    cdp_evict_threshold,
                    "chatgpt_web send: pooled tab hit CDP error threshold, evicting and closing dead tab"
                );
                // reused_from_pool implies conversation_id is always Some
                // (pool lookup at line 176 requires it).
                if let Some(cid) = conversation_id {
                    if let Some(evicted) =
                        super::browser::evict_conversation_tab(&config.profile_dir, cid)
                    {
                        evicted.shutdown(&browser).await;
                    } else {
                        // Already evicted by a concurrent sender — just close
                        // our reference to free the target.
                        warn!(
                            target_id = %tid,
                            "chatgpt_web send: tab already evicted by another sender, closing CDP"
                        );
                        cdp_arc.close();
                        let _ = browser.close_target(tid).await;
                    }
                }
            } else {
                info!(
                    target_id = %tid,
                    error = %e,
                    consecutive_cdp_errors = consecutive_cdp_errors + 1,
                    cdp_evict_threshold,
                    "chatgpt_web send: pooled tab CDP error, keeping tab for recovery (under threshold)"
                );
            }
        }
        (Err(_), _, _) => {
            // Remaining failure cases:
            //   - Reused tab with non-CDP error → keep it, CDP is still valid.
            //   - Error before tab creation → nothing to close.
            if reused_from_pool {
                info!("chatgpt_web send: keeping pooled tab open after non-CDP failure");
            } else {
                debug!(
                    "chatgpt_web send: no tab to clean up (error before tab creation or already handled)"
                );
            }
        }
        _ => {}
    }

    // Append screenshot path to error so upstream can send it via IM.
    match result {
        Ok(send) => Ok(send),
        Err(e) => {
            if let Some(ref path) = screenshot_path {
                Err(format!(
                    "{e}\n[diagnostic_screenshot:{path}]",
                    path = path.display()
                ))
            } else {
                Err(e)
            }
        }
    }
}

#[derive(Debug)]
pub(in crate::im_gateway::chatgpt_web) struct SendResult {
    pub(in crate::im_gateway::chatgpt_web) conversation_id: Option<String>,
    pub(in crate::im_gateway::chatgpt_web) turn_exchange_id: Option<String>,
    pub(in crate::im_gateway::chatgpt_web) event_types: Vec<String>,
    /// Complete conversation detail parsed from the SSE stream body.
    /// When present, `wait_final` can skip all API polling and use this directly.
    pub(in crate::im_gateway::chatgpt_web) sse_detail: Option<Value>,
}

/// Capture a full-page screenshot via CDP and save it to a temp file.
/// Returns the path to the saved PNG on success.
async fn capture_diagnostic_screenshot(cdp: &CdpClient) -> Result<PathBuf, String> {
    let result = cdp
        .send(
            "Page.captureScreenshot",
            json!({
                "format": "png",
                "captureBeyondViewport": true,
            }),
        )
        .await?;
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or("captureScreenshot returned no data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("base64 decode screenshot: {e}"))?;

    // Save to a temp directory with a timestamped filename.
    let dir = std::env::temp_dir().join("bifrost-diagnostic-screenshots");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create screenshot dir: {e}"))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("diag-{ts}.png"));
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| format!("write screenshot: {e}"))?;
    Ok(path)
}

/// Create a new isolated tab and connect CDP with automatic retry.
///
/// The browser may occasionally return HTTP 500 when connecting to the
/// WebSocket endpoint of a freshly-created tab. This function retries
/// with exponential backoff, closing failed tabs before each retry.
async fn create_tab_with_cdp_retry(
    browser: &BrowserSession,
    max_attempts: usize,
) -> Result<(CdpPage, CdpClient), String> {
    let mut last_error = String::new();
    for attempt in 1..=max_attempts {
        let page = match browser.create_new_page().await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    attempt,
                    max_attempts,
                    error = %e,
                    "chatgpt_web send: create_new_page failed, retrying"
                );
                last_error = e;
                sleep(Duration::from_millis(500 * attempt as u64)).await;
                continue;
            }
        };
        match CdpClient::connect_with_retry(&page.web_socket_debugger_url, 2).await {
            Ok(cdp) => return Ok((page, cdp)),
            Err(e) => {
                warn!(
                    attempt,
                    max_attempts,
                    page_id = %page.id,
                    error = %e,
                    "chatgpt_web send: CDP connect failed, closing tab and retrying"
                );
                // Close the orphaned tab before retrying.
                let _ = browser.close_target(&page.id).await;
                last_error = e;
                sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    Err(format!(
        "failed to create tab + CDP after {max_attempts} attempts: {last_error}"
    ))
}

/// Wait until a default execution context is available in the CDP target.
/// Newly created tabs may not have a context yet if the page hasn't loaded.
/// We retry a simple JS evaluation until it succeeds or we time out.
async fn wait_for_execution_context(cdp: &CdpClient, max_wait: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + max_wait;
    loop {
        match cdp
            .send("Runtime.evaluate", json!({"expression": "1+1"}))
            .await
        {
            Ok(_) => {
                debug!("chatgpt_web send: execution context ready");
                return Ok(());
            }
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "timed out waiting for execution context in new tab: {e}"
                    ));
                }
                debug!(
                    error = %e,
                    "chatgpt_web send: execution context not yet ready, retrying…"
                );
                sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

async fn wait_send_handoff(
    cdp: &CdpClient,
    browser: &mut BrowserSession,
    events: &mut broadcast::Receiver<CdpEvent>,
    _after_ms: u64,
    duration: Duration,
    stop_marker_path: &Path,
    request_ids: &mut BTreeSet<String>,
) -> Result<SendResult, String> {
    info!(
        ?duration,
        existing_tracked = request_ids.len(),
        "chatgpt_web send: waiting for f/conversation handoff"
    );
    let deadline = tokio::time::Instant::now() + duration;
    let mut conversation_id: Option<String> = None;
    let mut turn_exchange_id: Option<String> = None;
    let mut event_types: Vec<String> = Vec::new();
    let mut next_page_probe =
        tokio::time::Instant::now() + Duration::from_secs(HANDOFF_HEARTBEAT_INTERVAL_SECS);
    while tokio::time::Instant::now() < deadline {
        if stop_requested(stop_marker_path).await {
            return Err(stopped_error());
        }
        let browser_exited = browser.has_exited()?;
        if let Some(error) = handoff_heartbeat_error(browser_exited, cdp.is_closed()) {
            warn!(%error, "chatgpt_web handoff heartbeat failed");
            return Err(error);
        }
        if tokio::time::Instant::now() >= next_page_probe {
            if let Err(error) = probe_handoff_page(cdp).await {
                warn!(%error, "chatgpt_web handoff page heartbeat failed");
                return Err(error);
            }
            next_page_probe =
                tokio::time::Instant::now() + Duration::from_secs(HANDOFF_HEARTBEAT_INTERVAL_SECS);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining.min(Duration::from_millis(1000)), events.recv()).await {
            Ok(Ok(event)) if event.method == "Network.requestWillBeSent" => {
                let req_url = event
                    .params
                    .get("request")
                    .and_then(|r| r.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let req_method = event
                    .params
                    .get("request")
                    .and_then(|r| r.get("method"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if req_url.contains("/backend-api/") {
                    debug!(
                        url = req_url,
                        method = req_method,
                        "chatgpt_web handoff: observed backend-api request"
                    );
                }
                if req_method == "POST" && req_url == F_CONVERSATION_URL {
                    if let Some(request_id) = event.params.get("requestId").and_then(Value::as_str)
                    {
                        info!(
                            request_id,
                            "chatgpt_web handoff: captured f/conversation POST"
                        );
                        request_ids.insert(request_id.to_string());
                    }
                }
            }
            Ok(Ok(event)) if event.method == "Network.dataReceived" => {
                // Check if this is data from our tracked request — extract conversation_id
                // from the streaming SSE chunks without waiting for the full response.
                let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
                    continue;
                };
                if !request_ids.contains(request_id) {
                    continue;
                }
                // We got data for our tracked request. Try to get the response body so far.
                // On the first data chunk we can often extract conversation_id immediately.
                if conversation_id.is_none() {
                    // Try to read partial response body
                    if let Ok(body) = cdp
                        .send("Network.getResponseBody", json!({"requestId": request_id}))
                        .await
                    {
                        let text = if body
                            .get("base64Encoded")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            let body_text =
                                body.get("body").and_then(Value::as_str).unwrap_or_default();
                            base64::engine::general_purpose::STANDARD
                                .decode(body_text)
                                .ok()
                                .and_then(|bytes| String::from_utf8(bytes).ok())
                                .unwrap_or_default()
                        } else {
                            body.get("body")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        let partial = parse_send_sse(&text);
                        if partial.conversation_id.is_some() {
                            conversation_id = partial.conversation_id;
                            turn_exchange_id = partial.turn_exchange_id;
                            event_types = partial.event_types;
                            info!(
                                conversation_id = ?conversation_id,
                                "chatgpt_web handoff: got conversation_id from SSE stream, waiting for stream to finish"
                            );
                            // Continue monitoring the SSE stream until loadingFinished
                            // using the OVERALL deadline (not a short 3s window).
                            // This avoids the need for separate API polling in wait_final.
                            loop {
                                let remain =
                                    deadline.saturating_duration_since(tokio::time::Instant::now());
                                if remain.is_zero() {
                                    info!("chatgpt_web handoff: deadline reached while waiting for SSE completion");
                                    break;
                                }
                                match timeout(
                                    remain.min(Duration::from_millis(1000)),
                                    events.recv(),
                                )
                                .await
                                {
                                    Ok(Ok(ev)) if ev.method == "Network.loadingFinished" => {
                                        let fin_req = ev
                                            .params
                                            .get("requestId")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if request_ids.contains(fin_req) {
                                            info!("chatgpt_web handoff: SSE stream finished (loadingFinished)");
                                            if let Ok(fin_body) = cdp
                                                .send(
                                                    "Network.getResponseBody",
                                                    json!({"requestId": fin_req}),
                                                )
                                                .await
                                            {
                                                let fin_text = if fin_body
                                                    .get("base64Encoded")
                                                    .and_then(Value::as_bool)
                                                    .unwrap_or(false)
                                                {
                                                    let b = fin_body
                                                        .get("body")
                                                        .and_then(Value::as_str)
                                                        .unwrap_or_default();
                                                    base64::engine::general_purpose::STANDARD
                                                        .decode(b)
                                                        .ok()
                                                        .and_then(|bytes| {
                                                            String::from_utf8(bytes).ok()
                                                        })
                                                        .unwrap_or_default()
                                                } else {
                                                    fin_body
                                                        .get("body")
                                                        .and_then(Value::as_str)
                                                        .unwrap_or_default()
                                                        .to_string()
                                                };
                                                info!(
                                                    body_len = fin_text.len(),
                                                    "chatgpt_web handoff: parsing completed SSE body"
                                                );
                                                let mut completed = parse_send_sse(&fin_text);
                                                if completed.conversation_id.is_none() {
                                                    completed.conversation_id =
                                                        conversation_id.clone();
                                                }
                                                if completed.turn_exchange_id.is_none() {
                                                    completed.turn_exchange_id =
                                                        turn_exchange_id.clone();
                                                }
                                                if completed.event_types.is_empty() {
                                                    completed.event_types = event_types.clone();
                                                }
                                                // If SSE contained full message content, return immediately.
                                                if completed.sse_detail.is_some() {
                                                    return Ok(completed);
                                                }
                                                // stream_handoff: SSE had no message content.
                                                // The page will fetch conversation detail via GET.
                                                // Continue monitoring to intercept that request.
                                                let conv_id_for_get = completed
                                                    .conversation_id
                                                    .clone()
                                                    .unwrap_or_default();
                                                if conv_id_for_get.is_empty() {
                                                    warn!("chatgpt_web handoff: stream_handoff but no conversation_id, cannot intercept GET");
                                                    return Ok(completed);
                                                }
                                                let detail_url_pattern = format!(
                                                    "/backend-api/conversation/{}",
                                                    conv_id_for_get
                                                );
                                                info!(
                                                    conversation_id = %conv_id_for_get,
                                                    "chatgpt_web handoff: stream_handoff detected, waiting for page GET conversation detail"
                                                );
                                                let mut detail_request_id: Option<String> = None;
                                                // Brief wait: page rarely fetches full detail
                                                // via GET — it uses another SSE stream instead.
                                                // 5s is enough to detect if GET happens.
                                                let get_deadline = tokio::time::Instant::now()
                                                    + Duration::from_secs(5);
                                                loop {
                                                    let remain = get_deadline
                                                        .saturating_duration_since(
                                                            tokio::time::Instant::now(),
                                                        );
                                                    if remain.is_zero() {
                                                        info!("chatgpt_web handoff: deadline reached waiting for GET conversation detail");
                                                        break;
                                                    }
                                                    match timeout(
                                                        remain.min(Duration::from_millis(1000)),
                                                        events.recv(),
                                                    )
                                                    .await
                                                    {
                                                        Ok(Ok(ev))
                                                            if ev.method
                                                                == "Network.requestWillBeSent" =>
                                                        {
                                                            let req_method = ev
                                                                .params
                                                                .get("request")
                                                                .and_then(|r| r.get("method"))
                                                                .and_then(Value::as_str)
                                                                .unwrap_or_default();
                                                            let req_url = ev
                                                                .params
                                                                .get("request")
                                                                .and_then(|r| r.get("url"))
                                                                .and_then(Value::as_str)
                                                                .unwrap_or_default();
                                                            if req_method == "GET"
                                                                && req_url
                                                                    .contains(&detail_url_pattern)
                                                                && !req_url.contains(&format!(
                                                                    "{}/",
                                                                    detail_url_pattern
                                                                ))
                                                            {
                                                                if let Some(rid) = ev
                                                                    .params
                                                                    .get("requestId")
                                                                    .and_then(Value::as_str)
                                                                {
                                                                    info!(
                                                                        request_id = rid,
                                                                        url = req_url,
                                                                        "chatgpt_web handoff: captured page GET conversation detail"
                                                                    );
                                                                    detail_request_id =
                                                                        Some(rid.to_string());
                                                                }
                                                            }
                                                        }
                                                        Ok(Ok(ev))
                                                            if ev.method
                                                                == "Network.loadingFinished" =>
                                                        {
                                                            if let Some(ref tracked) =
                                                                detail_request_id
                                                            {
                                                                let fin_req = ev
                                                                    .params
                                                                    .get("requestId")
                                                                    .and_then(Value::as_str)
                                                                    .unwrap_or_default();
                                                                if fin_req == tracked.as_str() {
                                                                    info!("chatgpt_web handoff: page GET conversation detail finished");
                                                                    if let Ok(body) = cdp
                                                                        .send(
                                                                            "Network.getResponseBody",
                                                                            json!({"requestId": fin_req}),
                                                                        )
                                                                        .await
                                                                    {
                                                                        let text = if body
                                                                            .get("base64Encoded")
                                                                            .and_then(
                                                                                Value::as_bool,
                                                                            )
                                                                            .unwrap_or(false)
                                                                        {
                                                                            base64::engine::general_purpose::STANDARD
                                                                                .decode(
                                                                                    body.get("body")
                                                                                        .and_then(Value::as_str)
                                                                                        .unwrap_or_default(),
                                                                                )
                                                                                .ok()
                                                                                .and_then(|b| {
                                                                                    String::from_utf8(b).ok()
                                                                                })
                                                                                .unwrap_or_default()
                                                                        } else {
                                                                            body.get("body")
                                                                                .and_then(
                                                                                    Value::as_str,
                                                                                )
                                                                                .unwrap_or_default()
                                                                                .to_string()
                                                                        };
                                                                        info!(
                                                                            body_len = text.len(),
                                                                            "chatgpt_web handoff: parsing page conversation detail"
                                                                        );
                                                                        if let Ok(detail_json) =
                                                                            serde_json::from_str::<
                                                                                Value,
                                                                            >(
                                                                                &text
                                                                            )
                                                                        {
                                                                            completed.sse_detail =
                                                                                Some(detail_json);
                                                                            return Ok(completed);
                                                                        } else {
                                                                            warn!("chatgpt_web handoff: failed to parse conversation detail JSON");
                                                                        }
                                                                    }
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                        Ok(Ok(ev))
                                                            if ev.method
                                                                == "Network.loadingFailed" =>
                                                        {
                                                            if let Some(ref tracked) =
                                                                detail_request_id
                                                            {
                                                                let fail_req = ev
                                                                    .params
                                                                    .get("requestId")
                                                                    .and_then(Value::as_str)
                                                                    .unwrap_or_default();
                                                                if fail_req == tracked.as_str() {
                                                                    warn!("chatgpt_web handoff: page GET conversation detail failed (loadingFailed)");
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                        Err(_) => {
                                                            let browser_exited = browser
                                                                .has_exited()
                                                                .unwrap_or(false);
                                                            if let Some(error) =
                                                                handoff_heartbeat_error(
                                                                    browser_exited,
                                                                    cdp.is_closed(),
                                                                )
                                                            {
                                                                warn!(%error, "chatgpt_web handoff: heartbeat failed during GET wait");
                                                                return Err(error);
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                                return Ok(completed);
                                            }
                                        }
                                        break;
                                    }
                                    Ok(Ok(ev)) if ev.method == "Network.loadingFailed" => {
                                        let fail_req = ev
                                            .params
                                            .get("requestId")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if request_ids.contains(fail_req) {
                                            warn!("chatgpt_web handoff: SSE stream failed (loadingFailed)");
                                            break;
                                        }
                                    }
                                    Ok(Ok(ev)) if ev.method == "Network.requestWillBeSent" => {
                                        // Track additional requests (e.g. multi-turn follow-ups)
                                        let req_method = ev
                                            .params
                                            .get("request")
                                            .and_then(|r| r.get("method"))
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        let req_url = ev
                                            .params
                                            .get("request")
                                            .and_then(|r| r.get("url"))
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if req_method == "POST" && req_url == F_CONVERSATION_URL {
                                            if let Some(request_id) =
                                                ev.params.get("requestId").and_then(Value::as_str)
                                            {
                                                info!(request_id, "chatgpt_web handoff: additional f/conversation POST during SSE wait");
                                                request_ids.insert(request_id.to_string());
                                            }
                                        }
                                    }
                                    Ok(Ok(_)) | Ok(Err(_)) => {}
                                    Err(_) => {
                                        // timeout tick — check heartbeat
                                        let browser_exited = browser.has_exited().unwrap_or(false);
                                        if let Some(error) =
                                            handoff_heartbeat_error(browser_exited, cdp.is_closed())
                                        {
                                            warn!(%error, "chatgpt_web handoff: heartbeat failed during SSE wait");
                                            return Err(error);
                                        }
                                    }
                                }
                            }
                            // If we broke out without loadingFinished completing,
                            // return partial result — wait_final will use DOM fallback.
                            return Ok(SendResult {
                                conversation_id,
                                turn_exchange_id,
                                event_types,
                                sse_detail: None,
                            });
                        }
                    }
                }
            }
            Ok(Ok(event)) if event.method == "Network.loadingFinished" => {
                let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
                    continue;
                };
                if !request_ids.contains(request_id) {
                    continue;
                }
                info!(
                    request_id,
                    "chatgpt_web handoff: f/conversation response finished, reading body"
                );
                let body = cdp
                    .send("Network.getResponseBody", json!({"requestId": request_id}))
                    .await?;
                let text = if body
                    .get("base64Encoded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    let body_text = body.get("body").and_then(Value::as_str).unwrap_or_default();
                    base64::engine::general_purpose::STANDARD
                        .decode(body_text)
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .unwrap_or_default()
                } else {
                    body.get("body")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                info!(
                    body_len = text.len(),
                    "chatgpt_web handoff: parsing SSE body"
                );
                return Ok(parse_send_sse(&text));
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
        }
    }
    // If we captured conversation_id from partial data but timed out waiting for completion,
    // return what we have — wait_final will poll for the final response.
    if conversation_id.is_some() {
        info!(
            ?conversation_id,
            "chatgpt_web handoff: timeout but have conversation_id, returning partial"
        );
        return Ok(SendResult {
            conversation_id,
            turn_exchange_id,
            event_types,
            sse_detail: None,
        });
    }
    let diagnostic = inspect_page_detailed(cdp)
        .await
        .unwrap_or_else(|_| json!({}));
    let diag_summary = format_send_diagnostic(&diagnostic);
    warn!(
        tracked_requests = request_ids.len(),
        diagnosis = %diag_summary,
        "chatgpt_web handoff: timed out waiting for f/conversation"
    );
    if request_ids.is_empty() {
        return Err(format!("browser_send_not_submitted: {diag_summary}"));
    }
    Err(format!(
        "sse_interrupted: handoff did not complete. {diag_summary}"
    ))
}

async fn probe_handoff_page(cdp: &CdpClient) -> Result<(), String> {
    let result = cdp
        .send_with_timeout(
            "Runtime.evaluate",
            json!({
                "expression": "({ href: location.href, readyState: document.readyState })",
                "returnByValue": true
            }),
            Duration::from_secs(HANDOFF_PAGE_PROBE_TIMEOUT_SECS),
        )
        .await
        .map_err(|error| handoff_page_heartbeat_error(&error))?;
    debug!(?result, "chatgpt_web handoff page heartbeat ok");
    Ok(())
}

pub(in crate::im_gateway::chatgpt_web) fn handoff_heartbeat_error(
    browser_exited: bool,
    cdp_closed: bool,
) -> Option<String> {
    if browser_exited {
        return Some(
            "browser_unavailable: ChatGPT Web browser exited while waiting for handoff".to_string(),
        );
    }
    if cdp_closed {
        return Some(
            "browser_unavailable: ChatGPT Web CDP connection closed while waiting for handoff"
                .to_string(),
        );
    }
    None
}

pub(in crate::im_gateway::chatgpt_web) fn handoff_page_heartbeat_error(error: &str) -> String {
    format!("browser_unavailable: ChatGPT Web page heartbeat failed during handoff: {error}")
}

/// Check the current conversation ID on the page.
///
/// This goes beyond a simple URL check: it also verifies that the page
/// actually has conversation content rendered (message turn elements).
/// ChatGPT's SPA may keep the `/c/xxx` URL but reset the React state
/// after 429 rate limiting, showing the default "new conversation" UI.
/// In that case we return `Ok(None)` so the caller falls back to navigation.
async fn current_conversation_id(cdp: &CdpClient) -> Result<Option<String>, String> {
    let value = evaluate_value(
        cdp,
        r#"(() => {
          const match = location.href.match(/\/c\/([^/?#]+)/);
          const urlCid = match ? decodeURIComponent(match[1]) : null;
          // Verify the page actually shows conversation content, not just the URL.
          const turns = document.querySelectorAll(
            '[data-message-id], [data-testid^="conversation-turn-"]'
          );
          const turnCount = turns.length;
          // Return a JSON object so we get full diagnostics in logs.
          return JSON.stringify({ urlCid, turnCount, href: location.href });
        })()"#,
    )
    .await?;
    // Parse the diagnostic JSON
    let raw = value.as_str().unwrap_or("");
    let diag: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
    let url_cid = diag
        .get("urlCid")
        .and_then(|v| v.as_str())
        .map(String::from);
    let turn_count = diag.get("turnCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let href = diag.get("href").and_then(|v| v.as_str()).unwrap_or("");

    if url_cid.is_some() && turn_count == 0 {
        tracing::warn!(
            url_cid = ?url_cid,
            turn_count,
            href,
            "chatgpt_web: page URL has conversation ID but DOM has NO turn elements — treating as stale/reset page"
        );
        return Ok(None);
    }
    tracing::debug!(
        url_cid = ?url_cid,
        turn_count,
        href,
        "chatgpt_web: current_conversation_id check"
    );
    Ok(url_cid)
}

/// Detect and dismiss ChatGPT's rate-limit / error modal dialogs.
///
/// The ChatGPT rate-limit modal is a Radix popover:
///   <div role="dialog" data-state="open" class="popover ...">
///     <h2>请求过于频繁</h2>
///     <button class="btn btn-primary"><div>明白了</div></button>
///   </div>
///
/// Detection is intentionally **loose** — we'd rather click an extra button
/// than miss a blocking modal.  The dismiss-text labels ("明白了", "OK", etc.)
/// never appear as normal page buttons, so false positives are safe.
///
/// Returns `true` if a modal was detected and dismissed, `false` otherwise.
pub(super) async fn dismiss_rate_limit_modal(cdp: &CdpClient) -> bool {
    let result = evaluate_value(
        cdp,
        r#"(() => {
          const dismissTexts = ['明白了', '我明白了', 'OK', 'Got it', 'Dismiss',
                                'okay', 'Okay', 'I understand', '知道了',
                                '确定', '关闭', 'Close'];

          // Loose visibility check — covers fixed/absolute/popover positioning
          const isVisible = (el) => {
            if (!el) return false;
            const s = getComputedStyle(el);
            if (s.display === 'none' || s.visibility === 'hidden') return false;
            const r = el.getBoundingClientRect();
            return r.width > 0 && r.height > 0;
          };

          // Strategy 1 (fastest): any visible [role="dialog"] — click its
          // first visible button that looks like a dismiss label.
          // No keyword check on dialog text — if there's an open dialog with
          // a dismiss button, we always want to close it.
          for (const dlg of document.querySelectorAll('[role="dialog"]')) {
            if (!isVisible(dlg)) continue;
            for (const btn of dlg.querySelectorAll('button')) {
              if (!isVisible(btn)) continue;
              const t = (btn.innerText || btn.textContent || '').trim();
              if (dismissTexts.some(d => t.includes(d)) || dlg.querySelectorAll('button').length === 1) {
                btn.click();
                return JSON.stringify({
                  dismissed: true,
                  buttonText: t,
                  selector: 'dialog-button',
                  context: (dlg.innerText || '').slice(0, 200)
                });
              }
            }
          }

          // Strategy 2 (broadest): scan ALL visible buttons on the entire page.
          // If a button's text matches any dismiss label, just click it.
          // These labels never appear in ChatGPT's normal UI, so this is safe.
          for (const btn of document.querySelectorAll('button, [role="button"]')) {
            if (!isVisible(btn)) continue;
            const t = (btn.innerText || btn.textContent || '').trim();
            if (!t) continue;
            if (dismissTexts.some(d => t === d || t === d + '。')) {
              btn.click();
              // Gather some context from nearest ancestor for logging
              let ctx = '';
              let el = btn.parentElement;
              for (let i = 0; el && i < 5; i++, el = el.parentElement) {
                const pt = (el.innerText || '').trim();
                if (pt.length > t.length + 10) { ctx = pt.slice(0, 200); break; }
              }
              return JSON.stringify({
                dismissed: true,
                buttonText: t,
                selector: 'any-dismiss-button',
                context: ctx
              });
            }
          }

          return JSON.stringify({ dismissed: false });
        })()"#,
    )
    .await;

    match result {
        Ok(value) => {
            let raw = value.as_str().unwrap_or("{}");
            let diag: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
            let dismissed = diag
                .get("dismissed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if dismissed {
                let modal_text = diag.get("modalText").and_then(|v| v.as_str()).unwrap_or("");
                let button_text = diag
                    .get("buttonText")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let selector = diag.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                tracing::warn!(
                    modal_text,
                    button_text,
                    selector,
                    "chatgpt_web: dismissed rate-limit modal dialog"
                );
                true
            } else {
                false
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "chatgpt_web: dismiss_rate_limit_modal CDP error (page may be navigating)");
            false
        }
    }
}

/// Attempt to dismiss any blocking modal, wait briefly, then retry.
/// Used at key points in the send flow where a modal could block progress.
pub(super) async fn dismiss_modal_and_wait(cdp: &CdpClient) {
    if dismiss_rate_limit_modal(cdp).await {
        // Brief pause for DOM to settle after dismissing the modal.
        // 1s is sufficient — React re-render completes within a frame or two.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        // Try once more in case there are stacked modals
        if dismiss_rate_limit_modal(cdp).await {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

/// Navigate with one retry on CDP transient failure.
/// ChatGPT's SPA may transiently block CDP commands (heavy JS, internal
/// navigation, React hydration). If the first attempt fails with a CDP
/// transient error we back off briefly and retry once.
async fn navigate_with_cdp_retry(cdp: &CdpClient, url: &str) -> Result<Value, String> {
    match cdp.navigate_and_wait(url, Duration::from_secs(20)).await {
        Ok(v) => Ok(v),
        Err(e) if is_cdp_transient_error(&e) => {
            warn!(
                error = %e,
                %url,
                "chatgpt_web send: navigate CDP transient error, retrying after 3s backoff"
            );
            sleep(Duration::from_secs(3)).await;
            cdp.navigate_and_wait(url, Duration::from_secs(20)).await
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
pub(in crate::im_gateway::chatgpt_web) fn extract_conversation_id_from_url(
    url: &str,
) -> Option<String> {
    let marker = "/c/";
    let start = url.find(marker)? + marker.len();
    let tail = &url[start..];
    let id = tail
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default()
        .trim();
    (!id.is_empty()).then(|| id.to_string())
}

pub(in crate::im_gateway::chatgpt_web) fn parse_send_sse(text: &str) -> SendResult {
    let mut conversation_id = None;
    let mut turn_exchange_id = None;
    let mut event_types = Vec::new();
    // Collect all message objects from SSE events to build a synthetic
    // conversation detail — this allows wait_final to skip API polling entirely.
    let mut messages: serde_json::Map<String, Value> = serde_json::Map::new();

    for block in text.split("\n\n") {
        for line in block.lines().filter(|line| line.starts_with("data: ")) {
            let value = line.trim_start_matches("data: ");
            if let Ok(json) = serde_json::from_str::<Value>(value) {
                let event_type = json
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("patch")
                    .to_string();
                if conversation_id.is_none() {
                    conversation_id = json
                        .get("conversation_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
                if turn_exchange_id.is_none() {
                    turn_exchange_id = json
                        .get("turn_exchange_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }

                // Extract message objects from SSE events.
                // ChatGPT SSE sends events with a "message" field containing
                // the full message state (updated incrementally).  The last
                // occurrence for each message_id has the final content.
                if let Some(message) = json.get("message") {
                    if let Some(msg_id) = message.get("id").and_then(Value::as_str) {
                        messages.insert(msg_id.to_string(), json!({ "message": message }));
                    }
                }

                event_types.push(event_type);
            }
        }
    }

    // Build synthetic conversation detail if we collected any messages.
    let sse_detail = if messages.is_empty() {
        None
    } else {
        Some(json!({ "mapping": messages }))
    };

    SendResult {
        conversation_id,
        turn_exchange_id,
        event_types,
        sse_detail,
    }
}

async fn wait_composer(
    cdp: &CdpClient,
    expected_conversation_id: Option<&str>,
    duration: Duration,
) -> Result<(), String> {
    let start = tokio::time::Instant::now();
    // Phase 1: Event-driven wait for the composer element to appear in DOM.
    // Uses MutationObserver internally — much faster than polling every 500ms.
    let composer_selector =
        "#prompt-textarea, [data-testid=\"composer-text-input\"], [contenteditable=\"true\"]";
    let selector_timeout = Duration::from_secs(30).min(duration);
    match cdp
        .wait_for_selector(composer_selector, selector_timeout)
        .await
    {
        Ok(_) => {
            debug!(
                elapsed_ms = start.elapsed().as_millis() as u64,
                "chatgpt_web wait_composer: composer element found via MutationObserver"
            );
        }
        Err(e) => {
            debug!(
                error = %e,
                "chatgpt_web wait_composer: wait_for_selector failed, falling back to polling"
            );
        }
    }

    // Phase 2: Verify page state (conversationId alignment + composer ready).
    // The DOM element may appear before React finishes hydrating the correct
    // conversation state, so we still need page-level verification.
    let deadline = start + duration;
    let mut last = json!({});
    while tokio::time::Instant::now() < deadline {
        let diagnostic = inspect_page(cdp).await?;
        last = diagnostic.clone();
        if page_state_is_auth_flow_page(&diagnostic) {
            return Err(auth_required_page_error(&diagnostic));
        }
        if target_page_matches(&diagnostic, expected_conversation_id, true) {
            // Brief settling wait — allows React to finish any micro-task
            // after DOM is ready. 100ms is sufficient; if hydration is still
            // in progress, set_composer_text will fail and retry with fallback.
            sleep(Duration::from_millis(100)).await;
            return Ok(());
        }
        if target_page_is_terminal_mismatch(&diagnostic, expected_conversation_id) {
            return Err(target_page_error(&diagnostic, expected_conversation_id));
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(format!("browser_ui: composer not ready: {last}"))
}

/// Enhanced page inspection that includes send button state and composer content.
/// Used for diagnostic error messages when send fails.
async fn inspect_page_detailed(cdp: &CdpClient) -> Result<Value, String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);

          // Composer / input state
          const composerEl = document.querySelector('#prompt-textarea') ||
                             document.querySelector('[data-testid="composer-text-input"]') ||
                             document.querySelector('[contenteditable="true"]') ||
                             document.querySelector('textarea');
          const composerText = composerEl ? (composerEl.innerText || composerEl.value || '').trim() : null;
          const composerVisible = composerEl ? isVisible(composerEl) : false;
          const composerDisabled = composerEl ? (composerEl.disabled || composerEl.readOnly || composerEl.getAttribute('aria-disabled') === 'true') : null;

          // Send button state
          const sendBtn = document.querySelector('[data-testid="send-button"]') ||
                          document.querySelector('button[aria-label="Send prompt"]') ||
                          document.querySelector('button[aria-label="发送提示"]');
          const sendBtnVisible = sendBtn ? isVisible(sendBtn) : false;
          const sendBtnDisabled = sendBtn ? sendBtn.disabled : null;
          const sendBtnText = sendBtn ? (sendBtn.innerText || sendBtn.getAttribute('aria-label') || '').trim().slice(0, 50) : null;

          // Stop button (indicates a response is in progress)
          const stopBtn = document.querySelector('[data-testid="stop-button"]') ||
                          document.querySelector('button[aria-label="Stop streaming"]') ||
                          document.querySelector('button[aria-label="停止流式传输"]');
          const stopBtnVisible = stopBtn ? isVisible(stopBtn) : false;

          // Page state
          const busyEls = Array.from(document.querySelectorAll(
            '[aria-busy="true"], [data-loading="true"], [data-testid*="loading"], .animate-pulse'
          )).filter(isVisible);

          // Rate-limit / error banners
          const banners = Array.from(document.querySelectorAll(
            '[role="alert"], .text-red-500, [data-testid*="error"], [data-testid*="limit"]'
          )).filter(isVisible).map(el => (el.innerText || '').trim().slice(0, 200));

          return {
            url: location.href,
            title: document.title,
            readyState: document.readyState,
            composerFound: !!composerEl,
            composerVisible,
            composerDisabled,
            composerHasText: composerText ? composerText.length > 0 : false,
            composerTextPreview: composerText ? composerText.slice(0, 100) : null,
            sendButtonFound: !!sendBtn,
            sendButtonVisible: sendBtnVisible,
            sendButtonDisabled: sendBtnDisabled,
            sendButtonText: sendBtnText,
            stopButtonVisible: stopBtnVisible,
            busyCount: busyEls.length,
            errorBanners: banners.slice(0, 3),
            webdriver: navigator.webdriver
          };
        })()"#,
    )
    .await
}

/// Format diagnostic JSON into a human-readable summary for error messages.
fn format_send_diagnostic(diag: &Value) -> String {
    let mut parts = Vec::new();

    // Page URL
    if let Some(url) = diag.get("url").and_then(Value::as_str) {
        parts.push(format!("page={url}"));
    }

    // Composer state
    let composer_found = diag
        .get("composerFound")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let composer_visible = diag
        .get("composerVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let composer_has_text = diag
        .get("composerHasText")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let composer_disabled = diag
        .get("composerDisabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !composer_found {
        parts.push("composer=NOT_FOUND".to_string());
    } else if !composer_visible {
        parts.push("composer=HIDDEN".to_string());
    } else if composer_disabled {
        parts.push("composer=DISABLED".to_string());
    } else if !composer_has_text {
        parts.push("composer=EMPTY(text not injected)".to_string());
    } else {
        let preview = diag
            .get("composerTextPreview")
            .and_then(Value::as_str)
            .unwrap_or("?");
        parts.push(format!("composer=OK(text=\"{preview}\")"));
    }

    // Send button state
    let btn_found = diag
        .get("sendButtonFound")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let btn_visible = diag
        .get("sendButtonVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let btn_disabled = diag
        .get("sendButtonDisabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !btn_found {
        parts.push("sendButton=NOT_FOUND".to_string());
    } else if !btn_visible {
        parts.push("sendButton=HIDDEN".to_string());
    } else if btn_disabled {
        parts.push("sendButton=DISABLED".to_string());
    } else {
        parts.push("sendButton=OK".to_string());
    }

    // Stop button (response in progress)
    if diag
        .get("stopButtonVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        parts.push("stopButton=VISIBLE(response in progress?)".to_string());
    }

    // Busy indicators
    let busy = diag.get("busyCount").and_then(Value::as_u64).unwrap_or(0);
    if busy > 0 {
        parts.push(format!("busyElements={busy}"));
    }

    // Error banners
    if let Some(banners) = diag.get("errorBanners").and_then(Value::as_array) {
        for banner in banners {
            if let Some(text) = banner.as_str() {
                if !text.is_empty() {
                    parts.push(format!("banner=\"{text}\""));
                }
            }
        }
    }

    parts.join(", ")
}

async fn inspect_page(cdp: &CdpClient) -> Result<Value, String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const bodyText = (document.body?.innerText || "").replace(/\s+/g, " ").slice(0, 500);
          const match = location.href.match(/\/c\/([^/?#]+)/);
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);
          const visibleComposers = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          )).filter((el) => isVisible(el) && !el.disabled && !el.readOnly);
          const busyCount = Array.from(document.querySelectorAll(
            '[aria-busy="true"], [data-loading="true"], [data-testid*="loading"], .animate-pulse'
          )).filter(isVisible).length;
          // Detect if the page actually has conversation content rendered.
          // ChatGPT SPA may keep the /c/xxx URL but reset the page state
          // after 429 rate limiting, showing the default "new conversation"
          // UI on a conversation URL.  Check for message turn elements.
          const turnElements = document.querySelectorAll(
            '[data-message-id], [data-testid^="conversation-turn-"]'
          );
          const hasConversationContent = turnElements.length > 0;
          // Only trust the URL-based conversationId if the page actually
          // shows conversation content.  Otherwise treat as new_conversation.
          const effectiveCid = (match && hasConversationContent) ? decodeURIComponent(match[1]) : null;
          return {
            url: location.href,
            title: document.title,
            readyState: document.readyState,
            bodyText,
            conversationId: effectiveCid,
            urlConversationId: match ? decodeURIComponent(match[1]) : null,
            pageKind: effectiveCid ? "conversation" : "new_conversation",
            hasConversationContent,
            composerCount: document.querySelectorAll('#prompt-textarea, textarea, [contenteditable="true"]').length,
            visibleComposerCount: visibleComposers.length,
            busyCount,
            webdriver: navigator.webdriver
          };
        })()"#,
    )
    .await
}

async fn wait_target_page(
    cdp: &CdpClient,
    expected_conversation_id: Option<&str>,
    duration: Duration,
    require_composer: bool,
) -> Result<(), String> {
    let start = tokio::time::Instant::now();
    let deadline = start + duration;
    // Grace period: after Page.navigate the ChatGPT SPA may briefly show the
    // new_conversation page before routing to the correct conversation URL.
    // Don't declare a terminal mismatch until both:
    //   (a) at least TERMINAL_GRACE_SECS have passed since start, AND
    //   (b) readyState has been "complete" for at least READY_SETTLE_SECS.
    const TERMINAL_GRACE_SECS: u64 = 10;
    const READY_SETTLE_SECS: u64 = 3;

    // Phase 1: Wait for navigation to complete via CDP events (event-driven,
    // much faster than polling readyState).  Use a SHORT opportunistic timeout
    // because if the caller already used navigate_and_wait(), the load event
    // has already fired and the subscription here will miss it.  A short wait
    // catches the case where wait_target_page is called immediately after
    // Page.navigate (before the load event fires), while not adding 15s
    // overhead when the event was already consumed.
    let nav_timeout = Duration::from_secs(3).min(duration);
    match cdp.wait_for_navigation(nav_timeout).await {
        Ok(()) => {
            debug!("chatgpt_web wait_target_page: navigation event received");
        }
        Err(e) => {
            // Navigation event didn't arrive — this can happen for SPA pushState
            // navigations. Fall through to polling phase.
            debug!(
                error = %e,
                "chatgpt_web wait_target_page: no navigation event, falling back to polling"
            );
        }
    }

    // Phase 2: Verify the page state matches expectations. Use polling for SPA
    // state verification since the URL/conversationId may change after the load event.
    let mut last = json!({});
    let mut stable_signature: Option<String> = None;
    let mut stable_since: Option<tokio::time::Instant> = None;
    let mut ready_complete_since: Option<tokio::time::Instant> = None;
    let mut log_count: u32 = 0;

    while tokio::time::Instant::now() < deadline {
        let state = inspect_page(cdp).await?;
        last = state.clone();
        if page_state_is_auth_flow_page(&state) {
            return Err(auth_required_page_error(&state));
        }

        let ready = state
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or_default()
            == "complete";
        let actual_cid = state.get("conversationId").and_then(Value::as_str);
        let page_kind = state
            .get("pageKind")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let page_url = state.get("url").and_then(Value::as_str).unwrap_or_default();

        // Track when readyState first becomes "complete".
        if ready && ready_complete_since.is_none() {
            ready_complete_since = Some(tokio::time::Instant::now());
            info!(
                actual_cid = ?actual_cid,
                %page_kind,
                %page_url,
                expected_conversation_id,
                require_composer,
                "chatgpt_web wait_target_page: readyState=complete, starting settle window"
            );
        }

        // Log every ~2 seconds for observability.
        log_count += 1;
        if log_count % 8 == 1 {
            let elapsed_ms = start.elapsed().as_millis();
            let ready_elapsed_ms = ready_complete_since
                .map(|since| since.elapsed().as_millis())
                .unwrap_or(0);
            let visible_composers = state
                .get("visibleComposerCount")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let busy = state.get("busyCount").and_then(Value::as_u64).unwrap_or(0);
            debug!(
                elapsed_ms,
                ready_elapsed_ms,
                %ready,
                actual_cid = ?actual_cid,
                %page_kind,
                %page_url,
                expected_conversation_id,
                require_composer,
                visible_composers,
                busy,
                "chatgpt_web wait_target_page: poll"
            );
        }

        if target_page_matches(&state, expected_conversation_id, require_composer) {
            let signature = target_page_signature(&state);
            if stable_signature.as_deref() == Some(signature.as_str()) {
                if stable_since.is_some_and(|since| {
                    since.elapsed() >= Duration::from_millis(TARGET_PAGE_STABLE_MS)
                }) {
                    info!(
                        expected_conversation_id,
                        require_composer,
                        page_state = %state,
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        "chatgpt_web wait_target_page: target page stable ✓"
                    );
                    return Ok(());
                }
            } else {
                stable_signature = Some(signature);
                stable_since = Some(tokio::time::Instant::now());
            }
        } else {
            stable_signature = None;
            stable_since = None;
        }

        if target_page_is_terminal_mismatch(&state, expected_conversation_id) {
            // Only treat as terminal after the grace period to allow SPA routing.
            let grace_expired = start.elapsed() >= Duration::from_secs(TERMINAL_GRACE_SECS);
            let ready_settled = ready_complete_since
                .is_some_and(|since| since.elapsed() >= Duration::from_secs(READY_SETTLE_SECS));

            if grace_expired && ready_settled {
                warn!(
                    actual_cid = ?actual_cid,
                    %page_kind,
                    %page_url,
                    expected_conversation_id,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "chatgpt_web wait_target_page: terminal mismatch after grace period"
                );
                return Err(target_page_error(&state, expected_conversation_id));
            }
            debug!(
                actual_cid = ?actual_cid,
                %page_kind,
                %page_url,
                expected_conversation_id,
                grace_remaining_ms = Duration::from_secs(TERMINAL_GRACE_SECS)
                    .saturating_sub(start.elapsed())
                    .as_millis() as u64,
                ready_elapsed_ms = ready_complete_since
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0),
                "chatgpt_web wait_target_page: mismatch during grace period, continuing"
            );
        }
        sleep(Duration::from_millis(TARGET_PAGE_POLL_MS)).await;
    }
    warn!(
        expected_conversation_id,
        require_composer,
        elapsed_ms = start.elapsed().as_millis() as u64,
        last = %last,
        "chatgpt_web wait_target_page: timed out"
    );
    Err(format!(
        "browser_wrong_page: timed out waiting for stable target page: expected_conversation_id={:?} require_composer={} last={}",
        expected_conversation_id, require_composer, last
    ))
}

async fn assert_target_page(
    cdp: &CdpClient,
    expected_conversation_id: Option<&str>,
    require_composer: bool,
) -> Result<(), String> {
    let state = inspect_page(cdp).await?;
    if target_page_matches(&state, expected_conversation_id, require_composer) {
        return Ok(());
    }
    if page_state_is_auth_flow_page(&state) {
        return Err(auth_required_page_error(&state));
    }
    Err(target_page_error(&state, expected_conversation_id))
}

pub(in crate::im_gateway::chatgpt_web) fn target_page_matches(
    state: &Value,
    expected_conversation_id: Option<&str>,
    require_composer: bool,
) -> bool {
    let ready = state
        .get("readyState")
        .and_then(Value::as_str)
        .unwrap_or_default()
        == "complete";
    if !ready {
        return false;
    }
    if require_composer
        && state
            .get("visibleComposerCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
    {
        return false;
    }
    let actual = state.get("conversationId").and_then(Value::as_str);
    let url_conversation_id = state.get("urlConversationId").and_then(Value::as_str);
    match expected_conversation_id {
        Some(expected) => actual == Some(expected),
        None => actual.is_none() && url_conversation_id.is_none(),
    }
}

fn target_page_signature(state: &Value) -> String {
    let url = state.get("url").and_then(Value::as_str).unwrap_or_default();
    let page_kind = state
        .get("pageKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let conversation_id = state
        .get("conversationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let visible_composer_count = state
        .get("visibleComposerCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let busy_count = state.get("busyCount").and_then(Value::as_u64).unwrap_or(0);
    format!("{url}|{page_kind}|{conversation_id}|{visible_composer_count}|{busy_count}")
}

pub(in crate::im_gateway::chatgpt_web) fn target_page_is_terminal_mismatch(
    state: &Value,
    expected_conversation_id: Option<&str>,
) -> bool {
    let ready = state
        .get("readyState")
        .and_then(Value::as_str)
        .unwrap_or_default()
        == "complete";
    if !ready {
        return false;
    }
    // When the page body shows a fatal error (e.g. 429 "Too many requests"),
    // there is no point waiting any longer — the conversation cannot load.
    if page_body_has_fatal_error(state) {
        return true;
    }
    let actual = state.get("conversationId").and_then(Value::as_str);
    let url_conversation_id = state.get("urlConversationId").and_then(Value::as_str);
    match (expected_conversation_id, actual) {
        (Some(expected), Some(actual)) => actual != expected,
        (Some(_), None) => state
            .get("pageKind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "new_conversation"),
        (None, Some(_)) => true,
        (None, None) if url_conversation_id.is_some() => true,
        _ => false,
    }
}

pub(in crate::im_gateway::chatgpt_web) fn target_page_error(
    state: &Value,
    expected_conversation_id: Option<&str>,
) -> String {
    if page_state_is_auth_flow_page(state) {
        return auth_required_page_error(state);
    }
    let actual = state.get("conversationId").and_then(Value::as_str);
    let url_conversation_id = state.get("urlConversationId").and_then(Value::as_str);
    let url = state.get("url").and_then(Value::as_str).unwrap_or_default();
    let page_kind = state
        .get("pageKind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let body_hint = if page_body_has_fatal_error(state) {
        let body = state
            .get("bodyText")
            .and_then(Value::as_str)
            .unwrap_or_default();
        format!(" body_error={}", body.chars().take(120).collect::<String>())
    } else {
        String::new()
    };
    format!(
        "browser_wrong_page: expected_conversation_id={:?} actual_conversation_id={:?} url_conversation_id={:?} page_kind={} url={}{}",
        expected_conversation_id, actual, url_conversation_id, page_kind, url, body_hint
    )
}

async fn recover_visible_auth_flow_if_needed(
    cdp: &CdpClient,
    navigate_url: &str,
    expected_conversation_id: Option<&str>,
    duration: Duration,
) -> Result<(), String> {
    let mut state = inspect_page(cdp).await?;
    if !page_state_is_auth_flow_page(&state) {
        return Ok(());
    }
    warn!(
        page_state = %state,
        expected_conversation_id,
        "chatgpt_web send: browser entered auth flow; waiting instead of re-navigating"
    );

    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        sleep(Duration::from_secs(1)).await;
        state = inspect_page(cdp).await?;
        if page_state_is_auth_flow_page(&state) {
            continue;
        }
        if target_page_matches(&state, expected_conversation_id, false) {
            info!(
                page_state = %state,
                expected_conversation_id,
                "chatgpt_web send: auth flow returned to expected page"
            );
            return Ok(());
        }
        warn!(
            %navigate_url,
            page_state = %state,
            expected_conversation_id,
            "chatgpt_web send: auth flow finished away from target, navigating back once"
        );
        cdp.navigate_and_wait(navigate_url, Duration::from_secs(20))
            .await?;
        return Ok(());
    }

    Err(auth_required_page_error(&state))
}

/// This is not an auth validator. Full login-state validation remains in
/// `auth_status_from_state`; this only prevents the send loop from treating a
/// visible ChatGPT/OpenAI third-party auth flow as a stale conversation or
/// composer delay.
pub(in crate::im_gateway::chatgpt_web) fn page_state_is_auth_flow_page(state: &Value) -> bool {
    let url = state.get("url").and_then(Value::as_str).unwrap_or_default();
    let url_lower = url.to_ascii_lowercase();
    if url_lower.contains("/auth/login")
        || url_lower.contains("/auth/")
        || url_lower.contains("auth.openai.com")
        || url_lower.contains("auth0.openai.com")
        || url_lower.contains("accounts.google.")
        || url_lower.contains("appleid.apple.com")
        || url_lower.contains("login.microsoftonline.com")
    {
        return true;
    }
    let title = state
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if title.contains("log in")
        || title.contains("login")
        || title.contains("开始使用")
        || title.contains("chatgpt")
            && state
                .get("bodyText")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("登录即可开始聊天")
    {
        return true;
    }
    let body = state
        .get("bodyText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    body.contains("log in to chatgpt")
        || body.contains("sign up")
        || body.contains("continue with google")
        || body.contains("登录即可开始聊天")
        || body.contains("使用 google 账号继续")
}

fn auth_required_page_error(state: &Value) -> String {
    let url = state.get("url").and_then(Value::as_str).unwrap_or_default();
    let title = state
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ready = state
        .get("readyState")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!(
        "auth_required: ChatGPT Web browser is still in auth flow: url={url} title={title:?} readyState={ready}"
    )
}

/// Detect fatal error pages (429, server errors, etc.) where the conversation
/// will never load and continuing to poll is pointless.
fn page_body_has_fatal_error(state: &Value) -> bool {
    let body = state
        .get("bodyText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    body.contains("too many requests")
        || body.contains("429")
        || body.contains("something went wrong")
        || body.contains("internal server error")
        || body.contains("unable to load conversation")
        || body.contains("conversation not found")
}

pub(in crate::im_gateway::chatgpt_web) fn should_retry_as_new_conversation(error: &str) -> bool {
    if !error.starts_with("browser_wrong_page:") {
        return false;
    }
    // Page landed on new_conversation / homepage — conversation is gone/expired.
    error.contains("actual_conversation_id=None")
        || error.contains("page_kind=new_conversation")
        || error.ends_with("url=https://chatgpt.com/")
        // Page shows a fatal error (429, server error, etc.) — the
        // conversation cannot load, fall back to a new one.
        || error.contains("body_error=")
}

#[allow(dead_code)]
async fn navigate_to_new_conversation(
    cdp: &CdpClient,
    config: &RuntimeConfig,
    reason: &str,
) -> Result<(), String> {
    // Check if the page is already on the homepage — if so, skip navigation
    // to avoid unnecessary page reload / 429 rate-limits.
    let current = inspect_page(cdp).await.unwrap_or(json!({}));
    let current_kind = current
        .get("pageKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_url_conversation_id = current.get("urlConversationId").and_then(Value::as_str);
    let needs_navigate = current_kind != "new_conversation"
        || current_url_conversation_id.is_some()
        || page_body_has_fatal_error(&current);

    if needs_navigate {
        let new_url = new_conversation_url(config);
        warn!(
            new_url = %new_url,
            %reason,
            current_kind,
            current_url_conversation_id,
            "chatgpt_web send: page not on homepage or has errors, navigating to new conversation"
        );
        cdp.navigate_and_wait(&new_url, Duration::from_secs(15))
            .await?;
        wait_target_page(cdp, None, Duration::from_secs(30), false).await
    } else {
        warn!(
            %reason,
            "chatgpt_web send: page already on homepage, using current page directly"
        );
        Ok(())
    }
}

fn new_conversation_url(config: &RuntimeConfig) -> String {
    let base = config.chatgpt.base_url.trim_end_matches('/');
    let nonce = now_ms();
    format!("{base}/?bifrost_new_chat={nonce}")
}

async fn set_composer_text(cdp: &CdpClient, text: &str) -> Result<Value, String> {
    let locate_expression = r#"(() => {
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);
          const candidates = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          ))
            .filter((el) => isVisible(el) && !el.disabled && !el.readOnly)
            .map((el) => {
              const rect = el.getBoundingClientRect();
              const score =
                (el.id === "prompt-textarea" ? 1000 : 0) +
                (el.getAttribute("contenteditable") === "true" ? 500 : 0) +
                (el.tagName === "TEXTAREA" ? 200 : 0) +
                rect.bottom;
              return { el, rect, score };
            })
            .sort((a, b) => b.score - a.score);
          const item = candidates[0];
          if (!item) return { ok: false, error: "composer_not_found", url: location.href };
          const composer = item.el;
          const rect = item.rect;
          composer.scrollIntoView({ block: "center", inline: "nearest" });
          composer.focus();
          return {
            ok: true,
            url: location.href,
            tag: composer.tagName,
            id: composer.id || null,
            contentEditable: composer.getAttribute("contenteditable"),
            x: Math.max(1, rect.left + Math.min(rect.width / 2, 24)),
            y: Math.max(1, rect.top + rect.height / 2)
          };
        })()"#
    .to_string();
    let target = evaluate_value(cdp, &locate_expression).await?;
    if !target.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("browser_ui: UI locate composer failed: {target}"));
    }
    let x = target.get("x").and_then(Value::as_f64).unwrap_or(1.0);
    let y = target.get("y").and_then(Value::as_f64).unwrap_or(1.0);
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
    )
    .await?;
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
    )
    .await?;
    select_all(cdp).await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"Backspace","code":"Backspace","windowsVirtualKeyCode":8}),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"Backspace","code":"Backspace","windowsVirtualKeyCode":8}),
    )
    .await?;
    clear_visible_composer(cdp).await?;
    let injection_mode = composer_text_injection_mode(text);
    match injection_mode {
        ComposerTextInjectionMode::InsertText => {
            cdp.send("Input.insertText", json!({"text": text})).await?;
        }
        ComposerTextInjectionMode::NativeClipboardPaste => {
            let paste_result = paste_composer_text(cdp, text, injection_mode).await?;
            debug!(
                result = %paste_result,
                "chatgpt_web send: injected composer text via native clipboard paste path"
            );
        }
    }
    // ProseMirror needs time to process text into paragraph nodes.
    // Scale wait by text complexity: short texts are instant, long texts
    // with newlines need more time for DOM node creation.
    let prosemirror_wait = if text.len() > 200 || text.contains('\n') {
        Duration::from_millis(200)
    } else {
        Duration::from_millis(50)
    };
    sleep(prosemirror_wait).await;
    let result = match injection_mode {
        ComposerTextInjectionMode::InsertText => verify_composer_text_exact(cdp, text).await?,
        ComposerTextInjectionMode::NativeClipboardPaste => json!({
            "ok": true,
            "mode": "native_clipboard_paste_dispatched"
        }),
    };
    if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(result)
    } else {
        Err(format!("browser_ui: UI set composer text failed: {result}"))
    }
}

async fn verify_composer_text_exact(cdp: &CdpClient, text: &str) -> Result<Value, String> {
    let text_json =
        serde_json::to_string(text).map_err(|error| format!("serialize prompt failed: {error}"))?;
    let result = evaluate_value(
        cdp,
        &format!(
            r#"(() => {{
          const text = {text_json};
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);
          const candidates = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          )).filter((el) => isVisible(el) && !el.disabled && !el.readOnly);
          const composer = candidates
            .sort((a, b) => b.getBoundingClientRect().bottom - a.getBoundingClientRect().bottom)[0];
          if (!composer) return {{ ok: false, error: "composer_not_found_after_insert" }};
          let actual = ("value" in composer ? composer.value : (composer.innerText || composer.textContent || "")).trim();
          // ProseMirror transforms newlines into <p> paragraph nodes, so
          // innerText returns double-newlines between paragraphs.  Normalize
          // both sides: collapse all runs of whitespace (including \n) into
          // single spaces for comparison.
          const normalize = (s) => s.replace(/\s+/g, ' ').trim();
          const actualNorm = normalize(actual);
          const expectedNorm = normalize(text);
          const btn =
            document.querySelector('[data-testid="send-button"]') ||
            document.querySelector('[data-testid="composer-submit-button"]') ||
            document.querySelector('button[aria-label*="发送"]') ||
            document.querySelector('button.composer-submit-btn');
          return {{
            ok: actualNorm === expectedNorm || actual.length > 0 && actualNorm.includes(expectedNorm.substring(0, Math.min(100, expectedNorm.length))),
            url: location.href,
            tag: composer.tagName,
            id: composer.id || null,
            contentEditable: composer.getAttribute("contenteditable"),
            text: actual,
            sendButton: btn ? {{
              disabled: Boolean(btn.disabled),
              testId: btn.getAttribute("data-testid") || "",
              aria: btn.getAttribute("aria-label") || "",
              className: btn.className || ""
            }} : null
          }};
        }})()"#
        ),
    )
    .await?;
    // The JS-side `ok` already uses normalized comparison.  On the Rust side
    // we also normalize: ProseMirror turns newlines into paragraph boundaries
    // whose innerText representation may differ from the raw input.
    let js_ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let has_text = result
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|actual| {
            let norm_actual = normalize_whitespace(actual);
            let norm_expected = normalize_whitespace(text);
            norm_actual == norm_expected
                || (!norm_actual.is_empty()
                    && norm_actual.contains(&norm_expected[..norm_expected.len().min(100)]))
        });
    Ok(json!({
        "ok": js_ok || has_text,
        "mode": "exact",
        "result": result
    }))
}

async fn paste_composer_text(
    cdp: &CdpClient,
    text: &str,
    mode: ComposerTextInjectionMode,
) -> Result<Value, String> {
    match mode {
        ComposerTextInjectionMode::InsertText => unreachable!("insertText mode does not paste"),
        ComposerTextInjectionMode::NativeClipboardPaste => {
            paste_composer_text_native(cdp, text).await
        }
    }
}

async fn paste_composer_text_native(cdp: &CdpClient, text: &str) -> Result<Value, String> {
    write_browser_clipboard(cdp, text).await?;
    dispatch_paste_shortcut(cdp).await?;
    sleep(Duration::from_millis(500)).await;
    let state = inspect_composer_after_native_paste(cdp).await?;
    let text_len = state
        .get("textLength")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if text_len < 20 {
        focus_composer(cdp).await?;
        cdp.send(
            "Input.insertText",
            json!({"text": native_clipboard_paste_followup_instruction()}),
        )
        .await?;
    }
    Ok(json!({
        "ok": true,
        "mode": "native_clipboard_paste",
        "composerState": state,
        "followupInstructionInserted": text_len < 20
    }))
}

async fn inspect_composer_after_native_paste(cdp: &CdpClient) -> Result<Value, String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const isVisible = (el) => !!(el && (el.offsetWidth || el.offsetHeight || el.getClientRects().length));
          const candidates = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          )).filter((el) => isVisible(el) && !el.disabled && !el.readOnly);
          const composer = candidates
            .sort((a, b) => b.getBoundingClientRect().bottom - a.getBoundingClientRect().bottom)[0];
          const text = composer ? (composer.value || composer.innerText || composer.textContent || '') : '';
          const attachmentCount = document.querySelectorAll(
            '[data-testid*="attachment"], [data-testid*="file"], [aria-label*="file"], [aria-label*="文件"], [aria-label*="附件"]'
          ).length;
          return {
            ok: !!composer,
            textLength: text.trim().length,
            attachmentCount,
            preview: text.trim().slice(0, 120)
          };
        })()"#,
    )
    .await
}

async fn write_browser_clipboard(cdp: &CdpClient, text: &str) -> Result<Value, String> {
    let origin = evaluate_value(cdp, "location.origin")
        .await
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .filter(|origin| origin.starts_with("http"))
        .unwrap_or_else(|| "https://chatgpt.com".to_string());
    cdp.send(
        "Browser.grantPermissions",
        json!({
            "origin": origin,
            "permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"]
        }),
    )
    .await
    .map_err(|error| format!("grant browser clipboard permission failed: {error}"))?;

    let text_json =
        serde_json::to_string(text).map_err(|error| format!("serialize prompt failed: {error}"))?;
    let result = evaluate_value(
        cdp,
        &format!(
            r#"(async () => {{
              try {{
                await navigator.clipboard.writeText({text_json});
                return {{ ok: true }};
              }} catch (error) {{
                return {{
                  ok: false,
                  name: error?.name || "Error",
                  message: error?.message || String(error)
                }};
              }}
            }})()"#
        ),
    )
    .await?;
    if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(result)
    } else {
        Err(format!("browser clipboard write failed: {result}"))
    }
}

pub(in crate::im_gateway::chatgpt_web) async fn dispatch_paste_shortcut(
    cdp: &CdpClient,
) -> Result<(), String> {
    let modifier = paste_modifier();
    let (modifier_key, modifier_code, modifier_virtual_key) = paste_modifier_key();
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyDown",
            "key": modifier_key,
            "code": modifier_code,
            "windowsVirtualKeyCode": modifier_virtual_key,
            "nativeVirtualKeyCode": modifier_virtual_key,
            "modifiers": modifier
        }),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyDown",
            "key": "v",
            "code": "KeyV",
            "windowsVirtualKeyCode": 86,
            "nativeVirtualKeyCode": 86,
            "modifiers": modifier,
            "commands": ["paste"]
        }),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": "v",
            "code": "KeyV",
            "windowsVirtualKeyCode": 86,
            "nativeVirtualKeyCode": 86,
            "modifiers": modifier
        }),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": modifier_key,
            "code": modifier_code,
            "windowsVirtualKeyCode": modifier_virtual_key,
            "nativeVirtualKeyCode": modifier_virtual_key,
            "modifiers": 0
        }),
    )
    .await?;
    Ok(())
}

pub(in crate::im_gateway::chatgpt_web) fn paste_modifier() -> i32 {
    if cfg!(target_os = "macos") {
        4
    } else {
        2
    }
}

fn paste_modifier_key() -> (&'static str, &'static str, i32) {
    if cfg!(target_os = "macos") {
        ("Meta", "MetaLeft", 91)
    } else {
        ("Control", "ControlLeft", 17)
    }
}

/// Locate the send button using Playwright-style actionability checks.
/// Returns coordinates and state without clicking. Uses check_actionability
/// which verifies visible, stable, enabled, and receives-events.
async fn locate_send_button(cdp: &CdpClient) -> Result<Value, String> {
    let selector = r#"[data-testid="send-button"], [data-testid="composer-submit-button"], button[aria-label*="发送"], button.composer-submit-btn"#;
    // Short timeout since wait_send_button_ready already ensured actionability.
    match cdp
        .check_actionability(selector, Duration::from_secs(3))
        .await
    {
        Ok(info) => {
            // Map check_actionability response to the format callers expect.
            Ok(json!({
                "ok": true,
                "disabled": false,
                "x": info.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                "y": info.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                "tag": info.get("tag").and_then(Value::as_str).unwrap_or(""),
                "id": info.get("id"),
            }))
        }
        Err(e) => {
            // Return in legacy format so caller can fall back to Enter key.
            Ok(json!({
                "ok": false,
                "disabled": true,
                "error": e,
            }))
        }
    }
}

async fn wait_send_button_ready(cdp: &CdpClient, duration: Duration) -> Result<(), String> {
    // Use Playwright-style actionability check: waits for the send button to be
    // visible, stable (not animating), enabled, and receiving events (not obscured).
    // Replaces the old 250ms polling loop with an event-driven approach.
    let selector = r#"[data-testid="send-button"], [data-testid="composer-submit-button"], button[aria-label*="发送"], button.composer-submit-btn"#;
    match cdp.check_actionability(selector, duration).await {
        Ok(info) => {
            let tag = info.get("tag").and_then(|v| v.as_str()).unwrap_or("?");
            let x = info.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = info.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            debug!(
                tag,
                x, y, "chatgpt_web wait_send_button_ready: send button actionable ✓"
            );
            Ok(())
        }
        Err(e) => Err(format!("browser_ui: send button not actionable: {e}")),
    }
}

async fn click_send_button_dom(cdp: &CdpClient) -> Result<Value, String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const btn =
            document.querySelector('[data-testid="send-button"]') ||
            document.querySelector('[data-testid="composer-submit-button"]') ||
            document.querySelector('button[aria-label*="发送"]') ||
            document.querySelector('button.composer-submit-btn');
          if (!btn || !(btn.offsetWidth || btn.offsetHeight || btn.getClientRects().length)) {
            return { ok: false, error: "send_button_not_found" };
          }
          if (btn.disabled) return { ok: false, error: "send_button_disabled" };
          btn.scrollIntoView({ block: "center", inline: "center" });
          btn.click();
          return {
            ok: true,
            testId: btn.getAttribute("data-testid") || "",
            aria: btn.getAttribute("aria-label") || "",
            className: btn.className || ""
          };
        })()"#,
    )
    .await
}

async fn select_all(cdp: &CdpClient) -> Result<(), String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const active = document.activeElement;
          if (!active) return { ok: false, error: "no_active_element" };
          if (typeof active.select === "function") {
            active.select();
            return { ok: true, mode: "form-control" };
          }
          const range = document.createRange();
          range.selectNodeContents(active);
          const selection = window.getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
          return {
            ok: true,
            mode: "contenteditable",
            tag: active.tagName,
            id: active.id || null
          };
        })()"#,
    )
    .await?;
    Ok(())
}

async fn clear_visible_composer(cdp: &CdpClient) -> Result<(), String> {
    let result = evaluate_value(
        cdp,
        r#"(() => {
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);
          const candidates = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          )).filter((el) => isVisible(el) && !el.disabled && !el.readOnly)
            .sort((a, b) => b.getBoundingClientRect().bottom - a.getBoundingClientRect().bottom);
          const composer = candidates[0];
          if (!composer) return { ok: false, error: "composer_not_found" };
          composer.focus();
          if ("value" in composer) {
            composer.value = "";
          } else {
            composer.textContent = "";
            composer.innerHTML = "";
          }
          composer.dispatchEvent(new InputEvent("input", {
            bubbles: true,
            cancelable: true,
            inputType: "deleteContentBackward",
            data: null
          }));
          const actual = ("value" in composer ? composer.value : (composer.innerText || composer.textContent || "")).trim();
          return {
            ok: actual.length === 0,
            text: actual,
            tag: composer.tagName,
            id: composer.id || null,
            contentEditable: composer.getAttribute("contenteditable")
          };
        })()"#,
    )
    .await?;
    if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(())
    } else {
        Err(format!("browser_ui: clear composer failed: {result}"))
    }
}

async fn dispatch_enter(cdp: &CdpClient) -> Result<(), String> {
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"Enter","code":"Enter","windowsVirtualKeyCode":13}),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"Enter","code":"Enter","windowsVirtualKeyCode":13}),
    )
    .await?;
    Ok(())
}

/// Focus the composer element so that subsequent CDP key events target it.
async fn focus_composer(cdp: &CdpClient) -> Result<(), String> {
    evaluate_value(
        cdp,
        r#"(() => {
          const isVisible = (el) => !!(el?.offsetWidth || el?.offsetHeight || el?.getClientRects().length);
          const candidates = Array.from(document.querySelectorAll(
            '[data-testid="composer-text-input"], #prompt-textarea, [contenteditable="true"], textarea'
          ))
            .filter((el) => isVisible(el) && !el.disabled && !el.readOnly)
            .map((el) => {
              const rect = el.getBoundingClientRect();
              const score =
                (el.id === "prompt-textarea" ? 1000 : 0) +
                (el.getAttribute("contenteditable") === "true" ? 500 : 0) +
                (el.tagName === "TEXTAREA" ? 200 : 0) +
                rect.bottom;
              return { el, score };
            })
            .sort((a, b) => b.score - a.score);
          const item = candidates[0];
          if (item) { item.el.focus(); return { ok: true, tag: item.el.tagName, id: item.el.id || null }; }
          return { ok: false };
        })()"#,
    )
    .await?;
    Ok(())
}

/// Click at the given viewport coordinates using CDP trusted mouse events
/// (with pointerType so that the browser generates both PointerEvent and
/// MouseEvent sequences).
async fn cdp_click(cdp: &CdpClient, x: f64, y: f64) -> Result<(), String> {
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1,"pointerType":"mouse"}),
    )
    .await?;
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1,"pointerType":"mouse"}),
    )
    .await?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn normalize_whitespace_collapses_runs_and_trims() {
        assert_eq!(normalize_whitespace("  a  b\n\n c\t\td"), "a b c d");
        assert_eq!(normalize_whitespace("   \n\t  "), "");
    }

    #[test]
    fn is_retryable_send_error_matches_known_prefixes() {
        for msg in [
            "browser_send_not_submitted: composer not ready",
            "send_button_not_found on page",
            "browser_wrong_page: expected_conversation_id=None",
            "500 Internal Server Error from chatgpt.com",
            "Cannot find default execution context",
            "create browser page failed: 500",
            "connect CDP websocket failed: timeout",
        ] {
            assert!(is_retryable_send_error(msg), "{msg} should be retryable");
        }
        assert!(!is_retryable_send_error("permanent_failure: auth_required"));
    }

    #[test]
    fn is_cdp_transient_error_matches_all_variants() {
        for msg in [
            "CDP command timed out: Runtime.evaluate",
            "CDP connection closed unexpectedly",
            "Inspected target navigated or closed",
            "Target closed",
            "Session closed by browser",
            "CDP event channel closed",
        ] {
            assert!(is_cdp_transient_error(msg), "{msg} should be CDP transient");
        }
        assert!(!is_cdp_transient_error("other error"));
    }

    #[test]
    fn send_button_ready_max_wait_short_and_long() {
        let short = "a".repeat(50);
        assert_eq!(send_button_ready_max_wait(&short), Duration::from_secs(10));

        let long = "a".repeat(121);
        // chars / 10_000 = 0 here, but mode switches to paste path with min 30s
        assert_eq!(send_button_ready_max_wait(&long), Duration::from_secs(30),);

        let very_long = "a".repeat(2_000_000);
        // 30 + 2000000/10000 = 230, clamped to 180
        assert_eq!(
            send_button_ready_max_wait(&very_long),
            Duration::from_secs(180),
        );
    }

    #[test]
    fn send_button_ready_retry_max_wait_depends_on_injection_mode() {
        let short = "x".repeat(10);
        let long = "x".repeat(COMPOSER_PASTE_THRESHOLD_CHARS + 1);
        assert_eq!(
            composer_text_injection_mode(&short),
            ComposerTextInjectionMode::InsertText
        );
        assert_eq!(
            composer_text_injection_mode(&long),
            ComposerTextInjectionMode::NativeClipboardPaste
        );
        assert_eq!(
            send_button_ready_retry_max_wait(&short),
            Duration::from_secs(15),
        );
        assert_eq!(
            send_button_ready_retry_max_wait(&long),
            Duration::from_secs(60),
        );
    }

    #[test]
    fn parse_send_sse_collects_messages_and_event_types() {
        let sse = "data: {\"type\":\"delta\",\"conversation_id\":\"c1\",\"turn_exchange_id\":\"t1\",\"message\":{\"id\":\"m1\",\"text\":\"hello\"}}\n\n"
            .to_string()
            + "data: {\"type\":\"stream_handoff\",\"conversation_id\":\"c1\",\"message\":{\"id\":\"m1\",\"text\":\"final\"}}\n\n";
        let parsed = parse_send_sse(&sse);
        assert_eq!(parsed.conversation_id.as_deref(), Some("c1"));
        assert_eq!(parsed.turn_exchange_id.as_deref(), Some("t1"));
        assert_eq!(parsed.event_types, vec!["delta", "stream_handoff"]);
        let mapping = parsed
            .sse_detail
            .as_ref()
            .and_then(|v| v.get("mapping"))
            .and_then(Value::as_object)
            .expect("mapping");
        let msg = mapping.get("m1").expect("m1 entry");
        assert_eq!(
            msg.get("message")
                .and_then(|m| m.get("text"))
                .and_then(Value::as_str),
            Some("final"),
        );
    }

    #[test]
    fn page_body_has_fatal_error_matches_common_patterns() {
        for body in [
            "Too many requests", // 429
            "HTTP 429: Too Many Requests",
            "Something went wrong while loading",
            "Internal Server Error", // 500
            "Unable to load conversation",
            "Conversation not found",
        ] {
            let state = json!({ "bodyText": body });
            assert!(page_body_has_fatal_error(&state));
        }

        let ok_state = json!({ "bodyText": "normal page" });
        assert!(!page_body_has_fatal_error(&ok_state));
    }

    #[test]
    fn target_page_signature_includes_core_fields() {
        let state = json!({
            "url": "https://chatgpt.com/c/c1",
            "pageKind": "conversation",
            "conversationId": "c1",
            "visibleComposerCount": 2,
            "busyCount": 1,
        });
        let sig = target_page_signature(&state);
        assert!(sig.contains("https://chatgpt.com/c/c1"));
        assert!(sig.contains("conversation"));
        assert!(sig.contains("c1"));
        assert!(sig.contains("2"));
        assert!(sig.ends_with("|1"));
    }

    #[test]
    fn paste_modifier_and_key_are_consistent_for_platform() {
        let modifier = paste_modifier();
        let (key, code, vk) = paste_modifier_key();
        if cfg!(target_os = "macos") {
            assert_eq!(modifier, 4);
            assert_eq!((key, code, vk), ("Meta", "MetaLeft", 91));
        } else {
            assert_eq!(modifier, 2);
            assert_eq!((key, code, vk), ("Control", "ControlLeft", 17));
        }
    }

    #[test]
    fn extract_conversation_id_from_url_handles_variants() {
        assert_eq!(
            extract_conversation_id_from_url("https://chatgpt.com/c/abc-123?model=gpt",).as_deref(),
            Some("abc-123"),
        );
        assert_eq!(
            extract_conversation_id_from_url("https://chatgpt.com/c/abc-123/extra"),
            Some("abc-123".to_string()),
        );
        assert_eq!(
            extract_conversation_id_from_url("https://chatgpt.com/"),
            None,
        );
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use serde_json::{json, Value};

    // --- parse_send_sse additional coverage ---

    #[test]
    fn parse_send_sse_empty_input_returns_defaults() {
        let parsed = parse_send_sse("");
        assert!(parsed.conversation_id.is_none());
        assert!(parsed.turn_exchange_id.is_none());
        assert!(parsed.event_types.is_empty());
        assert!(parsed.sse_detail.is_none());
    }

    #[test]
    fn parse_send_sse_ignores_non_data_and_invalid_json_lines() {
        let sse = "event: message\n\n data: not-json\n\n data: {\"incomplete\": true";
        let parsed = parse_send_sse(sse);
        assert!(parsed.conversation_id.is_none());
        assert!(parsed.turn_exchange_id.is_none());
        assert!(parsed.event_types.is_empty());
        assert!(parsed.sse_detail.is_none());
    }

    #[test]
    fn parse_send_sse_defaults_type_to_patch_when_missing() {
        let sse = "data: {\"conversation_id\":\"c1\",\"turn_exchange_id\":\"t1\"}\n\n";
        let parsed = parse_send_sse(sse);
        assert_eq!(parsed.conversation_id.as_deref(), Some("c1"));
        assert_eq!(parsed.turn_exchange_id.as_deref(), Some("t1"));
        assert_eq!(parsed.event_types, vec!["patch".to_string()]);
    }

    #[test]
    fn parse_send_sse_keeps_first_ids_even_when_later_events_change() {
        let sse = "data: {\"type\":\"delta\",\"conversation_id\":\"c1\",\"turn_exchange_id\":\"t1\"}\n\n"
            .to_string()
            + "data: {\"type\":\"delta\",\"conversation_id\":\"c2\",\"turn_exchange_id\":\"t2\"}\n\n";
        let parsed = parse_send_sse(&sse);
        assert_eq!(parsed.conversation_id.as_deref(), Some("c1"));
        assert_eq!(parsed.turn_exchange_id.as_deref(), Some("t1"));
        assert_eq!(
            parsed.event_types,
            vec!["delta".to_string(), "delta".to_string()]
        );
    }

    #[test]
    fn parse_send_sse_collects_multiple_message_ids() {
        let sse = "data: {\"type\":\"delta\",\"conversation_id\":\"c1\",\"message\":{\"id\":\"m1\"}}\n\n"
            .to_string()
            + "data: {\"type\":\"delta\",\"conversation_id\":\"c1\",\"message\":{\"id\":\"m2\"}}\n\n";
        let parsed = parse_send_sse(&sse);
        let mapping = parsed
            .sse_detail
            .as_ref()
            .and_then(|v| v.get("mapping"))
            .and_then(Value::as_object)
            .expect("mapping");
        assert!(mapping.contains_key("m1"));
        assert!(mapping.contains_key("m2"));
    }

    // --- format_send_diagnostic coverage ---

    #[test]
    fn format_send_diagnostic_includes_page_and_defaults() {
        let diag = json!({
            "url": "https://chatgpt.com/",
        });
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("page=https://chatgpt.com/"));
        assert!(s.contains("composer=NOT_FOUND"));
        assert!(s.contains("sendButton=NOT_FOUND"));
    }

    #[test]
    fn format_send_diagnostic_marks_composer_hidden_disabled_and_empty() {
        // hidden
        let hidden = json!({
            "composerFound": true,
            "composerVisible": false,
        });
        let s = format_send_diagnostic(&hidden);
        assert!(s.contains("composer=HIDDEN"));

        // disabled
        let disabled = json!({
            "composerFound": true,
            "composerVisible": true,
            "composerDisabled": true,
        });
        let s = format_send_diagnostic(&disabled);
        assert!(s.contains("composer=DISABLED"));

        // empty
        let empty = json!({
            "composerFound": true,
            "composerVisible": true,
            "composerDisabled": false,
            "composerHasText": false,
        });
        let s = format_send_diagnostic(&empty);
        assert!(s.contains("composer=EMPTY"));
    }

    #[test]
    fn format_send_diagnostic_includes_text_preview_when_composer_ok() {
        let diag = json!({
            "composerFound": true,
            "composerVisible": true,
            "composerDisabled": false,
            "composerHasText": true,
            "composerTextPreview": "hello world",
        });
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("composer=OK"));
        assert!(s.contains("hello world"));
    }

    #[test]
    fn format_send_diagnostic_covers_send_button_states() {
        let not_found = json!({});
        let s = format_send_diagnostic(&not_found);
        assert!(s.contains("sendButton=NOT_FOUND"));

        let hidden = json!({
            "sendButtonFound": true,
            "sendButtonVisible": false,
        });
        let s = format_send_diagnostic(&hidden);
        assert!(s.contains("sendButton=HIDDEN"));

        let disabled = json!({
            "sendButtonFound": true,
            "sendButtonVisible": true,
            "sendButtonDisabled": true,
        });
        let s = format_send_diagnostic(&disabled);
        assert!(s.contains("sendButton=DISABLED"));

        let ok = json!({
            "sendButtonFound": true,
            "sendButtonVisible": true,
            "sendButtonDisabled": false,
        });
        let s = format_send_diagnostic(&ok);
        assert!(s.contains("sendButton=OK"));
    }

    #[test]
    fn format_send_diagnostic_includes_stop_busy_and_banners() {
        let diag = json!({
            "stopButtonVisible": true,
            "busyCount": 3,
            "errorBanners": ["Too many requests", ""],
        });
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("stopButton=VISIBLE"));
        assert!(s.contains("busyElements=3"));
        assert!(s.contains("banner=\"Too many requests\""));
        assert!(!s.contains("banner=\"\""));
    }

    // --- target_page_matches / terminal_mismatch / error ---

    #[test]
    fn target_page_matches_false_when_not_ready() {
        let state = json!({"readyState": "loading"});
        assert!(!target_page_matches(&state, None, false));
    }

    #[test]
    fn target_page_matches_requires_visible_composer_when_requested() {
        let state = json!({
            "readyState": "complete",
            "conversationId": "c1",
            "visibleComposerCount": 0,
        });
        assert!(target_page_matches(&state, Some("c1"), false));
        assert!(!target_page_matches(&state, Some("c1"), true));
    }

    #[test]
    fn target_page_matches_for_new_conversation_without_id() {
        let state = json!({
            "readyState": "complete",
            "conversationId": null,
            "visibleComposerCount": 1,
        });
        assert!(target_page_matches(&state, None, true));
        assert!(!target_page_matches(&state, Some("c1"), true));
    }

    #[test]
    fn target_page_is_terminal_mismatch_false_when_not_ready() {
        let state = json!({
            "readyState": "loading",
            "conversationId": "c1",
        });
        assert!(!target_page_is_terminal_mismatch(&state, Some("c2")));
    }

    #[test]
    fn target_page_is_terminal_mismatch_true_when_actual_has_id_but_expected_none() {
        let state = json!({
            "readyState": "complete",
            "conversationId": "c1",
        });
        assert!(target_page_is_terminal_mismatch(&state, None));
    }

    #[test]
    fn target_page_is_terminal_mismatch_false_when_expected_some_actual_none_and_not_new_page() {
        let state = json!({
            "readyState": "complete",
            "conversationId": null,
            "pageKind": "conversation",
        });
        assert!(!target_page_is_terminal_mismatch(&state, Some("c1")));
    }

    #[test]
    fn target_page_is_terminal_mismatch_true_when_body_has_fatal_error() {
        let state = json!({
            "readyState": "complete",
            "bodyText": "HTTP 500 Internal Server Error",
        });
        assert!(target_page_is_terminal_mismatch(&state, None));
    }

    #[test]
    fn target_page_error_includes_body_hint_for_fatal_error() {
        let state = json!({
            "url": "https://chatgpt.com/",
            "readyState": "complete",
            "pageKind": "conversation",
            "bodyText": "Too many requests",
        });
        let err = target_page_error(&state, Some("c1"));
        assert!(err.contains("browser_wrong_page"));
        assert!(err.contains("body_error="));
    }

    #[test]
    fn target_page_error_omits_body_hint_when_body_ok() {
        let state = json!({
            "url": "https://chatgpt.com/c/c1",
            "readyState": "complete",
            "pageKind": "conversation",
            "bodyText": "normal page",
        });
        let err = target_page_error(&state, None);
        assert!(err.contains("browser_wrong_page"));
        assert!(!err.contains("body_error="));
    }

    // --- auth_flow page detection ---

    #[test]
    fn page_state_is_auth_flow_page_matches_auth0_and_oauth_domains() {
        let auth0 = json!({"url": "https://auth0.openai.com/auth"});
        assert!(page_state_is_auth_flow_page(&auth0));

        let ms = json!({"url": "https://login.microsoftonline.com/oauth2"});
        assert!(page_state_is_auth_flow_page(&ms));
    }

    #[test]
    fn page_state_is_auth_flow_page_matches_google_and_apple_domains() {
        let google = json!({"url": "https://accounts.google.com/o/oauth2/v2/auth"});
        assert!(page_state_is_auth_flow_page(&google));

        let apple = json!({"url": "https://appleid.apple.com/auth"});
        assert!(page_state_is_auth_flow_page(&apple));
    }

    #[test]
    fn page_state_is_auth_flow_page_matches_login_titles() {
        let title = json!({
            "title": "Log in - ChatGPT",
            "bodyText": "",
        });
        assert!(page_state_is_auth_flow_page(&title));

        let chinese = json!({
            "title": "开始使用 | ChatGPT",
            "bodyText": "登录即可开始聊天",
        });
        assert!(page_state_is_auth_flow_page(&chinese));
    }

    #[test]
    fn page_state_is_auth_flow_page_matches_body_keywords() {
        let body1 = json!({"bodyText": "Log in to ChatGPT"});
        assert!(page_state_is_auth_flow_page(&body1));

        let body2 = json!({"bodyText": "please sign up to continue"});
        assert!(page_state_is_auth_flow_page(&body2));

        let body3 = json!({"bodyText": "Continue with Google"});
        assert!(page_state_is_auth_flow_page(&body3));

        let body4 = json!({"bodyText": "使用 Google 账号继续"});
        assert!(page_state_is_auth_flow_page(&body4));
    }

    #[test]
    fn auth_required_page_error_formats_fields() {
        let state = json!({
            "url": "https://chatgpt.com/auth/login",
            "title": "Log in - ChatGPT",
            "readyState": "complete",
        });
        let err = auth_required_page_error(&state);
        assert!(err.contains("auth_required:"));
        assert!(err.contains("url=https://chatgpt.com/auth/login"));
        assert!(err.contains("title=\"Log in - ChatGPT\""));
        assert!(err.contains("readyState=complete"));
    }

    // --- small helpers: page_body_has_fatal_error, should_retry_as_new_conversation ---

    #[test]
    fn page_body_has_fatal_error_is_false_for_normal_body() {
        let state = json!({"bodyText": "All good here"});
        assert!(!page_body_has_fatal_error(&state));
    }

    #[test]
    fn should_retry_as_new_conversation_false_for_non_browser_wrong_page() {
        assert!(!should_retry_as_new_conversation("other_error: something"));
    }

    #[test]
    fn should_retry_as_new_conversation_true_for_new_conversation_homepage() {
        let err = "browser_wrong_page: expected_conversation_id=Some(\"old\") actual_conversation_id=None page_kind=new_conversation url=https://chatgpt.com/";
        assert!(should_retry_as_new_conversation(err));
    }

    #[test]
    fn should_retry_as_new_conversation_true_for_error_with_body_error_hint() {
        let err = "browser_wrong_page: expected_conversation_id=None actual_conversation_id=None page_kind=unknown url=https://chatgpt.com/ body_error=Too many requests";
        assert!(should_retry_as_new_conversation(err));
    }

    // --- heartbeat helpers ---

    #[test]
    fn handoff_heartbeat_error_none_when_browser_and_cdp_healthy() {
        assert!(handoff_heartbeat_error(false, false).is_none());
    }

    #[test]
    fn handoff_page_heartbeat_error_wraps_original_error() {
        let err = handoff_page_heartbeat_error("CDP command timed out: Runtime.evaluate");
        assert!(err.starts_with("browser_unavailable:"));
        assert!(err.contains("Runtime.evaluate"));
    }

    // --- URL extraction & whitespace helpers ---

    #[test]
    fn extract_conversation_id_from_url_trims_and_stops_at_query_or_hash() {
        let url = "https://chatgpt.com/c/abc-123?model=gpt#section";
        assert_eq!(
            extract_conversation_id_from_url(url).as_deref(),
            Some("abc-123"),
        );
    }

    #[test]
    fn extract_conversation_id_from_url_ignores_trailing_slash_segments() {
        let url = "https://chatgpt.com/c/abc-123/extra/path";
        assert_eq!(
            extract_conversation_id_from_url(url).as_deref(),
            Some("abc-123"),
        );
    }

    #[test]
    fn normalize_whitespace_handles_mixed_unicode_and_ascii() {
        let s = "  你 好   world\n\n  test\t";
        assert_eq!(normalize_whitespace(s), "你 好 world test");
    }

    #[test]
    fn normalize_whitespace_leaves_single_spaces_intact() {
        let s = "a b c";
        assert_eq!(normalize_whitespace(s), "a b c");
    }

    #[test]
    fn now_ms_is_monotonic_non_decreasing() {
        let a = now_ms();
        let b = now_ms();
        assert!(b >= a);
    }

    // --- send button timing helpers ---

    #[test]
    fn send_button_ready_max_wait_uses_char_count_for_unicode() {
        let short = "你".repeat(COMPOSER_PASTE_THRESHOLD_CHARS);
        let long = "你".repeat(COMPOSER_PASTE_THRESHOLD_CHARS + 1);
        assert_eq!(
            composer_text_injection_mode(&short),
            ComposerTextInjectionMode::InsertText
        );
        assert_eq!(
            composer_text_injection_mode(&long),
            ComposerTextInjectionMode::NativeClipboardPaste
        );
        assert_eq!(send_button_ready_max_wait(&short), Duration::from_secs(10));
        assert_eq!(
            send_button_ready_retry_max_wait(&short),
            Duration::from_secs(15)
        );
        assert!(send_button_ready_max_wait(&long) >= Duration::from_secs(30));
        assert_eq!(
            send_button_ready_retry_max_wait(&long),
            Duration::from_secs(60)
        );
    }

    // --- paste modifier helpers ---

    #[test]
    fn paste_modifier_matches_key_mapping() {
        let modifier = paste_modifier();
        let (key, _code, vk) = paste_modifier_key();
        if cfg!(target_os = "macos") {
            assert_eq!(modifier, 4);
            assert_eq!((key, vk), ("Meta", 91));
        } else {
            assert_eq!(modifier, 2);
            assert_eq!((key, vk), ("Control", 17));
        }
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use serde_json::{json, Value};

    // --- parse_send_sse additional edge cases ---

    #[test]
    fn parse_send_sse_sets_ids_from_later_event_when_initial_missing() {
        let sse = "data: {\"type\":\"delta\"}\n\n".to_string()
            + "data: {\"type\":\"final\",\"conversation_id\":\"c-123\",\"turn_exchange_id\":\"t-1\",\"message\":{\"id\":\"m-1\"}}\n\n";
        let parsed = parse_send_sse(&sse);
        assert_eq!(parsed.conversation_id.as_deref(), Some("c-123"));
        assert_eq!(parsed.turn_exchange_id.as_deref(), Some("t-1"));
        assert_eq!(
            parsed.event_types,
            vec!["delta".to_string(), "final".to_string()]
        );
        let mapping = parsed
            .sse_detail
            .as_ref()
            .and_then(|v| v.get("mapping"))
            .and_then(Value::as_object)
            .expect("mapping");
        assert!(mapping.contains_key("m-1"));
    }

    #[test]
    fn parse_send_sse_ignores_lines_without_data_prefix() {
        let sse = "event: message\n\ndata: {\"type\":\"delta\",\"conversation_id\":\"c1\"}\n\ncomment: ignore-me";
        let parsed = parse_send_sse(sse);
        assert_eq!(parsed.conversation_id.as_deref(), Some("c1"));
        assert_eq!(parsed.event_types, vec!["delta".to_string()]);
    }

    // --- format_send_diagnostic edge cases ---

    #[test]
    fn format_send_diagnostic_defaults_when_only_url_present() {
        let diag = json!({"url": "https://chatgpt.com/"});
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("page=https://chatgpt.com/"));
        assert!(s.contains("composer=NOT_FOUND"));
        assert!(s.contains("sendButton=NOT_FOUND"));
        assert!(!s.contains("busyElements="));
    }

    #[test]
    fn format_send_diagnostic_composer_ok_without_preview_uses_placeholder() {
        let diag = json!({
            "composerFound": true,
            "composerVisible": true,
            "composerDisabled": false,
            "composerHasText": true
        });
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("composer=OK"));
        assert!(s.contains("text=\"?\""));
    }

    #[test]
    fn format_send_diagnostic_send_button_disabled_when_visibility_missing() {
        let diag = json!({
            "composerFound": true,
            "composerVisible": true,
            "composerHasText": true,
            "sendButtonFound": true
        });
        let s = format_send_diagnostic(&diag);
        // sendButtonVisible defaults to false, so this is treated as HIDDEN even without explicit flag
        assert!(s.contains("sendButton=HIDDEN"));
    }

    #[test]
    fn format_send_diagnostic_skips_non_string_banners() {
        let diag = json!({
            "errorBanners": ["Too many requests", 123, null, true]
        });
        let s = format_send_diagnostic(&diag);
        assert!(s.contains("banner=\"Too many requests\""));
        // numeric / null / bool banners should be ignored
        assert_eq!(s.matches("banner=\"").count(), 1);
    }

    // --- target_page_* and auth flow heuristics ---

    #[test]
    fn target_page_error_uses_auth_required_for_auth_flow_pages() {
        let state = json!({
            "url": "https://auth0.openai.com/authorize",
            "title": "Log in - ChatGPT",
            "readyState": "complete"
        });
        let err = target_page_error(&state, Some("c1"));
        assert!(err.starts_with("auth_required:"));
    }

    #[test]
    fn target_page_error_uses_unknown_page_kind_when_missing() {
        let state = json!({
            "url": "https://chatgpt.com/c/c1",
            "readyState": "complete",
            "conversationId": "c1"
        });
        let err = target_page_error(&state, Some("c2"));
        assert!(err.contains("page_kind=unknown"));
    }

    #[test]
    fn target_page_matches_true_for_new_conversation_without_composer_requirement() {
        let state = json!({
            "readyState": "complete",
            "conversationId": null,
            "visibleComposerCount": 0
        });
        assert!(target_page_matches(&state, None, false));
        assert!(!target_page_matches(&state, Some("c1"), false));
    }

    #[test]
    fn target_page_is_terminal_mismatch_respects_ready_state() {
        let loading = json!({
            "readyState": "loading",
            "conversationId": "c1",
            "bodyText": "HTTP 500 Internal Server Error"
        });
        // Even with fatal error body, non-complete pages are not terminal
        assert!(!target_page_is_terminal_mismatch(&loading, Some("c2")));
    }

    #[test]
    fn page_state_is_auth_flow_page_false_for_regular_page() {
        let state = json!({
            "url": "https://chatgpt.com/c/abc",
            "title": "ChatGPT",
            "bodyText": "normal page"
        });
        assert!(!page_state_is_auth_flow_page(&state));
    }

    #[test]
    fn page_body_has_fatal_error_detects_case_insensitive_patterns() {
        let state = json!({"bodyText": "INTERNAL SERVER ERROR while loading"});
        assert!(page_body_has_fatal_error(&state));
    }

    #[test]
    fn page_body_has_fatal_error_false_when_body_missing() {
        let state = json!({});
        assert!(!page_body_has_fatal_error(&state));
    }

    #[test]
    fn should_retry_as_new_conversation_requires_browser_wrong_page_prefix() {
        assert!(!should_retry_as_new_conversation(
            "some_other_error: page_kind=new_conversation"
        ));
    }

    #[test]
    fn should_retry_as_new_conversation_true_for_homepage_url_suffix() {
        let err = "browser_wrong_page: expected_conversation_id=None actual_conversation_id=None page_kind=unknown url=https://chatgpt.com/";
        assert!(should_retry_as_new_conversation(err));
    }

    #[test]
    fn handoff_heartbeat_error_prefers_browser_exit_when_both_fail() {
        let err = handoff_heartbeat_error(true, true).expect("error");
        assert!(err.contains("browser exited"));
    }

    #[test]
    fn auth_required_page_error_handles_missing_optional_fields() {
        let state = json!({"url": "https://chatgpt.com/auth/login"});
        let err = auth_required_page_error(&state);
        assert!(err.contains("url=https://chatgpt.com/auth/login"));
        // title and readyState should still be present in formatted string
        assert!(err.contains("title=\"\""));
        assert!(err.contains("readyState="));
    }

    // --- URL & text helpers ---

    #[test]
    fn extract_conversation_id_from_url_returns_none_for_blank_segment() {
        let url = "https://chatgpt.com/c/   ";
        assert_eq!(extract_conversation_id_from_url(url), None);
    }

    #[test]
    fn extract_conversation_id_from_url_handles_hash_without_query() {
        let url = "https://chatgpt.com/c/conv-1#section";
        assert_eq!(
            extract_conversation_id_from_url(url).as_deref(),
            Some("conv-1")
        );
    }

    #[test]
    fn normalize_whitespace_handles_only_whitespace_string() {
        assert_eq!(normalize_whitespace("\n\t  \n"), "");
    }

    #[test]
    fn normalize_whitespace_preserves_inner_spaces_between_words() {
        assert_eq!(normalize_whitespace("foo   bar"), "foo bar");
    }

    #[test]
    fn now_ms_is_monotonic_over_multiple_calls() {
        let a = now_ms();
        let b = now_ms();
        let c = now_ms();
        assert!(b >= a);
        assert!(c >= b);
    }

    // --- send button timing helpers additional coverage ---

    #[test]
    fn send_button_ready_max_wait_short_unicode_is_fixed_10s() {
        let text = "测".repeat(50);
        assert_eq!(send_button_ready_max_wait(&text), Duration::from_secs(10));
    }

    #[test]
    fn send_button_ready_max_wait_clamps_very_large_prompts() {
        let text = "x".repeat(5_000_000);
        assert_eq!(send_button_ready_max_wait(&text), Duration::from_secs(180));
    }

    #[test]
    fn send_button_ready_retry_max_wait_insert_text_and_clipboard_modes() {
        let short = "abc";
        let long = &"x".repeat(COMPOSER_PASTE_THRESHOLD_CHARS + 5);
        assert_eq!(
            composer_text_injection_mode(short),
            ComposerTextInjectionMode::InsertText
        );
        assert_eq!(
            composer_text_injection_mode(long),
            ComposerTextInjectionMode::NativeClipboardPaste
        );
        assert_eq!(
            send_button_ready_retry_max_wait(short),
            Duration::from_secs(15)
        );
        assert_eq!(
            send_button_ready_retry_max_wait(long),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn page_state_is_auth_flow_page_matches_openai_auth_domain() {
        let state = json!({"url": "https://auth.openai.com/login"});
        assert!(page_state_is_auth_flow_page(&state));
    }

    #[test]
    fn target_page_matches_and_mismatch_for_specific_ids() {
        let state = json!({
            "readyState": "complete",
            "conversationId": "c-expected",
            "visibleComposerCount": 1
        });
        assert!(target_page_matches(&state, Some("c-expected"), true));
        assert!(!target_page_matches(&state, Some("other"), true));
    }

    #[test]
    fn target_page_is_terminal_mismatch_true_when_expected_some_actual_none_and_new_page() {
        let state = json!({
            "readyState": "complete",
            "conversationId": null,
            "pageKind": "new_conversation"
        });
        assert!(target_page_is_terminal_mismatch(&state, Some("c1")));
    }
}
