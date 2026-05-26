//! Session-local compaction helpers.

use crate::config::AgentConfig;
use crate::session_status::ActiveTurnProgress;

pub(super) fn model_response_progress(
    config: &AgentConfig,
    iteration: u32,
    local_tool_count: usize,
    mcp_tool_count: usize,
) -> ActiveTurnProgress {
    ActiveTurnProgress {
        state: "model_response",
        current_loop_iteration: iteration,
        completed_loop_iterations: iteration,
        max_loop_iterations: config.get_max_turn_iterations(),
        local_tool_count,
        mcp_tool_count,
    }
}
