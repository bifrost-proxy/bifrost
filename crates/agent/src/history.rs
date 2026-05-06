//! Chat history invariants for OpenAI-compatible tool calling.

use crate::types::ChatMessage;
use std::collections::HashSet;

/// Result of sanitizing a chat history before it is sent to the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistorySanitizeReport {
    pub dropped_orphan_tool_messages: usize,
    pub dropped_incomplete_tool_call_messages: usize,
}

impl HistorySanitizeReport {
    pub fn dropped_anything(&self) -> bool {
        self.dropped_orphan_tool_messages > 0 || self.dropped_incomplete_tool_call_messages > 0
    }
}

/// Remove malformed tool-call segments from a Chat Completions message list.
///
/// Valid tool-call history is a complete contiguous segment:
/// `assistant(tool_calls=[ids...])`, followed immediately by one `tool` message
/// for each requested id. Any standalone `tool` message, unknown tool id, or
/// incomplete assistant tool-call suffix is removed rather than forwarded.
pub fn sanitize_chat_history(
    messages: &[ChatMessage],
) -> (Vec<ChatMessage>, HistorySanitizeReport) {
    let mut sanitized = Vec::with_capacity(messages.len());
    let mut report = HistorySanitizeReport::default();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];
        if msg.role == "tool" {
            report.dropped_orphan_tool_messages += 1;
            i += 1;
            continue;
        }

        let Some(tool_calls) = msg.tool_calls.as_ref().filter(|calls| !calls.is_empty()) else {
            sanitized.push(msg.clone());
            i += 1;
            continue;
        };

        if msg.role != "assistant" {
            sanitized.push(msg.clone());
            i += 1;
            continue;
        }

        let expected_ids: HashSet<&str> = tool_calls.iter().map(|tc| tc.id.as_str()).collect();
        let mut seen_ids: HashSet<&str> = HashSet::new();
        let mut segment = vec![msg.clone()];
        let mut j = i + 1;
        let mut malformed = false;

        while seen_ids.len() < expected_ids.len() {
            let Some(next) = messages.get(j) else {
                malformed = true;
                break;
            };
            if next.role != "tool" {
                malformed = true;
                break;
            }
            let Some(call_id) = next.tool_call_id.as_deref() else {
                malformed = true;
                break;
            };
            if !expected_ids.contains(call_id) || !seen_ids.insert(call_id) {
                malformed = true;
                break;
            }
            segment.push(next.clone());
            j += 1;
        }

        if malformed {
            report.dropped_incomplete_tool_call_messages += 1;
            while matches!(messages.get(j), Some(next) if next.role == "tool") {
                report.dropped_orphan_tool_messages += 1;
                j += 1;
            }
        } else {
            sanitized.extend(segment);
        }
        i = j;
    }

    (sanitized, report)
}

pub fn is_valid_chat_history(messages: &[ChatMessage]) -> bool {
    !sanitize_chat_history(messages).1.dropped_anything()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCallMessage;

    fn tool_call(id: &str, name: &str) -> ToolCallMessage {
        ToolCallMessage::function_call(id.to_string(), name.to_string(), "{}".to_string())
    }

    #[test]
    fn test_sanitize_history_drops_orphan_tool_suffix() {
        let messages = vec![
            ChatMessage::system("system"),
            ChatMessage::user("hello"),
            ChatMessage::tool_result("missing", "orphan"),
        ];

        let (sanitized, report) = sanitize_chat_history(&messages);

        assert_eq!(sanitized.len(), 2);
        assert_eq!(report.dropped_orphan_tool_messages, 1);
        assert!(is_valid_chat_history(&sanitized));
    }

    #[test]
    fn test_sanitize_history_keeps_complete_tool_pair() {
        let messages = vec![
            ChatMessage::user("list files"),
            ChatMessage::assistant_with_tool_calls(vec![tool_call("call-1", "list_directory")]),
            ChatMessage::tool_result("call-1", "[file] Cargo.toml"),
            ChatMessage::assistant("done"),
        ];

        let (sanitized, report) = sanitize_chat_history(&messages);

        assert_eq!(sanitized.len(), messages.len());
        assert!(!report.dropped_anything());
        assert!(is_valid_chat_history(&sanitized));
    }

    #[test]
    fn test_sanitize_history_drops_incomplete_assistant_tool_call() {
        let messages = vec![
            ChatMessage::user("list files"),
            ChatMessage::assistant_with_tool_calls(vec![tool_call("call-1", "list_directory")]),
            ChatMessage::assistant("fallback"),
        ];

        let (sanitized, report) = sanitize_chat_history(&messages);

        assert_eq!(sanitized.len(), 2);
        assert_eq!(sanitized[1].content.as_deref(), Some("fallback"));
        assert_eq!(report.dropped_incomplete_tool_call_messages, 1);
        assert!(is_valid_chat_history(&sanitized));
    }

    #[test]
    fn test_sanitize_history_drops_unknown_tool_result_id() {
        let messages = vec![
            ChatMessage::user("list files"),
            ChatMessage::assistant_with_tool_calls(vec![tool_call("call-1", "list_directory")]),
            ChatMessage::tool_result("call-2", "wrong"),
            ChatMessage::user("next"),
        ];

        let (sanitized, report) = sanitize_chat_history(&messages);

        assert_eq!(sanitized.len(), 2);
        assert_eq!(sanitized[1].content.as_deref(), Some("next"));
        assert_eq!(report.dropped_incomplete_tool_call_messages, 1);
        assert_eq!(report.dropped_orphan_tool_messages, 1);
        assert!(is_valid_chat_history(&sanitized));
    }

    #[test]
    fn test_retry_rollback_clear_style_suffixes_remain_valid_after_sanitize() {
        let messages = vec![
            ChatMessage::user("before"),
            ChatMessage::assistant("ok"),
            ChatMessage::tool_result("stale-after-rollback", "stale"),
            ChatMessage::user("after clear"),
        ];

        let (sanitized, report) = sanitize_chat_history(&messages);

        assert_eq!(report.dropped_orphan_tool_messages, 1);
        assert_eq!(
            sanitized
                .iter()
                .map(|m| m.role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "user"]
        );
        assert!(is_valid_chat_history(&sanitized));
    }
}
