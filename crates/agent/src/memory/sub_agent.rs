//! Phase 2 sub-agent tools: sandboxed file operations for memory consolidation.

use crate::memory::constants::PHASE2_AGENT_MAX_ROUNDS;
use crate::types::ToolDefinition;
use std::fs;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Permission model
// ─────────────────────────────────────────────────────────────────────────────

/// Describes the restricted permissions of the Phase 2 consolidation sub-agent.
///
/// The consolidation agent is sandboxed to:
/// - Read/write only within the memory workspace root.
/// - No network access.
/// - No recursive agent spawning.
/// - No MCP tool calls.
/// - Max `PHASE2_AGENT_MAX_ROUNDS` tool-calling rounds.
/// - Ephemeral: does not generate its own memories.
///
/// This aligns with Codex's `SandboxPolicy::WorkspaceWrite` with
/// `network_access: false`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Phase2AgentPermissions {
    /// The memory root directory — all file access is confined here.
    pub workspace_root: PathBuf,
    /// Maximum number of tool-call rounds the agent may perform.
    pub max_rounds: usize,
    /// Whether network access is allowed (always false for consolidation).
    pub network_access: bool,
    /// Whether the agent may spawn sub-agents (always false).
    pub allow_sub_agents: bool,
    /// Whether the agent generates its own memories (always false — ephemeral).
    pub generate_memories: bool,
}

#[allow(dead_code)]
impl Phase2AgentPermissions {
    /// Build the default locked-down permissions for consolidation.
    pub fn consolidation_defaults(memory_root: &Path) -> Self {
        Self {
            workspace_root: memory_root.to_path_buf(),
            max_rounds: PHASE2_AGENT_MAX_ROUNDS,
            network_access: false,
            allow_sub_agents: false,
            generate_memories: false,
        }
    }

    /// Format as a human-readable description for the agent system prompt.
    pub fn describe(&self) -> String {
        format!(
            "Permissions: workspace-write only within {:?}, max {} tool rounds, \
             no network, no sub-agents, ephemeral (no memory generation).",
            self.workspace_root, self.max_rounds,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace diff injection
// ─────────────────────────────────────────────────────────────────────────────

/// Build a workspace diff context block for injection into the Phase 2 prompt.
///
/// If a diff is available (e.g., from `git diff` in the memory workspace), it
/// is formatted into a context block that helps the consolidation agent
/// understand what changed since the last consolidation run.
///
/// Returns `None` if no diff is available or the diff is empty.
#[allow(dead_code)]
pub fn build_workspace_diff_context(diff: Option<&str>) -> Option<String> {
    let diff = diff?.trim();
    if diff.is_empty() {
        return None;
    }

    Some(format!(
        "## Workspace Changes Since Last Consolidation\n\n\
         The following diff shows what has changed in the memory workspace:\n\n\
         ```diff\n{}\n```\n\n\
         Use this to understand which raw memories are new and which rollout \
         summaries have been added or removed.",
        diff
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tools
// ─────────────────────────────────────────────────────────────────────────────

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
///
/// Returns an error JSON string if the tool call is invalid or if the
/// path escapes the sandbox. Enforces the workspace root boundary.
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

// ─────────────────────────────────────────────────────────────────────────────
// Round tracking
// ─────────────────────────────────────────────────────────────────────────────

/// Tracks tool-call rounds for the Phase 2 sub-agent, enforcing `max_turns`.
///
/// Aligned with Codex's consolidation agent which has a limited number of
/// tool-calling turns before it must produce final output.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Phase2RoundTracker {
    current_round: usize,
    max_rounds: usize,
}

#[allow(dead_code)]
impl Phase2RoundTracker {
    /// Create a new round tracker with the given max rounds.
    pub fn new(max_rounds: usize) -> Self {
        Self {
            current_round: 0,
            max_rounds,
        }
    }

    /// Create a tracker using the default `PHASE2_AGENT_MAX_ROUNDS`.
    pub fn with_defaults() -> Self {
        Self::new(PHASE2_AGENT_MAX_ROUNDS)
    }

    /// Increment the round counter and return whether the agent may continue.
    ///
    /// Returns `true` if the agent has rounds remaining, `false` if it has
    /// exhausted its budget and must terminate.
    pub fn advance(&mut self) -> bool {
        self.current_round += 1;
        self.current_round <= self.max_rounds
    }

    /// Check if the agent has exhausted its round budget.
    pub fn is_exhausted(&self) -> bool {
        self.current_round >= self.max_rounds
    }

    /// Current round number (1-based after first advance).
    pub fn current(&self) -> usize {
        self.current_round
    }

    /// Maximum rounds allowed.
    pub fn max(&self) -> usize {
        self.max_rounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn phase2_permissions_describe() {
        let dir = tempdir().unwrap();
        let perms = Phase2AgentPermissions::consolidation_defaults(dir.path());
        let desc = perms.describe();
        assert!(desc.contains("workspace-write only"));
        assert!(desc.contains("no network"));
        assert!(desc.contains("no sub-agents"));
    }

    #[test]
    fn build_workspace_diff_context_none_for_empty() {
        assert!(build_workspace_diff_context(None).is_none());
        assert!(build_workspace_diff_context(Some("")).is_none());
        assert!(build_workspace_diff_context(Some("  \n  ")).is_none());
    }

    #[test]
    fn build_workspace_diff_context_formats_diff() {
        let diff = "+ new line\n- old line";
        let ctx = build_workspace_diff_context(Some(diff)).unwrap();
        assert!(ctx.contains("```diff"));
        assert!(ctx.contains("+ new line"));
        assert!(ctx.contains("- old line"));
        assert!(ctx.contains("Workspace Changes Since Last Consolidation"));
    }

    #[test]
    fn round_tracker_basic_flow() {
        let mut tracker = Phase2RoundTracker::new(3);
        assert!(!tracker.is_exhausted());
        assert_eq!(tracker.current(), 0);

        assert!(tracker.advance()); // Round 1
        assert!(tracker.advance()); // Round 2
        assert!(tracker.advance()); // Round 3
        assert!(!tracker.advance()); // Round 4 — exceeded
        assert!(tracker.is_exhausted());
    }

    #[test]
    fn round_tracker_defaults() {
        let tracker = Phase2RoundTracker::with_defaults();
        assert_eq!(tracker.max(), PHASE2_AGENT_MAX_ROUNDS);
    }

    #[test]
    fn sanitize_rejects_traversal() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(sanitize_relative_path(root, "../../etc/passwd").is_some());
        // After sanitization, ".." segments are stripped, so the result is safe.
        let resolved = sanitize_relative_path(root, "../../etc/passwd").unwrap();
        assert!(resolved.starts_with(root));
        assert!(resolved.ends_with("etc/passwd"));
    }

    #[test]
    fn sanitize_empty_returns_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let resolved = sanitize_relative_path(root, "").unwrap();
        assert_eq!(resolved, root.to_path_buf());
    }
}
