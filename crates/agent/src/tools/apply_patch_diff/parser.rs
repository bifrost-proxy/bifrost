//! Structured diff patch parser.
//!
//! Parses the `*** Begin Patch / *** End Patch` format into typed `Hunk` structures.
//! Supports Add File, Delete File, and Update File (with optional Move To) operations.

use std::path::PathBuf;
use thiserror::Error;
use tracing::warn;

// ─── Constants ───────────────────────────────────────────────────────────────

pub const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub const END_PATCH_MARKER: &str = "*** End Patch";
pub const ADD_FILE_MARKER: &str = "*** Add File: ";
pub const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub const MOVE_TO_MARKER: &str = "*** Move To: ";
pub const MOVE_TO_MARKER_ALT: &str = "*** Move to: ";
pub const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub const CHANGE_CONTEXT_MARKER_BARE: &str = "@@";
pub const EOF_MARKER: &str = "*** End of File";

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum PatchError {
    #[error("parse error at line {line}: {message}")]
    ParseError { line: usize, message: String },
    #[error("invalid format: {0}")]
    InvalidFormat(String),
}

// ─── Hunk Types ──────────────────────────────────────────────────────────────

/// A single operation within a patch.
#[derive(Debug, Clone, PartialEq)]
pub enum Hunk {
    /// Create a new file with the given contents.
    AddFile { path: PathBuf, contents: String },
    /// Delete an existing file.
    DeleteFile { path: PathBuf },
    /// Update an existing file with diff chunks, optionally moving it.
    UpdateFile {
        path: PathBuf,
        move_path: Option<PathBuf>,
        chunks: Vec<UpdateFileChunk>,
    },
}

/// A single change chunk within an Update File hunk.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateFileChunk {
    /// The context line used to locate where to apply changes (from @@ marker).
    pub change_context: String,
    /// Lines from the original file (context + removed lines).
    pub old_lines: Vec<String>,
    /// Lines for the new file (context + added lines).
    pub new_lines: Vec<String>,
    /// Whether this chunk extends to the end of file.
    pub is_end_of_file: bool,
}

// ─── Parser ──────────────────────────────────────────────────────────────────

/// Parse a patch string into a list of hunks.
pub fn parse_patch(input: &str) -> Result<Vec<Hunk>, PatchError> {
    // Find *** Begin Patch and *** End Patch markers.
    let begin_idx = input
        .find(BEGIN_PATCH_MARKER)
        .ok_or_else(|| PatchError::InvalidFormat("missing *** Begin Patch marker".to_string()))?;
    let end_idx = input
        .find(END_PATCH_MARKER)
        .ok_or_else(|| PatchError::InvalidFormat("missing *** End Patch marker".to_string()))?;

    if end_idx <= begin_idx {
        return Err(PatchError::InvalidFormat(
            "*** End Patch appears before *** Begin Patch".to_string(),
        ));
    }

    // Extract content between markers.
    let start = begin_idx + BEGIN_PATCH_MARKER.len();
    let body = &input[start..end_idx];

    // Split into lines, stripping the leading newline after Begin marker.
    let lines: Vec<&str> = body.lines().collect();

    if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
        return Ok(Vec::new());
    }

    parse_hunks(&lines)
}

/// Parse a slice of lines into hunks.
fn parse_hunks(lines: &[&str]) -> Result<Vec<Hunk>, PatchError> {
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix(ADD_FILE_MARKER) {
            let path = path.trim();
            if path.is_empty() {
                return Err(PatchError::ParseError {
                    line: i + 1,
                    message: "Add File marker has empty path".to_string(),
                });
            }
            let (hunk, next_i) = parse_add_file(path, lines, i + 1)?;
            hunks.push(hunk);
            i = next_i;
        } else if let Some(path) = line.strip_prefix(DELETE_FILE_MARKER) {
            let path = path.trim();
            if path.is_empty() {
                return Err(PatchError::ParseError {
                    line: i + 1,
                    message: "Delete File marker has empty path".to_string(),
                });
            }
            hunks.push(Hunk::DeleteFile {
                path: PathBuf::from(path),
            });
            i += 1;
        } else if let Some(path) = line.strip_prefix(UPDATE_FILE_MARKER) {
            let path = path.trim();
            if path.is_empty() {
                return Err(PatchError::ParseError {
                    line: i + 1,
                    message: "Update File marker has empty path".to_string(),
                });
            }
            let (hunk, next_i) = parse_update_file(path, lines, i + 1)?;
            hunks.push(hunk);
            i = next_i;
        } else {
            warn!(line_num = i + 1, line = line, "skipping unrecognized line");
            i += 1;
        }
    }

    Ok(hunks)
}

/// Parse an Add File section. Content lines are collected until the next marker.
fn parse_add_file(path: &str, lines: &[&str], start: usize) -> Result<(Hunk, usize), PatchError> {
    let mut contents = Vec::new();
    let mut i = start;

    while i < lines.len() {
        let line = lines[i];
        if is_hunk_marker(line) {
            break;
        }
        // Add file lines may have a + prefix (Codex format) or be plain text.
        if let Some(stripped) = line.strip_prefix('+') {
            contents.push(stripped.to_string());
        } else {
            contents.push(line.to_string());
        }
        i += 1;
    }

    let content_str = if contents.is_empty() {
        String::new()
    } else {
        let mut s = contents.join("\n");
        s.push('\n');
        s
    };

    Ok((
        Hunk::AddFile {
            path: PathBuf::from(path),
            contents: content_str,
        },
        i,
    ))
}

/// Parse an Update File section, including optional Move To and change chunks.
fn parse_update_file(
    path: &str,
    lines: &[&str],
    start: usize,
) -> Result<(Hunk, usize), PatchError> {
    let mut i = start;
    let mut move_path = None;

    // Check for optional *** Move To: marker.
    if i < lines.len() {
        if let Some(mp) = strip_move_to_marker(lines[i]) {
            let mp = mp.trim();
            if !mp.is_empty() {
                move_path = Some(PathBuf::from(mp));
            }
            i += 1;
        }
    }

    // Parse change chunks (each starts with @@).
    let mut chunks = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if is_hunk_marker(line) && !is_change_context_marker(line) {
            break;
        }
        if is_change_context_marker(line) {
            let (chunk, next_i) = parse_update_chunk(lines, i)?;
            chunks.push(chunk);
            i = next_i;
        } else {
            // Unexpected line in update section, skip.
            warn!(
                line_num = i + 1,
                line = line,
                "unexpected line in Update File section"
            );
            i += 1;
        }
    }

    Ok((
        Hunk::UpdateFile {
            path: PathBuf::from(path),
            move_path,
            chunks,
        },
        i,
    ))
}

/// Parse a single @@ change chunk.
fn parse_update_chunk(
    lines: &[&str],
    start: usize,
) -> Result<(UpdateFileChunk, usize), PatchError> {
    let context_line = lines[start];
    let change_context = context_line
        .strip_prefix(CHANGE_CONTEXT_MARKER_BARE)
        .unwrap_or("")
        .trim_start()
        .to_string();

    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();
    let mut is_end_of_file = false;
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];

        // Check for EOF marker.
        if line == EOF_MARKER {
            is_end_of_file = true;
            i += 1;
            break;
        }

        // Check for next chunk or hunk marker.
        if is_change_context_marker(line) || is_hunk_marker(line) {
            break;
        }

        // Parse line based on prefix.
        if let Some(rest) = line.strip_prefix('-') {
            old_lines.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('+') {
            new_lines.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix(' ') {
            // Context line (space prefix): appears in both old and new.
            old_lines.push(rest.to_string());
            new_lines.push(rest.to_string());
        } else if line.is_empty() {
            // Empty line treated as empty context line.
            old_lines.push(String::new());
            new_lines.push(String::new());
        } else {
            // No recognized prefix — treat as context (best effort).
            warn!(
                line_num = i + 1,
                line = line,
                "line without recognized prefix, treating as context"
            );
            old_lines.push(line.to_string());
            new_lines.push(line.to_string());
        }

        i += 1;
    }

    Ok((
        UpdateFileChunk {
            change_context,
            old_lines,
            new_lines,
            is_end_of_file,
        },
        i,
    ))
}

/// Check if a line is a hunk-level marker (not a @@ chunk marker).
fn is_hunk_marker(line: &str) -> bool {
    line.starts_with(ADD_FILE_MARKER)
        || line.starts_with(DELETE_FILE_MARKER)
        || line.starts_with(UPDATE_FILE_MARKER)
        || line == EOF_MARKER
}

fn is_change_context_marker(line: &str) -> bool {
    line == CHANGE_CONTEXT_MARKER_BARE || line.starts_with(CHANGE_CONTEXT_MARKER)
}

fn strip_move_to_marker(line: &str) -> Option<&str> {
    line.strip_prefix(MOVE_TO_MARKER)
        .or_else(|| line.strip_prefix(MOVE_TO_MARKER_ALT))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_add_file() {
        let patch = "\
*** Begin Patch
*** Add File: src/new.rs
+fn main() {}
+// end
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::AddFile { path, contents } => {
                assert_eq!(path, &PathBuf::from("src/new.rs"));
                assert!(contents.contains("fn main() {}"));
                assert!(contents.contains("// end"));
            }
            _ => panic!("expected AddFile"),
        }
    }

    #[test]
    fn test_parse_simple_delete_file() {
        let patch = "\
*** Begin Patch
*** Delete File: src/old.rs
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::DeleteFile { path } => {
                assert_eq!(path, &PathBuf::from("src/old.rs"));
            }
            _ => panic!("expected DeleteFile"),
        }
    }

    #[test]
    fn test_parse_simple_update_file() {
        let patch = "\
*** Begin Patch
*** Update File: src/main.rs
@@ fn main() {
-    println!(\"Hello, world!\");
+    println!(\"Hello, Bifrost!\");
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                assert_eq!(path, &PathBuf::from("src/main.rs"));
                assert!(move_path.is_none());
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].change_context, "fn main() {");
                assert_eq!(
                    chunks[0].old_lines,
                    vec!["    println!(\"Hello, world!\");"]
                );
                assert_eq!(
                    chunks[0].new_lines,
                    vec!["    println!(\"Hello, Bifrost!\");"]
                );
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_with_multiple_chunks() {
        let patch = "\
*** Begin Patch
*** Update File: src/lib.rs
@@ fn foo() {
-    old_foo();
+    new_foo();
@@ fn bar() {
-    old_bar();
+    new_bar();
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks.len(), 2);
                assert_eq!(chunks[0].change_context, "fn foo() {");
                assert_eq!(chunks[1].change_context, "fn bar() {");
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_with_move() {
        let patch = "\
*** Begin Patch
*** Update File: src/old_name.rs
*** Move To: src/new_name.rs
@@ fn test() {
-    v1();
+    v2();
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::UpdateFile {
                path, move_path, ..
            } => {
                assert_eq!(path, &PathBuf::from("src/old_name.rs"));
                assert_eq!(
                    move_path.as_ref().unwrap(),
                    &PathBuf::from("src/new_name.rs")
                );
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_with_lowercase_move_to() {
        let patch = "\
*** Begin Patch
*** Update File: src/old_name.rs
*** Move to: src/new_name.rs
@@ fn renamed() {
-old();
+new();
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        match &hunks[0] {
            Hunk::UpdateFile { move_path, .. } => {
                assert_eq!(move_path.as_ref(), Some(&PathBuf::from("src/new_name.rs")));
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_chunk_with_bare_context_marker() {
        let patch = "\
*** Begin Patch
*** Update File: src/main.rs
@@
-old();
+new();
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        match &hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks.len(), 1);
                assert!(chunks[0].change_context.is_empty());
                assert_eq!(chunks[0].old_lines, vec!["old();"]);
                assert_eq!(chunks[0].new_lines, vec!["new();"]);
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let patch = "\
*** Begin Patch
*** Add File: new.txt
+hello
*** Delete File: old.txt
*** Update File: existing.txt
@@ line1
-old
+new
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 3);
        assert!(matches!(&hunks[0], Hunk::AddFile { .. }));
        assert!(matches!(&hunks[1], Hunk::DeleteFile { .. }));
        assert!(matches!(&hunks[2], Hunk::UpdateFile { .. }));
    }

    #[test]
    fn test_parse_end_of_file_marker() {
        let patch = "\
*** Begin Patch
*** Update File: src/main.rs
@@ fn main() {
-    old();
+    new();
*** End of File
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        match &hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                assert!(chunks[0].is_end_of_file);
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_missing_begin_marker() {
        let patch = "*** Update File: foo.rs\n*** End Patch";
        let result = parse_patch(patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Begin Patch"));
    }

    #[test]
    fn test_parse_missing_end_marker() {
        let patch = "*** Begin Patch\n*** Update File: foo.rs";
        let result = parse_patch(patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("End Patch"));
    }

    #[test]
    fn test_parse_empty_patch() {
        let patch = "*** Begin Patch\n*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert!(hunks.is_empty());
    }

    #[test]
    fn test_parse_context_lines_in_chunk() {
        let patch = "\
*** Begin Patch
*** Update File: src/main.rs
@@ fn main() {
 unchanged line 1
-    old_line();
+    new_line();
 unchanged line 2
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        match &hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                let chunk = &chunks[0];
                // Context lines go into both old and new
                assert_eq!(chunk.old_lines.len(), 3); // unchanged1, old_line, unchanged2
                assert_eq!(chunk.new_lines.len(), 3); // unchanged1, new_line, unchanged2
                assert_eq!(chunk.old_lines[0], "unchanged line 1");
                assert_eq!(chunk.old_lines[1], "    old_line();");
                assert_eq!(chunk.old_lines[2], "unchanged line 2");
                assert_eq!(chunk.new_lines[0], "unchanged line 1");
                assert_eq!(chunk.new_lines[1], "    new_line();");
                assert_eq!(chunk.new_lines[2], "unchanged line 2");
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_add_file_with_empty_content() {
        let patch = "\
*** Begin Patch
*** Add File: empty.txt
*** End Patch";
        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            Hunk::AddFile { path, contents } => {
                assert_eq!(path, &PathBuf::from("empty.txt"));
                assert!(contents.is_empty());
            }
            _ => panic!("expected AddFile"),
        }
    }
}
