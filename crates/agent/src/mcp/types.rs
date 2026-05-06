//! Enhanced MCP protocol types for Bifrost Agent.
//!
//! These types mirror the Model Context Protocol specification and are inspired
//! by the Codex protocol types (`codex-rs/protocol/src/mcp.rs`). They provide
//! full metadata support for tools, resources, content, elicitation, and
//! pagination.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Tool types
// ---------------------------------------------------------------------------

/// MCP Tool definition with full metadata support.
///
/// Corresponds to the MCP spec's `Tool` object with extensions for title,
/// output schema, annotations, and icons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<ToolIcon>>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Annotations describing tool behavior characteristics.
///
/// Used by clients to make approval/safety decisions without requiring
/// explicit per-tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
}

/// Icon associated with a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIcon {
    pub r#type: String,
    pub url: String,
}

impl McpTool {
    /// Check if tool is destructive based on annotations.
    pub fn is_destructive(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.destructive_hint)
            .unwrap_or(false)
    }

    /// Check if tool is read-only based on annotations.
    pub fn is_read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }

    /// Check if tool accesses external network.
    pub fn is_open_world(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.open_world_hint)
            .unwrap_or(false)
    }

    /// Check if the tool is idempotent.
    pub fn is_idempotent(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.idempotent_hint)
            .unwrap_or(false)
    }

    /// Deserialize from a raw MCP JSON value (tolerant of field naming variants).
    pub fn from_mcp_value(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

// ---------------------------------------------------------------------------
// Resource types
// ---------------------------------------------------------------------------

/// A known resource that the server is capable of reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Value>>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// A template description for resources available on the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceTemplate {
    pub uri_template: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
}

/// Contents returned when reading a resource from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpResourceContent {
    #[serde(rename_all = "camelCase")]
    Text {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    Blob {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        blob: String,
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Value>,
    },
}

impl McpResource {
    /// Deserialize from a raw MCP JSON value.
    pub fn from_mcp_value(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

impl McpResourceTemplate {
    /// Deserialize from a raw MCP JSON value.
    pub fn from_mcp_value(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

// ---------------------------------------------------------------------------
// Content and CallToolResult types
// ---------------------------------------------------------------------------

/// The server's response to a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallToolResult {
    pub content: Vec<McpContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Content items within a tool result or message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: McpResourceContent },
}

impl McpContent {
    /// Extract text content if this is a text variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            McpContent::Text { text } => Some(text),
            _ => None,
        }
    }
}

impl McpCallToolResult {
    /// Concatenate all text content into a single string.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.as_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether this result represents an error.
    pub fn is_error(&self) -> bool {
        self.is_error.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Elicitation types
// ---------------------------------------------------------------------------

/// A request from the server for user input (elicitation).
///
/// Tagged enum with two modes matching MCP spec:
/// - `Form`: Schema-based input request with optional JSON schema.
/// - `Url`: URL-based input request (e.g., OAuth redirect).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ElicitationRequest {
    /// Schema-based form input request.
    Form {
        /// Optional metadata.
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Value>,
        /// Human-readable message to display.
        message: String,
        /// JSON Schema describing the expected input.
        #[serde(
            rename = "requestedSchema",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        requested_schema: Option<Value>,
    },
    /// URL-based input request (e.g., OAuth redirect).
    Url {
        /// Optional metadata.
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        meta: Option<Value>,
        /// Human-readable message to display.
        message: String,
        /// URL for the user to visit.
        url: String,
        /// Unique identifier for this elicitation.
        #[serde(rename = "elicitationId")]
        elicitation_id: String,
    },
}

impl ElicitationRequest {
    /// Get the human-readable message from either variant.
    pub fn message(&self) -> &str {
        match self {
            Self::Form { message, .. } | Self::Url { message, .. } => message,
        }
    }
}

/// Response to an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// Optional metadata for the elicitation response (e.g., persistence hints).
    /// Serialized as `_meta` per MCP spec.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Actions for elicitation responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

// ---------------------------------------------------------------------------
// Tool approval mode
// ---------------------------------------------------------------------------

/// Approval mode for tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolApprovalMode {
    /// Always auto-approve.
    Auto,
    /// Always prompt user (default).
    #[default]
    Prompt,
    /// Require explicit approval.
    Approve,
}

// ---------------------------------------------------------------------------
// Pagination types
// ---------------------------------------------------------------------------

/// A paginated request with optional cursor.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A paginated response wrapping items and optional next cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedResponse<T> {
    #[serde(flatten)]
    pub items: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mcp_tool_serialization() {
        let tool = McpTool {
            name: "read_file".to_string(),
            title: Some("Read File".to_string()),
            description: Some("Read contents of a file".to_string()),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            output_schema: None,
            annotations: Some(ToolAnnotations {
                destructive_hint: Some(false),
                read_only_hint: Some(true),
                open_world_hint: None,
                idempotent_hint: Some(true),
            }),
            icons: None,
            meta: None,
        };

        let json_str = serde_json::to_string(&tool).unwrap();
        assert!(json_str.contains("\"name\":\"read_file\""));
        assert!(json_str.contains("\"title\":\"Read File\""));
        assert!(json_str.contains("\"inputSchema\""));
        assert!(json_str.contains("\"readOnlyHint\":true"));
        assert!(json_str.contains("\"destructiveHint\":false"));
        // outputSchema should be omitted when None
        assert!(!json_str.contains("outputSchema"));
    }

    #[test]
    fn test_mcp_tool_deserialization() {
        let json = json!({
            "name": "write_file",
            "title": "Write File",
            "description": "Write to a file",
            "inputSchema": {"type": "object"},
            "outputSchema": {"type": "object"},
            "annotations": {
                "destructiveHint": true,
                "readOnlyHint": false,
                "openWorldHint": false,
                "idempotentHint": false
            },
            "icons": [{"type": "svg", "url": "https://example.com/icon.svg"}]
        });

        let tool: McpTool = serde_json::from_value(json).unwrap();
        assert_eq!(tool.name, "write_file");
        assert_eq!(tool.title.as_deref(), Some("Write File"));
        assert!(tool.is_destructive());
        assert!(!tool.is_read_only());
        assert!(!tool.is_open_world());
        assert!(!tool.is_idempotent());
        assert!(tool.output_schema.is_some());
        assert_eq!(tool.icons.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_mcp_tool_from_mcp_value() {
        let value = json!({
            "name": "search",
            "inputSchema": {"type": "object"}
        });
        let tool = McpTool::from_mcp_value(value).unwrap();
        assert_eq!(tool.name, "search");
        assert!(tool.annotations.is_none());
        assert!(!tool.is_destructive());
    }

    #[test]
    fn test_mcp_tool_annotations_defaults() {
        let tool = McpTool {
            name: "test".to_string(),
            title: None,
            description: None,
            input_schema: json!({"type": "object"}),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
        };
        assert!(!tool.is_destructive());
        assert!(!tool.is_read_only());
        assert!(!tool.is_open_world());
        assert!(!tool.is_idempotent());
    }

    #[test]
    fn test_mcp_resource_serialization() {
        let resource = McpResource {
            uri: "file:///tmp/test.txt".to_string(),
            name: "test.txt".to_string(),
            title: None,
            description: Some("A test file".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(1024),
            annotations: None,
            icons: None,
            meta: None,
        };

        let json_str = serde_json::to_string(&resource).unwrap();
        assert!(json_str.contains("\"uri\":\"file:///tmp/test.txt\""));
        assert!(json_str.contains("\"mimeType\":\"text/plain\""));
        assert!(json_str.contains("\"size\":1024"));
    }

    #[test]
    fn test_mcp_resource_deserialization() {
        let json = json!({
            "uri": "file:///data.json",
            "name": "data.json",
            "mimeType": "application/json",
            "size": 2048
        });
        let resource: McpResource = serde_json::from_value(json).unwrap();
        assert_eq!(resource.uri, "file:///data.json");
        assert_eq!(resource.mime_type.as_deref(), Some("application/json"));
        assert_eq!(resource.size, Some(2048));
    }

    #[test]
    fn test_mcp_resource_template_serialization() {
        let template = McpResourceTemplate {
            uri_template: "file:///projects/{project}/readme.md".to_string(),
            name: "Project README".to_string(),
            title: None,
            description: Some("README for a project".to_string()),
            mime_type: Some("text/markdown".to_string()),
            annotations: None,
        };

        let json_str = serde_json::to_string(&template).unwrap();
        assert!(json_str.contains("\"uriTemplate\""));
        assert!(json_str.contains("\"mimeType\":\"text/markdown\""));
    }

    #[test]
    fn test_mcp_resource_content_text() {
        let content = McpResourceContent::Text {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "hello world".to_string(),
            meta: None,
        };
        let json_str = serde_json::to_string(&content).unwrap();
        assert!(json_str.contains("\"text\":\"hello world\""));
        assert!(json_str.contains("\"uri\":\"file:///test.txt\""));
    }

    #[test]
    fn test_mcp_resource_content_blob() {
        let content = McpResourceContent::Blob {
            uri: "file:///image.png".to_string(),
            mime_type: Some("image/png".to_string()),
            blob: "base64data==".to_string(),
            meta: None,
        };
        let json_str = serde_json::to_string(&content).unwrap();
        assert!(json_str.contains("\"blob\":\"base64data==\""));
    }

    #[test]
    fn test_mcp_call_tool_result() {
        let result = McpCallToolResult {
            content: vec![
                McpContent::Text {
                    text: "line 1".to_string(),
                },
                McpContent::Text {
                    text: "line 2".to_string(),
                },
            ],
            is_error: Some(false),
            structured_content: None,
            meta: None,
        };

        assert!(!result.is_error());
        assert_eq!(result.text_content(), "line 1\nline 2");

        let json_str = serde_json::to_string(&result).unwrap();
        assert!(json_str.contains("\"isError\":false"));
        assert!(!json_str.contains("structuredContent"));
    }

    #[test]
    fn test_mcp_call_tool_result_with_error() {
        let result = McpCallToolResult {
            content: vec![McpContent::Text {
                text: "something went wrong".to_string(),
            }],
            is_error: Some(true),
            structured_content: None,
            meta: None,
        };

        assert!(result.is_error());
        assert_eq!(result.text_content(), "something went wrong");
    }

    #[test]
    fn test_mcp_call_tool_result_deserialization() {
        let json = json!({
            "content": [
                {"type": "text", "text": "result data"},
                {"type": "image", "data": "abc123", "mimeType": "image/png"}
            ],
            "isError": false,
            "structuredContent": {"key": "value"}
        });
        let result: McpCallToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.content[0].as_text(), Some("result data"));
        assert_eq!(result.content[1].as_text(), None);
        assert!(result.structured_content.is_some());
    }

    #[test]
    fn test_mcp_content_variants() {
        let text = McpContent::Text {
            text: "hello".to_string(),
        };
        assert_eq!(text.as_text(), Some("hello"));

        let image = McpContent::Image {
            data: "base64".to_string(),
            mime_type: "image/png".to_string(),
        };
        assert_eq!(image.as_text(), None);
    }

    #[test]
    fn test_elicitation_request_form_serialization() {
        let req = ElicitationRequest::Form {
            meta: None,
            message: "Please confirm".to_string(),
            requested_schema: Some(json!({"type": "object"})),
        };

        let json_str = serde_json::to_string(&req).unwrap();
        assert!(json_str.contains("\"mode\":\"form\""));
        assert!(json_str.contains("\"message\":\"Please confirm\""));
        assert!(json_str.contains("\"requestedSchema\""));
    }

    #[test]
    fn test_elicitation_request_url_serialization() {
        let req = ElicitationRequest::Url {
            meta: None,
            message: "Visit this URL".to_string(),
            url: "https://example.com/confirm".to_string(),
            elicitation_id: "elicit-001".to_string(),
        };

        let json_str = serde_json::to_string(&req).unwrap();
        assert!(json_str.contains("\"mode\":\"url\""));
        assert!(json_str.contains("\"message\":\"Visit this URL\""));
        assert!(json_str.contains("\"elicitationId\":\"elicit-001\""));
    }

    #[test]
    fn test_elicitation_request_message_accessor() {
        let form = ElicitationRequest::Form {
            meta: None,
            message: "Form message".to_string(),
            requested_schema: None,
        };
        assert_eq!(form.message(), "Form message");

        let url = ElicitationRequest::Url {
            meta: None,
            message: "URL message".to_string(),
            url: "https://example.com".to_string(),
            elicitation_id: "id".to_string(),
        };
        assert_eq!(url.message(), "URL message");
    }

    #[test]
    fn test_elicitation_response_serialization() {
        let resp = ElicitationResponse {
            action: ElicitationAction::Accept,
            content: Some(json!({"confirmed": true})),
            meta: None,
        };

        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(json_str.contains("\"action\":\"accept\""));
        assert!(json_str.contains("\"confirmed\":true"));
    }

    #[test]
    fn test_elicitation_action_values() {
        let accept: ElicitationAction = serde_json::from_str("\"accept\"").unwrap();
        assert_eq!(accept, ElicitationAction::Accept);

        let decline: ElicitationAction = serde_json::from_str("\"decline\"").unwrap();
        assert_eq!(decline, ElicitationAction::Decline);

        let cancel: ElicitationAction = serde_json::from_str("\"cancel\"").unwrap();
        assert_eq!(cancel, ElicitationAction::Cancel);
    }

    #[test]
    fn test_tool_approval_mode() {
        let auto: ToolApprovalMode = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(auto, ToolApprovalMode::Auto);

        let prompt: ToolApprovalMode = serde_json::from_str("\"prompt\"").unwrap();
        assert_eq!(prompt, ToolApprovalMode::Prompt);

        let approve: ToolApprovalMode = serde_json::from_str("\"approve\"").unwrap();
        assert_eq!(approve, ToolApprovalMode::Approve);

        assert_eq!(ToolApprovalMode::default(), ToolApprovalMode::Prompt);
    }

    #[test]
    fn test_paginated_request_default() {
        let req = PaginatedRequest::default();
        assert!(req.cursor.is_none());

        let json_str = serde_json::to_string(&req).unwrap();
        assert_eq!(json_str, "{}");
    }

    #[test]
    fn test_paginated_request_with_cursor() {
        let json = json!({"cursor": "next-page-token"});
        let req: PaginatedRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.cursor.as_deref(), Some("next-page-token"));
    }

    #[test]
    fn test_paginated_response_serialization() {
        #[derive(Debug, Serialize, Deserialize)]
        struct ToolList {
            tools: Vec<String>,
        }

        let resp = PaginatedResponse {
            items: ToolList {
                tools: vec!["tool1".to_string(), "tool2".to_string()],
            },
            next_cursor: Some("cursor-abc".to_string()),
        };

        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(json_str.contains("\"tools\":[\"tool1\",\"tool2\"]"));
        assert!(json_str.contains("\"nextCursor\":\"cursor-abc\""));
    }

    #[test]
    fn test_paginated_response_no_cursor() {
        #[derive(Debug, Serialize, Deserialize)]
        struct ResourceList {
            resources: Vec<String>,
        }

        let resp = PaginatedResponse {
            items: ResourceList {
                resources: vec!["res1".to_string()],
            },
            next_cursor: None,
        };

        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(json_str.contains("\"resources\":[\"res1\"]"));
        assert!(!json_str.contains("nextCursor"));
    }

    #[test]
    fn test_tool_icon_serialization() {
        let icon = ToolIcon {
            r#type: "svg".to_string(),
            url: "https://example.com/icon.svg".to_string(),
        };
        let json_str = serde_json::to_string(&icon).unwrap();
        assert!(json_str.contains("\"type\":\"svg\""));
        assert!(json_str.contains("\"url\":\"https://example.com/icon.svg\""));
    }
}
