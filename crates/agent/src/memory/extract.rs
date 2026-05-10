//! Memory extraction: auto-extract memories from conversation turns.

use crate::config::AgentConfig;
use crate::memory::consolidation::run_phase2_consolidation;
use crate::memory::consolidation::{load_phase2_state, save_phase2_state};
use crate::memory::constants::*;
use crate::memory::layout::ensure_memory_layout;
use crate::memory::parse::parse_extracted_memories;
use crate::memory::parse::phase1_output_schema;
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
use tracing::warn;

/// Generate durable file-backed memories after a turn.
///
/// Spawns a background task so that the turn can return immediately.
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
