//! `exec_command` tool surface.
//!
//! This tool intentionally reuses the existing PTY session backend so Bifrost
//! keeps one interactive process model for one-shot and interactive commands.

use crate::tools::pty_shell::{PtySessionManager, PtyShellTool};
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub struct ExecCommandTool {
    timeout_secs: u64,
    session_manager: Arc<PtySessionManager>,
}

impl ExecCommandTool {
    pub fn new(timeout_secs: u64, session_manager: Arc<PtySessionManager>) -> Self {
        Self {
            timeout_secs,
            session_manager,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExecCommandArgs {
    cmd: String,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    tty: Option<bool>,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

#[async_trait]
impl ToolHandler for ExecCommandTool {
    fn name(&self) -> &str {
        "exec_command"
    }

    fn description(&self) -> &str {
        "Runs a command in a PTY, returning output or a session ID for ongoing interaction. Set `tty=true` for interactive commands. In Bifrost Agent, prefer `shell_pty` with `wait_for_completion=false` plus `write_stdin` when the user needs a persistent foreground session, long-running observation, stdin forwarding, or delegated agent-style command."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "Shell command to execute."
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional working directory to run the command in; defaults to the turn cwd."
                },
                "shell": {
                    "type": "string",
                    "description": "Shell binary to launch. Defaults to the user's default shell."
                },
                "tty": {
                    "type": "boolean",
                    "description": "Whether to allocate a TTY for the command. Defaults to false (plain pipes); set true for interactive commands."
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "How long to wait (in milliseconds) for output before yielding."
                },
                "max_output_tokens": {
                    "type": "integer",
                    "description": "Maximum number of tokens to return. Excess output will be truncated."
                }
            },
            "required": ["cmd"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ExecCommandArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                };
            }
        };
        let run_dir = match args.workdir.as_deref() {
            Some(path) => {
                let candidate = resolve_path(path, work_dir);
                if !candidate.is_dir() {
                    return ToolResult {
                        success: false,
                        output: format!("workdir is not a directory: {}", candidate.display()),
                    };
                }
                candidate
            }
            None => work_dir.to_path_buf(),
        };

        let yield_time_ms = args.yield_time_ms.unwrap_or(1000);
        let real_pty = args.tty.unwrap_or(false);
        let wait_for_completion = true;
        let timeout_secs = if real_pty {
            yield_time_ms.div_ceil(1000).max(1)
        } else {
            self.timeout_secs
        };
        let pty_args = json!({
            "command": command_for_shell(args.shell.as_deref(), &args.cmd),
            "timeout": timeout_secs,
            "yield_time_ms": yield_time_ms,
            "wait_for_completion": wait_for_completion,
            "real_pty": real_pty
        });

        let start = Instant::now();
        let pty_tool = PtyShellTool::new(timeout_secs, self.session_manager.clone());
        let result = pty_tool.execute(&pty_args.to_string(), &run_dir).await;
        let wall_time_seconds = (start.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        if !result.success && !result.output.contains("exit_indicator: timeout") {
            return result;
        }

        let parsed = parse_pty_output(&result.output);
        let mut output = parsed.output;
        let original_token_count = estimate_tokens(&output);
        if let Some(limit) = args.max_output_tokens {
            output = truncate_to_token_budget(&output, limit);
        }

        let session_id = if real_pty
            || parsed.exit_indicator.as_deref() == Some("running")
            || parsed.exit_indicator.as_deref() == Some("timeout")
        {
            parsed.session_id
        } else {
            None
        };

        let response = json!({
            "chunk_id": null,
            "wall_time_seconds": wall_time_seconds,
            "exit_code": parsed.exit_code,
            "session_id": session_id,
            "original_token_count": original_token_count,
            "output": output
        });

        ToolResult {
            success: parsed.exit_code.unwrap_or(0) == 0 || session_id.is_some(),
            output: response.to_string(),
        }
    }
}

fn resolve_path(path: &str, work_dir: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    }
}

fn command_for_shell(shell: Option<&str>, cmd: &str) -> String {
    let Some(shell) = shell.filter(|value| !value.trim().is_empty()) else {
        return cmd.to_string();
    };
    if shell.ends_with("bash") || shell.ends_with("zsh") || shell.ends_with("sh") {
        format!("{} -lc {}", shell_quote(shell), shell_quote(cmd))
    } else {
        cmd.to_string()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Default)]
struct ParsedPtyOutput {
    session_id: Option<String>,
    exit_indicator: Option<String>,
    exit_code: Option<i32>,
    output: String,
}

fn parse_pty_output(text: &str) -> ParsedPtyOutput {
    let mut parsed = ParsedPtyOutput::default();
    let mut output = String::new();
    let mut section: Option<&str> = None;

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("session_id: ") {
            parsed.session_id = Some(value.trim().to_string());
            section = None;
        } else if let Some(value) = line.strip_prefix("exit_indicator: ") {
            parsed.exit_indicator = Some(value.trim().to_string());
            section = None;
        } else if let Some(value) = line.strip_prefix("exit_code: ") {
            parsed.exit_code = value.trim().parse().ok();
            section = None;
        } else if line == "stdout:" {
            section = Some("stdout");
        } else if line == "stderr:" {
            section = Some("stderr");
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("[stderr]\n");
        } else if section.is_some() {
            output.push_str(line);
            output.push('\n');
        }
    }

    parsed.output = output.trim_end().to_string();
    parsed
}

fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let max_bytes = max_tokens.saturating_mul(4);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[... output truncated ...]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn exec_command_returns_completed_output() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = ExecCommandTool::new(5, manager);
        let dir = tempdir().unwrap();
        let result = tool
            .execute(r#"{"cmd":"printf hello","yield_time_ms":50}"#, dir.path())
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["output"], "hello");
    }

    #[tokio::test]
    async fn test_exec_command_tty_reports_isatty_true() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = ExecCommandTool::new(5, manager);
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"python3 -c 'import os,sys; print(os.isatty(0), os.isatty(1))'","tty":true,"yield_time_ms":500}"#,
                dir.path(),
            )
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert!(
            value["output"].as_str().unwrap_or("").contains("True True"),
            "{}",
            value
        );
        assert!(
            value["session_id"]
                .as_str()
                .is_some_and(|sid| !sid.is_empty()),
            "{}",
            value
        );
    }

    #[test]
    fn parse_pty_output_extracts_stderr() {
        let parsed = parse_pty_output(
            "session_id: abc\nexit_indicator: done\nexit_code: 1\nstdout:\nout\nstderr:\nerr\n",
        );
        assert_eq!(parsed.session_id.as_deref(), Some("abc"));
        assert_eq!(parsed.exit_code, Some(1));
        assert_eq!(parsed.output, "out\n[stderr]\nerr");
    }
}
