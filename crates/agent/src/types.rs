//! Chat Completions API types with tool calling support.

use serde::{Deserialize, Serialize};

use crate::tools::update_plan::PlanStep;

/// A message in the Chat Completions API format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCallMessage>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            name: None,
        }
    }
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallInfo>,
}

/// Function call details within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallInfo {
    pub name: String,
    pub arguments: String,
}

impl ToolCallMessage {
    pub fn function_call(id: String, name: String, arguments: String) -> Self {
        Self {
            id,
            call_type: "function".to_string(),
            function: Some(FunctionCallInfo { name, arguments }),
        }
    }

    pub fn name(&self) -> &str {
        self.function
            .as_ref()
            .map(|function| function.name.as_str())
            .unwrap_or("")
    }

    pub fn arguments(&self) -> &str {
        self.function
            .as_ref()
            .map(|function| function.arguments.as_str())
            .unwrap_or("")
    }
}

/// Tool definition sent to the model in the `tools` parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
}

/// Function schema within a tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl ToolDefinition {
    pub fn function(
        name: String,
        description: String,
        parameters: Option<serde_json::Value>,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: Some(FunctionDefinition {
                name,
                description,
                parameters,
            }),
        }
    }

    pub fn name(&self) -> &str {
        self.function
            .as_ref()
            .map(|function| function.name.as_str())
            .unwrap_or("")
    }
}

/// Parsed model response with tool call support.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Vec<ToolCallMessage>,
    pub finish_reason: String,
    pub usage: Option<TokenUsage>,
}

/// Token usage information from the model response.
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
}

/// Record of a single tool call during a turn (for logging/display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallLog {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub success: bool,
}

/// Result of an agent turn (one complete interaction cycle).
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// The final text response from the assistant.
    pub response: String,
    /// Log of all tool calls made during this turn.
    pub tool_calls_log: Vec<ToolCallLog>,
    /// If the agent switched working directory during this turn.
    /// Contains the new work_dir path.
    pub work_dir_switched: Option<String>,
    /// If the agent updated the session title during this turn.
    pub title_updated: Option<String>,
    /// If the agent updated the task plan during this turn.
    pub plan_steps: Option<Vec<PlanStep>>,
    /// Whether the active goal still needs continuation after this turn.
    pub goal_needs_continuation: bool,
    /// The objective of the active goal, if any.
    pub goal_objective: Option<String>,
}
