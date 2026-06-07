//! Conversation persistence: recording and replaying conversation events.
//!
//! Events are stored in JSONL files organized by date and session key:
//! `{data_dir}/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl`

use crate::history;
use crate::tools::goal::GoalState;
use crate::tools::update_plan::PlanStep;
use crate::types::{ChatImageInput, ChatMessage, ToolCallMessage};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

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
    pub const ASSISTANT_DELTA: &str = "assistant_delta";
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
    pub const PLAN_UPDATED: &str = "plan_updated";
    pub const PLAN_CLEARED: &str = "plan_cleared";
    pub const PROPOSED_PLAN: &str = "proposed_plan";
    pub const RUN_STATE_CHANGED: &str = "run_state_changed";
}

// ---------------------------------------------------------------------------
// ConversationRecorder
// ---------------------------------------------------------------------------

/// Records conversation events to a JSONL file.
pub struct ConversationRecorder {
    file_path: PathBuf,
    writer: Option<BufWriter<std::fs::File>>,
    max_bytes: Option<usize>,
    event_count: Option<usize>,
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
            event_count: Some(0),
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

    /// Continue recording into an existing, already-validated JSONL file.
    pub fn from_existing_file(file_path: PathBuf, max_bytes: Option<usize>) -> Self {
        Self {
            file_path,
            writer: None,
            max_bytes: max_bytes.filter(|value| *value > 0),
            event_count: None,
        }
    }

    /// Record a conversation event.
    pub fn record(&mut self, event: ConversationEvent) -> Result<(), String> {
        let current_event_count = self.ensure_event_count()?;
        let writer = self.get_or_create_writer()?;

        let line = serde_json::to_string(&event).map_err(|e| format!("serialize event: {e}"))?;

        writeln!(writer, "{}", line).map_err(|e| format!("write event: {e}"))?;

        // Flush immediately so events are durable even if the process crashes
        // or the recorder is held open across turns.
        writer.flush().map_err(|e| format!("flush event: {e}"))?;
        self.event_count = Some(current_event_count.saturating_add(1));
        if self.enforce_max_bytes()? {
            self.event_count = Some(count_conversation_event_lines(&self.file_path)?);
        }

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

    pub fn record_user_message_with_images(
        &mut self,
        session_key: &str,
        content: &str,
        images: &[ChatImageInput],
    ) -> Result<(), String> {
        if images.is_empty() {
            return self.record_user_message(session_key, content);
        }
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::USER_MESSAGE.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "message": content,
                "images": images,
            }),
        })
    }

    /// Record an assistant message event.
    pub fn record_assistant_message(
        &mut self,
        session_key: &str,
        content: &str,
    ) -> Result<(), String> {
        self.record_assistant_message_with_tokens(session_key, content, None)
    }

    pub fn record_assistant_message_with_tokens(
        &mut self,
        session_key: &str,
        content: &str,
        tokens: Option<u64>,
    ) -> Result<(), String> {
        self.record_assistant_message_with_token_usage(session_key, content, tokens, None)
    }

    pub fn record_assistant_message_with_token_usage(
        &mut self,
        session_key: &str,
        content: &str,
        tokens: Option<u64>,
        context_tokens: Option<u64>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::ASSISTANT_MESSAGE.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "message": content,
                "tokens": tokens,
                "context_tokens": context_tokens,
            }),
        })
    }

    pub fn record_assistant_delta(
        &mut self,
        session_key: &str,
        content: &str,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::ASSISTANT_DELTA.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "message": content }),
        })
    }

    pub fn record_run_state(
        &mut self,
        session_key: &str,
        state: &str,
        source_channel: Option<&str>,
        agent_kind: Option<&str>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::RUN_STATE_CHANGED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "state": state,
                "source_channel": source_channel,
                "agent_kind": agent_kind,
            }),
        })
    }

    /// Record a tool call event with the provider's tool call id.
    pub fn record_tool_call_with_id(
        &mut self,
        session_key: &str,
        tool_name: &str,
        arguments: &str,
        call_id: Option<&str>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::TOOL_CALL.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "call_id": call_id,
                "call_type": "function",
                "tool_name": tool_name,
                "arguments": arguments,
            }),
        })
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

    pub fn record_plan_cleared(&mut self, session_key: &str, reason: &str) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::PLAN_CLEARED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "reason": reason }),
        })
    }

    pub fn record_proposed_plan(&mut self, session_key: &str, content: &str) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::PROPOSED_PLAN.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({ "content": content }),
        })
    }

    pub fn record_plan_updated(
        &mut self,
        session_key: &str,
        plan: &[PlanStep],
        explanation: Option<&str>,
    ) -> Result<(), String> {
        self.record(ConversationEvent {
            timestamp: current_time_secs(),
            event_type: event_types::PLAN_UPDATED.to_string(),
            session_key: session_key.to_string(),
            content: serde_json::json!({
                "explanation": explanation,
                "plan": plan,
            }),
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

    /// Number of non-empty JSONL events recorded in this file after the last write.
    pub fn event_count(&self) -> Option<usize> {
        self.event_count
    }

    fn ensure_event_count(&mut self) -> Result<usize, String> {
        if let Some(count) = self.event_count {
            return Ok(count);
        }
        let count = count_conversation_event_lines(&self.file_path)?;
        self.event_count = Some(count);
        Ok(count)
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

    fn enforce_max_bytes(&mut self) -> Result<bool, String> {
        let Some(max_bytes) = self.max_bytes else {
            return Ok(false);
        };
        let metadata = std::fs::metadata(&self.file_path)
            .map_err(|e| format!("stat session file for max_bytes: {e}"))?;
        if metadata.len() as usize <= max_bytes {
            return Ok(false);
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
        Ok(true)
    }
}

fn count_conversation_event_lines(path: &Path) -> Result<usize, String> {
    if !path.exists() {
        return Ok(0);
    }
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);
    let mut count = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("read conversation file: {e}"))?;
        if !line.trim().is_empty() {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
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

#[derive(Debug, Clone)]
pub struct ConversationLoadReport {
    pub messages: Vec<ChatMessage>,
    pub skipped_lines: usize,
    pub session_key: Option<String>,
}

/// Validate that a caller-provided history path points to a JSONL session file
/// under the configured agent data directory.
pub fn validate_conversation_path(data_dir: &Path, path: &Path) -> Result<PathBuf, String> {
    let sessions_root = data_dir.join("sessions");
    let canonical_root = sessions_root
        .canonicalize()
        .map_err(|e| format!("session history root is not available: {e}"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|e| format!("history file is not available: {e}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("history path is outside the agent sessions directory".to_string());
    }
    let metadata =
        std::fs::metadata(&canonical_path).map_err(|e| format!("stat history file: {e}"))?;
    if !metadata.is_file() {
        return Err("history path is not a file".to_string());
    }
    if canonical_path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return Err("history path must point to a .jsonl file".to_string());
    }
    bifrost_core::text::check_file_size(&canonical_path, MAX_SESSION_FILE_BYTES)
        .map_err(|e| format!("session file too large to load: {e}"))?;
    Ok(canonical_path)
}

/// Load a previous conversation from a JSONL file.
pub fn load_conversation(path: &Path) -> Result<Vec<ChatMessage>, String> {
    bifrost_core::text::check_file_size(path, MAX_SESSION_FILE_BYTES)
        .map_err(|e| format!("session file too large to load: {e}"))?;
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    load_conversation_from_reader(reader, true).map(|report| report.messages)
}

/// Load a previous conversation and skip malformed JSONL rows.
///
/// This is intended for restore/continue flows where a process crash may leave
/// a partially-written final line. Valid rows are still sanitized before use.
pub fn load_conversation_lossy(path: &Path) -> Result<ConversationLoadReport, String> {
    bifrost_core::text::check_file_size(path, MAX_SESSION_FILE_BYTES)
        .map_err(|e| format!("session file too large to load: {e}"))?;
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    load_conversation_from_reader(reader, false)
}

fn load_conversation_from_reader<R: BufRead>(
    reader: R,
    strict: bool,
) -> Result<ConversationLoadReport, String> {
    let mut messages = Vec::new();
    let mut pending_tool_calls: HashMap<String, ToolCallMessage> = HashMap::new();
    let mut skipped_lines = 0usize;
    let mut session_key = None;
    for (line_no, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(e) if strict => return Err(format!("read line: {e}")),
            Err(e) => {
                skipped_lines += 1;
                warn!(line = line_no + 1, error = %e, "skipping unreadable conversation JSONL line");
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let event: ConversationEvent = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(e) if strict => return Err(format!("parse event: {e}")),
            Err(e) => {
                skipped_lines += 1;
                warn!(line = line_no + 1, error = %e, "skipping malformed conversation JSONL line");
                continue;
            }
        };
        if session_key.is_none() && !event.session_key.is_empty() {
            session_key = Some(event.session_key.clone());
        }

        match event.event_type.as_str() {
            event_types::USER_MESSAGE => {
                pending_tool_calls.clear();
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    let images: Vec<ChatImageInput> = event
                        .content
                        .get("images")
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                        .unwrap_or_default();
                    messages.push(ChatMessage::user_with_images(msg, &images));
                }
            }
            event_types::ASSISTANT_MESSAGE => {
                pending_tool_calls.clear();
                if let Some(msg) = event.content.get("message").and_then(|v| v.as_str()) {
                    messages.push(ChatMessage::assistant(msg));
                }
            }
            event_types::TOOL_CALL => {
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
                let Some(call_id) = event
                    .content
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                else {
                    continue;
                };
                let tool_call = ToolCallMessage::function_call(
                    call_id.clone(),
                    tool_name.to_string(),
                    arguments.to_string(),
                );
                pending_tool_calls.insert(call_id, tool_call);
            }
            event_types::TOOL_RESULT => {
                if let Some(result) = event.content.get("result").and_then(|v| v.as_str()) {
                    let result_call_id = event
                        .content
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let tool_call =
                        result_call_id.and_then(|call_id| pending_tool_calls.remove(call_id));
                    if let Some(tool_call) = tool_call {
                        let call_id = tool_call.id.clone();
                        messages.push(ChatMessage::assistant_with_tool_calls(vec![tool_call]));
                        messages.push(ChatMessage::tool_result(&call_id, result));
                    }
                }
            }
            event_types::COMPACTION => {
                if let Some(replacement_history) = event.content.get("replacement_history") {
                    match serde_json::from_value::<Vec<ChatMessage>>(replacement_history.clone()) {
                        Ok(replacement_history) => {
                            messages = history::sanitize_chat_history(&replacement_history).0;
                            pending_tool_calls.clear();
                        }
                        Err(error) if strict => {
                            return Err(format!("parse compaction replacement history: {error}"));
                        }
                        Err(error) => {
                            skipped_lines += 1;
                            warn!(
                                line = line_no + 1,
                                error = %error,
                                "skipping malformed compaction replacement history"
                            );
                        }
                    }
                }
            }
            _ => {
                // Skip other meta events for replay.
            }
        }
    }

    Ok(ConversationLoadReport {
        messages: history::sanitize_chat_history(&messages).0,
        skipped_lines,
        session_key,
    })
}

/// Load raw conversation events from a JSONL file.
///
/// Unlike `load_conversation` which converts events to `ChatMessage`,
/// this returns the raw `ConversationEvent` objects with full details
/// (timestamps, tool call arguments, results, metadata, etc.).
pub fn load_conversation_events(path: &Path) -> Result<Vec<ConversationEvent>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);
    load_conversation_events_from_reader(reader, true)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationEventPageOptions {
    pub limit: Option<usize>,
    /// End-exclusive event index used when loading older history pages.
    pub cursor: Option<usize>,
    /// Load the newest window instead of starting from the beginning.
    pub tail: bool,
    /// Start-inclusive event index used by polling to fetch only new events.
    pub since: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ConversationEventPage {
    pub events: Vec<ConversationEvent>,
    pub total_count: usize,
    pub start_index: usize,
    pub end_index: usize,
    pub next_cursor: Option<usize>,
    pub has_more: bool,
}

pub fn load_conversation_events_page(
    path: &Path,
    options: ConversationEventPageOptions,
) -> Result<ConversationEventPage, String> {
    let limit = options.limit.filter(|value| *value > 0);
    let paged_request =
        limit.is_some() || options.cursor.is_some() || options.since.is_some() || options.tail;
    if !paged_request {
        let events = load_conversation_events(path)?;
        let total_count = events.len();
        return Ok(ConversationEventPage {
            events,
            total_count,
            start_index: 0,
            end_index: total_count,
            next_cursor: None,
            has_more: false,
        });
    }

    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);

    if let Some(since) = options.since {
        let mut events = Vec::new();
        let mut total_count = 0usize;
        for (line_no, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| format!("read line: {e}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let event_index = total_count;
            total_count += 1;
            if event_index < since {
                continue;
            }
            events.push(parse_conversation_event_line(line_no, &line)?);
        }
        let start_index = since.min(total_count);
        return Ok(ConversationEventPage {
            events,
            total_count,
            start_index,
            end_index: total_count,
            next_cursor: Some(total_count),
            has_more: false,
        });
    }

    let mut total_count = 0usize;
    let mut selected_lines: VecDeque<(usize, usize, String)> = VecDeque::new();
    let keep_limit = if options.tail {
        limit
    } else if let Some(cursor) = options.cursor {
        Some(limit.unwrap_or(cursor))
    } else {
        limit
    };
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let event_index = total_count;
        total_count += 1;

        if let Some(cursor) = options.cursor {
            if event_index >= cursor {
                continue;
            }
        }

        if options.tail || options.cursor.is_some() {
            selected_lines.push_back((event_index, line_no, line));
            if let Some(limit) = keep_limit {
                while selected_lines.len() > limit {
                    selected_lines.pop_front();
                }
            }
            continue;
        }

        if let Some(limit) = keep_limit {
            if selected_lines.len() < limit {
                selected_lines.push_back((event_index, line_no, line));
            }
        }
    }

    let mut events = Vec::with_capacity(selected_lines.len());
    let mut start_index = 0usize;
    let mut end_index = 0usize;
    for (position, (event_index, line_no, line)) in selected_lines.into_iter().enumerate() {
        if position == 0 {
            start_index = event_index;
        }
        end_index = event_index + 1;
        events.push(parse_conversation_event_line(line_no, &line)?);
    }
    if events.is_empty() {
        if let Some(cursor) = options.cursor {
            start_index = cursor.min(total_count);
            end_index = start_index;
        } else if options.tail {
            start_index = total_count;
            end_index = total_count;
        }
    }
    let has_more =
        start_index > 0 || (!options.tail && options.cursor.is_none() && end_index < total_count);
    let next_cursor = if options.tail || options.cursor.is_some() {
        Some(start_index)
    } else if limit.is_some() {
        Some(end_index)
    } else {
        None
    };
    Ok(ConversationEventPage {
        events,
        total_count,
        start_index,
        end_index,
        next_cursor,
        has_more,
    })
}

fn parse_conversation_event_line(line_no: usize, line: &str) -> Result<ConversationEvent, String> {
    serde_json::from_str::<ConversationEvent>(line)
        .map_err(|error| format!("parse event at line {}: {error}", line_no + 1))
}

fn load_conversation_events_lossy(path: &Path) -> Result<Vec<ConversationEvent>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open conversation file: {e}"))?;
    let reader = std::io::BufReader::new(file);
    load_conversation_events_from_reader(reader, false)
}

fn load_conversation_events_from_reader<R: BufRead>(
    reader: R,
    strict: bool,
) -> Result<Vec<ConversationEvent>, String> {
    let mut events = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<ConversationEvent>(&line) {
            Ok(event) => events.push(event),
            Err(error) if strict => return Err(format!("parse event: {error}")),
            Err(error) => {
                warn!(
                    line = line_no + 1,
                    error = %error,
                    "skipping malformed event while loading runtime state"
                );
            }
        }
    }

    Ok(events)
}

#[derive(Debug, Clone, Default)]
pub struct SessionRuntimeState {
    pub current_goal: Option<GoalState>,
    pub current_plan: Option<Vec<PlanStep>>,
    pub total_tokens_used: Option<u64>,
    pub last_response_tokens: Option<u64>,
    pub compaction_count: u32,
    pub base_instructions: Option<String>,
}

pub fn load_session_runtime_state(path: &Path) -> Result<SessionRuntimeState, String> {
    let events = load_conversation_events_lossy(path)?;
    let mut state = SessionRuntimeState::default();

    for event in events {
        match event.event_type.as_str() {
            event_types::SESSION_START if state.base_instructions.is_none() => {
                state.base_instructions = event
                    .content
                    .get("base_instructions")
                    .and_then(|v| v.as_str())
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string);
            }
            event_types::GOAL_UPDATED => {
                let goal: GoalState = serde_json::from_value(event.content)
                    .map_err(|e| format!("parse goal state: {e}"))?;
                state.current_goal = Some(goal);
            }
            event_types::GOAL_CLEARED => {
                state.current_goal = None;
            }
            event_types::PLAN_UPDATED => {
                let plan = event
                    .content
                    .get("plan")
                    .cloned()
                    .ok_or_else(|| "plan_updated event missing plan".to_string())?;
                state.current_plan = Some(
                    serde_json::from_value(plan).map_err(|e| format!("parse plan state: {e}"))?,
                );
            }
            event_types::PLAN_CLEARED => {
                state.current_plan = None;
            }
            event_types::COMPACTION => {
                if let Some(tokens) = event
                    .content
                    .get("post_tokens")
                    .and_then(|value| value.as_u64())
                {
                    state.last_response_tokens = Some(tokens);
                }
            }
            event_types::ASSISTANT_MESSAGE => {
                if let Some(tokens) = event
                    .content
                    .get("context_tokens")
                    .or_else(|| event.content.get("tokens"))
                    .and_then(|v| v.as_u64())
                {
                    state.last_response_tokens = Some(tokens);
                }
            }
            _ => {}
        }
    }

    let summary = scan_session_summary(path);
    state.total_tokens_used = Some(summary.total_tokens);
    state.compaction_count = summary.compaction_count;

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
    pub compaction_count: u32,
    pub event_count: u32,
    pub work_dir: Option<String>,
    pub source: String,
    pub runner_type: Option<String>,
    pub runner_id: Option<String>,
    /// The original session key as stored in the JSONL events (may differ from the sanitized filename).
    pub session_key: Option<String>,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
    pub run_state: Option<String>,
    /// First user message preview used as a stable display title fallback.
    pub first_user_message: Option<String>,
}

fn truncate_session_title(value: &str) -> String {
    const MAX_CHARS: usize = 80;
    let preview: String = value.chars().take(MAX_CHARS).collect();
    if value.chars().count() > MAX_CHARS {
        format!("{preview}…")
    } else {
        preview
    }
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
                    if let Some(runner_type) = obj
                        .get("adapter")
                        .or_else(|| obj.get("runner_type"))
                        .or_else(|| obj.get("runtime"))
                        .and_then(|v| v.as_str())
                    {
                        summary.runner_type = Some(runner_type.to_string());
                    }
                    if let Some(runner_id) = obj.get("runner_id").and_then(|v| v.as_str()) {
                        summary.runner_id = Some(runner_id.to_string());
                    }
                }
            }
            "user_message" => {
                summary.user_turns += 1;
                if summary.first_user_message.is_none() {
                    if let Some(message) = event.content.get("message").and_then(|v| v.as_str()) {
                        let message = message.trim();
                        if !message.is_empty() {
                            summary.first_user_message = Some(truncate_session_title(message));
                        }
                    }
                }
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
                summary.compaction_count = summary.compaction_count.saturating_add(1);
                if let Some(obj) = event.content.as_object() {
                    if let Some(count) = obj
                        .get("compaction_count")
                        .and_then(|value| value.as_u64())
                        .and_then(|value| u32::try_from(value).ok())
                    {
                        summary.compaction_count = summary.compaction_count.max(count);
                    }
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
                summary.run_state = Some("completed".to_string());
            }
            "run_state_changed" => {
                if let Some(state) = event.content.get("state").and_then(|v| v.as_str()) {
                    summary.run_state = Some(state.to_string());
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
        let prefix = format!("session-{sanitized}-");
        files.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix) && n.ends_with(".jsonl"))
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
    fn test_validate_conversation_path_rejects_paths_outside_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        let outside = dir.path().join("outside.jsonl");
        std::fs::write(&outside, "").unwrap();

        let error = validate_conversation_path(dir.path(), &outside).unwrap_err();

        assert!(error.contains("outside the agent sessions directory"));
    }

    #[test]
    fn test_load_conversation_lossy_skips_malformed_jsonl_lines() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let path = sessions_dir.join("session-lossy-1.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"user_message","session_key":"lossy","content":{"message":"hello"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"assistant_message","session_key":"lossy","content":{"message":"hi"}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"assistant_message""#
                + "\n",
        )
        .unwrap();

        assert!(load_conversation(&path).is_err());
        let report = load_conversation_lossy(&path).unwrap();

        assert_eq!(report.skipped_lines, 1);
        assert_eq!(report.session_key.as_deref(), Some("lossy"));
        assert_eq!(report.messages.len(), 2);
        assert_eq!(report.messages[0].content.as_deref(), Some("hello"));
        assert_eq!(report.messages[1].content.as_deref(), Some("hi"));
    }

    #[test]
    fn test_existing_file_recorder_appends_to_restored_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "continue-session");
        recorder
            .record_user_message("continue-session", "first")
            .unwrap();
        let path = recorder.file_path().to_path_buf();
        recorder.close();

        let mut continuation = ConversationRecorder::from_existing_file(path.clone(), None);
        continuation
            .record_assistant_message("continue-session", "second")
            .unwrap();
        continuation.close();

        let messages = load_conversation(&path).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.as_deref(), Some("first"));
        assert_eq!(messages[1].content.as_deref(), Some("second"));
    }

    #[test]
    fn test_conversation_recorder_persists_user_images() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "image-session");

        recorder
            .record_user_message_with_images(
                "image-session",
                "describe image",
                &[ChatImageInput {
                    mime_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                }],
            )
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        let images = events[0]
            .content
            .get("images")
            .and_then(|value| value.as_array())
            .expect("images persisted");
        assert_eq!(images[0]["mime_type"], "image/png");
        assert_eq!(images[0]["data"], "aGVsbG8=");

        let messages = load_conversation(recorder.file_path()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.as_deref(), Some("describe image"));
        assert!(messages[0].content_parts.is_some());
        let value = serde_json::to_value(&messages[0]).unwrap();
        assert_eq!(value["content"][1]["type"], "image_url");
        assert_eq!(
            value["content"][1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }

    #[test]
    fn test_conversation_recorder_tool_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test");

        recorder.record_user_message("test", "run ls").unwrap();
        recorder
            .record_tool_call_with_id(
                "test",
                "exec_command",
                r#"{"cmd": "ls"}"#,
                Some("call-exec-command"),
            )
            .unwrap();
        recorder
            .record_tool_result_with_call_id(
                "test",
                "exec_command",
                "file1.txt\nfile2.txt",
                true,
                Some("call-exec-command"),
            )
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
                + r#"{"timestamp":2,"event_type":"tool_result","session_key":"s","content":{"tool_name":"exec_command","result":"orphan","success":true}}"#
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
    fn test_list_conversations_uses_exact_session_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut exact = ConversationRecorder::new(dir.path(), "abc");
        exact.record_user_message("abc", "exact").unwrap();
        exact.close();
        let mut superset = ConversationRecorder::new(dir.path(), "xabc");
        superset.record_user_message("xabc", "wrong").unwrap();
        superset.close();

        let files = list_conversations(dir.path(), Some("abc"));

        assert_eq!(files.len(), 1);
        assert!(files[0]
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("session-abc-")));
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
            .record_tool_call_with_id(
                "test-events",
                "exec_command",
                r#"{"cmd": "ls"}"#,
                Some("call-exec-command"),
            )
            .unwrap();
        recorder
            .record_tool_result_with_call_id(
                "test-events",
                "exec_command",
                "file1.txt",
                true,
                Some("call-exec-command"),
            )
            .unwrap();
        recorder
            .record_assistant_delta("test-events", "checking result")
            .unwrap();
        recorder
            .record_assistant_message("test-events", "done")
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events.len(), 5);

        // Verify event types and content
        assert_eq!(events[0].event_type, USER_MESSAGE);
        assert_eq!(events[0].content["message"], "hello");

        assert_eq!(events[1].event_type, TOOL_CALL);
        assert_eq!(events[1].content["call_id"], "call-exec-command");
        assert_eq!(events[1].content["tool_name"], "exec_command");
        assert_eq!(events[1].content["arguments"], r#"{"cmd": "ls"}"#);

        assert_eq!(events[2].event_type, TOOL_RESULT);
        assert_eq!(events[2].content["call_id"], "call-exec-command");
        assert_eq!(events[2].content["tool_name"], "exec_command");
        assert_eq!(events[2].content["result"], "file1.txt");
        assert_eq!(events[2].content["success"], true);

        assert_eq!(events[3].event_type, ASSISTANT_DELTA);
        assert_eq!(events[3].content["message"], "checking result");

        assert_eq!(events[4].event_type, ASSISTANT_MESSAGE);
        assert_eq!(events[4].content["message"], "done");

        // Verify timestamps are present
        for event in &events {
            assert!(event.timestamp > 0);
        }
    }

    #[test]
    fn test_load_conversation_events_page_supports_tail_cursor_and_since() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "paged-events");

        recorder
            .record_user_message("paged-events", "first")
            .unwrap();
        recorder
            .record_assistant_delta("paged-events", "thinking")
            .unwrap();
        recorder
            .record_tool_call_with_id(
                "paged-events",
                "exec_command",
                r#"{"cmd":"pwd"}"#,
                Some("call-1"),
            )
            .unwrap();
        recorder
            .record_tool_result_with_call_id(
                "paged-events",
                "exec_command",
                "/tmp/project",
                true,
                Some("call-1"),
            )
            .unwrap();
        recorder
            .record_user_message("paged-events", "second")
            .unwrap();
        recorder
            .record_assistant_message("paged-events", "done")
            .unwrap();
        recorder.close();

        let tail = load_conversation_events_page(
            recorder.file_path(),
            ConversationEventPageOptions {
                limit: Some(2),
                tail: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(tail.total_count, 6);
        assert_eq!(tail.start_index, 4);
        assert_eq!(tail.end_index, 6);
        assert_eq!(tail.next_cursor, Some(4));
        assert!(tail.has_more);
        assert_eq!(tail.events[0].event_type, USER_MESSAGE);
        assert_eq!(tail.events[1].event_type, ASSISTANT_MESSAGE);

        let older = load_conversation_events_page(
            recorder.file_path(),
            ConversationEventPageOptions {
                limit: Some(3),
                cursor: tail.next_cursor,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(older.start_index, 1);
        assert_eq!(older.end_index, 4);
        assert_eq!(older.next_cursor, Some(1));
        assert!(older.has_more);
        assert_eq!(
            older
                .events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![ASSISTANT_DELTA, TOOL_CALL, TOOL_RESULT]
        );

        let delta = load_conversation_events_page(
            recorder.file_path(),
            ConversationEventPageOptions {
                since: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(delta.start_index, 5);
        assert_eq!(delta.end_index, 6);
        assert_eq!(delta.next_cursor, Some(6));
        assert!(!delta.has_more);
        assert_eq!(delta.events.len(), 1);
        assert_eq!(delta.events[0].event_type, ASSISTANT_MESSAGE);
    }

    #[test]
    fn test_load_conversation_events_page_does_not_parse_unselected_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{not valid json}\n",
                "{\"timestamp\":1,\"event_type\":\"user_message\",\"session_key\":\"paged-events\",\"content\":{\"message\":\"old\"}}\n",
                "{\"timestamp\":2,\"event_type\":\"user_message\",\"session_key\":\"paged-events\",\"content\":{\"message\":\"new\"}}\n",
                "{\"timestamp\":3,\"event_type\":\"assistant_message\",\"session_key\":\"paged-events\",\"content\":{\"message\":\"done\"}}\n",
            ),
        )
        .unwrap();

        let page = load_conversation_events_page(
            &path,
            ConversationEventPageOptions {
                limit: Some(2),
                tail: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(page.total_count, 4);
        assert_eq!(page.start_index, 2);
        assert_eq!(page.end_index, 4);
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].content["message"], "new");
        assert_eq!(page.events[1].content["message"], "done");
        assert!(load_conversation_events(&path).is_err());
    }

    #[test]
    fn test_load_session_runtime_state_restores_latest_goal() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "goal-runtime");
        let goal = GoalState {
            goal_id: "goal-1".to_string(),
            objective: "finish codex parity".to_string(),
            status: crate::tools::goal::GoalStatus::Complete,
            pause_reason: None,
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
        assert_eq!(state.total_tokens_used, Some(0));
    }

    #[test]
    fn test_load_session_runtime_state_respects_goal_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "goal-runtime-clear");
        let goal = GoalState {
            goal_id: "goal-2".to_string(),
            objective: "temporary goal".to_string(),
            status: crate::tools::goal::GoalStatus::Active,
            pause_reason: None,
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
        assert_eq!(state.total_tokens_used, Some(0));
    }

    #[test]
    fn test_load_session_runtime_state_restores_total_tokens_from_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goal-runtime-tokens.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"assistant_message","session_key":"s","content":{"message":"done","tokens":120}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"compaction","session_key":"s","content":{"total_tokens":140,"compaction_count":1}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"session_end","session_key":"s","content":{"total_tokens":150}}"#
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        assert_eq!(state.current_goal, None);
        assert_eq!(state.total_tokens_used, Some(150));
        assert_eq!(state.last_response_tokens, Some(120));
        assert_eq!(state.compaction_count, 1);
    }

    #[test]
    fn test_load_session_runtime_state_keeps_context_snapshot_separate_from_cumulative_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context-snapshot.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"assistant_message","session_key":"s","content":{"message":"first","tokens":120,"context_tokens":100}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"assistant_message","session_key":"s","content":{"message":"second","tokens":180,"context_tokens":150}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"session_end","session_key":"s","content":{"total_tokens":50000}}"#
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        assert_eq!(state.total_tokens_used, Some(50000));
        assert_eq!(state.last_response_tokens, Some(150));
    }

    #[test]
    fn test_load_session_runtime_state_falls_back_to_total_tokens_for_old_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context-snapshot-legacy.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"assistant_message","session_key":"s","content":{"message":"legacy","tokens":180}}"#
                .to_string()
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        assert_eq!(state.total_tokens_used, Some(180));
        assert_eq!(state.last_response_tokens, Some(180));
    }

    #[test]
    fn test_load_session_runtime_state_uses_compaction_post_tokens_as_context_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context-snapshot-compacted.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"assistant_message","session_key":"s","content":{"message":"before","tokens":1200}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"compaction","session_key":"s","content":{"total_tokens":1500,"post_tokens":240,"compaction_count":1}}"#
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        assert_eq!(state.total_tokens_used, Some(1500));
        assert_eq!(state.last_response_tokens, Some(240));
        assert_eq!(state.compaction_count, 1);
    }

    #[test]
    fn test_record_assistant_message_with_tokens_updates_runtime_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "assistant-tokens");
        recorder
            .record_assistant_message_with_token_usage(
                "assistant-tokens",
                "done",
                Some(42),
                Some(35),
            )
            .unwrap();
        recorder.close();

        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert_eq!(state.total_tokens_used, Some(42));
        assert_eq!(state.last_response_tokens, Some(35));
        assert_eq!(state.compaction_count, 0);
    }

    #[test]
    fn scan_session_summary_uses_first_user_message_as_title_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-title.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"session_start","session_key":"s","content":{"source":"admin-api","work_dir":"/tmp/work"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"user_message","session_key":"s","content":{"message":"  hello from the first turn  "}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"assistant_message","session_key":"s","content":{"message":"done"}}"#
                + "\n",
        )
        .unwrap();

        let summary = scan_session_summary(&path);
        assert_eq!(
            summary.first_user_message.as_deref(),
            Some("hello from the first turn")
        );
        assert_eq!(summary.title, None);
        assert_eq!(summary.work_dir.as_deref(), Some("/tmp/work"));
    }

    #[test]
    fn scan_session_summary_keeps_explicit_title_separate_from_first_user_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-explicit-title.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"session_start","session_key":"s","content":{"source":"admin-api"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"user_message","session_key":"s","content":{"message":"first user fallback text"}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"title_updated","session_key":"s","content":{"title":"Bifrost Edge"}}"#
                + "\n",
        )
        .unwrap();

        let summary = scan_session_summary(&path);
        assert_eq!(summary.title.as_deref(), Some("Bifrost Edge"));
        assert_eq!(
            summary.first_user_message.as_deref(),
            Some("first user fallback text")
        );
    }

    #[test]
    fn scan_session_summary_tracks_latest_run_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "run-state-session");
        recorder
            .record_session_start(
                "run-state-session",
                serde_json::json!({"source": "im", "runner_id": "builtin"}),
            )
            .unwrap();
        recorder
            .record_run_state("run-state-session", "running", Some("im"), Some("builtin"))
            .unwrap();
        recorder
            .record_run_state(
                "run-state-session",
                "completed",
                Some("web"),
                Some("builtin"),
            )
            .unwrap();
        recorder.close();

        let summary = scan_session_summary(recorder.file_path());
        assert_eq!(summary.run_state.as_deref(), Some("completed"));
        assert_eq!(summary.event_count, 3);
    }

    #[test]
    fn scan_session_summary_tracks_external_runner_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "external-runner-session");
        recorder
            .record_session_start(
                "external-runner-session",
                serde_json::json!({
                    "source": "admin-api",
                    "runtime": "external_cli",
                    "adapter": "traex",
                    "runner_id": "traex",
                    "work_dir": "/tmp/work",
                }),
            )
            .unwrap();
        recorder
            .record_user_message("external-runner-session", "hello")
            .unwrap();
        recorder.close();

        let summary = scan_session_summary(recorder.file_path());
        assert_eq!(summary.runner_type.as_deref(), Some("traex"));
        assert_eq!(summary.runner_id.as_deref(), Some("traex"));
        assert_eq!(summary.work_dir.as_deref(), Some("/tmp/work"));
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
        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert_eq!(state.compaction_count, 1);
    }

    #[test]
    fn test_plan_updated_round_trip_restores_runtime_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "plan-session");
        let plan = vec![
            PlanStep {
                step: "inspect logs".to_string(),
                status: crate::tools::update_plan::PlanStepStatus::Completed,
            },
            PlanStep {
                step: "fix compaction".to_string(),
                status: crate::tools::update_plan::PlanStepStatus::InProgress,
            },
        ];

        recorder
            .record_plan_updated("plan-session", &plan, Some("initial plan"))
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events[0].event_type, PLAN_UPDATED);
        assert_eq!(events[0].content["plan"][0]["step"], "inspect logs");
        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert_eq!(state.current_plan, Some(plan));
    }

    #[test]
    fn test_plan_cleared_round_trip_resets_runtime_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "plan-clear-session");
        let plan = vec![PlanStep {
            step: "old task".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::Completed,
        }];

        recorder
            .record_plan_updated("plan-clear-session", &plan, Some("done"))
            .unwrap();
        recorder
            .record_plan_cleared("plan-clear-session", "new_turn_after_completion")
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events[0].event_type, PLAN_UPDATED);
        assert_eq!(events[1].event_type, PLAN_CLEARED);
        assert_eq!(events[1].content["reason"], "new_turn_after_completion");
        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert!(state.current_plan.is_none());
    }

    #[test]
    fn test_proposed_plan_round_trip_records_plan_mode_result() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "proposed-plan-session");

        recorder
            .record_proposed_plan("proposed-plan-session", "- inspect\n- verify")
            .unwrap();
        recorder.close();

        let events = load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events[0].event_type, PROPOSED_PLAN);
        assert_eq!(events[0].content["content"], "- inspect\n- verify");
    }

    #[test]
    fn test_compaction_does_not_mutate_plan_runtime_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "plan-compact-session");
        let plan = vec![PlanStep {
            step: "current task".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::Completed,
        }];

        recorder
            .record_plan_updated("plan-compact-session", &plan, Some("done"))
            .unwrap();
        recorder
            .record_compaction(
                "plan-compact-session",
                serde_json::json!({
                    "summary": "compact handoff summary",
                }),
            )
            .unwrap();
        recorder.close();

        let state = load_session_runtime_state(recorder.file_path()).unwrap();
        assert_eq!(state.current_plan, Some(plan));
    }

    #[test]
    fn test_runtime_state_skips_malformed_jsonl_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime-lossy.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"plan_updated","session_key":"s","content":{"plan":[{"step":"keep plan","status":"completed"}]}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"assistant_message""#
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        let plan = state.current_plan.expect("plan restored");
        assert_eq!(plan[0].step, "keep plan");
    }

    #[test]
    fn test_load_conversation_uses_compaction_replacement_history_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-compacted.jsonl");
        let replacement_history = serde_json::to_string(&vec![
            ChatMessage::user("latest user request"),
            ChatMessage::user("compacted summary"),
        ])
        .unwrap();
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                r#"{"timestamp":1,"event_type":"user_message","session_key":"s","content":{"message":"old user"}}"#,
                r#"{"timestamp":2,"event_type":"compaction","session_key":"s","content":{"compaction_count":1,"replacement_history":"#
                    .to_string()
                    + &replacement_history
                    + "}}",
                r#"{"timestamp":3,"event_type":"assistant_message","session_key":"s","content":{"message":"continued after compact"}}"#,
            ),
        )
        .unwrap();

        let messages = load_conversation(&path).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content.as_deref(), Some("latest user request"));
        assert_eq!(messages[1].content.as_deref(), Some("compacted summary"));
        assert_eq!(
            messages[2].content.as_deref(),
            Some("continued after compact")
        );
        assert!(messages
            .iter()
            .all(|message| { message.content.as_deref() != Some("old user") }));
    }

    #[test]
    fn scan_session_summary_uses_recorded_compaction_count_when_higher() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"compaction","session_key":"s","content":{"total_tokens":10,"compaction_count":3}}"#
                .to_string()
                + "\n",
        )
        .unwrap();

        let state = load_session_runtime_state(&path).unwrap();
        assert_eq!(state.compaction_count, 3);
    }
}
