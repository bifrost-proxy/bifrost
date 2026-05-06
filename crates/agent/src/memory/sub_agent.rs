//! Phase 2 sub-agent tools: sandboxed file operations for memory consolidation.

use crate::types::ToolDefinition;
use std::fs;
use std::path::{Path, PathBuf};

/// Tools available to the Phase 2 consolidation sub-agent.
pub(crate) fn phase2_agent_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::function(
            "read_file".to_string(),
            "Read a file from the memory workspace. The path must be relative to the memory root.".to_string(),
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file to read" }
                },
                "required": ["path"]
            })),
        ),
        ToolDefinition::function(
            "write_file".to_string(),
            "Write content to a file in the memory workspace. The path must be relative to the memory root.".to_string(),
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file to write" },
                    "content": { "type": "string", "description": "Content to write to the file" }
                },
                "required": ["path", "content"]
            })),
        ),
        ToolDefinition::function(
            "list_files".to_string(),
            "List files in a directory within the memory workspace. Returns file names and basic metadata.".to_string(),
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the directory to list (defaults to memory root)" }
                },
                "required": []
            })),
        ),
    ]
}

/// Execute a Phase 2 tool call within the memory sandbox.
pub(crate) fn execute_phase2_tool(root: &Path, tool_name: &str, arguments: &str) -> String {
    match tool_name {
        "read_file" => {
            let args: serde_json::Value = match serde_json::from_str(arguments) {
                Ok(v) => v,
                Err(e) => return format!(r#"{{"error":"invalid arguments: {e}"}}"#),
            };
            let relative = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return r#"{"error":"missing path argument"}"#.to_string(),
            };
            let path = match sanitize_relative_path(root, relative) {
                Some(p) => p,
                None => return r#"{"error":"path escapes memory workspace"}"#.to_string(),
            };
            match fs::read_to_string(&path) {
                Ok(content) => {
                    serde_json::json!({"content": content, "path": relative}).to_string()
                }
                Err(e) => format!(r#"{{"error":"read file failed: {e}"}}"#),
            }
        }
        "write_file" => {
            let args: serde_json::Value = match serde_json::from_str(arguments) {
                Ok(v) => v,
                Err(e) => return format!(r#"{{"error":"invalid arguments: {e}"}}"#),
            };
            let relative = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return r#"{"error":"missing path argument"}"#.to_string(),
            };
            let content = match args.get("content").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return r#"{"error":"missing content argument"}"#.to_string(),
            };
            let path = match sanitize_relative_path(root, relative) {
                Some(p) => p,
                None => return r#"{"error":"path escapes memory workspace"}"#.to_string(),
            };
            if let Some(parent) = path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return format!(r#"{{"error":"create directory failed: {e}"}}"#);
                }
            }
            match fs::write(&path, content) {
                Ok(()) => format!(r#"{{"success":true,"path":"{}"}}"#, relative),
                Err(e) => format!(r#"{{"error":"write file failed: {e}"}}"#),
            }
        }
        "list_files" => {
            let args: serde_json::Value = match serde_json::from_str(arguments) {
                Ok(v) => v,
                Err(e) => return format!(r#"{{"error":"invalid arguments: {e}"}}"#),
            };
            let relative = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let path = match sanitize_relative_path(root, relative) {
                Some(p) => p,
                None => return r#"{"error":"path escapes memory workspace"}"#.to_string(),
            };
            match fs::read_dir(&path) {
                Ok(entries) => {
                    let files: Vec<serde_json::Value> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            serde_json::json!({"name": name, "is_dir": is_dir})
                        })
                        .collect();
                    serde_json::json!({"files": files, "path": relative}).to_string()
                }
                Err(e) => format!(r#"{{"error":"list directory failed: {e}"}}"#),
            }
        }
        _ => format!(r#"{{"error":"unknown tool: {tool_name}"}}"#),
    }
}

/// Sanitize a relative path to prevent path traversal attacks.
/// Returns `None` if the resulting path would escape the memory root.
pub(crate) fn sanitize_relative_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let cleaned = relative
        .replace('\\', "/")
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/");
    if cleaned.is_empty() {
        return Some(root.to_path_buf());
    }
    let resolved = root.join(&cleaned);
    // Defense-in-depth: verify the resolved path is actually under root
    // (handles edge cases like symlinks or platform-specific behavior)
    if !resolved.starts_with(root) {
        return None;
    }
    Some(resolved)
}
