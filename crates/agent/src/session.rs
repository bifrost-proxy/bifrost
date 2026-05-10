//! Agent session management and core turn loop.
//!
//! Session lifecycle:
//! - Created on first message from a user
//! - Tracks conversation history, token usage, and compaction state
//! - Expired sessions are cleaned up by the session manager
//!
//! Turn loop (inspired by Codex's session/turn.rs):
//! 1. Build prompt prefix and run Codex-style pre-sampling compaction when needed
//! 2. Add the new user message and send to model with available tools
//! 3. If model returns tool_calls or pending input → mid-turn compaction check → loop
//! 4. If model returns text → return as final response

use crate::client::AgentClient;
use crate::compact;
use crate::config::AgentConfig;
use crate::history;
use crate::mcp::McpManager;
use crate::memory;
use crate::persistence;
use crate::persistence::ConversationRecorder;
use crate::prompt;
use crate::session_status::{
    config_context_window_tokens, context_usage_percent, refresh_active_turn_status,
    ActiveTurnProgress, ActiveTurnStatus, ActiveTurnStatusHandle,
};
use crate::skill_authoring::SkillAuthoringHub;
use crate::slash::{BuiltinCommand, Dispatch, SlashCommandRouter};
use crate::tools::tool_search::{
    parse_loadable_tool_definitions, tool_search_definition, ToolSearchEntry, ToolSearchTool,
    TOOL_SEARCH_TOOL_NAME,
};
use crate::tools::ToolHandler;
use crate::tools::ToolRegistry;
use crate::types::{ChatImageInput, ChatMessage, ToolCallLog, ToolCallMessage, TurnResult};
use bifrost_skills::{default_roots, SkillRegistry, SkillStore};
use dashmap::{DashMap, DashSet};
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// AgentSession
// ---------------------------------------------------------------------------

pub type AgentStopSignalHandle = Arc<AgentStopSignal>;

/// Cooperative stop signal for the turn currently checked out by a session.
#[derive(Debug)]
pub struct AgentStopSignal {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl AgentStopSignal {
    fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn request_stop(&self) -> bool {
        let was_requested = self.requested.swap(true, Ordering::SeqCst);
        if !was_requested {
            self.notify.notify_waiters();
        }
        !was_requested
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        if self.is_requested() {
            return;
        }
        loop {
            self.notify.notified().await;
            if self.is_requested() {
                return;
            }
        }
    }
}

/// Manages conversation history and context state for a single session.
pub struct AgentSession {
    /// Conversation history (system prompt is NOT stored here; it's prepended at build time).
    pub history: Vec<ChatMessage>,

    /// Session key (e.g. user ID or chat ID).
    pub session_key: String,

    /// Stable user ID used for per-user long-term memory scope.
    pub user_id: Option<String>,

    /// Created timestamp (seconds since epoch).
    pub created_at: u64,

    /// Last active timestamp (seconds since epoch).
    pub last_active_at: u64,

    /// How many times this session has been compacted.
    pub compaction_count: u32,

    /// Cumulative token usage from API responses (real, not estimated).
    /// Updated after each model call. `None` means no API call has been made yet.
    pub total_tokens_used: Option<u64>,

    /// Token count from the last API response (for mid-turn budget checks).
    pub last_response_tokens: Option<u64>,

    /// History length immediately after the last model-generated item covered by
    /// `last_response_tokens`. Items appended after this boundary are estimated
    /// and added to the last API usage, matching Codex context accounting.
    pub last_response_history_len: Option<usize>,

    /// Bumped whenever history is rewritten (compaction, rollback, clear).
    /// Mirrors Codex's `history_version` for detecting stale state.
    pub history_version: u64,

    /// Working directory for this session. Overrides config.work_dir when set.
    pub work_dir: Option<String>,

    /// Source of the session (e.g., "feishu", "api", "unknown").
    pub source: String,

    /// Optional conversation recorder for session persistence.
    /// When set, events are recorded to a JSONL file across turns.
    pub recorder: Option<ConversationRecorder>,

    /// Slash command router for built-in commands and skill-declared slash commands.
    pub slash_router: SlashCommandRouter,

    /// Shared skill registry attached to this session.
    pub skill_registry: Option<Arc<SkillRegistry>>,

    /// Hub tracking in-flight `/skill` authoring state machines, keyed by
    /// `session_key`. Populated lazily by the `/skill` handler.
    pub skill_authoring: SkillAuthoringHub,

    /// When true, skip memory injection for this turn (set by /clear, reset after use).
    /// This prevents the model from "remembering" prior context immediately after a clear.
    pub memory_cleared: bool,

    /// Guide-mode injection channel.
    /// When set, the turn loop checks this after each tool call batch completes.
    /// If a message is present, it is appended to history before the next model call.
    pub guide_channel: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,

    /// Pending message queue. When the turn loop finishes (model returns stop
    /// with no tool calls and no guide message), it checks this queue. If there
    /// are messages pending, the next one is popped and processed as a new user
    /// message within the same `run_turn_with_mcp` call, avoiding the need for
    /// the outer loop in the IM handler to drive queue drain.
    pub pending_messages: std::collections::VecDeque<String>,

    /// Session title (intent/topic) set by the agent via set_title tool.
    /// Displayed in the Feishu card header instead of "Bifrost AI".
    pub title: Option<String>,

    /// Current task plan owned by the runtime for this session.
    /// Updated whenever update_plan succeeds, and used as the source of truth
    /// for final TurnResult.plan_steps and IM card rendering.
    pub current_plan: Option<Vec<crate::tools::update_plan::PlanStep>>,

    /// Current goal owned by this session.
    /// Scoped per session to avoid cross-session goal leakage.
    pub current_goal: Option<crate::tools::goal::GoalState>,

    /// Pre-loaded user instructions (AGENTS.md + config user_instructions).
    /// Resolved once at session creation time, matching Codex's behavior.
    /// Reused across all turns in this session.
    pub user_instructions: Option<String>,

    /// Resolved base/system instructions for this session.
    /// When no per-turn route override is present, this is resolved once and
    /// reused so later config changes do not silently alter an active session.
    pub resolved_base_instructions: Option<String>,

    /// How many times the runtime has reminded the model to close an unfinished
    /// plan before allowing the final answer to return.
    pub plan_repair_attempts: u8,

    /// Channel to push real-time plan updates to IM (e.g., for Feishu card rendering).
    /// Each message carries (plan_steps, current_session_title) so the card always
    /// reflects the latest title even when set_title is called mid-turn.
    pub plan_sender: Option<
        tokio::sync::mpsc::UnboundedSender<(
            Vec<crate::tools::update_plan::PlanStep>,
            Option<String>,
        )>,
    >,

    /// Shared runtime status for the turn currently executing this session.
    /// The session manager keeps another handle while the session is checked
    /// out, so `/status` can inspect progress without taking the session lock.
    pub active_turn_status: Option<ActiveTurnStatusHandle>,

    /// Cooperative stop signal owned by the active turn.
    /// `/stop` sets this through the session manager while the session remains
    /// checked out by the running loop.
    pub stop_signal: Option<AgentStopSignalHandle>,
}

impl AgentSession {
    pub fn new(session_key: &str) -> Self {
        let now = current_time_secs();
        Self {
            history: Vec::new(),
            session_key: session_key.to_string(),
            user_id: None,
            created_at: now,
            last_active_at: now,
            compaction_count: 0,
            total_tokens_used: None,
            last_response_tokens: None,
            last_response_history_len: None,
            history_version: 0,
            work_dir: None,
            source: "unknown".to_string(),
            recorder: None,
            slash_router: SlashCommandRouter::with_default_builtins(),
            skill_registry: None,
            skill_authoring: SkillAuthoringHub::new(),
            memory_cleared: false,
            guide_channel: None,
            pending_messages: std::collections::VecDeque::new(),
            title: None,
            current_plan: None,
            current_goal: None,
            user_instructions: None,
            resolved_base_instructions: None,
            plan_repair_attempts: 0,
            plan_sender: None,
            active_turn_status: None,
            stop_signal: None,
        }
    }

    pub fn new_with_work_dir(session_key: &str, work_dir: Option<String>) -> Self {
        let mut session = Self::new(session_key);
        session.work_dir = work_dir;
        session.attach_default_skill_registry();
        session
    }

    /// Reinitialize this session around a new working directory.
    ///
    /// Switching project roots changes both AGENTS.md discovery and repo-local
    /// skills, so this deliberately clears conversation state and rebuilds the
    /// skill registry from the new directory.
    pub fn reinitialize_work_dir(&mut self, work_dir: String) {
        self.work_dir = Some(work_dir);
        self.clear();
        self.slash_router = SlashCommandRouter::with_default_builtins();
        self.skill_registry = None;
        self.attach_default_skill_registry();
    }

    /// Load user instructions (AGENTS.md + config user_instructions) once at session
    /// creation time, matching Codex's behavior. The result is stored on the
    /// session and reused across all turns.
    pub fn load_user_instructions(&mut self, config: &crate::config::AgentConfig) {
        let work_dir = self
            .work_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| config.resolve_work_dir());
        let home_dir = crate::config::agent_home_dir();
        let manager = crate::agents_md::AgentsMdManager::new(config);
        self.user_instructions = manager.user_instructions(
            &work_dir,
            Some(&home_dir),
            config.user_instructions.as_deref(),
        );
    }

    pub fn with_skills(mut self, skills: Arc<SkillRegistry>) -> Self {
        self.slash_router = self.slash_router.with_skills(Arc::clone(&skills));
        self.skill_registry = Some(skills);
        self
    }

    fn attach_default_skill_registry(&mut self) {
        let work_dir = self
            .work_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::config::AgentConfig::default().resolve_work_dir());
        let roots = default_roots(crate::config::user_home_dir(), work_dir);
        let store = Arc::new(SkillStore::new(roots));
        match SkillRegistry::without_watcher(store) {
            Ok(registry) => {
                let registry = Arc::new(registry);
                self.slash_router = self.slash_router.clone().with_skills(Arc::clone(&registry));
                self.skill_registry = Some(registry);
            }
            Err(error) => {
                warn!(error = %error, "failed to attach default skill registry");
            }
        }
    }

    pub fn with_source(mut self, source: &str) -> Self {
        self.source = source.to_string();
        self
    }

    pub fn clear(&mut self) {
        if self.current_goal.is_some() {
            if let Some(recorder) = self.recorder.as_mut() {
                if let Err(error) = recorder.record_goal_cleared(&self.session_key) {
                    warn!(error = %error, "failed to record goal clear");
                }
            }
        }
        self.history.clear();
        self.compaction_count = 0;
        self.total_tokens_used = None;
        self.invalidate_token_snapshot();
        self.history_version = self.history_version.saturating_add(1);
        self.current_plan = None;
        self.current_goal = None;
        self.plan_repair_attempts = 0;
        // Invalidate cached user instructions so they are reloaded on next turn
        // (e.g. after switch_workdir changes the project context).
        self.user_instructions = None;
        self.resolved_base_instructions = None;
        // Mark that memory should be skipped for the next turn
        self.memory_cleared = true;
        // Drop the recorder so a new file will be created for the fresh session
        self.recorder = None;
    }

    /// Drop the last `num_turns` user turns from history (rollback).
    ///
    /// A "user turn" starts at a user message and includes all subsequent
    /// assistant/tool messages until the next user message.
    /// Mirrors Codex's `drop_last_n_user_turns`.
    pub fn rollback(&mut self, num_turns: u32) -> usize {
        if num_turns == 0 || self.history.is_empty() {
            return 0;
        }

        // Find indices of all user messages
        let user_positions: Vec<usize> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "user")
            .map(|(i, _)| i)
            .collect();

        if user_positions.is_empty() {
            return 0;
        }

        let n = num_turns as usize;
        let cut_idx = if n >= user_positions.len() {
            0
        } else {
            user_positions[user_positions.len() - n]
        };

        let old_len = self.history.len();
        let summary = if self.compaction_count > 0 {
            self.history
                .iter()
                .find(|message| compact::is_summary_message(message))
                .cloned()
        } else {
            None
        };
        self.history.truncate(cut_idx);
        if let Some(summary) = summary {
            if !self.history.iter().any(compact::is_summary_message) {
                self.history.push(summary);
            }
        }
        let removed = old_len.saturating_sub(self.history.len());
        self.history_version = self.history_version.saturating_add(1);
        // Reset token snapshot since history changed.
        self.invalidate_token_snapshot();
        removed
    }

    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        current_time_secs() - self.last_active_at > ttl_secs
    }

    fn touch(&mut self) {
        self.last_active_at = current_time_secs();
    }

    /// Track token usage from an API response.
    pub(crate) fn track_token_usage(&mut self, total_tokens: u64) {
        self.last_response_tokens = Some(total_tokens);
        // The assistant/model item is appended just after the API response is
        // processed. Until that append happens, the current history length is
        // the correct "items after last model item" boundary.
        self.last_response_history_len = Some(self.history.len());
        self.accumulate_token_usage(total_tokens);
    }

    /// Track token usage for background model calls that do not represent the
    /// current session context, such as compaction summaries.
    pub(crate) fn track_background_token_usage(&mut self, total_tokens: u64) {
        self.accumulate_token_usage(total_tokens);
    }

    fn accumulate_token_usage(&mut self, total_tokens: u64) {
        self.total_tokens_used = Some(
            self.total_tokens_used
                .unwrap_or(0)
                .saturating_add(total_tokens),
        );
    }

    pub(crate) fn invalidate_token_snapshot(&mut self) {
        self.last_response_tokens = None;
        self.last_response_history_len = None;
    }

    /// Recompute the context snapshot after a history replacement.
    ///
    /// Compaction installs a much smaller replacement history. Keeping the old
    /// API-response token count would keep `effective_token_count()` above the
    /// threshold and cause the next loop to compact again immediately.
    pub(crate) fn recompute_token_snapshot_from_history(
        &mut self,
        base_instructions: Option<&str>,
    ) {
        let base_tokens = base_instructions
            .filter(|instructions| !instructions.trim().is_empty())
            .map(|instructions| {
                Self::estimate_messages_tokens(&[ChatMessage::system(instructions)])
            })
            .unwrap_or(0);
        self.last_response_tokens = Some(u64::from(
            self.estimate_tokens().saturating_add(base_tokens),
        ));
        self.last_response_history_len = Some(self.history.len());
    }

    fn add_user_message(&mut self, content: &str) {
        self.history.push(ChatMessage::user(content));
        self.touch();
    }

    fn add_user_message_with_images(&mut self, content: &str, images: &[ChatImageInput]) {
        self.history
            .push(ChatMessage::user_with_images(content, images));
        self.touch();
    }

    fn add_assistant_message(&mut self, content: &str) {
        let previous_len = self.history.len();
        self.history.push(ChatMessage::assistant(content));
        self.mark_model_item_boundary_if_pending(previous_len);
        self.touch();
    }

    fn add_assistant_tool_calls(&mut self, tool_calls: &[ToolCallMessage]) {
        let previous_len = self.history.len();
        self.history
            .push(ChatMessage::assistant_with_tool_calls(tool_calls.to_vec()));
        self.mark_model_item_boundary_if_pending(previous_len);
        self.touch();
    }

    fn add_tool_result(&mut self, call_id: &str, content: &str) {
        self.history
            .push(ChatMessage::tool_result(call_id, content));
        self.touch();
    }

    fn mark_model_item_boundary_if_pending(&mut self, previous_len: usize) {
        if self.last_response_tokens.is_some()
            && self.last_response_history_len == Some(previous_len)
        {
            self.last_response_history_len = Some(self.history.len());
        }
    }

    /// Build the full message list for a model call.
    ///
    /// Enforces `max_history_messages` limit by dropping the oldest non-summary
    /// messages while preserving the compaction summary wherever Codex-style
    /// replacement history placed it.
    fn build_messages(
        &self,
        prompt_prefix: &[ChatMessage],
        memory_message: Option<&ChatMessage>,
        max_history: u32,
    ) -> Vec<ChatMessage> {
        let history = &self.history;
        let max = max_history as usize;

        let selected_history = if max > 0 && history.len() > max {
            if let Some(summary_idx) = history.iter().position(compact::is_summary_message) {
                // Preserve the compaction summary wherever it lives. Codex local
                // compaction places it after preserved user messages, and later
                // turns may append newer messages after it.
                let summary = history[summary_idx].clone();
                let tail_start = history.len().saturating_sub(max);
                if summary_idx >= tail_start {
                    history[tail_start..].to_vec()
                } else {
                    let tail_budget = max.saturating_sub(1);
                    let tail_start = history.len().saturating_sub(tail_budget);
                    let mut tail = Vec::with_capacity(tail_budget.saturating_add(1));
                    tail.insert(0, summary);
                    tail.extend_from_slice(&history[tail_start..]);
                    tail
                }
            } else {
                // No compaction summary — just take the most recent messages
                let tail_start = history.len().saturating_sub(max);
                history[tail_start..].to_vec()
            }
        } else {
            history.to_vec()
        };

        let mut messages = Vec::with_capacity(
            prompt_prefix
                .len()
                .saturating_add(usize::from(memory_message.is_some()))
                .saturating_add(selected_history.len()),
        );
        for message in prompt_prefix {
            if message.role == "system"
                || !Self::messages_contain_equivalent_context_message(&selected_history, message)
            {
                messages.push(message.clone());
            }
        }
        if let Some(memory_message) = memory_message {
            if !Self::messages_contain_equivalent_context_message(&selected_history, memory_message)
            {
                messages.push(memory_message.clone());
            }
        }
        messages.extend(selected_history);

        let (sanitized, report) = history::sanitize_chat_history(&messages);
        if report.dropped_anything() {
            warn!(
                dropped_orphan_tool_messages = report.dropped_orphan_tool_messages,
                dropped_incomplete_tool_call_messages =
                    report.dropped_incomplete_tool_call_messages,
                original_message_count = messages.len(),
                sanitized_message_count = sanitized.len(),
                "sanitized malformed agent chat history before model request"
            );
        }
        sanitized
    }

    fn messages_contain_equivalent_context_message(
        messages: &[ChatMessage],
        candidate: &ChatMessage,
    ) -> bool {
        if candidate.role == "system" || candidate.tool_calls.is_some() {
            return false;
        }
        messages
            .iter()
            .any(|message| Self::context_messages_equivalent(message, candidate))
    }

    fn context_messages_equivalent(left: &ChatMessage, right: &ChatMessage) -> bool {
        left.role == right.role
            && left.content == right.content
            && left.content_parts == right.content_parts
            && left.tool_calls.is_none()
            && right.tool_calls.is_none()
            && left.tool_call_id == right.tool_call_id
            && left.name == right.name
    }

    /// Rough token count estimate (1 token ≈ 4 chars) for a message slice.
    /// This is a coarse lower bound, same approach as Codex
    /// (codex-rs/utils/string/src/truncate.rs).
    fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
        let total_chars: usize = messages
            .iter()
            .map(|m| {
                m.content.as_ref().map(|c| c.len()).unwrap_or(0)
                    + m.content_parts
                        .as_ref()
                        .map(|parts| {
                            parts
                                .iter()
                                .map(|part| match part {
                                    crate::types::ChatContentPart::Text { text } => text.len(),
                                    crate::types::ChatContentPart::ImageUrl { image_url } => {
                                        image_url.url.len()
                                    }
                                })
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
                    + m.tool_calls
                        .as_ref()
                        .map(|tcs| {
                            tcs.iter()
                                .map(|tc| tc.arguments().len() + tc.name().len())
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
            })
            .sum();
        (total_chars / 4).min(u32::MAX as usize) as u32
    }

    /// Rough full-history token count estimate.
    pub fn estimate_tokens(&self) -> u32 {
        Self::estimate_messages_tokens(&self.history)
    }

    /// Get the current context token count.
    ///
    /// This follows Codex's trigger口径: last API response usage plus an
    /// estimate for items appended after the last model-generated history item.
    /// If the response snapshot was invalidated by history rewrite, fall back to
    /// a full-history estimate.
    pub fn effective_token_count(&self) -> u32 {
        let Some(last_response_tokens) = self.last_response_tokens else {
            return self.estimate_tokens();
        };
        let boundary = self
            .last_response_history_len
            .unwrap_or(self.history.len())
            .min(self.history.len());
        let appended_tokens = Self::estimate_messages_tokens(&self.history[boundary..]);
        u32::try_from(last_response_tokens)
            .unwrap_or(u32::MAX)
            .saturating_add(appended_tokens)
    }
}

// ---------------------------------------------------------------------------
// AgentSessionManager
// ---------------------------------------------------------------------------

/// Manages multiple agent sessions with concurrent access.
pub struct AgentSessionManager {
    sessions: DashMap<String, AgentSession>,
    /// Tracks session keys that are currently being processed by a turn loop.
    active_sessions: DashSet<String>,
    active_turn_statuses: DashMap<String, ActiveTurnStatusHandle>,
    active_stop_signals: DashMap<String, AgentStopSignalHandle>,
    session_ttl_secs: u64,
}

impl AgentSessionManager {
    pub fn new(session_ttl_secs: u64) -> Self {
        Self {
            sessions: DashMap::new(),
            active_sessions: DashSet::new(),
            active_turn_statuses: DashMap::new(),
            active_stop_signals: DashMap::new(),
            session_ttl_secs,
        }
    }

    fn create_active_turn_status(&self, session_key: &str) -> ActiveTurnStatusHandle {
        let handle = Arc::new(std::sync::Mutex::new(ActiveTurnStatus::new(session_key)));
        self.active_turn_statuses
            .insert(session_key.to_string(), Arc::clone(&handle));
        handle
    }

    fn attach_active_turn_status(&self, session_key: &str, session: &mut AgentSession) {
        session.active_turn_status = Some(self.create_active_turn_status(session_key));
    }

    fn create_stop_signal(&self, session_key: &str) -> AgentStopSignalHandle {
        let signal = Arc::new(AgentStopSignal::new());
        self.active_stop_signals
            .insert(session_key.to_string(), Arc::clone(&signal));
        signal
    }

    fn attach_stop_signal(&self, session_key: &str, session: &mut AgentSession) {
        session.stop_signal = Some(self.create_stop_signal(session_key));
    }

    fn attach_active_turn_handles(&self, session_key: &str, session: &mut AgentSession) {
        self.attach_active_turn_status(session_key, session);
        self.attach_stop_signal(session_key, session);
    }

    fn apply_work_dir_override(session: &mut AgentSession, work_dir: Option<String>) {
        let Some(work_dir) = work_dir else {
            return;
        };
        let work_dir = work_dir.trim();
        if work_dir.is_empty() || session.work_dir.as_deref() == Some(work_dir) {
            return;
        }
        session.reinitialize_work_dir(work_dir.to_string());
        session.memory_cleared = false;
    }

    /// Check if a session is currently being processed by a turn loop.
    pub fn is_session_active(&self, session_key: &str) -> bool {
        self.active_sessions.contains(session_key)
    }

    /// Take a session out of the manager for exclusive use during a turn.
    /// Returns a new session if one doesn't exist.
    /// Marks the session as active (busy).
    pub fn take_session(&self, session_key: &str) -> AgentSession {
        self.active_sessions.insert(session_key.to_string());
        let mut session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new(session_key));
        self.attach_active_turn_handles(session_key, &mut session);
        session
    }

    /// Try to take a session. Returns `None` if the session is currently
    /// being processed by another turn loop (busy).
    pub fn try_take_session(&self, session_key: &str) -> Option<AgentSession> {
        // DashSet::insert returns true if the value was newly inserted.
        if !self.active_sessions.insert(session_key.to_string()) {
            return None; // already active
        }
        let mut session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new(session_key));
        self.attach_active_turn_handles(session_key, &mut session);
        Some(session)
    }

    /// Take a session, creating one with a specific work_dir if it doesn't exist.
    pub fn take_session_with_work_dir(
        &self,
        session_key: &str,
        work_dir: Option<String>,
    ) -> AgentSession {
        self.active_sessions.insert(session_key.to_string());
        let mut session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new_with_work_dir(session_key, work_dir.clone()));
        Self::apply_work_dir_override(&mut session, work_dir);
        self.attach_active_turn_handles(session_key, &mut session);
        session
    }

    /// Try to take a session with a specific work_dir. Returns `None` if busy.
    pub fn try_take_session_with_work_dir(
        &self,
        session_key: &str,
        work_dir: Option<String>,
    ) -> Option<AgentSession> {
        if !self.active_sessions.insert(session_key.to_string()) {
            return None;
        }
        let mut session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new_with_work_dir(session_key, work_dir.clone()));
        Self::apply_work_dir_override(&mut session, work_dir);
        self.attach_active_turn_handles(session_key, &mut session);
        Some(session)
    }

    /// Return a session to the manager after a turn completes.
    /// Also removes the session from the active set.
    pub fn return_session(&self, session: AgentSession) {
        let mut session = session;
        let key = session.session_key.clone();
        session.active_turn_status = None;
        session.stop_signal = None;
        self.sessions.insert(key.clone(), session);
        self.active_sessions.remove(&key);
        self.active_turn_statuses.remove(&key);
        self.active_stop_signals.remove(&key);
    }

    /// Release a session from the active set without returning it.
    /// Use this when the session was lost (e.g. due to a panic) to prevent
    /// the session from being permanently marked as busy.
    pub fn release_active(&self, session_key: &str) {
        self.active_sessions.remove(session_key);
        self.active_turn_statuses.remove(session_key);
        self.active_stop_signals.remove(session_key);
    }

    /// Remove expired sessions.
    pub fn cleanup_expired(&self) {
        let ttl = self.session_ttl_secs;
        self.sessions.retain(|_, s| !s.is_expired(ttl));
    }

    /// List active session keys with metadata.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|r| {
                let s = r.value();
                SessionInfo {
                    session_key: s.session_key.clone(),
                    user_id: s.user_id.clone(),
                    message_count: s.history.len(),
                    created_at: s.created_at,
                    last_active_at: s.last_active_at,
                    compaction_count: s.compaction_count,
                    total_tokens_used: s.total_tokens_used,
                    estimated_tokens: s.effective_token_count(),
                    history_version: s.history_version,
                    work_dir: s.work_dir.clone(),
                    source: s.source.clone(),
                    title: s.title.clone(),
                }
            })
            .collect()
    }

    /// Clear a specific session.
    pub fn clear_session(&self, session_key: &str) {
        self.sessions.remove(session_key);
        self.active_turn_statuses.remove(session_key);
        self.active_stop_signals.remove(session_key);
        self.active_sessions.remove(session_key);
    }

    /// Clear all sessions.
    pub fn clear_all_sessions(&self) {
        self.sessions.clear();
        self.active_turn_statuses.clear();
        self.active_stop_signals.clear();
        self.active_sessions.clear();
    }

    /// Request cooperative cancellation of the active turn for a session.
    /// Returns true when a running turn was signalled.
    pub fn request_stop(&self, session_key: &str) -> bool {
        let Some(signal) = self.active_stop_signals.get(session_key) else {
            return false;
        };
        let requested = signal.request_stop();
        if let Some(status_handle) = self.active_turn_statuses.get(session_key) {
            if let Ok(mut status) = status_handle.lock() {
                status.state = "stopping".to_string();
            }
        }
        requested || self.active_sessions.contains(session_key)
    }

    pub fn get_active_turn_status(&self, session_key: &str) -> Option<ActiveTurnStatus> {
        self.active_turn_statuses
            .get(session_key)
            .and_then(|handle| handle.lock().ok().map(|status| status.clone()))
    }

    /// Get detailed info for a specific session (including message history).
    pub fn get_session_detail(&self, session_key: &str) -> Option<SessionDetail> {
        self.sessions.get(session_key).map(|r| {
            let s = r.value();
            let (goal_status, goal_objective) = match &s.current_goal {
                Some(g) => (Some(format!("{:?}", g.status)), Some(g.objective.clone())),
                None => (None, None),
            };
            SessionDetail {
                session_key: s.session_key.clone(),
                user_id: s.user_id.clone(),
                message_count: s.history.len(),
                created_at: s.created_at,
                last_active_at: s.last_active_at,
                compaction_count: s.compaction_count,
                total_tokens_used: s.total_tokens_used,
                estimated_tokens: s.effective_token_count(),
                history_version: s.history_version,
                work_dir: s.work_dir.clone(),
                source: s.source.clone(),
                title: s.title.clone(),
                messages: s
                    .history
                    .iter()
                    .map(|m| SessionMessage {
                        role: m.role.clone(),
                        content: m.content.clone().unwrap_or_default(),
                        content_parts: m
                            .content_parts
                            .as_ref()
                            .and_then(|parts| serde_json::to_value(parts).ok()),
                        tool_calls: m.tool_calls.as_ref().map(|tc| {
                            tc.iter()
                                .map(|t| format!("{}({})", t.name(), t.arguments()))
                                .collect()
                        }),
                    })
                    .collect(),
                goal_status,
                goal_objective,
            }
        })
    }
}

/// Session metadata for listing/monitoring.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub session_key: String,
    pub user_id: Option<String>,
    pub message_count: usize,
    pub created_at: u64,
    pub last_active_at: u64,
    pub compaction_count: u32,
    pub total_tokens_used: Option<u64>,
    pub estimated_tokens: u32,
    pub history_version: u64,
    pub work_dir: Option<String>,
    pub source: String,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
}

/// A single message in session detail view.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
}

/// Detailed session info including message history.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionDetail {
    pub session_key: String,
    pub user_id: Option<String>,
    pub message_count: usize,
    pub created_at: u64,
    pub last_active_at: u64,
    pub compaction_count: u32,
    pub total_tokens_used: Option<u64>,
    pub estimated_tokens: u32,
    pub history_version: u64,
    pub work_dir: Option<String>,
    pub source: String,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
    pub messages: Vec<SessionMessage>,
    /// Current goal status (if any).
    pub goal_status: Option<String>,
    /// Current goal objective (if any).
    pub goal_objective: Option<String>,
}

// ---------------------------------------------------------------------------
// Session-free command handling (no session lock required)
// ---------------------------------------------------------------------------

/// Handle built-in commands that don't require session state.
///
/// These commands can respond immediately even while another turn loop is
/// processing the same session. Returns `Some(response)` if the command was
/// handled, `None` if the message is not a session-free command.
pub fn handle_session_free_command(
    session_key: &str,
    user_message: &str,
    config: &AgentConfig,
) -> Option<String> {
    let dispatch = SlashCommandRouter::dispatch_builtin_only(user_message);
    match dispatch {
        Dispatch::Builtin {
            command: BuiltinCommand::Help,
            ..
        } => {
            // Return builtin-only help text (skill commands not available without session).
            let router = SlashCommandRouter::with_default_builtins();
            Some(router.help_text())
        }
        Dispatch::Builtin {
            command: BuiltinCommand::Remember,
            ref args,
        } => {
            if args.trim().is_empty() {
                return Some("用法: /remember <text>".to_string());
            }
            // Build a minimal session for remember_explicit (only needs session_key)
            let stub = AgentSession::new(session_key);
            match memory::remember_explicit(config, &stub, args.trim()) {
                Ok(record) => Some(format!("已记住长期记忆: {}", record.id)),
                Err(e) => Some(format!("保存记忆失败: {e}")),
            }
        }
        Dispatch::Builtin {
            command: BuiltinCommand::Memories,
            ..
        } => {
            let stub = AgentSession::new(session_key);
            match memory::list_visible_memories(config, &stub, 20) {
                Ok(records) if records.is_empty() => Some("当前 scope 没有长期记忆。".to_string()),
                Ok(records) => {
                    let mut lines = vec!["当前可见长期记忆文件条目:".to_string()];
                    for record in records {
                        lines.push(format!(
                            "- {} [{}] {}",
                            record.id, record.path, record.content
                        ));
                    }
                    Some(lines.join("\n"))
                }
                Err(e) => Some(format!("查询记忆失败: {e}")),
            }
        }
        Dispatch::Builtin {
            command: BuiltinCommand::Forget,
            ref args,
        } => {
            if args.trim().is_empty() {
                return Some("用法: /forget <id|last>".to_string());
            }
            let stub = AgentSession::new(session_key);
            match memory::forget_memory(config, &stub, args.trim()) {
                Ok(Some(id)) => Some(format!("已忘记长期记忆: {id}")),
                Ok(None) => Some("没有找到可忘记的长期记忆。".to_string()),
                Err(e) => Some(format!("删除记忆失败: {e}")),
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Turn Loop
// ---------------------------------------------------------------------------

const STOPPED_RESPONSE: &str = "已收到 /stop，正在执行的 Agent loop 已停止。";
const STOPPED_ERROR: &str = "__bifrost_agent_stopped_by_user__";

fn stop_requested(session: &AgentSession) -> bool {
    session
        .stop_signal
        .as_ref()
        .is_some_and(|signal| signal.is_requested())
}

async fn chat_completion_or_stop(
    client: &AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    messages: &[ChatMessage],
    tools: &[crate::types::ToolDefinition],
) -> Result<Option<crate::types::ModelResponse>, String> {
    let request = client.chat_completion(config, messages, tools);
    let Some(signal) = session.stop_signal.clone() else {
        return request.await.map(Some);
    };
    tokio::select! {
        result = request => result.map(Some),
        _ = signal.cancelled() => Ok(None),
    }
}

fn stopped_turn_result(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_calls_log: Vec<ToolCallLog>,
) -> TurnResult {
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
            reason: crate::tools::goal::GoalAbortReason::Interrupted,
        },
    );
    session.add_assistant_message(STOPPED_RESPONSE);
    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_assistant_message(&session.session_key, STOPPED_RESPONSE) {
            warn!(error = %e, "failed to record stop assistant message");
        }
    }
    TurnResult {
        response: STOPPED_RESPONSE.to_string(),
        tool_calls_log,
        work_dir_switched: None,
        title_updated: session.title.clone(),
        plan_steps: session.current_plan.clone(),
        goal_needs_continuation: false,
        goal_objective: None,
    }
}

fn append_cancelled_tool_results(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_calls_log: &mut Vec<ToolCallLog>,
    tool_calls: &[ToolCallMessage],
    start_index: usize,
) {
    const CANCELLED_TOOL_OUTPUT: &str = "Tool call cancelled because /stop was requested.";
    for tc in tool_calls.iter().skip(start_index) {
        session.add_tool_result(&tc.id, CANCELLED_TOOL_OUTPUT);
        if let Some(rec) = recorder.as_deref_mut() {
            if let Err(e) = rec.record_tool_result_with_call_id(
                &session.session_key,
                tc.name(),
                CANCELLED_TOOL_OUTPUT,
                false,
                Some(&tc.id),
            ) {
                warn!(error = %e, "failed to record cancelled tool result");
            }
        }
        tool_calls_log.push(ToolCallLog {
            tool_name: tc.name().to_string(),
            arguments: tc.arguments().to_string(),
            result: CANCELLED_TOOL_OUTPUT.to_string(),
            success: false,
        });
    }
}

/// Run a single turn of the agent conversation (backward-compatible wrapper).
///
/// This implements the core turn loop (same pattern as Codex's session/turn.rs):
/// 1. Handle built-in commands (/clear, /reset)
/// 2. Build prompt prefix and run Codex-style pre-sampling compaction when needed
/// 3. Add the user message and send to model with available tools
/// 4. If model returns tool_calls → execute → mid-turn compaction check → loop
/// 5. If model returns text → return as final response
pub async fn run_turn(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    user_message: &str,
    system_prompt_override: Option<&str>,
) -> Result<TurnResult, String> {
    run_turn_with_mcp(
        client,
        config,
        session,
        tools,
        None,
        user_message,
        system_prompt_override,
        None,
    )
    .await
}

/// Run a single turn with optional MCP tool support.
///
/// Same as `run_turn` but accepts an optional `McpManager` for routing tool calls
/// to MCP servers when the model invokes MCP-provided tools.
#[allow(clippy::too_many_arguments)]
pub async fn run_turn_with_mcp(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    mcp: Option<&mut McpManager>,
    user_message: &str,
    system_prompt_override: Option<&str>,
    recorder: Option<&mut ConversationRecorder>,
) -> Result<TurnResult, String> {
    run_turn_with_mcp_multimodal(
        client,
        config,
        session,
        tools,
        mcp,
        user_message,
        &[],
        system_prompt_override,
        recorder,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_turn_with_mcp_multimodal(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    mut mcp: Option<&mut McpManager>,
    user_message: &str,
    images: &[ChatImageInput],
    system_prompt_override: Option<&str>,
    mut recorder: Option<&mut ConversationRecorder>,
) -> Result<TurnResult, String> {
    if !config.enabled {
        return Err("agent is disabled".to_string());
    }

    // Handle built-in commands
    let trimmed = user_message.trim();
    let slash_dispatch = session.slash_router.dispatch(trimmed);
    match &slash_dispatch {
        Dispatch::Unknown(command) => {
            // /q and /rq are IM-gateway queue commands, only valid when a session
            // is already busy. If the session is free, give a helpful explanation.
            let response = if command == "/q" || command.starts_with("/rq") {
                format!(
                    "{command} 是排队命令，仅在 Agent 正在处理任务时有效。\n\
                     当 Agent 空闲时，直接发送消息即可开始对话。"
                )
            } else {
                format!("未知命令: {command}")
            };
            return Ok(TurnResult {
                response,
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        Dispatch::RunSkill { record, invocation } => {
            let report = bifrost_skills::SkillExecutor::default()
                .execute(record.as_ref(), invocation.clone())
                .await?;
            return Ok(TurnResult {
                response: if report.stdout.trim().is_empty() {
                    "Skill 执行完成。".to_string()
                } else {
                    report.stdout.trim().to_string()
                },
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        Dispatch::Builtin { .. } | Dispatch::NotACommand => {}
    }

    // /help — show available commands
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Help,
            ..
        }
    ) {
        return Ok(TurnResult {
            response: session.slash_router.help_text(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /stop reaches this path only when no other loop is active for the session.
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Stop,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        return Ok(TurnResult {
            response: "当前没有正在执行的 Agent loop。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Clear | BuiltinCommand::Reset,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        session.clear();
        return Ok(TurnResult {
            response: "会话已重置，可以开始新的对话。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /undo [N] — rollback last N user turns (default 1)
    if let Dispatch::Builtin {
        command: BuiltinCommand::Undo,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let n: u32 = args.parse().unwrap_or(1);
        let removed = session.rollback(n);
        return Ok(TurnResult {
            response: format!(
                "已回退 {n} 轮对话（移除了 {removed} 条消息）。当前历史: {} 条消息。",
                session.history.len()
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /compact — manual compaction (same as Codex's CompactionTrigger::Manual)
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Compact,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        if session.history.is_empty() {
            return Ok(TurnResult {
                response: "历史消息太少，无需压缩。".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        let base_instructions = prompt::resolve_base_instructions_text(config, None);
        match compact::compact_session(
            client,
            config,
            session,
            compact::CompactionTrigger::Manual,
            compact::CompactionReason::UserRequested,
            compact::CompactionPhase::StandaloneTurn,
            Some(&base_instructions),
            compact::InitialContextInjection::DoNotInject,
            vec![],
        )
        .await
        {
            Ok(result) if result.performed => {
                record_compaction_event(
                    &mut recorder,
                    session,
                    &result,
                    "manual",
                    "user_requested",
                    "standalone_turn",
                    false,
                );
                return Ok(TurnResult {
                    response: format!(
                        "记忆压缩完成。\n- 压缩前 token: ~{}\n- 压缩后 token: ~{}\n- 节省: ~{}\n- 移除消息: {}\n- 累计压缩次数: {}\n- 耗时: {}ms",
                        result.pre_tokens, result.post_tokens, result.tokens_saved,
                        result.messages_removed, session.compaction_count,
                        result.duration_ms.unwrap_or(0)
                    ),
                    tool_calls_log: Vec::new(),
                    work_dir_switched: None,
                    title_updated: None,
                    plan_steps: None,
                    goal_needs_continuation: false,
                    goal_objective: None,
                });
            }
            Ok(result) => {
                return Ok(TurnResult {
                    response: format!(
                        "压缩已跳过: {}",
                        result.reason.unwrap_or_else(|| "unknown".to_string())
                    ),
                    tool_calls_log: Vec::new(),
                    work_dir_switched: None,
                    title_updated: None,
                    plan_steps: None,
                    goal_needs_continuation: false,
                    goal_objective: None,
                });
            }
            Err(e) => {
                return Err(format!("压缩失败: {e}"));
            }
        }
    }

    // /remember <text> — explicit long-term memory write.
    if let Dispatch::Builtin {
        command: BuiltinCommand::Remember,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        if args.trim().is_empty() {
            return Ok(TurnResult {
                response: "用法: /remember <text>".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        let record = memory::remember_explicit(config, session, args.trim())?;
        return Ok(TurnResult {
            response: format!("已记住长期记忆: {}", record.id),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /memories — list visible long-term memories.
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Memories,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let records = memory::list_visible_memories(config, session, 20)?;
        let response = if records.is_empty() {
            "当前 scope 没有长期记忆。".to_string()
        } else {
            let mut lines = vec!["当前可见长期记忆文件条目:".to_string()];
            for record in records {
                lines.push(format!(
                    "- {} [{}] {}",
                    record.id, record.path, record.content
                ));
            }
            lines.join("\n")
        };
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /forget <id|last> — soft delete one visible memory.
    if let Dispatch::Builtin {
        command: BuiltinCommand::Forget,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        if args.trim().is_empty() {
            return Ok(TurnResult {
                response: "用法: /forget <id|last>".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        let response = match memory::forget_memory(config, session, args.trim())? {
            Some(id) => format!("已忘记长期记忆: {id}"),
            None => "没有找到可忘记的长期记忆。".to_string(),
        };
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if let Dispatch::Builtin {
        command: BuiltinCommand::Goal,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let response = handle_goal_command(session, args).await;
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /status — show session state (token usage, compaction count, etc.)
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Status,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let est = session.estimate_tokens();
        let context_tokens = session.effective_token_count();
        let real = session
            .total_tokens_used
            .map(|t| t.to_string())
            .unwrap_or_else(|| "N/A".to_string());
        let mcp_tool_count = mcp.as_ref().map(|m| m.list_tools().len()).unwrap_or(0);
        let context_window = config_context_window_tokens(config);
        let context_usage = context_usage_percent(context_tokens, context_window)
            .map(|percent| format!("{percent:.1}%"))
            .unwrap_or_else(|| "N/A".to_string());
        let context_window_text = context_window
            .map(|window| window.to_string())
            .unwrap_or_else(|| "N/A".to_string());
        let work_dir_text = session.work_dir.as_deref().unwrap_or("N/A");
        return Ok(TurnResult {
            response: format!(
                "会话状态:\n- 工作路径: {}\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- Context 用量: ~{} / {} ({})\n- 压缩次数: {}\n- 历史版本: {}\n- MCP 工具数: {}",
                work_dir_text,
                session.history.len(),
                est,
                real,
                context_tokens,
                context_window_text,
                context_usage,
                session.compaction_count,
                session.history_version,
                mcp_tool_count
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /resume — load the most recent JSONL conversation and restore session history
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Resume,
            ..
        }
    ) {
        let agent_home = crate::config::agent_home_dir();
        let mut files = persistence::list_conversations(&agent_home, Some(&session.session_key));
        if files.is_empty() {
            let legacy_dir = config.resolve_work_dir();
            let legacy_files =
                persistence::list_conversations(&legacy_dir, Some(&session.session_key));
            if !legacy_files.is_empty() {
                warn!(
                    agent_home = %agent_home.display(),
                    fallback_dir = %legacy_dir.display(),
                    "resume fell back to legacy work_dir session path"
                );
                files = legacy_files;
            }
        }
        // Exclude the current recorder's file (it was just created for this request
        // and contains no conversation data yet).
        if let Some(ref rec) = recorder {
            let current_file = rec.file_path().to_path_buf();
            files.retain(|f| f != &current_file);
        }
        // Try files from newest to oldest, skipping any that load 0 messages.
        while let Some(candidate) = files.pop() {
            match persistence::load_conversation(&candidate) {
                Ok(messages) if !messages.is_empty() => {
                    let count = messages.len();
                    session.history = messages;
                    if let Ok(runtime_state) = persistence::load_session_runtime_state(&candidate) {
                        session.current_goal = runtime_state.current_goal;
                        session.total_tokens_used = runtime_state.total_tokens_used;
                        session.resolved_base_instructions = runtime_state.base_instructions;
                        crate::tools::goal::goal_runtime_apply(
                            session,
                            crate::tools::goal::GoalRuntimeEvent::ThreadResumed,
                        );
                    }
                    session.history_version = session.history_version.saturating_add(1);
                    return Ok(TurnResult {
                        response: format!(
                            "已恢复会话历史，加载了 {} 条消息（来源: {}）。",
                            count,
                            candidate.display()
                        ),
                        tool_calls_log: Vec::new(),
                        work_dir_switched: None,
                        title_updated: None,
                        plan_steps: None,
                        goal_needs_continuation: false,
                        goal_objective: None,
                    });
                }
                Ok(_) => {
                    // 0 messages — skip this file (e.g. session_start only)
                    debug!(
                        file = %candidate.display(),
                        "skipping session file with 0 messages during resume"
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        file = %candidate.display(),
                        error = %e,
                        "failed to load session file during resume, trying next"
                    );
                    continue;
                }
            }
        }
        return Ok(TurnResult {
            response: "没有找到可恢复的会话记录。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if let Dispatch::Builtin {
        command: BuiltinCommand::Skill,
        ref args,
    } = slash_dispatch
    {
        // Route through the authoring hub so `/skill start|draft|commit|cancel`
        // drives `SkillAuthoringSession` → `SkillStore::commit`, getting
        // validation, manifest.json, checksum, and `.history` archival for free.
        let store: Option<Arc<SkillStore>> = session
            .skill_registry
            .as_ref()
            .map(|registry| registry.store());
        if let Some(store) = store {
            let outcome = session.skill_authoring.dispatch(
                &session.session_key,
                store,
                session.skill_registry.as_deref(),
                args,
            );
            // Refresh the registry so a freshly committed skill becomes
            // discoverable via slash commands in the same session.
            if outcome.committed.is_some() {
                if let Some(registry) = session.skill_registry.as_ref() {
                    if let Err(e) = registry.reload_all() {
                        warn!(error = %e, "failed to reload skill registry after commit");
                    }
                }
            }
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                    warn!(error = %e, "failed to record user message");
                }
            }
            return Ok(TurnResult {
                response: outcome.response,
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        return Ok(TurnResult {
            response: "Skill registry 尚未初始化，无法创建 skill。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // A normal new user turn resumes an interrupted goal, matching Codex's
    // thread-resume behavior more closely than keeping the goal paused until
    // an explicit /resume slash command.
    if !trimmed.starts_with('/') {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::MaybeContinueIfIdle,
        );
    }

    // Signal turn started for goal accounting baseline snapshot.
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TurnStarted,
    );

    // Lazily load user instructions on first turn (matching Codex's once-per-session
    // loading). Subsequent turns reuse the cached value.
    if session.user_instructions.is_none() {
        session.load_user_instructions(config);
    }

    let base_instructions = if let Some(route_base_prompt) = system_prompt_override {
        route_base_prompt.to_string()
    } else if let Some(existing) = session.resolved_base_instructions.clone() {
        existing
    } else {
        let resolved = prompt::resolve_base_instructions_text(config, None);
        session.resolved_base_instructions = Some(resolved.clone());
        resolved
    };

    // Build Codex-style prompt prefix:
    // system(base instructions) + developer sections + contextual user sections.
    // Route-level `system_prompt_override` is treated as a base prompt override,
    // not as a full prompt replacement, so tools/skills/AGENTS/env still apply.
    let prompt_messages = prompt::build_prompt_messages_with_skill_registry(
        config,
        &base_instructions,
        session.work_dir.as_deref(),
        session.skill_registry.as_deref(),
        session.user_instructions.as_deref(),
    );
    let prompt_prefix = prompt_messages.prefix;

    compact_pre_sampling_if_needed(client, config, session, &mut recorder, &base_instructions)
        .await?;

    // Add user message to history
    session.add_user_message_with_images(user_message, images);

    // Record user message
    if let Some(ref mut rec) = recorder {
        if let Err(e) =
            rec.record_user_message_with_images(&session.session_key, user_message, images)
        {
            warn!(error = %e, "failed to record user message");
        }
    }

    // Merge tool definitions: local tools + direct MCP tools. Codex-style
    // deferred MCP tools are available through turn-scoped `tool_search`.
    let mut tool_defs = tools.definitions();
    let local_tool_count = tool_defs.len();
    let mut visible_tool_names = tool_defs
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<HashSet<_>>();
    let mut tool_search_entries = Vec::new();
    let mcp_tool_count = mcp.as_ref().map(|m| m.list_tools().len()).unwrap_or(0);
    if let Some(ref mcp_mgr) = mcp {
        let exposure = mcp_mgr.tool_exposure();
        for definition in exposure.direct_tools {
            visible_tool_names.insert(definition.name().to_string());
            tool_defs.push(definition);
        }
        tool_search_entries.extend(exposure.deferred_tools.into_iter().map(|definition| {
            let source = definition
                .name()
                .split_once("__")
                .map(|(prefix, _)| format!("MCP tools: {prefix}"))
                .unwrap_or_else(|| "MCP tools".to_string());
            ToolSearchEntry::new(definition, source)
        }));
    }
    let tool_search_tool = if tool_search_entries.is_empty() {
        None
    } else {
        let definition = tool_search_definition(&tool_search_entries);
        visible_tool_names.insert(definition.name().to_string());
        tool_defs.push(definition);
        Some(ToolSearchTool::new(tool_search_entries))
    };
    info!(
        local_tools = local_tool_count,
        mcp_tools = mcp_tool_count,
        visible_tools = tool_defs.len(),
        tool_search_visible = tool_search_tool.is_some(),
        "assembled agent tool definitions"
    );

    let work_dir = session
        .work_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.resolve_work_dir());
    // Ensure work_dir exists
    let _ = std::fs::create_dir_all(&work_dir);
    let mut tool_calls_log: Vec<crate::types::ToolCallLog> = Vec::new();
    let max_iterations = config.get_max_turn_iterations() as usize;
    let tool_output_limit = config
        .tool_output_token_limit
        .unwrap_or(AgentConfig::DEFAULT_TOOL_OUTPUT_TOKEN_LIMIT);

    // Reset plan from previous turn so stale steps are not returned if the
    // model does not call update_plan in this turn.
    session.current_plan = None;
    refresh_active_turn_status(
        session,
        config,
        ActiveTurnProgress {
            state: "starting",
            current_loop_iteration: 0,
            completed_loop_iterations: 0,
            max_loop_iterations: config.get_max_turn_iterations(),
            local_tool_count,
            mcp_tool_count,
        },
    );

    info!(
        session_key = %session.session_key,
        message_count = session.history.len(),
        tool_count = tool_defs.len(),
        compaction_count = session.compaction_count,
        "starting agent turn"
    );

    // Skip memory injection if session was just cleared (prevents model from
    // "remembering" prior conversation context immediately after /clear).
    let memory_message = if session.memory_cleared {
        info!(
            session_key = %session.session_key,
            "skipping memory injection after /clear"
        );
        session.memory_cleared = false; // reset for future turns
        None
    } else {
        memory::recall_system_message(config, session, user_message)
    };

    for iteration in 0..max_iterations {
        // Build messages with history limit enforcement
        let messages = session.build_messages(
            &prompt_prefix,
            memory_message.as_ref(),
            config.get_max_history_messages(),
        );
        let model_request_progress = ActiveTurnProgress {
            state: "model_request",
            current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
            completed_loop_iterations: u32::try_from(iteration).unwrap_or(u32::MAX),
            max_loop_iterations: config.get_max_turn_iterations(),
            local_tool_count,
            mcp_tool_count,
        };
        refresh_active_turn_status(session, config, model_request_progress);

        debug!(
            iteration,
            message_count = messages.len(),
            history_len = session.history.len(),
            "sending model request"
        );

        let response =
            chat_completion_or_stop(client, config, session, &messages, &tool_defs).await;

        // Retry with exponential backoff on transient errors;
        // Codex-style degradation on context window overflow:
        //   1. Emergency compact (best effort — preserves most context)
        //   2. Loop trim oldest messages until within budget
        //   3. Give up only when history is exhausted
        let response = match response {
            Ok(Some(r)) => r,
            Ok(None) => {
                info!(
                    session_key = %session.session_key,
                    iteration = iteration + 1,
                    "agent turn stopped during model request"
                );
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            }
            Err(e) => {
                if is_context_window_error(&e) {
                    warn!(
                        error = %e,
                        history_len = session.history.len(),
                        estimated_tokens = session.estimate_tokens(),
                        "context window exceeded, starting degradation chain"
                    );

                    // Step 1: Try emergency compact first (best quality reduction)
                    let compacted = match compact::compact_session(
                        client,
                        config,
                        session,
                        compact::CompactionTrigger::Auto,
                        compact::CompactionReason::ContextLimit,
                        compact::CompactionPhase::MidTurn,
                        Some(&base_instructions),
                        compact::InitialContextInjection::BeforeLastUserMessage,
                        build_mid_turn_initial_context(
                            session,
                            &prompt_prefix,
                            memory_message.as_ref(),
                        ),
                    )
                    .await
                    {
                        Ok(result) if result.performed => {
                            record_compaction_event(
                                &mut recorder,
                                session,
                                &result,
                                "auto",
                                "context_limit",
                                "mid_turn",
                                true,
                            );
                            info!(
                                tokens_saved = result.tokens_saved,
                                duration_ms = ?result.duration_ms,
                                "emergency compact succeeded"
                            );
                            refresh_active_turn_status(session, config, model_request_progress);
                            true
                        }
                        Ok(_) => {
                            info!("emergency compact skipped");
                            false
                        }
                        Err(compact_err) => {
                            warn!(error = %compact_err, "emergency compact failed, falling back to trim");
                            false
                        }
                    };

                    // Step 2: If compact worked, retry once
                    if compacted {
                        let retry_messages = session.build_messages(
                            &prompt_prefix,
                            memory_message.as_ref(),
                            config.get_max_history_messages(),
                        );
                        match chat_completion_or_stop(
                            client,
                            config,
                            session,
                            &retry_messages,
                            &tool_defs,
                        )
                        .await
                        {
                            Ok(Some(r)) => r,
                            Ok(None) => {
                                info!(
                                    session_key = %session.session_key,
                                    "agent turn stopped during post-compact retry"
                                );
                                return Ok(stopped_turn_result(
                                    session,
                                    &mut recorder,
                                    tool_calls_log,
                                ));
                            }
                            Err(e2) if is_context_window_error(&e2) => {
                                // Compact wasn't enough, fall through to trim loop
                                warn!("still over context limit after compact, starting trim loop");
                                match trim_loop_retry(
                                    client,
                                    config,
                                    session,
                                    &prompt_prefix,
                                    memory_message.as_ref(),
                                    &tool_defs,
                                    model_request_progress,
                                )
                                .await
                                {
                                    Ok(r) => r,
                                    Err(e3) if e3 == STOPPED_ERROR => {
                                        return Ok(stopped_turn_result(
                                            session,
                                            &mut recorder,
                                            tool_calls_log,
                                        ));
                                    }
                                    Err(e3) => {
                                        crate::tools::goal::goal_runtime_apply(
                                            session,
                                            crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                                reason:
                                                    crate::tools::goal::GoalAbortReason::Interrupted,
                                            },
                                        );
                                        return Err(e3);
                                    }
                                }
                            }
                            Err(e2) => {
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Err(e2);
                            }
                        }
                    } else {
                        // Compact didn't run or failed, go straight to trim loop
                        match trim_loop_retry(
                            client,
                            config,
                            session,
                            &prompt_prefix,
                            memory_message.as_ref(),
                            &tool_defs,
                            model_request_progress,
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(e3) if e3 == STOPPED_ERROR => {
                                return Ok(stopped_turn_result(
                                    session,
                                    &mut recorder,
                                    tool_calls_log,
                                ));
                            }
                            Err(e3) => {
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Err(e3);
                            }
                        }
                    }
                } else if is_retryable_error(&e) {
                    // Transient error — exponential backoff retry (up to 5 attempts)
                    const MAX_RETRIES: usize = 5;
                    let mut last_err = e;
                    let mut succeeded = None;
                    for retry in 0..MAX_RETRIES {
                        let delay_ms = 1000 * (1u64 << retry.min(3));
                        warn!(
                            error = %last_err,
                            retry_attempt = retry + 1,
                            max_retries = MAX_RETRIES,
                            retry_in_ms = delay_ms,
                            "transient API error, retrying"
                        );
                        if let Some(signal) = session.stop_signal.clone() {
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                _ = signal.cancelled() => {
                                    info!(
                                        session_key = %session.session_key,
                                        "agent turn stopped during retry backoff"
                                    );
                                    return Ok(stopped_turn_result(
                                        session,
                                        &mut recorder,
                                        tool_calls_log,
                                    ));
                                }
                            }
                        } else {
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        }
                        match chat_completion_or_stop(
                            client, config, session, &messages, &tool_defs,
                        )
                        .await
                        {
                            Ok(Some(r)) => {
                                info!(retry_attempt = retry + 1, "retry succeeded");
                                succeeded = Some(r);
                                break;
                            }
                            Ok(None) => {
                                info!(
                                    session_key = %session.session_key,
                                    "agent turn stopped during retry model request"
                                );
                                return Ok(stopped_turn_result(
                                    session,
                                    &mut recorder,
                                    tool_calls_log,
                                ));
                            }
                            Err(e2) => {
                                last_err = e2;
                            }
                        }
                    }
                    match succeeded {
                        Some(r) => r,
                        None => {
                            // All retries exhausted — graceful degradation
                            if !tool_calls_log.is_empty() {
                                // We have partial results from tool calls; return them
                                warn!(
                                    error = %last_err,
                                    tool_calls_done = tool_calls_log.len(),
                                    "all retries failed, returning partial results"
                                );
                                let mut partial_response = String::from(
                                    "⚠️ **模型调用失败，以下是已执行的工具结果：**\n\n",
                                );
                                for log in &tool_calls_log {
                                    let icon = if log.success { "✅" } else { "❌" };
                                    partial_response
                                        .push_str(&format!("{} `{}`\n", icon, log.tool_name));
                                }
                                partial_response.push_str(&format!(
                                    "\n---\n**错误原因**: {}\n\n请重新发送消息或稍后重试。",
                                    last_err
                                ));
                                session.add_assistant_message(&partial_response);
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Ok(TurnResult {
                                    response: partial_response,
                                    tool_calls_log,
                                    work_dir_switched: None,
                                    title_updated: None,
                                    plan_steps: None,
                                    goal_needs_continuation: false,
                                    goal_objective: None,
                                });
                            }
                            crate::tools::goal::goal_runtime_apply(
                                session,
                                crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                    reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                },
                            );
                            return Err(last_err);
                        }
                    }
                } else {
                    // Non-retryable, non-context-window error
                    // If we have partial tool results, return them gracefully
                    if !tool_calls_log.is_empty() {
                        warn!(
                            error = %e,
                            tool_calls_done = tool_calls_log.len(),
                            "non-retryable error, returning partial results"
                        );
                        let mut partial_response =
                            String::from("⚠️ **模型调用失败，以下是已执行的工具结果：**\n\n");
                        for log in &tool_calls_log {
                            let icon = if log.success { "✅" } else { "❌" };
                            partial_response.push_str(&format!("{} `{}`\n", icon, log.tool_name));
                        }
                        partial_response.push_str(&format!(
                            "\n---\n**错误原因**: {}\n\n请重新发送消息或稍后重试。",
                            e
                        ));
                        session.add_assistant_message(&partial_response);
                        crate::tools::goal::goal_runtime_apply(
                            session,
                            crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                reason: crate::tools::goal::GoalAbortReason::Interrupted,
                            },
                        );
                        return Ok(TurnResult {
                            response: partial_response,
                            tool_calls_log,
                            work_dir_switched: None,
                            title_updated: None,
                            plan_steps: None,
                            goal_needs_continuation: false,
                            goal_objective: None,
                        });
                    }
                    crate::tools::goal::goal_runtime_apply(
                        session,
                        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                            reason: crate::tools::goal::GoalAbortReason::Interrupted,
                        },
                    );
                    return Err(e);
                }
            }
        };

        if stop_requested(session) {
            info!(
                session_key = %session.session_key,
                iteration = iteration + 1,
                "agent turn stopped after model response"
            );
            return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
        }

        // Track real token usage from API response
        if let Some(ref usage) = response.usage {
            session.track_token_usage(usage.total_tokens);
        }
        refresh_active_turn_status(
            session,
            config,
            ActiveTurnProgress {
                state: "model_response",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
        );

        // Check if model wants to call tools
        if response.tool_calls.is_empty() {
            if session
                .current_plan
                .as_ref()
                .is_some_and(|plan| plan_has_unfinished_steps(plan))
                && session.plan_repair_attempts < 2
            {
                session.plan_repair_attempts = session.plan_repair_attempts.saturating_add(1);
                session.history.push(ChatMessage::system(
                    "You already have an active task plan. Before concluding, call update_plan to reflect the final task state. If the work is complete, mark all steps as completed.",
                ));
                session.touch();
                continue;
            }

            // Guide-mode final check: if a guide message arrived while the model
            // was generating its final response (no tool calls), the mid-turn
            // checkpoint (after tool execution) never consumed it. Check here
            // before ending the turn so the message is not silently lost.
            let guide_at_end = session
                .guide_channel
                .as_ref()
                .and_then(|ch| ch.lock().unwrap().take())
                .filter(|msg| !msg.trim().is_empty());
            if let Some(guide_msg) = guide_at_end {
                info!(
                    session_key = %session.session_key,
                    guide_msg_len = guide_msg.len(),
                    "guide message found at turn end, continuing iteration"
                );
                // Keep the model's final text as assistant message before injecting guide
                let content = response
                    .content
                    .or(response.reasoning_content)
                    .unwrap_or_default();
                if !content.is_empty() {
                    session.add_assistant_message(&content);
                    if let Some(ref mut rec) = recorder {
                        let response_tokens =
                            response.usage.as_ref().map(|usage| usage.total_tokens);
                        if let Err(e) = rec.record_assistant_message_with_tokens(
                            &session.session_key,
                            &content,
                            response_tokens,
                        ) {
                            warn!(error = %e, "failed to record assistant message (guide inject)");
                        }
                    }
                }
                session.add_user_message(&guide_msg);
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_user_message(&session.session_key, &guide_msg) {
                        warn!(error = %e, "failed to record guide message at turn end");
                    }
                }
                compact_mid_turn_if_needed(
                    client,
                    config,
                    session,
                    &mut recorder,
                    &prompt_prefix,
                    memory_message.as_ref(),
                    ActiveTurnProgress {
                        state: "model_response",
                        current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        max_loop_iterations: config.get_max_turn_iterations(),
                        local_tool_count,
                        mcp_tool_count,
                    },
                )
                .await;
                continue;
            }

            // Pending queue check: if messages are queued (e.g. from IM interleave),
            // pop the next one and continue the turn loop instead of returning.
            if let Some(queued_msg) = session.pending_messages.pop_front() {
                if !queued_msg.trim().is_empty() {
                    info!(
                        session_key = %session.session_key,
                        queued_msg_len = queued_msg.len(),
                        remaining_queue = session.pending_messages.len(),
                        "processing queued message from pending_messages"
                    );
                    // Keep the model's final text as assistant message
                    let content = response
                        .content
                        .or(response.reasoning_content)
                        .unwrap_or_default();
                    if !content.is_empty() {
                        session.add_assistant_message(&content);
                        if let Some(ref mut rec) = recorder {
                            let response_tokens =
                                response.usage.as_ref().map(|usage| usage.total_tokens);
                            if let Err(e) = rec.record_assistant_message_with_tokens(
                                &session.session_key,
                                &content,
                                response_tokens,
                            ) {
                                warn!(error = %e, "failed to record assistant message (queue)");
                            }
                        }
                    }
                    session.add_user_message(&queued_msg);
                    if let Some(ref mut rec) = recorder {
                        if let Err(e) = rec.record_user_message(&session.session_key, &queued_msg) {
                            warn!(error = %e, "failed to record queued message");
                        }
                    }
                    compact_mid_turn_if_needed(
                        client,
                        config,
                        session,
                        &mut recorder,
                        &prompt_prefix,
                        memory_message.as_ref(),
                        ActiveTurnProgress {
                            state: "model_response",
                            current_loop_iteration: u32::try_from(iteration + 1)
                                .unwrap_or(u32::MAX),
                            completed_loop_iterations: u32::try_from(iteration + 1)
                                .unwrap_or(u32::MAX),
                            max_loop_iterations: config.get_max_turn_iterations(),
                            local_tool_count,
                            mcp_tool_count,
                        },
                    )
                    .await;
                    continue;
                }
            }

            // Model finished — extract text response
            let content = response
                .content
                .or(response.reasoning_content)
                .unwrap_or_default();
            crate::tools::goal::goal_runtime_apply(
                session,
                crate::tools::goal::GoalRuntimeEvent::TurnFinished {
                    turn_completed: true,
                },
            );

            session.add_assistant_message(&content);

            // Record assistant message
            if let Some(ref mut rec) = recorder {
                let response_tokens = response.usage.as_ref().map(|usage| usage.total_tokens);
                if let Err(e) = rec.record_assistant_message_with_tokens(
                    &session.session_key,
                    &content,
                    response_tokens,
                ) {
                    warn!(error = %e, "failed to record assistant message");
                }
            }

            info!(
                session_key = %session.session_key,
                iterations = iteration + 1,
                tool_calls = tool_calls_log.len(),
                total_tokens_used = ?session.total_tokens_used,
                "agent turn completed"
            );

            memory::auto_extract_after_turn(
                std::sync::Arc::new(client.clone()),
                config.clone(),
                session.session_key.clone(),
                user_message.to_string(),
                content.clone(),
            );

            return Ok(TurnResult {
                response: content,
                tool_calls_log,
                work_dir_switched: None,
                title_updated: session.title.clone(),
                plan_steps: session.current_plan.clone(),
                goal_needs_continuation: session
                    .current_goal
                    .as_ref()
                    .is_some_and(|g| g.status == crate::tools::goal::GoalStatus::Active),
                goal_objective: session
                    .current_goal
                    .as_ref()
                    .filter(|g| g.status == crate::tools::goal::GoalStatus::Active)
                    .map(|g| g.objective.clone()),
            });
        }

        // Model wants to call tools
        info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "model requested tool calls"
        );
        refresh_active_turn_status(
            session,
            config,
            ActiveTurnProgress {
                state: "tool_calls",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
        );

        // Record the assistant message with tool calls
        session.add_assistant_tool_calls(&response.tool_calls);

        // Execute each tool call
        for (tool_index, tc) in response.tool_calls.iter().enumerate() {
            if stop_requested(session) {
                info!(
                    session_key = %session.session_key,
                    tool = %tc.name(),
                    "agent turn stopped before tool call"
                );
                append_cancelled_tool_results(
                    session,
                    &mut recorder,
                    &mut tool_calls_log,
                    &response.tool_calls,
                    tool_index,
                );
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            }
            info!(
                tool = %tc.name(),
                call_id = %tc.id,
                "executing tool call"
            );

            // Record tool call
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_tool_call_with_id(
                    &session.session_key,
                    tc.name(),
                    tc.arguments(),
                    Some(&tc.id),
                ) {
                    warn!(error = %e, "failed to record tool call");
                }
            }

            let stop_signal = session.stop_signal.clone();
            let tool_execution = async {
                if let Some(result) =
                    crate::tools::goal::execute_goal_tool(session, tc.name(), tc.arguments())
                {
                    result
                } else if tc.name() == TOOL_SEARCH_TOOL_NAME {
                    match tool_search_tool.as_ref() {
                        Some(tool) => tool.execute(tc.arguments(), &work_dir).await,
                        None => crate::types::ToolResult {
                            success: false,
                            output:
                                "tool_search is unavailable because no deferred tools are active"
                                    .to_string(),
                        },
                    }
                } else if mcp.as_ref().is_some_and(|m| m.is_mcp_tool(tc.name())) {
                    // Route to MCP server
                    match mcp.as_mut() {
                        Some(m) => match m.call_tool(tc.name(), tc.arguments()).await {
                            Ok(r) => r,
                            Err(e) => crate::types::ToolResult {
                                success: false,
                                output: format!("MCP tool error: {e}"),
                            },
                        },
                        None => crate::types::ToolResult {
                            success: false,
                            output: "MCP manager unavailable".to_string(),
                        },
                    }
                } else {
                    // Route to local tool registry
                    tools.execute(tc.name(), tc.arguments(), &work_dir).await
                }
            };
            let result = if let Some(signal) = stop_signal {
                tokio::select! {
                    result = tool_execution => Some(result),
                    _ = signal.cancelled() => None,
                }
            } else {
                Some(tool_execution.await)
            };
            let Some(result) = result else {
                info!(
                    session_key = %session.session_key,
                    tool = %tc.name(),
                    "agent turn stopped during tool call"
                );
                append_cancelled_tool_results(
                    session,
                    &mut recorder,
                    &mut tool_calls_log,
                    &response.tool_calls,
                    tool_index,
                );
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            };

            debug!(
                tool = %tc.name(),
                success = result.success,
                output_len = result.output.len(),
                output_preview = %telemetry_preview(&result.output),
                "tool call completed"
            );

            if tc.name() == TOOL_SEARCH_TOOL_NAME && result.success {
                for definition in parse_loadable_tool_definitions(&result.output) {
                    if visible_tool_names.insert(definition.name().to_string()) {
                        debug!(
                            tool = %definition.name(),
                            "loaded deferred tool definition from tool_search"
                        );
                        tool_defs.push(definition);
                    }
                }
            }

            // Apply tool output token limit truncation with 1.2x serialization
            // budget (matching Codex's policy * 1.2) to account for JSON escaping
            // overhead when the text is serialized into the API request payload
            let output = if tool_output_limit > 0 {
                let budget = (tool_output_limit as u64)
                    .saturating_mul(12)
                    .saturating_div(10) as usize;
                truncate_tool_output(&result.output, budget)
            } else {
                result.output.clone()
            };

            tool_calls_log.push(ToolCallLog {
                tool_name: tc.name().to_string(),
                arguments: tc.arguments().to_string(),
                result: result.output.clone(),
                success: result.success,
            });

            // Record tool result
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_tool_result_with_call_id(
                    &session.session_key,
                    tc.name(),
                    &result.output,
                    result.success,
                    Some(&tc.id),
                ) {
                    warn!(error = %e, "failed to record tool result");
                }
            }

            // Add tool result to history
            session.add_tool_result(&tc.id, &output);
            refresh_active_turn_status(
                session,
                config,
                ActiveTurnProgress {
                    state: "tool_calls",
                    current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                    completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                    max_loop_iterations: config.get_max_turn_iterations(),
                    local_tool_count,
                    mcp_tool_count,
                },
            );
            // Codex-aligned: skip ToolCompleted accounting for update_goal (the goal
            // handler itself fires ToolCompletedGoal with suppressed steering).
            if tc.name() != crate::tools::goal::UPDATE_GOAL_TOOL_NAME {
                crate::tools::goal::goal_runtime_apply(
                    session,
                    crate::tools::goal::GoalRuntimeEvent::ToolCompleted,
                );
            }
        }

        // Check if switch_workdir was called — if so, apply the switch and exit the turn
        if let Some(switch_log) = tool_calls_log
            .iter()
            .find(|l| l.tool_name == "switch_workdir" && l.success)
        {
            if let Some(new_dir) = switch_log.result.strip_prefix("SWITCH_WORKDIR:") {
                let new_dir = new_dir.to_string();
                info!(
                    session_key = %session.session_key,
                    new_work_dir = %new_dir,
                    "switching session work directory"
                );
                session.reinitialize_work_dir(new_dir.clone());
                return Ok(TurnResult {
                    response: format!(
                        "已切换工作目录到: {}\n\n会话历史已清空，已重新加载项目配置。",
                        new_dir
                    ),
                    tool_calls_log,
                    work_dir_switched: Some(new_dir),
                    title_updated: None,
                    plan_steps: None,
                    goal_needs_continuation: false,
                    goal_objective: None,
                });
            }
        }

        // Check if set_title was called — if so, update session.title
        if let Some(title_log) = tool_calls_log
            .iter()
            .rfind(|l| l.tool_name == "set_title" && l.success)
        {
            if let Some(new_title) = title_log.result.strip_prefix("SET_TITLE:") {
                session.title = Some(new_title.to_string());
                // Persist title update event
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_title_updated(&session.session_key, new_title) {
                        warn!(error = %e, "failed to record title update");
                    }
                }
            }
        }

        // Check if update_plan was called — if so, extract plan for IM card rendering
        if let Some(steps) = tool_calls_log
            .iter()
            .rfind(|l| l.tool_name == "update_plan" && l.success)
            .and_then(|l| extract_plan_steps_from_tool_result(&l.result))
        {
            session.current_plan = Some(steps.clone());
            session.plan_repair_attempts = 0;
            // Push real-time plan update to IM channel (include current title)
            if let Some(ref sender) = session.plan_sender {
                if let Err(e) = sender.send((steps, session.title.clone())) {
                    debug!(error = %e, "plan_sender channel closed");
                }
            }
        }

        // Guide-mode injection: check if an IM user sent a guide message while
        // this tool call batch was executing. If so, append it to history so the
        // model sees it in the next iteration (before compaction to preserve it).
        let guide_msg = session
            .guide_channel
            .as_ref()
            .and_then(|ch| ch.lock().unwrap().take());
        if let Some(guide_msg) = guide_msg {
            info!(
                session_key = %session.session_key,
                guide_msg_len = guide_msg.len(),
                "guide message injected mid-turn"
            );
            session.add_user_message(&guide_msg);
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_user_message(&session.session_key, &guide_msg) {
                    warn!(error = %e, "failed to record guide message");
                }
            }
        }

        // Mid-turn compaction check (same pattern as Codex's auto-compact in turn.rs)
        compact_mid_turn_if_needed(
            client,
            config,
            session,
            &mut recorder,
            &prompt_prefix,
            memory_message.as_ref(),
            ActiveTurnProgress {
                state: "tool_calls",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
        )
        .await;
    }

    error!(
        session_key = %session.session_key,
        max_iterations,
        "agent turn exceeded max iterations"
    );
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
            reason: crate::tools::goal::GoalAbortReason::Interrupted,
        },
    );
    Err(format!("exceeded maximum iterations ({max_iterations})"))
}

async fn handle_goal_command(session: &mut AgentSession, args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" || trimmed == "status" {
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    // External mutation starting: flush accounting before we modify goal state.
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::ExternalMutationStarting,
    );

    if trimmed == "complete" {
        return crate::tools::goal::execute_goal_tool(
            session,
            "update_goal",
            r#"{"status":"complete"}"#,
        )
        .expect("update_goal handler")
        .output;
    }

    if trimmed == "pause" {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::ExternalSet {
                status: crate::tools::goal::GoalStatus::Paused,
            },
        );
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    if trimmed == "resume" {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::ExternalSet {
                status: crate::tools::goal::GoalStatus::Active,
            },
        );
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    if let Some(rest) = trimmed.strip_prefix("set ") {
        let rest = rest.trim();
        if rest.is_empty() {
            return "用法: /goal [show|set <objective>|set --budget N <objective>|pause|resume|complete]"
                .to_string();
        }

        let (token_budget, objective) = if let Some(without_flag) = rest.strip_prefix("--budget ") {
            let without_flag = without_flag.trim();
            let Some((budget, objective)) = without_flag.split_once(char::is_whitespace) else {
                return "用法: /goal set --budget <N> <objective>".to_string();
            };
            let Ok(token_budget) = budget.parse::<u64>() else {
                return "goal budget 必须是正整数。".to_string();
            };
            (Some(token_budget), objective.trim())
        } else {
            (None, rest)
        };

        if objective.is_empty() {
            return "goal objective 不能为空。".to_string();
        }

        return crate::tools::goal::execute_goal_tool(
            session,
            "create_goal",
            &serde_json::json!({
                "objective": objective,
                "token_budget": token_budget,
            })
            .to_string(),
        )
        .expect("create_goal handler")
        .output;
    }

    "用法: /goal [show|set <objective>|set --budget N <objective>|pause|resume|complete]"
        .to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Maximum bytes for telemetry previews (matching Codex's TELEMETRY_PREVIEW_MAX_BYTES).
const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB

/// Maximum lines for telemetry previews (matching Codex's TELEMETRY_PREVIEW_MAX_LINES).
const TELEMETRY_PREVIEW_MAX_LINES: usize = 64;

/// Create a truncated preview of tool output for telemetry/logging.
/// Truncates both by bytes and lines, whichever limit is hit first.
fn telemetry_preview(output: &str) -> String {
    // First truncate by bytes (using safe UTF-8 boundary truncation)
    let truncated = if output.len() > TELEMETRY_PREVIEW_MAX_BYTES {
        let end = bifrost_core::text::floor_char_boundary(output, TELEMETRY_PREVIEW_MAX_BYTES);
        format!("{}... ({} bytes total)", &output[..end], output.len())
    } else {
        output.to_string()
    };

    // Then truncate by lines
    let lines: Vec<&str> = truncated.lines().collect();
    if lines.len() > TELEMETRY_PREVIEW_MAX_LINES {
        let kept: Vec<&str> = lines
            .into_iter()
            .take(TELEMETRY_PREVIEW_MAX_LINES)
            .collect();
        format!("{}\n... (more lines omitted)", kept.join("\n"))
    } else {
        truncated
    }
}

fn extract_plan_steps_from_tool_result(
    result: &str,
) -> Option<Vec<crate::tools::update_plan::PlanStep>> {
    result.strip_prefix("UPDATE_PLAN:").and_then(|json| {
        match serde_json::from_str::<crate::tools::update_plan::UpdatePlanArgs>(json) {
            Ok(args) => Some(args.plan),
            Err(e) => {
                warn!(error = %e, "failed to parse update_plan args");
                None
            }
        }
    })
}

fn plan_has_unfinished_steps(plan: &[crate::tools::update_plan::PlanStep]) -> bool {
    plan.iter().any(|step| {
        matches!(
            step.status,
            crate::tools::update_plan::PlanStepStatus::Pending
                | crate::tools::update_plan::PlanStepStatus::InProgress
        )
    })
}

/// Truncate tool output to stay within a token budget.
///
/// Uses the approximation of 1 token ≈ 4 characters. If the output exceeds
/// `max_tokens * 4` characters, removes the middle section and inserts a marker.
fn truncate_tool_output(output: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if output.len() <= max_chars {
        return output.to_string();
    }

    // Include total line count so the model knows the full output size
    // (matching Codex's formatted_truncate_text behavior)
    let total_lines = output.lines().count();

    // Keep first half and last half, remove middle
    let half = max_chars / 2;
    let head_end = output.floor_char_boundary(half);
    let tail_start = output.ceil_char_boundary(output.len() - half);
    let head = &output[..head_end];
    let tail = &output[tail_start..];
    let removed = output.len() - max_chars;
    format!(
        "Total output lines: {total_lines}\n\n{}\n\n... [{} characters truncated] ...\n\n{}",
        head, removed, tail
    )
}

/// Build initial context items for mid-turn compaction injection.
///
/// Inserted before the last user message in the compacted history via
/// [`compact::InitialContextInjection::BeforeLastUserMessage`] to preserve
/// continuity within the turn loop. Base/system instructions stay outside
/// replacement history, matching Codex's `Prompt.base_instructions` boundary;
/// only developer/contextual user prompt items plus optional memory are
/// reinjected.
fn build_mid_turn_initial_context(
    _session: &AgentSession,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
) -> Vec<ChatMessage> {
    let mut context =
        Vec::with_capacity(prompt_prefix.len() + usize::from(memory_message.is_some()));
    context.extend(
        prompt_prefix
            .iter()
            .filter(|message| message.role != "system")
            .cloned(),
    );
    if let Some(memory_message) = memory_message {
        context.push(memory_message.clone());
    }
    context
}

async fn compact_mid_turn_if_needed(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
    progress: ActiveTurnProgress,
) {
    if !compact::should_compact(session, config) {
        return;
    }

    info!(
        estimated_tokens = session.estimate_tokens(),
        compaction_count = session.compaction_count,
        "triggering mid-turn compaction"
    );
    let initial_context = build_mid_turn_initial_context(session, prompt_prefix, memory_message);
    match compact::compact_session(
        client,
        config,
        session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::MidTurn,
        prompt_prefix
            .iter()
            .find(|message| message.role == "system")
            .and_then(|message| message.content.as_deref()),
        compact::InitialContextInjection::BeforeLastUserMessage,
        initial_context,
    )
    .await
    {
        Ok(result) if result.performed => {
            record_compaction_event(
                recorder,
                session,
                &result,
                "auto",
                "context_limit",
                "mid_turn",
                false,
            );
            info!(
                tokens_saved = result.tokens_saved,
                duration_ms = ?result.duration_ms,
                "mid-turn compaction succeeded"
            );
            refresh_active_turn_status(session, config, progress);
        }
        Ok(_) => {}
        Err(e) => {
            warn!(error = %e, "mid-turn compaction failed");
        }
    }
}

async fn compact_pre_sampling_if_needed(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    base_instructions: &str,
) -> Result<(), String> {
    if !compact::should_compact(session, config) {
        return Ok(());
    }

    info!(
        session_key = %session.session_key,
        estimated_tokens = session.effective_token_count(),
        threshold = config.get_compact_threshold_tokens(),
        "pre-sampling token limit reached; running compaction before adding new input"
    );

    let result = compact::compact_session(
        client,
        config,
        session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::PreTurn,
        Some(base_instructions),
        compact::InitialContextInjection::DoNotInject,
        vec![],
    )
    .await?;

    if result.performed {
        record_compaction_event(
            recorder,
            session,
            &result,
            "auto",
            "context_limit",
            "pre_turn",
            false,
        );
        info!(
            tokens_saved = result.tokens_saved,
            duration_ms = ?result.duration_ms,
            "pre-sampling compact completed"
        );
    }

    Ok(())
}

fn record_compaction_event(
    recorder: &mut Option<&mut ConversationRecorder>,
    session: &AgentSession,
    result: &compact::CompactionResult,
    trigger: &str,
    reason: &str,
    phase: &str,
    emergency: bool,
) {
    let mut metadata = serde_json::json!({
        "trigger": trigger,
        "reason": reason,
        "phase": phase,
        "pre_tokens": result.pre_tokens,
        "post_tokens": result.post_tokens,
        "tokens_saved": result.tokens_saved,
        "messages_removed": result.messages_removed,
        "compaction_count": session.compaction_count,
        "total_tokens": session.total_tokens_used.unwrap_or(0),
    });
    if emergency {
        metadata["emergency"] = serde_json::Value::Bool(true);
    }

    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_compaction(&session.session_key, metadata) {
            warn!(error = %e, "failed to record compaction event");
        }
    }
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Check if an API error indicates context window overflow.
/// Matches common patterns from OpenAI/Azure/ModelHub error messages.
fn is_context_window_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("maximum context length")
        || lower.contains("token limit")
        || (lower.contains("too many tokens") && lower.contains("max"))
}

/// Check if an API error is transient and worth retrying.
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

/// Trim loop for context window overflow: repeatedly trim oldest messages
/// and retry until the request succeeds or history is exhausted.
/// This matches Codex's approach of aggressive degradation to keep the session alive.
async fn trim_loop_retry(
    client: &crate::client::AgentClient,
    config: &crate::config::AgentConfig,
    session: &mut AgentSession,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
    tool_defs: &[crate::types::ToolDefinition],
    progress: ActiveTurnProgress,
) -> Result<crate::types::ModelResponse, String> {
    const MAX_TRIM_ITERATIONS: usize = 10;
    const TRIM_BATCH_SIZE: usize = 4;

    for i in 0..MAX_TRIM_ITERATIONS {
        // Trim a batch of oldest messages
        let trimmed = trim_oldest_messages_count(session, TRIM_BATCH_SIZE);
        if trimmed == 0 {
            return Err("context window exceeded and history exhausted".to_string());
        }
        refresh_active_turn_status(session, config, progress);

        warn!(
            iteration = i + 1,
            trimmed_count = trimmed,
            history_len = session.history.len(),
            "trimmed oldest messages, retrying"
        );

        let retry_messages = session.build_messages(
            prompt_prefix,
            memory_message,
            config.get_max_history_messages(),
        );
        match chat_completion_or_stop(client, config, session, &retry_messages, tool_defs).await {
            Ok(Some(r)) => {
                info!(
                    iterations = i + 1,
                    total_trimmed = (i + 1) * TRIM_BATCH_SIZE,
                    "trim loop succeeded"
                );
                return Ok(r);
            }
            Ok(None) => return Err(STOPPED_ERROR.to_string()),
            Err(e) if is_context_window_error(&e) => {
                // Still over limit, continue trimming
                continue;
            }
            Err(e) => {
                // Different error, give up
                return Err(e);
            }
        }
    }

    Err("context window exceeded after maximum trim iterations".to_string())
}

/// Trim oldest messages and return the count actually removed.
fn trim_oldest_messages_count(session: &mut AgentSession, count: usize) -> usize {
    if session.history.is_empty() {
        return 0;
    }
    let mut removed = 0;
    let mut idx = 0;
    while idx < session.history.len() && removed < count {
        if session.compaction_count > 0 && compact::is_summary_message(&session.history[idx]) {
            idx += 1;
            continue;
        }
        session.history.remove(idx);
        removed += 1;
    }

    if removed > 0 {
        let (sanitized, report) = history::sanitize_chat_history(&session.history);
        if report.dropped_anything() {
            warn!(
                dropped_orphan_tool_messages = report.dropped_orphan_tool_messages,
                dropped_incomplete_tool_call_messages =
                    report.dropped_incomplete_tool_call_messages,
                "trim removed tool-call context; sanitized malformed history suffix"
            );
            session.history = sanitized;
        }
        session.history_version = session.history_version.saturating_add(1);
        session.invalidate_token_snapshot();
        removed
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelProviderConfig;
    use crate::types::ToolCallMessage;
    use std::collections::HashMap;

    fn test_tool_call(id: &str) -> ToolCallMessage {
        ToolCallMessage::function_call(
            id.to_string(),
            "list_directory".to_string(),
            r#"{"path":"."}"#.to_string(),
        )
    }

    fn count_messages_with_content(messages: &[ChatMessage], content: &str) -> usize {
        messages
            .iter()
            .filter(|message| message.content.as_deref() == Some(content))
            .count()
    }

    async fn chat_response_url(responses: Vec<serde_json::Value>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = Vec::new();
                let mut header_end = None;
                loop {
                    let mut chunk = [0_u8; 1024];
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if header_end.is_none() {
                        header_end = buffer.windows(4).position(|w| w == b"\r\n\r\n");
                    }
                    if let Some(pos) = header_end {
                        let headers = String::from_utf8_lossy(&buffer[..pos]);
                        let content_len = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if buffer.len() >= pos + 4 + content_len {
                            break;
                        }
                    }
                }

                let body = response.to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(http.as_bytes()).await.unwrap();
            }
        });
        format!("http://{addr}/chat/completions")
    }

    async fn slow_chat_response_url(
        delay: std::time::Duration,
    ) -> (String, tokio::sync::oneshot::Receiver<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = accepted_tx.send(());
            let mut buffer = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            tokio::time::sleep(delay).await;
            let body = chat_text_response("too late", 11).to_string();
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(http.as_bytes()).await;
        });
        (format!("http://{addr}/chat/completions"), accepted_rx)
    }

    fn provider_config_for_url(url: String) -> AgentConfig {
        let mut model_providers = HashMap::new();
        model_providers.insert(
            "test".to_string(),
            ModelProviderConfig {
                name: Some("test".to_string()),
                base_url: Some(url),
                env_key: None,
                api_key: Some("test-key".to_string()),
                http_headers: None,
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        AgentConfig {
            model: Some("test-model".to_string()),
            model_provider: Some("test".to_string()),
            model_providers,
            model_context_window: Some(1_000),
            model_auto_compact_token_limit: Some(100),
            request_timeout_secs: Some(30),
            ..Default::default()
        }
    }

    fn chat_text_response(content: &str, total_tokens: u64) -> serde_json::Value {
        serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": content
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": total_tokens.saturating_sub(1),
                "completion_tokens": 1,
                "total_tokens": total_tokens
            }
        })
    }

    async fn provider_config_for_responses(responses: Vec<serde_json::Value>) -> AgentConfig {
        let url = chat_response_url(responses).await;
        provider_config_for_url(url)
    }

    async fn single_summary_provider_config(total_tokens: u64) -> AgentConfig {
        provider_config_for_responses(vec![chat_text_response("compacted summary", total_tokens)])
            .await
    }

    #[test]
    fn test_session_new() {
        let session = AgentSession::new("test-user");
        assert_eq!(session.session_key, "test-user");
        assert!(session.history.is_empty());
        assert_eq!(session.compaction_count, 0);
        assert!(session.total_tokens_used.is_none());
    }

    #[test]
    fn test_session_clear_resets_all() {
        let mut session = AgentSession::new("test");
        session.add_user_message("hello");
        session.compaction_count = 3;
        session.total_tokens_used = Some(50000);
        session.current_plan = Some(vec![crate::tools::update_plan::PlanStep {
            step: "Do work".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::InProgress,
        }]);
        session.plan_repair_attempts = 1;
        session.clear();
        assert!(session.history.is_empty());
        assert_eq!(session.compaction_count, 0);
        assert!(session.total_tokens_used.is_none());
        assert!(session.current_plan.is_none());
        assert_eq!(session.plan_repair_attempts, 0);
        assert!(
            session.memory_cleared,
            "memory_cleared should be set after clear"
        );
        assert!(
            session.recorder.is_none(),
            "recorder should be dropped after clear"
        );
    }

    #[test]
    fn test_track_token_usage() {
        let mut session = AgentSession::new("test");
        assert!(session.total_tokens_used.is_none());

        session.add_user_message("hello");
        session.track_token_usage(1000);
        assert_eq!(session.total_tokens_used, Some(1000));
        assert_eq!(session.last_response_tokens, Some(1000));
        assert_eq!(session.last_response_history_len, Some(1));

        session.add_assistant_message("hi");
        assert_eq!(session.last_response_history_len, Some(2));
        session.track_token_usage(2000);
        assert_eq!(session.total_tokens_used, Some(3000));
        assert_eq!(session.last_response_tokens, Some(2000));
        assert_eq!(session.last_response_history_len, Some(2));
    }

    #[test]
    fn test_background_token_usage_does_not_update_context_snapshot() {
        let mut session = AgentSession::new("test");

        session.track_token_usage(1000);
        session.track_background_token_usage(2500);

        assert_eq!(session.total_tokens_used, Some(3500));
        assert_eq!(session.last_response_tokens, Some(1000));
    }

    #[test]
    fn test_record_compaction_event_includes_emergency_and_total_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = ConversationRecorder::new(dir.path(), "test");
        let file_path = recorder.file_path().to_path_buf();

        let mut session = AgentSession::new("test");
        session.compaction_count = 2;
        session.total_tokens_used = Some(1234);
        let result = compact::CompactionResult {
            performed: true,
            pre_tokens: 900,
            post_tokens: 300,
            tokens_saved: 600,
            messages_removed: 8,
            reason: None,
            trigger: None,
            compaction_reason: None,
            phase: None,
            duration_ms: Some(10),
        };

        {
            let mut recorder_ref = Some(&mut recorder);
            record_compaction_event(
                &mut recorder_ref,
                &session,
                &result,
                "auto",
                "context_limit",
                "mid_turn",
                true,
            );
        }
        recorder.close();

        let events = persistence::load_conversation_events(&file_path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, persistence::event_types::COMPACTION);
        assert_eq!(events[0].content["emergency"], true);
        assert_eq!(events[0].content["total_tokens"], 1234);
        assert_eq!(events[0].content["compaction_count"], 2);
        assert_eq!(events[0].content["phase"], "mid_turn");
    }

    #[test]
    fn test_history_growth_after_response_is_incrementally_accounted() {
        let mut session = AgentSession::new("test");
        session.track_token_usage(100);
        assert_eq!(session.last_response_tokens, Some(100));

        session.add_user_message(&"x".repeat(1_000));

        assert_eq!(session.last_response_tokens, Some(100));
        assert_eq!(session.effective_token_count(), 350);
    }

    #[test]
    fn test_model_message_advances_incremental_token_boundary() {
        let mut session = AgentSession::new("test");
        session.add_user_message(&"old context ".repeat(100));
        session.track_token_usage(500);
        session.add_assistant_message("covered by usage");
        session.add_user_message(&"new context ".repeat(100));

        assert_eq!(session.last_response_history_len, Some(2));
        assert_eq!(session.effective_token_count(), 800);
    }

    #[test]
    fn test_recompute_token_snapshot_includes_base_instructions() {
        let mut session = AgentSession::new("test");
        session.add_user_message("hello");
        let base = "base instructions ".repeat(20);

        session.recompute_token_snapshot_from_history(Some(&base));

        let expected =
            session
                .estimate_tokens()
                .saturating_add(AgentSession::estimate_messages_tokens(&[
                    ChatMessage::system(&base),
                ]));
        assert_eq!(session.last_response_tokens, Some(u64::from(expected)));
        assert_eq!(session.effective_token_count(), expected);
    }

    #[test]
    fn test_mid_turn_initial_context_excludes_base_instructions() {
        let session = AgentSession::new("test");
        let prompt_prefix = vec![
            ChatMessage::system("base instructions"),
            ChatMessage::developer("developer context"),
            ChatMessage::user("<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"),
        ];
        let memory_message = ChatMessage::developer("memory context");

        let context =
            build_mid_turn_initial_context(&session, &prompt_prefix, Some(&memory_message));

        assert_eq!(context.len(), 3);
        assert!(context
            .iter()
            .all(|message| message.content.as_deref() != Some("base instructions")));
        assert_eq!(
            count_messages_with_content(&context, "developer context"),
            1
        );
        assert_eq!(
            count_messages_with_content(
                &context,
                "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
            ),
            1
        );
        assert_eq!(count_messages_with_content(&context, "memory context"), 1);
    }

    #[test]
    fn test_build_messages_dedupes_injected_mid_turn_context() {
        let mut session = AgentSession::new("test");
        let env_context = "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>";
        session
            .history
            .push(ChatMessage::developer("developer context"));
        session.history.push(ChatMessage::user(env_context));
        session
            .history
            .push(ChatMessage::developer("memory context"));
        session.history.push(ChatMessage::user(&format!(
            "{}\nsummary",
            compact::SUMMARY_PREFIX
        )));
        let prompt_prefix = vec![
            ChatMessage::system("base instructions"),
            ChatMessage::developer("developer context"),
            ChatMessage::user(env_context),
        ];
        let memory_message = ChatMessage::developer("memory context");

        let messages = session.build_messages(&prompt_prefix, Some(&memory_message), 0);

        assert_eq!(
            count_messages_with_content(&messages, "base instructions"),
            1
        );
        assert_eq!(
            count_messages_with_content(&messages, "developer context"),
            1
        );
        assert_eq!(count_messages_with_content(&messages, env_context), 1);
        assert_eq!(count_messages_with_content(&messages, "memory context"), 1);
        assert!(messages[0].role == "system");
    }

    #[test]
    fn test_build_messages_only_dedupes_context_selected_for_request() {
        let mut session = AgentSession::new("test");
        session
            .history
            .push(ChatMessage::developer("developer context"));
        for i in 0..10 {
            session.add_user_message(&format!("msg {i}"));
        }
        let prompt_prefix = vec![
            ChatMessage::system("base instructions"),
            ChatMessage::developer("developer context"),
        ];

        let messages = session.build_messages(&prompt_prefix, None, 5);

        assert_eq!(
            count_messages_with_content(&messages, "base instructions"),
            1
        );
        assert_eq!(
            count_messages_with_content(&messages, "developer context"),
            1
        );
        assert_eq!(messages.len(), 7);
        assert_eq!(messages[2].content.as_deref(), Some("msg 5"));
        assert_eq!(messages[6].content.as_deref(), Some("msg 9"));
    }

    #[tokio::test]
    async fn test_queued_continuation_compacts_before_next_model_request() {
        let mut config = single_summary_provider_config(77).await;
        config.model_auto_compact_token_limit = Some(500);
        let client = AgentClient::new();
        let mut session = AgentSession::new("queued-compaction");
        let status_handle = std::sync::Arc::new(std::sync::Mutex::new(ActiveTurnStatus::new(
            &session.session_key,
        )));
        session.active_turn_status = Some(status_handle.clone());

        session.add_user_message("first");
        session.add_assistant_message("reply");
        session.add_user_message("second");
        session.add_assistant_message("final before queued message");
        session.track_token_usage(17);
        session.add_user_message(&"queued context ".repeat(200));

        assert!(compact::should_compact(&session, &config));
        let mut recorder: Option<&mut ConversationRecorder> = None;
        let prompt_prefix = vec![
            ChatMessage::system("base instructions"),
            ChatMessage::developer("developer context"),
            ChatMessage::user("<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"),
        ];
        let memory_message = ChatMessage::developer("memory context");
        compact_mid_turn_if_needed(
            &client,
            &config,
            &mut session,
            &mut recorder,
            &prompt_prefix,
            Some(&memory_message),
            ActiveTurnProgress {
                state: "model_response",
                current_loop_iteration: 1,
                completed_loop_iterations: 1,
                max_loop_iterations: 8,
                local_tool_count: 0,
                mcp_tool_count: 0,
            },
        )
        .await;

        assert_eq!(session.compaction_count, 1);
        assert_eq!(session.total_tokens_used, Some(94));
        let expected_snapshot_tokens =
            session
                .estimate_tokens()
                .saturating_add(AgentSession::estimate_messages_tokens(&[
                    ChatMessage::system("base instructions"),
                ]));
        assert_eq!(
            session.last_response_tokens,
            Some(u64::from(expected_snapshot_tokens))
        );
        assert_eq!(
            session.last_response_history_len,
            Some(session.history.len())
        );
        assert!(session
            .history
            .iter()
            .all(|message| message.content.as_deref() != Some("base instructions")));
        assert_eq!(
            count_messages_with_content(&session.history, "developer context"),
            1
        );
        assert_eq!(
            count_messages_with_content(
                &session.history,
                "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
            ),
            1
        );
        assert_eq!(
            count_messages_with_content(&session.history, "memory context"),
            1
        );
        assert!(session
            .history
            .last()
            .is_some_and(compact::is_summary_message));
        let messages = session.build_messages(&prompt_prefix, Some(&memory_message), 0);
        assert_eq!(
            count_messages_with_content(&messages, "base instructions"),
            1
        );
        assert_eq!(
            count_messages_with_content(&messages, "developer context"),
            1
        );
        assert_eq!(
            count_messages_with_content(
                &messages,
                "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
            ),
            1
        );
        assert_eq!(count_messages_with_content(&messages, "memory context"), 1);
        let status = status_handle.lock().unwrap().clone();
        assert_eq!(status.compaction_count, session.compaction_count);
        assert_eq!(status.history_version, session.history_version);
        assert_eq!(
            status.estimated_context_tokens,
            session.effective_token_count()
        );
    }

    #[tokio::test]
    async fn test_pre_sampling_auto_compacts_before_model_request() {
        let mut config = provider_config_for_responses(vec![
            chat_text_response("compacted summary", 77),
            chat_text_response("final answer", 33),
        ])
        .await;
        config.model_auto_compact_token_limit = Some(100);
        let client = AgentClient::new();
        let tools = ToolRegistry::new();
        let mut session = AgentSession::new("pre-sampling-compaction");
        session.add_user_message(&"old context ".repeat(1_000));

        assert!(compact::should_compact(&session, &config));
        let result = run_turn(
            &client,
            &config,
            &mut session,
            &tools,
            "answer without tools",
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.response, "final answer");
        assert_eq!(session.compaction_count, 1);
        assert!(session.history.iter().any(compact::is_summary_message));
        assert!(session
            .history
            .iter()
            .all(|message| message.content.as_deref() != Some("base instructions")));
        assert!(session
            .history
            .iter()
            .any(|message| message.content.as_deref() == Some("answer without tools")));
        assert!(session
            .history
            .iter()
            .any(|message| message.content.as_deref() == Some("final answer")));
    }

    #[tokio::test]
    async fn test_stop_request_cancels_in_flight_model_request() {
        let (url, accepted_rx) = slow_chat_response_url(std::time::Duration::from_secs(30)).await;
        let config = provider_config_for_url(url);
        let client = AgentClient::new();
        let tools = ToolRegistry::new();
        let manager = std::sync::Arc::new(AgentSessionManager::new(60));
        let mut session = manager
            .try_take_session("stop-model-request")
            .expect("session should be available");
        let manager_for_stop = std::sync::Arc::clone(&manager);

        let handle = tokio::spawn(async move {
            let result = run_turn(
                &client,
                &config,
                &mut session,
                &tools,
                "please wait for a slow model",
                None,
            )
            .await;
            (result, session)
        });

        accepted_rx
            .await
            .expect("slow mock should receive the model request");
        assert!(manager_for_stop.request_stop("stop-model-request"));

        let (result, session) = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
            .await
            .expect("turn should stop promptly")
            .expect("join should succeed");
        let result = result.expect("stop should return a normal turn result");
        assert!(result.response.contains("已收到 /stop"));
        assert!(session
            .history
            .iter()
            .any(|message| message.content.as_deref() == Some(STOPPED_RESPONSE)));
    }

    #[tokio::test]
    async fn test_stop_when_idle_does_not_call_model() {
        let config =
            provider_config_for_responses(vec![chat_text_response("should not call", 1)]).await;
        let client = AgentClient::new();
        let tools = ToolRegistry::new();
        let mut session = AgentSession::new("idle-stop");

        let result = run_turn(&client, &config, &mut session, &tools, "/stop", None)
            .await
            .unwrap();

        assert_eq!(result.response, "当前没有正在执行的 Agent loop。");
        assert!(session.history.is_empty());
        assert!(session.total_tokens_used.is_none());
    }

    #[tokio::test]
    async fn test_compaction_recomputes_context_snapshot_and_drops_tool_suffix() {
        let mut config = single_summary_provider_config(77).await;
        config.model_auto_compact_token_limit = Some(500);
        let client = AgentClient::new();
        let mut session = AgentSession::new("tool-output-compaction");

        session.add_user_message("first");
        session.add_assistant_message("reply");
        session.add_user_message("inspect generated output");
        session.track_token_usage(17);
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", &"tool-output ".repeat(2_000));

        let pre_tokens = session.effective_token_count();
        assert!(compact::should_compact(&session, &config));

        let result = compact::compact_session(
            &client,
            &config,
            &mut session,
            compact::CompactionTrigger::Auto,
            compact::CompactionReason::ContextLimit,
            compact::CompactionPhase::MidTurn,
            None,
            compact::InitialContextInjection::DoNotInject,
            Vec::new(),
        )
        .await
        .unwrap();

        assert!(result.performed);
        assert_eq!(result.pre_tokens, pre_tokens);
        assert_eq!(session.compaction_count, 1);
        assert_eq!(session.total_tokens_used, Some(94));
        assert!(session.history.iter().all(|message| message.role == "user"));
        assert!(session
            .history
            .iter()
            .all(|message| message.content.as_deref() != Some("reply")));
        assert!(session.history.iter().all(|message| message.role != "tool"));
        assert!(session
            .history
            .last()
            .is_some_and(compact::is_summary_message));
        assert_eq!(
            session.last_response_tokens,
            Some(u64::from(session.estimate_tokens()))
        );
        assert_eq!(
            session.last_response_history_len,
            Some(session.history.len())
        );
        assert_eq!(result.post_tokens, session.effective_token_count());
        assert!(result.post_tokens < config.get_compact_threshold_tokens());
        assert!(!compact::should_compact(&session, &config));
    }

    #[tokio::test]
    async fn test_compaction_runs_for_first_tool_turn_below_four_messages() {
        let mut config = single_summary_provider_config(77).await;
        config.model_auto_compact_token_limit = Some(500);
        let client = AgentClient::new();
        let mut session = AgentSession::new("first-tool-turn-compaction");

        session.add_user_message("inspect generated output");
        session.track_token_usage(17);
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", &"tool-output ".repeat(2_000));

        assert_eq!(session.history.len(), 3);
        assert!(compact::should_compact(&session, &config));

        let result = compact::compact_session(
            &client,
            &config,
            &mut session,
            compact::CompactionTrigger::Auto,
            compact::CompactionReason::ContextLimit,
            compact::CompactionPhase::MidTurn,
            None,
            compact::InitialContextInjection::DoNotInject,
            Vec::new(),
        )
        .await
        .unwrap();

        assert!(result.performed);
        assert_eq!(session.compaction_count, 1);
        assert!(session.history.iter().all(|message| message.role == "user"));
        assert!(session.history.iter().all(|message| message.role != "tool"));
        assert!(session
            .history
            .last()
            .is_some_and(compact::is_summary_message));
        assert!(result.post_tokens < config.get_compact_threshold_tokens());
        assert!(!compact::should_compact(&session, &config));
    }

    #[test]
    fn test_context_rewrite_refreshes_active_status_snapshot() {
        let config = AgentConfig {
            model_context_window: Some(1_000),
            ..Default::default()
        };
        let mut session = AgentSession::new("status-refresh");
        let status_handle = std::sync::Arc::new(std::sync::Mutex::new(ActiveTurnStatus::new(
            &session.session_key,
        )));
        session.active_turn_status = Some(status_handle.clone());
        session.add_user_message(&"before ".repeat(800));
        refresh_active_turn_status(
            &session,
            &config,
            ActiveTurnProgress {
                state: "model_request",
                current_loop_iteration: 1,
                completed_loop_iterations: 0,
                max_loop_iterations: 8,
                local_tool_count: 0,
                mcp_tool_count: 0,
            },
        );
        let before = status_handle.lock().unwrap().estimated_context_tokens;

        session.history = vec![ChatMessage::user("compacted summary")];
        session.compaction_count = 1;
        session.history_version = session.history_version.saturating_add(1);
        refresh_active_turn_status(
            &session,
            &config,
            ActiveTurnProgress {
                state: "model_request",
                current_loop_iteration: 1,
                completed_loop_iterations: 0,
                max_loop_iterations: 8,
                local_tool_count: 0,
                mcp_tool_count: 0,
            },
        );

        let status = status_handle.lock().unwrap().clone();
        assert!(before > status.estimated_context_tokens);
        assert_eq!(status.compaction_count, 1);
        assert_eq!(status.history_version, session.history_version);
        assert_eq!(status.estimated_context_tokens, session.estimate_tokens());
    }

    #[test]
    fn test_trim_oldest_messages_invalidates_response_tokens() {
        let mut session = AgentSession::new("trim");
        session.add_user_message("inspect");
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", "[file] Cargo.toml");
        session.add_user_message("continue");
        session.track_token_usage(10);

        let trimmed = trim_oldest_messages_count(&mut session, 2);

        assert_eq!(trimmed, 2);
        assert!(session.last_response_tokens.is_none());
        assert_eq!(session.history_version, 1);
    }

    #[test]
    fn test_build_messages_with_limit() {
        let mut session = AgentSession::new("test");
        for i in 0..10 {
            session.add_user_message(&format!("msg {i}"));
        }
        // Limit to 5 messages
        let prefix = vec![ChatMessage::system("system")];
        let messages = session.build_messages(&prefix, None, 5);
        // 1 system + 5 history = 6
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, "system");
        // Should have the last 5 user messages
        assert_eq!(messages[1].content.as_deref(), Some("msg 5"));
        assert_eq!(messages[5].content.as_deref(), Some("msg 9"));
    }

    #[test]
    fn test_build_messages_preserves_compaction_summary() {
        let mut session = AgentSession::new("test");
        session.compaction_count = 1; // Mark as compacted
        for i in 0..10 {
            session.add_user_message(&format!("msg {i}"));
        }
        session.add_user_message(&format!("{}\nprevious context", compact::SUMMARY_PREFIX));
        // Limit to 5 messages
        let prefix = vec![ChatMessage::system("system")];
        let messages = session.build_messages(&prefix, None, 5);
        // 1 system + 5 history (4 recent + summary) = 6
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].content.as_deref(), Some("msg 6"));
        assert!(messages[5]
            .content
            .as_deref()
            .is_some_and(|content| content.starts_with(compact::SUMMARY_PREFIX)));
    }

    #[test]
    fn test_build_messages_no_limit() {
        let mut session = AgentSession::new("test");
        for i in 0..5 {
            session.add_user_message(&format!("msg {i}"));
        }
        let prefix = vec![ChatMessage::system("system")];
        let messages = session.build_messages(&prefix, None, 0); // 0 = no limit
        assert_eq!(messages.len(), 6); // 1 system + 5 history
    }

    #[test]
    fn test_build_messages_sanitizes_tool_when_history_limit_cuts_assistant_tool_calls() {
        let mut session = AgentSession::new("test");
        session.add_user_message("inspect");
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", "[file] Cargo.toml");

        let prefix = vec![ChatMessage::system("system")];
        let messages = session.build_messages(&prefix, None, 1);

        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_session_info_from_list() {
        let manager = AgentSessionManager::new(3600);
        let mut session = AgentSession::new("user-1");
        session.add_user_message("hello");
        session.total_tokens_used = Some(500);
        manager.return_session(session);

        let sessions = manager.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_key, "user-1");
        assert_eq!(sessions[0].message_count, 1);
        assert_eq!(sessions[0].total_tokens_used, Some(500));
    }

    #[test]
    fn test_take_session_with_work_dir_overrides_existing_status_session() {
        let manager = AgentSessionManager::new(3600);
        let mut status_session = manager
            .try_take_session_with_work_dir("status-first", None)
            .expect("first take should create status session");
        status_session.add_user_message("/status");
        manager.return_session(status_session);

        let session = manager
            .try_take_session_with_work_dir("status-first", Some("/tmp/bifrost-work".to_string()))
            .expect("second take should not be busy");

        assert_eq!(session.work_dir.as_deref(), Some("/tmp/bifrost-work"));
        assert!(session.history.is_empty());
        assert!(!session.memory_cleared);
    }

    #[test]
    fn test_rollback_single_turn() {
        let mut session = AgentSession::new("test");
        session.add_user_message("msg1");
        session.add_assistant_message("reply1");
        session.add_user_message("msg2");
        session.add_assistant_message("reply2");
        assert_eq!(session.history.len(), 4);

        let removed = session.rollback(1);
        assert_eq!(removed, 2); // user + assistant
        assert_eq!(session.history.len(), 2);
        assert_eq!(session.history[0].content.as_deref(), Some("msg1"));
        assert_eq!(session.history_version, 1);
    }

    #[test]
    fn test_rollback_multiple_turns() {
        let mut session = AgentSession::new("test");
        for i in 0..3 {
            session.add_user_message(&format!("msg{i}"));
            session.add_assistant_message(&format!("reply{i}"));
        }
        assert_eq!(session.history.len(), 6);

        let removed = session.rollback(2);
        assert_eq!(removed, 4);
        assert_eq!(session.history.len(), 2);
    }

    #[test]
    fn test_rollback_preserves_compaction_summary() {
        let mut session = AgentSession::new("test");
        session.compaction_count = 1; // Marked as compacted
        session.add_user_message("msg1");
        session.add_assistant_message("reply1");
        session.add_user_message(&format!("{}\nsummary", compact::SUMMARY_PREFIX));

        // Roll back more turns than exist
        let removed = session.rollback(999);
        assert_eq!(session.history.len(), 1); // Summary preserved
        assert!(compact::is_summary_message(&session.history[0]));
        assert!(removed > 0);
    }

    #[test]
    fn test_rollback_zero_is_noop() {
        let mut session = AgentSession::new("test");
        session.add_user_message("hello");
        let removed = session.rollback(0);
        assert_eq!(removed, 0);
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.history_version, 0);
    }

    #[test]
    fn test_clear_bumps_history_version() {
        let mut session = AgentSession::new("test");
        session.add_user_message("hello");
        assert_eq!(session.history_version, 0);
        session.clear();
        assert_eq!(session.history_version, 1);
    }

    #[test]
    fn test_clear_sets_memory_cleared_flag() {
        let mut session = AgentSession::new("test");
        assert!(!session.memory_cleared);
        session.add_user_message("hello");
        session.add_assistant_message("hi");
        session.clear();
        assert!(session.memory_cleared, "clear should set memory_cleared");
        assert!(session.history.is_empty());

        // After adding a new message, the flag should still be set
        // (it's only reset by the turn loop, not by adding messages)
        session.add_user_message("new message");
        assert!(session.memory_cleared);
    }

    #[test]
    fn test_is_context_window_error() {
        assert!(is_context_window_error(
            "API error (status 400): context_length_exceeded"
        ));
        assert!(is_context_window_error(
            "This model's maximum context length is 128000"
        ));
        assert!(!is_context_window_error(
            "API error (status 500): internal error"
        ));
    }

    #[test]
    fn test_is_retryable_error() {
        assert!(is_retryable_error("API error (status 429): rate limit"));
        assert!(is_retryable_error("HTTP request failed: timeout"));
        assert!(is_retryable_error("connection reset"));
        assert!(!is_retryable_error("invalid api key"));
    }

    #[test]
    fn test_trim_oldest_messages() {
        let mut session = AgentSession::new("test");
        session.add_user_message("inspect");
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", "[file] Cargo.toml");
        session.add_user_message("continue");
        let trimmed = trim_oldest_messages_count(&mut session, 2);
        assert_eq!(trimmed, 2);
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.history[0].content.as_deref(), Some("continue"));
        assert!(history::is_valid_chat_history(&session.history));
    }

    #[test]
    fn test_trim_oldest_preserves_summary() {
        let mut session = AgentSession::new("test");
        session.compaction_count = 1;
        for i in 0..5 {
            session.add_user_message(&format!("msg{i}"));
        }
        session.add_user_message(&format!("{}\nsummary", compact::SUMMARY_PREFIX));
        let trimmed = trim_oldest_messages_count(&mut session, 2);
        assert_eq!(trimmed, 2);
        assert_eq!(session.history.len(), 4); // summary + 3 remaining
        assert_eq!(session.history[0].content.as_deref(), Some("msg2"));
        assert!(compact::is_summary_message(&session.history[3]));
    }

    #[test]
    fn test_extract_plan_steps_from_tool_result_valid() {
        let result = r#"UPDATE_PLAN:{"plan":[{"step":"Inspect files","status":"completed"},{"step":"Write summary","status":"in_progress"}],"explanation":"Progress update"}"#;
        let steps = extract_plan_steps_from_tool_result(result).expect("should parse");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step, "Inspect files");
        assert_eq!(
            steps[0].status,
            crate::tools::update_plan::PlanStepStatus::Completed
        );
        assert_eq!(
            steps[1].status,
            crate::tools::update_plan::PlanStepStatus::InProgress
        );
    }

    #[test]
    fn test_extract_plan_steps_from_tool_result_invalid() {
        assert!(extract_plan_steps_from_tool_result("NOT_UPDATE_PLAN:{}").is_none());
        assert!(extract_plan_steps_from_tool_result("UPDATE_PLAN:{invalid-json}").is_none());
    }

    #[test]
    fn test_plan_has_unfinished_steps_false_when_all_completed() {
        let plan = vec![
            crate::tools::update_plan::PlanStep {
                step: "A".to_string(),
                status: crate::tools::update_plan::PlanStepStatus::Completed,
            },
            crate::tools::update_plan::PlanStep {
                step: "B".to_string(),
                status: crate::tools::update_plan::PlanStepStatus::Completed,
            },
        ];
        assert!(!plan_has_unfinished_steps(&plan));
    }

    #[test]
    fn test_plan_has_unfinished_steps_true_for_pending_or_in_progress() {
        let pending_plan = vec![crate::tools::update_plan::PlanStep {
            step: "A".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::Pending,
        }];
        assert!(plan_has_unfinished_steps(&pending_plan));

        let in_progress_plan = vec![crate::tools::update_plan::PlanStep {
            step: "B".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::InProgress,
        }];
        assert!(plan_has_unfinished_steps(&in_progress_plan));
    }

    #[test]
    fn test_truncate_tool_output_short() {
        let output = "short output";
        let result = truncate_tool_output(output, 100);
        assert_eq!(result, output);
    }

    #[test]
    fn test_truncate_tool_output_long() {
        let output = "a".repeat(1000);
        let result = truncate_tool_output(&output, 100); // 100 tokens = 400 chars limit
        assert!(result.len() < output.len());
        assert!(result.contains("characters truncated"));
    }
}
