//! Turn runtime primitives.
//!
//! The agent loop is modeled as a stream of turn events and routed tool work.
//! These types keep that runtime shape explicit without coupling it to one
//! provider wire format.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// CancellationToken — two-phase cancellation
// ---------------------------------------------------------------------------

/// Cancellation phase constants.
const PHASE_ACTIVE: u8 = 0;
const PHASE_GRACE: u8 = 1;
const PHASE_ABORT: u8 = 2;

/// Two-phase cancellation token inspired by Codex's use of
/// `tokio_util::sync::CancellationToken`.
///
/// Phase 1 (Grace): The turn is asked to stop; the loop should finish the
/// current tool call or model chunk and then exit cleanly.
///
/// Phase 2 (Abort): The grace period has expired; the turn must terminate
/// immediately (in-flight futures are dropped).
///
/// This replaces the simpler `AgentStopSignal` (AtomicBool + Notify) with
/// richer lifecycle semantics.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

#[derive(Debug)]
struct CancellationInner {
    /// 0 = active, 1 = grace, 2 = abort
    phase: AtomicU8,
    /// Wakes waiters when phase transitions.
    notify: Notify,
    /// Grace period duration before escalating to abort.
    grace_timeout: Duration,
    /// Whether the token has been "used" (any cancel request received).
    cancelled: AtomicBool,
}

impl CancellationToken {
    /// Create a new active cancellation token with the given grace period.
    pub fn new(grace_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(CancellationInner {
                phase: AtomicU8::new(PHASE_ACTIVE),
                notify: Notify::new(),
                grace_timeout,
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    /// Create a token with the default 100ms grace period.
    pub fn default_grace() -> Self {
        Self::new(Duration::from_millis(100))
    }

    /// Request graceful cancellation (phase 1).
    ///
    /// Returns `true` if this call actually initiated cancellation (i.e. was
    /// the first call). A background task is spawned to escalate to abort
    /// after the grace timeout.
    pub fn cancel(&self) -> bool {
        let was_active = self
            .inner
            .phase
            .compare_exchange(
                PHASE_ACTIVE,
                PHASE_GRACE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok();

        if was_active {
            self.inner.cancelled.store(true, Ordering::SeqCst);
            self.inner.notify.notify_waiters();

            // Spawn escalation timer.
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                tokio::time::sleep(inner.grace_timeout).await;
                inner.force_abort();
            });
        }
        was_active
    }

    /// Immediately escalate to abort (phase 2), skipping grace.
    pub fn force_abort(&self) {
        self.inner.force_abort();
    }

    /// Whether any cancellation has been requested (grace or abort).
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Whether the token is in the abort phase.
    pub fn is_aborted(&self) -> bool {
        self.inner.phase.load(Ordering::SeqCst) == PHASE_ABORT
    }

    /// Whether the token is in the grace phase (but not yet aborted).
    pub fn is_grace_period(&self) -> bool {
        self.inner.phase.load(Ordering::SeqCst) == PHASE_GRACE
    }

    /// Current phase as a numeric value (0=active, 1=grace, 2=abort).
    pub fn phase(&self) -> u8 {
        self.inner.phase.load(Ordering::SeqCst)
    }

    /// Wait until the token enters grace or abort phase.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        loop {
            self.inner.notify.notified().await;
            if self.is_cancelled() {
                return;
            }
        }
    }

    /// Wait until the token enters the abort phase specifically.
    pub async fn aborted(&self) {
        if self.is_aborted() {
            return;
        }
        loop {
            self.inner.notify.notified().await;
            if self.is_aborted() {
                return;
            }
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::default_grace()
    }
}

impl CancellationInner {
    fn force_abort(&self) {
        let prev = self.phase.swap(PHASE_ABORT, Ordering::SeqCst);
        if prev != PHASE_ABORT {
            self.cancelled.store(true, Ordering::SeqCst);
            self.notify.notify_waiters();
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCallQueue — streaming tool call accumulation
// ---------------------------------------------------------------------------

/// A pending tool call accumulated from streaming deltas.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    /// The tool call ID from the model response.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// Accumulated JSON arguments (may be partial during streaming).
    pub arguments: String,
    /// Whether the arguments are complete (final delta received).
    pub complete: bool,
}

/// Queue for tool calls received during streaming.
///
/// As the model streams `tool_call` deltas, callers push partial tool calls
/// into this queue. Once a tool call is marked complete, it can be drained
/// for execution — potentially starting execution before the full model
/// response is finished (streaming tool call optimization).
#[derive(Debug, Clone, Default)]
pub struct ToolCallQueue {
    inner: Arc<Mutex<ToolCallQueueInner>>,
}

#[derive(Debug, Default)]
struct ToolCallQueueInner {
    pending: VecDeque<PendingToolCall>,
}

impl ToolCallQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ToolCallQueueInner {
                pending: VecDeque::new(),
            })),
        }
    }

    /// Push or update a tool call. If a call with the same `call_id` exists,
    /// its arguments are appended; otherwise a new entry is created.
    pub fn push_delta(&self, call_id: &str, name: &str, arguments_delta: &str, complete: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(existing) = inner.pending.iter_mut().find(|tc| tc.call_id == call_id) {
                existing.arguments.push_str(arguments_delta);
                if complete {
                    existing.complete = true;
                }
            } else {
                inner.pending.push_back(PendingToolCall {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    arguments: arguments_delta.to_string(),
                    complete,
                });
            }
        }
    }

    /// Drain all completed tool calls (leaving incomplete ones in the queue).
    pub fn drain_completed(&self) -> Vec<PendingToolCall> {
        let mut completed = Vec::new();
        if let Ok(mut inner) = self.inner.lock() {
            let mut remaining = VecDeque::new();
            for tc in inner.pending.drain(..) {
                if tc.complete {
                    completed.push(tc);
                } else {
                    remaining.push_back(tc);
                }
            }
            inner.pending = remaining;
        }
        completed
    }

    /// Drain all tool calls regardless of completion status.
    pub fn drain_all(&self) -> Vec<PendingToolCall> {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    /// Number of pending tool calls (complete and incomplete).
    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.pending.len()).unwrap_or(0)
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of completed tool calls ready for execution.
    pub fn completed_count(&self) -> usize {
        self.inner
            .lock()
            .map(|q| q.pending.iter().filter(|tc| tc.complete).count())
            .unwrap_or(0)
    }

    /// Reset the queue (clear all entries).
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Turn events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexTurnEventKind {
    TurnStarted,
    ModelRequestStarted,
    ModelResponseCompleted,
    AssistantToolCallsRecorded,
    ToolBatchStarted,
    ToolCallStarted,
    ToolCallCompleted,
    DeferredToolLoaded,
    ToolBatchCompleted,
    TurnSuspended,
    LongTaskWatchStarted,
    LongTaskHeartbeat,
    LongTaskOutputAvailable,
    LongTaskExited,
    LongTaskResumeFailed,
    TurnResumed,
    TurnCompleted,
    TurnStopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTurnEvent {
    pub seq: u64,
    pub kind: CodexTurnEventKind,
    pub iteration: Option<u32>,
    pub tool_name: Option<String>,
    pub call_id: Option<String>,
    pub success: Option<bool>,
    pub detail: Option<String>,
}

impl CodexTurnEvent {
    pub fn new(seq: u64, kind: CodexTurnEventKind) -> Self {
        Self {
            seq,
            kind,
            iteration: None,
            tool_name: None,
            call_id: None,
            success: None,
            detail: None,
        }
    }

    pub fn with_iteration(mut self, iteration: u32) -> Self {
        self.iteration = Some(iteration);
        self
    }

    pub fn with_tool(mut self, tool_name: impl Into<String>, call_id: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self.call_id = Some(call_id.into());
        self
    }

    pub fn with_success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Tool execution mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Ordered,
}

pub fn local_tool_execution_mode(tool_name: &str) -> ToolExecutionMode {
    match tool_name {
        // Only explicitly side-effect-free local tools opt into parallelism.
        "read_file" | "list_directory" | "view_image" => ToolExecutionMode::Parallel,
        _ => ToolExecutionMode::Ordered,
    }
}

pub fn local_tool_supports_parallel(tool_name: &str) -> bool {
    matches!(
        local_tool_execution_mode(tool_name),
        ToolExecutionMode::Parallel
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_effect_tools_are_ordered() {
        for name in [
            "exec_command",
            "write_file",
            "apply_patch",
            "switch_workdir",
            "set_title",
            "update_plan",
            "request_user_input",
            "tool_search",
            "write_stdin",
            "get_goal",
            "create_goal",
            "update_goal",
            "send_msg",
            "schedule_create",
            "schedule_update",
            "schedule_delete",
        ] {
            assert_eq!(local_tool_execution_mode(name), ToolExecutionMode::Ordered);
        }
    }

    #[test]
    fn ordinary_local_tools_can_run_in_parallel() {
        assert!(local_tool_supports_parallel("read_file"));
        assert!(local_tool_supports_parallel("list_directory"));
        assert!(!local_tool_supports_parallel("exec_command"));
    }

    #[tokio::test]
    async fn cancellation_token_basic_lifecycle() {
        let token = CancellationToken::new(Duration::from_millis(50));
        assert!(!token.is_cancelled());
        assert!(!token.is_aborted());
        assert_eq!(token.phase(), PHASE_ACTIVE);

        // Cancel should transition to grace.
        assert!(token.cancel());
        assert!(token.is_cancelled());
        assert!(token.is_grace_period());
        assert!(!token.is_aborted());

        // Second cancel should be no-op.
        assert!(!token.cancel());

        // After grace timeout, should escalate to abort.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(token.is_aborted());
    }

    #[tokio::test]
    async fn cancellation_token_force_abort() {
        let token = CancellationToken::default_grace();
        token.force_abort();
        assert!(token.is_cancelled());
        assert!(token.is_aborted());
    }

    #[test]
    fn tool_call_queue_push_and_drain() {
        let queue = ToolCallQueue::new();
        queue.push_delta("call-1", "read_file", r#"{"path":""#, false);
        queue.push_delta("call-1", "read_file", r#"foo.rs"}"#, true);
        queue.push_delta("call-2", "exec_command", r#"{"cmd":"ls"#, false);

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.completed_count(), 1);

        let completed = queue.drain_completed();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].call_id, "call-1");
        assert_eq!(completed[0].arguments, r#"{"path":"foo.rs"}"#);

        // Remaining incomplete call still in queue.
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn tool_call_queue_drain_all() {
        let queue = ToolCallQueue::new();
        queue.push_delta("c1", "tool_a", "args", true);
        queue.push_delta("c2", "tool_b", "partial", false);

        let all = queue.drain_all();
        assert_eq!(all.len(), 2);
        assert!(queue.is_empty());
    }
}
