use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;

use bifrost_core::{ClientAccessControl, SystemProxyManager};
use bifrost_storage::{ConfigManager, RulesStorage, SharedConfigManager, ValuesStorage};
use bifrost_sync::SharedSyncManager;

use crate::admin_auth_db::{AuthDb, SharedAuthDb};
use parking_lot::RwLock as ParkingRwLock;
use tokio::sync::RwLock;

use crate::app_icon::SharedAppIconCache;
use crate::async_traffic::{AsyncTrafficWriter, SharedAsyncTrafficWriter};
use crate::body_store::SharedBodyStore;
use crate::breakpoint::{BreakpointManager, SharedBreakpointManager};
use crate::client_trust_tracker::ClientTlsTrustTracker;
use crate::connection_monitor::{ConnectionMonitor, SharedConnectionMonitor};
use crate::connection_registry::{ConnectionRegistry, SharedConnectionRegistry};
use crate::devtools::{BrowserDebugBroker, SharedBrowserDebugBroker};
use crate::frame_store::{FrameStore, SharedFrameStore};
use crate::handlers::scripts::ScriptManager;
use crate::ip_tls_pending::IpTlsPendingManager;
use crate::metrics::{MetricsCollector, SharedMetricsCollector};
use crate::port_rebind::SharedPortRebindManager;
use crate::replay_db::{ReplayDbStore, SharedReplayDbStore};
use crate::replay_executor::SharedReplayExecutor;
use crate::sse::SseHub;
use crate::temp_ports::SharedTemporaryPortManager;
use crate::traffic::{SocketStatus, TrafficRecord};
use crate::traffic_db::TrafficSummaryCompact;
use crate::traffic_db::{SharedTrafficDbStore, TrafficDbStore};
use crate::version_check::{SharedVersionChecker, VersionChecker};
use crate::ws_payload_store::{SharedWsPayloadStore, WsPayloadStore};
use once_cell::sync::OnceCell;

pub type SharedScriptManager = Arc<RwLock<ScriptManager>>;
pub type SharedIpTlsPendingManager = Arc<IpTlsPendingManager>;
pub type SharedClientTrustTracker = Arc<ClientTlsTrustTracker>;

pub type SharedAccessControl = Arc<RwLock<ClientAccessControl>>;
pub type SharedValuesStorage = Arc<ParkingRwLock<ValuesStorage>>;
pub type SharedSystemProxyManager = Arc<RwLock<SystemProxyManager>>;
pub type SharedRuntimeConfig = Arc<RwLock<RuntimeConfig>>;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
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

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            enable_tls_interception: true,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            unsafe_ssl: false,
            disconnect_on_config_change: true,
        }
    }
}

impl RuntimeConfig {
    pub fn from_tls_config(tls: &bifrost_storage::TlsConfig) -> Self {
        Self {
            enable_tls_interception: tls.enable_interception,
            intercept_exclude: tls.intercept_exclude.clone(),
            intercept_include: tls.intercept_include.clone(),
            app_intercept_exclude: tls.app_intercept_exclude.clone(),
            app_intercept_include: tls.app_intercept_include.clone(),
            ip_intercept_exclude: tls.ip_intercept_exclude.clone(),
            ip_intercept_include: tls.ip_intercept_include.clone(),
            unsafe_ssl: tls.unsafe_ssl,
            disconnect_on_config_change: tls.disconnect_on_change,
        }
    }
}

pub struct AdminState {
    pub traffic_db_store: Option<SharedTrafficDbStore>,
    pub async_traffic_writer: Option<SharedAsyncTrafficWriter>,
    pub metrics_collector: SharedMetricsCollector,
    pub rules_storage: RulesStorage,
    pub values_storage: Option<SharedValuesStorage>,
    pub auth_db: Option<SharedAuthDb>,
    pub access_control: Option<SharedAccessControl>,
    pub body_store: Option<SharedBodyStore>,
    pub frame_store: Option<SharedFrameStore>,
    pub ws_payload_store: Option<SharedWsPayloadStore>,
    pub start_time: u64,
    port: AtomicU16,
    pub ca_cert_path: Option<PathBuf>,
    pub system_proxy_manager: Option<SharedSystemProxyManager>,
    pub connection_monitor: SharedConnectionMonitor,
    pub sse_hub: Arc<SseHub>,
    pub runtime_config: SharedRuntimeConfig,
    pub connection_registry: SharedConnectionRegistry,
    pub config_manager: Option<SharedConfigManager>,
    pub max_body_buffer_size: AtomicUsize,
    pub max_body_probe_size: AtomicUsize,
    pub binary_traffic_performance_mode: AtomicBool,
    pub app_icon_cache: Option<SharedAppIconCache>,
    pub version_checker: SharedVersionChecker,
    pub script_manager: Option<SharedScriptManager>,
    pub replay_db_store: Option<SharedReplayDbStore>,
    pub replay_executor: OnceCell<SharedReplayExecutor>,
    pub port_rebind_manager: Option<SharedPortRebindManager>,
    pub sync_manager: Option<SharedSyncManager>,
    pub ip_tls_pending_manager: Option<Arc<IpTlsPendingManager>>,
    pub client_trust_tracker: Option<SharedClientTrustTracker>,
    pub devtools_broker: SharedBrowserDebugBroker,
    pub breakpoint_manager: SharedBreakpointManager,
    temporary_port_manager: parking_lot::RwLock<Option<SharedTemporaryPortManager>>,
    /// Keep-awake manager; None if not initialized (e.g. in tests).
    pub keepawake_manager: Option<bifrost_power::SharedKeepAwakeManager>,
    remote_invoke_worker:
        parking_lot::RwLock<Option<crate::handlers::remote_invoke::SharedRemoteInvokeWorker>>,
    im_gateway_service:
        parking_lot::RwLock<Option<crate::handlers::im_gateway::SharedImGatewayService>>,
    group_name_cache: parking_lot::Mutex<HashMap<String, String>>,
    group_cache_resolved: AtomicBool,
    badge_rules_cache: parking_lot::RwLock<String>,
}

const DEFAULT_MAX_BODY_BUFFER_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_BODY_PROBE_SIZE: usize = 64 * 1024;

impl AdminState {
    pub fn new(port: u16) -> Self {
        Self::new_with_rules_storage(port, RulesStorage::default())
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(port: u16, rules_storage: RulesStorage) -> Self {
        Self::new_with_rules_storage(port, rules_storage)
    }

    fn new_with_rules_storage(port: u16, rules_storage: RulesStorage) -> Self {
        Self {
            traffic_db_store: None,
            async_traffic_writer: None,
            metrics_collector: Arc::new(MetricsCollector::default()),
            rules_storage,
            values_storage: None,
            auth_db: None,
            access_control: None,
            body_store: None,
            frame_store: None,
            ws_payload_store: None,
            start_time: chrono::Utc::now().timestamp() as u64,
            port: AtomicU16::new(port),
            ca_cert_path: None,
            system_proxy_manager: None,
            connection_monitor: Arc::new(ConnectionMonitor::new()),
            sse_hub: SseHub::new(),
            runtime_config: Arc::new(RwLock::new(RuntimeConfig::default())),
            connection_registry: Arc::new(ConnectionRegistry::default()),
            config_manager: None,
            max_body_buffer_size: AtomicUsize::new(DEFAULT_MAX_BODY_BUFFER_SIZE),
            max_body_probe_size: AtomicUsize::new(DEFAULT_MAX_BODY_PROBE_SIZE),
            binary_traffic_performance_mode: AtomicBool::new(true),
            app_icon_cache: None,
            version_checker: Arc::new(VersionChecker::new()),
            script_manager: None,
            replay_db_store: None,
            replay_executor: OnceCell::new(),
            port_rebind_manager: None,
            sync_manager: None,
            ip_tls_pending_manager: None,
            client_trust_tracker: None,
            devtools_broker: Arc::new(BrowserDebugBroker::new()),
            breakpoint_manager: Arc::new(BreakpointManager::new()),
            temporary_port_manager: parking_lot::RwLock::new(None),
            keepawake_manager: None,
            remote_invoke_worker: parking_lot::RwLock::new(None),
            im_gateway_service: parking_lot::RwLock::new(None),
            group_name_cache: parking_lot::Mutex::new(HashMap::new()),
            group_cache_resolved: AtomicBool::new(false),
            badge_rules_cache: parking_lot::RwLock::new(
                r#"{"rules":[],"merged_content":""}"#.to_string(),
            ),
        }
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        let old = self.port.swap(port, Ordering::SeqCst);
        if old != port {
            tracing::info!("AdminState port updated: {} -> {}", old, port);
        }
    }

    pub fn get_max_body_buffer_size(&self) -> usize {
        self.max_body_buffer_size.load(Ordering::Relaxed)
    }

    pub fn get_max_body_probe_size(&self) -> usize {
        self.max_body_probe_size.load(Ordering::Relaxed)
    }

    pub fn get_binary_traffic_performance_mode(&self) -> bool {
        self.binary_traffic_performance_mode.load(Ordering::Relaxed)
    }

    pub fn set_max_body_buffer_size(&self, size: usize) {
        let old = self.max_body_buffer_size.swap(size, Ordering::SeqCst);
        if old != size {
            tracing::info!(
                "AdminState config updated: max_body_buffer_size {} -> {}",
                old,
                size
            );
        }
    }

    pub fn set_max_body_probe_size(&self, size: usize) {
        let old = self.max_body_probe_size.swap(size, Ordering::SeqCst);
        if old != size {
            tracing::info!(
                "AdminState config updated: max_body_probe_size {} -> {}",
                old,
                size
            );
        }
    }

    pub fn set_binary_traffic_performance_mode(&self, enabled: bool) {
        let old = self
            .binary_traffic_performance_mode
            .swap(enabled, Ordering::SeqCst);
        if old != enabled {
            tracing::info!(
                "AdminState config updated: binary_traffic_performance_mode {} -> {}",
                old,
                enabled
            );
        }
    }

    #[inline]
    pub fn record_traffic(&self, record: crate::traffic::TrafficRecord) {
        if let Some(ref writer) = self.async_traffic_writer {
            writer.record(record);
        } else if let Some(ref db_store) = self.traffic_db_store {
            db_store.record(record);
        } else {
            tracing::error!("[ADMIN_STATE] No traffic_db_store configured; drop record");
        }
    }

    #[inline]
    pub fn update_traffic_by_id<F>(&self, id: &str, updater: F)
    where
        F: Fn(&mut crate::traffic::TrafficRecord) + Send + Sync + Clone + 'static,
    {
        if let Some(ref writer) = self.async_traffic_writer {
            writer.update_by_id(id, updater);
        } else if let Some(ref db_store) = self.traffic_db_store {
            db_store.update_by_id(id, updater);
        } else {
            tracing::error!("[ADMIN_STATE] No traffic_db_store configured; drop update");
        }
    }

    fn runtime_socket_status(&self, id: &str, is_sse: bool) -> Option<SocketStatus> {
        if is_sse {
            return self.sse_hub.get_socket_status(id);
        }

        if let Some(status) = self.connection_monitor.get_connection_status(id) {
            return Some(status);
        }

        self.frame_store.as_ref().and_then(|fs| {
            fs.get_metadata(id).map(|metadata| SocketStatus {
                is_open: !metadata.is_closed,
                frame_count: metadata.frame_count as usize,
                ..Default::default()
            })
        })
    }

    fn synthesized_closed_socket_status(frame_count: usize) -> SocketStatus {
        SocketStatus {
            is_open: false,
            frame_count,
            ..Default::default()
        }
    }

    fn socket_status_has_traffic(socket_status: &SocketStatus) -> bool {
        socket_status.send_count > 0
            || socket_status.receive_count > 0
            || socket_status.send_bytes > 0
            || socket_status.receive_bytes > 0
            || socket_status.frame_count > 0
    }

    fn merge_closed_socket_status_from_summary(
        is_sse: bool,
        summary_frame_count: usize,
        summary_response_size: usize,
        socket_status: &SocketStatus,
    ) -> SocketStatus {
        let mut merged = socket_status.clone();
        merged.is_open = false;

        let merged_frame_count = summary_frame_count.max(merged.frame_count);
        merged.frame_count = merged_frame_count;

        if is_sse {
            if merged.receive_count == 0 && merged_frame_count > 0 {
                merged.receive_count = merged_frame_count as u64;
            }
            if merged.receive_bytes == 0 && summary_response_size > 0 {
                merged.receive_bytes = summary_response_size as u64;
            }
        }

        merged
    }

    pub fn reconcile_socket_summary(&self, summary: &mut TrafficSummaryCompact) {
        if !summary.is_sse() && !summary.is_websocket() && !summary.is_tunnel() {
            return;
        }

        let summary_is_sse = summary.is_sse();
        let summary_frame_count = summary.fc;
        let summary_response_size = summary.res_sz;

        if let Some(status) = self.runtime_socket_status(&summary.id, summary_is_sse) {
            summary.fc = status.frame_count;
            summary.ss = Some(status);
        } else if let Some(socket_status) = summary.ss.as_mut() {
            if socket_status.is_open {
                let has_summary_traffic = summary_frame_count > 0 || summary_response_size > 0;
                let has_socket_traffic = Self::socket_status_has_traffic(socket_status);
                if summary_is_sse && !has_summary_traffic && !has_socket_traffic {
                    socket_status.is_open = false;
                    return;
                }

                let status = Self::merge_closed_socket_status_from_summary(
                    summary_is_sse,
                    summary_frame_count,
                    summary_response_size,
                    socket_status,
                );
                *socket_status = status.clone();
                let frame_count = summary.fc.max(status.frame_count);
                let response_size = if summary.is_sse() {
                    Some(
                        summary
                            .res_sz
                            .max((status.send_bytes.saturating_add(status.receive_bytes)) as usize),
                    )
                } else {
                    None
                };
                let record_id = summary.id.clone();
                self.update_traffic_by_id(&record_id, move |record| {
                    record.socket_status = Some(status.clone());
                    record.frame_count = record.frame_count.max(frame_count);
                    if let Some(size) = response_size {
                        record.response_size = record.response_size.max(size);
                    }
                });
            }
        } else {
            let status = Self::synthesized_closed_socket_status(summary.fc);
            summary.ss = Some(status.clone());
            let record_id = summary.id.clone();
            self.update_traffic_by_id(&record_id, move |record| {
                if record.socket_status.is_none() {
                    record.socket_status = Some(status.clone());
                    record.frame_count = record.frame_count.max(status.frame_count);
                    record.last_frame_id = record.last_frame_id.max(status.frame_count as u64);
                }
            });
        }

        if summary.is_sse() {
            if let Some(ref socket_status) = summary.ss {
                let total = socket_status.send_bytes + socket_status.receive_bytes;
                summary.res_sz = summary.res_sz.max(total as usize);
            }
        }
    }

    pub fn reconcile_traffic_record(&self, record: &mut TrafficRecord) {
        if !record.is_sse && !record.is_websocket && !record.is_tunnel {
            return;
        }

        if let Some(status) = self.runtime_socket_status(&record.id, record.is_sse) {
            record.frame_count = status.frame_count;
            record.last_frame_id = status.frame_count as u64;
            record.socket_status = Some(status);
        } else if let Some(socket_status) = record.socket_status.as_mut() {
            if socket_status.is_open {
                socket_status.is_open = false;
            }
        } else {
            let status = Self::synthesized_closed_socket_status(record.frame_count);
            record.last_frame_id = record.last_frame_id.max(status.frame_count as u64);
            record.socket_status = Some(status);
        }

        if let Some(ref socket_status) = record.socket_status {
            let total = socket_status.send_bytes + socket_status.receive_bytes;
            if record.is_sse && total > 0 {
                record.response_size = record.response_size.max(total as usize);
            }
        }
    }

    pub fn cleanup_total_disk_usage_if_needed(&self) {
        self.cleanup_total_disk_usage();
    }

    fn cleanup_total_disk_usage(&self) {
        let Some(ref traffic_db_store) = self.traffic_db_store else {
            return;
        };
        let max_db_size_bytes = traffic_db_store.max_db_size_bytes();
        if max_db_size_bytes == 0 {
            return;
        }

        let db_stats = traffic_db_store.stats();
        let mut total_size = db_stats.db_size;

        let body_sizes = self
            .body_store
            .as_ref()
            .and_then(|store| store.read().sizes_by_id().ok())
            .unwrap_or_default();
        total_size += body_sizes.values().sum::<u64>();

        let frame_sizes = self
            .frame_store
            .as_ref()
            .and_then(|store| store.sizes_by_id().ok())
            .unwrap_or_default();
        total_size += frame_sizes.values().sum::<u64>();

        let ws_payload_sizes = self
            .ws_payload_store
            .as_ref()
            .and_then(|store| store.sizes_by_safe_id().ok())
            .unwrap_or_default();
        total_size += ws_payload_sizes.values().sum::<u64>();

        if total_size <= max_db_size_bytes {
            return;
        }

        let existing_db_ids = traffic_db_store.all_ids_set();

        let recent_threshold = std::time::Duration::from_secs(300);

        let protected_body_ids = self
            .body_store
            .as_ref()
            .map(|store| {
                let store = store.read();
                let mut protected = store.active_stream_id_set();
                protected.extend(store.recently_modified_ids(recent_threshold));
                protected
            })
            .unwrap_or_default();

        let protected_frame_ids = self
            .frame_store
            .as_ref()
            .map(|store| store.recently_modified_ids(recent_threshold))
            .unwrap_or_default();

        let protected_ws_ids = self
            .ws_payload_store
            .as_ref()
            .map(|store| {
                let mut protected = store.active_connection_ids();
                protected.extend(store.recently_modified_ids(recent_threshold));
                protected
            })
            .unwrap_or_default();

        let orphan_body_ids: Vec<String> = body_sizes
            .keys()
            .filter(|id| {
                !existing_db_ids.contains(id.as_str()) && !protected_body_ids.contains(id.as_str())
            })
            .cloned()
            .collect();
        let orphan_frame_ids: Vec<String> = frame_sizes
            .keys()
            .filter(|id| {
                !existing_db_ids.contains(id.as_str()) && !protected_frame_ids.contains(id.as_str())
            })
            .cloned()
            .collect();
        let orphan_ws_ids: Vec<String> = ws_payload_sizes
            .keys()
            .filter(|id| {
                !existing_db_ids.contains(id.as_str()) && !protected_ws_ids.contains(id.as_str())
            })
            .cloned()
            .collect();

        let mut orphan_bytes = 0u64;
        for id in &orphan_body_ids {
            orphan_bytes += body_sizes.get(id).copied().unwrap_or(0);
        }
        for id in &orphan_frame_ids {
            orphan_bytes += frame_sizes.get(id).copied().unwrap_or(0);
        }
        for id in &orphan_ws_ids {
            orphan_bytes += ws_payload_sizes.get(id).copied().unwrap_or(0);
        }

        if orphan_bytes > 0 {
            if let Some(ref body_store) = self.body_store {
                let _ = body_store.write().delete_by_ids(&orphan_body_ids);
            }
            if let Some(ref frame_store) = self.frame_store {
                let _ = frame_store.delete_by_ids(&orphan_frame_ids);
            }
            if let Some(ref ws_payload_store) = self.ws_payload_store {
                let _ = ws_payload_store.delete_by_ids(&orphan_ws_ids);
            }
            total_size = total_size.saturating_sub(orphan_bytes);
            tracing::info!(
                orphan_body = orphan_body_ids.len(),
                orphan_frame = orphan_frame_ids.len(),
                orphan_ws = orphan_ws_ids.len(),
                orphan_bytes = orphan_bytes,
                protected_body = protected_body_ids.len(),
                protected_frame = protected_frame_ids.len(),
                protected_ws = protected_ws_ids.len(),
                "[TRAFFIC] Cleaned up orphaned cache files"
            );
        }

        if total_size <= max_db_size_bytes {
            return;
        }

        let target_size = max_db_size_bytes.saturating_sub(max_db_size_bytes / 4);
        let mut remaining_to_remove = total_size.saturating_sub(target_size);
        if remaining_to_remove == 0 {
            return;
        }

        let initial_record_count = db_stats.record_count.max(existing_db_ids.len());
        if initial_record_count == 0 {
            return;
        }

        let avg_db_bytes = (db_stats.db_size / initial_record_count as u64).max(1);
        let mut total_deleted = 0usize;
        let mut cumulative_offset = 0usize;
        const MAX_ROUNDS: usize = 10;

        for round in 0..MAX_ROUNDS {
            let current_count = if round == 0 {
                initial_record_count
            } else {
                initial_record_count.saturating_sub(total_deleted)
            };
            if current_count == 0 {
                break;
            }
            let max_delete = (current_count / 4).max(1);

            let mut ids_to_delete = Vec::new();
            let mut removed_estimate = 0u64;
            let batch = 500usize;

            while removed_estimate < remaining_to_remove {
                if ids_to_delete.len() >= max_delete {
                    break;
                }
                let ids = traffic_db_store.oldest_ids(batch, cumulative_offset);
                if ids.is_empty() {
                    break;
                }
                let fetched = ids.len();
                for id in ids {
                    let mut size = avg_db_bytes;
                    if let Some(extra) = body_sizes.get(&id) {
                        size += *extra;
                    }
                    if let Some(extra) = frame_sizes.get(&id) {
                        size += *extra;
                    }
                    let safe_id = WsPayloadStore::safe_connection_id(&id);
                    if let Some(extra) = ws_payload_sizes.get(&safe_id) {
                        size += *extra;
                    }
                    removed_estimate += size;
                    ids_to_delete.push(id);
                    if removed_estimate >= remaining_to_remove || ids_to_delete.len() >= max_delete
                    {
                        break;
                    }
                }
                cumulative_offset += fetched;
            }

            if ids_to_delete.is_empty() {
                break;
            }

            let round_deleted = ids_to_delete.len();
            traffic_db_store.delete_by_ids(&ids_to_delete);
            if let Some(ref body_store) = self.body_store {
                let _ = body_store.write().delete_by_ids(&ids_to_delete);
            }
            if let Some(ref frame_store) = self.frame_store {
                let _ = frame_store.delete_by_ids(&ids_to_delete);
            }
            if let Some(ref ws_payload_store) = self.ws_payload_store {
                let _ = ws_payload_store.delete_by_ids(&ids_to_delete);
            }
            total_deleted += round_deleted;
            remaining_to_remove = remaining_to_remove.saturating_sub(removed_estimate);
            cumulative_offset = 0;

            if remaining_to_remove == 0 {
                break;
            }
        }

        if total_deleted > 0 {
            traffic_db_store.compact_db(false);
            tracing::info!(
                deleted = total_deleted,
                total_size = total_size,
                max_db_size_bytes = max_db_size_bytes,
                target_size = target_size,
                "[TRAFFIC] Cleaned up data due to total size limit"
            );
        }
    }

    pub fn update_client_process(
        &self,
        id: &str,
        client_app: String,
        client_pid: u32,
        client_path: Option<String>,
    ) {
        tracing::info!(
            req_id = id,
            client_app = %client_app,
            client_pid,
            client_path = ?client_path,
            "Updating traffic record with resolved client process"
        );
        self.update_traffic_by_id(id, move |record| {
            record.client_app = Some(client_app.clone());
            record.client_pid = Some(client_pid);
            record.client_path = client_path.clone();
        });
    }

    pub fn with_rules_storage(mut self, storage: RulesStorage) -> Self {
        self.rules_storage = storage;
        self
    }

    pub fn with_values_storage(mut self, storage: ValuesStorage) -> Self {
        self.values_storage = Some(Arc::new(ParkingRwLock::new(storage)));
        self
    }

    pub fn with_auth_db(mut self, db: AuthDb) -> Self {
        self.auth_db = Some(Arc::new(db));
        self
    }

    pub fn with_traffic_db_store(mut self, store: TrafficDbStore) -> Self {
        self.traffic_db_store = Some(Arc::new(store));
        self
    }

    pub fn with_traffic_db_store_shared(mut self, store: SharedTrafficDbStore) -> Self {
        self.traffic_db_store = Some(store);
        self
    }

    pub fn with_metrics_collector(mut self, collector: MetricsCollector) -> Self {
        self.metrics_collector = Arc::new(collector);
        self
    }

    pub fn with_access_control(mut self, access_control: SharedAccessControl) -> Self {
        self.access_control = Some(access_control);
        self
    }

    pub fn with_body_store(mut self, body_store: SharedBodyStore) -> Self {
        self.body_store = Some(body_store);
        self
    }

    pub fn with_ws_payload_store(mut self, store: SharedWsPayloadStore) -> Self {
        self.ws_payload_store = Some(store);
        self
    }

    pub fn with_frame_store(mut self, frame_store: FrameStore) -> Self {
        self.frame_store = Some(Arc::new(frame_store));
        self
    }

    pub fn with_frame_store_shared(mut self, frame_store: SharedFrameStore) -> Self {
        self.frame_store = Some(frame_store);
        self
    }

    pub fn with_ca_cert_path(mut self, ca_cert_path: PathBuf) -> Self {
        self.ca_cert_path = Some(ca_cert_path);
        self
    }

    pub fn with_system_proxy_manager(mut self, manager: SystemProxyManager) -> Self {
        self.system_proxy_manager = Some(Arc::new(RwLock::new(manager)));
        self
    }

    pub fn with_system_proxy_manager_shared(mut self, manager: SharedSystemProxyManager) -> Self {
        self.system_proxy_manager = Some(manager);
        self
    }

    pub fn with_runtime_config(mut self, config: RuntimeConfig) -> Self {
        self.runtime_config = Arc::new(RwLock::new(config));
        self
    }

    pub fn with_runtime_config_shared(mut self, config: SharedRuntimeConfig) -> Self {
        self.runtime_config = config;
        self
    }

    pub fn with_connection_registry(mut self, registry: ConnectionRegistry) -> Self {
        self.connection_registry = Arc::new(registry);
        self
    }

    pub fn with_connection_registry_shared(mut self, registry: SharedConnectionRegistry) -> Self {
        self.connection_registry = registry;
        self
    }

    pub fn with_config_manager(mut self, manager: ConfigManager) -> Self {
        self.config_manager = Some(Arc::new(manager));
        self
    }

    pub fn with_config_manager_shared(mut self, manager: SharedConfigManager) -> Self {
        self.config_manager = Some(manager);
        self
    }

    pub fn with_max_body_buffer_size(self, size: usize) -> Self {
        self.max_body_buffer_size.store(size, Ordering::SeqCst);
        self
    }

    pub fn with_max_body_probe_size(self, size: usize) -> Self {
        self.max_body_probe_size.store(size, Ordering::SeqCst);
        self
    }

    pub fn with_binary_traffic_performance_mode(self, enabled: bool) -> Self {
        self.binary_traffic_performance_mode
            .store(enabled, Ordering::SeqCst);
        self
    }

    pub fn with_breakpoint_timeout_ms(self, timeout_ms: u64) -> Self {
        self.breakpoint_manager.set_timeout_ms(timeout_ms);
        self
    }

    pub fn with_app_icon_cache(mut self, cache: SharedAppIconCache) -> Self {
        self.app_icon_cache = Some(cache);
        self
    }

    pub fn with_async_traffic_writer(mut self, writer: AsyncTrafficWriter) -> Self {
        self.async_traffic_writer = Some(Arc::new(writer));
        self
    }

    pub fn with_async_traffic_writer_shared(mut self, writer: SharedAsyncTrafficWriter) -> Self {
        self.async_traffic_writer = Some(writer);
        self
    }

    pub fn with_script_manager(mut self, manager: ScriptManager) -> Self {
        self.script_manager = Some(Arc::new(RwLock::new(manager)));
        self
    }

    pub fn with_script_manager_shared(mut self, manager: SharedScriptManager) -> Self {
        self.script_manager = Some(manager);
        self
    }

    pub fn with_replay_db_store(mut self, store: ReplayDbStore) -> Self {
        self.replay_db_store = Some(Arc::new(store));
        self
    }

    pub fn with_replay_db_store_shared(mut self, store: SharedReplayDbStore) -> Self {
        self.replay_db_store = Some(store);
        self
    }

    pub fn with_replay_db_store_shared_opt(mut self, store: Option<SharedReplayDbStore>) -> Self {
        self.replay_db_store = store;
        self
    }

    pub fn with_port_rebind_manager_shared(mut self, manager: SharedPortRebindManager) -> Self {
        self.port_rebind_manager = Some(manager);
        self
    }

    pub fn with_sync_manager_shared(mut self, manager: SharedSyncManager) -> Self {
        self.sync_manager = Some(manager);
        self
    }

    pub fn with_ip_tls_pending_manager(mut self, manager: IpTlsPendingManager) -> Self {
        self.ip_tls_pending_manager = Some(Arc::new(manager));
        self
    }

    pub fn with_client_trust_tracker(mut self, tracker: ClientTlsTrustTracker) -> Self {
        self.client_trust_tracker = Some(Arc::new(tracker));
        self
    }

    pub fn with_remote_invoke_worker(
        self,
        worker: crate::handlers::remote_invoke::SharedRemoteInvokeWorker,
    ) -> Self {
        self.set_remote_invoke_worker(worker);
        self
    }

    pub fn set_remote_invoke_worker(
        &self,
        worker: crate::handlers::remote_invoke::SharedRemoteInvokeWorker,
    ) {
        *self.remote_invoke_worker.write() = Some(worker);
    }

    pub fn remote_invoke_worker(
        &self,
    ) -> Option<crate::handlers::remote_invoke::SharedRemoteInvokeWorker> {
        self.remote_invoke_worker.read().clone()
    }

    pub fn with_im_gateway_service(
        self,
        service: crate::handlers::im_gateway::SharedImGatewayService,
    ) -> Self {
        self.set_im_gateway_service(service);
        self
    }

    pub fn set_im_gateway_service(
        &self,
        service: crate::handlers::im_gateway::SharedImGatewayService,
    ) {
        *self.im_gateway_service.write() = Some(service);
    }

    pub fn im_gateway_service(
        &self,
    ) -> Option<crate::handlers::im_gateway::SharedImGatewayService> {
        self.im_gateway_service.read().clone()
    }

    pub fn set_replay_executor(&self, executor: SharedReplayExecutor) {
        let _ = self.replay_executor.set(executor);
    }

    pub fn set_temporary_port_manager(&self, manager: SharedTemporaryPortManager) {
        *self.temporary_port_manager.write() = Some(manager);
    }

    pub fn temporary_port_manager(&self) -> Option<SharedTemporaryPortManager> {
        self.temporary_port_manager.read().clone()
    }

    pub fn get_replay_executor(&self) -> Option<&SharedReplayExecutor> {
        self.replay_executor.get()
    }

    pub fn group_name_cache(&self) -> GroupNameCacheGuard<'_> {
        GroupNameCacheGuard {
            guard: self.group_name_cache.lock(),
        }
    }

    pub fn is_group_cache_resolved(&self) -> bool {
        self.group_cache_resolved.load(Ordering::Relaxed)
    }

    pub fn set_group_cache_resolved(&self) {
        self.group_cache_resolved.store(true, Ordering::Relaxed);
    }

    pub fn clear_group_cache_resolved(&self) {
        self.group_cache_resolved.store(false, Ordering::Relaxed);
    }

    fn group_cache_path(&self) -> PathBuf {
        self.rules_storage.base_dir().join(".group_cache.json")
    }

    pub fn load_group_name_cache(&self) {
        let path = self.group_cache_path();
        if !path.exists() {
            return;
        }
        const MAX_CACHE_FILE_BYTES: u64 = 256 * 1024 * 1024;
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_CACHE_FILE_BYTES {
            tracing::warn!(
                target: "bifrost_admin::state",
                "group cache file too large, skipping"
            );
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<HashMap<String, String>>(&data) {
                Ok(map) => {
                    let mut cache = self.group_name_cache.lock();
                    for (k, v) in map {
                        cache.entry(k).or_insert(v);
                    }
                    tracing::info!(
                        target: "bifrost_admin::state",
                        count = cache.len(),
                        "loaded group name cache from disk"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "bifrost_admin::state",
                        error = %e,
                        "failed to parse group name cache file, ignoring"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::state",
                    error = %e,
                    "failed to read group name cache file"
                );
            }
        }
    }

    pub fn persist_group_name_cache(&self) {
        let cache = self.group_name_cache.lock();
        if cache.is_empty() {
            return;
        }
        let path = self.group_cache_path();
        match serde_json::to_string(&*cache) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&path, data) {
                    tracing::warn!(
                        target: "bifrost_admin::state",
                        error = %e,
                        "failed to persist group name cache"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::state",
                    error = %e,
                    "failed to serialize group name cache"
                );
            }
        }
    }

    pub fn clear_group_name_cache(&self) {
        {
            let mut cache = self.group_name_cache.lock();
            cache.clear();
        }
        let path = self.group_cache_path();
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(
                    target: "bifrost_admin::state",
                    error = %e,
                    "failed to remove group name cache file"
                );
            }
        }
        tracing::info!(
            target: "bifrost_admin::state",
            "group name cache cleared"
        );
    }

    pub fn badge_rules_json(&self) -> String {
        self.badge_rules_cache.read().clone()
    }

    pub fn refresh_badge_rules_cache(&self) {
        let mut items = Vec::new();
        let mut content_parts = Vec::new();

        if let Ok(rules) = self.rules_storage.load_enabled() {
            for rule in rules {
                let rule_count = rule
                    .content
                    .lines()
                    .filter(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with('#')
                    })
                    .count();
                content_parts.push(rule.content.clone());
                items.push(format!(
                    r#"{{"name":{},"rule_count":{},"group_id":null,"group_name":null}}"#,
                    serde_json::to_string(&rule.name).unwrap_or_else(|_| "\"\"".to_string()),
                    rule_count,
                ));
            }
        }

        let base_dir = self.rules_storage.base_dir();
        let has_group_session = self
            .sync_manager
            .as_ref()
            .map(|sm| sm.has_session())
            .unwrap_or(false);
        if has_group_session {
            if let Ok(entries) = std::fs::read_dir(base_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    let group_name = {
                        let cache = self.group_name_cache();
                        // The injected Badge intentionally keeps the local directory name in
                        // group_id and the remote id in group_name. Its script prefers
                        // group_name for the URL parameter, preserving the historical
                        // name/id reversal needed for correct Rules page routing.
                        cache
                            .reverse_lookup(&dir_name)
                            .unwrap_or_else(|| dir_name.clone())
                    };
                    if let Ok(sub_storage) = RulesStorage::with_dir(path) {
                        if let Ok(sub_rules) = sub_storage.load_enabled() {
                            for rule in sub_rules {
                                let rule_count = rule
                                    .content
                                    .lines()
                                    .filter(|l| {
                                        let t = l.trim();
                                        !t.is_empty() && !t.starts_with('#')
                                    })
                                    .count();
                                content_parts.push(rule.content.clone());
                                items.push(format!(
                                r#"{{"name":{},"rule_count":{},"group_id":{},"group_name":{}}}"#,
                                serde_json::to_string(&rule.name)
                                    .unwrap_or_else(|_| "\"\"".to_string()),
                                rule_count,
                                serde_json::to_string(&dir_name)
                                    .unwrap_or_else(|_| "\"\"".to_string()),
                                serde_json::to_string(&group_name)
                                    .unwrap_or_else(|_| "\"\"".to_string()),
                            ));
                            }
                        }
                    }
                }
            }
        }

        let merged = content_parts.join("\n");
        let json = format!(
            r#"{{"rules":[{}],"merged_content":{},"admin_port":{}}}"#,
            items.join(","),
            serde_json::to_string(&merged).unwrap_or_else(|_| "\"\"".to_string()),
            self.port(),
        );
        *self.badge_rules_cache.write() = json;
    }
    pub fn with_keepawake_manager(mut self, mgr: bifrost_power::SharedKeepAwakeManager) -> Self {
        self.keepawake_manager = Some(mgr);
        self
    }

    pub fn ensure_keepawake_manager_installed(mut self) -> Self {
        if self.keepawake_manager.is_some() {
            return self;
        }
        let initial_mode: bifrost_power::Mode =
            match self.config_manager.as_ref().and_then(|cm| cm.try_config()) {
                Some(c) => c.keepawake.mode.parse().unwrap_or_default(),
                None => bifrost_power::Mode::default(),
            };
        let persister: std::sync::Arc<dyn bifrost_power::ModePersister> =
            match self.config_manager.clone() {
                Some(cm) => std::sync::Arc::new(ConfigManagerModePersister { cm }),
                None => std::sync::Arc::new(bifrost_power::NoopPersister),
            };
        self.keepawake_manager = Some(bifrost_power::KeepAwakeManager::new(
            initial_mode,
            persister,
        ));
        self
    }
}

struct ConfigManagerModePersister {
    cm: bifrost_storage::SharedConfigManager,
}

impl bifrost_power::ModePersister for ConfigManagerModePersister {
    fn persist(&self, mode: bifrost_power::Mode) -> std::result::Result<(), String> {
        let cm = self.cm.clone();
        let mode_str = mode.as_str().to_string();
        tokio::spawn(async move {
            let _ = cm
                .update_config(|c| {
                    c.keepawake.mode = mode_str;
                })
                .await;
        });
        Ok(())
    }
}

pub struct GroupNameCacheGuard<'a> {
    guard: parking_lot::MutexGuard<'a, HashMap<String, String>>,
}

impl GroupNameCacheGuard<'_> {
    pub fn get(&self, group_id: &str) -> Option<String> {
        self.guard.get(group_id).cloned()
    }

    pub fn insert(&mut self, group_id: String, name: String) {
        self.guard.insert(group_id, name);
    }

    pub fn reverse_lookup(&self, name: &str) -> Option<String> {
        self.guard
            .iter()
            .find(|(_, v)| v.as_str() == name)
            .map(|(k, _)| k.clone())
    }

    pub fn all_dir_names(&self) -> std::collections::HashSet<String> {
        self.guard.values().cloned().collect()
    }

    pub fn entries(&self) -> Vec<(String, String)> {
        self.guard
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn remove(&mut self, group_id: &str) {
        self.guard.remove(group_id);
    }

    pub fn persist(&self, rules_dir: &std::path::Path) {
        let cache_path = rules_dir.join(".group_cache.json");
        match serde_json::to_string_pretty(&*self.guard) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&cache_path, json) {
                    tracing::warn!(
                        target: "bifrost_admin::state",
                        error = %e,
                        "failed to persist group name cache after cleanup"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::state",
                    error = %e,
                    "failed to serialize group name cache for cleanup persist"
                );
            }
        }
    }
}

pub type SharedAdminState = Arc<AdminState>;

pub fn start_total_disk_cleanup_task(state: SharedAdminState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await;
        loop {
            interval.tick().await;
            let s = state.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || {
                s.cleanup_total_disk_usage_if_needed();
            })
            .await
            {
                tracing::error!("Total disk cleanup task panicked: {}", e);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!(
            "bifrost_state_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            counter
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn cleanup_test_dir(dir: &PathBuf) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reconcile_socket_summary_closes_stale_open_connections() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let mut state = AdminState::new(9900);
        state.traffic_db_store = Some(store.clone());

        let mut record = TrafficRecord::new(
            "stale-open-1".to_string(),
            "CONNECT".to_string(),
            "https://ab.chatgpt.com".to_string(),
        );
        record.status = 200;
        record.is_tunnel = true;
        record.socket_status = Some(SocketStatus {
            is_open: true,
            send_count: 1,
            receive_count: 1,
            send_bytes: 128,
            receive_bytes: 64,
            frame_count: 2,
            close_code: None,
            close_reason: None,
        });
        store.record(record);

        let mut summary = store
            .query(&crate::traffic_db::QueryParams {
                limit: Some(10),
                direction: crate::traffic_db::Direction::Forward,
                ..Default::default()
            })
            .records
            .into_iter()
            .find(|item| item.id == "stale-open-1")
            .expect("summary should exist");

        state.reconcile_socket_summary(&mut summary);

        assert_eq!(summary.ss.as_ref().map(|s| s.is_open), Some(false));

        let persisted = store
            .get_by_id("stale-open-1")
            .expect("record should still exist");
        assert_eq!(
            persisted.socket_status.as_ref().map(|s| s.is_open),
            Some(false)
        );

        cleanup_test_dir(&dir);
    }

    #[test]
    fn reconcile_socket_summary_synthesizes_closed_status_for_missing_socket_state() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let mut state = AdminState::new(9900);
        state.traffic_db_store = Some(store.clone());

        let mut record = TrafficRecord::new(
            "missing-status-1".to_string(),
            "CONNECT".to_string(),
            "https://example.com".to_string(),
        );
        record.status = 200;
        record.is_tunnel = true;
        store.record(record);

        let mut summary = store
            .query(&crate::traffic_db::QueryParams {
                limit: Some(10),
                direction: crate::traffic_db::Direction::Forward,
                ..Default::default()
            })
            .records
            .into_iter()
            .find(|item| item.id == "missing-status-1")
            .expect("summary should exist");

        assert!(summary.ss.is_none());

        state.reconcile_socket_summary(&mut summary);

        assert_eq!(summary.ss.as_ref().map(|s| s.is_open), Some(false));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn reconcile_socket_summary_preserves_sse_counts_when_closing_stale_open_status() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let mut state = AdminState::new(9900);
        state.traffic_db_store = Some(store.clone());

        let mut record = TrafficRecord::new(
            "stale-sse-open-1".to_string(),
            "GET".to_string(),
            "https://example.com/stream".to_string(),
        );
        record.status = 200;
        record.is_sse = true;
        record.content_type = Some("text/event-stream".to_string());
        record.response_size = 4096;
        record.frame_count = 12;
        record.last_frame_id = 12;
        record.socket_status = Some(SocketStatus {
            is_open: true,
            send_count: 0,
            receive_count: 0,
            send_bytes: 0,
            receive_bytes: 0,
            frame_count: 0,
            close_code: None,
            close_reason: None,
        });
        store.record(record);

        let mut summary = store
            .query(&crate::traffic_db::QueryParams {
                limit: Some(10),
                direction: crate::traffic_db::Direction::Forward,
                ..Default::default()
            })
            .records
            .into_iter()
            .find(|item| item.id == "stale-sse-open-1")
            .expect("summary should exist");

        summary.fc = 12;
        summary.res_sz = 4096;

        state.reconcile_socket_summary(&mut summary);

        let socket_status = summary.ss.expect("socket status should exist");
        assert!(!socket_status.is_open);
        assert_eq!(socket_status.frame_count, 12);
        assert_eq!(socket_status.receive_count, 12);
        assert_eq!(socket_status.receive_bytes, 4096);

        let persisted = store
            .get_by_id("stale-sse-open-1")
            .expect("record should still exist");
        let persisted_status = persisted.socket_status.expect("persisted socket status");
        assert!(!persisted_status.is_open);
        assert_eq!(persisted_status.frame_count, 12);
        assert_eq!(persisted_status.receive_count, 12);
        assert_eq!(persisted_status.receive_bytes, 4096);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn reconcile_socket_summary_does_not_persist_empty_sse_close_from_stale_snapshot() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let mut state = AdminState::new(9900);
        state.traffic_db_store = Some(store.clone());

        let mut record = TrafficRecord::new(
            "stale-sse-empty-1".to_string(),
            "GET".to_string(),
            "https://example.com/stream".to_string(),
        );
        record.status = 200;
        record.is_sse = true;
        record.content_type = Some("text/event-stream".to_string());
        record.socket_status = Some(SocketStatus {
            is_open: true,
            send_count: 0,
            receive_count: 0,
            send_bytes: 0,
            receive_bytes: 0,
            frame_count: 0,
            close_code: None,
            close_reason: None,
        });
        store.record(record);

        let mut summary = store
            .query(&crate::traffic_db::QueryParams {
                limit: Some(10),
                direction: crate::traffic_db::Direction::Forward,
                ..Default::default()
            })
            .records
            .into_iter()
            .find(|item| item.id == "stale-sse-empty-1")
            .expect("summary should exist");

        state.reconcile_socket_summary(&mut summary);

        let socket_status = summary.ss.expect("socket status should exist");
        assert!(!socket_status.is_open);
        assert_eq!(socket_status.receive_count, 0);
        assert_eq!(socket_status.receive_bytes, 0);

        let persisted = store
            .get_by_id("stale-sse-empty-1")
            .expect("record should still exist");
        let persisted_status = persisted.socket_status.expect("persisted socket status");
        assert!(persisted_status.is_open);
        assert_eq!(persisted_status.receive_count, 0);
        assert_eq!(persisted_status.receive_bytes, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn group_name_cache_persist_and_load_round_trip() {
        let dir = create_test_dir();
        let rules_dir = dir.join("rules");
        let _ = fs::create_dir_all(&rules_dir);
        let storage = RulesStorage::with_dir(rules_dir.clone()).unwrap();

        let state = AdminState::new_for_test(19900, storage);

        {
            let mut cache = state.group_name_cache();
            cache.insert("g1".to_string(), "GroupAlpha".to_string());
            cache.insert("g2".to_string(), "GroupBeta".to_string());
        }
        state.persist_group_name_cache();

        let cache_file = rules_dir.join(".group_cache.json");
        assert!(cache_file.exists(), "cache file should be written to disk");

        let state2 = AdminState::new_for_test(19901, RulesStorage::with_dir(rules_dir).unwrap());
        state2.load_group_name_cache();

        {
            let cache = state2.group_name_cache();
            assert_eq!(cache.get("g1"), Some("GroupAlpha".to_string()));
            assert_eq!(cache.get("g2"), Some("GroupBeta".to_string()));
            assert_eq!(cache.reverse_lookup("GroupAlpha"), Some("g1".to_string()));
        }

        cleanup_test_dir(&dir);
    }

    #[test]
    fn group_name_cache_load_missing_file_is_noop() {
        let dir = create_test_dir();
        let rules_dir = dir.join("rules");
        let _ = fs::create_dir_all(&rules_dir);
        let storage = RulesStorage::with_dir(rules_dir).unwrap();

        let state = AdminState::new_for_test(19902, storage);

        state.load_group_name_cache();
        let cache = state.group_name_cache();
        assert_eq!(cache.get("any"), None);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn group_name_cache_load_corrupt_file_is_ignored() {
        let dir = create_test_dir();
        let rules_dir = dir.join("rules");
        let _ = fs::create_dir_all(&rules_dir);

        fs::write(rules_dir.join(".group_cache.json"), "not json!!!").unwrap();

        let storage = RulesStorage::with_dir(rules_dir).unwrap();
        let state = AdminState::new_for_test(19903, storage);

        state.load_group_name_cache();
        let cache = state.group_name_cache();
        assert_eq!(cache.get("any"), None);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn group_name_cache_persist_empty_is_noop() {
        let dir = create_test_dir();
        let rules_dir = dir.join("rules");
        let _ = fs::create_dir_all(&rules_dir);
        let storage = RulesStorage::with_dir(rules_dir.clone()).unwrap();

        let state = AdminState::new_for_test(19904, storage);

        state.persist_group_name_cache();
        assert!(
            !rules_dir.join(".group_cache.json").exists(),
            "empty cache should not create file"
        );

        cleanup_test_dir(&dir);
    }

    #[test]
    fn badge_rules_cache_preserves_group_navigation_mapping() {
        let dir = create_test_dir();
        let rules_dir = dir.join("rules");
        let group_dir = rules_dir.join("TeamAlpha");
        let _ = fs::create_dir_all(&group_dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(dir.join("config")).unwrap());
        let sync_manager = Arc::new(bifrost_sync::SyncManager::new(config_manager, 19903).unwrap());
        runtime
            .block_on(sync_manager.save_token("badge-test-token".to_string()))
            .unwrap();
        let storage = RulesStorage::with_dir(rules_dir).unwrap();
        let group_storage = RulesStorage::with_dir(group_dir).unwrap();

        let mut group_rule =
            bifrost_storage::RuleFile::new("team-rule", "team.example.com status://200");
        group_rule.enabled = true;
        group_rule.group = Some("TeamAlpha".to_string());
        group_storage.save(&group_rule).unwrap();

        let state = AdminState::new_for_test(19903, storage).with_sync_manager_shared(sync_manager);
        {
            let mut cache = state.group_name_cache();
            cache.insert("gid-alpha".to_string(), "TeamAlpha".to_string());
        }

        state.refresh_badge_rules_cache();
        let json: serde_json::Value = serde_json::from_str(&state.badge_rules_json()).unwrap();
        let rule = json["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "team-rule")
            .expect("badge rule should exist");

        assert_eq!(rule["group_id"], "TeamAlpha");
        assert_eq!(rule["group_name"], "gid-alpha");

        cleanup_test_dir(&dir);
    }
}
