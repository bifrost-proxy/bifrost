//! Conversation persistence: recording and replaying conversation events.
//!
//! Events are stored in JSONL files organized by date and session key:
//! `{data_dir}/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl`

use crate::history;
use crate::tools::goal::GoalState;
use crate::types::{ChatMessage, ToolCallMessage};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
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
    pub const TITLE_UPDATED: &str = "title_updated";
    pub const GOAL_UPDATED: &str = "goal_updated";
    pub const GOAL_CLEARED: &str = "goal_cleared";
}

// ---------------------------------------------------------------------------
// ConversationRecorder
// ---------------------------------------------------------------------------

/// Records conversation events to a JSONL file.
pub struct ConversationRecorder {
    file_path: PathBuf,
    writer: Option<BufWriter<std::fs::File>>,
    max_bytes: Option<usize>,
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
            max_bytes: None,
        }
    }

    /// Create a recorder with a maximum JSONL file size.
    pub fn new_with_max_bytes(
        data_dir: &Path,
        session_key: &str,
        max_bytes: Option<usize>,
    ) -> Self {
        let mut recorder = Self::new(data_dir, session_key);
        recorder.max_bytes = max_bytes.filter(|value| *value > 0);
        recorder
    }

    /// Record a conversation event.
    pub fn record(&mut self, event: ConversationEvent) -> Result<(), String> {
        let writer = self.get_or_create_writer()?;

        let line = serde_json::to_string(&event).map_err(|e| format!("serialize event: {e}"))?;

        writeln!(writer, "{}", line).map_err(|e| format!("write event: {e}"))?;

        // Flush immediately so events are durable even if the process crashes
        // or the recorder is held open across turns.
        writer.flush().map_err(|e| format!("flush event: {e}"))?;
        self.enforce_max_bytes()?;

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
        self.record_tool_call_with_id(session_key, tool_name, arguments, "function", None)
    }

    /// Record a tool call event with the provider's tool call id.
    pub fn record_tool_call_with_id(
        &mut self,
        session_key: &str,
        tool_name: &str,
        arguments: &str,
        call_type: &str,
        call_id: Option<&str>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TOOL_CALL.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "call_id": call_id,
                "call_type": call_type,
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
        self.record_tool_result_with_call_id(session_key, tool_name, result, success, None)
    }

    /// Record a tool result event with the provider's tool call id.
    pub fn record_tool_result_with_call_id(
        &mut self,
        session_key: &str,
        tool_name: &str,
        result: &str,
        success: bool,
        call_id: Option<&str>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TOOL_RESULT.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "call_id": call_id,
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

    /// Record a compaction event.
    pub fn record_compaction(
        &mut self,
        session_key: &str,
        metadata: serde_json::Value,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::COMPACTION.to_string(),
            session_key: session_key.to_string(),
            content: metadata,
        })
    }

    /// Record a title update event (set by the agent via set_title tool).
    pub fn record_title_updated(&mut self, session_key: &str, title: &str) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TITLE_UPDATED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "title": title }),
        })
    }

    pub fn record_goal_updated(
        &mut self,
        session_key: &str,
        goal: &GoalState,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::GOAL_UPDATED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::to_value(goal)
                .map_err(|e| format!("serialize goal state: {e}"))?,
        })
    }

    pub fn record_goal_cleared(&mut self, session_key: &str) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::GOAL_CLEARED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({}),
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

    fn enforce_max_bytes(&mut self) -> Result<(), String> {
        let Some(max_bytes) = self.max_bytes else {
            return Ok(());
        };
        let metadata = std::fs::metadata(&self.file_path)
            .map_err(|e| format!("stat session file for max_bytes: {e}"))?;
        if metadata.len() as usize <= max_bytes {
            return Ok(());
        }

        if let Some(writer) = self.writer.as_mut() {
            writer
                .flush()
                .map_err(|e| format!("flush before history trim: {e}"))?;
        }
        self.writer = None;

        let content = std::fs::read_to_string(&self.file_path)
            .map_err(|e| format!("read session file for max_bytes trim: {e}"))?;
        let mut kept = Vec::new();
        let mut total = 0usize;
        for line in content.lines().rev() {
            let line_len = line.len() + 1;
            if !kept.is_empty() && total + line_len > max_bytes {
                break;
            }
            kept.push(line);
            total += line_len;
        }
        kept.reverse();
        let next = if kept.is_empty() {
            String::new()
        } else {
            format!("{}\n", kept.join("\n"))
        };
        std::fs::write(&self.file_path, next)
            .map_err(|e| format!("write trimmed session file: {e}"))?;
        Ok(())
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

/// Maximum session file size we will attempt to load (64 MiB).
const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Load a previous conversation from a JSONL file.
pub fn load_conversation(path: &Path) -> Result<Vec<ChatMessage>, String> {
    bifrost_core::text::check_file_size(path, MAX_SESSION_FILE_BYTES)
        .map_err(|e| format!("session file too large to load: {e}"))?;
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    let mut messages = Vec::new();
    let mut pending_tool_calls: HashMap<String, ToolCallMessage> = HashMap::new();
    let mut pending_tool_call_order: VecDeque<String> = VecDeque::new();
    let mut recovered_tool_call_count = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let event: ConversationEvent =
            serde_json::from_str(&line).map_err(|e| format!("parse event: {e}"))?;

        match event.event_type.as_str() {
            event_types::USER_MESSAGE => {
                pending_tool_calls.clear();
                pending_tool_call_order.clear();
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::user(msg));
                }
            }
            event_types::ASSISTANT_MESSAGE => {
                pending_tool_calls.clear();
                pending_tool_call_order.clear();
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::assistant(msg));
                }
            }
            event_types::TOOL_CALL => {
                recovered_tool_call_count += 1;
                let tool_name = event
                    .content
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_tool");
                let arguments = event
                    .content
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let call_type = event
                    .content
                    .get("call_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("function");
                let call_id = event
                    .content
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("recovered-tool-call-{recovered_tool_call_count}"));
                let tool_call = if call_type == "custom" {
                    ToolCallMessage::custom_call(
                        call_id.clone(),
                        tool_name.to_string(),
                        arguments.to_string(),
                    )
                } else {
                    ToolCallMessage::function_call(
                        call_id.clone(),
                        tool_name.to_string(),
                        arguments.to_string(),
                    )
                };
                if !pending_tool_calls.contains_key(&call_id) {
                    pending_tool_call_order.push_back(call_id.clone());
                }
                pending_tool_calls.insert(call_id, tool_call);
            }
            event_types::TOOL_RESULT => {
                if let Some(result) = event.content.get("result").and_then(|v| v.as_str()) {
                    let result_call_id = event
                        .content
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let tool_call = result_call_id
                        .and_then(|call_id| {
                            pending_tool_call_order.retain(|id| id != call_id);
                            pending_tool_calls.remove(call_id)
                        })
                        .or_else(|| {
                            while let Some(call_id) = pending_tool_call_order.pop_front() {
                                if let Some(tool_call) = pending_tool_calls.remove(&call_id) {
                                    return Some(tool_call);
                                }
                            }
                            None
                        });
                    if let Some(tool_call) = tool_call {
                        let call_id = tool_call.id.clone();
                        messages.push(ChatMessage::assistant_with_tool_calls(vec![tool_call]));
                        messages.push(ChatMessage::tool_result(&call_id, result));
                    }
                }
            }
            _ => {
                // Skip tool_call, compaction, and other meta events for replay
            }
        }
    }

    Ok(history::sanitize_chat_history(&messages).0)
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

#[derive(Debug, Clone, Default)]
pub struct SessionRuntimeState {
    pub current_goal: Option<GoalState>,
}

pub fn load_session_runtime_state(path: &Path) -> Result<SessionRuntimeState, String> {
    let events = load_conversation_events(path)?;
    let mut state = SessionRuntimeState::default();

    for event in events {
        match event.event_type.as_str() {
            event_types::GOAL_UPDATED => {
                let goal: GoalState = serde_json::from_value(event.content)
                    .map_err(|e| format!("parse goal state: {e}"))?;
                state.current_goal = Some(goal);
            }
            event_types::GOAL_CLEARED => {
                state.current_goal = None;
            }
            _ => {}
        }
    }

    Ok(state)
}

/// Summary of a session file extracted from scanning events.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SessionFileSummary {
    pub start_time: u64,
    pub end_time: u64,
    pub total_tokens: u64,
    pub user_turns: u32,
    pub assistant_turns: u32,
    pub tool_calls: u32,
    pub event_count: u32,
    pub work_dir: Option<String>,
    pub source: String,
    /// The original session key as stored in the JSONL events (may differ from the sanitized filename).
    pub session_key: Option<String>,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
}

/// Quick scan of a session JSONL file to extract summary info without loading all events.
/// Returns (total_tokens, user_message_count, start_time, end_time, work_dir, source)
pub fn scan_session_summary(path: &Path) -> SessionFileSummary {
    let mut summary = SessionFileSummary::default();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return summary,
    };
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let event: ConversationEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Extract the original session key from the first event
        if summary.session_key.is_none() && !event.session_key.is_empty() {
            summary.session_key = Some(event.session_key.clone());
        }

        // Track timestamps
        if summary.start_time == 0 || event.timestamp < summary.start_time {
            summary.start_time = event.timestamp;
        }
        if event.timestamp > summary.end_time {
            summary.end_time = event.timestamp;
        }

        match event.event_type.as_str() {
            "session_start" => {
                if let Some(obj) = event.content.as_object() {
                    if let Some(s) = obj.get("source").and_then(|v| v.as_str()) {
                        summary.source = s.to_string();
                    }
                    if let Some(wd) = obj.get("work_dir").and_then(|v| v.as_str()) {
                        summary.work_dir = Some(wd.to_string());
                    }
                }
            }
            "user_message" => {
                summary.user_turns += 1;
            }
            "assistant_message" => {
                summary.assistant_turns += 1;
                if let Some(obj) = event.content.as_object() {
                    if let Some(tokens) = obj.get("tokens").and_then(|v| v.as_u64()) {
                        summary.total_tokens += tokens;
                    }
                }
            }
            "compaction" => {
                if let Some(obj) = event.content.as_object() {
                    if let Some(tokens) = obj.get("total_tokens").and_then(|v| v.as_u64()) {
                        summary.total_tokens = tokens; // Use the latest total from compaction
                    }
                }
            }
            "session_end" => {
                if let Some(obj) = event.content.as_object() {
                    if let Some(tokens) = obj.get("total_tokens").and_then(|v| v.as_u64()) {
                        summary.total_tokens = tokens;
                    }
                }
            }
            "tool_call" => {
                summary.tool_calls += 1;
            }
            "title_updated" => {
                if let Some(obj) = event.content.as_object() {
                    if let Some(t) = obj.get("title").and_then(|v| v.as_str()) {
                        summary.title = Some(t.to_string());
                    }
                }
            }
            _ => {}
        }
        summary.event_count += 1;
    }

    summary
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

/// Remove session JSONL files whose last activity is older than `cutoff_secs`
/// (unix timestamp). Returns the number of files removed.
pub fn cleanup_expired_sessions(data_dir: &Path, cutoff_secs: u64) -> usize {
    let files = list_conversations(data_dir, None);
    let mut removed = 0;
    for p in files {
        let summary = scan_session_summary(&p);
        let last_time = if summary.end_time > 0 {
            summary.end_time
        } else {
            summary.start_time
        };
        if last_time > 0 && last_time < cutoff_secs && std::fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Maximum recursion depth for directory traversal to prevent stack overflow
/// from symlink loops or excessively nested directories.
const MAX_DIR_RECURSION_DEPTH: usize = 16;

/// Recursively collect .jsonl files with a depth limit.
fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) {
    collect_jsonl_files_depth(dir, files, 0);
}

fn collect_jsonl_files_depth(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth >= MAX_DIR_RECURSION_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files_depth(&path, files, depth + 1);
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
        // user_message + recovered assistant(tool_calls) + tool_result + assistant_message
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[1].tool_calls.is_some());
        assert_eq!(messages[2].role, "tool");
        assert_eq!(
            messages[2].tool_call_id.as_deref(),
            Some(messages[1].tool_calls.as_ref().unwrap()[0].id.as_str())
        );
        assert!(history::is_valid_chat_history(&messages));
    }

    #[test]
    fn test_load_conversation_does_not_restore_orphan_tool_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-orphan.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"user_message","session_key":"s","content":{"message":"hello"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"tool_result","session_key":"s","content":{"tool_name":"shell","result":"orphan","success":true}}"#
                + "\n",
        )
        .unwrap();

        let messages = load_conversation(&path).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert!(history::is_valid_chat_history(&messages));
    }

    #[test]
    fn test_resume_rebuilds_valid_chat_message_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "resume-valid");

        recorder
            .record_user_message("resume-valid", "list files")
            .unwrap();
        recorder
            .record_tool_call_with_id(
                "resume-valid",
                "list_directory",
                r#"{"path":"."}"#,
                "function",
                Some("call-real-id"),
            )
            .unwrap();
        recorder
            .record_tool_result_with_call_id(
                "resume-valid",
                "list_directory",
                "[file] Cargo.toml",
                true,
                Some("call-real-id"),
            )
            .unwrap();
        recorder
            .record_assistant_message("resume-valid", "done")
            .unwrap();
        recorder.close();

        let messages = load_conversation(recorder.file_path()).unwrap();

        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[1].tool_calls.as_ref().unwrap()[0].id,
            "call-real-id"
        );
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-real-id"));
        assert!(history::is_valid_chat_history(&messages));
    }

    #[test]
    fn test_load_conversation_matches_multiple_pending_tool_calls_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-multi-tools.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"user_message","session_key":"s","content":{"message":"inspect"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"tool_call","session_key":"s","content":{"call_id":"call-a","tool_name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"tool_call","session_key":"s","content":{"call_id":"call-b","tool_name":"list_directory","arguments":"{\"path\":\".\"}"}}"#
                + "\n"
                + r#"{"timestamp":4,"event_type":"tool_result","session_key":"s","content":{"call_id":"call-a","tool_name":"read_file","result":"cargo content","success":true}}"#
                + "\n"
                + r#"{"timestamp":5,"event_type":"tool_result","session_key":"s","content":{"call_id":"call-b","tool_name":"list_directory","result":"directory content","success":true}}"#
                + "\n",
        )
        .unwrap();

        let messages = load_conversation(&path).unwrap();

        assert_eq!(messages.len(), 5);
        assert_eq!(messages[1].tool_calls.as_ref().unwrap()[0].id, "call-a");
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-a"));
        assert_eq!(messages[2].content.as_deref(), Some("cargo content"));
        assert_eq!(messages[3].tool_calls.as_ref().unwrap()[0].id, "call-b");
        assert_eq!(messages[4].tool_call_id.as_deref(), Some("call-b"));
        assert_eq!(messages[4].content.as_deref(), Some("directory content"));
        assert!(history::is_valid_chat_history(&messages));
    }

    #[test]
    fn test_load_conversation_matches_legacy_tool_results_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-legacy-multi-tools.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"user_message","session_key":"s","content":{"message":"inspect"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"tool_call","session_key":"s","content":{"tool_name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"tool_call","session_key":"s","content":{"tool_name":"list_directory","arguments":"{\"path\":\".\"}"}}"#
                + "\n"
                + r#"{"timestamp":4,"event_type":"tool_result","session_key":"s","content":{"tool_name":"read_file","result":"cargo content","success":true}}"#
                + "\n"
                + r#"{"timestamp":5,"event_type":"tool_result","session_key":"s","content":{"tool_name":"list_directory","result":"directory content","success":true}}"#
                + "\n",
        )
        .unwrap();

        let messages = load_conversation(&path).unwrap();

        assert_eq!(messages.len(), 5);
        assert_eq!(
            messages[1].tool_calls.as_ref().unwrap()[0].name(),
            "read_file"
        );
        assert_eq!(messages[2].content.as_deref(), Some("cargo content"));
        assert_eq!(
            messages[3].tool_calls.as_ref().unwrap()[0].name(),
            "list_directory"
        );
        assert_eq!(messages[4].content.as_deref(), Some("directory content"));
        assert!(history::is_valid_chat_history(&messages));
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
    fn test_load_session_runtime_state_restores_latest_goal() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "goal-runtime");
        let goal = GoalState {
            goal_id: "goal-1".to_string(),
            objective: "finish codex parity".to_string(),
            status: crate::tools::goal::GoalStatus::Complete,
            token_budget: Some(1000),
            created_at: 1,
            updated_at: 2,
            accumulated_tokens_used: 275,
            accumulated_time_used_seconds: 33,
            active_total_tokens_baseline: None,
            active_started_at: None,
            start_total_tokens: 100,
            completed_total_tokens: Some(275),
            completed_time_used_seconds: Some(33),
        };
        recorder.record_goal_updated("goal-runtime", &goal).unwrap();
        recorder.close();

        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert_eq!(state.current_goal, Some(goal));
    }

    #[test]
    fn test_load_session_runtime_state_respects_goal_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "goal-runtime-clear");
        let goal = GoalState {
            goal_id: "goal-2".to_string(),
            objective: "temporary goal".to_string(),
            status: crate::tools::goal::GoalStatus::Active,
            token_budget: None,
            created_at: 1,
            updated_at: 1,
            accumulated_tokens_used: 0,
            accumulated_time_used_seconds: 0,
            active_total_tokens_baseline: Some(0),
            active_started_at: Some(1),
            start_total_tokens: 0,
            completed_total_tokens: None,
            completed_time_used_seconds: None,
        };
        recorder
            .record_goal_updated("goal-runtime-clear", &goal)
            .unwrap();
        recorder.record_goal_cleared("goal-runtime-clear").unwrap();
        recorder.close();

        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert!(state.current_goal.is_none());
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

    #[test]
    fn record_compaction_event_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "compact-session");
        recorder
            .record_compaction(
                "compact-session",
                serde_json::json!({
                    "trigger": "manual",
                    "tokens_saved": 42,
                }),
            )
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, COMPACTION);
        assert_eq!(events[0].content["tokens_saved"], 42);
    }
}
