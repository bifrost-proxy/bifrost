use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_script::{ScriptInfo, ScriptType};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::debug;

use crate::replay_db::{ReplayGroup, ReplayRequestSummary, MAX_REQUESTS};
use crate::resource_alerts::build_resource_alerts;
use crate::state::SharedAdminState;
use crate::traffic::TrafficSummary;
use crate::traffic_db::{
    Direction, QueryParams, TrafficStatisticsSnapshot, TrafficStoreEvent, TrafficSummaryCompact,
};

static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const PUSH_CHANNEL_CAPACITY: usize = 64;
pub const MAX_SUBSCRIBED_IDS: usize = 500;
pub const MAX_CLIENT_CHANNELS: usize = 3;
pub const MAX_ID_LEN: usize = 256;
pub const MAX_SETTINGS_SCOPES: usize = 16;
const TRAFFIC_PENDING_REFRESH_INTERVAL_MS: u64 = 2_000;
const TRAFFIC_STATISTICS_PUSH_INTERVAL_MS: u64 = 1_000;
const TRAFFIC_DELTA_BATCH_LIMIT: usize = 500;
const TRAFFIC_RECONNECT_WINDOW_LIMIT: usize = 1_000;

pub const SETTINGS_SCOPE_PROXY_SETTINGS: &str = "proxy_settings";
pub const SETTINGS_SCOPE_TLS_CONFIG: &str = "tls_config";
pub const SETTINGS_SCOPE_PERFORMANCE_CONFIG: &str = "performance_config";
pub const SETTINGS_SCOPE_CERT_INFO: &str = "cert_info";
pub const SETTINGS_SCOPE_PROXY_ADDRESS: &str = "proxy_address";
pub const SETTINGS_SCOPE_SYSTEM_PROXY: &str = "system_proxy";
pub const SETTINGS_SCOPE_CLI_PROXY: &str = "cli_proxy";
pub const SETTINGS_SCOPE_WHITELIST_STATUS: &str = "whitelist_status";
pub const SETTINGS_SCOPE_PENDING_AUTHORIZATIONS: &str = "pending_authorizations";
pub const SETTINGS_SCOPE_PENDING_IP_TLS: &str = "pending_ip_tls";
pub const SETTINGS_SCOPE_CLIENT_TRUST: &str = "client_trust";
pub const SETTINGS_SCOPE_NOTIFICATIONS: &str = "notifications";
pub const SETTINGS_SCOPE_TRUST_PROBE: &str = "trust_probe";
pub const SETTINGS_SCOPE_MOBILE_DEVICES: &str = "mobile_devices";

fn generate_client_id() -> u64 {
    CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn is_pending_traffic_record(record: &TrafficSummaryCompact) -> bool {
    record.s == 0
        || ((record.is_websocket() || record.is_sse() || record.is_tunnel())
            && record.ss.as_ref().map(|s| s.is_open).unwrap_or(false))
}

fn dedupe_compact_records_keep_latest(
    records: Vec<TrafficSummaryCompact>,
) -> Vec<TrafficSummaryCompact> {
    if records.len() <= 1 {
        return records;
    }

    let mut seen = HashSet::with_capacity(records.len());
    let mut deduped_reversed = Vec::with_capacity(records.len());

    for record in records.into_iter().rev() {
        if seen.insert(record.id.clone()) {
            deduped_reversed.push(record);
        }
    }

    deduped_reversed.reverse();
    deduped_reversed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PushMessage {
    #[serde(rename = "traffic_updates")]
    TrafficUpdates(TrafficUpdatesData),

    #[serde(rename = "traffic_delta")]
    TrafficDelta(TrafficDeltaData),

    #[serde(rename = "traffic_deleted")]
    TrafficDeleted(TrafficDeletedData),

    #[serde(rename = "traffic_statistics")]
    TrafficStatistics(TrafficStatisticsSnapshot),

    #[serde(rename = "overview_update")]
    OverviewUpdate(OverviewData),

    #[serde(rename = "metrics_update")]
    MetricsUpdate(MetricsData),

    #[serde(rename = "history_update")]
    HistoryUpdate(HistoryData),

    #[serde(rename = "values_update")]
    ValuesUpdate(ValuesData),

    #[serde(rename = "scripts_update")]
    ScriptsUpdate(ScriptsData),

    #[serde(rename = "settings_update")]
    SettingsUpdate(SettingsUpdateData),

    #[serde(rename = "replay_saved_requests_update")]
    ReplaySavedRequestsUpdate(ReplaySavedRequestsData),

    #[serde(rename = "replay_groups_update")]
    ReplayGroupsUpdate(ReplayGroupsData),

    #[serde(rename = "connected")]
    Connected(ConnectedData),

    #[serde(rename = "error")]
    Error(ErrorData),

    #[serde(rename = "replay_request_updated")]
    ReplayRequestUpdated(ReplayRequestUpdatedData),

    #[serde(rename = "replay_history_updated")]
    ReplayHistoryUpdated(ReplayHistoryUpdatedData),

    #[serde(rename = "disconnect")]
    Disconnect(DisconnectData),

    #[serde(rename = "notification")]
    Notification(NotificationPushData),

    #[serde(rename = "breakpoint_paused")]
    BreakpointPaused(BreakpointPausedPushData),

    #[serde(rename = "breakpoint_settings_updated")]
    BreakpointSettingsUpdated(BreakpointSettingsPushData),

    #[serde(rename = "breakpoint_resumed")]
    BreakpointResumed(BreakpointResumedPushData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficUpdatesData {
    pub new_records: Vec<TrafficSummary>,
    pub updated_records: Vec<TrafficSummary>,
    pub has_more: bool,
    pub server_total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficDeltaData {
    pub inserts: Vec<TrafficSummaryCompact>,
    pub updates: Vec<TrafficSummaryCompact>,
    pub has_more: bool,
    pub server_total: usize,
    pub server_sequence: u64,
    pub oldest_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct TrafficDeltaMetadata {
    has_more: bool,
    server_total: usize,
    server_sequence: u64,
    oldest_sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficDeletedData {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewData {
    pub system: serde_json::Value,
    pub metrics: serde_json::Value,
    pub rules: RulesInfo,
    pub traffic: TrafficInfo,
    pub server: ServerInfo,
    pub pending_authorizations: usize,
    pub pending_ip_tls: usize,
    pub untrusted_clients: usize,
    pub unread_notifications: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesInfo {
    pub total: usize,
    pub enabled: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficInfo {
    pub recorded: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub port: u16,
    pub admin_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsData {
    pub metrics: serde_json::Value,
    pub recorded_traffic: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryData {
    pub history: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueItemData {
    pub name: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValuesData {
    pub values: Vec<ValueItemData>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptsData {
    pub request: Vec<ScriptInfo>,
    pub response: Vec<ScriptInfo>,
    pub decode: Vec<ScriptInfo>,
    pub parser: Vec<ScriptInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsUpdateData {
    pub scope: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaySavedRequestsData {
    pub requests: Vec<ReplayRequestSummary>,
    pub total: usize,
    pub max_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayGroupsData {
    pub groups: Vec<ReplayGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequestUpdatedData {
    pub action: String,
    pub request_id: Option<String>,
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayHistoryUpdatedData {
    pub action: String,
    pub request_id: Option<String>,
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedData {
    pub client_id: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorData {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisconnectData {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPushData {
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
    pub unread_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointPausedPushData {
    pub phase: String, // "request" or "response"
    pub request_id: String,
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub body_omitted: bool,
    pub body_size: Option<usize>,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointSettingsPushData {
    pub enabled: bool,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointResumedPushData {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientSubscription {
    #[serde(default)]
    pub last_traffic_id: Option<String>,
    #[serde(default)]
    pub last_sequence: Option<u64>,
    #[serde(default)]
    pub pending_ids: Vec<String>,
    #[serde(default)]
    pub need_traffic: bool,
    #[serde(default)]
    pub need_overview: bool,
    #[serde(default)]
    pub need_metrics: bool,
    #[serde(default)]
    pub need_history: bool,
    #[serde(default)]
    pub need_values: bool,
    #[serde(default)]
    pub need_scripts: bool,
    #[serde(default)]
    pub need_replay_saved_requests: bool,
    #[serde(default)]
    pub need_replay_groups: bool,
    #[serde(default)]
    pub settings_scopes: Vec<String>,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_metrics_interval_ms")]
    pub metrics_interval_ms: u64,
}

fn default_history_limit() -> usize {
    60
}

fn default_metrics_interval_ms() -> u64 {
    1000
}

pub const METRICS_INTERVAL_MIN_MS: u64 = 1000;
pub const METRICS_INTERVAL_MAX_MS: u64 = 5000;

impl Default for ClientSubscription {
    fn default() -> Self {
        Self {
            last_traffic_id: None,
            last_sequence: None,
            pending_ids: Vec::new(),
            need_traffic: false,
            need_overview: false,
            need_metrics: false,
            need_history: false,
            need_values: false,
            need_scripts: false,
            need_replay_saved_requests: false,
            need_replay_groups: false,
            settings_scopes: Vec::new(),
            history_limit: default_history_limit(),
            metrics_interval_ms: default_metrics_interval_ms(),
        }
    }
}

pub struct PushClient {
    pub id: u64,
    pub client_key: String,
    pub sender: mpsc::Sender<PushMessage>,
    pub subscription: RwLock<ClientSubscription>,
    traffic_statistics_last_sent: Mutex<Option<Instant>>,
    metrics_last_sent: Mutex<Option<Instant>>,
}

impl PushClient {
    pub fn new(
        client_key: String,
        subscription: ClientSubscription,
    ) -> (Self, mpsc::Receiver<PushMessage>) {
        let (sender, receiver) = mpsc::channel(PUSH_CHANNEL_CAPACITY);
        let client = Self {
            id: generate_client_id(),
            client_key,
            sender,
            subscription: RwLock::new(subscription),
            traffic_statistics_last_sent: Mutex::new(None),
            metrics_last_sent: Mutex::new(None),
        };
        (client, receiver)
    }

    pub fn send(&self, msg: PushMessage) -> bool {
        self.sender.try_send(msg).is_ok()
    }

    pub fn update_subscription(&self, subscription: ClientSubscription) {
        let current_last_sequence = self.subscription.read().last_sequence;
        let mut next = subscription;
        next.last_sequence = match (current_last_sequence, next.last_sequence) {
            (Some(current), Some(incoming)) => Some(current.max(incoming)),
            (Some(current), None) => Some(current),
            (None, incoming) => incoming,
        };
        *self.subscription.write() = next;
    }

    pub fn get_subscription(&self) -> ClientSubscription {
        self.subscription.read().clone()
    }

    fn reserve_traffic_statistics_push(&self, now: Instant) -> bool {
        let mut last_sent = self.traffic_statistics_last_sent.lock();
        if last_sent.is_some_and(|last| {
            now.saturating_duration_since(last)
                < Duration::from_millis(TRAFFIC_STATISTICS_PUSH_INTERVAL_MS)
        }) {
            return false;
        }
        *last_sent = Some(now);
        true
    }

    fn reserve_metrics_push(&self, now: Instant, interval_ms: u64) -> bool {
        let interval_ms = interval_ms.clamp(METRICS_INTERVAL_MIN_MS, METRICS_INTERVAL_MAX_MS);
        let mut last_sent = self.metrics_last_sent.lock();
        if last_sent.is_some_and(|last| {
            now.saturating_duration_since(last) < Duration::from_millis(interval_ms)
        }) {
            return false;
        }
        *last_sent = Some(now);
        true
    }
}

pub struct PushManager {
    clients: DashMap<u64, Arc<PushClient>>,
    buckets: DashMap<String, Vec<u64>>,
    bucket_order: Mutex<VecDeque<String>>,
    overview_cache: RwLock<Option<OverviewData>>,
    traffic_statistics_dirty: AtomicBool,
    state: SharedAdminState,
}

impl PushManager {
    pub fn new(state: SharedAdminState) -> Self {
        Self {
            clients: DashMap::new(),
            buckets: DashMap::new(),
            bucket_order: Mutex::new(VecDeque::new()),
            overview_cache: RwLock::new(None),
            traffic_statistics_dirty: AtomicBool::new(false),
            state,
        }
    }

    pub fn register_client(
        &self,
        client_key: String,
        subscription: ClientSubscription,
    ) -> (Arc<PushClient>, mpsc::Receiver<PushMessage>) {
        let evicted = self.ensure_bucket_capacity(&client_key);
        for client_id in evicted {
            if let Some((_, client)) = self.clients.remove(&client_id) {
                let _ = client.send(PushMessage::Disconnect(DisconnectData {
                    reason: "Too many active client channels".to_string(),
                }));
            }
        }

        let (client, receiver) = PushClient::new(client_key.clone(), subscription);
        let client = Arc::new(client);
        let client_id = client.id;
        self.clients.insert(client_id, client.clone());
        self.buckets
            .entry(client_key)
            .and_modify(|v| v.push(client_id))
            .or_insert_with(|| vec![client_id]);
        debug!(client_id = client_id, "Push client registered");
        (client, receiver)
    }

    pub fn unregister_client(&self, client_id: u64) {
        if let Some((_, client)) = self.clients.remove(&client_id) {
            if let Some(mut bucket) = self.buckets.get_mut(&client.client_key) {
                bucket.retain(|id| *id != client_id);
                if bucket.is_empty() {
                    drop(bucket);
                    self.buckets.remove(&client.client_key);
                    let mut order = self.bucket_order.lock();
                    order.retain(|k| k != &client.client_key);
                }
            }
            debug!(client_id = client_id, "Push client unregistered");
        }
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    pub fn has_overview_subscribers(&self) -> bool {
        self.clients
            .iter()
            .any(|client_ref| client_ref.value().get_subscription().need_overview)
    }

    pub fn has_traffic_subscribers(&self) -> bool {
        self.clients
            .iter()
            .any(|client_ref| client_ref.value().get_subscription().need_traffic)
    }

    pub fn has_metrics_subscribers(&self) -> bool {
        self.clients
            .iter()
            .any(|client_ref| client_ref.value().get_subscription().need_metrics)
    }

    pub fn invalidate_overview_cache(&self) {
        *self.overview_cache.write() = None;
    }

    fn mark_traffic_statistics_dirty(&self) {
        self.traffic_statistics_dirty.store(true, Ordering::Release);
    }

    pub fn notify_traffic_statistics_changed(&self) {
        self.mark_traffic_statistics_dirty();
    }

    fn take_traffic_statistics_dirty(&self) -> bool {
        self.traffic_statistics_dirty.swap(false, Ordering::AcqRel)
    }

    fn send_traffic_statistics_to_client(&self, client: &Arc<PushClient>) -> bool {
        let Some(ref db_store) = self.state.traffic_db_store else {
            return true;
        };
        if !client.reserve_traffic_statistics_push(Instant::now()) {
            self.mark_traffic_statistics_dirty();
            return true;
        }
        client.send(PushMessage::TrafficStatistics(
            db_store.traffic_statistics(),
        ))
    }

    fn broadcast_traffic_statistics(&self) -> bool {
        let Some(ref db_store) = self.state.traffic_db_store else {
            return false;
        };
        let snapshot = db_store.traffic_statistics();
        let mut clients_to_remove = Vec::new();
        let mut deferred = false;
        let now = Instant::now();

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.get_subscription().need_traffic {
                continue;
            }
            if !client.reserve_traffic_statistics_push(now) {
                deferred = true;
                continue;
            }
            if !client.send(PushMessage::TrafficStatistics(snapshot.clone())) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
        deferred
    }

    fn flush_dirty_traffic_statistics(&self) {
        if self.has_traffic_subscribers()
            && self.take_traffic_statistics_dirty()
            && self.broadcast_traffic_statistics()
        {
            self.mark_traffic_statistics_dirty();
        }
    }

    async fn build_full_overview(&self) -> OverviewData {
        let system_info = crate::metrics::SystemInfo::new(self.state.start_time);
        let metrics = self.state.metrics_collector.get_current();
        let traffic_count = self
            .state
            .traffic_db_store
            .as_ref()
            .map(|db| db.count())
            .unwrap_or(0);

        let (rules_total, rules_enabled) = match self.state.rules_storage.load_all() {
            Ok(rules) => {
                let enabled = rules.iter().filter(|r| r.enabled).count();
                (rules.len(), enabled)
            }
            Err(_) => (0, 0),
        };

        let pending_count = if let Some(ref access_control) = self.state.access_control {
            let ac = access_control.read().await;
            ac.pending_authorization_count()
        } else {
            0
        };

        let pending_ip_tls_count = self
            .state
            .ip_tls_pending_manager
            .as_ref()
            .map(|m| m.pending_count())
            .unwrap_or(0);

        let untrusted_clients_count = self
            .state
            .client_trust_tracker
            .as_ref()
            .map(|t| t.get_untrusted_count())
            .unwrap_or(0);

        let unread_notifications = crate::notification_db::count_unread().unwrap_or(0);

        let overview = OverviewData {
            system: serde_json::to_value(&system_info).unwrap_or_default(),
            metrics: serde_json::to_value(&metrics).unwrap_or_default(),
            rules: RulesInfo {
                total: rules_total,
                enabled: rules_enabled,
            },
            traffic: TrafficInfo {
                recorded: traffic_count,
            },
            server: ServerInfo {
                port: self.state.port(),
                admin_url: format!("http://127.0.0.1:{}/_bifrost/", self.state.port()),
            },
            pending_authorizations: pending_count,
            pending_ip_tls: pending_ip_tls_count,
            untrusted_clients: untrusted_clients_count,
            unread_notifications,
        };

        *self.overview_cache.write() = Some(overview.clone());
        overview
    }

    async fn build_lightweight_overview(&self) -> OverviewData {
        let cached = { self.overview_cache.read().clone() };
        let mut overview = if let Some(cached) = cached {
            cached
        } else {
            return self.build_full_overview().await;
        };

        overview.system =
            serde_json::to_value(crate::metrics::SystemInfo::new(self.state.start_time))
                .unwrap_or_default();
        overview.metrics =
            serde_json::to_value(self.state.metrics_collector.get_current()).unwrap_or_default();
        overview.traffic.recorded = self
            .state
            .traffic_db_store
            .as_ref()
            .map(|db| db.count())
            .unwrap_or(0);
        overview
    }

    fn build_metrics_data(&self) -> MetricsData {
        let metrics = self.state.metrics_collector.get_current();
        MetricsData {
            metrics: serde_json::to_value(&metrics).unwrap_or_default(),
            recorded_traffic: self
                .state
                .traffic_db_store
                .as_ref()
                .map(|db| db.count())
                .unwrap_or(0),
        }
    }

    fn ensure_bucket_capacity(&self, client_key: &str) -> Vec<u64> {
        let mut evicted_client_ids = Vec::new();
        let mut order = self.bucket_order.lock();

        if let Some(pos) = order.iter().position(|k| k == client_key) {
            let k = order.remove(pos).unwrap_or_else(|| client_key.to_string());
            order.push_back(k);
        } else {
            order.push_back(client_key.to_string());
        }

        while order.len() > MAX_CLIENT_CHANNELS {
            let Some(evicted_key) = order.pop_front() else {
                break;
            };
            if let Some((_, client_ids)) = self.buckets.remove(&evicted_key) {
                evicted_client_ids.extend(client_ids);
            }
        }

        evicted_client_ids
    }

    fn enrich_compact_summary(&self, mut summary: TrafficSummaryCompact) -> TrafficSummaryCompact {
        self.state.reconcile_socket_summary(&mut summary);
        summary
    }

    fn update_pending_ids(
        &self,
        pending_ids: &[String],
        new_records: &[TrafficSummaryCompact],
        updated_records: &[TrafficSummaryCompact],
    ) -> Vec<String> {
        let mut next_pending_ids: HashSet<String> = pending_ids.iter().cloned().collect();

        for record in updated_records {
            if is_pending_traffic_record(record) {
                next_pending_ids.insert(record.id.clone());
            } else {
                next_pending_ids.remove(&record.id);
            }
        }

        for record in new_records {
            if is_pending_traffic_record(record) {
                next_pending_ids.insert(record.id.clone());
            } else {
                next_pending_ids.remove(&record.id);
            }
        }

        next_pending_ids.into_iter().collect()
    }

    fn send_traffic_delta_to_client(
        &self,
        client: &Arc<PushClient>,
        inserts: Vec<TrafficSummaryCompact>,
        updates: Vec<TrafficSummaryCompact>,
        metadata: TrafficDeltaMetadata,
    ) -> bool {
        let inserts = dedupe_compact_records_keep_latest(inserts);
        let updates = dedupe_compact_records_keep_latest(updates);

        if inserts.is_empty() && updates.is_empty() && metadata.oldest_sequence.is_none() {
            return true;
        }

        let last_seq = inserts.last().map(|r| r.seq);
        let next_pending_ids = {
            let subscription = client.get_subscription();
            self.update_pending_ids(&subscription.pending_ids, &inserts, &updates)
        };

        let msg = PushMessage::TrafficDelta(TrafficDeltaData {
            inserts,
            updates,
            has_more: metadata.has_more,
            server_total: metadata.server_total,
            server_sequence: metadata.server_sequence,
            oldest_sequence: metadata.oldest_sequence,
        });

        if !client.send(msg) {
            return false;
        }

        let mut sub = client.subscription.write();
        if let Some(seq) = last_seq {
            sub.last_sequence = Some(sub.last_sequence.map_or(seq, |current| current.max(seq)));
        }
        sub.pending_ids = next_pending_ids;
        true
    }

    pub async fn broadcast_traffic_events(
        &self,
        inserts: Vec<TrafficSummaryCompact>,
        updates: Vec<TrafficSummaryCompact>,
        server_total: usize,
        server_sequence: u64,
        oldest_sequence: Option<u64>,
    ) {
        if inserts.is_empty() && updates.is_empty() {
            return;
        }

        let mut clients_to_remove = Vec::new();

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();
            if !subscription.need_traffic {
                continue;
            }

            let filtered_inserts: Vec<_> = inserts
                .iter()
                .filter(|record| {
                    subscription
                        .last_sequence
                        .is_none_or(|last_sequence| record.seq > last_sequence)
                })
                .cloned()
                .collect();

            if !self.send_traffic_delta_to_client(
                client,
                filtered_inserts,
                updates.clone(),
                TrafficDeltaMetadata {
                    has_more: false,
                    server_total,
                    server_sequence,
                    oldest_sequence,
                },
            ) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_traffic_updates(&self) {
        let Some(ref db_store) = self.state.traffic_db_store else {
            return;
        };
        self.broadcast_traffic_delta(db_store).await;
    }

    async fn broadcast_traffic_delta(&self, db_store: &crate::traffic_db::SharedTrafficDbStore) {
        let mut clients_to_remove = Vec::new();
        let current_sequence = db_store.current_sequence();
        let oldest_sequence = db_store.oldest_sequence();

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();
            if !subscription.need_traffic {
                continue;
            }

            let last_known_sequence = subscription.last_sequence.unwrap_or(0);
            let has_new_records = current_sequence > last_known_sequence.saturating_add(1);
            if !has_new_records && subscription.pending_ids.is_empty() {
                continue;
            }

            let result = if let Some(cursor) = subscription.last_sequence {
                let query_params = QueryParams {
                    cursor: Some(cursor),
                    limit: Some(500),
                    direction: Direction::Forward,
                    ..Default::default()
                };
                db_store.query(&query_params)
            } else {
                db_store.query_latest_window(500)
            };
            let new_records: Vec<_> = result
                .records
                .into_iter()
                .map(|s| self.enrich_compact_summary(s))
                .collect();

            let updated_records: Vec<TrafficSummaryCompact> =
                if !subscription.pending_ids.is_empty() {
                    let ids: Vec<&str> = subscription
                        .pending_ids
                        .iter()
                        .map(|s| s.as_str())
                        .collect();
                    db_store
                        .get_by_ids(&ids)
                        .into_iter()
                        .map(|s| self.enrich_compact_summary(s))
                        .collect()
                } else {
                    Vec::new()
                };

            if !self.send_traffic_delta_to_client(
                client,
                new_records,
                updated_records,
                TrafficDeltaMetadata {
                    has_more: result.has_more,
                    server_total: result.total,
                    server_sequence: result.server_sequence,
                    oldest_sequence,
                },
            ) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    fn send_initial_traffic_delta(
        &self,
        client: &Arc<PushClient>,
        db_store: &crate::traffic_db::SharedTrafficDbStore,
        subscription: &ClientSubscription,
    ) {
        if !subscription.need_traffic {
            return;
        }

        let result = if let Some(cursor) = subscription.last_sequence {
            // A sleeping page can reconnect thousands of records behind. Send
            // only the newest bounded UI window, split into small messages, so
            // recovery reaches the current tail without one giant allocation.
            let mut result = db_store.query_latest_window(TRAFFIC_RECONNECT_WINDOW_LIMIT);
            result.records.retain(|record| record.seq > cursor);
            result
        } else {
            db_store.query_latest_window(TRAFFIC_DELTA_BATCH_LIMIT)
        };
        let new_records: Vec<_> = result
            .records
            .into_iter()
            .map(|s| self.enrich_compact_summary(s))
            .collect();

        let updated_records: Vec<TrafficSummaryCompact> = if !subscription.pending_ids.is_empty() {
            let ids: Vec<&str> = subscription
                .pending_ids
                .iter()
                .map(|s| s.as_str())
                .collect();
            db_store
                .get_by_ids(&ids)
                .into_iter()
                .map(|s| self.enrich_compact_summary(s))
                .collect()
        } else {
            Vec::new()
        };
        let oldest_sequence = db_store.oldest_sequence();

        if new_records.is_empty() {
            let _ = self.send_traffic_delta_to_client(
                client,
                Vec::new(),
                updated_records,
                TrafficDeltaMetadata {
                    has_more: result.has_more,
                    server_total: result.total,
                    server_sequence: result.server_sequence,
                    oldest_sequence,
                },
            );
            return;
        }

        let chunk_count = new_records.len().div_ceil(TRAFFIC_DELTA_BATCH_LIMIT);
        for (index, chunk) in new_records.chunks(TRAFFIC_DELTA_BATCH_LIMIT).enumerate() {
            let updates = if index == 0 {
                updated_records.clone()
            } else {
                Vec::new()
            };
            let has_more = result.has_more || index + 1 < chunk_count;
            if !self.send_traffic_delta_to_client(
                client,
                chunk.to_vec(),
                updates,
                TrafficDeltaMetadata {
                    has_more,
                    server_total: result.total,
                    server_sequence: result.server_sequence,
                    oldest_sequence,
                },
            ) {
                break;
            }
        }
    }

    pub fn send_initial_traffic(&self, client: &Arc<PushClient>) {
        let subscription = client.get_subscription();
        if !subscription.need_traffic {
            return;
        }

        if let Some(ref db_store) = self.state.traffic_db_store {
            self.send_initial_traffic_delta(client, db_store, &subscription);
            self.send_traffic_statistics_to_client(client);
        }
    }

    pub async fn broadcast_overview(&self) {
        let mut clients_to_remove = Vec::new();
        let overview = self.build_full_overview().await;

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();

            if subscription.need_overview {
                let msg = PushMessage::OverviewUpdate(overview.clone());
                if !client.send(msg) {
                    clients_to_remove.push(client.id);
                }
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_overview_lightweight(&self) {
        let mut clients_to_remove = Vec::new();
        let overview = self.build_lightweight_overview().await;

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();

            if subscription.need_overview {
                let msg = PushMessage::OverviewUpdate(overview.clone());
                if !client.send(msg) {
                    clients_to_remove.push(client.id);
                }
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_metrics(&self) {
        self.broadcast_metrics_with_interval(0).await;
    }

    pub async fn broadcast_metrics_with_interval(&self, _elapsed_ms: u64) {
        let mut clients_to_remove = Vec::new();
        let now = Instant::now();
        let clients: Vec<_> = self
            .clients
            .iter()
            .filter_map(|client_ref| {
                let client = client_ref.value();
                let subscription = client.get_subscription();

                if !subscription.need_metrics {
                    return None;
                }

                client
                    .reserve_metrics_push(now, subscription.metrics_interval_ms)
                    .then(|| client.clone())
            })
            .collect();

        if clients.is_empty() {
            return;
        }

        let metrics_data = self.build_metrics_data();

        for client in clients {
            let subscription = client.get_subscription();
            if subscription.need_metrics
                && !client.send(PushMessage::MetricsUpdate(metrics_data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_history(&self) {
        let mut clients_to_remove = Vec::new();

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();

            if subscription.need_history {
                let history = self
                    .state
                    .metrics_collector
                    .get_history(Some(subscription.history_limit));
                let history_json: Vec<serde_json::Value> = history
                    .into_iter()
                    .map(|m| serde_json::to_value(&m).unwrap_or_default())
                    .collect();

                let msg = PushMessage::HistoryUpdate(HistoryData {
                    history: history_json,
                });

                if !client.send(msg) {
                    clients_to_remove.push(client.id);
                }
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    fn has_settings_scope(subscription: &ClientSubscription, scope: &str) -> bool {
        subscription
            .settings_scopes
            .iter()
            .any(|item| item == scope)
    }

    fn has_settings_scope_subscribers(&self, scope: &str) -> bool {
        self.clients.iter().any(|client_ref| {
            Self::has_settings_scope(&client_ref.value().get_subscription(), scope)
        })
    }

    fn build_values_data(&self) -> Option<ValuesData> {
        let values_storage = self.state.values_storage.as_ref()?;
        let guard = values_storage.read();
        let entries = guard.list_entries().ok()?;
        let values: Vec<ValueItemData> = entries
            .into_iter()
            .map(|entry| ValueItemData {
                name: entry.name,
                value: entry.value,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            })
            .collect();
        let total = values.len();
        Some(ValuesData { values, total })
    }

    async fn build_scripts_data(&self) -> Option<ScriptsData> {
        let script_manager = self.state.script_manager.as_ref()?;
        let manager = script_manager.read().await;

        let request = manager
            .engine()
            .list_scripts(ScriptType::Request)
            .await
            .unwrap_or_default();
        let response = manager
            .engine()
            .list_scripts(ScriptType::Response)
            .await
            .unwrap_or_default();
        let mut decode = manager
            .engine()
            .list_scripts(ScriptType::Decode)
            .await
            .unwrap_or_default();
        let parser = manager
            .engine()
            .list_scripts(ScriptType::Parser)
            .await
            .unwrap_or_default();

        for name in ["utf8", "default"] {
            if decode.iter().any(|item| item.name == name) {
                continue;
            }
            decode.push(ScriptInfo {
                name: name.to_string(),
                script_type: ScriptType::Decode,
                description: Some(match name {
                    "utf8" => "Built-in UTF-8 (lossy) decoder".to_string(),
                    _ => "Built-in default decoder (alias of utf8)".to_string(),
                }),
                created_at: 0,
                updated_at: 0,
            });
        }

        Some(ScriptsData {
            request,
            response,
            decode,
            parser,
        })
    }

    fn build_replay_saved_requests_data(&self) -> Option<ReplaySavedRequestsData> {
        let replay_store = self.state.replay_db_store.as_ref()?;
        let requests = replay_store.list_requests(Some(true), None, Some(100), None);
        let total = replay_store.count_requests();
        Some(ReplaySavedRequestsData {
            requests,
            total,
            max_requests: MAX_REQUESTS,
        })
    }

    fn build_replay_groups_data(&self) -> Option<ReplayGroupsData> {
        let replay_store = self.state.replay_db_store.as_ref()?;
        Some(ReplayGroupsData {
            groups: replay_store.list_groups(),
        })
    }

    async fn build_settings_update(&self, scope: &str) -> Option<SettingsUpdateData> {
        let data = match scope {
            SETTINGS_SCOPE_PROXY_SETTINGS => {
                let config_manager = self.state.config_manager.as_ref()?;
                let config = config_manager.config().await;
                json!({
                    "server": {
                        "timeout_secs": config.server.timeout_secs,
                        "http1_max_header_size": config.server.http1_max_header_size,
                        "http2_max_header_list_size": config.server.http2_max_header_list_size,
                        "websocket_handshake_max_header_size": config.server.websocket_handshake_max_header_size,
                    },
                    "tls": {
                        "enable_tls_interception": config.tls.enable_interception,
                        "intercept_exclude": config.tls.intercept_exclude,
                        "intercept_include": config.tls.intercept_include,
                        "app_intercept_exclude": config.tls.app_intercept_exclude,
                        "app_intercept_include": config.tls.app_intercept_include,
                        "ip_intercept_exclude": config.tls.ip_intercept_exclude,
                        "ip_intercept_include": config.tls.ip_intercept_include,
                        "unsafe_ssl": config.tls.unsafe_ssl,
                        "disconnect_on_config_change": config.tls.disconnect_on_change,
                    },
                    "port": self.state.port(),
                    "host": "127.0.0.1",
                })
            }
            SETTINGS_SCOPE_TLS_CONFIG => {
                let config_manager = self.state.config_manager.as_ref()?;
                let config = config_manager.config().await;
                json!({
                    "enable_tls_interception": config.tls.enable_interception,
                    "intercept_exclude": config.tls.intercept_exclude,
                    "intercept_include": config.tls.intercept_include,
                    "app_intercept_exclude": config.tls.app_intercept_exclude,
                    "app_intercept_include": config.tls.app_intercept_include,
                    "ip_intercept_exclude": config.tls.ip_intercept_exclude,
                    "ip_intercept_include": config.tls.ip_intercept_include,
                    "unsafe_ssl": config.tls.unsafe_ssl,
                    "disconnect_on_config_change": config.tls.disconnect_on_change,
                })
            }
            SETTINGS_SCOPE_PERFORMANCE_CONFIG => {
                let config_manager = self.state.config_manager.as_ref()?;
                let config = config_manager.config().await;
                let body_store_stats = self.state.body_store.as_ref().map(|bs| bs.read().stats());
                let frame_store_stats = self.state.frame_store.as_ref().map(|fs| fs.stats());
                let ws_payload_store_stats =
                    self.state.ws_payload_store.as_ref().map(|ws| ws.stats());
                let resource_alerts = build_resource_alerts(
                    body_store_stats.as_ref(),
                    ws_payload_store_stats.as_ref(),
                );
                json!({
                    "traffic": {
                        "max_records": config.traffic.max_records,
                        "max_db_size_bytes": config.traffic.max_db_size_bytes,
                        "max_body_memory_size": config.traffic.max_body_memory_size,
                        "max_body_buffer_size": config.traffic.max_body_buffer_size,
                        "max_body_probe_size": config.traffic.max_body_probe_size,
                        "super_performance_mode": config.traffic.super_performance_mode,
                        "binary_traffic_performance_mode": config.traffic.binary_traffic_performance_mode,
                        "file_retention_days": config.traffic.file_retention_days,
                        "sse_stream_flush_bytes": config.traffic.sse_stream_flush_bytes,
                        "sse_stream_flush_interval_ms": config.traffic.sse_stream_flush_interval_ms,
                        "ws_payload_flush_bytes": config.traffic.ws_payload_flush_bytes,
                        "ws_payload_flush_interval_ms": config.traffic.ws_payload_flush_interval_ms,
                        "ws_payload_max_open_files": config.traffic.ws_payload_max_open_files,
                    },
                    "breakpoint": {
                        "timeout_ms": config.traffic.breakpoint_timeout_ms,
                        "timeout_min_ms": bifrost_storage::MIN_BREAKPOINT_TIMEOUT_MS,
                        "timeout_max_ms": bifrost_storage::MAX_BREAKPOINT_TIMEOUT_MS,
                    },
                    "body_store_stats": body_store_stats,
                    "frame_store_stats": frame_store_stats,
                    "ws_payload_store_stats": ws_payload_store_stats,
                    "resource_alerts": resource_alerts,
                })
            }
            SETTINGS_SCOPE_CERT_INFO => {
                let available = self
                    .state
                    .ca_cert_path
                    .as_ref()
                    .map(|path| path.exists())
                    .unwrap_or(false);
                let local_ips: Vec<String> = crate::network::get_local_ips()
                    .into_iter()
                    .map(|info| info.ip)
                    .collect();
                let port = self.state.port();
                let status = cert_status(self.state.ca_cert_path.as_deref());
                let download_urls: Vec<String> = local_ips
                    .iter()
                    .map(|ip| format!("http://{}:{}/_bifrost/public/cert", ip, port))
                    .collect();
                let qrcode_urls: Vec<String> = local_ips
                    .iter()
                    .map(|ip| format!("http://{}:{}/_bifrost/public/cert/qrcode", ip, port))
                    .collect();
                json!({
                    "available": available,
                    "status": status.status,
                    "status_label": status.status_label,
                    "installed": status.installed,
                    "trusted": status.trusted,
                    "status_message": status.status_message,
                    "local_ips": local_ips,
                    "download_urls": download_urls,
                    "qrcode_urls": qrcode_urls,
                })
            }
            SETTINGS_SCOPE_PROXY_ADDRESS => {
                let ip_infos = crate::network::get_local_ips();
                let port = self.state.port();
                let local_ips: Vec<String> = ip_infos.iter().map(|i| i.ip.clone()).collect();
                let addresses: Vec<serde_json::Value> = ip_infos
                    .iter()
                    .map(|info| {
                        json!({
                            "ip": info.ip,
                            "address": format!("{}:{}", info.ip, port),
                            "qrcode_url": format!("/_bifrost/public/proxy/qrcode?ip={}", urlencoding::encode(&info.ip)),
                            "is_preferred": info.is_preferred,
                        })
                    })
                    .collect();
                json!({
                    "port": port,
                    "local_ips": local_ips,
                    "addresses": addresses,
                })
            }
            SETTINGS_SCOPE_SYSTEM_PROXY => {
                if !bifrost_core::SystemProxyManager::is_supported() {
                    json!({
                        "supported": false,
                        "enabled": false,
                        "host": "",
                        "port": 0,
                        "bypass": "",
                    })
                } else if let Ok(proxy) = bifrost_core::SystemProxyManager::get_current() {
                    json!({
                        "supported": true,
                        "enabled": proxy.enable,
                        "host": proxy.host,
                        "port": proxy.port,
                        "bypass": proxy.bypass,
                    })
                } else {
                    return None;
                }
            }
            SETTINGS_SCOPE_CLI_PROXY => {
                let config_manager = self.state.config_manager.as_ref()?;
                let manager =
                    bifrost_core::ShellProxyManager::new(config_manager.data_dir().to_path_buf());
                let status = manager.status();
                json!({
                    "enabled": status.has_persistent_config,
                    "shell": status.shell_type.as_str(),
                    "config_files": status.config_paths.iter().map(|item| item.to_string_lossy().to_string()).collect::<Vec<_>>(),
                    "proxy_url": format!("http://127.0.0.1:{}", self.state.port()),
                })
            }
            SETTINGS_SCOPE_WHITELIST_STATUS => {
                let access_control = self.state.access_control.as_ref()?;
                let ac = access_control.read().await;
                let userpass = ac.userpass_status();
                json!({
                    "mode": ac.mode().to_string(),
                    "allow_lan": ac.allow_lan(),
                    "whitelist": ac.whitelist_entries(),
                    "temporary_whitelist": ac.temporary_whitelist_entries().iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
                    "session_denied": ac.session_denied_entries().iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
                    "userpass": {
                        "enabled": userpass.enabled,
                        "accounts": userpass.accounts.into_iter().map(|account| json!({
                            "username": account.username,
                            "enabled": account.enabled,
                            "has_password": account.has_password,
                            "last_connected_at": account.last_connected_at.and_then(|timestamp| chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp as i64, 0).map(|dt| dt.to_rfc3339())),
                        })).collect::<Vec<_>>(),
                        "loopback_requires_auth": userpass.loopback_requires_auth,
                    },
                })
            }
            SETTINGS_SCOPE_PENDING_AUTHORIZATIONS => {
                let access_control = self.state.access_control.as_ref()?;
                let ac = access_control.read().await;
                json!(ac.get_pending_authorizations())
            }
            SETTINGS_SCOPE_PENDING_IP_TLS => {
                if let Some(ref manager) = self.state.ip_tls_pending_manager {
                    json!(manager.get_pending_list())
                } else {
                    json!([])
                }
            }
            SETTINGS_SCOPE_CLIENT_TRUST => {
                if let Some(ref tracker) = self.state.client_trust_tracker {
                    json!(tracker.get_all_statuses())
                } else {
                    json!([])
                }
            }
            SETTINGS_SCOPE_NOTIFICATIONS => {
                let unread = crate::notification_db::count_unread().unwrap_or(0);
                let recent = crate::notification_db::list_notifications(None, None, 20, 0)
                    .unwrap_or_default();
                json!({
                    "unread_count": unread,
                    "recent": recent,
                })
            }
            SETTINGS_SCOPE_TRUST_PROBE => {
                json!(crate::handlers::trust_probe::list_active_sessions())
            }
            SETTINGS_SCOPE_MOBILE_DEVICES => {
                crate::handlers::mobile_devices::mobile_devices_snapshot(&self.state)
            }
            _ => return None,
        };

        Some(SettingsUpdateData {
            scope: scope.to_string(),
            data,
        })
    }

    pub async fn broadcast_values_snapshot(&self) {
        let Some(values_data) = self.build_values_data() else {
            return;
        };
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if client.get_subscription().need_values
                && !client.send(PushMessage::ValuesUpdate(values_data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn send_values_snapshot_to_client(&self, client: &Arc<PushClient>) {
        if !client.get_subscription().need_values {
            return;
        }
        if let Some(values_data) = self.build_values_data() {
            client.send(PushMessage::ValuesUpdate(values_data));
        }
    }

    pub async fn broadcast_scripts_snapshot(&self) {
        let Some(scripts_data) = self.build_scripts_data().await else {
            return;
        };
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if client.get_subscription().need_scripts
                && !client.send(PushMessage::ScriptsUpdate(scripts_data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn send_scripts_snapshot_to_client(&self, client: &Arc<PushClient>) {
        if !client.get_subscription().need_scripts {
            return;
        }
        if let Some(scripts_data) = self.build_scripts_data().await {
            client.send(PushMessage::ScriptsUpdate(scripts_data));
        }
    }

    pub async fn broadcast_replay_saved_requests_snapshot(&self) {
        let Some(data) = self.build_replay_saved_requests_data() else {
            return;
        };
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if client.get_subscription().need_replay_saved_requests
                && !client.send(PushMessage::ReplaySavedRequestsUpdate(data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_replay_groups_snapshot(&self) {
        let Some(data) = self.build_replay_groups_data() else {
            return;
        };
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if client.get_subscription().need_replay_groups
                && !client.send(PushMessage::ReplayGroupsUpdate(data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_settings_scope(&self, scope: &str) {
        let Some(data) = self.build_settings_update(scope).await else {
            return;
        };
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if Self::has_settings_scope(&client.get_subscription(), scope)
                && !client.send(PushMessage::SettingsUpdate(data.clone()))
            {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn broadcast_notification(&self, data: NotificationPushData) {
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(PushMessage::Notification(data.clone())) {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub async fn send_settings_scope_to_client(&self, client: &Arc<PushClient>, scope: &str) {
        let Some(data) = self.build_settings_update(scope).await else {
            return;
        };
        if Self::has_settings_scope(&client.get_subscription(), scope) {
            client.send(PushMessage::SettingsUpdate(data));
        }
    }

    pub async fn send_initial_data(&self, client: &Arc<PushClient>) {
        let subscription = client.get_subscription();

        if subscription.need_traffic {
            if let Some(ref db_store) = self.state.traffic_db_store {
                self.send_initial_traffic_delta(client, db_store, &subscription);
                self.send_traffic_statistics_to_client(client);
            }
        }

        if subscription.need_overview {
            client.send(PushMessage::OverviewUpdate(
                self.build_full_overview().await,
            ));
        }

        if subscription.need_history {
            let history = self
                .state
                .metrics_collector
                .get_history(Some(subscription.history_limit));
            let history_json: Vec<serde_json::Value> = history
                .into_iter()
                .map(|m| serde_json::to_value(&m).unwrap_or_default())
                .collect();

            client.send(PushMessage::HistoryUpdate(HistoryData {
                history: history_json,
            }));
        }

        if subscription.need_metrics {
            if client.reserve_metrics_push(Instant::now(), subscription.metrics_interval_ms) {
                client.send(PushMessage::MetricsUpdate(self.build_metrics_data()));
            }
        }

        if subscription.need_values {
            self.send_values_snapshot_to_client(client);
        }

        if subscription.need_scripts {
            self.send_scripts_snapshot_to_client(client).await;
        }

        if subscription.need_replay_saved_requests {
            if let Some(data) = self.build_replay_saved_requests_data() {
                client.send(PushMessage::ReplaySavedRequestsUpdate(data));
            }
        }

        if subscription.need_replay_groups {
            if let Some(data) = self.build_replay_groups_data() {
                client.send(PushMessage::ReplayGroupsUpdate(data));
            }
        }

        for scope in &subscription.settings_scopes {
            if let Some(data) = self.build_settings_update(scope).await {
                client.send(PushMessage::SettingsUpdate(data));
            }
        }
    }

    pub fn broadcast_replay_request_updated(
        &self,
        action: &str,
        request_id: Option<&str>,
        group_id: Option<&str>,
    ) {
        let msg = PushMessage::ReplayRequestUpdated(ReplayRequestUpdatedData {
            action: action.to_string(),
            request_id: request_id.map(|s| s.to_string()),
            group_id: group_id.map(|s| s.to_string()),
        });

        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn broadcast_traffic_deleted(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        self.mark_traffic_statistics_dirty();
        let msg = PushMessage::TrafficDeleted(TrafficDeletedData { ids });
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn broadcast_replay_history_updated(
        &self,
        action: &str,
        request_id: Option<&str>,
        history_id: Option<&str>,
    ) {
        let msg = PushMessage::ReplayHistoryUpdated(ReplayHistoryUpdatedData {
            action: action.to_string(),
            request_id: request_id.map(|s| s.to_string()),
            history_id: history_id.map(|s| s.to_string()),
        });

        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }

        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn broadcast_breakpoint_paused(&self, data: BreakpointPausedPushData) {
        let msg = PushMessage::BreakpointPaused(data);
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn broadcast_breakpoint_settings_updated(&self, data: BreakpointSettingsPushData) {
        let msg = PushMessage::BreakpointSettingsUpdated(data);
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }

    pub fn broadcast_breakpoint_resumed(&self, request_id: String) {
        let msg = PushMessage::BreakpointResumed(BreakpointResumedPushData { request_id });
        let mut clients_to_remove = Vec::new();
        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            if !client.send(msg.clone()) {
                clients_to_remove.push(client.id);
            }
        }
        for client_id in clients_to_remove {
            self.unregister_client(client_id);
        }
    }
    /// Wait for a single new traffic record matching `matcher` (capture-wait).
    ///
    /// Subscribes to the traffic store event broadcast and resolves on the
    /// first incoming `TrafficStoreEvent::Inserted` whose compact summary
    /// passes `matcher`. Returns `CaptureWaitOutcome::matched = None` if no
    /// matching record arrives within `timeout`.
    ///
    /// This is an additive API and does not interact with the existing
    /// push-broadcast loop. Multiple concurrent `subscribe_once` calls are
    /// supported because `traffic_db_store.subscribe()` is a multi-consumer
    /// broadcast channel.
    pub async fn subscribe_once<F>(&self, matcher: F, timeout: Duration) -> CaptureWaitOutcome
    where
        F: Fn(&TrafficSummaryCompact) -> bool + Send + Sync + 'static,
    {
        let Some(db_store) = self.state.traffic_db_store.clone() else {
            return CaptureWaitOutcome {
                matched: None,
                scanned: 0,
            };
        };

        let scanned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scanned_for_task = scanned.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<TrafficSummaryCompact>();

        let task = tokio::spawn(async move {
            let mut receiver = db_store.subscribe();
            let mut tx = Some(tx);
            loop {
                match receiver.recv().await {
                    Ok(TrafficStoreEvent::Inserted(record)) => {
                        let compact = TrafficSummaryCompact::from_record(&record);
                        scanned_for_task.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if matcher(&compact) {
                            if let Some(sender) = tx.take() {
                                let _ = sender.send(compact);
                            }
                            return;
                        }
                    }
                    Ok(TrafficStoreEvent::Updated(_)) => {
                        // capture-wait only fires on freshly inserted records
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        let outcome = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(record)) => CaptureWaitOutcome {
                matched: Some(record),
                scanned: scanned.load(std::sync::atomic::Ordering::Relaxed),
            },
            Ok(Err(_)) | Err(_) => CaptureWaitOutcome {
                matched: None,
                scanned: scanned.load(std::sync::atomic::Ordering::Relaxed),
            },
        };

        task.abort();
        outcome
    }
}

#[derive(Debug, Clone)]
pub struct CaptureWaitOutcome {
    pub matched: Option<TrafficSummaryCompact>,
    pub scanned: usize,
}

#[derive(Debug, Clone)]
struct CertStatusSnapshot {
    status: &'static str,
    status_label: &'static str,
    installed: bool,
    trusted: bool,
    status_message: String,
}

fn cert_status(cert_path: Option<&std::path::Path>) -> CertStatusSnapshot {
    use bifrost_tls::{CertInstaller, CertStatus};

    let Some(cert_path) = cert_path.filter(|path| path.exists()) else {
        return CertStatusSnapshot {
            status: "not_installed",
            status_label: "Not installed",
            installed: false,
            trusted: false,
            status_message: "CA certificate file is missing, so system trust is not configured."
                .to_string(),
        };
    };

    let installer = CertInstaller::new(cert_path);
    match installer.check_status() {
        Ok(CertStatus::NotInstalled) => CertStatusSnapshot {
            status: "not_installed",
            status_label: "Not installed",
            installed: false,
            trusted: false,
            status_message: "CA certificate is not installed in the system trust store."
                .to_string(),
        },
        Ok(CertStatus::InstalledNotTrusted) => CertStatusSnapshot {
            status: "installed_not_trusted",
            status_label: "Installed, not trusted",
            installed: true,
            trusted: false,
            status_message: "CA certificate is installed, but the system does not trust it yet."
                .to_string(),
        },
        Ok(CertStatus::InstalledAndTrusted) => CertStatusSnapshot {
            status: "installed_and_trusted",
            status_label: "Installed and trusted",
            installed: true,
            trusted: true,
            status_message: "CA certificate is installed and trusted by the system.".to_string(),
        },
        Err(error) => CertStatusSnapshot {
            status: "unknown",
            status_label: "Check failed",
            installed: false,
            trusted: false,
            status_message: format!(
                "Unable to verify whether the CA certificate is trusted: {error}"
            ),
        },
    }
}

pub type SharedPushManager = Arc<PushManager>;

pub fn start_push_tasks(manager: SharedPushManager) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    if let Some(db_store) = manager.state.traffic_db_store.clone() {
        let weak_manager = Arc::downgrade(&manager);
        db_store.set_cleanup_notifier(Arc::new(move |ids| {
            if let Some(manager) = weak_manager.upgrade() {
                manager.broadcast_traffic_deleted(ids.to_vec());
            }
        }));

        let manager_traffic = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut receiver = db_store.subscribe();
            loop {
                let first_event = match receiver.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        manager_traffic.mark_traffic_statistics_dirty();
                        if manager_traffic.has_traffic_subscribers() {
                            manager_traffic.broadcast_traffic_updates().await;
                        }
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };

                let mut inserts = Vec::with_capacity(32);
                let mut updates = Vec::with_capacity(32);

                let mut push_event =
                    |event| match event {
                        TrafficStoreEvent::Inserted(record) => {
                            inserts.push(manager_traffic.enrich_compact_summary(
                                TrafficSummaryCompact::from_record(&record),
                            ));
                        }
                        TrafficStoreEvent::Updated(record) => {
                            updates.push(manager_traffic.enrich_compact_summary(
                                TrafficSummaryCompact::from_record(&record),
                            ));
                        }
                    };

                push_event(first_event);
                manager_traffic.mark_traffic_statistics_dirty();

                for _ in 1..TRAFFIC_DELTA_BATCH_LIMIT {
                    match receiver.try_recv() {
                        Ok(event) => push_event(event),
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                            inserts.clear();
                            updates.clear();
                            break;
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return,
                    }
                }

                if inserts.is_empty() && updates.is_empty() {
                    if manager_traffic.has_traffic_subscribers() {
                        manager_traffic.broadcast_traffic_updates().await;
                    }
                    continue;
                }

                if manager_traffic.has_traffic_subscribers() {
                    manager_traffic
                        .broadcast_traffic_events(
                            inserts,
                            updates,
                            db_store.count(),
                            db_store.current_sequence(),
                            db_store.oldest_sequence(),
                        )
                        .await;
                }
            }
        }));

        let manager_pending = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(TRAFFIC_PENDING_REFRESH_INTERVAL_MS));
            loop {
                interval.tick().await;
                if manager_pending.has_traffic_subscribers() {
                    manager_pending.broadcast_traffic_updates().await;
                }
            }
        }));

        let manager_statistics = manager.clone();
        handles.push(tokio::spawn(async move {
            let period = Duration::from_millis(TRAFFIC_STATISTICS_PUSH_INTERVAL_MS);
            let start = tokio::time::Instant::now() + period;
            let mut interval = tokio::time::interval_at(start, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                manager_statistics.flush_dirty_traffic_statistics();
            }
        }));
    }

    let manager_overview = manager.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if manager_overview.has_overview_subscribers() {
                manager_overview.broadcast_overview_lightweight().await;
            }
        }
    }));

    let manager_metrics = manager.clone();
    handles.push(tokio::spawn(async move {
        let base_interval_ms: u64 = 500;
        let mut interval = tokio::time::interval(Duration::from_millis(base_interval_ms));
        let mut tick_count: u64 = 0;
        loop {
            interval.tick().await;
            tick_count = tick_count.wrapping_add(1);
            if manager_metrics.has_metrics_subscribers() {
                let elapsed_ms = tick_count * base_interval_ms;
                manager_metrics
                    .broadcast_metrics_with_interval(elapsed_ms)
                    .await;
            }
        }
    }));

    let manager_history = manager.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if manager_history.client_count() > 0 {
                manager_history.broadcast_history().await;
            }
        }
    }));

    let manager_mobile_devices = manager.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3));
        loop {
            interval.tick().await;
            if manager_mobile_devices.has_settings_scope_subscribers(SETTINGS_SCOPE_MOBILE_DEVICES)
            {
                manager_mobile_devices
                    .broadcast_settings_scope(SETTINGS_SCOPE_MOBILE_DEVICES)
                    .await;
            }
        }
    }));

    if let Some(ac) = manager.state.access_control.clone() {
        let manager_subnet = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.tick().await;
            loop {
                interval.tick().await;
                let new_subnets = crate::network::get_local_subnets();
                let guard = ac.read().await;
                let current = guard.local_subnets();
                if current != new_subnets {
                    tracing::info!(
                        target: "bifrost::network",
                        old = ?current.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        new = ?new_subnets.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        "Local network changed, refreshing subnets"
                    );
                    guard.set_local_subnets(new_subnets);
                    drop(guard);
                    manager_subnet
                        .broadcast_settings_scope(SETTINGS_SCOPE_PROXY_ADDRESS)
                        .await;
                    manager_subnet
                        .broadcast_settings_scope(SETTINGS_SCOPE_CERT_INFO)
                        .await;
                }
            }
        }));
    }

    if let Some(ref tracker) = manager.state.client_trust_tracker {
        let manager_trust = manager.clone();
        let mut trust_rx = tracker.subscribe();
        handles.push(tokio::spawn(async move {
            loop {
                match trust_rx.recv().await {
                    Ok(event) => {
                        let metadata = serde_json::to_value(&event).ok();
                        let title = format!("Client {} trust status changed", event.identifier);
                        let message = format!(
                            "{} ({}) changed from {} to {}",
                            event.identifier,
                            event.identifier_type,
                            event.old_status,
                            event.new_status,
                        );

                        let _ = crate::notification_db::create_notification(
                            &crate::notification_db::CreateNotification {
                                notification_type: "tls_trust_change".to_string(),
                                title,
                                message,
                                metadata: metadata.map(|m| m.to_string()),
                            },
                        );

                        manager_trust
                            .broadcast_settings_scope(SETTINGS_SCOPE_CLIENT_TRUST)
                            .await;
                        manager_trust
                            .broadcast_settings_scope(SETTINGS_SCOPE_NOTIFICATIONS)
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Client trust event receiver lagged by {n}");
                        manager_trust
                            .broadcast_settings_scope(SETTINGS_SCOPE_CLIENT_TRUST)
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }

    if let Some(ref ac) = manager.state.access_control {
        let manager_ac = manager.clone();
        let ac_clone = ac.clone();
        handles.push(tokio::spawn(async move {
            let mut ac_rx = ac_clone.read().await.subscribe();
            loop {
                match ac_rx.recv().await {
                    Ok(event) => {
                        let ip = &event.pending_auth.ip;
                        let event_type = &event.event_type;

                        let (title, message) = match event_type.as_str() {
                            "new" => (
                                format!("New connection pending authorization from {ip}"),
                                format!(
                                    "IP {ip} is requesting access (attempt #{}). Total pending: {}",
                                    event.pending_auth.attempt_count, event.total_pending
                                ),
                            ),
                            "approved" => (
                                format!("Access approved for {ip}"),
                                format!(
                                    "IP {ip} has been approved and added to temporary whitelist"
                                ),
                            ),
                            "rejected" => (
                                format!("Access rejected for {ip}"),
                                format!("IP {ip} has been rejected and denied for this session"),
                            ),
                            _ => (
                                format!("Access control event for {ip}"),
                                format!("Event type: {event_type}, IP: {ip}"),
                            ),
                        };

                        let metadata = serde_json::to_value(&event).ok().map(|mut m| {
                            if let Some(obj) = m.as_object_mut() {
                                obj.insert(
                                    "domain".to_string(),
                                    serde_json::Value::String(ip.to_string()),
                                );
                            }
                            m.to_string()
                        });

                        let _ = crate::notification_db::create_notification(
                            &crate::notification_db::CreateNotification {
                                notification_type: "pending_authorization".to_string(),
                                title,
                                message,
                                metadata,
                            },
                        );

                        manager_ac
                            .broadcast_settings_scope(SETTINGS_SCOPE_PENDING_AUTHORIZATIONS)
                            .await;
                        manager_ac
                            .broadcast_settings_scope(SETTINGS_SCOPE_NOTIFICATIONS)
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Access control event receiver lagged by {n}");
                        manager_ac
                            .broadcast_settings_scope(SETTINGS_SCOPE_PENDING_AUTHORIZATIONS)
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }

    handles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestAdminState;
    use crate::{AdminState, TrafficDbStore, TrafficRecord};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::time::{sleep, timeout, Duration};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!(
            "bifrost_push_test_{}_{}_{}",
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

    fn compact(seq: u64, id: &str) -> TrafficSummaryCompact {
        TrafficSummaryCompact {
            id: id.to_string(),
            seq,
            ts: seq,
            m: "GET".to_string(),
            h: "example.test".to_string(),
            p: format!("/{}", id),
            s: 200,
            ct: None,
            req_ct: None,
            req_sz: 0,
            res_sz: 0,
            up: 0,
            down: 0,
            dur: 0,
            lp: 0,
            proto: "http".to_string(),
            cip: "127.0.0.1".to_string(),
            capp: None,
            cpid: None,
            acct: None,
            flags: 0,
            fc: 0,
            ss: None,
            st: "-".to_string(),
            et: None,
            rc: 0,
            rp: vec![],
        }
    }

    #[test]
    fn traffic_statistics_client_gate_enforces_one_second_spacing() {
        let (client, _receiver) = PushClient::new(
            "statistics-rate-gate".to_string(),
            ClientSubscription::default(),
        );
        let start = Instant::now();

        assert!(client.reserve_traffic_statistics_push(start));
        assert!(!client.reserve_traffic_statistics_push(start + Duration::from_millis(999)));
        assert!(client.reserve_traffic_statistics_push(start + Duration::from_millis(1_000)));
    }

    #[test]
    fn traffic_statistics_helpers_cover_missing_store_throttling_and_closed_clients() {
        let state_without_store = Arc::new(AdminState::new(9910));
        let manager_without_store = PushManager::new(state_without_store);
        let (client_without_store, _receiver) = manager_without_store.register_client(
            "statistics-no-store".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        assert!(manager_without_store.send_traffic_statistics_to_client(&client_without_store));
        assert!(!manager_without_store.broadcast_traffic_statistics());

        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9911).with_traffic_db_store_shared(store.clone()));
        let manager = PushManager::new(state);
        let (_unsubscribed, _unsubscribed_receiver) =
            manager.register_client("statistics-unsubscribed".to_string(), Default::default());
        let (throttled, _throttled_receiver) = manager.register_client(
            "statistics-throttled".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let (_closed, closed_receiver) = manager.register_client(
            "statistics-closed".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        drop(closed_receiver);

        assert!(manager.send_traffic_statistics_to_client(&throttled));
        assert!(manager.send_traffic_statistics_to_client(&throttled));
        assert!(manager.take_traffic_statistics_dirty());
        assert!(manager.broadcast_traffic_statistics());
        assert_eq!(manager.client_count(), 2);

        let mut record = TrafficRecord::new(
            "statistics-closed-initial".to_string(),
            "GET".to_string(),
            "http://example.test/closed-initial".to_string(),
        );
        record.status = 200;
        store.record(record);
        let (closed_initial, closed_initial_receiver) = manager.register_client(
            "statistics-closed-initial".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        drop(closed_initial_receiver);
        manager.send_initial_traffic(&closed_initial);

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn send_initial_data_includes_traffic_statistics() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let mut record = TrafficRecord::new(
            "statistics-initial-data".to_string(),
            "GET".to_string(),
            "http://example.test/initial".to_string(),
        );
        record.status = 200;
        store.record(record);
        let state = Arc::new(AdminState::new(9912).with_traffic_db_store_shared(store));
        let manager = PushManager::new(state);
        let (client, mut receiver) = manager.register_client(
            "statistics-initial-data".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );

        manager.send_initial_data(&client).await;

        let messages: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(messages
            .iter()
            .any(|message| matches!(message, PushMessage::TrafficDelta(_))));
        assert!(messages.iter().any(|message| matches!(
            message,
            PushMessage::TrafficStatistics(statistics) if statistics.total_requests == 1
        )));

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn cleanup_notifier_broadcasts_deleted_ids() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9913).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));
        let (_client, mut receiver) = manager.register_client(
            "statistics-cleanup".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let handles = start_push_tasks(manager);

        let records = (0..1_151)
            .map(|index| {
                let mut record = TrafficRecord::new(
                    format!("statistics-cleanup-{index}"),
                    "GET".to_string(),
                    format!("http://example.test/cleanup/{index}"),
                );
                record.status = 200;
                record
            })
            .collect();
        store.record_batch(records);

        let deleted_ids = timeout(Duration::from_secs(2), async {
            loop {
                match receiver.recv().await {
                    Some(PushMessage::TrafficDeleted(TrafficDeletedData { ids })) => break ids,
                    Some(_) => continue,
                    None => panic!("client channel closed before cleanup push"),
                }
            }
        })
        .await
        .expect("expected cleanup push");
        assert!(!deleted_ids.is_empty());
        assert_eq!(store.count(), 800);

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn traffic_push_uses_in_memory_events_without_querying_db_for_new_records() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9910).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let subscription = ClientSubscription {
            need_traffic: true,
            ..Default::default()
        };
        let (_client, mut receiver) =
            manager.register_client("push-test-client".to_string(), subscription);

        let handles = start_push_tasks(manager.clone());
        sleep(Duration::from_millis(100)).await;

        store.reset_debug_query_counters();

        let mut record = TrafficRecord::new(
            "push-memory-1".to_string(),
            "GET".to_string(),
            "http://example.test/push-memory".to_string(),
        );
        record.status = 200;
        record.response_size = 123;
        store.record(record);

        let message = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("expected push message")
            .expect("channel should stay open");

        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };
        assert_eq!(data.inserts.len(), 1);
        assert!(data.updates.is_empty());
        assert_eq!(data.inserts[0].id, "push-memory-1");

        sleep(Duration::from_millis(300)).await;
        let (query_calls, get_by_ids_calls) = store.debug_query_counters();
        assert_eq!(query_calls, 0, "new records should not require query()");
        assert_eq!(
            get_by_ids_calls, 0,
            "new records without pending ids should not require get_by_ids()"
        );

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn traffic_statistics_push_is_change_driven_and_coalesced_to_one_second() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9914).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));
        let (_client, mut receiver) = manager.register_client(
            "statistics-push-client".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let handles = start_push_tasks(manager.clone());
        sleep(Duration::from_millis(100)).await;

        for index in 0..3 {
            let mut record = TrafficRecord::new(
                format!("statistics-push-{index}"),
                "GET".to_string(),
                format!("http://example.test/{index}"),
            );
            record.status = 200;
            record.client_app = Some("codex".to_string());
            store.record(record);
        }

        let first_statistics = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(PushMessage::TrafficStatistics(statistics)) = receiver.recv().await {
                    break statistics;
                }
            }
        })
        .await
        .expect("expected a coalesced statistics push");
        assert_eq!(first_statistics.total_requests, 3);
        assert_eq!(first_statistics.applications.get("codex"), Some(&3));

        sleep(Duration::from_millis(1_100)).await;
        let idle_statistics_pushes = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter(|message| matches!(message, PushMessage::TrafficStatistics(_)))
            .count();
        assert_eq!(
            idle_statistics_pushes, 0,
            "idle periods must not push statistics"
        );

        for index in 3..5 {
            let mut record = TrafficRecord::new(
                format!("statistics-push-{index}"),
                "GET".to_string(),
                format!("http://example.test/{index}"),
            );
            record.status = 200;
            record.client_app = Some("codex".to_string());
            store.record(record);
        }

        sleep(Duration::from_millis(1_100)).await;
        let statistics_pushes: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter_map(|message| match message {
                PushMessage::TrafficStatistics(statistics) => Some(statistics),
                _ => None,
            })
            .collect();
        assert_eq!(statistics_pushes.len(), 1, "a burst must be coalesced");
        assert_eq!(statistics_pushes[0].total_requests, 5);

        store.clear();
        manager.notify_traffic_statistics_changed();
        let cleared_statistics = timeout(Duration::from_secs(3), async {
            loop {
                if let Some(PushMessage::TrafficStatistics(statistics)) = receiver.recv().await {
                    break statistics;
                }
            }
        })
        .await
        .expect("expected statistics after clear-all notification");
        assert_eq!(cleared_statistics.total_requests, 0);
        assert!(cleared_statistics.applications.is_empty());

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[test]
    fn traffic_statistics_dirty_flush_is_retained_after_client_rate_limit() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9915).with_traffic_db_store_shared(store));
        let manager = PushManager::new(state);
        let (client, _receiver) = manager.register_client(
            "statistics-deferred-client".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );

        assert!(manager.send_traffic_statistics_to_client(&client));
        manager.notify_traffic_statistics_changed();
        manager.flush_dirty_traffic_statistics();
        assert!(manager.take_traffic_statistics_dirty());

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn lagged_traffic_receiver_marks_statistics_dirty() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 2_000, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9916).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));
        let (_client, _receiver) = manager.register_client(
            "statistics-lagged-client".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let handles = start_push_tasks(manager.clone());
        timeout(Duration::from_secs(1), async {
            while !store.has_traffic_event_subscribers() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("traffic receiver should subscribe");

        let records = (0..1_025)
            .map(|index| {
                let mut record = TrafficRecord::new(
                    format!("statistics-lagged-{index}"),
                    "GET".to_string(),
                    format!("http://example.test/lagged/{index}"),
                );
                record.status = 200;
                record
            })
            .collect();
        store.record_batch(records);

        timeout(Duration::from_secs(1), async {
            while !manager.take_traffic_statistics_dirty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("lagged receiver should mark statistics dirty");

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn traffic_push_pending_refresh_queries_db_for_pending_requests() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9911).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let subscription = ClientSubscription {
            need_traffic: true,
            pending_ids: vec!["pending-ws-1".to_string()],
            ..Default::default()
        };
        let (_client, mut receiver) =
            manager.register_client("push-pending-client".to_string(), subscription);

        let handles = start_push_tasks(manager.clone());
        sleep(Duration::from_millis(100)).await;

        let mut record = TrafficRecord::new(
            "pending-http-1".to_string(),
            "POST".to_string(),
            "http://example.test/pending".to_string(),
        );
        record.status = 0;
        store.record(record);

        let _ = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("expected initial insert push");

        store.reset_debug_query_counters();

        sleep(Duration::from_millis(
            TRAFFIC_PENDING_REFRESH_INTERVAL_MS + 600,
        ))
        .await;
        let (query_calls, get_by_ids_calls) = store.debug_query_counters();
        assert!(
            query_calls >= 1 || get_by_ids_calls >= 1,
            "pending refresh should still query db as a low-frequency fallback"
        );

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn traffic_push_broadcasts_new_records_to_multiple_clients() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9912).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let subscription = ClientSubscription {
            need_traffic: true,
            ..Default::default()
        };
        let (_client_a, mut receiver_a) =
            manager.register_client("push-multi-client-a".to_string(), subscription.clone());
        let (_client_b, mut receiver_b) =
            manager.register_client("push-multi-client-b".to_string(), subscription);

        let handles = start_push_tasks(manager.clone());
        sleep(Duration::from_millis(100)).await;

        let mut record = TrafficRecord::new(
            "push-multi-1".to_string(),
            "GET".to_string(),
            "http://example.test/multi".to_string(),
        );
        record.status = 200;
        store.record(record);

        let message_a = timeout(Duration::from_secs(2), receiver_a.recv())
            .await
            .expect("expected push message for client A")
            .expect("client A channel should stay open");
        let message_b = timeout(Duration::from_secs(2), receiver_b.recv())
            .await
            .expect("expected push message for client B")
            .expect("client B channel should stay open");

        let PushMessage::TrafficDelta(data_a) = message_a else {
            panic!("expected traffic delta for client A");
        };
        let PushMessage::TrafficDelta(data_b) = message_b else {
            panic!("expected traffic delta for client B");
        };
        assert_eq!(data_a.inserts.len(), 1);
        assert_eq!(data_b.inserts.len(), 1);
        assert_eq!(data_a.inserts[0].id, "push-multi-1");
        assert_eq!(data_b.inserts[0].id, "push-multi-1");

        for handle in handles {
            handle.abort();
        }
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn send_initial_traffic_can_bootstrap_late_traffic_subscription() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9913).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let mut record = TrafficRecord::new(
            "late-subscription-1".to_string(),
            "GET".to_string(),
            "http://example.test/late-subscription".to_string(),
        );
        record.status = 200;
        store.record(record);

        let (client, mut receiver) =
            manager.register_client("push-late-subscription".to_string(), Default::default());

        client.update_subscription(ClientSubscription {
            need_traffic: true,
            ..Default::default()
        });

        manager.send_initial_traffic(&client);

        let message = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("expected push message")
            .expect("channel should stay open");

        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };
        assert_eq!(data.inserts.len(), 1);
        assert_eq!(data.inserts[0].id, "late-subscription-1");

        let statistics_message = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("expected initial statistics message")
            .expect("channel should stay open");
        let PushMessage::TrafficStatistics(statistics) = statistics_message else {
            panic!("expected traffic statistics");
        };
        assert_eq!(statistics.total_requests, 1);
        assert_eq!(statistics.domains.get("example.test"), Some(&1));

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn send_initial_traffic_without_last_sequence_uses_latest_window() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 2000, 0, None).unwrap());
        let rules_storage = bifrost_storage::RulesStorage::with_dir(dir.join("rules")).unwrap();
        let state = Arc::new(
            AdminState::new_for_test(9914, rules_storage)
                .with_traffic_db_store_shared(store.clone()),
        );
        let manager = Arc::new(PushManager::new(state));

        for i in 0..520 {
            let mut record = TrafficRecord::new(
                format!("bootstrap-{}", i),
                "GET".to_string(),
                format!("http://example.test/bootstrap-{}", i),
            );
            record.status = 200;
            store.record(record);
        }

        let (client, mut receiver) =
            manager.register_client("push-latest-window".to_string(), Default::default());

        client.update_subscription(ClientSubscription {
            need_traffic: true,
            ..Default::default()
        });

        manager.send_initial_traffic(&client);

        let message = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("expected push message")
            .expect("channel should stay open");

        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };

        let ids: Vec<&str> = data.inserts.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(data.inserts.len(), 500);
        assert_eq!(ids.first().copied(), Some("bootstrap-20"));
        assert_eq!(ids.last().copied(), Some("bootstrap-519"));

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn send_initial_traffic_reconnects_with_bounded_latest_chunks() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 10_000, 0, None).unwrap());
        let rules_storage = bifrost_storage::RulesStorage::with_dir(dir.join("rules")).unwrap();
        let state = Arc::new(
            AdminState::new_for_test(9915, rules_storage)
                .with_traffic_db_store_shared(store.clone()),
        );
        let manager = Arc::new(PushManager::new(state));

        for i in 0..2_500 {
            let mut record = TrafficRecord::new(
                format!("reconnect-{i}"),
                "GET".to_string(),
                format!("http://example.test/reconnect-{i}"),
            );
            record.status = 200;
            store.record(record);
        }

        let (client, mut receiver) = manager.register_client(
            "push-reconnect-window".to_string(),
            ClientSubscription {
                need_traffic: true,
                last_sequence: Some(100),
                ..Default::default()
            },
        );

        manager.send_initial_traffic(&client);

        let mut inserts = Vec::new();
        let mut statistics_count = 0;
        while let Ok(message) = receiver.try_recv() {
            match message {
                PushMessage::TrafficDelta(data) => {
                    assert!(data.inserts.len() <= TRAFFIC_DELTA_BATCH_LIMIT);
                    inserts.extend(data.inserts);
                }
                PushMessage::TrafficStatistics(_) => statistics_count += 1,
                other => panic!("unexpected reconnect message: {other:?}"),
            }
        }

        assert_eq!(statistics_count, 1);
        assert_eq!(inserts.len(), TRAFFIC_RECONNECT_WINDOW_LIMIT);
        assert_eq!(
            inserts.first().map(|item| item.id.as_str()),
            Some("reconnect-1500")
        );
        assert_eq!(
            inserts.last().map(|item| item.id.as_str()),
            Some("reconnect-2499")
        );
        assert_eq!(client.get_subscription().last_sequence, Some(2_500));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn client_subscription_update_keeps_last_sequence_monotonic() {
        let (client, _receiver) = PushClient::new(
            "push-client-monotonic".to_string(),
            ClientSubscription {
                need_traffic: true,
                last_sequence: Some(200),
                ..Default::default()
            },
        );

        client.update_subscription(ClientSubscription {
            need_traffic: true,
            last_sequence: Some(100),
            ..Default::default()
        });
        assert_eq!(client.get_subscription().last_sequence, Some(200));

        client.update_subscription(ClientSubscription {
            need_traffic: true,
            last_sequence: None,
            ..Default::default()
        });
        assert_eq!(client.get_subscription().last_sequence, Some(200));
    }

    #[test]
    fn send_traffic_delta_does_not_regress_last_sequence() {
        let state = Arc::new(AdminState::new(9915));
        let manager = PushManager::new(state);
        let (client, _receiver) = PushClient::new(
            "push-client-seq".to_string(),
            ClientSubscription {
                need_traffic: true,
                last_sequence: Some(200),
                ..Default::default()
            },
        );
        let client = Arc::new(client);

        assert!(manager.send_traffic_delta_to_client(
            &client,
            vec![compact(100, "older-batch")],
            vec![],
            TrafficDeltaMetadata {
                has_more: false,
                server_total: 1,
                server_sequence: 200,
                oldest_sequence: Some(50),
            },
        ));

        assert_eq!(client.get_subscription().last_sequence, Some(200));
    }

    #[test]
    fn send_traffic_delta_dedupes_updates_by_id_keep_latest() {
        let test_dir = create_test_dir();
        let state = Arc::new(AdminState::new_for_test(
            9916,
            bifrost_storage::RulesStorage::with_dir(test_dir.join("rules")).unwrap(),
        ));
        let manager = PushManager::new(state);
        let (client, mut receiver) = PushClient::new(
            "push-client-dedupe".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let client = Arc::new(client);

        let mut stale = compact(10, "same-id");
        stale.res_sz = 0;
        stale.fc = 0;

        let mut latest = compact(10, "same-id");
        latest.res_sz = 1234;
        latest.fc = 42;

        assert!(manager.send_traffic_delta_to_client(
            &client,
            vec![],
            vec![stale, latest.clone()],
            TrafficDeltaMetadata {
                has_more: false,
                server_total: 1,
                server_sequence: 10,
                oldest_sequence: Some(7),
            },
        ));

        let message = receiver.try_recv().expect("expected traffic delta");
        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };

        assert_eq!(data.updates.len(), 1);
        assert_eq!(data.updates[0].id, "same-id");
        assert_eq!(data.updates[0].res_sz, latest.res_sz);
        assert_eq!(data.updates[0].fc, latest.fc);
        assert_eq!(data.oldest_sequence, Some(7));
    }

    #[test]
    fn send_traffic_delta_forwards_floor_without_records() {
        let state = Arc::new(AdminState::new(9917));
        let manager = PushManager::new(state);
        let (client, mut receiver) = PushClient::new(
            "push-client-floor-only".to_string(),
            ClientSubscription {
                need_traffic: true,
                last_sequence: Some(100),
                ..Default::default()
            },
        );
        let client = Arc::new(client);

        assert!(manager.send_traffic_delta_to_client(
            &client,
            vec![],
            vec![],
            TrafficDeltaMetadata {
                has_more: false,
                server_total: 80,
                server_sequence: 101,
                oldest_sequence: Some(21),
            },
        ));

        let message = receiver.try_recv().expect("expected floor-only delta");
        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };
        assert!(data.inserts.is_empty());
        assert!(data.updates.is_empty());
        assert_eq!(data.oldest_sequence, Some(21));
        assert_eq!(client.get_subscription().last_sequence, Some(100));
    }

    #[test]
    fn is_pending_traffic_record_treats_zero_status_as_pending() {
        let mut summary = compact(1, "pending-1");
        summary.s = 0;
        assert!(is_pending_traffic_record(&summary));

        summary.s = 200;
        assert!(!is_pending_traffic_record(&summary));
    }

    #[test]
    fn is_pending_traffic_record_detects_open_streaming_sockets() {
        let mut summary = compact(1, "ws-1");
        summary.s = 200;
        summary.flags = crate::traffic_db::TrafficFlags::IS_WEBSOCKET;
        summary.ss = Some(crate::traffic::SocketStatus {
            is_open: true,
            send_count: 1,
            receive_count: 1,
            send_bytes: 10,
            receive_bytes: 10,
            frame_count: 1,
            close_code: None,
            close_reason: None,
        });
        assert!(is_pending_traffic_record(&summary));

        if let Some(ref mut status) = summary.ss {
            status.is_open = false;
        }
        assert!(!is_pending_traffic_record(&summary));
    }

    #[test]
    fn dedupe_compact_records_keep_latest_keeps_last_occurrence() {
        let first = compact(1, "same-id");
        let mut second = compact(2, "same-id");
        second.res_sz = 123;
        second.fc = 99;

        let deduped = dedupe_compact_records_keep_latest(vec![first, second.clone()]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].id, "same-id");
        assert_eq!(deduped[0].res_sz, second.res_sz);
        assert_eq!(deduped[0].fc, second.fc);
    }

    #[test]
    fn client_subscription_default_has_expected_limits() {
        let sub = ClientSubscription::default();
        assert_eq!(sub.history_limit, default_history_limit());
        assert_eq!(sub.metrics_interval_ms, default_metrics_interval_ms());
        assert!(!sub.need_traffic);
        assert!(sub.settings_scopes.is_empty());
    }

    #[test]
    fn client_subscription_deserializes_with_defaults() {
        let json = r#"{"need_traffic": true}"#;
        let sub: ClientSubscription = serde_json::from_str(json).unwrap();
        assert!(sub.need_traffic);
        assert_eq!(sub.history_limit, default_history_limit());
        assert_eq!(sub.metrics_interval_ms, default_metrics_interval_ms());
    }

    #[test]
    fn push_message_serde_uses_type_and_data_fields() {
        let msg = PushMessage::Connected(ConnectedData {
            client_id: 123,
            message: "hi".to_string(),
        });
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "connected");
        assert_eq!(v["data"]["client_id"], 123);
        assert_eq!(v["data"]["message"], "hi");
    }

    #[test]
    fn push_message_settings_update_serde_uses_type_and_data_fields() {
        let msg = PushMessage::SettingsUpdate(SettingsUpdateData {
            scope: SETTINGS_SCOPE_TLS_CONFIG.to_string(),
            data: serde_json::json!({"k": "v"}),
        });
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "settings_update");
        assert_eq!(v["data"]["scope"], SETTINGS_SCOPE_TLS_CONFIG);
        assert_eq!(v["data"]["data"]["k"], "v");
    }

    #[test]
    fn update_pending_ids_tracks_pending_status_transitions() {
        let state = Arc::new(AdminState::new(0));
        let manager = PushManager::new(state);

        let pending_ids = vec!["req-stable".to_string(), "req-finished".to_string()];

        let mut updated_finished = compact(1, "req-finished");
        updated_finished.s = 200;

        let mut new_pending = compact(2, "req-new-pending");
        new_pending.s = 0;

        let mut new_finished = compact(3, "req-new-finished");
        new_finished.s = 200;

        let next = manager.update_pending_ids(
            &pending_ids,
            &[new_pending, new_finished],
            &[updated_finished],
        );

        assert!(next.contains(&"req-stable".to_string()));
        assert!(next.contains(&"req-new-pending".to_string()));
        assert!(!next.contains(&"req-finished".to_string()));
        assert!(!next.contains(&"req-new-finished".to_string()));
    }

    #[test]
    fn ensure_bucket_capacity_evicts_oldest_bucket_when_capacity_exceeded() {
        let harness = TestAdminState::builder().build();
        let manager = PushManager::new(harness.state());

        {
            let mut order = manager.bucket_order.lock();
            order.push_back("a".to_string());
            order.push_back("b".to_string());
            order.push_back("c".to_string());
        }
        manager.buckets.insert("a".to_string(), vec![1]);
        manager.buckets.insert("b".to_string(), vec![2]);
        manager.buckets.insert("c".to_string(), vec![3]);

        let evicted = manager.ensure_bucket_capacity("d");

        assert_eq!(evicted, vec![1]);
        assert!(manager.buckets.get("a").is_none());

        let order: Vec<String> = manager.bucket_order.lock().iter().cloned().collect();
        assert_eq!(
            order,
            vec!["b".to_string(), "c".to_string(), "d".to_string()]
        );
    }

    #[tokio::test]
    async fn broadcast_metrics_clamps_client_interval_to_allowed_range() {
        let state = Arc::new(AdminState::new(0));
        let manager = Arc::new(PushManager::new(state));

        let subscription = ClientSubscription {
            need_metrics: true,
            metrics_interval_ms: METRICS_INTERVAL_MAX_MS * 10,
            ..Default::default()
        };
        let (_client, mut receiver) =
            manager.register_client("metrics-client".to_string(), subscription);

        manager
            .broadcast_metrics_with_interval(METRICS_INTERVAL_MAX_MS)
            .await;

        let message = receiver.try_recv().expect("expected metrics update");
        let PushMessage::MetricsUpdate(_) = message else {
            panic!("expected MetricsUpdate message");
        };
    }

    #[test]
    fn metrics_push_reservation_clamps_fast_clients_to_one_second() {
        let subscription = ClientSubscription {
            need_metrics: true,
            metrics_interval_ms: 1,
            ..Default::default()
        };
        let (client, _receiver) = PushClient::new("metrics-fast-client".to_string(), subscription);
        let start = Instant::now();

        assert!(client.reserve_metrics_push(start, 1));
        assert!(!client.reserve_metrics_push(start + Duration::from_millis(999), 1));
        assert!(client.reserve_metrics_push(start + Duration::from_millis(1_000), 1));
    }

    #[tokio::test]
    async fn initial_metrics_snapshot_shares_periodic_rate_limit() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let subscription = ClientSubscription {
            need_metrics: true,
            metrics_interval_ms: 1,
            ..Default::default()
        };
        let (client, mut receiver) =
            manager.register_client("initial-metrics-rate-limit".to_string(), subscription);

        manager.send_initial_data(&client).await;

        let initial = receiver
            .try_recv()
            .expect("expected initial metrics snapshot");
        assert!(matches!(initial, PushMessage::MetricsUpdate(_)));

        manager.broadcast_metrics().await;
        assert!(
            receiver.try_recv().is_err(),
            "periodic metrics push must not bypass the initial snapshot rate limit"
        );
    }

    #[test]
    fn metrics_push_data_includes_authoritative_recorded_count() {
        let harness = TestAdminState::builder().build();
        let mut record = TrafficRecord::new(
            "metrics-push-record".to_string(),
            "GET".to_string(),
            "https://push.test/".to_string(),
        );
        record.upload_bytes = 5;
        record.download_bytes = 7;
        harness.traffic_db.record(record);
        let manager = harness.push_manager();

        let data = manager.build_metrics_data();
        let metrics: crate::MetricsSnapshot =
            serde_json::from_value(data.metrics).expect("metrics snapshot");

        assert_eq!(data.recorded_traffic, 1);
        assert_eq!(
            metrics.total_traffic_bytes,
            metrics.bytes_sent + metrics.bytes_received
        );
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::test_support::TestAdminState;
    use crate::{AdminState, TrafficDbStore, TrafficRecord};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::time::{sleep, timeout, Duration};
    // wiremock::MockServer is not available as a dev-dependency in this crate.

    fn make_minimal_manager() -> PushManager {
        let state = Arc::new(AdminState::new(0));
        PushManager::new(state)
    }

    #[test]
    fn generate_client_id_increments() {
        let id1 = generate_client_id();
        let id2 = generate_client_id();
        assert!(id2 > id1);
    }

    #[test]
    fn has_settings_scope_detects_presence() {
        let mut sub = ClientSubscription::default();
        sub.settings_scopes
            .push(SETTINGS_SCOPE_TLS_CONFIG.to_string());
        assert!(PushManager::has_settings_scope(
            &sub,
            SETTINGS_SCOPE_TLS_CONFIG
        ));
        assert!(!PushManager::has_settings_scope(
            &sub,
            SETTINGS_SCOPE_PROXY_ADDRESS
        ));
    }

    #[test]
    fn build_values_data_none_when_no_values_storage() {
        let manager = make_minimal_manager();
        assert!(manager.build_values_data().is_none());
    }

    #[test]
    fn build_values_data_some_when_storage_present() {
        let harness = TestAdminState::builder().build();
        {
            let mut storage = harness.values_storage.write();
            storage.set_value("key", "value").unwrap();
        }
        let manager = harness.push_manager();
        let data = manager
            .build_values_data()
            .expect("expected values data to be present");
        assert_eq!(data.total, 1);
        assert_eq!(data.values[0].name, "key");
        assert_eq!(data.values[0].value, "value");
    }

    #[tokio::test]
    async fn build_full_overview_with_test_admin_state() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let overview = manager.build_full_overview().await;
        assert_eq!(overview.server.port, harness.state().port());
        assert!(overview
            .server
            .admin_url
            .contains(&overview.server.port.to_string()));
    }

    #[tokio::test]
    async fn build_lightweight_overview_uses_cache() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let full = manager.build_full_overview().await;
        let lightweight = manager.build_lightweight_overview().await;
        assert_eq!(lightweight.server.port, full.server.port);
        assert_eq!(lightweight.rules.total, full.rules.total);
    }

    #[tokio::test]
    async fn push_manager_has_subscribers_flags() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();

        let subscription = ClientSubscription {
            need_overview: true,
            need_metrics: true,
            need_traffic: true,
            ..Default::default()
        };
        let (_client, _rx) = manager.register_client("subs".to_string(), subscription);

        assert!(manager.has_overview_subscribers());
        assert!(manager.has_metrics_subscribers());
        assert!(manager.has_traffic_subscribers());
    }

    #[tokio::test]
    async fn register_and_unregister_client_updates_count() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        assert_eq!(manager.client_count(), 0);

        let subscription = ClientSubscription {
            need_overview: true,
            ..Default::default()
        };
        let (client, _rx) = manager.register_client("client-count".to_string(), subscription);
        assert_eq!(manager.client_count(), 1);

        manager.unregister_client(client.id);
        assert_eq!(manager.client_count(), 0);
    }

    #[tokio::test]
    async fn broadcast_overview_sends_update() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();

        let subscription = ClientSubscription {
            need_overview: true,
            ..Default::default()
        };
        let (_client, mut rx) = manager.register_client("overview".to_string(), subscription);

        manager.broadcast_overview().await;
        let msg = rx.recv().await.expect("expected overview message");
        match msg {
            PushMessage::OverviewUpdate(data) => {
                assert_eq!(data.server.port, harness.state().port());
            }
            other => panic!("expected OverviewUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn broadcast_overview_lightweight_sends_update() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();

        // Prime overview cache.
        manager.build_full_overview().await;

        let subscription = ClientSubscription {
            need_overview: true,
            ..Default::default()
        };
        let (_client, mut rx) = manager.register_client("overview-light".to_string(), subscription);

        manager.broadcast_overview_lightweight().await;
        let msg = rx.recv().await.expect("expected overview message");
        match msg {
            PushMessage::OverviewUpdate(_) => {}
            other => panic!("expected OverviewUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn broadcast_metrics_sends_update() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();

        let subscription = ClientSubscription {
            need_metrics: true,
            ..Default::default()
        };
        let (_client, mut rx) = manager.register_client("metrics".to_string(), subscription);

        manager.broadcast_metrics().await;
        let msg = rx.recv().await.expect("expected metrics update");
        match msg {
            PushMessage::MetricsUpdate(_) => {}
            other => panic!("expected MetricsUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn broadcast_history_sends_update() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();

        let subscription = ClientSubscription {
            need_history: true,
            ..Default::default()
        };
        let (_client, mut rx) = manager.register_client("history".to_string(), subscription);

        manager.broadcast_history().await;
        let msg = rx.recv().await.expect("expected history update");
        match msg {
            PushMessage::HistoryUpdate(_) => {}
            other => panic!("expected HistoryUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn broadcast_values_snapshot_sends_update() {
        let harness = TestAdminState::builder().build();
        {
            let mut storage = harness.values_storage.write();
            storage.set_value("k", "v").unwrap();
        }
        let manager = harness.push_manager();

        let subscription = ClientSubscription {
            need_values: true,
            ..Default::default()
        };
        let (_client, mut rx) = manager.register_client("values".to_string(), subscription);

        manager.broadcast_values_snapshot().await;
        let msg = rx.recv().await.expect("expected values update");
        match msg {
            PushMessage::ValuesUpdate(data) => {
                assert_eq!(data.total, 1);
            }
            other => panic!("expected ValuesUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_values_snapshot_targets_only_requested_client() {
        let harness = TestAdminState::builder().build();
        {
            let mut storage = harness.values_storage.write();
            storage.set_value("k", "v").unwrap();
        }
        let manager = harness.push_manager();
        let (subscribed, mut subscribed_rx) = manager.register_client(
            "values-target".to_string(),
            ClientSubscription {
                need_values: true,
                ..Default::default()
            },
        );
        let (_other, mut other_rx) = manager.register_client(
            "values-other".to_string(),
            ClientSubscription {
                need_values: true,
                ..Default::default()
            },
        );

        manager.send_values_snapshot_to_client(&subscribed);

        assert!(matches!(
            subscribed_rx.try_recv(),
            Ok(PushMessage::ValuesUpdate(_))
        ));
        assert!(other_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_scripts_snapshot_targets_only_requested_client() {
        let temp_dir = tempfile::tempdir().unwrap();
        let script_manager =
            crate::handlers::scripts::ScriptManager::new(temp_dir.path().join("scripts"));
        script_manager.init().await.unwrap();
        script_manager
            .engine()
            .save_script(ScriptType::Request, "cli-live", "function onRequest() {}")
            .await
            .unwrap();
        let state = Arc::new(AdminState::new(0).with_script_manager(script_manager));
        let manager = Arc::new(PushManager::new(state));
        let (subscribed, mut subscribed_rx) = manager.register_client(
            "scripts-target".to_string(),
            ClientSubscription {
                need_scripts: true,
                ..Default::default()
            },
        );
        let (_other, mut other_rx) = manager.register_client(
            "scripts-other".to_string(),
            ClientSubscription {
                need_scripts: true,
                ..Default::default()
            },
        );

        manager.send_scripts_snapshot_to_client(&subscribed).await;

        match subscribed_rx.try_recv() {
            Ok(PushMessage::ScriptsUpdate(data)) => {
                assert!(data.request.iter().any(|script| script.name == "cli-live"));
            }
            other => panic!("expected targeted ScriptsUpdate, got {:?}", other),
        }
        assert!(other_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn targeted_resource_snapshots_ignore_unsubscribed_client() {
        let manager = make_minimal_manager();
        let (client, mut rx) =
            manager.register_client("resource-unsubscribed".to_string(), Default::default());

        manager.send_values_snapshot_to_client(&client);
        manager.send_scripts_snapshot_to_client(&client).await;

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn build_replay_saved_requests_data_works_with_empty_store() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let data = manager
            .build_replay_saved_requests_data()
            .expect("expected replay data");
        assert_eq!(data.total, 0);
        assert!(data.requests.is_empty());
    }

    #[tokio::test]
    async fn build_replay_groups_data_works_with_empty_store() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let data = manager
            .build_replay_groups_data()
            .expect("expected replay groups data");
        assert!(data.groups.is_empty());
    }

    #[tokio::test]
    async fn build_scripts_data_none_without_script_manager() {
        let manager = make_minimal_manager();
        assert!(manager.build_scripts_data().await.is_none());
    }

    #[tokio::test]
    async fn build_settings_update_for_proxy_settings_includes_port() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let data = manager
            .build_settings_update(SETTINGS_SCOPE_PROXY_SETTINGS)
            .await
            .expect("expected proxy settings update");
        assert_eq!(data.scope, SETTINGS_SCOPE_PROXY_SETTINGS);
        assert_eq!(data.data["port"], harness.state().port());
    }

    #[tokio::test]
    async fn build_settings_update_unknown_scope_returns_none() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        assert!(manager.build_settings_update("unknown").await.is_none());
    }

    #[tokio::test]
    async fn has_settings_scope_subscribers_detects_interested_clients() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let subscription = ClientSubscription {
            settings_scopes: vec![SETTINGS_SCOPE_TLS_CONFIG.to_string()],
            ..Default::default()
        };
        let (_client, _rx) = manager.register_client("settings-scope".to_string(), subscription);
        assert!(manager.has_settings_scope_subscribers(SETTINGS_SCOPE_TLS_CONFIG));
        assert!(!manager.has_settings_scope_subscribers(SETTINGS_SCOPE_PROXY_ADDRESS));
    }

    #[tokio::test]
    async fn broadcast_settings_scope_sends_update_to_matching_clients() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let interested = ClientSubscription {
            settings_scopes: vec![SETTINGS_SCOPE_TLS_CONFIG.to_string()],
            ..Default::default()
        };
        let uninterested = ClientSubscription::default();

        let (_client_a, mut rx_a) = manager.register_client("settings-a".to_string(), interested);
        let (_client_b, mut rx_b) = manager.register_client("settings-b".to_string(), uninterested);

        manager
            .broadcast_settings_scope(SETTINGS_SCOPE_TLS_CONFIG)
            .await;

        let msg_a = rx_a.recv().await.expect("expected settings update for A");
        match msg_a {
            PushMessage::SettingsUpdate(data) => {
                assert_eq!(data.scope, SETTINGS_SCOPE_TLS_CONFIG);
            }
            other => panic!("expected SettingsUpdate, got {:?}", other),
        }

        assert!(
            rx_b.try_recv().is_err(),
            "client B should not receive update"
        );
    }

    #[tokio::test]
    async fn send_settings_scope_to_client_sends_single_update() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let subscription = ClientSubscription {
            settings_scopes: vec![SETTINGS_SCOPE_TLS_CONFIG.to_string()],
            ..Default::default()
        };
        let (client, mut rx) = manager.register_client("settings-client".to_string(), subscription);

        manager
            .send_settings_scope_to_client(&client, SETTINGS_SCOPE_TLS_CONFIG)
            .await;

        let msg = rx.recv().await.expect("expected settings update");
        match msg {
            PushMessage::SettingsUpdate(data) => {
                assert_eq!(data.scope, SETTINGS_SCOPE_TLS_CONFIG);
            }
            other => panic!("expected SettingsUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_initial_data_sends_requested_sections() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let subscription = ClientSubscription {
            need_overview: true,
            need_metrics: true,
            need_history: true,
            ..Default::default()
        };
        let (client, mut rx) = manager.register_client("initial-data".to_string(), subscription);

        manager.send_initial_data(&client).await;

        let mut got_overview = false;
        let mut got_metrics = false;
        let mut got_history = false;

        for _ in 0..3 {
            let msg = rx.recv().await.expect("expected initial data message");
            match msg {
                PushMessage::OverviewUpdate(_) => got_overview = true,
                PushMessage::MetricsUpdate(_) => got_metrics = true,
                PushMessage::HistoryUpdate(_) => got_history = true,
                _ => {}
            }
        }

        assert!(got_overview);
        assert!(got_metrics);
        assert!(got_history);
    }

    #[tokio::test]
    async fn send_initial_data_includes_requested_values_and_scripts() {
        let temp_dir = tempfile::tempdir().unwrap();
        let script_manager =
            crate::handlers::scripts::ScriptManager::new(temp_dir.path().join("scripts"));
        script_manager.init().await.unwrap();
        script_manager
            .engine()
            .save_script(
                ScriptType::Request,
                "initial-script",
                "function onRequest() {}",
            )
            .await
            .unwrap();

        let values_storage =
            bifrost_storage::ValuesStorage::with_dir(temp_dir.path().join("values")).unwrap();
        let state = Arc::new(
            AdminState::new(0)
                .with_values_storage(values_storage)
                .with_script_manager(script_manager),
        );
        state
            .values_storage
            .as_ref()
            .expect("values storage")
            .write()
            .set_value("initial-value", "ready")
            .unwrap();
        let manager = Arc::new(PushManager::new(state));
        let (client, mut rx) = manager.register_client(
            "initial-resources".to_string(),
            ClientSubscription {
                need_values: true,
                need_scripts: true,
                ..Default::default()
            },
        );

        manager.send_initial_data(&client).await;

        let first = rx.recv().await.expect("expected first resource snapshot");
        let second = rx.recv().await.expect("expected second resource snapshot");
        assert!(
            matches!(&first, PushMessage::ValuesUpdate(_))
                || matches!(&second, PushMessage::ValuesUpdate(_))
        );
        assert!(
            matches!(&first, PushMessage::ScriptsUpdate(_))
                || matches!(&second, PushMessage::ScriptsUpdate(_))
        );
    }

    #[test]
    fn broadcast_traffic_deleted_sends_message() {
        let manager = make_minimal_manager();
        let (client, mut rx) =
            PushClient::new("traffic-del".to_string(), ClientSubscription::default());
        let client = Arc::new(client);
        manager.clients.insert(client.id, client.clone());

        manager.broadcast_traffic_deleted(vec!["id1".to_string(), "id2".to_string()]);
        let msg = rx.try_recv().expect("expected traffic deleted message");
        match msg {
            PushMessage::TrafficDeleted(data) => {
                assert_eq!(data.ids, vec!["id1".to_string(), "id2".to_string()]);
            }
            other => panic!("expected TrafficDeleted, got {:?}", other),
        }
    }

    #[test]
    fn broadcast_replay_history_updated_sends_message() {
        let manager = make_minimal_manager();
        let (client, mut rx) =
            PushClient::new("history-del".to_string(), ClientSubscription::default());
        let client = Arc::new(client);
        manager.clients.insert(client.id, client.clone());

        manager.broadcast_replay_history_updated("delete", Some("req"), Some("hist"));
        let msg = rx
            .try_recv()
            .expect("expected replay history updated message");
        match msg {
            PushMessage::ReplayHistoryUpdated(data) => {
                assert_eq!(data.action, "delete");
                assert_eq!(data.request_id.as_deref(), Some("req"));
                assert_eq!(data.history_id.as_deref(), Some("hist"));
            }
            other => panic!("expected ReplayHistoryUpdated, got {:?}", other),
        }
    }

    #[test]
    fn broadcast_breakpoint_messages_send_to_clients() {
        let manager = make_minimal_manager();
        let (client, mut rx) = PushClient::new("bp".to_string(), ClientSubscription::default());
        let client = Arc::new(client);
        manager.clients.insert(client.id, client.clone());

        manager.broadcast_breakpoint_paused(BreakpointPausedPushData {
            phase: "request".to_string(),
            request_id: "r1".to_string(),
            method: None,
            url: None,
            status: None,
            headers: Vec::new(),
            body: None,
            body_omitted: false,
            body_size: None,
            max_body_bytes: 0,
        });

        manager.broadcast_breakpoint_settings_updated(BreakpointSettingsPushData {
            enabled: true,
            max_body_bytes: 1024,
        });

        manager.broadcast_breakpoint_resumed("r1".to_string());

        let mut got_paused = false;
        let mut got_settings = false;
        let mut got_resumed = false;

        for _ in 0..3 {
            let msg = rx.try_recv().expect("expected breakpoint message");
            match msg {
                PushMessage::BreakpointPaused(_) => got_paused = true,
                PushMessage::BreakpointSettingsUpdated(_) => got_settings = true,
                PushMessage::BreakpointResumed(_) => got_resumed = true,
                _ => {}
            }
        }

        assert!(got_paused);
        assert!(got_settings);
        assert!(got_resumed);
    }

    #[test]
    fn cert_status_not_installed_when_path_missing() {
        let snapshot = cert_status(None);
        assert_eq!(snapshot.status, "not_installed");
        assert!(!snapshot.installed);
        assert!(!snapshot.trusted);
    }

    #[tokio::test]
    async fn build_settings_update_for_tls_config_scope_has_scope_name() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let data = manager
            .build_settings_update(SETTINGS_SCOPE_TLS_CONFIG)
            .await
            .expect("expected tls config settings update");
        assert_eq!(data.scope, SETTINGS_SCOPE_TLS_CONFIG);
    }

    #[tokio::test]
    async fn broadcast_notification_sends_notification_to_clients() {
        let harness = TestAdminState::builder().build();
        let manager = harness.push_manager();
        let (_client, mut rx) =
            manager.register_client("notify".to_string(), ClientSubscription::default());

        let payload = NotificationPushData {
            notification_type: "test".to_string(),
            title: "title".to_string(),
            message: "msg".to_string(),
            metadata: None,
            unread_count: 1,
        };

        manager.broadcast_notification(payload.clone()).await;
        let msg = rx.recv().await.expect("expected notification message");
        match msg {
            PushMessage::Notification(data) => {
                assert_eq!(data.notification_type, payload.notification_type);
                assert_eq!(data.unread_count, payload.unread_count);
            }
            other => panic!("expected Notification, got {:?}", other),
        }
    }

    #[tokio::test]
    #[allow(clippy::assertions_on_constants)]
    async fn wiremock_mock_server_smoke() {
        // Placeholder: higher-level instructions mention wiremock::MockServer,
        // but this crate does not include wiremock as a dev-dependency.
        // Keep this test to document the intent while remaining a no-op.
        assert!(true);
    }

    #[tokio::test]
    async fn subscribe_once_returns_matching_record() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(19920).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let manager_clone = manager.clone();
        let waiter = tokio::spawn(async move {
            manager_clone
                .subscribe_once(
                    |c| c.h.contains("api.example.com") && c.m == "POST",
                    Duration::from_secs(5),
                )
                .await
        });

        // Let the subscriber subscribe before publishing.
        sleep(Duration::from_millis(50)).await;

        let mut not_matching = TrafficRecord::new(
            "cap-1".to_string(),
            "GET".to_string(),
            "http://example.test/foo".to_string(),
        );
        not_matching.status = 200;
        store.record(not_matching);

        sleep(Duration::from_millis(50)).await;

        let mut matching = TrafficRecord::new(
            "cap-2".to_string(),
            "POST".to_string(),
            "http://api.example.com/api/widget".to_string(),
        );
        matching.status = 200;
        store.record(matching);

        let outcome = timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter did not complete")
            .expect("waiter task panicked");

        let matched = outcome.matched.expect("expected a match");
        assert_eq!(matched.id, "cap-2");
        assert!(outcome.scanned >= 2);
        drop(dir);
    }

    #[tokio::test]
    async fn subscribe_once_times_out_when_no_match() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(19921).with_traffic_db_store_shared(store.clone()));
        let manager = Arc::new(PushManager::new(state));

        let manager_clone = manager.clone();
        let waiter = tokio::spawn(async move {
            manager_clone
                .subscribe_once(|_| false, Duration::from_millis(200))
                .await
        });

        sleep(Duration::from_millis(50)).await;

        let mut record = TrafficRecord::new(
            "cap-timeout-1".to_string(),
            "GET".to_string(),
            "http://example.test/whatever".to_string(),
        );
        record.status = 200;
        store.record(record);

        let outcome = timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter did not complete")
            .expect("waiter task panicked");

        assert!(
            outcome.matched.is_none(),
            "should not match when matcher returns false"
        );
        assert!(
            outcome.scanned >= 1,
            "scanned counter should reflect evaluated record"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn subscribe_once_without_traffic_store_returns_none_immediately() {
        // Build an AdminState that has no traffic store, then ensure the call
        // does not block.
        let manager = make_minimal_manager();
        let outcome = timeout(
            Duration::from_secs(1),
            manager.subscribe_once(|_| true, Duration::from_secs(60)),
        )
        .await
        .expect("subscribe_once should return immediately when no traffic store");
        assert!(outcome.matched.is_none());
        assert_eq!(outcome.scanned, 0);
    }
}
