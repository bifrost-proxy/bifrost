//! Agent session management and core turn loop.
//!
//! Session lifecycle:
//! - Created on first message from a user
//! - Tracks conversation history, token usage, and compaction state
//! - Expired sessions are cleaned up by the session manager
//!
//! Turn loop (inspired by Codex's session/turn.rs):
//! 1. Pre-turn compaction check (if context is too large)
//! 2. Build system prompt + history + user message
//! 3. Send to model with available tools
//! 4. If model returns tool_calls → execute → mid-turn compaction check → loop
//! 5. If model returns text → return as final response

use crate::client::AgentClient;
use crate::compact;
use crate::config::AgentConfig;
use crate::history;
use crate::mcp::McpManager;
use crate::memory;
use crate::persistence;
use crate::persistence::ConversationRecorder;
use crate::prompt;
use crate::skill_authoring::SkillAuthoringHub;
use crate::slash::{BuiltinCommand, Dispatch, SlashCommandRouter};
use crate::tools::ToolRegistry;
use crate::types::{ChatMessage, ToolCallLog, ToolCallMessage, TurnResult};
use bifrost_skills::{default_roots, SkillRegistry, SkillStore};
use dashmap::{DashMap, DashSet};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// AgentSession
// ---------------------------------------------------------------------------

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

    /// Pre-loaded user instructions (AGENTS.md + config instructions).
    /// Resolved once at session creation time, matching Codex's behavior.
    /// Reused across all turns in this session.
    pub user_instructions: Option<String>,

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
            plan_repair_attempts: 0,
            plan_sender: None,
        }
    }

    pub fn new_with_work_dir(session_key: &str, work_dir: Option<String>) -> Self {
        let mut session = Self::new(session_key);
        session.work_dir = work_dir;
        session.attach_default_skill_registry();
        session
    }

    /// Load user instructions (AGENTS.md + config instructions) once at session
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
        self.user_instructions =
            manager.user_instructions(&work_dir, Some(&home_dir), config.instructions.as_deref());
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
        self.last_response_tokens = None;
        self.history_version = self.history_version.saturating_add(1);
        self.current_plan = None;
        self.current_goal = None;
        self.plan_repair_attempts = 0;
        // Invalidate cached user instructions so they are reloaded on next turn
        // (e.g. after switch_workdir changes the project context).
        self.user_instructions = None;
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
            // If rolling back more turns than exist, preserve compaction summary if present
            if self.compaction_count > 0 && !self.history.is_empty() {
                // Keep the compaction summary (first message)
                1
            } else {
                0
            }
        } else {
            user_positions[user_positions.len() - n]
        };

        let removed = self.history.len() - cut_idx;
        self.history.truncate(cut_idx);
        self.history_version = self.history_version.saturating_add(1);
        // Reset last_response_tokens since history changed
        self.last_response_tokens = None;
        removed
    }

    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        current_time_secs() - self.last_active_at > ttl_secs
    }

    fn touch(&mut self) {
        self.last_active_at = current_time_secs();
    }

    /// Track token usage from an API response.
    fn track_token_usage(&mut self, total_tokens: u64) {
        self.last_response_tokens = Some(total_tokens);
        self.total_tokens_used = Some(
            self.total_tokens_used
                .unwrap_or(0)
                .saturating_add(total_tokens),
        );
    }

    fn add_user_message(&mut self, content: &str) {
        self.history.push(ChatMessage::user(content));
        self.touch();
    }

    fn add_assistant_message(&mut self, content: &str) {
        self.history.push(ChatMessage::assistant(content));
        self.touch();
    }

    fn add_assistant_tool_calls(&mut self, tool_calls: &[ToolCallMessage]) {
        self.history
            .push(ChatMessage::assistant_with_tool_calls(tool_calls.to_vec()));
        self.touch();
    }

    fn add_tool_result(&mut self, call_id: &str, content: &str) {
        self.history
            .push(ChatMessage::tool_result(call_id, content));
        self.touch();
    }

    /// Build the full message list for a model call.
    ///
    /// Enforces `max_history_messages` limit by dropping the oldest non-summary messages
    /// while preserving the compaction summary (first message after compaction).
    fn build_messages(
        &self,
        system_prompt: &str,
        memory_message: Option<&ChatMessage>,
        max_history: u32,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();
        if !system_prompt.is_empty() {
            messages.push(ChatMessage::system(system_prompt));
        }
        if let Some(memory_message) = memory_message {
            messages.push(memory_message.clone());
        }

        let history = &self.history;
        let max = max_history as usize;

        if max > 0 && history.len() > max {
            // Keep the first message if it's a compaction summary, then the most recent messages
            let has_summary = self.compaction_count > 0 && !history.is_empty();
            if has_summary {
                // Preserve compaction summary (first message) + most recent (max-1) messages
                messages.push(history[0].clone());
                let tail_start = history.len().saturating_sub(max.saturating_sub(1));
                let tail_start = tail_start.max(1); // Don't duplicate the first message
                messages.extend_from_slice(&history[tail_start..]);
            } else {
                // No compaction summary — just take the most recent messages
                let tail_start = history.len().saturating_sub(max);
                messages.extend_from_slice(&history[tail_start..]);
            }
        } else {
            messages.extend(history.iter().cloned());
        }

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

    /// Rough token count estimate (1 token ≈ 4 chars).
    /// This is a coarse lower bound, same approach as Codex (codex-rs/utils/string/src/truncate.rs).
    pub fn estimate_tokens(&self) -> u32 {
        let total_chars: usize = self
            .history
            .iter()
            .map(|m| {
                m.content.as_ref().map(|c| c.len()).unwrap_or(0)
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

    /// Get the effective token count — prefers real API data over estimates.
    pub fn effective_token_count(&self) -> u32 {
        self.last_response_tokens
            .map(|t| u32::try_from(t).unwrap_or(u32::MAX))
            .unwrap_or_else(|| self.estimate_tokens())
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
    session_ttl_secs: u64,
}

impl AgentSessionManager {
    pub fn new(session_ttl_secs: u64) -> Self {
        Self {
            sessions: DashMap::new(),
            active_sessions: DashSet::new(),
            session_ttl_secs,
        }
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
        self.sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new(session_key))
    }

    /// Try to take a session. Returns `None` if the session is currently
    /// being processed by another turn loop (busy).
    pub fn try_take_session(&self, session_key: &str) -> Option<AgentSession> {
        // DashSet::insert returns true if the value was newly inserted.
        if !self.active_sessions.insert(session_key.to_string()) {
            return None; // already active
        }
        let session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new(session_key));
        Some(session)
    }

    /// Take a session, creating one with a specific work_dir if it doesn't exist.
    pub fn take_session_with_work_dir(
        &self,
        session_key: &str,
        work_dir: Option<String>,
    ) -> AgentSession {
        self.active_sessions.insert(session_key.to_string());
        self.sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new_with_work_dir(session_key, work_dir))
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
        let session = self
            .sessions
            .remove(session_key)
            .map(|(_, s)| s)
            .unwrap_or_else(|| AgentSession::new_with_work_dir(session_key, work_dir));
        Some(session)
    }

    /// Return a session to the manager after a turn completes.
    /// Also removes the session from the active set.
    pub fn return_session(&self, session: AgentSession) {
        let key = session.session_key.clone();
        self.sessions.insert(key.clone(), session);
        self.active_sessions.remove(&key);
    }

    /// Release a session from the active set without returning it.
    /// Use this when the session was lost (e.g. due to a panic) to prevent
    /// the session from being permanently marked as busy.
    pub fn release_active(&self, session_key: &str) {
        self.active_sessions.remove(session_key);
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
                    estimated_tokens: s.estimate_tokens(),
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
    }

    /// Clear all sessions.
    pub fn clear_all_sessions(&self) {
        self.sessions.clear();
    }

    /// Get detailed info for a specific session (including message history).
    pub fn get_session_detail(&self, session_key: &str) -> Option<SessionDetail> {
        self.sessions.get(session_key).map(|r| {
            let s = r.value();
            SessionDetail {
                session_key: s.session_key.clone(),
                user_id: s.user_id.clone(),
                message_count: s.history.len(),
                created_at: s.created_at,
                last_active_at: s.last_active_at,
                compaction_count: s.compaction_count,
                total_tokens_used: s.total_tokens_used,
                estimated_tokens: s.estimate_tokens(),
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
                        tool_calls: m.tool_calls.as_ref().map(|tc| {
                            tc.iter()
                                .map(|t| format!("{}({})", t.name(), t.arguments()))
                                .collect()
                        }),
                    })
                    .collect(),
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

/// Run a single turn of the agent conversation (backward-compatible wrapper).
///
/// This implements the core turn loop (same pattern as Codex's session/turn.rs):
/// 1. Handle built-in commands (/clear, /reset)
/// 2. Pre-turn compaction check (based on real tokens or estimate)
/// 3. Build prompt with system instructions + history + user message
/// 4. Send to model with available tools
/// 5. If model returns tool_calls → execute → mid-turn compaction check → loop
/// 6. If model returns text → return as final response
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
    mut mcp: Option<&mut McpManager>,
    user_message: &str,
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
        if session.history.len() < 4 {
            return Ok(TurnResult {
                response: "历史消息太少，无需压缩。".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
            });
        }
        match compact::compact_session(
            client,
            config,
            session,
            compact::CompactionTrigger::Manual,
            compact::CompactionReason::UserRequested,
            compact::CompactionPhase::StandaloneTurn,
            compact::InitialContextInjection::DoNotInject,
            vec![],
        )
        .await
        {
            Ok(result) if result.performed => {
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_compaction(
                        &session.session_key,
                        serde_json::json!({
                            "trigger": "manual",
                            "reason": "user_requested",
                            "phase": "standalone_turn",
                            "pre_tokens": result.pre_tokens,
                            "post_tokens": result.post_tokens,
                            "tokens_saved": result.tokens_saved,
                            "messages_removed": result.messages_removed,
                            "compaction_count": session.compaction_count,
                        }),
                    ) {
                        warn!(error = %e, "failed to record compaction event");
                    }
                }
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
            });
        }
        let record = memory::remember_explicit(config, session, args.trim())?;
        return Ok(TurnResult {
            response: format!("已记住长期记忆: {}", record.id),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
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
        let real = session
            .total_tokens_used
            .map(|t| t.to_string())
            .unwrap_or_else(|| "N/A".to_string());
        let mcp_tool_count = mcp.as_ref().map(|m| m.list_tools().len()).unwrap_or(0);
        return Ok(TurnResult {
            response: format!(
                "会话状态:\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- 压缩次数: {}\n- 历史版本: {}\n- MCP 工具数: {}",
                session.history.len(), est, real, session.compaction_count, session.history_version, mcp_tool_count
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
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
            });
        }
        return Ok(TurnResult {
            response: "Skill registry 尚未初始化，无法创建 skill。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
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

    // Pre-turn compaction: check using real tokens if available, else estimate
    if compact::should_compact(session, config) {
        info!(
            session_key = %session.session_key,
            estimated_tokens = session.estimate_tokens(),
            threshold = config.get_compact_threshold_tokens(),
            compaction_count = session.compaction_count,
            "triggering pre-turn compaction"
        );
        match compact::compact_session(
            client,
            config,
            session,
            compact::CompactionTrigger::Auto,
            compact::CompactionReason::ContextLimit,
            compact::CompactionPhase::PreTurn,
            compact::InitialContextInjection::DoNotInject,
            vec![],
        )
        .await
        {
            Ok(result) if result.performed => {
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_compaction(
                        &session.session_key,
                        serde_json::json!({
                            "trigger": "auto",
                            "reason": "context_limit",
                            "phase": "pre_turn",
                            "pre_tokens": result.pre_tokens,
                            "post_tokens": result.post_tokens,
                            "tokens_saved": result.tokens_saved,
                            "messages_removed": result.messages_removed,
                            "compaction_count": session.compaction_count,
                        }),
                    ) {
                        warn!(error = %e, "failed to record compaction event");
                    }
                }
                info!(
                    tokens_saved = result.tokens_saved,
                    duration_ms = ?result.duration_ms,
                    "pre-turn compaction succeeded"
                );
            }
            Ok(_) => {} // skipped
            Err(e) => {
                warn!(error = %e, "pre-turn compaction failed, continuing with full history");
            }
        }
    }

    // Lazily load user instructions on first turn (matching Codex's once-per-session
    // loading). Subsequent turns reuse the cached value.
    if session.user_instructions.is_none() {
        session.load_user_instructions(config);
    }

    // Build system prompt
    // Uses pre-loaded user_instructions from session (loaded once at creation,
    // matching Codex's behavior).
    let system_prompt = prompt::build_system_prompt_with_skill_registry(
        config,
        system_prompt_override,
        session.work_dir.as_deref(),
        session.skill_registry.as_deref(),
        session.user_instructions.as_deref(),
    );

    // Add user message to history
    session.add_user_message(user_message);

    // Record user message
    if let Some(ref mut rec) = recorder {
        if let Err(e) = rec.record_user_message(&session.session_key, user_message) {
            warn!(error = %e, "failed to record user message");
        }
    }

    // Merge tool definitions: local tools + MCP tools
    let mut tool_defs = tools.definitions();
    if let Some(ref mcp_mgr) = mcp {
        tool_defs.extend(mcp_mgr.list_tools());
    }

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
            &system_prompt,
            memory_message.as_ref(),
            config.get_max_history_messages(),
        );

        debug!(
            iteration,
            message_count = messages.len(),
            history_len = session.history.len(),
            "sending model request"
        );

        let response = client.chat_completion(config, &messages, &tool_defs).await;

        // Retry with exponential backoff on transient errors;
        // Codex-style degradation on context window overflow:
        //   1. Emergency compact (best effort — preserves most context)
        //   2. Loop trim oldest messages until within budget
        //   3. Give up only when history is exhausted
        let response = match response {
            Ok(r) => r,
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
                        compact::InitialContextInjection::BeforeLastUserMessage,
                        build_mid_turn_initial_context(session),
                    )
                    .await
                    {
                        Ok(result) if result.performed => {
                            info!(
                                tokens_saved = result.tokens_saved,
                                duration_ms = ?result.duration_ms,
                                "emergency compact succeeded"
                            );
                            session.last_response_tokens = None;
                            true
                        }
                        Ok(_) => {
                            info!("emergency compact skipped (too few messages)");
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
                            &system_prompt,
                            memory_message.as_ref(),
                            config.get_max_history_messages(),
                        );
                        match client
                            .chat_completion(config, &retry_messages, &tool_defs)
                            .await
                        {
                            Ok(r) => r,
                            Err(e2) if is_context_window_error(&e2) => {
                                // Compact wasn't enough, fall through to trim loop
                                warn!("still over context limit after compact, starting trim loop");
                                match trim_loop_retry(
                                    client,
                                    config,
                                    session,
                                    &system_prompt,
                                    &tool_defs,
                                )
                                .await
                                {
                                    Ok(r) => r,
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
                        match trim_loop_retry(client, config, session, &system_prompt, &tool_defs)
                            .await
                        {
                            Ok(r) => r,
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
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        match client.chat_completion(config, &messages, &tool_defs).await {
                            Ok(r) => {
                                info!(retry_attempt = retry + 1, "retry succeeded");
                                succeeded = Some(r);
                                break;
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

        // Track real token usage from API response
        if let Some(ref usage) = response.usage {
            session.track_token_usage(usage.total_tokens);
        }

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
            });
        }

        // Model wants to call tools
        info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "model requested tool calls"
        );

        // Record the assistant message with tool calls
        session.add_assistant_tool_calls(&response.tool_calls);

        // Execute each tool call
        for tc in &response.tool_calls {
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

            let result = if let Some(result) =
                crate::tools::goal::execute_goal_tool(session, tc.name(), tc.arguments())
            {
                result
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
            };

            debug!(
                tool = %tc.name(),
                success = result.success,
                output_len = result.output.len(),
                output_preview = %telemetry_preview(&result.output),
                "tool call completed"
            );

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
                session.work_dir = Some(new_dir.clone());
                session.clear();
                return Ok(TurnResult {
                    response: format!(
                        "已切换工作目录到: {}\n\n会话历史已清空，已重新加载项目配置。",
                        new_dir
                    ),
                    tool_calls_log,
                    work_dir_switched: Some(new_dir),
                    title_updated: None,
                    plan_steps: None,
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
        if compact::should_compact(session, config) {
            info!(
                estimated_tokens = session.estimate_tokens(),
                compaction_count = session.compaction_count,
                "triggering mid-turn compaction"
            );
            match compact::compact_session(
                client,
                config,
                session,
                compact::CompactionTrigger::Auto,
                compact::CompactionReason::ContextLimit,
                compact::CompactionPhase::MidTurn,
                compact::InitialContextInjection::BeforeLastUserMessage,
                build_mid_turn_initial_context(session),
            )
            .await
            {
                Ok(result) if result.performed => {
                    if let Some(ref mut rec) = recorder {
                        if let Err(e) = rec.record_compaction(
                            &session.session_key,
                            serde_json::json!({
                                "trigger": "auto",
                                "reason": "context_limit",
                                "phase": "mid_turn",
                                "pre_tokens": result.pre_tokens,
                                "post_tokens": result.post_tokens,
                                "tokens_saved": result.tokens_saved,
                                "messages_removed": result.messages_removed,
                                "compaction_count": session.compaction_count,
                            }),
                        ) {
                            warn!(error = %e, "failed to record compaction event");
                        }
                    }
                    info!(
                        tokens_saved = result.tokens_saved,
                        duration_ms = ?result.duration_ms,
                        "mid-turn compaction succeeded"
                    );
                    // Reset last_response_tokens so next check uses estimate until new API call
                    session.last_response_tokens = None;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "mid-turn compaction failed");
                }
            }
        }
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
/// continuity within the turn loop. The full system prompt is already
/// prepended by `build_messages()` on every model call, so this provides a
/// lightweight context reminder rather than duplicating the full prompt.
///
/// Extend this when the architecture stores initial context in history
/// (i.e. when system prompt is no longer prepended at `build_messages()` time).
fn build_mid_turn_initial_context(session: &AgentSession) -> Vec<ChatMessage> {
    let work_dir = session.work_dir.as_deref().unwrap_or("(default)");
    vec![ChatMessage::system(&format!(
        "[Context after mid-turn compaction]\n\
         Working directory: {work_dir}\n\
         Session: {source}\n\
         Compaction #{count} — continue following system instructions.",
        source = session.source,
        count = session.compaction_count + 1,
    ))]
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
    system_prompt: &str,
    tool_defs: &[crate::types::ToolDefinition],
) -> Result<crate::types::ModelResponse, String> {
    const MAX_TRIM_ITERATIONS: usize = 10;
    const TRIM_BATCH_SIZE: usize = 4;

    for i in 0..MAX_TRIM_ITERATIONS {
        // Trim a batch of oldest messages
        let trimmed = trim_oldest_messages_count(session, TRIM_BATCH_SIZE);
        if trimmed == 0 {
            return Err("context window exceeded and history exhausted".to_string());
        }

        warn!(
            iteration = i + 1,
            trimmed_count = trimmed,
            history_len = session.history.len(),
            "trimmed oldest messages, retrying"
        );

        let retry_messages =
            session.build_messages(system_prompt, None, config.get_max_history_messages());
        match client
            .chat_completion(config, &retry_messages, tool_defs)
            .await
        {
            Ok(r) => {
                info!(
                    iterations = i + 1,
                    total_trimmed = (i + 1) * TRIM_BATCH_SIZE,
                    "trim loop succeeded"
                );
                return Ok(r);
            }
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
    let start = if session.compaction_count > 0 { 1 } else { 0 };
    let end = (start + count).min(session.history.len());
    if start < end {
        let removed = end - start;
        session.history.drain(start..end);
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
        removed
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCallMessage;

    fn test_tool_call(id: &str) -> ToolCallMessage {
        ToolCallMessage::function_call(
            id.to_string(),
            "list_directory".to_string(),
            r#"{"path":"."}"#.to_string(),
        )
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

        session.track_token_usage(1000);
        assert_eq!(session.total_tokens_used, Some(1000));
        assert_eq!(session.last_response_tokens, Some(1000));

        session.track_token_usage(2000);
        assert_eq!(session.total_tokens_used, Some(3000));
        assert_eq!(session.last_response_tokens, Some(2000));
    }

    #[test]
    fn test_build_messages_with_limit() {
        let mut session = AgentSession::new("test");
        for i in 0..10 {
            session.add_user_message(&format!("msg {i}"));
        }
        // Limit to 5 messages
        let messages = session.build_messages("system", None, 5);
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
        session.add_user_message("SUMMARY: previous context"); // This is the compaction summary
        for i in 0..10 {
            session.add_user_message(&format!("msg {i}"));
        }
        // Limit to 5 messages
        let messages = session.build_messages("system", None, 5);
        // 1 system + 1 summary + 4 recent = 6
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, "system");
        assert_eq!(
            messages[1].content.as_deref(),
            Some("SUMMARY: previous context")
        );
    }

    #[test]
    fn test_build_messages_no_limit() {
        let mut session = AgentSession::new("test");
        for i in 0..5 {
            session.add_user_message(&format!("msg {i}"));
        }
        let messages = session.build_messages("system", None, 0); // 0 = no limit
        assert_eq!(messages.len(), 6); // 1 system + 5 history
    }

    #[test]
    fn test_build_messages_sanitizes_tool_when_history_limit_cuts_assistant_tool_calls() {
        let mut session = AgentSession::new("test");
        session.add_user_message("inspect");
        session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
        session.add_tool_result("call-1", "[file] Cargo.toml");

        let messages = session.build_messages("system", None, 1);

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
        session.add_user_message("SUMMARY");
        session.add_user_message("msg1");
        session.add_assistant_message("reply1");

        // Roll back more turns than exist
        let removed = session.rollback(999);
        assert_eq!(session.history.len(), 1); // Summary preserved
        assert_eq!(session.history[0].content.as_deref(), Some("SUMMARY"));
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
        session.add_user_message("SUMMARY");
        for i in 0..5 {
            session.add_user_message(&format!("msg{i}"));
        }
        let trimmed = trim_oldest_messages_count(&mut session, 2);
        assert_eq!(trimmed, 2);
        assert_eq!(session.history.len(), 4); // summary + 3 remaining
        assert_eq!(session.history[0].content.as_deref(), Some("SUMMARY"));
        assert_eq!(session.history[1].content.as_deref(), Some("msg2"));
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
