use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;

use bifrost_core::{ClientAccessControl, SystemProxyManager};
use bifrost_storage::{ConfigManager, RulesStorage, SharedConfigManager, ValuesStorage};
use bifrost_sync::SharedSyncManager;
use parking_lot::RwLock as ParkingRwLock;
use tokio::sync::RwLock;

use crate::app_icon::SharedAppIconCache;
use crate::async_traffic::{AsyncTrafficWriter, SharedAsyncTrafficWriter};
use crate::body_store::SharedBodyStore;
use crate::connection_monitor::{ConnectionMonitor, SharedConnectionMonitor};
use crate::connection_registry::{ConnectionRegistry, SharedConnectionRegistry};
use crate::frame_store::{FrameStore, SharedFrameStore};
use crate::handlers::scripts::ScriptManager;
use crate::metrics::{MetricsCollector, SharedMetricsCollector};
use crate::port_rebind::SharedPortRebindManager;
use crate::replay_db::{ReplayDbStore, SharedReplayDbStore};
use crate::replay_executor::SharedReplayExecutor;
use crate::sse::SseHub;
use crate::traffic::{SocketStatus, TrafficRecord};
use crate::traffic_db::TrafficSummaryCompact;
use crate::traffic_db::{SharedTrafficDbStore, TrafficDbStore};
use crate::version_check::{SharedVersionChecker, VersionChecker};
use crate::ws_payload_store::{SharedWsPayloadStore, WsPayloadStore};
use once_cell::sync::OnceCell;

pub type SharedScriptManager = Arc<RwLock<ScriptManager>>;

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
    pub total_size_cleanup_counter: AtomicUsize,
    pub port_rebind_manager: Option<SharedPortRebindManager>,
    pub sync_manager: Option<SharedSyncManager>,
}

const DEFAULT_MAX_BODY_BUFFER_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_BODY_PROBE_SIZE: usize = 64 * 1024;

impl AdminState {
    pub fn new(port: u16) -> Self {
        Self {
            traffic_db_store: None,
            async_traffic_writer: None,
            metrics_collector: Arc::new(MetricsCollector::default()),
            rules_storage: RulesStorage::default(),
            values_storage: None,
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
            total_size_cleanup_counter: AtomicUsize::new(0),
            port_rebind_manager: None,
            sync_manager: None,
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
        self.maybe_cleanup_total_disk_usage();
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

    fn maybe_cleanup_total_disk_usage(&self) {
        const CLEANUP_CHECK_INTERVAL: usize = 100;
        let counter = self
            .total_size_cleanup_counter
            .fetch_add(1, Ordering::Relaxed);
        if !counter.is_multiple_of(CLEANUP_CHECK_INTERVAL) {
            return;
        }
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

        let orphan_body_ids: Vec<String> = body_sizes
            .keys()
            .filter(|id| !existing_db_ids.contains(id.as_str()))
            .cloned()
            .collect();
        let orphan_frame_ids: Vec<String> = frame_sizes
            .keys()
            .filter(|id| !existing_db_ids.contains(id.as_str()))
            .cloned()
            .collect();
        let orphan_ws_ids: Vec<String> = ws_payload_sizes
            .keys()
            .filter(|id| !existing_db_ids.contains(id.as_str()))
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
                "[TRAFFIC] Cleaned up orphaned cache files"
            );
        }

        if total_size <= max_db_size_bytes {
            return;
        }

        let target_size = max_db_size_bytes.saturating_sub(max_db_size_bytes / 4);
        let bytes_to_remove = total_size.saturating_sub(target_size);
        if bytes_to_remove == 0 {
            return;
        }

        let record_count = existing_db_ids.len();
        if record_count == 0 {
            return;
        }

        let record_count = db_stats.record_count.max(record_count);
        let avg_db_bytes = (db_stats.db_size / record_count as u64).max(1);

        let mut ids_to_delete = Vec::new();
        let mut removed_estimate = 0u64;
        let mut offset = 0usize;
        let batch = 500usize;

        while removed_estimate < bytes_to_remove {
            let ids = traffic_db_store.oldest_ids(batch, offset);
            if ids.is_empty() {
                break;
            }
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
                if removed_estimate >= bytes_to_remove {
                    break;
                }
            }
            offset += batch;
            if offset >= record_count {
                break;
            }
        }

        if ids_to_delete.is_empty() {
            return;
        }

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
        traffic_db_store.compact_db(false);

        tracing::info!(
            deleted = ids_to_delete.len(),
            total_size = total_size,
            max_db_size_bytes = max_db_size_bytes,
            target_size = target_size,
            "[TRAFFIC] Cleaned up data due to total size limit"
        );
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

    pub fn set_replay_executor(&self, executor: SharedReplayExecutor) {
        let _ = self.replay_executor.set(executor);
    }

    pub fn get_replay_executor(&self) -> Option<&SharedReplayExecutor> {
        self.replay_executor.get()
    }
}

pub type SharedAdminState = Arc<AdminState>;

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
}
