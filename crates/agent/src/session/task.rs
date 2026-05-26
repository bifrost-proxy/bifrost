//! SessionTask trait abstraction for Bifrost Agent.
//!
//! Inspired by Codex's task system (`core/src/state/turn.rs`), this module
//! defines a trait-based interface for session tasks, enabling different turn
//! kinds (regular, compact, review, shell command) to share a common lifecycle.

use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;

use super::steer::SteerInput;
use super::turn_context::BifrostTurnContext;
use crate::turn_runtime::CancellationToken;

// ---------------------------------------------------------------------------
// TaskKind
// ---------------------------------------------------------------------------

/// Discriminant for the type of task executing within a session.
///
/// Aligns with Codex's `TaskKind` in `state/turn.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// A normal user-initiated turn.
    Regular,
    /// An automatic or manual compaction task.
    Compact,
    /// A code-review or self-review sub-task.
    Review,
    /// A shell command execution sub-task (e.g. from a goal continuation).
    ShellCommand,
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskKind::Regular => write!(f, "regular"),
            TaskKind::Compact => write!(f, "compact"),
            TaskKind::Review => write!(f, "review"),
            TaskKind::ShellCommand => write!(f, "shell_command"),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskResult
// ---------------------------------------------------------------------------

/// Outcome of a completed session task.
#[derive(Debug, Clone, Default)]
pub struct TaskResult {
    /// Whether the model/task needs a follow-up turn.
    pub needs_follow_up: bool,
    /// Last assistant message produced, if any.
    pub last_agent_message: Option<String>,
    /// Whether the task was cancelled.
    pub was_cancelled: bool,
}

// ---------------------------------------------------------------------------
// SessionTask trait
// ---------------------------------------------------------------------------

/// Trait for executable session tasks.
///
/// Each concrete task type (regular turn, compact, review) implements this
/// trait so the session runtime can manage them uniformly through the same
/// lifecycle hooks and cancellation mechanisms.
#[async_trait]
pub trait SessionTask: Send + Sync + fmt::Debug {
    /// Returns the kind of task.
    fn kind(&self) -> TaskKind;

    /// Unique identifier for this task instance.
    fn task_id(&self) -> &str;

    /// Execute the task using the provided turn context.
    async fn run(&self, ctx: &BifrostTurnContext) -> TaskResult;

    /// Returns the cancellation token associated with this task.
    fn cancellation_token(&self) -> &CancellationToken;

    /// Whether this task type records token usage on the tracing span.
    fn records_turn_token_usage_on_span(&self) -> bool {
        matches!(self.kind(), TaskKind::Regular)
    }

    /// Whether steer input is applicable for this task type.
    fn accepts_steer_input(&self) -> bool {
        matches!(self.kind(), TaskKind::Regular)
    }

    /// Attempt to inject a steer input into this running task.
    /// Default implementation does nothing; tasks that support steering
    /// should override.
    fn inject_steer(&self, _input: SteerInput) {
        // no-op by default
    }
}

/// Type-erased `SessionTask` handle.
pub type AnySessionTask = Arc<dyn SessionTask>;

// ---------------------------------------------------------------------------
// RegularTask (placeholder implementation)
// ---------------------------------------------------------------------------

/// A standard user-initiated turn task.
#[derive(Debug)]
pub struct RegularTask {
    id: String,
    token: CancellationToken,
}

impl RegularTask {
    pub fn new(id: String, token: CancellationToken) -> Self {
        Self { id, token }
    }
}

#[async_trait]
impl SessionTask for RegularTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    fn task_id(&self) -> &str {
        &self.id
    }

    async fn run(&self, _ctx: &BifrostTurnContext) -> TaskResult {
        // The actual turn loop execution is still driven by `turn_loop::run_turn*`.
        // This placeholder enables task lifecycle tracking.
        TaskResult::default()
    }

    fn cancellation_token(&self) -> &CancellationToken {
        &self.token
    }
}

/// A compaction sub-task.
#[derive(Debug)]
pub struct CompactTask {
    id: String,
    token: CancellationToken,
}

impl CompactTask {
    pub fn new(id: String, token: CancellationToken) -> Self {
        Self { id, token }
    }
}

#[async_trait]
impl SessionTask for CompactTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Compact
    }

    fn task_id(&self) -> &str {
        &self.id
    }

    async fn run(&self, _ctx: &BifrostTurnContext) -> TaskResult {
        TaskResult::default()
    }

    fn cancellation_token(&self) -> &CancellationToken {
        &self.token
    }

    fn records_turn_token_usage_on_span(&self) -> bool {
        false
    }
}
