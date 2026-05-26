//! Memory extraction: auto-extract memories from conversation turns.

use crate::config::AgentConfig;
use crate::memory::consolidation::run_phase2_consolidation;
use crate::memory::consolidation::{load_phase2_state, save_phase2_state};
use crate::memory::constants::*;
use crate::memory::layout::ensure_memory_layout;
use crate::memory::parse::parse_extracted_memories;
use crate::memory::parse::phase1_output_schema;
use crate::memory::pollution::{PollutionDetector, ThreadMemoryMode};
use crate::memory::read_path::generate_memories_enabled;
use crate::memory::retention::prune_memory_artifacts;
use crate::memory::telemetry::telemetry_event;
use crate::memory::utils::{now_secs, truncate_middle_approx_tokens};
use crate::memory::write::write_phase1_extraction;
use crate::memory_guard;
use crate::memory_prompts::{EXTRACT_INPUT_TEMPLATE, EXTRACT_SYSTEM_PROMPT};
use crate::session::AgentSession;
use crate::types::ChatMessage;
use std::time::Duration;
use tracing::{debug, warn};

/// Generate durable file-backed memories after a turn.
///
/// Spawns a background task so that the turn can return immediately.
///
/// **Deprecated**: prefer [`auto_extract_after_turn_with_pollution_check`] which
/// skips extraction when the session is polluted by external context.
#[deprecated(note = "use auto_extract_after_turn_with_pollution_check instead")]
pub fn auto_extract_after_turn(
    client: std::sync::Arc<crate::client::AgentClient>,
    config: AgentConfig,
    session_key: String,
    user_message: String,
    assistant_message: String,
) {
    if !generate_memories_enabled(&config) {
        return;
    }
    tokio::spawn(async move {
        let begin = std::time::Instant::now();
        telemetry_event("auto_extract.begin", 0, true, None);
        let deadline = Duration::from_secs(MEMORY_EXTRACT_TIMEOUT_SECS);
        let work = auto_extract_after_turn_inner(
            &client,
            &config,
            &session_key,
            &user_message,
            &assistant_message,
        );
        match tokio::time::timeout(deadline, work).await {
            Ok(Ok(())) => {
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    true,
                    None,
                );
            }
            Ok(Err(error)) => {
                warn!(error = %error, "failed to generate file-backed memories");
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    false,
                    Some(&error),
                );
            }
            Err(_) => {
                warn!(
                    secs = MEMORY_EXTRACT_TIMEOUT_SECS,
                    "auto memory extraction timed out"
                );
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    false,
                    Some("timeout"),
                );
            }
        }
    });
}

/// Synchronous variant that drives extraction deterministically without
/// spawning a task. Intended for tests and for rare callers that must observe
/// the final on-disk memory state before returning.
pub async fn auto_extract_after_turn_blocking(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    user_message: &str,
    assistant_message: &str,
) -> Result<(), String> {
    if !generate_memories_enabled(config) {
        return Ok(());
    }
    auto_extract_after_turn_inner(
        client,
        config,
        &session.session_key,
        user_message,
        assistant_message,
    )
    .await
}

async fn auto_extract_after_turn_inner(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session_key: &str,
    user_message: &str,
    assistant_message: &str,
) -> Result<(), String> {
    ensure_memory_layout()?;

    let user_message = user_message.trim();
    let assistant_message = assistant_message.trim();
    let user_message = memory_guard::filter_developer_content(user_message);
    let assistant_message = memory_guard::filter_developer_content(assistant_message);
    let user_message = user_message.trim();
    let assistant_message = assistant_message.trim();
    if user_message.is_empty() && assistant_message.is_empty() {
        return Ok(());
    }

    let mut extract_config = config.clone();
    if let Some(model) = config
        .get_memories_config()
        .extract_model
        .as_ref()
        .filter(|model| !model.trim().is_empty())
    {
        extract_config.model = Some(model.trim().to_string());
    }

    let prompt = EXTRACT_INPUT_TEMPLATE
        .replace("{session_key}", session_key)
        .replace(
            "{user_message}",
            &truncate_middle_approx_tokens(user_message, MEMORY_EXTRACT_USER_LIMIT_TOKENS),
        )
        .replace(
            "{assistant_message}",
            &truncate_middle_approx_tokens(
                assistant_message,
                MEMORY_EXTRACT_ASSISTANT_LIMIT_TOKENS,
            ),
        );
    let response = client
        .chat_completion_with_schema(
            &extract_config,
            &[
                ChatMessage::system(EXTRACT_SYSTEM_PROMPT),
                ChatMessage::user(&prompt),
            ],
            &[],
            Some(&phase1_output_schema()),
        )
        .await?;
    let content = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_default();
    let extracted = parse_extracted_memories(&content);
    let mut wrote_memory = false;

    if extracted
        .raw_memory
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        write_phase1_extraction(session_key, &extracted)?;
        wrote_memory = true;
    }

    if wrote_memory {
        if let Err(error) = prune_memory_artifacts(config) {
            warn!(error = %error, "memory retention sweep failed");
        }
        let consolidation_deadline = Duration::from_secs(MEMORY_CONSOLIDATION_TIMEOUT_SECS);
        match tokio::time::timeout(
            consolidation_deadline,
            run_phase2_consolidation(client, config),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(error = %error, "phase-2 consolidation failed");
                bump_phase2_failure(&error);
            }
            Err(_) => {
                warn!(
                    secs = MEMORY_CONSOLIDATION_TIMEOUT_SECS,
                    "phase-2 consolidation timed out"
                );
                bump_phase2_failure("timeout");
            }
        }
    }
    Ok(())
}

pub(crate) fn bump_phase2_failure(reason: &str) {
    if let Ok(root) = ensure_memory_layout() {
        let mut state = load_phase2_state(&root);
        state.failure_count = state.failure_count.saturating_add(1);
        if state.failure_count >= MEMORY_CONSOLIDATION_FAILURE_LIMIT {
            state.pinned_failure_hash = Some(state.last_input_hash.clone());
        }
        state.updated_at_unix = now_secs();
        let _ = save_phase2_state(&root, &state);
        telemetry_event(
            "phase2.failure",
            state.failure_count as u64,
            false,
            Some(reason),
        );
    }
}

// ---------------------------------------------------------------------------
// Pollution-aware extraction
// ---------------------------------------------------------------------------

/// Generate memories after a turn with pollution awareness.
///
/// This is the preferred entry point for callers that have a `PollutionDetector`
/// available. If the session is polluted, extraction is skipped entirely.
pub fn auto_extract_after_turn_with_pollution_check(
    client: std::sync::Arc<crate::client::AgentClient>,
    config: AgentConfig,
    session_key: String,
    user_message: String,
    assistant_message: String,
    pollution_detector: PollutionDetector,
) {
    if !generate_memories_enabled(&config) {
        return;
    }

    // Check pollution state before spawning the extraction task.
    if !pollution_detector.allows_memory_write() {
        let mode = pollution_detector.current_mode();
        debug!(
            mode = ?mode,
            session_key = %session_key,
            "skipping memory extraction due to pollution/disabled state"
        );
        telemetry_event(
            "auto_extract.skip_polluted",
            0,
            true,
            match &mode {
                ThreadMemoryMode::Polluted { reason } => Some(reason.as_str()),
                ThreadMemoryMode::Disabled => Some("disabled"),
                ThreadMemoryMode::Enabled => None,
            },
        );
        return;
    }

    // Delegate to the standard extraction path.
    #[allow(deprecated)]
    auto_extract_after_turn(client, config, session_key, user_message, assistant_message);
}

// ---------------------------------------------------------------------------
// Token limit self-adaptive
// ---------------------------------------------------------------------------

/// Compute the rollout token limit based on model context window.
///
/// Returns 70% of the effective context window, aligned with Codex's approach.
/// Falls back to `DEFAULT_ROLLOUT_TOKEN_LIMIT` if context window is unknown.
#[allow(dead_code)]
pub fn compute_rollout_token_limit(context_window_tokens: Option<usize>) -> usize {
    match context_window_tokens {
        Some(window) if window > 0 => (window * ROLLOUT_CONTEXT_WINDOW_PERCENT / 100).max(1),
        _ => DEFAULT_ROLLOUT_TOKEN_LIMIT,
    }
}

// ---------------------------------------------------------------------------
// Batch extraction (parallel)
// ---------------------------------------------------------------------------

/// Content of a turn to be extracted.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TurnContent {
    pub session_key: String,
    pub user_message: String,
    pub assistant_message: String,
}

/// Result of a single extraction attempt.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExtractionResult {
    pub session_key: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Extract memories from multiple turns in parallel.
///
/// `concurrency_limit` controls the maximum number of concurrent extraction
/// calls. This is useful for batch processing of historical turns.
///
/// Callers should ensure pollution checking is done BEFORE calling this
/// function (i.e., do not pass polluted turns into the batch).
#[allow(dead_code)]
pub async fn extract_batch(
    client: std::sync::Arc<crate::client::AgentClient>,
    config: &AgentConfig,
    turns: Vec<TurnContent>,
    concurrency_limit: usize,
) -> Vec<ExtractionResult> {
    use futures::stream::{self, StreamExt};

    if turns.is_empty() {
        return Vec::new();
    }

    let concurrency = concurrency_limit.max(1);

    stream::iter(turns)
        .map(|turn| {
            let client = client.clone();
            let config = config.clone();
            async move {
                let result = auto_extract_after_turn_inner(
                    &client,
                    &config,
                    &turn.session_key,
                    &turn.user_message,
                    &turn.assistant_message,
                )
                .await;
                ExtractionResult {
                    session_key: turn.session_key,
                    success: result.is_ok(),
                    error: result.err(),
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await
}
