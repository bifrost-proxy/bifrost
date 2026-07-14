use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::time::sleep;

use super::send::dismiss_rate_limit_modal;
use super::storage::write_secret_file;
use super::{chatgpt_json, AuthState, RuntimeConfig};
use crate::im_gateway::external_cli::{
    ExternalCliProgressEvent, ExternalCliProgressEventType, ExternalCliRunRequest,
};

pub(super) async fn requested_or_session_conversation(
    request: &ExternalCliRunRequest,
    config: &RuntimeConfig,
) -> Result<Option<String>, String> {
    if let Ok(conversation_id) = conversation_id_from_params(request) {
        return Ok(Some(conversation_id));
    }
    let Some(session_key) = request.session_key.as_deref() else {
        return Ok(None);
    };
    let map = read_session_map(&config.sessions_path).await?;
    Ok(map.get(session_key).cloned())
}

pub(super) fn conversation_id_from_params(
    request: &ExternalCliRunRequest,
) -> Result<String, String> {
    request
        .params
        .get("conversationId")
        .or_else(|| request.params.get("conversation_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "conversationId is required".to_string())
}

async fn read_session_map(path: &Path) -> Result<BTreeMap<String, String>, String> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|error| format!("parse session map failed: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(format!("read session map failed: {error}")),
    }
}

pub(super) async fn save_session_conversation(
    config: &RuntimeConfig,
    session_key: &str,
    conversation_id: &str,
) -> Result<(), String> {
    let lock = super::session_locks()
        .entry(config.sessions_path.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    let mut map = read_session_map(&config.sessions_path).await?;
    map.insert(session_key.to_string(), conversation_id.to_string());
    let bytes = serde_json::to_vec_pretty(&map)
        .map_err(|error| format!("serialize session map failed: {error}"))?;
    write_secret_file(&config.sessions_path, &bytes).await
}

pub async fn clear_session_conversation(session_key: &str) {
    let sessions_path = bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chatgpt_web")
        .join("sessions.json");
    let lock = super::session_locks()
        .entry(sessions_path.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    let mut map = match read_session_map(&sessions_path).await {
        Ok(map) => map,
        Err(error) => {
            tracing::warn!(
                session_key = %session_key,
                error = %error,
                "failed to read session map for clear"
            );
            return;
        }
    };
    if map.remove(session_key).is_none() {
        return;
    }
    let bytes = match serde_json::to_vec_pretty(&map) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(
                session_key = %session_key,
                error = %error,
                "failed to serialize session map after clear"
            );
            return;
        }
    };
    if let Err(error) = write_secret_file(&sessions_path, &bytes).await {
        tracing::warn!(
            session_key = %session_key,
            error = %error,
            "failed to write session map after clear"
        );
    }
}

pub async fn session_conversation_exists(session_key: &str) -> bool {
    let sessions_path = bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chatgpt_web")
        .join("sessions.json");
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return false;
    }
    match read_session_map(&sessions_path).await {
        Ok(map) => map
            .get(session_key)
            .map(|conversation_id| !conversation_id.trim().is_empty())
            .unwrap_or(false),
        Err(error) => {
            tracing::warn!(
                session_key = %session_key,
                error = %error,
                "failed to read session map for existence check"
            );
            false
        }
    }
}

pub(super) fn summarize_conversations(json: &Value) -> Value {
    let items = json
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            json!({
                "id": item.get("id").and_then(Value::as_str),
                "title": item.get("title").and_then(Value::as_str),
                "createTime": item.get("create_time").cloned(),
                "updateTime": item.get("update_time").cloned(),
                "isArchived": item.get("is_archived").and_then(Value::as_bool),
                "isTemporaryChat": item.get("is_temporary_chat").and_then(Value::as_bool),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "total": json.get("total").cloned(),
        "items": items,
    })
}

pub(super) fn summarize_conversation_detail(json: &Value) -> Value {
    json!({
        "id": json.get("conversation_id").or_else(|| json.get("id")).and_then(Value::as_str),
        "title": json.get("title").and_then(Value::as_str),
        "currentNode": json.get("current_node").and_then(Value::as_str),
        "mappingCount": json.get("mapping").and_then(Value::as_object).map(|mapping| mapping.len()),
        "finalAssistant": find_final_assistant(json, None),
        "messages": extract_conversation_messages(json),
    })
}

pub(super) fn event(
    event_type: ExternalCliProgressEventType,
    content: impl Into<String>,
    title: Option<&str>,
    raw: Value,
) -> ExternalCliProgressEvent {
    ExternalCliProgressEvent {
        event_type,
        content: content.into(),
        title: title.map(ToString::to_string),
        raw,
    }
}

pub(super) struct WaitedFinal {
    pub(super) final_message: Value,
    pub(super) summary: Value,
    /// Individual assistant message texts (ordered by create_time).
    /// When ChatGPT produces multiple messages (e.g. thinking + answer),
    /// each one is stored here separately for per-message delivery to IM.
    pub(super) all_texts: Vec<String>,
    /// True when this result was obtained via DOM fallback or after a 429
    /// recovery.  ChatGPT's server-side context may be unreliable for the
    /// next turn, so callers should mark the session for context replay.
    pub(super) had_429_or_fallback: bool,
}

struct TerminalReadyCandidate {
    waited: WaitedFinal,
    text_len: usize,
    image_count: usize,
    signature: u64,
    stable_since: tokio::time::Instant,
}

#[derive(Clone, Copy)]
pub(super) struct WaitFinalOptions<'a> {
    pub(super) duration: std::time::Duration,
    pub(super) stop_marker_path: &'a Path,
    pub(super) profile_dir: Option<&'a Path>,
    pub(super) terminal_idle_stable_for: std::time::Duration,
}

/// Try to construct a `WaitedFinal` directly from the SSE stream detail.
/// This avoids polling the conversation detail API (which triggers 429).
///
/// The `sse_detail` is a synthetic conversation object built from SSE events,
/// with the same `mapping` structure as the conversation detail API.
pub(super) fn try_waited_final_from_sse(
    sse_detail: &Value,
    after_time: Option<f64>,
) -> Option<WaitedFinal> {
    // Check for still-generating messages before accepting any final-looking
    // content. This keeps partial multi-image tool updates from ending the run
    // while still allowing asset-only final tool messages once all activity is
    // finished.
    if has_in_progress_messages(sse_detail, after_time) {
        tracing::info!("chatgpt_web SSE: still has in_progress messages, SSE detail incomplete");
        return None;
    }

    // Check for generated images first (same priority as wait_final)
    if let Some(final_message) = find_generated_images_tool_message(sse_detail, after_time) {
        tracing::info!(
            "chatgpt_web SSE: found completed generated image tool result from SSE stream"
        );
        return Some(WaitedFinal {
            final_message: final_message.clone(),
            summary: summarize_conversation_detail(sse_detail),
            all_texts: vec![final_message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()],
            had_429_or_fallback: false,
        });
    }

    // Look for the final assistant message
    let final_message = find_final_assistant(sse_detail, after_time)?;
    let end_turn = final_message
        .get("endTurn")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let status_finished = final_message
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status == "finished_successfully")
        .unwrap_or(false);

    if !end_turn || !status_finished {
        tracing::info!(
            end_turn,
            status = ?final_message.get("status"),
            "chatgpt_web SSE: assistant message not finalized (endTurn={end_turn}, finished={status_finished})"
        );
        return None;
    }

    // Use find_all_assistant_texts to handle multi-message responses
    let (merged_message, all_texts) = find_all_assistant_texts(sse_detail, after_time)
        .unwrap_or_else(|| {
            (
                final_message.clone(),
                vec![final_message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()],
            )
        });

    let text_len = merged_message
        .get("text")
        .and_then(Value::as_str)
        .map(|s| s.len())
        .unwrap_or(0);

    tracing::info!(
        text_len,
        message_count = all_texts.len(),
        "chatgpt_web SSE: built WaitedFinal directly from SSE stream (no API polling needed)"
    );

    Some(WaitedFinal {
        final_message: merged_message,
        summary: summarize_conversation_detail(sse_detail),
        all_texts,
        had_429_or_fallback: false,
    })
}

pub(super) async fn wait_final(
    _config: &RuntimeConfig,
    _auth: &AuthState,
    conversation_id: &str,
    _after_time: Option<f64>,
    options: WaitFinalOptions<'_>,
) -> Result<WaitedFinal, String> {
    // NOTE: This function no longer makes active API requests to ChatGPT.
    // All data should be captured via CDP network interception in wait_send_handoff.
    // This function only serves as a DOM-based fallback when CDP interception fails.
    let wait_started = tokio::time::Instant::now();
    let deadline = wait_started + options.duration;
    let Some(pdir) = options.profile_dir else {
        return Err("wait_final: no profile_dir available for DOM extraction".to_string());
    };

    tracing::info!(
        conversation_id,
        timeout_secs = options.duration.as_secs(),
        "chatgpt_web wait_final: DOM-only mode (no API polling)"
    );

    // Brief initial wait for content to appear on page
    sleep(std::time::Duration::from_secs(1)).await;
    let mut terminal_idle_since: Option<tokio::time::Instant> = None;
    let mut terminal_ready_candidate: Option<TerminalReadyCandidate> = None;

    while tokio::time::Instant::now() < deadline {
        if stop_requested(options.stop_marker_path).await {
            return Err(stopped_error());
        }

        // Timeout CDP calls to prevent indefinite hangs
        let dom_outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            try_extract_dom_outcome(_config, pdir, conversation_id),
        )
        .await;

        let dom_outcome = match dom_outcome {
            Ok(outcome) => outcome,
            Err(_timeout) => {
                tracing::warn!(
                    conversation_id,
                    "chatgpt_web wait_final: DOM extraction timed out (10s), retrying"
                );
                sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        match dom_outcome {
            DomExtractOutcome::Ready(waited) => {
                let dom_text_len = waited
                    .final_message
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|s| s.len())
                    .unwrap_or(0);
                let dom_image_count = waited
                    .final_message
                    .get("generatedImages")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let ready_signature = dom_ready_signature(&waited);
                let now = tokio::time::Instant::now();
                let stable_since = *terminal_idle_since.get_or_insert(now);
                let terminal_idle_stable_for = options.terminal_idle_stable_for;
                if now.duration_since(stable_since) < terminal_idle_stable_for {
                    tracing::info!(
                        conversation_id,
                        dom_text_len,
                        dom_image_count,
                        stable_for_ms = now.duration_since(stable_since).as_millis(),
                        required_stable_ms = terminal_idle_stable_for.as_millis(),
                        "chatgpt_web wait_final: DOM terminal idle waiting for stop button/composer stability"
                    );
                    sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }

                let settle_for = dom_terminal_content_settle_for(dom_text_len, dom_image_count);
                if settle_for.is_zero() {
                    tracing::info!(
                        conversation_id,
                        dom_text_len,
                        dom_image_count,
                        "chatgpt_web wait_final: generation complete (stop button gone), returning"
                    );
                    return Ok(waited);
                }

                let ready_candidate = TerminalReadyCandidate {
                    waited,
                    text_len: dom_text_len,
                    image_count: dom_image_count,
                    signature: ready_signature,
                    stable_since: now,
                };
                match terminal_ready_candidate.as_mut() {
                    None => {
                        terminal_ready_candidate = Some(ready_candidate);
                        tracing::info!(
                            conversation_id,
                            dom_text_len,
                            dom_image_count,
                            required_stable_ms = settle_for.as_millis(),
                            "chatgpt_web wait_final: terminal DOM ready, waiting for rendered content to settle"
                        );
                    }
                    Some(candidate) if ready_candidate.signature != candidate.signature => {
                        tracing::info!(
                            conversation_id,
                            previous_text_len = candidate.text_len,
                            dom_text_len = ready_candidate.text_len,
                            previous_image_count = candidate.image_count,
                            dom_image_count = ready_candidate.image_count,
                            required_stable_ms = settle_for.as_millis(),
                            "chatgpt_web wait_final: terminal DOM content changed, resetting settle timer"
                        );
                        terminal_ready_candidate = Some(ready_candidate);
                    }
                    Some(candidate) => {
                        let stable_for = now.duration_since(candidate.stable_since);
                        if stable_for >= settle_for {
                            let candidate = terminal_ready_candidate
                                .take()
                                .expect("terminal candidate should exist");
                            tracing::info!(
                                conversation_id,
                                dom_text_len = candidate.text_len,
                                dom_image_count = candidate.image_count,
                                stable_for_ms = stable_for.as_millis(),
                                "chatgpt_web wait_final: generation complete and terminal DOM content settled"
                            );
                            return Ok(candidate.waited);
                        }
                        tracing::info!(
                            conversation_id,
                            dom_text_len = candidate.text_len,
                            dom_image_count = candidate.image_count,
                            stable_for_ms = stable_for.as_millis(),
                            required_stable_ms = settle_for.as_millis(),
                            "chatgpt_web wait_final: terminal DOM content still settling"
                        );
                    }
                }
                sleep(dom_terminal_content_settle_poll_for()).await;
                continue;
            }
            DomExtractOutcome::Streaming {
                text_len,
                image_count,
                reason,
            } => {
                terminal_idle_since = None;
                terminal_ready_candidate = None;
                // Still generating — just log and wait.
                // TODO: future enhancement — check if content is long enough for partial/batch output
                tracing::info!(
                    conversation_id,
                    text_len,
                    image_count,
                    reason,
                    streaming = true,
                    "chatgpt_web wait_final: still generating, waiting for completion"
                );
            }
            DomExtractOutcome::NotFound => {
                terminal_idle_since = None;
                terminal_ready_candidate = None;
                tracing::debug!(
                    conversation_id,
                    "chatgpt_web wait_final: no content found yet"
                );
            }
        }

        sleep(std::time::Duration::from_secs(1)).await;
    }

    // Deadline exceeded — final forced extraction attempt
    tracing::warn!(
        conversation_id,
        "chatgpt_web wait_final: deadline exceeded, final forced extraction"
    );
    if let Some(waited) = try_extract_latest_from_dom_force(_config, pdir, conversation_id).await {
        return Ok(waited);
    }

    Err("timed_out: DOM extraction could not capture final content".to_string())
}

pub(super) fn extract_conversation_messages(conversation: &Value) -> Vec<Value> {
    let Some(mapping) = conversation.get("mapping").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut messages = mapping
        .values()
        .filter_map(|node| node.get("message"))
        .filter_map(|message| {
            let role = message
                .get("author")
                .and_then(|author| author.get("role"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !matches!(role, "user" | "assistant" | "tool") {
                return None;
            }
            let text = extract_text_parts(message);
            if text.trim().is_empty() {
                return None;
            }
            Some(json!({
                "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
                "role": role,
                "createTime": message.get("create_time").and_then(Value::as_f64).unwrap_or_default(),
                "status": message.get("status").and_then(Value::as_str).unwrap_or_default(),
                "endTurn": message.get("end_turn").and_then(Value::as_bool).unwrap_or(false),
                "model": message.get("metadata").and_then(|metadata| metadata.get("model_slug").or_else(|| metadata.get("default_model_slug"))).and_then(Value::as_str),
                "text": text,
            }))
        })
        .collect::<Vec<_>>();
    messages.sort_by(|left, right| {
        left.get("createTime")
            .and_then(Value::as_f64)
            .partial_cmp(&right.get("createTime").and_then(Value::as_f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    messages
}

pub(super) async fn get_conversation_detail(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
) -> Result<Value, String> {
    chatgpt_json(
        config,
        auth,
        &format!("/backend-api/conversation/{conversation_id}"),
    )
    .await
}

pub(super) fn is_transient_conversation_read_error(error: &str) -> bool {
    // Match both native HTTP and browser-context HTTP error formats.
    // Native format:  "ChatGPT native HTTP 429"
    // Browser format: "ChatGPT browser-context HTTP 429 for /backend-api/..."
    error.contains("HTTP 404")
        || error.contains("HTTP 403")
        || error.contains("HTTP 429")
        || error.contains("HTTP 500")
        || error.contains("HTTP 502")
        || error.contains("HTTP 503")
        || error.contains("HTTP 504")
}

pub(super) async fn stop_requested(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

pub(super) fn stopped_error() -> String {
    "stopped: ChatGPT Web run was stopped by request.".to_string()
}

/// Check if the conversation has any messages still being generated.
///
/// This is used to prevent `wait_final` from returning prematurely when ChatGPT
/// has produced an intermediate "thinking/planning" assistant message that is
/// marked `endTurn=true, status=finished_successfully`, but the actual final
/// response hasn't been generated yet (e.g. during web search flows where the
/// model first declares intent, then executes the search, then produces the
/// real answer).
pub(super) fn has_in_progress_messages(conversation: &Value, after_time: Option<f64>) -> bool {
    let Some(mapping) = conversation.get("mapping").and_then(Value::as_object) else {
        return false;
    };
    for node in mapping.values() {
        let Some(message) = node.get("message") else {
            continue;
        };
        let create_time = message
            .get("create_time")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if after_time.is_some_and(|after| create_time < after) {
            continue;
        }
        let status = message
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status == "in_progress" {
            return true;
        }
    }
    false
}

pub(super) fn find_final_assistant(conversation: &Value, after_time: Option<f64>) -> Option<Value> {
    let mut candidates = Vec::new();
    let mapping = conversation.get("mapping")?.as_object()?;
    for node in mapping.values() {
        let Some(message) = node.get("message") else {
            continue;
        };
        if message
            .get("author")
            .and_then(|author| author.get("role"))
            .and_then(Value::as_str)
            != Some("assistant")
        {
            continue;
        }
        let create_time = message
            .get("create_time")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if after_time.is_some_and(|after| create_time < after) {
            continue;
        }
        let text = extract_text_parts(message);
        if text.trim().is_empty() {
            continue;
        }
        candidates.push(json!({
            "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
            "createTime": create_time,
            "status": message.get("status").and_then(Value::as_str).unwrap_or_default(),
            "endTurn": message.get("end_turn").and_then(Value::as_bool).unwrap_or(false),
            "model": message.get("metadata").and_then(|metadata| metadata.get("model_slug").or_else(|| metadata.get("default_model_slug"))).and_then(Value::as_str),
            "text": text,
        }));
    }
    candidates.sort_by(|left, right| {
        left.get("createTime")
            .and_then(Value::as_f64)
            .partial_cmp(&right.get("createTime").and_then(Value::as_f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.pop()
}

/// Find all assistant messages in the conversation after `after_time` and
/// concatenate their text content. This handles the case where ChatGPT
/// produces multiple assistant messages for a single user question (e.g.
/// a "thinking" message followed by the actual answer, both with
/// `endTurn=true`).
///
/// Returns `(merged_value, individual_texts)` where `individual_texts`
/// contains each assistant message's text separately (for per-message
/// delivery to IM channels).
pub(super) fn find_all_assistant_texts(
    conversation: &Value,
    after_time: Option<f64>,
) -> Option<(Value, Vec<String>)> {
    let mut candidates = Vec::new();
    let mapping = conversation.get("mapping")?.as_object()?;
    for node in mapping.values() {
        let Some(message) = node.get("message") else {
            continue;
        };
        if message
            .get("author")
            .and_then(|author| author.get("role"))
            .and_then(Value::as_str)
            != Some("assistant")
        {
            continue;
        }
        let create_time = message
            .get("create_time")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if after_time.is_some_and(|after| create_time < after) {
            continue;
        }
        let status = message
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Only include finished messages
        if status != "finished_successfully" {
            continue;
        }
        let text = extract_text_parts(message);
        if text.trim().is_empty() {
            continue;
        }
        candidates.push((create_time, text, message.clone()));
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let individual_texts: Vec<String> =
        candidates.iter().map(|(_, text, _)| text.clone()).collect();
    // If there's only one, return it directly (same as find_final_assistant)
    if candidates.len() == 1 {
        let (create_time, text, message) = candidates.pop().unwrap();
        let value = json!({
            "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
            "createTime": create_time,
            "status": "finished_successfully",
            "endTurn": message.get("end_turn").and_then(Value::as_bool).unwrap_or(false),
            "model": message.get("metadata").and_then(|metadata| metadata.get("model_slug").or_else(|| metadata.get("default_model_slug"))).and_then(Value::as_str),
            "text": text,
        });
        return Some((value, individual_texts));
    }
    // Multiple assistant messages: use the last one as the "primary" but
    // concatenate all texts
    let combined_text = candidates
        .iter()
        .map(|(_, text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let (create_time, _, message) = candidates.last().unwrap();
    let value = json!({
        "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
        "createTime": create_time,
        "status": "finished_successfully",
        "endTurn": message.get("end_turn").and_then(Value::as_bool).unwrap_or(false),
        "model": message.get("metadata").and_then(|metadata| metadata.get("model_slug").or_else(|| metadata.get("default_model_slug"))).and_then(Value::as_str),
        "text": combined_text,
    });
    Some((value, individual_texts))
}

pub(super) fn find_generated_images_tool_message(
    conversation: &Value,
    after_time: Option<f64>,
) -> Option<Value> {
    let mut candidates = Vec::new();
    let mapping = conversation.get("mapping")?.as_object()?;
    for node in mapping.values() {
        let Some(message) = node.get("message") else {
            continue;
        };
        if message
            .get("author")
            .and_then(|author| author.get("role"))
            .and_then(Value::as_str)
            != Some("tool")
        {
            continue;
        }
        let create_time = message
            .get("create_time")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if after_time.is_some_and(|after| create_time < after) {
            continue;
        }
        let status = message
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status != "finished_successfully" {
            continue;
        }
        let text = extract_text_parts(message);
        let mut images = extract_generated_image_placeholders(&text);
        let image_assets = collect_generated_image_assets_from_message(message, create_time);
        if images.is_empty() {
            images = image_assets;
        } else {
            attach_generated_image_assets(&mut images, &image_assets);
        }
        if images.is_empty() {
            continue;
        }
        candidates.push(json!({
            "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
            "createTime": create_time,
            "status": status,
            "endTurn": true,
            "model": message.get("metadata").and_then(|metadata| metadata.get("model_slug").or_else(|| metadata.get("default_model_slug"))).and_then(Value::as_str),
            "text": "已生成图片，正在发送原图。",
            "generatedImages": images,
        }));
    }
    candidates.sort_by(|left, right| {
        left.get("createTime")
            .and_then(Value::as_f64)
            .partial_cmp(&right.get("createTime").and_then(Value::as_f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.pop()
}

fn collect_generated_image_assets_from_message(message: &Value, create_time: f64) -> Vec<Value> {
    let mut images = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let Some(parts) = message
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    else {
        return images;
    };
    for part in parts {
        if part.get("content_type").and_then(Value::as_str) != Some("image_asset_pointer") {
            continue;
        }
        let Some(asset_pointer) = part
            .get("asset_pointer")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("sediment://file_"))
        else {
            continue;
        };
        let file_id = asset_pointer.trim_start_matches("sediment://");
        if !seen.insert(file_id.to_string()) {
            continue;
        }
        images.push(json!({
            "assetPointer": asset_pointer,
            "fileId": file_id,
            "width": part.get("width").and_then(Value::as_u64),
            "height": part.get("height").and_then(Value::as_u64),
            "sizeBytes": part.get("size_bytes").and_then(Value::as_u64),
            "createTime": create_time,
        }));
    }
    images
}

fn attach_generated_image_assets(images: &mut [Value], image_assets: &[Value]) {
    if images.is_empty() || image_assets.is_empty() {
        return;
    }
    let start = image_assets.len().saturating_sub(images.len());
    for (image, asset) in images.iter_mut().zip(image_assets[start..].iter()) {
        if let Some(object) = image.as_object_mut() {
            if let Some(asset_pointer) = asset.get("assetPointer").and_then(Value::as_str) {
                object.insert(
                    "assetPointer".to_string(),
                    Value::String(asset_pointer.to_string()),
                );
            }
            if let Some(file_id) = asset.get("fileId").and_then(Value::as_str) {
                object.insert("fileId".to_string(), Value::String(file_id.to_string()));
            }
            for key in ["width", "height", "sizeBytes"] {
                if let Some(value) = asset.get(key) {
                    object.insert(key.to_string(), value.clone());
                }
            }
        }
    }
}

pub(super) fn extract_generated_image_placeholders(text: &str) -> Vec<Value> {
    if !text.contains("Generated images") && !text.contains("/mnt/data/") {
        return Vec::new();
    }
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let path_start = trimmed.find("/mnt/data/")?;
            let rest = &trimmed[path_start..];
            let path_end = rest
                .find(|ch: char| ch.is_whitespace() || ch == ')' || ch == '(')
                .unwrap_or(rest.len());
            let path = rest[..path_end].trim();
            if path.is_empty() {
                return None;
            }
            let dimensions = trimmed
                .split("wxh =")
                .nth(1)
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            Some(json!({
                "sourcePath": path,
                "dimensions": dimensions,
            }))
        })
        .collect()
}

pub(super) fn extract_text_parts(message: &Value) -> String {
    message
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| {
                    part.as_str().map(ToString::to_string).or_else(|| {
                        part.get("text")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Try to dismiss the rate-limit modal ("请求过于频繁") by looking up the
/// conversation's CDP client from the tab pool and running the dismiss JS.
///
/// Returns `true` if the modal was successfully detected and dismissed.
/// Returns `false` if the tab is not in the pool, the CDP is closed, or
/// no modal was found on the page.
#[allow(dead_code)] // retained for rate-limit recovery experiments on pooled tabs.
async fn try_dismiss_modal_via_pool(profile_dir: &Path, conversation_id: &str) -> bool {
    let tab = match super::browser::get_conversation_tab(profile_dir, conversation_id) {
        Some(tab) => tab,
        None => return false,
    };
    if tab.cdp.is_closed() {
        return false;
    }
    dismiss_rate_limit_modal(&tab.cdp).await
}

/// JavaScript function that converts HTML DOM nodes to Markdown.
/// Injected into all DOM extraction scripts to preserve formatting.
const JS_HTML_TO_MD: &str = r#"
function htmlToMd(el) {
  if (!el) return '';
  if (el.nodeType === 3) return el.textContent || '';
  if (el.nodeType !== 1) return '';
  const tag = el.tagName.toLowerCase();
  if (tag === 'button' || tag === 'nav') return '';
  if (el.dataset && (el.dataset.testid || '').match(/overlay|action/)) return '';
  if (el.classList && el.classList.contains('citation-badge')) return '';
  const children = () => { let s = ''; for (const c of el.childNodes) s += htmlToMd(c); return s; };
  switch (tag) {
    case 'h1': return '\n# ' + children().trim() + '\n\n';
    case 'h2': return '\n## ' + children().trim() + '\n\n';
    case 'h3': return '\n### ' + children().trim() + '\n\n';
    case 'h4': return '\n#### ' + children().trim() + '\n\n';
    case 'h5': return '\n##### ' + children().trim() + '\n\n';
    case 'h6': return '\n###### ' + children().trim() + '\n\n';
    case 'strong': case 'b': return '**' + children() + '**';
    case 'em': case 'i': return '*' + children() + '*';
    case 'code':
      if (el.parentElement && el.parentElement.tagName === 'PRE') return children();
      return '`' + children() + '`';
    case 'pre': {
      const codeEl = el.querySelector('code');
      const lang = codeEl ? (codeEl.className.match(/language-(\w+)/)||[])[1] || '' : '';
      const code = codeEl ? (codeEl.innerText || '') : (el.innerText || '');
      return '\n```' + lang + '\n' + code.trim() + '\n```\n\n';
    }
    case 'blockquote': return '\n> ' + children().trim().replace(/\n/g, '\n> ') + '\n\n';
    case 'p': return children().trim() + '\n\n';
    case 'br': return '\n';
    case 'hr': return '\n---\n\n';
    case 'a': { const href = el.getAttribute('href') || ''; const txt = children().trim(); return href ? '[' + txt + '](' + href + ')' : txt; }
    case 'ul': { let s = '\n'; for (const li of el.children) { if (li.tagName === 'LI') s += '- ' + htmlToMd(li).trim() + '\n'; } return s + '\n'; }
    case 'ol': { let s = '\n'; let idx = 1; for (const li of el.children) { if (li.tagName === 'LI') { s += idx + '. ' + htmlToMd(li).trim() + '\n'; idx++; } } return s + '\n'; }
    case 'li': return children();
    case 'table': {
      const rows = el.querySelectorAll('tr');
      if (rows.length === 0) return children();
      let md = '\n';
      for (let r = 0; r < rows.length; r++) {
        const cells = rows[r].querySelectorAll('th, td');
        const row = [];
        for (const cell of cells) row.push(htmlToMd(cell).trim().replace(/\|/g, '\\|'));
        md += '| ' + row.join(' | ') + ' |\n';
        if (r === 0) md += '| ' + row.map(() => '---').join(' | ') + ' |\n';
      }
      return md + '\n';
    }
    case 'thead': case 'tbody': case 'tfoot': case 'tr': case 'th': case 'td': return children();
    case 'img': { const alt = el.alt || ''; const src = el.src || ''; return src ? '![' + alt + '](' + src + ')' : ''; }
    case 'del': case 's': return '~~' + children() + '~~';
    case 'sup': return '^(' + children() + ')';
    case 'sub': return '~(' + children() + ')';
    default: return children();
  }
}
function cleanMd(s) { return s.replace(/\n{3,}/g, '\n\n').trim(); }
function extractChatgptBehaviorArtifacts(root) {
  const artifacts = [];
  const seen = new Set();
  const ignoreText = /^(复制回复|分享|来源|切换模型|更多操作|Pro 反馈|Copy|Share|Sources?|More actions?|Switch model)$/i;
  const normalize = (value) => (value || '').replace(/\s+/g, ' ').trim();
  const classify = (label, context) => {
    const combined = `${label} ${context}`;
    if (/(zip|压缩包|打包下载|archive)/i.test(combined)) return 'archive';
    if (/(图片|图像|image|png|jpg|jpeg|webp)/i.test(combined)) return 'image';
    return 'image';
  };
  const buttons = root ? root.querySelectorAll('button.behavior-btn, button.entity-underline, .behavior-btn') : [];
  for (const button of buttons) {
    const label = normalize(button.getAttribute('title') || button.innerText || button.textContent);
    const buttonText = normalize(button.innerText || button.textContent || label);
    const context = normalize((button.closest('p, li, h1, h2, h3, h4') || {}).innerText || '');
    if (!label || ignoreText.test(label)) continue;
    if (/粘贴的\s*markdown|pasted\s+markdown|\.md$/i.test(label)) continue;
    if (button.closest('[data-testid*="citation"], [aria-label*="来源"], [aria-label*="Sources"]')) continue;
    const kind = classify(label, context);
    const key = `${kind}:${label}`;
    if (seen.has(key)) continue;
    seen.add(key);
    artifacts.push({
      kind,
      label,
      buttonText,
      context,
      source: 'chatgpt_behavior_button',
    });
  }
  return artifacts;
}
"#;

pub(super) fn append_dom_artifact_notice(text: &str, artifacts: &[Value]) -> String {
    if artifacts.is_empty() {
        return text.to_string();
    }

    let mut lines = Vec::new();
    for artifact in artifacts {
        let label = artifact
            .get("label")
            .or_else(|| artifact.get("buttonText"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(label) = label else {
            continue;
        };
        let kind = artifact
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("artifact");
        let kind_label = match kind {
            "archive" => "压缩包",
            "image" => "图片",
            _ => "附件",
        };
        let line = format!("- {kind_label}: {label}");
        if !lines.contains(&line) {
            lines.push(line);
        }
    }

    if lines.is_empty() {
        return text.to_string();
    }

    let mut output = text.trim().to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("ChatGPT Web 页面还提供了这些可点击下载项：\n");
    output.push_str(&lines.join("\n"));
    output
}

/// Result of a DOM extraction attempt, distinguishing between "complete content",
/// "streaming content (with observable text length)", and "no content found".
pub(super) enum DomExtractOutcome {
    /// Content extraction succeeded, streaming indicators are clear.
    Ready(WaitedFinal),
    /// Content found on page but streaming indicators are still active.
    /// The text_len is provided so the caller can track content stability
    /// even while "streaming" — if text_len stops changing, the streaming
    /// indicator is stale and content is actually complete.
    Streaming {
        text_len: usize,
        image_count: usize,
        reason: &'static str,
    },
    /// No usable content found (wrong page, no assistant turn, CDP error, etc.)
    NotFound,
}

/// Extract the latest assistant message content directly from the page DOM.
///
/// This is a fallback for when the backend API returns 429 but the page
/// has already rendered the response (text and/or images).  It avoids
/// duplicate sends by confirming the AI has already replied.
///
/// Returns `Some(WaitedFinal)` if a completed assistant message is found
/// on the page, `None` otherwise.
#[allow(dead_code)] // legacy DOM fallback wrapper kept for diagnostics and comparison.
pub(super) async fn try_extract_latest_from_dom(
    profile_dir: &Path,
    conversation_id: &str,
) -> Option<WaitedFinal> {
    let tab = super::browser::get_conversation_tab(profile_dir, conversation_id)?;
    if tab.cdp.is_closed() {
        return None;
    }
    // Guard: CDP evaluate can hang if the page is in a bad state.
    // Wrap the entire DOM extraction in a 10-second timeout.
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        extract_latest_assistant_from_dom(&tab.cdp),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                conversation_id,
                "chatgpt_web DOM extract: CDP evaluate timed out after 10s"
            );
            None
        }
    }
}

/// Run JS on the page to extract the latest assistant turn.
///
/// Scans `<section data-turn="assistant">` elements for:
/// - Turn ID (`data-turn-id`)
/// - Text content (from markdown rendering)
/// - Image URLs (from `<img>` tags with image generation patterns)
/// - Whether the message is still streaming (has busy indicators)
///
/// Returns `Some(WaitedFinal)` if a completed (non-streaming) assistant
/// message is found, `None` if no content or still streaming.
#[allow(dead_code)] // used only through the legacy wrapper above.
async fn extract_latest_assistant_from_dom(cdp: &super::CdpClient) -> Option<WaitedFinal> {
    let result = super::evaluate_value(
        cdp,
        r#"(() => {
          // ── Guard 1: Verify we're on a conversation page, not the homepage ──
          // After 429, ChatGPT sometimes redirects to the homepage.
          const path = window.location.pathname || '';
          if (!path.startsWith('/c/')) {
            return JSON.stringify({ found: false, reason: 'not_on_conversation_page' });
          }

          // ── Guard 2: Verify last turn is from assistant, not user ──
          // All turns on the page (both user and assistant).
          // If the very last turn is a user message, it means our message
          // was posted but the AI hasn't started responding yet — returning
          // here would give back stale content from a previous exchange.
          const allTurns = document.querySelectorAll(
            'section[data-turn-id-container]'
          );
          if (allTurns.length === 0) {
            return JSON.stringify({ found: false, reason: 'no_turns' });
          }
          const lastTurnRole = allTurns[allTurns.length - 1].getAttribute('data-turn');
          if (lastTurnRole !== 'assistant') {
            return JSON.stringify({
              found: false,
              reason: 'last_turn_is_' + lastTurnRole,
              turnCount: allTurns.length,
            });
          }

          // The last assistant turn
          const lastTurn = allTurns[allTurns.length - 1];
          const turnId = lastTurn.getAttribute('data-turn-id') || '';

          // ── Streaming / busy detection (multi-signal) ──
          // Removed .animate-pulse and .animate-spin (too broad — catches
          // unrelated UI animations and causes stale-streaming deadlocks).
          const isStreaming = lastTurn.querySelector(
            '[data-is-streaming="true"], .result-streaming'
          ) !== null;

          // Image generation in-progress: DALL-E shows spinners inside
          // image-gen containers while rendering.
          const imageGenBusy = lastTurn.querySelector(
            '[id^="image-"] .animate-spin, [id^="image-"] .animate-pulse'
          ) !== null;

          // Global busy: stop button visible is a strong signal that
          // generation is still active. Removed send-button[disabled] as it
          // can be disabled for reasons other than streaming.
          const globalBusy = document.querySelector(
            '[data-testid="stop-button"]'
          ) !== null;

          const busy = isStreaming || imageGenBusy || globalBusy;

          // ── Extract images ──
          const images = [];
          const seenSrcs = new Set();
          const imgEls = lastTurn.querySelectorAll('img');
          for (const img of imgEls) {
            const alt = img.alt || '';
            const src = img.src || '';
            if (!src) continue;
            // Skip tiny icons, avatars, etc.
            const w = img.naturalWidth || parseInt(img.width) || 0;
            if (w > 0 && w < 50) continue;
            // Generated images: "已生成图片" in alt, or from estuary/dall-e CDN
            if (alt.startsWith('已生成图片') || alt.startsWith('Generated image') ||
                src.includes('estuary/content') ||
                src.includes('oaidalleapi') || src.includes('dall-e')) {
              // Deduplicate by src — same image may appear multiple times
              // (thumbnail, expanded view, preview) in the DOM.
              const srcKey = src.split('?')[0] + (new URL(src, location.origin).searchParams.get('id') || src);
              if (seenSrcs.has(srcKey)) continue;
              seenSrcs.add(srcKey);
              images.push({
                alt: alt,
                src: src,
                width: parseInt(img.width) || 0,
                height: parseInt(img.height) || 0,
              });
            }
          }

          // ── HTML to Markdown converter ──
          // ChatGPT renders Markdown → HTML inside .markdown containers.
          // We reverse it to preserve formatting (headers, bold, tables, etc.)
          function htmlToMd(el) {
            if (!el) return '';
            if (el.nodeType === 3) return el.textContent || '';
            if (el.nodeType !== 1) return '';
            const tag = el.tagName.toLowerCase();
            // Skip UI chrome elements
            if (tag === 'button' || tag === 'nav') return '';
            if (el.dataset && (el.dataset.testid || '').match(/overlay|action/)) return '';
            // Skip citation badge containers
            if (el.classList && el.classList.contains('citation-badge')) return '';

            const children = () => {
              let s = '';
              for (const c of el.childNodes) s += htmlToMd(c);
              return s;
            };

            switch (tag) {
              case 'h1': return '\n# ' + children().trim() + '\n\n';
              case 'h2': return '\n## ' + children().trim() + '\n\n';
              case 'h3': return '\n### ' + children().trim() + '\n\n';
              case 'h4': return '\n#### ' + children().trim() + '\n\n';
              case 'h5': return '\n##### ' + children().trim() + '\n\n';
              case 'h6': return '\n###### ' + children().trim() + '\n\n';
              case 'strong': case 'b': return '**' + children() + '**';
              case 'em': case 'i': return '*' + children() + '*';
              case 'code':
                if (el.parentElement && el.parentElement.tagName === 'PRE') return children();
                return '`' + children() + '`';
              case 'pre': {
                const codeEl = el.querySelector('code');
                const lang = codeEl ? (codeEl.className.match(/language-(\w+)/)||[])[1] || '' : '';
                const code = codeEl ? (codeEl.innerText || '') : (el.innerText || '');
                return '\n```' + lang + '\n' + code.trim() + '\n```\n\n';
              }
              case 'blockquote': return '\n> ' + children().trim().replace(/\n/g, '\n> ') + '\n\n';
              case 'p': return children().trim() + '\n\n';
              case 'br': return '\n';
              case 'hr': return '\n---\n\n';
              case 'a': {
                const href = el.getAttribute('href') || '';
                const txt = children().trim();
                return href ? '[' + txt + '](' + href + ')' : txt;
              }
              case 'ul': {
                let s = '\n';
                for (const li of el.children) {
                  if (li.tagName === 'LI') s += '- ' + htmlToMd(li).trim() + '\n';
                }
                return s + '\n';
              }
              case 'ol': {
                let s = '\n';
                let idx = 1;
                for (const li of el.children) {
                  if (li.tagName === 'LI') { s += idx + '. ' + htmlToMd(li).trim() + '\n'; idx++; }
                }
                return s + '\n';
              }
              case 'li': return children();
              case 'table': {
                const rows = el.querySelectorAll('tr');
                if (rows.length === 0) return children();
                let md = '\n';
                for (let r = 0; r < rows.length; r++) {
                  const cells = rows[r].querySelectorAll('th, td');
                  const row = [];
                  for (const cell of cells) row.push(htmlToMd(cell).trim().replace(/\|/g, '\\|'));
                  md += '| ' + row.join(' | ') + ' |\n';
                  if (r === 0) md += '| ' + row.map(() => '---').join(' | ') + ' |\n';
                }
                return md + '\n';
              }
              case 'thead': case 'tbody': case 'tfoot': case 'tr':
              case 'th': case 'td':
                return children();
              case 'img': {
                const alt = el.alt || '';
                const src = el.src || '';
                return src ? '![' + alt + '](' + src + ')' : '';
              }
              case 'del': case 's': return '~~' + children() + '~~';
              case 'sup': return '^(' + children() + ')';
              case 'sub': return '~(' + children() + ')';
              default: return children();
            }
          }

          // Clean up excessive blank lines
          function cleanMd(s) {
            return s.replace(/\n{3,}/g, '\n\n').trim();
          }

          // ── Extract text content as Markdown ──
          // Strategy: find .markdown containers inside the turn.
          // After 429 recovery, ChatGPT may render duplicate .markdown
          // blocks for the same assistant turn.  We collect ALL distinct
          // markdown blocks for multi-message delivery, and use the
          // LONGEST one as the primary text fallback.
          let text = '';
          const markdownEls = lastTurn.querySelectorAll('.markdown');
          const allMarkdownTexts = [];
          const domDebug = {
            markdownCount: markdownEls.length,
            markdownLengths: [],
            selectedIdx: -1,
            strategy: 'none',
          };
          if (markdownEls.length > 0) {
            let bestIdx = 0;
            let bestLen = 0;
            for (let i = 0; i < markdownEls.length; i++) {
              const t = cleanMd(htmlToMd(markdownEls[i]));
              domDebug.markdownLengths.push(t.length);
              allMarkdownTexts.push(t);
              if (t.length > bestLen) {
                bestLen = t.length;
                bestIdx = i;
              }
            }
            text = allMarkdownTexts[bestIdx];
            domDebug.selectedIdx = bestIdx;
            domDebug.strategy = 'longest_markdown';
          }
          if (!text) {
            // Fallback: clone the turn, strip UI chrome, use raw innerText.
            const clone = lastTurn.cloneNode(true);
            clone.querySelectorAll(
              'button, nav, [data-testid*="overlay"], [data-testid*="action"]'
            ).forEach(e => e.remove());
            text = (clone.innerText || '').trim();
            domDebug.strategy = 'clone_fallback';
          }

          return JSON.stringify({
            found: true,
            turnId: turnId,
            turnCount: allTurns.length,
            isStreaming: busy,
            text: text,
            textLength: text.length,
            imageCount: images.length,
            images: images.slice(0, 10),
            domDebug: domDebug,
            allMarkdownTexts: allMarkdownTexts.filter(t => t.length > 0),
          });
        })()"#,
    )
    .await
    .ok()?;

    let data: Value = if let Some(s) = result.as_str() {
        serde_json::from_str(s).ok()?
    } else {
        result
    };

    if !data.get("found").and_then(Value::as_bool).unwrap_or(false) {
        let reason = data
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        tracing::debug!(
            reason,
            turn_count = data.get("turnCount").and_then(|v| v.as_u64()).unwrap_or(0),
            "chatgpt_web DOM extract: content not found"
        );
        return None;
    }

    // Don't return content if still streaming — it's incomplete
    if data
        .get("isStreaming")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        tracing::info!(
            turn_id = data.get("turnId").and_then(|v| v.as_str()).unwrap_or(""),
            text_length = data.get("textLength").and_then(|v| v.as_u64()).unwrap_or(0),
            image_count = data.get("imageCount").and_then(|v| v.as_u64()).unwrap_or(0),
            "chatgpt_web DOM extract: found assistant turn but still streaming, skipping"
        );
        return None;
    }

    let text = data
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let image_count = data.get("imageCount").and_then(Value::as_u64).unwrap_or(0);
    let turn_id = data
        .get("turnId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // Log DOM extraction debug info (always, for post-mortem analysis)
    if let Some(debug) = data.get("domDebug") {
        let strategy = debug
            .get("strategy")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let md_count = debug
            .get("markdownCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let selected_idx = debug
            .get("selectedIdx")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let md_lengths: Vec<u64> = debug
            .get("markdownLengths")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        tracing::info!(
            turn_id,
            strategy,
            markdown_count = md_count,
            selected_idx,
            markdown_lengths = ?md_lengths,
            "chatgpt_web DOM extract debug: extraction details"
        );
        // Warn if we detected duplicate .markdown containers
        if md_count > 1 {
            tracing::warn!(
                turn_id,
                markdown_count = md_count,
                markdown_lengths = ?md_lengths,
                selected_idx,
                "chatgpt_web DOM extract: DUPLICATE .markdown containers detected — \
                 429 recovery likely created duplicate content blocks. \
                 Using longest container."
            );
        }
    }

    // Need either text or images. Short replies like "OK" are valid and must
    // still be delivered back to IM.
    if !dom_content_is_usable(&text, image_count as usize) {
        return None;
    }

    tracing::info!(
        turn_id,
        text_length = text.len(),
        image_count,
        "chatgpt_web DOM extract: successfully extracted completed assistant message from page DOM"
    );

    // Build a WaitedFinal that looks like what the API would return
    let mut final_message = json!({
        "id": turn_id,
        "status": "finished_successfully",
        "endTurn": true,
        "text": text,
        "source": "dom_fallback",
    });

    // Attach image info if present
    if image_count > 0 {
        let images = data.get("images").cloned().unwrap_or(json!([]));
        // Convert DOM images to the format our downstream expects
        let generated_images: Vec<Value> = images
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|img| {
                        let src = img.get("src").and_then(Value::as_str).unwrap_or("");
                        json!({
                            // Use "sourceUrl" — this is what
                            // collect_generated_image_download_urls() looks for.
                            "sourceUrl": src,
                            "url": src,
                            "alt": img.get("alt").and_then(Value::as_str).unwrap_or(""),
                            "width": img.get("width"),
                            "height": img.get("height"),
                            "source": "dom_fallback",
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !generated_images.is_empty() {
            final_message["generatedImages"] = json!(generated_images);
            final_message["text"] = json!("已生成图片，正在发送原图。");
        }
    }

    // Build all_texts from the individual markdown blocks if available.
    // This allows per-message IM delivery when ChatGPT produces multiple
    // response segments (thinking + answer) in the same turn.
    //
    // Important: After 429 recovery, ChatGPT may render DUPLICATE .markdown
    // containers (same content twice). We deduplicate by checking if a shorter
    // block is a prefix/substring of a longer one.
    let all_texts: Vec<String> = {
        let md_texts: Vec<String> = data
            .get("allMarkdownTexts")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if md_texts.len() > 1 {
            // Deduplicate: remove blocks that are substrings of other blocks
            let mut unique: Vec<String> = Vec::new();
            for text in &md_texts {
                let is_duplicate = md_texts
                    .iter()
                    .any(|other| other.len() > text.len() && other.contains(text.as_str()));
                if !is_duplicate {
                    // Also skip exact duplicates
                    if !unique.iter().any(|u| u == text) {
                        unique.push(text.clone());
                    }
                }
            }
            if unique.len() > 1 {
                unique
            } else {
                // After dedup only one remains — use primary text
                vec![final_message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()]
            }
        } else {
            // Single block or fallback: use the primary text
            vec![final_message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()]
        }
    };

    Some(WaitedFinal {
        final_message: final_message.clone(),
        summary: json!({
            "source": "dom_fallback",
            "turnId": turn_id,
            "imageCount": image_count,
        }),
        all_texts,
        had_429_or_fallback: true,
    })
}

/// Like `try_extract_latest_from_dom` but returns a richer result that
/// includes streaming status and text_len even when content is still streaming.
/// This allows callers to track content stability without losing information.
pub(super) async fn try_extract_dom_outcome(
    config: &RuntimeConfig,
    profile_dir: &Path,
    conversation_id: &str,
) -> DomExtractOutcome {
    let tab = match super::browser::get_conversation_tab(profile_dir, conversation_id) {
        Some(tab) => tab,
        None => {
            match super::browser::recover_conversation_tab_from_browser(config, conversation_id)
                .await
            {
                Ok(Some(tab)) => tab,
                Ok(None) => return DomExtractOutcome::NotFound,
                Err(error) => {
                    tracing::warn!(
                        conversation_id,
                        error = %error,
                        "chatgpt_web DOM extract: failed to recover existing conversation tab"
                    );
                    return DomExtractOutcome::NotFound;
                }
            }
        }
    };
    if tab.cdp.is_closed() {
        return DomExtractOutcome::NotFound;
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        extract_dom_outcome_inner(&tab.cdp),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                conversation_id,
                "chatgpt_web DOM extract: CDP evaluate timed out after 10s"
            );
            DomExtractOutcome::NotFound
        }
    }
}

/// Inner implementation that returns DomExtractOutcome.
async fn extract_dom_outcome_inner(cdp: &super::CdpClient) -> DomExtractOutcome {
    let js = format!(
        r#"(() => {{
          {html_to_md}

          const path = window.location.pathname || '';
          if (!path.startsWith('/c/')) {{
            return JSON.stringify({{ found: false, reason: 'not_on_conversation_page' }});
          }}
          const isVisible = (el) => !!(
            el && (el.offsetWidth || el.offsetHeight || el.getClientRects().length)
          );
          const roleMessageNodes = Array.from(document.querySelectorAll(
            '[data-message-author-role][data-message-id]'
          )).filter(isVisible);
          const allTurns = Array.from(document.querySelectorAll(
            'section[data-turn-id-container], section[data-testid^="conversation-turn-"]'
          )).filter(isVisible);
          if (allTurns.length === 0 && roleMessageNodes.length === 0) {{
            return JSON.stringify({{ found: false, reason: 'no_turns' }});
          }}
          let lastTurn = null;
          let turnId = '';
          if (allTurns.length > 0) {{
            const lastTurnRole = allTurns[allTurns.length - 1].getAttribute('data-turn');
            if (lastTurnRole !== 'assistant' && roleMessageNodes.length === 0) {{
              return JSON.stringify({{
                found: false,
                reason: 'last_turn_is_' + lastTurnRole,
                turnCount: allTurns.length,
              }});
            }}
            lastTurn = allTurns[allTurns.length - 1];
            turnId = lastTurn.getAttribute('data-turn-id') || '';
          }}
          let assistantBatchNodes = [];
          let provisionalAssistantShell = false;
          if (roleMessageNodes.length > 0) {{
            let lastUserIdx = -1;
            for (let i = 0; i < roleMessageNodes.length; i++) {{
              if (roleMessageNodes[i].getAttribute('data-message-author-role') === 'user') {{
                lastUserIdx = i;
              }}
            }}
            assistantBatchNodes = roleMessageNodes
              .slice(lastUserIdx + 1)
              .filter((node) => node.getAttribute('data-message-author-role') === 'assistant');
            if (assistantBatchNodes.length > 0) {{
              lastTurn = assistantBatchNodes[assistantBatchNodes.length - 1];
              turnId = lastTurn.getAttribute('data-message-id') || turnId;
            }}
          }}
          if (assistantBatchNodes.length === 0 && roleMessageNodes.length > 0 && allTurns.length > 0) {{
            const userNodes = roleMessageNodes
              .filter((node) => node.getAttribute('data-message-author-role') === 'user');
            const lastUserNode = userNodes[userNodes.length - 1];
            if (lastUserNode) {{
              const followsLastUser = (node) => !!(
                lastUserNode.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING
              );
              assistantBatchNodes = allTurns
                .filter(followsLastUser)
                .filter((node) => {{
                  if (node.querySelector('[data-message-author-role="user"]')) return false;
                  if (node.querySelector('[data-message-author-role="assistant"]')) return true;
                  const rawText = (node.innerText || node.textContent || '').trim();
                  const hasGeneratedImage = node.querySelector(
                    'img[alt^="已生成图片"], img[alt^="Generated image"], img[src*="estuary/content"], img[src*="oaidalleapi"], img[src*="dall-e"]'
                  ) !== null;
                  const hasAssistantShell = /^ChatGPT\\s*(说|says)[:：]?/i.test(rawText);
                  return hasGeneratedImage || hasAssistantShell;
                }});
              if (assistantBatchNodes.length > 0) {{
                lastTurn = assistantBatchNodes[assistantBatchNodes.length - 1];
                turnId = lastTurn.getAttribute('data-turn-id') ||
                  lastTurn.getAttribute('data-testid') ||
                  turnId;
                const hasCommittedAssistantMessage = !!(
                  lastTurn.matches('[data-message-author-role="assistant"][data-message-id]') ||
                  lastTurn.querySelector('[data-message-author-role="assistant"][data-message-id]')
                );
                const hasGeneratedImage = lastTurn.querySelector(
                  'img[alt^="已生成图片"], img[alt^="Generated image"], img[src*="estuary/content"], img[src*="oaidalleapi"], img[src*="dall-e"]'
                ) !== null;
                provisionalAssistantShell = !hasCommittedAssistantMessage && !hasGeneratedImage;
              }}
            }}
          }}
          if (assistantBatchNodes.length === 0 && roleMessageNodes.length > 0) {{
            const userNodes = roleMessageNodes
              .filter((node) => node.getAttribute('data-message-author-role') === 'user');
            const lastUserNode = userNodes[userNodes.length - 1];
            const hasGeneratedImageAfterLastUser = !!lastUserNode && allTurns
              .filter((node) => !!(
                lastUserNode.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING
              ))
              .some((node) => node.querySelector(
                'img[alt^="已生成图片"], img[alt^="Generated image"], img[src*="estuary/content"], img[src*="oaidalleapi"], img[src*="dall-e"]'
              ) !== null);
            provisionalAssistantShell = !!lastUserNode && !hasGeneratedImageAfterLastUser;
          }}
          if (!lastTurn) {{
            return JSON.stringify({{ found: false, reason: 'no_assistant_turn' }});
          }}

          // Streaming detection — SCOPED to actual streaming signals only.
          const isStreaming = lastTurn.querySelector(
            '[data-is-streaming="true"], .result-streaming'
          ) !== null;
          const imageGenBusy = lastTurn.querySelector(
            '[id^="image-"] .animate-spin, [id^="image-"] .animate-pulse, [aria-label*="Creating image"], [aria-label*="Generating image"], [aria-label*="正在创建图片"], [aria-label*="正在生成图片"]'
          ) !== null;

          const composerEl = document.querySelector('#prompt-textarea') ||
                             document.querySelector('[data-testid="composer-text-input"]') ||
                             document.querySelector('[contenteditable="true"]') ||
                             document.querySelector('textarea');
          const composerVisible = isVisible(composerEl);
          const composerDisabled = composerEl ? (
            composerEl.disabled ||
            composerEl.readOnly ||
            composerEl.getAttribute('aria-disabled') === 'true'
          ) : true;
          const composerTextLength = composerEl ? (
            (composerEl.value || composerEl.innerText || composerEl.textContent || '').trim().length
          ) : 0;

          const sendBtn = document.querySelector('[data-testid="send-button"]') ||
                          document.querySelector('[data-testid="composer-submit-button"]') ||
                          document.querySelector('button[aria-label="Send prompt"]') ||
                          document.querySelector('button[aria-label*="发送"]') ||
                          document.querySelector('button[class*="composer-submit-button"]') ||
                          document.querySelector('button.composer-submit-btn');
          const sendButtonVisible = isVisible(sendBtn);
          const sendButtonDisabled = sendBtn ? (
            !!sendBtn.disabled ||
            sendBtn.getAttribute('aria-disabled') === 'true' ||
            sendBtn.dataset.disabled === 'true'
          ) : null;

          const stopBtnCandidates = Array.from(document.querySelectorAll(
            '[data-testid="stop-button"], button[aria-label*="Stop"], button[aria-label*="停止"], button[aria-label*="Cancel"], button[aria-label*="取消"]'
          ));
          const stopBtn = stopBtnCandidates.find((button) => {{
            const label = ((button.getAttribute('aria-label') || '') + ' ' + (button.innerText || '')).trim();
            if (!label) return false;
            return /stop\\s*(streaming|generating|responding|response)?|cancel\\s*(generation|response)|停止\\s*(流式传输|生成|回答|回复)|取消\\s*(生成|回答|回复)/i.test(label);
          }}) || null;
          const stopButtonVisible = isVisible(stopBtn);

          // Extract text as Markdown
          let text = '';
          const allMarkdownTexts = [];
          const toolCalls = [];
          const pushToolCall = (node, batchIndex) => {{
            if (!node || !isVisible(node)) return;
            const clone = node.cloneNode(true);
            clone.querySelectorAll('.markdown, button[aria-label*="Copy"], button[aria-label*="复制"]').forEach(e => e.remove());
            const text = (clone.innerText || node.getAttribute('aria-label') || node.textContent || '').trim();
            if (!text || text.length < 2) return;
            const normalized = text.replace(/\s+/g, ' ').trim();
            if (!/(search|searched|searching|web|browse|browsing|read|reading|fetch|fetching|搜索|检索|浏览|读取|访问|网页|来源|工具)/i.test(normalized)) return;
            if (toolCalls.some((tool) => tool.text === normalized)) return;
            const label = node.getAttribute('aria-label') || '';
            const name = (label || normalized.split('\n')[0] || 'ChatGPT Web tool').slice(0, 120);
            toolCalls.push({{
              name,
              text: normalized.slice(0, 1000),
              status: 'finished',
              batchIndex,
            }});
          }};
          if (assistantBatchNodes.length > 0) {{
            for (let batchIndex = 0; batchIndex < assistantBatchNodes.length; batchIndex++) {{
              const batchNode = assistantBatchNodes[batchIndex];
              const batchText = Array.from(batchNode.querySelectorAll('.markdown'))
                .map((el) => cleanMd(htmlToMd(el)))
                .filter((value) => value.length > 0)
                .join('\n\n')
                .trim();
              if (batchText) allMarkdownTexts.push(batchText);
              batchNode.querySelectorAll(
                '[data-testid*="tool"], [data-testid*="search"], [data-testid*="browse"], [aria-label*="Search"], [aria-label*="search"], [aria-label*="Browse"], [aria-label*="browse"], [aria-label*="搜索"], [aria-label*="检索"], [aria-label*="浏览"], [aria-label*="工具"]'
              ).forEach((node) => pushToolCall(node, batchIndex));
            }}
            text = allMarkdownTexts[allMarkdownTexts.length - 1] || '';
          }}
          if (!text) {{
            const markdownEls = lastTurn.querySelectorAll('.markdown');
            let bestLen = 0;
            let bestIdx = 0;
            for (let i = 0; i < markdownEls.length; i++) {{
              const t = cleanMd(htmlToMd(markdownEls[i]));
              allMarkdownTexts.push(t);
              if (t.length > bestLen) {{
                bestLen = t.length;
                bestIdx = i;
              }}
            }}
            text = allMarkdownTexts[bestIdx];
          }}
          if (!text) {{
            const clone = lastTurn.cloneNode(true);
            clone.querySelectorAll(
              'button, nav, [data-testid*="overlay"], [data-testid*="action"]'
            ).forEach(e => e.remove());
            text = (clone.innerText || '').trim();
          }}
          const normalizedStatusText = text
            .replace(/^ChatGPT\\s*(说|says)[:：]?\\s*/i, '')
            .trim();
          const pendingOutputStatusText = /^(正在打草稿|正在撰写|正在思考|正在创建图片|正在生成图片|正在生成图像|Drafting|Thinking|Creating images?|Generating images?)$/i
            .test(normalizedStatusText) ||
            normalizedStatusText.length === 0 ||
            /^(最后微调一下|正在微调|Finishing up|Making final adjustments)\\.?$/i
              .test(normalizedStatusText);
          const bodyText = (document.body && document.body.innerText) || '';
          const lowerBodyText = bodyText.toLowerCase();
          const waitingForCompleteReply =
            (bodyText.includes('连接已中断') && bodyText.includes('等待完整回复')) ||
            (
              lowerBodyText.includes('connection') &&
              lowerBodyText.includes('interrupted') &&
              lowerBodyText.includes('waiting') &&
              (lowerBodyText.includes('complete reply') || lowerBodyText.includes('complete response'))
            );

          // Extract images (deduplicate by src)
          const images = [];
          const seenSrcs = new Set();
          const imgEls = lastTurn.querySelectorAll('img');
          for (const img of imgEls) {{
            const alt = img.alt || '';
            const src = img.src || '';
            if (!src) continue;
            const w = img.naturalWidth || parseInt(img.width) || 0;
            if (w > 0 && w < 50) continue;
            if (alt.startsWith('已生成图片') || alt.startsWith('Generated image') ||
                src.includes('estuary/content') ||
                src.includes('oaidalleapi') || src.includes('dall-e')) {{
              const srcKey = src.split('?')[0] + (new URL(src, location.origin).searchParams.get('id') || src);
              if (seenSrcs.has(srcKey)) continue;
              seenSrcs.add(srcKey);
              images.push({{ alt, src, width: parseInt(img.width) || 0, height: parseInt(img.height) || 0 }});
            }}
          }}
          const artifacts = extractChatgptBehaviorArtifacts(lastTurn);

          return JSON.stringify({{
            found: true,
            turnId: turnId,
            turnCount: allTurns.length,
            isStreaming: isStreaming || imageGenBusy || stopButtonVisible || waitingForCompleteReply || (pendingOutputStatusText && images.length === 0),
            streamingSignal: isStreaming,
            imageGenBusy,
            stopButtonVisible,
            waitingForCompleteReply,
            provisionalAssistantShell,
            composerVisible,
            composerDisabled,
            composerTextLength,
            sendButtonVisible,
            sendButtonDisabled,
            pendingOutputStatusText,
            text: text,
            textLength: text.length,
            imageCount: images.length,
            images: images.slice(0, 10),
            artifactCount: artifacts.length,
            artifacts: artifacts.slice(0, 20),
            toolCalls: toolCalls.slice(0, 20),
            allMarkdownTexts: allMarkdownTexts.filter(t => t.length > 0),
          }});
        }})()"#,
        html_to_md = JS_HTML_TO_MD
    );
    let result = match super::evaluate_value(cdp, &js).await {
        Ok(v) => v,
        Err(_) => return DomExtractOutcome::NotFound,
    };

    let data: Value = if let Some(s) = result.as_str() {
        match serde_json::from_str(s) {
            Ok(v) => v,
            Err(_) => return DomExtractOutcome::NotFound,
        }
    } else {
        result
    };

    if !data.get("found").and_then(|v| v.as_bool()).unwrap_or(false) {
        return DomExtractOutcome::NotFound;
    }

    let text = data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let text_len = text.len();
    let image_count = data.get("imageCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let turn_id = data
        .get("turnId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(reason) = dom_output_in_progress_reason(&data) {
        tracing::info!(
            turn_id,
            text_len,
            image_count,
            reason,
            "chatgpt_web DOM extract: output still active, reporting text_len for stability tracking"
        );
        return DomExtractOutcome::Streaming {
            text_len,
            image_count: image_count as usize,
            reason,
        };
    }

    // Not streaming — build the WaitedFinal. Short replies are valid content.
    if !dom_content_is_usable(&text, image_count as usize) {
        return DomExtractOutcome::NotFound;
    }

    tracing::info!(
        turn_id,
        text_len,
        image_count,
        "chatgpt_web DOM extract outcome: complete content found"
    );

    // Build final_message
    let all_texts: Vec<String> = data
        .get("allMarkdownTexts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let images: Vec<Value> = data
        .get("images")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let artifacts: Vec<Value> = data
        .get("artifacts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let artifact_count = artifacts.len();

    let mut final_message = json!({
        "text": text,
        "turnId": turn_id,
        "source": "dom_fallback",
        "imageCount": image_count,
    });

    if image_count > 0 && !images.is_empty() {
        let generated_images: Vec<Value> = images
            .iter()
            .map(|img| {
                let src = img.get("src").and_then(Value::as_str).unwrap_or("");
                json!({
                    "sourceUrl": src,
                    "url": src,
                    "alt": img.get("alt").and_then(Value::as_str).unwrap_or(""),
                    "width": img.get("width"),
                    "height": img.get("height"),
                    "source": "dom_fallback",
                })
            })
            .collect();
        final_message["generatedImages"] = json!(generated_images);
        final_message["text"] = json!("已生成图片，正在发送原图。");
    }

    let mut final_text = final_message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if artifact_count > 0 {
        final_text = append_dom_artifact_notice(&final_text, &artifacts);
        final_message["text"] = json!(final_text);
        final_message["artifacts"] = json!(artifacts.clone());
    }

    DomExtractOutcome::Ready(WaitedFinal {
        final_message,
        summary: json!({
            "source": "dom_fallback_outcome",
            "turnId": turn_id,
            "imageCount": image_count,
            "artifactCount": artifact_count,
            "artifacts": artifacts,
            "toolCalls": data.get("toolCalls").cloned().unwrap_or_else(|| json!([])),
        }),
        all_texts: if artifact_count > 0 {
            vec![final_text]
        } else if all_texts.is_empty() {
            vec![text]
        } else {
            all_texts
        },
        had_429_or_fallback: true,
    })
}

/// Like `try_extract_latest_from_dom` but ignores streaming indicators.
/// Used when content stability has been confirmed despite stale streaming flags.
pub(super) async fn try_extract_latest_from_dom_force(
    config: &RuntimeConfig,
    profile_dir: &Path,
    conversation_id: &str,
) -> Option<WaitedFinal> {
    match try_extract_dom_outcome(config, profile_dir, conversation_id).await {
        DomExtractOutcome::Ready(waited) => Some(waited),
        DomExtractOutcome::Streaming { .. } => {
            // Force extraction — streaming is stale, get content anyway
            let tab =
                super::browser::recover_conversation_tab_from_browser(config, conversation_id)
                    .await
                    .ok()
                    .flatten()?;
            if tab.cdp.is_closed() {
                return None;
            }
            extract_latest_assistant_from_dom_ignore_streaming(&tab.cdp).await
        }
        DomExtractOutcome::NotFound => None,
    }
}

/// Extract DOM content ignoring streaming indicators.
/// Used when stability detection has confirmed the streaming flag is stale.
async fn extract_latest_assistant_from_dom_ignore_streaming(
    cdp: &super::CdpClient,
) -> Option<WaitedFinal> {
    // Reuse the same JS as the main extraction but skip the isStreaming check
    // at the Rust level. The JS still reports isStreaming for logging.
    let js = format!(
        r#"(() => {{
          {html_to_md}

          const path = window.location.pathname || '';
          if (!path.startsWith('/c/')) {{
            return JSON.stringify({{ found: false, reason: 'not_on_conversation_page' }});
          }}
          const allTurns = document.querySelectorAll(
            'section[data-turn-id-container]'
          );
          if (allTurns.length === 0) {{
            return JSON.stringify({{ found: false, reason: 'no_turns' }});
          }}
          const lastTurnRole = allTurns[allTurns.length - 1].getAttribute('data-turn');
          if (lastTurnRole !== 'assistant') {{
            return JSON.stringify({{
              found: false,
              reason: 'last_turn_is_' + lastTurnRole,
              turnCount: allTurns.length,
            }});
          }}
          const lastTurn = allTurns[allTurns.length - 1];
          const turnId = lastTurn.getAttribute('data-turn-id') || '';

          // Extract text as Markdown
          let text = '';
          const markdownEls = lastTurn.querySelectorAll('.markdown');
          const allMarkdownTexts = [];
          if (markdownEls.length > 0) {{
            let bestLen = 0;
            let bestIdx = 0;
            for (let i = 0; i < markdownEls.length; i++) {{
              const t = cleanMd(htmlToMd(markdownEls[i]));
              allMarkdownTexts.push(t);
              if (t.length > bestLen) {{
                bestLen = t.length;
                bestIdx = i;
              }}
            }}
            text = allMarkdownTexts[bestIdx];
          }}
          if (!text) {{
            const clone = lastTurn.cloneNode(true);
            clone.querySelectorAll(
              'button, nav, [data-testid*="overlay"], [data-testid*="action"]'
            ).forEach(e => e.remove());
            text = (clone.innerText || '').trim();
          }}

          // Extract images (deduplicate by src)
          const images = [];
          const seenSrcs = new Set();
          const imgEls = lastTurn.querySelectorAll('img');
          for (const img of imgEls) {{
            const alt = img.alt || '';
            const src = img.src || '';
            if (!src) continue;
            const w = img.naturalWidth || parseInt(img.width) || 0;
            if (w > 0 && w < 50) continue;
            if (alt.startsWith('已生成图片') || alt.startsWith('Generated image') ||
                src.includes('estuary/content') ||
                src.includes('oaidalleapi') || src.includes('dall-e')) {{
              const srcKey = src.split('?')[0] + (new URL(src, location.origin).searchParams.get('id') || src);
              if (seenSrcs.has(srcKey)) continue;
              seenSrcs.add(srcKey);
              images.push({{ alt, src, width: parseInt(img.width) || 0, height: parseInt(img.height) || 0 }});
            }}
          }}
          const artifacts = extractChatgptBehaviorArtifacts(lastTurn);

          return JSON.stringify({{
            found: true,
            turnId: turnId,
            turnCount: allTurns.length,
            text: text,
            textLength: text.length,
            imageCount: images.length,
            images: images.slice(0, 10),
            artifactCount: artifacts.length,
            artifacts: artifacts.slice(0, 20),
            allMarkdownTexts: allMarkdownTexts.filter(t => t.length > 0),
          }});
        }})()"#,
        html_to_md = JS_HTML_TO_MD
    );
    let result = super::evaluate_value(cdp, &js).await.ok()?;

    let data: Value = if let Some(s) = result.as_str() {
        serde_json::from_str(s).ok()?
    } else {
        result
    };

    if !data.get("found").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }

    // SKIP the isStreaming check — that's the whole point of this function

    let text = data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let image_count = data.get("imageCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let turn_id = data
        .get("turnId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if !dom_content_is_usable(&text, image_count as usize) {
        return None;
    }

    tracing::info!(
        turn_id,
        text_length = text.len(),
        image_count,
        "chatgpt_web DOM extract: force-extracted content (ignoring streaming flag)"
    );

    // Build WaitedFinal (same as the Ready path in extract_dom_outcome_inner)
    let all_texts: Vec<String> = data
        .get("allMarkdownTexts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let images: Vec<Value> = data
        .get("images")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let artifacts: Vec<Value> = data
        .get("artifacts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let artifact_count = artifacts.len();

    let mut final_message = json!({
        "text": text,
        "turnId": turn_id,
        "source": "dom_fallback",
        "imageCount": image_count,
    });

    if image_count > 0 && !images.is_empty() {
        let generated_images: Vec<Value> = images
            .iter()
            .map(|img| {
                let src = img.get("src").and_then(Value::as_str).unwrap_or("");
                json!({
                    "sourceUrl": src,
                    "url": src,
                    "alt": img.get("alt").and_then(Value::as_str).unwrap_or(""),
                    "width": img.get("width"),
                    "height": img.get("height"),
                    "source": "dom_fallback",
                })
            })
            .collect();
        final_message["generatedImages"] = json!(generated_images);
        final_message["text"] = json!("已生成图片，正在发送原图。");
    }

    let mut final_text = final_message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if artifact_count > 0 {
        final_text = append_dom_artifact_notice(&final_text, &artifacts);
        final_message["text"] = json!(final_text);
        final_message["artifacts"] = json!(artifacts.clone());
    }

    Some(WaitedFinal {
        final_message,
        summary: json!({
            "source": "dom_fallback_force",
            "turnId": turn_id,
            "imageCount": image_count,
            "artifactCount": artifact_count,
            "artifacts": artifacts,
        }),
        all_texts: if artifact_count > 0 {
            vec![final_text]
        } else if all_texts.is_empty() {
            vec![text]
        } else {
            all_texts
        },
        had_429_or_fallback: true,
    })
}

pub(super) fn dom_content_is_usable(text: &str, image_count: usize) -> bool {
    if image_count > 0 {
        return true;
    }
    let normalized = normalize_dom_status_text(text);
    !normalized.is_empty() && !dom_text_is_pending_status(normalized)
}

pub(super) fn required_dom_terminal_idle_for(_stream_handoff: bool) -> std::time::Duration {
    std::time::Duration::ZERO
}

pub(super) fn dom_terminal_content_settle_for(
    text_len: usize,
    image_count: usize,
) -> std::time::Duration {
    if image_count > 0 {
        return std::time::Duration::from_secs(2);
    }
    if text_len >= 4_096 {
        return std::time::Duration::from_secs(5);
    }
    if text_len >= 256 {
        return std::time::Duration::from_secs(3);
    }
    std::time::Duration::from_secs(2)
}

pub(super) fn dom_terminal_content_settle_poll_for() -> std::time::Duration {
    std::time::Duration::from_millis(500)
}

fn dom_ready_signature(waited: &WaitedFinal) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&waited.final_message)
        .unwrap_or_default()
        .hash(&mut hasher);
    waited.all_texts.hash(&mut hasher);
    serde_json::to_string(&waited.summary)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

fn normalize_dom_status_text(text: &str) -> &str {
    text.trim()
        .trim_start_matches('\u{feff}')
        .trim_start_matches("ChatGPT 说")
        .trim_start_matches("ChatGPT says")
        .trim_start_matches([':', '：'])
        .trim()
}

fn dom_text_is_pending_status(normalized: &str) -> bool {
    let trimmed = normalized.trim().trim_end_matches(['.', '。', '…']).trim();
    if trimmed.is_empty() {
        return true;
    }
    if dom_text_is_waiting_for_complete_reply(trimmed) {
        return true;
    }
    matches!(
        trimmed,
        "正在打草稿"
            | "正在撰写"
            | "正在思考"
            | "正在创建图片"
            | "正在生成图片"
            | "正在生成图像"
            | "最后微调一下"
            | "正在微调"
            | "Drafting"
            | "Thinking"
            | "Creating image"
            | "Creating images"
            | "Generating image"
            | "Generating images"
            | "Finishing up"
            | "Making final adjustments"
    )
}

fn dom_text_is_waiting_for_complete_reply(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    (trimmed.contains("连接已中断") && trimmed.contains("等待完整回复"))
        || (lower.contains("connection")
            && lower.contains("interrupted")
            && lower.contains("waiting")
            && (lower.contains("complete reply") || lower.contains("complete response")))
}

pub(super) fn dom_output_in_progress_reason(data: &Value) -> Option<&'static str> {
    if data
        .get("streamingSignal")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("streaming_signal");
    }
    if data
        .get("imageGenBusy")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("image_generation_busy");
    }
    if data
        .get("stopButtonVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("stop_button_visible");
    }
    if data
        .get("waitingForCompleteReply")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("waiting_for_complete_reply");
    }
    if data
        .get("provisionalAssistantShell")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("assistant_message_not_committed");
    }
    let composer_visible = data
        .get("composerVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let composer_disabled = data
        .get("composerDisabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let composer_text_length = data
        .get("composerTextLength")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let composer_idle = composer_visible && !composer_disabled && composer_text_length == 0;
    if composer_idle {
        return None;
    }
    let image_count = data
        .get("imageCount")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if image_count == 0
        && data
            .get("pendingOutputStatusText")
            .or_else(|| data.get("pendingImageStatusText"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Some("pending_output_status_text");
    }
    if data
        .get("isStreaming")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("output_busy");
    }
    if !composer_visible {
        return Some("composer_not_visible");
    }
    if composer_disabled {
        return Some("composer_disabled");
    }
    if composer_text_length > 0 {
        return Some("composer_has_unsent_text");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn required_dom_terminal_idle_for_returns_immediately_after_controls_idle() {
        assert!(required_dom_terminal_idle_for(false).is_zero());
        assert!(required_dom_terminal_idle_for(true).is_zero());
    }

    #[test]
    fn dom_terminal_content_settle_waits_after_controls_idle_for_markdown_render() {
        assert_eq!(
            dom_terminal_content_settle_for(32, 0),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            dom_terminal_content_settle_for(512, 0),
            std::time::Duration::from_secs(3)
        );
        assert_eq!(
            dom_terminal_content_settle_for(0, 1),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            dom_terminal_content_settle_for(4_096, 0),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            dom_terminal_content_settle_poll_for(),
            std::time::Duration::from_millis(500)
        );
    }

    #[test]
    fn dom_pending_status_detects_waiting_for_complete_reply() {
        assert!(!dom_content_is_usable("连接已中断。正在等待完整回复", 0));
        assert!(!dom_content_is_usable(
            "Connection interrupted. Waiting for complete response",
            0
        ));
        assert!(dom_content_is_usable("## 今日日报\n\n工作项完整输出。", 0));
    }

    #[test]
    fn dom_output_in_progress_reason_detects_waiting_for_complete_reply() {
        let data = json!({
            "waitingForCompleteReply": true,
            "imageCount": 0,
            "isStreaming": false,
        });

        assert_eq!(
            dom_output_in_progress_reason(&data),
            Some("waiting_for_complete_reply")
        );
    }

    #[test]
    fn dom_output_in_progress_reason_uses_stop_button_before_text_state() {
        let data = json!({
            "streamingSignal": false,
            "imageGenBusy": false,
            "stopButtonVisible": true,
            "waitingForCompleteReply": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "isStreaming": false,
            "text": "我会直接把这份上传的 Markdown 当作完整输入来整理。",
        });

        assert_eq!(
            dom_output_in_progress_reason(&data),
            Some("stop_button_visible")
        );
    }

    #[test]
    fn dom_output_in_progress_reason_waits_for_committed_assistant_message() {
        let data = json!({
            "streamingSignal": false,
            "imageGenBusy": false,
            "stopButtonVisible": false,
            "waitingForCompleteReply": false,
            "provisionalAssistantShell": true,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "isStreaming": false,
            "text": "我先按国内、国际、科技和财经四个板块筛选。",
        });

        assert_eq!(
            dom_output_in_progress_reason(&data),
            Some("assistant_message_not_committed")
        );

        let committed = json!({
            "provisionalAssistantShell": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "isStreaming": false,
            "text": "# 7 月 14 日晚间要闻\n\n完整正文",
        });
        assert_eq!(dom_output_in_progress_reason(&committed), None);
    }

    #[test]
    fn dom_ready_signature_detects_equal_length_content_replacement() {
        let waited = |text: &str| WaitedFinal {
            final_message: json!({
                "text": text,
                "turnId": "assistant-message-1",
                "source": "dom_fallback",
                "imageCount": 0,
            }),
            summary: json!({"source": "dom_fallback_outcome"}),
            all_texts: vec![text.to_string()],
            had_429_or_fallback: true,
        };

        assert_ne!(
            dom_ready_signature(&waited("ABCD")),
            dom_ready_signature(&waited("WXYZ"))
        );
    }

    #[test]
    fn has_in_progress_messages_respects_after_time() {
        let conversation = json!({
            "mapping": {
                "early": {"message": {"create_time": 10.0, "status": "in_progress"}},
                "late": {"message": {"create_time": 20.0, "status": "finished_successfully"}}
            }
        });

        assert!(has_in_progress_messages(&conversation, None));
        assert!(!has_in_progress_messages(&conversation, Some(15.0)));
        assert!(!has_in_progress_messages(&json!({}), None));
    }

    #[test]
    fn find_final_assistant_ignores_non_assistant_and_empty_text() {
        let conversation = json!({
            "mapping": {
                "user": {
                    "message": {
                        "id": "u1",
                        "author": {"role": "user"},
                        "create_time": 1.0,
                        "content": {"parts": ["hello"]}
                    }
                },
                "assistant_empty": {
                    "message": {
                        "id": "a0",
                        "author": {"role": "assistant"},
                        "create_time": 2.0,
                        "status": "finished_successfully",
                        "content": {"parts": ["   "]}
                    }
                },
                "assistant_final": {
                    "message": {
                        "id": "a1",
                        "author": {"role": "assistant"},
                        "create_time": 3.0,
                        "status": "finished_successfully",
                        "end_turn": true,
                        "metadata": {"default_model_slug": "gpt-4"},
                        "content": {"parts": ["answer"]}
                    }
                }
            }
        });

        let final_msg = find_final_assistant(&conversation, None).expect("final message");
        assert_eq!(final_msg.get("id").and_then(Value::as_str), Some("a1"));
        assert_eq!(
            final_msg.get("model").and_then(Value::as_str),
            Some("gpt-4")
        );
        assert_eq!(
            final_msg.get("text").and_then(Value::as_str),
            Some("answer")
        );
    }

    #[test]
    fn find_all_assistant_texts_merges_multiple_finished_messages() {
        let conversation = json!({
            "mapping": {
                "a1": {
                    "message": {
                        "id": "a1",
                        "author": {"role": "assistant"},
                        "create_time": 1.0,
                        "status": "finished_successfully",
                        "content": {"parts": ["step 1"]}
                    }
                },
                "a2": {
                    "message": {
                        "id": "a2",
                        "author": {"role": "assistant"},
                        "create_time": 2.0,
                        "status": "finished_successfully",
                        "content": {"parts": ["step 2"]}
                    }
                },
                "in_progress": {
                    "message": {
                        "id": "a3",
                        "author": {"role": "assistant"},
                        "create_time": 3.0,
                        "status": "in_progress",
                        "content": {"parts": ["ignored"]}
                    }
                }
            }
        });

        let (merged, texts) = find_all_assistant_texts(&conversation, None).expect("merged texts");
        assert_eq!(texts, vec!["step 1".to_string(), "step 2".to_string()]);
        assert_eq!(
            merged.get("text").and_then(Value::as_str),
            Some("step 1\n\nstep 2")
        );
        assert_eq!(merged.get("id").and_then(Value::as_str), Some("a2"));
    }

    #[test]
    fn extract_generated_image_placeholders_parses_paths_and_dimensions() {
        let text = "Generated images from the last image_gen call were saved at:\n\
- /mnt/data/cat1.png (wxh = 1024 x 1024)\n\
- /mnt/data/cat2.png (wxh = 512 x 512)";
        let images = extract_generated_image_placeholders(text);
        assert_eq!(images.len(), 2);
        assert_eq!(
            images[0].get("sourcePath").and_then(Value::as_str),
            Some("/mnt/data/cat1.png")
        );
        assert_eq!(
            images[0].get("dimensions").and_then(Value::as_str),
            Some("1024 x 1024)")
        );
        assert_eq!(
            images[1].get("sourcePath").and_then(Value::as_str),
            Some("/mnt/data/cat2.png")
        );
    }

    #[test]
    fn collect_and_attach_generated_image_assets_deduplicates_and_aligns() {
        let message = json!({
            "content": {
                "parts": [
                    {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_1", "width": 640_u64, "height": 480_u64, "size_bytes": 11_u64},
                    {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_2", "width": 800_u64, "height": 600_u64, "size_bytes": 22_u64},
                    {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_1", "width": 1_u64, "height": 1_u64, "size_bytes": 1_u64}
                ]
            }
        });
        let assets = collect_generated_image_assets_from_message(&message, 123.0);
        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets[0].get("assetPointer").and_then(Value::as_str),
            Some("sediment://file_cat_1")
        );
        assert_eq!(
            assets[1].get("fileId").and_then(Value::as_str),
            Some("file_cat_2")
        );

        let mut placeholders = vec![
            json!({"sourcePath": "/mnt/data/cat1.png"}),
            json!({"sourcePath": "/mnt/data/cat2.png"}),
        ];
        attach_generated_image_assets(&mut placeholders, &assets);
        assert_eq!(
            placeholders[0].get("fileId").and_then(Value::as_str),
            Some("file_cat_1")
        );
        assert_eq!(
            placeholders[1].get("width").and_then(Value::as_u64),
            Some(800)
        );
    }

    #[test]
    fn dom_content_is_usable_considers_images_and_text() {
        assert!(dom_content_is_usable("", 1));
        assert!(!dom_content_is_usable("   ", 0));
        assert!(dom_content_is_usable("ChatGPT 说：OK", 0));
    }

    #[test]
    fn event_helper_builds_progress_event() {
        let raw = json!({"phase": "test"});
        let evt = event(
            ExternalCliProgressEventType::AssistantDelta,
            "hello",
            Some("title"),
            raw.clone(),
        );
        assert!(matches!(
            evt.event_type,
            ExternalCliProgressEventType::AssistantDelta
        ));
        assert_eq!(evt.content, "hello");
        assert_eq!(evt.title.as_deref(), Some("title"));
        assert_eq!(evt.raw, raw);
    }
}
