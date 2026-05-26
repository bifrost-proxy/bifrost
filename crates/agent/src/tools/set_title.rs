//! Set session title tool.
//!
//! Allows the agent to set a descriptive title for the current conversation session.
//! The title is displayed in the IM card header (e.g., Feishu) to reflect the
//! current topic or intent of the conversation.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;

/// Tool that sets the session title to reflect the current conversation intent.
///
/// This is a "signal" tool — the actual session mutation happens in the turn loop
/// after this tool returns. The tool validates the title and returns a prefixed output
/// that the turn loop parses to update session.title.
pub struct SetTitleTool;

#[derive(Deserialize)]
struct SetTitleArgs {
    /// A short, descriptive title for the current conversation topic.
    title: String,
}

#[async_trait]
impl ToolHandler for SetTitleTool {
    fn name(&self) -> &str {
        "set_title"
    }

    fn description(&self) -> &str {
        "Set the conversation title to reflect the current topic or intent. \
         Call this whenever the conversation topic changes or at the start of a new task. \
         The title is displayed in message headers visible to the user. \
         Keep it short (under 30 characters) and descriptive."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "A short descriptive title for the current conversation topic (max 30 chars)"
                }
            },
            "required": ["title"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: SetTitleArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                    runtime_events: Vec::new(),
                }
            }
        };

        let title = args.title.trim().to_string();
        if title.is_empty() {
            return ToolResult {
                success: false,
                output: "title cannot be empty".to_string(),
                runtime_events: Vec::new(),
            };
        }

        // Truncate to 30 chars for display
        let title = if title.chars().count() > 30 {
            let truncated: String = title.chars().take(30).collect();
            format!("{truncated}…")
        } else {
            title
        };

        ToolResult {
            success: true,
            output: format!("SET_TITLE:{title}"),
            runtime_events: Vec::new(),
        }
    }
}
