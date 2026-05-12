//! Turn runtime primitives.
//!
//! The agent loop is modeled as a stream of turn events and routed tool work.
//! These types keep that runtime shape explicit without coupling it to one
//! provider wire format.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Ordered,
}

pub fn local_tool_execution_mode(tool_name: &str) -> ToolExecutionMode {
    match tool_name {
        // These tools mutate session-level state, depend on an immediately
        // visible runtime side effect, or alter the visible tool set.
        "switch_workdir" | "set_title" | "update_plan" | "request_user_input" | "tool_search"
        | "write_stdin" | "get_goal" | "create_goal" | "update_goal" => ToolExecutionMode::Ordered,
        _ => ToolExecutionMode::Parallel,
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
    fn stateful_tools_are_ordered() {
        for name in [
            "switch_workdir",
            "set_title",
            "update_plan",
            "tool_search",
            "write_stdin",
            "get_goal",
            "create_goal",
            "update_goal",
        ] {
            assert_eq!(local_tool_execution_mode(name), ToolExecutionMode::Ordered);
        }
    }

    #[test]
    fn ordinary_local_tools_can_run_in_parallel() {
        assert!(local_tool_supports_parallel("read_file"));
        assert!(local_tool_supports_parallel("exec_command"));
    }
}
