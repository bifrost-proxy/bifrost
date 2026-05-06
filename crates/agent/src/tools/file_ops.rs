//! File operation tools: write_file, read_file, list_directory.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use bifrost_core::text::{check_file_size, floor_char_boundary, MAX_READ_FILE_BYTES};
use serde::Deserialize;
use std::path::Path;
use tracing::info;

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub struct WriteFileTool;

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[async_trait]
impl ToolHandler for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if they don't exist. Overwrites existing files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative to working directory or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: WriteFileArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                }
            }
        };

        let file_path = resolve_path(&args.path, work_dir);
        info!(path = %file_path.display(), bytes = args.content.len(), "writing file");

        if let Some(parent) = file_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult {
                    success: false,
                    output: format!("failed to create directories: {e}"),
                };
            }
        }

        match std::fs::write(&file_path, &args.content) {
            Ok(_) => ToolResult {
                success: true,
                output: format!(
                    "wrote {} bytes to {}",
                    args.content.len(),
                    file_path.display()
                ),
            },
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to write file: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub struct ReadFileTool;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the content of a file. Supports optional line offset and limit for large files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative to working directory or absolute)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (optional)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ReadFileArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                }
            }
        };

        let file_path = resolve_path(&args.path, work_dir);
        info!(path = %file_path.display(), "reading file");

        // Check file size before reading (prevent OOM)
        if let Err(e) = check_file_size(&file_path, MAX_READ_FILE_BYTES) {
            return ToolResult {
                success: false,
                output: e,
            };
        }

        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();
                let offset = args.offset.unwrap_or(1).max(1) - 1;
                let limit = args.limit.unwrap_or(total_lines);
                let selected: Vec<&str> = lines.iter().skip(offset).take(limit).copied().collect();
                let showing = selected.len();

                let result = selected.join("\n");
                // Cap at 128 KiB with UTF-8 safe truncation
                const MAX_OUTPUT_BYTES: usize = 128 * 1024;
                let result = if result.len() > MAX_OUTPUT_BYTES {
                    let end = floor_char_boundary(&result, MAX_OUTPUT_BYTES);
                    format!(
                        "(file has {total_lines} lines, showing lines {}-{}, output truncated)\n{}",
                        offset + 1,
                        offset + showing,
                        &result[..end]
                    )
                } else if offset > 0 || showing < total_lines {
                    format!(
                        "(file has {total_lines} lines, showing lines {}-{})\n{result}",
                        offset + 1,
                        offset + showing
                    )
                } else {
                    result
                };

                ToolResult {
                    success: true,
                    output: result,
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to read file: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// list_directory
// ---------------------------------------------------------------------------

pub struct ListDirectoryTool;

#[derive(Deserialize)]
struct ListDirArgs {
    path: Option<String>,
}

#[async_trait]
impl ToolHandler for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and directories in a given path. Defaults to the working directory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path (relative to working directory or absolute). Defaults to working directory."
                }
            }
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ListDirArgs =
            serde_json::from_str(arguments).unwrap_or(ListDirArgs { path: None });
        let dir = args
            .path
            .as_ref()
            .map(|p| resolve_path(p, work_dir))
            .unwrap_or_else(|| work_dir.to_path_buf());

        info!(path = %dir.display(), "listing directory");

        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let mut items: Vec<String> = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let file_type = if entry.path().is_dir() { "dir" } else { "file" };
                    items.push(format!("[{file_type}] {name}"));
                }
                items.sort();
                if items.is_empty() {
                    ToolResult {
                        success: true,
                        output: "(empty directory)".to_string(),
                    }
                } else {
                    ToolResult {
                        success: true,
                        output: items.join("\n"),
                    }
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to list directory: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn resolve_path(path: &str, work_dir: &Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        work_dir.join(p)
    }
}
