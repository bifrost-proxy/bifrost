//! Chat Completions API types with tool calling support.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::tools::update_plan::PlanStep;

/// A message in the Chat Completions API format.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,

    pub content: Option<String>,

    /// Optional OpenAI-compatible multimodal content parts.
    ///
    /// When present, `content` is serialized as an array of parts instead of a
    /// plain string. The text is still mirrored in `content` so existing history,
    /// slash commands, memory, and previews keep working as text-first logic.
    pub content_parts: Option<Vec<ChatContentPart>>,

    pub tool_calls: Option<Vec<ToolCallMessage>>,

    pub tool_call_id: Option<String>,

    pub name: Option<String>,
}

impl<'de> Deserialize<'de> for ChatMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            role: String,
            #[serde(default)]
            content: Option<serde_json::Value>,
            #[serde(default)]
            tool_calls: Option<Vec<ToolCallMessage>>,
            #[serde(default)]
            tool_call_id: Option<String>,
            #[serde(default)]
            name: Option<String>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let (content, content_parts) = match wire.content {
            Some(serde_json::Value::String(text)) => (Some(text), None),
            Some(serde_json::Value::Array(parts)) => {
                let parts: Vec<ChatContentPart> =
                    serde_json::from_value(serde_json::Value::Array(parts))
                        .map_err(serde::de::Error::custom)?;
                let text = parts.iter().find_map(|part| match part {
                    ChatContentPart::Text { text } => Some(text.clone()),
                    ChatContentPart::ImageUrl { .. } => None,
                });
                (text, Some(parts))
            }
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "unsupported chat message content: {other}"
                )));
            }
            None => (None, None),
        };

        Ok(Self {
            role: wire.role,
            content,
            content_parts,
            tool_calls: wire.tool_calls,
            tool_call_id: wire.tool_call_id,
            name: wire.name,
        })
    }
}

impl Serialize for ChatMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ChatMessage", 5)?;
        state.serialize_field("role", &self.role)?;
        if let Some(parts) = &self.content_parts {
            state.serialize_field("content", parts)?;
        } else if let Some(content) = &self.content {
            state.serialize_field("content", content)?;
        }
        if let Some(tool_calls) = &self.tool_calls {
            state.serialize_field("tool_calls", tool_calls)?;
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            state.serialize_field("tool_call_id", tool_call_id)?;
        }
        if let Some(name) = &self.name {
            state.serialize_field("name", name)?;
        }
        state.end()
    }
}

/// A user-visible image attached to a chat turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatImageInput {
    pub mime_type: String,
    pub data: String,
}

impl ChatImageInput {
    pub fn data_url(&self) -> String {
        if self.data.starts_with("data:") {
            return self.data.clone();
        }
        format!("data:{};base64,{}", self.mime_type, self.data)
    }
}

/// OpenAI-compatible chat content part.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatContentPart {
    Text { text: String },
    ImageUrl { image_url: ChatImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn developer(content: &str) -> Self {
        Self {
            role: "developer".to_string(),
            content: Some(content.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user_with_images(content: &str, images: &[ChatImageInput]) -> Self {
        if images.is_empty() {
            return Self::user(content);
        }

        let mut parts = Vec::new();
        if !content.trim().is_empty() {
            parts.push(ChatContentPart::Text {
                text: content.to_string(),
            });
        }
        for image in images {
            parts.push(ChatContentPart::ImageUrl {
                image_url: ChatImageUrl {
                    url: image.data_url(),
                    detail: Some("auto".to_string()),
                },
            });
        }

        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            content_parts: Some(parts),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCallMessage>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            content_parts: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            content_parts: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_with_images_serializes_openai_content_parts() {
        let message = ChatMessage::user_with_images(
            "这张图片是什么？",
            &[ChatImageInput {
                mime_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            }],
        );

        let value = serde_json::to_value(&message).expect("serialize message");
        let content = value
            .get("content")
            .and_then(|value| value.as_array())
            .expect("content parts");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "这张图片是什么？");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(message.content.as_deref(), Some("这张图片是什么？"));
    }
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
