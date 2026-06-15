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
                    runtime_events: Vec::new(),
                }
            }
        };

        let file_path = match resolve_path_confined(&args.path, work_dir) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("path rejected: {e}"),
                    runtime_events: Vec::new(),
                }
            }
        };
        info!(path = %file_path.display(), bytes = args.content.len(), "writing file");

        if let Some(parent) = file_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult {
                    success: false,
                    output: format!("failed to create directories: {e}"),
                    runtime_events: Vec::new(),
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
                runtime_events: Vec::new(),
            },
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to write file: {e}"),
                runtime_events: Vec::new(),
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
                    runtime_events: Vec::new(),
                }
            }
        };

        let file_path = match resolve_path_confined(&args.path, work_dir) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("path rejected: {e}"),
                    runtime_events: Vec::new(),
                }
            }
        };
        info!(path = %file_path.display(), "reading file");

        // Check file size before reading (prevent OOM)
        if let Err(e) = check_file_size(&file_path, MAX_READ_FILE_BYTES) {
            return ToolResult {
                success: false,
                output: e,
                runtime_events: Vec::new(),
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
                    runtime_events: Vec::new(),
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to read file: {e}"),
                runtime_events: Vec::new(),
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
        let dir = match args.path.as_ref() {
            Some(p) => match resolve_path_confined(p, work_dir) {
                Ok(d) => d,
                Err(e) => {
                    return ToolResult {
                        success: false,
                        output: format!("path rejected: {e}"),
                        runtime_events: Vec::new(),
                    }
                }
            },
            None => work_dir.to_path_buf(),
        };

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
                        runtime_events: Vec::new(),
                    }
                } else {
                    ToolResult {
                        success: true,
                        output: items.join("\n"),
                        runtime_events: Vec::new(),
                    }
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to list directory: {e}"),
                runtime_events: Vec::new(),
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

/// Lexically normalize a path by resolving `.` and `..` components purely
/// textually (no filesystem access, no symlink following).
///
/// This must run *before* the symlink-aware canonicalization pass: relying on
/// `Path::file_name()` to peel components drops `..` segments silently (it
/// returns `None` when a path ends in `..`), which would let
/// `sub/../../escape.txt` collapse back *inside* the sandbox instead of
/// escaping it. Normalizing first guarantees every `..` actually pops a
/// component (or, when it would climb above the anchor, is preserved so the
/// final containment check fails closed).
fn lexical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop a real component; if there is nothing to pop (the path is
                // trying to climb above its anchor) keep the `..` so the final
                // containment check rejects it.
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

/// Resolve `path` and confine it to `work_dir`.
///
/// The agent's file tools take a model-controlled `path`. Without confinement
/// an indirect prompt injection could read arbitrary files (`~/.ssh/id_rsa`)
/// or overwrite host files (`/etc/...`), escalating to RCE. This helper
/// resolves the candidate (joining relative paths onto `work_dir`, accepting
/// absolute paths only if they still land inside `work_dir`), normalizes
/// `..`/`.` components, resolves symlinks on the longest existing prefix, and
/// rejects anything that escapes the `work_dir` sandbox.
pub fn resolve_path_confined(path: &str, work_dir: &Path) -> Result<std::path::PathBuf, String> {
    let candidate = resolve_path(path, work_dir);

    // Canonicalize the sandbox root. If the root itself cannot be resolved we
    // cannot make a safe decision, so fail closed.
    let root = work_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve working directory: {e}"))?;

    // Collapse `.`/`..` lexically first (see `lexical_normalize`), then resolve
    // the longest existing prefix of the normalized candidate (this follows
    // symlinks for components that exist on disk) and re-attach the
    // not-yet-existing tail. This lets `write_file` create new files while
    // still blocking symlink/`..` escapes through existing dirs.
    let normalized = lexical_normalize(&candidate);

    let mut existing = normalized.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let resolved_prefix = loop {
        match existing.canonicalize() {
            Ok(p) => break p,
            Err(_) => match existing.parent() {
                Some(parent) => {
                    if let Some(name) = existing.file_name() {
                        tail.push(name.to_os_string());
                    }
                    existing = parent;
                }
                None => return Err(format!("invalid path: {}", candidate.display())),
            },
        }
    };

    let mut resolved = resolved_prefix;
    for name in tail.into_iter().rev() {
        // Defensive: reject `..` / `.` that survived into the tail.
        if name == std::ffi::OsStr::new("..") || name == std::ffi::OsStr::new(".") {
            return Err("path traversal is not allowed".to_string());
        }
        resolved.push(name);
    }

    if !resolved.starts_with(&root) {
        return Err(format!(
            "path '{}' escapes the working directory sandbox",
            path
        ));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confined_allows_relative_inside_root() {
        let tmp = std::env::temp_dir().join(format!(
            "bifrost-agent-confine-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("sub/a.txt"), b"x").unwrap();

        let ok = resolve_path_confined("sub/a.txt", &tmp).unwrap();
        assert!(ok.starts_with(tmp.canonicalize().unwrap()));

        // New (not-yet-existing) file inside root is allowed.
        let new_ok = resolve_path_confined("sub/new.txt", &tmp).unwrap();
        assert!(new_ok.starts_with(tmp.canonicalize().unwrap()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn confined_rejects_dotdot_escape() {
        let tmp = std::env::temp_dir().join(format!(
            "bifrost-agent-confine-dd-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        assert!(resolve_path_confined("../escape.txt", &tmp).is_err());
        assert!(resolve_path_confined("sub/../../escape.txt", &tmp).is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn confined_rejects_absolute_outside_root() {
        let tmp = std::env::temp_dir().join(format!(
            "bifrost-agent-confine-abs-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        if !cfg!(target_os = "windows") {
            assert!(resolve_path_confined("/etc/passwd", &tmp).is_err());
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn confined_rejects_symlink_escape() {
        if cfg!(target_os = "windows") {
            return;
        }
        let base = std::env::temp_dir().join(format!(
            "bifrost-agent-confine-sym-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top secret").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // Reading through a symlink that points outside the root must fail.
        assert!(resolve_path_confined("link/secret.txt", &root).is_err());

        let _ = std::fs::remove_dir_all(&base);
    }
}
