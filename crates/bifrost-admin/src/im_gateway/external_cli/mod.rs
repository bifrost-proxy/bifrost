use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write as StdWrite};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

use bifrost_agent::{PlanStep, PlanStepStatus};

const DEFAULT_RUNTIME: &str = "external_cli";
const DEFAULT_ADAPTER: &str = "codex";
pub const TRAEX_ADAPTER: &str = "traex";
pub const DEFAULT_CODEX_RUNNER_ID: &str = "Codex";
pub const DEFAULT_TREEX_RUNNER_ID: &str = "TreeX";
const CONFIG_FILENAME: &str = "im_gateway_external_cli_agent.json";
const CONFIG_VERSION: u32 = 1;
const MAX_EXTERNAL_RUNNER_IMAGES_PER_MESSAGE: usize = 6;
const WORKER_STOP_GRACE_MS: u64 = 1500;
#[cfg(unix)]
const PROCESS_KILL_GRACE_MS: u64 = 250;

static ACTIVE_RUNS: once_cell::sync::Lazy<dashmap::DashMap<String, u32>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static ACTIVE_SESSIONS: once_cell::sync::Lazy<dashmap::DashMap<String, String>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static ACTIVE_WORKER_SESSIONS: once_cell::sync::Lazy<
    dashmap::DashMap<String, ExternalCliWorkerStopHandle>,
> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Clone)]
struct ExternalCliWorkerStopHandle {
    pid: u32,
    stop_tx: tokio::sync::mpsc::UnboundedSender<ExternalCliWorkerStopRequest>,
}

struct ExternalCliWorkerStopRequest {
    ack_tx: oneshot::Sender<()>,
}

impl ExternalCliWorkerStopRequest {
    fn ack(self) {
        let _ = self.ack_tx.send(());
    }
}

pub(crate) fn terminate_process_group(pid: u32) -> Result<(), String> {
    terminate_process(pid)
}

pub async fn request_worker_session_stop(session_key: &str) -> bool {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return false;
    }
    let Some((_, handle)) = ACTIVE_WORKER_SESSIONS.remove(session_key) else {
        return false;
    };
    let (ack_tx, mut ack_rx) = oneshot::channel();
    if handle
        .stop_tx
        .send(ExternalCliWorkerStopRequest { ack_tx })
        .is_err()
    {
        tracing::warn!(
            session_key,
            pid = handle.pid,
            "external_cli worker: stop receiver is gone; skipping pid termination for stale worker entry"
        );
        return false;
    }
    match tokio::time::timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), &mut ack_rx).await {
        Ok(Ok(())) | Ok(Err(_)) => return true,
        Err(_) => {}
    }
    let _ = terminate_process(handle.pid);
    true
}

pub fn run_worker_stdio() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-external-runner-worker")
        .build()
        .map_err(|error| format!("build external runner worker runtime failed: {error}"))?;
    runtime.block_on(run_worker_stdio_async())
}

async fn run_worker_stdio_async() -> Result<(), String> {
    let mut stdin = std::io::BufReader::new(std::io::stdin()).lines();
    let Some(first_line) = stdin
        .next()
        .transpose()
        .map_err(|error| format!("read external runner worker command failed: {error}"))?
    else {
        return Err("external runner worker expected a run command".to_string());
    };
    let ExternalCliWorkerCommand::Run { request } = serde_json::from_str(&first_line)
        .map_err(|error| format!("parse external runner worker command failed: {error}"))?
    else {
        return Err("external runner worker first command must be run".to_string());
    };
    if request.protocol_version != CONFIG_VERSION {
        return Err(format!(
            "unsupported external runner worker protocol version {}",
            request.protocol_version
        ));
    }
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        while let Some(Ok(line)) = stdin.next() {
            if matches!(
                serde_json::from_str::<ExternalCliWorkerCommand>(&line),
                Ok(ExternalCliWorkerCommand::Stop)
            ) {
                let _ = stop_tx.send(());
                break;
            }
        }
    });
    send_external_cli_worker_event(&ExternalCliWorkerEvent::Started {
        session_key: request.request.session_key.clone(),
        pid: std::process::id(),
    })?;
    let request = *request;
    let runtime = ExternalCliRuntime::new(PathBuf::from(&request.runs_root));
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let progress_task = tokio::spawn(async move {
        while let Some(event) = progress_rx.recv().await {
            let _ = send_external_cli_worker_event(&ExternalCliWorkerEvent::Progress { event });
        }
    });
    let run = tokio::spawn(async move {
        runtime
            .run_in_current_process_with_progress(request.request, Some(progress_tx))
            .await
    });
    tokio::pin!(stop_rx);
    tokio::pin!(run);
    tokio::select! {
        _ = &mut stop_rx => {
            kill_all_active_runs();
            run.abort();
            send_external_cli_worker_event(&ExternalCliWorkerEvent::Stopped)?;
        }
        result = &mut run => {
            match result {
                Ok(Ok(result)) => {
                    let _ = progress_task.await;
                    send_external_cli_worker_event(&ExternalCliWorkerEvent::Finished { result: Box::new(result) })?
                },
                Ok(Err(error)) => send_external_cli_worker_event(&ExternalCliWorkerEvent::Failed { error })?,
                Err(error) if error.is_cancelled() => send_external_cli_worker_event(&ExternalCliWorkerEvent::Stopped)?,
                Err(error) => send_external_cli_worker_event(&ExternalCliWorkerEvent::Failed { error: format!("external runner worker task failed: {error}") })?,
            }
        }
    }
    Ok(())
}

mod command_spec;
use command_spec::build_command_spec;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunRequest {
    pub message: String,
    #[serde(default)]
    pub images: Vec<ExternalCliImageInput>,
    #[serde(default = "default_operation")]
    pub operation: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_adapter")]
    pub adapter: String,
    #[serde(default)]
    pub work_dir: Option<PathBuf>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub adapter_config: ExternalCliAdapterConfig,
    #[serde(default)]
    pub allow_work_dirs: Vec<String>,
    #[serde(default)]
    pub inject_bifrost_tools: bool,
    #[serde(default)]
    pub skill_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliImageInput {
    #[serde(default = "default_image_mime_type", alias = "mime_type")]
    pub mime_type: String,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn default_image_mime_type() -> String {
    "image/png".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliSavedImageAttachment {
    pub path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliAdapterConfig {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub profile_v2: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default, alias = "approvalPolicy", alias = "approval-policy")]
    pub approval_policy: Option<String>,
    #[serde(default, alias = "permissionMode", alias = "permission-mode")]
    pub permission_mode: Option<String>,
    #[serde(default, alias = "reasoningEffort", alias = "reasoning-effort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, alias = "reasoningSummary", alias = "reasoning-summary")]
    pub reasoning_summary: Option<String>,
    #[serde(default, alias = "dangerFullAccess", alias = "danger-full-access")]
    pub danger_full_access: Option<bool>,
    #[serde(
        default,
        alias = "dangerouslyBypassHookTrust",
        alias = "dangerously-bypass-hook-trust",
        alias = "bypassHookTrust",
        alias = "bypass-hook-trust"
    )]
    pub dangerously_bypass_hook_trust: Option<bool>,
    #[serde(default, alias = "strictConfig", alias = "strict-config")]
    pub strict_config: Option<bool>,
    #[serde(default, alias = "skipGitRepoCheck", alias = "skip-git-repo-check")]
    pub skip_git_repo_check: Option<bool>,
    #[serde(default, alias = "ignoreUserConfig", alias = "ignore-user-config")]
    pub ignore_user_config: Option<bool>,
    #[serde(default, alias = "ignoreRules", alias = "ignore-rules")]
    pub ignore_rules: Option<bool>,
    #[serde(default)]
    pub oss: Option<bool>,
    #[serde(default, alias = "localProvider", alias = "local-provider")]
    pub local_provider: Option<String>,
    #[serde(default, alias = "outputSchema", alias = "output-schema")]
    pub output_schema: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default, alias = "addDirs", alias = "add-dirs")]
    pub add_dirs: Vec<String>,
    #[serde(default, alias = "configOverrides", alias = "config-overrides")]
    pub config_overrides: Vec<String>,
    #[serde(default, alias = "enableFeatures", alias = "enable-features")]
    pub enable_features: Vec<String>,
    #[serde(default, alias = "disableFeatures", alias = "disable-features")]
    pub disable_features: Vec<String>,
    #[serde(default)]
    pub search: Option<bool>,
    #[serde(default)]
    pub ephemeral: Option<bool>,
    #[serde(
        default,
        rename = "timeoutSecs",
        alias = "timeout_secs",
        alias = "timeout-secs"
    )]
    pub timeout_secs: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalCliResolvedModelConfig {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_summary: Option<String>,
    pub model_source: Option<String>,
    pub reasoning_source: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCliDeliveryMode {
    NoIm,
    #[default]
    FinalReply,
    ProgressCard,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliAgentSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_adapter")]
    pub adapter: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub adapter_config: ExternalCliAdapterConfig,
    #[serde(default = "default_true")]
    pub inject_bifrost_tools: bool,
    #[serde(default)]
    pub skill_paths: Vec<String>,
    #[serde(default)]
    pub delivery_mode: ExternalCliDeliveryMode,
}

impl Default for ExternalCliAgentSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            adapter: DEFAULT_ADAPTER.to_string(),
            instructions: None,
            adapter_config: ExternalCliAdapterConfig::default(),
            inject_bifrost_tools: true,
            skill_paths: Vec::new(),
            delivery_mode: ExternalCliDeliveryMode::FinalReply,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliChannelSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub delivery_mode: Option<ExternalCliDeliveryMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliGatewayConfig {
    pub version: u32,
    #[serde(default = "default_runner_id")]
    pub default_runner_id: String,
    #[serde(default)]
    pub runners: BTreeMap<String, ExternalCliAgentSettings>,
    #[serde(default)]
    pub channels: BTreeMap<String, ExternalCliChannelSettings>,
}

impl Default for ExternalCliGatewayConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            default_runner_id: default_runner_id(),
            runners: default_external_cli_runners(),
            channels: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliEffectiveConfig {
    pub provider_id: Option<String>,
    pub runner_id: String,
    pub settings: ExternalCliAgentSettings,
    pub sources: BTreeMap<String, String>,
}

pub struct ExternalCliConfigStore {
    file_path: PathBuf,
    data: RwLock<ExternalCliGatewayConfig>,
}

impl ExternalCliConfigStore {
    pub fn new(data_dir: &Path) -> Self {
        let file_path = data_dir.join("admin").join(CONFIG_FILENAME);
        let loaded = load_config_from_disk(&file_path);
        let data = normalized_gateway_config(loaded.clone().unwrap_or_default());
        if loaded.as_ref() != Some(&data) {
            if let Err(error) = save_config_to_disk(&file_path, &data) {
                tracing::warn!(
                    path = %file_path.display(),
                    %error,
                    "external_cli config: failed to persist normalized default runners"
                );
            }
        }
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn load(&self) -> ExternalCliGatewayConfig {
        self.data.read().clone()
    }

    pub fn save(&self, config: ExternalCliGatewayConfig) -> Result<(), String> {
        let mut data = self.data.write();
        *data = normalized_gateway_config(config);
        save_config_to_disk(&self.file_path, &data)
    }

    pub fn update_channel(
        &self,
        provider_id: &str,
        patch: ExternalCliChannelSettings,
    ) -> Result<ExternalCliEffectiveConfig, String> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err("provider_id cannot be empty".to_string());
        }
        let mut data = self.data.write();
        let patch = normalized_channel_settings(patch);
        if is_empty_channel_settings(&patch) {
            data.channels.remove(provider_id);
        } else {
            data.channels.insert(provider_id.to_string(), patch);
        }
        save_config_to_disk(&self.file_path, &data)?;
        Ok(effective_config_for_provider(&data, Some(provider_id)))
    }
}

pub fn effective_config_for_provider(
    config: &ExternalCliGatewayConfig,
    provider_id: Option<&str>,
) -> ExternalCliEffectiveConfig {
    effective_config_for_provider_and_runner(config, provider_id, None)
}

pub fn effective_config_for_provider_and_runner(
    config: &ExternalCliGatewayConfig,
    provider_id: Option<&str>,
    runner_id_override: Option<&str>,
) -> ExternalCliEffectiveConfig {
    let mut runner_id = config.default_runner_id.trim().to_string();
    if runner_id.is_empty() {
        runner_id = default_runner_id();
    }
    if let Some(runner_id_override) = runner_id_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        runner_id = runner_id_override.to_string();
    }
    if let Some(provider_id) = provider_id {
        if runner_id_override.is_none() {
            if let Some(channel) = config.channels.get(provider_id) {
                if let Some(channel_runner_id) = channel
                    .runner_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    runner_id = channel_runner_id.to_string();
                }
            }
        }
    }
    runner_id = canonical_runner_id(config, &runner_id);
    let mut settings = config.runners.get(&runner_id).cloned().unwrap_or_default();
    let mut sources = BTreeMap::new();
    sources.insert("runnerId".to_string(), "runner".to_string());
    sources.insert("enabled".to_string(), "runner".to_string());
    sources.insert("adapter".to_string(), "runner".to_string());
    sources.insert("instructions".to_string(), "runner".to_string());
    sources.insert("adapterConfig".to_string(), "runner".to_string());
    sources.insert("injectBifrostTools".to_string(), "runner".to_string());
    sources.insert("skillPaths".to_string(), "runner".to_string());
    sources.insert("deliveryMode".to_string(), "runner".to_string());
    if runner_id_override.is_some() {
        sources.insert("runnerId".to_string(), "agent".to_string());
    }

    if let Some(provider_id) = provider_id {
        if let Some(channel) = config.channels.get(provider_id) {
            if let Some(enabled) = channel.enabled {
                settings.enabled = enabled;
                sources.insert("enabled".to_string(), "channel".to_string());
            }
            if channel.runner_id.is_some() {
                sources.insert("runnerId".to_string(), "channel".to_string());
            }
            if let Some(delivery_mode) = channel.delivery_mode {
                settings.delivery_mode = delivery_mode;
                sources.insert("deliveryMode".to_string(), "channel".to_string());
            }
        }
    }

    ExternalCliEffectiveConfig {
        provider_id: provider_id.map(ToString::to_string),
        runner_id,
        settings: normalized_agent_settings(settings),
        sources,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunResult {
    pub run_id: String,
    pub session_key: Option<String>,
    pub runtime: String,
    pub adapter: String,
    pub status: ExternalCliRunStatus,
    pub exit_code: Option<i32>,
    pub response: String,
    /// Individual response messages for per-message IM delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<String>,
    pub started_at: u64,
    pub finished_at: u64,
    pub duration_ms: u64,
    pub artifacts: ExternalCliRunArtifacts,
    pub events: Vec<ExternalCliProgressEvent>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl ExternalCliRunResult {
    fn stopped(session_key: Option<String>, adapter: String) -> Self {
        let now = now_ms();
        Self {
            run_id: format!("stopped-{now}"),
            session_key,
            runtime: DEFAULT_RUNTIME.to_string(),
            adapter,
            status: ExternalCliRunStatus::Stopped,
            exit_code: None,
            response: "External runner worker was stopped by request.".to_string(),
            responses: vec!["External runner worker was stopped by request.".to_string()],
            started_at: now,
            finished_at: now,
            duration_ms: 0,
            artifacts: ExternalCliRunArtifacts {
                run_dir: String::new(),
                prompt: String::new(),
                command_snapshot: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                normalized_events: String::new(),
                last_message: String::new(),
            },
            events: vec![ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::RunFailed,
                content: "External runner worker was stopped by request.".to_string(),
                title: Some("Stopped".to_string()),
                raw: serde_json::json!({ "type": "worker_stopped" }),
            }],
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCliRunStatus {
    Succeeded,
    Failed,
    Stopped,
    TimedOut,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunArtifacts {
    pub run_dir: String,
    pub prompt: String,
    pub command_snapshot: String,
    pub stdout: String,
    pub stderr: String,
    pub normalized_events: String,
    pub last_message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliProgressEvent {
    pub event_type: ExternalCliProgressEventType,
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCliProgressEventType {
    RunStarted,
    Status,
    PlanUpdated,
    AssistantDelta,
    AssistantFinal,
    ToolStarted,
    ToolFinished,
    RunFinished,
    RunFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunDetail {
    pub run_id: String,
    pub snapshot: serde_json::Value,
    pub events: Vec<ExternalCliProgressEvent>,
    pub response: String,
    pub stdout: String,
    pub stderr: String,
    pub artifacts: ExternalCliRunArtifacts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandSnapshot {
    executable: String,
    args: Vec<String>,
    env_keys: Vec<String>,
    work_dir: Option<String>,
    runtime: String,
    adapter: String,
    params: serde_json::Value,
    timeout_secs: Option<u64>,
}

#[derive(Clone, Debug)]
struct CommandSpec {
    executable: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    work_dir: Option<PathBuf>,
    timeout_secs: Option<u64>,
}

#[derive(Clone, Debug)]
struct CommandOutput {
    status: ExternalCliRunStatus,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    events: Vec<ExternalCliProgressEvent>,
}

#[derive(Clone, Debug)]
pub struct ExternalCliRuntime {
    runs_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalCliWorkerRunRequest {
    protocol_version: u32,
    runs_root: String,
    request: ExternalCliRunRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExternalCliWorkerCommand {
    Run {
        request: Box<ExternalCliWorkerRunRequest>,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExternalCliWorkerEvent {
    Started {
        session_key: Option<String>,
        pid: u32,
    },
    Finished {
        result: Box<ExternalCliRunResult>,
    },
    Progress {
        event: ExternalCliProgressEvent,
    },
    Failed {
        error: String,
    },
    Stopped,
}

struct ExternalCliWorkerClient {
    executable: PathBuf,
}

struct ExternalCliWorkerRun {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    events: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
}

impl ExternalCliWorkerClient {
    fn current_exe() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("resolve current executable failed: {error}"))?;
        let executable = labeled_process_executable(&executable, "bifrost-runner");
        Ok(Self { executable })
    }

    async fn spawn(
        &self,
        runs_root: PathBuf,
        request: ExternalCliRunRequest,
    ) -> Result<ExternalCliWorkerRun, String> {
        let mut child = spawn_external_cli_worker_process(&self.executable)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "external runner worker stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "external runner worker stdout unavailable".to_string())?;
        write_external_cli_worker_command(
            &mut stdin,
            &ExternalCliWorkerCommand::Run {
                request: Box::new(ExternalCliWorkerRunRequest {
                    protocol_version: CONFIG_VERSION,
                    runs_root: runs_root.display().to_string(),
                    request,
                }),
            },
        )
        .await?;
        Ok(ExternalCliWorkerRun {
            child,
            stdin,
            events: tokio::io::BufReader::new(stdout).lines(),
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
                "falling back to unlabeled bifrost runner executable"
            );
            executable.to_path_buf()
        }
    }
}

impl ExternalCliWorkerRun {
    fn child_id(&self) -> Option<u32> {
        self.child.id()
    }

    async fn request_stop(&mut self) -> Result<(), String> {
        write_external_cli_worker_command(&mut self.stdin, &ExternalCliWorkerCommand::Stop).await
    }

    async fn terminate(mut self) -> Result<(), String> {
        let _ = self.request_stop().await;
        match tokio::time::timeout(
            Duration::from_millis(WORKER_STOP_GRACE_MS),
            self.child.wait(),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(format!("wait external runner worker failed: {error}")),
            Err(_) => {
                if let Some(pid) = self.child.id() {
                    let _ = terminate_process(pid);
                }
                let _ = self.child.kill().await;
                Ok(())
            }
        }
    }

    async fn wait_final(
        &mut self,
        progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
    ) -> Result<ExternalCliRunResult, String> {
        loop {
            let Some(line) =
                self.events.next_line().await.map_err(|error| {
                    format!("read external runner worker event failed: {error}")
                })?
            else {
                let status = self
                    .child
                    .wait()
                    .await
                    .map_err(|error| format!("wait external runner worker failed: {error}"))?;
                return Err(format!(
                    "external runner worker exited before final event: {status}"
                ));
            };
            match serde_json::from_str::<ExternalCliWorkerEvent>(&line).map_err(|error| {
                format!("parse external runner worker event failed: {error}; line={line}")
            })? {
                ExternalCliWorkerEvent::Started { .. } => {}
                ExternalCliWorkerEvent::Progress { event } => {
                    if let Some(progress_tx) = progress_tx.as_ref() {
                        let _ = progress_tx.send(event);
                    }
                }
                ExternalCliWorkerEvent::Finished { result } => return Ok(*result),
                ExternalCliWorkerEvent::Failed { error } => return Err(error),
                ExternalCliWorkerEvent::Stopped => {
                    return Ok(ExternalCliRunResult::stopped(
                        None,
                        DEFAULT_ADAPTER.to_string(),
                    ))
                }
            }
        }
    }
}

impl ExternalCliRuntime {
    pub fn new(runs_root: impl Into<PathBuf>) -> Self {
        Self {
            runs_root: runs_root.into(),
        }
    }

    pub async fn run(
        &self,
        request: ExternalCliRunRequest,
    ) -> Result<ExternalCliRunResult, String> {
        self.run_with_progress(request, None).await
    }

    pub async fn run_with_progress(
        &self,
        request: ExternalCliRunRequest,
        progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
    ) -> Result<ExternalCliRunResult, String> {
        if std::env::var_os("BIFROST_EXTERNAL_CLI_WORKER").is_some() {
            return self
                .run_in_current_process_with_progress(request, progress_tx)
                .await;
        }
        #[cfg(test)]
        if std::env::var_os("BIFROST_FORCE_EXTERNAL_CLI_WORKER").is_none() {
            return self
                .run_in_current_process_with_progress(request, progress_tx)
                .await;
        }
        let worker_client = ExternalCliWorkerClient::current_exe()?;
        let mut worker = worker_client
            .spawn(self.runs_root.clone(), request.clone())
            .await?;
        let (stop_tx, mut stop_rx) =
            tokio::sync::mpsc::unbounded_channel::<ExternalCliWorkerStopRequest>();
        if let (Some(session_key), Some(pid)) = (request.session_key.as_deref(), worker.child_id())
        {
            ACTIVE_WORKER_SESSIONS.insert(
                session_key.to_string(),
                ExternalCliWorkerStopHandle { pid, stop_tx },
            );
        }
        let session_key = request.session_key.clone();
        let mut stop_ack = None;
        let result = tokio::select! {
            maybe_stop = stop_rx.recv() => {
                let _ = worker.terminate().await;
                stop_ack = maybe_stop;
                Ok(ExternalCliRunResult::stopped(request.session_key.clone(), request.adapter.clone()))
            }
            result = worker.wait_final(progress_tx) => result,
        };
        if let Some(session_key) = session_key.as_deref() {
            ACTIVE_WORKER_SESSIONS.remove(session_key);
        }
        if let Some(stop_request) = stop_ack {
            stop_request.ack();
        }
        result
    }

    async fn run_in_current_process_with_progress(
        &self,
        request: ExternalCliRunRequest,
        progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
    ) -> Result<ExternalCliRunResult, String> {
        validate_run_request(&request)?;
        validate_work_dir(&request)?;
        let started_at = now_ms();
        let run_id = format!("{}-{}", started_at, uuid::Uuid::new_v4());
        let run_dir = self.runs_root.join(&run_id);
        tokio::fs::create_dir_all(&run_dir)
            .await
            .map_err(|error| format!("create run dir failed: {error}"))?;

        let prompt_path = run_dir.join("prompt.md");
        let last_message_path = run_dir.join("last_message.md");
        let stdout_path = run_dir.join("cli.stdout.log");
        let stderr_path = run_dir.join("cli.stderr.log");
        let snapshot_path = run_dir.join("runtime_snapshot.json");
        let events_path = run_dir.join("normalized_events.jsonl");
        let stop_marker_path = run_dir.join("stop_requested");
        let saved_images = save_image_attachments(&run_dir, &request).await?;

        let prompt = build_prompt(&request, &saved_images).await?;
        tokio::fs::write(&prompt_path, &prompt)
            .await
            .map_err(|error| format!("write prompt failed: {error}"))?;

        let spec = build_command_spec(&request, &last_message_path)?;
        let cli_version = detect_cli_version(&request.adapter, &spec).await;
        let snapshot = command_snapshot(&request, &spec);
        write_json_pretty(&snapshot_path, &snapshot).await?;

        // Session conflict detection: stop any existing run for the same session_key
        // before starting a new one. This applies to ALL adapters.
        if let Some(session_key) = request.session_key.as_deref() {
            if let Some(existing_run_id) =
                ACTIVE_SESSIONS.get(session_key).map(|v| v.value().clone())
            {
                tracing::info!(
                    session_key,
                    existing_run_id = %existing_run_id,
                    new_run_id = %run_id,
                    "stopping existing active run for session before new run"
                );
                let _ = request_run_stop(&self.runs_root, &existing_run_id).await;
                // Give the old run a moment to wind down
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.clone());
        }

        let mut command_started_at = None;
        let mut command_finished_at = None;
        let run_output = if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
            let output_result =
                crate::im_gateway::chatgpt_web::run_adapter(&request, &prompt, &run_dir).await;
            remove_active_sessions_for_run(&run_id);
            let output = match output_result {
                Ok(output) => output,
                Err(error) if error.starts_with("stopped:") => {
                    crate::im_gateway::chatgpt_web::ChatGptWebRunOutput {
                        status: ExternalCliRunStatus::Stopped,
                        exit_code: None,
                        stdout: Vec::new(),
                        stderr: error.as_bytes().to_vec(),
                        response: "ChatGPT Web run was stopped by request.".to_string(),
                        responses: vec!["ChatGPT Web run was stopped by request.".to_string()],
                        events: vec![ExternalCliProgressEvent {
                            event_type: ExternalCliProgressEventType::RunFailed,
                            content: "ChatGPT Web run was stopped by request.".to_string(),
                            title: Some("Stopped".to_string()),
                            raw: serde_json::json!({ "type": "run_stopped" }),
                        }],
                        metadata: BTreeMap::new(),
                    }
                }
                Err(error) => {
                    let response = format!("ChatGPT Web run failed: {error}");
                    let mut metadata = BTreeMap::new();
                    let diagnostics_path = run_dir.join("failure_diagnostics.json");
                    if tokio::fs::try_exists(&diagnostics_path)
                        .await
                        .unwrap_or(false)
                    {
                        metadata.insert(
                            "failureDiagnostics".to_string(),
                            diagnostics_path.display().to_string(),
                        );
                    }
                    crate::im_gateway::chatgpt_web::ChatGptWebRunOutput {
                        status: ExternalCliRunStatus::Failed,
                        exit_code: None,
                        stdout: Vec::new(),
                        stderr: error.as_bytes().to_vec(),
                        response: response.clone(),
                        responses: vec![response.clone()],
                        events: vec![ExternalCliProgressEvent {
                            event_type: ExternalCliProgressEventType::RunFailed,
                            content: response,
                            title: Some("Failed".to_string()),
                            raw: serde_json::json!({
                                "type": "run_failed",
                                "adapter": crate::im_gateway::chatgpt_web::ADAPTER_ID,
                                "error": error,
                            }),
                        }],
                        metadata,
                    }
                }
            };
            tokio::fs::write(&last_message_path, output.response.as_bytes())
                .await
                .map_err(|error| format!("write last message failed: {error}"))?;
            AdapterRunOutput {
                status: output.status,
                exit_code: output.exit_code,
                stdout: output.stdout,
                stderr: output.stderr,
                events: output.events,
                responses: output.responses,
                response: output.response,
                metadata: output.metadata,
            }
        } else {
            let session_key_for_stop = request.session_key.clone();
            command_started_at = Some(now_ms());
            let command_output = run_command(
                &run_id,
                session_key_for_stop.as_deref(),
                spec.clone(),
                prompt.clone(),
                stop_marker_path.clone(),
                progress_tx,
            )
            .await?;
            command_finished_at = Some(now_ms());
            remove_active_sessions_for_run(&run_id);
            let was_stopped = tokio::fs::try_exists(&stop_marker_path)
                .await
                .unwrap_or(false);
            let stdout_text = String::from_utf8_lossy(&command_output.stdout).to_string();
            let events = if was_stopped {
                vec![ExternalCliProgressEvent {
                    event_type: ExternalCliProgressEventType::RunFailed,
                    content: "External CLI run was stopped by request.".to_string(),
                    title: Some("Stopped".to_string()),
                    raw: serde_json::json!({ "type": "run_stopped" }),
                }]
            } else {
                if command_output.events.is_empty() {
                    parse_progress_events(&stdout_text)
                } else {
                    command_output.events.clone()
                }
            };
            let response = if was_stopped {
                "External CLI run was stopped by request.".to_string()
            } else {
                final_response(&last_message_path, &stdout_text, &events).await?
            };
            let status = if was_stopped {
                ExternalCliRunStatus::Stopped
            } else {
                command_output.status
            };
            let stderr_text = String::from_utf8_lossy(&command_output.stderr).to_string();
            let response = visible_terminal_response(
                status.clone(),
                response,
                &stdout_text,
                &stderr_text,
                &events,
            );
            AdapterRunOutput {
                status,
                exit_code: if was_stopped {
                    None
                } else {
                    command_output.exit_code
                },
                stdout: command_output.stdout,
                stderr: command_output.stderr,
                events,
                responses: vec![response.clone()],
                response,
                metadata: BTreeMap::new(),
            }
        };
        let finished_at = now_ms();
        let mut metadata = run_output.metadata;
        append_external_cli_metadata(&request.adapter, &run_output.events, &mut metadata);
        append_external_cli_request_metadata(&request, &mut metadata);
        append_external_cli_observability_metadata(
            ExternalCliObservabilityInput {
                request: &request,
                spec: &spec,
                prompt: &prompt,
                saved_images: &saved_images,
                stdout: &run_output.stdout,
                stderr: &run_output.stderr,
                events: &run_output.events,
                timings: ExternalCliObservabilityTimings {
                    started_at,
                    command_started_at,
                    command_finished_at,
                    finished_at,
                },
                cli_version: cli_version.as_deref(),
            },
            &mut metadata,
        );
        if !saved_images.is_empty() {
            metadata.insert(
                "attachments.images".to_string(),
                serde_json::to_string(&saved_images).unwrap_or_else(|_| "[]".to_string()),
            );
        }
        tokio::fs::write(&stdout_path, &run_output.stdout)
            .await
            .map_err(|error| format!("write stdout failed: {error}"))?;
        tokio::fs::write(&stderr_path, &run_output.stderr)
            .await
            .map_err(|error| format!("write stderr failed: {error}"))?;
        write_events_jsonl(&events_path, &run_output.events).await?;
        let artifacts = ExternalCliRunArtifacts {
            run_dir: run_dir.display().to_string(),
            prompt: prompt_path.display().to_string(),
            command_snapshot: snapshot_path.display().to_string(),
            stdout: stdout_path.display().to_string(),
            stderr: stderr_path.display().to_string(),
            normalized_events: events_path.display().to_string(),
            last_message: last_message_path.display().to_string(),
        };

        let result = ExternalCliRunResult {
            run_id,
            session_key: request.session_key,
            runtime: request.runtime,
            adapter: request.adapter,
            status: run_output.status,
            exit_code: run_output.exit_code,
            response: run_output.response,
            responses: run_output.responses,
            started_at,
            finished_at,
            duration_ms: finished_at.saturating_sub(started_at),
            artifacts,
            events: run_output.events,
            metadata,
        };
        let result_path = run_dir.join("result.json");
        write_json_pretty(&result_path, &result).await?;
        Ok(result)
    }
}

pub fn default_runs_root() -> PathBuf {
    bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chat_runs")
}

pub fn run_request_from_settings(
    message: impl Into<String>,
    provider_id: Option<String>,
    session_key: Option<String>,
    settings: &ExternalCliAgentSettings,
) -> ExternalCliRunRequest {
    ExternalCliRunRequest {
        message: message.into(),
        images: Vec::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id,
        runner_id: None,
        session_key,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: settings.adapter.clone(),
        work_dir: None,
        instructions: settings.instructions.clone(),
        adapter_config: settings.adapter_config.clone(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: settings.inject_bifrost_tools,
        skill_paths: settings.skill_paths.clone(),
    }
}

pub fn merge_run_request_with_settings(
    mut request: ExternalCliRunRequest,
    settings: &ExternalCliAgentSettings,
) -> ExternalCliRunRequest {
    if request.adapter == DEFAULT_ADAPTER {
        request.adapter = settings.adapter.clone();
    }
    if request.instructions.is_none() {
        request.instructions = settings.instructions.clone();
    }
    if request.adapter_config == ExternalCliAdapterConfig::default() {
        request.adapter_config = settings.adapter_config.clone();
    }
    // NOTE: inject_bifrost_tools defaults to false via serde, so we can't distinguish
    // "not provided" from "explicitly set to false". Runner settings always win when
    // the request value is false. This is acceptable: runner config is authoritative.
    if !request.inject_bifrost_tools {
        request.inject_bifrost_tools = settings.inject_bifrost_tools;
    }
    if request.skill_paths.is_empty() {
        request.skill_paths = settings.skill_paths.clone();
    }
    request
}

#[derive(Clone, Debug)]
struct AdapterRunOutput {
    status: ExternalCliRunStatus,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    events: Vec<ExternalCliProgressEvent>,
    response: String,
    responses: Vec<String>,
    metadata: BTreeMap<String, String>,
}

pub async fn read_run_detail(
    runs_root: impl AsRef<Path>,
    run_id: &str,
) -> Result<ExternalCliRunDetail, String> {
    validate_run_id(run_id)?;
    let run_dir = runs_root.as_ref().join(run_id);
    let snapshot_path = run_dir.join("runtime_snapshot.json");
    let events_path = run_dir.join("normalized_events.jsonl");
    let last_message_path = run_dir.join("last_message.md");
    let stdout_path = run_dir.join("cli.stdout.log");
    let stderr_path = run_dir.join("cli.stderr.log");
    let result_path = run_dir.join("result.json");

    let snapshot: serde_json::Value = read_json(&snapshot_path).await?;
    let events = read_events_jsonl(&events_path).await?;
    let stdout = read_text_or_default(&stdout_path).await?;
    let stderr = read_text_or_default(&stderr_path).await?;
    let result = match read_json(&result_path).await {
        Ok(value) => serde_json::from_value::<ExternalCliRunResult>(value).ok(),
        Err(_) => None,
    };
    let response = result
        .as_ref()
        .map(|result| result.response.clone())
        .filter(|response| !response.trim().is_empty())
        .unwrap_or(final_response(&last_message_path, &stdout, &events).await?);
    let metadata = result.map(|result| result.metadata).unwrap_or_default();

    Ok(ExternalCliRunDetail {
        run_id: run_id.to_string(),
        snapshot,
        events,
        response,
        stdout,
        stderr,
        metadata,
        artifacts: ExternalCliRunArtifacts {
            run_dir: run_dir.display().to_string(),
            prompt: run_dir.join("prompt.md").display().to_string(),
            command_snapshot: snapshot_path.display().to_string(),
            stdout: stdout_path.display().to_string(),
            stderr: stderr_path.display().to_string(),
            normalized_events: events_path.display().to_string(),
            last_message: last_message_path.display().to_string(),
        },
    })
}

pub async fn request_run_stop(runs_root: impl AsRef<Path>, run_id: &str) -> Result<(), String> {
    validate_run_id(run_id)?;
    let run_dir = runs_root.as_ref().join(run_id);
    if !run_dir.exists() {
        return Err(format!("run '{}' not found", run_id));
    }
    tokio::fs::write(run_dir.join("stop_requested"), now_ms().to_string())
        .await
        .map_err(|error| format!("write stop marker failed: {error}"))?;
    if let Some((_, pid)) = ACTIVE_RUNS.remove(run_id) {
        terminate_process(pid)?;
    }
    remove_active_sessions_for_run(run_id);
    Ok(())
}

pub async fn request_session_stop(
    runs_root: impl AsRef<Path>,
    session_key: &str,
) -> Result<(), String> {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return Err("session_key cannot be empty".to_string());
    }
    let run_id = ACTIVE_SESSIONS
        .get(session_key)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| format!("no active external cli run for session '{}'", session_key))?;
    request_run_stop(runs_root, &run_id).await
}

fn validate_run_request(request: &ExternalCliRunRequest) -> Result<(), String> {
    if request.runtime.trim().is_empty() {
        return Err("runtime cannot be empty".to_string());
    }
    if request.runtime != DEFAULT_RUNTIME {
        return Err(format!(
            "unsupported runtime '{}'; currently supported runtime is '{}'",
            request.runtime, DEFAULT_RUNTIME
        ));
    }
    if request.adapter.trim().is_empty() {
        return Err("adapter cannot be empty".to_string());
    }
    let operation = request.operation.trim();
    let needs_message = operation.is_empty() || matches!(operation, "ask" | "create" | "send");
    let has_image = request
        .images
        .iter()
        .any(|image| !image.data.trim().is_empty());
    if needs_message && request.message.trim().is_empty() && !has_image {
        return Err("message cannot be empty".to_string());
    }
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty()
        || run_id.contains('/')
        || run_id.contains('\\')
        || run_id.contains("..")
        || run_id.starts_with('.')
    {
        return Err("invalid run_id".to_string());
    }
    Ok(())
}

fn validate_work_dir(request: &ExternalCliRunRequest) -> Result<(), String> {
    let Some(work_dir) = request.work_dir.as_ref() else {
        return Ok(());
    };
    if request.allow_work_dirs.is_empty() {
        return Ok(());
    }
    // If the work_dir doesn't exist yet (e.g. stale config), skip the
    // allowlist check — the OS will report the error at spawn time, and the
    // caller may have already cleared the path via graceful fallback.
    let canonical_work_dir = match canonicalize_for_allowlist(work_dir) {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };
    let allowed = request
        .allow_work_dirs
        .iter()
        .filter_map(|path| canonicalize_for_allowlist(Path::new(path)).ok())
        .any(|allowed_dir| canonical_work_dir.starts_with(allowed_dir));
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "work_dir '{}' is outside allowWorkDirs",
            work_dir.display()
        ))
    }
}

fn canonicalize_for_allowlist(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|error| format!("canonicalize '{}' failed: {error}", path.display()))
}

async fn build_prompt(
    request: &ExternalCliRunRequest,
    saved_images: &[ExternalCliSavedImageAttachment],
) -> Result<String, String> {
    let mut prompt = String::new();
    if let Some(instructions) = request.instructions.as_deref() {
        if !instructions.trim().is_empty() {
            prompt.push_str(instructions.trim());
            prompt.push_str("\n\n");
        }
    }
    if request.inject_bifrost_tools && request.adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID
    {
        prompt.push_str("## Bifrost Tool Context\n\n");
        prompt.push_str(
            "- You are being invoked by Bifrost IM Gateway through an external CLI adapter.\n",
        );
        prompt.push_str("- Prefer the local `bifrost` CLI for Bifrost proxy, traffic, rule, IM, and remote-invoke operations when the requested task needs those capabilities.\n");
        prompt.push_str("- Keep filesystem work inside the configured working directory and allowed extra directories.\n\n");
    }
    for skill_path in &request.skill_paths {
        let path = Path::new(skill_path);
        // Validate skill_path is within allowed directories to prevent path traversal
        if !is_skill_path_allowed(path, request.work_dir.as_deref(), &request.allow_work_dirs) {
            tracing::warn!(
                skill_path = %skill_path,
                "skill_path rejected: not within allowed directories"
            );
            continue;
        }
        let content_path = if path.is_dir() {
            path.join("SKILL.md")
        } else {
            path.to_path_buf()
        };
        let content = read_text_or_default(&content_path).await?;
        let content = content.trim();
        if !content.is_empty() {
            prompt.push_str("## Skill: ");
            prompt.push_str(&content_path.display().to_string());
            prompt.push_str("\n\n");
            prompt.push_str(content);
            prompt.push_str("\n\n");
        }
    }
    if !saved_images.is_empty() {
        prompt.push_str("## Attached Images\n\n");
        prompt.push_str(
            "The user pasted the following local image files. Use these paths when you need to inspect or reason about the images.\n\n",
        );
        for (index, image) in saved_images.iter().enumerate() {
            prompt.push_str(&format!(
                "{}. `{}` (mime_type: {}, size_bytes: {})\n",
                index + 1,
                image.path,
                image.mime_type,
                image.size_bytes
            ));
        }
        prompt.push('\n');
    }
    prompt.push_str(request.message.trim());
    prompt.push('\n');
    Ok(prompt)
}

async fn save_image_attachments(
    run_dir: &Path,
    request: &ExternalCliRunRequest,
) -> Result<Vec<ExternalCliSavedImageAttachment>, String> {
    let images = &request.images;
    let normalized: Vec<&ExternalCliImageInput> = images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .take(MAX_EXTERNAL_RUNNER_IMAGES_PER_MESSAGE)
        .collect();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    if images.len() > MAX_EXTERNAL_RUNNER_IMAGES_PER_MESSAGE {
        tracing::warn!(
            image_count = images.len(),
            max_images = MAX_EXTERNAL_RUNNER_IMAGES_PER_MESSAGE,
            "too many external runner images in one request; truncating images"
        );
    }
    let images_dir = trusted_session_attachment_base_dir(request)
        .map(|value| {
            value.join(
                run_dir
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("run")),
            )
        })
        .unwrap_or_else(|| run_dir.join("attachments"))
        .join("images");
    tokio::fs::create_dir_all(&images_dir)
        .await
        .map_err(|error| format!("create image attachments dir failed: {error}"))?;
    let mut saved = Vec::with_capacity(normalized.len());
    for (index, image) in normalized.into_iter().enumerate() {
        let bytes = decode_image_data(&image.data)?;
        let ext = image_extension(&image.mime_type);
        let path = images_dir.join(format!("image-{}.{}", index + 1, ext));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|error| format!("write image attachment failed: {error}"))?;
        saved.push(ExternalCliSavedImageAttachment {
            path: path.display().to_string(),
            mime_type: image.mime_type.clone(),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            name: image.name.clone(),
        });
    }
    Ok(saved)
}

fn trusted_session_attachment_base_dir(request: &ExternalCliRunRequest) -> Option<PathBuf> {
    let value = request
        .params
        .get("attachmentBaseDir")
        .or_else(|| request.params.get("attachment_base_dir"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let path = PathBuf::from(value);
    let has_parent_dir = path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir));
    let sessions_root = bifrost_agent::config::agent_home_dir().join("sessions");
    if path.is_absolute() && !has_parent_dir && path.starts_with(&sessions_root) {
        Some(path)
    } else {
        tracing::warn!(
            attachment_base_dir = %value,
            sessions_root = %sessions_root.display(),
            "ignoring untrusted external runner attachment base dir"
        );
        None
    }
}

fn decode_image_data(data: &str) -> Result<Vec<u8>, String> {
    let data = data.trim();
    let payload = data
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .map(|(_, payload)| payload)
        .unwrap_or(data);
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| format!("decode image attachment failed: {error}"))
}

fn image_extension(mime_type: &str) -> &'static str {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/png" => "png",
        _ => "img",
    }
}

fn is_skill_path_allowed(
    skill_path: &Path,
    work_dir: Option<&Path>,
    allow_work_dirs: &[String],
) -> bool {
    // Reject paths containing ".." components (path traversal)
    if skill_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    // Allow simple relative paths without traversal (resolved relative to work_dir)
    if skill_path.is_relative() {
        return true;
    }
    // Check if the path is under the work_dir
    if let Some(work_dir) = work_dir {
        if skill_path.starts_with(work_dir) {
            return true;
        }
    }
    // Check if under any allowed extra directory
    for allowed in allow_work_dirs {
        if skill_path.starts_with(Path::new(allowed)) {
            return true;
        }
    }
    // Check common skill directories
    let home = bifrost_agent::config::user_home_dir();
    let agents_dir = home.join(".agents");
    if skill_path.starts_with(&agents_dir) {
        return true;
    }
    false
}

fn append_external_cli_metadata(
    adapter: &str,
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    if !is_codex_like_adapter(adapter) {
        return;
    }
    if !metadata.contains_key("threadId") {
        if let Some(thread_id) = events.iter().find_map(|event| {
            event
                .raw
                .get("thread_id")
                .or_else(|| event.raw.get("threadId"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        }) {
            metadata.insert("threadId".to_string(), thread_id.to_string());
        }
    }
    append_external_cli_usage_metadata(events, metadata);
}

fn append_external_cli_usage_metadata(
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    let Some(usage) = events.iter().rev().find_map(|event| {
        event
            .raw
            .get("usage")
            .and_then(serde_json::Value::as_object)
    }) else {
        return;
    };

    let input = usage_u64(usage, "input_tokens");
    let output = usage_u64(usage, "output_tokens");
    let cached = usage_u64(usage, "cached_input_tokens");
    let reasoning = usage_u64(usage, "reasoning_output_tokens");
    if let Some(value) = input {
        metadata
            .entry("usageInputTokens".to_string())
            .or_insert_with(|| value.to_string());
    }
    if let Some(value) = cached {
        metadata
            .entry("usageCachedInputTokens".to_string())
            .or_insert_with(|| value.to_string());
    }
    if let Some(value) = output {
        metadata
            .entry("usageOutputTokens".to_string())
            .or_insert_with(|| value.to_string());
    }
    if let Some(value) = reasoning {
        metadata
            .entry("usageReasoningOutputTokens".to_string())
            .or_insert_with(|| value.to_string());
    }
    if let Some(total) = usage_u64(usage, "total_tokens").or_else(|| {
        Some(input.unwrap_or(0).saturating_add(output.unwrap_or(0))).filter(|value| *value > 0)
    }) {
        metadata
            .entry("usageTotalTokens".to_string())
            .or_insert_with(|| total.to_string());
    }
}

fn usage_u64(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(serde_json::Value::as_u64)
}

pub fn resolve_external_cli_model_config(
    adapter: &str,
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    let mut resolved = if config.ignore_user_config.unwrap_or(false) {
        ExternalCliResolvedModelConfig::default()
    } else {
        load_external_cli_user_model_config(adapter, config)
    };
    apply_config_overrides_to_model_config(&mut resolved, &config.config_overrides);
    if let Some(model) = clean_optional_string(config.model.as_deref()) {
        resolved.model = Some(model);
        resolved.model_source = Some("runner config".to_string());
    }
    if let Some(effort) = clean_optional_string(config.reasoning_effort.as_deref()) {
        resolved.reasoning_effort = Some(effort);
        resolved.reasoning_source = Some("runner config".to_string());
    }
    if let Some(summary) = clean_optional_string(config.reasoning_summary.as_deref()) {
        resolved.reasoning_summary = Some(summary);
        resolved.reasoning_source = Some("runner config".to_string());
    }
    resolved
}

fn load_external_cli_user_model_config(
    adapter: &str,
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    let Some((config_path, profile_suffix, source)) = external_cli_user_config_path(adapter) else {
        return ExternalCliResolvedModelConfig::default();
    };
    let mut resolved = read_model_config_toml(&config_path, source);
    if let Some(profile) = clean_optional_string(config.profile_v2.as_deref())
        .or_else(|| clean_optional_string(config.profile.as_deref()))
    {
        let profile_path = config_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(format!("{profile}{profile_suffix}"));
        let profile_config = read_model_config_toml(&profile_path, source);
        merge_model_config(&mut resolved, profile_config);
    }
    resolved
}

fn external_cli_user_config_path(adapter: &str) -> Option<(PathBuf, &'static str, &'static str)> {
    match adapter.trim() {
        DEFAULT_ADAPTER => {
            let home = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".codex"));
            Some((home.join("config.toml"), ".config.toml", "codex config"))
        }
        TRAEX_ADAPTER => {
            let home = std::env::var_os("TRAE_HOME")
                .or_else(|| std::env::var_os("TRAEX_HOME"))
                .map(PathBuf::from)
                .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".trae"));
            Some((home.join("traecli.toml"), ".traecli.toml", "trae config"))
        }
        _ => None,
    }
}

fn read_model_config_toml(path: &Path, source: &str) -> ExternalCliResolvedModelConfig {
    let Ok(content) = std::fs::read_to_string(path) else {
        return ExternalCliResolvedModelConfig::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return ExternalCliResolvedModelConfig::default();
    };
    let model = toml_string(&value, "model");
    let model_provider = toml_string(&value, "model_provider");
    let reasoning_effort = toml_string(&value, "model_reasoning_effort")
        .or_else(|| toml_string(&value, "reasoning_effort"));
    let reasoning_summary = toml_string(&value, "model_reasoning_summary")
        .or_else(|| toml_string(&value, "reasoning_summary"));
    ExternalCliResolvedModelConfig {
        model,
        model_provider,
        reasoning_effort,
        reasoning_summary,
        model_source: Some(source.to_string()),
        reasoning_source: Some(source.to_string()),
    }
}

fn toml_string(value: &toml::Value, key: &str) -> Option<String> {
    clean_optional_string(value.get(key).and_then(toml::Value::as_str))
}

fn merge_model_config(
    base: &mut ExternalCliResolvedModelConfig,
    overlay: ExternalCliResolvedModelConfig,
) {
    if overlay.model.is_some() {
        base.model = overlay.model;
        base.model_source = overlay.model_source.clone();
    }
    if overlay.model_provider.is_some() {
        base.model_provider = overlay.model_provider;
    }
    if overlay.reasoning_effort.is_some() {
        base.reasoning_effort = overlay.reasoning_effort;
        base.reasoning_source = overlay.reasoning_source.clone();
    }
    if overlay.reasoning_summary.is_some() {
        base.reasoning_summary = overlay.reasoning_summary;
        base.reasoning_source = overlay.reasoning_source;
    }
}

fn apply_config_overrides_to_model_config(
    resolved: &mut ExternalCliResolvedModelConfig,
    overrides: &[String],
) {
    for value in overrides {
        let Some((key, raw_value)) = value.split_once('=') else {
            continue;
        };
        let Some(parsed) = parse_config_override_string(raw_value) else {
            continue;
        };
        match key.trim() {
            "model" => {
                resolved.model = Some(parsed);
                resolved.model_source = Some("runner config".to_string());
            }
            "model_provider" => resolved.model_provider = Some(parsed),
            "model_reasoning_effort" | "reasoning_effort" => {
                resolved.reasoning_effort = Some(parsed);
                resolved.reasoning_source = Some("runner config".to_string());
            }
            "model_reasoning_summary" | "reasoning_summary" => {
                resolved.reasoning_summary = Some(parsed);
                resolved.reasoning_source = Some("runner config".to_string());
            }
            _ => {}
        }
    }
}

fn parse_config_override_string(raw_value: &str) -> Option<String> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return None;
    }
    toml::from_str::<toml::Value>(&format!("value = {trimmed}"))
        .ok()
        .and_then(|value| {
            value
                .get("value")
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| clean_optional_string(Some(trimmed.trim_matches('"'))))
}

fn clean_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn append_external_cli_request_metadata(
    request: &ExternalCliRunRequest,
    metadata: &mut BTreeMap<String, String>,
) {
    let resolved = resolve_external_cli_model_config(&request.adapter, &request.adapter_config);
    if let Some(model) = resolved.model.as_deref() {
        metadata
            .entry("model".to_string())
            .or_insert_with(|| model.to_string());
        if let Some(provider) = resolved.model_provider.as_deref() {
            metadata
                .entry("modelProvider".to_string())
                .or_insert_with(|| provider.to_string());
        }
        metadata
            .entry("modelSource".to_string())
            .or_insert_with(|| {
                resolved
                    .model_source
                    .clone()
                    .unwrap_or_else(|| "runner config".to_string())
            });
        metadata
            .entry("modelLabel".to_string())
            .or_insert_with(|| model.to_string());
    } else if is_codex_like_adapter(&request.adapter) {
        let (label, source) = match request.adapter.trim() {
            TRAEX_ADAPTER => (
                "Trae default model (not explicitly configured)",
                "trae default",
            ),
            _ => (
                "Codex default model (not explicitly configured)",
                "codex default",
            ),
        };
        metadata
            .entry("modelLabel".to_string())
            .or_insert_with(|| label.to_string());
        metadata
            .entry("modelSource".to_string())
            .or_insert_with(|| source.to_string());
    }
    if let Some(effort) = resolved.reasoning_effort.as_deref() {
        metadata
            .entry("modelReasoningEffort".to_string())
            .or_insert_with(|| effort.to_string());
    }
    if let Some(summary) = resolved.reasoning_summary.as_deref() {
        metadata
            .entry("modelReasoningSummary".to_string())
            .or_insert_with(|| summary.to_string());
    }
}

fn command_snapshot(request: &ExternalCliRunRequest, spec: &CommandSpec) -> CommandSnapshot {
    CommandSnapshot {
        executable: spec.executable.clone(),
        args: spec.args.clone(),
        env_keys: spec.env.keys().cloned().collect(),
        work_dir: spec
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        runtime: request.runtime.clone(),
        adapter: request.adapter.clone(),
        params: request.params.clone(),
        timeout_secs: spec.timeout_secs,
    }
}

async fn detect_cli_version(adapter: &str, spec: &CommandSpec) -> Option<String> {
    if !is_codex_like_adapter(adapter) {
        return None;
    }
    let mut command = Command::new(&spec.executable);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(work_dir) = spec.work_dir.as_ref() {
        command.current_dir(work_dir);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    let output = timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(truncate_metadata_value(value, 200))
}

struct ExternalCliObservabilityInput<'a> {
    request: &'a ExternalCliRunRequest,
    spec: &'a CommandSpec,
    prompt: &'a str,
    saved_images: &'a [ExternalCliSavedImageAttachment],
    stdout: &'a [u8],
    stderr: &'a [u8],
    events: &'a [ExternalCliProgressEvent],
    timings: ExternalCliObservabilityTimings,
    cli_version: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct ExternalCliObservabilityTimings {
    started_at: u64,
    command_started_at: Option<u64>,
    command_finished_at: Option<u64>,
    finished_at: u64,
}

fn append_external_cli_observability_metadata(
    input: ExternalCliObservabilityInput<'_>,
    metadata: &mut BTreeMap<String, String>,
) {
    let request = input.request;
    let spec = input.spec;
    let prompt = input.prompt;
    let saved_images = input.saved_images;
    let stdout = input.stdout;
    let stderr = input.stderr;
    let events = input.events;
    let timings = input.timings;
    insert_metadata(metadata, "cli.executable", &spec.executable);
    insert_metadata_json(metadata, "cli.args", &spec.args);
    if let Some(work_dir) = spec.work_dir.as_ref() {
        insert_metadata(metadata, "cli.workDir", &work_dir.display().to_string());
    }
    if let Some(timeout_secs) = spec.timeout_secs {
        insert_metadata_u64(metadata, "cli.timeoutSecs", timeout_secs);
    }
    if let Some(version) = input.cli_version {
        insert_metadata(metadata, "cli.version", version);
    }
    insert_metadata(metadata, "runner.adapter", &request.adapter);
    if let Some(runner_id) = request.runner_id.as_deref() {
        insert_metadata(metadata, "runner.id", runner_id);
    }
    insert_metadata_bool(
        metadata,
        "runner.injectBifrostTools",
        request.inject_bifrost_tools,
    );
    insert_metadata_json(metadata, "config.addDirs", &request.adapter_config.add_dirs);
    insert_metadata_json(
        metadata,
        "config.enableFeatures",
        &request.adapter_config.enable_features,
    );
    insert_metadata_json(
        metadata,
        "config.disableFeatures",
        &request.adapter_config.disable_features,
    );
    if let Some(policy) = request.adapter_config.approval_policy.as_deref() {
        insert_metadata(metadata, "config.approvalPolicy", policy);
    }
    if let Some(sandbox) = request.adapter_config.sandbox.as_deref() {
        insert_metadata(metadata, "config.sandbox", sandbox);
    }
    if let Some(permission_mode) = request.adapter_config.permission_mode.as_deref() {
        insert_metadata(metadata, "config.permissionMode", permission_mode);
    }
    if let Some(danger_full_access) = request.adapter_config.danger_full_access {
        insert_metadata_bool(metadata, "config.dangerFullAccess", danger_full_access);
    }
    if let Some(search) = request.adapter_config.search {
        insert_metadata_bool(metadata, "config.search", search);
    }
    if let Some(ephemeral) = request.adapter_config.ephemeral {
        insert_metadata_bool(metadata, "config.ephemeral", ephemeral);
    }

    insert_metadata_u64(metadata, "prompt.bytes", prompt.len() as u64);
    insert_metadata_u64(metadata, "prompt.chars", prompt.chars().count() as u64);
    insert_metadata_u64(
        metadata,
        "prompt.estimatedTokens",
        estimate_tokens_from_chars(prompt.chars().count()),
    );
    insert_metadata_u64(
        metadata,
        "prompt.attachmentPathCount",
        saved_images.len() as u64,
    );
    insert_metadata_u64(metadata, "attachments.count", saved_images.len() as u64);
    insert_metadata_u64(
        metadata,
        "attachments.totalBytes",
        saved_images.iter().map(|image| image.size_bytes).sum(),
    );
    insert_metadata_json(
        metadata,
        "attachments.summary",
        &saved_images
            .iter()
            .map(|image| {
                serde_json::json!({
                    "mimeType": image.mime_type,
                    "name": image.name,
                    "sizeBytes": image.size_bytes,
                    "path": image.path,
                })
            })
            .collect::<Vec<_>>(),
    );

    insert_metadata_u64(metadata, "io.stdoutBytes", stdout.len() as u64);
    insert_metadata_u64(metadata, "io.stderrBytes", stderr.len() as u64);
    insert_metadata_u64(metadata, "io.stdoutLines", line_count(stdout));
    insert_metadata_u64(metadata, "io.stderrLines", line_count(stderr));
    insert_metadata_bool(metadata, "io.stdoutTruncated", false);
    insert_metadata_bool(metadata, "io.stderrTruncated", false);

    if let Some(command_started_at) = timings.command_started_at {
        insert_metadata_u64(
            metadata,
            "timing.commandStartLatencyMs",
            command_started_at.saturating_sub(timings.started_at),
        );
    }
    if let (Some(command_started_at), Some(command_finished_at)) =
        (timings.command_started_at, timings.command_finished_at)
    {
        insert_metadata_u64(
            metadata,
            "timing.commandDurationMs",
            command_finished_at.saturating_sub(command_started_at),
        );
    }
    if let Some(first_event_at) = events
        .iter()
        .filter_map(|event| raw_u64(&event.raw, "observedAtMs"))
        .min()
    {
        insert_metadata_u64(
            metadata,
            "timing.firstEventLatencyMs",
            first_event_at.saturating_sub(timings.started_at),
        );
    }
    insert_metadata_u64(
        metadata,
        "timing.totalDurationMs",
        timings.finished_at.saturating_sub(timings.started_at),
    );

    if let Some(thread_id) = request
        .params
        .get("threadId")
        .and_then(serde_json::Value::as_str)
    {
        insert_metadata_bool(metadata, "resume.requested", true);
        insert_metadata(metadata, "resume.requestedThreadId", thread_id);
    } else {
        insert_metadata_bool(metadata, "resume.requested", false);
    }

    append_tool_observability_metadata(events, metadata);
    append_plan_observability_metadata(events, metadata);
    append_message_observability_metadata(events, metadata);
}

fn append_tool_observability_metadata(
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    let calls = events
        .iter()
        .filter(|event| event.event_type == ExternalCliProgressEventType::ToolFinished)
        .map(|event| {
            let success = raw_bool(&event.raw, "success")
                .or_else(|| status_success(&event.raw))
                .unwrap_or_else(|| !event.content.to_ascii_lowercase().contains("failed"));
            let output_bytes = event.content.len() as u64;
            serde_json::json!({
                "id": raw_tool_id(&event.raw),
                "name": raw_tool_name(&event.raw).unwrap_or_else(|| event.title.clone().unwrap_or_else(|| "tool".to_string())),
                "command": raw_string_path(&event.raw, &["item", "command"]).or_else(|| raw_string_path(&event.raw, &["command"])),
                "exitCode": raw_i64_path(&event.raw, &["item", "exit_code"]).or_else(|| raw_i64_path(&event.raw, &["exitCode"])),
                "success": success,
                "durationMs": raw_u64(&event.raw, "durationMs"),
                "outputBytes": output_bytes,
            })
        })
        .collect::<Vec<_>>();
    let failed_count = calls
        .iter()
        .filter(|call| call.get("success").and_then(serde_json::Value::as_bool) == Some(false))
        .count() as u64;
    let total_duration = calls
        .iter()
        .filter_map(|call| call.get("durationMs").and_then(serde_json::Value::as_u64))
        .sum::<u64>();
    let total_output_bytes = calls
        .iter()
        .filter_map(|call| call.get("outputBytes").and_then(serde_json::Value::as_u64))
        .sum::<u64>();

    insert_metadata_u64(metadata, "tools.count", calls.len() as u64);
    insert_metadata_u64(metadata, "tools.failedCount", failed_count);
    insert_metadata_u64(metadata, "tools.totalDurationMs", total_duration);
    insert_metadata_u64(metadata, "tools.outputBytes", total_output_bytes);
    insert_metadata_json(metadata, "tools.calls", &calls);
}

fn append_plan_observability_metadata(
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    let plan_updates = events
        .iter()
        .filter(|event| event.event_type == ExternalCliProgressEventType::PlanUpdated)
        .count() as u64;
    let latest_plan = events
        .iter()
        .rev()
        .find(|event| event.event_type == ExternalCliProgressEventType::PlanUpdated)
        .and_then(|event| event.raw.get("plan").or_else(|| event.raw.get("todos")))
        .and_then(serde_json::Value::as_array);
    insert_metadata_u64(metadata, "plan.updates", plan_updates);
    if let Some(plan) = latest_plan {
        let total = plan.len() as u64;
        let completed = plan_status_count(plan, "completed");
        let in_progress = plan_status_count(plan, "in_progress");
        let pending = total.saturating_sub(completed).saturating_sub(in_progress);
        insert_metadata_u64(metadata, "plan.total", total);
        insert_metadata_u64(metadata, "plan.completed", completed);
        insert_metadata_u64(metadata, "plan.inProgress", in_progress);
        insert_metadata_u64(metadata, "plan.pending", pending);
        insert_metadata_u64(
            metadata,
            "plan.completionPercent",
            completed
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0),
        );
    }
}

fn append_message_observability_metadata(
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    let assistant_chars = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                ExternalCliProgressEventType::AssistantDelta
                    | ExternalCliProgressEventType::AssistantFinal
            )
        })
        .map(|event| event.content.chars().count() as u64)
        .sum();
    let reasoning_chars = events
        .iter()
        .filter(|event| {
            event
                .title
                .as_deref()
                .map(|title| title.to_ascii_lowercase().contains("reasoning"))
                .unwrap_or(false)
                || raw_string_path(&event.raw, &["type"])
                    .map(|event_type| event_type.contains("reasoning"))
                    .unwrap_or(false)
        })
        .map(|event| event.content.chars().count() as u64)
        .sum();
    insert_metadata_u64(metadata, "messages.assistantChars", assistant_chars);
    insert_metadata_u64(metadata, "messages.reasoningChars", reasoning_chars);
}

fn insert_metadata(metadata: &mut BTreeMap<String, String>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        metadata.insert(key.to_string(), truncate_metadata_value(value, 2000));
    }
}

fn insert_metadata_u64(metadata: &mut BTreeMap<String, String>, key: &str, value: u64) {
    metadata.insert(key.to_string(), value.to_string());
}

fn insert_metadata_bool(metadata: &mut BTreeMap<String, String>, key: &str, value: bool) {
    metadata.insert(key.to_string(), value.to_string());
}

fn insert_metadata_json<T: Serialize>(
    metadata: &mut BTreeMap<String, String>,
    key: &str,
    value: &T,
) {
    if let Ok(serialized) = serde_json::to_string(value) {
        metadata.insert(key.to_string(), truncate_metadata_value(&serialized, 4000));
    }
}

fn truncate_metadata_value(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for ch in value.chars().take(max_chars) {
        output.push(ch);
    }
    output
}

fn estimate_tokens_from_chars(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

fn line_count(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
}

fn raw_u64(raw: &serde_json::Value, key: &str) -> Option<u64> {
    raw.get(key).and_then(serde_json::Value::as_u64)
}

fn raw_bool(raw: &serde_json::Value, key: &str) -> Option<bool> {
    raw.get(key).and_then(serde_json::Value::as_bool)
}

fn raw_string_path(raw: &serde_json::Value, path: &[&str]) -> Option<String> {
    let mut current = raw;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(str::to_string)
}

fn raw_i64_path(raw: &serde_json::Value, path: &[&str]) -> Option<i64> {
    let mut current = raw;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_i64()
}

fn raw_tool_id(raw: &serde_json::Value) -> Option<String> {
    raw_string_path(raw, &["item", "id"])
        .or_else(|| raw_string_path(raw, &["item_id"]))
        .or_else(|| raw_string_path(raw, &["id"]))
}

fn raw_tool_name(raw: &serde_json::Value) -> Option<String> {
    raw_string_path(raw, &["item", "name"])
        .or_else(|| raw_string_path(raw, &["name"]))
        .or_else(|| raw_string_path(raw, &["item", "type"]))
        .or_else(|| raw_string_path(raw, &["tool"]))
}

fn status_success(raw: &serde_json::Value) -> Option<bool> {
    raw_string_path(raw, &["item", "status"])
        .or_else(|| raw_string_path(raw, &["status"]))
        .map(|status| matches!(status.as_str(), "completed" | "success" | "succeeded"))
}

fn plan_status_count(plan: &[serde_json::Value], status: &str) -> u64 {
    plan.iter()
        .filter(|step| {
            step.get("status")
                .and_then(serde_json::Value::as_str)
                .map(|value| value == status)
                .unwrap_or(false)
        })
        .count() as u64
}

async fn run_command(
    run_id: &str,
    session_key: Option<&str>,
    spec: CommandSpec,
    prompt: String,
    stop_marker_path: PathBuf,
    progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
) -> Result<CommandOutput, String> {
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if let Some(work_dir) = spec.work_dir.as_ref() {
        command.current_dir(work_dir);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn external cli failed: {error}"))?;
    let pid = child.id().unwrap_or(0);
    if pid != 0 {
        ACTIVE_RUNS.insert(run_id.to_string(), pid);
    }
    if let Some(session_key) = session_key {
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| format!("write prompt to external cli failed: {error}"))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "external cli stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "external cli stderr unavailable".to_string())?;
    let stdout_task = tokio::spawn(read_stdout_events(stdout, progress_tx));
    let stderr_task = tokio::spawn(read_stderr_lines(stderr));

    let wait_result = {
        let mut wait_future = std::pin::pin!(child.wait());
        if let Some(timeout_secs) = spec.timeout_secs {
            tokio::select! {
                result = &mut wait_future => CommandWaitOutcome::Exited(result),
                _ = sleep(Duration::from_secs(timeout_secs)) => CommandWaitOutcome::TimedOut,
                _ = wait_for_stop_marker(stop_marker_path.clone()) => CommandWaitOutcome::Stopped,
            }
        } else {
            tokio::select! {
                result = &mut wait_future => CommandWaitOutcome::Exited(result),
                _ = wait_for_stop_marker(stop_marker_path.clone()) => CommandWaitOutcome::Stopped,
            }
        }
    };

    match wait_result {
        CommandWaitOutcome::Exited(Ok(exit_status)) => {
            let (stdout, events) = stdout_task
                .await
                .map_err(|error| format!("join external cli stdout task failed: {error}"))??;
            let stderr = stderr_task
                .await
                .map_err(|error| format!("join external cli stderr task failed: {error}"))??;
            let status = if exit_status.success() {
                ExternalCliRunStatus::Succeeded
            } else {
                ExternalCliRunStatus::Failed
            };
            ACTIVE_RUNS.remove(run_id);
            remove_active_sessions_for_run(run_id);
            Ok(CommandOutput {
                status,
                exit_code: exit_status.code(),
                stdout,
                stderr,
                events,
            })
        }
        CommandWaitOutcome::Exited(Err(error)) => {
            ACTIVE_RUNS.remove(run_id);
            remove_active_sessions_for_run(run_id);
            Err(format!("wait external cli failed: {error}"))
        }
        CommandWaitOutcome::TimedOut => {
            collect_interrupted_command_output(
                run_id,
                pid,
                child,
                stdout_task,
                stderr_task,
                ExternalCliRunStatus::TimedOut,
                format!(
                    "external cli timed out after {} seconds\n",
                    spec.timeout_secs.unwrap_or_default()
                ),
            )
            .await
        }
        CommandWaitOutcome::Stopped => {
            collect_interrupted_command_output(
                run_id,
                pid,
                child,
                stdout_task,
                stderr_task,
                ExternalCliRunStatus::Stopped,
                "external cli stopped by request\n".to_string(),
            )
            .await
        }
    }
}

async fn collect_interrupted_command_output(
    run_id: &str,
    pid: u32,
    mut child: tokio::process::Child,
    stdout_task: ExternalCliStdoutTask,
    stderr_task: ExternalCliStderrTask,
    status: ExternalCliRunStatus,
    terminal_message: String,
) -> Result<CommandOutput, String> {
    let stopped = matches!(status, ExternalCliRunStatus::Stopped);
    if pid != 0 {
        if let Err(error) = terminate_process(pid) {
            tracing::warn!(
                pid,
                stopped,
                error = %error,
                "failed to terminate external cli process group"
            );
        }
    }
    let _ = child.kill().await;
    let (stdout, events) = stdout_task
        .await
        .map_err(|error| format!("join external cli stdout task failed: {error}"))??;
    let mut stderr = stderr_task
        .await
        .map_err(|error| format!("join external cli stderr task failed: {error}"))??;
    stderr.extend_from_slice(terminal_message.as_bytes());
    ACTIVE_RUNS.remove(run_id);
    remove_active_sessions_for_run(run_id);
    Ok(CommandOutput {
        status,
        exit_code: None,
        stdout,
        stderr,
        events,
    })
}

enum CommandWaitOutcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Stopped,
}

type ExternalCliStdoutTask =
    tokio::task::JoinHandle<Result<(Vec<u8>, Vec<ExternalCliProgressEvent>), String>>;
type ExternalCliStderrTask = tokio::task::JoinHandle<Result<Vec<u8>, String>>;

async fn wait_for_stop_marker(path: PathBuf) {
    loop {
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn read_stdout_events(
    stdout: tokio::process::ChildStdout,
    progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
) -> Result<(Vec<u8>, Vec<ExternalCliProgressEvent>), String> {
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut bytes = Vec::new();
    let mut events = Vec::new();
    let mut tool_started_at: HashMap<String, u64> = HashMap::new();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("read external cli stdout failed: {error}"))?
    {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if let Some(mut event) = parse_progress_event_line(&line) {
            let observed_at = now_ms();
            enrich_progress_event_observation(&mut event, observed_at, &mut tool_started_at);
            if let Some(progress_tx) = progress_tx.as_ref() {
                let _ = progress_tx.send(event.clone());
            }
            events.push(event);
        }
    }
    Ok((bytes, events))
}

fn enrich_progress_event_observation(
    event: &mut ExternalCliProgressEvent,
    observed_at: u64,
    tool_started_at: &mut HashMap<String, u64>,
) {
    if let Some(object) = event.raw.as_object_mut() {
        object
            .entry("observedAtMs".to_string())
            .or_insert_with(|| serde_json::json!(observed_at));
    }
    let Some(item_id) = event
        .raw
        .get("item")
        .and_then(|item| item.get("id"))
        .or_else(|| event.raw.get("item_id"))
        .or_else(|| event.raw.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    match event.event_type {
        ExternalCliProgressEventType::ToolStarted => {
            tool_started_at.insert(item_id, observed_at);
        }
        ExternalCliProgressEventType::ToolFinished => {
            if let Some(started_at) = tool_started_at.remove(&item_id) {
                let duration_ms = observed_at.saturating_sub(started_at);
                if let Some(object) = event.raw.as_object_mut() {
                    object
                        .entry("durationMs".to_string())
                        .or_insert_with(|| serde_json::json!(duration_ms));
                }
            }
        }
        _ => {}
    }
}

async fn read_stderr_lines(stderr: tokio::process::ChildStderr) -> Result<Vec<u8>, String> {
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    let mut bytes = Vec::new();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("read external cli stderr failed: {error}"))?
    {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn remove_active_sessions_for_run(run_id: &str) {
    let session_keys: Vec<String> = ACTIVE_SESSIONS
        .iter()
        .filter_map(|entry| {
            if entry.value() == run_id {
                Some(entry.key().clone())
            } else {
                None
            }
        })
        .collect();
    for session_key in session_keys {
        ACTIVE_SESSIONS.remove(&session_key);
    }
}

fn spawn_external_cli_worker_process(executable: &Path) -> Result<tokio::process::Child, String> {
    let mut command = Command::new(executable);
    command
        .arg("agent")
        .arg("external-runner-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("BIFROST_EXTERNAL_CLI_WORKER", "1")
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
        .spawn()
        .map_err(|error| format!("spawn external runner worker failed: {error}"))
}

async fn write_external_cli_worker_command(
    stdin: &mut tokio::process::ChildStdin,
    command: &ExternalCliWorkerCommand,
) -> Result<(), String> {
    let line = serde_json::to_string(command)
        .map_err(|error| format!("serialize external runner worker command failed: {error}"))?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("write external runner worker command failed: {error}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| format!("write external runner worker command newline failed: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush external runner worker command failed: {error}"))
}

fn send_external_cli_worker_event(event: &ExternalCliWorkerEvent) -> Result<(), String> {
    let line = serde_json::to_string(event)
        .map_err(|error| format!("serialize external runner worker event failed: {error}"))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .map_err(|error| format!("write external runner worker event failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("write external runner worker event newline failed: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush external runner worker event failed: {error}"))
}

/// 终止所有正在运行的 external CLI 子进程。在 Bifrost 进程退出时调用，
/// 防止使用 `process_group(0)` 启动的子进程组变成孤儿进程。
pub fn kill_all_active_runs() {
    let entries: Vec<(String, u32)> = ACTIVE_RUNS
        .iter()
        .map(|entry| (entry.key().clone(), *entry.value()))
        .collect();
    for (run_id, pid) in entries {
        tracing::info!(run_id, pid, "external_cli: killing active run on shutdown");
        if let Err(error) = terminate_process(pid) {
            tracing::warn!(run_id, pid, %error, "external_cli: failed to terminate on shutdown");
        }
        ACTIVE_RUNS.remove(&run_id);
    }
    ACTIVE_SESSIONS.clear();
}

fn terminate_process(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("refusing to terminate pid 0".to_string());
    }
    terminate_process_impl(pid)
}

#[cfg(unix)]
fn terminate_process_impl(pid: u32) -> Result<(), String> {
    use nix::errno::Errno;

    if pid > i32::MAX as u32 {
        return Err(format!("pid {pid} is too large to terminate"));
    }

    match signal_process_group_or_child(pid, nix::sys::signal::Signal::SIGTERM) {
        Ok(ProcessSignalOutcome::Signaled) => {
            schedule_process_group_force_kill(pid);
            Ok(())
        }
        Ok(ProcessSignalOutcome::NotFound) | Err(Errno::ESRCH) => {
            // Entire group is already gone — nothing to do, and do not schedule
            // a delayed SIGKILL that could hit a reused pid/process group.
            Ok(())
        }
        Err(error) => Err(format!("failed to terminate process group {pid}: {error}")),
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessSignalOutcome {
    Signaled,
    NotFound,
}

#[cfg(unix)]
fn signal_process_group_or_child(
    pid: u32,
    signal: nix::sys::signal::Signal,
) -> Result<ProcessSignalOutcome, nix::errno::Errno> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    // We always spawn with process_group(0), so the child PID is the group leader.
    // Signal the process group first so shell-spawned children are covered.
    let group_pid = Pid::from_raw(-(pid as i32));
    match kill(group_pid, signal) {
        Ok(()) => Ok(ProcessSignalOutcome::Signaled),
        Err(Errno::ESRCH) => Ok(ProcessSignalOutcome::NotFound),
        Err(Errno::EPERM) => {
            let child_pid = Pid::from_raw(pid as i32);
            match kill(child_pid, signal) {
                Ok(()) => Ok(ProcessSignalOutcome::Signaled),
                Err(Errno::ESRCH) => Ok(ProcessSignalOutcome::NotFound),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn schedule_process_group_force_kill(pid: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(PROCESS_KILL_GRACE_MS));
        match signal_process_group_or_child(pid, nix::sys::signal::Signal::SIGKILL) {
            Ok(ProcessSignalOutcome::Signaled | ProcessSignalOutcome::NotFound) => {}
            Err(error) => {
                tracing::warn!(
                    pid,
                    %error,
                    "failed to force-kill process group after SIGTERM grace"
                );
            }
        }
    });
}

#[cfg(windows)]
fn terminate_process_impl(pid: u32) -> Result<(), String> {
    let output = StdCommand::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .arg("/F")
        .output()
        .map_err(|error| format!("failed to invoke taskkill for pid {pid}: {error}"))?;
    if output.status.success()
        || taskkill_message_indicates_missing_process(&output.stdout, &output.stderr)
        || !windows_process_exists(pid)
    {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "taskkill /PID {pid} /T /F exited with status {}; stdout: {}; stderr: {}",
            output.status,
            stdout.trim(),
            stderr.trim()
        ))
    }
}

#[cfg(any(windows, test))]
fn taskkill_message_indicates_missing_process(stdout: &[u8], stderr: &[u8]) -> bool {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let message = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    message.contains("not found")
        || message.contains("no running instance of the task")
        || message.contains("cannot find the process")
}

#[cfg(windows)]
fn windows_process_exists(pid: u32) -> bool {
    let Ok(output) = StdCommand::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .arg("/FO")
        .arg("CSV")
        .arg("/NH")
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.contains(&format!("\"{pid}\""))
}

pub fn parse_progress_events(stdout: &str) -> Vec<ExternalCliProgressEvent> {
    let mut events = Vec::new();
    for line in stdout.lines() {
        if let Some(event) = parse_progress_event_line(line) {
            events.push(event);
        }
    }
    events
}

fn parse_progress_event_line(line: &str) -> Option<ExternalCliProgressEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    parse_progress_event(raw)
}

fn parse_progress_event(raw: serde_json::Value) -> Option<ExternalCliProgressEvent> {
    let event_type = value_text(&raw, &["type", "event", "kind"])?;
    if let Some(event) = parse_codex_cli_event(&event_type, &raw) {
        return Some(event);
    }
    if matches!(event_type.as_str(), "plan_updated" | "todo_list") {
        let steps = todo_list_steps_from_raw(&raw);
        if !steps.is_empty() {
            return Some(ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::PlanUpdated,
                content: format!("plan updated ({} steps)", steps.len()),
                title: value_text(&raw, &["title", "name"]),
                raw,
            });
        }
    }
    let content = value_text(
        &raw,
        &[
            "content", "delta", "message", "text", "summary", "final", "status",
        ],
    )
    .unwrap_or_default();
    let title = value_text(&raw, &["title", "name", "tool_name"]);
    let normalized_type = match event_type.as_str() {
        "run_started" | "started" => ExternalCliProgressEventType::RunStarted,
        "status" | "status_changed" => ExternalCliProgressEventType::Status,
        "assistant_delta" | "agent_message_delta" | "message_delta" => {
            ExternalCliProgressEventType::AssistantDelta
        }
        "assistant_final" | "agent_message" | "message" => {
            ExternalCliProgressEventType::AssistantFinal
        }
        "tool_started" | "tool_call_started" => ExternalCliProgressEventType::ToolStarted,
        "tool_finished" | "tool_call_finished" => ExternalCliProgressEventType::ToolFinished,
        "run_finished" | "finished" | "done" => ExternalCliProgressEventType::RunFinished,
        "run_failed" | "failed" | "error" => ExternalCliProgressEventType::RunFailed,
        other if other.contains("delta") => ExternalCliProgressEventType::AssistantDelta,
        other if other.contains("final") => ExternalCliProgressEventType::AssistantFinal,
        _ => return None,
    };
    Some(ExternalCliProgressEvent {
        event_type: normalized_type,
        content,
        title,
        raw,
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExternalCliProgressStatusContext<'a> {
    runner_id: Option<&'a str>,
    model: Option<&'a str>,
    model_source: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
    reasoning_summary: Option<&'a str>,
    work_dir: Option<&'a Path>,
}

impl<'a> ExternalCliProgressStatusContext<'a> {
    pub fn new(
        runner_id: Option<&'a str>,
        model: Option<&'a str>,
        model_source: Option<&'a str>,
        reasoning_effort: Option<&'a str>,
        reasoning_summary: Option<&'a str>,
        work_dir: Option<&'a Path>,
    ) -> Self {
        Self {
            runner_id,
            model,
            model_source,
            reasoning_effort,
            reasoning_summary,
            work_dir,
        }
    }
}

pub fn external_progress_to_agent_turn_event(
    session_key: &str,
    adapter: &str,
    context: ExternalCliProgressStatusContext<'_>,
    event: &ExternalCliProgressEvent,
) -> Option<bifrost_agent::AgentTurnProgressEvent> {
    match event.event_type {
        ExternalCliProgressEventType::RunStarted | ExternalCliProgressEventType::Status => {
            let mut status = bifrost_agent::ActiveTurnStatus::new(session_key);
            status.state = if event.content.trim().is_empty() {
                "running".to_string()
            } else {
                event.content.trim().to_string()
            };
            status.runner_type = Some(adapter.to_string());
            status.runner_id = context.runner_id.map(str::to_string);
            status.model = context.model.map(str::to_string);
            status.model_provider = context.model_source.map(str::to_string);
            status.model_reasoning_effort = context.reasoning_effort.map(str::to_string);
            status.model_reasoning_summary = context.reasoning_summary.map(str::to_string);
            status.work_dir = context.work_dir.map(|path| path.display().to_string());
            Some(bifrost_agent::AgentTurnProgressEvent::Status(Box::new(
                status,
            )))
        }
        ExternalCliProgressEventType::AssistantDelta => {
            Some(bifrost_agent::AgentTurnProgressEvent::AssistantDelta {
                content: event.content.clone(),
            })
        }
        ExternalCliProgressEventType::AssistantFinal => {
            Some(bifrost_agent::AgentTurnProgressEvent::AssistantFinal {
                content: event.content.clone(),
            })
        }
        ExternalCliProgressEventType::PlanUpdated => {
            let steps = external_progress_plan_steps(event);
            if steps.is_empty() {
                None
            } else {
                Some(bifrost_agent::AgentTurnProgressEvent::PlanUpdated {
                    steps,
                    title: event.title.clone(),
                })
            }
        }
        ExternalCliProgressEventType::ToolStarted => {
            Some(bifrost_agent::AgentTurnProgressEvent::ToolStarted {
                tool_name: event_title_or_default(event, "runner"),
                arguments: event.content.clone(),
            })
        }
        ExternalCliProgressEventType::ToolFinished => {
            Some(bifrost_agent::AgentTurnProgressEvent::ToolFinished {
                log: bifrost_agent::ToolCallLog {
                    tool_name: event_title_or_default(event, "runner"),
                    arguments: external_progress_arguments_text(event),
                    result: event.content.clone(),
                    success: event
                        .raw
                        .get("success")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true),
                },
                duration_ms: event
                    .raw
                    .get("durationMs")
                    .or_else(|| event.raw.get("duration_ms"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            })
        }
        ExternalCliProgressEventType::RunFinished => {
            Some(bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                content: event.content.clone(),
            })
        }
        ExternalCliProgressEventType::RunFailed => {
            Some(bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                error: event.content.clone(),
            })
        }
    }
}

fn external_progress_arguments_text(event: &ExternalCliProgressEvent) -> String {
    event
        .raw
        .get("arguments")
        .and_then(|value| {
            value
                .get("command")
                .and_then(serde_json::Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| event.raw.get("args").and_then(serde_json::Value::as_str))
        .or_else(|| {
            event
                .raw
                .get("item")
                .and_then(|item| item.get("command"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            event
                .raw
                .get("item")
                .and_then(|item| item.get("arguments"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_default()
}

fn event_title_or_default(event: &ExternalCliProgressEvent, default: &str) -> String {
    event
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            event
                .raw
                .get("tool_name")
                .or_else(|| event.raw.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(default)
        .to_string()
}

fn is_codex_like_adapter(adapter: &str) -> bool {
    matches!(adapter, DEFAULT_ADAPTER | TRAEX_ADAPTER)
}

fn parse_codex_cli_event(
    event_type: &str,
    raw: &serde_json::Value,
) -> Option<ExternalCliProgressEvent> {
    match event_type {
        "thread.started" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::RunStarted,
            content: value_text(raw, &["thread_id"])
                .unwrap_or_else(|| "thread started".to_string()),
            title: Some("Codex thread".to_string()),
            raw: raw.clone(),
        }),
        "turn.started" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::Status,
            content: "turn started".to_string(),
            title: Some("Codex turn".to_string()),
            raw: raw.clone(),
        }),
        "turn.completed" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::RunFinished,
            content: "turn completed".to_string(),
            title: Some("Codex turn".to_string()),
            raw: raw.clone(),
        }),
        "item.started" => {
            let item_type = value_text_path(raw, &["item", "type"])?;
            if item_type == "todo_list" {
                return codex_todo_list_event(raw);
            }
            match item_type.as_str() {
                "command_execution" => Some(codex_command_execution_event(
                    raw,
                    ExternalCliProgressEventType::ToolStarted,
                )),
                "tool_call" => Some(ExternalCliProgressEvent {
                    event_type: ExternalCliProgressEventType::ToolStarted,
                    content: value_text_path(raw, &["item", "arguments"])
                        .or_else(|| value_text_path(raw, &["item", "command"]))
                        .unwrap_or_default(),
                    title: value_text_path(raw, &["item", "name"]).or(Some(item_type)),
                    raw: raw.clone(),
                }),
                _ => Some(ExternalCliProgressEvent {
                    event_type: ExternalCliProgressEventType::Status,
                    content: value_text_path(raw, &["item", "status"])
                        .unwrap_or_else(|| format!("{item_type} started")),
                    title: Some(item_type),
                    raw: raw.clone(),
                }),
            }
        }
        "item.updated" => {
            let item_type = value_text_path(raw, &["item", "type"])?;
            if item_type == "todo_list" {
                codex_todo_list_event(raw)
            } else {
                None
            }
        }
        "item.completed" => {
            let item_type = value_text_path(raw, &["item", "type"])?;
            if item_type == "todo_list" {
                return codex_todo_list_event(raw);
            }
            if item_type == "command_execution" {
                return Some(codex_command_execution_event(
                    raw,
                    ExternalCliProgressEventType::ToolFinished,
                ));
            }
            let content = value_text_path(raw, &["item", "text"])
                .or_else(|| value_text_path(raw, &["item", "message"]))
                .or_else(|| value_text_path(raw, &["item", "summary"]))
                .or_else(|| value_text_path(raw, &["item", "content"]))
                .or_else(|| value_text_path(raw, &["item", "title"]))
                .unwrap_or_default();
            let normalized_type = match item_type.as_str() {
                "agent_message" => ExternalCliProgressEventType::AssistantFinal,
                "reasoning" | "reasoning_summary" => ExternalCliProgressEventType::AssistantDelta,
                // Codex CLI currently emits non-fatal config warnings as
                // `item.completed` with `item.type=error`. The process exit
                // status remains the source of truth for run failure.
                "error" => ExternalCliProgressEventType::Status,
                "tool_call" | "tool_result" => ExternalCliProgressEventType::ToolFinished,
                _ => ExternalCliProgressEventType::Status,
            };
            Some(ExternalCliProgressEvent {
                event_type: normalized_type,
                content,
                title: Some(item_type),
                raw: raw.clone(),
            })
        }
        _ => None,
    }
}

fn codex_todo_list_event(raw: &serde_json::Value) -> Option<ExternalCliProgressEvent> {
    let steps = todo_list_steps_from_raw(raw);
    if steps.is_empty() {
        return None;
    }
    Some(ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::PlanUpdated,
        content: format!("plan updated ({} steps)", steps.len()),
        title: None,
        raw: raw.clone(),
    })
}

pub fn external_progress_plan_steps(event: &ExternalCliProgressEvent) -> Vec<PlanStep> {
    todo_list_steps_from_raw(&event.raw)
}

fn todo_list_steps_from_raw(raw: &serde_json::Value) -> Vec<PlanStep> {
    raw.get("item")
        .and_then(|item| item.get("items"))
        .or_else(|| raw.get("items"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(todo_list_item_to_plan_step)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn todo_list_item_to_plan_step(item: &serde_json::Value) -> Option<PlanStep> {
    let step = item
        .get("text")
        .or_else(|| item.get("step"))
        .or_else(|| item.get("content"))
        .or_else(|| item.get("title"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let status = if item
        .get("completed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        PlanStepStatus::Completed
    } else {
        item.get("status")
            .and_then(serde_json::Value::as_str)
            .and_then(|status| status.parse::<PlanStepStatus>().ok())
            .unwrap_or(PlanStepStatus::Pending)
    };
    Some(PlanStep { step, status })
}

fn codex_command_execution_event(
    raw: &serde_json::Value,
    event_type: ExternalCliProgressEventType,
) -> ExternalCliProgressEvent {
    let command = value_text_path(raw, &["item", "command"]).unwrap_or_default();
    let output = value_text_path(raw, &["item", "aggregated_output"]).unwrap_or_default();
    let exit_code = raw
        .get("item")
        .and_then(|item| item.get("exit_code"))
        .and_then(serde_json::Value::as_i64);
    let mut enriched_raw = raw.clone();
    if let Some(object) = enriched_raw.as_object_mut() {
        object
            .entry("tool_name".to_string())
            .or_insert_with(|| serde_json::json!("exec_command"));
        object
            .entry("arguments".to_string())
            .or_insert_with(|| serde_json::json!({ "command": command }));
        if let Some(exit_code) = exit_code {
            object
                .entry("success".to_string())
                .or_insert_with(|| serde_json::json!(exit_code == 0));
        }
    }
    ExternalCliProgressEvent {
        content: match &event_type {
            ExternalCliProgressEventType::ToolStarted => command,
            ExternalCliProgressEventType::ToolFinished => output,
            _ => String::new(),
        },
        event_type,
        title: Some("exec_command".to_string()),
        raw: enriched_raw,
    }
}

fn value_text(raw: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = raw.get(*key) {
            if let Some(text) = value.as_str() {
                return Some(text.to_string());
            }
            if value.is_number() || value.is_boolean() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn value_text_path(raw: &serde_json::Value, path: &[&str]) -> Option<String> {
    let mut value = raw;
    for key in path {
        value = value.get(*key)?;
    }
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if value.is_number() || value.is_boolean() {
        return Some(value.to_string());
    }
    None
}

async fn final_response(
    last_message_path: &Path,
    stdout_text: &str,
    events: &[ExternalCliProgressEvent],
) -> Result<String, String> {
    let last_message = read_text_or_default(last_message_path).await?;
    let trimmed_last_message = last_message.trim();
    if !trimmed_last_message.is_empty() {
        return Ok(trimmed_last_message.to_string());
    }
    if let Some(event) = events.iter().rev().find(|event| {
        event.event_type == ExternalCliProgressEventType::AssistantFinal
            && !event.content.trim().is_empty()
    }) {
        return Ok(event.content.trim().to_string());
    }
    if let Some(event) = events.iter().rev().find(|event| {
        event.event_type == ExternalCliProgressEventType::RunFinished
            && !event.content.trim().is_empty()
    }) {
        return Ok(event.content.trim().to_string());
    }
    Ok(stdout_text.trim().to_string())
}

fn visible_terminal_response(
    status: ExternalCliRunStatus,
    response: String,
    stdout_text: &str,
    stderr_text: &str,
    events: &[ExternalCliProgressEvent],
) -> String {
    if status == ExternalCliRunStatus::Succeeded || !response.trim().is_empty() {
        return response;
    }
    if let Some(event) = events.iter().rev().find(|event| {
        event.event_type == ExternalCliProgressEventType::RunFailed
            && !event.content.trim().is_empty()
    }) {
        return event.content.trim().to_string();
    }
    let stderr = stderr_text.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    let stdout = stdout_text.trim();
    if !stdout.is_empty() {
        return stdout.to_string();
    }
    match status {
        ExternalCliRunStatus::Failed => "External CLI run failed.".to_string(),
        ExternalCliRunStatus::Stopped => "External CLI run was stopped by request.".to_string(),
        ExternalCliRunStatus::TimedOut => "External CLI run timed out.".to_string(),
        ExternalCliRunStatus::Succeeded => response,
    }
}

async fn read_text_or_default(path: &Path) -> Result<String, String> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("read {} failed: {error}", path.display())),
    }
}

async fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("serialize {} failed: {error}", path.display()))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| format!("write {} failed: {error}", path.display()))
}

async fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("read {} failed: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("parse {} failed: {error}", path.display()))
}

async fn write_events_jsonl(
    path: &Path,
    events: &[ExternalCliProgressEvent],
) -> Result<(), String> {
    let mut content = String::new();
    for event in events {
        let line = serde_json::to_string(event)
            .map_err(|error| format!("serialize progress event failed: {error}"))?;
        content.push_str(&line);
        content.push('\n');
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|error| format!("write {} failed: {error}", path.display()))
}

async fn read_events_jsonl(path: &Path) -> Result<Vec<ExternalCliProgressEvent>, String> {
    let content = read_text_or_default(path).await?;
    let mut events = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        events.push(
            serde_json::from_str(trimmed)
                .map_err(|error| format!("parse {} failed: {error}", path.display()))?,
        );
    }
    Ok(events)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn normalized_gateway_config(mut config: ExternalCliGatewayConfig) -> ExternalCliGatewayConfig {
    config.version = CONFIG_VERSION;
    config.default_runner_id = config.default_runner_id.trim().to_string();
    if config.default_runner_id.is_empty() {
        config.default_runner_id = default_runner_id();
    }
    if config.runners.is_empty() {
        config.runners = default_external_cli_runners();
    } else {
        config.runners = config
            .runners
            .into_iter()
            .filter_map(|(runner_id, settings)| {
                let runner_id = runner_id.trim().to_string();
                (!runner_id.is_empty()).then_some((runner_id, normalized_agent_settings(settings)))
            })
            .collect();
    }
    ensure_default_external_cli_runners(&mut config.runners);
    if !config.runners.contains_key(&config.default_runner_id) {
        let canonical_default_runner_id = canonical_runner_id(&config, &config.default_runner_id);
        if config.runners.contains_key(&canonical_default_runner_id) {
            config.default_runner_id = canonical_default_runner_id;
        } else {
            config
                .runners
                .entry(config.default_runner_id.clone())
                .or_default();
        }
    }
    config.channels = config
        .channels
        .into_iter()
        .filter_map(|(provider_id, channel)| {
            let channel = normalized_channel_settings(channel);
            (!is_empty_channel_settings(&channel)).then_some((provider_id, channel))
        })
        .collect();
    config
}

fn default_external_cli_runners() -> BTreeMap<String, ExternalCliAgentSettings> {
    let mut runners = BTreeMap::new();
    ensure_default_external_cli_runners(&mut runners);
    runners
}

fn ensure_default_external_cli_runners(runners: &mut BTreeMap<String, ExternalCliAgentSettings>) {
    runners
        .entry(DEFAULT_CODEX_RUNNER_ID.to_string())
        .or_insert_with(|| default_runner_settings(DEFAULT_ADAPTER));
    runners
        .entry(DEFAULT_TREEX_RUNNER_ID.to_string())
        .or_insert_with(|| default_runner_settings(TRAEX_ADAPTER));
}

fn default_runner_settings(adapter: &str) -> ExternalCliAgentSettings {
    ExternalCliAgentSettings {
        enabled: true,
        adapter: adapter.to_string(),
        ..ExternalCliAgentSettings::default()
    }
}

fn canonical_runner_id(config: &ExternalCliGatewayConfig, runner_id: &str) -> String {
    let runner_id = runner_id.trim();
    if config.runners.contains_key(runner_id) {
        return runner_id.to_string();
    }
    match runner_id.to_ascii_lowercase().as_str() {
        "codex" if config.runners.contains_key(DEFAULT_CODEX_RUNNER_ID) => {
            DEFAULT_CODEX_RUNNER_ID.to_string()
        }
        "treex" | "traex" | "trae" if config.runners.contains_key(DEFAULT_TREEX_RUNNER_ID) => {
            DEFAULT_TREEX_RUNNER_ID.to_string()
        }
        _ => runner_id.to_string(),
    }
}

fn normalized_agent_settings(mut settings: ExternalCliAgentSettings) -> ExternalCliAgentSettings {
    if settings.adapter.trim().is_empty() {
        settings.adapter = DEFAULT_ADAPTER.to_string();
    } else {
        settings.adapter = settings.adapter.trim().to_string();
    }
    settings.instructions = settings
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    settings.skill_paths = normalize_string_list(settings.skill_paths);
    settings
}

fn normalized_channel_settings(
    mut settings: ExternalCliChannelSettings,
) -> ExternalCliChannelSettings {
    settings.runner_id = settings
        .runner_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    settings
}

fn is_empty_channel_settings(settings: &ExternalCliChannelSettings) -> bool {
    settings.enabled.is_none() && settings.runner_id.is_none() && settings.delivery_mode.is_none()
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn load_config_from_disk(path: &Path) -> Option<ExternalCliGatewayConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<ExternalCliGatewayConfig>(&content).ok()
}

fn save_config_to_disk(path: &Path, config: &ExternalCliGatewayConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("mkdir {} failed: {error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("serialize external cli config failed: {error}"))?;
    std::fs::write(path, content)
        .map_err(|error| format!("write {} failed: {error}", path.display()))
}

fn default_runtime() -> String {
    DEFAULT_RUNTIME.to_string()
}

fn default_adapter() -> String {
    DEFAULT_ADAPTER.to_string()
}

fn default_operation() -> String {
    "ask".to_string()
}

fn default_runner_id() -> String {
    DEFAULT_CODEX_RUNNER_ID.to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests;
