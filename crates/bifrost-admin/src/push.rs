use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
use crate::traffic_db::{Direction, QueryParams, TrafficStoreEvent, TrafficSummaryCompact};

static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const PUSH_CHANNEL_CAPACITY: usize = 64;
pub const MAX_SUBSCRIBED_IDS: usize = 500;
pub const MAX_CLIENT_CHANNELS: usize = 3;
pub const MAX_ID_LEN: usize = 256;
pub const MAX_SETTINGS_SCOPES: usize = 16;
const TRAFFIC_PENDING_REFRESH_INTERVAL_MS: u64 = 2_000;

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

pub const METRICS_INTERVAL_MIN_MS: u64 = 200;
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
}

pub struct PushManager {
    clients: DashMap<u64, Arc<PushClient>>,
    buckets: DashMap<String, Vec<u64>>,
    bucket_order: Mutex<VecDeque<String>>,
    overview_cache: RwLock<Option<OverviewData>>,
    state: SharedAdminState,
}

impl PushManager {
    pub fn new(state: SharedAdminState) -> Self {
        Self {
            clients: DashMap::new(),
            buckets: DashMap::new(),
            bucket_order: Mutex::new(VecDeque::new()),
            overview_cache: RwLock::new(None),
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

    async fn build_full_overview(&self) -> OverviewData {
        let system_info = crate::metrics::SystemInfo::new(self.state.start_time);
        let metrics = self.state.metrics_collector.get_current();
        let traffic_count = self
            .state
            .traffic_db_store
            .as_ref()
            .map(|db| db.stats().record_count)
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
        overview
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
        has_more: bool,
        server_total: usize,
        server_sequence: u64,
    ) -> bool {
        let inserts = dedupe_compact_records_keep_latest(inserts);
        let updates = dedupe_compact_records_keep_latest(updates);

        if inserts.is_empty() && updates.is_empty() {
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
            has_more,
            server_total,
            server_sequence,
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
                false,
                server_total,
                server_sequence,
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
                result.has_more,
                result.total,
                result.server_sequence,
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

        let _ = self.send_traffic_delta_to_client(
            client,
            new_records,
            updated_records,
            result.has_more,
            result.total,
            result.server_sequence,
        );
    }

    pub fn send_initial_traffic(&self, client: &Arc<PushClient>) {
        let subscription = client.get_subscription();
        if !subscription.need_traffic {
            return;
        }

        if let Some(ref db_store) = self.state.traffic_db_store {
            self.send_initial_traffic_delta(client, db_store, &subscription);
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

    pub async fn broadcast_metrics_with_interval(&self, elapsed_ms: u64) {
        let mut clients_to_remove = Vec::new();

        let metrics = self.state.metrics_collector.get_current();
        let metrics_data = MetricsData {
            metrics: serde_json::to_value(&metrics).unwrap_or_default(),
        };

        for client_ref in self.clients.iter() {
            let client = client_ref.value();
            let subscription = client.get_subscription();

            if subscription.need_metrics {
                let client_interval = subscription
                    .metrics_interval_ms
                    .clamp(METRICS_INTERVAL_MIN_MS, METRICS_INTERVAL_MAX_MS);

                let should_send = elapsed_ms == 0 || elapsed_ms % client_interval < 500;

                if should_send {
                    let msg = PushMessage::MetricsUpdate(metrics_data.clone());
                    if !client.send(msg) {
                        clients_to_remove.push(client.id);
                    }
                }
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
            let metrics = self.state.metrics_collector.get_current();
            client.send(PushMessage::MetricsUpdate(MetricsData {
                metrics: serde_json::to_value(&metrics).unwrap_or_default(),
            }));
        }

        if subscription.need_values {
            if let Some(values_data) = self.build_values_data() {
                client.send(PushMessage::ValuesUpdate(values_data));
            }
        }

        if subscription.need_scripts {
            if let Some(scripts_data) = self.build_scripts_data().await {
                client.send(PushMessage::ScriptsUpdate(scripts_data));
            }
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
        let manager_traffic = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut receiver = db_store.subscribe();
            loop {
                let first_event = match receiver.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
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

                loop {
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
            dur: 0,
            lp: 0,
            proto: "http".to_string(),
            cip: "127.0.0.1".to_string(),
            capp: None,
            cpid: None,
            flags: 0,
            fc: 0,
            ss: None,
            st: "-".to_string(),
            et: None,
            rc: 0,
            rp: vec![],
        }
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

        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn send_initial_traffic_without_last_sequence_uses_latest_window() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 2000, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9914).with_traffic_db_store_shared(store.clone()));
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
            false,
            1,
            200,
        ));

        assert_eq!(client.get_subscription().last_sequence, Some(200));
    }

    #[test]
    fn send_traffic_delta_dedupes_updates_by_id_keep_latest() {
        let state = Arc::new(AdminState::new(9916));
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
            false,
            1,
            10,
        ));

        let message = receiver.try_recv().expect("expected traffic delta");
        let PushMessage::TrafficDelta(data) = message else {
            panic!("expected traffic delta");
        };

        assert_eq!(data.updates.len(), 1);
        assert_eq!(data.updates[0].id, "same-id");
        assert_eq!(data.updates[0].res_sz, latest.res_sz);
        assert_eq!(data.updates[0].fc, latest.fc);
    }
}
