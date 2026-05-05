//! Goal tracking helpers for multi-turn agent sessions.
//!
//! Goal state must be scoped to a single `AgentSession`, matching Codex's
//! persisted thread-goal semantics instead of living on the shared tool
//! registry.

use crate::session::AgentSession;
use crate::types::{FunctionDefinition, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

pub const GET_GOAL_TOOL_NAME: &str = "get_goal";
pub const CREATE_GOAL_TOOL_NAME: &str = "create_goal";
pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

/// Status of a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Complete,
}

/// User-visible goal snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Session-owned goal runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalState {
    objective: String,
    status: GoalStatus,
    token_budget: Option<u64>,
    created_at: u64,
    updated_at: u64,
    start_total_tokens: u64,
    completed_total_tokens: Option<u64>,
    completed_time_used_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CreateGoalArgs {
    objective: String,
    #[serde(default)]
    token_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UpdateGoalArgs {
    status: GoalStatus,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoalToolResponse {
    goal: Option<Goal>,
    remaining_tokens: Option<i64>,
    completion_budget_report: Option<String>,
}

#[derive(Clone, Copy)]
enum CompletionBudgetReport {
    Include,
    Omit,
}

impl GoalState {
    fn new(objective: String, token_budget: Option<u64>, total_tokens_used: u64, now: u64) -> Self {
        Self {
            objective,
            status: GoalStatus::Active,
            token_budget,
            created_at: now,
            updated_at: now,
            start_total_tokens: total_tokens_used,
            completed_total_tokens: None,
            completed_time_used_seconds: None,
        }
    }

    fn snapshot(&self, total_tokens_used: u64, now: u64) -> Goal {
        let tokens_used = self
            .completed_total_tokens
            .unwrap_or_else(|| total_tokens_used.saturating_sub(self.start_total_tokens));
        let time_used_seconds = self
            .completed_time_used_seconds
            .unwrap_or_else(|| now.saturating_sub(self.created_at));
        Goal {
            objective: self.objective.clone(),
            status: self.status,
            token_budget: self.token_budget,
            tokens_used,
            time_used_seconds,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    fn mark_complete(&mut self, total_tokens_used: u64, now: u64) {
        self.status = GoalStatus::Complete;
        self.updated_at = now;
        self.completed_total_tokens =
            Some(total_tokens_used.saturating_sub(self.start_total_tokens));
        self.completed_time_used_seconds = Some(now.saturating_sub(self.created_at));
    }
}

pub fn goal_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: GET_GOAL_TOOL_NAME.to_string(),
                description: "Get the current goal for this session, including status, budgets, token and elapsed-time usage, and remaining token budget.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                })),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: CREATE_GOAL_TOOL_NAME.to_string(),
                description: format!(
                    "Create a goal only when explicitly requested by the user or system/developer instructions; do not infer goals from ordinary tasks. Set token_budget only when an explicit token budget is requested. Fails if a goal exists; use {UPDATE_GOAL_TOOL_NAME} only for status."
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "objective": {
                            "type": "string",
                            "description": "Required. The concrete objective to start pursuing. This starts a new active goal only when no goal is currently defined; if a goal already exists, this tool fails."
                        },
                        "token_budget": {
                            "type": "integer",
                            "description": "Optional positive token budget for the new active goal."
                        }
                    },
                    "required": ["objective"]
                })),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: UPDATE_GOAL_TOOL_NAME.to_string(),
                description: "Update the existing goal. Use this tool only to mark the goal achieved. Set status to `complete` only when the objective has actually been achieved and no required work remains. Do not mark a goal complete merely because its budget is nearly exhausted or because you are stopping work. You cannot use this tool to pause, resume, or budget-limit a goal; those status changes are controlled by the user or system. When marking a budgeted goal achieved with status `complete`, report the final token usage from the tool result to the user.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["complete"],
                            "description": "Required. Set to complete only when the objective is achieved and no required work remains."
                        }
                    },
                    "required": ["status"]
                })),
            },
        },
    ]
}

pub fn goal_tool_names() -> [&'static str; 3] {
    [
        GET_GOAL_TOOL_NAME,
        CREATE_GOAL_TOOL_NAME,
        UPDATE_GOAL_TOOL_NAME,
    ]
}

pub fn execute_goal_tool(
    session: &mut AgentSession,
    name: &str,
    arguments: &str,
) -> Option<ToolResult> {
    match name {
        GET_GOAL_TOOL_NAME => Some(handle_get_goal(session)),
        CREATE_GOAL_TOOL_NAME => Some(handle_create_goal(session, arguments)),
        UPDATE_GOAL_TOOL_NAME => Some(handle_update_goal(session, arguments)),
        _ => None,
    }
}

fn handle_get_goal(session: &AgentSession) -> ToolResult {
    goal_response(session, CompletionBudgetReport::Omit)
}

fn handle_create_goal(session: &mut AgentSession, arguments: &str) -> ToolResult {
    let args: CreateGoalArgs = match serde_json::from_str(arguments) {
        Ok(args) => args,
        Err(error) => {
            return ToolResult {
                success: false,
                output: format!("invalid arguments: {error}"),
            };
        }
    };

    let objective = args.objective.trim();
    if objective.is_empty() {
        return ToolResult {
            success: false,
            output: "objective cannot be empty".to_string(),
        };
    }

    if session.current_goal.is_some() {
        return ToolResult {
            success: false,
            output: "cannot create a new goal because this session already has a goal; use update_goal only when the existing goal is complete".to_string(),
        };
    }

    let now = current_time_secs();
    let total_tokens_used = session.total_tokens_used.unwrap_or(0);
    session.current_goal = Some(GoalState::new(
        objective.to_string(),
        args.token_budget,
        total_tokens_used,
        now,
    ));

    info!(session_key = %session.session_key, objective, "goal created");
    goal_response(session, CompletionBudgetReport::Omit)
}

fn handle_update_goal(session: &mut AgentSession, arguments: &str) -> ToolResult {
    let args: UpdateGoalArgs = match serde_json::from_str(arguments) {
        Ok(args) => args,
        Err(error) => {
            return ToolResult {
                success: false,
                output: format!("invalid arguments: {error}"),
            };
        }
    };

    if args.status != GoalStatus::Complete {
        return ToolResult {
            success: false,
            output: "update_goal can only mark the existing goal complete; pause, resume, and budget-limited status changes are controlled by the user or system".to_string(),
        };
    }

    let now = current_time_secs();
    let total_tokens_used = session.total_tokens_used.unwrap_or(0);
    let Some(goal) = session.current_goal.as_mut() else {
        return ToolResult {
            success: false,
            output: "no goal exists to update".to_string(),
        };
    };

    goal.mark_complete(total_tokens_used, now);
    info!(
        session_key = %session.session_key,
        objective = %goal.objective,
        "goal marked complete"
    );
    goal_response(session, CompletionBudgetReport::Include)
}

fn goal_response(session: &AgentSession, report_mode: CompletionBudgetReport) -> ToolResult {
    let now = current_time_secs();
    let total_tokens_used = session.total_tokens_used.unwrap_or(0);
    let goal = session
        .current_goal
        .as_ref()
        .map(|goal| goal.snapshot(total_tokens_used, now));
    let remaining_tokens = goal.as_ref().and_then(|goal| {
        goal.token_budget
            .map(|budget| (budget as i64 - goal.tokens_used as i64).max(0))
    });
    let completion_budget_report = match report_mode {
        CompletionBudgetReport::Include => goal
            .as_ref()
            .filter(|goal| goal.status == GoalStatus::Complete)
            .and_then(completion_budget_report),
        CompletionBudgetReport::Omit => None,
    };
    let payload = GoalToolResponse {
        goal,
        remaining_tokens,
        completion_budget_report,
    };

    match serde_json::to_string_pretty(&payload) {
        Ok(output) => ToolResult {
            success: true,
            output,
        },
        Err(error) => {
            warn!(error = %error, "failed to serialize goal response");
            ToolResult {
                success: false,
                output: format!("failed to serialize goal response: {error}"),
            }
        }
    }
}

fn completion_budget_report(goal: &Goal) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(budget) = goal.token_budget {
        parts.push(format!("tokens used: {} of {budget}", goal.tokens_used));
    }
    if goal.time_used_seconds > 0 {
        parts.push(format!("time used: {} seconds", goal.time_used_seconds));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!(
            "Goal achieved. Report final budget usage to the user: {}.",
            parts.join("; ")
        ))
    }
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AgentSession;

    fn make_session() -> AgentSession {
        AgentSession::new("goal-test")
    }

    #[test]
    fn create_goal_success() {
        let mut session = make_session();
        session.total_tokens_used = Some(1_000);

        let result = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"Implement feature X","token_budget":5000}"#,
        )
        .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Implement feature X"));
        assert!(result.output.contains("\"remainingTokens\": 5000"));
        assert!(session.current_goal.is_some());
    }

    #[test]
    fn duplicate_goal_fails() {
        let mut session = make_session();
        let _ = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"First goal"}"#,
        );

        let result = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"Second goal"}"#,
        )
        .unwrap();

        assert!(!result.success);
        assert!(result.output.contains("already has a goal"));
    }

    #[test]
    fn get_goal_when_none_returns_null_goal() {
        let mut session = make_session();
        let result = execute_goal_tool(&mut session, GET_GOAL_TOOL_NAME, "{}").unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"goal\": null"));
        assert!(result.output.contains("\"remainingTokens\": null"));
    }

    #[test]
    fn get_goal_reports_budget_usage() {
        let mut session = make_session();
        session.total_tokens_used = Some(100);
        let _ = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"Test objective","token_budget":1000}"#,
        );
        session.total_tokens_used = Some(325);

        let result = execute_goal_tool(&mut session, GET_GOAL_TOOL_NAME, "{}").unwrap();

        assert!(result.success);
        assert!(result.output.contains("Test objective"));
        assert!(result.output.contains("\"tokensUsed\": 225"));
        assert!(result.output.contains("\"remainingTokens\": 775"));
    }

    #[test]
    fn update_goal_to_complete_reports_completion_budget() {
        let mut session = make_session();
        session.total_tokens_used = Some(10);
        let _ = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"Complete this task","token_budget":100}"#,
        );
        session.total_tokens_used = Some(40);

        let result = execute_goal_tool(
            &mut session,
            UPDATE_GOAL_TOOL_NAME,
            r#"{"status":"complete"}"#,
        )
        .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"status\": \"complete\""));
        assert!(result
            .output
            .contains("Goal achieved. Report final budget usage to the user"));
        assert!(result.output.contains("tokens used: 30 of 100"));
    }

    #[test]
    fn update_goal_without_existing_goal_fails() {
        let mut session = make_session();
        let result = execute_goal_tool(
            &mut session,
            UPDATE_GOAL_TOOL_NAME,
            r#"{"status":"complete"}"#,
        )
        .unwrap();

        assert!(!result.success);
        assert!(result.output.contains("no goal exists"));
    }

    #[test]
    fn update_goal_invalid_status_fails() {
        let mut session = make_session();
        let _ = execute_goal_tool(
            &mut session,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"Some goal"}"#,
        );

        let result = execute_goal_tool(
            &mut session,
            UPDATE_GOAL_TOOL_NAME,
            r#"{"status":"active"}"#,
        )
        .unwrap();

        assert!(!result.success);
        assert!(result
            .output
            .contains("can only mark the existing goal complete"));
    }

    #[test]
    fn goal_state_is_session_scoped() {
        let mut session_a = AgentSession::new("session-a");
        let mut session_b = AgentSession::new("session-b");

        let _ = execute_goal_tool(
            &mut session_a,
            CREATE_GOAL_TOOL_NAME,
            r#"{"objective":"only session a"}"#,
        );

        let result_a = execute_goal_tool(&mut session_a, GET_GOAL_TOOL_NAME, "{}").unwrap();
        let result_b = execute_goal_tool(&mut session_b, GET_GOAL_TOOL_NAME, "{}").unwrap();

        assert!(result_a.output.contains("only session a"));
        assert!(result_b.output.contains("\"goal\": null"));
    }
}
