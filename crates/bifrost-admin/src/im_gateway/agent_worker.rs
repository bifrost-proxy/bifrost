use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use bifrost_agent::persistence::ConversationRecorder;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

const WORKER_PROTOCOL_VERSION: u32 = 1;
const WORKER_STOP_GRACE_MS: u64 = 1500;
const WORKER_OUTPUT_CLOSED: &str = "__bifrost_agent_worker_output_closed__";

static ACTIVE_WORKERS: once_cell::sync::Lazy<dashmap::DashMap<String, AgentWorkerStopHandle>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Clone)]
struct AgentWorkerStopHandle {
    pid: u32,
    stop_tx: mpsc::UnboundedSender<AgentWorkerStopRequest>,
    final_event_tx: Option<mpsc::UnboundedSender<bifrost_agent::AgentTurnProgressEvent>>,
}

pub struct AgentWorkerStopRequest {
    ack_tx: oneshot::Sender<()>,
    reason: Option<String>,
}

impl AgentWorkerStopRequest {
    pub fn ack(self) {
        let _ = self.ack_tx.send(());
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkerRunRequest {
    pub protocol_version: u32,
    pub session_key: String,
    pub message: String,
    #[serde(default)]
    pub config: Option<bifrost_agent::AgentConfig>,
    #[serde(default)]
    pub images: Vec<bifrost_agent::ChatImageInput>,
    #[serde(default)]
    pub queued_messages: Vec<String>,
    #[serde(default)]
    pub guide_messages: Vec<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub collaboration_mode: Option<bifrost_agent::CollaborationMode>,
    #[serde(default)]
    pub work_dir: Option<String>,
    #[serde(default)]
    pub history_path: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub default_message_channel: Option<crate::im_gateway::types::ImMessageChannelBinding>,
    #[serde(default)]
    pub agent_proxy_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkerRunResult {
    pub response: String,
    #[serde(default)]
    pub tool_calls_log: Vec<bifrost_agent::ToolCallLog>,
    #[serde(default)]
    pub work_dir_switched: Option<String>,
    #[serde(default)]
    pub title_updated: Option<String>,
    #[serde(default)]
    pub plan_steps: Option<Vec<bifrost_agent::PlanStep>>,
    #[serde(default)]
    pub proposed_plan: Option<String>,
    #[serde(default)]
    pub goal_needs_continuation: bool,
    #[serde(default)]
    pub goal_objective: Option<String>,
    #[serde(default)]
    pub history_path: Option<String>,
    /// Guide messages the worker's turn loop actually consumed (injected into
    /// history) during this run. The parent uses this to re-queue any guide it
    /// handed off but the worker never consumed — closing the IPC race where a
    /// guide arrives after the turn-end checkpoint and would otherwise be lost.
    #[serde(default)]
    pub consumed_guide_messages: Vec<String>,
}

impl From<bifrost_agent::TurnResult> for AgentWorkerRunResult {
    fn from(value: bifrost_agent::TurnResult) -> Self {
        Self {
            response: value.response,
            tool_calls_log: value.tool_calls_log,
            work_dir_switched: value.work_dir_switched,
            title_updated: value.title_updated,
            plan_steps: value.plan_steps,
            proposed_plan: value.proposed_plan,
            goal_needs_continuation: value.goal_needs_continuation,
            goal_objective: value.goal_objective,
            history_path: None,
            consumed_guide_messages: Vec::new(),
        }
    }
}

impl From<AgentWorkerRunResult> for bifrost_agent::TurnResult {
    fn from(value: AgentWorkerRunResult) -> Self {
        Self {
            response: value.response,
            tool_calls_log: value.tool_calls_log,
            work_dir_switched: value.work_dir_switched,
            title_updated: value.title_updated,
            plan_steps: value.plan_steps,
            proposed_plan: value.proposed_plan,
            goal_needs_continuation: value.goal_needs_continuation,
            goal_objective: value.goal_objective,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentWorkerCommand {
    Run { request: Box<AgentWorkerRunRequest> },
    Guide { message: String },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentWorkerEvent {
    Started {
        session_key: String,
        pid: u32,
    },
    Progress {
        event: bifrost_agent::AgentTurnProgressEvent,
    },
    Finished {
        result: AgentWorkerRunResult,
    },
    Failed {
        error: String,
    },
    Stopped,
}

#[derive(Debug, Clone)]
pub struct AgentWorkerClient {
    executable: PathBuf,
}

impl AgentWorkerClient {
    pub fn current_exe() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("resolve current executable failed: {error}"))?;
        let executable = labeled_process_executable(&executable, "bifrost-agent");
        Ok(Self { executable })
    }

    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// 启动 agent worker。生产环境必须使用外部 worker 进程；启动失败时返回错误，
    /// 避免把 Agent loop 静默回退到主进程内执行。
    pub async fn spawn_or_fallback(
        request: AgentWorkerRunRequest,
    ) -> Result<AgentWorkerRun, String> {
        #[cfg(test)]
        if std::env::var_os("BIFROST_FORCE_AGENT_WORKER").is_none() {
            return Ok(spawn_in_process_worker(request));
        }
        let client = match Self::current_exe() {
            Ok(client) => client,
            Err(error) => {
                return Err(format!("resolve agent worker executable failed: {error}"));
            }
        };
        Self::spawn_or_fallback_with_client(client, request).await
    }

    async fn spawn_or_fallback_with_client(
        client: AgentWorkerClient,
        request: AgentWorkerRunRequest,
    ) -> Result<AgentWorkerRun, String> {
        match client.spawn_process(request).await {
            Ok(worker) => Ok(worker),
            Err(error) => Err(format!(
                "agent worker process failed (executable: {}): {error}",
                client.executable.display()
            )),
        }
    }

    /// 尝试通过外部进程启动 worker，失败时返回错误。
    async fn spawn_process(
        &self,
        request: AgentWorkerRunRequest,
    ) -> Result<AgentWorkerRun, String> {
        let mut child = spawn_worker_process(&self.executable)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "agent worker stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "agent worker stdout unavailable".to_string())?;
        write_worker_command(
            &mut stdin,
            &AgentWorkerCommand::Run {
                request: Box::new(request),
            },
        )
        .await?;
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AgentWorkerCommand>();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                if write_worker_command(&mut stdin, &command).await.is_err() {
                    break;
                }
            }
        });
        Ok(AgentWorkerRun {
            child: Some(child),
            command_tx,
            events: AgentWorkerEventStream::Process(BufReader::new(stdout).lines()),
            task: None,
        })
    }
}

fn labeled_process_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    match bifrost_core::process_alias_executable(executable, &alias_dir, alias_name) {
        Ok(alias) => alias,
        Err(error) => {
            tracing::warn!(
                executable = %executable.display(),
                alias_name = %alias_name,
                error = %error,
                "falling back to unlabeled bifrost worker executable"
            );
            executable.to_path_buf()
        }
    }
}

pub struct AgentWorkerRun {
    child: Option<Child>,
    command_tx: mpsc::UnboundedSender<AgentWorkerCommand>,
    events: AgentWorkerEventStream,
    task: Option<tokio::task::JoinHandle<()>>,
}

enum AgentWorkerEventStream {
    Process(tokio::io::Lines<BufReader<tokio::process::ChildStdout>>),
    #[cfg(test)]
    Channel(mpsc::UnboundedReceiver<AgentWorkerEvent>),
}

pub fn register_active_worker(
    session_key: &str,
    pid: u32,
    stop_tx: mpsc::UnboundedSender<AgentWorkerStopRequest>,
    final_event_tx: Option<mpsc::UnboundedSender<bifrost_agent::AgentTurnProgressEvent>>,
) {
    ACTIVE_WORKERS.insert(
        session_key.to_string(),
        AgentWorkerStopHandle {
            pid,
            stop_tx,
            final_event_tx,
        },
    );
}

pub fn clear_active_worker(session_key: &str) {
    ACTIVE_WORKERS.remove(session_key);
}

fn drain_active_workers() -> Vec<(String, AgentWorkerStopHandle)> {
    let entries: Vec<(String, AgentWorkerStopHandle)> = ACTIVE_WORKERS
        .iter()
        .map(|entry| (entry.key().clone(), entry.value().clone()))
        .collect();
    for (session_key, _) in &entries {
        ACTIVE_WORKERS.remove(session_key);
    }
    entries
}

async fn stop_active_worker_entries(
    entries: Vec<(String, AgentWorkerStopHandle)>,
    grace: Duration,
    reason: Option<&str>,
) {
    if entries.is_empty() {
        return;
    }
    let mut signaled_entries = Vec::new();
    for (session_key, handle) in entries {
        tracing::info!(session_key = %session_key, pid = handle.pid, "agent worker: stopping active worker");
        if let (Some(reason), Some(final_event_tx)) = (reason, handle.final_event_tx.as_ref()) {
            let _ = final_event_tx.send(bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                error: reason.to_string(),
            });
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        if handle
            .stop_tx
            .send(AgentWorkerStopRequest {
                ack_tx,
                reason: reason.map(str::to_string),
            })
            .is_ok()
        {
            signaled_entries.push((session_key, handle.pid, ack_rx));
        } else {
            tracing::warn!(
                session_key = %session_key,
                pid = handle.pid,
                "agent worker: stop receiver is gone; skipping pid termination for stale active worker entry"
            );
        }
    }
    if signaled_entries.is_empty() || signaled_entries.iter().all(|(_, pid, _)| *pid == 0) {
        return;
    }
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let mut index = 0;
        while index < signaled_entries.len() {
            match signaled_entries[index].2.try_recv() {
                Ok(()) | Err(oneshot::error::TryRecvError::Closed) => {
                    signaled_entries.swap_remove(index);
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    index += 1;
                }
            }
        }
        if signaled_entries.is_empty() {
            return;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep(std::cmp::min(
            Duration::from_millis(10),
            deadline.saturating_duration_since(now),
        ))
        .await;
    }
    for (session_key, pid, _) in signaled_entries {
        if pid == 0 {
            continue;
        }
        if let Err(error) = crate::im_gateway::external_cli::terminate_process_group(pid) {
            tracing::warn!(
                session_key = %session_key,
                pid,
                %error,
                "agent worker: failed to terminate active worker process group"
            );
        }
    }
}

pub async fn kill_all_active_workers() {
    stop_active_worker_entries(
        drain_active_workers(),
        Duration::from_millis(WORKER_STOP_GRACE_MS),
        Some("Bifrost 正在重启或关闭，当前 Agent worker 任务已中断。请在服务恢复后重新发起任务。"),
    )
    .await;
}

pub async fn request_session_stop(session_key: &str) -> bool {
    let Some((_, handle)) = ACTIVE_WORKERS.remove(session_key) else {
        return false;
    };
    let (ack_tx, mut ack_rx) = oneshot::channel();
    if handle
        .stop_tx
        .send(AgentWorkerStopRequest {
            ack_tx,
            reason: Some("已收到 /stop，Agent worker 子进程已停止。".to_string()),
        })
        .is_err()
    {
        tracing::warn!(
            session_key,
            pid = handle.pid,
            "agent worker: stop receiver is gone; skipping pid termination for stale active worker entry"
        );
        return false;
    }
    match tokio::time::timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), &mut ack_rx).await {
        Ok(Ok(())) | Ok(Err(_)) => return true,
        Err(_) => {}
    }
    // pid == 0 means in-process worker (no external process to kill)
    if handle.pid != 0 {
        let _ = crate::im_gateway::external_cli::terminate_process_group(handle.pid);
    }
    true
}

impl AgentWorkerRun {
    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.id())
    }

    pub async fn next_event(&mut self) -> Result<Option<AgentWorkerEvent>, String> {
        match &mut self.events {
            AgentWorkerEventStream::Process(events) => {
                let Some(line) = events
                    .next_line()
                    .await
                    .map_err(|error| format!("read agent worker event failed: {error}"))?
                else {
                    let Some(child) = self.child.as_mut() else {
                        return Ok(None);
                    };
                    let status = child
                        .wait()
                        .await
                        .map_err(|error| format!("wait agent worker failed: {error}"))?;
                    if status.success() {
                        return Ok(None);
                    }
                    return Err(format!("agent worker exited before final event: {status}"));
                };
                serde_json::from_str(&line).map(Some).map_err(|error| {
                    format!(
                        "parse agent worker event failed: {error}; line={}",
                        truncate_worker_line(&line)
                    )
                })
            }
            #[cfg(test)]
            AgentWorkerEventStream::Channel(events) => Ok(events.recv().await),
        }
    }

    pub async fn request_stop(&mut self) -> Result<(), String> {
        self.command_tx
            .send(AgentWorkerCommand::Stop)
            .map_err(|error| format!("send agent worker stop command failed: {error}"))
    }

    pub async fn send_guide(&mut self, message: String) -> Result<(), String> {
        self.command_tx
            .send(AgentWorkerCommand::Guide { message })
            .map_err(|error| format!("send agent worker guide command failed: {error}"))
    }

    pub async fn terminate(mut self) -> Result<(), String> {
        let _ = self.request_stop().await;
        if let Some(task) = self.task.take() {
            task.abort();
            return Ok(());
        }
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        match tokio::time::timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), child.wait()).await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(format!("wait agent worker failed: {error}")),
            Err(_) => {
                let _ = child.kill().await;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
fn spawn_in_process_worker(request: AgentWorkerRunRequest) -> AgentWorkerRun {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AgentWorkerCommand>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentWorkerEvent>();
    let task = tokio::spawn(async move {
        let (progress_tx, mut progress_rx) =
            mpsc::unbounded_channel::<bifrost_agent::AgentTurnProgressEvent>();
        let guide_channel: bifrost_agent::session::GuideChannel =
            std::sync::Arc::new(bifrost_agent::session::GuideMessageChannel::new());
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let command_guide_channel = guide_channel.clone();
        tokio::spawn(async move {
            let mut stop_tx = Some(stop_tx);
            while let Some(command) = command_rx.recv().await {
                match command {
                    AgentWorkerCommand::Stop => {
                        if let Some(stop_tx) = stop_tx.take() {
                            let _ = stop_tx.send(());
                        }
                        break;
                    }
                    AgentWorkerCommand::Guide { message } => {
                        if !message.trim().is_empty() {
                            command_guide_channel.push_back(message);
                        }
                    }
                    AgentWorkerCommand::Run { .. } => {}
                }
            }
        });
        let _ = event_tx.send(AgentWorkerEvent::Started {
            session_key: request.session_key.clone(),
            pid: std::process::id(),
        });
        let run = tokio::spawn(run_builtin_agent_turn(request, progress_tx, guide_channel));
        tokio::pin!(run);
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    run.abort();
                    let _ = event_tx.send(AgentWorkerEvent::Stopped);
                    return;
                }
                maybe_event = progress_rx.recv() => {
                    if let Some(event) = maybe_event {
                        let _ = event_tx.send(AgentWorkerEvent::Progress { event });
                    }
                }
                result = &mut run => {
                    while let Ok(event) = progress_rx.try_recv() {
                        let _ = event_tx.send(AgentWorkerEvent::Progress { event });
                    }
                    match result {
                        Ok(Ok(result)) => { let _ = event_tx.send(AgentWorkerEvent::Finished { result }); }
                        Ok(Err(error)) => { let _ = event_tx.send(AgentWorkerEvent::Failed { error }); }
                        Err(error) if error.is_cancelled() => { let _ = event_tx.send(AgentWorkerEvent::Stopped); }
                        Err(error) => { let _ = event_tx.send(AgentWorkerEvent::Failed { error: format!("agent worker task failed: {error}") }); }
                    }
                    return;
                }
            }
        }
    });
    AgentWorkerRun {
        child: None,
        command_tx,
        events: AgentWorkerEventStream::Channel(event_rx),
        task: Some(task),
    }
}

fn spawn_worker_process(executable: &Path) -> Result<Child, String> {
    let mut command = Command::new(executable);
    command
        .arg("agent")
        .arg("worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map_err(|error| {
        format!(
            "spawn agent worker failed: {error} (executable: {})",
            executable.display()
        )
    })
}

async fn write_worker_command(
    stdin: &mut ChildStdin,
    command: &AgentWorkerCommand,
) -> Result<(), String> {
    let line = serde_json::to_string(command)
        .map_err(|error| format!("serialize agent worker command failed: {error}"))?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("write agent worker command failed: {error}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| format!("write agent worker command newline failed: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush agent worker command failed: {error}"))
}

fn truncate_worker_line(line: &str) -> String {
    const MAX: usize = 512;
    if line.len() <= MAX {
        line.to_string()
    } else {
        format!("{}...", &line[..MAX])
    }
}

pub fn protocol_version() -> u32 {
    WORKER_PROTOCOL_VERSION
}

pub fn build_run_request(
    session_key: impl Into<String>,
    message: impl Into<String>,
    images: Vec<bifrost_agent::ChatImageInput>,
    config: &bifrost_agent::AgentConfig,
    work_dir: Option<String>,
    history_path: Option<String>,
    source: Option<String>,
) -> AgentWorkerRunRequest {
    AgentWorkerRunRequest {
        protocol_version: WORKER_PROTOCOL_VERSION,
        session_key: session_key.into(),
        message: message.into(),
        config: Some(config.clone()),
        images,
        queued_messages: Vec::new(),
        guide_messages: Vec::new(),
        system_prompt: None,
        collaboration_mode: None,
        work_dir,
        history_path,
        source,
        default_message_channel: config.default_message_channel.clone(),
        agent_proxy_port: resolve_agent_proxy_port_from_data_dir(&bifrost_storage::data_dir()),
    }
}

fn resolve_agent_proxy_port_from_data_dir(data_dir: &Path) -> Option<u16> {
    if let Some(port) = std::env::var("BIFROST_ADMIN_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
    {
        return Some(port);
    }

    let runtime_path = data_dir.join("runtime.json");
    let runtime = std::fs::read_to_string(runtime_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&runtime).ok()?;
    value
        .get("port")
        .and_then(|port| port.as_u64())
        .and_then(|port| u16::try_from(port).ok())
}

pub fn run_worker_stdio() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-agent-worker")
        .build()
        .map_err(|error| format!("build agent worker runtime failed: {error}"))?;
    match runtime.block_on(run_worker_stdio_async()) {
        Err(error) if error == WORKER_OUTPUT_CLOSED => {
            tracing::debug!("agent worker stdout closed; exiting without reporting worker failure");
            Ok(())
        }
        result => result,
    }
}

async fn run_worker_stdio_async() -> Result<(), String> {
    let mut stdin = std::io::BufReader::new(std::io::stdin()).lines();
    let Some(first_line) = stdin
        .next()
        .transpose()
        .map_err(|error| format!("read agent worker command failed: {error}"))?
    else {
        return Err("agent worker expected a run command".to_string());
    };
    let command: AgentWorkerCommand = serde_json::from_str(&first_line)
        .map_err(|error| format!("parse agent worker command failed: {error}"))?;
    let AgentWorkerCommand::Run { request } = command else {
        return Err("agent worker first command must be run".to_string());
    };
    validate_request(&request)?;

    let (progress_tx, mut progress_rx) =
        mpsc::unbounded_channel::<bifrost_agent::AgentTurnProgressEvent>();
    let guide_channel: bifrost_agent::session::GuideChannel =
        std::sync::Arc::new(bifrost_agent::session::GuideMessageChannel::new());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    spawn_worker_command_reader(stdin, stop_tx, guide_channel.clone());

    send_worker_event(&AgentWorkerEvent::Started {
        session_key: request.session_key.clone(),
        pid: std::process::id(),
    })?;

    let run = tokio::spawn(run_builtin_agent_turn(*request, progress_tx, guide_channel));
    tokio::pin!(stop_rx);
    tokio::pin!(run);
    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                run.abort();
                send_worker_event(&AgentWorkerEvent::Stopped)?;
                return Ok(());
            }
            maybe_event = progress_rx.recv() => {
                if let Some(event) = maybe_event {
                    send_worker_event(&AgentWorkerEvent::Progress { event })?;
                }
            }
            result = &mut run => {
                while let Ok(event) = progress_rx.try_recv() {
                    send_worker_event(&AgentWorkerEvent::Progress { event })?;
                }
                match result {
                    Ok(Ok(result)) => send_worker_event(&AgentWorkerEvent::Finished { result })?,
                    Ok(Err(error)) => send_worker_event(&AgentWorkerEvent::Failed { error })?,
                    Err(error) if error.is_cancelled() => send_worker_event(&AgentWorkerEvent::Stopped)?,
                    Err(error) => send_worker_event(&AgentWorkerEvent::Failed { error: format!("agent worker task failed: {error}") })?,
                }
                return Ok(());
            }
        }
    }
}

fn spawn_worker_command_reader<R>(
    mut stdin: std::io::Lines<R>,
    stop_tx: tokio::sync::oneshot::Sender<()>,
    guide_channel: bifrost_agent::session::GuideChannel,
) where
    R: BufRead + Send + 'static,
{
    std::thread::spawn(move || {
        let mut stop_tx = Some(stop_tx);
        while let Some(Ok(line)) = stdin.next() {
            match serde_json::from_str::<AgentWorkerCommand>(&line) {
                Ok(AgentWorkerCommand::Stop) => {
                    if let Some(stop_tx) = stop_tx.take() {
                        let _ = stop_tx.send(());
                    }
                    break;
                }
                Ok(AgentWorkerCommand::Guide { message }) => {
                    if !message.trim().is_empty() {
                        guide_channel.push_back(message);
                    }
                }
                Ok(AgentWorkerCommand::Run { .. }) | Err(_) => {}
            }
        }
        // Parent death closes the command pipe. Treat EOF like a stop request
        // so a detached worker does not keep running after Bifrost exits.
        if let Some(stop_tx) = stop_tx.take() {
            let _ = stop_tx.send(());
        }
    });
}

fn validate_request(request: &AgentWorkerRunRequest) -> Result<(), String> {
    if request.protocol_version != WORKER_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported agent worker protocol version {}",
            request.protocol_version
        ));
    }
    if request.session_key.trim().is_empty() {
        return Err("session_key cannot be empty".to_string());
    }
    if request.message.trim().is_empty() && request.images.is_empty() {
        return Err("message or images are required".to_string());
    }
    Ok(())
}

async fn run_builtin_agent_turn(
    request: AgentWorkerRunRequest,
    progress_tx: mpsc::UnboundedSender<bifrost_agent::AgentTurnProgressEvent>,
    guide_channel: bifrost_agent::session::GuideChannel,
) -> Result<AgentWorkerRunResult, String> {
    let source = request.source.as_deref().unwrap_or("agent-worker");
    let mut config = request.config.clone().unwrap_or_else(|| {
        bifrost_agent::AgentConfigStore::new(&bifrost_agent::config::agent_home_dir()).load()
    });
    if request.default_message_channel.is_some() {
        config.default_message_channel = request.default_message_channel.clone();
    }
    if let Some(work_dir) = request
        .work_dir
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        config.work_dir = Some(work_dir.clone());
    }

    let data_dir = bifrost_storage::data_dir();
    let service = crate::handlers::im_gateway::ImGatewayService::new_with_agent_proxy_port(
        &data_dir,
        request
            .agent_proxy_port
            .or_else(|| resolve_agent_proxy_port_from_data_dir(&data_dir)),
    );
    let agent_client = service.agent_client.clone();
    let turn_tools = service.build_agent_tool_registry(config.default_message_channel.clone());
    let mut session = bifrost_agent::AgentSession::new_with_work_dir(
        &request.session_key,
        config.work_dir.clone(),
    );
    session.source = source.to_string();
    session.mark_bifrost_agent_runtime();
    session.active_turn_status = Some(std::sync::Arc::new(std::sync::Mutex::new(
        bifrost_agent::ActiveTurnStatus::new(&request.session_key),
    )));
    session.progress_sender = Some(progress_tx);
    session.guide_channel = Some(guide_channel);
    for message in &request.guide_messages {
        if !message.trim().is_empty() {
            if let Some(channel) = session.guide_channel.as_ref() {
                channel.push_back(message.clone());
            }
        }
    }
    for message in &request.queued_messages {
        if !message.trim().is_empty() {
            session.pending_messages.push_back(message.clone());
        }
    }
    if let Some(history_path) = request
        .history_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        restore_session_from_history_path(
            &mut session,
            history_path,
            &request.session_key,
            config
                .history
                .as_ref()
                .and_then(|history| history.max_bytes),
        )?;
    } else {
        restore_latest_session_for_key(
            &mut session,
            &request.session_key,
            config
                .history
                .as_ref()
                .and_then(|history| history.max_bytes),
        );
    }
    let mut recorder =
        create_conversation_recorder(&config, &request.session_key, &mut session, source);
    let mcp_http_network = agent_client
        .model_proxy_url()
        .map(|proxy_url| {
            bifrost_agent::mcp::McpHttpNetwork::with_proxy_and_ca(
                proxy_url.to_string(),
                Some(data_dir.join("certs").join("ca.crt")),
            )
        })
        .unwrap_or_else(bifrost_agent::mcp::McpHttpNetwork::direct);
    let mut mcp_manager = bifrost_agent::mcp::McpManager::new_with_http_network(
        &config.mcp_servers,
        mcp_http_network,
    )
    .await;
    let mcp_opt = if mcp_manager.list_tools().is_empty() {
        None
    } else {
        Some(&mut mcp_manager)
    };
    let mut result = if request.message.trim() == "/compact" {
        bifrost_agent::session::run_manual_compaction_command(
            &agent_client,
            &config,
            &mut session,
            recorder.as_mut(),
        )
        .await
    } else {
        bifrost_agent::session::run_turn_with_mcp_multimodal(
            &agent_client,
            &config,
            &mut session,
            &turn_tools,
            mcp_opt,
            &request.message,
            &request.images,
            request.system_prompt.as_deref(),
            recorder.as_mut(),
            request
                .collaboration_mode
                .unwrap_or(bifrost_agent::CollaborationMode::Default),
        )
        .await
    };
    const MAX_GOAL_CONTINUATIONS: usize = 25;
    let mut continuation_count = 0;
    while let Ok(ref turn_result) = result {
        if !turn_result.goal_needs_continuation || continuation_count >= MAX_GOAL_CONTINUATIONS {
            break;
        }
        continuation_count += 1;
        let Some(continuation_msg) = bifrost_agent::tools::goal::get_continuation_prompt(&session)
        else {
            break;
        };
        result = bifrost_agent::session::run_turn_with_mcp_multimodal(
            &agent_client,
            &config,
            &mut session,
            &turn_tools,
            None,
            &continuation_msg,
            &[],
            request.system_prompt.as_deref(),
            recorder.as_mut(),
            request
                .collaboration_mode
                .unwrap_or(bifrost_agent::CollaborationMode::Default),
        )
        .await;
    }
    mcp_manager.shutdown().await;
    let history_path = recorder
        .as_ref()
        .map(|recorder| recorder.file_path().display().to_string());
    if let Some(recorder) = recorder.as_mut() {
        let state = if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let _ =
            recorder.record_run_state(&request.session_key, state, Some(source), Some("builtin"));
    }
    result.map(|turn_result| AgentWorkerRunResult {
        history_path,
        consumed_guide_messages: session.consumed_guide_messages.clone(),
        ..AgentWorkerRunResult::from(turn_result)
    })
}

fn restore_session_from_history_path(
    session: &mut bifrost_agent::AgentSession,
    history_path: &str,
    expected_session_key: &str,
    max_bytes: Option<usize>,
) -> Result<(), String> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path =
        bifrost_agent::persistence::validate_conversation_path(&data_dir, Path::new(history_path))?;
    let report = bifrost_agent::persistence::load_conversation_lossy(&path)?;
    if let Some(restored_key) = report.session_key.as_deref() {
        if restored_key != expected_session_key {
            return Err("history session_key does not match the requested session_key".to_string());
        }
    }
    if report.messages.is_empty() {
        return Err("history file does not contain restorable chat messages".to_string());
    }
    session.history = report.messages;
    session.history_version = session.history_version.saturating_add(1);
    session.last_response_tokens = None;
    session.last_response_history_len = None;
    session.memory_cleared = false;
    let summary = bifrost_agent::persistence::scan_session_summary(&path);
    session.title = summary.title;
    if session.work_dir.is_none() {
        session.work_dir = summary.work_dir;
    }
    if !summary.source.is_empty() {
        session.source = summary.source;
    }
    match bifrost_agent::persistence::load_session_runtime_state(&path) {
        Ok(runtime_state) => {
            session.current_goal = runtime_state.current_goal;
            session.current_plan = runtime_state.current_plan;
            session.total_tokens_used = runtime_state.total_tokens_used;
            session.restore_token_snapshot(runtime_state.last_response_tokens);
            session.compaction_count = runtime_state.compaction_count;
            session.resolved_base_instructions = runtime_state.base_instructions;
            bifrost_agent::tools::goal::goal_runtime_apply(
                session,
                bifrost_agent::tools::goal::GoalRuntimeEvent::ThreadResumed,
            );
        }
        Err(error) => {
            tracing::warn!(history_path = %path.display(), error = %error, "restored worker agent history without runtime state");
            session.total_tokens_used = Some(summary.total_tokens);
            session.compaction_count = summary.compaction_count;
        }
    }
    session.recorder = Some(ConversationRecorder::from_existing_file(path, max_bytes));
    Ok(())
}

fn restore_latest_session_for_key(
    session: &mut bifrost_agent::AgentSession,
    session_key: &str,
    max_bytes: Option<usize>,
) {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let Some(path) = bifrost_agent::persistence::list_conversations(&data_dir, Some(session_key))
        .into_iter()
        .max_by_key(|path| bifrost_agent::persistence::scan_session_summary(path).end_time)
    else {
        return;
    };
    let _ = restore_session_from_history_path(
        session,
        &path.display().to_string(),
        session_key,
        max_bytes,
    );
}

fn create_conversation_recorder(
    config: &bifrost_agent::AgentConfig,
    session_key: &str,
    session: &mut bifrost_agent::AgentSession,
    source: &str,
) -> Option<ConversationRecorder> {
    if config.is_ephemeral() {
        return None;
    }
    let should_persist = config
        .history
        .as_ref()
        .map(|history| history.persistence != bifrost_agent::HistoryPersistence::None)
        .unwrap_or(true);
    if !should_persist {
        return None;
    }
    if let Some(mut recorder) = session.recorder.take() {
        let _ = recorder.record_run_state(session_key, "running", Some(source), Some("builtin"));
        return Some(recorder);
    }
    let data_dir = bifrost_agent::config::agent_home_dir();
    let max_bytes = config
        .history
        .as_ref()
        .and_then(|history| history.max_bytes);
    let mut recorder = ConversationRecorder::new_with_max_bytes(&data_dir, session_key, max_bytes);
    let _ = recorder.record_session_start(
        session_key,
        serde_json::json!({
            "model": config.model,
            "provider": config.model_provider,
            "source": source,
            "base_instructions": bifrost_agent::prompt::resolve_base_instructions_text(config, None),
        }),
    );
    let _ = recorder.record_run_state(session_key, "running", Some(source), Some("builtin"));
    Some(recorder)
}

fn send_worker_event(event: &AgentWorkerEvent) -> Result<(), String> {
    let line = serde_json::to_string(event)
        .map_err(|error| format!("serialize agent worker event failed: {error}"))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .map_err(map_worker_event_write_error)?;
    stdout
        .write_all(b"\n")
        .map_err(map_worker_event_write_error)?;
    stdout.flush().map_err(map_worker_event_write_error)
}

fn map_worker_event_write_error(error: std::io::Error) -> String {
    if matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
    ) {
        WORKER_OUTPUT_CLOSED.to_string()
    } else {
        format!("write agent worker event failed: {error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn admin_port_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn active_workers_registry_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    fn missing_worker_executable() -> &'static str {
        if cfg!(windows) {
            r"C:\definitely\missing\bifrost-agent-worker.exe"
        } else {
            "/definitely/missing/bifrost-agent-worker"
        }
    }

    fn is_missing_executable_error(error: &str) -> bool {
        error.contains("No such file")
            || error.contains("not found")
            || error.contains("cannot find the path")
            || error.contains("The system cannot find")
            || error.contains("(os error 3)")
    }

    #[test]
    fn build_run_request_uses_protocol_version_and_session() {
        let config = bifrost_agent::AgentConfig::default();
        let request = build_run_request(
            "s1",
            "hello",
            Vec::new(),
            &config,
            Some("/tmp".to_string()),
            None,
            Some("api".to_string()),
        );
        assert_eq!(request.protocol_version, protocol_version());
        assert_eq!(request.session_key, "s1");
        assert_eq!(request.message, "hello");
        assert_eq!(request.work_dir.as_deref(), Some("/tmp"));
        assert_eq!(request.source.as_deref(), Some("api"));
        assert!(request.config.is_some());
    }

    #[test]
    fn worker_proxy_port_resolution_reads_runtime_file_when_env_missing() {
        let _lock = admin_port_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _port_guard = EnvVarGuard::remove("BIFROST_ADMIN_PORT");
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp_dir.path().join("runtime.json"),
            r#"{"pid":123,"port":19191,"host":"127.0.0.1"}"#,
        )
        .expect("write runtime");

        assert_eq!(
            resolve_agent_proxy_port_from_data_dir(temp_dir.path()),
            Some(19191)
        );
    }

    #[test]
    fn worker_proxy_port_resolution_prefers_environment() {
        let _lock = admin_port_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _guard = EnvVarGuard::set("BIFROST_ADMIN_PORT", "19192");
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp_dir.path().join("runtime.json"),
            r#"{"pid":123,"port":19191,"host":"127.0.0.1"}"#,
        )
        .expect("write runtime");

        assert_eq!(
            resolve_agent_proxy_port_from_data_dir(temp_dir.path()),
            Some(19192)
        );
    }

    #[test]
    fn turn_result_roundtrip_preserves_stop_fields() {
        let result = bifrost_agent::TurnResult {
            response: "ok".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: Some("/tmp/next".to_string()),
            title_updated: Some("Title".to_string()),
            plan_steps: None,
            proposed_plan: Some("Plan".to_string()),
            goal_needs_continuation: true,
            goal_objective: Some("goal".to_string()),
        };
        let worker = AgentWorkerRunResult::from(result);
        let back = bifrost_agent::TurnResult::from(worker);
        assert_eq!(back.response, "ok");
        assert_eq!(back.work_dir_switched.as_deref(), Some("/tmp/next"));
        assert_eq!(back.proposed_plan.as_deref(), Some("Plan"));
        assert!(back.goal_needs_continuation);
    }

    #[test]
    fn validate_request_rejects_bad_protocol() {
        let mut request = build_run_request(
            "s1",
            "hello",
            Vec::new(),
            &bifrost_agent::AgentConfig::default(),
            None,
            None,
            None,
        );
        request.protocol_version = 0;
        assert!(validate_request(&request).unwrap_err().contains("protocol"));
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[tokio::test]
    async fn worker_command_reader_requests_stop_on_stdin_eof() {
        let input = std::io::Cursor::new(Vec::<u8>::new());
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let guide_channel: bifrost_agent::session::GuideChannel =
            std::sync::Arc::new(bifrost_agent::session::GuideMessageChannel::new());

        spawn_worker_command_reader(
            std::io::BufReader::new(input).lines(),
            stop_tx,
            guide_channel,
        );

        tokio::time::timeout(Duration::from_secs(1), stop_rx)
            .await
            .expect("stdin EOF should request stop")
            .expect("stop receiver should be signalled");
    }

    #[tokio::test]
    async fn stop_active_worker_entries_sends_stop_and_clears_registry() {
        let _lock = active_workers_registry_lock().lock().await;
        let _ = drain_active_workers();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        register_active_worker("test-stop-all-workers", 0, stop_tx, None);

        let entries = drain_active_workers();
        assert_eq!(entries.len(), 1);
        stop_active_worker_entries(entries, Duration::ZERO, Some("test shutdown")).await;

        let request = stop_rx
            .recv()
            .await
            .expect("stop receiver should be signalled");
        assert_eq!(request.reason(), Some("test shutdown"));
        assert!(!request_session_stop("test-stop-all-workers").await);
    }

    #[tokio::test]
    async fn stop_active_worker_entries_sends_progress_finalizer() {
        let _lock = active_workers_registry_lock().lock().await;
        let _ = drain_active_workers();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        register_active_worker("test-stop-finalizer", 0, stop_tx, Some(progress_tx));

        let entries = drain_active_workers();
        stop_active_worker_entries(entries, Duration::ZERO, Some("host restart")).await;

        assert!(stop_rx.recv().await.is_some());
        match progress_rx.recv().await {
            Some(bifrost_agent::AgentTurnProgressEvent::TurnFailed { error }) => {
                assert_eq!(error, "host restart");
            }
            other => panic!("expected TurnFailed finalizer, got {other:?}"),
        }
    }

    #[test]
    fn map_worker_event_write_error_treats_broken_pipe_as_closed_output() {
        let broken_pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed by parent");
        assert_eq!(
            map_worker_event_write_error(broken_pipe),
            WORKER_OUTPUT_CLOSED
        );

        let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "parent disconnected");
        assert_eq!(map_worker_event_write_error(reset), WORKER_OUTPUT_CLOSED);

        let other = std::io::Error::other("disk full");
        assert!(map_worker_event_write_error(other).contains("disk full"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_active_worker_entry_does_not_kill_pid_when_stop_receiver_is_gone() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command as StdProcessCommand, Stdio as StdStdio};

        let _lock = active_workers_registry_lock().lock().await;
        let _ = drain_active_workers();
        let mut child = StdProcessCommand::new("sh")
            .arg("-c")
            .arg("sleep 5")
            .stdin(StdStdio::null())
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn protected process");
        let pid = child.id();
        let (stop_tx, stop_rx) = mpsc::unbounded_channel();
        drop(stop_rx);
        register_active_worker("test-stale-worker-entry", pid, stop_tx, None);

        let entries = drain_active_workers();
        stop_active_worker_entries(entries, Duration::from_millis(20), None).await;

        assert!(
            child.try_wait().expect("poll protected process").is_none(),
            "stale worker registry entry must not terminate a pid when stop receiver is gone"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acknowledged_active_worker_stop_does_not_kill_pid() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command as StdProcessCommand, Stdio as StdStdio};

        let _lock = active_workers_registry_lock().lock().await;
        let _ = drain_active_workers();
        let mut child = StdProcessCommand::new("sh")
            .arg("-c")
            .arg("sleep 5")
            .stdin(StdStdio::null())
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn protected process");
        let pid = child.id();
        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
        register_active_worker("test-acked-worker-entry", pid, stop_tx, None);
        let ack_task = tokio::spawn(async move {
            if let Some(stop_request) = stop_rx.recv().await {
                stop_request.ack();
            }
        });

        let entries = drain_active_workers();
        stop_active_worker_entries(entries, Duration::from_millis(20), None).await;
        ack_task.await.expect("ack task");

        assert!(
            child.try_wait().expect("poll protected process").is_none(),
            "acknowledged worker stop must not be followed by pid termination"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn spawn_process_fallback_to_in_process_on_missing_executable() {
        let _lock = crate::test_env::agent_worker_env_lock().lock().await;
        // Force external worker mode by setting the env var
        let _guard = EnvVarGuard::set("BIFROST_FORCE_AGENT_WORKER", "1");
        // Use a non-existent executable to trigger the fallback
        let client = AgentWorkerClient::with_executable(missing_worker_executable());
        let request = build_run_request(
            "test-fallback",
            "hello",
            Vec::new(),
            &bifrost_agent::AgentConfig::default(),
            None,
            None,
            None,
        );
        // spawn_process should fail with ENOENT
        let result = client.spawn_process(request.clone()).await;
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("expected spawn_process to fail with missing executable"),
        };
        assert!(
            is_missing_executable_error(&error),
            "expected ENOENT error, got: {error}"
        );
    }

    #[tokio::test]
    async fn spawn_or_fallback_uses_in_process_worker_in_tests_without_force_env() {
        let _lock = crate::test_env::agent_worker_env_lock().lock().await;
        // Ensure we're NOT in forced external worker mode
        let _guard = EnvVarGuard::remove("BIFROST_FORCE_AGENT_WORKER");
        let request = build_run_request(
            "test-always-ok",
            "hello",
            Vec::new(),
            &bifrost_agent::AgentConfig::default(),
            None,
            None,
            None,
        );
        let worker = AgentWorkerClient::spawn_or_fallback(request)
            .await
            .expect("test fallback worker");
        // In test mode (without BIFROST_FORCE_AGENT_WORKER), should use in-process worker
        assert!(worker.child_id().is_none());
        // Should be able to terminate without error
        let _ = worker.terminate().await;
    }

    #[tokio::test]
    async fn spawn_or_fallback_fails_closed_when_forced_worker_cannot_start() {
        let _lock = crate::test_env::agent_worker_env_lock().lock().await;
        let _guard = EnvVarGuard::set("BIFROST_FORCE_AGENT_WORKER", "1");
        let client = AgentWorkerClient::with_executable(missing_worker_executable());
        let request = build_run_request(
            "test-fail-closed",
            "hello",
            Vec::new(),
            &bifrost_agent::AgentConfig::default(),
            None,
            None,
            None,
        );
        let error = match AgentWorkerClient::spawn_or_fallback_with_client(client, request).await {
            Err(error) => error,
            Ok(_) => panic!("missing worker executable should fail"),
        };
        assert!(
            error.contains("agent worker process failed") && is_missing_executable_error(&error),
            "{error}"
        );
    }
}
