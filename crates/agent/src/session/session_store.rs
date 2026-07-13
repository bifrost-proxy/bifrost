// ---------------------------------------------------------------------------
// AgentSessionManager
// ---------------------------------------------------------------------------

use super::{
    ActiveTurnStatus, ActiveTurnStatusHandle, AgentSession, AgentStopSignal, AgentStopSignalHandle,
};
use crate::types::ChatMessage;
use dashmap::{DashMap, DashSet};
use std::sync::Arc;
use tokio::sync::broadcast;

const SESSION_EVENT_CHANNEL_SIZE: usize = 256;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionEvent {
    pub event_type: String,
    pub session_key: Option<String>,
    pub reason: String,
    pub timestamp_ms: u64,
    pub history_path: Option<String>,
    pub end_index: Option<usize>,
}

/// Manages multiple agent sessions with concurrent access.
pub struct AgentSessionManager {
    sessions: DashMap<String, AgentSession>,
    /// Tracks session keys that are currently being processed by a turn loop.
    active_sessions: DashSet<String>,
    /// Lightweight metadata for sessions checked out by a turn loop.
    active_session_infos: DashMap<String, SessionInfo>,
    active_turn_statuses: DashMap<String, ActiveTurnStatusHandle>,
    active_stop_signals: DashMap<String, AgentStopSignalHandle>,
    session_events: broadcast::Sender<AgentSessionEvent>,
    session_ttl_secs: u64,
}

impl AgentSessionManager {
    pub fn new(session_ttl_secs: u64) -> Self {
        let (session_events, _) = broadcast::channel(SESSION_EVENT_CHANNEL_SIZE);
        Self {
            sessions: DashMap::new(),
            active_sessions: DashSet::new(),
            active_session_infos: DashMap::new(),
            active_turn_statuses: DashMap::new(),
            active_stop_signals: DashMap::new(),
            session_events,
            session_ttl_secs,
        }
    }

    pub fn subscribe_session_events(&self) -> broadcast::Receiver<AgentSessionEvent> {
        self.session_events.subscribe()
    }

    fn emit_session_event(&self, session_key: Option<&str>, reason: &str) {
        let _ = self.session_events.send(AgentSessionEvent {
            event_type: "sessions_changed".to_string(),
            session_key: session_key.map(str::to_string),
            reason: reason.to_string(),
            timestamp_ms: current_time_ms(),
            history_path: None,
            end_index: None,
        });
    }

    pub fn emit_timeline_changed(
        &self,
        session_key: &str,
        history_path: &str,
        end_index: Option<usize>,
        reason: &str,
    ) {
        let _ = self.session_events.send(AgentSessionEvent {
            event_type: "timeline_changed".to_string(),
            session_key: Some(session_key.to_string()),
            reason: reason.to_string(),
            timestamp_ms: current_time_ms(),
            history_path: Some(history_path.to_string()),
            end_index,
        });
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
        self.active_session_infos.insert(
            session_key.to_string(),
            session_info_from_session(session, true),
        );
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
        session.history_cleared = false;
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
        self.emit_session_event(Some(session_key), "started");
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
        self.emit_session_event(Some(session_key), "started");
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
        self.emit_session_event(Some(session_key), "started");
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
        self.emit_session_event(Some(session_key), "started");
        Some(session)
    }

    /// Return a session to the manager after a turn completes.
    /// Also removes the session from the active set.
    pub fn return_session(&self, session: AgentSession) {
        let mut session = session;
        let key = session.session_key.clone();
        session.active_turn_status = None;
        session.stop_signal = None;
        session.progress_sender = None;
        self.sessions.insert(key.clone(), session);
        self.active_sessions.remove(&key);
        self.active_session_infos.remove(&key);
        self.active_turn_statuses.remove(&key);
        self.active_stop_signals.remove(&key);
        self.emit_session_event(Some(&key), "returned");
    }

    /// Release a session from the active set without returning it.
    /// Use this when the session was lost (e.g. due to a panic) to prevent
    /// the session from being permanently marked as busy.
    pub fn release_active(&self, session_key: &str) {
        self.active_sessions.remove(session_key);
        self.active_session_infos.remove(session_key);
        self.active_turn_statuses.remove(session_key);
        self.active_stop_signals.remove(session_key);
        self.emit_session_event(Some(session_key), "released");
    }

    /// Remove expired sessions.
    pub fn cleanup_expired(&self) {
        let ttl = self.session_ttl_secs;
        self.sessions.retain(|_, s| !s.is_expired(ttl));
    }

    /// List active session keys with metadata.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let mut sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|r| session_info_from_session(r.value(), false))
            .collect();
        sessions.extend(
            self.active_session_infos
                .iter()
                .map(|entry| entry.value().clone()),
        );
        sessions
    }

    /// Update the list metadata for a session currently checked out by a turn.
    pub fn update_active_session_preview(
        &self,
        session_key: &str,
        title_fallback: Option<String>,
        work_dir: Option<String>,
        source: Option<String>,
        runner_type: Option<String>,
        runner_id: Option<String>,
    ) {
        let now = current_time_secs();
        let mut info = self
            .active_session_infos
            .get(session_key)
            .map(|entry| entry.value().clone())
            .unwrap_or_else(|| SessionInfo::running_placeholder(session_key, now));
        info.last_active_at = now;
        info.message_count = info.message_count.max(1);
        info.user_turn_count = info.user_turn_count.max(1);
        if info.title.is_none() {
            info.title = title_fallback;
        }
        if work_dir.is_some() {
            info.work_dir = work_dir;
        }
        if let Some(source) = source {
            info.source = source;
        }
        if runner_type.is_some() {
            info.runner_type = runner_type;
        }
        if runner_id.is_some() {
            info.runner_id = runner_id;
        }
        info.running = true;
        self.active_session_infos
            .insert(session_key.to_string(), info);
        self.emit_session_event(Some(session_key), "updated");
    }

    /// Mark an external runner session as active without taking an
    /// `AgentSession` out of the built-in session store.
    pub fn try_start_external_session_preview(
        &self,
        session_key: &str,
        title_fallback: Option<String>,
        work_dir: Option<String>,
        source: Option<String>,
        runner_type: Option<String>,
        runner_id: Option<String>,
    ) -> bool {
        if !self.active_sessions.insert(session_key.to_string()) {
            return false;
        }
        self.update_active_session_preview(
            session_key,
            title_fallback,
            work_dir,
            source,
            runner_type,
            runner_id,
        );
        true
    }

    pub fn clear_active_session_preview(&self, session_key: &str) {
        self.active_session_infos.remove(session_key);
        self.active_sessions.remove(session_key);
        self.emit_session_event(Some(session_key), "cleared_preview");
    }

    /// Clear a specific session.
    pub fn clear_session(&self, session_key: &str) {
        self.sessions.remove(session_key);
        self.active_turn_statuses.remove(session_key);
        self.active_stop_signals.remove(session_key);
        self.active_session_infos.remove(session_key);
        self.active_sessions.remove(session_key);
        self.emit_session_event(Some(session_key), "cleared");
    }

    /// Clear all sessions.
    pub fn clear_all_sessions(&self) {
        self.sessions.clear();
        self.active_turn_statuses.clear();
        self.active_stop_signals.clear();
        self.active_session_infos.clear();
        self.active_sessions.clear();
        self.emit_session_event(None, "cleared_all");
    }

    /// Request cooperative cancellation of the active turn for a session.
    /// Returns true when a running turn was signalled.
    pub fn request_stop(&self, session_key: &str) -> bool {
        let Some(signal) = self.active_stop_signals.get(session_key) else {
            if self.active_turn_statuses.contains_key(session_key) {
                self.release_active(session_key);
                return true;
            }
            return false;
        };
        let requested = signal.request_stop();
        if let Some(status_handle) = self.active_turn_statuses.get(session_key) {
            if let Ok(mut status) = status_handle.lock() {
                status.state = "stopping".to_string();
            }
        }
        self.emit_session_event(Some(session_key), "stop_requested");
        requested || self.active_sessions.contains(session_key)
    }

    pub fn get_active_turn_status(&self, session_key: &str) -> Option<ActiveTurnStatus> {
        self.active_turn_statuses
            .get(session_key)
            .and_then(|handle| handle.lock().ok().map(|status| status.clone()))
    }

    /// Replace the observable active status with a status emitted by an
    /// isolated worker process for the same session.
    pub fn update_active_turn_status_from_worker(&self, status: ActiveTurnStatus) {
        let session_key = status.session_key.clone();
        if let Some(handle) = self.active_turn_statuses.get(&session_key) {
            if let Ok(mut current) = handle.lock() {
                *current = status;
            }
        }
    }

    /// List currently running turn statuses.
    pub fn list_active_turn_statuses(&self) -> Vec<ActiveTurnStatus> {
        self.active_turn_statuses
            .iter()
            .filter_map(|entry| entry.value().lock().ok().map(|status| status.clone()))
            .collect()
    }

    /// Get detailed info for a specific session (including message history).
    pub fn get_session_detail(&self, session_key: &str) -> Option<SessionDetail> {
        self.sessions.get(session_key).map(|r| {
            let s = r.value();
            let history_path = s
                .recorder
                .as_ref()
                .map(|recorder| recorder.file_path().display().to_string());
            let (goal_status, goal_objective) = match &s.current_goal {
                Some(g) => (Some(format!("{:?}", g.status)), Some(g.objective.clone())),
                None => (None, None),
            };
            SessionDetail {
                session_key: s.session_key.clone(),
                user_id: s.user_id.clone(),
                message_count: s.history.len(),
                user_turn_count: s.user_turn_count(),
                created_at: s.created_at,
                last_active_at: s.last_active_at,
                compaction_count: s.compaction_count,
                total_tokens_used: s.total_tokens_used,
                estimated_tokens: s.effective_token_count(),
                history_version: s.history_version,
                work_dir: s.work_dir.clone(),
                source: s.source.clone(),
                agent_type: s.agent_type.clone(),
                runner_type: s.runner_type.clone(),
                runner_id: s.runner_id.clone(),
                model: s.model.clone(),
                model_provider: s.model_provider.clone(),
                model_reasoning_effort: s.model_reasoning_effort.clone(),
                model_reasoning_summary: s.model_reasoning_summary.clone(),
                external_conversation_id: s.external_conversation_id.clone(),
                external_thread_id: s.external_thread_id.clone(),
                metadata: None,
                title: s
                    .title
                    .clone()
                    .or_else(|| first_user_message_preview(&s.history)),
                messages: s
                    .history
                    .iter()
                    .map(|m| SessionMessage {
                        role: m.role.clone(),
                        content: m.content.clone().unwrap_or_default(),
                        timestamp: None,
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
                has_timeline: history_path.is_some(),
                history_path,
                timeline_event_count: 0,
                run_state: "idle".to_string(),
            }
        })
    }
}

/// Session metadata for listing/monitoring.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub session_key: String,
    pub running: bool,
    pub user_id: Option<String>,
    pub message_count: usize,
    pub user_turn_count: usize,
    pub created_at: u64,
    pub last_active_at: u64,
    pub compaction_count: u32,
    pub total_tokens_used: Option<u64>,
    pub estimated_tokens: u32,
    pub history_version: u64,
    pub work_dir: Option<String>,
    pub source: String,
    pub agent_type: Option<String>,
    pub runner_type: Option<String>,
    pub runner_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_reasoning_summary: Option<String>,
    pub external_conversation_id: Option<String>,
    pub external_thread_id: Option<String>,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
    pub history_path: Option<String>,
    pub has_timeline: bool,
    pub timeline_event_count: usize,
    pub run_state: String,
}

impl SessionInfo {
    fn running_placeholder(session_key: &str, now: u64) -> Self {
        Self {
            session_key: session_key.to_string(),
            running: true,
            user_id: None,
            message_count: 0,
            user_turn_count: 0,
            created_at: now,
            last_active_at: now,
            compaction_count: 0,
            total_tokens_used: None,
            estimated_tokens: 0,
            history_version: 0,
            work_dir: None,
            source: "admin-api".to_string(),
            agent_type: None,
            runner_type: None,
            runner_id: None,
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            title: None,
            history_path: None,
            has_timeline: false,
            timeline_event_count: 0,
            run_state: "running".to_string(),
        }
    }
}

fn session_info_from_session(s: &AgentSession, running: bool) -> SessionInfo {
    let history_path = s
        .recorder
        .as_ref()
        .map(|recorder| recorder.file_path().display().to_string());
    SessionInfo {
        session_key: s.session_key.clone(),
        running,
        user_id: s.user_id.clone(),
        message_count: s.history.len(),
        user_turn_count: s.user_turn_count(),
        created_at: s.created_at,
        last_active_at: s.last_active_at,
        compaction_count: s.compaction_count,
        total_tokens_used: s.total_tokens_used,
        estimated_tokens: s.effective_token_count(),
        history_version: s.history_version,
        work_dir: s.work_dir.clone(),
        source: s.source.clone(),
        agent_type: s.agent_type.clone(),
        runner_type: s.runner_type.clone(),
        runner_id: s.runner_id.clone(),
        model: s.model.clone(),
        model_provider: s.model_provider.clone(),
        model_reasoning_effort: s.model_reasoning_effort.clone(),
        model_reasoning_summary: s.model_reasoning_summary.clone(),
        external_conversation_id: s.external_conversation_id.clone(),
        external_thread_id: s.external_thread_id.clone(),
        title: s
            .title
            .clone()
            .or_else(|| first_user_message_preview(&s.history)),
        has_timeline: history_path.is_some(),
        history_path,
        timeline_event_count: 0,
        run_state: if running { "running" } else { "idle" }.to_string(),
    }
}

fn first_user_message_preview(history: &[ChatMessage]) -> Option<String> {
    history
        .iter()
        .find(|message| message.role == "user")
        .and_then(|message| message.content.as_deref())
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(|content| {
            const MAX_CHARS: usize = 80;
            let preview: String = content.chars().take(MAX_CHARS).collect();
            if content.chars().count() > MAX_CHARS {
                format!("{preview}…")
            } else {
                preview
            }
        })
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A single message in session detail view.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
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
    pub user_turn_count: usize,
    pub created_at: u64,
    pub last_active_at: u64,
    pub compaction_count: u32,
    pub total_tokens_used: Option<u64>,
    pub estimated_tokens: u32,
    pub history_version: u64,
    pub work_dir: Option<String>,
    pub source: String,
    pub agent_type: Option<String>,
    pub runner_type: Option<String>,
    pub runner_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_reasoning_summary: Option<String>,
    pub external_conversation_id: Option<String>,
    pub external_thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::BTreeMap<String, String>>,
    /// Session title (intent/topic) set by the agent via set_title tool.
    pub title: Option<String>,
    pub messages: Vec<SessionMessage>,
    /// Current goal status (if any).
    pub goal_status: Option<String>,
    /// Current goal objective (if any).
    pub goal_objective: Option<String>,
    pub history_path: Option<String>,
    pub has_timeline: bool,
    pub timeline_event_count: usize,
    pub run_state: String,
}
