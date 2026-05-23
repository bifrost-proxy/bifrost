use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_RUNTIME: &str = "external_cli";
const DEFAULT_ADAPTER: &str = "codex";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const CONFIG_FILENAME: &str = "im_gateway_external_cli_agent.json";
const CONFIG_VERSION: u32 = 1;

static ACTIVE_RUNS: once_cell::sync::Lazy<dashmap::DashMap<String, u32>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static ACTIVE_SESSIONS: once_cell::sync::Lazy<dashmap::DashMap<String, String>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunRequest {
    pub message: String,
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
    pub model: Option<String>,
    #[serde(default)]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<String>,
    #[serde(default)]
    pub search: Option<bool>,
    #[serde(default)]
    pub ephemeral: Option<bool>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCliDeliveryMode {
    NoIm,
    #[default]
    FinalReply,
    ProgressCard,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliChannelSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub delivery_mode: Option<ExternalCliDeliveryMode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
            runners: BTreeMap::from([(default_runner_id(), ExternalCliAgentSettings::default())]),
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
        let data = load_config_from_disk(&file_path).unwrap_or_default();
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
    timeout_secs: u64,
}

#[derive(Clone, Debug)]
struct CommandSpec {
    executable: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    work_dir: Option<PathBuf>,
    timeout_secs: u64,
}

#[derive(Clone, Debug)]
struct CommandOutput {
    status: ExternalCliRunStatus,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ExternalCliRuntime {
    runs_root: PathBuf,
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

        let prompt = build_prompt(&request).await?;
        tokio::fs::write(&prompt_path, &prompt)
            .await
            .map_err(|error| format!("write prompt failed: {error}"))?;

        let spec = build_command_spec(&request, &last_message_path)?;
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
                Err(error) => return Err(error),
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
            let command_output =
                run_command(&run_id, session_key_for_stop.as_deref(), spec, prompt).await?;
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
                parse_progress_events(&stdout_text)
            };
            let response = if was_stopped {
                "External CLI run was stopped by request.".to_string()
            } else {
                final_response(&last_message_path, &stdout_text, &events).await?
            };
            AdapterRunOutput {
                status: if was_stopped {
                    ExternalCliRunStatus::Stopped
                } else {
                    command_output.status
                },
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
        let mut metadata = run_output.metadata;
        append_external_cli_metadata(&request.adapter, &run_output.events, &mut metadata);
        tokio::fs::write(&stdout_path, &run_output.stdout)
            .await
            .map_err(|error| format!("write stdout failed: {error}"))?;
        tokio::fs::write(&stderr_path, &run_output.stderr)
            .await
            .map_err(|error| format!("write stderr failed: {error}"))?;
        write_events_jsonl(&events_path, &run_output.events).await?;
        let finished_at = now_ms();
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

    let snapshot: serde_json::Value = read_json(&snapshot_path).await?;
    let events = read_events_jsonl(&events_path).await?;
    let stdout = read_text_or_default(&stdout_path).await?;
    let stderr = read_text_or_default(&stderr_path).await?;
    let response = final_response(&last_message_path, &stdout, &events).await?;

    Ok(ExternalCliRunDetail {
        run_id: run_id.to_string(),
        snapshot,
        events,
        response,
        stdout,
        stderr,
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
    if needs_message && request.message.trim().is_empty() {
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

async fn build_prompt(request: &ExternalCliRunRequest) -> Result<String, String> {
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
    prompt.push_str(request.message.trim());
    prompt.push('\n');
    Ok(prompt)
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

fn build_command_spec(
    request: &ExternalCliRunRequest,
    last_message_path: &Path,
) -> Result<CommandSpec, String> {
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        return Ok(CommandSpec {
            executable: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
            args: vec![request.operation.clone()],
            env: BTreeMap::new(),
            work_dir: None,
            timeout_secs: request.adapter_config.timeout_secs.unwrap_or(7200),
        });
    }
    let config = &request.adapter_config;
    let executable = config
        .executable
        .clone()
        .unwrap_or_else(|| request.adapter.clone());
    let mut args = config.args.clone();

    if request.adapter == DEFAULT_ADAPTER {
        if args.is_empty() {
            if codex_thread_id_from_params(request).is_some() {
                args = vec![
                    "exec".to_string(),
                    "resume".to_string(),
                    "--json".to_string(),
                ];
            } else {
                args = vec!["exec".to_string(), "--json".to_string()];
            }
        }
        if let Some(work_dir) = request.work_dir.as_ref() {
            ensure_codex_work_dir_arg(&mut args, work_dir);
        }
        ensure_codex_last_message_arg(&mut args, last_message_path);
        if config.args.is_empty() {
            let resume_thread_id = codex_thread_id_from_params(request);
            let is_resume = resume_thread_id.is_some();
            if let Some(profile) = config.profile.as_deref() {
                if !is_resume {
                    args.push("--profile".to_string());
                    args.push(profile.to_string());
                }
            }
            if let Some(model) = config.model.as_deref() {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            if let Some(sandbox) = config.sandbox.as_deref() {
                if !is_resume {
                    args.push("--sandbox".to_string());
                    args.push(sandbox.to_string());
                }
            }
            if !is_resume && config.search.unwrap_or(false) {
                args.push("--search".to_string());
            }
            if config.ephemeral.unwrap_or(false) {
                args.push("--ephemeral".to_string());
            }
            if let Some(thread_id) = resume_thread_id {
                args.push(thread_id);
            }
            args.push("-".to_string());
        }
    } else if config.executable.is_none() && args.is_empty() {
        return Err(format!(
            "adapter '{}' requires explicit adapterConfig.args",
            request.adapter
        ));
    }

    Ok(CommandSpec {
        executable,
        args,
        env: config.env.clone(),
        work_dir: request.work_dir.clone(),
        timeout_secs: config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
    })
}

fn codex_thread_id_from_params(request: &ExternalCliRunRequest) -> Option<String> {
    request
        .params
        .get("threadId")
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn append_external_cli_metadata(
    adapter: &str,
    events: &[ExternalCliProgressEvent],
    metadata: &mut BTreeMap<String, String>,
) {
    if adapter != DEFAULT_ADAPTER {
        return;
    }
    if metadata.contains_key("threadId") {
        return;
    }
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

fn ensure_codex_work_dir_arg(args: &mut Vec<String>, work_dir: &Path) {
    if args.iter().any(|arg| arg == "--cd" || arg == "-C") {
        return;
    }
    let insert_at = match args.first().map(String::as_str) {
        Some("exec" | "e") => 1,
        _ => 0,
    };
    args.insert(insert_at, work_dir.display().to_string());
    args.insert(insert_at, "--cd".to_string());
}

fn ensure_codex_last_message_arg(args: &mut Vec<String>, last_message_path: &Path) {
    if !matches!(args.first().map(String::as_str), Some("exec" | "e")) {
        return;
    }
    if args
        .iter()
        .any(|arg| arg == "--output-last-message" || arg == "-o")
    {
        return;
    }
    let insert_at = if args.last().map(String::as_str) == Some("-") {
        args.len() - 1
    } else {
        args.len()
    };
    args.insert(insert_at, last_message_path.display().to_string());
    args.insert(insert_at, "--output-last-message".to_string());
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
        timeout_secs: spec.timeout_secs,
    }
}

async fn run_command(
    run_id: &str,
    session_key: Option<&str>,
    spec: CommandSpec,
    prompt: String,
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

    match timeout(
        Duration::from_secs(spec.timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => {
            let status = if output.status.success() {
                ExternalCliRunStatus::Succeeded
            } else {
                ExternalCliRunStatus::Failed
            };
            ACTIVE_RUNS.remove(run_id);
            remove_active_sessions_for_run(run_id);
            Ok(CommandOutput {
                status,
                exit_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
        Ok(Err(error)) => {
            ACTIVE_RUNS.remove(run_id);
            remove_active_sessions_for_run(run_id);
            Err(format!("wait external cli failed: {error}"))
        }
        Err(_) => {
            // Timeout: kill the entire process group
            if pid != 0 {
                if let Err(error) = terminate_process(pid) {
                    tracing::warn!(pid, error = %error, "failed to terminate timed-out process group");
                }
            }
            ACTIVE_RUNS.remove(run_id);
            remove_active_sessions_for_run(run_id);
            Ok(CommandOutput {
                status: ExternalCliRunStatus::TimedOut,
                exit_code: None,
                stdout: Vec::new(),
                stderr: format!("external cli timed out after {} seconds", spec.timeout_secs)
                    .into_bytes(),
            })
        }
    }
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
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    if pid > i32::MAX as u32 {
        return Err(format!("pid {pid} is too large to terminate"));
    }

    // We always spawn with process_group(0), so the child PID is the group leader.
    // Kill the entire process group via negative PID, regardless of whether the
    // leader is still alive (kill_on_drop may have already killed it). This ensures
    // background children spawned by the shell are also terminated.
    let group_pid = Pid::from_raw(-(pid as i32));
    match kill(group_pid, Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => {
            // Entire group is already gone — nothing to do
            Ok(())
        }
        Err(Errno::EPERM) => {
            // No permission for group kill, try the child directly as fallback
            let child_pid = Pid::from_raw(pid as i32);
            match kill(child_pid, Signal::SIGTERM) {
                Ok(()) | Err(Errno::ESRCH) => Ok(()),
                Err(error) => Err(format!("failed to terminate pid {pid}: {error}")),
            }
        }
        Err(error) => Err(format!("failed to terminate process group {pid}: {error}")),
    }
}

#[cfg(windows)]
fn terminate_process_impl(pid: u32) -> Result<(), String> {
    let status = StdCommand::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .arg("/F")
        .status()
        .map_err(|error| format!("failed to invoke taskkill for pid {pid}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "taskkill /PID {pid} /T /F exited with status {status}"
        ))
    }
}

pub fn parse_progress_events(stdout: &str) -> Vec<ExternalCliProgressEvent> {
    let mut events = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if let Some(event) = parse_progress_event(raw) {
            events.push(event);
        }
    }
    events
}

fn parse_progress_event(raw: serde_json::Value) -> Option<ExternalCliProgressEvent> {
    let event_type = value_text(&raw, &["type", "event", "kind"])?;
    if let Some(event) = parse_codex_cli_event(&event_type, &raw) {
        return Some(event);
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
        "item.completed" => {
            let item_type = value_text_path(raw, &["item", "type"])?;
            let content = value_text_path(raw, &["item", "text"])
                .or_else(|| value_text_path(raw, &["item", "message"]))
                .or_else(|| value_text_path(raw, &["item", "title"]))
                .unwrap_or_default();
            let normalized_type = match item_type.as_str() {
                "agent_message" => ExternalCliProgressEventType::AssistantFinal,
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
        config.runners.insert(
            config.default_runner_id.clone(),
            ExternalCliAgentSettings::default(),
        );
    } else {
        config.runners = config
            .runners
            .into_iter()
            .filter_map(|(runner_id, settings)| {
                let runner_id = runner_id.trim().to_string();
                (!runner_id.is_empty()).then_some((runner_id, normalized_agent_settings(settings)))
            })
            .collect();
        config
            .runners
            .entry(config.default_runner_id.clone())
            .or_default();
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
    let config = serde_json::from_str::<ExternalCliGatewayConfig>(&content).ok()?;
    Some(normalized_gateway_config(config))
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
    DEFAULT_ADAPTER.to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests;
