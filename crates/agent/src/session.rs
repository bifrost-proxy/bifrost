//! Shared session metadata for external runner execution and history surfaces.

use crate::persistence::ConversationRecorder;
use crate::session_status::{ActiveTurnStatus, ActiveTurnStatusHandle, AgentTurnProgressSender};
use crate::tools::goal::GoalState;
use crate::types::{ChatContentPart, ChatMessage};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub type AgentStopSignalHandle = Arc<AgentStopSignal>;
pub type GuideChannel = Arc<GuideMessageChannel>;

#[derive(Debug, Default)]
pub struct GuideMessageChannel {
    messages: std::sync::Mutex<VecDeque<String>>,
    notify: tokio::sync::Notify,
}

impl GuideMessageChannel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, VecDeque<String>>> {
        self.messages.lock()
    }

    pub fn push_back(&self, message: String) -> usize {
        let len = {
            let mut messages = self.messages.lock().unwrap();
            messages.push_back(message);
            messages.len()
        };
        self.notify.notify_one();
        len
    }

    pub fn has_pending(&self) -> bool {
        self.messages
            .lock()
            .map(|messages| messages.iter().any(|message| !message.trim().is_empty()))
            .unwrap_or(false)
    }

    pub fn drain(&self) -> Vec<String> {
        self.messages
            .lock()
            .map(|mut messages| messages.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.messages
            .lock()
            .map(|messages| messages.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

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

/// Conversation and status state shared by external runner adapters.
pub struct AgentSession {
    pub history: Vec<ChatMessage>,
    pub session_key: String,
    pub user_id: Option<String>,
    pub created_at: u64,
    pub last_active_at: u64,
    pub compaction_count: u32,
    pub total_tokens_used: Option<u64>,
    pub last_response_tokens: Option<u64>,
    pub last_response_history_len: Option<usize>,
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
    pub recorder: Option<ConversationRecorder>,
    pub history_cleared: bool,
    pub title: Option<String>,
    pub current_goal: Option<GoalState>,
    pub active_turn_status: Option<ActiveTurnStatusHandle>,
    pub stop_signal: Option<AgentStopSignalHandle>,
    pub progress_sender: Option<AgentTurnProgressSender>,
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
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            recorder: None,
            history_cleared: false,
            title: None,
            current_goal: None,
            active_turn_status: None,
            stop_signal: None,
            progress_sender: None,
        }
    }

    pub fn new_with_work_dir(session_key: &str, work_dir: Option<String>) -> Self {
        let mut session = Self::new(session_key);
        session.work_dir = work_dir;
        session
    }

    pub fn reinitialize_work_dir(&mut self, work_dir: String) {
        self.work_dir = Some(work_dir);
        self.clear();
    }

    pub fn with_source(mut self, source: &str) -> Self {
        self.source = source.to_string();
        self
    }

    pub fn mark_external_runner_runtime(&mut self, runner_id: &str, adapter: &str) {
        self.agent_type = Some("External Runner Agent".to_string());
        self.runner_type = Some(adapter.to_string());
        self.runner_id = Some(runner_id.to_string());
    }

    pub fn remember_runner_model_config(
        &mut self,
        model: Option<String>,
        model_provider: Option<String>,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
    ) {
        self.model = model;
        self.model_provider = model_provider;
        self.model_reasoning_effort = reasoning_effort;
        self.model_reasoning_summary = reasoning_summary;
    }

    pub fn remember_external_conversation_ref(
        &mut self,
        conversation_id: Option<String>,
        thread_id: Option<String>,
    ) {
        if let Some(value) = conversation_id.filter(|value| !value.trim().is_empty()) {
            self.external_conversation_id = Some(value);
        }
        if let Some(value) = thread_id.filter(|value| !value.trim().is_empty()) {
            self.external_thread_id = Some(value);
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
                let _ = recorder.record_goal_cleared(&self.session_key);
            }
        }
        self.history.clear();
        self.compaction_count = 0;
        self.total_tokens_used = None;
        self.last_response_tokens = None;
        self.last_response_history_len = None;
        self.history_version = self.history_version.saturating_add(1);
        self.current_goal = None;
        self.external_conversation_id = None;
        self.external_thread_id = None;
        self.history_cleared = true;
        self.recorder = None;
    }

    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        current_time_secs().saturating_sub(self.last_active_at) > ttl_secs
    }

    pub fn restore_token_snapshot(&mut self, last_response_tokens: Option<u64>) {
        self.last_response_tokens = last_response_tokens;
        self.last_response_history_len = last_response_tokens.map(|_| self.history.len());
    }

    fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
        let total_chars: usize = messages
            .iter()
            .map(|message| {
                message.content.as_ref().map_or(0, String::len)
                    + message.content_parts.as_ref().map_or(0, |parts| {
                        parts
                            .iter()
                            .map(|part| match part {
                                ChatContentPart::Text { text } => text.len(),
                                ChatContentPart::ImageUrl { image_url } => image_url.url.len(),
                            })
                            .sum::<usize>()
                    })
                    + message.tool_calls.as_ref().map_or(0, |calls| {
                        calls
                            .iter()
                            .map(|call| call.arguments().len() + call.name().len())
                            .sum::<usize>()
                    })
            })
            .sum();
        (total_chars / 4).min(u32::MAX as usize) as u32
    }

    pub fn estimate_tokens(&self) -> u32 {
        Self::estimate_messages_tokens(&self.history)
    }

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

pub fn combine_guide_messages(messages: Vec<String>) -> Option<String> {
    let messages: Vec<String> = messages
        .into_iter()
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .collect();
    match messages.len() {
        0 => None,
        1 => messages.into_iter().next(),
        _ => Some(
            messages
                .iter()
                .enumerate()
                .map(|(index, message)| format!("引导消息 {}:\n{}", index + 1, message))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
    }
}

#[path = "session/session_store.rs"]
mod session_store;
pub use session_store::{
    AgentSessionEvent, AgentSessionManager, SessionDetail, SessionInfo, SessionMessage,
};

#[path = "session/turn_timing.rs"]
pub mod turn_timing;

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
