use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashSet;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::handlers::im_gateway::{
    provider_runtime_status_value, start_provider_event_connection_runtime, ImGatewayService,
};

use super::{
    global_worker_supervisor, run_worker_stdio, ManagedWorker, ParentFrame, WorkerKind,
    WorkerSpawnSpec, WorkerStdioContext,
};

const IM_GATEWAY_WORKER_ENV: &str = "BIFROST_IM_GATEWAY_WORKER";
const IM_GATEWAY_WORKER_KEY: &str = "im_gateway:runtime";
const CONTROLLER_RECONCILE_SECS: u64 = 15;
const WORKER_REQUEST_TIMEOUT_SECS: u64 = 120;

static CONTROLLER_STARTED: AtomicBool = AtomicBool::new(false);
static CONTROLLER_STOPPING: AtomicBool = AtomicBool::new(false);
static CONTROLLER_ENDPOINT: OnceLock<parking_lot::RwLock<Option<ControllerEndpoint>>> =
    OnceLock::new();
static CONTROLLER_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();
static MANUAL_PROVIDER_LEASES: Lazy<DashSet<String>> = Lazy::new(DashSet::new);

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
    controller_notify().notify_waiters();
}

pub fn notify_config_changed() {
    if !is_im_gateway_worker_process() {
        notify_runtime_config_changed();
    }
}

pub(crate) async fn connect_provider(provider_id: &str) -> Result<(), String> {
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
    let Some(worker) = global_worker_supervisor().get(IM_GATEWAY_WORKER_KEY).await else {
        return Ok(None);
    };
    request_provider(&worker, "im.provider_status", provider_id)
        .await
        .map(Some)
}

async fn request_provider(
    worker: &ManagedWorker,
    operation: &str,
    provider_id: &str,
) -> Result<serde_json::Value, String> {
    worker
        .request_with_id(
            uuid::Uuid::new_v4().to_string(),
            Some(provider_id.to_string()),
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
    let worker = if changed {
        global_worker_supervisor()
            .restart(spawn_spec(&endpoint)?)
            .await?
    } else {
        global_worker_supervisor()
            .get_or_start(spawn_spec(&endpoint)?)
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
    global_worker_supervisor()
        .get_or_start(spawn_spec(&endpoint)?)
        .await
}

fn runtime_signature() -> Result<Option<String>, String> {
    let data_dir = bifrost_storage::data_dir();
    let mut providers = crate::im_gateway::ImProviderStore::new(&data_dir)
        .list()
        .into_iter()
        .filter(|provider| {
            provider.enabled
                && provider.event_connection_enabled
                && provider
                    .secret_ref
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        })
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

    if providers.is_empty() && schedules.is_empty() {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&(providers, schedules))
        .map_err(|error| format!("serialize IM Gateway runtime signature: {error}"))?;
    Ok(Some(blake3::hash(&bytes).to_hex().to_string()))
}

fn spawn_spec(endpoint: &ControllerEndpoint) -> Result<WorkerSpawnSpec, String> {
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
                    start_provider_event_connection_runtime(&service, &request.provider_id)
                        .await
                        .map(|()| serde_json::json!({"connected": true}))
                }
                "im.disconnect_provider" => {
                    let request = parse_provider_request(request.payload)?;
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
                    provider_runtime_status_value(&service, &request.provider_id)
                }
                "im.runtime_status" => serde_json::to_value(RuntimeStatus {
                    providers: service.connection_manager.list_statuses(),
                })
                .map_err(|error| format!("serialize IM Gateway runtime status: {error}")),
                other => Err(format!("unsupported IM Gateway worker operation '{other}'")),
            };
            context.response(request_id, result).await;
        }
        ParentFrame::Cancel { job_id, .. } => {
            if let Some(provider_id) = job_id {
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

fn labeled_worker_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    bifrost_core::process_alias_executable(executable, &alias_dir, alias_name)
        .unwrap_or_else(|_| executable.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn manual_provider_leases_are_deduplicated() {
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
}
