use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use bifrost_agent::persistence::ConversationRecorder;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

const WORKER_PROTOCOL_VERSION: u32 = 1;
const WORKER_STOP_GRACE_MS: u64 = 1500;

static ACTIVE_WORKERS: once_cell::sync::Lazy<dashmap::DashMap<String, AgentWorkerStopHandle>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Clone)]
struct AgentWorkerStopHandle {
    pid: u32,
    stop_tx: mpsc::UnboundedSender<()>,
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
    pub work_dir: Option<String>,
    #[serde(default)]
    pub history_path: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub default_message_channel: Option<crate::im_gateway::types::ImMessageChannelBinding>,
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
    pub goal_needs_continuation: bool,
    #[serde(default)]
    pub goal_objective: Option<String>,
    #[serde(default)]
    pub history_path: Option<String>,
}

impl From<bifrost_agent::TurnResult> for AgentWorkerRunResult {
    fn from(value: bifrost_agent::TurnResult) -> Self {
        Self {
            response: value.response,
            tool_calls_log: value.tool_calls_log,
            work_dir_switched: value.work_dir_switched,
            title_updated: value.title_updated,
            plan_steps: value.plan_steps,
            goal_needs_continuation: value.goal_needs_continuation,
            goal_objective: value.goal_objective,
            history_path: None,
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
        Ok(Self { executable })
    }

    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub async fn spawn(&self, request: AgentWorkerRunRequest) -> Result<AgentWorkerRun, String> {
        #[cfg(test)]
        if std::env::var_os("BIFROST_FORCE_AGENT_WORKER").is_none() {
            return Ok(spawn_in_process_worker(request));
        }
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
            #[cfg(test)]
            task: None,
        })
    }
}

pub struct AgentWorkerRun {
    child: Option<Child>,
    command_tx: mpsc::UnboundedSender<AgentWorkerCommand>,
    events: AgentWorkerEventStream,
    #[cfg(test)]
    task: Option<tokio::task::JoinHandle<()>>,
}

enum AgentWorkerEventStream {
    Process(tokio::io::Lines<BufReader<tokio::process::ChildStdout>>),
    #[cfg(test)]
    Channel(mpsc::UnboundedReceiver<AgentWorkerEvent>),
}

pub fn register_active_worker(session_key: &str, pid: u32, stop_tx: mpsc::UnboundedSender<()>) {
    ACTIVE_WORKERS.insert(
        session_key.to_string(),
        AgentWorkerStopHandle { pid, stop_tx },
    );
}

pub fn clear_active_worker(session_key: &str) {
    ACTIVE_WORKERS.remove(session_key);
}

pub async fn request_session_stop(session_key: &str) -> bool {
    let Some((_, handle)) = ACTIVE_WORKERS.remove(session_key) else {
        return false;
    };
    let _ = handle.stop_tx.send(());
    tokio::time::sleep(Duration::from_millis(WORKER_STOP_GRACE_MS)).await;
    let _ = crate::im_gateway::external_cli::terminate_process_group(handle.pid);
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
        #[cfg(test)]
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
    command
        .spawn()
        .map_err(|error| format!("spawn agent worker failed: {error}"))
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
        work_dir,
        history_path,
        source,
        default_message_channel: config.default_message_channel.clone(),
    }
}

pub fn run_worker_stdio() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-agent-worker")
        .build()
        .map_err(|error| format!("build agent worker runtime failed: {error}"))?;
    runtime.block_on(run_worker_stdio_async())
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
    let stdin_guide_channel = guide_channel.clone();
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
                        stdin_guide_channel.push_back(message);
                    }
                }
                Ok(AgentWorkerCommand::Run { .. }) | Err(_) => {}
            }
        }
    });

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
    let service = crate::handlers::im_gateway::ImGatewayService::new(&data_dir);
    let agent_client = service.agent_client.clone();
    let turn_tools = service.build_agent_tool_registry(config.default_message_channel.clone());
    let mut session = bifrost_agent::AgentSession::new_with_work_dir(
        &request.session_key,
        config.work_dir.clone(),
    );
    session.source = request.source.unwrap_or_else(|| "agent-worker".to_string());
    session.mark_bifrost_agent_runtime();
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
    let mut recorder = create_conversation_recorder(&config, &request.session_key, &mut session);
    let mut mcp_manager = bifrost_agent::mcp::McpManager::new(&config.mcp_servers).await;
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
            recorder.record_run_state(&request.session_key, state, Some("worker"), Some("builtin"));
    }
    result.map(|turn_result| AgentWorkerRunResult {
        history_path,
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
    let summary = bifrost_agent::persistence::scan_session_summary(&path);
    session.title = summary.title;
    if session.work_dir.is_none() {
        session.work_dir = summary.work_dir;
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
        let _ = recorder.record_run_state(session_key, "running", Some("worker"), Some("builtin"));
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
            "source": "agent-worker",
            "base_instructions": bifrost_agent::prompt::resolve_base_instructions_text(config, None),
        }),
    );
    let _ = recorder.record_run_state(session_key, "running", Some("worker"), Some("builtin"));
    Some(recorder)
}

fn send_worker_event(event: &AgentWorkerEvent) -> Result<(), String> {
    let line = serde_json::to_string(event)
        .map_err(|error| format!("serialize agent worker event failed: {error}"))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .map_err(|error| format!("write agent worker event failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("write agent worker event newline failed: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush agent worker event failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn turn_result_roundtrip_preserves_stop_fields() {
        let result = bifrost_agent::TurnResult {
            response: "ok".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: Some("/tmp/next".to_string()),
            title_updated: Some("Title".to_string()),
            plan_steps: None,
            goal_needs_continuation: true,
            goal_objective: Some("goal".to_string()),
        };
        let worker = AgentWorkerRunResult::from(result);
        let back = bifrost_agent::TurnResult::from(worker);
        assert_eq!(back.response, "ok");
        assert_eq!(back.work_dir_switched.as_deref(), Some("/tmp/next"));
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
}
