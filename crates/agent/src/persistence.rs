//! Conversation persistence: recording and replaying conversation events.
//!
//! Events are stored in JSONL files organized by date and session key:
//! `{data_dir}/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl`

use crate::types::ChatMessage;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

// ---------------------------------------------------------------------------
// ConversationEvent
// ---------------------------------------------------------------------------

/// A recorded conversation event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub session_key: String,
    pub content: serde_json::Value,
}

/// Event type constants.
pub mod event_types {
    pub const USER_MESSAGE: &str = "user_message";
    pub const ASSISTANT_MESSAGE: &str = "assistant_message";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const COMPACTION: &str = "compaction";
    pub const SESSION_START: &str = "session_start";
    pub const SESSION_END: &str = "session_end";
    pub const MCP_TOOLS_LOADED: &str = "mcp_tools_loaded";
    pub const SKILLS_LOADED: &str = "skills_loaded";
}

// ---------------------------------------------------------------------------
// ConversationRecorder
// ---------------------------------------------------------------------------

/// Records conversation events to a JSONL file.
pub struct ConversationRecorder {
    file_path: PathBuf,
    writer: Option<BufWriter<std::fs::File>>,
}

impl ConversationRecorder {
    /// Create a new recorder. File path is:
    /// `{data_dir}/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl`
    pub fn new(data_dir: &Path, session_key: &str) -> Self {
        let now = current_time_secs();
        let (year, month, day) = date_from_timestamp(now);

        let sessions_dir = data_dir
            .join("sessions")
            .join(year.to_string())
            .join(format!("{:02}", month))
            .join(format!("{:02}", day));

        let filename = format!("session-{}-{}.jsonl", sanitize_key(session_key), now);
        let file_path = sessions_dir.join(filename);

        Self {
            file_path,
            writer: None,
        }
    }

    /// Record a conversation event.
    pub fn record(&mut self, event: ConversationEvent) -> Result<(), String> {
        let writer = self.get_or_create_writer()?;

        let line = serde_json::to_string(&event).map_err(|e| format!("serialize event: {e}"))?;

        writeln!(writer, "{}", line).map_err(|e| format!("write event: {e}"))?;

        // Flush immediately so events are durable even if the process crashes
        // or the recorder is held open across turns.
        writer.flush().map_err(|e| format!("flush event: {e}"))?;

        Ok(())
    }

    /// Record a user message event.
    pub fn record_user_message(&mut self, session_key: &str, content: &str) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::USER_MESSAGE.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "message": content }),
        })
    }

    /// Record an assistant message event.
    pub fn record_assistant_message(
        &mut self,
        session_key: &str,
        content: &str,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::ASSISTANT_MESSAGE.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "message": content }),
        })
    }

    /// Record a tool call event.
    pub fn record_tool_call(
        &mut self,
        session_key: &str,
        tool_name: &str,
        arguments: &str,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TOOL_CALL.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "tool_name": tool_name,
                "arguments": arguments,
            }),
        })
    }

    /// Record a tool result event.
    pub fn record_tool_result(
        &mut self,
        session_key: &str,
        tool_name: &str,
        result: &str,
        success: bool,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TOOL_RESULT.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "tool_name": tool_name,
                "result": result,
                "success": success,
            }),
        })
    }

    /// Record a session start event with metadata (MCP tools, skills, config info).
    pub fn record_session_start(
        &mut self,
        session_key: &str,
        metadata: serde_json::Value,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::SESSION_START.to_string(),
            session_key: session_key.to_string(),
            content: metadata,
        })
    }

    /// Record a session end event with summary (total tokens, tool calls count, etc.).
    pub fn record_session_end(
        &mut self,
        session_key: &str,
        metadata: serde_json::Value,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::SESSION_END.to_string(),
            session_key: session_key.to_string(),
            content: metadata,
        })
    }

    /// Flush and close the recorder.
    pub fn close(&mut self) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.flush();
        }
        self.writer = None;
    }

    /// Get the file path where events are being recorded.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Get or create the writer, creating parent directories as needed.
    fn get_or_create_writer(&mut self) -> Result<&mut BufWriter<std::fs::File>, String> {
        if self.writer.is_none() {
            if let Some(parent) = self.file_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create sessions dir: {e}"))?;
            }

            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.file_path)
                .map_err(|e| format!("open session file: {e}"))?;

            debug!(path = %self.file_path.display(), "created conversation recorder");
            self.writer = Some(BufWriter::new(file));
        }

        Ok(self.writer.as_mut().unwrap())
    }
}

impl Drop for ConversationRecorder {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Loading conversations
// ---------------------------------------------------------------------------

/// Load a previous conversation from a JSONL file.
pub fn load_conversation(path: &Path) -> Result<Vec<ChatMessage>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let event: ConversationEvent =
            serde_json::from_str(&line).map_err(|e| format!("parse event: {e}"))?;

        match event.event_type.as_str() {
            event_types::USER_MESSAGE => {
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::user(msg));
                }
            }
            event_types::ASSISTANT_MESSAGE => {
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::assistant(msg));
                }
            }
            event_types::TOOL_RESULT => {
                if let Some(result) = event.content.get("result").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::tool_result("recovered", result));
                }
            }
            _ => {
                // Skip tool_call, compaction, and other meta events for replay
            }
        }
    }

    Ok(messages)
}

/// Load raw conversation events from a JSONL file.
///
/// Unlike `load_conversation` which converts events to `ChatMessage`,
/// this returns the raw `ConversationEvent` objects with full details
/// (timestamps, tool call arguments, results, metadata, etc.).
pub fn load_conversation_events(path: &Path) -> Result<Vec<ConversationEvent>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let event: ConversationEvent =
            serde_json::from_str(&line).map_err(|e| format!("parse event: {e}"))?;
        events.push(event);
    }

    Ok(events)
}

/// List available conversation files for a session.
pub fn list_conversations(data_dir: &Path, session_key: Option<&str>) -> Vec<PathBuf> {
    let sessions_dir = data_dir.join("sessions");
    if !sessions_dir.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    collect_jsonl_files(&sessions_dir, &mut files);

    // Filter by session key if provided
    if let Some(key) = session_key {
        let sanitized = sanitize_key(key);
        files.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(&sanitized))
                .unwrap_or(false)
        });
    }

    files.sort();
    files
}

/// Recursively collect .jsonl files.
fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Extract (year, month, day) from a unix timestamp.
fn date_from_timestamp(timestamp: u64) -> (u32, u32, u32) {
    // Simple date calculation without chrono
    let days = timestamp / 86400;
    let (year, month, day) = days_to_ymd(days as i64);
    (year as u32, month as u32, day as u32)
}

/// Convert days since epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i32, i32, i32) {
    // Civil calendar algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as i32, d as i32)
}

/// Sanitize a session key for use in filenames.
fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_types::*;

    #[test]
    fn test_conversation_recorder_basic() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test-session");

        recorder
            .record_user_message("test-session", "hello")
            .unwrap();
        recorder
            .record_assistant_message("test-session", "hi there")
            .unwrap();
        recorder.close();

        // Verify file was created
        assert!(recorder.file_path().exists());

        // Load and verify
        let messages = load_conversation(recorder.file_path()).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.as_deref(), Some("hello"));
        assert_eq!(messages[1].content.as_deref(), Some("hi there"));
    }

    #[test]
    fn test_conversation_recorder_tool_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test");

        recorder.record_user_message("test", "run ls").unwrap();
        recorder
            .record_tool_call("test", "shell", r#"{"command": "ls"}"#)
            .unwrap();
        recorder
            .record_tool_result("test", "shell", "file1.txt\nfile2.txt", true)
            .unwrap();
        recorder
            .record_assistant_message("test", "Here are the files")
            .unwrap();
        recorder.close();

        let messages = load_conversation(recorder.file_path()).unwrap();
        // user_message + tool_result + assistant_message = 3
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_list_conversations() {
        let dir = tempfile::tempdir().unwrap();

        // Create a session file
        let mut recorder = ConversationRecorder::new(dir.path(), "session-1");
        recorder.record_user_message("session-1", "test").unwrap();
        recorder.close();

        let files = list_conversations(dir.path(), Some("session-1"));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_list_conversations_empty() {
        let dir = tempfile::tempdir().unwrap();
        let files = list_conversations(dir.path(), None);
        assert!(files.is_empty());
    }

    #[test]
    fn test_sanitize_key() {
        assert_eq!(sanitize_key("hello-world"), "hello-world");
        assert_eq!(sanitize_key("user@email.com"), "user_email_com");
        assert_eq!(sanitize_key("path/to/thing"), "path_to_thing");
    }

    #[test]
    fn test_date_from_timestamp() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let (y, m, d) = date_from_timestamp(1704067200);
        assert_eq!(y, 2024);
        assert_eq!(m, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn test_date_from_timestamp_mid_year() {
        // 2024-06-15 12:00:00 UTC = 1718452800
        let (y, m, d) = date_from_timestamp(1718452800);
        assert_eq!(y, 2024);
        assert_eq!(m, 6);
        assert_eq!(d, 15);
    }

    #[test]
    fn test_recorder_drop_flushes() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut recorder = ConversationRecorder::new(dir.path(), "drop-test");
            recorder.record_user_message("drop-test", "data").unwrap();
            // Drop happens here
        }

        let files = list_conversations(dir.path(), Some("drop-test"));
        assert_eq!(files.len(), 1);
        let messages = load_conversation(&files[0]).unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_load_conversation_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test-events");

        recorder
            .record_user_message("test-events", "hello")
            .unwrap();
        recorder
            .record_tool_call("test-events", "shell", r#"{"command": "ls"}"#)
            .unwrap();
        recorder
            .record_tool_result("test-events", "shell", "file1.txt", true)
            .unwrap();
        recorder
            .record_assistant_message("test-events", "done")
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events.len(), 4);

        // Verify event types and content
        assert_eq!(events[0].event_type, USER_MESSAGE);
        assert_eq!(events[0].content["message"], "hello");

        assert_eq!(events[1].event_type, TOOL_CALL);
        assert_eq!(events[1].content["tool_name"], "shell");
        assert_eq!(events[1].content["arguments"], r#"{"command": "ls"}"#);

        assert_eq!(events[2].event_type, TOOL_RESULT);
        assert_eq!(events[2].content["tool_name"], "shell");
        assert_eq!(events[2].content["result"], "file1.txt");
        assert_eq!(events[2].content["success"], true);

        assert_eq!(events[3].event_type, ASSISTANT_MESSAGE);
        assert_eq!(events[3].content["message"], "done");

        // Verify timestamps are present
        for event in &events {
            assert!(event.timestamp > 0);
        }
    }

    #[test]
    fn test_record_session_start_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test-session-lifecycle");

        // Record session start with metadata
        let start_metadata = serde_json::json!({
            "mcp_tools": ["tool1", "tool2"],
            "skills": ["skill1"],
            "config": {"model": "gpt-4"}
        });
        recorder
            .record_session_start("test-session-lifecycle", start_metadata)
            .unwrap();

        recorder
            .record_user_message("test-session-lifecycle", "hello")
            .unwrap();

        // Record session end with summary
        let end_metadata = serde_json::json!({
            "total_tokens": 1500,
            "tool_calls_count": 3,
            "duration_secs": 45
        });
        recorder
            .record_session_end("test-session-lifecycle", end_metadata)
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events.len(), 3);

        assert_eq!(events[0].event_type, SESSION_START);
        assert_eq!(
            events[0].content["mcp_tools"],
            serde_json::json!(["tool1", "tool2"])
        );

        assert_eq!(events[1].event_type, USER_MESSAGE);

        assert_eq!(events[2].event_type, SESSION_END);
        assert_eq!(events[2].content["total_tokens"], 1500);
    }
}
