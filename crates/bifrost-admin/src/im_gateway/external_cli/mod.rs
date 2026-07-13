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
pub const DEFAULT_TRAEX_RUNNER_ID: &str = "Traex";
pub const CLAUDE_CODE_ADAPTER: &str = "claude_code";
pub const DEFAULT_CLAUDE_CODE_RUNNER_ID: &str = "Claude-Code";
const LEGACY_CLAUDE_CODE_RUNNER_ID: &str = "Claude Code";
const LEGACY_TRAEX_RUNNER_ALIAS: &str = concat!("tre", "ex");
const CONFIG_FILENAME: &str = "im_gateway_external_cli_agent.json";
const CONFIG_VERSION: u32 = 1;
const MAX_EXTERNAL_RUNNER_IMAGES_PER_MESSAGE: usize = 6;
const MAX_PENDING_EXTERNAL_GUIDES: usize = 32;
const WORKER_STOP_GRACE_MS: u64 = 1500;
#[cfg(unix)]
const PROCESS_KILL_GRACE_MS: u64 = 250;

static ACTIVE_RUNS: once_cell::sync::Lazy<dashmap::DashMap<String, u32>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static ACTIVE_SESSIONS: once_cell::sync::Lazy<dashmap::DashMap<String, String>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static ACTIVE_WORKER_SESSIONS: once_cell::sync::Lazy<
    dashmap::DashMap<String, ExternalCliWorkerControlHandle>,
> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Clone)]
struct ExternalCliWorkerControlHandle {
    pid: u32,
    control_tx: tokio::sync::mpsc::Sender<ExternalCliWorkerControlRequest>,
}

enum ExternalCliWorkerControlRequest {
    Stop {
        ack_tx: oneshot::Sender<()>,
    },
    Guide {
        guide_id: String,
        message: String,
        ack_tx: oneshot::Sender<ExternalCliGuideResult>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliGuideResult {
    pub guide_id: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
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
    match handle
        .control_tx
        .try_send(ExternalCliWorkerControlRequest::Stop { ack_tx })
    {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                session_key,
                pid = handle.pid,
                "external_cli worker: control channel is saturated; terminating worker directly"
            );
            let _ = terminate_process(handle.pid);
            return true;
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            tracing::warn!(
                session_key,
                pid = handle.pid,
                "external_cli worker: stop receiver is gone; skipping pid termination for stale worker entry"
            );
            return false;
        }
    }
    match tokio::time::timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), &mut ack_rx).await {
        Ok(Ok(())) | Ok(Err(_)) => return true,
        Err(_) => {}
    }
    let _ = terminate_process(handle.pid);
    true
}

pub async fn request_worker_session_guide(
    session_key: &str,
    guide_id: String,
    message: String,
) -> Result<ExternalCliGuideResult, String> {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return Err("session_key cannot be empty".to_string());
    }
    if message.trim().is_empty() {
        return Err("guide message cannot be empty".to_string());
    }
    let handle = ACTIVE_WORKER_SESSIONS
        .get(session_key)
        .map(|entry| entry.clone())
        .ok_or_else(|| format!("no active external runner for session '{session_key}'"))?;
    let (ack_tx, ack_rx) = oneshot::channel();
    handle
        .control_tx
        .try_send(ExternalCliWorkerControlRequest::Guide {
            guide_id,
            message,
            ack_tx,
        })
        .map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => format!(
                "external runner has too many pending guide requests for session '{session_key}'"
            ),
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                format!("external runner control channel closed for session '{session_key}'")
            }
        })?;
    timeout(Duration::from_secs(20), ack_rx)
        .await
        .map_err(|_| format!("external runner guide timed out for session '{session_key}'"))?
        .map_err(|_| format!("external runner guide response closed for session '{session_key}'"))
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
    let (command_tx, mut command_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExternalCliWorkerCommand>();
    std::thread::spawn(move || {
        while let Some(Ok(line)) = stdin.next() {
            if let Ok(command) = serde_json::from_str::<ExternalCliWorkerCommand>(&line) {
                let should_stop = matches!(command, ExternalCliWorkerCommand::Stop);
                if command_tx.send(command).is_err() || should_stop {
                    break;
                }
            }
        }
    });
    send_external_cli_worker_event(&ExternalCliWorkerEvent::Started {
        session_key: request.request.session_key.clone(),
        pid: std::process::id(),
    })?;
    let request = *request;
    let session_key = request.request.session_key.clone().unwrap_or_default();
    let supports_live_guide = matches!(
        app_server::resolved_transport(&request.request),
        Ok(ExternalCliTransport::AppServer)
    );
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
    tokio::pin!(run);
    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(ExternalCliWorkerCommand::Stop) | None => {
                        kill_all_active_runs();
                        run.abort();
                        send_external_cli_worker_event(&ExternalCliWorkerEvent::Stopped)?;
                        break;
                    }
                    Some(ExternalCliWorkerCommand::Guide { guide_id, message }) => {
                        let result = if supports_live_guide {
                            app_server::request_session_guide(
                                &session_key,
                                guide_id,
                                message,
                            )
                            .await
                        } else {
                            ExternalCliGuideResult {
                                guide_id,
                                accepted: false,
                                thread_id: None,
                                turn_id: None,
                                reason: Some(
                                    "active runner uses exec transport and cannot steer the current turn"
                                        .to_string(),
                                ),
                            }
                        };
                        send_external_cli_worker_event(&ExternalCliWorkerEvent::GuideResult {
                            result,
                        })?;
                    }
                    Some(ExternalCliWorkerCommand::Run { .. }) => {}
                }
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
                break;
            }
        }
    }
    Ok(())
}

mod app_server;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<ExternalCliTransport>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCliTransport {
    Exec,
    AppServer,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalCliModelSlashCommand {
    List,
    Show,
    Clear,
    Set(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalCliEffortSlashCommand {
    List,
    Show,
    Clear,
    Set(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliModelInfo {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_reasoning_levels: Vec<ExternalCliReasoningLevelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_in_api: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_speed_tiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_tiers: Vec<ExternalCliServiceTierInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_load: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliReasoningLevelInfo {
    pub effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliServiceTierInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    runner_id = canonical_external_cli_runner_id(config, &runner_id);
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
    Guide {
        guide_id: String,
        message: String,
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
    GuideResult {
        result: ExternalCliGuideResult,
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

    async fn request_guide(&mut self, guide_id: String, message: String) -> Result<(), String> {
        write_external_cli_worker_command(
            &mut self.stdin,
            &ExternalCliWorkerCommand::Guide { guide_id, message },
        )
        .await
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

    async fn next_event(&mut self) -> Result<ExternalCliWorkerEvent, String> {
        let Some(line) = self
            .events
            .next_line()
            .await
            .map_err(|error| format!("read external runner worker event failed: {error}"))?
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
        serde_json::from_str::<ExternalCliWorkerEvent>(&line).map_err(|error| {
            format!("parse external runner worker event failed: {error}; line={line}")
        })
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
        let worker_pid = worker.child_id();
        let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<
            ExternalCliWorkerControlRequest,
        >(MAX_PENDING_EXTERNAL_GUIDES);
        if let (Some(session_key), Some(pid)) = (request.session_key.as_deref(), worker_pid) {
            ACTIVE_WORKER_SESSIONS.insert(
                session_key.to_string(),
                ExternalCliWorkerControlHandle { pid, control_tx },
            );
        }
        let session_key = request.session_key.clone();
        let mut pending_guides = HashMap::<String, oneshot::Sender<ExternalCliGuideResult>>::new();
        let mut control_open = true;
        let result = loop {
            tokio::select! {
                control = control_rx.recv(), if control_open => {
                    match control {
                        Some(ExternalCliWorkerControlRequest::Stop { ack_tx }) => {
                            let _ = worker.terminate().await;
                            let _ = ack_tx.send(());
                            break Ok(ExternalCliRunResult::stopped(
                                request.session_key.clone(),
                                request.adapter.clone(),
                            ));
                        }
                        Some(ExternalCliWorkerControlRequest::Guide {
                            guide_id,
                            message,
                            ack_tx,
                        }) => {
                            if pending_guides.len() >= MAX_PENDING_EXTERNAL_GUIDES {
                                let _ = ack_tx.send(ExternalCliGuideResult {
                                    guide_id,
                                    accepted: false,
                                    thread_id: None,
                                    turn_id: None,
                                    reason: Some(format!(
                                        "too many pending guide requests (limit {MAX_PENDING_EXTERNAL_GUIDES})"
                                    )),
                                });
                                continue;
                            }
                            if pending_guides.contains_key(&guide_id) {
                                let _ = ack_tx.send(ExternalCliGuideResult {
                                    guide_id,
                                    accepted: false,
                                    thread_id: None,
                                    turn_id: None,
                                    reason: Some("duplicate guide id is already pending".to_string()),
                                });
                                continue;
                            }
                            if let Err(error) = worker.request_guide(guide_id.clone(), message).await {
                                let _ = ack_tx.send(ExternalCliGuideResult {
                                    guide_id,
                                    accepted: false,
                                    thread_id: None,
                                    turn_id: None,
                                    reason: Some(error),
                                });
                            } else {
                                pending_guides.insert(guide_id, ack_tx);
                            }
                        }
                        None => control_open = false,
                    }
                }
                event = worker.next_event() => {
                    match event {
                        Err(error) => break Err(error),
                        Ok(ExternalCliWorkerEvent::Started { .. }) => {}
                        Ok(ExternalCliWorkerEvent::Progress { event }) => {
                            if let Some(progress_tx) = progress_tx.as_ref() {
                                let _ = progress_tx.send(event);
                            }
                        }
                        Ok(ExternalCliWorkerEvent::GuideResult { result }) => {
                            if let Some(ack_tx) = pending_guides.remove(&result.guide_id) {
                                let _ = ack_tx.send(result);
                            }
                        }
                        Ok(ExternalCliWorkerEvent::Finished { result }) => break Ok(*result),
                        Ok(ExternalCliWorkerEvent::Failed { error }) => break Err(error),
                        Ok(ExternalCliWorkerEvent::Stopped) => {
                            break Ok(ExternalCliRunResult::stopped(
                                request.session_key.clone(),
                                request.adapter.clone(),
                            ));
                        }
                    }
                }
            }
        };
        for (guide_id, ack_tx) in pending_guides {
            let _ = ack_tx.send(ExternalCliGuideResult {
                guide_id,
                accepted: false,
                thread_id: None,
                turn_id: None,
                reason: Some("external runner finished before guide acknowledgement".to_string()),
            });
        }
        if let Some(session_key) = session_key.as_deref() {
            if let Some(pid) = worker_pid {
                ACTIVE_WORKER_SESSIONS.remove_if(session_key, |_, handle| handle.pid == pid);
            }
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

        let external_cli_transport =
            if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
                None
            } else {
                Some(app_server::resolved_transport(&request)?)
            };
        let spec = if external_cli_transport == Some(ExternalCliTransport::AppServer) {
            app_server::build_command_spec(&request)
        } else {
            build_command_spec(&request, &last_message_path)?
        };
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
            let command_output = match external_cli_transport
                .expect("non-chatgpt external CLI transport must be resolved")
            {
                ExternalCliTransport::AppServer => {
                    app_server::run_command(
                        &run_id,
                        session_key_for_stop.as_deref(),
                        &request,
                        prompt.clone(),
                        stop_marker_path.clone(),
                        progress_tx,
                    )
                    .await?
                }
                ExternalCliTransport::Exec => {
                    run_command(
                        &run_id,
                        session_key_for_stop.as_deref(),
                        spec.clone(),
                        prompt.clone(),
                        stop_marker_path.clone(),
                        progress_tx,
                    )
                    .await?
                }
            };
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
                .or_else(|| event.raw.get("session_id"))
                .or_else(|| event.raw.get("sessionId"))
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
            .or_else(|| {
                event
                    .raw
                    .get("message")
                    .and_then(|message| message.get("usage"))
            })
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

pub fn resolve_external_cli_status_model_config(
    adapter: &str,
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    let mut resolved = resolve_external_cli_model_config(adapter, config);
    if adapter.trim() == CLAUDE_CODE_ADAPTER && resolved.model.is_none() {
        let mut model_config = load_claude_code_status_model_config(config);
        apply_config_overrides_to_model_config(&mut model_config, &config.config_overrides);
        if let Some(model) = clean_optional_string(config.model.as_deref()) {
            model_config.model = Some(model);
            model_config.model_source = Some("runner config".to_string());
        }
        if model_config.model.is_some() {
            resolved.model = model_config.model;
            resolved.model_provider = model_config.model_provider;
            resolved.model_source = model_config.model_source;
        }
    }
    resolved
}

pub fn apply_external_cli_session_overrides_to_model_config(
    adapter: &str,
    state: Option<&crate::im_gateway::session_state::ImAgentSessionState>,
    config: &mut ExternalCliResolvedModelConfig,
) {
    let Some(state) = state else {
        return;
    };
    if supports_external_cli_model_slash(adapter) {
        if let Some(model) = clean_optional_string(state.model_override.as_deref()) {
            config.model = Some(model);
            config.model_source = Some(
                state
                    .model_override_source
                    .clone()
                    .unwrap_or_else(|| "session slash command".to_string()),
            );
        }
    }
    if let Some(effort) = clean_optional_string(state.reasoning_effort_override.as_deref()) {
        config.reasoning_effort = Some(effort);
        config.reasoning_source = Some(
            state
                .reasoning_effort_override_source
                .clone()
                .unwrap_or_else(|| "session slash command".to_string()),
        );
    }
}

pub fn apply_external_cli_session_overrides_to_run_request(
    request: &mut ExternalCliRunRequest,
    state: Option<&crate::im_gateway::session_state::ImAgentSessionState>,
) {
    let Some(state) = state else {
        return;
    };
    if supports_external_cli_model_slash(&request.adapter) {
        if let Some(model) = clean_optional_string(state.model_override.as_deref()) {
            request.adapter_config.model = Some(model);
        }
    }
    if let Some(effort) = clean_optional_string(state.reasoning_effort_override.as_deref()) {
        request.adapter_config.reasoning_effort = Some(effort);
    }
}

pub fn parse_external_cli_model_slash_command(
    message: &str,
) -> Option<Result<ExternalCliModelSlashCommand, String>> {
    let trimmed = message.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let command = parts.next()?;
    let rest = parts.next().unwrap_or("").trim();
    if command.eq_ignore_ascii_case("/models") {
        if rest.is_empty() {
            return Some(Ok(ExternalCliModelSlashCommand::List));
        }
        return Some(Err("用法: /models".to_string()));
    }
    if !command.eq_ignore_ascii_case("/model") {
        return None;
    }
    if rest.is_empty() {
        return Some(Ok(ExternalCliModelSlashCommand::Show));
    }
    if matches!(
        rest.to_ascii_lowercase().as_str(),
        "clear" | "reset" | "default"
    ) {
        return Some(Ok(ExternalCliModelSlashCommand::Clear));
    }
    if let Err(reason) = validate_external_model_slug(rest) {
        return Some(Err(reason));
    }
    Some(Ok(ExternalCliModelSlashCommand::Set(rest.to_string())))
}

pub fn parse_external_cli_effort_slash_command(
    message: &str,
) -> Option<Result<ExternalCliEffortSlashCommand, String>> {
    let trimmed = message.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let command = parts.next()?;
    let rest = parts.next().unwrap_or("").trim();
    if command.eq_ignore_ascii_case("/efforts") {
        if rest.is_empty() {
            return Some(Ok(ExternalCliEffortSlashCommand::List));
        }
        return Some(Err("用法: /efforts".to_string()));
    }
    if !command.eq_ignore_ascii_case("/effort") {
        return None;
    }
    if rest.is_empty() {
        return Some(Ok(ExternalCliEffortSlashCommand::Show));
    }
    if matches!(
        rest.to_ascii_lowercase().as_str(),
        "clear" | "reset" | "default" | "auto"
    ) {
        return Some(Ok(ExternalCliEffortSlashCommand::Clear));
    }
    if let Err(reason) = validate_external_effort_value(rest) {
        return Some(Err(reason));
    }
    Some(Ok(ExternalCliEffortSlashCommand::Set(
        rest.to_ascii_lowercase(),
    )))
}

pub async fn load_external_cli_model_catalog(
    adapter: &str,
    config: &ExternalCliAdapterConfig,
    work_dir: Option<&Path>,
) -> Result<Vec<ExternalCliModelInfo>, String> {
    let adapter = adapter.trim();
    if adapter == CLAUDE_CODE_ADAPTER {
        return Ok(default_claude_code_model_catalog());
    }
    let default_executable = match adapter {
        DEFAULT_ADAPTER => DEFAULT_ADAPTER,
        TRAEX_ADAPTER => TRAEX_ADAPTER,
        _ => {
            return Err(format!(
                "adapter `{adapter}` does not support model catalog"
            ))
        }
    };
    let executable = config
        .executable
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_executable);
    let mut command = Command::new(executable);
    command
        .arg("debug")
        .arg("models")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(work_dir) = work_dir {
        command.current_dir(work_dir);
    }
    for (key, value) in &config.env {
        command.env(key, value);
    }
    let output = timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| format!("{adapter} model catalog command timed out"))?
        .map_err(|error| format!("spawn {adapter} model catalog command failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let reason = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("debug models failed");
        return Err(format!(
            "{adapter} model catalog command failed: {}",
            truncate_metadata_value(reason, 500)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("{adapter} model catalog was not utf-8: {error}"))?;
    parse_external_cli_model_catalog(adapter, &stdout)
}

pub fn parse_external_cli_model_catalog(
    adapter: &str,
    raw_json: &str,
) -> Result<Vec<ExternalCliModelInfo>, String> {
    #[derive(Deserialize)]
    struct Catalog {
        #[serde(default)]
        models: Vec<RawModel>,
    }

    #[derive(Deserialize)]
    struct RawModel {
        slug: Option<String>,
        #[serde(default, alias = "displayName", alias = "name", alias = "title")]
        display_name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        default_reasoning_level: Option<String>,
        #[serde(default)]
        supported_reasoning_levels: Vec<ExternalCliReasoningLevelInfo>,
        #[serde(default)]
        visibility: Option<String>,
        #[serde(default)]
        supported_in_api: Option<bool>,
        #[serde(default)]
        additional_speed_tiers: Vec<String>,
        #[serde(default)]
        service_tiers: Vec<ExternalCliServiceTierInfo>,
        #[serde(default)]
        model_load: Option<serde_json::Value>,
        #[serde(default, alias = "modelLoad")]
        model_load_camel: Option<serde_json::Value>,
        #[serde(default, alias = "loadPercent")]
        load_percent: Option<serde_json::Value>,
        #[serde(default)]
        load: Option<serde_json::Value>,
        #[serde(default)]
        priority: Option<i64>,
    }

    let catalog: Catalog = serde_json::from_str(raw_json)
        .map_err(|error| format!("parse {adapter} model catalog failed: {error}"))?;
    let mut models = catalog
        .models
        .into_iter()
        .filter_map(|model| {
            let slug = clean_optional_string(model.slug.as_deref())?;
            if matches!(
                model.visibility.as_deref().map(str::trim),
                Some(value) if !value.eq_ignore_ascii_case("list")
            ) {
                return None;
            }
            Some(ExternalCliModelInfo {
                slug,
                display_name: clean_optional_string(model.display_name.as_deref()),
                description: clean_optional_string(model.description.as_deref()),
                default_reasoning_level: clean_optional_string(
                    model.default_reasoning_level.as_deref(),
                ),
                supported_reasoning_levels: model
                    .supported_reasoning_levels
                    .into_iter()
                    .filter(|level| !level.effort.trim().is_empty())
                    .collect(),
                visibility: clean_optional_string(model.visibility.as_deref()),
                supported_in_api: model.supported_in_api,
                additional_speed_tiers: model
                    .additional_speed_tiers
                    .into_iter()
                    .map(|tier| tier.trim().to_string())
                    .filter(|tier| !tier.is_empty())
                    .collect(),
                service_tiers: model
                    .service_tiers
                    .into_iter()
                    .filter(|tier| !tier.id.trim().is_empty())
                    .collect(),
                model_load: format_model_load(
                    model
                        .model_load
                        .as_ref()
                        .or(model.model_load_camel.as_ref())
                        .or(model.load_percent.as_ref())
                        .or(model.load.as_ref()),
                ),
                priority: model.priority,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| {
        left.priority
            .unwrap_or(i64::MAX)
            .cmp(&right.priority.unwrap_or(i64::MAX))
            .then_with(|| left.slug.cmp(&right.slug))
    });
    Ok(models)
}

pub fn format_external_cli_model_catalog(adapter: &str, models: &[ExternalCliModelInfo]) -> String {
    let label = external_cli_model_adapter_label(adapter);
    if models.is_empty() {
        return format!("{label} 当前没有返回可展示的模型。");
    }
    let mut lines = vec![format!("{label} 可用模型:")];
    for model in models.iter().take(40) {
        let mut extras = Vec::new();
        if let Some(reasoning) = model.default_reasoning_level.as_deref() {
            extras.push(format!("reasoning: {reasoning}"));
        }
        if !model.additional_speed_tiers.is_empty() {
            extras.push(format!("tiers: {}", model.additional_speed_tiers.join(",")));
        }
        if let Some(load) = model.model_load.as_deref() {
            extras.push(format!("Model load: {load}"));
        }
        if let Some(visibility) = model.visibility.as_deref() {
            extras.push(format!("visibility: {visibility}"));
        }
        let label = model.display_name.as_deref().unwrap_or(&model.slug);
        let suffix = if extras.is_empty() {
            String::new()
        } else {
            format!(" ({})", extras.join("; "))
        };
        lines.push(format!("- `{}` - {}{}", model.slug, label, suffix));
        if let Some(description) = model.description.as_deref() {
            lines.push(format!("  {}", truncate_metadata_value(description, 140)));
        }
    }
    if models.len() > 40 {
        lines.push(format!("... 另有 {} 个模型未展示", models.len() - 40));
    }
    lines.join("\n")
}

pub fn validate_external_cli_model_selection(
    adapter: &str,
    requested_model: &str,
    models: &[ExternalCliModelInfo],
) -> Result<String, String> {
    let requested_model = requested_model.trim();
    let label = external_cli_model_adapter_label(adapter);
    if adapter.trim() == CLAUDE_CODE_ADAPTER {
        validate_external_model_slug(requested_model)?;
        return Ok(models
            .iter()
            .find(|model| model.slug.eq_ignore_ascii_case(requested_model))
            .map(|model| model.slug.clone())
            .unwrap_or_else(|| requested_model.to_string()));
    }
    if let Some(model) = models.iter().find(|model| model.slug == requested_model) {
        return Ok(model.slug.clone());
    }
    if models.is_empty() {
        return Err(format!(
            "未切换模型：{label} 当前没有返回可展示的模型，不能设置为 `{requested_model}`。"
        ));
    }
    let mut available = models
        .iter()
        .take(8)
        .map(|model| format!("`{}`", model.slug))
        .collect::<Vec<_>>()
        .join(", ");
    if models.len() > 8 {
        available.push_str(&format!(" 等 {} 个", models.len()));
    }
    Err(format!(
        "未切换模型：`{requested_model}` 不在 {label} 可用模型列表中。\n可用模型包括: {available}\n请发送 `/models` 查看完整列表。"
    ))
}

pub fn format_external_cli_model_status(
    adapter: &str,
    effective_model: Option<&str>,
    source: Option<&str>,
    runner_id: &str,
) -> String {
    let label = external_cli_model_adapter_label(adapter);
    match effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(model) => format!(
            "当前 {label} Runner `{}` 使用模型: `{}`\n来源: {}",
            runner_id,
            model,
            source.unwrap_or("配置")
        ),
        None => format!(
            "当前 {label} Runner `{runner_id}` 未设置模型 override，将使用 {label} 默认模型。"
        ),
    }
}

pub fn external_cli_effort_options(adapter: &str) -> &'static [&'static str] {
    match adapter.trim() {
        CLAUDE_CODE_ADAPTER => &["low", "medium", "high", "xhigh", "max"],
        DEFAULT_ADAPTER | TRAEX_ADAPTER => &["minimal", "low", "medium", "high", "xhigh"],
        _ => &[],
    }
}

pub fn format_external_cli_effort_catalog(adapter: &str) -> String {
    let label = external_cli_model_adapter_label(adapter);
    let options = external_cli_effort_options(adapter);
    if options.is_empty() {
        return format!("{label} 当前没有可展示的 reasoning effort 选项。");
    }
    format!(
        "{label} 可用 Reasoning Effort:\n{}",
        options
            .iter()
            .map(|option| format!("- `{option}`"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub fn format_external_cli_effort_catalog_for_model(
    adapter: &str,
    effective_model: Option<&str>,
    models: &[ExternalCliModelInfo],
) -> String {
    let label = external_cli_model_adapter_label(adapter);
    let (levels, source) = external_cli_effort_levels_for_model(adapter, effective_model, models);
    if levels.is_empty() {
        return format!("{label} 当前没有可展示的 reasoning effort 选项。");
    }
    let model_suffix = effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|model| format!("（当前模型 `{model}`）"))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "{label} 可用 Reasoning Effort{model_suffix}:\n{}",
        levels
            .iter()
            .map(|level| match level.description.as_deref() {
                Some(description) if !description.trim().is_empty() => {
                    format!("- `{}` - {}", level.effort, description.trim())
                }
                _ => format!("- `{}`", level.effort),
            })
            .collect::<Vec<_>>()
            .join("\n")
    )];
    match source {
        ExternalCliEffortOptionSource::ModelCatalog => {
            lines.push("来源: 当前模型目录。".to_string());
        }
        ExternalCliEffortOptionSource::RunnerFallback => {
            lines
                .push("来源: 未读取到当前模型的推理强度列表，使用 Runner 兼容默认值。".to_string());
        }
    }
    lines.join("\n")
}

pub fn validate_external_cli_effort_selection(
    adapter: &str,
    requested_effort: &str,
) -> Result<String, String> {
    let effort = requested_effort.trim().to_ascii_lowercase();
    validate_external_effort_value(&effort)?;
    let options = external_cli_effort_options(adapter);
    if options.iter().any(|option| *option == effort) {
        return Ok(effort);
    }
    let label = external_cli_model_adapter_label(adapter);
    let available = options
        .iter()
        .map(|option| format!("`{option}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "未切换 Reasoning Effort：`{effort}` 不在 {label} 支持列表中。\n可用选项包括: {available}"
    ))
}

pub fn validate_external_cli_effort_selection_for_model(
    adapter: &str,
    requested_effort: &str,
    effective_model: Option<&str>,
    models: &[ExternalCliModelInfo],
) -> Result<String, String> {
    let effort = requested_effort.trim().to_ascii_lowercase();
    validate_external_effort_value(&effort)?;
    let (levels, _) = external_cli_effort_levels_for_model(adapter, effective_model, models);
    if levels
        .iter()
        .any(|level| level.effort.eq_ignore_ascii_case(&effort))
    {
        return Ok(effort);
    }
    let label = external_cli_model_adapter_label(adapter);
    let available = levels
        .iter()
        .map(|level| format!("`{}`", level.effort))
        .collect::<Vec<_>>()
        .join(", ");
    let model_hint = effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|model| format!(" 当前模型 `{model}`"))
        .unwrap_or_default();
    Err(format!(
        "未切换 Reasoning Effort：`{effort}` 不在 {label}{model_hint} 支持列表中。\n可用选项包括: {available}"
    ))
}

pub fn format_external_cli_effort_status(
    adapter: &str,
    effective_effort: Option<&str>,
    source: Option<&str>,
    runner_id: &str,
) -> String {
    let label = external_cli_model_adapter_label(adapter);
    match effective_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(effort) => format!(
            "当前 {label} Runner `{}` 使用 Reasoning Effort: `{}`\n来源: {}",
            runner_id,
            effort,
            source.unwrap_or("配置")
        ),
        None => format!(
            "当前 {label} Runner `{runner_id}` 未设置 Reasoning Effort override，将使用 {label} 默认值。"
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalCliEffortOptionSource {
    ModelCatalog,
    RunnerFallback,
}

fn external_cli_effort_levels_for_model(
    adapter: &str,
    effective_model: Option<&str>,
    models: &[ExternalCliModelInfo],
) -> (
    Vec<ExternalCliReasoningLevelInfo>,
    ExternalCliEffortOptionSource,
) {
    let mut levels = Vec::new();
    if let Some(model) = effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|model| {
            models
                .iter()
                .find(|candidate| candidate.slug.eq_ignore_ascii_case(model))
        })
    {
        if !model.supported_reasoning_levels.is_empty() {
            for level in &model.supported_reasoning_levels {
                push_unique_effort_level(&mut levels, level);
            }
            return (levels, ExternalCliEffortOptionSource::ModelCatalog);
        }
        if let Some(default) = clean_optional_string(model.default_reasoning_level.as_deref()) {
            push_unique_effort_level(
                &mut levels,
                &ExternalCliReasoningLevelInfo {
                    effort: default,
                    description: Some(
                        "Default reasoning level reported by the model catalog.".to_string(),
                    ),
                },
            );
            return (levels, ExternalCliEffortOptionSource::ModelCatalog);
        }
    }
    for option in external_cli_effort_options(adapter) {
        push_unique_effort_level(
            &mut levels,
            &ExternalCliReasoningLevelInfo {
                effort: (*option).to_string(),
                description: None,
            },
        );
    }
    (levels, ExternalCliEffortOptionSource::RunnerFallback)
}

fn push_unique_effort_level(
    levels: &mut Vec<ExternalCliReasoningLevelInfo>,
    level: &ExternalCliReasoningLevelInfo,
) {
    let Some(effort) =
        clean_optional_string(Some(level.effort.as_str())).map(|value| value.to_ascii_lowercase())
    else {
        return;
    };
    if !levels
        .iter()
        .any(|existing| existing.effort.eq_ignore_ascii_case(&effort))
    {
        levels.push(ExternalCliReasoningLevelInfo {
            effort,
            description: clean_optional_string(level.description.as_deref()),
        });
    }
}

pub fn external_cli_default_model_label(adapter: &str) -> Option<(&'static str, &'static str)> {
    match adapter.trim() {
        DEFAULT_ADAPTER => Some((
            "Codex default model (not explicitly configured)",
            "codex default",
        )),
        TRAEX_ADAPTER => Some((
            "Trae default model (not explicitly configured)",
            "trae default",
        )),
        CLAUDE_CODE_ADAPTER => Some((
            "Claude Code default model (not explicitly configured)",
            "claude code default",
        )),
        _ => None,
    }
}

pub fn external_cli_model_adapter_label(adapter: &str) -> &'static str {
    match adapter.trim() {
        DEFAULT_ADAPTER => "Codex",
        TRAEX_ADAPTER => "Traex",
        CLAUDE_CODE_ADAPTER => "Claude Code",
        _ => "External CLI",
    }
}

pub fn supports_external_cli_model_slash(adapter: &str) -> bool {
    matches!(
        adapter.trim(),
        DEFAULT_ADAPTER | TRAEX_ADAPTER | CLAUDE_CODE_ADAPTER
    )
}

fn default_claude_code_model_catalog() -> Vec<ExternalCliModelInfo> {
    const MODELS: &[(&str, &str, &str, i64)] = &[
        (
            "sonnet",
            "Sonnet",
            "Sonnet 4.6 - Efficient for routine tasks.",
            0,
        ),
        (
            "opus",
            "Opus",
            "Opus 4.8 - Best for everyday, complex tasks; roughly 2x usage vs Sonnet.",
            1,
        ),
        ("haiku", "Haiku", "Haiku 4.5 - Fastest for quick answers.", 2),
        (
            "fable",
            "Fable",
            "Claude Fable 5 is currently unavailable. Claude Code also accepts full model names via --model.",
            3,
        ),
    ];
    MODELS
        .iter()
        .map(
            |(slug, display_name, description, priority)| ExternalCliModelInfo {
                slug: (*slug).to_string(),
                display_name: Some((*display_name).to_string()),
                description: Some((*description).to_string()),
                default_reasoning_level: Some(
                    match *slug {
                        "opus" => "high",
                        _ => "medium",
                    }
                    .to_string(),
                ),
                supported_reasoning_levels: external_cli_effort_options(CLAUDE_CODE_ADAPTER)
                    .iter()
                    .map(|effort| ExternalCliReasoningLevelInfo {
                        effort: (*effort).to_string(),
                        description: None,
                    })
                    .collect(),
                visibility: Some("list".to_string()),
                priority: Some(*priority),
                ..Default::default()
            },
        )
        .collect()
}

fn validate_external_model_slug(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("用法: /model <model-slug>".to_string());
    }
    if value.len() > 128 {
        return Err("模型名称过长，请使用 128 个字符以内的模型 slug。".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | ':' | '@'))
    {
        return Err("模型名称只能包含字母、数字、点、下划线、短横线、斜杠、冒号或 @。".to_string());
    }
    Ok(())
}

fn validate_external_effort_value(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("用法: /effort <level>".to_string());
    }
    if value.len() > 32 {
        return Err("Reasoning Effort 过长，请使用 32 个字符以内的 level。".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err("Reasoning Effort 只能包含字母、数字、下划线或短横线。".to_string());
    }
    Ok(())
}

fn load_external_cli_user_model_config(
    adapter: &str,
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    if adapter.trim() == CLAUDE_CODE_ADAPTER {
        return load_claude_code_model_config(config);
    }
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

fn load_claude_code_model_config(
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    let mut resolved = ExternalCliResolvedModelConfig::default();
    let settings_path = config
        .extra
        .get("settings")
        .or_else(|| config.extra.get("settingsPath"))
        .or_else(|| config.extra.get("settings_path"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| clean_optional_string(Some(value)))
        .map(PathBuf::from);
    if let Some(path) = settings_path.as_deref() {
        merge_model_config(&mut resolved, read_claude_code_effort_config_json(path));
    } else {
        let home = std::env::var_os("CLAUDE_CONFIG_DIR")
            .or_else(|| std::env::var_os("CLAUDE_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".claude"));
        merge_model_config(
            &mut resolved,
            read_claude_code_effort_config_json(&home.join("settings.json")),
        );
        merge_model_config(
            &mut resolved,
            read_claude_code_effort_config_json(&home.join("settings.local.json")),
        );
    }
    apply_claude_code_env_effort(&mut resolved, "claude env", |key| std::env::var(key));
    apply_claude_code_env_effort(&mut resolved, "runner config", |key| {
        config
            .env
            .get(key)
            .cloned()
            .ok_or(std::env::VarError::NotPresent)
    });
    resolved
}

fn load_claude_code_status_model_config(
    config: &ExternalCliAdapterConfig,
) -> ExternalCliResolvedModelConfig {
    let settings_path = config
        .extra
        .get("settings")
        .or_else(|| config.extra.get("settingsPath"))
        .or_else(|| config.extra.get("settings_path"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| clean_optional_string(Some(value)))
        .map(PathBuf::from);
    let mut resolved = settings_path
        .as_deref()
        .map(read_claude_code_model_config_json)
        .unwrap_or_else(|| {
            let home = std::env::var_os("CLAUDE_CONFIG_DIR")
                .or_else(|| std::env::var_os("CLAUDE_HOME"))
                .map(PathBuf::from)
                .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".claude"));
            let mut resolved = read_claude_code_model_config_json(&home.join("settings.json"));
            merge_model_config(
                &mut resolved,
                read_claude_code_model_config_json(&home.join("settings.local.json")),
            );
            resolved
        });
    apply_claude_code_env_model_aliases(&mut resolved, "claude env", |key| std::env::var(key));
    apply_claude_code_env_model_aliases(&mut resolved, "runner config", |key| {
        config
            .env
            .get(key)
            .cloned()
            .ok_or(std::env::VarError::NotPresent)
    });
    resolved
}

fn read_claude_code_effort_config_json(path: &Path) -> ExternalCliResolvedModelConfig {
    let Ok(content) = std::fs::read_to_string(path) else {
        return ExternalCliResolvedModelConfig::default();
    };
    parse_claude_code_effort_config_json(&content, "claude settings")
}

fn parse_claude_code_effort_config_json(
    content: &str,
    source: &str,
) -> ExternalCliResolvedModelConfig {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return ExternalCliResolvedModelConfig::default();
    };
    let mut resolved = ExternalCliResolvedModelConfig {
        reasoning_effort: json_string(&value, "effortLevel")
            .or_else(|| json_string(&value, "effort_level")),
        reasoning_source: Some(source.to_string()),
        ..Default::default()
    };
    if let Some(env) = value.get("env").and_then(serde_json::Value::as_object) {
        apply_claude_code_env_effort(&mut resolved, source, |key| {
            env.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or(std::env::VarError::NotPresent)
        });
    }
    resolved
}

fn read_claude_code_model_config_json(path: &Path) -> ExternalCliResolvedModelConfig {
    let Ok(content) = std::fs::read_to_string(path) else {
        return ExternalCliResolvedModelConfig::default();
    };
    parse_claude_code_model_config_json(&content, "claude settings")
}

fn parse_claude_code_model_config_json(
    content: &str,
    source: &str,
) -> ExternalCliResolvedModelConfig {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return ExternalCliResolvedModelConfig::default();
    };
    let mut resolved = ExternalCliResolvedModelConfig {
        model: json_string(&value, "model"),
        model_source: Some(source.to_string()),
        ..Default::default()
    };
    if let Some(env) = value.get("env").and_then(serde_json::Value::as_object) {
        apply_claude_code_env_model_aliases(&mut resolved, source, |key| {
            env.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or(std::env::VarError::NotPresent)
        });
    }
    resolved
}

fn apply_claude_code_env_model_aliases(
    resolved: &mut ExternalCliResolvedModelConfig,
    source: &str,
    get_env: impl Fn(&str) -> Result<String, std::env::VarError>,
) {
    if let Some(model) = clean_optional_string(get_env("ANTHROPIC_MODEL").ok().as_deref()) {
        resolved.model = Some(model);
        resolved.model_provider = None;
        resolved.model_source = Some(source.to_string());
        return;
    }
    let Some(model) = resolved.model.as_deref().map(str::trim) else {
        return;
    };
    let Some(key) = claude_code_model_alias_env_key(model) else {
        return;
    };
    if let Some(alias_model) = clean_optional_string(get_env(key).ok().as_deref()) {
        resolved.model_provider = Some(model.to_string());
        resolved.model = Some(alias_model);
        resolved.model_source = Some(source.to_string());
    }
}

fn claude_code_model_alias_env_key(alias: &str) -> Option<&'static str> {
    match alias.to_ascii_lowercase().as_str() {
        "opus" => Some("ANTHROPIC_DEFAULT_OPUS_MODEL"),
        "sonnet" => Some("ANTHROPIC_DEFAULT_SONNET_MODEL"),
        "haiku" => Some("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        _ => None,
    }
}

fn apply_claude_code_env_effort(
    resolved: &mut ExternalCliResolvedModelConfig,
    source: &str,
    get_env: impl Fn(&str) -> Result<String, std::env::VarError>,
) {
    for key in ["CLAUDE_CODE_EFFORT_LEVEL", "CLAUDE_EFFORT"] {
        if let Some(effort) = clean_optional_string(get_env(key).ok().as_deref()) {
            resolved.reasoning_effort = Some(effort);
            resolved.reasoning_source = Some(source.to_string());
            return;
        }
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

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    clean_optional_string(value.get(key).and_then(serde_json::Value::as_str))
}

fn format_model_load(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64().map(|value| {
            let value = if value > 0.0 && value <= 1.0 {
                value * 100.0
            } else {
                value
            };
            if value.fract() == 0.0 {
                format!("{}%", value as i64)
            } else {
                format!("{value:.1}%")
            }
        }),
        serde_json::Value::String(value) => clean_optional_string(Some(value)).map(|value| {
            if value.ends_with('%') {
                value
            } else {
                format!("{value}%")
            }
        }),
        _ => None,
    }
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
    } else if let Some((label, source)) = external_cli_default_model_label(&request.adapter) {
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
    insert_metadata_u64(
        metadata,
        "runner.capacityRetryCount",
        capacity_retry_count(events),
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

fn capacity_retry_count(events: &[ExternalCliProgressEvent]) -> u64 {
    events
        .iter()
        .filter(|event| {
            event.raw.get("type").and_then(serde_json::Value::as_str) == Some("capacity_retry")
        })
        .count() as u64
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
    let mut parse_state = ExternalCliParseState::default();
    let mut tool_started_at: HashMap<String, u64> = HashMap::new();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("read external cli stdout failed: {error}"))?
    {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if let Some(mut event) = parse_progress_event_line_with_state(&line, &mut parse_state) {
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
        remove_active_session_if_owned(&session_key, run_id);
    }
}

fn remove_active_session_if_owned(session_key: &str, run_id: &str) -> bool {
    ACTIVE_SESSIONS
        .remove_if(session_key, |_, owner| owner == run_id)
        .is_some()
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
    let mut state = ExternalCliParseState::default();
    for line in stdout.lines() {
        if let Some(event) = parse_progress_event_line_with_state(line, &mut state) {
            events.push(event);
        }
    }
    events
}

#[derive(Default)]
struct ExternalCliParseState {
    claude_tools: BTreeMap<String, ClaudeCodeToolContext>,
}

#[derive(Clone, Debug, Default)]
struct ClaudeCodeToolContext {
    name: String,
    arguments: serde_json::Value,
}

fn parse_progress_event_line_with_state(
    line: &str,
    state: &mut ExternalCliParseState,
) -> Option<ExternalCliProgressEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    parse_progress_event(raw, state)
}

fn parse_progress_event(
    raw: serde_json::Value,
    state: &mut ExternalCliParseState,
) -> Option<ExternalCliProgressEvent> {
    let event_type = value_text(&raw, &["type", "event", "kind"])?;
    if let Some(event) = parse_codex_cli_event(&event_type, &raw) {
        return Some(event);
    }
    if let Some(event) = parse_claude_code_event(&event_type, &raw, state) {
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
                    result: external_progress_result_text(event),
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
    if let Some(text) = event
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
    {
        return text.to_string();
    }
    event
        .raw
        .get("arguments")
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn external_progress_result_text(event: &ExternalCliProgressEvent) -> String {
    if !event.content.trim().is_empty() {
        return event.content.clone();
    }
    external_progress_structured_detail(event).unwrap_or_default()
}

fn external_progress_structured_detail(event: &ExternalCliProgressEvent) -> Option<String> {
    let item = event.raw.get("item").unwrap_or(&event.raw);
    if external_progress_is_file_change(event, item) {
        return file_change_detail_from_value(item)
            .or_else(|| serde_json::to_string_pretty(item).ok());
    }
    None
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

fn is_file_change_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "file_change" | "file_changes" | "file_diff" | "file_edit" | "file_edits" | "patch"
    )
}

fn external_progress_is_file_change(
    event: &ExternalCliProgressEvent,
    item: &serde_json::Value,
) -> bool {
    event.title.as_deref().is_some_and(is_file_change_item_type)
        || value_text_path(item, &["type"])
            .is_some_and(|item_type| is_file_change_item_type(&item_type))
        || value_text_path(&event.raw, &["tool_name"])
            .is_some_and(|tool_name| is_file_change_item_type(&tool_name))
}

fn codex_file_change_event(raw: &serde_json::Value, item_type: &str) -> ExternalCliProgressEvent {
    let content = raw
        .get("item")
        .and_then(file_change_detail_from_value)
        .unwrap_or_else(|| {
            raw.get("item")
                .and_then(|item| serde_json::to_string_pretty(item).ok())
                .unwrap_or_default()
        });
    let mut enriched_raw = raw.clone();
    if let Some(object) = enriched_raw.as_object_mut() {
        object
            .entry("tool_name".to_string())
            .or_insert_with(|| serde_json::json!("file_change"));
        object
            .entry("success".to_string())
            .or_insert_with(|| serde_json::json!(true));
    }
    ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content,
        title: Some(if item_type == "patch" {
            "file_change".to_string()
        } else {
            item_type.to_string()
        }),
        raw: enriched_raw,
    }
}

fn file_change_detail_from_value(value: &serde_json::Value) -> Option<String> {
    let mut lines = Vec::new();
    for key in ["text", "message", "summary", "content", "description"] {
        if let Some(text) = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            lines.push(text.to_string());
        }
    }
    for key in ["file", "path", "file_path", "filePath", "filename", "name"] {
        if let Some(path) = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            lines.push(match file_change_action_label(value) {
                Some(action) => format!("file: {path} ({action})"),
                None => format!("file: {path}"),
            });
            break;
        }
    }
    for key in ["files", "changes", "edits"] {
        if let Some(items) = value.get(key).and_then(serde_json::Value::as_array) {
            append_file_change_items(&mut lines, key, items);
        }
    }
    for key in ["diff", "patch"] {
        if let Some(text) = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            lines.push(format!("{key}:\n{text}"));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn append_file_change_items(lines: &mut Vec<String>, label: &str, items: &[serde_json::Value]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for item in items {
        if let Some(path) = item.as_str().map(str::trim).filter(|path| !path.is_empty()) {
            lines.push(format!("- {path}"));
            continue;
        }
        if let Some(object) = item.as_object() {
            let path = ["path", "file_path", "filePath", "filename", "name"]
                .iter()
                .find_map(|key| {
                    object
                        .get(*key)
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("[unknown file]");
            lines.push(match file_change_action_label(item) {
                Some(action) => format!("- {path} ({action})"),
                None => format!("- {path}"),
            });
            if let Some(summary) = ["summary", "message", "description"]
                .iter()
                .find_map(|key| {
                    object
                        .get(*key)
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
            {
                lines.push(format!("  {summary}"));
            }
            if let Some(diff) = ["diff", "patch"].iter().find_map(|key| {
                object
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            }) {
                lines.push(format!("  {diff}"));
            }
        }
    }
}

fn file_change_action_label(value: &serde_json::Value) -> Option<&str> {
    ["action", "status", "operation", "kind"]
        .iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_codex_like_adapter(adapter: &str) -> bool {
    matches!(
        adapter,
        DEFAULT_ADAPTER | TRAEX_ADAPTER | CLAUDE_CODE_ADAPTER
    )
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
            if is_file_change_item_type(&item_type) {
                return Some(codex_file_change_event(raw, &item_type));
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
        _ if codex_event_is_token_usage_refresh(event_type, raw) => {
            Some(ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::Status,
                content: "token usage updated".to_string(),
                title: Some("token usage".to_string()),
                raw: raw.clone(),
            })
        }
        _ => None,
    }
}

fn codex_event_is_token_usage_refresh(event_type: &str, raw: &serde_json::Value) -> bool {
    if raw
        .get("usage")
        .or_else(|| raw.get("message").and_then(|message| message.get("usage")))
        .is_none()
    {
        return false;
    }
    let normalized = event_type
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '.', ' '], "_")
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    matches!(
        normalized.as_str(),
        "token_usage"
            | "token_usage_update"
            | "token_usage_updated"
            | "usage_update"
            | "usage_updated"
    )
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

fn parse_claude_code_event(
    event_type: &str,
    raw: &serde_json::Value,
    state: &mut ExternalCliParseState,
) -> Option<ExternalCliProgressEvent> {
    match event_type {
        "system" => {
            let subtype = value_text(raw, &["subtype"]).unwrap_or_default();
            let session_id = value_text(raw, &["session_id", "sessionId"]);
            if subtype == "init" || session_id.is_some() {
                Some(ExternalCliProgressEvent {
                    event_type: ExternalCliProgressEventType::RunStarted,
                    content: session_id.unwrap_or_else(|| "session started".to_string()),
                    title: Some("Claude Code session".to_string()),
                    raw: raw.clone(),
                })
            } else {
                Some(ExternalCliProgressEvent {
                    event_type: ExternalCliProgressEventType::Status,
                    content: subtype,
                    title: Some("Claude Code".to_string()),
                    raw: raw.clone(),
                })
            }
        }
        "assistant" => {
            if let Some(event) = claude_code_tool_use_event(raw, state) {
                return Some(event);
            }
            let content = claude_code_message_text(raw).unwrap_or_default();
            let event_type = if content.trim().is_empty() {
                ExternalCliProgressEventType::Status
            } else {
                ExternalCliProgressEventType::AssistantFinal
            };
            Some(ExternalCliProgressEvent {
                event_type,
                content,
                title: Some("Claude Code assistant".to_string()),
                raw: raw.clone(),
            })
        }
        "user" => claude_code_tool_result_event(raw, state),
        "result" => {
            let is_error = raw
                .get("is_error")
                .or_else(|| raw.get("isError"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let content = value_text(raw, &["result", "error", "message"]).unwrap_or_default();
            Some(ExternalCliProgressEvent {
                event_type: if is_error {
                    ExternalCliProgressEventType::RunFailed
                } else {
                    ExternalCliProgressEventType::RunFinished
                },
                content,
                title: Some("Claude Code result".to_string()),
                raw: raw.clone(),
            })
        }
        _ => None,
    }
}

fn claude_code_tool_use_event(
    raw: &serde_json::Value,
    state: &mut ExternalCliParseState,
) -> Option<ExternalCliProgressEvent> {
    let tool_use = claude_code_message_content(raw).and_then(|content| {
        content
            .as_array()?
            .iter()
            .find(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))
    })?;
    let tool_use_id = value_text(tool_use, &["id", "tool_use_id", "toolUseId"])?;
    let tool_name = value_text(tool_use, &["name"]).unwrap_or_else(|| "tool".to_string());
    let arguments = tool_use
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    state.claude_tools.insert(
        tool_use_id.clone(),
        ClaudeCodeToolContext {
            name: tool_name.clone(),
            arguments: arguments.clone(),
        },
    );
    let mut enriched_raw = raw.clone();
    if let Some(object) = enriched_raw.as_object_mut() {
        object
            .entry("tool_name".to_string())
            .or_insert_with(|| serde_json::json!(tool_name));
        object
            .entry("tool_use_id".to_string())
            .or_insert_with(|| serde_json::json!(tool_use_id));
        object
            .entry("arguments".to_string())
            .or_insert(arguments.clone());
    }
    Some(ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolStarted,
        content: claude_code_tool_arguments_text(&arguments),
        title: Some(tool_name),
        raw: enriched_raw,
    })
}

fn claude_code_tool_result_event(
    raw: &serde_json::Value,
    state: &mut ExternalCliParseState,
) -> Option<ExternalCliProgressEvent> {
    let tool_result = claude_code_message_content(raw).and_then(|content| {
        content.as_array()?.iter().find(|part| {
            part.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
        })
    })?;
    let tool_use_id = value_text(tool_result, &["tool_use_id", "toolUseId"])?;
    let context = state.claude_tools.get(&tool_use_id).cloned();
    let tool_name = context
        .as_ref()
        .map(|ctx| ctx.name.clone())
        .unwrap_or_else(|| "tool".to_string());
    let arguments = context
        .as_ref()
        .map(|ctx| ctx.arguments.clone())
        .unwrap_or(serde_json::Value::Null);
    let content = value_text(tool_result, &["content"])
        .or_else(|| value_text_path(raw, &["tool_use_result", "stdout"]))
        .unwrap_or_default();
    let stderr = value_text_path(raw, &["tool_use_result", "stderr"]).unwrap_or_default();
    let result = if stderr.trim().is_empty() {
        content.clone()
    } else if content.trim().is_empty() {
        stderr.clone()
    } else {
        format!("{content}\n{stderr}")
    };
    let is_error = tool_result
        .get("is_error")
        .or_else(|| tool_result.get("isError"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let interrupted = raw
        .get("tool_use_result")
        .and_then(|value| value.get("interrupted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut enriched_raw = raw.clone();
    if let Some(object) = enriched_raw.as_object_mut() {
        object
            .entry("tool_name".to_string())
            .or_insert_with(|| serde_json::json!(tool_name));
        object
            .entry("tool_use_id".to_string())
            .or_insert_with(|| serde_json::json!(tool_use_id));
        object
            .entry("arguments".to_string())
            .or_insert(arguments.clone());
        object
            .entry("success".to_string())
            .or_insert_with(|| serde_json::json!(!is_error && !interrupted));
    }
    Some(ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: result,
        title: Some(tool_name),
        raw: enriched_raw,
    })
}

fn claude_code_message_content(raw: &serde_json::Value) -> Option<&serde_json::Value> {
    raw.get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| raw.get("content"))
}

fn claude_code_tool_arguments_text(arguments: &serde_json::Value) -> String {
    if let Some(command) = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return command.to_string();
    }
    match arguments {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => text.clone(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn claude_code_message_text(raw: &serde_json::Value) -> Option<String> {
    let content = claude_code_message_content(raw)?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let parts = content.as_array()?;
    let texts = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| part.as_str())
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!texts.is_empty()).then(|| texts.join("\n"))
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
    migrate_legacy_traex_runner_id(&mut config);
    migrate_legacy_claude_code_runner_id(&mut config);
    ensure_default_external_cli_runners(&mut config.runners);
    if !config.runners.contains_key(&config.default_runner_id) {
        let canonical_default_runner_id =
            canonical_external_cli_runner_id(&config, &config.default_runner_id);
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
        .entry(DEFAULT_TRAEX_RUNNER_ID.to_string())
        .or_insert_with(|| default_runner_settings(TRAEX_ADAPTER));
    runners
        .entry(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string())
        .or_insert_with(|| default_runner_settings(CLAUDE_CODE_ADAPTER));
}

fn migrate_legacy_claude_code_runner_id(config: &mut ExternalCliGatewayConfig) {
    let legacy_runner_id = config
        .runners
        .keys()
        .find(|runner_id| {
            runner_id.eq_ignore_ascii_case(LEGACY_CLAUDE_CODE_RUNNER_ID)
                || runner_id.eq_ignore_ascii_case("claude-code")
                || runner_id.eq_ignore_ascii_case("claude_code")
                || runner_id.eq_ignore_ascii_case("claude")
        })
        .cloned();
    let Some(legacy_runner_id) = legacy_runner_id else {
        return;
    };
    let Some(legacy_settings) = config.runners.remove(&legacy_runner_id) else {
        return;
    };
    config
        .runners
        .entry(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string())
        .or_insert(legacy_settings);
    if config
        .default_runner_id
        .eq_ignore_ascii_case(&legacy_runner_id)
    {
        config.default_runner_id = DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string();
    }
    for channel in config.channels.values_mut() {
        if channel
            .runner_id
            .as_deref()
            .is_some_and(|runner_id| runner_id.eq_ignore_ascii_case(&legacy_runner_id))
        {
            channel.runner_id = Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string());
        }
    }
}

fn migrate_legacy_traex_runner_id(config: &mut ExternalCliGatewayConfig) {
    let legacy_runner_id = config
        .runners
        .keys()
        .find(|runner_id| runner_id.eq_ignore_ascii_case(LEGACY_TRAEX_RUNNER_ALIAS))
        .cloned();
    let Some(legacy_runner_id) = legacy_runner_id else {
        return;
    };
    let Some(legacy_settings) = config.runners.remove(&legacy_runner_id) else {
        return;
    };
    config
        .runners
        .entry(DEFAULT_TRAEX_RUNNER_ID.to_string())
        .or_insert(legacy_settings);
    if config
        .default_runner_id
        .eq_ignore_ascii_case(LEGACY_TRAEX_RUNNER_ALIAS)
    {
        config.default_runner_id = DEFAULT_TRAEX_RUNNER_ID.to_string();
    }
    for channel in config.channels.values_mut() {
        if channel
            .runner_id
            .as_deref()
            .is_some_and(|runner_id| runner_id.eq_ignore_ascii_case(LEGACY_TRAEX_RUNNER_ALIAS))
        {
            channel.runner_id = Some(DEFAULT_TRAEX_RUNNER_ID.to_string());
        }
    }
}

fn default_runner_settings(adapter: &str) -> ExternalCliAgentSettings {
    ExternalCliAgentSettings {
        enabled: true,
        adapter: adapter.to_string(),
        ..ExternalCliAgentSettings::default()
    }
}

pub fn canonical_external_cli_runner_id(
    config: &ExternalCliGatewayConfig,
    runner_id: &str,
) -> String {
    let runner_id = runner_id.trim();
    if runner_id.eq_ignore_ascii_case(LEGACY_TRAEX_RUNNER_ALIAS)
        && config.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID)
    {
        return DEFAULT_TRAEX_RUNNER_ID.to_string();
    }
    if config.runners.contains_key(runner_id) {
        return runner_id.to_string();
    }
    match runner_id.to_ascii_lowercase().as_str() {
        "codex" if config.runners.contains_key(DEFAULT_CODEX_RUNNER_ID) => {
            DEFAULT_CODEX_RUNNER_ID.to_string()
        }
        "traex" | "trae" if config.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID) => {
            DEFAULT_TRAEX_RUNNER_ID.to_string()
        }
        "claude_code" | "claude-code" | "claude" | "claude code"
            if config.runners.contains_key(DEFAULT_CLAUDE_CODE_RUNNER_ID) =>
        {
            DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()
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
