//! Apply patch tool: precise file editing via search-and-replace patches.
//!
//! Inspired by Codex's `apply_patch` tool. Instead of overwriting entire files,
//! this tool applies targeted search-and-replace operations for precise edits.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use tracing::{info, warn};

/// Tool that applies precise edits to files using search-and-replace patches.
pub struct ApplyPatchTool;

#[derive(Deserialize)]
struct PatchArgs {
    /// File path (relative to working directory or absolute).
    path: String,
    /// The exact text to find in the file.
    old_text: String,
    /// The replacement text.
    new_text: String,
    /// If true, replace all occurrences; otherwise replace only the first.
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl ToolHandler for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a precise edit to a file by replacing exact text. More targeted than write_file — use this for modifying existing files. The old_text must match exactly (including whitespace and indentation)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative to working directory or absolute)"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find in the file (must match precisely, including whitespace)"
                },
                "new_text": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences; otherwise replace only the first (default: false)"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: PatchArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                }
            }
        };

        let file_path = super::file_ops::resolve_path(&args.path, work_dir);
        info!(
            path = %file_path.display(),
            old_len = args.old_text.len(),
            new_len = args.new_text.len(),
            replace_all = args.replace_all,
            "applying patch"
        );

        // Read current file content
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("failed to read file: {e}"),
                }
            }
        };

        // Check if old_text exists in the file
        if !content.contains(&args.old_text) {
            // Try to help: show nearby content
            let preview = if content.len() > 500 {
                let end = content.floor_char_boundary(500);
                format!("{}...", &content[..end])
            } else {
                content.clone()
            };
            warn!(
                path = %file_path.display(),
                "old_text not found in file"
            );
            return ToolResult {
                success: false,
                output: format!(
                    "old_text not found in file. The file content starts with:\n{}",
                    preview
                ),
            };
        }

        // Apply the patch
        let (new_content, count) = if args.replace_all {
            let count = content.matches(&args.old_text).count();
            (content.replace(&args.old_text, &args.new_text), count)
        } else {
            (content.replacen(&args.old_text, &args.new_text, 1), 1)
        };

        // Write the patched content
        match std::fs::write(&file_path, &new_content) {
            Ok(_) => {
                info!(
                    path = %file_path.display(),
                    replacements = count,
                    "patch applied successfully"
                );
                ToolResult {
                    success: true,
                    output: format!(
                        "applied {} replacement(s) in {}",
                        count,
                        file_path.display()
                    ),
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to write file: {e}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_apply_patch_single_replacement() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world\nfoo bar\nhello again").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_text": "hello world",
            "new_text": "goodbye world"
        });

        let result = ApplyPatchTool
            .execute(&args.to_string(), Path::new("/tmp"))
            .await;
        assert!(result.success);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("goodbye world"));
        assert!(content.contains("hello again")); // Only first occurrence replaced
    }

    #[tokio::test]
    async fn test_apply_patch_replace_all() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world\nhello world\nhello world").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_text": "hello",
            "new_text": "goodbye",
            "replace_all": true
        });

        let result = ApplyPatchTool
            .execute(&args.to_string(), Path::new("/tmp"))
            .await;
        assert!(result.success);
        assert!(result.output.contains("3 replacement(s)"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("hello"));
        assert_eq!(content.matches("goodbye").count(), 3);
    }

    #[tokio::test]
    async fn test_apply_patch_not_found() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_text": "nonexistent",
            "new_text": "replacement"
        });

        let result = ApplyPatchTool
            .execute(&args.to_string(), Path::new("/tmp"))
            .await;
        assert!(!result.success);
        assert!(result.output.contains("old_text not found"));
    }
}
