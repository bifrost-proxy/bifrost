//! Memory compaction: summarize conversation history when context grows too long.
//!
//! Inspired by Codex's compaction system (Memento strategy):
//! 1. Append COMPACTION_PROMPT as a structured user input to cloned history
//! 2. Generate a handoff summary
//! 3. Reconstruct history with recent user messages + summary_prefix + summary
//! 4. Track compaction count and token savings
//!
//! Key concepts aligned with Codex:
//! - `CompactionTrigger`: Auto (threshold-based) vs Manual (user-requested)
//! - `CompactionPhase`: PreTurn (before user message) vs MidTurn (during tool loop)
//! - `InitialContextInjection`: Where to place the summary after compaction
//! - `AutoCompactWindow`: per-cycle status/debug telemetry
//! - `CompactionAnalytics`: Structured observability events
//! - `CompactionHook`: Pre/post lifecycle callbacks

use crate::client::AgentClient;
use crate::compaction_hooks::{
    CompactionAnalytics, CompactionContext, CompactionHookResult, CompactionImplementation,
    CompactionStatus, PostCompactOutcome, PreCompactOutcome,
};
use crate::config::AgentConfig;
use crate::history;
use crate::session::AgentSession;
use crate::session_status::{
    snapshot_agent_context, AgentCompactionProgress, AgentTurnProgressEvent,
};
use crate::token_counting::TokenUsageSnapshot;
use crate::turn_runtime::CancellationToken;
use crate::types::ChatMessage;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Compaction enums (matching Codex's analytics taxonomy)
// ---------------------------------------------------------------------------

/// What triggered the compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// System-initiated when token limit is reached.
    Auto,
    /// User-initiated via /compact command.
    Manual,
}

/// Why the compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    /// Context window token limit reached.
    ContextLimit,
    /// User explicitly requested compaction.
    UserRequested,
}

/// Phase of the turn when compaction occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionPhase {
    /// Before the turn starts (pre-turn check).
    PreTurn,
    /// During the turn (between tool call iterations).
    MidTurn,
    /// Standalone compaction turn (manual /compact).
    StandaloneTurn,
}

fn compaction_trigger_label(trigger: CompactionTrigger) -> &'static str {
    match trigger {
        CompactionTrigger::Auto => "auto",
        CompactionTrigger::Manual => "manual",
    }
}

fn compaction_reason_label(reason: CompactionReason) -> &'static str {
    match reason {
        CompactionReason::ContextLimit => "context_limit",
        CompactionReason::UserRequested => "user_requested",
    }
}

fn compaction_phase_label(phase: CompactionPhase) -> &'static str {
    match phase {
        CompactionPhase::PreTurn => "pre_turn",
        CompactionPhase::MidTurn => "mid_turn",
        CompactionPhase::StandaloneTurn => "standalone_turn",
    }
}

/// Controls whether compaction replacement history includes caller-provided
/// initial context items.
///
/// Pre-turn / manual compaction uses `DoNotInject`: the next regular turn will
/// naturally reinject the system prompt via `build_messages()`.
///
/// Mid-turn compaction uses `BeforeLastUserMessage`: context items are spliced
/// into the compacted history immediately before the last real user message,
/// matching Codex's `insert_initial_context_before_last_real_user_or_summary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialContextInjection {
    /// Inject initial context before the last user message in compacted history.
    BeforeLastUserMessage,
    /// Do not inject; the caller will handle context injection.
    DoNotInject,
}

// ---------------------------------------------------------------------------
// Compaction prompt templates (from Codex core/templates/compact/)
// ---------------------------------------------------------------------------

/// The compaction prompt instructs the model to create a handoff summary.
/// Mirrored from Codex's `templates/compact/prompt.md`.
const COMPACTION_PROMPT: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work.
";

/// The summary prefix is prepended to the compacted summary when injected back.
/// Mirrored from Codex's `templates/compact/summary_prefix.md`.
pub(crate) const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";

/// Maximum total approximate tokens for real user messages preserved after compaction.
/// Mirrors Codex's local compaction budget for user-message carry-over.
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Keep compaction summary requests comfortably below the model limit. The
/// summary prompt itself needs completion headroom, and the rough estimator is
/// intentionally coarse, so using the full context ceiling is too optimistic.
const COMPACTION_REQUEST_BUDGET_PERCENT: u32 = 70;

/// Unknown/transient compaction failures use time-gradient retries, capped so
/// the turn stops and hands the problem to a human instead of looping forever.
const COMPACTION_MAX_TRANSIENT_RETRIES: u64 = 5;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Error returned when compaction is interrupted by a cancellation token.
pub const COMPACTION_INTERRUPTED_ERROR: &str = "compaction interrupted by cancellation";

/// Compact (summarize) the session history using the model.
///
/// Strategy (Memento — same as Codex):
/// 1. Clone structured history and append the compaction prompt as a user input
/// 2. Ask the model to create a handoff summary
/// 3. Preserve recent user messages for continuity
/// 4. Rebuild history as: \[...recent_user_messages, summary_message\]
#[allow(clippy::too_many_arguments)]
pub async fn compact_session(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
    base_instructions: Option<&str>,
    initial_context_injection: InitialContextInjection,
    initial_context: Vec<ChatMessage>,
    cancellation_token: &CancellationToken,
) -> Result<CompactionResult, String> {
    let start_instant = std::time::Instant::now();
    let pre_tokens = session.effective_token_count();
    let pre_messages = session.history.len();

    info!(
        session_key = %session.session_key,
        history_len = pre_messages,
        estimated_tokens = pre_tokens,
        compaction_count = session.compaction_count,
        trigger = ?trigger,
        reason = ?reason,
        phase = ?phase,
        initial_context_injection = ?initial_context_injection,
        initial_context_items = initial_context.len(),
        "starting session compaction"
    );

    // Build compaction request in Codex local shape: keep the pre-compaction
    // history as structured messages and append the compaction prompt as a new
    // user input. Base instructions are carried separately as the system item
    // for the request, not flattened into replacement history.
    let request_has_base_instructions =
        base_instructions.is_some_and(|instructions| !instructions.trim().is_empty());
    let mut summary_messages = build_compaction_messages(&session.history, base_instructions);
    let history_start_index = usize::from(request_has_base_instructions);
    let protected_tail_len = 1;
    let request_budget = compaction_request_budget_tokens(config);
    let initial_removed = shrink_compaction_messages_to_budget(
        &mut summary_messages,
        history_start_index,
        protected_tail_len,
        request_budget,
    );
    if initial_removed > 0 {
        warn!(
            session_key = %session.session_key,
            removed_messages = initial_removed,
            remaining_compaction_messages = summary_messages.len(),
            request_budget_tokens = request_budget,
            estimated_request_tokens = estimate_messages_tokens_usize(&summary_messages),
            "compaction request exceeded safe budget before model call; dropped oldest history items"
        );
    }

    let max_retries = compaction_summary_max_retries(config);
    let mut retries = 0_u64;

    // Call the model to generate a summary (no tools for compaction)
    let response = loop {
        // Check cancellation before each attempt (matching Codex's Interrupted handling)
        if cancellation_token.is_cancelled() {
            warn!(
                session_key = %session.session_key,
                "compaction interrupted before model call"
            );
            return Err(COMPACTION_INTERRUPTED_ERROR.to_string());
        }

        // Race the model call against the cancellation token
        match tokio::select! {
            biased;
            _ = cancellation_token.cancelled() => {
                warn!(
                    session_key = %session.session_key,
                    "compaction interrupted during model call"
                );
                return Err(COMPACTION_INTERRUPTED_ERROR.to_string());
            }
            result = client.chat_completion(config, &summary_messages, &[]) => result,
        } {
            Ok(response) => break response,
            Err(error) if is_context_window_error(&error) => {
                let removed = shrink_compaction_messages_after_context_error(
                    &mut summary_messages,
                    history_start_index,
                    protected_tail_len,
                    request_budget,
                );
                if removed > 0 {
                    warn!(
                        session_key = %session.session_key,
                        removed_messages = removed,
                        remaining_compaction_messages = summary_messages.len(),
                        request_budget_tokens = request_budget,
                        estimated_request_tokens = estimate_messages_tokens_usize(&summary_messages),
                        error = %error,
                        "context window exceeded while compacting; dropped older history batch and retrying"
                    );
                    retries = 0;
                    continue;
                }
                return Err(error);
            }
            Err(error) if is_retryable_error(&error) && retries < max_retries => {
                retries = retries.saturating_add(1);
                let delay = retry_backoff(retries);
                warn!(
                    session_key = %session.session_key,
                    retry_attempt = retries,
                    max_retries,
                    retry_in_ms = delay.as_millis(),
                    error = %error,
                    "transient error while compacting; retrying summary request"
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            Err(error) => return Err(error),
        }
    };
    if let Some(ref usage) = response.usage {
        session.track_background_token_usage(usage.total_tokens);
    }

    let summary = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_else(|| "(compaction produced no summary)".to_string());

    // Collect real user messages to preserve for continuity. This must not
    // retain assistant/tool/tool_result suffixes; those are represented by the
    // handoff summary. Selection is budgeted from newest to oldest, matching
    // Codex local compaction.
    let user_messages = collect_user_messages(&session.history);

    let summary_text = format!("{SUMMARY_PREFIX}\n{summary}");

    // Build compacted history in Codex's local-compaction shape: preserved
    // recent user turns first, then the compaction summary as the last item.
    let mut new_history = build_compacted_history(&user_messages, &summary_text);

    // Mid-turn injection: place caller-provided context before the last real
    // user message so the model sees instructions after the compaction summary
    // but before the most recent user turn (matching Codex's pattern).
    if matches!(
        initial_context_injection,
        InitialContextInjection::BeforeLastUserMessage
    ) && !initial_context.is_empty()
    {
        new_history = insert_initial_context_before_last_user_message(new_history, initial_context);
    }

    let (new_history, sanitize_report) = history::sanitize_chat_history(&new_history);
    if sanitize_report.dropped_anything() {
        warn!(
            session_key = %session.session_key,
            dropped_orphan_tool_messages = sanitize_report.dropped_orphan_tool_messages,
            dropped_incomplete_tool_call_messages = sanitize_report.dropped_incomplete_tool_call_messages,
            "sanitized malformed history produced by compaction"
        );
    }

    let post_messages = new_history.len();

    // Replace session history
    session.history = new_history;
    session.compaction_count += 1;
    session.history_version = session.history_version.saturating_add(1);
    session.recompute_token_snapshot_from_history(base_instructions);

    // Advance the auto-compact window so the next window starts fresh.
    session.auto_compact_window.start_next_window();

    // Track token savings
    let post_tokens = session.effective_token_count();
    let tokens_saved = pre_tokens.saturating_sub(post_tokens);

    let duration_ms = start_instant.elapsed().as_millis() as u64;

    info!(
        session_key = %session.session_key,
        trigger = ?trigger,
        reason = ?reason,
        phase = ?phase,
        summary_len = summary.len(),
        pre_messages,
        post_messages,
        pre_tokens,
        post_tokens,
        tokens_saved,
        duration_ms,
        compaction_count = session.compaction_count,
        "compaction complete"
    );

    if tokens_saved == 0 {
        warn!(
            session_key = %session.session_key,
            "compaction did not reduce token count — may indicate ineffective summarization"
        );
    }

    Ok(CompactionResult {
        performed: true,
        pre_tokens,
        post_tokens,
        tokens_saved,
        messages_removed: pre_messages.saturating_sub(post_messages),
        reason: None,
        trigger: Some(trigger),
        compaction_reason: Some(reason),
        phase: Some(phase),
        duration_ms: Some(duration_ms),
    })
}

/// Check whether the session should be compacted based on token usage.
///
/// Matches Codex's runtime trigger: compare the current effective context
/// token count against the model/provider auto-compact threshold. Regular
/// prompt construction still uses full sanitized history; compaction is the
/// normal history-rewrite path once the token budget is reached.
pub fn should_compact(session: &AgentSession, config: &AgentConfig) -> bool {
    let context_tokens = u64::from(session.effective_token_count());
    let threshold = u64::from(config.get_compact_threshold_tokens());

    debug!(context_tokens, threshold, "should_compact check");

    context_tokens >= threshold
}

/// Get a `TokenUsageSnapshot` from the session's current state.
///
/// Uses `ServerObserved` when the session has real API response tokens,
/// falls back to `Estimated` from the character-based heuristic.
pub fn token_usage_snapshot(session: &AgentSession) -> TokenUsageSnapshot {
    if let Some(server_tokens) = session.last_response_tokens {
        let effective = u64::from(session.effective_token_count());
        if session.last_response_history_len == Some(session.history.len()) {
            TokenUsageSnapshot::from_server(effective)
        } else {
            TokenUsageSnapshot::from_mixed(server_tokens, effective)
        }
    } else {
        TokenUsageSnapshot::from_estimate(u64::from(session.effective_token_count()))
    }
}

/// Update the auto-compact window's prefill baseline from API response token usage.
///
/// Called when the API returns `usage.input_tokens` in its response. The
/// baseline is retained for status/debug telemetry; auto-compaction itself is
/// triggered by Codex-style total effective context tokens.
///
/// Safe to call multiple times — only the first server observation per window
/// takes effect (subsequent calls are no-ops).
pub fn update_token_baseline(session: &mut AgentSession, input_tokens: u64) {
    session
        .auto_compact_window
        .ensure_server_observed_prefill_from_usage(input_tokens);
}

/// Compact session with full hook lifecycle and analytics emission.
///
/// This is the preferred entry point for compaction that integrates:
/// 1. Pre-compact hooks (can abort compaction)
/// 2. The actual compaction (delegated to `compact_session`)
/// 3. Post-compact hooks (can signal turn abort)
/// 4. Analytics emission
/// 5. Reference context invalidation
///
/// Window advancement is handled by `compact_session()` internally.
///
/// Returns `Ok(CompactionResult)` on success, `Err` with reason on failure
/// or hook-stopped abort.
#[allow(clippy::too_many_arguments)]
pub async fn compact_session_with_hooks(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
    base_instructions: Option<&str>,
    initial_context_injection: InitialContextInjection,
    initial_context: Vec<ChatMessage>,
    cancellation_token: &CancellationToken,
) -> Result<CompactionResult, String> {
    let active_tokens_before = u64::from(session.effective_token_count());
    let window_ordinal = session.auto_compact_window.ordinal();
    let started_progress = build_compaction_progress(
        session,
        config,
        trigger,
        reason,
        phase,
        active_tokens_before,
    );
    emit_compaction_progress(
        session,
        AgentTurnProgressEvent::CompactionStarted {
            progress: started_progress.clone(),
        },
    );

    // --- Pre-compact hooks ---
    let hook_context = CompactionContext {
        trigger,
        reason,
        phase,
        active_context_tokens: active_tokens_before,
        session_key: session.session_key.clone(),
        compaction_count: session.compaction_count,
    };

    if !session.compaction_hooks.is_empty() {
        let pre_outcome = session
            .compaction_hooks
            .run_pre_compact(&hook_context)
            .await;
        match pre_outcome {
            PreCompactOutcome::Continue => {}
            PreCompactOutcome::Stopped {
                reason: stop_reason,
            } => {
                let reason_str =
                    stop_reason.unwrap_or_else(|| "pre-compact hook stopped execution".to_string());
                warn!(
                    session_key = %session.session_key,
                    reason = %reason_str,
                    "compaction aborted by pre-compact hook"
                );
                emit_compaction_progress(
                    session,
                    AgentTurnProgressEvent::CompactionFailed {
                        progress: started_progress,
                        error: reason_str.clone(),
                    },
                );
                return Err(reason_str);
            }
        }
    }

    // --- Execute compaction (also advances the auto-compact window) ---
    let start_instant = std::time::Instant::now();
    let result = compact_session(
        client,
        config,
        session,
        trigger,
        reason,
        phase,
        base_instructions,
        initial_context_injection,
        initial_context,
        cancellation_token,
    )
    .await;

    let duration_ms = start_instant.elapsed().as_millis() as u64;

    match &result {
        Ok(compaction_result) => {
            emit_compaction_progress(
                session,
                AgentTurnProgressEvent::CompactionFinished {
                    progress: build_compaction_progress_from_result(
                        session,
                        config,
                        trigger,
                        reason,
                        phase,
                        compaction_result,
                    ),
                },
            );
            // --- Post-compact hooks ---
            let hook_result = CompactionHookResult {
                pre_tokens: compaction_result.pre_tokens,
                post_tokens: compaction_result.post_tokens,
                tokens_saved: compaction_result.tokens_saved,
                duration_ms,
                success: true,
            };

            if !session.compaction_hooks.is_empty() {
                let post_outcome = session
                    .compaction_hooks
                    .run_post_compact(&hook_result)
                    .await;
                if matches!(post_outcome, PostCompactOutcome::Stopped) {
                    warn!(
                        session_key = %session.session_key,
                        "post-compact hook signaled stop"
                    );
                    return Err("post-compact hook stopped execution".to_string());
                }
            }

            // --- Invalidate reference context (needs re-injection next turn) ---
            session.reference_context_state.advance_generation();

            // --- Emit analytics ---
            let active_tokens_after = u64::from(session.effective_token_count());
            let _analytics = CompactionAnalytics {
                trigger,
                reason,
                phase,
                active_context_tokens_before: active_tokens_before,
                active_context_tokens_after: Some(active_tokens_after),
                duration_ms,
                implementation: CompactionImplementation::Local,
                status: CompactionStatus::Completed,
                error: None,
                session_key: session.session_key.clone(),
                window_ordinal,
            };

            info!(
                session_key = %session.session_key,
                window_ordinal,
                new_window_ordinal = session.auto_compact_window.ordinal(),
                active_tokens_before,
                active_tokens_after,
                "compaction window advanced"
            );
        }
        Err(error) => {
            emit_compaction_progress(
                session,
                AgentTurnProgressEvent::CompactionFailed {
                    progress: build_compaction_progress(
                        session,
                        config,
                        trigger,
                        reason,
                        phase,
                        active_tokens_before,
                    ),
                    error: error.clone(),
                },
            );
            let status = if error == COMPACTION_INTERRUPTED_ERROR {
                CompactionStatus::Interrupted
            } else {
                CompactionStatus::Failed
            };
            let _analytics = CompactionAnalytics {
                trigger,
                reason,
                phase,
                active_context_tokens_before: active_tokens_before,
                active_context_tokens_after: None,
                duration_ms,
                implementation: CompactionImplementation::Local,
                status,
                error: Some(error.clone()),
                session_key: session.session_key.clone(),
                window_ordinal,
            };
        }
    }

    result
}

fn build_compaction_progress(
    session: &AgentSession,
    config: &AgentConfig,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
    pre_tokens: u64,
) -> AgentCompactionProgress {
    AgentCompactionProgress {
        trigger: compaction_trigger_label(trigger).to_string(),
        reason: compaction_reason_label(reason).to_string(),
        phase: compaction_phase_label(phase).to_string(),
        pre_tokens: u32::try_from(pre_tokens).unwrap_or(u32::MAX),
        post_tokens: None,
        tokens_saved: None,
        messages_removed: None,
        duration_ms: None,
        compaction_count: session.compaction_count,
        history_version: session.history_version,
        context: snapshot_agent_context(session, config),
    }
}

fn build_compaction_progress_from_result(
    session: &AgentSession,
    config: &AgentConfig,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
    result: &CompactionResult,
) -> AgentCompactionProgress {
    AgentCompactionProgress {
        trigger: compaction_trigger_label(trigger).to_string(),
        reason: compaction_reason_label(reason).to_string(),
        phase: compaction_phase_label(phase).to_string(),
        pre_tokens: result.pre_tokens,
        post_tokens: Some(result.post_tokens),
        tokens_saved: Some(result.tokens_saved),
        messages_removed: Some(result.messages_removed),
        duration_ms: result.duration_ms,
        compaction_count: session.compaction_count,
        history_version: session.history_version,
        context: snapshot_agent_context(session, config),
    }
}

fn emit_compaction_progress(session: &AgentSession, event: AgentTurnProgressEvent) {
    if let Some(sender) = &session.progress_sender {
        let _ = sender.send(event);
    }
}

// ---------------------------------------------------------------------------
// CompactionResult
// ---------------------------------------------------------------------------

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub performed: bool,
    pub pre_tokens: u32,
    pub post_tokens: u32,
    pub tokens_saved: u32,
    pub messages_removed: usize,
    pub reason: Option<String>,
    /// Analytics: what triggered this compaction.
    pub trigger: Option<CompactionTrigger>,
    /// Analytics: why the compaction was triggered.
    pub compaction_reason: Option<CompactionReason>,
    /// Analytics: phase of the turn when compaction occurred.
    pub phase: Option<CompactionPhase>,
    /// Analytics: wall-clock duration of the compaction in milliseconds.
    pub duration_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Compaction request construction
// ---------------------------------------------------------------------------

fn build_compaction_messages(
    history: &[ChatMessage],
    base_instructions: Option<&str>,
) -> Vec<ChatMessage> {
    let base_instructions =
        base_instructions.filter(|instructions| !instructions.trim().is_empty());
    let mut messages = Vec::with_capacity(
        history
            .len()
            .saturating_add(usize::from(base_instructions.is_some()))
            .saturating_add(1),
    );

    if let Some(base_instructions) = base_instructions {
        messages.push(ChatMessage::system(base_instructions));
    }
    messages.extend_from_slice(history);
    messages.push(ChatMessage::user(COMPACTION_PROMPT));
    messages
}

fn compaction_request_budget_tokens(config: &AgentConfig) -> usize {
    let context_window = config
        .model_context_window
        .and_then(|value| u64::try_from(value.max(0)).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| u64::from(config.get_compact_threshold_tokens()));
    let request_budget =
        context_window.saturating_mul(u64::from(COMPACTION_REQUEST_BUDGET_PERCENT)) / 100;
    let completion_budget = u64::from(config.get_max_completion_tokens()).min(request_budget / 4);
    let request_budget = request_budget.saturating_sub(completion_budget);
    usize::try_from(request_budget.max(1)).unwrap_or(usize::MAX)
}

fn estimate_messages_tokens_usize(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            message
                .content
                .as_ref()
                .map(|c| approx_token_count(c))
                .unwrap_or(0)
                + message
                    .content_parts
                    .as_ref()
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|part| match part {
                                crate::types::ChatContentPart::Text { text } => {
                                    approx_token_count(text)
                                }
                                crate::types::ChatContentPart::ImageUrl { image_url } => {
                                    approx_token_count(&image_url.url)
                                }
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0)
                + message
                    .tool_calls
                    .as_ref()
                    .map(|tool_calls| {
                        tool_calls
                            .iter()
                            .map(|tool_call| {
                                approx_token_count(tool_call.name())
                                    + approx_token_count(tool_call.arguments())
                            })
                            .sum::<usize>()
                    })
                    .unwrap_or(0)
        })
        .sum()
}

fn shrink_compaction_messages_to_budget(
    messages: &mut Vec<ChatMessage>,
    history_start_index: usize,
    protected_tail_len: usize,
    max_tokens: usize,
) -> usize {
    let mut removed = 0usize;
    while estimate_messages_tokens_usize(messages) > max_tokens {
        if !remove_oldest_history_item_from_compaction_messages(
            messages,
            history_start_index,
            protected_tail_len,
        ) {
            break;
        }
        removed = removed.saturating_add(1);
    }
    removed
}

fn shrink_compaction_messages_after_context_error(
    messages: &mut Vec<ChatMessage>,
    history_start_index: usize,
    protected_tail_len: usize,
    request_budget: usize,
) -> usize {
    let current_tokens = estimate_messages_tokens_usize(messages);
    let fallback_budget = current_tokens.saturating_mul(2) / 3;
    let target = request_budget.min(fallback_budget.max(1));
    shrink_compaction_messages_to_budget(messages, history_start_index, protected_tail_len, target)
}

fn remove_oldest_history_item_from_compaction_messages(
    messages: &mut Vec<ChatMessage>,
    history_start_index: usize,
    protected_tail_len: usize,
) -> bool {
    let Some(protected_tail_start) = messages.len().checked_sub(protected_tail_len.max(1)) else {
        return false;
    };
    if history_start_index >= protected_tail_start {
        return false;
    }
    messages.remove(history_start_index);
    true
}

fn is_context_window_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("maximum context length")
        || lower.contains("token limit")
        || (lower.contains("too many tokens") && lower.contains("max"))
}

fn is_retryable_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("connection")
        || lower.contains("temporary")
        || lower.contains("server error")
        || lower.contains("internal error")
        || lower.contains("service unavailable")
        || lower.contains("gateway")
        || lower.contains("overloaded")
        || lower.contains("try again")
        || lower.contains("http request failed")
}

fn compaction_summary_max_retries(config: &AgentConfig) -> u64 {
    let provider_retries = config
        .resolve_effective_config()
        .map(|effective| effective.stream_max_retries)
        .unwrap_or(COMPACTION_MAX_TRANSIENT_RETRIES);
    let provider_retries = if provider_retries == 0 {
        COMPACTION_MAX_TRANSIENT_RETRIES
    } else {
        provider_retries
    };
    provider_retries.min(COMPACTION_MAX_TRANSIENT_RETRIES)
}

fn retry_backoff(retry_attempt: u64) -> std::time::Duration {
    if cfg!(test) {
        return std::time::Duration::from_millis(1);
    }
    let exponent = retry_attempt.saturating_sub(1).min(3);
    std::time::Duration::from_millis(1_000_u64.saturating_mul(1_u64 << exponent))
}

fn build_compacted_history(user_messages: &[String], summary_text: &str) -> Vec<ChatMessage> {
    build_compacted_history_with_limit(user_messages, summary_text, COMPACT_USER_MESSAGE_MAX_TOKENS)
}

fn build_compacted_history_with_limit(
    user_messages: &[String],
    summary_text: &str,
    max_tokens: usize,
) -> Vec<ChatMessage> {
    let mut selected_messages: Vec<String> = Vec::new();
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = approx_token_count(message);
            if tokens <= remaining {
                selected_messages.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else {
                selected_messages.push(truncate_middle_with_token_budget(message, remaining));
                break;
            }
        }
        selected_messages.reverse();
    }

    let mut history = Vec::with_capacity(selected_messages.len() + 1);
    for message in selected_messages {
        history.push(ChatMessage::user(&message));
    }

    let summary_text = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    };
    history.push(ChatMessage::user(&summary_text));
    history
}

/// Collect real user messages (preserving order).
/// Summary messages from prior compactions are excluded to prevent
/// stale summaries from accumulating across repeated compactions
/// (matching Codex's `collect_user_messages` filter).
/// Assistant/tool/tool_result messages are intentionally excluded; their useful
/// state belongs in the generated summary, not in replacement history.
fn collect_user_messages(history: &[ChatMessage]) -> Vec<String> {
    history
        .iter()
        .filter_map(|message| {
            let content = message.content.as_ref()?;
            if message.role == "user"
                && !is_summary_message(message)
                && !is_contextual_prompt_user_message(content)
            {
                Some(content.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Check if a message is a compaction summary (starts with [`SUMMARY_PREFIX`]).
pub(crate) fn is_summary_message(msg: &ChatMessage) -> bool {
    msg.content
        .as_ref()
        .is_some_and(|c| c.starts_with(format!("{SUMMARY_PREFIX}\n").as_str()))
}

fn is_contextual_prompt_user_message(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with(crate::prompt::USER_INSTRUCTIONS_OPEN_TAG)
        || trimmed.starts_with(crate::prompt::ENVIRONMENT_CONTEXT_OPEN_TAG)
        || trimmed.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.starts_with("<environment_context>")
}

const APPROX_BYTES_PER_TOKEN: usize = 4;

fn approx_token_count(text: &str) -> usize {
    text.len()
        .saturating_add(APPROX_BYTES_PER_TOKEN.saturating_sub(1))
        / APPROX_BYTES_PER_TOKEN
}

fn approx_bytes_for_tokens(tokens: usize) -> usize {
    tokens.saturating_mul(APPROX_BYTES_PER_TOKEN)
}

fn approx_tokens_from_byte_count(bytes: usize) -> u64 {
    let bytes_u64 = bytes as u64;
    bytes_u64.saturating_add((APPROX_BYTES_PER_TOKEN as u64).saturating_sub(1))
        / (APPROX_BYTES_PER_TOKEN as u64)
}

fn truncate_middle_with_token_budget(text: &str, max_tokens: usize) -> String {
    truncate_middle_with_byte_estimate(text, approx_bytes_for_tokens(max_tokens), true)
}

fn truncate_middle_with_byte_estimate(text: &str, max_bytes: usize, use_tokens: bool) -> String {
    if text.is_empty() {
        return String::new();
    }

    let total_chars = text.chars().count();
    if max_bytes == 0 {
        return format_truncation_marker(
            use_tokens,
            removed_units(use_tokens, text.len(), total_chars),
        );
    }

    if text.len() <= max_bytes {
        return text.to_string();
    }

    let (left_budget, right_budget) = split_budget(max_bytes);
    let (removed_chars, left, right) = split_string(text, left_budget, right_budget);
    let marker = format_truncation_marker(
        use_tokens,
        removed_units(
            use_tokens,
            text.len().saturating_sub(max_bytes),
            removed_chars,
        ),
    );

    let mut output = String::with_capacity(left.len() + marker.len() + right.len());
    output.push_str(left);
    output.push_str(&marker);
    output.push_str(right);
    output
}

fn split_budget(budget: usize) -> (usize, usize) {
    let left = budget / 2;
    (left, budget - left)
}

fn split_string(text: &str, beginning_bytes: usize, end_bytes: usize) -> (usize, &str, &str) {
    if text.is_empty() {
        return (0, "", "");
    }

    let tail_start_target = text.len().saturating_sub(end_bytes);
    let mut prefix_end = 0usize;
    let mut suffix_start = text.len();
    let mut removed_chars = 0usize;
    let mut suffix_started = false;

    for (idx, ch) in text.char_indices() {
        let char_end = idx + ch.len_utf8();
        if char_end <= beginning_bytes {
            prefix_end = char_end;
            continue;
        }

        if idx >= tail_start_target {
            if !suffix_started {
                suffix_start = idx;
                suffix_started = true;
            }
            continue;
        }

        removed_chars = removed_chars.saturating_add(1);
    }

    if suffix_start < prefix_end {
        suffix_start = prefix_end;
    }

    (removed_chars, &text[..prefix_end], &text[suffix_start..])
}

fn format_truncation_marker(use_tokens: bool, removed_count: u64) -> String {
    if use_tokens {
        format!("…{removed_count} tokens truncated…")
    } else {
        format!("…{removed_count} chars truncated…")
    }
}

fn removed_units(use_tokens: bool, removed_bytes: usize, removed_chars: usize) -> u64 {
    if use_tokens {
        approx_tokens_from_byte_count(removed_bytes)
    } else {
        u64::try_from(removed_chars).unwrap_or(u64::MAX)
    }
}

/// Insert initial context items before the last real user message in
/// compacted history.
///
/// Placement rules (matching Codex's
/// `insert_initial_context_before_last_real_user_or_summary`):
/// 1. Prefer immediately before the last real (non-summary) user message.
/// 2. If no real user messages remain, insert before the compaction summary.
/// 3. If nothing matches, append the context items.
pub fn insert_initial_context_before_last_user_message(
    mut history: Vec<ChatMessage>,
    initial_context: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    if initial_context.is_empty() {
        return history;
    }

    // Scan backwards for the last real user message and the last summary.
    let mut last_real_user_idx: Option<usize> = None;
    let mut last_any_user_idx: Option<usize> = None;
    for (i, m) in history.iter().enumerate().rev() {
        if m.role == "user" {
            if last_any_user_idx.is_none() {
                last_any_user_idx = Some(i);
            }
            if !is_summary_message(m) {
                last_real_user_idx = Some(i);
                break;
            }
        }
    }

    let insertion_idx = last_real_user_idx.or(last_any_user_idx);

    match insertion_idx {
        Some(idx) => {
            // Splice the context items in before the target message.
            history.splice(idx..idx, initial_context);
        }
        None => {
            // No user messages at all — append context at the end.
            history.extend(initial_context);
        }
    }

    history
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
