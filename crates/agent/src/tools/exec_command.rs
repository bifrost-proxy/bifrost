//! `exec_command` tool surface.
//!
//! This tool owns real child process sessions. Long-running commands return a
//! `session_id` after the initial yield window; follow-up `write_stdin` calls
//! poll the same process until its actual exit status is observed.

use crate::tools::head_tail_buffer::HeadTailBuffer;
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use dashmap::DashMap;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{debug, info};

pub const MIN_YIELD_TIME_MS: u64 = 250;
pub const MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS: u64 = 5_000;
pub const MAX_YIELD_TIME_MS: u64 = 30_000;
pub const DEFAULT_EXEC_YIELD_TIME_MS: u64 = 10_000;
pub const DEFAULT_WRITE_STDIN_YIELD_TIME_MS: u64 = 250;
pub const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
const EXIT_STATUS_POLL_INTERVAL_MS: u64 = 100;
const TRAILING_OUTPUT_GRACE_MS: u64 = 100;

pub struct ExecCommandTool {
    session_manager: Arc<ExecSessionManager>,
}

/// Tool that writes arbitrary input to a running exec_command session's stdin.
pub struct WriteStdinTool {
    session_manager: Arc<ExecSessionManager>,
}

impl ExecCommandTool {
    pub fn new(session_manager: Arc<ExecSessionManager>) -> Self {
        Self { session_manager }
    }
}

impl WriteStdinTool {
    pub fn new(session_manager: Arc<ExecSessionManager>) -> Self {
        Self { session_manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecCommandArgs {
    cmd: String,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    login: Option<bool>,
    #[serde(default)]
    tty: Option<bool>,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecWriteArgs {
    pub session_id: String,
    #[serde(default)]
    pub chars: Option<String>,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteStdinArgs {
    session_id: i64,
    #[serde(default)]
    chars: Option<String>,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

pub struct ExecSessionManager {
    sessions: DashMap<String, Arc<ExecSession>>,
    next_session_id: AtomicI32,
    max_empty_write_stdin_yield_time_ms: AtomicU64,
}

impl Default for ExecSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            next_session_id: AtomicI32::new(1),
            max_empty_write_stdin_yield_time_ms: AtomicU64::new(
                DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS,
            ),
        }
    }

    pub fn with_max_background_terminal_timeout(max_timeout_ms: u64) -> Self {
        let manager = Self::new();
        manager.set_max_background_terminal_timeout(max_timeout_ms);
        manager
    }

    pub fn set_max_background_terminal_timeout(&self, max_timeout_ms: u64) {
        self.max_empty_write_stdin_yield_time_ms.store(
            max_timeout_ms.max(MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS),
            Ordering::Relaxed,
        );
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    pub async fn has_completed_session(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get(session_id).map(|entry| entry.clone()) else {
            return false;
        };
        session.is_completed().await
    }

    async fn spawn(
        &self,
        args: &ExecCommandArgs,
        work_dir: &Path,
    ) -> Result<Arc<ExecSession>, String> {
        let command =
            ShellCommand::new(args.shell.as_deref(), &args.cmd, args.login.unwrap_or(true));
        let session_id = self
            .next_session_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let session = if args.tty.unwrap_or(false) {
            create_pty_exec_session(session_id, command, work_dir)?
        } else {
            create_pipe_exec_session(session_id, command, work_dir)?
        };
        self.sessions
            .insert(session.session_id.clone(), session.clone());
        spawn_exit_watcher(session.clone());

        Ok(session)
    }

    pub async fn write_and_poll(&self, args: ExecWriteArgs) -> ToolResult {
        let Some(session) = self
            .sessions
            .get(&args.session_id)
            .map(|entry| entry.clone())
        else {
            return ToolResult {
                success: false,
                output: format!("session not found: {}", args.session_id),
            };
        };

        let input = args.chars.unwrap_or_default();
        let requested_cancel = input == "\u{3}";
        if requested_cancel {
            session.terminate().await;
        } else if !input.is_empty() {
            if let Err(error) = session.write_all(input.as_bytes()).await {
                return ToolResult {
                    success: false,
                    output: format!("failed to write to exec session stdin: {error}"),
                };
            }
        }

        let poll = session
            .poll(
                Duration::from_millis(
                    self.write_stdin_yield_time_ms(args.yield_time_ms, input.is_empty()),
                ),
                PollMode::OutputOrCompletion,
            )
            .await;
        let response = format_poll_response(&poll, args.max_output_tokens);
        if poll.completed {
            self.sessions.remove(&args.session_id);
        }
        ToolResult {
            success: true,
            output: response,
        }
    }

    fn exec_yield_time_ms(&self, requested: Option<u64>) -> u64 {
        clamp_yield_time(requested.unwrap_or(DEFAULT_EXEC_YIELD_TIME_MS))
    }

    fn write_stdin_yield_time_ms(&self, requested: Option<u64>, empty_input: bool) -> u64 {
        let requested = requested.unwrap_or(DEFAULT_WRITE_STDIN_YIELD_TIME_MS);
        if empty_input {
            requested.clamp(
                MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS,
                self.max_empty_write_stdin_yield_time_ms
                    .load(Ordering::Relaxed),
            )
        } else {
            clamp_yield_time(requested)
        }
    }
}

fn clamp_yield_time(yield_time_ms: u64) -> u64 {
    yield_time_ms.clamp(MIN_YIELD_TIME_MS, MAX_YIELD_TIME_MS)
}

fn spawn_exit_watcher(session: Arc<ExecSession>) {
    tokio::spawn(async move {
        loop {
            session.refresh_exit_status().await;
            if session.is_completed().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(EXIT_STATUS_POLL_INTERVAL_MS)).await;
        }
    });
}

#[async_trait]
impl ToolHandler for WriteStdinTool {
    fn name(&self) -> &str {
        "write_stdin"
    }

    fn description(&self) -> &str {
        "Writes characters to an existing exec_command session and returns recent output."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
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
                    "description": "How long to wait (in milliseconds) for output before yielding. Defaults to 250 for writes; empty polls wait at least 5000 and are capped by background terminal timeout."
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
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                };
            }
        };

        self.session_manager
            .write_and_poll(ExecWriteArgs {
                session_id: args.session_id.to_string(),
                chars: Some(args.chars.unwrap_or_default()),
                yield_time_ms: args.yield_time_ms,
                max_output_tokens: args.max_output_tokens,
            })
            .await
    }
}

#[async_trait]
impl ToolHandler for ExecCommandTool {
    fn name(&self) -> &str {
        "exec_command"
    }

    fn description(&self) -> &str {
        "Runs a command as a real child process. If it is still running after `yield_time_ms`, returns a `session_id`; continue polling, writing stdin, or sending Ctrl-C with `write_stdin`. Set `tty=true` for interactive commands. The final poll reports the actual process exit code."
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
                "login": {
                    "type": "boolean",
                    "description": "Whether to run the shell with -l/-i semantics. Defaults to true."
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
            "required": ["cmd"],
            "additionalProperties": false
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

        let yield_time_ms = self.session_manager.exec_yield_time_ms(args.yield_time_ms);
        let start = Instant::now();
        let session = match self.session_manager.spawn(&args, &run_dir).await {
            Ok(session) => session,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: error,
                };
            }
        };
        let poll = session
            .poll(
                Duration::from_millis(yield_time_ms),
                PollMode::CompletionOrYield,
            )
            .await;
        let wall_time_seconds = (start.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        let mut output = poll.output;
        let original_token_count = estimate_tokens(&output);
        if let Some(max_tokens) = args.max_output_tokens {
            output = truncate_to_token_budget(&output, max_tokens);
        }
        let session_id = if poll.completed {
            self.session_manager.sessions.remove(&poll.session_id);
            serde_json::Value::Null
        } else {
            session_id_value(&poll.session_id)
        };
        let response = json!({
            "chunk_id": null,
            "wall_time_seconds": wall_time_seconds,
            "exit_code": poll.exit_code,
            "session_id": session_id,
            "original_token_count": original_token_count,
            "output": output
        });

        ToolResult {
            success: true,
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

fn session_id_value(session_id: &str) -> serde_json::Value {
    session_id
        .parse::<i32>()
        .map(serde_json::Value::from)
        .unwrap_or_else(|_| serde_json::Value::String(session_id.to_string()))
}

struct ShellCommand {
    program: String,
    args: Vec<String>,
}

impl ShellCommand {
    fn new(shell: Option<&str>, cmd: &str, use_login_shell: bool) -> Self {
        let shell = shell
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_shell);
        let flag = if shell.ends_with("fish") || !use_login_shell {
            "-c"
        } else {
            "-lc"
        };
        Self {
            program: shell,
            args: vec![flag.to_string(), cmd.to_string()],
        }
    }
}

fn default_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.trim().is_empty() {
            return shell;
        }
    }
    if cfg!(target_os = "macos") {
        "zsh".to_string()
    } else {
        "bash".to_string()
    }
}

enum ExecBackend {
    Pipe {
        child: Mutex<Child>,
        stdin: Mutex<Option<ChildStdin>>,
    },
    Pty {
        child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
        writer: Mutex<Box<dyn Write + Send>>,
    },
}

struct ExecState {
    completed: bool,
    exit_code: Option<i32>,
}

pub struct ExecSession {
    session_id: String,
    backend: ExecBackend,
    stdout_buffer: Mutex<HeadTailBuffer>,
    stderr_buffer: Mutex<HeadTailBuffer>,
    state: Mutex<ExecState>,
}

struct ExecPoll {
    session_id: String,
    completed: bool,
    exit_code: Option<i32>,
    output: String,
}

enum PollMode {
    CompletionOrYield,
    OutputOrCompletion,
}

impl ExecSession {
    async fn poll(&self, yield_duration: Duration, mode: PollMode) -> ExecPoll {
        let deadline = Instant::now() + yield_duration;
        loop {
            self.refresh_exit_status().await;
            if self.is_completed().await {
                break;
            }
            let has_output = self.has_output().await;
            if has_output && matches!(mode, PollMode::OutputOrCompletion) {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let stdout_text = {
            let mut stdout_buf = self.stdout_buffer.lock().await;
            chunks_to_string(stdout_buf.drain_chunks())
        };
        let stderr_text = {
            let mut stderr_buf = self.stderr_buffer.lock().await;
            chunks_to_string(stderr_buf.drain_chunks())
        };
        let output = merge_output(stdout_text, stderr_text);
        let state = self.state.lock().await;
        ExecPoll {
            session_id: self.session_id.clone(),
            completed: state.completed,
            exit_code: state.exit_code,
            output,
        }
    }

    async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        match &self.backend {
            ExecBackend::Pipe { stdin, .. } => {
                let mut stdin = stdin.lock().await;
                let Some(stdin) = stdin.as_mut() else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "stdin is already closed",
                    ));
                };
                stdin.write_all(bytes).await?;
                stdin.flush().await
            }
            ExecBackend::Pty { writer, .. } => {
                let mut writer = writer.lock().await;
                writer.write_all(bytes)?;
                writer.flush()
            }
        }
    }

    async fn terminate(&self) {
        match &self.backend {
            ExecBackend::Pipe { child, .. } => {
                let mut child = child.lock().await;
                let _ = child.start_kill();
            }
            ExecBackend::Pty { child, .. } => {
                let mut child = child.lock().await;
                let _ = child.kill();
            }
        }
    }

    async fn is_completed(&self) -> bool {
        self.state.lock().await.completed
    }

    async fn has_output(&self) -> bool {
        let stdout_has = {
            let stdout = self.stdout_buffer.lock().await;
            !stdout.to_bytes().is_empty()
        };
        if stdout_has {
            return true;
        }
        let stderr = self.stderr_buffer.lock().await;
        !stderr.to_bytes().is_empty()
    }

    async fn refresh_exit_status(&self) {
        if self.state.lock().await.completed {
            return;
        }
        let exit_code = match &self.backend {
            ExecBackend::Pipe { child, .. } => {
                let mut child = child.lock().await;
                match child.try_wait() {
                    Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                    Ok(None) | Err(_) => None,
                }
            }
            ExecBackend::Pty { child, .. } => {
                let mut child = child.lock().await;
                match child.try_wait() {
                    Ok(Some(status)) => Some(status.exit_code() as i32),
                    Ok(None) | Err(_) => None,
                }
            }
        };

        if let Some(exit_code) = exit_code {
            tokio::time::sleep(Duration::from_millis(TRAILING_OUTPUT_GRACE_MS)).await;
            let mut state = self.state.lock().await;
            if state.completed {
                return;
            }
            state.completed = true;
            state.exit_code = Some(exit_code);
        }
    }
}

impl Drop for ExecSession {
    fn drop(&mut self) {
        match &self.backend {
            ExecBackend::Pipe { child, .. } => {
                if let Ok(mut child) = child.try_lock() {
                    let _ = child.start_kill();
                }
            }
            ExecBackend::Pty { child, .. } => {
                if let Ok(mut child) = child.try_lock() {
                    let _ = child.kill();
                }
            }
        }
    }
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

fn chunks_to_string(chunks: Vec<Vec<u8>>) -> String {
    let bytes = chunks.into_iter().flatten().collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).trim_end().to_string()
}

fn merge_output(stdout_text: String, stderr_text: String) -> String {
    match (stdout_text.is_empty(), stderr_text.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout_text,
        (true, false) => format!("[stderr]\n{stderr_text}"),
        (false, false) => format!("{stdout_text}\n[stderr]\n{stderr_text}"),
    }
}

fn format_poll_response(poll: &ExecPoll, max_output_tokens: Option<usize>) -> String {
    let original_token_count = estimate_tokens(&poll.output);
    let output = max_output_tokens
        .map(|limit| truncate_to_token_budget(&poll.output, limit))
        .unwrap_or_else(|| poll.output.clone());
    let session_id = if poll.completed {
        serde_json::Value::Null
    } else {
        session_id_value(&poll.session_id)
    };
    json!({
        "session_id": session_id,
        "exit_code": poll.exit_code,
        "original_token_count": original_token_count,
        "output": output
    })
    .to_string()
}

fn create_pipe_exec_session(
    session_id: String,
    command: ShellCommand,
    work_dir: &Path,
) -> Result<Arc<ExecSession>, String> {
    info!(
        session_id = %session_id,
        program = %command.program,
        cwd = %work_dir.display(),
        "creating exec pipe session"
    );
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .current_dir(work_dir)
        .envs(std::env::vars())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to spawn command: {error}"))?;

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

    let session = Arc::new(ExecSession {
        session_id: session_id.clone(),
        backend: ExecBackend::Pipe {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
        },
        stdout_buffer: Mutex::new(HeadTailBuffer::default()),
        stderr_buffer: Mutex::new(HeadTailBuffer::default()),
        state: Mutex::new(ExecState {
            completed: false,
            exit_code: None,
        }),
    });

    spawn_pipe_reader(session.clone(), stdout, false);
    spawn_pipe_reader(session.clone(), stderr, true);
    Ok(session)
}

fn spawn_pipe_reader<R>(session: Arc<ExecSession>, mut reader: R, stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0_u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let target = if stderr {
                        &session.stderr_buffer
                    } else {
                        &session.stdout_buffer
                    };
                    let mut buffer = target.lock().await;
                    buffer.push_chunk(chunk[..n].to_vec());
                }
                Err(error) => {
                    debug!(%session.session_id, ?error, stderr, "exec pipe reader errored");
                    break;
                }
            }
        }
    });
}

fn create_pty_exec_session(
    session_id: String,
    command: ShellCommand,
    work_dir: &Path,
) -> Result<Arc<ExecSession>, String> {
    info!(
        session_id = %session_id,
        program = %command.program,
        cwd = %work_dir.display(),
        "creating exec PTY session"
    );
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("failed to open PTY: {error}"))?;

    let mut command_builder = CommandBuilder::new(&command.program);
    command_builder.args(command.args.iter().map(String::as_str));
    command_builder.cwd(work_dir.as_os_str());
    command_builder.env_remove("BASH_ENV");
    command_builder.env_remove("ENV");
    command_builder.env("TERM", "xterm-256color");
    let child = pair
        .slave
        .spawn_command(command_builder)
        .map_err(|error| format!("failed to spawn command in PTY: {error}"))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to clone PTY reader: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to take PTY writer: {error}"))?;

    let session = Arc::new(ExecSession {
        session_id: session_id.clone(),
        backend: ExecBackend::Pty {
            child: Mutex::new(child),
            writer: Mutex::new(writer),
        },
        stdout_buffer: Mutex::new(HeadTailBuffer::default()),
        stderr_buffer: Mutex::new(HeadTailBuffer::default()),
        state: Mutex::new(ExecState {
            completed: false,
            exit_code: None,
        }),
    });

    let session_stdout = session.clone();
    std::thread::Builder::new()
        .name(format!("bifrost-exec-pty-reader-{session_id}"))
        .spawn(move || {
            let mut chunk = vec![0_u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut buffer = session_stdout.stdout_buffer.blocking_lock();
                        buffer.push_chunk(chunk[..n].to_vec());
                    }
                    Err(error) => {
                        debug!(%session_stdout.session_id, ?error, "exec PTY reader errored");
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("failed to spawn PTY reader thread: {error}"))?;

    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn exec_command_returns_completed_output() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager);
        let dir = tempdir().unwrap();
        let result = tool
            .execute(r#"{"cmd":"printf hello","yield_time_ms":500}"#, dir.path())
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["output"], "hello");
    }

    #[tokio::test]
    async fn exec_command_yields_session_and_write_stdin_polls_to_exit() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"printf start; sleep 0.3; printf end","yield_time_ms":50}"#,
                dir.path(),
            )
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();
        assert_eq!(value["exit_code"], serde_json::Value::Null);

        let mut final_poll = serde_json::Value::Null;
        let mut combined_output = String::new();
        for _ in 0..20 {
            let poll = manager
                .write_and_poll(ExecWriteArgs {
                    session_id: session_id.clone(),
                    chars: None,
                    yield_time_ms: Some(100),
                    max_output_tokens: None,
                })
                .await;
            assert!(poll.success, "{}", poll.output);
            final_poll = serde_json::from_str(&poll.output).unwrap();
            combined_output.push_str(final_poll["output"].as_str().unwrap_or(""));
            if !final_poll["exit_code"].is_null() {
                break;
            }
        }
        assert_eq!(final_poll["exit_code"], 0);
        assert!(final_poll["session_id"].is_null());
        assert!(combined_output.contains("end"));
        assert!(!manager.has_session(&session_id));
    }

    #[tokio::test]
    async fn exec_command_background_watcher_observes_exit_before_next_poll() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"sleep 0.8; printf watched-done","yield_time_ms":50}"#,
                dir.path(),
            )
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();
        assert_eq!(value["exit_code"], serde_json::Value::Null);

        for _ in 0..20 {
            if manager.has_completed_session(&session_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            manager.has_completed_session(&session_id).await,
            "background watcher did not observe exit"
        );

        let poll = manager
            .write_and_poll(ExecWriteArgs {
                session_id: session_id.clone(),
                chars: None,
                yield_time_ms: Some(1),
                max_output_tokens: None,
            })
            .await;
        assert!(poll.success, "{}", poll.output);
        let value: serde_json::Value = serde_json::from_str(&poll.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert!(value["session_id"].is_null());
        assert!(value["output"]
            .as_str()
            .unwrap_or("")
            .contains("watched-done"));
        assert!(!manager.has_session(&session_id));
    }

    #[tokio::test]
    async fn exec_command_write_stdin_drives_pipe_process() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"python3 -u -c 'import sys; print(\"ready\"); print(sys.stdin.readline().strip())'","yield_time_ms":2000}"#,
                dir.path(),
            )
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();
        let mut combined_output = value["output"].as_str().unwrap_or("").to_string();
        for _ in 0..20 {
            if combined_output.contains("ready") {
                break;
            }
            let poll = manager
                .write_and_poll(ExecWriteArgs {
                    session_id: session_id.clone(),
                    chars: None,
                    yield_time_ms: Some(50),
                    max_output_tokens: None,
                })
                .await;
            assert!(poll.success, "{}", poll.output);
            let value: serde_json::Value = serde_json::from_str(&poll.output).unwrap();
            combined_output.push_str(value["output"].as_str().unwrap_or(""));
        }
        assert!(combined_output.contains("ready"), "{combined_output}");

        let poll = manager
            .write_and_poll(ExecWriteArgs {
                session_id,
                chars: Some("from-stdin\n".to_string()),
                yield_time_ms: Some(1000),
                max_output_tokens: None,
            })
            .await;
        assert!(poll.success, "{}", poll.output);
        let value: serde_json::Value = serde_json::from_str(&poll.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert!(value["session_id"].is_null());
        assert!(value["output"]
            .as_str()
            .unwrap_or("")
            .contains("from-stdin"));
    }

    #[tokio::test]
    async fn exec_command_ctrl_c_terminates_running_process() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let result = tool
            .execute(r#"{"cmd":"sleep 30","yield_time_ms":50}"#, dir.path())
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();

        let mut final_poll = serde_json::Value::Null;
        let mut chars = Some("\u{3}".to_string());
        for _ in 0..20 {
            let poll = manager
                .write_and_poll(ExecWriteArgs {
                    session_id: session_id.clone(),
                    chars: chars.take(),
                    yield_time_ms: Some(100),
                    max_output_tokens: None,
                })
                .await;
            assert!(poll.success, "{}", poll.output);
            final_poll = serde_json::from_str(&poll.output).unwrap();
            if !final_poll["exit_code"].is_null() {
                break;
            }
        }
        assert!(!final_poll["exit_code"].is_null(), "{final_poll}");
        assert!(!manager.has_session(&session_id));
    }

    #[tokio::test]
    async fn exec_command_nonzero_exit_is_successful_tool_result() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager);
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"printf nope; exit 7","yield_time_ms":500}"#,
                dir.path(),
            )
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(value["exit_code"], 7);
        assert_eq!(value["output"], "nope");
    }

    #[tokio::test]
    async fn write_stdin_rejects_legacy_protocol_fields() {
        let tool = WriteStdinTool::new(Arc::new(ExecSessionManager::new()));
        let dir = tempdir().unwrap();

        let hidden_input = tool
            .execute(r#"{"session_id":1,"input":"legacy"}"#, dir.path())
            .await;
        assert!(!hidden_input.success);
        assert!(hidden_input.output.contains("unknown field `input`"));

        let string_session_id = tool
            .execute(r#"{"session_id":"1","chars":""}"#, dir.path())
            .await;
        assert!(!string_session_id.success);
        assert!(string_session_id.output.contains("expected i64"));
    }

    #[test]
    fn exec_command_login_false_uses_non_login_shell_flag() {
        let non_login = ShellCommand::new(Some("zsh"), "printf ok", false);
        assert_eq!(non_login.args[0], "-c");

        let login = ShellCommand::new(Some("zsh"), "printf ok", true);
        assert_eq!(login.args[0], "-lc");
    }

    #[test]
    fn exec_command_yield_defaults_and_clamps_match_codex_unified_exec() {
        let manager = ExecSessionManager::new();
        assert_eq!(manager.exec_yield_time_ms(None), 10_000);
        assert_eq!(manager.exec_yield_time_ms(Some(1)), MIN_YIELD_TIME_MS);
        assert_eq!(manager.exec_yield_time_ms(Some(60_000)), MAX_YIELD_TIME_MS);

        assert_eq!(
            manager.write_stdin_yield_time_ms(None, false),
            DEFAULT_WRITE_STDIN_YIELD_TIME_MS
        );
        assert_eq!(
            manager.write_stdin_yield_time_ms(Some(1), false),
            MIN_YIELD_TIME_MS
        );
        assert_eq!(
            manager.write_stdin_yield_time_ms(Some(60_000), false),
            MAX_YIELD_TIME_MS
        );
        assert_eq!(
            manager.write_stdin_yield_time_ms(None, true),
            MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS
        );

        manager.set_max_background_terminal_timeout(6_000);
        assert_eq!(manager.write_stdin_yield_time_ms(Some(60_000), true), 6_000);

        manager.set_max_background_terminal_timeout(1_000);
        assert_eq!(
            manager.write_stdin_yield_time_ms(Some(60_000), true),
            MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS
        );
    }

    #[tokio::test]
    async fn test_exec_command_tty_reports_isatty_true() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager);
        let dir = tempdir().unwrap();
        let result = tool
            .execute(
                r#"{"cmd":"python3 -c 'import os,sys; print(os.isatty(0), os.isatty(1))'","tty":true,"yield_time_ms":3000}"#,
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
        assert_eq!(value["exit_code"], 0);
    }
}
