//! Shell command execution tool.
//!
//! Features:
//! - Auto-detects system shell (zsh on macOS, bash on Linux)
//! - Inherits environment variables from parent process
//! - Configurable timeout with per-command override
//! - Streaming output with HeadTailBuffer to prevent memory exhaustion
//!   (retains up to 1 MiB: 512 KiB head + 512 KiB tail)

use crate::tools::head_tail_buffer::HeadTailBuffer;
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

/// Tool that executes shell commands with auto-detected shell.
pub struct ShellTool {
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Detect the appropriate shell for the current system.
    /// On macOS, prefer zsh; on Linux, use bash.
    fn detect_shell() -> (&'static str, &'static str) {
        // Check explicit SHELL env var first
        if let Ok(shell) = std::env::var("SHELL") {
            if shell.contains("zsh") {
                return ("zsh", "-lc");
            } else if shell.contains("bash") {
                return ("bash", "-lc");
            } else if shell.contains("fish") {
                return ("fish", "-c");
            }
        }

        // Fallback based on OS
        #[cfg(target_os = "macos")]
        {
            ("zsh", "-lc")
        }
        #[cfg(target_os = "linux")]
        {
            ("bash", "-lc")
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            ("bash", "-lc")
        }
    }
}

#[derive(Deserialize)]
struct ShellArgs {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[async_trait]
impl ToolHandler for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Auto-detects zsh/bash based on system. Returns stdout, stderr, and exit code. Use for running scripts, installing packages, compilation, file operations, searching (use `rg` for grep), etc."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute (runs via zsh -lc on macOS, bash -lc on Linux)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in seconds (default: configured timeout)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ShellArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                }
            }
        };

        let timeout_duration = Duration::from_secs(args.timeout.unwrap_or(self.timeout_secs));
        let (shell, flag) = Self::detect_shell();

        info!(
            command = %args.command,
            cwd = %work_dir.display(),
            shell,
            timeout_secs = timeout_duration.as_secs(),
            "executing shell command"
        );

        // Spawn the process with piped stdout/stderr for streaming
        let mut child = match Command::new(shell)
            .arg(flag)
            .arg(&args.command)
            .current_dir(work_dir)
            .envs(std::env::vars())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("failed to spawn shell: {e}"),
                }
            }
        };

        // Set up streaming readers with HeadTailBuffer (1 MiB limit per stream)
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_buffer = HeadTailBuffer::default();
        let mut stderr_buffer = HeadTailBuffer::default();

        // Stream output with timeout
        let result = tokio::time::timeout(timeout_duration, async {
            // Read stdout and stderr concurrently using tokio::select
            let mut stdout_done = false;
            let mut stderr_done = false;

            while !stdout_done || !stderr_done {
                tokio::select! {
                    line = stdout_reader.next_line(), if !stdout_done => {
                        match line {
                            Ok(Some(l)) => {
                                stdout_buffer.push_chunk(l.into_bytes());
                                stdout_buffer.push_chunk(b"\n".to_vec());
                            }
                            Ok(None) => stdout_done = true,
                            Err(e) => {
                                warn!(error = %e, "error reading stdout");
                                stdout_done = true;
                            }
                        }
                    }
                    line = stderr_reader.next_line(), if !stderr_done => {
                        match line {
                            Ok(Some(l)) => {
                                stderr_buffer.push_chunk(l.into_bytes());
                                stderr_buffer.push_chunk(b"\n".to_vec());
                            }
                            Ok(None) => stderr_done = true,
                            Err(e) => {
                                warn!(error = %e, "error reading stderr");
                                stderr_done = true;
                            }
                        }
                    }
                }
            }

            // Wait for process to exit
            let status = child.wait().await;
            status
        })
        .await;

        match result {
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);
                let success = status.success();

                let stdout_str = stdout_buffer.to_formatted_string();
                let stderr_str = stderr_buffer.to_formatted_string();

                info!(
                    exit_code,
                    success,
                    stdout_bytes = stdout_buffer.total_bytes(),
                    stderr_bytes = stderr_buffer.total_bytes(),
                    stdout_omitted = stdout_buffer.omitted_bytes(),
                    stderr_omitted = stderr_buffer.omitted_bytes(),
                    "shell command completed"
                );

                let mut output_text = format!("exit_code: {exit_code}\n");
                if !stdout_str.is_empty() {
                    output_text.push_str(&format!("stdout:\n{stdout_str}\n"));
                }
                if !stderr_str.is_empty() {
                    output_text.push_str(&format!("stderr:\n{stderr_str}\n"));
                }

                ToolResult {
                    success,
                    output: output_text,
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "shell command execution failed");
                ToolResult {
                    success: false,
                    output: format!("failed to execute command: {e}"),
                }
            }
            Err(_) => {
                // Timeout - kill the process
                let _ = child.kill().await;
                warn!(
                    command = %args.command,
                    timeout_secs = timeout_duration.as_secs(),
                    "shell command timed out"
                );
                ToolResult {
                    success: false,
                    output: format!(
                        "command timed out after {} seconds",
                        timeout_duration.as_secs()
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_shell() {
        let (shell, flag) = ShellTool::detect_shell();
        // Should return a valid shell
        assert!(!shell.is_empty());
        assert!(!flag.is_empty());
    }

    #[tokio::test]
    async fn test_shell_echo() {
        let tool = ShellTool::new(30);
        let args = serde_json::json!({
            "command": "echo hello"
        });
        let result = tool.execute(&args.to_string(), Path::new("/tmp")).await;
        assert!(result.success);
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("exit_code: 0"));
    }

    #[tokio::test]
    async fn test_shell_timeout() {
        let tool = ShellTool::new(1); // 1 second timeout
        let args = serde_json::json!({
            "command": "sleep 5"
        });
        let result = tool.execute(&args.to_string(), Path::new("/tmp")).await;
        assert!(!result.success);
        assert!(result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn test_shell_large_output_is_buffered() {
        // Generate ~2 MiB of output, should be truncated to ~1 MiB by HeadTailBuffer
        let tool = ShellTool::new(60);
        // Each line is ~64 chars, 32768 lines = ~2 MiB
        let args = serde_json::json!({
            "command": "for i in $(seq 1 32768); do echo \"Line $i: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; done"
        });
        let result = tool.execute(&args.to_string(), Path::new("/tmp")).await;
        assert!(result.success);
        // Should contain head and tail, with omitted bytes notice
        assert!(result.output.contains("Line 1:"));
        assert!(result.output.contains("Line 32768:"));
        // The output should be much smaller than 2 MiB due to buffering
        assert!(result.output.len() < 2 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_shell_stderr() {
        let tool = ShellTool::new(30);
        let args = serde_json::json!({
            "command": "echo stdout_text && echo stderr_text >&2"
        });
        let result = tool.execute(&args.to_string(), Path::new("/tmp")).await;
        assert!(result.success);
        assert!(result.output.contains("stdout_text"));
        assert!(result.output.contains("stderr_text"));
    }
}
