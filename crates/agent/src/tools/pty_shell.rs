//! PTY-like shell execution with persistent sessions.
//!
//! Provides two tools:
//! - `shell_pty`: Execute commands in persistent shell sessions with session reuse
//! - `write_stdin`: Write arbitrary input to a running session's stdin
//!
//! Sessions are managed by `PtySessionManager` which maintains active shell processes
//! that can be reused across multiple tool invocations. Commands are sent via stdin
//! with a sentinel marker to detect completion, providing PTY-like interactive behavior
//! without requiring actual PTY allocation dependencies.

use crate::tools::head_tail_buffer::HeadTailBuffer;
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use dashmap::DashMap;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

/// A persistent shell session with piped stdin/stdout/stderr.
pub struct PtySession {
    /// Unique identifier for this session.
    pub session_id: String,
    /// The shell child process.
    child: Mutex<Child>,
    /// Stdin handle for writing commands.
    stdin: Mutex<ChildStdin>,
    /// Buffered stdout output since last read.
    stdout_buffer: Mutex<HeadTailBuffer>,
    /// Buffered stderr output since last read.
    stderr_buffer: Mutex<HeadTailBuffer>,
    /// When this session was created.
    created_at: Instant,
}

/// Manages persistent shell sessions.
pub struct PtySessionManager {
    sessions: DashMap<String, Arc<PtySession>>,
}

impl Default for PtySessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtySessionManager {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Create a new persistent shell session in the given working directory.
    pub fn create_session(&self, work_dir: &Path) -> Result<Arc<PtySession>, String> {
        create_session_internal(self, work_dir)
    }

    /// Get an existing session by ID.
    pub fn get_session(&self, session_id: &str) -> Option<Arc<PtySession>> {
        self.sessions.get(session_id).map(|s| s.value().clone())
    }

    /// Remove and return a session by ID.
    pub fn remove_session(&self, session_id: &str) -> Option<Arc<PtySession>> {
        self.sessions.remove(session_id).map(|(_, s)| s)
    }
}

/// Tool that executes commands in persistent PTY-like shell sessions.
pub struct PtyShellTool {
    timeout_secs: u64,
    session_manager: Arc<PtySessionManager>,
}

impl PtyShellTool {
    pub fn new(timeout_secs: u64, session_manager: Arc<PtySessionManager>) -> Self {
        Self {
            timeout_secs,
            session_manager,
        }
    }
}

/// Tool that writes arbitrary input to a running session's stdin.
pub struct WriteStdinTool {
    session_manager: Arc<PtySessionManager>,
}

impl WriteStdinTool {
    pub fn new(session_manager: Arc<PtySessionManager>) -> Self {
        Self { session_manager }
    }
}

#[derive(Deserialize)]
struct PtyShellArgs {
    command: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    wait_for_completion: Option<bool>,
}

#[derive(Deserialize)]
struct WriteStdinArgs {
    session_id: String,
    input: String,
    #[serde(default)]
    yield_time_ms: Option<u64>,
}

/// Detect the appropriate shell for the current system.
fn detect_shell() -> (&'static str, &'static str) {
    if let Ok(shell) = std::env::var("SHELL") {
        if shell.contains("zsh") {
            return ("zsh", "-i");
        }
        if shell.contains("bash") {
            return ("bash", "-i");
        }
    }
    #[cfg(target_os = "macos")]
    {
        ("zsh", "-i")
    }
    #[cfg(not(target_os = "macos"))]
    {
        ("bash", "-i")
    }
}

#[async_trait]
impl ToolHandler for PtyShellTool {
    fn name(&self) -> &str {
        "shell_pty"
    }

    fn description(&self) -> &str {
        "Execute a command in a persistent PTY-like shell session. Sessions persist across calls, allowing stateful interactions (cd, environment variables, running processes). Returns a session_id for reuse."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute in the session"
                },
                "session_id": {
                    "type": "string",
                    "description": "Optional session ID to reuse an existing session. If omitted, a new session is created."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in seconds for command completion (default: configured timeout)"
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "Time in ms to wait for output after command appears done (default: 500)"
                },
                "wait_for_completion": {
                    "type": "boolean",
                    "description": "Whether to wait for the command to finish via sentinel detection. Set false for interactive foreground programs that will be driven later via write_stdin."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: PtyShellArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                };
            }
        };

        let timeout_secs = args.timeout.unwrap_or(self.timeout_secs);
        let yield_time_ms = args.yield_time_ms.unwrap_or(500);
        let wait_for_completion = args.wait_for_completion.unwrap_or(true);

        // Get or create session.
        let session = if let Some(ref sid) = args.session_id {
            match self.session_manager.get_session(sid) {
                Some(s) => s,
                None => {
                    return ToolResult {
                        success: false,
                        output: format!("session not found: {sid}"),
                    };
                }
            }
        } else {
            match create_session_internal(&self.session_manager, work_dir) {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult {
                        success: false,
                        output: e,
                    };
                }
            }
        };

        let session_id = session.session_id.clone();

        info!(
            session_id = %session_id,
            command = %args.command,
            timeout_secs,
            "executing command in PTY session"
        );

        // Generate a unique sentinel to detect command completion.
        let sentinel = format!(
            "___BIFROST_CMD_DONE_{}_{}___",
            Uuid::new_v4(),
            std::process::id()
        );
        let sentinel_prefix = format!("{sentinel}:");

        // Drain any existing output from the buffers before executing.
        {
            let mut stdout_buf = session.stdout_buffer.lock().await;
            stdout_buf.drain_chunks();
        }
        {
            let mut stderr_buf = session.stderr_buffer.lock().await;
            stderr_buf.drain_chunks();
        }

        // Write command + sentinel echo to stdin.
        {
            let mut stdin = session.stdin.lock().await;
            let cmd_str = if wait_for_completion {
                format!(
                    "{}\n__bifrost_exit_code=$?\nprintf '%s:%s\\n' '{}' \"$__bifrost_exit_code\"\n",
                    args.command, sentinel
                )
            } else {
                format!("{}\n", args.command)
            };
            if let Err(e) = stdin.write_all(cmd_str.as_bytes()).await {
                return ToolResult {
                    success: false,
                    output: format!("failed to write to session stdin: {e}"),
                };
            }
            if let Err(e) = stdin.flush().await {
                return ToolResult {
                    success: false,
                    output: format!("failed to flush session stdin: {e}"),
                };
            }
        }

        let timeout_duration = Duration::from_secs(timeout_secs);
        let yield_duration = Duration::from_millis(yield_time_ms);
        let mut found_sentinel = false;
        let mut exit_code = None;

        if wait_for_completion {
            let start = Instant::now();
            loop {
                if start.elapsed() >= timeout_duration {
                    break;
                }

                // Check stdout buffer for sentinel.
                {
                    let stdout_buf = session.stdout_buffer.lock().await;
                    let bytes = stdout_buf.to_bytes();
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains(&sentinel_prefix) {
                        found_sentinel = true;
                        exit_code = extract_exit_code(&text, &sentinel_prefix);
                        break;
                    }
                }

                // Small poll interval to avoid busy-waiting.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        // Extra yield time to capture trailing output.
        tokio::time::sleep(yield_duration).await;

        // Collect output.
        let stdout_text = {
            let stdout_buf = session.stdout_buffer.lock().await;
            let text = stdout_buf.to_formatted_string();
            // Remove sentinel line from output.
            text.lines()
                .filter(|line| !line.contains(&sentinel_prefix))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let stderr_text = {
            let stderr_buf = session.stderr_buffer.lock().await;
            stderr_buf.to_formatted_string()
        };

        let exit_indicator = if wait_for_completion {
            if found_sentinel {
                "done"
            } else {
                "timeout"
            }
        } else {
            "running"
        };

        debug!(
            session_id = %session_id,
            exit_indicator,
            stdout_len = stdout_text.len(),
            stderr_len = stderr_text.len(),
            "PTY command completed"
        );

        let mut output = format!("session_id: {session_id}\nexit_indicator: {exit_indicator}\n");
        if let Some(exit_code) = exit_code {
            output.push_str(&format!("exit_code: {exit_code}\n"));
        }
        if !stdout_text.is_empty() {
            output.push_str(&format!("stdout:\n{stdout_text}\n"));
        }
        if !stderr_text.is_empty() {
            output.push_str(&format!("stderr:\n{stderr_text}\n"));
        }

        ToolResult {
            success: !wait_for_completion || found_sentinel,
            output,
        }
    }
}

fn extract_exit_code(text: &str, sentinel_prefix: &str) -> Option<i32> {
    text.lines().find_map(|line| {
        line.strip_prefix(sentinel_prefix)
            .and_then(|value| value.trim().parse::<i32>().ok())
    })
}

#[async_trait]
impl ToolHandler for WriteStdinTool {
    fn name(&self) -> &str {
        "write_stdin"
    }

    fn description(&self) -> &str {
        "Write arbitrary input to a running PTY session's stdin. Use this to interact with running processes (e.g., answer prompts, send signals via text, provide input to interactive programs)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "The session ID to write input to"
                },
                "input": {
                    "type": "string",
                    "description": "The text to write to the session's stdin"
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "Time in ms to wait for response after writing (default: 500)"
                }
            },
            "required": ["session_id", "input"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: WriteStdinArgs = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {e}"),
                };
            }
        };

        let yield_time_ms = args.yield_time_ms.unwrap_or(500);

        let session = match self.session_manager.get_session(&args.session_id) {
            Some(s) => s,
            None => {
                return ToolResult {
                    success: false,
                    output: format!("session not found: {}", args.session_id),
                };
            }
        };

        info!(
            session_id = %args.session_id,
            input_len = args.input.len(),
            "writing to PTY session stdin"
        );

        // Drain buffers before writing to capture only new output.
        {
            let mut stdout_buf = session.stdout_buffer.lock().await;
            stdout_buf.drain_chunks();
        }
        {
            let mut stderr_buf = session.stderr_buffer.lock().await;
            stderr_buf.drain_chunks();
        }

        // Write input to stdin.
        {
            let mut stdin = session.stdin.lock().await;
            if let Err(e) = stdin.write_all(args.input.as_bytes()).await {
                return ToolResult {
                    success: false,
                    output: format!("failed to write to session stdin: {e}"),
                };
            }
            if let Err(e) = stdin.flush().await {
                return ToolResult {
                    success: false,
                    output: format!("failed to flush session stdin: {e}"),
                };
            }
        }

        // Wait for output.
        tokio::time::sleep(Duration::from_millis(yield_time_ms)).await;

        // Collect output.
        let stdout_text = {
            let stdout_buf = session.stdout_buffer.lock().await;
            stdout_buf.to_formatted_string()
        };
        let stderr_text = {
            let stderr_buf = session.stderr_buffer.lock().await;
            stderr_buf.to_formatted_string()
        };

        let mut output = format!("session_id: {}\n", args.session_id);
        if !stdout_text.is_empty() {
            output.push_str(&format!("stdout:\n{stdout_text}\n"));
        }
        if !stderr_text.is_empty() {
            output.push_str(&format!("stderr:\n{stderr_text}\n"));
        }
        if stdout_text.is_empty() && stderr_text.is_empty() {
            output.push_str("(no output captured)\n");
        }

        ToolResult {
            success: true,
            output,
        }
    }
}

/// Internal helper to create a session with proper shared buffer architecture.
fn create_session_internal(
    manager: &PtySessionManager,
    work_dir: &Path,
) -> Result<Arc<PtySession>, String> {
    let (shell, _) = detect_shell();
    let session_id = Uuid::new_v4().to_string();

    info!(
        session_id = %session_id,
        shell = shell,
        cwd = %work_dir.display(),
        "creating new PTY session"
    );

    let mut child = Command::new(shell)
        .arg("-i")
        .current_dir(work_dir)
        .envs(std::env::vars())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn shell: {e}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to capture stdin".to_string())?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let session = Arc::new(PtySession {
        session_id: session_id.clone(),
        child: Mutex::new(child),
        stdin: Mutex::new(stdin),
        stdout_buffer: Mutex::new(HeadTailBuffer::default()),
        stderr_buffer: Mutex::new(HeadTailBuffer::default()),
        created_at: Instant::now(),
    });

    // Spawn background task to read stdout into session's buffer.
    let session_stdout = session.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let mut buf = session_stdout.stdout_buffer.lock().await;
            buf.push_chunk(line.into_bytes());
            buf.push_chunk(b"\n".to_vec());
        }
        debug!("stdout reader task ended for session");
    });

    // Spawn background task to read stderr into session's buffer.
    let session_stderr = session.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let mut buf = session_stderr.stderr_buffer.lock().await;
            buf.push_chunk(line.into_bytes());
            buf.push_chunk(b"\n".to_vec());
        }
        debug!("stderr reader task ended for session");
    });

    manager.sessions.insert(session_id, session.clone());
    Ok(session)
}

impl PtySession {
    /// Get the session's age.
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Check if the session's child process is still running.
    pub async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        matches!(child.try_wait(), Ok(None))
    }

    /// Kill the session's child process.
    pub async fn kill(&self) {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}

impl Drop for PtySessionManager {
    fn drop(&mut self) {
        // Best-effort cleanup: sessions will be dropped and children will
        // be killed by the OS when their handles are dropped.
        self.sessions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        PathBuf::from("/tmp")
    }

    #[tokio::test]
    async fn test_session_manager_create_and_get() {
        let manager = PtySessionManager::new();
        let session = create_session_internal(&manager, &tmp_dir()).unwrap();
        let session_id = session.session_id.clone();

        let retrieved = manager.get_session(&session_id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, session_id);

        // Cleanup
        session.kill().await;
    }

    #[tokio::test]
    async fn test_session_manager_remove() {
        let manager = PtySessionManager::new();
        let session = create_session_internal(&manager, &tmp_dir()).unwrap();
        let session_id = session.session_id.clone();

        let removed = manager.remove_session(&session_id);
        assert!(removed.is_some());
        assert!(manager.get_session(&session_id).is_none());

        // Cleanup
        session.kill().await;
    }

    #[tokio::test]
    async fn test_pty_shell_simple_command() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(30, manager.clone());

        let args = serde_json::json!({
            "command": "echo hello_pty_test"
        });
        let result = tool.execute(&args.to_string(), &tmp_dir()).await;

        assert!(result.output.contains("session_id:"));
        assert!(result.output.contains("exit_code: 0"));
        assert!(result.output.contains("hello_pty_test"));

        // Cleanup all sessions
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_pty_shell_with_session_reuse() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(30, manager.clone());

        // First command: set a variable
        let args1 = serde_json::json!({
            "command": "export MY_PTY_VAR=bifrost_test_42"
        });
        let result1 = tool.execute(&args1.to_string(), &tmp_dir()).await;
        assert!(result1.output.contains("session_id:"));

        // Extract session_id from output.
        let session_id = result1
            .output
            .lines()
            .find(|l| l.starts_with("session_id:"))
            .unwrap()
            .trim_start_matches("session_id: ")
            .trim()
            .to_string();

        // Second command: read the variable using same session
        let args2 = serde_json::json!({
            "command": "echo $MY_PTY_VAR",
            "session_id": session_id
        });
        let result2 = tool.execute(&args2.to_string(), &tmp_dir()).await;

        assert!(result2.output.contains("bifrost_test_42"));
        assert!(result2.output.contains(&session_id));

        // Cleanup
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_write_stdin_to_session() {
        let manager = Arc::new(PtySessionManager::new());
        let pty_tool = PtyShellTool::new(30, manager.clone());
        let stdin_tool = WriteStdinTool::new(manager.clone());

        // Create a session with a foreground command that reads from stdin.
        let args = serde_json::json!({
            "command": "python3 -u -c 'import sys; print(sys.stdin.readline().strip())'",
            "wait_for_completion": false,
            "yield_time_ms": 500
        });
        let result = pty_tool.execute(&args.to_string(), &tmp_dir()).await;
        assert!(result.success);
        assert!(result.output.contains("exit_indicator: running"));

        let session_id = result
            .output
            .lines()
            .find(|l| l.starts_with("session_id:"))
            .unwrap()
            .trim_start_matches("session_id: ")
            .trim()
            .to_string();

        // Write to stdin
        let write_args = serde_json::json!({
            "session_id": session_id,
            "input": "hello from stdin\n",
            "yield_time_ms": 1000
        });
        let write_result = stdin_tool
            .execute(&write_args.to_string(), &tmp_dir())
            .await;

        assert!(write_result.success);
        assert!(write_result.output.contains(&session_id));
        assert!(write_result.output.contains("hello from stdin"));

        // Cleanup
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_pty_shell_timeout() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(2, manager.clone());

        // Command that will never produce the sentinel (sleep longer than timeout)
        let args = serde_json::json!({
            "command": "sleep 10",
            "timeout": 1
        });
        let result = tool.execute(&args.to_string(), &tmp_dir()).await;

        assert!(!result.success);
        assert!(result.output.contains("timeout"));

        // Cleanup
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_pty_shell_creates_new_session() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(30, manager.clone());

        // No session_id provided should create new session
        let args = serde_json::json!({
            "command": "echo new_session_test"
        });
        let result = tool.execute(&args.to_string(), &tmp_dir()).await;

        assert!(result.output.contains("session_id:"));
        assert!(result.output.contains("exit_code: 0"));
        assert!(result.output.contains("new_session_test"));
        assert!(!manager.sessions.is_empty());

        // Cleanup
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_session_manager_concurrent_access() {
        let manager = Arc::new(PtySessionManager::new());

        // Create multiple sessions concurrently.
        let mut handles = vec![];
        for _ in 0..4 {
            let mgr = manager.clone();
            let handle = tokio::spawn(async move {
                let session = create_session_internal(&mgr, &PathBuf::from("/tmp")).unwrap();
                session.session_id.clone()
            });
            handles.push(handle);
        }

        let mut session_ids = vec![];
        for handle in handles {
            session_ids.push(handle.await.unwrap());
        }

        // All sessions should be unique and accessible.
        assert_eq!(session_ids.len(), 4);
        for sid in &session_ids {
            assert!(manager.get_session(sid).is_some());
        }

        // All session IDs should be distinct.
        let mut unique = session_ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 4);

        // Cleanup
        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_pty_shell_reports_non_zero_exit_code() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(30, manager.clone());

        let args = serde_json::json!({
            "command": "false"
        });
        let result = tool.execute(&args.to_string(), &tmp_dir()).await;

        assert!(result.success);
        assert!(result.output.contains("exit_indicator: done"));
        assert!(result.output.contains("exit_code: 1"));

        manager.sessions.clear();
    }

    #[tokio::test]
    async fn test_pty_shell_interactive_mode_returns_running() {
        let manager = Arc::new(PtySessionManager::new());
        let tool = PtyShellTool::new(30, manager.clone());

        let args = serde_json::json!({
            "command": "python3 -u -c 'import time; print(\"ready\"); time.sleep(2)'",
            "wait_for_completion": false,
            "yield_time_ms": 500
        });
        let result = tool.execute(&args.to_string(), &tmp_dir()).await;

        assert!(result.success);
        assert!(result.output.contains("exit_indicator: running"));
        assert!(result.output.contains("session_id:"));

        manager.sessions.clear();
    }

    #[test]
    fn test_detect_shell_returns_valid() {
        let (shell, flag) = detect_shell();
        assert!(!shell.is_empty());
        assert!(!flag.is_empty());
    }

    #[tokio::test]
    async fn test_session_is_alive() {
        let manager = PtySessionManager::new();
        let session = create_session_internal(&manager, &tmp_dir()).unwrap();

        assert!(session.is_alive().await);

        session.kill().await;
        // Give the process a moment to exit.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!session.is_alive().await);
    }
}
