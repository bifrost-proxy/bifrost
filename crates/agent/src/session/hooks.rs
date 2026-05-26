//! Turn Lifecycle Hooks for Bifrost Agent.
//!
//! Provides extension points at key moments of the turn lifecycle:
//! - `on_turn_start`: called when a new turn begins
//! - `on_turn_complete`: called when a turn finishes successfully
//! - `on_turn_abort`: called when a turn is cancelled or errors out
//!
//! Modeled after Codex's lifecycle event emission pattern but expressed as
//! a composable trait so external integrations (IM adapters, telemetry,
//! persistence) can hook into the turn without coupling to the loop itself.

use async_trait::async_trait;
use std::sync::Arc;

use super::task::TaskKind;
use super::turn_context::BifrostTurnContext;
use super::turn_timing::TurnTimingSummary;

// ---------------------------------------------------------------------------
// TurnOutcome
// ---------------------------------------------------------------------------

/// Describes how a turn ended.
#[derive(Debug, Clone)]
pub enum TurnOutcome {
    /// Turn completed successfully with an optional final message.
    Completed {
        last_message: Option<String>,
        needs_follow_up: bool,
    },
    /// Turn was cancelled (user-initiated or timeout).
    Cancelled { reason: String },
    /// Turn failed with an error.
    Failed { error: String },
}

// ---------------------------------------------------------------------------
// TurnHook trait
// ---------------------------------------------------------------------------

/// Trait for receiving turn lifecycle callbacks.
///
/// Implementations can perform side effects (logging, telemetry, IM updates)
/// without blocking the turn loop. All methods have default no-op implementations
/// so hooks only need to override what they care about.
#[async_trait]
pub trait TurnHook: Send + Sync + std::fmt::Debug {
    /// Called at the very beginning of a turn, after the TurnContext is created.
    async fn on_turn_start(&self, _ctx: &BifrostTurnContext, _task_kind: TaskKind) {}

    /// Called when a turn completes (success or follow-up needed).
    async fn on_turn_complete(
        &self,
        _ctx: &BifrostTurnContext,
        _outcome: &TurnOutcome,
        _timing: &TurnTimingSummary,
    ) {
    }

    /// Called when a turn is aborted (cancelled or errored).
    async fn on_turn_abort(
        &self,
        _ctx: &BifrostTurnContext,
        _outcome: &TurnOutcome,
        _timing: &TurnTimingSummary,
    ) {
    }
}

/// Type-erased hook handle.
pub type AnyTurnHook = Arc<dyn TurnHook>;

// ---------------------------------------------------------------------------
// HookRegistry
// ---------------------------------------------------------------------------

/// Registry that manages multiple turn hooks and dispatches lifecycle events.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    hooks: Vec<AnyTurnHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a new hook.
    pub fn register(&mut self, hook: AnyTurnHook) {
        self.hooks.push(hook);
    }

    /// Dispatch `on_turn_start` to all registered hooks.
    pub async fn dispatch_turn_start(&self, ctx: &BifrostTurnContext, task_kind: TaskKind) {
        for hook in &self.hooks {
            hook.on_turn_start(ctx, task_kind).await;
        }
    }

    /// Dispatch `on_turn_complete` to all registered hooks.
    pub async fn dispatch_turn_complete(
        &self,
        ctx: &BifrostTurnContext,
        outcome: &TurnOutcome,
        timing: &TurnTimingSummary,
    ) {
        for hook in &self.hooks {
            hook.on_turn_complete(ctx, outcome, timing).await;
        }
    }

    /// Dispatch `on_turn_abort` to all registered hooks.
    pub async fn dispatch_turn_abort(
        &self,
        ctx: &BifrostTurnContext,
        outcome: &TurnOutcome,
        timing: &TurnTimingSummary,
    ) {
        for hook in &self.hooks {
            hook.on_turn_abort(ctx, outcome, timing).await;
        }
    }

    /// Returns the number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether no hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Built-in hooks
// ---------------------------------------------------------------------------

/// A tracing-based hook that emits structured log events for each lifecycle point.
#[derive(Debug)]
pub struct TracingTurnHook;

#[async_trait]
impl TurnHook for TracingTurnHook {
    async fn on_turn_start(&self, ctx: &BifrostTurnContext, task_kind: TaskKind) {
        tracing::info!(
            turn_id = %ctx.turn_id,
            model = %ctx.model,
            task_kind = %task_kind,
            "turn started"
        );
    }

    async fn on_turn_complete(
        &self,
        ctx: &BifrostTurnContext,
        outcome: &TurnOutcome,
        timing: &TurnTimingSummary,
    ) {
        tracing::info!(
            turn_id = %ctx.turn_id,
            ttft_ms = ?timing.ttft_ms,
            ttfm_ms = ?timing.ttfm_ms,
            total_ms = ?timing.total_ms,
            outcome = ?outcome,
            "turn completed"
        );
    }

    async fn on_turn_abort(
        &self,
        ctx: &BifrostTurnContext,
        outcome: &TurnOutcome,
        timing: &TurnTimingSummary,
    ) {
        tracing::warn!(
            turn_id = %ctx.turn_id,
            total_ms = ?timing.total_ms,
            outcome = ?outcome,
            "turn aborted"
        );
    }
}
