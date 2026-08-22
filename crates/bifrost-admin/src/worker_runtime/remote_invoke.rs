use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::header::{ACCEPT, CONTENT_DISPOSITION, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Incoming, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::handlers::{error_response, full_body, BoxBody};
use crate::remote_invoke::{Identity, RemoteInvokeConfig, RemoteInvokeWorker};

use super::{
    global_worker_supervisor, run_worker_stdio, ManagedWorker, ParentFrame, WorkerKind,
    WorkerSpawnSpec, WorkerStdioContext,
};

const REMOTE_INVOKE_WORKER_ENV: &str = "BIFROST_REMOTE_INVOKE_WORKER";
const REMOTE_RELAY_URL_ENV: &str = "BIFROST_REMOTE_RELAY_URL";
const REMOTE_SESSION_TOKEN_ENV: &str = "BIFROST_REMOTE_SESSION_TOKEN";
const REMOTE_HTTP_TOKEN_ENV: &str = "BIFROST_REMOTE_WORKER_HTTP_TOKEN";
const REMOTE_WORKER_KEY_PREFIX: &str = "remote_invoke:";
const CONTROLLER_RECONCILE_SECS: u64 = 10;
const MAX_PROXY_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROXY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

static CONTROLLER_STARTED: AtomicBool = AtomicBool::new(false);
static CONTROLLER_STOPPING: AtomicBool = AtomicBool::new(false);
static CONTROLLER_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();
static DESIRED_STATE: OnceLock<parking_lot::RwLock<DesiredRemoteState>> = OnceLock::new();
static ACTIVE_CLIENTS: Lazy<parking_lot::RwLock<HashMap<String, Arc<RemoteWorkerClient>>>> =
    Lazy::new(|| parking_lot::RwLock::new(HashMap::new()));
static PRIMARY_RELAY: Lazy<parking_lot::RwLock<Option<String>>> =
    Lazy::new(|| parking_lot::RwLock::new(None));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInvokeTarget {
    pub provider_id: String,
    pub relay_url: String,
    pub session_token: String,
    #[serde(default)]
    pub allow_missing_session_token: bool,
}

#[derive(Clone, Default)]
struct DesiredRemoteState {
    targets: Vec<RemoteInvokeTarget>,
    admin_host: String,
    admin_port: u16,
    state: Option<crate::state::SharedAdminState>,
}

struct RemoteWorkerClient {
    worker: Arc<ManagedWorker>,
    http_port: u16,
    http_token: String,
    fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteEndpointResponse {
    port: u16,
}

pub(crate) fn is_remote_invoke_worker_process() -> bool {
    std::env::var(REMOTE_INVOKE_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub fn configure_runtime_targets(
    targets: Vec<RemoteInvokeTarget>,
    admin_host: String,
    admin_port: u16,
    state: crate::state::SharedAdminState,
) {
    if !super::worker_execution_enabled(WorkerKind::RemoteInvoke) {
        stop_runtime_controller();
        *desired_state().write() = DesiredRemoteState::default();
        tokio::spawn(async {
            global_worker_supervisor()
                .stop_kind(WorkerKind::RemoteInvoke, Duration::from_secs(3))
                .await;
        });
        return;
    }
    // Preserve the caller's priority order (internal before cloud) while
    // deduplicating relay URLs first-wins. Provider-id sorting changes which
    // target becomes PRIMARY_RELAY and can also leave duplicate URLs apart.
    let mut seen_relays = HashSet::new();
    let normalized = targets
        .into_iter()
        .filter_map(normalize_target)
        .filter(|target| seen_relays.insert(target.relay_url.clone()))
        .collect::<Vec<_>>();
    *desired_state().write() = DesiredRemoteState {
        targets: normalized,
        admin_host,
        admin_port,
        state: Some(state),
    };
    CONTROLLER_STOPPING.store(false, Ordering::Release);
    controller_notify().notify_waiters();
    if CONTROLLER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    tokio::spawn(async move {
        let notify = controller_notify();
        loop {
            if CONTROLLER_STOPPING.load(Ordering::Acquire) {
                break;
            }
            if let Err(error) = reconcile_runtime_targets().await {
                tracing::warn!(error = %error, "Remote Invoke worker reconciliation failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(CONTROLLER_RECONCILE_SECS)) => {}
                _ = notify.notified() => {}
            }
        }
        CONTROLLER_STARTED.store(false, Ordering::Release);
    });
}

pub fn stop_runtime_controller() {
    CONTROLLER_STOPPING.store(true, Ordering::Release);
    controller_notify().notify_waiters();
    ACTIVE_CLIENTS.write().clear();
    *PRIMARY_RELAY.write() = None;
}

pub fn has_active_client() -> bool {
    !ACTIVE_CLIENTS.read().is_empty()
}

pub fn runtime_configured() -> bool {
    super::worker_execution_enabled(WorkerKind::RemoteInvoke)
        && !desired_state().read().targets.is_empty()
}

pub fn admin_proxy_ready() -> bool {
    runtime_configured() && has_active_client()
}

pub async fn proxy_admin_request<B>(req: Request<B>, path: &str) -> Response<BoxBody>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let client = match primary_client() {
        Some(client) => client,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Remote invoke worker is not ready",
            )
        }
    };
    proxy_admin_request_with_client(req, path, client).await
}

async fn proxy_admin_request_with_client<B>(
    req: Request<B>,
    path: &str,
    client: Arc<RemoteWorkerClient>,
) -> Response<BoxBody>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    if !client.worker.is_healthy().await {
        controller_notify().notify_waiters();
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Remote invoke worker is unhealthy",
        );
    }

    let method = req.method().clone();
    // AdminRouter passes `path` after stripping the public /_bifrost prefix.
    // Preserve only the original query string when forwarding to the worker.
    let path_and_query = match req.uri().query() {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    };
    let content_type = req.headers().get(CONTENT_TYPE).cloned();
    let accept = req.headers().get(ACCEPT).cloned();
    let body = match collect_request_body(req).await {
        Ok(body) => body,
        Err(error) => return error_response(StatusCode::PAYLOAD_TOO_LARGE, &error),
    };

    let url = format!("http://127.0.0.1:{}{}", client.http_port, path_and_query);
    let http = match bifrost_core::direct_reqwest_client_builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("create Remote Invoke worker HTTP client: {error}"),
            )
        }
    };
    let mut request = http
        .request(method, url)
        .header("x-bifrost-worker-token", &client.http_token)
        .body(body);
    if let Some(value) = content_type.and_then(|value| value.to_str().ok().map(str::to_string)) {
        request = request.header(CONTENT_TYPE.as_str(), value);
    }
    if let Some(value) = accept.and_then(|value| value.to_str().ok().map(str::to_string)) {
        request = request.header(ACCEPT.as_str(), value);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            controller_notify().notify_waiters();
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Remote Invoke worker request failed: {error}"),
            );
        }
    };
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let response_content_type = response.headers().get(CONTENT_TYPE).cloned();
    let response_content_disposition = response.headers().get(CONTENT_DISPOSITION).cloned();
    let bytes = match collect_response_body(response).await {
        Ok(bytes) => bytes,
        Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error),
    };
    let mut builder = Response::builder().status(status);
    if let Some(value) = response_content_type {
        builder = builder.header(CONTENT_TYPE, value);
    }
    if let Some(value) = response_content_disposition {
        builder = builder.header(CONTENT_DISPOSITION, value);
    }
    builder.body(full_body(bytes)).unwrap_or_else(|_| {
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
    })
}

async fn collect_request_body<B>(req: Request<B>) -> Result<Bytes, String>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut body = req.into_body();
    let mut output = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("read Remote Invoke request body: {error}"))?;
        if let Some(data) = frame.data_ref() {
            if output.len().saturating_add(data.len()) > MAX_PROXY_REQUEST_BYTES {
                return Err(format!(
                    "Remote Invoke request body exceeds {MAX_PROXY_REQUEST_BYTES} bytes"
                ));
            }
            output.extend_from_slice(data);
        }
    }
    Ok(output.freeze())
}

async fn collect_response_body(response: reqwest::Response) -> Result<Bytes, String> {
    let mut stream = response.bytes_stream();
    let mut output = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("read Remote Invoke worker response: {error}"))?;
        if output.len().saturating_add(chunk.len()) > MAX_PROXY_RESPONSE_BYTES {
            return Err(format!(
                "Remote Invoke worker response exceeds {MAX_PROXY_RESPONSE_BYTES} bytes"
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output.freeze())
}

fn primary_client() -> Option<Arc<RemoteWorkerClient>> {
    let primary = PRIMARY_RELAY.read().clone()?;
    ACTIVE_CLIENTS.read().get(&primary).cloned()
}

async fn reconcile_runtime_targets() -> Result<(), String> {
    let desired = desired_state().read().clone();
    let desired_urls = desired
        .targets
        .iter()
        .map(|target| target.relay_url.clone())
        .collect::<HashSet<_>>();
    let stale_urls = ACTIVE_CLIENTS
        .read()
        .keys()
        .filter(|relay_url| !desired_urls.contains(*relay_url))
        .cloned()
        .collect::<Vec<_>>();
    for relay_url in stale_urls {
        let client = {
            let mut clients = ACTIVE_CLIENTS.write();
            clients.remove(&relay_url)
        };
        if let Some(client) = client {
            global_worker_supervisor()
                .unregister(client.worker.key(), Duration::from_secs(10))
                .await;
        }
    }

    let state = desired
        .state
        .clone()
        .ok_or_else(|| "Remote Invoke main broker state is not configured".to_string())?;
    for target in &desired.targets {
        let fingerprint = target_fingerprint(target, &desired.admin_host, desired.admin_port)?;
        let existing = {
            let clients = ACTIVE_CLIENTS.read();
            clients.get(&target.relay_url).cloned()
        };
        let needs_restart = match existing.as_ref() {
            Some(client) => client.fingerprint != fingerprint || !client.worker.is_healthy().await,
            None => true,
        };
        if !needs_restart {
            continue;
        }
        if let Some(client) = existing {
            let removed = {
                let mut clients = ACTIVE_CLIENTS.write();
                clients.remove(&target.relay_url)
            };
            drop(removed);
            global_worker_supervisor()
                .stop(client.worker.key(), Duration::from_secs(10))
                .await;
        }
        match start_target_worker(
            target.clone(),
            &desired.admin_host,
            desired.admin_port,
            state.clone(),
        )
        .await
        {
            Ok(client) => {
                ACTIVE_CLIENTS
                    .write()
                    .insert(target.relay_url.clone(), Arc::new(client));
            }
            Err(error) => tracing::warn!(
                relay_url = %target.relay_url,
                provider_id = %target.provider_id,
                error = %error,
                "failed to start isolated Remote Invoke worker"
            ),
        }
    }

    let primary = desired
        .targets
        .iter()
        .find(|target| ACTIVE_CLIENTS.read().contains_key(&target.relay_url))
        .map(|target| target.relay_url.clone());
    *PRIMARY_RELAY.write() = primary;
    Ok(())
}

async fn start_target_worker(
    target: RemoteInvokeTarget,
    admin_host: &str,
    admin_port: u16,
    state: crate::state::SharedAdminState,
) -> Result<RemoteWorkerClient, String> {
    let http_token = uuid::Uuid::new_v4().to_string();
    let fingerprint = target_fingerprint(&target, admin_host, admin_port)?;
    let broker =
        super::remote_broker::ensure_main_broker(state, admin_host.to_string(), admin_port).await?;
    let spec = spawn_spec(&target, admin_host, admin_port, &http_token, &broker)?;
    let worker = global_worker_supervisor().get_or_start(spec).await?;
    let value = worker
        .request(
            "remote.endpoint",
            serde_json::Value::Null,
            Some(Duration::from_secs(10)),
        )
        .await?;
    let endpoint: RemoteEndpointResponse = serde_json::from_value(value)
        .map_err(|error| format!("parse Remote Invoke worker endpoint: {error}"))?;
    Ok(RemoteWorkerClient {
        worker,
        http_port: endpoint.port,
        http_token,
        fingerprint,
    })
}

fn spawn_spec(
    target: &RemoteInvokeTarget,
    admin_host: &str,
    admin_port: u16,
    http_token: &str,
    broker: &super::remote_broker::BrokerEndpoint,
) -> Result<WorkerSpawnSpec, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve Remote Invoke worker executable: {error}"))?;
    let executable = labeled_worker_executable(&executable, "bifrost-remote-invoke-worker");
    let data_dir = bifrost_storage::data_dir();
    let mut spec = WorkerSpawnSpec::new(
        worker_key(&target.relay_url),
        WorkerKind::RemoteInvoke,
        executable,
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "remote_invoke".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            admin_host.to_string(),
            "--admin-port".to_string(),
            admin_port.to_string(),
        ],
    );
    spec.env
        .insert(REMOTE_INVOKE_WORKER_ENV.to_string(), "1".to_string());
    spec.env
        .insert(REMOTE_RELAY_URL_ENV.to_string(), target.relay_url.clone());
    spec.env.insert(
        REMOTE_SESSION_TOKEN_ENV.to_string(),
        target.session_token.clone(),
    );
    spec.env
        .insert(REMOTE_HTTP_TOKEN_ENV.to_string(), http_token.to_string());
    super::remote_broker::configure_worker_env(&mut spec, broker, &target.relay_url);
    spec.max_concurrency = 16;
    spec.startup_timeout = Duration::from_secs(20);
    spec.request_timeout = Duration::from_secs(120);
    spec.queue_wait_timeout = Duration::from_secs(15);
    spec.heartbeat_timeout = Duration::from_secs(45);
    spec.stderr_path = Some(runtime_root().join(format!("{}.log", short_hash(&target.relay_url))));
    Ok(spec)
}

pub fn run_remote_invoke_worker_stdio(admin_host: &str, admin_port: u16) -> Result<(), String> {
    std::env::set_var(REMOTE_INVOKE_WORKER_ENV, "1");
    let relay_url = std::env::var(REMOTE_RELAY_URL_ENV)
        .map_err(|_| format!("{REMOTE_RELAY_URL_ENV} is required"))?;
    let http_token = std::env::var(REMOTE_HTTP_TOKEN_ENV)
        .map_err(|_| format!("{REMOTE_HTTP_TOKEN_ENV} is required"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-remote-invoke-worker")
        .build()
        .map_err(|error| format!("build Remote Invoke worker runtime: {error}"))?;
    runtime.block_on(async move {
        let identity = Identity::load_or_create(&bifrost_storage::data_dir())
            .map_err(|error| format!("initialize Remote Invoke identity: {error}"))?;
        let minimal_state = Arc::new(crate::state::AdminState::new(admin_port));
        let worker = RemoteInvokeWorker::new(
            RemoteInvokeConfig {
                relay_url,
                ..Default::default()
            },
            identity,
            None,
            minimal_state,
            admin_host,
            admin_port,
        );
        worker.start();
        let (http_port, http_task) = start_worker_http_server(worker.clone(), http_token).await?;

        let result = run_worker_stdio(
            WorkerKind::RemoteInvoke,
            vec![
                "remote.endpoint".to_string(),
                "remote.runtime_status".to_string(),
            ],
            move |frame, context| {
                let worker = worker.clone();
                async move { handle_worker_frame(frame, context, worker, http_port).await }
            },
        )
        .await;
        http_task.abort();
        result
    })
}

async fn start_worker_http_server(
    worker: Arc<RemoteInvokeWorker>,
    http_token: String,
) -> Result<(u16, tokio::task::JoinHandle<()>), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("bind Remote Invoke worker HTTP server: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("read Remote Invoke worker HTTP address: {error}"))?
        .port();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(error = %error, "Remote Invoke worker HTTP accept failed");
                    break;
                }
            };
            let worker = worker.clone();
            let http_token = http_token.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let worker = worker.clone();
                    let http_token = http_token.clone();
                    async move {
                        if req
                            .headers()
                            .get("x-bifrost-worker-token")
                            .and_then(|value| value.to_str().ok())
                            != Some(http_token.as_str())
                        {
                            return Ok::<_, hyper::Error>(error_response(
                                StatusCode::FORBIDDEN,
                                "invalid worker token",
                            ));
                        }
                        let path = req.uri().path().to_string();
                        Ok::<_, hyper::Error>(
                            crate::handlers::remote_invoke::handle_remote_invoke(
                                req,
                                Some(worker),
                                &path,
                            )
                            .await,
                        )
                    }
                });
                let io = TokioIo::new(stream);
                if let Err(error) = http1::Builder::new().serve_connection(io, service).await {
                    tracing::debug!(error = %error, "Remote Invoke worker HTTP connection closed");
                }
            });
        }
    });
    Ok((port, task))
}

async fn handle_worker_frame(
    frame: ParentFrame,
    context: Arc<WorkerStdioContext>,
    worker: Arc<RemoteInvokeWorker>,
    http_port: u16,
) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => {
            let request_id = request.request_id.clone();
            let result = match request.operation.as_str() {
                "remote.endpoint" => {
                    serde_json::to_value(RemoteEndpointResponse { port: http_port })
                        .map_err(|error| format!("serialize Remote Invoke endpoint: {error}"))
                }
                "remote.runtime_status" => Ok(serde_json::json!({
                    "state": format!("{:?}", worker.state()),
                    "relayUrl": worker.relay_client().base_url(),
                    "activeCallIds": worker.active_call_ids(),
                    "pendingPairings": worker.pending_pairings().len(),
                })),
                other => Err(format!(
                    "unsupported Remote Invoke worker operation '{other}'"
                )),
            };
            context.response(request_id, result).await;
        }
        ParentFrame::Cancel { .. } => {}
        ParentFrame::ConfigApply {
            request_id,
            generation,
            ..
        } => {
            context
                .response(
                    request_id,
                    Ok(serde_json::json!({
                        "generation": generation,
                        "restartRequired": true
                    })),
                )
                .await;
        }
        ParentFrame::Shutdown { .. } => worker.stop(),
        ParentFrame::Ping { .. } => {}
    }
    Ok(())
}

fn normalize_target(mut target: RemoteInvokeTarget) -> Option<RemoteInvokeTarget> {
    target.provider_id = target.provider_id.trim().to_string();
    target.relay_url = target.relay_url.trim().trim_end_matches('/').to_string();
    target.session_token = target.session_token.trim().to_string();
    if target.provider_id.is_empty()
        || target.relay_url.is_empty()
        || (target.session_token.is_empty() && !target.allow_missing_session_token)
    {
        None
    } else {
        Some(target)
    }
}

fn target_fingerprint(
    target: &RemoteInvokeTarget,
    admin_host: &str,
    admin_port: u16,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        target,
        admin_host.trim(),
        admin_port,
        env!("CARGO_PKG_VERSION"),
    ))
    .map_err(|error| format!("serialize Remote Invoke worker fingerprint: {error}"))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn worker_key(relay_url: &str) -> String {
    format!("{REMOTE_WORKER_KEY_PREFIX}{}", short_hash(relay_url))
}

fn short_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex()[..16].to_string()
}

fn desired_state() -> &'static parking_lot::RwLock<DesiredRemoteState> {
    DESIRED_STATE.get_or_init(|| parking_lot::RwLock::new(DesiredRemoteState::default()))
}

fn controller_notify() -> Arc<Notify> {
    CONTROLLER_NOTIFY
        .get_or_init(|| Arc::new(Notify::new()))
        .clone()
}

fn runtime_root() -> PathBuf {
    bifrost_storage::data_dir().join("runtime/remote-invoke-worker")
}

fn labeled_worker_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    bifrost_core::process_alias_executable(executable, &alias_dir, alias_name)
        .unwrap_or_else(|_| executable.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_runtime::{WorkerFrame, WorkerRequest};

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn target() -> RemoteInvokeTarget {
        RemoteInvokeTarget {
            provider_id: "provider".to_string(),
            relay_url: "https://relay.example.test".to_string(),
            session_token: "session".to_string(),
            allow_missing_session_token: false,
        }
    }

    fn worker(temp: &std::path::Path) -> Arc<RemoteInvokeWorker> {
        let identity = Identity::load_or_create(temp).unwrap();
        RemoteInvokeWorker::new(
            RemoteInvokeConfig {
                relay_url: "http://127.0.0.1:9".to_string(),
                ..Default::default()
            },
            identity,
            None,
            Arc::new(crate::state::AdminState::new(0)),
            "127.0.0.1",
            0,
        )
    }

    #[test]
    fn target_normalization_rejects_incomplete_values() {
        assert!(normalize_target(RemoteInvokeTarget {
            provider_id: "provider".to_string(),
            relay_url: " ".to_string(),
            session_token: "token".to_string(),
            allow_missing_session_token: false,
        })
        .is_none());
    }

    #[test]
    fn target_normalization_trims_relay_and_token() {
        let target = normalize_target(RemoteInvokeTarget {
            provider_id: " provider ".to_string(),
            relay_url: "https://relay.example.test///".to_string(),
            session_token: " token ".to_string(),
            allow_missing_session_token: false,
        })
        .unwrap();
        assert_eq!(target.provider_id, "provider");
        assert_eq!(target.relay_url, "https://relay.example.test");
        assert_eq!(target.session_token, "token");
    }

    #[test]
    fn worker_key_does_not_expose_relay_url() {
        let key = worker_key("https://secret.example.test/path");
        assert!(key.starts_with(REMOTE_WORKER_KEY_PREFIX));
        assert!(!key.contains("secret.example.test"));
    }

    #[test]
    fn endpoint_round_trip() {
        let value = serde_json::to_value(RemoteEndpointResponse { port: 12345 }).unwrap();
        let parsed: RemoteEndpointResponse = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.port, 12345);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mode_fingerprint_spawn_and_controller_state_are_deterministic() {
        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let _lock = ENV_LOCK.lock().await;
        std::env::remove_var(REMOTE_INVOKE_WORKER_ENV);
        assert!(!is_remote_invoke_worker_process());
        std::env::set_var(REMOTE_INVOKE_WORKER_ENV, "TRUE");
        assert!(is_remote_invoke_worker_process());

        let target = target();
        let first = target_fingerprint(&target, " 127.0.0.1 ", 8080).unwrap();
        let second = target_fingerprint(&target, "127.0.0.1", 8080).unwrap();
        assert_eq!(first, second);
        assert_ne!(
            first,
            target_fingerprint(&target, "127.0.0.1", 8081).unwrap()
        );

        let broker = super::super::remote_broker::BrokerEndpoint::for_test(
            "127.0.0.1:1234",
            &target.relay_url,
            "broker-token",
        );
        let spec = spawn_spec(&target, "127.0.0.1", 8080, "http-token", &broker).unwrap();
        assert_eq!(spec.kind, WorkerKind::RemoteInvoke);
        assert_eq!(spec.max_concurrency, 16);
        assert_eq!(
            spec.env.get(REMOTE_SESSION_TOKEN_ENV).map(String::as_str),
            Some("session")
        );
        assert_eq!(
            spec.env.get(REMOTE_HTTP_TOKEN_ENV).map(String::as_str),
            Some("http-token")
        );
        assert!(spec
            .stderr_path
            .unwrap()
            .ends_with(format!("{}.log", short_hash("https://relay.example.test"))));
        assert!(runtime_root().ends_with("runtime/remote-invoke-worker"));
        assert!(Arc::ptr_eq(&controller_notify(), &controller_notify()));

        stop_runtime_controller();
        assert!(!has_active_client());
        assert!(primary_client().is_none());
        *desired_state().write() = DesiredRemoteState {
            targets: vec![target.clone()],
            ..Default::default()
        };
        assert!(runtime_configured());
        assert!(
            !admin_proxy_ready(),
            "configured runtime without a ready child must use the local admin fallback"
        );
        *desired_state().write() = DesiredRemoteState::default();
        std::env::remove_var(REMOTE_INVOKE_WORKER_ENV);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_runtime_without_ready_child_serves_local_admin_status() {
        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let _lock = ENV_LOCK.lock().await;
        std::env::remove_var(REMOTE_INVOKE_WORKER_ENV);
        stop_runtime_controller();
        *desired_state().write() = DesiredRemoteState {
            targets: vec![target()],
            ..Default::default()
        };
        assert!(runtime_configured());
        assert!(!admin_proxy_ready());

        let temp = tempfile::tempdir().expect("create remote invoke test data dir");
        let local_worker = worker(temp.path());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local admin fallback listener");
        let addr = listener.local_addr().expect("local admin fallback addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .expect("accept local admin fallback request");
            let service = service_fn(move |req: Request<Incoming>| {
                let local_worker = local_worker.clone();
                async move {
                    Ok::<_, hyper::Error>(
                        crate::handlers::remote_invoke::handle_remote_invoke(
                            req,
                            Some(local_worker),
                            "/api/remote-invoke/status",
                        )
                        .await,
                    )
                }
            });
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .expect("serve local admin fallback request");
        });

        let response = bifrost_core::direct_reqwest_client_builder()
            .build()
            .expect("build direct client")
            .get(format!("http://{addr}/_bifrost/api/remote-invoke/status"))
            .send()
            .await
            .expect("request local Remote Invoke status");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["state"],
            "Disconnected"
        );

        server.await.expect("join local admin fallback server");
        *desired_state().write() = DesiredRemoteState::default();
        stop_runtime_controller();
    }

    #[test]
    fn target_normalization_rejects_each_missing_authority_field() {
        for target in [
            RemoteInvokeTarget {
                provider_id: " ".to_string(),
                ..target()
            },
            RemoteInvokeTarget {
                relay_url: String::new(),
                ..target()
            },
            RemoteInvokeTarget {
                session_token: "\t".to_string(),
                ..target()
            },
        ] {
            assert!(normalize_target(target).is_none());
        }
    }

    #[test]
    fn standby_target_allows_only_the_session_token_to_be_missing() {
        let target = normalize_target(RemoteInvokeTarget {
            provider_id: "standby".to_string(),
            relay_url: "https://relay.example.test/".to_string(),
            session_token: "  ".to_string(),
            allow_missing_session_token: true,
        })
        .expect("standby target should keep the admin API available before login");
        assert_eq!(target.relay_url, "https://relay.example.test");
        assert!(target.session_token.is_empty());

        for invalid in [
            RemoteInvokeTarget {
                provider_id: " ".to_string(),
                relay_url: "https://relay.example.test".to_string(),
                session_token: String::new(),
                allow_missing_session_token: true,
            },
            RemoteInvokeTarget {
                provider_id: "standby".to_string(),
                relay_url: " ".to_string(),
                session_token: String::new(),
                allow_missing_session_token: true,
            },
        ] {
            assert!(normalize_target(invalid).is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn empty_reconciliation_requires_main_broker_state() {
        use http_body_util::Full;

        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let _lock = ENV_LOCK.lock().await;
        stop_runtime_controller();
        *desired_state().write() = DesiredRemoteState::default();
        let error = reconcile_runtime_targets().await.unwrap_err();
        assert!(error.contains("main broker state is not configured"));
        assert!(!runtime_configured());
        let response = proxy_admin_request(
            Request::builder()
                .uri("/_bifrost/api/remote-invoke/status")
                .body(Full::new(Bytes::new()))
                .unwrap(),
            "/api/remote-invoke/status",
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let key = format!("remote_invoke:stale-test-{}", uuid::Uuid::new_v4());
        let tail = r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"fake-instance","reason":"shutdown acknowledged"}\n'
      exit 0
      ;;
  esac
done
"#;
        let spec =
            crate::worker_runtime::test_shell_worker_spec(&key, WorkerKind::RemoteInvoke, tail);
        let worker = global_worker_supervisor().get_or_start(spec).await.unwrap();
        let stale_relay = "https://stale-relay.example.test".to_string();
        ACTIVE_CLIENTS.write().insert(
            stale_relay.clone(),
            Arc::new(RemoteWorkerClient {
                worker,
                http_port: 9,
                http_token: "stale-token".to_string(),
                fingerprint: "stale-fingerprint".to_string(),
            }),
        );
        *PRIMARY_RELAY.write() = Some(stale_relay);
        *desired_state().write() = DesiredRemoteState {
            state: Some(Arc::new(crate::state::AdminState::new(9))),
            ..Default::default()
        };
        reconcile_runtime_targets().await.unwrap();
        assert!(ACTIVE_CLIENTS.read().is_empty());
        assert!(PRIMARY_RELAY.read().is_none());
        assert!(global_worker_supervisor().get(&key).await.is_none());
        *desired_state().write() = DesiredRemoteState::default();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_mode_stops_the_isolated_runtime_controller() {
        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let _lock = ENV_LOCK.lock().await;
        stop_runtime_controller();
        std::env::set_var("BIFROST_REMOTE_INVOKE_EXECUTION_MODE", "legacy");
        configure_runtime_targets(
            vec![target()],
            "127.0.0.1".to_string(),
            9,
            Arc::new(crate::state::AdminState::new(9)),
        );
        tokio::task::yield_now().await;
        assert!(!runtime_configured());
        assert!(desired_state().read().targets.is_empty());
        std::env::remove_var("BIFROST_REMOTE_INVOKE_EXECUTION_MODE");
        global_worker_supervisor().resume_kind(WorkerKind::RemoteInvoke);
    }

    #[tokio::test]
    async fn worker_dispatch_and_http_capability_guard_cover_control_surface() {
        let temp = tempfile::tempdir().unwrap();
        let worker = worker(temp.path());
        let (context, mut output) = WorkerStdioContext::test_context(WorkerKind::RemoteInvoke);

        for (request_id, operation) in [
            ("endpoint", "remote.endpoint"),
            ("status", "remote.runtime_status"),
            ("unsupported", "remote.unknown"),
        ] {
            handle_worker_frame(
                ParentFrame::Request {
                    request: WorkerRequest {
                        request_id: request_id.to_string(),
                        job_id: None,
                        deadline_unix_ms: None,
                        operation: operation.to_string(),
                        payload: serde_json::Value::Null,
                    },
                },
                context.clone(),
                worker.clone(),
                43210,
            )
            .await
            .unwrap();
            let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
                panic!("expected worker response")
            };
            assert_eq!(response.request_id, request_id);
            if request_id == "endpoint" {
                assert_eq!(response.payload["port"], 43210);
            } else if request_id == "status" {
                assert_eq!(response.payload["relayUrl"], "http://127.0.0.1:9");
            } else {
                assert!(!response.ok);
            }
        }

        handle_worker_frame(
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 11,
                payload: serde_json::Value::Null,
            },
            context.clone(),
            worker.clone(),
            0,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected config response")
        };
        assert_eq!(response.payload["generation"], 11);

        for frame in [
            ParentFrame::Cancel {
                request_id: "cancel".to_string(),
                job_id: None,
            },
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
        ] {
            handle_worker_frame(frame, context.clone(), worker.clone(), 0)
                .await
                .unwrap();
        }

        let (port, http_task) =
            start_worker_http_server(worker.clone(), "secret-token".to_string())
                .await
                .unwrap();
        let response = bifrost_core::direct_reqwest_client_builder()
            .build()
            .unwrap()
            .get(format!("http://127.0.0.1:{port}/remote/status"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        http_task.abort();

        handle_worker_frame(
            ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            },
            context,
            worker,
            0,
        )
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn main_proxy_forwards_headers_query_and_body_to_healthy_isolated_worker() {
        use http_body_util::Full;

        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"remote_invoke","workerInstanceId":"remote-proxy-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"remote-proxy-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"remote-proxy-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let worker_key = format!("remote-proxy-test-{}", uuid::Uuid::new_v4());
        let mut spec = WorkerSpawnSpec::new(
            &worker_key,
            WorkerKind::RemoteInvoke,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        // The full instrumented suite can pause this test while thousands of
        // other async tests contend for CPU. Keep the fake worker healthy long
        // enough that the assertion below exercises the HTTP failure path,
        // rather than the independent heartbeat-unhealthy path.
        spec.heartbeat_timeout = Duration::from_secs(120);
        let worker = global_worker_supervisor().get_or_start(spec).await.unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(|req: Request<Incoming>| async move {
                assert_eq!(req.uri().path(), "/api/remote-invoke/calls");
                assert_eq!(req.uri().query(), Some("limit=2"));
                assert_eq!(
                    req.headers()
                        .get("x-bifrost-worker-token")
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    "http-token"
                );
                assert_eq!(
                    req.into_body().collect().await.unwrap().to_bytes(),
                    "request-body"
                );
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::CREATED)
                        .header(CONTENT_TYPE, "application/json")
                        .header(CONTENT_DISPOSITION, "attachment; filename=result.json")
                        .body(Full::new(Bytes::from_static(b"{\"forwarded\":true}")))
                        .unwrap(),
                )
            });
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let client = Arc::new(RemoteWorkerClient {
            worker: worker.clone(),
            http_port: port,
            http_token: "http-token".to_string(),
            fingerprint: "fingerprint".to_string(),
        });
        let response = proxy_admin_request_with_client(
            Request::builder()
                .method(hyper::Method::POST)
                .uri("/_bifrost/api/remote-invoke/calls?limit=2")
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json")
                .body(Full::new(Bytes::from_static(b"request-body")))
                .unwrap(),
            "/api/remote-invoke/calls",
            client.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert!(response.headers().contains_key(CONTENT_DISPOSITION));
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "{\"forwarded\":true}"
        );
        server.await.unwrap();

        let failed = proxy_admin_request_with_client(
            Request::builder()
                .method(hyper::Method::GET)
                .uri("/_bifrost/api/remote-invoke/calls")
                .body(Full::new(Bytes::new()))
                .unwrap(),
            "/api/remote-invoke/calls",
            client.clone(),
        )
        .await;
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);

        let oversized = proxy_admin_request_with_client(
            Request::builder()
                .method(hyper::Method::POST)
                .uri("/_bifrost/api/remote-invoke/calls")
                .body(Full::new(Bytes::from(vec![0; MAX_PROXY_REQUEST_BYTES + 1])))
                .unwrap(),
            "/api/remote-invoke/calls",
            client.clone(),
        )
        .await;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

        global_worker_supervisor()
            .unregister(&worker_key, Duration::from_secs(1))
            .await;
        let unhealthy = proxy_admin_request_with_client(
            Request::builder()
                .method(hyper::Method::GET)
                .uri("/_bifrost/api/remote-invoke/calls")
                .body(Full::new(Bytes::new()))
                .unwrap(),
            "/api/remote-invoke/calls",
            client,
        )
        .await;
        assert_eq!(unhealthy.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
