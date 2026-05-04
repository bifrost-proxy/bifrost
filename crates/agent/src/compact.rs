//! Memory compaction: summarize conversation history when context grows too long.
//!
//! Inspired by Codex's compaction system (Memento strategy):
//! 1. Format conversation history for the compaction model
//! 2. Use COMPACTION_PROMPT to generate a handoff summary
//! 3. Reconstruct history with summary_prefix + summary + recent user messages
//! 4. Track compaction count and token savings
//!
//! Key concepts aligned with Codex:
//! - `CompactionTrigger`: Auto (threshold-based) vs Manual (user-requested)
//! - `CompactionPhase`: PreTurn (before user message) vs MidTurn (during tool loop)
//! - `InitialContextInjection`: Where to place the summary after compaction

use crate::client::AgentClient;
use crate::config::AgentConfig;
use crate::history;
use crate::session::AgentSession;
use crate::types::ChatMessage;
use bifrost_core::text::truncate_chars_with_suffix;
use tracing::{info, warn};

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
/// Based on Codex's `templates/compact/prompt.md`.
const COMPACTION_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a concise handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences discovered
- What was accomplished (including tool calls and their results — summarize file paths, command outputs, and key findings)
- What remains to be done (clear next steps if any)
- Any critical data, examples, references, or error patterns needed to continue
- File paths that were read, written, or modified
- Environment details that affect execution (working directory, shell, OS)

Be concise and structured. Use bullet points. Focus on preserving information needed to continue the conversation seamlessly without re-discovering context."#;

/// The summary prefix is prepended to the compacted summary when injected back.
/// Based on Codex's `templates/compact/summary_prefix.md`.
const SUMMARY_PREFIX: &str = r#"Another language model started to work on this task and produced a summary of its progress. You also have access to the state of the tools that were used. Use this information to continue seamlessly:

"#;

/// Maximum tokens for user messages included in compaction input.
const COMPACT_USER_MESSAGE_MAX_CHARS: usize = 20_000 * 4; // ~20k tokens at 4 chars/token

/// Maximum total chars for the compaction input history.
const COMPACT_HISTORY_MAX_CHARS: usize = 100_000 * 4; // ~100k tokens

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compact (summarize) the session history using the model.
///
/// Strategy (Memento — same as Codex):
/// 1. Format conversation history into a structured text
/// 2. Ask the model to create a handoff summary
/// 3. Preserve recent user messages for continuity
/// 4. Rebuild history as: [summary_message, ...recent_user_messages]
#[allow(clippy::too_many_arguments)]
pub async fn compact_session(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
    initial_context_injection: InitialContextInjection,
    initial_context: Vec<ChatMessage>,
) -> Result<CompactionResult, String> {
    if session.history.len() < 4 {
        return Ok(CompactionResult::skipped("too few messages"));
    }

    let start_instant = std::time::Instant::now();
    let pre_tokens = session.estimate_tokens();
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

    // Build compaction request
    let formatted_history = format_history_for_compaction(&session.history);

    // Truncate if the formatted history is too large
    let truncated_history = if formatted_history.len() > COMPACT_HISTORY_MAX_CHARS {
        let half = COMPACT_HISTORY_MAX_CHARS / 2;
        let head_end = formatted_history.floor_char_boundary(half);
        let tail_start = formatted_history.ceil_char_boundary(formatted_history.len() - half);
        format!(
            "{}\n\n... ({} characters omitted for compaction) ...\n\n{}",
            &formatted_history[..head_end],
            formatted_history.len() - COMPACT_HISTORY_MAX_CHARS,
            &formatted_history[tail_start..]
        )
    } else {
        formatted_history
    };

    let summary_messages = vec![
        ChatMessage::system(COMPACTION_PROMPT),
        ChatMessage::user(&truncated_history),
    ];

    // Call the model to generate a summary (no tools for compaction)
    let response = client
        .chat_completion(config, &summary_messages, &[])
        .await?;

    let summary = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_else(|| "(compaction produced no summary)".to_string());

    // Collect recent user messages to preserve for continuity
    // Keep the last N user messages (with their tool context if adjacent)
    let recent_user_messages = collect_recent_user_messages(&session.history, 3);

    // Build new compacted history
    let mut new_history = Vec::new();

    // Add compaction warning if this is a repeated compaction
    let compaction_note = if session.compaction_count > 0 {
        format!(
            "\n\n[Note: This is compaction #{}, which means the conversation has been long. \
             Some earlier context may have been lost. Focus on the most recent information.]",
            session.compaction_count + 1
        )
    } else {
        String::new()
    };

    // Inject summary with prefix (matches Codex's InitialContextInjection pattern)
    new_history.push(ChatMessage::user(&format!(
        "{SUMMARY_PREFIX}{summary}{compaction_note}"
    )));

    // Preserve recent user messages for context continuity
    for msg in &recent_user_messages {
        new_history.push(msg.clone());
    }

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

    // Track token savings
    let post_tokens = session.estimate_tokens();
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
/// Matches Codex's approach: compare current context tokens against the
/// auto-compact threshold, which defaults to `context_window × 90%` and
/// is clamped to that ceiling even when the user sets a custom value.
pub fn should_compact(session: &AgentSession, config: &AgentConfig) -> bool {
    let context_tokens = session.effective_token_count();
    context_tokens > config.get_compact_threshold_tokens()
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

impl CompactionResult {
    fn skipped(reason: &str) -> Self {
        Self {
            performed: false,
            pre_tokens: 0,
            post_tokens: 0,
            tokens_saved: 0,
            messages_removed: 0,
            reason: Some(reason.to_string()),
            trigger: None,
            compaction_reason: None,
            phase: None,
            duration_ms: None,
        }
    }
}

// ---------------------------------------------------------------------------
// History formatting
// ---------------------------------------------------------------------------

fn format_history_for_compaction(history: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in history {
        let role = &msg.role;
        match &msg.content {
            Some(content) if !content.is_empty() => {
                // Truncate very long messages in the compaction input
                let char_count = content.chars().count();
                let truncated = if char_count > COMPACT_USER_MESSAGE_MAX_CHARS {
                    format!(
                        "{}(truncated, {} chars total)",
                        truncate_chars_with_suffix(content, COMPACT_USER_MESSAGE_MAX_CHARS, "..."),
                        char_count
                    )
                } else {
                    content.clone()
                };
                parts.push(format!("[{role}]: {truncated}"));
            }
            _ => {
                if let Some(tool_calls) = &msg.tool_calls {
                    let calls: Vec<String> = tool_calls
                        .iter()
                        .map(|tc| {
                            let args =
                                truncate_chars_with_suffix(&tc.function.arguments, 500, "...");
                            format!("{}({})", tc.function.name, args)
                        })
                        .collect();
                    parts.push(format!("[{role}]: called tools: {}", calls.join(", ")));
                }
            }
        }
    }
    parts.join("\n")
}

/// Collect the last N user messages (preserving order).
/// Also includes adjacent assistant/tool messages for context.
fn collect_recent_user_messages(history: &[ChatMessage], count: usize) -> Vec<ChatMessage> {
    // Find indices of user messages
    let user_indices: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user" && m.content.is_some())
        .map(|(i, _)| i)
        .collect();

    if user_indices.is_empty() {
        return Vec::new();
    }

    // Take the last `count` user message indices
    let start = user_indices.len().saturating_sub(count);
    let selected_indices = &user_indices[start..];

    if selected_indices.is_empty() {
        return Vec::new();
    }

    // Include all messages from the first selected user message to the end
    let first_idx = selected_indices[0];
    history[first_idx..].to_vec()
}

/// Check if a message is a compaction summary (starts with [`SUMMARY_PREFIX`]).
fn is_summary_message(msg: &ChatMessage) -> bool {
    msg.content
        .as_ref()
        .is_some_and(|c| c.starts_with(SUMMARY_PREFIX))
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
mod tests {
    use super::*;
    use crate::types::{FunctionCallInfo, ToolCallMessage};

    #[test]
    fn test_format_history_basic() {
        let history = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi there"),
            ChatMessage::user("do something"),
        ];
        let formatted = format_history_for_compaction(&history);
        assert!(formatted.contains("[user]: hello"));
        assert!(formatted.contains("[assistant]: hi there"));
        assert!(formatted.contains("[user]: do something"));
    }

    #[test]
    fn test_format_history_truncates_long_messages() {
        let long_msg = "x".repeat(COMPACT_USER_MESSAGE_MAX_CHARS + 1000);
        let history = vec![ChatMessage::user(&long_msg)];
        let formatted = format_history_for_compaction(&history);
        assert!(formatted.contains("...(truncated,"));
        assert!(formatted.len() < long_msg.len() + 200);
    }

    #[test]
    fn test_format_history_truncates_multibyte_user_messages() {
        let long_msg = "前".repeat(COMPACT_USER_MESSAGE_MAX_CHARS + 2);
        let history = vec![ChatMessage::user(&long_msg)];
        let formatted = format_history_for_compaction(&history);

        assert!(formatted.contains("...(truncated,"));
        assert!(formatted.contains("chars total)"));
    }

    #[test]
    fn test_format_history_handles_multibyte_tool_arguments() {
        let arguments = format!(
            "{{\"path\":\"/tmp/example.md\",\"content\":\"{}\"}}",
            "前".repeat(600)
        );
        let history = vec![ChatMessage::assistant_with_tool_calls(vec![
            ToolCallMessage {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCallInfo {
                    name: "write_file".to_string(),
                    arguments,
                },
            },
        ])];

        let formatted = format_history_for_compaction(&history);

        assert!(formatted.contains("write_file("));
        assert!(formatted.contains("..."));
        assert!(formatted.is_char_boundary(formatted.len()));
    }

    #[test]
    fn test_collect_recent_user_messages() {
        let history = vec![
            ChatMessage::user("first"),
            ChatMessage::assistant("reply1"),
            ChatMessage::user("second"),
            ChatMessage::assistant("reply2"),
            ChatMessage::user("third"),
            ChatMessage::assistant("reply3"),
            ChatMessage::user("fourth"),
        ];
        let recent = collect_recent_user_messages(&history, 2);
        // Should include from "third" onwards (last 2 user messages + everything after)
        assert!(recent.len() >= 2);
        assert_eq!(recent[0].content.as_deref(), Some("third"));
    }

    #[test]
    fn test_compaction_preserves_tool_call_tool_result_invariants() {
        let history = vec![
            ChatMessage::user("first"),
            ChatMessage::assistant("reply"),
            ChatMessage::user("inspect"),
            ChatMessage::assistant_with_tool_calls(vec![crate::types::ToolCallMessage {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: crate::types::FunctionCallInfo {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
            }]),
            ChatMessage::tool_result("call-1", "content"),
            ChatMessage::assistant("done"),
        ];

        let recent = collect_recent_user_messages(&history, 1);

        assert!(history::is_valid_chat_history(&recent));
        assert_eq!(recent[1].role, "assistant");
        assert_eq!(recent[2].role, "tool");
    }

    #[test]
    fn test_compaction_result_skipped() {
        let result = CompactionResult::skipped("too few");
        assert!(!result.performed);
        assert_eq!(result.reason.as_deref(), Some("too few"));
    }

    #[test]
    fn test_is_summary_message() {
        let summary = ChatMessage::user(&format!("{SUMMARY_PREFIX}some summary"));
        assert!(is_summary_message(&summary));

        let regular = ChatMessage::user("hello");
        assert!(!is_summary_message(&regular));

        let assistant = ChatMessage::assistant("reply");
        assert!(!is_summary_message(&assistant));
    }

    #[test]
    fn test_insert_initial_context_before_last_real_user_message() {
        let history = vec![
            ChatMessage::user(&format!("{SUMMARY_PREFIX}summary text")),
            ChatMessage::assistant("reply"),
            ChatMessage::user("recent question"),
        ];
        let context = vec![ChatMessage::system("[context reminder]")];

        let result = insert_initial_context_before_last_user_message(history, context);

        // Context should be inserted before "recent question" (last real user message)
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user"); // summary
        assert_eq!(result[1].role, "assistant"); // reply
        assert_eq!(result[2].role, "system"); // injected context
        assert_eq!(result[3].content.as_deref(), Some("recent question"));
    }

    #[test]
    fn test_insert_initial_context_only_summary() {
        // When only a summary user message exists, insert before it
        let history = vec![ChatMessage::user(&format!("{SUMMARY_PREFIX}summary"))];
        let context = vec![ChatMessage::system("[context]")];

        let result = insert_initial_context_before_last_user_message(history, context);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system"); // injected context
        assert!(result[1]
            .content
            .as_ref()
            .unwrap()
            .starts_with(SUMMARY_PREFIX));
    }

    #[test]
    fn test_insert_initial_context_empty_context_is_noop() {
        let history = vec![ChatMessage::user("hello")];
        let result = insert_initial_context_before_last_user_message(history.clone(), vec![]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content.as_deref(), Some("hello"));
    }

    #[test]
    fn test_insert_initial_context_no_user_messages() {
        // When there are no user messages at all, append to the end
        let history = vec![ChatMessage::assistant("reply")];
        let context = vec![ChatMessage::system("[context]")];

        let result = insert_initial_context_before_last_user_message(history, context);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "assistant");
        assert_eq!(result[1].role, "system");
    }

    #[test]
    fn test_insert_initial_context_multiple_context_items() {
        let history = vec![
            ChatMessage::user(&format!("{SUMMARY_PREFIX}summary")),
            ChatMessage::user("question"),
        ];
        let context = vec![ChatMessage::system("[ctx1]"), ChatMessage::system("[ctx2]")];

        let result = insert_initial_context_before_last_user_message(history, context);

        // Both context items inserted before "question"
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user"); // summary
        assert_eq!(result[1].content.as_deref(), Some("[ctx1]"));
        assert_eq!(result[2].content.as_deref(), Some("[ctx2]"));
        assert_eq!(result[3].content.as_deref(), Some("question"));
    }
}
