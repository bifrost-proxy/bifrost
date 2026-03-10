use bifrost_storage::{
    CollapsedSections, FilterPanelConfig, PinnedFilter, PinnedFilterType, TlsConfigUpdate,
    TrafficConfigUpdate, UiConfigUpdate,
};
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::body_store::{BodyStoreConfigUpdate, BodyStoreStats};
use crate::frame_store::FrameStoreStats;
use crate::state::SharedAdminState;
use crate::status_printer::TlsStatusInfo;
use crate::traffic_store::TrafficStoreStats;
use crate::ws_payload_store::{WsPayloadStoreConfigUpdate, WsPayloadStoreStats};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub unsafe_ssl: bool,
    pub disconnect_on_config_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxySettingsResponse {
    pub tls: TlsConfig,
    pub port: u16,
    pub host: String,
}

#[derive(Deserialize)]
pub struct UpdateTlsConfigRequest {
    pub enable_tls_interception: Option<bool>,
    pub intercept_exclude: Option<Vec<String>>,
    pub intercept_include: Option<Vec<String>>,
    pub app_intercept_exclude: Option<Vec<String>>,
    pub app_intercept_include: Option<Vec<String>>,
    pub unsafe_ssl: Option<bool>,
    pub disconnect_on_config_change: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficConfig {
    pub max_records: usize,
    pub max_db_size_bytes: u64,
    pub max_body_memory_size: usize,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub enable_body_index: bool,
    pub file_retention_days: u64,
    pub sse_stream_flush_bytes: usize,
    pub sse_stream_flush_interval_ms: u64,
    pub ws_payload_flush_bytes: usize,
    pub ws_payload_flush_interval_ms: u64,
    pub ws_payload_max_open_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfigResponse {
    pub traffic: TrafficConfig,
    pub body_store_stats: Option<BodyStoreStats>,
    pub traffic_store_stats: Option<TrafficStoreStats>,
    pub frame_store_stats: Option<FrameStoreStats>,
    pub ws_payload_store_stats: Option<WsPayloadStoreStats>,
}

#[derive(Deserialize)]
pub struct UpdateTrafficConfigRequest {
    pub max_records: Option<usize>,
    pub max_db_size_bytes: Option<u64>,
    pub max_body_memory_size: Option<usize>,
    pub max_body_buffer_size: Option<usize>,
    pub max_body_probe_size: Option<usize>,
    pub enable_body_index: Option<bool>,
    pub file_retention_days: Option<u64>,
    pub sse_stream_flush_bytes: Option<usize>,
    pub sse_stream_flush_interval_ms: Option<u64>,
    pub ws_payload_flush_bytes: Option<usize>,
    pub ws_payload_flush_interval_ms: Option<u64>,
    pub ws_payload_max_open_files: Option<usize>,
}

pub async fn handle_config(
    req: Request<Incoming>,
    state: SharedAdminState,
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
        "/api/config/performance" | "/api/config/performance/" => match method {
            Method::GET => get_performance_config(state).await,
            Method::PUT => update_performance_config(req, state).await,
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
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_proxy_settings(state: SharedAdminState) -> Response<BoxBody> {
    let runtime_config = state.runtime_config.read().await;

    let response = ProxySettingsResponse {
        tls: TlsConfig {
            enable_tls_interception: runtime_config.enable_tls_interception,
            intercept_exclude: runtime_config.intercept_exclude.clone(),
            intercept_include: runtime_config.intercept_include.clone(),
            app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
            app_intercept_include: runtime_config.app_intercept_include.clone(),
            unsafe_ssl: runtime_config.unsafe_ssl,
            disconnect_on_config_change: runtime_config.disconnect_on_config_change,
        },
        port: state.port,
        host: "127.0.0.1".to_string(),
    };

    json_response(&response)
}

async fn get_tls_config(state: SharedAdminState) -> Response<BoxBody> {
    let runtime_config = state.runtime_config.read().await;

    let tls_config = TlsConfig {
        enable_tls_interception: runtime_config.enable_tls_interception,
        intercept_exclude: runtime_config.intercept_exclude.clone(),
        intercept_include: runtime_config.intercept_include.clone(),
        app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
        app_intercept_include: runtime_config.app_intercept_include.clone(),
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
    let traffic_store_stats = state.traffic_store.as_ref().map(|ts| ts.stats());
    let frame_store_stats = state.frame_store.as_ref().map(|fs| fs.stats());
    let ws_payload_store_stats = state.ws_payload_store.as_ref().map(|ws| ws.stats());

    let traffic_config = if let Some(ref config_manager) = state.config_manager {
        let config = config_manager.config().await;
        TrafficConfig {
            max_records: config.traffic.max_records,
            max_db_size_bytes: config.traffic.max_db_size_bytes,
            max_body_memory_size: config.traffic.max_body_memory_size,
            max_body_buffer_size: config.traffic.max_body_buffer_size,
            max_body_probe_size: config.traffic.max_body_probe_size,
            enable_body_index: config.traffic.enable_body_index,
            file_retention_days: config.traffic.file_retention_days,
            sse_stream_flush_bytes: config.traffic.sse_stream_flush_bytes,
            sse_stream_flush_interval_ms: config.traffic.sse_stream_flush_interval_ms,
            ws_payload_flush_bytes: config.traffic.ws_payload_flush_bytes,
            ws_payload_flush_interval_ms: config.traffic.ws_payload_flush_interval_ms,
            ws_payload_max_open_files: config.traffic.ws_payload_max_open_files,
        }
    } else {
        TrafficConfig {
            max_records: 5000,
            max_db_size_bytes: 2 * 1024 * 1024 * 1024,
            max_body_memory_size: 512 * 1024,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            enable_body_index: false,
            file_retention_days: 7,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 128,
        }
    };

    let response = PerformanceConfigResponse {
        traffic: traffic_config,
        body_store_stats,
        traffic_store_stats,
        frame_store_stats,
        ws_payload_store_stats,
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

    if let Some(ref config_manager) = state.config_manager {
        let update = TrafficConfigUpdate {
            max_records: request.max_records,
            max_db_size_bytes: request.max_db_size_bytes,
            max_body_memory_size: request.max_body_memory_size,
            max_body_buffer_size: request.max_body_buffer_size,
            max_body_probe_size: request.max_body_probe_size,
            enable_body_index: request.enable_body_index,
            file_retention_days: request.file_retention_days,
            sse_stream_flush_bytes: request.sse_stream_flush_bytes,
            sse_stream_flush_interval_ms: request.sse_stream_flush_interval_ms,
            ws_payload_flush_bytes: request.ws_payload_flush_bytes,
            ws_payload_flush_interval_ms: request.ws_payload_flush_interval_ms,
            ws_payload_max_open_files: request.ws_payload_max_open_files,
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
        if let Some(ref traffic_store) = state.traffic_store {
            traffic_store.set_max_records(max_records);
        }
        if let Some(ref traffic_db_store) = state.traffic_db_store {
            traffic_db_store.set_max_records(max_records);
        }
    }

    if let Some(max_db_size_bytes) = request.max_db_size_bytes {
        if let Some(ref traffic_db_store) = state.traffic_db_store {
            traffic_db_store.set_max_db_size_bytes(max_db_size_bytes);
        }
    }

    if let Some(enable_body_index) = request.enable_body_index {
        if let Some(ref traffic_db_store) = state.traffic_db_store {
            traffic_db_store.set_body_index_enabled(enable_body_index);
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

    if let Some(ref traffic_store) = state.traffic_store {
        use crate::traffic_store::TrafficStoreConfigUpdate;
        let retention_hours = request.file_retention_days.map(|d| d * 24);
        let traffic_store_update = TrafficStoreConfigUpdate {
            max_records: request.max_records,
            retention_hours,
        };
        traffic_store.update_config(traffic_store_update);
    }

    if let Some(max_body_buffer_size) = request.max_body_buffer_size {
        state.set_max_body_buffer_size(max_body_buffer_size);
    }

    if let Some(max_body_probe_size) = request.max_body_probe_size {
        state.set_max_body_probe_size(max_body_probe_size);
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

    if let Some(ref traffic_store) = state.traffic_store {
        traffic_store.clear();
        let stats = traffic_store.stats();
        traffic_removed = stats.total_records_processed as usize;
        tracing::info!("Cleared traffic store records");
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
    Domain,
}

impl From<PinnedFilterType> for UiPinnedFilterType {
    fn from(t: PinnedFilterType) -> Self {
        match t {
            PinnedFilterType::ClientIp => Self::ClientIp,
            PinnedFilterType::ClientApp => Self::ClientApp,
            PinnedFilterType::Domain => Self::Domain,
        }
    }
}

impl From<UiPinnedFilterType> for PinnedFilterType {
    fn from(t: UiPinnedFilterType) -> Self {
        match t {
            UiPinnedFilterType::ClientIp => Self::ClientIp,
            UiPinnedFilterType::ClientApp => Self::ClientApp,
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
    pub domain: bool,
}

impl From<CollapsedSections> for UiCollapsedSections {
    fn from(s: CollapsedSections) -> Self {
        Self {
            pinned: s.pinned,
            client_ip: s.client_ip,
            client_app: s.client_app,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUiConfigRequest {
    #[serde(rename = "pinnedFilters")]
    pub pinned_filters: Option<Vec<UiPinnedFilter>>,
    #[serde(rename = "filterPanel")]
    pub filter_panel: Option<UiFilterPanelConfig>,
    #[serde(rename = "detailPanelCollapsed")]
    pub detail_panel_collapsed: Option<bool>,
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
