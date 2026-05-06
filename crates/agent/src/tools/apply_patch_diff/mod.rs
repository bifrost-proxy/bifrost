//! Apply diff tool: structured diff-based file patching.
//!
//! Supports the `*** Begin Patch / *** End Patch` format with:
//! - Add File: create new files
//! - Delete File: remove files
//! - Update File: apply context-based diff chunks
//! - Move To: rename files during update
//!
//! This is the Codex-style `apply_patch` implementation used for precise,
//! multi-file structured edits.

pub mod apply;
pub mod parser;

pub use apply::{apply_patch, ApplyError, ApplyResult};
pub use parser::{Hunk, PatchError, UpdateFileChunk};

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use tracing::{info, warn};

pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";

/// Tool that applies structured diff patches to files.
pub struct ApplyDiffTool;

#[derive(Deserialize)]
struct ApplyDiffArgs {
    /// Codex-compatible JSON form: the raw patch text under `input`.
    #[serde(default)]
    input: Option<String>,
    /// Backward-compatible alias for the raw patch text.
    #[serde(default)]
    patch: Option<String>,
}

#[async_trait]
impl ToolHandler for ApplyDiffTool {
    fn name(&self) -> &str {
        APPLY_PATCH_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Use the `apply_patch` tool to edit files. The patch format uses markers:\n\
         \n\
         *** Begin Patch\n\
         *** Add File: <path>\n\
         +line to add\n\
         +another line\n\
         *** Delete File: <path>\n\
         *** Update File: <path>\n\
         *** Move To: <new_path>          (optional, renames the file)\n\
         @@ context_line_to_find\n\
          unchanged line (space prefix)\n\
         -line to remove (minus prefix)\n\
         +line to add (plus prefix)\n\
         @@ another_context\n\
         -old line\n\
         +new line\n\
         *** End of File                  (optional, marks changes to end of file)\n\
         *** End Patch\n\
         \n\
         Rules:\n\
         - Begin/End Patch markers are required\n\
         - Paths must be relative, never absolute\n\
         - Add File: content lines use + prefix\n\
         - Update File: changes are in @@ chunks with space/+/- prefixed lines\n\
         - @@ lines specify a context line to locate in the file\n\
         - Space-prefixed lines are unchanged context\n\
         - Minus-prefixed lines are removed\n\
         - Plus-prefixed lines are added\n\
         - Multiple files can be patched in one call"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The raw patch content in *** Begin Patch / *** End Patch format"
                },
                "patch": {
                    "type": "string",
                    "description": "Backward-compatible alias of input"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let patch_text = if arguments
            .trim_start()
            .starts_with(parser::BEGIN_PATCH_MARKER)
        {
            arguments.to_string()
        } else {
            let args: ApplyDiffArgs = match serde_json::from_str(arguments) {
                Ok(a) => a,
                Err(e) => {
                    return ToolResult {
                        success: false,
                        output: format!("invalid arguments: {e}"),
                    };
                }
            };
            let Some(patch_text) = args.input.or(args.patch) else {
                return ToolResult {
                    success: false,
                    output: "invalid arguments: missing patch content in `input` or `patch`"
                        .to_string(),
                };
            };
            patch_text
        };
        info!(patch_len = patch_text.len(), "applying diff patch");

        // Parse the patch.
        let hunks = match parser::parse_patch(&patch_text) {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "failed to parse patch");
                return ToolResult {
                    success: false,
                    output: format!("patch parse error: {e}"),
                };
            }
        };

        if hunks.is_empty() {
            return ToolResult {
                success: false,
                output: "patch contained no operations".to_string(),
            };
        }

        // Apply the patch.
        match apply::apply_patch(work_dir, &hunks) {
            Ok(result) => {
                let summary = format!("{result}");
                let total = result.added.len()
                    + result.modified.len()
                    + result.deleted.len()
                    + result.moved.len();
                info!(
                    added = result.added.len(),
                    modified = result.modified.len(),
                    deleted = result.deleted.len(),
                    moved = result.moved.len(),
                    "diff patch applied successfully"
                );
                ToolResult {
                    success: true,
                    output: format!("{total} file(s) changed:\n{summary}"),
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to apply patch");
                ToolResult {
                    success: false,
                    output: format!("apply error: {e}"),
                }
            }
        }
    }
}

pub fn apply_patch_tool_definition() -> crate::types::ToolDefinition {
    crate::types::ToolDefinition::function(
        APPLY_PATCH_TOOL_NAME.to_string(),
        "Apply a patch to modify files. The `patch` parameter should contain the full patch content in the unified diff format starting with `*** Begin Patch` and ending with `*** End Patch`.".to_string(),
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The patch content in the structured diff format. Must start with '*** Begin Patch' and end with '*** End Patch'."
                }
            },
            "required": ["patch"]
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_apply_diff_tool_add_file() {
        let dir = TempDir::new().unwrap();
        let patch = r#"*** Begin Patch
*** Add File: hello.txt
+Hello, World!
*** End Patch"#;
        let args = serde_json::json!({ "input": patch });
        let result = ApplyDiffTool.execute(&args.to_string(), dir.path()).await;
        assert!(result.success, "output: {}", result.output);
        assert!(result.output.contains("1 file(s) changed"));
        let content = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert!(content.contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_apply_diff_tool_update_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();

        let patch = r#"*** Begin Patch
*** Update File: main.rs
@@ fn main() {
-    println!("old");
+    println!("new");
*** End Patch"#;
        let args = serde_json::json!({ "input": patch });
        let result = ApplyDiffTool.execute(&args.to_string(), dir.path()).await;
        assert!(result.success, "output: {}", result.output);
        let content = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert!(content.contains("println!(\"new\")"));
    }

    #[tokio::test]
    async fn test_apply_diff_tool_invalid_patch() {
        let dir = TempDir::new().unwrap();
        let patch = "this is not a valid patch";
        let args = serde_json::json!({ "input": patch });
        let result = ApplyDiffTool.execute(&args.to_string(), dir.path()).await;
        assert!(!result.success);
        assert!(result.output.contains("parse error"));
    }

    #[test]
    fn test_apply_patch_tool_definition_is_function() {
        let tool = apply_patch_tool_definition();
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.name(), APPLY_PATCH_TOOL_NAME);
        let func = tool.function.expect("function definition");
        assert!(func.parameters.is_some());
    }

    #[tokio::test]
    async fn test_apply_diff_tool_empty_patch() {
        let dir = TempDir::new().unwrap();
        let patch = "*** Begin Patch\n*** End Patch";
        let args = serde_json::json!({ "input": patch });
        let result = ApplyDiffTool.execute(&args.to_string(), dir.path()).await;
        assert!(!result.success);
        assert!(result.output.contains("no operations"));
    }

    #[tokio::test]
    async fn test_apply_diff_tool_delete_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("to_delete.txt"), "content").unwrap();

        let patch = "*** Begin Patch\n*** Delete File: to_delete.txt\n*** End Patch";
        let args = serde_json::json!({ "input": patch });
        let result = ApplyDiffTool.execute(&args.to_string(), dir.path()).await;
        assert!(result.success, "output: {}", result.output);
        assert!(!dir.path().join("to_delete.txt").exists());
    }

    #[tokio::test]
    async fn test_apply_diff_tool_accepts_raw_patch_body() {
        let dir = TempDir::new().unwrap();
        let patch = "*** Begin Patch\n*** Add File: raw.txt\n+raw body\n*** End Patch";
        let result = ApplyDiffTool.execute(patch, dir.path()).await;
        assert!(result.success, "output: {}", result.output);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("raw.txt")).unwrap(),
            "raw body\n"
        );
    }
}
