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
const REQUEST_TIMEOUT_HEADROOM_SECS: u64 = 5;
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

struct RemoveFileOnDrop(PathBuf);

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(crate) fn is_browser_worker_process() -> bool {
    std::env::var(BROWSER_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn forward_browser_progress(
    event: WorkerEvent,
    request_id: &str,
    progress_tx: Option<&mpsc::Sender<ExternalCliProgressEvent>>,
) {
    if event.request_id.as_deref() != Some(request_id) || event.event != "progress" {
        return;
    }
    let Some(progress_tx) = progress_tx else {
        return;
    };
    if let Ok(progress) = serde_json::from_value::<ExternalCliProgressEvent>(event.payload) {
        let _ = progress_tx.try_send(progress);
    }
}

pub(crate) async fn run_via_browser_worker(
    runs_root: PathBuf,
    request: ExternalCliRunRequest,
    progress_tx: Option<mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<ExternalCliRunResult, String> {
    let request_timeout = browser_worker_request_timeout(&request);
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
    let _request_cleanup = RemoveFileOnDrop(request_path.clone());

    let worker = match ensure_worker().await {
        Ok(worker) => worker,
        Err(error) => return Err(error),
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
        Some(request_timeout),
    );
    tokio::pin!(request_future);

    let response = loop {
        tokio::select! {
            result = &mut request_future => break result,
            event = events.recv() => {
                match event {
                    Ok(event) => forward_browser_progress(event, &request_id, progress_tx.as_ref()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "browser worker progress receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }
        }
    };
    loop {
        match events.try_recv() {
            Ok(event) => forward_browser_progress(event, &request_id, progress_tx.as_ref()),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "browser worker progress receiver lagged");
            }
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
        }
    }
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

fn browser_worker_request_timeout(request: &ExternalCliRunRequest) -> Duration {
    let timeout_secs = request
        .adapter_config
        .timeout_secs
        .map(|seconds| seconds.saturating_add(REQUEST_TIMEOUT_HEADROOM_SECS))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS)
        .max(DEFAULT_REQUEST_TIMEOUT_SECS);
    Duration::from_secs(timeout_secs)
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
    for key in [
        "BIFROST_E2E",
        "BIFROST_COVERAGE_E2E",
        "BIFROST_BROWSER_WORKER_IDLE_SECS",
        "BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN",
        "BIFROST_CHATGPT_WEB_E2E_MOCK",
        "BIFROST_CHATGPT_WEB_LIVE_E2E",
        "BIFROST_CHATGPT_WEB_E2E_MOCK_PLANNING_FIRST",
        "BIFROST_CHATGPT_WEB_E2E_FAIL_DATES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            spec.env
                .insert(key.to_string(), value.to_string_lossy().into_owned());
        }
    }
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
    use std::io::Write as _;

    use crate::worker_runtime::WorkerFrame;

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

    #[tokio::test]
    async fn progress_forwarding_ignores_unrelated_missing_and_malformed_events() {
        let event =
            |request_id: Option<&str>, event: &str, payload: serde_json::Value| WorkerEvent {
                request_id: request_id.map(str::to_string),
                job_id: None,
                event: event.to_string(),
                payload,
            };
        let (progress_tx, mut progress_rx) = mpsc::channel(1);
        forward_browser_progress(
            event(Some("other"), "progress", serde_json::Value::Null),
            "request",
            Some(&progress_tx),
        );
        forward_browser_progress(
            event(Some("request"), "other", serde_json::Value::Null),
            "request",
            Some(&progress_tx),
        );
        forward_browser_progress(
            event(
                Some("request"),
                "progress",
                serde_json::json!({"bad": true}),
            ),
            "request",
            Some(&progress_tx),
        );
        forward_browser_progress(
            event(Some("request"), "progress", serde_json::Value::Null),
            "request",
            None,
        );
        assert!(progress_rx.try_recv().is_err());
    }

    #[test]
    fn request_cleanup_guard_removes_abandoned_spool() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("request.json");
        std::fs::write(&path, b"request").unwrap();
        drop(RemoveFileOnDrop(path.clone()));
        assert!(!path.exists());
    }

    #[test]
    fn browser_worker_request_timeout_tracks_long_adapter_timeouts() {
        let mut request: ExternalCliRunRequest = serde_json::from_value(serde_json::json!({
            "message": "daily report",
            "operation": "send",
            "params": null,
            "runtime": "external_cli",
            "adapter": "chatgpt_web",
            "adapterConfig": {}
        }))
        .unwrap();

        assert_eq!(
            browser_worker_request_timeout(&request),
            Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)
        );

        request.adapter_config.timeout_secs = Some(60);
        assert_eq!(
            browser_worker_request_timeout(&request),
            Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)
        );

        request.adapter_config.timeout_secs = Some(7_170);
        assert_eq!(
            browser_worker_request_timeout(&request),
            Duration::from_secs(7_170 + REQUEST_TIMEOUT_HEADROOM_SECS)
        );
    }

    #[tokio::test]
    async fn session_controls_and_worker_frame_matrix_are_bounded() {
        assert!(!stop_session_run("   ").await);
        assert!(!stop_session_run("missing-session").await);

        let (context, mut output_rx) = WorkerStdioContext::test_context(WorkerKind::Browser);
        assert!(handle_worker_request(
            "unsupported",
            "browser.unsupported",
            serde_json::Value::Null,
            context.clone(),
        )
        .await
        .unwrap_err()
        .contains("unsupported"));

        for operation in [
            "browser.run",
            "browser.auth_status",
            "browser.open_login",
            "browser.stop_login",
            "browser.ensure_startup_auth_ready",
            "browser.clear_session_conversation",
            "browser.session_conversation_exists",
        ] {
            assert!(handle_worker_request(
                "invalid-payload",
                operation,
                serde_json::json!({"invalid": true}),
                context.clone(),
            )
            .await
            .is_err());
        }

        let exists = handle_worker_request(
            "exists",
            "browser.session_conversation_exists",
            serde_json::json!({"sessionKey": "missing-session"}),
            context.clone(),
        )
        .await
        .unwrap();
        assert_eq!(exists, serde_json::Value::Bool(false));
        let cleared = handle_worker_request(
            "clear",
            "browser.clear_session_conversation",
            serde_json::json!({"sessionKey": "missing-session"}),
            context.clone(),
        )
        .await
        .unwrap();
        assert_eq!(cleared["cleared"], true);

        handle_worker_frame(
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 7,
                payload: serde_json::Value::Null,
            },
            context.clone(),
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected config response")
        };
        assert!(response.ok);
        assert_eq!(response.payload["generation"], 7);

        handle_worker_frame(
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
            context.clone(),
        )
        .await
        .unwrap();
        handle_worker_frame(
            ParentFrame::Cancel {
                request_id: "cancel".to_string(),
                job_id: None,
            },
            context.clone(),
        )
        .await
        .unwrap();
        handle_worker_frame(
            ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            },
            context.clone(),
        )
        .await
        .unwrap();
        handle_worker_frame(
            ParentFrame::Request {
                request: crate::worker_runtime::WorkerRequest {
                    request_id: "request".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "browser.unsupported".to_string(),
                    payload: serde_json::Value::Null,
                },
            },
            context,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected request response")
        };
        assert!(!response.ok);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn browser_run_request_executes_and_spools_mock_result() {
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let request: ExternalCliRunRequest = serde_json::from_value(serde_json::json!({
            "message": "run inside browser worker",
            "adapter": "mock",
            "sessionKey": "browser-worker-session",
            "adapterConfig": {
                "executable": "sh",
                "args": [
                    "-c",
                    "cat >/dev/null; printf '%s\\n' '{\"type\":\"assistant_final\",\"content\":\"browser worker done\"}'"
                ],
                "timeoutSecs": 10
            }
        }))
        .unwrap();
        let request_path = request_dir().join("request-direct.json");
        write_json_file(
            &request_path,
            &BrowserRunFileRequest {
                runs_root: temp.path().join("runs"),
                request,
            },
            BROWSER_REQUEST_MAX_BYTES,
        )
        .unwrap();
        let (context, mut output_rx) = WorkerStdioContext::test_context(WorkerKind::Browser);

        let value = handle_worker_request(
            "direct-run",
            "browser.run",
            serde_json::to_value(BrowserRunReference {
                request_path: request_path.clone(),
            })
            .unwrap(),
            context,
        )
        .await
        .unwrap();

        let reference: BrowserResultReference = serde_json::from_value(value).unwrap();
        let result: ExternalCliRunResult =
            read_json_file(&reference.result_path, BROWSER_RESULT_MAX_BYTES).unwrap();
        assert_eq!(result.response, "browser worker done");
        assert!(!request_path.exists());
        assert!(matches!(
            output_rx.try_recv(),
            Ok(WorkerFrame::Event { .. }) | Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn browser_spool_helpers_cover_success_limits_and_path_validation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("value.json");
        write_json_file(&path, &serde_json::json!({"ok": true}), 128).unwrap();
        let value: serde_json::Value = read_json_file(&path, 128).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(
            validate_runtime_path(&path, &root).unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );

        let outside = temp.path().join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        assert!(validate_runtime_path(&outside, &root)
            .unwrap_err()
            .contains("outside"));
        assert!(validate_runtime_path(&root.join("missing"), &root).is_err());
        assert!(read_json_file::<serde_json::Value>(&root.join("missing"), 128).is_err());

        std::fs::write(&path, b"not-json").unwrap();
        assert!(read_json_file::<serde_json::Value>(&path, 128).is_err());
        std::fs::write(&path, vec![b'x'; 32]).unwrap();
        assert!(read_json_file::<serde_json::Value>(&path, 8)
            .unwrap_err()
            .contains("exceeds limit"));

        let oversized = root.join("oversized.json");
        assert!(write_json_file(&oversized, &"x".repeat(64), 8)
            .unwrap_err()
            .contains("serialize"));
        assert!(!oversized.exists());

        let destination_dir = root.join("destination-dir");
        std::fs::create_dir(&destination_dir).unwrap();
        assert!(
            write_json_file(&destination_dir, &serde_json::json!({}), 128)
                .unwrap_err()
                .contains("rename")
        );

        let mut bytes = Vec::new();
        {
            let mut writer = LimitedJsonWriter {
                inner: &mut bytes,
                written: 0,
                max_bytes: 4,
            };
            assert_eq!(writer.write(b"1234").unwrap(), 4);
            writer.flush().unwrap();
            assert_eq!(
                writer.write(b"5").unwrap_err().kind(),
                std::io::ErrorKind::FileTooLarge
            );
        }
        assert_eq!(bytes, b"1234");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_browser_control_api_round_trips_through_isolated_worker() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"browser","workerInstanceId":"browser-api-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"browser-api-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      case "$line" in
        *'browser.ensure_startup_auth_ready'*)
          payload='{"runnerId":"chatgpt","state":"ready","loggedIn":true,"openedLogin":false,"dryRun":false,"profileDir":"/tmp/profile","statePath":"/tmp/state","message":null}'
          ;;
        *'browser.session_conversation_exists'*)
          payload='true'
          ;;
        *'browser.clear_session_conversation'*)
          payload='{"cleared":true}'
          ;;
        *)
          payload='{"state":"ready","loggedIn":true,"identityComplete":true,"accountCheckOk":true,"accountStatus":200,"cookieCount":2,"capturedHeaderNames":["authorization"],"profileDir":"/tmp/profile","statePath":"/tmp/state","message":null}'
          ;;
      esac
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":%s,"error":null}}\n' "$request_id" "$payload"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"browser-api-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            BROWSER_WORKER_KEY,
            WorkerKind::Browser,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        let supervisor = global_worker_supervisor();
        supervisor.get_or_start(spec).await.unwrap();

        let settings = ExternalCliAgentSettings::default();
        let auth = auth_status(&settings).await;
        let login = open_login(&settings).await;
        let stop = stop_login(&settings).await;
        let startup = ensure_startup_auth_ready("chatgpt", &settings).await;
        let exists = session_conversation_exists("session-1").await;
        let cleared = clear_session_conversation("session-1").await;
        let unregistered = supervisor
            .unregister(BROWSER_WORKER_KEY, Duration::from_secs(1))
            .await;

        assert!(unregistered);
        assert!(auth.unwrap().logged_in);
        assert!(login.unwrap().logged_in);
        assert!(stop.unwrap().logged_in);
        let startup = startup.unwrap();
        assert_eq!(startup.runner_id, "chatgpt");
        assert!(startup.logged_in);
        assert!(exists.unwrap());
        cleared.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_browser_run_reads_result_and_forwards_progress() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let result_path = result_dir().join("result-parent.json");
        let result: ExternalCliRunResult = serde_json::from_value(serde_json::json!({
            "runId": "browser-run",
            "sessionKey": "browser-parent-session",
            "runtime": "chatgpt_web",
            "adapter": "chatgpt-web",
            "status": "failed",
            "exitCode": 1,
            "response": "browser failed safely",
            "startedAt": 1,
            "finishedAt": 2,
            "durationMs": 1,
            "artifacts": {
                "runDir": "", "prompt": "", "commandSnapshot": "",
                "stdout": "", "stderr": "", "normalizedEvents": "", "lastMessage": ""
            },
            "events": []
        }))
        .unwrap();
        write_json_file(&result_path, &result, BROWSER_RESULT_MAX_BYTES).unwrap();
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"browser","workerInstanceId":"browser-run-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"browser-run-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*'browser.run'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      printf '{"type":"event","event":{"requestId":"%s","jobId":"%s","event":"progress","payload":{"eventType":"status","content":"browser working","title":null,"raw":null}}}\n' "$request_id" "$request_id"
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":{"resultPath":"%s"},"error":null}}\n' "$request_id" "$BIFROST_TEST_BROWSER_RESULT_PATH"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"browser-run-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            BROWSER_WORKER_KEY,
            WorkerKind::Browser,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.env.insert(
            "BIFROST_TEST_BROWSER_RESULT_PATH".to_string(),
            result_path.display().to_string(),
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        let supervisor = global_worker_supervisor();
        supervisor.get_or_start(spec).await.unwrap();
        let request: ExternalCliRunRequest = serde_json::from_value(
            serde_json::json!({"message": "hello", "sessionKey": "browser-parent-session"}),
        )
        .unwrap();
        let (progress_tx, mut progress_rx) = mpsc::channel(2);

        let returned =
            run_via_browser_worker(temp.path().join("runs"), request, Some(progress_tx)).await;
        let progress = progress_rx.recv().await;
        let unregistered = supervisor
            .unregister(BROWSER_WORKER_KEY, Duration::from_secs(1))
            .await;

        assert!(unregistered);
        let returned = returned.unwrap();
        assert_eq!(returned.response, "browser failed safely");
        assert_eq!(progress.unwrap().content, "browser working");
        assert!(!result_path.exists());
        assert!(crate::worker_runtime::worker_jobs().iter().any(|job| {
            job.logical_job_id.as_deref() == Some("browser-parent-session")
                && job.status == crate::worker_runtime::WorkerJobStatus::Failed
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_browser_run_marks_stopped_result_cancelled() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let result_path = result_dir().join("result-stopped.json");
        let result: ExternalCliRunResult = serde_json::from_value(serde_json::json!({
            "runId": "browser-stopped-run",
            "sessionKey": "browser-stopped-session",
            "runtime": "chatgpt_web",
            "adapter": "chatgpt-web",
            "status": "stopped",
            "exitCode": null,
            "response": "browser stopped",
            "startedAt": 1,
            "finishedAt": 2,
            "durationMs": 1,
            "artifacts": {
                "runDir": "", "prompt": "", "commandSnapshot": "",
                "stdout": "", "stderr": "", "normalizedEvents": "", "lastMessage": ""
            },
            "events": []
        }))
        .unwrap();
        write_json_file(&result_path, &result, BROWSER_RESULT_MAX_BYTES).unwrap();
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"browser","workerInstanceId":"browser-stopped-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"browser-stopped-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*'browser.run'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":{"resultPath":"%s"},"error":null}}\n' "$request_id" "$BIFROST_TEST_BROWSER_RESULT_PATH"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"browser-stopped-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            BROWSER_WORKER_KEY,
            WorkerKind::Browser,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.env.insert(
            "BIFROST_TEST_BROWSER_RESULT_PATH".to_string(),
            result_path.display().to_string(),
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        let supervisor = global_worker_supervisor();
        supervisor.get_or_start(spec).await.unwrap();
        let request: ExternalCliRunRequest = serde_json::from_value(serde_json::json!({
            "message": "hello",
            "sessionKey": "browser-stopped-session"
        }))
        .unwrap();

        let returned = run_via_browser_worker(temp.path().join("runs"), request, None).await;
        let unregistered = supervisor
            .unregister(BROWSER_WORKER_KEY, Duration::from_secs(1))
            .await;

        assert!(unregistered);
        assert_eq!(
            returned.unwrap().status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped
        );
        assert!(!result_path.exists());
        assert!(crate::worker_runtime::worker_jobs().iter().any(|job| {
            job.logical_job_id.as_deref() == Some("browser-stopped-session")
                && job.status == crate::worker_runtime::WorkerJobStatus::Cancelled
        }));
    }
}
