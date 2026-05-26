//! Per-turn configuration snapshot for Bifrost Agent.
//!
//! Inspired by Codex's `TurnContext` (~100 fields), this provides an immutable
//! snapshot of configuration relevant to a single turn execution. Created at
//! turn start and passed through the turn loop, ensuring mid-turn config
//! changes don't affect an in-progress turn.

use std::sync::Arc;
use std::time::Instant;

use crate::turn_runtime::CancellationToken;

use super::turn_timing::TurnTimingState;

// ---------------------------------------------------------------------------
// BifrostTurnContext
// ---------------------------------------------------------------------------

/// Immutable per-turn context snapshot.
///
/// Created once at the start of each turn and threaded through the turn loop.
/// This decouples the turn execution from mutable session/config state, aligning
/// with Codex's `TurnContext` pattern.
#[derive(Debug, Clone)]
pub struct BifrostTurnContext {
    /// Unique identifier for this turn (UUID v4).
    pub turn_id: String,

    /// Model identifier used for this turn (e.g. "gpt-4o", "o3-mini").
    pub model: String,

    /// Optional reasoning effort configuration (e.g. "low", "medium", "high").
    pub reasoning_effort: Option<String>,

    /// Developer/system instructions to prepend.
    pub developer_instructions: Option<String>,

    /// Maximum number of loop iterations for this turn.
    pub max_iterations: u32,

    /// Maximum token budget for model output.
    pub max_output_tokens: Option<u32>,

    /// Temperature for sampling.
    pub temperature: Option<f64>,

    /// Whether verbose logging is enabled for this turn.
    pub verbose_logging: bool,

    /// The cancellation token governing this turn's lifecycle.
    pub cancellation_token: CancellationToken,

    /// Turn timing state for TTFT/TTFM metrics.
    pub turn_timing: Arc<TurnTimingState>,

    /// Monotonic instant when this turn started.
    pub started_at: Instant,

    /// Features/flags active for this turn.
    pub features: TurnFeatures,
}

impl BifrostTurnContext {
    /// Check if the turn has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Wait until the turn is cancelled.
    pub async fn cancelled(&self) {
        self.cancellation_token.cancelled().await;
    }
}

// ---------------------------------------------------------------------------
// TurnFeatures
// ---------------------------------------------------------------------------

/// Feature flags active for a particular turn.
///
/// These are snapshotted from the session/config at turn start so the turn
/// loop has a stable view of enabled features.
#[derive(Debug, Clone, Default)]
pub struct TurnFeatures {
    /// Whether MCP tool calls are enabled.
    pub mcp_enabled: bool,

    /// Whether streaming responses are enabled.
    pub streaming_enabled: bool,

    /// Whether mid-turn compaction is enabled.
    pub mid_turn_compaction: bool,

    /// Whether tool search (dynamic tool loading) is enabled.
    pub tool_search_enabled: bool,

    /// Whether multimodal (image) input is supported.
    pub multimodal_enabled: bool,
}
