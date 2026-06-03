use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use dashmap::DashMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::external_cli::{
    ExternalCliAdapterConfig, ExternalCliAgentSettings, ExternalCliProgressEvent,
    ExternalCliProgressEventType, ExternalCliRunRequest, ExternalCliRunStatus,
};

mod artifacts;
mod browser;
mod diagnostics;
mod images;
mod interaction;
mod native;
mod send;
mod storage;
#[cfg(test)]
mod tests;

use artifacts::download_behavior_artifacts_for_conversation;
pub use browser::kill_all_managed_browsers;
use browser::{BrowserSession, CdpClient, CdpEvent};
use diagnostics::{
    capture_run_failure_artifacts, conversation_id_hint_from_request, write_run_failure_manifest,
};
use images::download_generated_images_for_conversation;
pub use interaction::{clear_session_conversation, session_conversation_exists};
use interaction::{
    conversation_id_from_params, event, extract_text_parts, get_conversation_detail,
    is_transient_conversation_read_error, requested_or_session_conversation,
    save_session_conversation, stop_requested, stopped_error, summarize_conversation_detail,
    summarize_conversations, try_waited_final_from_sse, wait_final, WaitedFinal,
};
use native::native_account_probe;
use send::send_with_browser;
#[cfg(test)]
use send::{
    composer_text_injection_mode, extract_conversation_id_from_url, handoff_heartbeat_error,
    handoff_page_heartbeat_error, page_state_is_auth_flow_page, parse_send_sse, paste_modifier,
    send_button_ready_max_wait, send_button_ready_retry_max_wait, should_retry_as_new_conversation,
    target_page_error, target_page_is_terminal_mismatch, target_page_matches,
    ComposerTextInjectionMode,
};
use storage::{read_auth_state, write_auth_state, write_redacted_json};

pub const ADAPTER_ID: &str = "chatgpt_web";
pub const STARTUP_AUTH_DRY_RUN_ENV: &str = "BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN";

const DEFAULT_BASE_URL: &str = "https://chatgpt.com";
const ACCOUNT_CHECK_PATH_PREFIX: &str = "/backend-api/accounts/check";
const CONVERSATIONS_PATH: &str =
    "/backend-api/conversations?offset=0&limit=20&order=updated&is_archived=false&is_starred=false";
const F_CONVERSATION_URL: &str = "https://chatgpt.com/backend-api/f/conversation";
const AUTH_EXPIRY_SKEW_SECS: i64 = 60;
const BROWSER_ACCOUNT_CHECK_PROOF_MAX_AGE_SECS: i64 = 15 * 60;
const BROWSER_ACCOUNT_CHECK_PROOF_FUTURE_SKEW_SECS: i64 = 60;
static LOGIN_SESSION_SEQ: AtomicU64 = AtomicU64::new(1);
static LOGIN_SESSIONS: OnceLock<DashMap<String, ActiveLoginSession>> = OnceLock::new();

// ── Per-session conversation history for context replay on conversation switch ──

static CONVERSATION_HISTORY: OnceLock<DashMap<String, Vec<ConversationHistoryEntry>>> =
    OnceLock::new();

// ── Per-session flag: marks sessions whose context may have been lost ──
// When 429/timeout occurs, the ChatGPT server may not retain the previous
// turn in context. We set this flag so the NEXT message includes a context
// replay prefix even within the same conversation.
static CONTEXT_MAY_BE_LOST: OnceLock<DashMap<String, bool>> = OnceLock::new();

fn context_lost_flags() -> &'static DashMap<String, bool> {
    CONTEXT_MAY_BE_LOST.get_or_init(DashMap::new)
}

/// Mark a session as having potentially lost context.
/// Currently only used by tests — production code handles context replay
/// inline in the `conversation_lost:` error path rather than via async flags.
#[cfg(test)]
fn mark_context_may_be_lost(session_key: &str) {
    context_lost_flags().insert(session_key.to_string(), true);
    info!(
        session_key,
        "chatgpt_web: marked session context_may_be_lost (next message will include history)"
    );
}

/// Check and clear the context-lost flag. Returns true if flag was set.
fn take_context_may_be_lost(session_key: &str) -> bool {
    context_lost_flags()
        .remove(session_key)
        .map(|(_, v)| v)
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
struct ConversationHistoryEntry {
    role: &'static str, // "user" or "assistant"
    content: String,
}

/// Max chars to keep per history entry (user or assistant).
/// When content exceeds this limit, we keep the head and tail portions
/// with a "[...裁剪了 N 字...]" marker in between, so context replay
/// retains both the opening setup and the final cliffhanger/conclusion.
const HISTORY_SUMMARY_MAX_CHARS: usize = 2000;

/// How much of the head / tail to preserve when truncating.
/// head gets 60%, tail gets 40% — openings usually carry more context.
const HISTORY_TRUNCATE_HEAD_RATIO: f64 = 0.6;

/// Max total chars for the context replay prefix (excluding new_message).
/// Keeps the prefix under ~15K chars to stay well within ProseMirror /
/// ChatGPT composer limits (~32K chars typically accepted).
const CONTEXT_REPLAY_MAX_TOTAL_CHARS: usize = 15_000;

fn conversation_history_store() -> &'static DashMap<String, Vec<ConversationHistoryEntry>> {
    CONVERSATION_HISTORY.get_or_init(DashMap::new)
}

/// Sanitize text before recording: strip local file paths, image markdown,
/// and replace uninformative image-only responses with a short marker.
fn sanitize_for_history(text: &str) -> String {
    let trimmed = text.trim();
    // Replace image-only placeholder responses
    if trimmed == "已生成图片，正在发送原图。"
        || trimmed == "已生成图片（图片下载失败，请手动查看对话）"
    {
        return "[此轮生成了图片]".to_string();
    }
    // Strip markdown image references like ![alt](path) that may contain local file paths
    let mut cleaned = String::with_capacity(trimmed.len());
    let mut rest = trimmed;
    while let Some(start) = rest.find("![") {
        cleaned.push_str(&rest[..start]);
        // Only strip if this is a valid image ref: ![...](...) pattern
        if let Some(bracket_end) = rest[start..].find("](") {
            if let Some(paren_end) = rest[start + bracket_end..].find(')') {
                rest = &rest[start + bracket_end + paren_end + 1..];
                continue;
            }
        }
        // Not a valid image ref — keep the "![" and advance past it
        cleaned.push_str("![");
        rest = &rest[start + 2..];
    }
    cleaned.push_str(rest);
    // Strip any remaining local file paths (e.g. /tmp/..., /Users/...)
    let result: String = cleaned
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.starts_with("/tmp/")
                && !t.starts_with("/Users/")
                && !t.starts_with("/var/")
                && !t.contains("[diagnostic_screenshot:")
        })
        .collect::<Vec<_>>()
        .join("\n");
    truncate_keep_head_tail(&result, HISTORY_SUMMARY_MAX_CHARS)
}

/// Truncate a string to `max_chars` by keeping the head and tail portions,
/// inserting a "[...裁剪了 N 字...]" marker in between.  If the string
/// fits within `max_chars`, it is returned as-is.
fn truncate_keep_head_tail(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    // Reserve ~30 chars for the marker itself
    let marker_reserve = 30;
    let usable = max_chars.saturating_sub(marker_reserve);
    let head_chars = ((usable as f64) * HISTORY_TRUNCATE_HEAD_RATIO) as usize;
    let tail_chars = usable.saturating_sub(head_chars);
    let trimmed_count = char_count - head_chars - tail_chars;

    let head: String = s.chars().take(head_chars).collect();
    let tail: String = s.chars().skip(char_count - tail_chars).collect();
    format!("{head}\n\n[...裁剪了 {trimmed_count} 字...]\n\n{tail}")
}

/// Record a user→assistant exchange for potential future context replay.
fn record_conversation_exchange(session_key: &str, user_msg: &str, assistant_msg: &str) {
    const MAX_HISTORY_ENTRIES: usize = 100;

    let mut history = conversation_history_store()
        .entry(session_key.to_string())
        .or_default();
    history.push(ConversationHistoryEntry {
        role: "user",
        content: truncate_keep_head_tail(user_msg, HISTORY_SUMMARY_MAX_CHARS),
    });
    let assistant_sanitized = sanitize_for_history(assistant_msg);
    if !assistant_sanitized.is_empty() {
        history.push(ConversationHistoryEntry {
            role: "assistant",
            content: assistant_sanitized,
        });
    }
    // Prevent unbounded memory growth while preserving both setup and continuity:
    // keep earliest entries plus the most recent entries, and drop the middle.
    if history.len() > MAX_HISTORY_ENTRIES {
        let keep_head = MAX_HISTORY_ENTRIES / 2;
        let keep_tail = MAX_HISTORY_ENTRIES - keep_head;
        let tail_start = history.len().saturating_sub(keep_tail);
        if keep_head < tail_start {
            history.drain(keep_head..tail_start);
        }
    }
    info!(
        session_key,
        entries = history.len(),
        "chatgpt_web: recorded conversation exchange for context replay"
    );
}

/// Build a context-prefixed message for sending to a new conversation after
/// the original conversation was lost.  Returns None if no history exists.
///
/// Strategy: keep the EARLIEST rounds (world-building / setup) and the LATEST
/// rounds (most recent plot / context), drop middle rounds when the total
/// exceeds `CONTEXT_REPLAY_MAX_TOTAL_CHARS`.  This ensures both the initial
/// premise and the immediate continuity are preserved.
fn build_context_replay_message(session_key: &str, new_message: &str) -> Option<String> {
    let store = conversation_history_store();
    let entries = store.get(session_key)?;
    if entries.is_empty() {
        return None;
    }
    let header = "[重要提示：由于技术原因，我们的对话历史丢失。以下是之前对话的摘要，请基于此继续任务。]\n\n## 之前的对话记录\n\n";
    let footer = "---\n\n**[继续任务]**\n\n";

    // ── Step 1: Format every round into a string chunk ──
    let mut rounds: Vec<String> = Vec::new();
    let mut i = 0;
    let mut round_num = 0u32;
    while i < entries.len() {
        let entry = &entries[i];
        let chunk = if entry.role == "user" {
            round_num += 1;
            let content_formatted = if entry.content.contains('\n') {
                format!(
                    "**第{}轮 - 用户**:\n> {}\n",
                    round_num,
                    entry.content.replace('\n', "\n> ")
                )
            } else {
                format!("**第{}轮 - 用户**: {}\n", round_num, entry.content)
            };
            if i + 1 < entries.len() && entries[i + 1].role == "assistant" {
                let assistant_content = &entries[i + 1].content;
                let assistant_formatted = if assistant_content.contains('\n') {
                    format!(
                        "**第{}轮 - 助手**:\n> {}\n\n",
                        round_num,
                        assistant_content.replace('\n', "\n> ")
                    )
                } else {
                    format!("**第{}轮 - 助手**: {}\n\n", round_num, assistant_content)
                };
                i += 2;
                format!("{}{}", content_formatted, assistant_formatted)
            } else {
                i += 1;
                content_formatted
            }
        } else {
            i += 1;
            format!("**助手**: {}\n\n", entry.content)
        };
        rounds.push(chunk);
    }

    let total_rounds = rounds.len();

    // ── Step 2: If everything fits, include all rounds ──
    let total_size: usize = rounds.iter().map(|r| r.len()).sum();
    if header.len() + total_size + footer.len() <= CONTEXT_REPLAY_MAX_TOTAL_CHARS {
        let mut buf =
            String::with_capacity(header.len() + total_size + footer.len() + new_message.len());
        buf.push_str(header);
        for r in &rounds {
            buf.push_str(r);
        }
        buf.push_str(footer);
        buf.push_str(new_message);
        return Some(buf);
    }

    // ── Step 3: Head-tail selection — keep early + recent, drop middle ──
    // Budget: total chars available for round content
    let budget = CONTEXT_REPLAY_MAX_TOTAL_CHARS
        .saturating_sub(header.len())
        .saturating_sub(footer.len())
        .saturating_sub(80); // reserve for the "omitted" marker

    // Greedily include rounds from the tail (most recent first), then
    // fill remaining budget from the head (earliest).
    // Tail gets priority (60%) because immediate context is more critical.
    let tail_budget = (budget as f64 * 0.6) as usize;
    let head_budget = budget.saturating_sub(tail_budget);

    // Select tail rounds (from the end, working backwards)
    let mut tail_start = total_rounds;
    let mut tail_used = 0usize;
    for idx in (0..total_rounds).rev() {
        let rlen = rounds[idx].len();
        if tail_used + rlen > tail_budget {
            break;
        }
        tail_used += rlen;
        tail_start = idx;
    }

    // Select head rounds (from the start)
    let mut head_end = 0;
    let mut head_used = 0usize;
    for (idx, round) in rounds.iter().enumerate().take(tail_start) {
        let rlen = round.len();
        if head_used + rlen > head_budget {
            break;
        }
        head_used += rlen;
        head_end = idx + 1;
    }

    // ── Step 4: Assemble ──
    let omitted = tail_start.saturating_sub(head_end);
    let mut buf = String::with_capacity(
        header.len() + head_used + tail_used + footer.len() + new_message.len() + 80,
    );
    buf.push_str(header);

    // Head rounds
    for r in &rounds[..head_end] {
        buf.push_str(r);
    }

    // Omission marker
    if omitted > 0 {
        buf.push_str(&format!(
            "...(已省略中间 {} 轮对话以控制消息长度，保留了最早 {} 轮和最近 {} 轮)\n\n",
            omitted,
            head_end,
            total_rounds - tail_start,
        ));
    }

    // Tail rounds
    for r in &rounds[tail_start..] {
        buf.push_str(r);
    }

    buf.push_str(footer);
    buf.push_str(new_message);
    Some(buf)
}

static PROFILE_LOCKS: OnceLock<DashMap<PathBuf, Arc<tokio::sync::Semaphore>>> = OnceLock::new();

fn profile_locks() -> &'static DashMap<PathBuf, Arc<tokio::sync::Semaphore>> {
    PROFILE_LOCKS.get_or_init(DashMap::new)
}

/// Acquire a per-profile semaphore to limit concurrent browser operations that
/// share the same --user-data-dir.  Each run gets its own isolated tab, so we
/// allow up to MAX_CONCURRENT_RUNS concurrent runs on the same browser profile.
/// The semaphore prevents unbounded tab creation which could overwhelm the
/// browser or trigger ChatGPT rate-limits.
const MAX_CONCURRENT_RUNS_PER_PROFILE: usize = 3;

async fn acquire_profile_lock(profile_dir: &Path) -> tokio::sync::OwnedSemaphorePermit {
    let sem = profile_locks()
        .entry(profile_dir.to_path_buf())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS_PER_PROFILE)))
        .clone();
    sem.acquire_owned().await.expect("profile semaphore closed")
}

static SESSION_LOCKS: OnceLock<DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>> = OnceLock::new();

fn session_locks() -> &'static DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>> {
    SESSION_LOCKS.get_or_init(DashMap::new)
}

#[derive(Debug)]
pub struct ChatGptWebRunOutput {
    pub status: ExternalCliRunStatus,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub response: String,
    /// Individual assistant message texts for per-message IM delivery.
    pub responses: Vec<String>,
    pub events: Vec<ExternalCliProgressEvent>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptWebAuthStatus {
    pub state: String,
    pub logged_in: bool,
    pub identity_complete: bool,
    pub account_check_ok: bool,
    pub account_status: Option<u16>,
    pub cookie_count: usize,
    pub captured_header_names: Vec<String>,
    pub profile_dir: String,
    pub state_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptWebStartupAuthStatus {
    pub runner_id: String,
    pub state: String,
    pub logged_in: bool,
    pub opened_login: bool,
    pub dry_run: bool,
    pub profile_dir: String,
    pub state_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatGptWebAdapterConfig {
    #[serde(default)]
    browser: BrowserConfig,
    #[serde(default)]
    chatgpt: ChatGptConfig,
    #[serde(default)]
    auth: AuthConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserConfig {
    #[serde(default = "default_browser_channel")]
    channel: String,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    profile_dir: Option<String>,
    #[serde(default = "default_open_on_auth_required")]
    open_on_auth_required: bool,
    #[serde(default)]
    keep_browser_open_after_login: bool,
    #[serde(default = "default_execution_mode")]
    execution_mode: String,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            channel: default_browser_channel(),
            executable: None,
            profile_dir: None,
            open_on_auth_required: true,
            keep_browser_open_after_login: false,
            execution_mode: default_execution_mode(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatGptConfig {
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_poll_interval_ms")]
    poll_interval_ms: u64,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
    /// Session consistency mode when hitting 429 rate limits.
    ///
    /// - `"strong"` (default): wait and retry the **same** conversation on the
    ///   **same** tab.  Never create a new conversation unless the original has
    ///   been deleted.  Each retry waits `rate_limit_retry_secs` (default 180s).
    /// - `"weak"`: if the page redirects to the homepage after a 429, continue
    ///   sending on the **current** tab (new conversation) without opening
    ///   additional tabs.
    #[serde(default = "default_session_consistency")]
    session_consistency: String,
    /// Seconds to wait before retrying after a 429 rate-limit in strong
    /// consistency mode.  Default 180 (3 minutes).
    #[serde(default = "default_rate_limit_retry_secs")]
    rate_limit_retry_secs: u64,
    /// Maximum number of retry attempts for 429 in strong consistency mode.
    #[serde(default = "default_rate_limit_max_retries")]
    rate_limit_max_retries: u32,
}

impl Default for ChatGptConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            poll_interval_ms: default_poll_interval_ms(),
            timeout_secs: default_timeout_secs(),
            session_consistency: default_session_consistency(),
            rate_limit_retry_secs: default_rate_limit_retry_secs(),
            rate_limit_max_retries: default_rate_limit_max_retries(),
        }
    }
}

impl ChatGptConfig {
    fn is_strong_consistency(&self) -> bool {
        self.session_consistency != "weak"
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthConfig {
    #[serde(default)]
    state_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthState {
    captured_at: String,
    base_url: String,
    user_agent: String,
    captured_auth_headers: BTreeMap<String, String>,
    captured_auth_identity: AuthorizationIdentity,
    #[serde(default)]
    captured_account_check: Option<BrowserAccountCheckProof>,
    cookies: Vec<BrowserCookie>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserAccountCheckProof {
    captured_at: String,
    status: u16,
    logged_in: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizationIdentity {
    has_bearer_token: bool,
    has_profile_email: bool,
    profile_email_verified: Option<bool>,
    has_user_identity: bool,
    has_account_identity: bool,
    complete: bool,
    expires_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserCookie {
    name: String,
    value: String,
    domain: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    secure: Option<bool>,
    #[serde(default)]
    http_only: Option<bool>,
    #[serde(default)]
    same_site: Option<String>,
    #[serde(default)]
    expires: Option<f64>,
}

#[derive(Clone, Debug)]
struct RuntimeConfig {
    browser: BrowserConfig,
    chatgpt: ChatGptConfig,
    profile_dir: PathBuf,
    state_path: PathBuf,
    sessions_path: PathBuf,
    attachments_dir: PathBuf,
}

#[derive(Clone)]
struct ActiveLoginSession {
    id: u64,
    stop: broadcast::Sender<()>,
}

struct LoginSessionGuard {
    key: String,
    id: u64,
}

impl Drop for LoginSessionGuard {
    fn drop(&mut self) {
        if let Some(entry) = login_sessions().get(&self.key) {
            if entry.id == self.id {
                drop(entry);
                login_sessions().remove(&self.key);
            }
        }
    }
}

pub async fn run_adapter(
    request: &ExternalCliRunRequest,
    prompt: &str,
    run_dir: &Path,
) -> Result<ChatGptWebRunOutput, String> {
    #[cfg(debug_assertions)]
    if std::env::var("BIFROST_CHATGPT_WEB_E2E_MOCK")
        .ok()
        .as_deref()
        == Some("1")
    {
        return Ok(mock_run_adapter_for_e2e(request, prompt));
    }

    let config = match runtime_config(&request.adapter_config) {
        Ok(config) => config,
        Err(error) => {
            write_run_failure_manifest(run_dir, "configure", None, &error, Vec::new()).await;
            return Err(error);
        }
    };
    let _profile_permit = acquire_profile_lock(&config.profile_dir).await;
    let operation = request.operation.trim();
    let operation = if operation.is_empty() {
        "ask"
    } else {
        operation
    };
    let stop_marker_path = run_dir.join("stop_requested");

    let mut events = vec![event(
        ExternalCliProgressEventType::RunStarted,
        "ChatGPT Web run started",
        Some("ChatGPT Web"),
        json!({"operation": operation}),
    )];

    let auth = match ensure_authenticated(&config, true).await {
        Ok(auth) => auth,
        Err(error) => {
            write_run_failure_manifest(run_dir, operation, None, &error, Vec::new()).await;
            return Err(error);
        }
    };
    let auth_status = match auth_status_from_state(&config, &auth).await {
        Ok(status) => status,
        Err(error) => {
            capture_run_failure_artifacts(&config, &auth, run_dir, operation, None, &error).await;
            return Err(error);
        }
    };
    if let Err(error) = write_redacted_json(
        &run_dir.join("auth_probe.json"),
        &json!({
            "state": auth_status.state,
            "loggedIn": auth_status.logged_in,
            "identityComplete": auth_status.identity_complete,
            "accountCheckOk": auth_status.account_check_ok,
            "accountStatus": auth_status.account_status,
            "cookieCount": auth_status.cookie_count,
            "capturedHeaderNames": auth_status.captured_header_names,
        }),
    )
    .await
    {
        capture_run_failure_artifacts(&config, &auth, run_dir, operation, None, &error).await;
        return Err(error);
    }

    let mut diagnostic_conversation_id = conversation_id_hint_from_request(request);
    let result = run_authenticated_operation(
        &config,
        &auth,
        request,
        prompt,
        run_dir,
        operation,
        &stop_marker_path,
        &mut diagnostic_conversation_id,
    )
    .await;
    if let Err(error) = &result {
        capture_run_failure_artifacts(
            &config,
            &auth,
            run_dir,
            operation,
            diagnostic_conversation_id.as_deref(),
            error,
        )
        .await;
    }
    let result = result?;

    let response_events = chatgpt_web_response_events(&result.0, &result.1, &result.2);
    events.extend(response_events);
    events.push(event(
        ExternalCliProgressEventType::RunFinished,
        "ChatGPT Web run finished",
        Some("ChatGPT Web"),
        json!({"status": "succeeded"}),
    ));
    let mut metadata = BTreeMap::new();
    if let Some(conversation_id) = result
        .1
        .get("conversationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("conversationId".to_string(), conversation_id.to_string());
    }

    Ok(ChatGptWebRunOutput {
        status: ExternalCliRunStatus::Succeeded,
        exit_code: Some(0),
        stdout: serde_json::to_vec(&json!({"response": result.0})).unwrap_or_default(),
        stderr: Vec::new(),
        responses: result.2,
        response: result.0,
        events,
        metadata,
    })
}

#[cfg(debug_assertions)]
fn mock_run_adapter_for_e2e(request: &ExternalCliRunRequest, prompt: &str) -> ChatGptWebRunOutput {
    let conversation_id = conversation_id_hint_from_request(request)
        .unwrap_or_else(|| format!("mock-conversation-{}", uuid::Uuid::new_v4()));
    let response = format!("chatgpt_web_e2e_mock: {}", prompt.trim());
    let mut metadata = BTreeMap::new();
    metadata.insert("conversationId".to_string(), conversation_id.clone());
    ChatGptWebRunOutput {
        status: ExternalCliRunStatus::Succeeded,
        exit_code: Some(0),
        stdout: serde_json::to_vec(&json!({
            "response": response,
            "conversationId": conversation_id
        }))
        .unwrap_or_default(),
        stderr: Vec::new(),
        response: response.clone(),
        responses: vec![response],
        events: vec![
            event(
                ExternalCliProgressEventType::RunStarted,
                "ChatGPT Web E2E mock run started",
                Some("ChatGPT Web"),
                json!({"operation": request.operation}),
            ),
            event(
                ExternalCliProgressEventType::RunFinished,
                "ChatGPT Web E2E mock run finished",
                Some("ChatGPT Web"),
                json!({"status": "succeeded"}),
            ),
        ],
        metadata,
    }
}

fn chatgpt_web_response_events(
    response: &str,
    raw: &Value,
    responses: &[String],
) -> Vec<ExternalCliProgressEvent> {
    let parts = responses
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let tool_events = chatgpt_web_tool_events(raw);
    if parts.len() <= 1 {
        let mut events = tool_events;
        events.push(event(
            ExternalCliProgressEventType::AssistantFinal,
            response.trim().to_string(),
            Some("ChatGPT Web"),
            with_phase(raw, "final", 0),
        ));
        return events;
    }
    let last_index = parts.len().saturating_sub(1);
    let mut events = parts
        .into_iter()
        .enumerate()
        .map(|(index, content)| {
            if index == last_index {
                event(
                    ExternalCliProgressEventType::AssistantFinal,
                    content,
                    Some("ChatGPT Web"),
                    with_phase(raw, "final", index),
                )
            } else {
                event(
                    ExternalCliProgressEventType::AssistantDelta,
                    content,
                    Some("ChatGPT Web"),
                    with_phase(raw, "thinking", index),
                )
            }
        })
        .collect::<Vec<_>>();
    events.splice(last_index..last_index, tool_events);
    events
}

fn chatgpt_web_tool_events(raw: &Value) -> Vec<ExternalCliProgressEvent> {
    raw.get("toolCalls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, tool)| {
            let content = tool
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| tool.get("name").and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                return None;
            }
            Some(event(
                ExternalCliProgressEventType::ToolFinished,
                content.to_string(),
                tool.get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .or(Some("ChatGPT Web tool")),
                with_phase(tool, "tool", index),
            ))
        })
        .collect()
}

fn with_phase(raw: &Value, phase: &str, batch_index: usize) -> Value {
    let mut value = raw.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("phase".to_string(), json!(phase));
        object.insert("batchIndex".to_string(), json!(batch_index));
    }
    value
}

#[allow(clippy::too_many_arguments)]
async fn run_authenticated_operation(
    config: &RuntimeConfig,
    auth: &AuthState,
    request: &ExternalCliRunRequest,
    prompt: &str,
    run_dir: &Path,
    operation: &str,
    stop_marker_path: &Path,
    diagnostic_conversation_id: &mut Option<String>,
) -> Result<(String, Value, Vec<String>), String> {
    let result = match operation {
        "list" => {
            let conversations = chatgpt_json(config, auth, CONVERSATIONS_PATH).await?;
            let summary = summarize_conversations(&conversations);
            let response = serde_json::to_string_pretty(&summary)
                .map_err(|error| format!("serialize conversations failed: {error}"))?;
            (
                response.clone(),
                json!({"operation": "list", "conversations": summary}),
                vec![response],
            )
        }
        "get" => {
            let conversation_id = conversation_id_from_params(request)?;
            *diagnostic_conversation_id = Some(conversation_id.clone());
            let detail = get_conversation_detail(config, auth, &conversation_id).await?;
            let summary = summarize_conversation_detail(&detail);
            let response = serde_json::to_string_pretty(&summary)
                .map_err(|error| format!("serialize conversation failed: {error}"))?;
            (
                response.clone(),
                json!({"operation": "get", "conversation": summary}),
                vec![response],
            )
        }
        "wait" => {
            let conversation_id = conversation_id_from_params(request)?;
            *diagnostic_conversation_id = Some(conversation_id.clone());
            let waited = wait_final(
                config,
                auth,
                &conversation_id,
                None,
                Duration::from_secs(config.chatgpt.timeout_secs),
                stop_marker_path,
                Some(&config.profile_dir),
            )
            .await?;
            let mut response = waited
                .final_message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let downloaded_artifacts = match download_chatgpt_behavior_artifacts(
                config,
                auth,
                &conversation_id,
                &waited,
                run_dir,
                stop_marker_path,
            )
            .await
            {
                Ok(items) => items,
                Err(error) => {
                    warn!(
                        conversation_id = %conversation_id,
                        error = %error,
                        "chatgpt_web wait: behavior artifact download failed"
                    );
                    if response.is_empty() {
                        response = "ChatGPT Web 附件下载失败，请手动查看原对话。".to_string();
                    } else {
                        response.push_str("\n\n[ChatGPT Web 附件下载失败，请手动查看原对话]");
                    }
                    Vec::new()
                }
            };
            append_downloaded_behavior_artifact_links(&mut response, &downloaded_artifacts);
            let responses = chatgpt_web_responses_for_delivery(
                &response,
                &waited.all_texts,
                !downloaded_artifacts.is_empty(),
            );
            (
                response,
                json!({
                    "operation": "wait",
                    "conversation": waited.summary,
                    "downloadedArtifacts": downloaded_artifacts,
                }),
                responses,
            )
        }
        "create" | "send" | "ask" => {
            let message = prompt.trim();
            if message.is_empty() {
                return Err("message cannot be empty".to_string());
            }
            let phase_start = std::time::Instant::now();
            let conversation_id = requested_or_session_conversation(request, config).await?;
            *diagnostic_conversation_id = conversation_id.clone();

            // Try sending in the existing conversation first.  If it hits a 429
            // rate-limit and we were continuing an existing conversation, fall
            // back to a brand-new conversation automatically.
            let fell_back_to_new = false;

            // ── Context replay for same-conversation context loss ──
            // If the previous round had 429/timeout, ChatGPT's server-side context
            // may be corrupted.  Prepend conversation history to this message so
            // the AI can "see" what happened before.
            let context_replayed = request
                .session_key
                .as_deref()
                .map(take_context_may_be_lost)
                .unwrap_or(false);
            let effective_message = if context_replayed {
                let replay = request
                    .session_key
                    .as_deref()
                    .and_then(|sk| build_context_replay_message(sk, message));
                if let Some(ref replay_msg) = replay {
                    info!(
                        session_key = request.session_key.as_deref().unwrap_or("<none>"),
                        original_len = message.len(),
                        replay_len = replay_msg.len(),
                        "chatgpt_web ask: context_may_be_lost flag set, prepending history to message"
                    );
                }
                replay.unwrap_or_else(|| message.to_string())
            } else {
                message.to_string()
            };

            let ask_result = ask_send_and_wait(
                config,
                auth,
                conversation_id.as_deref(),
                &effective_message,
                stop_marker_path,
                run_dir,
            )
            .await;

            let (send_elapsed, final_conversation_id, waited) = match ask_result {
                Ok((elapsed, cid, w)) => {
                    // DOM fallback or 429-then-success means we read the
                    // response via an alternative method, but ChatGPT's
                    // server-side context is intact — the message WAS
                    // processed.  Do NOT mark context_may_be_lost here.
                    // Marking on every DOM fallback causes an infinite loop:
                    //   DOM fallback → mark → next turn prepends history →
                    //   429 again → DOM fallback → mark → ...
                    if w.had_429_or_fallback {
                        info!(
                            session_key = request.session_key.as_deref().unwrap_or("<none>"),
                            "chatgpt_web ask: response obtained via DOM fallback, but server-side \
                             context is intact — NOT marking context_may_be_lost"
                        );
                    }
                    (elapsed, cid, w)
                }
                Err(error) if error.contains("conversation_lost:") => {
                    // Conversation was lost (redirect/429 recovery failed in strong
                    // consistency mode).  Retry as a NEW conversation with all
                    // previous conversation history prepended to the message so
                    // the AI retains context continuity.
                    let session_key = request.session_key.as_deref();
                    let effective_message = session_key
                        .and_then(|sk| build_context_replay_message(sk, message))
                        .unwrap_or_else(|| message.to_string());
                    let has_context = effective_message.len() > message.len();
                    warn!(
                        session_key = session_key.unwrap_or("<none>"),
                        original_conversation_id = ?conversation_id,
                        has_context,
                        history_chars = effective_message.len(),
                        original_error = %error,
                        "chatgpt_web ask: conversation lost, retrying with context replay in new conversation"
                    );
                    let replay_result = ask_send_and_wait(
                        config,
                        auth,
                        None, // new conversation
                        &effective_message,
                        stop_marker_path,
                        run_dir,
                    )
                    .await?;
                    // NOTE: Do NOT clear conversation history here.  If the new
                    // conversation is also lost later, we still need the full
                    // history for the next context replay.  Memory impact is
                    // minimal (~500 chars × 2 entries per round).
                    replay_result
                }
                Err(error) if conversation_id.is_some() && error.contains("HTTP 429") => {
                    // The message was already sent and wait_final hit 429.
                    // The AI response is still generating in the background.
                    // In BOTH consistency modes, we should NOT re-send the
                    // message — just wait and retry wait_final.
                    let cooldown_secs = if config.chatgpt.is_strong_consistency() {
                        config.chatgpt.rate_limit_retry_secs
                    } else {
                        15u64
                    };
                    let mode = if config.chatgpt.is_strong_consistency() {
                        "strong"
                    } else {
                        "weak"
                    };
                    warn!(
                        original_conversation_id = ?conversation_id,
                        error = %error,
                        cooldown_secs,
                        mode,
                        "chatgpt_web ask [{mode}]: 429 during wait_final, waiting {cooldown_secs}s then retrying wait_final only (no re-send, no new tab)"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(cooldown_secs)).await;
                    let cid = conversation_id.as_deref().unwrap();
                    let waited = wait_final(
                        config,
                        auth,
                        cid,
                        None, // no after_time filter — pick up whatever is there
                        Duration::from_secs(config.chatgpt.timeout_secs),
                        stop_marker_path,
                        Some(&config.profile_dir),
                    )
                    .await?;
                    // The 429 was a transient read error.  Since wait_final
                    // succeeded after cooldown, the message WAS processed
                    // by ChatGPT — server-side context is intact regardless
                    // of whether we read the response via API or DOM fallback.
                    // Do NOT mark context_may_be_lost — doing so causes an
                    // infinite replay loop on subsequent turns.
                    if waited.had_429_or_fallback {
                        info!(
                            session_key = request.session_key.as_deref().unwrap_or("<none>"),
                            "chatgpt_web ask: 429 retry succeeded via DOM fallback, \
                             server context intact — NOT marking context_may_be_lost"
                        );
                    }
                    (phase_start.elapsed(), cid.to_string(), waited)
                }
                Err(error) if error.contains("DOM fallback") => {
                    // Request failed and error mentions DOM fallback.
                    // Since the request failed entirely, the caller will
                    // retry — no need to mark context_may_be_lost here.
                    // (This branch is likely dead code: DOM fallback
                    // success returns Ok, not Err.)
                    warn!(
                        error = %error,
                        "chatgpt_web ask: request failed with DOM fallback error"
                    );
                    return Err(error);
                }
                Err(error) => return Err(error),
            };

            *diagnostic_conversation_id = Some(final_conversation_id.clone());
            if let Some(session_key) = request.session_key.as_deref() {
                save_session_conversation(config, session_key, &final_conversation_id).await?;
            }

            // Record conversation exchange for future context replay (in case
            // a later round loses the conversation and needs to replay history).
            let response_text_for_history = waited
                .final_message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(session_key) = request.session_key.as_deref() {
                record_conversation_exchange(session_key, message, response_text_for_history);
            }

            let total_ms = phase_start.elapsed().as_millis() as u64;
            info!(
                send_ms = send_elapsed.as_millis() as u64,
                total_ms, fell_back_to_new, "chatgpt_web ask: complete"
            );

            write_redacted_json(&run_dir.join("conversation_final.json"), &waited.summary).await?;
            let mut response = waited
                .final_message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let generated_image_count = waited
                .final_message
                .get("generatedImages")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or_default();
            let downloaded_images = if generated_image_count > 0 {
                let generated_images = waited
                    .final_message
                    .get("generatedImages")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let requested_image_count_hint = requested_image_count_hint(message);
                match download_generated_images_for_conversation(
                    config,
                    auth,
                    &final_conversation_id,
                    generated_images,
                    requested_image_count_hint,
                    stop_marker_path,
                )
                .await
                {
                    Ok(images) => {
                        write_redacted_json(
                            &run_dir.join("generated_images.json"),
                            &json!({
                                "conversationId": final_conversation_id,
                                "images": images,
                            }),
                        )
                        .await?;
                        images
                    }
                    Err(dl_error) => {
                        // Graceful degradation: image download failed but the AI
                        // DID respond successfully. Return text response with an
                        // error note rather than failing the entire operation.
                        // This prevents the caller from re-sending the message
                        // (which would create a duplicate conversation turn).
                        warn!(
                            error = %dl_error,
                            conversation_id = %final_conversation_id,
                            image_count = generated_image_count,
                            "chatgpt_web ask: image download failed, degrading to text-only response"
                        );
                        if response.is_empty() {
                            response = "已生成图片（图片下载失败，请手动查看对话）".to_string();
                        } else {
                            response.push_str("\n\n[图片下载失败，请手动查看对话]");
                        }
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            if !downloaded_images.is_empty() {
                if response.is_empty() {
                    response = "已生成图片，正在发送原图。".to_string();
                }
                for (index, image) in downloaded_images.iter().enumerate() {
                    if let Some(path) = image.get("path").and_then(Value::as_str) {
                        response.push_str(&format!(
                            "\n\n![ChatGPT 生成图片 {}]({})",
                            index + 1,
                            path
                        ));
                    }
                }
            }
            let downloaded_artifacts = match download_chatgpt_behavior_artifacts(
                config,
                auth,
                &final_conversation_id,
                &waited,
                run_dir,
                stop_marker_path,
            )
            .await
            {
                Ok(items) => items,
                Err(error) => {
                    warn!(
                        conversation_id = %final_conversation_id,
                        error = %error,
                        "chatgpt_web ask: behavior artifact download failed"
                    );
                    if response.is_empty() {
                        response = "ChatGPT Web 附件下载失败，请手动查看原对话。".to_string();
                    } else {
                        response.push_str("\n\n[ChatGPT Web 附件下载失败，请手动查看原对话]");
                    }
                    Vec::new()
                }
            };
            append_downloaded_behavior_artifact_links(&mut response, &downloaded_artifacts);
            // Prepend a notice if we fell back to a new conversation.
            if fell_back_to_new {
                let notice = "[由于原对话触发频率限制，已通过新建对话重新发送消息]\n\n";
                response = format!("{notice}{response}");
            }
            let responses = chatgpt_web_responses_for_delivery(
                &response,
                &waited.all_texts,
                generated_image_count > 0 || fell_back_to_new || !downloaded_artifacts.is_empty(),
            );
            (
                response,
                json!({
                    "operation": operation,
                    "conversationId": final_conversation_id,
                    "fellBackToNew": fell_back_to_new,
                    "final": waited.summary,
                    "toolCalls": waited.summary.get("toolCalls").cloned().unwrap_or_else(|| json!([])),
                    "artifacts": waited.summary.get("artifacts").cloned().unwrap_or_else(|| json!([])),
                    "generatedImages": downloaded_images,
                    "downloadedArtifacts": downloaded_artifacts,
                }),
                responses,
            )
        }
        other => return Err(format!("unsupported chatgpt_web operation '{other}'")),
    };

    Ok(result)
}

fn chatgpt_behavior_artifacts(waited: &WaitedFinal) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut artifacts = Vec::new();
    for value in [&waited.final_message, &waited.summary] {
        let Some(items) = value.get("artifacts").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let label = item
                .get("label")
                .or_else(|| item.get("buttonText"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if label.is_empty() {
                continue;
            }
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("artifact");
            if seen.insert(format!("{kind}\n{label}")) {
                artifacts.push(item.clone());
            }
        }
    }
    artifacts
}

async fn download_chatgpt_behavior_artifacts(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
    waited: &WaitedFinal,
    run_dir: &Path,
    stop_marker_path: &Path,
) -> Result<Vec<Value>, String> {
    let artifacts = chatgpt_behavior_artifacts(waited);
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }
    let downloaded = download_behavior_artifacts_for_conversation(
        config,
        auth,
        conversation_id,
        &artifacts,
        stop_marker_path,
    )
    .await?;
    if !downloaded.is_empty() {
        write_redacted_json(
            &run_dir.join("behavior_artifacts.json"),
            &json!({
                "conversationId": conversation_id,
                "artifacts": downloaded,
            }),
        )
        .await?;
    }
    Ok(downloaded)
}

fn append_downloaded_behavior_artifact_links(response: &mut String, artifacts: &[Value]) {
    if artifacts.is_empty() {
        return;
    }
    let failure_items = artifacts
        .iter()
        .filter(|artifact| {
            artifact.get("kind").and_then(Value::as_str) == Some("download_failures")
        })
        .flat_map(|artifact| {
            artifact
                .get("failures")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect::<Vec<_>>();
    let images = artifacts
        .iter()
        .filter(|artifact| artifact.get("kind").and_then(Value::as_str) == Some("image"))
        .collect::<Vec<_>>();
    let attachments = artifacts
        .iter()
        .filter(|artifact| {
            let kind = artifact.get("kind").and_then(Value::as_str);
            kind != Some("image") && kind != Some("download_failures")
        })
        .collect::<Vec<_>>();

    if !images.is_empty() {
        if response.trim().is_empty() {
            response.push_str("已自动下载 ChatGPT Web 图片：");
        }
        for artifact in images {
            let label = artifact
                .get("label")
                .or_else(|| artifact.get("fileName"))
                .and_then(Value::as_str)
                .unwrap_or("ChatGPT Web 图片");
            if let Some(path) = artifact.get("path").and_then(Value::as_str) {
                response.push_str(&format!("\n\n![{label}]({path})"));
            }
        }
    }
    if !attachments.is_empty() {
        if response.trim().is_empty() {
            response.push_str("已自动下载 ChatGPT Web 附件：");
        } else {
            response.push_str("\n\n已自动下载 ChatGPT Web 附件：");
        }
        for artifact in attachments {
            let label = artifact
                .get("label")
                .or_else(|| artifact.get("fileName"))
                .and_then(Value::as_str)
                .unwrap_or("ChatGPT Web 附件");
            if let Some(path) = artifact.get("path").and_then(Value::as_str) {
                response.push_str(&format!("\n- [{label}]({path})"));
            } else {
                response.push_str(&format!("\n- {label}"));
            }
        }
    }
    if !failure_items.is_empty() {
        if response.trim().is_empty() {
            response.push_str("部分 ChatGPT Web 附件未能自动下载：");
        } else {
            response.push_str("\n\n部分 ChatGPT Web 附件未能自动下载：");
        }
        for failure in failure_items {
            let kind = failure
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("artifact");
            let label = failure
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("未命名附件");
            let error = failure
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            response.push_str(&format!(
                "\n- {}: {} ({})",
                artifact_kind_label(kind),
                label,
                truncate_for_log(error, 120)
            ));
        }
    }
}

fn artifact_kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "图片",
        "archive" => "压缩包",
        _ => "附件",
    }
}

fn chatgpt_web_responses_for_delivery(
    response: &str,
    all_texts: &[String],
    force_single_response: bool,
) -> Vec<String> {
    if force_single_response {
        return vec![response.trim().to_string()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect();
    }
    let responses = all_texts
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if responses.is_empty() {
        vec![response.trim().to_string()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect()
    } else {
        responses
    }
}

fn requested_image_count_hint(message: &str) -> Option<usize> {
    let digit_count = Regex::new(r"(?i)(\d{1,2})\s*(?:张|个|幅|枚|images?|pictures?|photos?)")
        .ok()
        .and_then(|regex| {
            regex
                .captures(message)
                .and_then(|captures| captures.get(1))
                .and_then(|value| value.as_str().parse::<usize>().ok())
        });
    let count = digit_count.or_else(|| {
        Regex::new(r"(十|[一二两三四五六七八九])\s*(?:张|个|幅|枚)")
            .ok()
            .and_then(|regex| {
                regex
                    .captures(message)
                    .and_then(|captures| captures.get(1))
                    .and_then(|value| chinese_image_count(value.as_str()))
            })
    })?;
    (1..=50).contains(&count).then_some(count)
}

fn chinese_image_count(value: &str) -> Option<usize> {
    Some(match value {
        "一" => 1,
        "二" | "两" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "七" => 7,
        "八" => 8,
        "九" => 9,
        "十" => 10,
        _ => return None,
    })
}

/// Encapsulates the send + wait_final flow for a single attempt.
/// Returns (send_elapsed, final_conversation_id, waited_final).
async fn ask_send_and_wait(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: Option<&str>,
    message: &str,
    stop_marker_path: &Path,
    run_dir: &Path,
) -> Result<(std::time::Duration, String, WaitedFinal), String> {
    let phase_start = std::time::Instant::now();
    let after_time = now_secs_f64() - 5.0;
    let send = send_with_browser(config, auth, conversation_id, message, stop_marker_path).await?;
    let send_elapsed = phase_start.elapsed();
    info!(
        send_ms = send_elapsed.as_millis() as u64,
        conversation_id = ?send.conversation_id,
        "chatgpt_web ask: send phase complete"
    );
    if stop_requested(stop_marker_path).await {
        return Err(stopped_error());
    }
    let final_conversation_id = send
        .conversation_id
        .clone()
        .or_else(|| conversation_id.map(String::from))
        .ok_or_else(|| "missing conversation id from ChatGPT Web handoff".to_string())?;
    write_redacted_json(
        &run_dir.join("conversation_handoff.json"),
        &json!({
            "conversationId": final_conversation_id,
            "turnExchangeId": send.turn_exchange_id,
            "eventTypes": send.event_types,
        }),
    )
    .await?;
    if send
        .event_types
        .iter()
        .any(|event_type| event_type == "handoff_recovered_after_sse_interrupt")
        && !has_handoff_submission_evidence(&send.event_types)
    {
        if let Err(error) = wait_user_message_visible(
            config,
            auth,
            &final_conversation_id,
            message,
            after_time,
            Duration::from_secs(10),
            stop_marker_path,
        )
        .await
        {
            let fetch_error = send
                .event_types
                .iter()
                .find(|event_type| event_type.starts_with("browser_fetch_failed:"));
            if let Some(fetch_error) = fetch_error {
                return Err(format!("{error}; {fetch_error}"));
            }
            return Err(error);
        }
    }
    // If the SSE stream completed and contains the full conversation detail,
    // construct WaitedFinal directly — no API polling needed.  This is the
    // primary path and avoids triggering 429 rate limits from extra requests.
    if let Some(ref sse_detail) = send.sse_detail {
        if let Some(waited) = try_waited_final_from_sse(sse_detail, Some(after_time)) {
            info!(
                conversation_id = %final_conversation_id,
                "chatgpt_web ask: using SSE stream data directly (no API polling)"
            );
            return Ok((send_elapsed, final_conversation_id, waited));
        }
        info!(
            conversation_id = %final_conversation_id,
            "chatgpt_web ask: SSE detail incomplete, falling back to DOM/API wait"
        );
    }
    let waited = wait_final(
        config,
        auth,
        &final_conversation_id,
        Some(after_time),
        Duration::from_secs(config.chatgpt.timeout_secs),
        stop_marker_path,
        Some(&config.profile_dir),
    )
    .await?;
    Ok((send_elapsed, final_conversation_id, waited))
}

pub async fn auth_status(
    settings: &ExternalCliAgentSettings,
) -> Result<ChatGptWebAuthStatus, String> {
    let config = runtime_config(&settings.adapter_config)?;
    match read_auth_state(&config.state_path).await {
        Ok(state) => auth_status_from_state(&config, &state).await,
        Err(error) => Ok(ChatGptWebAuthStatus {
            state: "logged_out".to_string(),
            logged_in: false,
            identity_complete: false,
            account_check_ok: false,
            account_status: None,
            cookie_count: 0,
            captured_header_names: Vec::new(),
            profile_dir: config.profile_dir.display().to_string(),
            state_path: config.state_path.display().to_string(),
            message: Some(error),
        }),
    }
}

pub async fn open_login(
    settings: &ExternalCliAgentSettings,
) -> Result<ChatGptWebAuthStatus, String> {
    let config = runtime_config(&settings.adapter_config)?;
    let state = open_login_and_capture(&config).await?;
    auth_status_from_state(&config, &state).await
}

pub async fn stop_login(
    settings: &ExternalCliAgentSettings,
) -> Result<ChatGptWebAuthStatus, String> {
    let config = runtime_config(&settings.adapter_config)?;
    if let Some(session) = login_sessions().get(&login_session_key(&config)) {
        let _ = session.stop.send(());
    }
    auth_status(settings).await
}

pub async fn ensure_startup_auth_ready(
    runner_id: &str,
    settings: &ExternalCliAgentSettings,
) -> Result<ChatGptWebStartupAuthStatus, String> {
    let status = auth_status(settings).await?;
    if status.logged_in {
        return Ok(startup_auth_status_from_auth(
            runner_id, status, false, false,
        ));
    }

    if startup_auth_dry_run_enabled() {
        info!(
            runner_id,
            state_path = %status.state_path,
            "chatgpt_web startup auth: login required; dry run would open login browser"
        );
        let reason = status
            .message
            .clone()
            .unwrap_or_else(|| status.state.clone());
        return Ok(startup_auth_status_from_auth(
            runner_id,
            ChatGptWebAuthStatus {
                message: Some(format!(
                    "startup auth dry run: login required; would open login browser ({})",
                    reason
                )),
                ..status
            },
            false,
            true,
        ));
    }

    info!(
        runner_id,
        state_path = %status.state_path,
        "chatgpt_web startup auth: login required; opening login browser"
    );
    let opened = open_login(settings).await?;
    Ok(startup_auth_status_from_auth(
        runner_id, opened, true, false,
    ))
}

fn startup_auth_status_from_auth(
    runner_id: &str,
    status: ChatGptWebAuthStatus,
    opened_login: bool,
    dry_run: bool,
) -> ChatGptWebStartupAuthStatus {
    ChatGptWebStartupAuthStatus {
        runner_id: runner_id.to_string(),
        state: status.state,
        logged_in: status.logged_in,
        opened_login,
        dry_run,
        profile_dir: status.profile_dir,
        state_path: status.state_path,
        message: status.message,
    }
}

fn startup_auth_dry_run_enabled() -> bool {
    std::env::var(STARTUP_AUTH_DRY_RUN_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

async fn ensure_authenticated(
    config: &RuntimeConfig,
    open_on_required: bool,
) -> Result<AuthState, String> {
    match read_auth_state(&config.state_path).await {
        Ok(state) => {
            let status = auth_status_from_state(config, &state).await?;
            if status.logged_in {
                return Ok(state);
            }
            if !open_on_required || !config.browser.open_on_auth_required {
                return Err(format!(
                    "auth_required: ChatGPT Web login is not valid: {}",
                    status.message.unwrap_or(status.state)
                ));
            }
        }
        Err(error) => {
            if !open_on_required || !config.browser.open_on_auth_required {
                return Err(format!("auth_required: {error}"));
            }
        }
    }
    open_login_and_capture(config).await
}

async fn auth_status_from_state(
    config: &RuntimeConfig,
    state: &AuthState,
) -> Result<ChatGptWebAuthStatus, String> {
    let native = native_account_probe(config, state).await;
    let identity_issue = authorization_identity_issue(&state.captured_auth_identity);
    let captured_account_check_stale_message = state
        .captured_account_check
        .as_ref()
        .filter(|proof| proof.logged_in && (200..300).contains(&proof.status))
        .and_then(|proof| {
            if browser_account_check_proof_is_fresh(proof) {
                None
            } else {
                Some(format!(
                    "captured browser accounts/check proof is stale or has an invalid timestamp: {}",
                    proof.captured_at
                ))
            }
        });
    let captured_account_check_ok = state.captured_account_check.as_ref().is_some_and(|proof| {
        proof.logged_in
            && (200..300).contains(&proof.status)
            && browser_account_check_proof_is_fresh(proof)
    });
    let (account_check_ok, account_status, message) = match native {
        Ok(probe) if probe.logged_in => (true, Some(probe.status), None),
        Ok(probe) if captured_account_check_ok => (
            true,
            Some(probe.status),
            Some(format!(
                "native accounts/check returned {}; using captured browser accounts/check proof",
                probe.status
            )),
        ),
        Ok(probe) => (
            false,
            Some(probe.status),
            captured_account_check_stale_message,
        ),
        Err(error) if captured_account_check_ok => (
            true,
            state.captured_account_check.as_ref().map(|proof| proof.status),
            Some(format!(
                "native accounts/check probe failed; using captured browser accounts/check proof: {error}"
            )),
        ),
        Err(error) => (
            false,
            None,
            Some(append_status_message(
                captured_account_check_stale_message,
                error,
            )),
        ),
    };
    let identity_complete = state.captured_auth_identity.complete && identity_issue.is_none();
    let logged_in = account_check_ok && identity_complete;
    let message = match identity_issue {
        Some(issue) => Some(append_status_message(message, issue)),
        None => message,
    };
    Ok(ChatGptWebAuthStatus {
        state: if logged_in {
            "logged_in"
        } else {
            "auth_required"
        }
        .to_string(),
        logged_in,
        identity_complete,
        account_check_ok,
        account_status,
        cookie_count: state.cookies.len(),
        captured_header_names: state.captured_auth_headers.keys().cloned().collect(),
        profile_dir: config.profile_dir.display().to_string(),
        state_path: config.state_path.display().to_string(),
        message,
    })
}

fn append_status_message(existing: Option<String>, next: impl Into<String>) -> String {
    match existing {
        Some(existing) if !existing.is_empty() => format!("{existing}; {}", next.into()),
        _ => next.into(),
    }
}

fn authorization_identity_issue(identity: &AuthorizationIdentity) -> Option<String> {
    let expires_at = identity.expires_at.as_deref()?;
    let expires_at = match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(value) => value.with_timezone(&chrono::Utc),
        Err(error) => {
            return Some(format!(
                "authorization token expiry could not be parsed: {error}"
            ));
        }
    };
    if chrono::Utc::now() + chrono::Duration::seconds(AUTH_EXPIRY_SKEW_SECS) >= expires_at {
        return Some(format!("authorization token expired at {expires_at}"));
    }
    None
}

fn browser_account_check_proof_is_fresh(proof: &BrowserAccountCheckProof) -> bool {
    let Ok(captured_at) = chrono::DateTime::parse_from_rfc3339(&proof.captured_at) else {
        return false;
    };
    let age = chrono::Utc::now().signed_duration_since(captured_at.with_timezone(&chrono::Utc));
    if age < -chrono::Duration::seconds(BROWSER_ACCOUNT_CHECK_PROOF_FUTURE_SKEW_SECS) {
        return false;
    }
    age <= chrono::Duration::seconds(BROWSER_ACCOUNT_CHECK_PROOF_MAX_AGE_SECS)
}

async fn open_login_and_capture(config: &RuntimeConfig) -> Result<AuthState, String> {
    tokio::fs::create_dir_all(&config.profile_dir)
        .await
        .map_err(|error| format!("create profile dir failed: {error}"))?;
    let (_login_guard, mut stop_login) = register_login_session(config);
    info!(profile_dir = %config.profile_dir.display(), "chatgpt_web: opening login browser");
    BrowserSession::kill_headless_for_profile(config).await;
    let mut browser = BrowserSession::get_or_launch(config, false).await?;
    let mut client = None;
    let mut target_id = None;
    let result = async {
        let page = browser.open_or_find_page(&config.chatgpt.base_url).await?;
        target_id = Some(page.id.clone());
        let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3).await?;
        cdp.enable_domains().await?;
        cdp.send("Page.bringToFront", json!({})).await?;
        let mut events = cdp.subscribe();
        cdp.send("Page.navigate", json!({"url": config.chatgpt.base_url}))
            .await?;

        let mut captured_headers = BTreeMap::new();
        let mut account_request_ids = BTreeSet::new();
        let mut account_response_statuses = BTreeMap::new();
        let mut captured_account_check = None;
        loop {
            while let Ok(event) = events.try_recv() {
                collect_account_headers(
                    &config.chatgpt.base_url,
                    &event,
                    &mut account_request_ids,
                    &mut captured_headers,
                );
                collect_account_response_status(
                    &event,
                    &account_request_ids,
                    &mut account_response_statuses,
                );
                if let Some(proof) =
                    collect_account_check_proof(&cdp, &event, &account_response_statuses).await
                {
                    captured_account_check = Some(proof);
                }
            }
            if captured_headers.contains_key("authorization")
                && captured_account_check
                    .as_ref()
                    .is_some_and(|proof: &BrowserAccountCheckProof| proof.logged_in)
            {
                info!(
                    header_count = captured_headers.len(),
                    "chatgpt_web login: auth headers captured successfully"
                );
                break;
            }
            if browser.has_exited()? {
                return Err(
                    "auth_required: ChatGPT browser login window was closed before login completed"
                        .to_string(),
                );
            }
            if cdp.is_closed() {
                return Err(
                    "auth_required: ChatGPT browser login window was closed before login completed"
                        .to_string(),
                );
            }
            tokio::select! {
                received = stop_login.recv() => {
                    if received.is_ok() || matches!(received, Err(broadcast::error::RecvError::Lagged(_))) {
                        return Err("auth_required: ChatGPT browser login was stopped by request".to_string());
                    }
                }
                _ = sleep(Duration::from_millis(500)) => {}
            }
        }
        if captured_headers.is_empty() {
            return Err("auth_required: no ChatGPT accounts/check auth headers captured from browser traffic".to_string());
        }
        let user_agent = evaluate_string(&cdp, "navigator.userAgent").await?;
        let cookies = get_all_chatgpt_cookies(&cdp).await?;
        let identity = summarize_authorization_identity(&captured_headers);
        let state = AuthState {
            captured_at: iso_now(),
            base_url: config.chatgpt.base_url.clone(),
            user_agent,
            captured_auth_headers: captured_headers,
            captured_auth_identity: identity,
            captured_account_check,
            cookies,
        };
        write_auth_state(&config.state_path, &state).await?;
        let status = auth_status_from_state(config, &state).await?;
        if !status.logged_in {
            return Err(format!(
                "auth_required: ChatGPT browser login was captured but native accounts/check did not prove a complete login: {:?}",
                status.message
            ));
        }
        client = Some(cdp);
        Ok(state)
    }
    .await;

    if !config.browser.keep_browser_open_after_login {
        if let Some(target_id) = target_id.as_deref() {
            let _ = browser.close_target(target_id).await;
        }
        browser.kill().await;
    }
    if let Some(cdp) = client {
        cdp.close();
    }
    result
}

fn has_handoff_submission_evidence(event_types: &[String]) -> bool {
    event_types.iter().any(|event_type| {
        !matches!(
            event_type.as_str(),
            "handoff_recovered_after_sse_interrupt" | "browser_ui"
        )
    })
}

async fn wait_user_message_visible(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
    text: &str,
    after_time: f64,
    duration: Duration,
    stop_marker_path: &Path,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + duration;
    let mut last_error = None;
    while tokio::time::Instant::now() < deadline {
        if stop_requested(stop_marker_path).await {
            return Err(stopped_error());
        }
        let detail = match get_conversation_detail(config, auth, conversation_id).await {
            Ok(detail) => detail,
            Err(error) if is_transient_conversation_read_error(&error) => {
                last_error = Some(error);
                sleep(Duration::from_millis(config.chatgpt.poll_interval_ms)).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        if conversation_has_user_message_after(&detail, text, after_time) {
            return Ok(());
        }
        sleep(Duration::from_millis(config.chatgpt.poll_interval_ms)).await;
    }
    Err(format!(
        "browser_ui: ChatGPT Web did not show the submitted user message after handoff recovery{}",
        last_error
            .map(|error| format!("; last read error: {error}"))
            .unwrap_or_default()
    ))
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

async fn chatgpt_json(
    config: &RuntimeConfig,
    auth: &AuthState,
    path: &str,
) -> Result<Value, String> {
    // Always use browser-context fetch — native HTTP consistently fails against
    // ChatGPT's API due to CloudFlare challenges / 429 rate-limits, and the
    // timeout wasted on the native attempt delays the browser-context fallback.
    browser_context_json(config, auth, path).await
}

async fn browser_context_json(
    config: &RuntimeConfig,
    auth: &AuthState,
    path: &str,
) -> Result<Value, String> {
    let headless = config.browser.execution_mode != "headed";
    let browser = BrowserSession::get_or_launch(config, headless).await?;
    let mut _target_id = None;
    let mut client = None;
    let result = async {
        let page = browser.open_or_find_page(&config.chatgpt.base_url).await?;
        _target_id = Some(page.id.clone());
        // CDP WebSocket connect with automatic retry (handles transient HTTP 500).
        let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3).await?;
        // Enable Page, Network, and Runtime domains in parallel.
        cdp.enable_domains().await?;
        // Ensure the page is on the chatgpt.com domain before running fetch.
        // When open_or_find_page creates a new tab, the page may still be on
        // about:blank (navigating), causing relative fetch() URLs to fail with
        // "Failed to parse URL".
        wait_page_on_domain(&cdp, &config.chatgpt.base_url, Duration::from_secs(30)).await?;
        seed_chatgpt_cookies(&cdp, auth).await?;
        // Do NOT Page.navigate — the page is already on chatgpt.com domain and
        // credentials: "include" will send the right cookies for the fetch.
        // Extra navigations trigger ChatGPT 429 rate-limits.
        let headers = browser_fetch_headers(auth);
        let expression = format!(
            r#"(async () => {{
              const response = await fetch({path:?}, {{
                method: "GET",
                credentials: "include",
                headers: {headers}
              }});
              const text = await response.text();
              return {{ status: response.status, ok: response.ok, text }};
            }})()"#,
            path = path,
            headers = serde_json::to_string(&headers)
                .map_err(|error| format!("serialize browser fetch headers failed: {error}"))?
        );
        let fetched = evaluate_value(&cdp, &expression).await?;
        client = Some(cdp);
        let status = fetched
            .get("status")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let text = fetched
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !fetched.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Err(format!(
                "ChatGPT browser-context HTTP {status} for {path}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        serde_json::from_str(text)
            .map_err(|error| format!("parse ChatGPT browser-context JSON failed: {error}"))
    }
    .await;
    // Do NOT navigate away — stay on the current page to avoid 429 rate-limits.
    if let Some(cdp) = client {
        cdp.close();
    }
    result
}

/// Wait until the page's `location.origin` matches the expected base URL's
/// origin. This is necessary because `open_or_find_page` may return a
/// freshly-created tab that is still navigating from `about:blank` to the
/// target URL. Running `fetch()` with a relative path on `about:blank` fails
/// with "Failed to parse URL".
async fn wait_page_on_domain(
    cdp: &CdpClient,
    base_url: &str,
    max_wait: Duration,
) -> Result<(), String> {
    // Extract origin from base_url (e.g. "https://chatgpt.com" from "https://chatgpt.com/foo")
    let expected_origin = base_url
        .find("://")
        .map(|scheme_end| {
            let after_scheme = &base_url[scheme_end + 3..];
            let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
            &base_url[..scheme_end + 3 + host_end]
        })
        .unwrap_or(base_url);

    let deadline = tokio::time::Instant::now() + max_wait;
    loop {
        match evaluate_value(cdp, "location.origin").await {
            Ok(value) => {
                let origin = value.as_str().unwrap_or_default();
                if origin == expected_origin {
                    debug!(
                        %origin,
                        "chatgpt_web browser_context_json: page is on expected domain"
                    );
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "browser_context_json: page origin {origin:?} did not become {expected_origin:?} within timeout"
                    ));
                }
                debug!(
                    actual_origin = %origin,
                    %expected_origin,
                    "chatgpt_web browser_context_json: page not on expected domain yet, waiting…"
                );
            }
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "browser_context_json: timed out waiting for page domain: {e}"
                    ));
                }
                debug!(
                    error = %e,
                    "chatgpt_web browser_context_json: failed to evaluate location.origin, retrying…"
                );
            }
        }
        sleep(Duration::from_millis(300)).await;
    }
}

fn browser_fetch_headers(auth: &AuthState) -> BTreeMap<String, String> {
    auth.captured_auth_headers
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "cookie" | "user-agent"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn conversation_has_user_message_after(
    conversation: &Value,
    expected_text: &str,
    after_time: f64,
) -> bool {
    let Some(mapping) = conversation.get("mapping").and_then(Value::as_object) else {
        return false;
    };
    mapping.values().any(|node| {
        let Some(message) = node.get("message") else {
            return false;
        };
        if message
            .get("author")
            .and_then(|author| author.get("role"))
            .and_then(Value::as_str)
            != Some("user")
        {
            return false;
        }
        let create_time = message
            .get("create_time")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        create_time >= after_time && extract_text_parts(message).trim() == expected_text.trim()
    })
}

pub(in crate::im_gateway::chatgpt_web) async fn evaluate_value(
    cdp: &CdpClient,
    expression: &str,
) -> Result<Value, String> {
    let result = cdp
        .send(
            "Runtime.evaluate",
            json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
        )
        .await?;
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(format!("Runtime.evaluate failed: {exception}"));
    }
    Ok(result
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

async fn evaluate_string(cdp: &CdpClient, expression: &str) -> Result<String, String> {
    evaluate_value(cdp, expression)
        .await
        .map(|value| value.as_str().unwrap_or_default().to_string())
}

async fn get_all_chatgpt_cookies(cdp: &CdpClient) -> Result<Vec<BrowserCookie>, String> {
    let result = cdp.send("Network.getAllCookies", json!({})).await?;
    let cookies = result
        .get("cookies")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let cookies = serde_json::from_value::<Vec<BrowserCookie>>(cookies)
        .map_err(|error| format!("parse browser cookies failed: {error}"))?;
    Ok(cookies
        .into_iter()
        .filter(|cookie| cookie.domain.ends_with("chatgpt.com"))
        .collect())
}

pub(in crate::im_gateway::chatgpt_web) async fn seed_chatgpt_cookies(
    cdp: &CdpClient,
    auth: &AuthState,
) -> Result<(), String> {
    let cookies: Vec<Value> = auth
        .cookies
        .iter()
        .filter(|cookie| cookie.domain.ends_with("chatgpt.com"))
        .filter(|cookie| !cookie.name.is_empty() && !cookie.value.is_empty())
        .map(|cookie| {
            let mut item = serde_json::Map::new();
            item.insert("name".to_string(), Value::String(cookie.name.clone()));
            item.insert("value".to_string(), Value::String(cookie.value.clone()));
            item.insert("domain".to_string(), Value::String(cookie.domain.clone()));
            item.insert(
                "path".to_string(),
                Value::String(cookie.path.clone().unwrap_or_else(|| "/".to_string())),
            );
            if let Some(secure) = cookie.secure {
                item.insert("secure".to_string(), Value::Bool(secure));
            }
            if let Some(http_only) = cookie.http_only {
                item.insert("httpOnly".to_string(), Value::Bool(http_only));
            }
            if let Some(expires) = cookie.expires.filter(|expires| *expires > 0.0) {
                item.insert("expires".to_string(), json!(expires));
            }
            if let Some(same_site) = cookie
                .same_site
                .as_deref()
                .filter(|value| matches!(*value, "Strict" | "Lax" | "None"))
            {
                item.insert("sameSite".to_string(), Value::String(same_site.to_string()));
            }
            Value::Object(item)
        })
        .collect();
    if cookies.is_empty() {
        return Err(
            "auth_required: no ChatGPT cookies available for browser execution".to_string(),
        );
    }
    cdp.send("Network.setCookies", json!({ "cookies": cookies }))
        .await
        .map_err(|error| format!("seed ChatGPT cookies into browser failed: {error}"))?;
    Ok(())
}

fn collect_account_headers(
    base_url: &str,
    event: &CdpEvent,
    account_request_ids: &mut BTreeSet<String>,
    target: &mut BTreeMap<String, String>,
) {
    if event.method != "Network.requestWillBeSent"
        && event.method != "Network.requestWillBeSentExtraInfo"
    {
        return;
    }
    if event.method == "Network.requestWillBeSent" {
        let url = event
            .params
            .get("request")
            .and_then(|request| request.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if url.contains("/backend-api/") {
            debug!(url, "chatgpt_web login: observed backend-api request");
        }
        if !url.starts_with(&format!("{base_url}{ACCOUNT_CHECK_PATH_PREFIX}")) {
            return;
        }
        info!(url, "chatgpt_web login: captured accounts/check request");
        if let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) {
            account_request_ids.insert(request_id.to_string());
        }
        if let Some(headers) = event
            .params
            .get("request")
            .and_then(|request| request.get("headers"))
            .and_then(Value::as_object)
        {
            extract_reusable_headers(headers, target);
        }
    } else if event
        .params
        .get("requestId")
        .and_then(Value::as_str)
        .is_some_and(|request_id| account_request_ids.contains(request_id))
    {
        let Some(headers) = event.params.get("headers").and_then(Value::as_object) else {
            return;
        };
        extract_reusable_headers(headers, target);
    }
}

async fn collect_account_check_proof(
    cdp: &CdpClient,
    event: &CdpEvent,
    account_response_statuses: &BTreeMap<String, u16>,
) -> Option<BrowserAccountCheckProof> {
    if event.method != "Network.loadingFinished" {
        return None;
    }
    let request_id = event.params.get("requestId").and_then(Value::as_str)?;
    let status = *account_response_statuses.get(request_id)?;
    let logged_in = cdp
        .send(
            "Network.getResponseBody",
            json!({ "requestId": request_id }),
        )
        .await
        .ok()
        .and_then(|body| decode_cdp_response_body(&body))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|json| native::account_check_logged_in(&json))
        .unwrap_or(false);
    Some(BrowserAccountCheckProof {
        captured_at: iso_now(),
        status,
        logged_in,
    })
}

fn collect_account_response_status(
    event: &CdpEvent,
    account_request_ids: &BTreeSet<String>,
    target: &mut BTreeMap<String, u16>,
) {
    if event.method != "Network.responseReceived" {
        return;
    }
    let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
        return;
    };
    if !account_request_ids.contains(request_id) {
        return;
    }
    let Some(status) = event
        .params
        .get("response")
        .and_then(|response| response.get("status"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
    else {
        return;
    };
    target.insert(request_id.to_string(), status);
}

fn decode_cdp_response_body(result: &Value) -> Option<String> {
    let body = result.get("body").and_then(Value::as_str)?;
    if result
        .get("base64Encoded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    } else {
        Some(body.to_string())
    }
}

fn extract_reusable_headers(
    headers: &serde_json::Map<String, Value>,
    target: &mut BTreeMap<String, String>,
) {
    let reusable = [
        "authorization",
        "chatgpt-account-id",
        "oai-device-id",
        "oai-session-id",
        "oai-language",
        "oai-client-build-number",
        "oai-client-version",
        "x-oai-is",
        "user-agent",
    ];
    for (key, value) in headers {
        let normalized = key.to_ascii_lowercase();
        if reusable.contains(&normalized.as_str()) {
            if let Some(value) = value.as_str() {
                target.insert(normalized, value.to_string());
            }
        }
    }
}

fn summarize_authorization_identity(headers: &BTreeMap<String, String>) -> AuthorizationIdentity {
    let authorization = headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return AuthorizationIdentity::default();
    };
    let Some(payload) = token.split('.').nth(1) else {
        return AuthorizationIdentity {
            has_bearer_token: true,
            ..Default::default()
        };
    };
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return AuthorizationIdentity {
            has_bearer_token: true,
            ..Default::default()
        };
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&decoded) else {
        return AuthorizationIdentity {
            has_bearer_token: true,
            ..Default::default()
        };
    };
    let profile = claims
        .get("https://api.openai.com/profile")
        .unwrap_or(&Value::Null);
    let auth = claims
        .get("https://api.openai.com/auth")
        .unwrap_or(&Value::Null);
    let email = profile
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let has_profile_email = !email.is_empty();
    let profile_email_verified = profile.get("email_verified").and_then(Value::as_bool);
    let user_ids = [
        auth.get("user_id").and_then(Value::as_str),
        auth.get("chatgpt_user_id").and_then(Value::as_str),
        auth.get("chatgpt_account_user_id").and_then(Value::as_str),
    ];
    let has_user_identity = user_ids
        .iter()
        .flatten()
        .any(|value| value.starts_with("user-"));
    let has_account_identity = auth
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    AuthorizationIdentity {
        has_bearer_token: true,
        has_profile_email,
        profile_email_verified,
        has_user_identity,
        has_account_identity,
        complete: has_profile_email && has_user_identity && has_account_identity,
        expires_at: claims
            .get("exp")
            .and_then(Value::as_i64)
            .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0))
            .map(|time| time.to_rfc3339()),
    }
}

fn runtime_config(config: &ExternalCliAdapterConfig) -> Result<RuntimeConfig, String> {
    let value = serde_json::to_value(config)
        .map_err(|error| format!("serialize adapter config failed: {error}"))?;
    let mut parsed: ChatGptWebAdapterConfig = serde_json::from_value(value)
        .map_err(|error| format!("parse chatgpt_web adapter config failed: {error}"))?;
    if let Some(timeout_secs) = config.timeout_secs {
        parsed.chatgpt.timeout_secs = timeout_secs;
    }
    let base = bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chatgpt_web");
    let profile_dir = resolve_config_path(
        parsed.browser.profile_dir.as_deref(),
        &base,
        "browser_profile",
    );
    let state_path =
        resolve_config_path(parsed.auth.state_path.as_deref(), &base, "auth_state.json");
    Ok(RuntimeConfig {
        sessions_path: base.join("sessions.json"),
        attachments_dir: bifrost_agent::config::agent_home_dir()
            .join("im_gateway")
            .join("attachments")
            .join("chatgpt_web"),
        profile_dir,
        state_path,
        browser: parsed.browser,
        chatgpt: parsed.chatgpt,
    })
}

fn login_sessions() -> &'static DashMap<String, ActiveLoginSession> {
    LOGIN_SESSIONS.get_or_init(DashMap::new)
}

fn login_session_key(config: &RuntimeConfig) -> String {
    config.state_path.to_string_lossy().to_string()
}

fn register_login_session(config: &RuntimeConfig) -> (LoginSessionGuard, broadcast::Receiver<()>) {
    let key = login_session_key(config);
    let id = LOGIN_SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let (stop, receiver) = broadcast::channel(8);
    if let Some(previous) = login_sessions().insert(key.clone(), ActiveLoginSession { id, stop }) {
        let _ = previous.stop.send(());
    }
    (LoginSessionGuard { key, id }, receiver)
}

fn resolve_config_path(value: Option<&str>, base: &Path, fallback: &str) -> PathBuf {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|path| base.join(path))
                .unwrap_or_else(|| base.join(fallback))
        })
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn default_browser_channel() -> String {
    "edge".to_string()
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn default_open_on_auth_required() -> bool {
    true
}

fn default_execution_mode() -> String {
    "headed".to_string()
}

fn default_poll_interval_ms() -> u64 {
    1000
}

fn default_timeout_secs() -> u64 {
    7200
}

fn default_session_consistency() -> String {
    "strong".to_string()
}

fn default_rate_limit_retry_secs() -> u64 {
    180
}

fn default_rate_limit_max_retries() -> u32 {
    5
}

#[cfg(test)]
mod context_replay_tests {
    use super::*;

    #[test]
    fn test_sanitize_strips_image_placeholder() {
        assert_eq!(
            sanitize_for_history("已生成图片，正在发送原图。"),
            "[此轮生成了图片]"
        );
        assert_eq!(
            sanitize_for_history("已生成图片（图片下载失败，请手动查看对话）"),
            "[此轮生成了图片]"
        );
    }

    #[test]
    fn test_sanitize_strips_markdown_image_refs() {
        let input = "这是正文\n\n![ChatGPT 生成图片 1](/tmp/bifrost/img.png)\n\n更多文字";
        let result = sanitize_for_history(input);
        assert!(
            !result.contains("/tmp/"),
            "should strip local path: {result}"
        );
        assert!(
            !result.contains("!["),
            "should strip image markdown: {result}"
        );
        assert!(result.contains("这是正文"), "should keep text: {result}");
        assert!(result.contains("更多文字"), "should keep text: {result}");
    }

    #[test]
    fn test_sanitize_does_not_strip_exclamation_without_parens() {
        // "![注意]" without "](..." is NOT a valid image ref, should be preserved
        let input = "![注意] 这里有问题";
        let result = sanitize_for_history(input);
        assert!(
            result.contains("![注意]"),
            "should keep non-image ![]: {result}"
        );
    }

    #[test]
    fn test_sanitize_strips_local_path_lines() {
        let input = "正常文本\n/Users/testuser/some/path.png\n/tmp/test/file\n继续";
        let result = sanitize_for_history(input);
        assert!(
            !result.contains("/Users/"),
            "should strip /Users/ line: {result}"
        );
        assert!(
            !result.contains("/tmp/"),
            "should strip /tmp/ line: {result}"
        );
        assert!(
            result.contains("正常文本"),
            "should keep normal text: {result}"
        );
        assert!(result.contains("继续"), "should keep normal text: {result}");
    }

    #[test]
    fn test_sanitize_truncates_to_max_chars() {
        // Text well under the limit — no truncation
        let short_text = "a".repeat(100);
        let result = sanitize_for_history(&short_text);
        assert_eq!(result.len(), 100, "short text should not be truncated");

        // Text over the limit — should be truncated with head+tail strategy
        let long_text = "a".repeat(5000);
        let result = sanitize_for_history(&long_text);
        assert!(
            result.chars().count() <= HISTORY_SUMMARY_MAX_CHARS + 40,
            "truncated result should be around HISTORY_SUMMARY_MAX_CHARS, got {}",
            result.chars().count()
        );
        assert!(
            result.contains("裁剪了"),
            "should contain truncation marker: {result}"
        );
    }

    #[test]
    fn test_truncate_keep_head_tail_preserves_both_ends() {
        // Build a string that exceeds the limit: head(20) + filler(200) + tail(20)
        let head = "AAAA这是开头内容1234567890";
        let filler = "X".repeat(200);
        let tail = "结尾部分ZZZZ1234567890DDDD";
        let text = format!("{head}{filler}{tail}");
        // Truncate to 80 chars — much smaller than the ~240-char input
        let result = truncate_keep_head_tail(&text, 80);
        assert!(result.starts_with("AAAA"), "should keep head: {result}");
        assert!(result.ends_with("DDDD"), "should keep tail: {result}");
        assert!(
            result.contains("裁剪了"),
            "should have truncation marker: {result}"
        );
    }

    #[test]
    fn test_truncate_keep_head_tail_no_op_when_short() {
        let text = "短文本不截断";
        let result = truncate_keep_head_tail(text, 2000);
        assert_eq!(result, text);
    }

    #[test]
    fn test_record_and_build_context_replay() {
        let sk = "test-session-replay-001";
        // Clean up any prior state
        conversation_history_store().remove(sk);

        // Record 3 rounds
        record_conversation_exchange(sk, "写第1章", "第一章内容很长很长...");
        record_conversation_exchange(sk, "生成第1章插图", "已生成图片，正在发送原图。");
        record_conversation_exchange(sk, "写第2章", "第二章内容...");

        // Build replay message
        let replay = build_context_replay_message(sk, "写第3章").unwrap();

        // Verify structure
        assert!(
            replay.contains("对话历史丢失"),
            "should have header: {}",
            &replay[..200]
        );
        assert!(
            replay.contains("第1轮 - 用户"),
            "should have round 1: {replay}"
        );
        assert!(replay.contains("写第1章"), "should have user msg round 1");
        assert!(
            replay.contains("第一章内容很长很长"),
            "should have assistant reply round 1"
        );
        assert!(
            replay.contains("[此轮生成了图片]"),
            "should sanitize image placeholder"
        );
        assert!(replay.contains("第3轮 - 用户"), "should have round 3");
        assert!(
            replay.contains("第二章内容"),
            "should have round 3 assistant"
        );
        assert!(replay.contains("[继续任务]"), "should have footer");
        assert!(replay.ends_with("写第3章"), "should end with new message");

        // Verify no local paths leaked
        assert!(!replay.contains("/tmp/"), "no /tmp/ paths");
        assert!(!replay.contains("/Users/"), "no /Users/ paths");

        // Clean up
        conversation_history_store().remove(sk);
    }

    #[test]
    fn test_build_context_replay_none_when_no_history() {
        let result = build_context_replay_message("nonexistent-session-xyz", "hello");
        assert!(result.is_none());
    }

    #[test]
    fn test_build_context_replay_respects_max_length() {
        let sk = "test-session-replay-maxlen";
        conversation_history_store().remove(sk);

        // Record many rounds with long content to exceed 15K limit
        for i in 0..60 {
            let user_msg = format!("轮次{}: {}", i, "用户消息内容".repeat(10));
            let asst_msg = format!("助手回复{}: {}", i, "回复内容详情".repeat(10));
            record_conversation_exchange(sk, &user_msg, &asst_msg);
        }

        let replay = build_context_replay_message(sk, "新消息").unwrap();
        // The prefix (before "新消息") should be limited
        let prefix_end = replay.rfind("新消息").unwrap();
        let prefix = &replay[..prefix_end];
        // Should contain truncation notice
        assert!(
            prefix.contains("已省略中间"),
            "should have head-tail truncation notice: {}",
            &prefix[prefix.len().saturating_sub(200)..]
        );
        // Total prefix should be roughly under 15K (+/- one chunk)
        assert!(
            prefix.len() < CONTEXT_REPLAY_MAX_TOTAL_CHARS + 2000,
            "prefix too long: {} chars",
            prefix.len()
        );

        // ── Head-tail strategy verification ──
        // Should contain the FIRST round (earliest context)
        assert!(
            prefix.contains("轮次0"),
            "should keep earliest round (轮次0): {}",
            &prefix[..prefix.len().min(500)]
        );
        // Should contain the LAST round (most recent context)
        assert!(
            prefix.contains("轮次59") || prefix.contains("助手回复59"),
            "should keep most recent round (轮次59): {}",
            &prefix[prefix.len().saturating_sub(500)..]
        );
        // Middle rounds should be omitted
        assert!(
            !prefix.contains("轮次30"),
            "middle round (轮次30) should be omitted"
        );

        conversation_history_store().remove(sk);
    }

    #[test]
    fn test_build_context_replay_multiline_blockquote() {
        let sk = "test-session-replay-multiline";
        conversation_history_store().remove(sk);

        record_conversation_exchange(sk, "第一行\n第二行\n第三行", "回复ok");

        let replay = build_context_replay_message(sk, "继续").unwrap();
        // Multi-line user content should be in blockquote
        assert!(
            replay.contains("> 第一行"),
            "should have blockquote: {replay}"
        );
        assert!(
            replay.contains("> 第二行"),
            "should have blockquote line 2: {replay}"
        );

        conversation_history_store().remove(sk);
    }

    // ── context_may_be_lost flag tests ──

    #[test]
    fn test_mark_and_take_context_may_be_lost() {
        let sk = "test-flag-mark-take";
        // Clean state
        context_lost_flags().remove(sk);

        // Initially false
        assert!(!take_context_may_be_lost(sk), "should be false initially");

        // Mark it
        mark_context_may_be_lost(sk);
        assert!(take_context_may_be_lost(sk), "should be true after marking");

        // Take clears it
        assert!(
            !take_context_may_be_lost(sk),
            "should be false after take (consumed)"
        );
    }

    #[test]
    fn test_context_replay_triggered_by_flag() {
        let sk = "test-flag-replay-trigger";
        conversation_history_store().remove(sk);
        context_lost_flags().remove(sk);

        // Record some history
        record_conversation_exchange(sk, "写第1章", "第一章内容...");
        record_conversation_exchange(sk, "生成插图", "已生成图片，正在发送原图。");

        // Without flag, take returns false → no replay needed
        assert!(!take_context_may_be_lost(sk));

        // Set the flag (simulating 429)
        mark_context_may_be_lost(sk);

        // Flag is set → take returns true
        assert!(take_context_may_be_lost(sk));

        // Build replay to verify it works with the history
        let replay = build_context_replay_message(sk, "写第2章").unwrap();
        assert!(
            replay.contains("写第1章"),
            "should include round 1 user msg"
        );
        assert!(
            replay.contains("第一章内容"),
            "should include round 1 assistant"
        );
        assert!(
            replay.contains("[此轮生成了图片]"),
            "should sanitize image placeholder"
        );
        assert!(
            replay.contains("写第2章"),
            "should include new message at end"
        );

        // Clean up
        conversation_history_store().remove(sk);
        context_lost_flags().remove(sk);
    }

    #[test]
    fn test_flag_does_not_affect_other_sessions() {
        let sk1 = "test-flag-isolation-a";
        let sk2 = "test-flag-isolation-b";
        context_lost_flags().remove(sk1);
        context_lost_flags().remove(sk2);

        mark_context_may_be_lost(sk1);

        assert!(take_context_may_be_lost(sk1), "sk1 should be flagged");
        assert!(!take_context_may_be_lost(sk2), "sk2 should NOT be flagged");

        context_lost_flags().remove(sk1);
        context_lost_flags().remove(sk2);
    }

    #[test]
    fn test_multiple_429s_keep_flag_set() {
        let sk = "test-flag-multiple-429";
        context_lost_flags().remove(sk);

        // Simulate multiple 429s in a row
        mark_context_may_be_lost(sk);
        mark_context_may_be_lost(sk); // double-mark should be idempotent

        assert!(take_context_may_be_lost(sk), "should still be true");
        assert!(
            !take_context_may_be_lost(sk),
            "should be cleared after take"
        );

        context_lost_flags().remove(sk);
    }

    /// Simulate the full ask-flow: 429 on round N → mark flag → round N+1
    /// automatically prepends history → record only original message (no snowball).
    #[test]
    fn test_full_flow_429_then_context_replay_no_snowball() {
        let sk = "test-full-flow-snowball";
        conversation_history_store().remove(sk);
        context_lost_flags().remove(sk);

        // ── Round 1: normal ──
        let user_r1 = "写第1章：一个关于勇士的故事";
        let asst_r1 = "第一章：勇士踏上了旅途...（1200字正文）";
        record_conversation_exchange(sk, user_r1, asst_r1);

        // ── Round 2: 429 hit during wait_final → mark flag ──
        let user_r2 = "生成第1章配图";
        let asst_r2 = "已生成图片，正在发送原图。";
        // Simulate successful response via DOM fallback
        record_conversation_exchange(sk, user_r2, asst_r2);
        // 429 occurred → mark flag for next round
        mark_context_may_be_lost(sk);

        // ── Round 3: auto context replay ──
        let user_r3_original = "写第2章";
        assert!(
            take_context_may_be_lost(sk),
            "flag should be set from round 2 429"
        );
        let replay = build_context_replay_message(sk, user_r3_original).unwrap();

        // Verify replay contains history from rounds 1 and 2
        assert!(replay.contains("写第1章"), "should have round 1 user msg");
        assert!(
            replay.contains("勇士踏上了旅途"),
            "should have round 1 assistant"
        );
        assert!(
            replay.contains("[此轮生成了图片]"),
            "should sanitize image placeholder from round 2"
        );
        assert!(
            replay.ends_with("写第2章"),
            "should end with original user msg"
        );

        // KEY ASSERTION: record only the ORIGINAL message, not the replay
        // This prevents snowball growth across rounds.
        let asst_r3 = "第二章：勇士遇到了一条龙...";
        record_conversation_exchange(sk, user_r3_original, asst_r3);

        // Verify history has clean entries (no replay prefix embedded)
        // NOTE: clone entries out of the DashMap Ref to avoid holding the
        // read lock when we later call remove() (which needs a write lock).
        {
            let entries = conversation_history_store().get(sk).unwrap();
            let user_entries: Vec<_> = entries.iter().filter(|e| e.role == "user").collect();
            assert_eq!(user_entries.len(), 3, "should have 3 user entries");
            // Round 3 user entry should be the short original, NOT the long replay
            assert_eq!(user_entries[2].content, "写第2章");
            assert!(
                user_entries[2].content.len() < 20,
                "user entry should be original (short), not replay (long): {}",
                user_entries[2].content
            );
        } // Ref dropped here, releasing read lock

        // ── Round 4: no flag → no replay ──
        assert!(
            !take_context_may_be_lost(sk),
            "flag should be cleared (round 3 was normal)"
        );
        let r4_msg = "写第3章";
        let no_replay = build_context_replay_message(sk, r4_msg).unwrap();
        // This returns a replay (history exists), but in real flow we only
        // call build_context_replay_message when flag is set.
        // The key point: since flag is NOT set, the real flow would use
        // the original message directly.

        // Verify round 4 history includes all 3 prior rounds
        assert!(
            no_replay.contains("写第2章"),
            "round 3 user msg should be in history"
        );
        assert!(
            no_replay.contains("勇士遇到了一条龙"),
            "round 3 assistant should be in history"
        );

        // Clean up
        conversation_history_store().remove(sk);
        context_lost_flags().remove(sk);
    }

    /// Test context replay after conversation_lost (conversation switch scenario).
    #[test]
    fn test_conversation_lost_triggers_replay_with_full_history() {
        let sk = "test-conversation-lost-replay";
        conversation_history_store().remove(sk);
        context_lost_flags().remove(sk);

        // Record 5 rounds of history (simulating a long conversation)
        for i in 1..=5 {
            record_conversation_exchange(
                sk,
                &format!("第{}章指令", i),
                &format!("第{}章内容：精彩的故事第{}段...", i, i),
            );
        }

        // Simulate conversation_lost error → build replay for new conversation
        let new_msg = "继续写第6章";
        let replay = build_context_replay_message(sk, new_msg).unwrap();

        // Should contain all 5 rounds of history
        for i in 1..=5 {
            assert!(
                replay.contains(&format!("第{}章指令", i)),
                "should have round {} user msg",
                i
            );
            assert!(
                replay.contains(&format!("第{}章内容", i)),
                "should have round {} assistant msg",
                i
            );
        }
        assert!(replay.contains("[继续任务]"), "should have footer marker");
        assert!(replay.ends_with(new_msg), "should end with new message");

        // History should NOT be cleared after replay (retained for future replays)
        {
            let entries = conversation_history_store().get(sk).unwrap();
            assert_eq!(entries.len(), 10, "5 rounds × 2 = 10 entries");
        }

        conversation_history_store().remove(sk);
    }

    /// Test that sanitize handles mixed content: images + paths + normal text + code blocks.
    #[test]
    fn test_sanitize_mixed_complex_content() {
        let input = r#"这是正文内容。

```python
def hello():
    path = "/Users/someone/file.py"
    print("hello")
```

![生成的图片](https://chatgpt.com/backend/img.png)
一些普通文字
/tmp/bifrost/screenshot_001.png
![另一张图](/Users/testuser/local/img.jpg)
结尾文字"#;

        let result = sanitize_for_history(input);

        // Should keep code block content (it's inside a code block, not a line starting with /Users/)
        assert!(result.contains("def hello()"), "should keep code: {result}");
        // Should strip the image markdown references
        assert!(
            !result.contains("![生成的图片]"),
            "should strip valid img ref: {result}"
        );
        assert!(
            !result.contains("![另一张图]"),
            "should strip local img ref: {result}"
        );
        // Should strip lines starting with /tmp/
        assert!(
            !result.contains("/tmp/bifrost/"),
            "should strip /tmp/ line: {result}"
        );
        // Should keep normal text
        assert!(
            result.contains("一些普通文字"),
            "should keep text: {result}"
        );
        assert!(result.contains("结尾文字"), "should keep text: {result}");
    }

    /// Test that record_conversation_exchange with empty assistant message doesn't add entry.
    #[test]
    fn test_record_skips_empty_assistant() {
        let sk = "test-record-skip-empty";
        conversation_history_store().remove(sk);

        record_conversation_exchange(sk, "用户消息", "");

        {
            let entries = conversation_history_store().get(sk).unwrap();
            // Should only have user entry, no assistant entry for empty content
            assert_eq!(entries.len(), 1, "should only have user entry");
            assert_eq!(entries[0].role, "user");
        }

        conversation_history_store().remove(sk);
    }

    /// Test context replay when session has only user messages (assistant was empty/stripped).
    #[test]
    fn test_replay_with_only_user_entries() {
        let sk = "test-replay-user-only";
        conversation_history_store().remove(sk);

        // Record exchange where assistant response is entirely local paths (gets stripped)
        record_conversation_exchange(sk, "写第1章", "/tmp/bifrost/some_output.txt");

        let replay = build_context_replay_message(sk, "写第2章").unwrap();
        assert!(replay.contains("写第1章"), "should have user msg: {replay}");
        // Assistant was stripped, but we should still get valid replay
        assert!(
            replay.contains("[继续任务]"),
            "should have footer: {replay}"
        );
        assert!(replay.ends_with("写第2章"), "should end with new msg");

        conversation_history_store().remove(sk);
    }
}
