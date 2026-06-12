//! `exec_command` tool surface.
//!
//! This tool owns real child process sessions. Long-running commands return a
//! `session_id` after the initial yield window; follow-up `write_stdin` calls
//! poll the same process until its actual exit status is observed.

mod transcript;

use self::transcript::{ExecStream, ExecTranscript};
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
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

pub const MIN_YIELD_TIME_MS: u64 = 250;
pub const MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS: u64 = 5_000;
pub const MAX_YIELD_TIME_MS: u64 = 30_000;
pub const DEFAULT_EXEC_YIELD_TIME_MS: u64 = 10_000;
pub const DEFAULT_WRITE_STDIN_YIELD_TIME_MS: u64 = 250;
pub const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
pub const MAX_EXEC_SESSIONS: usize = 64;
const EXIT_STATUS_POLL_INTERVAL_MS: u64 = 100;
const TRAILING_OUTPUT_GRACE_MS: u64 = 100;
const PROCESS_GROUP_KILL_GRACE_MS: u64 = 500;

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
    pub since_chunk_id: Option<u64>,
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
    since_chunk_id: Option<u64>,
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
        self.prune_completed_sessions().await;
        if self.sessions.len() >= MAX_EXEC_SESSIONS {
            return Err(format!(
                "resource_pressure: max exec sessions reached ({MAX_EXEC_SESSIONS}); running tasks were not silently pruned"
            ));
        }
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

    async fn prune_completed_sessions(&self) {
        let sessions = self
            .sessions
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        for (session_id, session) in sessions {
            if session.is_completed().await {
                self.sessions.remove(&session_id);
            }
        }
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
                runtime_events: Vec::new(),
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
                    runtime_events: Vec::new(),
                };
            }
        }

        let poll = session
            .poll(
                Duration::from_millis(
                    self.write_stdin_yield_time_ms(args.yield_time_ms, input.is_empty()),
                ),
                PollMode::OutputOrCompletion,
                PollConsumer::Model {
                    since_chunk_id: args.since_chunk_id,
                },
            )
            .await;
        let response = format_poll_response(&poll, args.max_output_tokens);
        if poll.completed {
            self.sessions.remove(&args.session_id);
        }
        ToolResult {
            success: true,
            output: response,
            runtime_events: Vec::new(),
        }
    }

    pub async fn poll_existing_session(
        &self,
        session_id: &str,
        yield_time_ms: u64,
        max_output_tokens: Option<usize>,
    ) -> ToolResult {
        let Some(session) = self.sessions.get(session_id).map(|entry| entry.clone()) else {
            return ToolResult {
                success: false,
                output: format!("session not found: {session_id}"),
                runtime_events: Vec::new(),
            };
        };

        let poll = session
            .poll(
                Duration::from_millis(clamp_yield_time(yield_time_ms)),
                PollMode::OutputOrCompletion,
                PollConsumer::Runtime,
            )
            .await;
        let response = format_poll_response(&poll, max_output_tokens);
        if poll.completed {
            self.sessions.remove(session_id);
        }
        ToolResult {
            success: true,
            output: response,
            runtime_events: Vec::new(),
        }
    }

    pub async fn terminate_session(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get(session_id).map(|entry| entry.clone()) else {
            return false;
        };
        session.terminate().await;
        true
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
                "since_chunk_id": {
                    "type": "integer",
                    "description": "Optional output cursor. When supported, callers can request only output after this chunk id."
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
                    runtime_events: Vec::new(),
                };
            }
        };

        self.session_manager
            .write_and_poll(ExecWriteArgs {
                session_id: args.session_id.to_string(),
                chars: Some(args.chars.unwrap_or_default()),
                since_chunk_id: args.since_chunk_id,
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
                    runtime_events: Vec::new(),
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
                        runtime_events: Vec::new(),
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
                    runtime_events: Vec::new(),
                };
            }
        };
        let poll = session
            .poll(
                Duration::from_millis(yield_time_ms),
                PollMode::CompletionOrYield,
                PollConsumer::Initial,
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
        let running = !poll.completed;
        let tty = args.tty.unwrap_or(false);
        let long_task_candidate = running;
        let suggested_wait_profile = if long_task_candidate {
            Some(if tty { "interactive" } else { "adaptive" })
        } else {
            None
        };
        let response = json!({
            "chunk_id": poll.chunk_id,
            "wall_time_seconds": wall_time_seconds,
            "exit_code": poll.exit_code,
            "session_id": session_id,
            "original_token_count": original_token_count,
            "output": output,
            "new_output_bytes": poll.new_output_bytes,
            "output_lossy": poll.output_lossy,
            "lost_chunk_count": poll.lost_chunk_count,
            "truncated_bytes": poll.truncated_bytes,
            "running": running,
            "long_task_candidate": long_task_candidate,
            "suggested_wait_profile": suggested_wait_profile,
            "duration_class": classify_duration_class(running, wall_time_seconds),
            "response_strategy": classify_response_strategy(long_task_candidate),
            "next_output_cursor": {
                "chunk_id": poll.chunk_id,
                "stdout_bytes": null,
                "stderr_bytes": null
            }
        });

        ToolResult {
            success: true,
            output: response.to_string(),
            runtime_events: Vec::new(),
        }
    }
}

fn classify_duration_class(running: bool, wall_time_seconds: f64) -> &'static str {
    if running {
        "unknown_running"
    } else if wall_time_seconds < 1.0 {
        "instant"
    } else {
        "short"
    }
}

fn classify_response_strategy(long_task_candidate: bool) -> &'static str {
    if long_task_candidate {
        "adaptive_monitor"
    } else {
        "inline_or_manual"
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
        let args = if is_powershell_shell(&shell) {
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                cmd.to_string(),
            ]
        } else if is_cmd_shell(&shell) {
            vec!["/C".to_string(), cmd.to_string()]
        } else {
            let flag = if shell.ends_with("fish") || !use_login_shell {
                "-c"
            } else {
                "-lc"
            };
            vec![flag.to_string(), cmd.to_string()]
        };
        Self {
            program: shell,
            args,
        }
    }
}

fn shell_basename_lower(shell: &str) -> String {
    shell
        .trim()
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase()
}

fn is_powershell_shell(shell: &str) -> bool {
    matches!(
        shell_basename_lower(shell).as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    )
}

fn is_cmd_shell(shell: &str) -> bool {
    matches!(shell_basename_lower(shell).as_str(), "cmd" | "cmd.exe")
}

fn default_shell() -> String {
    if cfg!(windows) {
        return "powershell.exe".to_string();
    }
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
    transcript: Mutex<ExecTranscript>,
    model_visible_cursor: AtomicU64,
    runtime_cursor: AtomicU64,
    state: Mutex<ExecState>,
    notify: Notify,
}

struct ExecPoll {
    session_id: String,
    completed: bool,
    exit_code: Option<i32>,
    output: String,
    chunk_id: u64,
    new_output_bytes: usize,
    output_lossy: bool,
    lost_chunk_count: u64,
    truncated_bytes: u64,
}

enum PollMode {
    CompletionOrYield,
    OutputOrCompletion,
}

enum PollConsumer {
    Initial,
    Runtime,
    Model { since_chunk_id: Option<u64> },
}

impl ExecSession {
    async fn poll(
        &self,
        yield_duration: Duration,
        mode: PollMode,
        consumer: PollConsumer,
    ) -> ExecPoll {
        let cursor = self.cursor_for_consumer(&consumer);
        let deadline = Instant::now() + yield_duration;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            self.refresh_exit_status().await;
            if self.is_completed().await {
                break;
            }
            let has_output = self.has_output_after(cursor).await;
            if has_output && matches!(mode, PollMode::OutputOrCompletion) {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if tokio::time::timeout(remaining, &mut notified)
                .await
                .is_err()
            {
                break;
            }
        }

        let read = {
            let transcript = self.transcript.lock().await;
            transcript.read_since(cursor)
        };
        self.advance_cursor_for_consumer(&consumer, read.chunk_id);
        let state = self.state.lock().await;
        ExecPoll {
            session_id: self.session_id.clone(),
            completed: state.completed,
            exit_code: state.exit_code,
            output: read.output,
            chunk_id: read.chunk_id,
            new_output_bytes: read.new_output_bytes,
            output_lossy: read.output_lossy,
            lost_chunk_count: read.lost_chunk_count,
            truncated_bytes: read.truncated_bytes,
        }
    }

    fn cursor_for_consumer(&self, consumer: &PollConsumer) -> u64 {
        match consumer {
            PollConsumer::Initial => 0,
            PollConsumer::Runtime => self
                .runtime_cursor
                .load(Ordering::Relaxed)
                .max(self.model_visible_cursor.load(Ordering::Relaxed)),
            PollConsumer::Model { since_chunk_id } => {
                since_chunk_id.unwrap_or_else(|| self.model_visible_cursor.load(Ordering::Relaxed))
            }
        }
    }

    fn advance_cursor_for_consumer(&self, consumer: &PollConsumer, chunk_id: u64) {
        if matches!(consumer, PollConsumer::Initial | PollConsumer::Runtime) {
            self.runtime_cursor.fetch_max(chunk_id, Ordering::Relaxed);
        }
        if matches!(consumer, PollConsumer::Initial | PollConsumer::Model { .. }) {
            self.model_visible_cursor
                .fetch_max(chunk_id, Ordering::Relaxed);
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
                terminate_pipe_child(&mut child, true);
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

    async fn has_output_after(&self, cursor: u64) -> bool {
        let transcript = self.transcript.lock().await;
        transcript.has_after(cursor)
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
            self.notify.notify_waiters();
        }
    }
}

impl Drop for ExecSession {
    fn drop(&mut self) {
        match &self.backend {
            ExecBackend::Pipe { child, .. } => {
                if let Ok(mut child) = child.try_lock() {
                    terminate_pipe_child(&mut child, false);
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
        "chunk_id": poll.chunk_id,
        "session_id": session_id,
        "exit_code": poll.exit_code,
        "original_token_count": original_token_count,
        "output": output,
        "running": !poll.completed,
        "unchanged": !poll.completed && poll.output.is_empty(),
        "new_output_bytes": poll.new_output_bytes,
        "output_lossy": poll.output_lossy,
        "lost_chunk_count": poll.lost_chunk_count,
        "truncated_bytes": poll.truncated_bytes,
        "next_output_cursor": {
            "chunk_id": poll.chunk_id,
            "stdout_bytes": null,
            "stderr_bytes": null
        }
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
    let mut command_builder = Command::new(&command.program);
    command_builder
        .args(&command.args)
        .current_dir(work_dir)
        .envs(std::env::vars_os())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command_builder.process_group(0);
    let mut child = command_builder
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
        transcript: Mutex::new(ExecTranscript::default()),
        model_visible_cursor: AtomicU64::new(0),
        runtime_cursor: AtomicU64::new(0),
        state: Mutex::new(ExecState {
            completed: false,
            exit_code: None,
        }),
        notify: Notify::new(),
    });

    spawn_pipe_reader(session.clone(), stdout, false);
    spawn_pipe_reader(session.clone(), stderr, true);
    Ok(session)
}

fn terminate_pipe_child(child: &mut Child, schedule_force_kill: bool) {
    let Some(pid) = child.id() else {
        let _ = child.start_kill();
        return;
    };
    if let Err(error) = signal_exec_process_group_or_child(pid, ExecSignal::Terminate) {
        warn!(pid, %error, "failed to terminate exec process group; falling back to child kill");
        let _ = child.start_kill();
    }
    if schedule_force_kill {
        schedule_exec_process_group_force_kill(pid);
    }
}

#[derive(Clone, Copy)]
enum ExecSignal {
    Terminate,
    Kill,
}

#[cfg(unix)]
fn signal_exec_process_group_or_child(pid: u32, signal: ExecSignal) -> Result<(), String> {
    use nix::errno::Errno;
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let signal = match signal {
        ExecSignal::Terminate => Signal::SIGTERM,
        ExecSignal::Kill => Signal::SIGKILL,
    };
    let raw_pid = i32::try_from(pid).map_err(|_| format!("pid out of range: {pid}"))?;
    let group_pid = Pid::from_raw(-raw_pid);
    match kill(group_pid, signal) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => kill(Pid::from_raw(raw_pid), signal)
            .map_err(|error| format!("signal child process {pid}: {error}")),
        Err(error) => Err(format!("signal process group {pid}: {error}")),
    }
}

#[cfg(not(unix))]
fn signal_exec_process_group_or_child(_pid: u32, _signal: ExecSignal) -> Result<(), String> {
    Err("process group signaling is not supported on this platform".to_string())
}

fn schedule_exec_process_group_force_kill(pid: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(PROCESS_GROUP_KILL_GRACE_MS));
        if let Err(error) = signal_exec_process_group_or_child(pid, ExecSignal::Kill) {
            debug!(pid, %error, "exec process group force kill skipped or failed");
        }
    });
}

fn spawn_pipe_reader<R>(session: Arc<ExecSession>, mut reader: R, stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0_u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => {
                    session.notify.notify_waiters();
                    break;
                }
                Ok(n) => {
                    let stream = if stderr {
                        ExecStream::Stderr
                    } else {
                        ExecStream::Stdout
                    };
                    let mut transcript = session.transcript.lock().await;
                    transcript.push_chunk(stream, chunk[..n].to_vec());
                    session.notify.notify_waiters();
                }
                Err(error) => {
                    debug!(%session.session_id, ?error, stderr, "exec pipe reader errored");
                    session.notify.notify_waiters();
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
        transcript: Mutex::new(ExecTranscript::default()),
        model_visible_cursor: AtomicU64::new(0),
        runtime_cursor: AtomicU64::new(0),
        state: Mutex::new(ExecState {
            completed: false,
            exit_code: None,
        }),
        notify: Notify::new(),
    });

    let session_stdout = session.clone();
    std::thread::Builder::new()
        .name(format!("bifrost-exec-pty-reader-{session_id}"))
        .spawn(move || {
            let mut chunk = vec![0_u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        session_stdout.notify.notify_waiters();
                        break;
                    }
                    Ok(n) => {
                        let mut transcript = session_stdout.transcript.blocking_lock();
                        transcript.push_chunk(ExecStream::Stdout, chunk[..n].to_vec());
                        session_stdout.notify.notify_waiters();
                    }
                    Err(error) => {
                        debug!(%session_stdout.session_id, ?error, "exec PTY reader errored");
                        session_stdout.notify.notify_waiters();
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

    #[cfg(windows)]
    fn ps_string(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    #[cfg(windows)]
    fn ps_write(value: &str) -> String {
        format!(
            "[Console]::Out.Write({}); [Console]::Out.Flush()",
            ps_string(value)
        )
    }

    #[cfg(not(windows))]
    fn sh_string(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[cfg(windows)]
    fn print_command(value: &str) -> String {
        ps_write(value)
    }

    #[cfg(not(windows))]
    fn print_command(value: &str) -> String {
        format!("printf %s {}", sh_string(value))
    }

    #[cfg(windows)]
    fn delayed_print_command(first: &str, delay_ms: u64, second: &str) -> String {
        format!(
            "{}; Start-Sleep -Milliseconds {}; {}",
            ps_write(first),
            delay_ms,
            ps_write(second)
        )
    }

    #[cfg(not(windows))]
    fn delayed_print_command(first: &str, delay_ms: u64, second: &str) -> String {
        format!(
            "printf %s {}; sleep {:.3}; printf %s {}",
            sh_string(first),
            delay_ms as f64 / 1000.0,
            sh_string(second)
        )
    }

    #[cfg(windows)]
    fn delayed_print_then_sleep_command(delay_ms: u64, output: &str, sleep_secs: u64) -> String {
        format!(
            "Start-Sleep -Milliseconds {}; {}; Start-Sleep -Seconds {}",
            delay_ms,
            ps_write(output),
            sleep_secs
        )
    }

    #[cfg(not(windows))]
    fn delayed_print_then_sleep_command(delay_ms: u64, output: &str, sleep_secs: u64) -> String {
        format!(
            "sleep {:.3}; printf %s {}; sleep {}",
            delay_ms as f64 / 1000.0,
            sh_string(output),
            sleep_secs
        )
    }

    #[cfg(windows)]
    fn stdin_echo_command() -> String {
        "[Console]::Out.WriteLine('ready'); $line = [Console]::In.ReadLine(); [Console]::Out.WriteLine($line)".to_string()
    }

    #[cfg(not(windows))]
    fn stdin_echo_command() -> String {
        "printf '%s\\n' 'ready'; IFS= read -r line; printf '%s\\n' \"$line\"".to_string()
    }

    #[cfg(windows)]
    fn long_sleep_command() -> String {
        "Start-Sleep -Seconds 30".to_string()
    }

    #[cfg(not(windows))]
    fn long_sleep_command() -> String {
        "sleep 30".to_string()
    }

    #[cfg(windows)]
    fn nonzero_command() -> String {
        format!("{}; exit 7", ps_write("nope"))
    }

    #[cfg(not(windows))]
    fn nonzero_command() -> String {
        "printf %s 'nope'; exit 7".to_string()
    }

    #[cfg(windows)]
    fn tty_probe_command() -> String {
        "echo TTY_READY".to_string()
    }

    #[cfg(not(windows))]
    fn tty_probe_command() -> String {
        "python3 -c 'import os,sys; print(os.isatty(0), os.isatty(1))'".to_string()
    }

    #[cfg(not(windows))]
    fn tty_probe_expected_output() -> &'static str {
        "True True"
    }

    #[cfg(windows)]
    fn tty_probe_shell() -> serde_json::Value {
        serde_json::Value::String("cmd.exe".to_string())
    }

    #[cfg(not(windows))]
    fn tty_probe_shell() -> serde_json::Value {
        serde_json::Value::Null
    }

    #[tokio::test]
    async fn exec_command_returns_completed_output() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager);
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": print_command("hello"),
            "yield_time_ms": 500
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
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
        let args = serde_json::json!({
            "cmd": delayed_print_command("start", 300, "end"),
            "yield_time_ms": 50
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();
        assert_eq!(value["exit_code"], serde_json::Value::Null);
        assert_eq!(value["running"], true);
        assert_eq!(value["long_task_candidate"], true);
        assert_eq!(value["suggested_wait_profile"], "adaptive");
        assert_eq!(value["duration_class"], "unknown_running");
        assert_eq!(value["response_strategy"], "adaptive_monitor");

        let mut final_poll = serde_json::Value::Null;
        let mut combined_output = String::new();
        for _ in 0..20 {
            let poll = manager
                .write_and_poll(ExecWriteArgs {
                    session_id: session_id.clone(),
                    chars: None,
                    since_chunk_id: None,
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
    async fn runtime_poll_exec_session_reports_unchanged_without_model_tool_call() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": delayed_print_command("", 800, "done"),
            "yield_time_ms": 50,
            "login": false
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();

        let unchanged = manager.poll_existing_session(&session_id, 250, None).await;
        assert!(unchanged.success, "{}", unchanged.output);
        let value: serde_json::Value = serde_json::from_str(&unchanged.output).unwrap();
        assert_eq!(value["exit_code"], serde_json::Value::Null);
        assert_eq!(value["running"], true);
        assert_eq!(value["unchanged"], true);
        assert_eq!(value["new_output_bytes"], 0);

        let output_poll = manager.poll_existing_session(&session_id, 1000, None).await;
        assert!(output_poll.success, "{}", output_poll.output);
        let value: serde_json::Value = serde_json::from_str(&output_poll.output).unwrap();
        assert!(value["output"].as_str().unwrap_or("").contains("done"));

        let final_poll = if value["exit_code"].is_null() {
            manager.poll_existing_session(&session_id, 1000, None).await
        } else {
            output_poll
        };
        assert!(final_poll.success, "{}", final_poll.output);
        let value: serde_json::Value = serde_json::from_str(&final_poll.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["running"], false);
    }

    #[tokio::test]
    async fn runtime_poll_exec_session_wakes_on_output_before_deadline() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": delayed_print_then_sleep_command(500, "notify-ready", 3),
            "yield_time_ms": 50,
            "login": false
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();

        let started = Instant::now();
        let poll = manager
            .poll_existing_session(&session_id, 3_000, None)
            .await;
        let elapsed = started.elapsed();

        assert!(poll.success, "{}", poll.output);
        let value: serde_json::Value = serde_json::from_str(&poll.output).unwrap();
        assert!(
            elapsed < Duration::from_millis(2_000),
            "poll waited for deadline instead of output notification: {elapsed:?}"
        );
        assert_eq!(value["running"], true);
        assert!(value["output"]
            .as_str()
            .unwrap_or("")
            .contains("notify-ready"));
        assert!(manager.terminate_session(&session_id).await);
    }

    #[test]
    fn adaptive_long_task_metadata_is_runtime_state_based() {
        assert_eq!(classify_duration_class(true, 0.1), "unknown_running");
        assert_eq!(classify_duration_class(false, 0.4), "instant");
        assert_eq!(classify_duration_class(false, 1.2), "short");
        assert_eq!(classify_response_strategy(true), "adaptive_monitor");
        assert_eq!(classify_response_strategy(false), "inline_or_manual");
    }

    #[tokio::test]
    async fn exec_command_background_watcher_observes_exit_before_next_poll() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": delayed_print_command("", 800, "watched-done"),
            "yield_time_ms": 50,
            "login": false
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();
        assert_eq!(value["exit_code"], serde_json::Value::Null);

        for _ in 0..30 {
            if manager.has_completed_session(&session_id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            manager.has_completed_session(&session_id).await,
            "background watcher did not observe exit"
        );

        let poll = manager
            .write_and_poll(ExecWriteArgs {
                session_id: session_id.clone(),
                chars: None,
                since_chunk_id: None,
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
        let args = serde_json::json!({
            "cmd": stdin_echo_command(),
            "yield_time_ms": 50
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
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
                    since_chunk_id: None,
                    yield_time_ms: Some(50),
                    max_output_tokens: None,
                })
                .await;
            assert!(poll.success, "{}", poll.output);
            let value: serde_json::Value = serde_json::from_str(&poll.output).unwrap();
            combined_output.push_str(value["output"].as_str().unwrap_or(""));
        }
        assert!(combined_output.contains("ready"), "{combined_output}");

        let output_poll = manager
            .write_and_poll(ExecWriteArgs {
                session_id: session_id.clone(),
                chars: Some("from-stdin\n".to_string()),
                since_chunk_id: None,
                yield_time_ms: Some(1000),
                max_output_tokens: None,
            })
            .await;
        assert!(output_poll.success, "{}", output_poll.output);
        let value: serde_json::Value = serde_json::from_str(&output_poll.output).unwrap();
        assert!(value["output"]
            .as_str()
            .unwrap_or("")
            .contains("from-stdin"));

        let final_poll = if value["exit_code"].is_null() {
            manager.poll_existing_session(&session_id, 1000, None).await
        } else {
            output_poll
        };
        assert!(final_poll.success, "{}", final_poll.output);
        let value: serde_json::Value = serde_json::from_str(&final_poll.output).unwrap();
        assert_eq!(value["exit_code"], 0);
        assert!(value["session_id"].is_null());
    }

    #[tokio::test]
    async fn exec_command_ctrl_c_terminates_running_process() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": long_sleep_command(),
            "yield_time_ms": 50
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
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
                    since_chunk_id: None,
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

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_command_ctrl_c_terminates_pipe_process_group_children() {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        fn process_exists(pid: i32) -> bool {
            match kill(Pid::from_raw(pid), None) {
                Ok(()) => true,
                Err(Errno::EPERM) => true,
                Err(Errno::ESRCH) => false,
                Err(_) => false,
            }
        }

        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let cmd = format!(
            "sleep 30 & child=$!; echo $child > {}; wait $child",
            pid_file.display()
        );
        let args = serde_json::json!({
            "cmd": cmd,
            "shell": "/bin/sh",
            "login": false,
            "yield_time_ms": 50
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let session_id = value["session_id"]
            .as_i64()
            .expect("session id")
            .to_string();

        for _ in 0..20 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let grandchild_pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("grandchild pid file")
            .trim()
            .parse()
            .expect("grandchild pid");
        assert!(process_exists(grandchild_pid));

        let mut final_poll = serde_json::Value::Null;
        let mut chars = Some("\u{3}".to_string());
        for _ in 0..20 {
            let poll = manager
                .write_and_poll(ExecWriteArgs {
                    session_id: session_id.clone(),
                    chars: chars.take(),
                    since_chunk_id: None,
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

        for _ in 0..20 {
            if !process_exists(grandchild_pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !process_exists(grandchild_pid),
            "grandchild process {grandchild_pid} should be killed with exec process group"
        );
    }

    #[tokio::test]
    async fn exec_command_nonzero_exit_is_successful_tool_result() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": nonzero_command(),
            "yield_time_ms": 500
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let mut value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let mut output = value["output"].as_str().unwrap_or("").to_string();
        let session_id = value["session_id"].as_i64().map(|id| id.to_string());
        if value["exit_code"].is_null() {
            let session_id = session_id.expect("running session id");
            for _ in 0..20 {
                let poll = manager.poll_existing_session(&session_id, 250, None).await;
                assert!(poll.success, "{}", poll.output);
                value = serde_json::from_str(&poll.output).unwrap();
                output.push_str(value["output"].as_str().unwrap_or(""));
                if !value["exit_code"].is_null() {
                    break;
                }
            }
        }
        assert_eq!(value["exit_code"], 7);
        assert!(output.contains("nope"), "{output}");
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

        let powershell = ShellCommand::new(Some("powershell.exe"), "Write-Output ok", true);
        assert_eq!(powershell.args[0], "-NoProfile");
        assert_eq!(powershell.args[1], "-NonInteractive");
        assert_eq!(powershell.args[2], "-Command");

        let cmd = ShellCommand::new(Some("cmd.exe"), "echo ok", true);
        assert_eq!(cmd.args[0], "/C");
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
    #[cfg_attr(
        windows,
        ignore = "Windows ConPTY child launch is not stable in hosted and ARM-emulated x86_64 test environments"
    )]
    async fn test_exec_command_tty_reports_isatty_true() {
        let manager = Arc::new(ExecSessionManager::new());
        let tool = ExecCommandTool::new(manager.clone());
        let dir = tempdir().unwrap();
        let args = serde_json::json!({
            "cmd": tty_probe_command(),
            "shell": tty_probe_shell(),
            "tty": true,
            "yield_time_ms": 3000
        })
        .to_string();
        let result = tool.execute(&args, dir.path()).await;
        assert!(result.success, "{}", result.output);
        let mut value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let mut output = value["output"].as_str().unwrap_or("").to_string();
        let session_id = value["session_id"].as_i64().map(|id| id.to_string());
        if let Some(session_id) = session_id {
            for _ in 0..20 {
                #[cfg(not(windows))]
                let probe_complete =
                    output.contains(tty_probe_expected_output()) && !value["exit_code"].is_null();
                #[cfg(windows)]
                let probe_complete = !value["exit_code"].is_null();
                if probe_complete {
                    break;
                }
                if !value["exit_code"].is_null() {
                    break;
                }
                let poll = manager.poll_existing_session(&session_id, 500, None).await;
                assert!(poll.success, "{}", poll.output);
                value = serde_json::from_str(&poll.output).unwrap();
                output.push_str(value["output"].as_str().unwrap_or(""));
            }
        }
        #[cfg(not(windows))]
        assert!(output.contains(tty_probe_expected_output()), "{output}");
        assert_eq!(value["exit_code"], 0);
        if value.get("long_task_candidate").is_some() {
            assert_eq!(value["long_task_candidate"], false);
            assert!(value["suggested_wait_profile"].is_null());
        }
    }
}
