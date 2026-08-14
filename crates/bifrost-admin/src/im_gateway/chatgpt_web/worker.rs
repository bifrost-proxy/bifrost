use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::im_gateway::external_cli::{
    ExternalCliAgentSettings, ExternalCliProgressEvent, ExternalCliRunRequest,
    ExternalCliRunResult, ExternalCliRuntime,
};
use crate::worker_runtime::{
    global_worker_supervisor, run_worker_stdio, ParentFrame, WorkerEvent, WorkerKind,
    WorkerSpawnSpec, WorkerStdioContext,
};

const BROWSER_WORKER_ENV: &str = "BIFROST_BROWSER_WORKER";
const BROWSER_WORKER_KEY: &str = "browser:chatgpt_web";
const BROWSER_REQUEST_MAX_BYTES: u64 = 384 * 1024 * 1024;
const BROWSER_RESULT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_IDLE_SECS: u64 = 10 * 60;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30 * 60;
const MAX_PROGRESS_CONTENT_BYTES: usize = 16 * 1024;
const MAX_PROGRESS_TITLE_BYTES: usize = 1024;
const MAX_PROGRESS_RAW_BYTES: usize = 32 * 1024;

static LAST_USED_MS: AtomicU64 = AtomicU64::new(0);
static IDLE_REAPER_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserRunFileRequest {
    runs_root: PathBuf,
    request: ExternalCliRunRequest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserRunReference {
    request_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserResultReference {
    result_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRequest {
    settings: ExternalCliAgentSettings,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartupAuthRequest {
    runner_id: String,
    settings: ExternalCliAgentSettings,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRequest {
    session_key: String,
}

pub(crate) fn is_browser_worker_process() -> bool {
    std::env::var(BROWSER_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(crate) async fn run_via_browser_worker(
    runs_root: PathBuf,
    request: ExternalCliRunRequest,
    progress_tx: Option<mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<ExternalCliRunResult, String> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let logical_job_id = request
        .session_key
        .clone()
        .unwrap_or_else(|| request_id.clone());
    let request_path = request_dir().join(format!("request-{request_id}.json"));
    write_json_file(
        &request_path,
        &BrowserRunFileRequest { runs_root, request },
        BROWSER_REQUEST_MAX_BYTES,
    )?;

    let worker = match ensure_worker().await {
        Ok(worker) => worker,
        Err(error) => {
            let _ = std::fs::remove_file(&request_path);
            return Err(error);
        }
    };
    let mut events = worker.subscribe_events();
    let request_future = worker.request_with_id(
        request_id.clone(),
        Some(logical_job_id),
        "browser.run",
        serde_json::to_value(BrowserRunReference {
            request_path: request_path.clone(),
        })
        .map_err(|error| format!("serialize browser worker request reference: {error}"))?,
        Some(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)),
    );
    tokio::pin!(request_future);

    let response = loop {
        tokio::select! {
            result = &mut request_future => break result,
            event = events.recv() => {
                match event {
                    Ok(event) if event.request_id.as_deref() == Some(request_id.as_str()) && event.event == "progress" => {
                        if let Some(progress_tx) = progress_tx.as_ref() {
                            if let Ok(progress) = serde_json::from_value::<ExternalCliProgressEvent>(event.payload) {
                                let _ = progress_tx.try_send(progress);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "browser worker progress receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }
        }
    };
    let _ = std::fs::remove_file(&request_path);
    touch();
    let value = response?;
    let reference: BrowserResultReference = serde_json::from_value(value)
        .map_err(|error| format!("parse browser worker result reference: {error}"))?;
    let result_path = validate_runtime_path(&reference.result_path, &result_dir())?;
    let result = read_json_file::<ExternalCliRunResult>(&result_path, BROWSER_RESULT_MAX_BYTES);
    let _ = std::fs::remove_file(result_path);
    let result = result?;
    match result.status {
        crate::im_gateway::external_cli::ExternalCliRunStatus::Failed
        | crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => {
            crate::worker_runtime::mark_worker_job_failed(&request_id, result.response.clone());
        }
        crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => {
            crate::worker_runtime::mark_worker_job_cancelled(
                &request_id,
                Some("browser run stopped".to_string()),
            );
        }
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => {}
    }
    Ok(result)
}

pub(crate) async fn stop_session_run(session_key: &str) -> bool {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return false;
    }
    let Some(worker) = global_worker_supervisor().get(BROWSER_WORKER_KEY).await else {
        return false;
    };
    let jobs = crate::worker_runtime::worker_jobs();
    let Some(job) = jobs.into_iter().find(|job| {
        job.worker_key == BROWSER_WORKER_KEY
            && job.logical_job_id.as_deref() == Some(session_key)
            && !matches!(
                job.status,
                crate::worker_runtime::WorkerJobStatus::Succeeded
                    | crate::worker_runtime::WorkerJobStatus::Failed
                    | crate::worker_runtime::WorkerJobStatus::Cancelled
            )
    }) else {
        return false;
    };
    worker
        .cancel_request(&job.request_id, session_key)
        .await
        .unwrap_or(false)
}

pub(crate) async fn auth_status(
    settings: &ExternalCliAgentSettings,
) -> Result<super::ChatGptWebAuthStatus, String> {
    request_json(
        "browser.auth_status",
        &AuthRequest {
            settings: settings.clone(),
        },
    )
    .await
}

pub(crate) async fn open_login(
    settings: &ExternalCliAgentSettings,
) -> Result<super::ChatGptWebAuthStatus, String> {
    request_json(
        "browser.open_login",
        &AuthRequest {
            settings: settings.clone(),
        },
    )
    .await
}

pub(crate) async fn stop_login(
    settings: &ExternalCliAgentSettings,
) -> Result<super::ChatGptWebAuthStatus, String> {
    request_json(
        "browser.stop_login",
        &AuthRequest {
            settings: settings.clone(),
        },
    )
    .await
}

pub(crate) async fn ensure_startup_auth_ready(
    runner_id: &str,
    settings: &ExternalCliAgentSettings,
) -> Result<super::ChatGptWebStartupAuthStatus, String> {
    request_json(
        "browser.ensure_startup_auth_ready",
        &StartupAuthRequest {
            runner_id: runner_id.to_string(),
            settings: settings.clone(),
        },
    )
    .await
}

pub(crate) async fn clear_session_conversation(session_key: &str) -> Result<(), String> {
    let _: serde_json::Value = request_json(
        "browser.clear_session_conversation",
        &SessionRequest {
            session_key: session_key.to_string(),
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn session_conversation_exists(session_key: &str) -> Result<bool, String> {
    request_json(
        "browser.session_conversation_exists",
        &SessionRequest {
            session_key: session_key.to_string(),
        },
    )
    .await
}

async fn request_json<T: Serialize, R: DeserializeOwned>(
    operation: &str,
    value: &T,
) -> Result<R, String> {
    let worker = ensure_worker().await?;
    let payload = serde_json::to_value(value)
        .map_err(|error| format!("serialize {operation} payload: {error}"))?;
    let response = if matches!(operation, "browser.auth_status" | "browser.stop_login") {
        worker.request_control(operation, payload, None).await
    } else {
        worker.request(operation, payload, None).await
    };
    touch();
    let response = response?;
    serde_json::from_value(response).map_err(|error| format!("parse {operation} response: {error}"))
}

async fn ensure_worker() -> Result<Arc<crate::worker_runtime::ManagedWorker>, String> {
    touch();
    start_idle_reaper();
    global_worker_supervisor().get_or_start(spawn_spec()?).await
}

fn spawn_spec() -> Result<WorkerSpawnSpec, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve browser worker executable: {error}"))?;
    let executable = labeled_worker_executable(&executable, "bifrost-browser-worker");
    let data_dir = bifrost_storage::data_dir();
    let mut spec = WorkerSpawnSpec::new(
        BROWSER_WORKER_KEY,
        WorkerKind::Browser,
        executable,
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "browser".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.env
        .insert(BROWSER_WORKER_ENV.to_string(), "1".to_string());
    // Browser cancellation currently tears down the managed browser process tree.
    // Serialize requests so cancelling one run cannot terminate another live run.
    spec.max_concurrency = 1;
    spec.request_timeout = Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS);
    spec.queue_wait_timeout = Duration::from_secs(30);
    spec.heartbeat_timeout = Duration::from_secs(40);
    spec.stderr_path = Some(runtime_root().join("browser-worker.log"));
    Ok(spec)
}

fn start_idle_reaper() {
    if IDLE_REAPER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let idle = Duration::from_secs(idle_secs());
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let last = LAST_USED_MS.load(Ordering::Acquire);
            if last == 0
                || crate::worker_runtime::worker_now_ms().saturating_sub(last)
                    < idle.as_millis() as u64
            {
                continue;
            }
            if let Some(worker) = global_worker_supervisor().get(BROWSER_WORKER_KEY).await {
                let snapshot = worker.snapshot(0, None, None);
                if snapshot.active_jobs == 0 && snapshot.queued_jobs == 0 {
                    global_worker_supervisor()
                        .stop(BROWSER_WORKER_KEY, Duration::from_secs(5))
                        .await;
                    LAST_USED_MS.store(0, Ordering::Release);
                }
            }
        }
    });
}

fn touch() {
    LAST_USED_MS.store(crate::worker_runtime::worker_now_ms(), Ordering::Release);
}

fn idle_secs() -> u64 {
    std::env::var("BIFROST_BROWSER_WORKER_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 30)
        .unwrap_or(DEFAULT_IDLE_SECS)
}

pub(crate) fn run_browser_worker_stdio() -> Result<(), String> {
    std::env::set_var(BROWSER_WORKER_ENV, "1");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-browser-worker")
        .build()
        .map_err(|error| format!("build browser worker runtime: {error}"))?;
    runtime.block_on(run_worker_stdio(
        WorkerKind::Browser,
        vec![
            "browser.run".to_string(),
            "browser.auth_status".to_string(),
            "browser.open_login".to_string(),
            "browser.stop_login".to_string(),
            "browser.ensure_startup_auth_ready".to_string(),
            "browser.clear_session_conversation".to_string(),
            "browser.session_conversation_exists".to_string(),
        ],
        handle_worker_frame,
    ))
}

async fn handle_worker_frame(
    frame: ParentFrame,
    context: Arc<WorkerStdioContext>,
) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => {
            let request_id = request.request_id.clone();
            let result = handle_worker_request(
                &request_id,
                &request.operation,
                request.payload,
                context.clone(),
            )
            .await;
            context.response(request_id, result).await;
        }
        ParentFrame::Cancel { .. } => {
            super::kill_all_managed_browsers();
        }
        ParentFrame::Shutdown { .. } => {
            super::kill_all_managed_browsers();
        }
        ParentFrame::ConfigApply {
            request_id,
            generation,
            ..
        } => {
            context
                .response(
                    request_id,
                    Ok(serde_json::json!({"generation": generation, "applied": true})),
                )
                .await;
        }
        ParentFrame::Ping { .. } => {}
    }
    Ok(())
}

async fn handle_worker_request(
    request_id: &str,
    operation: &str,
    payload: serde_json::Value,
    context: Arc<WorkerStdioContext>,
) -> Result<serde_json::Value, String> {
    match operation {
        "browser.run" => {
            let reference: BrowserRunReference = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser run reference: {error}"))?;
            let request_path = validate_runtime_path(&reference.request_path, &request_dir())?;
            let request =
                read_json_file::<BrowserRunFileRequest>(&request_path, BROWSER_REQUEST_MAX_BYTES);
            let _ = std::fs::remove_file(&request_path);
            let request = request?;
            let (progress_tx, mut progress_rx) =
                mpsc::channel(super::super::external_cli::EXTERNAL_CLI_PROGRESS_CHANNEL_CAPACITY);
            let progress_context = context.clone();
            let progress_request_id = request_id.to_string();
            let progress_task = tokio::spawn(async move {
                while let Some(event) = progress_rx.recv().await {
                    let _ = progress_context.try_event(WorkerEvent {
                        request_id: Some(progress_request_id.clone()),
                        job_id: Some(progress_request_id.clone()),
                        event: "progress".to_string(),
                        payload: serde_json::to_value(compact_progress_event(event))
                            .unwrap_or(serde_json::Value::Null),
                    });
                }
            });
            let result = ExternalCliRuntime::new(request.runs_root)
                .run_in_current_process_with_progress(request.request, Some(progress_tx))
                .await;
            let _ = progress_task.await;
            let result = result?;
            let result_path = result_dir().join(format!("result-{request_id}.json"));
            write_json_file(&result_path, &result, BROWSER_RESULT_MAX_BYTES)?;
            serde_json::to_value(BrowserResultReference { result_path })
                .map_err(|error| format!("serialize browser result reference: {error}"))
        }
        "browser.auth_status" => {
            let request: AuthRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser auth request: {error}"))?;
            serde_json::to_value(super::auth_status(&request.settings).await?)
                .map_err(|error| error.to_string())
        }
        "browser.open_login" => {
            let request: AuthRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser login request: {error}"))?;
            serde_json::to_value(super::open_login(&request.settings).await?)
                .map_err(|error| error.to_string())
        }
        "browser.stop_login" => {
            let request: AuthRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser stop login request: {error}"))?;
            serde_json::to_value(super::stop_login(&request.settings).await?)
                .map_err(|error| error.to_string())
        }
        "browser.ensure_startup_auth_ready" => {
            let request: StartupAuthRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser startup auth request: {error}"))?;
            serde_json::to_value(
                super::ensure_startup_auth_ready(&request.runner_id, &request.settings).await?,
            )
            .map_err(|error| error.to_string())
        }
        "browser.clear_session_conversation" => {
            let request: SessionRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser session request: {error}"))?;
            super::interaction::clear_session_conversation(&request.session_key).await;
            Ok(serde_json::json!({"cleared": true}))
        }
        "browser.session_conversation_exists" => {
            let request: SessionRequest = serde_json::from_value(payload)
                .map_err(|error| format!("parse browser session request: {error}"))?;
            Ok(serde_json::Value::Bool(
                super::interaction::session_conversation_exists(&request.session_key).await,
            ))
        }
        other => Err(format!("unsupported browser worker operation '{other}'")),
    }
}

fn compact_progress_event(mut event: ExternalCliProgressEvent) -> ExternalCliProgressEvent {
    event.content = truncate_bytes(&event.content, MAX_PROGRESS_CONTENT_BYTES);
    event.title = event
        .title
        .map(|value| truncate_bytes(&value, MAX_PROGRESS_TITLE_BYTES));
    if serde_json::to_vec(&event.raw)
        .map(|bytes| bytes.len() > MAX_PROGRESS_RAW_BYTES)
        .unwrap_or(true)
    {
        event.raw = serde_json::json!({"truncated": true});
    }
    event
}

fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn runtime_root() -> PathBuf {
    bifrost_storage::data_dir().join("runtime/browser-worker")
}
fn request_dir() -> PathBuf {
    runtime_root().join("requests")
}
fn result_dir() -> PathBuf {
    runtime_root().join("results")
}

struct LimitedJsonWriter<W> {
    inner: W,
    written: u64,
    max_bytes: u64,
}

impl<W: Write> Write for LimitedJsonWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "browser worker JSON exceeds configured limit",
            ));
        }
        let written = self.inner.write(buf)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn open_private_temp(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))
}

fn write_json_file<T: Serialize>(path: &Path, value: &T, max_bytes: u64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let file = open_private_temp(&temp)?;
    let mut writer = LimitedJsonWriter {
        inner: file,
        written: 0,
        max_bytes,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("serialize {}: {error}", path.display()));
    }
    if let Err(error) = writer.flush() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("flush {}: {error}", temp.display()));
    }
    drop(writer);
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "rename {} -> {}: {error}",
            temp.display(),
            path.display()
        ));
    }
    Ok(())
}

fn read_json_file<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Result<T, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "browser worker file exceeds limit: {} > {max_bytes}",
            metadata.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn validate_runtime_path(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("canonicalize {}: {error}", root.display()))?;
    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("canonicalize {}: {error}", path.display()))?;
    if !path.starts_with(&root) {
        return Err(format!(
            "browser worker path {} is outside {}",
            path.display(),
            root.display()
        ));
    }
    Ok(path)
}

fn labeled_worker_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    bifrost_core::process_alias_executable(executable, &alias_dir, alias_name)
        .unwrap_or_else(|_| executable.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_compaction_bounds_fields() {
        let event = ExternalCliProgressEvent {
            event_type: crate::im_gateway::external_cli::ExternalCliProgressEventType::Status,
            content: "x".repeat(MAX_PROGRESS_CONTENT_BYTES + 100),
            title: Some("y".repeat(MAX_PROGRESS_TITLE_BYTES + 100)),
            raw: serde_json::json!({"data": "z".repeat(MAX_PROGRESS_RAW_BYTES + 100)}),
        };
        let compact = compact_progress_event(event);
        assert!(compact.content.len() <= MAX_PROGRESS_CONTENT_BYTES + 3);
        assert!(compact.title.unwrap().len() <= MAX_PROGRESS_TITLE_BYTES + 3);
        assert_eq!(compact.raw, serde_json::json!({"truncated": true}));
    }
}
