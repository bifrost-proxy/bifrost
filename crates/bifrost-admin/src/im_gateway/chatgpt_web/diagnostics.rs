use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use super::storage::write_redacted_json;
use super::{
    evaluate_value, get_conversation_detail, iso_now, seed_chatgpt_cookies, truncate_for_log,
    AuthState, BrowserSession, CdpClient, ExternalCliRunRequest, RuntimeConfig,
};

pub(in crate::im_gateway::chatgpt_web) fn conversation_id_hint_from_request(
    request: &ExternalCliRunRequest,
) -> Option<String> {
    ["conversationId", "conversation_id"]
        .iter()
        .find_map(|key| request.params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(in crate::im_gateway::chatgpt_web) async fn capture_run_failure_artifacts(
    config: &RuntimeConfig,
    auth: &AuthState,
    run_dir: &Path,
    operation: &str,
    conversation_id: Option<&str>,
    error: &str,
) {
    let mut artifacts = Vec::new();
    if let Some(conversation_id) = conversation_id {
        let artifact = match timeout(
            Duration::from_secs(20),
            get_conversation_detail(config, auth, conversation_id),
        )
        .await
        {
            Ok(Ok(detail)) => {
                let path = run_dir.join("conversation_response.json");
                match write_redacted_json(&path, &detail).await {
                    Ok(()) => json!({
                        "name": "conversation_response",
                        "status": "written",
                        "path": path.to_string_lossy(),
                    }),
                    Err(error) => json!({
                        "name": "conversation_response",
                        "status": "write_failed",
                        "error": error,
                    }),
                }
            }
            Ok(Err(error)) => json!({
                "name": "conversation_response",
                "status": "capture_failed",
                "conversationId": conversation_id,
                "error": error,
            }),
            Err(_) => json!({
                "name": "conversation_response",
                "status": "capture_timeout",
                "conversationId": conversation_id,
                "timeoutSecs": 20,
            }),
        };
        artifacts.push(artifact);
    } else {
        artifacts.push(json!({
            "name": "conversation_response",
            "status": "skipped",
            "reason": "conversation_id_unavailable",
        }));
    }

    let page_artifact = match timeout(
        Duration::from_secs(60),
        capture_page_dom_artifact(config, auth, run_dir, conversation_id),
    )
    .await
    {
        Ok(Ok(artifact)) => artifact,
        Ok(Err(error)) => json!({
            "name": "page_dom",
            "status": "capture_failed",
            "error": error,
        }),
        Err(_) => json!({
            "name": "page_dom",
            "status": "capture_timeout",
            "timeoutSecs": 60,
        }),
    };
    artifacts.push(page_artifact);

    write_run_failure_manifest(run_dir, operation, conversation_id, error, artifacts).await;
}

pub(in crate::im_gateway::chatgpt_web) async fn write_run_failure_manifest(
    run_dir: &Path,
    operation: &str,
    conversation_id: Option<&str>,
    error: &str,
    artifacts: Vec<Value>,
) {
    let manifest = json!({
        "status": "failed",
        "operation": operation,
        "conversationId": conversation_id,
        "error": error,
        "capturedAt": iso_now(),
        "artifacts": artifacts,
    });
    if let Err(write_error) =
        write_redacted_json(&run_dir.join("failure_diagnostics.json"), &manifest).await
    {
        warn!(
            error = %write_error,
            "chatgpt_web failed to write failure diagnostics manifest"
        );
    }
}

async fn capture_page_dom_artifact(
    config: &RuntimeConfig,
    auth: &AuthState,
    run_dir: &Path,
    conversation_id: Option<&str>,
) -> Result<Value, String> {
    let base_url = config.chatgpt.base_url.trim_end_matches('/');
    let url = conversation_id
        .map(|conversation_id| format!("{base_url}/c/{conversation_id}"))
        .unwrap_or_else(|| config.chatgpt.base_url.clone());
    let headless = config.browser.execution_mode != "headed";
    let browser = BrowserSession::get_or_launch(config, headless).await?;
    let mut _target_id = None;
    let mut client = None;
    let result = async {
        let page = browser.open_or_find_page(&url).await?;
        _target_id = Some(page.id.clone());
        let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3).await?;
        cdp.enable_domains().await?;
        seed_chatgpt_cookies(&cdp, auth).await?;
        if !headless {
            cdp.send("Page.bringToFront", json!({})).await?;
        }
        // Skip navigation if the page is already on the target URL to avoid 429.
        let already_on_page = page.url.starts_with(&url);
        if already_on_page {
            info!(page_url = %page.url, "chatgpt_web diagnostics: page already on target, skipping navigation");
        } else {
            info!(page_url = %page.url, %url, "chatgpt_web diagnostics: navigating to target page");
            cdp.navigate_and_wait(&url, Duration::from_secs(15)).await?;
        }
        sleep(Duration::from_secs(3)).await;
        let capture = evaluate_value(&cdp, PAGE_DOM_CAPTURE_SCRIPT).await?;
        let html = capture
            .get("html")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let html_path = run_dir.join("page_dom.html");
        tokio::fs::write(&html_path, html.as_bytes())
            .await
            .map_err(|error| format!("write {} failed: {error}", html_path.display()))?;
        let metadata = page_dom_metadata(&capture, &html_path, html.len());
        let json_path = run_dir.join("page_dom.json");
        write_redacted_json(&json_path, &metadata).await?;
        client = Some(cdp);
        Ok(json!({
            "name": "page_dom",
            "status": "written",
            "htmlPath": html_path.to_string_lossy(),
            "jsonPath": json_path.to_string_lossy(),
        }))
    }
    .await;
    // Do NOT navigate away — stay on the current page to avoid 429 rate-limits.
    if let Some(cdp) = client {
        cdp.close();
    }
    result
}

pub(in crate::im_gateway::chatgpt_web) fn page_dom_metadata(
    capture: &Value,
    html_path: &Path,
    html_bytes: usize,
) -> Value {
    json!({
        "capturedAt": iso_now(),
        "url": capture.get("url").cloned().unwrap_or(Value::Null),
        "title": capture.get("title").cloned().unwrap_or(Value::Null),
        "readyState": capture.get("readyState").cloned().unwrap_or(Value::Null),
        "activeElement": capture.get("activeElement").cloned().unwrap_or(Value::Null),
        "composer": capture.get("composer").cloned().unwrap_or(Value::Null),
        "landmarks": capture.get("landmarks").cloned().unwrap_or(Value::Null),
        "bodyText": capture
            .get("bodyText")
            .and_then(Value::as_str)
            .map(|value| truncate_for_log(value, 20_000))
            .unwrap_or_default(),
        "htmlPath": html_path.to_string_lossy(),
        "htmlBytes": html_bytes,
    })
}

const PAGE_DOM_CAPTURE_SCRIPT: &str = r#"
(() => {
  const describe = (el) => {
    if (!el) return null;
    const rect = el.getBoundingClientRect ? el.getBoundingClientRect() : null;
    return {
      tag: el.tagName,
      id: el.id || null,
      role: el.getAttribute('role'),
      testId: el.getAttribute('data-testid'),
      ariaLabel: el.getAttribute('aria-label'),
      placeholder: el.getAttribute('placeholder'),
      contentEditable: el.getAttribute('contenteditable'),
      text: (el.innerText || el.value || '').slice(0, 2000),
      visible: !!rect && rect.width > 0 && rect.height > 0,
      rect: rect ? {
        x: Math.round(rect.x),
        y: Math.round(rect.y),
        width: Math.round(rect.width),
        height: Math.round(rect.height)
      } : null
    };
  };
  const candidates = Array.from(document.querySelectorAll(
    '#prompt-textarea, [data-testid*="composer"], [role="textbox"], textarea, [contenteditable="true"]'
  )).slice(0, 30).map(describe);
  const landmarks = Array.from(document.querySelectorAll(
    'main, form, nav, header, footer, [role], [data-testid]'
  )).slice(0, 120).map(describe);
  return {
    url: location.href,
    title: document.title,
    readyState: document.readyState,
    bodyText: document.body ? document.body.innerText : '',
    html: document.documentElement ? document.documentElement.outerHTML : '',
    activeElement: describe(document.activeElement),
    composer: candidates,
    landmarks
  };
})()
"#;
