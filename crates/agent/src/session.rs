//! Agent session management and core turn loop.
//!
//! Session lifecycle:
//! - Created on first message from a user
//! - Tracks conversation history, token usage, and compaction state
//! - Expired sessions are cleaned up by the session manager
//!
//! Turn loop:
//! 1. Build prompt prefix and run pre-sampling compaction when needed
//! 2. Add the new user message and send to model with available tools
//! 3. If model returns tool_calls or pending input → mid-turn compaction check → loop
//! 4. If model returns text → return as final response

use crate::authorization::ToolOrchestrator;
use crate::auto_compact_window::AutoCompactWindow;
use crate::client::AgentClient;
use crate::compact;
use crate::compaction_hooks::{CompactionHookRegistry, ReferenceContextState};
use crate::config::AgentConfig;
use crate::history;
use crate::mcp::McpManager;
use crate::memory;
use crate::persistence;
use crate::persistence::ConversationRecorder;
use crate::prompt;
use crate::session_status::{
    config_context_window_tokens, context_usage_percent, refresh_active_turn_status,
    ActiveTurnProgress, ActiveTurnStatus, ActiveTurnStatusHandle, AgentTurnProgressEvent,
    AgentTurnProgressSender,
};
use crate::skill_authoring::SkillAuthoringHub;
use crate::slash::{BuiltinCommand, Dispatch, SlashCommandRouter};
use crate::tools::tool_search::{
    parse_loadable_tool_definitions, tool_search_definition, ToolSearchEntry, ToolSearchTool,
    TOOL_SEARCH_TOOL_NAME,
};
use crate::tools::ToolHandler;
use crate::tools::ToolRegistry;
use crate::turn_runtime::{local_tool_supports_parallel, CodexTurnEvent, CodexTurnEventKind};
use crate::types::{ChatImageInput, ChatMessage, ToolCallLog, ToolCallMessage, TurnResult};
use bifrost_skills::{default_roots, SkillRegistry, SkillStore};
use futures::stream::{FuturesOrdered, StreamExt};
use std::collections::{HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// AgentSession
// ---------------------------------------------------------------------------

pub type AgentStopSignalHandle = Arc<AgentStopSignal>;
pub type GuideChannel = Arc<std::sync::Mutex<VecDeque<String>>>;

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

    /// User-visible agent runtime family used by status surfaces.
    pub agent_type: Option<String>,

    /// Runner adapter/runtime label, such as `bifrost_agent`, `codex`, or `chatgpt_web`.
    pub runner_type: Option<String>,

    /// Configured runner id for external runner sessions.
    pub runner_id: Option<String>,

    /// External web runner conversation id, when available.
    pub external_conversation_id: Option<String>,

    /// External Codex runner thread id, when available.
    pub external_thread_id: Option<String>,

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
    pub guide_channel: Option<GuideChannel>,

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

    /// Last turn's runtime event stream.
    ///
    /// The loop records turn phases so tool routing, cancellation, and history
    /// state are observable without scraping logs.
    pub last_turn_events: Vec<CodexTurnEvent>,

    /// Optional provider-neutral progress sink for IM renderers.
    pub progress_sender: Option<AgentTurnProgressSender>,

    /// Auto-compact window tracking for scoped compaction budgets.
    ///
    /// Tracks the current compaction window ordinal and prefill baseline.
    /// Used by `should_compact_with_window()` to compute body-after-prefix.
    pub auto_compact_window: AutoCompactWindow,

    /// Reference context state for incremental context injection.
    ///
    /// Tracks content hashes of injected context fragments to detect changes
    /// and avoid unnecessary full rebuilds.
    pub reference_context_state: ReferenceContextState,

    /// Compaction hook registry for pre/post lifecycle callbacks.
    ///
    /// Hooks are invoked by `compact_session_with_hooks()` and can abort
    /// compaction or signal turn abort.
    pub compaction_hooks: CompactionHookRegistry,

    /// 记忆污染检测器 - 检测外部上下文是否污染了当前 session
    pub pollution_detector: crate::memory::pollution::PollutionDetector,

    /// Citation consumer - tracks memory citations from assistant responses (Codex parity).
    pub citation_consumer: crate::memory::citation_consumer::CitationConsumer,

    /// Steer queue - 允许运行时注入消息到下次 prompt 构建
    pub steer_queue: steer::SteerQueue,

    /// Turn lifecycle hooks registry
    pub hook_registry: hooks::HookRegistry,
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
            agent_type: None,
            runner_type: None,
            runner_id: None,
            external_conversation_id: None,
            external_thread_id: None,
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
            last_turn_events: Vec::new(),
            progress_sender: None,
            auto_compact_window: AutoCompactWindow::new(),
            reference_context_state: ReferenceContextState::new(),
            compaction_hooks: CompactionHookRegistry::new(),
            pollution_detector: crate::memory::pollution::PollutionDetector::default(),
            citation_consumer: crate::memory::citation_consumer::CitationConsumer::new(),
            steer_queue: steer::SteerQueue::new(),
            hook_registry: hooks::HookRegistry::new(),
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
        let turn_events = std::mem::take(&mut self.last_turn_events);
        self.work_dir = Some(work_dir);
        self.clear();
        self.last_turn_events = turn_events;
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

    pub fn mark_bifrost_agent_runtime(&mut self) {
        self.agent_type = Some("Bifrost Agent".to_string());
        self.runner_type = Some("bifrost_agent".to_string());
        self.runner_id = None;
    }

    pub fn mark_external_runner_runtime(&mut self, runner_id: &str, adapter: &str) {
        self.agent_type = Some("External Runner Agent".to_string());
        self.runner_type = Some(adapter.to_string());
        self.runner_id = Some(runner_id.to_string());
    }

    pub fn remember_external_conversation_ref(
        &mut self,
        conversation_id: Option<String>,
        thread_id: Option<String>,
    ) {
        if let Some(conversation_id) = conversation_id.filter(|value| !value.trim().is_empty()) {
            self.external_conversation_id = Some(conversation_id);
        }
        if let Some(thread_id) = thread_id.filter(|value| !value.trim().is_empty()) {
            self.external_thread_id = Some(thread_id);
        }
    }

    pub fn user_turn_count(&self) -> usize {
        self.history
            .iter()
            .filter(|message| message.role == "user")
            .count()
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
        self.external_conversation_id = None;
        self.external_thread_id = None;
        self.plan_repair_attempts = 0;
        // Invalidate cached user instructions so they are reloaded on next turn
        // (e.g. after switch_workdir changes the project context).
        self.user_instructions = None;
        self.resolved_base_instructions = None;
        // Mark that memory should be skipped for the next turn
        self.memory_cleared = true;
        // Drop the recorder so a new file will be created for the fresh session
        self.recorder = None;
        self.last_turn_events.clear();
    }

    pub fn clear_turn_events(&mut self) {
        self.last_turn_events.clear();
    }

    pub fn record_turn_event(&mut self, kind: CodexTurnEventKind) {
        let seq = self.last_turn_events.len() as u64;
        self.last_turn_events.push(CodexTurnEvent::new(seq, kind));
    }

    pub fn record_turn_event_entry(&mut self, mut event: CodexTurnEvent) {
        event.seq = self.last_turn_events.len() as u64;
        self.last_turn_events.push(event);
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

    /// Restore a server-observed context snapshot after replaying persisted history.
    pub fn restore_token_snapshot(&mut self, last_response_tokens: Option<u64>) {
        self.last_response_tokens = last_response_tokens;
        self.last_response_history_len = last_response_tokens.map(|_| self.history.len());
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
    /// messages while preserving the compaction summary wherever replacement
    /// history placed it.
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

#[path = "session/session_store.rs"]
mod session_store;
pub use session_store::{AgentSessionManager, SessionDetail, SessionInfo, SessionMessage};
#[path = "session/pending_input.rs"]
mod pending_input;
#[path = "session/progress.rs"]
mod progress;
#[path = "session/compaction.rs"]
mod session_compaction;
#[path = "session/slash_commands.rs"]
mod slash_commands;
#[path = "session/tool_dispatch.rs"]
mod tool_dispatch;
#[path = "session/turn_loop.rs"]
mod turn_loop;

// --- Agent Loop parity modules (Codex alignment) ---
#[path = "session/hooks.rs"]
pub mod hooks;
#[path = "session/steer.rs"]
pub mod steer;
#[path = "session/task.rs"]
pub mod task;
#[path = "session/turn_context.rs"]
pub mod turn_context;
#[path = "session/turn_timing.rs"]
pub mod turn_timing;

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
fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub use turn_loop::{
    combine_guide_messages, run_turn, run_turn_with_mcp, run_turn_with_mcp_multimodal,
};

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
