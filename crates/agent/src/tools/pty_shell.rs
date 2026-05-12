//! PTY/PTY-like shell execution with persistent sessions.
//!
//! Provides two tools:
//! - `shell_pty`: Execute commands in persistent shell sessions with session reuse
//! - `write_stdin`: Write arbitrary input to a running session's stdin
//!
//! Sessions are managed by `PtySessionManager` which maintains active shell processes
//! that can be reused across multiple tool invocations. Commands are sent via stdin
//! with a sentinel marker to detect completion. `exec_command` uses the real PTY
//! backend when `tty=true`; plain pipe sessions remain available for existing
//! non-TTY command execution.

use crate::tools::exec_command::{ExecSessionManager, ExecWriteArgs};
use crate::tools::head_tail_buffer::HeadTailBuffer;
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use dashmap::DashMap;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

enum PtySessionBackend {
    Pipe {
        child: Mutex<Child>,
        stdin: Mutex<ChildStdin>,
    },
    RealPty {
        child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
        writer: Mutex<Box<dyn Write + Send>>,
    },
}

/// A persistent shell session with either piped stdio or a real PTY.
pub struct PtySession {
    /// Unique identifier for this session.
    pub session_id: String,
    backend: PtySessionBackend,
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
        create_session_internal(self, work_dir, false)
    }

    /// Create a new real PTY-backed persistent shell session.
    pub fn create_real_pty_session(&self, work_dir: &Path) -> Result<Arc<PtySession>, String> {
        create_session_internal(self, work_dir, true)
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
    exec_session_manager: Arc<ExecSessionManager>,
}

impl WriteStdinTool {
    pub fn new(
        session_manager: Arc<PtySessionManager>,
        exec_session_manager: Arc<ExecSessionManager>,
    ) -> Self {
        Self {
            session_manager,
            exec_session_manager,
        }
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
    #[serde(default)]
    real_pty: Option<bool>,
}

#[derive(Deserialize)]
struct WriteStdinArgs {
    session_id: SessionIdArg,
    #[serde(default)]
    chars: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SessionIdArg {
    Number(i64),
    String(String),
}

impl SessionIdArg {
    fn into_string(self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::String(value) => value,
        }
    }
}

/// Detect the appropriate shell for the current system.
struct ShellSpec {
    program: &'static str,
    args: &'static [&'static str],
}

fn detect_shell() -> ShellSpec {
    if let Ok(shell) = std::env::var("SHELL") {
        if shell.contains("zsh") {
            return ShellSpec {
                program: "zsh",
                args: &["-f"],
            };
        }
        if shell.contains("bash") {
            return ShellSpec {
                program: "bash",
                args: &["--noprofile", "--norc"],
            };
        }
    }
    #[cfg(target_os = "macos")]
    {
        ShellSpec {
            program: "zsh",
            args: &["-f"],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        ShellSpec {
            program: "bash",
            args: &["--noprofile", "--norc"],
        }
    }
}

#[async_trait]
impl ToolHandler for PtyShellTool {
    fn name(&self) -> &str {
        "shell_pty"
    }

    fn description(&self) -> &str {
        "Execute a command in a persistent PTY-like shell session. Use this for long-running foreground commands, servers/watchers, TUI/readline programs, interactive CLIs, delegated agent-style tasks, and commands that wait for stdin. Sessions persist across calls, allowing stateful interactions (cd, environment variables, running processes). For foreground programs that should remain observable, set `wait_for_completion=false`, keep the returned `session_id`, and continue with `write_stdin`."
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
                    "description": "Whether to wait for the command to finish via sentinel detection. Set false for long-running, interactive, stdin-waiting, or delegated foreground programs that will be observed or driven later via write_stdin."
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

        let real_pty = args.real_pty.unwrap_or(false);

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
            match create_session_internal(&self.session_manager, work_dir, real_pty) {
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
            let cmd_str = if wait_for_completion {
                format!(
                    "{}\n__bifrost_exit_code=$?\nprintf '%s:%s\\n' '{}' \"$__bifrost_exit_code\"\n",
                    args.command, sentinel
                )
            } else {
                format!("{}\n", args.command)
            };
            if let Err(e) = session.write_all(cmd_str.as_bytes()).await {
                return ToolResult {
                    success: false,
                    output: format!("failed to write to session stdin: {e}"),
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
            strip_sentinel_from_output(&stdout_buf.to_formatted_string(), &sentinel_prefix)
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

fn strip_sentinel_from_output(text: &str, sentinel_prefix: &str) -> String {
    text.lines()
        .filter_map(|line| match line.find(sentinel_prefix) {
            Some(0) => None,
            Some(index) => Some(line[..index].to_string()),
            None => Some(line.to_string()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_exit_code(text: &str, sentinel_prefix: &str) -> Option<i32> {
    text.lines().find_map(|line| {
        let start = line.find(sentinel_prefix)?;
        line[start..]
            .strip_prefix(sentinel_prefix)
            .and_then(|value| value.trim().parse::<i32>().ok())
    })
}

#[async_trait]
impl ToolHandler for WriteStdinTool {
    fn name(&self) -> &str {
        "write_stdin"
    }

    fn description(&self) -> &str {
        "Writes characters to an existing exec/PTY session and returns recent output."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "number",
                    "description": "Identifier of the running unified exec session."
                },
                "chars": {
                    "type": "string",
                    "description": "Bytes to write to stdin (may be empty to poll)."
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "Time in ms to wait for response after writing (default: 500)"
                },
                "max_output_tokens": {
                    "type": "integer",
                    "description": "Maximum number of tokens to return. Excess output will be truncated."
                }
            },
            "required": ["session_id"],
            "additionalProperties": false
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

        let session_id = args.session_id.into_string();
        let input = args.chars.or(args.input).unwrap_or_default();

        if self.exec_session_manager.has_session(&session_id) {
            return self
                .exec_session_manager
                .write_and_poll(ExecWriteArgs {
                    session_id,
                    chars: Some(input),
                    yield_time_ms: args.yield_time_ms,
                    max_output_tokens: args.max_output_tokens,
                })
                .await;
        }

        let yield_time_ms = args.yield_time_ms.unwrap_or(500);

        let session = match self.session_manager.get_session(&session_id) {
            Some(s) => s,
            None => {
                return ToolResult {
                    success: false,
                    output: format!("session not found: {session_id}"),
                };
            }
        };

        info!(
            session_id = %session_id,
            input_len = input.len(),
            "writing to PTY session stdin"
        );

        // Do not discard pre-existing output before a poll. A long-running
        // command can finish between tool calls; draining first would lose the
        // very bytes (and sentinel) the caller is trying to observe.
        let (stdout_bytes_before_write, stderr_bytes_before_write) = {
            let stdout_buf = session.stdout_buffer.lock().await;
            let stderr_buf = session.stderr_buffer.lock().await;
            (stdout_buf.total_bytes(), stderr_buf.total_bytes())
        };

        // Write input to stdin.
        if !input.is_empty() {
            if let Err(e) = session.write_all(input.as_bytes()).await {
                return ToolResult {
                    success: false,
                    output: format!("failed to write to session stdin: {e}"),
                };
            }
        }

        // Poll until relevant output arrives or the yield budget is exhausted.
        // If this call wrote input, wait for bytes produced after the write
        // instead of returning immediately because older buffered output exists.
        let deadline = Instant::now() + Duration::from_millis(yield_time_ms);
        loop {
            let has_output = {
                let stdout_buf = session.stdout_buffer.lock().await;
                if (input.is_empty() && !stdout_buf.to_bytes().is_empty())
                    || (!input.is_empty() && stdout_buf.total_bytes() > stdout_bytes_before_write)
                {
                    true
                } else {
                    let stderr_buf = session.stderr_buffer.lock().await;
                    if input.is_empty() {
                        !stderr_buf.to_bytes().is_empty()
                    } else {
                        stderr_buf.total_bytes() > stderr_bytes_before_write
                    }
                }
            };
            if has_output || Instant::now() >= deadline || !session.is_alive().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Collect output.
        let stdout_text = {
            let stdout_buf = session.stdout_buffer.lock().await;
            stdout_buf.to_formatted_string()
        };
        let stderr_text = {
            let stderr_buf = session.stderr_buffer.lock().await;
            stderr_buf.to_formatted_string()
        };

        let mut output = format!("session_id: {session_id}\n");
        if !stdout_text.is_empty() {
            output.push_str(&format!("stdout:\n{stdout_text}\n"));
        }
        if !stderr_text.is_empty() {
            output.push_str(&format!("stderr:\n{stderr_text}\n"));
        }
        if stdout_text.is_empty() && stderr_text.is_empty() {
            output.push_str("(no output captured)\n");
        }
        if let Some(max_tokens) = args.max_output_tokens {
            output = truncate_to_token_budget(&output, max_tokens);
        }

        ToolResult {
            success: true,
            output,
        }
    }
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

/// Internal helper to create a session with proper shared buffer architecture.
fn create_session_internal(
    manager: &PtySessionManager,
    work_dir: &Path,
    real_pty: bool,
) -> Result<Arc<PtySession>, String> {
    if real_pty {
        return create_real_pty_session_internal(manager, work_dir);
    }

    let shell = detect_shell();
    let session_id = Uuid::new_v4().to_string();

    info!(
        session_id = %session_id,
        shell = shell.program,
        cwd = %work_dir.display(),
        "creating new PTY session"
    );

    let mut child = Command::new(shell.program)
        .args(shell.args)
        .current_dir(work_dir)
        .env_remove("BASH_ENV")
        .env_remove("ENV")
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
        backend: PtySessionBackend::Pipe {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
        },
        stdout_buffer: Mutex::new(HeadTailBuffer::default()),
        stderr_buffer: Mutex::new(HeadTailBuffer::default()),
        created_at: Instant::now(),
    });

    // Spawn background task to read stdout into session's buffer.
    let session_stdout = session.clone();
    tokio::spawn(async move {
        let mut stdout = stdout;
        let mut chunk = vec![0_u8; 4096];
        loop {
            match stdout.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut buf = session_stdout.stdout_buffer.lock().await;
                    buf.push_chunk(chunk[..n].to_vec());
                }
                Err(error) => {
                    debug!(%session_stdout.session_id, ?error, "stdout reader task errored");
                    break;
                }
            }
        }
        debug!("stdout reader task ended for session");
    });

    // Spawn background task to read stderr into session's buffer.
    let session_stderr = session.clone();
    tokio::spawn(async move {
        let mut stderr = stderr;
        let mut chunk = vec![0_u8; 4096];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut buf = session_stderr.stderr_buffer.lock().await;
                    buf.push_chunk(chunk[..n].to_vec());
                }
                Err(error) => {
                    debug!(%session_stderr.session_id, ?error, "stderr reader task errored");
                    break;
                }
            }
        }
        debug!("stderr reader task ended for session");
    });

    manager.sessions.insert(session_id, session.clone());
    Ok(session)
}

fn create_real_pty_session_internal(
    manager: &PtySessionManager,
    work_dir: &Path,
) -> Result<Arc<PtySession>, String> {
    let shell = detect_shell();
    let session_id = Uuid::new_v4().to_string();

    info!(
        session_id = %session_id,
        shell = shell.program,
        cwd = %work_dir.display(),
        "creating new real PTY session"
    );

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open PTY: {e}"))?;

    let mut command = CommandBuilder::new(shell.program);
    command.args(shell.args);
    command.cwd(work_dir.as_os_str());
    command.env_remove("BASH_ENV");
    command.env_remove("ENV");
    command.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("failed to spawn shell in PTY: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to clone PTY reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to take PTY writer: {e}"))?;

    let session = Arc::new(PtySession {
        session_id: session_id.clone(),
        backend: PtySessionBackend::RealPty {
            child: Mutex::new(child),
            writer: Mutex::new(writer),
        },
        stdout_buffer: Mutex::new(HeadTailBuffer::default()),
        stderr_buffer: Mutex::new(HeadTailBuffer::default()),
        created_at: Instant::now(),
    });

    let session_stdout = session.clone();
    std::thread::Builder::new()
        .name(format!("bifrost-pty-reader-{session_id}"))
        .spawn(move || {
            let mut chunk = vec![0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut buf = session_stdout.stdout_buffer.blocking_lock();
                        buf.push_chunk(chunk[..n].to_vec());
                    }
                    Err(error) => {
                        debug!(%session_stdout.session_id, ?error, "PTY reader thread errored");
                        break;
                    }
                }
            }
            debug!("PTY reader thread ended for session");
        })
        .map_err(|e| format!("failed to spawn PTY reader thread: {e}"))?;

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
        match &self.backend {
            PtySessionBackend::Pipe { child, .. } => {
                let mut child = child.lock().await;
                matches!(child.try_wait(), Ok(None))
            }
            PtySessionBackend::RealPty { child, .. } => {
                let mut child = child.lock().await;
                matches!(child.try_wait(), Ok(None))
            }
        }
    }

    /// Kill the session's child process.
    pub async fn kill(&self) {
        match &self.backend {
            PtySessionBackend::Pipe { child, .. } => {
                let mut child = child.lock().await;
                let _ = child.kill().await;
            }
            PtySessionBackend::RealPty { child, .. } => {
                let mut child = child.lock().await;
                let _ = child.kill();
            }
        }
    }

    async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        match &self.backend {
            PtySessionBackend::Pipe { stdin, .. } => {
                let mut stdin = stdin.lock().await;
                stdin.write_all(bytes).await?;
                stdin.flush().await
            }
            PtySessionBackend::RealPty { writer, .. } => {
                let bytes = bytes.to_vec();
                let mut writer = writer.lock().await;
                writer.write_all(&bytes)?;
                writer.flush()
            }
        }
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
        let session = create_session_internal(&manager, &tmp_dir(), false).unwrap();
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
        let session = create_session_internal(&manager, &tmp_dir(), false).unwrap();
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
        let stdin_tool = WriteStdinTool::new(manager.clone(), Arc::new(ExecSessionManager::new()));

        // Create a session with a foreground command that reads from stdin.
        let args = serde_json::json!({
            "command": "python3 -u -c 'import sys; print(\"ready\"); print(sys.stdin.readline().strip())'",
            "wait_for_completion": false,
            "yield_time_ms": 5000
        });
        let result = pty_tool.execute(&args.to_string(), &tmp_dir()).await;
        assert!(result.success);
        assert!(result.output.contains("exit_indicator: running"));
        assert!(result.output.contains("ready"));

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
            "chars": "hello from stdin\n",
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
                let session = create_session_internal(&mgr, &PathBuf::from("/tmp"), false).unwrap();
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
        let shell = detect_shell();
        assert!(!shell.program.is_empty());
        assert!(!shell.args.is_empty());
    }

    #[tokio::test]
    async fn test_session_is_alive() {
        let manager = PtySessionManager::new();
        let session = create_session_internal(&manager, &tmp_dir(), false).unwrap();

        assert!(session.is_alive().await);

        session.kill().await;
        // Give the process a moment to exit.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!session.is_alive().await);
    }
}
