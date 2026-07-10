use bifrost_storage::{
    CollapsedSections, FilterPanelConfig, PinnedFilter, PinnedFilterType, SandboxConfigUpdate,
    SandboxFileConfigUpdate, SandboxLimitsConfigUpdate, SandboxNetConfigUpdate, ServerConfigUpdate,
    TlsConfigUpdate, TrafficConfigUpdate, TrayConfigUpdate, TraySystemStatsItems,
    TraySystemStatsItemsUpdate, UiConfigUpdate, DEFAULT_BREAKPOINT_TIMEOUT_MS,
    DEFAULT_TRAFFIC_MAX_RECORDS, MAX_BREAKPOINT_TIMEOUT_MS, MAX_TRAFFIC_MAX_RECORDS,
    MIN_BREAKPOINT_TIMEOUT_MS, MIN_TRAFFIC_MAX_RECORDS,
};
use bytes::Bytes;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::{error_response, json_response, method_not_allowed, success_response, BoxBody};
use crate::body_store::{BodyStoreConfigUpdate, BodyStoreStats};
use crate::frame_store::FrameStoreStats;
use crate::port_rebind::PortRebindResponse;
use crate::push::SharedPushManager;
use crate::resource_alerts::{build_resource_alerts, ResourceAlerts};
use crate::state::SharedAdminState;
use crate::status_printer::TlsStatusInfo;
use crate::ws_payload_store::{WsPayloadStoreConfigUpdate, WsPayloadStoreStats};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub ip_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,
    pub unsafe_ssl: bool,
    pub disconnect_on_config_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxySettingsResponse {
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub tray: TrayConfig,
    pub port: u16,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    pub enabled: bool,
    pub supported: bool,
    pub system_stats_supported: bool,
    pub show_system_stats: bool,
    pub system_stats_items: TraySystemStatsItemsResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraySystemStatsItemsResponse {
    pub cpu: bool,
    pub memory: bool,
    pub disk: bool,
    pub upload: bool,
    pub download: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub timeout_secs: u64,
    pub http1_max_header_size: usize,
    pub http2_max_header_list_size: usize,
    pub websocket_handshake_max_header_size: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateServerConfigRequest {
    pub port: Option<u16>,
    pub timeout_secs: Option<u64>,
    pub http1_max_header_size: Option<usize>,
    pub http2_max_header_list_size: Option<usize>,
    pub websocket_handshake_max_header_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateServerPortResponse {
    pub expected_port: u16,
    pub actual_port: u16,
}

#[derive(Deserialize)]
pub struct UpdateTlsConfigRequest {
    pub enable_tls_interception: Option<bool>,
    pub intercept_exclude: Option<Vec<String>>,
    pub intercept_include: Option<Vec<String>>,
    pub app_intercept_exclude: Option<Vec<String>>,
    pub app_intercept_include: Option<Vec<String>>,
    pub ip_intercept_exclude: Option<Vec<String>>,
    pub ip_intercept_include: Option<Vec<String>>,
    pub unsafe_ssl: Option<bool>,
    pub disconnect_on_config_change: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateTrayConfigRequest {
    pub enabled: Option<bool>,
    pub show_system_stats: Option<bool>,
    pub system_stats_items: Option<UpdateTraySystemStatsItemsRequest>,
}

#[derive(Deserialize)]
pub struct UpdateTraySystemStatsItemsRequest {
    pub cpu: Option<bool>,
    pub memory: Option<bool>,
    pub disk: Option<bool>,
    pub upload: Option<bool>,
    pub download: Option<bool>,
}

impl From<&TraySystemStatsItems> for TraySystemStatsItemsResponse {
    fn from(items: &TraySystemStatsItems) -> Self {
        Self {
            cpu: items.cpu,
            memory: items.memory,
            disk: items.disk,
            upload: items.upload,
            download: items.download,
        }
    }
}

impl From<UpdateTraySystemStatsItemsRequest> for TraySystemStatsItemsUpdate {
    fn from(request: UpdateTraySystemStatsItemsRequest) -> Self {
        Self {
            cpu: request.cpu,
            memory: request.memory,
            disk: request.disk,
            upload: request.upload,
            download: request.download,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficConfig {
    pub max_records: usize,
    pub max_db_size_bytes: u64,
    pub max_body_memory_size: usize,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub super_performance_mode: bool,
    pub binary_traffic_performance_mode: bool,
    pub inject_bifrost_badge: bool,
    pub file_retention_days: u64,
    pub sse_stream_flush_bytes: usize,
    pub sse_stream_flush_interval_ms: u64,
    pub ws_payload_flush_bytes: usize,
    pub ws_payload_flush_interval_ms: u64,
    pub ws_payload_max_open_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointPerformanceConfig {
    pub timeout_ms: u64,
    pub timeout_min_ms: u64,
    pub timeout_max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfigResponse {
    pub traffic: TrafficConfig,
    pub breakpoint: BreakpointPerformanceConfig,
    pub body_store_stats: Option<BodyStoreStats>,
    pub frame_store_stats: Option<FrameStoreStats>,
    pub ws_payload_store_stats: Option<WsPayloadStoreStats>,
    pub resource_alerts: ResourceAlerts,
}

#[derive(Deserialize)]
pub struct UpdateTrafficConfigRequest {
    pub max_records: Option<usize>,
    pub max_db_size_bytes: Option<u64>,
    pub max_body_memory_size: Option<usize>,
    pub max_body_buffer_size: Option<usize>,
    pub max_body_probe_size: Option<usize>,
    pub super_performance_mode: Option<bool>,
    pub binary_traffic_performance_mode: Option<bool>,
    pub inject_bifrost_badge: Option<bool>,
    pub file_retention_days: Option<u64>,
    pub sse_stream_flush_bytes: Option<usize>,
    pub sse_stream_flush_interval_ms: Option<u64>,
    pub ws_payload_flush_bytes: Option<usize>,
    pub ws_payload_flush_interval_ms: Option<u64>,
    pub ws_payload_max_open_files: Option<usize>,
    pub breakpoint_timeout_ms: Option<u64>,
}

pub async fn handle_config(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/api/config" | "/api/config/" => match method {
            Method::GET => get_proxy_settings(state).await,
            _ => method_not_allowed(),
        },
        "/api/config/tls" | "/api/config/tls/" => match method {
            Method::GET => get_tls_config(state).await,
            Method::PUT => update_tls_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/tray" | "/api/config/tray/" => match method {
            Method::GET => get_tray_config(state).await,
            Method::PUT => update_tray_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/server" | "/api/config/server/" => match method {
            Method::GET => get_server_config(state).await,
            Method::PUT => update_server_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/performance" | "/api/config/performance/" => match method {
            Method::GET => get_performance_config(state).await,
            Method::PUT => update_performance_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/sandbox" | "/api/config/sandbox/" => match method {
            Method::GET => get_sandbox_config(state).await,
            Method::PUT => update_sandbox_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/performance/clear-cache" | "/api/config/performance/clear-cache/" => {
            match method {
                Method::DELETE => clear_body_cache(state).await,
                _ => method_not_allowed(),
            }
        }
        "/api/config/connections/disconnect" | "/api/config/connections/disconnect/" => {
            match method {
                Method::POST => disconnect_by_domain(req, state).await,
                _ => method_not_allowed(),
            }
        }
        "/api/config/connections/disconnect-by-app"
        | "/api/config/connections/disconnect-by-app/" => match method {
            Method::POST => disconnect_by_app(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/connections" | "/api/config/connections/" => match method {
            Method::GET => list_connections(state).await,
            _ => method_not_allowed(),
        },
        "/api/config/ui" | "/api/config/ui/" => match method {
            Method::GET => get_ui_config(state).await,
            Method::PUT => update_ui_config(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/config/ip-tls/pending" | "/api/config/ip-tls/pending/" => match method {
            Method::GET => get_ip_tls_pending(state).await,
            Method::DELETE => clear_ip_tls_pending(state, push_manager).await,
            _ => method_not_allowed(),
        },
        "/api/config/ip-tls/pending/stream" | "/api/config/ip-tls/pending/stream/" => {
            match method {
                Method::GET => ip_tls_pending_stream(state).await,
                _ => method_not_allowed(),
            }
        }
        "/api/config/ip-tls/pending/approve" | "/api/config/ip-tls/pending/approve/" => {
            match method {
                Method::POST => approve_ip_tls(req, state, push_manager).await,
                _ => method_not_allowed(),
            }
        }
        "/api/config/ip-tls/pending/skip" | "/api/config/ip-tls/pending/skip/" => match method {
            Method::POST => skip_ip_tls(req, state, push_manager).await,
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_sandbox_config(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let config = config_manager.config().await;
    json_response(&config.sandbox)
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSandboxConfigRequest {
    #[serde(default)]
    pub file: Option<UpdateSandboxFileConfigRequest>,
    #[serde(default)]
    pub net: Option<UpdateSandboxNetConfigRequest>,
    #[serde(default)]
    pub limits: Option<UpdateSandboxLimitsConfigRequest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSandboxFileConfigRequest {
    pub sandbox_dir: Option<String>,
    pub allowed_dirs: Option<Vec<String>>,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSandboxNetConfigRequest {
    pub enabled: Option<bool>,
    pub allow_private_network: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub max_request_bytes: Option<usize>,
    pub max_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSandboxLimitsConfigRequest {
    pub timeout_ms: Option<u64>,
    pub max_memory_bytes: Option<usize>,
    pub max_decode_input_bytes: Option<usize>,
    pub max_decompress_output_bytes: Option<usize>,
}

async fn update_sandbox_config(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateSandboxConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    // 简单校验
    if let Some(ref file) = request.file {
        if let Some(ref dir) = file.sandbox_dir {
            if dir.trim().is_empty() {
                return error_response(StatusCode::BAD_REQUEST, "sandbox_dir cannot be empty");
            }
        }
        if let Some(ref dirs) = file.allowed_dirs {
            for d in dirs {
                let dd = d.trim();
                if dd.is_empty() {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "allowed_dirs contains empty entry",
                    );
                }
                if !Path::new(dd).is_absolute() {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "allowed_dirs must be absolute paths",
                    );
                }
            }
        }
        if let Some(max_bytes) = file.max_bytes {
            if max_bytes == 0 {
                return error_response(StatusCode::BAD_REQUEST, "file.max_bytes must be > 0");
            }
        }
    }
    if let Some(ref net) = request.net {
        if let Some(timeout_ms) = net.timeout_ms {
            if timeout_ms == 0 {
                return error_response(StatusCode::BAD_REQUEST, "net.timeout_ms must be > 0");
            }
        }
        if let Some(v) = net.max_request_bytes {
            if v == 0 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "net.max_request_bytes must be > 0",
                );
            }
        }
        if let Some(v) = net.max_response_bytes {
            if v == 0 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "net.max_response_bytes must be > 0",
                );
            }
        }
    }
    if let Some(ref limits) = request.limits {
        if let Some(timeout_ms) = limits.timeout_ms {
            if timeout_ms == 0 {
                return error_response(StatusCode::BAD_REQUEST, "limits.timeout_ms must be > 0");
            }
        }
        if let Some(mem) = limits.max_memory_bytes {
            if mem == 0 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "limits.max_memory_bytes must be > 0",
                );
            }
        }
        if let Some(v) = limits.max_decode_input_bytes {
            if v == 0 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "limits.max_decode_input_bytes must be > 0",
                );
            }
        }
        if let Some(v) = limits.max_decompress_output_bytes {
            if v == 0 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "limits.max_decompress_output_bytes must be > 0",
                );
            }
        }
    }

    let update = SandboxConfigUpdate {
        file: request.file.map(|f| SandboxFileConfigUpdate {
            sandbox_dir: f.sandbox_dir,
            allowed_dirs: f.allowed_dirs,
            max_bytes: f.max_bytes,
        }),
        net: request.net.map(|n| SandboxNetConfigUpdate {
            enabled: n.enabled,
            allow_private_network: n.allow_private_network,
            timeout_ms: n.timeout_ms,
            max_request_bytes: n.max_request_bytes,
            max_response_bytes: n.max_response_bytes,
        }),
        limits: request.limits.map(|l| SandboxLimitsConfigUpdate {
            timeout_ms: l.timeout_ms,
            max_memory_bytes: l.max_memory_bytes,
            max_decode_input_bytes: l.max_decode_input_bytes,
            max_decompress_output_bytes: l.max_decompress_output_bytes,
        }),
    };

    match config_manager.update_sandbox_config(update).await {
        Ok(sandbox) => {
            tracing::info!("Sandbox config updated and persisted");
            json_response(&sandbox)
        }
        Err(e) => {
            tracing::error!("Failed to persist sandbox config: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save config: {}", e),
            )
        }
    }
}

async fn get_proxy_settings(state: SharedAdminState) -> Response<BoxBody> {
    let runtime_config = state.runtime_config.read().await;
    let (server_config, tray_config) = if let Some(ref config_manager) = state.config_manager {
        let config = config_manager.config().await;
        (
            ServerConfig {
                timeout_secs: config.server.timeout_secs,
                http1_max_header_size: config.server.http1_max_header_size,
                http2_max_header_list_size: config.server.http2_max_header_list_size,
                websocket_handshake_max_header_size: config
                    .server
                    .websocket_handshake_max_header_size,
            },
            tray_response_from_config(config.tray.enabled, &config.tray),
        )
    } else {
        (
            ServerConfig {
                timeout_secs: 30,
                http1_max_header_size: 64 * 1024,
                http2_max_header_list_size: 256 * 1024,
                websocket_handshake_max_header_size: 64 * 1024,
            },
            TrayConfig {
                enabled: tray_supported(),
                supported: tray_supported(),
                system_stats_supported: tray_system_stats_supported(),
                show_system_stats: tray_system_stats_supported(),
                system_stats_items: tray_system_stats_items_response(
                    &TraySystemStatsItems::default(),
                ),
            },
        )
    };

    let response = ProxySettingsResponse {
        server: server_config,
        tls: TlsConfig {
            enable_tls_interception: runtime_config.enable_tls_interception,
            intercept_exclude: runtime_config.intercept_exclude.clone(),
            intercept_include: runtime_config.intercept_include.clone(),
            app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
            app_intercept_include: runtime_config.app_intercept_include.clone(),
            ip_intercept_exclude: runtime_config.ip_intercept_exclude.clone(),
            ip_intercept_include: runtime_config.ip_intercept_include.clone(),
            unsafe_ssl: runtime_config.unsafe_ssl,
            disconnect_on_config_change: runtime_config.disconnect_on_config_change,
        },
        tray: tray_config,
        port: state.port(),
        host: "127.0.0.1".to_string(),
    };

    json_response(&response)
}

async fn get_tray_config(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let config = config_manager.config().await;
    json_response(&tray_response_from_config(
        config.tray.enabled,
        &config.tray,
    ))
}

async fn update_tray_config(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateTrayConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if request.enabled.is_none()
        && request.show_system_stats.is_none()
        && request.system_stats_items.is_none()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "enabled, show_system_stats, or system_stats_items is required",
        );
    }

    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let system_stats_update_allowed = tray_system_stats_supported()
        || (request.show_system_stats.is_none() && request.system_stats_items.is_none());
    if !system_stats_update_allowed {
        return error_response(
            StatusCode::BAD_REQUEST,
            "tray system stats are not supported on this platform",
        );
    }

    match config_manager
        .update_tray_config(TrayConfigUpdate {
            enabled: request.enabled,
            show_system_stats: request.show_system_stats,
            system_stats_items: request.system_stats_items.map(Into::into),
        })
        .await
    {
        Ok(config) => {
            tracing::info!(
                enabled = config.enabled,
                show_system_stats = config.show_system_stats,
                system_stats_cpu = config.system_stats_items.cpu,
                system_stats_memory = config.system_stats_items.memory,
                system_stats_disk = config.system_stats_items.disk,
                system_stats_upload = config.system_stats_items.upload,
                system_stats_download = config.system_stats_items.download,
                "Tray config updated and persisted"
            );
            if should_request_tray_launch_after_config_update(config.enabled) {
                if state.request_tray_launch() {
                    tracing::info!("Tray launch requested after config enable");
                } else {
                    tracing::debug!("Tray config enabled but no launch callback is registered");
                }
            }
            json_response(&tray_response_from_config(config.enabled, &config))
        }
        Err(e) => {
            tracing::error!("Failed to persist tray config: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save config: {}", e),
            )
        }
    }
}

fn should_request_tray_launch_after_config_update(enabled: bool) -> bool {
    enabled && tray_supported()
}

fn tray_supported() -> bool {
    !cfg!(target_os = "linux")
}

fn tray_system_stats_supported() -> bool {
    cfg!(target_os = "macos")
}

fn tray_system_stats_items_response(items: &TraySystemStatsItems) -> TraySystemStatsItemsResponse {
    if tray_system_stats_supported() {
        items.into()
    } else {
        TraySystemStatsItemsResponse {
            cpu: false,
            memory: false,
            disk: false,
            upload: false,
            download: false,
        }
    }
}

fn tray_response_from_config(enabled: bool, config: &bifrost_storage::TrayConfig) -> TrayConfig {
    TrayConfig {
        enabled,
        supported: tray_supported(),
        system_stats_supported: tray_system_stats_supported(),
        show_system_stats: tray_system_stats_supported() && config.show_system_stats,
        system_stats_items: tray_system_stats_items_response(&config.system_stats_items),
    }
}

async fn get_server_config(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let config = config_manager.config().await;
    json_response(&ServerConfig {
        timeout_secs: config.server.timeout_secs,
        http1_max_header_size: config.server.http1_max_header_size,
        http2_max_header_list_size: config.server.http2_max_header_list_size,
        websocket_handshake_max_header_size: config.server.websocket_handshake_max_header_size,
    })
}

async fn update_server_config(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateServerConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if let Some(port) = request.port {
        if port == 0 {
            return error_response(StatusCode::BAD_REQUEST, "port must be between 1 and 65535");
        }

        let Some(ref manager) = state.port_rebind_manager else {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Port rebind is not available in the current runtime",
            );
        };

        match manager.rebind_port(port).await {
            Ok(PortRebindResponse {
                expected_port,
                actual_port,
            }) => {
                return json_response(&UpdateServerPortResponse {
                    expected_port,
                    actual_port,
                });
            }
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to rebind port: {}", error),
                );
            }
        }
    }

    if let Some(timeout_secs) = request.timeout_secs {
        if timeout_secs == 0 {
            return error_response(StatusCode::BAD_REQUEST, "timeout_secs must be > 0");
        }
    }
    if let Some(v) = request.http1_max_header_size {
        if v == 0 {
            return error_response(StatusCode::BAD_REQUEST, "http1_max_header_size must be > 0");
        }
    }
    if let Some(v) = request.http2_max_header_list_size {
        if v == 0 {
            return error_response(
                StatusCode::BAD_REQUEST,
                "http2_max_header_list_size must be > 0",
            );
        }
        if v > u32::MAX as usize {
            return error_response(
                StatusCode::BAD_REQUEST,
                "http2_max_header_list_size must be <= 4294967295",
            );
        }
    }
    if let Some(v) = request.websocket_handshake_max_header_size {
        if v == 0 {
            return error_response(
                StatusCode::BAD_REQUEST,
                "websocket_handshake_max_header_size must be > 0",
            );
        }
    }

    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let update = ServerConfigUpdate {
        timeout_secs: request.timeout_secs,
        http1_max_header_size: request.http1_max_header_size,
        http2_max_header_list_size: request.http2_max_header_list_size,
        websocket_handshake_max_header_size: request.websocket_handshake_max_header_size,
    };

    match config_manager.update_server_config(update).await {
        Ok(config) => {
            tracing::info!("Server config updated and persisted");
            json_response(&ServerConfig {
                timeout_secs: config.timeout_secs,
                http1_max_header_size: config.http1_max_header_size,
                http2_max_header_list_size: config.http2_max_header_list_size,
                websocket_handshake_max_header_size: config.websocket_handshake_max_header_size,
            })
        }
        Err(e) => {
            tracing::error!("Failed to persist server config: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save config: {}", e),
            )
        }
    }
}

async fn get_tls_config(state: SharedAdminState) -> Response<BoxBody> {
    let runtime_config = state.runtime_config.read().await;

    let tls_config = TlsConfig {
        enable_tls_interception: runtime_config.enable_tls_interception,
        intercept_exclude: runtime_config.intercept_exclude.clone(),
        intercept_include: runtime_config.intercept_include.clone(),
        app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
        app_intercept_include: runtime_config.app_intercept_include.clone(),
        ip_intercept_exclude: runtime_config.ip_intercept_exclude.clone(),
        ip_intercept_include: runtime_config.ip_intercept_include.clone(),
        unsafe_ssl: runtime_config.unsafe_ssl,
        disconnect_on_config_change: runtime_config.disconnect_on_config_change,
    };

    json_response(&tls_config)
}

#[derive(Deserialize)]
pub struct DisconnectByDomainRequest {
    pub domain: String,
}

#[derive(Serialize)]
pub struct DisconnectResponse {
    pub success: bool,
    pub disconnected_count: usize,
    pub message: String,
}

async fn disconnect_by_domain(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: DisconnectByDomainRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let domain = request.domain.trim();
    if domain.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Domain cannot be empty");
    }

    let pattern = domain.to_string();

    let disconnected = state
        .connection_registry
        .disconnect_by_host_pattern(std::slice::from_ref(&pattern));

    let count = disconnected.len();
    tracing::info!(
        "Force disconnect by domain '{}': {} connections closed",
        domain,
        count
    );

    let response = DisconnectResponse {
        success: true,
        disconnected_count: count,
        message: if count > 0 {
            format!("Disconnected {} connection(s) matching '{}'", count, domain)
        } else {
            format!("No active connections found matching '{}'", domain)
        },
    };

    json_response(&response)
}

#[derive(Deserialize)]
pub struct DisconnectByAppRequest {
    pub app: String,
}

async fn disconnect_by_app(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: DisconnectByAppRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let app_name = request.app.trim();
    if app_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "App name cannot be empty");
    }

    let disconnected = state.connection_registry.disconnect_by_app(app_name);

    let count = disconnected.len();
    tracing::info!(
        "Force disconnect by app '{}': {} connections closed",
        app_name,
        count
    );

    let response = DisconnectResponse {
        success: true,
        disconnected_count: count,
        message: if count > 0 {
            format!(
                "Disconnected {} connection(s) for app '{}'",
                count, app_name
            )
        } else {
            format!("No active connections found for app '{}'", app_name)
        },
    };

    json_response(&response)
}

#[derive(Serialize)]
pub struct ConnectionInfoResponse {
    pub req_id: String,
    pub host: String,
    pub port: u16,
    pub intercept_mode: bool,
    pub client_app: Option<String>,
}

#[derive(Serialize)]
pub struct ListConnectionsResponse {
    pub connections: Vec<ConnectionInfoResponse>,
    pub total: usize,
}

async fn list_connections(state: SharedAdminState) -> Response<BoxBody> {
    let connections: Vec<ConnectionInfoResponse> = state
        .connection_registry
        .list_connections_full()
        .into_iter()
        .map(
            |(req_id, host, port, intercept_mode, client_app)| ConnectionInfoResponse {
                req_id,
                host,
                port,
                intercept_mode,
                client_app,
            },
        )
        .collect();

    let total = connections.len();
    let response = ListConnectionsResponse { connections, total };
    json_response(&response)
}

async fn get_performance_config(state: SharedAdminState) -> Response<BoxBody> {
    let body_store_stats = state.body_store.as_ref().map(|bs| bs.read().stats());
    let frame_store_stats = state.frame_store.as_ref().map(|fs| fs.stats());
    let ws_payload_store_stats = state.ws_payload_store.as_ref().map(|ws| ws.stats());
    let resource_alerts =
        build_resource_alerts(body_store_stats.as_ref(), ws_payload_store_stats.as_ref());

    let traffic_config = if let Some(ref config_manager) = state.config_manager {
        let config = config_manager.config().await;
        TrafficConfig {
            max_records: config.traffic.max_records,
            max_db_size_bytes: config.traffic.max_db_size_bytes,
            max_body_memory_size: config.traffic.max_body_memory_size,
            max_body_buffer_size: config.traffic.max_body_buffer_size,
            max_body_probe_size: config.traffic.max_body_probe_size,
            super_performance_mode: config.traffic.super_performance_mode,
            binary_traffic_performance_mode: config.traffic.binary_traffic_performance_mode,
            inject_bifrost_badge: config.traffic.inject_bifrost_badge,
            file_retention_days: config.traffic.file_retention_days,
            sse_stream_flush_bytes: config.traffic.sse_stream_flush_bytes,
            sse_stream_flush_interval_ms: config.traffic.sse_stream_flush_interval_ms,
            ws_payload_flush_bytes: config.traffic.ws_payload_flush_bytes,
            ws_payload_flush_interval_ms: config.traffic.ws_payload_flush_interval_ms,
            ws_payload_max_open_files: config.traffic.ws_payload_max_open_files,
        }
    } else {
        TrafficConfig {
            max_records: DEFAULT_TRAFFIC_MAX_RECORDS,
            max_db_size_bytes: 2 * 1024 * 1024 * 1024,
            max_body_memory_size: 512 * 1024,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            super_performance_mode: state.get_super_performance_mode(),
            binary_traffic_performance_mode: true,
            inject_bifrost_badge: true,
            file_retention_days: 7,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 128,
        }
    };
    let breakpoint_config = BreakpointPerformanceConfig {
        timeout_ms: if let Some(ref config_manager) = state.config_manager {
            let config = config_manager.config().await;
            config.traffic.breakpoint_timeout_ms
        } else {
            DEFAULT_BREAKPOINT_TIMEOUT_MS
        },
        timeout_min_ms: MIN_BREAKPOINT_TIMEOUT_MS,
        timeout_max_ms: MAX_BREAKPOINT_TIMEOUT_MS,
    };

    let response = PerformanceConfigResponse {
        traffic: traffic_config,
        breakpoint: breakpoint_config,
        body_store_stats,
        frame_store_stats,
        ws_payload_store_stats,
        resource_alerts,
    };

    json_response(&response)
}

async fn update_performance_config(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateTrafficConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if let Some(days) = request.file_retention_days {
        if days > 7 {
            return error_response(
                StatusCode::BAD_REQUEST,
                "file_retention_days cannot exceed 7 days",
            );
        }
    }

    if let Some(max_records) = request.max_records {
        if !(MIN_TRAFFIC_MAX_RECORDS..=MAX_TRAFFIC_MAX_RECORDS).contains(&max_records) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!(
                    "max_records must be between {} and {}",
                    MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS
                ),
            );
        }
    }

    if let Some(timeout_ms) = request.breakpoint_timeout_ms {
        if !(MIN_BREAKPOINT_TIMEOUT_MS..=MAX_BREAKPOINT_TIMEOUT_MS).contains(&timeout_ms) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!(
                    "breakpoint_timeout_ms must be between {} and {}",
                    MIN_BREAKPOINT_TIMEOUT_MS, MAX_BREAKPOINT_TIMEOUT_MS
                ),
            );
        }
    }

    if let Some(ref config_manager) = state.config_manager {
        let update = TrafficConfigUpdate {
            max_records: request.max_records,
            max_db_size_bytes: request.max_db_size_bytes,
            max_body_memory_size: request.max_body_memory_size,
            max_body_buffer_size: request.max_body_buffer_size,
            max_body_probe_size: request.max_body_probe_size,
            super_performance_mode: request.super_performance_mode,
            binary_traffic_performance_mode: request.binary_traffic_performance_mode,
            inject_bifrost_badge: request.inject_bifrost_badge,
            file_retention_days: request.file_retention_days,
            sse_stream_flush_bytes: request.sse_stream_flush_bytes,
            sse_stream_flush_interval_ms: request.sse_stream_flush_interval_ms,
            ws_payload_flush_bytes: request.ws_payload_flush_bytes,
            ws_payload_flush_interval_ms: request.ws_payload_flush_interval_ms,
            ws_payload_max_open_files: request.ws_payload_max_open_files,
            breakpoint_timeout_ms: request.breakpoint_timeout_ms,
        };

        if let Err(e) = config_manager.update_traffic_config(update).await {
            tracing::error!("Failed to persist traffic config: {}", e);
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save config: {}", e),
            );
        }
        tracing::info!("Traffic config updated and persisted");
    } else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Config manager not available",
        );
    }

    if let Some(max_records) = request.max_records {
        if let Some(ref traffic_db_store) = state.traffic_db_store {
            traffic_db_store.set_max_records(max_records);
        }
    }

    if let Some(max_db_size_bytes) = request.max_db_size_bytes {
        if let Some(ref traffic_db_store) = state.traffic_db_store {
            traffic_db_store.set_max_db_size_bytes(max_db_size_bytes);
        }
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_update = BodyStoreConfigUpdate {
            max_memory_size: request.max_body_memory_size,
            retention_days: request.file_retention_days,
            stream_flush_bytes: request.sse_stream_flush_bytes,
            stream_flush_interval_ms: request.sse_stream_flush_interval_ms,
        };
        body_store.write().update_config(body_store_update);
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_update = WsPayloadStoreConfigUpdate {
            flush_bytes: request.ws_payload_flush_bytes,
            flush_interval_ms: request.ws_payload_flush_interval_ms,
            max_open_files: request.ws_payload_max_open_files,
            retention_days: request.file_retention_days,
        };
        ws_payload_store.update_config(ws_payload_update);
    }

    if let Some(max_body_buffer_size) = request.max_body_buffer_size {
        state.set_max_body_buffer_size(max_body_buffer_size);
    }

    if let Some(max_body_probe_size) = request.max_body_probe_size {
        state.set_max_body_probe_size(max_body_probe_size);
    }

    if let Some(super_performance_mode) = request.super_performance_mode {
        state.set_super_performance_mode(super_performance_mode);
    }

    if let Some(binary_traffic_performance_mode) = request.binary_traffic_performance_mode {
        state.set_binary_traffic_performance_mode(binary_traffic_performance_mode);
    }

    if let Some(timeout_ms) = request.breakpoint_timeout_ms {
        state.breakpoint_manager.set_timeout_ms(timeout_ms);
    }

    get_performance_config(state).await
}

#[derive(Debug, Clone, Serialize)]
struct ClearCacheResponse {
    body_cache_removed: usize,
    traffic_cache_removed: usize,
    frame_cache_removed: usize,
    ws_payload_cache_removed: usize,
    message: String,
}

async fn clear_body_cache(state: SharedAdminState) -> Response<BoxBody> {
    let mut body_removed = 0usize;
    let mut traffic_removed = 0usize;
    let mut frame_removed = 0usize;
    let mut ws_payload_removed = 0usize;
    let mut errors = Vec::new();

    if let Some(ref body_store) = state.body_store {
        match body_store.write().clear() {
            Ok(count) => {
                body_removed = count;
                tracing::info!("Cleared {} body cache files", count);
            }
            Err(e) => {
                tracing::error!("Failed to clear body cache: {}", e);
                errors.push(format!("body cache: {}", e));
            }
        }
    }

    if let Some(ref traffic_db_store) = state.traffic_db_store {
        // 仅保留活跃连接记录，避免清理导致进行中的连接记录缺失。
        let active_connection_ids = state.connection_monitor.active_connection_ids();
        let before = traffic_db_store.stats().record_count;
        traffic_db_store.clear_with_active_ids(&active_connection_ids);
        let after = traffic_db_store.stats().record_count;
        traffic_removed = before.saturating_sub(after);
        tracing::info!("Cleared traffic db records (active preserved)");
    }

    if let Some(ref frame_store) = state.frame_store {
        match frame_store.clear() {
            Ok(count) => {
                frame_removed = count;
                tracing::info!("Cleared {} frame store files", count);
            }
            Err(e) => {
                tracing::error!("Failed to clear frame store: {}", e);
                errors.push(format!("frame store: {}", e));
            }
        }
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        match ws_payload_store.clear() {
            Ok(count) => {
                ws_payload_removed = count;
                tracing::info!("Cleared {} ws payload files", count);
            }
            Err(e) => {
                tracing::error!("Failed to clear ws payload cache: {}", e);
                errors.push(format!("ws payload cache: {}", e));
            }
        }
    }

    if !errors.is_empty() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Partial failure: {}", errors.join("; ")),
        );
    }

    let response = ClearCacheResponse {
        body_cache_removed: body_removed,
        traffic_cache_removed: traffic_removed,
        frame_cache_removed: frame_removed,
        ws_payload_cache_removed: ws_payload_removed,
        message: format!(
            "Successfully cleared {} body cache files, {} traffic records, {} frame files, and {} ws payload files",
            body_removed, traffic_removed, frame_removed, ws_payload_removed
        ),
    };
    json_response(&response)
}

async fn update_tls_config(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateTlsConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let old_config = {
        let config = state.runtime_config.read().await;
        config.clone()
    };

    let mut affected_patterns: Vec<String> = Vec::new();
    let mut global_changed = false;

    {
        let mut runtime_config = state.runtime_config.write().await;

        if let Some(enable) = request.enable_tls_interception {
            if enable != runtime_config.enable_tls_interception {
                global_changed = true;
                tracing::info!(
                    "TLS config changed: enable_tls_interception: {} -> {}",
                    runtime_config.enable_tls_interception,
                    enable
                );
            }
            runtime_config.enable_tls_interception = enable;
        }

        if let Some(ref exclude) = request.intercept_exclude {
            let added: Vec<_> = exclude
                .iter()
                .filter(|p| !old_config.intercept_exclude.contains(p))
                .cloned()
                .collect();
            let removed: Vec<_> = old_config
                .intercept_exclude
                .iter()
                .filter(|p| !exclude.contains(p))
                .cloned()
                .collect();

            if !added.is_empty() || !removed.is_empty() {
                tracing::info!(
                    "TLS config changed: intercept_exclude added={:?}, removed={:?}",
                    added,
                    removed
                );
                affected_patterns.extend(added);
                affected_patterns.extend(removed);
            }
            runtime_config.intercept_exclude = exclude.clone();
        }

        if let Some(ref include) = request.intercept_include {
            let added: Vec<_> = include
                .iter()
                .filter(|p| !old_config.intercept_include.contains(p))
                .cloned()
                .collect();
            let removed: Vec<_> = old_config
                .intercept_include
                .iter()
                .filter(|p| !include.contains(p))
                .cloned()
                .collect();

            if !added.is_empty() || !removed.is_empty() {
                tracing::info!(
                    "TLS config changed: intercept_include added={:?}, removed={:?}",
                    added,
                    removed
                );
                affected_patterns.extend(added);
                affected_patterns.extend(removed);
            }
            runtime_config.intercept_include = include.clone();
        }

        if let Some(ref app_exclude) = request.app_intercept_exclude {
            if *app_exclude != old_config.app_intercept_exclude {
                tracing::info!(
                    "TLS config changed: app_intercept_exclude {:?} -> {:?}",
                    old_config.app_intercept_exclude,
                    app_exclude
                );
            }
            runtime_config.app_intercept_exclude = app_exclude.clone();
        }

        if let Some(ref app_include) = request.app_intercept_include {
            if *app_include != old_config.app_intercept_include {
                tracing::info!(
                    "TLS config changed: app_intercept_include {:?} -> {:?}",
                    old_config.app_intercept_include,
                    app_include
                );
            }
            runtime_config.app_intercept_include = app_include.clone();
        }

        if let Some(ref ip_exclude) = request.ip_intercept_exclude {
            if *ip_exclude != old_config.ip_intercept_exclude {
                tracing::info!(
                    "TLS config changed: ip_intercept_exclude {:?} -> {:?}",
                    old_config.ip_intercept_exclude,
                    ip_exclude
                );
            }
            runtime_config.ip_intercept_exclude = ip_exclude.clone();
        }

        if let Some(ref ip_include) = request.ip_intercept_include {
            if *ip_include != old_config.ip_intercept_include {
                tracing::info!(
                    "TLS config changed: ip_intercept_include {:?} -> {:?}",
                    old_config.ip_intercept_include,
                    ip_include
                );
            }
            runtime_config.ip_intercept_include = ip_include.clone();
        }

        if let Some(unsafe_ssl) = request.unsafe_ssl {
            runtime_config.unsafe_ssl = unsafe_ssl;
        }

        if let Some(disconnect_on_change) = request.disconnect_on_config_change {
            runtime_config.disconnect_on_config_change = disconnect_on_change;
            tracing::info!(
                "TLS config changed: disconnect_on_config_change = {}",
                disconnect_on_change
            );
        }
    }

    if let Some(ref config_manager) = state.config_manager {
        let update = TlsConfigUpdate {
            enable_interception: request.enable_tls_interception,
            intercept_exclude: request.intercept_exclude.clone(),
            intercept_include: request.intercept_include.clone(),
            app_intercept_exclude: request.app_intercept_exclude.clone(),
            app_intercept_include: request.app_intercept_include.clone(),
            ip_intercept_exclude: request.ip_intercept_exclude.clone(),
            ip_intercept_include: request.ip_intercept_include.clone(),
            unsafe_ssl: request.unsafe_ssl,
            disconnect_on_change: request.disconnect_on_config_change,
        };

        if let Err(e) = config_manager.update_tls_config(update).await {
            tracing::error!("Failed to persist TLS config: {}", e);
        } else {
            tracing::debug!("TLS config persisted to config.toml");
        }
    }

    let should_disconnect = state
        .runtime_config
        .read()
        .await
        .disconnect_on_config_change;

    if should_disconnect {
        if global_changed {
            let new_enabled = state.runtime_config.read().await.enable_tls_interception;
            let disconnected = state
                .connection_registry
                .disconnect_all_with_mode(!new_enabled);
            if !disconnected.is_empty() {
                tracing::info!(
                    "Global TLS switch change disconnected {} connections",
                    disconnected.len()
                );
            }
        } else if !affected_patterns.is_empty() {
            let disconnected = state
                .connection_registry
                .disconnect_by_host_pattern(&affected_patterns);
            if !disconnected.is_empty() {
                tracing::info!(
                    "TLS pattern change disconnected {} connections matching {:?}",
                    disconnected.len(),
                    affected_patterns
                );
            }
        }
    } else if global_changed || !affected_patterns.is_empty() {
        tracing::info!(
            "TLS config changed but disconnect_on_config_change is disabled, {} existing connections will continue with old config",
            state.connection_registry.active_count()
        );
    }

    if global_changed || !affected_patterns.is_empty() {
        let config = state.runtime_config.read().await;
        let status_info =
            TlsStatusInfo::from_runtime_config(&config, state.connection_registry.active_count());
        status_info.log_update_banner();
    }

    let runtime_config = state.runtime_config.read().await;
    let tls_config = TlsConfig {
        enable_tls_interception: runtime_config.enable_tls_interception,
        intercept_exclude: runtime_config.intercept_exclude.clone(),
        intercept_include: runtime_config.intercept_include.clone(),
        app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
        app_intercept_include: runtime_config.app_intercept_include.clone(),
        ip_intercept_exclude: runtime_config.ip_intercept_exclude.clone(),
        ip_intercept_include: runtime_config.ip_intercept_include.clone(),
        unsafe_ssl: runtime_config.unsafe_ssl,
        disconnect_on_config_change: runtime_config.disconnect_on_config_change,
    };

    json_response(&tls_config)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiPinnedFilterType {
    ClientIp,
    ClientApp,
    AccountName,
    Domain,
}

impl From<PinnedFilterType> for UiPinnedFilterType {
    fn from(t: PinnedFilterType) -> Self {
        match t {
            PinnedFilterType::ClientIp => Self::ClientIp,
            PinnedFilterType::ClientApp => Self::ClientApp,
            PinnedFilterType::AccountName => Self::AccountName,
            PinnedFilterType::Domain => Self::Domain,
        }
    }
}

impl From<UiPinnedFilterType> for PinnedFilterType {
    fn from(t: UiPinnedFilterType) -> Self {
        match t {
            UiPinnedFilterType::ClientIp => Self::ClientIp,
            UiPinnedFilterType::ClientApp => Self::ClientApp,
            UiPinnedFilterType::AccountName => Self::AccountName,
            UiPinnedFilterType::Domain => Self::Domain,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPinnedFilter {
    pub id: String,
    #[serde(rename = "type")]
    pub filter_type: UiPinnedFilterType,
    pub value: String,
    pub label: String,
}

impl From<PinnedFilter> for UiPinnedFilter {
    fn from(f: PinnedFilter) -> Self {
        Self {
            id: f.id,
            filter_type: f.filter_type.into(),
            value: f.value,
            label: f.label,
        }
    }
}

impl From<UiPinnedFilter> for PinnedFilter {
    fn from(f: UiPinnedFilter) -> Self {
        Self {
            id: f.id,
            filter_type: f.filter_type.into(),
            value: f.value,
            label: f.label,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiCollapsedSections {
    pub pinned: bool,
    #[serde(rename = "clientIp")]
    pub client_ip: bool,
    #[serde(rename = "clientApp")]
    pub client_app: bool,
    #[serde(rename = "accountName", default)]
    pub account_name: bool,
    pub domain: bool,
}

impl From<CollapsedSections> for UiCollapsedSections {
    fn from(s: CollapsedSections) -> Self {
        Self {
            pinned: s.pinned,
            client_ip: s.client_ip,
            client_app: s.client_app,
            account_name: s.account_name,
            domain: s.domain,
        }
    }
}

impl From<UiCollapsedSections> for CollapsedSections {
    fn from(s: UiCollapsedSections) -> Self {
        Self {
            pinned: s.pinned,
            client_ip: s.client_ip,
            client_app: s.client_app,
            account_name: s.account_name,
            domain: s.domain,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiFilterPanelConfig {
    pub collapsed: bool,
    pub width: u32,
    #[serde(rename = "collapsedSections")]
    pub collapsed_sections: UiCollapsedSections,
}

impl From<FilterPanelConfig> for UiFilterPanelConfig {
    fn from(c: FilterPanelConfig) -> Self {
        Self {
            collapsed: c.collapsed,
            width: c.width,
            collapsed_sections: c.collapsed_sections.into(),
        }
    }
}

impl From<UiFilterPanelConfig> for FilterPanelConfig {
    fn from(c: UiFilterPanelConfig) -> Self {
        Self {
            collapsed: c.collapsed,
            width: c.width,
            collapsed_sections: c.collapsed_sections.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfigResponse {
    #[serde(rename = "pinnedFilters")]
    pub pinned_filters: Vec<UiPinnedFilter>,
    #[serde(rename = "filterPanel")]
    pub filter_panel: UiFilterPanelConfig,
    #[serde(rename = "detailPanelCollapsed")]
    pub detail_panel_collapsed: bool,
    #[serde(rename = "rulesSortMode")]
    pub rules_sort_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUiConfigRequest {
    #[serde(rename = "pinnedFilters")]
    pub pinned_filters: Option<Vec<UiPinnedFilter>>,
    #[serde(rename = "filterPanel")]
    pub filter_panel: Option<UiFilterPanelConfig>,
    #[serde(rename = "detailPanelCollapsed")]
    pub detail_panel_collapsed: Option<bool>,
    #[serde(rename = "rulesSortMode")]
    pub rules_sort_mode: Option<String>,
}

async fn get_ui_config(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let ui_config = config_manager.get_ui_config().await;

    let response = UiConfigResponse {
        pinned_filters: ui_config
            .pinned_filters
            .into_iter()
            .map(Into::into)
            .collect(),
        filter_panel: ui_config.filter_panel.into(),
        detail_panel_collapsed: ui_config.detail_panel_collapsed,
        rules_sort_mode: ui_config.rules_sort_mode,
    };

    json_response(&response)
}

async fn update_ui_config(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateUiConfigRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let update = UiConfigUpdate {
        pinned_filters: request
            .pinned_filters
            .map(|filters| filters.into_iter().map(Into::into).collect()),
        filter_panel: request.filter_panel.map(Into::into),
        detail_panel_collapsed: request.detail_panel_collapsed,
        rules_sort_mode: request.rules_sort_mode,
    };

    match config_manager.update_ui_config(update).await {
        Ok(ui_config) => {
            tracing::info!("UI config updated and persisted");
            let response = UiConfigResponse {
                pinned_filters: ui_config
                    .pinned_filters
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                filter_panel: ui_config.filter_panel.into(),
                detail_panel_collapsed: ui_config.detail_panel_collapsed,
                rules_sort_mode: ui_config.rules_sort_mode,
            };
            json_response(&response)
        }
        Err(e) => {
            tracing::error!("Failed to persist UI config: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save config: {}", e),
            )
        }
    }
}

async fn get_ip_tls_pending(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref manager) = state.ip_tls_pending_manager else {
        return json_response(&Vec::<serde_json::Value>::new());
    };
    let pending = manager.get_pending_list();
    json_response(&pending)
}

async fn ip_tls_pending_stream(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref manager) = state.ip_tls_pending_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "IP TLS pending manager not available",
        );
    };
    let receiver = manager.subscribe();

    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(event) => {
            let data = serde_json::to_string(&event).ok()?;
            let sse_data = format!("data: {}\n\n", data);
            Some(sse_data)
        }
        Err(_) => None,
    });

    let body_stream = http_body_util::StreamBody::new(
        stream.map(|s| Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(s)))),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

#[derive(Deserialize)]
struct IpTlsRequest {
    ip: String,
}

async fn approve_ip_tls(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let Some(ref manager) = state.ip_tls_pending_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "IP TLS pending manager not available",
        );
    };

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: IpTlsRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: std::net::IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    if manager.approve(&ip) {
        let Some(ref config_manager) = state.config_manager else {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Config manager not available",
            );
        };

        let mut config = config_manager.config().await;
        let ip_str = ip.to_string();
        if !config.tls.ip_intercept_include.contains(&ip_str) {
            config.tls.ip_intercept_include.push(ip_str.clone());
        }
        config.tls.ip_intercept_exclude.retain(|e| e != &ip_str);

        let update = bifrost_storage::TlsConfigUpdate {
            ip_intercept_include: Some(config.tls.ip_intercept_include.clone()),
            ip_intercept_exclude: Some(config.tls.ip_intercept_exclude.clone()),
            ..Default::default()
        };

        if let Err(e) = config_manager.update_tls_config(update).await {
            tracing::error!("Failed to persist IP TLS config: {}", e);
        }

        {
            let mut runtime_config = state.runtime_config.write().await;
            runtime_config.ip_intercept_include = config.tls.ip_intercept_include;
            runtime_config.ip_intercept_exclude = config.tls.ip_intercept_exclude;
        }

        if let Some(ref pm) = push_manager {
            pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_TLS_CONFIG)
                .await;
            pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_PENDING_IP_TLS)
                .await;
        }

        tracing::info!("Approved TLS interception for IP {} via API", ip);
        success_response(&format!(
            "Approved TLS interception for {} and added to ip_intercept_include",
            ip
        ))
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            &format!("{} not found in pending IP TLS list", ip),
        )
    }
}

async fn skip_ip_tls(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let Some(ref manager) = state.ip_tls_pending_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "IP TLS pending manager not available",
        );
    };

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: IpTlsRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: std::net::IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    if manager.skip(&ip) {
        let Some(ref config_manager) = state.config_manager else {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Config manager not available",
            );
        };

        let mut config = config_manager.config().await;
        let ip_str = ip.to_string();
        if !config.tls.ip_intercept_exclude.contains(&ip_str) {
            config.tls.ip_intercept_exclude.push(ip_str.clone());
        }
        config.tls.ip_intercept_include.retain(|e| e != &ip_str);

        let update = bifrost_storage::TlsConfigUpdate {
            ip_intercept_include: Some(config.tls.ip_intercept_include.clone()),
            ip_intercept_exclude: Some(config.tls.ip_intercept_exclude.clone()),
            ..Default::default()
        };

        if let Err(e) = config_manager.update_tls_config(update).await {
            tracing::error!("Failed to persist IP TLS config: {}", e);
        }

        {
            let mut runtime_config = state.runtime_config.write().await;
            runtime_config.ip_intercept_include = config.tls.ip_intercept_include;
            runtime_config.ip_intercept_exclude = config.tls.ip_intercept_exclude;
        }

        if let Some(ref pm) = push_manager {
            pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_TLS_CONFIG)
                .await;
            pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_PENDING_IP_TLS)
                .await;
        }

        tracing::info!("Skipped TLS interception for IP {} via API", ip);
        success_response(&format!(
            "Skipped TLS interception for {} and added to ip_intercept_exclude",
            ip
        ))
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            &format!("{} not found in pending IP TLS list", ip),
        )
    }
}

async fn clear_ip_tls_pending(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let Some(ref manager) = state.ip_tls_pending_manager else {
        return success_response("No pending IP TLS decisions");
    };
    manager.clear_pending();

    if let Some(ref pm) = push_manager {
        pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_PENDING_IP_TLS)
            .await;
    }

    tracing::info!("Cleared all pending IP TLS decisions via API");
    success_response("Cleared all pending IP TLS decisions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AdminState;
    use http_body_util::BodyExt;

    #[test]
    fn tray_launch_is_requested_only_for_enable_on_supported_platforms() {
        assert_eq!(
            should_request_tray_launch_after_config_update(true),
            !cfg!(target_os = "linux")
        );
        assert!(!should_request_tray_launch_after_config_update(false));
    }

    #[test]
    fn tls_config_round_trips_through_json() {
        let original = TlsConfig {
            enable_tls_interception: true,
            intercept_exclude: vec!["example.com".to_string()],
            intercept_include: vec!["*.test".to_string()],
            app_intercept_exclude: vec!["AppA".to_string()],
            app_intercept_include: vec!["AppB".to_string()],
            ip_intercept_exclude: vec!["127.0.0.1".to_string()],
            ip_intercept_include: vec!["10.0.0.1".to_string()],
            unsafe_ssl: true,
            disconnect_on_config_change: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: TlsConfig = serde_json::from_str(&json).unwrap();

        assert!(decoded.enable_tls_interception);
        assert_eq!(decoded.intercept_exclude, vec!["example.com".to_string()]);
        assert_eq!(decoded.intercept_include, vec!["*.test".to_string()]);
        assert_eq!(decoded.app_intercept_exclude, vec!["AppA".to_string()]);
        assert_eq!(decoded.app_intercept_include, vec!["AppB".to_string()]);
        assert_eq!(decoded.ip_intercept_exclude, vec!["127.0.0.1".to_string()]);
        assert_eq!(decoded.ip_intercept_include, vec!["10.0.0.1".to_string()]);
        assert!(decoded.unsafe_ssl);
        assert!(!decoded.disconnect_on_config_change);
    }

    #[test]
    fn ui_pinned_filter_type_uses_snake_case_in_json() {
        let json = serde_json::to_string(&UiPinnedFilterType::ClientIp).unwrap();
        assert_eq!(json, r#""client_ip""#);

        let decoded: UiPinnedFilterType = serde_json::from_str(r#""client_app""#).unwrap();
        assert!(matches!(decoded, UiPinnedFilterType::ClientApp));
    }

    #[test]
    fn pinned_filter_type_round_trips_between_ui_and_storage() {
        for storage_ty in [
            PinnedFilterType::ClientIp,
            PinnedFilterType::ClientApp,
            PinnedFilterType::AccountName,
            PinnedFilterType::Domain,
        ] {
            match storage_ty {
                PinnedFilterType::ClientIp => {
                    let ui: UiPinnedFilterType = storage_ty.into();
                    assert!(matches!(ui, UiPinnedFilterType::ClientIp));
                    let back: PinnedFilterType = ui.into();
                    assert!(matches!(back, PinnedFilterType::ClientIp));
                }
                PinnedFilterType::ClientApp => {
                    let ui: UiPinnedFilterType = storage_ty.into();
                    assert!(matches!(ui, UiPinnedFilterType::ClientApp));
                    let back: PinnedFilterType = ui.into();
                    assert!(matches!(back, PinnedFilterType::ClientApp));
                }
                PinnedFilterType::AccountName => {
                    let ui: UiPinnedFilterType = storage_ty.into();
                    assert!(matches!(ui, UiPinnedFilterType::AccountName));
                    let back: PinnedFilterType = ui.into();
                    assert!(matches!(back, PinnedFilterType::AccountName));
                }
                PinnedFilterType::Domain => {
                    let ui: UiPinnedFilterType = storage_ty.into();
                    assert!(matches!(ui, UiPinnedFilterType::Domain));
                    let back: PinnedFilterType = ui.into();
                    assert!(matches!(back, PinnedFilterType::Domain));
                }
            }
        }
    }

    #[test]
    fn ui_pinned_filter_round_trips_via_storage_type() {
        let ui = UiPinnedFilter {
            id: "id-1".to_string(),
            filter_type: UiPinnedFilterType::Domain,
            value: "example.com".to_string(),
            label: "Example".to_string(),
        };

        let storage: PinnedFilter = ui.clone().into();
        let back: UiPinnedFilter = storage.into();

        assert_eq!(back.id, "id-1");
        assert!(matches!(back.filter_type, UiPinnedFilterType::Domain));
        assert_eq!(back.value, "example.com");
        assert_eq!(back.label, "Example");
    }

    #[test]
    fn ui_collapsed_sections_round_trips_via_storage_type() {
        let ui = UiCollapsedSections {
            pinned: true,
            client_ip: false,
            client_app: true,
            account_name: true,
            domain: false,
        };

        let storage: CollapsedSections = ui.clone().into();
        let back: UiCollapsedSections = storage.into();

        assert_eq!(back.pinned, ui.pinned);
        assert_eq!(back.client_ip, ui.client_ip);
        assert_eq!(back.client_app, ui.client_app);
        assert_eq!(back.account_name, ui.account_name);
        assert_eq!(back.domain, ui.domain);
    }

    #[test]
    fn ui_filter_panel_round_trips_via_storage_type() {
        let ui = UiFilterPanelConfig {
            collapsed: true,
            width: 320,
            collapsed_sections: UiCollapsedSections {
                pinned: false,
                client_ip: true,
                client_app: false,
                account_name: true,
                domain: true,
            },
        };

        let storage: FilterPanelConfig = ui.clone().into();
        let back: UiFilterPanelConfig = storage.into();

        assert_eq!(back.collapsed, ui.collapsed);
        assert_eq!(back.width, ui.width);
        assert_eq!(back.collapsed_sections.pinned, ui.collapsed_sections.pinned);
        assert_eq!(
            back.collapsed_sections.client_ip,
            ui.collapsed_sections.client_ip
        );
        assert_eq!(
            back.collapsed_sections.client_app,
            ui.collapsed_sections.client_app
        );
        assert_eq!(
            back.collapsed_sections.account_name,
            ui.collapsed_sections.account_name
        );
        assert_eq!(back.collapsed_sections.domain, ui.collapsed_sections.domain);
    }

    #[test]
    fn ui_config_response_serde_uses_expected_field_names() {
        let response = UiConfigResponse {
            pinned_filters: vec![UiPinnedFilter {
                id: "id-1".to_string(),
                filter_type: UiPinnedFilterType::ClientIp,
                value: "1.2.3.4".to_string(),
                label: "Client IP".to_string(),
            }],
            filter_panel: UiFilterPanelConfig {
                collapsed: false,
                width: 240,
                collapsed_sections: UiCollapsedSections {
                    pinned: true,
                    client_ip: false,
                    client_app: false,
                    account_name: true,
                    domain: true,
                },
            },
            detail_panel_collapsed: true,
            rules_sort_mode: "byName".to_string(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json.get("pinnedFilters").is_some());
        assert!(json.get("filterPanel").is_some());
        assert!(json.get("detailPanelCollapsed").is_some());
        assert!(json.get("rulesSortMode").is_some());

        let pinned = &json["pinnedFilters"][0];
        assert_eq!(pinned["id"], "id-1");
        assert_eq!(pinned["type"], "client_ip");
        assert_eq!(pinned["value"], "1.2.3.4");
        assert_eq!(pinned["label"], "Client IP");

        let collapsed_sections = &json["filterPanel"]["collapsedSections"];
        assert_eq!(collapsed_sections["pinned"], true);
        assert_eq!(collapsed_sections["clientIp"], false);
        assert_eq!(collapsed_sections["clientApp"], false);
        assert_eq!(collapsed_sections["accountName"], true);
        assert_eq!(collapsed_sections["domain"], true);
    }

    #[test]
    fn update_ui_config_request_deserializes_partial_payloads() {
        let json = r#"{ "detailPanelCollapsed": true, "rulesSortMode": "byRule" }"#;
        let req: UpdateUiConfigRequest = serde_json::from_str(json).unwrap();

        assert!(req.pinned_filters.is_none());
        assert!(req.filter_panel.is_none());
        assert_eq!(req.detail_panel_collapsed, Some(true));
        assert_eq!(req.rules_sort_mode.as_deref(), Some("byRule"));
    }

    #[tokio::test]
    async fn performance_config_uses_defaults_without_config_manager() {
        let state = std::sync::Arc::new(AdminState::new(19999));

        let resp = get_performance_config(state).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let cfg: PerformanceConfigResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(cfg.traffic.max_records, DEFAULT_TRAFFIC_MAX_RECORDS);
        assert_eq!(cfg.breakpoint.timeout_ms, DEFAULT_BREAKPOINT_TIMEOUT_MS);
        assert_eq!(cfg.breakpoint.timeout_min_ms, MIN_BREAKPOINT_TIMEOUT_MS);
        assert_eq!(cfg.breakpoint.timeout_max_ms, MAX_BREAKPOINT_TIMEOUT_MS);
    }

    #[test]
    fn update_sandbox_config_request_deserializes_nested_sections() {
        let json = r#"{
            "file": { "sandbox_dir": "/tmp/sandbox" },
            "net": { "enabled": true, "timeout_ms": 1234 },
            "limits": { "timeout_ms": 5678, "max_memory_bytes": 1000 }
        }"#;
        let req: UpdateSandboxConfigRequest = serde_json::from_str(json).unwrap();

        assert_eq!(
            req.file.as_ref().and_then(|f| f.sandbox_dir.as_deref()),
            Some("/tmp/sandbox")
        );
        assert_eq!(req.net.as_ref().and_then(|n| n.enabled), Some(true));
        assert_eq!(req.net.as_ref().and_then(|n| n.timeout_ms), Some(1234));
        assert_eq!(req.limits.as_ref().and_then(|l| l.timeout_ms), Some(5678));
        assert_eq!(
            req.limits.as_ref().and_then(|l| l.max_memory_bytes),
            Some(1000)
        );
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::state::AdminState;
    use crate::test_support::TestAdminState;
    use http_body_util::BodyExt;
    use hyper::StatusCode;

    #[tokio::test]
    async fn get_proxy_settings_uses_defaults_when_config_manager_absent() {
        let state = std::sync::Arc::new(AdminState::new(19999));

        let resp = get_proxy_settings(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: ProxySettingsResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(parsed.server.timeout_secs, 30);
        assert_eq!(parsed.server.http1_max_header_size, 64 * 1024);
        assert_eq!(parsed.server.http2_max_header_list_size, 256 * 1024);
        assert_eq!(parsed.server.websocket_handshake_max_header_size, 64 * 1024);
        assert_eq!(parsed.tray.supported, tray_supported());
        assert_eq!(
            parsed.tray.system_stats_supported,
            tray_system_stats_supported()
        );
        assert_eq!(parsed.tray.show_system_stats, tray_system_stats_supported());
    }

    #[tokio::test]
    async fn get_proxy_settings_reads_values_from_config_manager_when_present() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        let resp = get_proxy_settings(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: ProxySettingsResponse = serde_json::from_slice(&body).unwrap();

        let cfg = harness.config_manager.config().await;
        assert_eq!(parsed.server.timeout_secs, cfg.server.timeout_secs);
        assert_eq!(
            parsed.server.http1_max_header_size,
            cfg.server.http1_max_header_size
        );
        assert_eq!(
            parsed.server.http2_max_header_list_size,
            cfg.server.http2_max_header_list_size
        );
        assert_eq!(
            parsed.server.websocket_handshake_max_header_size,
            cfg.server.websocket_handshake_max_header_size
        );
    }

    #[tokio::test]
    async fn get_tray_config_errors_when_config_manager_missing() {
        let state = std::sync::Arc::new(AdminState::new(19999));
        let resp = get_tray_config(state).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_tray_config_reads_from_config_manager() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        let resp = get_tray_config(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: TrayConfig = serde_json::from_slice(&body).unwrap();

        let cfg = harness.config_manager.config().await;
        assert_eq!(parsed.enabled, cfg.tray.enabled);
        assert_eq!(parsed.supported, tray_supported());
        assert_eq!(parsed.system_stats_supported, tray_system_stats_supported());
        assert_eq!(
            parsed.show_system_stats,
            tray_system_stats_supported() && cfg.tray.show_system_stats
        );
        assert_eq!(
            parsed.system_stats_items.download,
            tray_system_stats_supported() && cfg.tray.system_stats_items.download
        );
    }

    #[tokio::test]
    async fn get_server_config_handles_missing_and_present_config_manager() {
        let state_without = std::sync::Arc::new(AdminState::new(20000));
        let resp = get_server_config(state_without).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let resp = get_server_config(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: ServerConfig = serde_json::from_slice(&body).unwrap();
        let cfg = harness.config_manager.config().await;
        assert_eq!(parsed.timeout_secs, cfg.server.timeout_secs);
        assert_eq!(
            parsed.http1_max_header_size,
            cfg.server.http1_max_header_size
        );
        assert_eq!(
            parsed.http2_max_header_list_size,
            cfg.server.http2_max_header_list_size
        );
    }

    #[tokio::test]
    async fn get_tls_config_reflects_runtime_config_fields() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        // Use default runtime config
        let resp = get_tls_config(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: TlsConfig = serde_json::from_slice(&body).unwrap();

        let runtime = state.runtime_config.read().await.clone();
        assert_eq!(
            parsed.enable_tls_interception,
            runtime.enable_tls_interception
        );
        assert_eq!(parsed.intercept_exclude, runtime.intercept_exclude);
        assert_eq!(parsed.intercept_include, runtime.intercept_include);
        assert_eq!(parsed.app_intercept_exclude, runtime.app_intercept_exclude);
        assert_eq!(parsed.app_intercept_include, runtime.app_intercept_include);
        assert_eq!(parsed.ip_intercept_exclude, runtime.ip_intercept_exclude);
        assert_eq!(parsed.ip_intercept_include, runtime.ip_intercept_include);
        assert_eq!(parsed.unsafe_ssl, runtime.unsafe_ssl);
        assert_eq!(
            parsed.disconnect_on_config_change,
            runtime.disconnect_on_config_change
        );
    }

    #[tokio::test]
    async fn list_connections_returns_empty_list_for_fresh_state() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        let resp = list_connections(state).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["total"].as_u64().unwrap_or_default(), 0);
        assert!(v["connections"]
            .as_array()
            .map(|arr| arr.is_empty())
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn get_performance_config_uses_config_manager_when_present() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        let resp = get_performance_config(state.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: PerformanceConfigResponse = serde_json::from_slice(&body).unwrap();

        let cfg = harness.config_manager.config().await;
        assert_eq!(parsed.traffic.max_records, cfg.traffic.max_records);
        assert_eq!(
            parsed.traffic.max_db_size_bytes,
            cfg.traffic.max_db_size_bytes
        );
        assert_eq!(
            parsed.traffic.file_retention_days,
            cfg.traffic.file_retention_days
        );
    }

    #[tokio::test]
    async fn clear_body_cache_returns_success_response() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        let resp = clear_body_cache(state).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Successfully cleared"));
    }

    #[tokio::test]
    async fn get_ip_tls_pending_returns_empty_list_when_manager_missing() {
        let state = std::sync::Arc::new(AdminState::new(19999));
        let resp = get_ip_tls_pending(state).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let pending: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn ip_tls_pending_stream_returns_service_unavailable_without_manager() {
        let state = std::sync::Arc::new(AdminState::new(19999));
        let resp = ip_tls_pending_stream(state).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn clear_ip_tls_pending_reports_when_manager_missing() {
        let state = std::sync::Arc::new(AdminState::new(19999));
        let resp = clear_ip_tls_pending(state, None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["message"], "No pending IP TLS decisions");
    }

    #[tokio::test]
    async fn clear_ip_tls_pending_clears_manager_list() {
        let state = std::sync::Arc::new(
            AdminState::new(19998).with_ip_tls_pending_manager(crate::IpTlsPendingManager::new()),
        );

        {
            let manager = state.ip_tls_pending_manager.as_ref().unwrap();
            let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
            assert!(manager.check_and_add_pending(ip));
            assert_eq!(manager.pending_count(), 1);
        }

        let resp = clear_ip_tls_pending(state.clone(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Cleared all pending IP TLS decisions"));

        let manager = state.ip_tls_pending_manager.as_ref().unwrap();
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn tray_supported_matches_target_os_linux_flag() {
        if cfg!(target_os = "linux") {
            assert!(!tray_supported());
        } else {
            assert!(tray_supported());
        }
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::test_support::TestAdminState;

    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{body::Incoming, Request, StatusCode};
    use hyper_util::rt::TokioIo;
    use serde_json::json;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::TcpListener;

    async fn spawn_config_api_server(
        state: SharedAdminState,
    ) -> (String, std::thread::JoinHandle<()>) {
        // Run the server on a dedicated OS thread with its own runtime so that
        // the blocking `ureq` client in the test body cannot starve the server
        // task (the test itself runs on a current-thread runtime).
        let (tx, rx) = std::sync::mpsc::channel::<SocketAddr>();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build config api server runtime");
            rt.block_on(async move {
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                    .await
                    .expect("bind config api listener");
                let addr: SocketAddr = listener.local_addr().expect("config api local addr");
                tx.send(addr).expect("send config api addr");
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let io = TokioIo::new(stream);
                    let state_inner = state.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req: Request<Incoming>| {
                            let state = state_inner.clone();
                            async move {
                                let path = req.uri().path().to_string();
                                let resp = handle_config(req, state, None, &path).await;
                                Ok::<_, hyper::Error>(resp)
                            }
                        });
                        let _ = http1::Builder::new().serve_connection(io, service).await;
                    });
                }
            });
        });

        let addr = rx.recv().expect("recv config api addr");
        let base = format!("http://{}", addr);

        (base, handle)
    }

    fn put_json(url: &str, body: serde_json::Value) -> ureq::Response {
        match ureq::AgentBuilder::new()
            .build()
            .put(url)
            .set("content-type", "application/json")
            .send_string(&body.to_string())
        {
            Ok(resp) => resp,
            // ureq treats any non-2xx status as an error; tests asserting on
            // 4xx responses still need the underlying Response.
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(other) => panic!("config api request failed: {other}"),
        }
    }

    #[tokio::test]
    async fn update_performance_config_round_trips_via_http() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state.clone()).await;
        let url = format!("{}/api/config/performance", base);

        let body = json!({
            "max_records": MIN_TRAFFIC_MAX_RECORDS + 1,
            "file_retention_days": 1,
            "breakpoint_timeout_ms": DEFAULT_BREAKPOINT_TIMEOUT_MS,
        });

        let resp = put_json(&url, body);
        assert_eq!(resp.status(), 200);
        let resp_body = resp.into_string().unwrap();
        let cfg: PerformanceConfigResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(cfg.traffic.max_records, MIN_TRAFFIC_MAX_RECORDS + 1);

        let stored = harness.config_manager.config().await;
        assert_eq!(stored.traffic.max_records, cfg.traffic.max_records);
    }

    #[tokio::test]
    async fn update_performance_config_rejects_retention_days_over_limit() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/performance", base);

        let body = json!({ "file_retention_days": 8 });
        let resp = put_json(&url, body);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("cannot exceed 7 days"));
    }

    #[tokio::test]
    async fn update_performance_config_rejects_max_records_out_of_range() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/performance", base);

        let body = json!({ "max_records": MAX_TRAFFIC_MAX_RECORDS + 1 });
        let resp = put_json(&url, body);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("max_records must be between"));
    }

    #[tokio::test]
    async fn update_tls_config_updates_runtime_and_persists() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state.clone()).await;
        let url = format!("{}/api/config/tls", base);

        let body = json!({
            "enable_tls_interception": true,
            "intercept_exclude": ["example.com"],
            "intercept_include": [],
            "app_intercept_exclude": [],
            "app_intercept_include": [],
            "ip_intercept_exclude": [],
            "ip_intercept_include": [],
            "unsafe_ssl": true,
            "disconnect_on_config_change": true,
        });

        let resp = put_json(&url, body);
        assert_eq!(resp.status(), 200);
        let resp_body = resp.into_string().unwrap();
        let tls: TlsConfig = serde_json::from_str(&resp_body).unwrap();
        assert!(tls.enable_tls_interception);
        assert!(tls.unsafe_ssl);

        let runtime = state.runtime_config.read().await.clone();
        assert!(runtime.enable_tls_interception);
        assert!(runtime.unsafe_ssl);

        let stored = harness.config_manager.config().await;
        assert!(stored.tls.enable_interception);
        assert!(stored.tls.unsafe_ssl);
    }

    #[tokio::test]
    async fn update_tls_config_rejects_invalid_json() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/tls", base);

        let resp = match ureq::AgentBuilder::new()
            .build()
            .put(&url)
            .set("content-type", "application/json")
            .send_string("not-json")
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(other) => panic!("config api request failed: {other}"),
        };
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("Invalid JSON"));
    }

    #[tokio::test]
    async fn update_server_config_rejects_zero_port() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/server", base);

        let body = json!({ "port": 0 });
        let resp = put_json(&url, body);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("port must be between"));
    }

    #[tokio::test]
    async fn update_server_config_reports_unavailable_when_rebind_manager_missing() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/server", base);

        let body = json!({ "port": 12345 });
        let resp = put_json(&url, body);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("Port rebind is not available"));
    }

    #[tokio::test]
    async fn update_tray_config_requires_at_least_one_field() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/tray", base);

        let resp = put_json(&url, json!({}));
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("enabled, show_system_stats, or system_stats_items is required"));
    }

    #[tokio::test]
    async fn update_tray_config_accepts_system_stats_only() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state.clone()).await;
        let url = format!("{}/api/config/tray", base);

        let resp = put_json(&url, json!({"show_system_stats": false}));
        if tray_system_stats_supported() {
            assert_eq!(resp.status(), StatusCode::OK.as_u16());
            let body = resp.into_string().unwrap();
            let parsed: TrayConfig = serde_json::from_str(&body).unwrap();
            assert!(!parsed.show_system_stats);

            let cfg = harness.config_manager.config().await;
            assert!(!cfg.tray.show_system_stats);
            assert!(cfg.tray.enabled);
        } else {
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
            let msg = resp.into_string().unwrap();
            assert!(msg.contains("tray system stats are not supported on this platform"));
        }
    }

    #[tokio::test]
    async fn update_tray_config_accepts_individual_system_stats_items() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state.clone()).await;
        let url = format!("{}/api/config/tray", base);

        let resp = put_json(&url, json!({"system_stats_items": {"download": false}}));
        if tray_system_stats_supported() {
            assert_eq!(resp.status(), StatusCode::OK.as_u16());
            let body = resp.into_string().unwrap();
            let parsed: TrayConfig = serde_json::from_str(&body).unwrap();
            assert!(!parsed.system_stats_items.download);
            assert!(parsed.system_stats_items.upload);
            assert!(parsed.show_system_stats);

            let cfg = harness.config_manager.config().await;
            assert!(!cfg.tray.system_stats_items.download);
            assert!(cfg.tray.system_stats_items.upload);
            assert!(cfg.tray.show_system_stats);
        } else {
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
            let msg = resp.into_string().unwrap();
            assert!(msg.contains("tray system stats are not supported on this platform"));
        }
    }

    #[tokio::test]
    async fn update_sandbox_config_accepts_valid_payload() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/sandbox", base);

        let body = json!({
            "file": { "sandbox_dir": "/tmp/sandbox", "max_bytes": 1024 },
            "net": { "enabled": true, "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 2048 },
            "limits": { "timeout_ms": 2000, "max_memory_bytes": 4096 },
        });

        let resp = put_json(&url, body);
        assert_eq!(resp.status(), 200);
        let bytes = resp.into_string().unwrap();
        let v: serde_json::Value = serde_json::from_str(&bytes).unwrap();
        assert!(v.get("file").is_some());
    }

    #[tokio::test]
    async fn update_sandbox_config_rejects_non_absolute_allowed_dir() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, _handle) = spawn_config_api_server(state).await;
        let url = format!("{}/api/config/sandbox", base);

        let body = json!({
            "file": { "allowed_dirs": ["relative/path"] }
        });

        let resp = put_json(&url, body);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST.as_u16());
        let msg = resp.into_string().unwrap();
        assert!(msg.contains("allowed_dirs must be absolute paths"));
    }
}
