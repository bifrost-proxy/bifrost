use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine;
use dashmap::DashSet;
use http_body_util::BodyExt;
use hyper::header::CONTENT_TYPE;
use hyper::{Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::handlers::im_gateway::{
    provider_runtime_status_value, start_provider_event_connection_runtime, ImGatewayService,
};
use crate::handlers::{error_response, full_body, BoxBody};

use super::{
    global_worker_supervisor, run_worker_stdio, ManagedWorker, ParentFrame, WorkerKind,
    WorkerSpawnSpec, WorkerStdioContext,
};

const IM_GATEWAY_WORKER_ENV: &str = "BIFROST_IM_GATEWAY_WORKER";
const IM_GATEWAY_WORKER_KEY: &str = "im_gateway:runtime";
const CONTROLLER_RECONCILE_SECS: u64 = 15;
const WORKER_REQUEST_TIMEOUT_SECS: u64 = 120;
const SEND_REQUEST_MAX_BYTES: u64 = 16 * 1024 * 1024;
const SEND_RESPONSE_MAX_BYTES: usize = 256 * 1024;
const UPLOAD_REQUEST_MAX_BYTES: u64 = 32 * 1024 * 1024;

static CONTROLLER_STARTED: AtomicBool = AtomicBool::new(false);
static CONTROLLER_STOPPING: AtomicBool = AtomicBool::new(false);
static CONTROLLER_ENDPOINT: OnceLock<parking_lot::RwLock<Option<ControllerEndpoint>>> =
    OnceLock::new();
static CONTROLLER_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();
static MANUAL_PROVIDER_LEASES: Lazy<DashSet<String>> = Lazy::new(DashSet::new);
static ACTIVE_PROVIDER_REQUESTS: Lazy<dashmap::DashMap<String, String>> =
    Lazy::new(dashmap::DashMap::new);

#[derive(Debug, Clone)]
struct ControllerEndpoint {
    admin_host: String,
    admin_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderRequest {
    provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageReference {
    request_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadMessageReference {
    body_path: PathBuf,
    provider_id: String,
    kind: String,
    file_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    image_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerHttpResponse {
    status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    body_base64: String,
}

#[derive(Debug)]
struct RemoveFileOnDrop(PathBuf);

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeStatus {
    providers: Vec<(String, crate::im_gateway::types::ConnectionStatus)>,
}

pub(crate) fn is_im_gateway_worker_process() -> bool {
    std::env::var(IM_GATEWAY_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Starts the lightweight main-process controller. It only watches the
/// persisted IM configuration and starts/stops the isolated runtime worker;
/// provider sockets, event loops and scheduler tasks live in the worker.
pub fn start_runtime_controller(admin_host: String, admin_port: u16) {
    if !super::worker_execution_enabled(WorkerKind::ImGateway) {
        stop_runtime_controller();
        tokio::spawn(async {
            global_worker_supervisor()
                .stop_kind(WorkerKind::ImGateway, Duration::from_secs(3))
                .await;
        });
        return;
    }
    *controller_endpoint().write() = Some(ControllerEndpoint {
        admin_host,
        admin_port,
    });
    CONTROLLER_STOPPING.store(false, Ordering::Release);
    if CONTROLLER_STARTED.swap(true, Ordering::AcqRel) {
        notify_runtime_config_changed();
        return;
    }

    tokio::spawn(async move {
        let notify = controller_notify();
        let mut last_signature: Option<String> = None;
        loop {
            if CONTROLLER_STOPPING.load(Ordering::Acquire) {
                break;
            }
            match reconcile_runtime(&mut last_signature).await {
                Ok(()) => {}
                Err(error) => tracing::warn!(
                    error = %error,
                    "IM Gateway worker controller reconciliation failed"
                ),
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
    MANUAL_PROVIDER_LEASES.clear();
    controller_notify().notify_waiters();
}

pub fn notify_runtime_config_changed() {
    if super::worker_execution_enabled(WorkerKind::ImGateway) {
        controller_notify().notify_waiters();
    }
}

pub fn notify_config_changed() {
    if super::worker_execution_enabled(WorkerKind::ImGateway) && !is_im_gateway_worker_process() {
        notify_runtime_config_changed();
    }
}

pub(crate) async fn connect_provider(provider_id: &str) -> Result<(), String> {
    if !super::worker_execution_enabled(WorkerKind::ImGateway) {
        return Err("isolated IM Gateway runtime is disabled by execution mode".to_string());
    }
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err("provider_id cannot be empty".to_string());
    }
    MANUAL_PROVIDER_LEASES.insert(provider_id.to_string());
    notify_runtime_config_changed();
    let worker = ensure_worker_from_controller().await?;
    match request_provider(&worker, "im.connect_provider", provider_id).await {
        Ok(_) => Ok(()),
        Err(error) => {
            MANUAL_PROVIDER_LEASES.remove(provider_id);
            notify_runtime_config_changed();
            Err(error)
        }
    }
}

pub(crate) async fn disconnect_provider(provider_id: &str) -> Result<(), String> {
    if !super::worker_execution_enabled(WorkerKind::ImGateway) {
        return Err("isolated IM Gateway runtime is disabled by execution mode".to_string());
    }
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err("provider_id cannot be empty".to_string());
    }
    let result = if let Some(worker) = global_worker_supervisor().get(IM_GATEWAY_WORKER_KEY).await {
        request_provider(&worker, "im.disconnect_provider", provider_id)
            .await
            .map(|_| ())
    } else {
        Ok(())
    };
    MANUAL_PROVIDER_LEASES.remove(provider_id);
    notify_runtime_config_changed();
    result
}

/// Returns None without launching an idle worker. This keeps read-only status
/// probes from turning an optional runtime into an on-start dependency.
pub(crate) async fn provider_status(
    provider_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    if !super::worker_execution_enabled(WorkerKind::ImGateway) {
        return Ok(None);
    }
    let Some(worker) = global_worker_supervisor().get(IM_GATEWAY_WORKER_KEY).await else {
        return Ok(None);
    };
    request_provider(&worker, "im.provider_status", provider_id)
        .await
        .map(Some)
}

pub(crate) async fn send_message(
    request: crate::handlers::im_gateway::SendMessageRequest,
) -> Response<BoxBody> {
    let worker = match ensure_worker_from_controller().await {
        Ok(worker) => worker,
        Err(error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("IM Gateway worker is unavailable: {error}"),
            )
        }
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let request_path = request_dir().join(format!("send-{request_id}.json"));
    if let Err(error) = write_json_file(&request_path, &request, SEND_REQUEST_MAX_BYTES) {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, &error);
    }
    let _request_cleanup = RemoveFileOnDrop(request_path.clone());
    let payload = match serde_json::to_value(SendMessageReference { request_path }) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialize IM send worker reference: {error}"),
            )
        }
    };
    let response = match worker
        .request_with_id(
            request_id,
            None,
            "im.send_message",
            payload,
            Some(Duration::from_secs(WORKER_REQUEST_TIMEOUT_SECS)),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("IM Gateway worker send failed: {error}"),
            )
        }
    };
    let response = match serde_json::from_value::<WorkerHttpResponse>(response) {
        Ok(response) => response,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("parse IM Gateway worker send response: {error}"),
            )
        }
    };
    worker_http_response(response)
}

pub(crate) async fn upload_message_stream<B>(
    metadata: crate::handlers::im_gateway::UploadMessageMetadata,
    body: B,
    provider_max_bytes: u64,
) -> Response<BoxBody>
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let worker = match ensure_worker_from_controller().await {
        Ok(worker) => worker,
        Err(error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("IM Gateway worker is unavailable: {error}"),
            )
        }
    };
    let max_bytes = provider_max_bytes.min(UPLOAD_REQUEST_MAX_BYTES);
    let request_id = uuid::Uuid::new_v4().to_string();
    let body_path = request_dir().join(format!("upload-{request_id}.bin"));
    let request_cleanup = match spool_upload_body(&body_path, body, max_bytes).await {
        Ok(cleanup) => cleanup,
        Err((status, error)) => return error_response(status, &error),
    };
    let _request_cleanup = request_cleanup;
    let payload = match serde_json::to_value(UploadMessageReference {
        body_path,
        provider_id: metadata.provider_id,
        kind: metadata.kind,
        file_name: metadata.file_name,
        mime_type: metadata.mime_type,
        image_type: metadata.image_type,
    }) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialize IM upload worker reference: {error}"),
            )
        }
    };
    let response = match worker
        .request_with_id(
            request_id,
            None,
            "im.upload_message",
            payload,
            Some(Duration::from_secs(WORKER_REQUEST_TIMEOUT_SECS)),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("IM Gateway worker upload failed: {error}"),
            )
        }
    };
    let response = match serde_json::from_value::<WorkerHttpResponse>(response) {
        Ok(response) => response,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("parse IM Gateway worker upload response: {error}"),
            )
        }
    };
    worker_http_response(response)
}

async fn spool_upload_body<B>(
    body_path: &Path,
    mut body: B,
    max_bytes: u64,
) -> Result<RemoveFileOnDrop, (StatusCode, String)>
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    if let Some(parent) = body_path.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create IM upload spool directory: {error}"),
            ));
        }
    }
    let file = match open_private_file(body_path) {
        Ok(file) => file,
        Err(error) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create IM upload spool: {error}"),
            ))
        }
    };
    let request_cleanup = RemoveFileOnDrop(body_path.to_path_buf());
    let mut file = tokio::fs::File::from_std(file);
    let mut written = 0_u64;
    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("read IM upload body: {error}"),
                ))
            }
        };
        let Some(data) = frame.data_ref() else {
            continue;
        };
        written = written.saturating_add(data.len() as u64);
        if written > max_bytes {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("IM upload exceeds {max_bytes} bytes"),
            ));
        }
        if let Err(error) = file.write_all(data).await {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write IM upload spool: {error}"),
            ));
        }
    }
    if written == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "upload body must not be empty".to_string(),
        ));
    }
    if let Err(error) = file.flush().await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("flush IM upload spool: {error}"),
        ));
    }
    drop(file);
    Ok(request_cleanup)
}

async fn request_provider(
    worker: &ManagedWorker,
    operation: &str,
    provider_id: &str,
) -> Result<serde_json::Value, String> {
    worker
        .request_with_id(
            uuid::Uuid::new_v4().to_string(),
            Some(format!(
                "provider:{}",
                &blake3::hash(provider_id.as_bytes()).to_hex()[..24]
            )),
            operation,
            serde_json::to_value(ProviderRequest {
                provider_id: provider_id.to_string(),
            })
            .map_err(|error| format!("serialize IM provider worker request: {error}"))?,
            Some(Duration::from_secs(WORKER_REQUEST_TIMEOUT_SECS)),
        )
        .await
}

async fn reconcile_runtime(last_signature: &mut Option<String>) -> Result<(), String> {
    let signature = runtime_signature()?;
    let desired = signature.is_some() || !MANUAL_PROVIDER_LEASES.is_empty();
    if !desired {
        global_worker_supervisor()
            .stop(IM_GATEWAY_WORKER_KEY, Duration::from_secs(10))
            .await;
        *last_signature = None;
        return Ok(());
    }

    let endpoint = controller_endpoint()
        .read()
        .clone()
        .ok_or_else(|| "IM Gateway worker controller endpoint is not configured".to_string())?;
    let changed = *last_signature != signature;
    let broker = super::im_broker::ensure_main_broker().await?;
    let worker = if changed {
        global_worker_supervisor()
            .restart(spawn_spec(&endpoint, &broker)?)
            .await?
    } else {
        global_worker_supervisor()
            .get_or_start(spawn_spec(&endpoint, &broker)?)
            .await?
    };
    *last_signature = signature;

    let leases = MANUAL_PROVIDER_LEASES
        .iter()
        .map(|entry| entry.key().clone())
        .collect::<Vec<_>>();
    for provider_id in leases {
        if let Err(error) = request_provider(&worker, "im.connect_provider", &provider_id).await {
            tracing::warn!(
                provider_id,
                error = %error,
                "failed to restore manually connected IM provider after worker start"
            );
        }
    }
    Ok(())
}

async fn ensure_worker_from_controller() -> Result<Arc<ManagedWorker>, String> {
    let endpoint = controller_endpoint()
        .read()
        .clone()
        .ok_or_else(|| "IM Gateway worker controller endpoint is not configured".to_string())?;
    let broker = super::im_broker::ensure_main_broker().await?;
    global_worker_supervisor()
        .get_or_start(spawn_spec(&endpoint, &broker)?)
        .await
}

fn runtime_signature() -> Result<Option<String>, String> {
    let data_dir = bifrost_storage::data_dir();
    let mut providers = crate::im_gateway::ImProviderStore::new(&data_dir)
        .list()
        .into_iter()
        .filter(provider_requires_runtime)
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.id.cmp(&right.id));
    for provider in &mut providers {
        provider.created_at = 0;
        provider.updated_at = 0;
    }

    let mut schedules = crate::im_gateway::ImScheduleStore::new(&data_dir)
        .list()
        .into_iter()
        .filter(|schedule| schedule.enabled)
        .collect::<Vec<_>>();
    schedules.sort_by(|left, right| left.id.cmp(&right.id));
    for schedule in &mut schedules {
        schedule.next_run_at = None;
        schedule.last_run_at = None;
        schedule.created_at = 0;
        schedule.updated_at = 0;
    }

    let mut routes = crate::im_gateway::ImRouteStore::new(&data_dir).list();
    routes.sort_by(|left, right| left.id.cmp(&right.id));
    let mut targets = crate::im_gateway::ImTargetStore::new(&data_dir).list();
    targets.sort_by(|left, right| left.id.cmp(&right.id));

    if providers.is_empty() && schedules.is_empty() {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&(providers, schedules, routes, targets))
        .map_err(|error| format!("serialize IM Gateway runtime signature: {error}"))?;
    Ok(Some(blake3::hash(&bytes).to_hex().to_string()))
}

fn provider_requires_runtime(provider: &crate::im_gateway::types::ImProviderConfig) -> bool {
    provider.enabled
        && provider
            .secret_ref
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn spawn_spec(
    endpoint: &ControllerEndpoint,
    broker: &super::im_broker::BrokerEndpoint,
) -> Result<WorkerSpawnSpec, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve IM Gateway worker executable: {error}"))?;
    let executable = labeled_worker_executable(&executable, "bifrost-im-gateway-worker");
    let data_dir = bifrost_storage::data_dir();
    let mut spec = WorkerSpawnSpec::new(
        IM_GATEWAY_WORKER_KEY,
        WorkerKind::ImGateway,
        executable,
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "im_gateway".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            endpoint.admin_host.clone(),
            "--admin-port".to_string(),
            endpoint.admin_port.to_string(),
        ],
    );
    spec.env
        .insert(IM_GATEWAY_WORKER_ENV.to_string(), "1".to_string());
    super::im_broker::configure_worker_env(&mut spec, broker);
    spec.max_concurrency = 8;
    spec.startup_timeout = Duration::from_secs(20);
    spec.request_timeout = Duration::from_secs(WORKER_REQUEST_TIMEOUT_SECS);
    spec.queue_wait_timeout = Duration::from_secs(15);
    spec.heartbeat_timeout = Duration::from_secs(45);
    spec.stderr_path = Some(runtime_root().join("im-gateway-worker.log"));
    Ok(spec)
}

pub fn run_im_gateway_worker_stdio(_admin_host: &str, admin_port: u16) -> Result<(), String> {
    std::env::set_var(IM_GATEWAY_WORKER_ENV, "1");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-im-gateway-worker")
        .build()
        .map_err(|error| format!("build IM Gateway worker runtime: {error}"))?;
    runtime.block_on(async move {
        let data_dir = bifrost_storage::data_dir();
        let service = Arc::new(ImGatewayService::new_with_agent_proxy_port(
            &data_dir,
            Some(admin_port),
        ));
        service.start_scheduler();
        let auto_connect = service.clone();
        tokio::spawn(async move {
            auto_connect.auto_connect_providers().await;
        });

        run_worker_stdio(
            WorkerKind::ImGateway,
            vec![
                "im.connect_provider".to_string(),
                "im.disconnect_provider".to_string(),
                "im.provider_status".to_string(),
                "im.send_message".to_string(),
                "im.upload_message".to_string(),
                "im.runtime_status".to_string(),
            ],
            move |frame, context| {
                let service = service.clone();
                async move { handle_worker_frame(frame, context, service).await }
            },
        )
        .await
    })
}

async fn handle_worker_frame(
    frame: ParentFrame,
    context: Arc<WorkerStdioContext>,
    service: Arc<ImGatewayService>,
) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => {
            let request_id = request.request_id.clone();
            let result = match request.operation.as_str() {
                "im.connect_provider" => {
                    let request = parse_provider_request(request.payload)?;
                    ACTIVE_PROVIDER_REQUESTS
                        .insert(request_id.clone(), request.provider_id.clone());
                    start_provider_event_connection_runtime(&service, &request.provider_id)
                        .await
                        .map(|()| serde_json::json!({"connected": true}))
                }
                "im.disconnect_provider" => {
                    let request = parse_provider_request(request.payload)?;
                    ACTIVE_PROVIDER_REQUESTS
                        .insert(request_id.clone(), request.provider_id.clone());
                    if service.provider_store.get(&request.provider_id).is_none() {
                        Err(format!("provider '{}' not found", request.provider_id))
                    } else {
                        service
                            .connection_manager
                            .stop_connection(&request.provider_id);
                        Ok(serde_json::json!({"disconnected": true}))
                    }
                }
                "im.provider_status" => {
                    let request = parse_provider_request(request.payload)?;
                    ACTIVE_PROVIDER_REQUESTS
                        .insert(request_id.clone(), request.provider_id.clone());
                    provider_runtime_status_value(&service, &request.provider_id)
                }
                "im.send_message" => {
                    let reference: SendMessageReference =
                        serde_json::from_value(request.payload)
                            .map_err(|error| format!("parse IM send worker reference: {error}"))?;
                    let request_path =
                        validate_runtime_path(&reference.request_path, &request_dir())?;
                    let request = read_json_file::<crate::handlers::im_gateway::SendMessageRequest>(
                        &request_path,
                        SEND_REQUEST_MAX_BYTES,
                    );
                    let _ = std::fs::remove_file(&request_path);
                    let response =
                        crate::handlers::im_gateway::handle_messages_send_body(&service, request?)
                            .await;
                    serde_json::to_value(capture_worker_http_response(response).await?)
                        .map_err(|error| format!("serialize IM send worker response: {error}"))
                }
                "im.upload_message" => {
                    let reference: UploadMessageReference = serde_json::from_value(request.payload)
                        .map_err(|error| format!("parse IM upload worker reference: {error}"))?;
                    let body_path = validate_runtime_path(&reference.body_path, &request_dir())?;
                    let body = read_bytes_file(&body_path, UPLOAD_REQUEST_MAX_BYTES);
                    let _ = std::fs::remove_file(&body_path);
                    let response = crate::handlers::im_gateway::handle_messages_upload_body(
                        &service,
                        crate::handlers::im_gateway::UploadMessageRequest {
                            metadata: crate::handlers::im_gateway::UploadMessageMetadata {
                                provider_id: reference.provider_id,
                                kind: reference.kind,
                                file_name: reference.file_name,
                                mime_type: reference.mime_type,
                                image_type: reference.image_type,
                            },
                            body: body?,
                        },
                    )
                    .await;
                    serde_json::to_value(capture_worker_http_response(response).await?)
                        .map_err(|error| format!("serialize IM upload worker response: {error}"))
                }
                "im.runtime_status" => serde_json::to_value(RuntimeStatus {
                    providers: service.connection_manager.list_statuses(),
                })
                .map_err(|error| format!("serialize IM Gateway runtime status: {error}")),
                other => Err(format!("unsupported IM Gateway worker operation '{other}'")),
            };
            let tracked_request_id = request_id.clone();
            context.response(request_id, result).await;
            ACTIVE_PROVIDER_REQUESTS.remove(&tracked_request_id);
        }
        ParentFrame::Cancel { request_id, .. } => {
            if let Some((_, provider_id)) = ACTIVE_PROVIDER_REQUESTS.remove(&request_id) {
                service.connection_manager.stop_connection(&provider_id);
            }
        }
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
        ParentFrame::Shutdown { .. } => {
            service.connection_manager.stop_all();
            service.scheduler.stop_all();
            crate::im_gateway::external_cli::kill_all_active_runs();
            super::global_worker_supervisor()
                .stop_all(Duration::from_secs(3))
                .await;
        }
        ParentFrame::Ping { .. } => {}
    }
    Ok(())
}

fn parse_provider_request(value: serde_json::Value) -> Result<ProviderRequest, String> {
    serde_json::from_value(value)
        .map_err(|error| format!("parse IM provider worker request: {error}"))
}

async fn capture_worker_http_response(
    response: Response<BoxBody>,
) -> Result<WorkerHttpResponse, String> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| format!("collect IM worker send response: {error}"))?
        .to_bytes();
    if body.len() > SEND_RESPONSE_MAX_BYTES {
        return Err(format!(
            "IM worker send response exceeds {SEND_RESPONSE_MAX_BYTES} bytes"
        ));
    }
    Ok(WorkerHttpResponse {
        status,
        content_type,
        body_base64: base64::engine::general_purpose::STANDARD.encode(body),
    })
}

fn worker_http_response(response: WorkerHttpResponse) -> Response<BoxBody> {
    let status = match StatusCode::from_u16(response.status) {
        Ok(status) => status,
        Err(_) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "IM Gateway worker returned an invalid HTTP status",
            )
        }
    };
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = response.content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    let body = match base64::engine::general_purpose::STANDARD.decode(response.body_base64) {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("IM Gateway worker returned an invalid response body: {error}"),
            )
        }
    };
    if body.len() > SEND_RESPONSE_MAX_BYTES {
        return error_response(
            StatusCode::BAD_GATEWAY,
            "IM Gateway worker returned an oversized response body",
        );
    }
    builder.body(full_body(body)).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build IM Gateway worker response",
        )
    })
}

struct LimitedJsonWriter<W> {
    inner: W,
    written: u64,
    max_bytes: u64,
}

impl<W: Write> Write for LimitedJsonWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(buffer.len() as u64) > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "IM worker request exceeds configured limit",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn open_private_file(path: &Path) -> Result<std::fs::File, String> {
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

#[cfg(test)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("IM worker request path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut file = open_private_file(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
        let _ = std::fs::remove_file(path);
        return Err(format!("write {}: {error}", path.display()));
    }
    Ok(())
}

fn write_json_file<T: Serialize>(path: &Path, value: &T, max_bytes: u64) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("IM worker request path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let file = open_private_file(&temp)?;
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

fn read_json_file<T: serde::de::DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "IM worker request file exceeds limit: {} > {max_bytes}",
            metadata.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn read_bytes_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "IM worker upload file exceeds limit: {} > {max_bytes}",
            metadata.len()
        ));
    }
    std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn validate_runtime_path(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("canonicalize {}: {error}", root.display()))?;
    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("canonicalize {}: {error}", path.display()))?;
    if !path.starts_with(&root) {
        return Err("IM worker request path escapes its runtime directory".to_string());
    }
    Ok(path)
}

fn controller_endpoint() -> &'static parking_lot::RwLock<Option<ControllerEndpoint>> {
    CONTROLLER_ENDPOINT.get_or_init(|| parking_lot::RwLock::new(None))
}

fn controller_notify() -> Arc<Notify> {
    CONTROLLER_NOTIFY
        .get_or_init(|| Arc::new(Notify::new()))
        .clone()
}

fn runtime_root() -> PathBuf {
    bifrost_storage::data_dir().join("runtime/im-gateway-worker")
}

fn request_dir() -> PathBuf {
    runtime_root().join("requests")
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

    static LEASE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn provider_request_round_trip() {
        let request = ProviderRequest {
            provider_id: "provider-main".to_string(),
        };
        let value = serde_json::to_value(&request).unwrap();
        let parsed = parse_provider_request(value).unwrap();
        assert_eq!(parsed.provider_id, request.provider_id);
    }

    #[test]
    fn runtime_status_round_trip() {
        let value = serde_json::to_value(RuntimeStatus {
            providers: Vec::new(),
        })
        .unwrap();
        let parsed: RuntimeStatus = serde_json::from_value(value).unwrap();
        assert!(parsed.providers.is_empty());
    }

    #[test]
    fn outbound_only_enabled_provider_requires_the_im_runtime() {
        let mut provider = crate::im_gateway::types::ImProviderConfig {
            id: "outbound-only".to_string(),
            provider_type: crate::im_gateway::types::ImProviderType::Feishu,
            display_name: "Outbound only".to_string(),
            enabled: true,
            base_url: None,
            app_id: Some("app-id".to_string()),
            secret_ref: Some("secret-ref".to_string()),
            owner_open_id: None,
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 1,
            updated_at: 1,
        };
        assert!(provider_requires_runtime(&provider));
        provider.enabled = false;
        assert!(!provider_requires_runtime(&provider));
        provider.enabled = true;
        provider.secret_ref = Some("  ".to_string());
        assert!(!provider_requires_runtime(&provider));
    }

    #[tokio::test]
    async fn worker_http_response_round_trip_is_bounded_and_validated() {
        let response = worker_http_response(WorkerHttpResponse {
            status: StatusCode::CONFLICT.as_u16(),
            content_type: Some("application/json".to_string()),
            body_base64: base64::engine::general_purpose::STANDARD.encode(b"{\"ok\":false}"),
        });
        let captured = capture_worker_http_response(response).await.unwrap();
        assert_eq!(captured.status, StatusCode::CONFLICT.as_u16());
        assert_eq!(captured.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(captured.body_base64)
                .unwrap(),
            b"{\"ok\":false}"
        );

        assert_eq!(
            worker_http_response(WorkerHttpResponse {
                status: 0,
                content_type: None,
                body_base64: String::new(),
            })
            .status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            worker_http_response(WorkerHttpResponse {
                status: 200,
                content_type: None,
                body_base64: "%%%".to_string(),
            })
            .status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            worker_http_response(WorkerHttpResponse {
                status: 200,
                content_type: None,
                body_base64: base64::engine::general_purpose::STANDARD.encode(vec![
                    b'x';
                    SEND_RESPONSE_MAX_BYTES
                        + 1
                ]),
            })
            .status(),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn im_worker_spool_is_bounded_private_and_confined() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("requests");
        let json_path = root.join("request.json");
        write_json_file(&json_path, &serde_json::json!({"ok": true}), 128).unwrap();
        assert_eq!(
            read_json_file::<serde_json::Value>(&json_path, 128).unwrap(),
            serde_json::json!({"ok": true})
        );
        assert_eq!(
            validate_runtime_path(&json_path, &root).unwrap(),
            std::fs::canonicalize(&json_path).unwrap()
        );

        let raw_path = root.join("upload.bin");
        write_private_file(&raw_path, b"private-upload").unwrap();
        assert_eq!(read_bytes_file(&raw_path, 128).unwrap(), b"private-upload");
        assert!(read_bytes_file(&raw_path, 4)
            .unwrap_err()
            .contains("exceeds"));
        assert!(
            write_json_file(&root.join("large.json"), &"x".repeat(64), 8)
                .unwrap_err()
                .contains("exceeds")
        );

        let outside = temp.path().join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        assert!(validate_runtime_path(&outside, &root)
            .unwrap_err()
            .contains("escapes"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(raw_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn upload_stream_spool_covers_success_empty_limit_and_body_errors() {
        use http_body_util::{Empty, Full, StreamBody};
        use hyper::body::{Bytes, Frame};

        let temp = tempfile::tempdir().unwrap();
        let success = temp.path().join("nested/success.bin");
        let cleanup = spool_upload_body(&success, Full::new(Bytes::from_static(b"upload")), 64)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&success).unwrap(), b"upload");
        drop(cleanup);
        assert!(!success.exists());

        let empty = temp.path().join("empty.bin");
        let (status, error) = spool_upload_body(&empty, Empty::<Bytes>::new(), 64)
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error.contains("must not be empty"));
        assert!(!empty.exists());

        let oversized = temp.path().join("oversized.bin");
        let (status, error) =
            spool_upload_body(&oversized, Full::new(Bytes::from_static(b"too-large")), 2)
                .await
                .unwrap_err();
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.contains("exceeds 2 bytes"));
        assert!(!oversized.exists());

        let failed = temp.path().join("failed.bin");
        let body = StreamBody::new(futures_util::stream::iter(vec![Err::<
            Frame<Bytes>,
            io::Error,
        >(
            io::Error::other("body failed"),
        )]));
        let (status, error) = spool_upload_body(&failed, body, 64).await.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error.contains("body failed"));
        assert!(!failed.exists());

        let trailers_only = temp.path().join("trailers.bin");
        let body = StreamBody::new(futures_util::stream::iter(vec![Ok::<
            Frame<Bytes>,
            io::Error,
        >(Frame::trailers(
            hyper::HeaderMap::new(),
        ))]));
        assert_eq!(
            spool_upload_body(&trailers_only, body, 64)
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );

        let blocked_parent = temp.path().join("blocked-parent");
        std::fs::write(&blocked_parent, b"file").unwrap();
        let (status, error) = spool_upload_body(
            &blocked_parent.join("upload.bin"),
            Full::new(Bytes::from_static(b"upload")),
            64,
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(error.contains("create IM upload spool directory"));

        let directory_path = temp.path().join("directory-as-file");
        std::fs::create_dir(&directory_path).unwrap();
        let (status, error) = spool_upload_body(
            &directory_path,
            Full::new(Bytes::from_static(b"upload")),
            64,
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(error.contains("create IM upload spool"));
    }

    #[test]
    fn manual_provider_leases_are_deduplicated() {
        let _lock = LEASE_TEST_LOCK.blocking_lock();
        let provider_id = format!("test-provider-{}", uuid::Uuid::new_v4());
        MANUAL_PROVIDER_LEASES.insert(provider_id.clone());
        MANUAL_PROVIDER_LEASES.insert(provider_id.clone());
        let matches = MANUAL_PROVIDER_LEASES
            .iter()
            .filter(|entry| entry.key().as_str() == provider_id)
            .count();
        MANUAL_PROVIDER_LEASES.remove(&provider_id);
        assert_eq!(matches, 1);
    }

    #[test]
    fn provider_identity_is_retained_by_bounded_request_id() {
        let request_id = uuid::Uuid::new_v4().to_string();
        let provider_id = "provider-".repeat(64);
        ACTIVE_PROVIDER_REQUESTS.insert(request_id.clone(), provider_id.clone());
        let (_, restored) = ACTIVE_PROVIDER_REQUESTS.remove(&request_id).unwrap();
        assert_eq!(restored, provider_id);
        assert!(request_id.len() <= crate::worker_runtime::protocol::WORKER_MAX_ID_BYTES);
    }

    #[test]
    fn worker_mode_spawn_and_controller_helpers_preserve_process_boundary() {
        std::env::remove_var(IM_GATEWAY_WORKER_ENV);
        assert!(!is_im_gateway_worker_process());
        std::env::set_var(IM_GATEWAY_WORKER_ENV, "true");
        assert!(is_im_gateway_worker_process());

        let endpoint = ControllerEndpoint {
            admin_host: "127.0.0.1".to_string(),
            admin_port: 9876,
        };
        let broker = super::super::im_broker::BrokerEndpoint {
            addr: "127.0.0.1:1234".to_string(),
            token: "token".to_string(),
        };
        let spec = spawn_spec(&endpoint, &broker).unwrap();
        assert_eq!(spec.key, IM_GATEWAY_WORKER_KEY);
        assert_eq!(spec.kind, WorkerKind::ImGateway);
        assert_eq!(spec.max_concurrency, 8);
        assert_eq!(
            spec.env.get(IM_GATEWAY_WORKER_ENV).map(String::as_str),
            Some("1")
        );
        assert_eq!(
            spec.env
                .get(super::super::im_broker::BROKER_TOKEN_ENV)
                .map(String::as_str),
            Some("token")
        );
        assert!(spec
            .args
            .windows(2)
            .any(|pair| pair == ["--admin-port", "9876"]));
        assert!(runtime_root().ends_with("runtime/im-gateway-worker"));
        assert!(Arc::ptr_eq(&controller_notify(), &controller_notify()));
        std::env::remove_var(IM_GATEWAY_WORKER_ENV);
    }

    #[tokio::test]
    async fn worker_dispatch_handles_status_errors_cancel_and_config() {
        let temp = tempfile::tempdir().unwrap();
        let service = Arc::new(ImGatewayService::new(temp.path()));
        let (context, mut output) = WorkerStdioContext::test_context(WorkerKind::ImGateway);

        for (request_id, operation, payload) in [
            ("status", "im.runtime_status", serde_json::json!({})),
            (
                "missing-provider",
                "im.disconnect_provider",
                serde_json::json!({"providerId": "missing"}),
            ),
            ("unsupported", "im.unknown", serde_json::json!({})),
        ] {
            handle_worker_frame(
                ParentFrame::Request {
                    request: WorkerRequest {
                        request_id: request_id.to_string(),
                        job_id: None,
                        deadline_unix_ms: None,
                        operation: operation.to_string(),
                        payload,
                    },
                },
                context.clone(),
                service.clone(),
            )
            .await
            .unwrap();
            let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
                panic!("expected worker response")
            };
            assert_eq!(response.request_id, request_id);
            if request_id == "status" {
                assert!(response.ok);
                assert!(response.payload["providers"].is_array());
            } else {
                assert!(!response.ok);
            }
        }

        let bad = handle_worker_frame(
            ParentFrame::Request {
                request: WorkerRequest {
                    request_id: "bad".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "im.provider_status".to_string(),
                    payload: serde_json::json!({}),
                },
            },
            context.clone(),
            service.clone(),
        )
        .await
        .unwrap_err();
        assert!(bad.contains("parse IM provider worker request"));

        ACTIVE_PROVIDER_REQUESTS.insert("cancel".to_string(), "missing".to_string());
        handle_worker_frame(
            ParentFrame::Cancel {
                request_id: "cancel".to_string(),
                job_id: None,
            },
            context.clone(),
            service.clone(),
        )
        .await
        .unwrap();
        assert!(!ACTIVE_PROVIDER_REQUESTS.contains_key("cancel"));

        handle_worker_frame(
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 9,
                payload: serde_json::Value::Null,
            },
            context.clone(),
            service.clone(),
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected config response")
        };
        assert_eq!(response.payload["generation"], 9);
        assert_eq!(response.payload["restartRequired"], true);

        handle_worker_frame(
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
            context,
            service,
        )
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idle_controller_paths_do_not_launch_workers() {
        let _runtime_guard = crate::worker_runtime::worker_runtime_test_guard_async().await;
        let _lock = LEASE_TEST_LOCK.lock().await;
        MANUAL_PROVIDER_LEASES.clear();
        let _ = runtime_signature().unwrap();
        assert!(provider_status("missing").await.unwrap().is_none());
        assert!(connect_provider("  ").await.unwrap_err().contains("empty"));
        assert!(disconnect_provider("").await.unwrap_err().contains("empty"));
        stop_runtime_controller();
        notify_runtime_config_changed();
        notify_config_changed();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_im_api_round_trips_provider_and_send_through_isolated_worker() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let _lock = LEASE_TEST_LOCK.lock().await;
        MANUAL_PROVIDER_LEASES.clear();
        *controller_endpoint().write() = Some(ControllerEndpoint {
            admin_host: "127.0.0.1".to_string(),
            admin_port: 9876,
        });
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"im_gateway","workerInstanceId":"im-api-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"im-api-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      case "$line" in
        *'im.send_message'*)
          payload='{"status":201,"contentType":"application/json","bodyBase64":"eyJvayI6dHJ1ZX0="}'
          ;;
        *'im.upload_message'*)
          payload='{"status":200,"contentType":"application/json","bodyBase64":"eyJ1cGxvYWRlZCI6dHJ1ZX0="}'
          ;;
        *'im.provider_status'*)
          payload='{"providerId":"provider-test","status":"connected"}'
          ;;
        *)
          payload='{"ok":true}'
          ;;
      esac
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":%s,"error":null}}\n' "$request_id" "$payload"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"im-api-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            IM_GATEWAY_WORKER_KEY,
            WorkerKind::ImGateway,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        let supervisor = global_worker_supervisor();
        supervisor.get_or_start(spec).await.unwrap();

        connect_provider(" provider-test ").await.unwrap();
        assert!(MANUAL_PROVIDER_LEASES.contains("provider-test"));
        let status = provider_status("provider-test").await.unwrap().unwrap();
        assert_eq!(status["status"], "connected");

        let request: crate::handlers::im_gateway::SendMessageRequest =
            serde_json::from_value(serde_json::json!({
                "provider_id": "provider-test",
                "msg_type": "text",
                "text": "hello isolated worker"
            }))
            .unwrap();
        let response = send_message(request).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"{\"ok\":true}");

        let upload = upload_message_stream(
            crate::handlers::im_gateway::UploadMessageMetadata {
                provider_id: "provider-test".to_string(),
                kind: "file".to_string(),
                file_name: "artifact.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                image_type: "message".to_string(),
            },
            http_body_util::Full::new(hyper::body::Bytes::from_static(b"upload-body")),
            1024,
        )
        .await;
        assert_eq!(upload.status(), StatusCode::OK);
        let body = upload.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"{\"uploaded\":true}");

        disconnect_provider("provider-test").await.unwrap();
        assert!(!MANUAL_PROVIDER_LEASES.contains("provider-test"));
        assert!(
            supervisor
                .unregister(IM_GATEWAY_WORKER_KEY, Duration::from_secs(1))
                .await
        );
        *controller_endpoint().write() = None;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_im_api_contains_worker_failures_invalid_payloads_and_missing_controller() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let _lock = LEASE_TEST_LOCK.lock().await;
        MANUAL_PROVIDER_LEASES.clear();
        *controller_endpoint().write() = Some(ControllerEndpoint {
            admin_host: "127.0.0.1".to_string(),
            admin_port: 9876,
        });
        let supervisor = global_worker_supervisor();

        let error_script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"im_gateway","workerInstanceId":"im-error-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"im-error-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      printf '{"type":"response","response":{"requestId":"%s","ok":false,"cancelled":false,"payload":null,"error":"injected IM worker failure"}}\n' "$request_id"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"im-error-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            IM_GATEWAY_WORKER_KEY,
            WorkerKind::ImGateway,
            "/bin/sh",
            vec!["-c".to_string(), error_script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        supervisor.get_or_start(spec).await.unwrap();

        assert!(connect_provider("failing-provider").await.is_err());
        assert!(!MANUAL_PROVIDER_LEASES.contains("failing-provider"));
        let request = || {
            serde_json::from_value::<crate::handlers::im_gateway::SendMessageRequest>(
                serde_json::json!({
                "provider_id": "failing-provider",
                "msg_type": "text",
                "text": "failure"
                }),
            )
            .unwrap()
        };
        let metadata = || crate::handlers::im_gateway::UploadMessageMetadata {
            provider_id: "failing-provider".to_string(),
            kind: "file".to_string(),
            file_name: "failure.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            image_type: "message".to_string(),
        };
        assert_eq!(
            send_message(request()).await.status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            upload_message_stream(
                metadata(),
                http_body_util::Full::new(hyper::body::Bytes::from_static(b"upload")),
                1024,
            )
            .await
            .status(),
            StatusCode::BAD_GATEWAY
        );
        assert!(
            supervisor
                .unregister(IM_GATEWAY_WORKER_KEY, Duration::from_secs(1))
                .await
        );

        MANUAL_PROVIDER_LEASES.insert("no-worker".to_string());
        disconnect_provider("no-worker").await.unwrap();
        assert!(!MANUAL_PROVIDER_LEASES.contains("no-worker"));

        let invalid_script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"im_gateway","workerInstanceId":"im-invalid-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"im-invalid-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":null,"error":null}}\n' "$request_id"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"im-invalid-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            IM_GATEWAY_WORKER_KEY,
            WorkerKind::ImGateway,
            "/bin/sh",
            vec!["-c".to_string(), invalid_script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        supervisor.get_or_start(spec).await.unwrap();
        assert_eq!(
            send_message(request()).await.status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            upload_message_stream(
                metadata(),
                http_body_util::Full::new(hyper::body::Bytes::from_static(b"upload")),
                1024,
            )
            .await
            .status(),
            StatusCode::BAD_GATEWAY
        );
        assert!(
            supervisor
                .unregister(IM_GATEWAY_WORKER_KEY, Duration::from_secs(1))
                .await
        );

        *controller_endpoint().write() = None;
        assert_eq!(
            send_message(request()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            upload_message_stream(
                metadata(),
                http_body_util::Full::new(hyper::body::Bytes::from_static(b"upload")),
                1024,
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
