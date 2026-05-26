//! MCP memory tool interface definitions and executor.
//!
//! Provides a unified entry point for memory operations (List, Read, Search)
//! that can be exposed as MCP tools. Aligned with Codex ext/memories tool design.

use crate::memory::search::search_memory_files_advanced;
use crate::memory::types::{MemoryTool, MemoryToolResult, SearchQuery};
use std::path::Path;
use tracing::debug;

/// Maximum lines returned by a single Read operation.
#[allow(dead_code)]
const MAX_READ_LINES: usize = 500;

/// Execute a memory tool command and return the result.
#[allow(dead_code)]
pub(crate) fn execute_memory_tool(
    root: &Path,
    tool: MemoryTool,
) -> Result<MemoryToolResult, String> {
    match tool {
        MemoryTool::List {
            path,
            cursor,
            limit,
        } => execute_list(root, path.as_deref(), cursor.as_deref(), limit),
        MemoryTool::Read {
            path,
            line_offset,
            max_lines,
        } => execute_read(root, &path, line_offset, max_lines),
        MemoryTool::Search(query) => execute_search(root, &query),
    }
}

// ---------------------------------------------------------------------------
// List tool
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn execute_list(
    root: &Path,
    path: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
) -> Result<MemoryToolResult, String> {
    let target = match path {
        Some(rel) => {
            let p = root.join(rel);
            if !p.exists() {
                return Err(format!("path '{}' was not found", rel));
            }
            p
        }
        None => root.to_path_buf(),
    };

    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_target = target.canonicalize().unwrap_or_else(|_| target.clone());
    if !canonical_target.starts_with(&canonical_root) {
        return Err(format!(
            "path '{}' escapes the memory root",
            path.unwrap_or(".")
        ));
    }

    if !canonical_target.is_dir() {
        return Err(format!("path '{}' is not a directory", path.unwrap_or(".")));
    }

    let entries = std::fs::read_dir(&canonical_target).map_err(|e| format!("read_dir: {e}"))?;

    let mut items: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files.
        if name.starts_with('.') {
            continue;
        }
        let entry_type = if entry.path().is_dir() {
            "directory"
        } else {
            "file"
        };
        items.push(format!("{}\t{}", name, entry_type));
    }
    items.sort();

    let start_index = match cursor {
        Some(c) => c.parse::<usize>().unwrap_or(0),
        None => 0,
    };
    let total_count = items.len();
    let clamped_limit = limit.min(items.len().saturating_sub(start_index));
    let end_index = start_index + clamped_limit;
    let next_cursor = if end_index < items.len() {
        Some(end_index.to_string())
    } else {
        None
    };

    let content = items[start_index..end_index].join("\n");

    debug!(
        path = path.unwrap_or("."),
        total_count,
        returned = clamped_limit,
        "memory list executed"
    );

    Ok(MemoryToolResult {
        content,
        next_cursor,
        total_count: Some(total_count),
    })
}

// ---------------------------------------------------------------------------
// Read tool
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn execute_read(
    root: &Path,
    path: &str,
    line_offset: Option<usize>,
    max_lines: Option<usize>,
) -> Result<MemoryToolResult, String> {
    let target = root.join(path);
    if !target.exists() {
        return Err(format!("path '{}' was not found", path));
    }
    if !target.is_file() {
        return Err(format!("path '{}' is not a file", path));
    }

    // Path traversal check.
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_target = target.canonicalize().unwrap_or_else(|_| target.clone());
    if !canonical_target.starts_with(&canonical_root) {
        return Err(format!("path '{}' escapes the memory root", path));
    }

    let content = std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", path))?;

    let lines: Vec<&str> = content.lines().collect();
    let offset = line_offset.unwrap_or(1).max(1);
    let start_idx = offset.saturating_sub(1);

    if start_idx >= lines.len() && !lines.is_empty() {
        return Err("line_offset exceeds file length".to_string());
    }

    let max = max_lines.unwrap_or(MAX_READ_LINES).min(MAX_READ_LINES);
    let end_idx = start_idx.saturating_add(max).min(lines.len());
    let truncated = end_idx < lines.len();

    let result_content = lines[start_idx..end_idx].join("\n");
    let next_cursor = if truncated {
        Some((end_idx + 1).to_string())
    } else {
        None
    };

    debug!(
        path,
        start_line = offset,
        lines_returned = end_idx - start_idx,
        truncated,
        "memory read executed"
    );

    Ok(MemoryToolResult {
        content: result_content,
        next_cursor,
        total_count: Some(lines.len()),
    })
}

// ---------------------------------------------------------------------------
// Search tool
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn execute_search(root: &Path, query: &SearchQuery) -> Result<MemoryToolResult, String> {
    let response = search_memory_files_advanced(root, query)?;
    let total = response.matches.len();
    let content = serde_json::to_string_pretty(&response)
        .map_err(|e| format!("serialize search response: {e}"))?;

    Ok(MemoryToolResult {
        content,
        next_cursor: response.next_cursor,
        total_count: Some(total),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::MemoryTool;

    #[test]
    fn list_rejects_path_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        let result = execute_memory_tool(
            root.path(),
            MemoryTool::List {
                path: Some(outside.path().to_string_lossy().to_string()),
                cursor: None,
                limit: 10,
            },
        );

        assert!(result
            .expect_err("outside path rejected")
            .contains("escapes the memory root"));
    }
}
