//! Git worktree tools for isolated development.
//!
//! `EnterWorktreeTool` creates a new git worktree and switches the session's
//! working directory to it **without** clearing conversation history.
//! `ExitWorktreeTool` switches back to the original directory and optionally
//! removes the worktree.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use tracing::info;

// ---------------------------------------------------------------------------
// EnterWorktree
// ---------------------------------------------------------------------------

pub struct EnterWorktreeTool;

#[derive(Deserialize)]
struct EnterWorktreeArgs {
    /// Optional human-friendly name for the worktree (slugified for path safety).
    #[serde(default)]
    name: Option<String>,
    /// Optional branch name. Defaults to a generated name based on `name`.
    #[serde(default)]
    branch: Option<String>,
}

fn slugify(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn generate_worktree_path(work_dir: &Path, name: Option<&str>) -> (std::path::PathBuf, String) {
    let slug = match name {
        Some(n) if !n.trim().is_empty() => {
            let s = slugify(n);
            if s.len() > 50 {
                s[..50].to_string()
            } else {
                s
            }
        }
        _ => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("wt-{ts}")
        }
    };

    let repo_name = work_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let dir_name = format!("{repo_name}-{slug}");
    let parent = work_dir.parent().unwrap_or(work_dir);
    let path = parent.join(&dir_name);
    (path, slug)
}

#[async_trait]
impl ToolHandler for EnterWorktreeTool {
    fn name(&self) -> &str {
        "enter_worktree"
    }

    fn description(&self) -> &str {
        "Create a git worktree for isolated development and switch the session's working directory to it. Unlike switch_workdir, this preserves conversation history. Use when you need to work on a separate branch without affecting the main working tree."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional name for the worktree (converted to a safe slug). If omitted, a timestamp-based name is generated."
                },
                "branch": {
                    "type": "string",
                    "description": "Optional new branch name to create. If omitted, derived from the worktree name."
                }
            }
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: EnterWorktreeArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                    runtime_events: Vec::new(),
                };
            }
        };

        // Verify we're in a git repo
        let git_check = std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(work_dir)
            .output();
        match git_check {
            Ok(out) if out.status.success() => {}
            _ => {
                return ToolResult {
                    success: false,
                    output: "not inside a git repository".to_string(),
                    runtime_events: Vec::new(),
                };
            }
        }

        let (wt_path, slug) = generate_worktree_path(work_dir, args.name.as_deref());
        let branch_name = args.branch.unwrap_or_else(|| format!("worktree/{slug}"));

        // If the worktree path already exists, fail gracefully
        if wt_path.exists() {
            return ToolResult {
                success: false,
                output: format!(
                    "worktree path already exists: {}. Choose a different name.",
                    wt_path.display()
                ),
                runtime_events: Vec::new(),
            };
        }

        // Create the worktree with a new branch from HEAD
        let output = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                &wt_path.to_string_lossy(),
                "HEAD",
            ])
            .current_dir(work_dir)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let wt_abs = std::fs::canonicalize(&wt_path).unwrap_or_else(|_| wt_path.clone());
                info!(
                    worktree = %wt_abs.display(),
                    branch = %branch_name,
                    "enter_worktree: created worktree"
                );
                ToolResult {
                    success: true,
                    output: format!("ENTER_WORKTREE:{}:{}", wt_abs.display(), work_dir.display()),
                    runtime_events: Vec::new(),
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                ToolResult {
                    success: false,
                    output: format!("git worktree add failed: {stderr}"),
                    runtime_events: Vec::new(),
                }
            }
            Err(e) => ToolResult {
                success: false,
                output: format!("failed to run git: {e}"),
                runtime_events: Vec::new(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ExitWorktree
// ---------------------------------------------------------------------------

pub struct ExitWorktreeTool;

#[derive(Deserialize)]
struct ExitWorktreeArgs {
    /// "keep" leaves the worktree on disk; "remove" deletes it.
    #[serde(default = "default_action")]
    action: String,
}

fn default_action() -> String {
    "keep".to_string()
}

#[async_trait]
impl ToolHandler for ExitWorktreeTool {
    fn name(&self) -> &str {
        "exit_worktree"
    }

    fn description(&self) -> &str {
        "Exit the current git worktree and return to the original working directory. Use action='keep' to preserve the worktree for later, or action='remove' to delete it."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "Whether to keep or remove the worktree. Defaults to 'keep'."
                }
            }
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ExitWorktreeArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                    runtime_events: Vec::new(),
                };
            }
        };

        // The actual directory switch is handled by the turn loop reading the signal.
        // Here we just optionally remove the worktree.
        if args.action == "remove" {
            let result = std::process::Command::new("git")
                .args(["worktree", "remove", "--force", &work_dir.to_string_lossy()])
                .current_dir(work_dir.parent().unwrap_or(work_dir))
                .output();

            match result {
                Ok(out) if out.status.success() => {
                    info!(worktree = %work_dir.display(), "exit_worktree: removed worktree");
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    return ToolResult {
                        success: false,
                        output: format!("failed to remove worktree: {stderr}"),
                        runtime_events: Vec::new(),
                    };
                }
                Err(e) => {
                    return ToolResult {
                        success: false,
                        output: format!("failed to run git: {e}"),
                        runtime_events: Vec::new(),
                    };
                }
            }
        }

        ToolResult {
            success: true,
            output: format!("EXIT_WORKTREE:{}", args.action),
            runtime_events: Vec::new(),
        }
    }
}
