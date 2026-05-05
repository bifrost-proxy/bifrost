//! Apply patch hunks to the filesystem.
//!
//! Core algorithm:
//! 1. For each `Hunk`, resolve the file path and apply the operation.
//! 2. For `UpdateFile` hunks, use `seek_sequence` to locate context lines,
//!    then `compute_replacements` to build replacement ranges, and
//!    `apply_replacements` to produce the new file content.

use super::parser::{Hunk, UpdateFileChunk};
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, info, warn};

// ─── Types ───────────────────────────────────────────────────────────────────

/// Result of a successful patch application.
#[derive(Debug, Default)]
pub struct ApplyResult {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub moved: Vec<(PathBuf, PathBuf)>,
}

impl fmt::Display for ApplyResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for p in &self.added {
            writeln!(f, "A  {}", p.display())?;
        }
        for p in &self.modified {
            writeln!(f, "M  {}", p.display())?;
        }
        for p in &self.deleted {
            writeln!(f, "D  {}", p.display())?;
        }
        for (from, to) in &self.moved {
            writeln!(f, "R  {} -> {}", from.display(), to.display())?;
        }
        Ok(())
    }
}

/// Errors that can occur while applying a patch.
#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("I/O error: {0}")]
    IoError(String),
    #[error("absolute paths are not supported in apply_patch_diff: {0}")]
    AbsolutePath(PathBuf),
    #[error("context not found in {file}: \"{context}\"")]
    ContextNotFound { file: PathBuf, context: String },
    #[error("old lines do not match in {file} near context \"{context}\"")]
    OldLinesNotMatch { file: PathBuf, context: String },
    #[error("file already exists: {0}")]
    FileAlreadyExists(PathBuf),
    #[error("file not found: {0}")]
    FileNotFound(PathBuf),
}

/// Internal representation of a replacement to apply.
#[derive(Debug)]
struct Replacement {
    /// Start line index (inclusive).
    start_line: usize,
    /// End line index (exclusive).
    end_line: usize,
    /// New lines to insert in place of [start_line, end_line).
    new_lines: Vec<String>,
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Apply a list of parsed hunks to files relative to `work_dir`.
pub fn apply_patch(work_dir: &Path, hunks: &[Hunk]) -> Result<ApplyResult, ApplyError> {
    let mut result = ApplyResult::default();

    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                ensure_relative_path(path)?;
                let full_path = resolve_path(path, work_dir);
                if full_path.exists() {
                    return Err(ApplyError::FileAlreadyExists(path.clone()));
                }
                // Create parent directories.
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| ApplyError::IoError(format!("mkdir: {e}")))?;
                }
                std::fs::write(&full_path, contents).map_err(|e| {
                    ApplyError::IoError(format!("write {}: {e}", full_path.display()))
                })?;
                info!(path = %path.display(), "added file");
                result.added.push(path.clone());
            }

            Hunk::DeleteFile { path } => {
                ensure_relative_path(path)?;
                let full_path = resolve_path(path, work_dir);
                if !full_path.exists() {
                    return Err(ApplyError::FileNotFound(path.clone()));
                }
                std::fs::remove_file(&full_path).map_err(|e| {
                    ApplyError::IoError(format!("delete {}: {e}", full_path.display()))
                })?;
                info!(path = %path.display(), "deleted file");
                result.deleted.push(path.clone());
            }

            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                ensure_relative_path(path)?;
                if let Some(move_path) = move_path {
                    ensure_relative_path(move_path)?;
                }
                let full_path = resolve_path(path, work_dir);
                if !full_path.exists() {
                    return Err(ApplyError::FileNotFound(path.clone()));
                }

                let content = std::fs::read_to_string(&full_path).map_err(|e| {
                    ApplyError::IoError(format!("read {}: {e}", full_path.display()))
                })?;
                let file_lines: Vec<&str> = content.lines().collect();

                let replacements = compute_replacements(&file_lines, chunks, path)?;
                let new_lines = apply_replacements(&file_lines, &replacements);
                let new_content = new_lines.join("\n");
                // Preserve trailing newline if original had one.
                let new_content = if content.ends_with('\n') && !new_content.ends_with('\n') {
                    format!("{new_content}\n")
                } else {
                    new_content
                };

                // Handle move: write to new path and delete old.
                if let Some(mp) = move_path {
                    let new_full_path = resolve_path(mp, work_dir);
                    if let Some(parent) = new_full_path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| ApplyError::IoError(format!("mkdir: {e}")))?;
                    }
                    std::fs::write(&new_full_path, &new_content).map_err(|e| {
                        ApplyError::IoError(format!("write {}: {e}", new_full_path.display()))
                    })?;
                    if full_path != new_full_path {
                        std::fs::remove_file(&full_path).map_err(|e| {
                            ApplyError::IoError(format!("delete old {}: {e}", full_path.display()))
                        })?;
                    }
                    info!(
                        from = %path.display(),
                        to = %mp.display(),
                        "moved and updated file"
                    );
                    result.moved.push((path.clone(), mp.clone()));
                } else {
                    std::fs::write(&full_path, &new_content).map_err(|e| {
                        ApplyError::IoError(format!("write {}: {e}", full_path.display()))
                    })?;
                    info!(path = %path.display(), "updated file");
                    result.modified.push(path.clone());
                }
            }
        }
    }

    Ok(result)
}

// ─── Core Algorithms ─────────────────────────────────────────────────────────

/// Find the line index where `pattern` appears in `lines`, starting from `start`.
/// Uses multi-level matching: exact → trimmed → contains.
pub fn seek_sequence(lines: &[&str], pattern: &str, start: usize) -> Option<usize> {
    let pattern_trimmed = pattern.trim();
    if pattern_trimmed.is_empty() {
        // Empty pattern matches the start position.
        return Some(start);
    }

    // Level 1: exact match after trimming both sides.
    for (idx, line) in lines.iter().enumerate().skip(start) {
        if line.trim() == pattern_trimmed {
            return Some(idx);
        }
    }

    // Level 2: contains match (fuzzy).
    for (idx, line) in lines.iter().enumerate().skip(start) {
        if line.contains(pattern_trimmed) {
            debug!(
                idx,
                pattern = pattern_trimmed,
                "seek_sequence: fuzzy contains match"
            );
            return Some(idx);
        }
    }

    None
}

/// Compute replacements for all chunks in an Update File hunk.
fn compute_replacements(
    file_lines: &[&str],
    chunks: &[UpdateFileChunk],
    file_path: &Path,
) -> Result<Vec<Replacement>, ApplyError> {
    let mut replacements = Vec::new();
    let mut search_start: usize = 0;

    for chunk in chunks {
        let old_line_count = chunk.old_lines.len();
        let match_start =
            if let Some(change_context) = chunk.change_context.as_deref() {
                let context_idx = seek_sequence(file_lines, change_context, search_start)
                    .ok_or_else(|| ApplyError::ContextNotFound {
                        file: file_path.to_path_buf(),
                        context: change_context.to_string(),
                    })?;
                context_idx + 1
            } else if old_line_count == 0 {
                file_lines.len()
            } else {
                seek_line_block(file_lines, &chunk.old_lines, search_start).ok_or_else(|| {
                    ApplyError::OldLinesNotMatch {
                        file: file_path.to_path_buf(),
                        context: "<no @@ context>".to_string(),
                    }
                })?
            };

        // Determine the end of the replacement range.
        let end_line = if chunk.is_end_of_file {
            file_lines.len()
        } else if old_line_count == 0 {
            // Pure insertion after context line.
            match_start
        } else {
            // Verify old_lines match the file content.
            let available = file_lines.len().saturating_sub(match_start);
            let match_count = old_line_count.min(available);

            for j in 0..match_count {
                let file_line = file_lines[match_start + j].trim();
                let expected = chunk.old_lines[j].trim();
                if file_line != expected {
                    warn!(
                        file_line_idx = match_start + j,
                        file_line, expected, "old_lines mismatch"
                    );
                    return Err(ApplyError::OldLinesNotMatch {
                        file: file_path.to_path_buf(),
                        context: chunk
                            .change_context
                            .clone()
                            .unwrap_or_else(|| "<no @@ context>".to_string()),
                    });
                }
            }

            match_start + match_count
        };

        replacements.push(Replacement {
            start_line: match_start,
            end_line,
            new_lines: chunk.new_lines.clone(),
        });

        // Advance search start past this replacement.
        search_start = end_line;
    }

    Ok(replacements)
}

fn seek_line_block(lines: &[&str], pattern: &[String], start: usize) -> Option<usize> {
    if pattern.is_empty() {
        return Some(lines.len());
    }

    let pattern_trimmed: Vec<&str> = pattern.iter().map(|line| line.trim()).collect();
    for idx in start..=lines.len().saturating_sub(pattern.len()) {
        let matches = pattern_trimmed
            .iter()
            .enumerate()
            .all(|(offset, expected)| lines[idx + offset].trim() == *expected);
        if matches {
            return Some(idx);
        }
    }

    None
}

/// Apply replacements in reverse order to preserve line indices.
fn apply_replacements(file_lines: &[&str], replacements: &[Replacement]) -> Vec<String> {
    let mut result: Vec<String> = file_lines.iter().map(|s| s.to_string()).collect();

    // Apply in reverse order to keep indices valid.
    for rep in replacements.iter().rev() {
        let start = rep.start_line.min(result.len());
        let end = rep.end_line.min(result.len());
        result.splice(start..end, rep.new_lines.iter().cloned());
    }

    result
}

/// Resolve a path relative to work_dir.
fn resolve_path(path: &Path, work_dir: &Path) -> PathBuf {
    work_dir.join(path)
}

fn ensure_relative_path(path: &Path) -> Result<(), ApplyError> {
    if path.is_absolute() {
        return Err(ApplyError::AbsolutePath(path.to_path_buf()));
    }
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn read_file(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn test_apply_add_file() {
        let dir = TempDir::new().unwrap();
        let hunks = vec![Hunk::AddFile {
            path: PathBuf::from("new_file.txt"),
            contents: "hello world\n".to_string(),
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.added.len(), 1);
        assert_eq!(read_file(dir.path(), "new_file.txt"), "hello world\n");
    }

    #[test]
    fn test_apply_delete_file() {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "to_delete.txt", "bye");
        let hunks = vec![Hunk::DeleteFile {
            path: PathBuf::from("to_delete.txt"),
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.deleted.len(), 1);
        assert!(!dir.path().join("to_delete.txt").exists());
    }

    #[test]
    fn test_apply_update_simple() {
        let dir = TempDir::new().unwrap();
        create_file(
            dir.path(),
            "main.rs",
            "fn main() {\n    println!(\"old\");\n}\n",
        );
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("main.rs"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: Some("fn main() {".to_string()),
                old_lines: vec!["    println!(\"old\");".to_string()],
                new_lines: vec!["    println!(\"new\");".to_string()],
                is_end_of_file: false,
            }],
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.modified.len(), 1);
        let content = read_file(dir.path(), "main.rs");
        assert!(content.contains("println!(\"new\")"));
        assert!(!content.contains("println!(\"old\")"));
    }

    #[test]
    fn test_apply_update_multiple_chunks() {
        let dir = TempDir::new().unwrap();
        create_file(
            dir.path(),
            "lib.rs",
            "fn foo() {\n    old_foo();\n}\nfn bar() {\n    old_bar();\n}\n",
        );
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("lib.rs"),
            move_path: None,
            chunks: vec![
                UpdateFileChunk {
                    change_context: Some("fn foo() {".to_string()),
                    old_lines: vec!["    old_foo();".to_string()],
                    new_lines: vec!["    new_foo();".to_string()],
                    is_end_of_file: false,
                },
                UpdateFileChunk {
                    change_context: Some("fn bar() {".to_string()),
                    old_lines: vec!["    old_bar();".to_string()],
                    new_lines: vec!["    new_bar();".to_string()],
                    is_end_of_file: false,
                },
            ],
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.modified.len(), 1);
        let content = read_file(dir.path(), "lib.rs");
        assert!(content.contains("new_foo()"));
        assert!(content.contains("new_bar()"));
    }

    #[test]
    fn test_apply_update_without_explicit_context_marker() {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "main.rs", "old();\nkeep();\n");
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("main.rs"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: vec!["old();".to_string()],
                new_lines: vec!["new();".to_string()],
                is_end_of_file: false,
            }],
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.modified.len(), 1);
        assert_eq!(read_file(dir.path(), "main.rs"), "new();\nkeep();\n");
    }

    #[test]
    fn test_apply_move_file() {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "old.rs", "fn test() {\n    v1();\n}\n");
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("old.rs"),
            move_path: Some(PathBuf::from("new.rs")),
            chunks: vec![UpdateFileChunk {
                change_context: Some("fn test() {".to_string()),
                old_lines: vec!["    v1();".to_string()],
                new_lines: vec!["    v2();".to_string()],
                is_end_of_file: false,
            }],
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.moved.len(), 1);
        assert!(!dir.path().join("old.rs").exists());
        let content = read_file(dir.path(), "new.rs");
        assert!(content.contains("v2()"));
    }

    #[test]
    fn test_seek_sequence_exact_match() {
        let lines = vec!["fn main() {", "    hello();", "}"];
        assert_eq!(seek_sequence(&lines, "fn main() {", 0), Some(0));
        assert_eq!(seek_sequence(&lines, "    hello();", 0), Some(1));
        assert_eq!(seek_sequence(&lines, "}", 0), Some(2));
    }

    #[test]
    fn test_seek_sequence_trimmed_match() {
        let lines = vec!["  fn main() {  ", "    hello();"];
        // Trimmed match should work.
        assert_eq!(seek_sequence(&lines, "fn main() {", 0), Some(0));
    }

    #[test]
    fn test_apply_mixed_operations() {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "existing.txt", "line1\nline2\nline3\n");
        create_file(dir.path(), "to_delete.txt", "bye");

        let hunks = vec![
            Hunk::AddFile {
                path: PathBuf::from("brand_new.txt"),
                contents: "new content\n".to_string(),
            },
            Hunk::DeleteFile {
                path: PathBuf::from("to_delete.txt"),
            },
            Hunk::UpdateFile {
                path: PathBuf::from("existing.txt"),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: Some("line1".to_string()),
                    old_lines: vec!["line2".to_string()],
                    new_lines: vec!["modified_line2".to_string()],
                    is_end_of_file: false,
                }],
            },
        ];

        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.deleted.len(), 1);
        assert_eq!(result.modified.len(), 1);
    }

    #[test]
    fn test_apply_end_of_file() {
        let dir = TempDir::new().unwrap();
        create_file(
            dir.path(),
            "eof.rs",
            "fn main() {\n    old1();\n    old2();\n}\n",
        );
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("eof.rs"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: Some("fn main() {".to_string()),
                old_lines: vec![],
                new_lines: vec!["    new1();".to_string(), "    new2();".to_string()],
                is_end_of_file: true,
            }],
        }];
        let result = apply_patch(dir.path(), &hunks).unwrap();
        assert_eq!(result.modified.len(), 1);
        let content = read_file(dir.path(), "eof.rs");
        assert!(content.contains("new1()"));
        assert!(content.contains("new2()"));
    }

    #[test]
    fn test_apply_error_file_not_found() {
        let dir = TempDir::new().unwrap();
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("nonexistent.rs"),
            move_path: None,
            chunks: vec![],
        }];
        let err = apply_patch(dir.path(), &hunks).unwrap_err();
        assert!(matches!(err, ApplyError::FileNotFound(_)));
    }

    #[test]
    fn test_apply_rejects_absolute_path() {
        let dir = TempDir::new().unwrap();
        let hunks = vec![Hunk::AddFile {
            path: PathBuf::from("/tmp/absolute.txt"),
            contents: "hello\n".to_string(),
        }];
        let err = apply_patch(dir.path(), &hunks).unwrap_err();
        assert!(matches!(err, ApplyError::AbsolutePath(_)));
    }

    #[test]
    fn test_apply_error_context_not_found() {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "test.rs", "fn main() {\n    hello();\n}\n");
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("test.rs"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: Some("fn nonexistent() {".to_string()),
                old_lines: vec!["    something();".to_string()],
                new_lines: vec!["    other();".to_string()],
                is_end_of_file: false,
            }],
        }];
        let err = apply_patch(dir.path(), &hunks).unwrap_err();
        assert!(matches!(err, ApplyError::ContextNotFound { .. }));
    }

    #[test]
    fn test_apply_result_display() {
        let result = ApplyResult {
            added: vec![PathBuf::from("new.rs")],
            modified: vec![PathBuf::from("mod.rs")],
            deleted: vec![PathBuf::from("old.rs")],
            moved: vec![(PathBuf::from("a.rs"), PathBuf::from("b.rs"))],
        };
        let display = format!("{result}");
        assert!(display.contains("A  new.rs"));
        assert!(display.contains("M  mod.rs"));
        assert!(display.contains("D  old.rs"));
        assert!(display.contains("R  a.rs -> b.rs"));
    }
}
