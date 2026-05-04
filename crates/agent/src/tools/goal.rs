//! Goal tracking tool for multi-turn agent sessions.
//!
//! Provides goal lifecycle management: create, query, and complete goals.
//! Goals track the agent's current objective and optional token budget.
//!
//! Tools:
//! - `get_goal`: Query the current goal state
//! - `create_goal`: Start a new goal (fails if one already exists)
//! - `update_goal`: Mark the current goal as complete

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

// ─── Goal Types ──────────────────────────────────────────────────────────────

/// Status of a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Complete,
}

impl std::fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalStatus::Active => write!(f, "active"),
            GoalStatus::Complete => write!(f, "complete"),
        }
    }
}

/// A tracked goal with objective and optional token budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<u64>,
}

// ─── GoalManager ─────────────────────────────────────────────────────────────

/// Thread-safe manager for the current session goal.
pub struct GoalManager {
    goal: Mutex<Option<Goal>>,
}

impl GoalManager {
    /// Create a new GoalManager with no active goal.
    pub fn new() -> Self {
        Self {
            goal: Mutex::new(None),
        }
    }

    /// Get the current goal, if any.
    pub fn get(&self) -> Option<Goal> {
        self.goal.lock().unwrap().clone()
    }

    /// Create a new goal. Fails if a goal already exists.
    pub fn create(&self, objective: String, token_budget: Option<u64>) -> Result<Goal, String> {
        let mut guard = self.goal.lock().unwrap();
        if guard.is_some() {
            return Err(
                "cannot create a new goal because one already exists; use update_goal to mark it complete first"
                    .to_string(),
            );
        }
        let goal = Goal {
            objective,
            status: GoalStatus::Active,
            token_budget,
        };
        *guard = Some(goal.clone());
        Ok(goal)
    }

    /// Update the goal status. Only allows setting to Complete.
    /// Fails if no goal exists.
    pub fn update_status(&self, status: GoalStatus) -> Result<Goal, String> {
        let mut guard = self.goal.lock().unwrap();
        match guard.as_mut() {
            None => Err("no goal exists to update".to_string()),
            Some(goal) => {
                if status != GoalStatus::Complete {
                    return Err("update_goal can only mark the goal as complete".to_string());
                }
                goal.status = GoalStatus::Complete;
                Ok(goal.clone())
            }
        }
    }
}

impl Default for GoalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── GetGoalTool ─────────────────────────────────────────────────────────────

/// Tool to query the current goal state.
pub struct GetGoalTool {
    manager: Arc<GoalManager>,
}

impl GetGoalTool {
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolHandler for GetGoalTool {
    fn name(&self) -> &str {
        "get_goal"
    }

    fn description(&self) -> &str {
        "Get the current goal for this session, including status, objective, and token budget."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _arguments: &str, _work_dir: &Path) -> ToolResult {
        match self.manager.get() {
            Some(goal) => {
                let output = serde_json::to_string_pretty(&goal).unwrap_or_else(|e| {
                    warn!(error = %e, "failed to serialize goal");
                    format!("{{\"error\": \"serialization failed: {e}\"}}")
                });
                ToolResult {
                    success: true,
                    output,
                }
            }
            None => ToolResult {
                success: true,
                output: "no goal set".to_string(),
            },
        }
    }
}

// ─── CreateGoalTool ──────────────────────────────────────────────────────────

/// Arguments for create_goal.
#[derive(Debug, Deserialize)]
struct CreateGoalArgs {
    objective: String,
    #[serde(default)]
    token_budget: Option<u64>,
}

/// Tool to create a new goal.
pub struct CreateGoalTool {
    manager: Arc<GoalManager>,
}

impl CreateGoalTool {
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolHandler for CreateGoalTool {
    fn name(&self) -> &str {
        "create_goal"
    }

    fn description(&self) -> &str {
        "Create a goal only when explicitly requested. Set token_budget only when an explicit \
         token budget is requested. Fails if a goal already exists; use update_goal to mark \
         the existing goal as complete first."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "objective": {
                    "type": "string",
                    "description": "The concrete objective to pursue"
                },
                "token_budget": {
                    "type": "integer",
                    "description": "Optional positive token budget for this goal"
                }
            },
            "required": ["objective"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: CreateGoalArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                };
            }
        };

        if args.objective.is_empty() {
            return ToolResult {
                success: false,
                output: "objective cannot be empty".to_string(),
            };
        }

        match self.manager.create(args.objective, args.token_budget) {
            Ok(goal) => {
                info!(objective = %goal.objective, "goal created");
                let output = serde_json::to_string_pretty(&goal).unwrap_or_else(|e| {
                    warn!(error = %e, "failed to serialize goal");
                    format!("{{\"error\": \"serialization failed: {e}\"}}")
                });
                ToolResult {
                    success: true,
                    output,
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: e,
            },
        }
    }
}

// ─── UpdateGoalTool ──────────────────────────────────────────────────────────

/// Arguments for update_goal.
#[derive(Debug, Deserialize)]
struct UpdateGoalArgs {
    status: String,
}

/// Tool to update the current goal status (only "complete" is valid).
pub struct UpdateGoalTool {
    manager: Arc<GoalManager>,
}

impl UpdateGoalTool {
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolHandler for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        "Update the existing goal. Use this tool only to mark the goal as achieved. \
         Set status to \"complete\" only when the objective has actually been achieved \
         and no required work remains."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["complete"],
                    "description": "Set to 'complete' when the objective is achieved"
                }
            },
            "required": ["status"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: UpdateGoalArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                };
            }
        };

        if args.status != "complete" {
            return ToolResult {
                success: false,
                output: format!(
                    "invalid status \"{}\": only \"complete\" is allowed",
                    args.status
                ),
            };
        }

        match self.manager.update_status(GoalStatus::Complete) {
            Ok(goal) => {
                info!(objective = %goal.objective, "goal marked complete");
                let output = serde_json::to_string_pretty(&goal).unwrap_or_else(|e| {
                    warn!(error = %e, "failed to serialize goal");
                    format!("{{\"error\": \"serialization failed: {e}\"}}")
                });
                ToolResult {
                    success: true,
                    output,
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: e,
            },
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> Arc<GoalManager> {
        Arc::new(GoalManager::new())
    }

    #[tokio::test]
    async fn test_create_goal_success() {
        let manager = make_manager();
        let tool = CreateGoalTool::new(manager.clone());
        let args = r#"{"objective": "Implement feature X", "token_budget": 5000}"#;
        let result = tool.execute(args, Path::new("/tmp")).await;

        assert!(result.success);
        assert!(result.output.contains("Implement feature X"));
        assert!(result.output.contains("5000"));

        let goal = manager.get().unwrap();
        assert_eq!(goal.objective, "Implement feature X");
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.token_budget, Some(5000));
    }

    #[tokio::test]
    async fn test_create_goal_duplicate_fails() {
        let manager = make_manager();
        let tool = CreateGoalTool::new(manager.clone());
        let args = r#"{"objective": "First goal"}"#;

        let result = tool.execute(args, Path::new("/tmp")).await;
        assert!(result.success);

        let result = tool.execute(args, Path::new("/tmp")).await;
        assert!(!result.success);
        assert!(result.output.contains("already exists"));
    }

    #[tokio::test]
    async fn test_get_goal_when_none() {
        let manager = make_manager();
        let tool = GetGoalTool::new(manager);
        let result = tool.execute("{}", Path::new("/tmp")).await;

        assert!(result.success);
        assert_eq!(result.output, "no goal set");
    }

    #[tokio::test]
    async fn test_get_goal_after_create() {
        let manager = make_manager();
        manager
            .create("Test objective".to_string(), Some(1000))
            .unwrap();

        let tool = GetGoalTool::new(manager);
        let result = tool.execute("{}", Path::new("/tmp")).await;

        assert!(result.success);
        assert!(result.output.contains("Test objective"));
        assert!(result.output.contains("active"));
        assert!(result.output.contains("1000"));
    }

    #[tokio::test]
    async fn test_update_goal_to_complete() {
        let manager = make_manager();
        manager
            .create("Complete this task".to_string(), None)
            .unwrap();

        let tool = UpdateGoalTool::new(manager.clone());
        let args = r#"{"status": "complete"}"#;
        let result = tool.execute(args, Path::new("/tmp")).await;

        assert!(result.success);
        assert!(result.output.contains("complete"));

        let goal = manager.get().unwrap();
        assert_eq!(goal.status, GoalStatus::Complete);
    }

    #[tokio::test]
    async fn test_update_goal_no_goal_fails() {
        let manager = make_manager();
        let tool = UpdateGoalTool::new(manager);
        let args = r#"{"status": "complete"}"#;
        let result = tool.execute(args, Path::new("/tmp")).await;

        assert!(!result.success);
        assert!(result.output.contains("no goal exists"));
    }

    #[tokio::test]
    async fn test_update_goal_invalid_status() {
        let manager = make_manager();
        manager.create("Some goal".to_string(), None).unwrap();

        let tool = UpdateGoalTool::new(manager);
        let args = r#"{"status": "paused"}"#;
        let result = tool.execute(args, Path::new("/tmp")).await;

        assert!(!result.success);
        assert!(result.output.contains("invalid status"));
        assert!(result.output.contains("only \"complete\" is allowed"));
    }

    #[tokio::test]
    async fn test_goal_manager_thread_safety() {
        let manager = make_manager();
        let mut handles = vec![];

        // Spawn multiple tasks that try to create goals concurrently.
        // Only one should succeed.
        for i in 0..10 {
            let mgr = manager.clone();
            let handle = tokio::spawn(async move { mgr.create(format!("goal {i}"), None) });
            handles.push(handle);
        }

        let mut success_count = 0;
        for handle in handles {
            if handle.await.unwrap().is_ok() {
                success_count += 1;
            }
        }

        assert_eq!(success_count, 1, "exactly one create should succeed");
        assert!(manager.get().is_some());
    }
}
