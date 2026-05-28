//! Update plan tool (TODO/checklist).
//!
//! Allows the agent to maintain a structured task plan with step status tracking.
//! The plan is rendered as a progress card in Feishu IM to show task progress to users.
//!
//! This is a typed runtime-event tool: the model-visible output is a short
//! confirmation, while the parsed plan is returned as a `ToolRuntimeEvent` for
//! the turn loop to apply to `TurnResult.plan_steps` and progress displays.

use crate::tools::ToolHandler;
use crate::types::{ToolResult, ToolRuntimeEvent};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Status of a plan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for PlanStepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanStepStatus::Pending => write!(f, "pending"),
            PlanStepStatus::InProgress => write!(f, "in_progress"),
            PlanStepStatus::Completed => write!(f, "completed"),
        }
    }
}

impl std::str::FromStr for PlanStepStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(PlanStepStatus::Pending),
            "in_progress" => Ok(PlanStepStatus::InProgress),
            "completed" => Ok(PlanStepStatus::Completed),
            _ => Err(format!("invalid plan step status: {}", s)),
        }
    }
}

impl PlanStepStatus {
    /// Returns the emoji icon for this status.
    pub fn emoji(&self) -> &'static str {
        match self {
            PlanStepStatus::Pending => "⏳",
            PlanStepStatus::InProgress => "🔄",
            PlanStepStatus::Completed => "✅",
        }
    }
}

/// A single step in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step description.
    pub step: String,
    /// Current status of this step.
    pub status: PlanStepStatus,
}

/// Arguments for the update_plan tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlanArgs {
    /// Optional explanation for the plan update.
    #[serde(default)]
    pub explanation: Option<String>,
    /// List of plan steps.
    pub plan: Vec<PlanStep>,
}

/// Tool that updates the task plan.
pub struct UpdatePlanTool;

#[async_trait]
impl ToolHandler for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        "Update the task plan (TODO/checklist) to track progress on multi-step tasks. \
         Each step has a status: pending, in_progress, or completed. \
         At most one step should be in_progress at a time. \
         Send the complete current plan snapshot; use an empty plan to clear the current plan. \
         The plan is displayed to the user as a progress card."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional explanation for the plan update"
                },
                "plan": {
                    "type": "array",
                    "description": "Complete current plan snapshot. Use an empty list to clear the current plan when no task plan applies. One or more steps may be in_progress when work is genuinely concurrent.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {
                                "type": "string",
                                "description": "Step description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Step status"
                            }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args = match parse_update_plan_arguments(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: e,
                    runtime_events: Vec::new(),
                }
            }
        };

        // Validate at most one in_progress step
        let in_progress_count = args
            .plan
            .iter()
            .filter(|s| s.status == PlanStepStatus::InProgress)
            .count();
        if in_progress_count > 1 {
            return ToolResult {
                success: false,
                output: "at most one step can be in_progress at a time".to_string(),
                runtime_events: Vec::new(),
            };
        }

        ToolResult {
            success: true,
            output: "Plan updated".to_string(),
            runtime_events: vec![ToolRuntimeEvent::PlanUpdate(args)],
        }
    }
}

pub fn parse_update_plan_arguments(arguments: &str) -> Result<UpdatePlanArgs, String> {
    serde_json::from_str(arguments).map_err(|e| format!("invalid arguments: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_arguments() {
        let json = r#"{"plan":[{"step":"Read file","status":"completed"},{"step":"Edit file","status":"in_progress"}]}"#;
        let args: UpdatePlanArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.plan.len(), 2);
        assert_eq!(args.plan[0].status, PlanStepStatus::Completed);
        assert_eq!(args.plan[1].status, PlanStepStatus::InProgress);
    }

    #[test]
    fn test_with_explanation() {
        let json = r#"{"explanation":"Starting implementation","plan":[{"step":"Setup","status":"in_progress"}]}"#;
        let args: UpdatePlanArgs = serde_json::from_str(json).unwrap();
        assert_eq!(
            args.explanation,
            Some("Starting implementation".to_string())
        );
    }

    #[test]
    fn test_invalid_status() {
        let json = r#"{"plan":[{"step":"Test","status":"invalid"}]}"#;
        let result: Result<UpdatePlanArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_in_progress_rejected() {
        let tool = UpdatePlanTool;
        let json =
            r#"{"plan":[{"step":"A","status":"in_progress"},{"step":"B","status":"in_progress"}]}"#;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(json, std::path::Path::new("/tmp")));
        assert!(!result.success);
        assert!(result
            .output
            .contains("at most one step can be in_progress"));
    }

    #[test]
    fn test_empty_plan_allowed_as_clear_snapshot() {
        let tool = UpdatePlanTool;
        let json = r#"{"plan":[]}"#;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(json, std::path::Path::new("/tmp")));
        assert!(result.success);
        assert_eq!(result.output, "Plan updated");
        assert_eq!(result.runtime_events.len(), 1);
        match &result.runtime_events[0] {
            ToolRuntimeEvent::PlanUpdate(args) => {
                assert!(args.plan.is_empty());
                assert!(args.explanation.is_none());
            }
        }
    }

    #[test]
    fn test_update_plan_returns_runtime_event() {
        let tool = UpdatePlanTool;
        let json = r#"{"plan":[{"step":"Task","status":"pending"}]}"#;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(json, std::path::Path::new("/tmp")));
        assert!(result.success);
        assert_eq!(result.output, "Plan updated");
        assert_eq!(result.runtime_events.len(), 1);
        match &result.runtime_events[0] {
            ToolRuntimeEvent::PlanUpdate(args) => {
                assert_eq!(args.plan.len(), 1);
                assert_eq!(args.plan[0].step, "Task");
                assert_eq!(args.plan[0].status, PlanStepStatus::Pending);
            }
        }
    }
}
