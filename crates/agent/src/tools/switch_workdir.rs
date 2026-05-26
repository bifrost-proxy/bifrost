//! Switch working directory tool.
//!
//! Allows the agent to change the working directory of the current session.
//! When invoked, the session's work_dir is updated and history is cleared
//! to start fresh in the new context.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use tracing::info;

/// Tool that switches the working directory.
///
/// This is a "signal" tool — the actual session mutation happens in the turn loop
/// after this tool returns. The tool validates the directory and returns success/failure.
pub struct SwitchWorkdirTool;

#[derive(Deserialize)]
struct SwitchWorkdirArgs {
    /// The new working directory path (must be absolute).
    path: String,
}

#[async_trait]
impl ToolHandler for SwitchWorkdirTool {
    fn name(&self) -> &str {
        "switch_workdir"
    }

    fn description(&self) -> &str {
        "Switch the working directory of the current session. WARNING: This is a destructive operation — it clears ALL conversation history and reloads project-specific configuration (AGENTS.md, skills) for the new directory. ONLY invoke this tool when the user has EXPLICITLY and UNAMBIGUOUSLY requested to switch/change the working directory (e.g., 'switch to /path/to/project', 'change working directory to ...'). NEVER invoke this tool when the user merely mentions, discusses, references, or asks about a directory path. When in doubt, ask the user for confirmation before calling this tool."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute path to the new working directory"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: SwitchWorkdirArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                    runtime_events: Vec::new(),
                }
            }
        };

        let new_path = Path::new(&args.path);

        // Validate: must be absolute path
        if !new_path.is_absolute() {
            return ToolResult {
                success: false,
                output: format!("path must be absolute, got: {}", args.path),
                runtime_events: Vec::new(),
            };
        }

        // Validate: directory must exist
        if !new_path.is_dir() {
            return ToolResult {
                success: false,
                output: format!("directory does not exist: {}", args.path),
                runtime_events: Vec::new(),
            };
        }

        info!(new_work_dir = %args.path, "switch_workdir tool: directory validated");

        ToolResult {
            success: true,
            output: format!("SWITCH_WORKDIR:{}", args.path),
            runtime_events: Vec::new(),
        }
    }
}
