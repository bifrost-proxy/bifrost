use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use ring::digest;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::debug;

#[path = "devtools_network.rs"]
mod devtools_network;
use devtools_network::{merge_network_event, merge_network_events, network_event_from_input};

const BRIDGE_COMMAND_QUEUE_CAPACITY: usize = 128;
const SESSION_LIVE_QUEUE_CAPACITY: usize = 512;
const BRIDGE_SEEN_SEQ_CAPACITY: usize = 2048;
const PAGE_CONSOLE_CACHE_CAPACITY: usize = 500;
const PAGE_NETWORK_CACHE_CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugAdapterKind {
    PageBridge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugFidelity {
    Fallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugPageState {
    Candidate,
    Discoverable,
    FallbackAttached,
    Stale,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DevtoolsMode {
    Read,
    Control,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedDevtoolsRule {
    pub pattern: String,
    pub raw: Option<String>,
    pub line: Option<usize>,
    #[serde(default)]
    pub evaluate_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityMatrix {
    pub console_subscribe: String,
    pub dom_snapshot: String,
    pub runtime_evaluate: String,
    pub network_observe: String,
    pub debugger_breakpoints: String,
    pub page_screenshot: String,
    pub input_dispatch: String,
}

impl CapabilityMatrix {
    fn page_bridge(_mode: &DevtoolsMode) -> Self {
        Self {
            console_subscribe: "supported".to_string(),
            dom_snapshot: "supported".to_string(),
            runtime_evaluate: "supported".to_string(),
            network_observe: "partial".to_string(),
            debugger_breakpoints: "unsupported".to_string(),
            page_screenshot: "unsupported".to_string(),
            input_dispatch: "unsupported".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugPage {
    pub page_id: String,
    pub title: Option<String>,
    pub url: String,
    pub origin: String,
    pub user_agent: Option<String>,
    pub adapter: DebugAdapterKind,
    pub fidelity: DebugFidelity,
    pub state: DebugPageState,
    pub mode: DevtoolsMode,
    pub matched_rule: Option<MatchedDevtoolsRule>,
    pub traffic_ids: Vec<String>,
    pub last_seen_at_ms: u64,
    pub capabilities: CapabilityMatrix,
    pub status_reason: Option<String>,
    #[serde(skip_serializing)]
    pub bridge_token: String,
    #[serde(skip_serializing)]
    pub bridge_tab_id: Option<String>,
    #[serde(skip_serializing)]
    pub dom_snapshot: Option<String>,
    #[serde(skip_serializing)]
    pub dom_tree: Option<serde_json::Value>,
    #[serde(skip_serializing)]
    pub dom_updated_at_ms: u64,
    #[serde(skip_serializing)]
    pub console_messages: Vec<ConsoleMessage>,
    #[serde(skip_serializing)]
    pub network_events: Vec<NetworkEvent>,
    #[serde(skip_serializing)]
    pub storage_snapshot: Option<StorageSnapshot>,
    #[serde(default)]
    pub evaluate_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub level: String,
    pub text: String,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEvent {
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub resource_type: String,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_params: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_req_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageSnapshot {
    pub local_storage: Vec<(String, String)>,
    pub session_storage: Vec<(String, String)>,
    pub cookies: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub session_id: String,
    pub page_id: String,
    pub adapter: DebugAdapterKind,
    pub mode: DevtoolsMode,
    pub state: String,
    pub opened_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpTargetInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub web_socket_debugger_url: String,
}

#[derive(Debug, Clone)]
pub struct RegisterPageInput {
    pub url: String,
    pub origin: String,
    pub traffic_id: String,
    pub mode: DevtoolsMode,
    pub matched_rule: Option<MatchedDevtoolsRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateAuditRecord {
    pub ts_unix_ms: u64,
    pub rule_id: Option<String>,
    pub target_url: String,
    pub target_page_id: String,
    pub caller_client_id: Option<String>,
    pub expression_sha256: String,
    pub expression_preview: String,
    pub world: String,
    pub rejected_by_allowlist: bool,
}

#[derive(Debug, Deserialize)]
pub struct BridgeHelloPayload {
    pub token: String,
    #[serde(default)]
    pub scope: Option<String>,
    pub tab_id: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub dom_snapshot: Option<String>,
    #[serde(default)]
    pub dom_tree: Option<serde_json::Value>,
    #[serde(default)]
    pub storage: Option<StorageSnapshot>,
    #[serde(default)]
    pub console: Vec<ConsoleMessage>,
    #[serde(default)]
    pub network: Vec<NetworkEventInput>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeConsolePayload {
    pub token: String,
    pub level: Option<String>,
    pub text: String,
    #[serde(default)]
    pub at_ms: Option<u64>,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
    #[serde(default)]
    pub raw: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeClosePayload {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkEventInput {
    pub url: String,
    pub method: Option<String>,
    pub status: Option<u16>,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub client_req_id: Option<String>,
    #[serde(default)]
    pub query_params: Vec<(String, String)>,
    #[serde(default)]
    pub request_headers: Vec<(String, String)>,
    #[serde(default)]
    pub response_headers: Vec<(String, String)>,
    #[serde(default)]
    pub from_cache: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeNetworkPayload {
    pub token: String,
    pub event: NetworkEventInput,
}

#[derive(Debug, Deserialize)]
pub struct BridgeNodeSelectedPayload {
    pub token: String,
    pub node_id: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeEvalCommand {
    pub eval_id: u64,
    pub expression: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeOverlayCommand {
    HighlightNode { node_id: u64 },
    StartInspect,
    HideHighlight,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeServerMessage {
    Eval { command: BridgeEvalCommand },
    Overlay { command: BridgeOverlayCommand },
    SnapshotRequest { scope: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DevtoolsLiveMessage {
    Snapshot { snapshot: serde_json::Value },
    Console { message: ConsoleMessage },
    Network { event: NetworkEvent },
    NodeSelected { node_id: u64 },
    Disconnected { page_id: String, reason: String },
}

#[derive(Debug, Deserialize)]
pub struct BridgeEvalPollPayload {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct BridgeEvalResultPayload {
    pub token: String,
    pub eval_id: u64,
    pub result: Option<serde_json::Value>,
    pub exception: Option<String>,
}

pub struct BrowserDebugBroker {
    pages: RwLock<HashMap<String, DebugPage>>,
    sessions: RwLock<HashMap<String, DebugSession>>,
    eval_next_id: AtomicU64,
    eval_pending: RwLock<HashMap<String, Vec<BridgeEvalCommand>>>,
    eval_results: RwLock<HashMap<u64, Result<serde_json::Value, String>>>,
    overlay_pending: RwLock<HashMap<String, Vec<BridgeOverlayCommand>>>,
    bridge_senders: RwLock<HashMap<String, mpsc::Sender<BridgeServerMessage>>>,
    session_senders: RwLock<HashMap<String, mpsc::Sender<DevtoolsLiveMessage>>>,
    bridge_seen_seqs: RwLock<HashMap<String, VecDeque<u64>>>,
    evaluate_audit: RwLock<VecDeque<EvaluateAuditRecord>>,
    evaluate_audit_capacity: usize,
}

pub type SharedBrowserDebugBroker = Arc<BrowserDebugBroker>;

impl BrowserDebugBroker {
    pub fn new() -> Self {
        Self {
            pages: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            eval_next_id: AtomicU64::new(0),
            eval_pending: RwLock::new(HashMap::new()),
            eval_results: RwLock::new(HashMap::new()),
            overlay_pending: RwLock::new(HashMap::new()),
            bridge_senders: RwLock::new(HashMap::new()),
            session_senders: RwLock::new(HashMap::new()),
            bridge_seen_seqs: RwLock::new(HashMap::new()),
            evaluate_audit: RwLock::new(VecDeque::new()),
            evaluate_audit_capacity: std::env::var("BIFROST_DEVTOOLS_EVALUATE_AUDIT_CAPACITY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1000),
        }
    }

    pub fn register_page_candidate(&self, input: RegisterPageInput) -> (String, String) {
        let page_id = format!("pg_{}", uuid::Uuid::new_v4().simple());
        let bridge_token = format!("bdt_{}", uuid::Uuid::new_v4().simple());
        let capabilities = CapabilityMatrix::page_bridge(&input.mode);
        let evaluate_allowlist = input
            .matched_rule
            .as_ref()
            .map(|rule| rule.evaluate_allowlist.clone())
            .unwrap_or_default();
        let page = DebugPage {
            page_id: page_id.clone(),
            title: None,
            url: input.url,
            origin: input.origin,
            user_agent: None,
            adapter: DebugAdapterKind::PageBridge,
            fidelity: DebugFidelity::Fallback,
            state: DebugPageState::Candidate,
            mode: input.mode,
            matched_rule: input.matched_rule,
            traffic_ids: vec![input.traffic_id],
            last_seen_at_ms: now_ms(),
            capabilities,
            status_reason: None,
            bridge_token: bridge_token.clone(),
            bridge_tab_id: None,
            dom_snapshot: None,
            dom_tree: None,
            dom_updated_at_ms: 0,
            console_messages: Vec::new(),
            network_events: Vec::new(),
            storage_snapshot: None,
            evaluate_allowlist,
        };
        self.pages.write().insert(page_id.clone(), page);
        (page_id, bridge_token)
    }

    pub fn bridge_hello(&self, page_id: &str, payload: BridgeHelloPayload) -> Result<(), String> {
        let connected_page_ids: HashSet<String> =
            self.bridge_senders.read().keys().cloned().collect();
        let mut pages = self.pages.write();
        let expected_token = pages
            .get(page_id)
            .map(|page| page.bridge_token.clone())
            .ok_or_else(|| "page not found".to_string())?;
        if expected_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        let mut tab_id = payload
            .tab_id
            .clone()
            .filter(|value| !value.trim().is_empty());
        if let Some(original_tab_id) = tab_id.clone() {
            let same_tab_online = pages.iter().any(|(id, existing)| {
                id != page_id
                    && connected_page_ids.contains(id)
                    && existing.bridge_tab_id.as_deref() == Some(original_tab_id.as_str())
                    && matches!(existing.adapter, DebugAdapterKind::PageBridge)
            });
            if same_tab_online {
                tab_id = Some(format!("{original_tab_id}:{page_id}"));
            }
        }
        let replaced_page_ids: Vec<String> = tab_id
            .as_ref()
            .map(|tab_id| {
                pages
                    .iter()
                    .filter_map(|(id, existing)| {
                        if id != page_id
                            && !connected_page_ids.contains(id)
                            && existing.bridge_tab_id.as_deref() == Some(tab_id.as_str())
                            && matches!(existing.adapter, DebugAdapterKind::PageBridge)
                        {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(tab_id) = &tab_id {
            pages.retain(|id, existing| {
                id == page_id
                    || connected_page_ids.contains(id)
                    || existing.bridge_tab_id.as_deref() != Some(tab_id.as_str())
                    || !matches!(existing.adapter, DebugAdapterKind::PageBridge)
            });
        }
        drop(pages);
        for replaced_page_id in replaced_page_ids {
            self.migrate_page_references(&replaced_page_id, page_id);
        }
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        page.bridge_tab_id = tab_id;
        if let Some(title) = payload.title.clone() {
            if !title.trim().is_empty() {
                page.title = Some(title);
            }
        }
        if let Some(url) = payload.url.clone() {
            page.url = url;
        }
        if payload.user_agent.is_some() {
            page.user_agent = payload.user_agent.clone();
        }
        page.state = DebugPageState::Discoverable;
        page.last_seen_at_ms = now_ms();
        if payload.dom_snapshot.is_some() || payload.dom_tree.is_some() {
            page.dom_snapshot = payload.dom_snapshot.clone();
            page.dom_tree = payload.dom_tree.clone();
            page.dom_updated_at_ms = now_ms();
        }
        if let Some(storage) = payload.storage.clone() {
            page.storage_snapshot = Some(storage);
        }
        if !payload.console.is_empty() {
            page.console_messages.extend(payload.console.clone());
            trim_vec_front(&mut page.console_messages, PAGE_CONSOLE_CACHE_CAPACITY);
        }
        if !payload.network.is_empty() {
            let events: Vec<NetworkEvent> = payload
                .network
                .iter()
                .cloned()
                .filter_map(network_event_from_input)
                .collect();
            merge_network_events(&mut page.network_events, events);
            trim_vec_front(&mut page.network_events, PAGE_NETWORK_CACHE_CAPACITY);
        }
        let snapshot = snapshot_from_bridge_payload(page, payload);
        drop(pages);
        self.push_live_to_page(page_id, DevtoolsLiveMessage::Snapshot { snapshot });
        Ok(())
    }

    pub fn bridge_console(
        &self,
        page_id: &str,
        payload: BridgeConsolePayload,
    ) -> Result<(), String> {
        let Some(mut pages) = self.pages.try_write() else {
            debug!(
                page_id = %page_id,
                "devtools broker busy; dropping console event to avoid blocking proxy runtime"
            );
            return Ok(());
        };
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        let message = ConsoleMessage {
            level: payload.level.unwrap_or_else(|| "log".to_string()),
            text: payload.text,
            at_ms: payload.at_ms.unwrap_or_else(now_ms),
            args: payload.args,
            raw: payload.raw,
        };
        page.last_seen_at_ms = now_ms();
        page.console_messages.push(message.clone());
        trim_vec_front(&mut page.console_messages, PAGE_CONSOLE_CACHE_CAPACITY);
        drop(pages);
        self.push_live_to_page(page_id, DevtoolsLiveMessage::Console { message });
        Ok(())
    }

    pub fn bridge_close(&self, page_id: &str, payload: BridgeClosePayload) -> Result<(), String> {
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        page.state = DebugPageState::Stale;
        page.status_reason =
            Some("page bridge disconnected; waiting for reload reconnect".to_string());
        page.last_seen_at_ms = now_ms();
        let reason = page
            .status_reason
            .clone()
            .unwrap_or_else(|| "page bridge disconnected".to_string());
        drop(pages);
        self.push_live_to_page(
            page_id,
            DevtoolsLiveMessage::Disconnected {
                page_id: page_id.to_string(),
                reason,
            },
        );
        Ok(())
    }

    pub fn queue_eval(&self, page_id: &str, expression: String) -> Result<u64, String> {
        if !self.pages.read().contains_key(page_id) {
            return Err("page not found".to_string());
        }
        let eval_id = self.eval_next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.eval_pending
            .write()
            .entry(page_id.to_string())
            .or_default()
            .push(BridgeEvalCommand {
                eval_id,
                expression: expression.clone(),
            });
        self.send_eval_if_connected(page_id);
        Ok(eval_id)
    }

    pub fn bridge_eval_next(
        &self,
        page_id: &str,
        payload: BridgeEvalPollPayload,
    ) -> Result<Option<BridgeEvalCommand>, String> {
        let pages = self.pages.read();
        let page = pages
            .get(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        drop(pages);
        let mut pending = self.eval_pending.write();
        let Some(commands) = pending.get_mut(page_id) else {
            return Ok(None);
        };
        if commands.is_empty() {
            Ok(None)
        } else {
            Ok(Some(commands.remove(0)))
        }
    }

    pub fn bridge_eval_result(
        &self,
        page_id: &str,
        payload: BridgeEvalResultPayload,
    ) -> Result<(), String> {
        let pages = self.pages.read();
        let page = pages
            .get(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        drop(pages);
        let result = match payload.exception {
            Some(exception) => Err(exception),
            None => Ok(payload.result.unwrap_or_else(
                || serde_json::json!({"type": "undefined", "description": "undefined"}),
            )),
        };
        self.eval_results.write().insert(payload.eval_id, result);
        Ok(())
    }

    pub fn bridge_ws_attach(
        &self,
        page_id: &str,
        token: &str,
        sender: mpsc::Sender<BridgeServerMessage>,
    ) -> Result<(), String> {
        let pages = self.pages.read();
        let page = pages
            .get(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != token {
            return Err("bridge token mismatch".to_string());
        }
        drop(pages);
        self.bridge_senders
            .write()
            .insert(page_id.to_string(), sender);
        self.send_eval_if_connected(page_id);
        self.send_overlay_if_connected(page_id);
        Ok(())
    }

    pub fn remember_bridge_seq(&self, page_id: &str, seq: u64) -> bool {
        let mut seen_by_page = self.bridge_seen_seqs.write();
        let seen = seen_by_page.entry(page_id.to_string()).or_default();
        if seen.contains(&seq) {
            return false;
        }
        seen.push_back(seq);
        while seen.len() > BRIDGE_SEEN_SEQ_CAPACITY {
            seen.pop_front();
        }
        true
    }

    pub fn bridge_ws_detach(&self, page_id: &str) {
        self.bridge_senders.write().remove(page_id);
        self.push_live_to_page(
            page_id,
            DevtoolsLiveMessage::Disconnected {
                page_id: page_id.to_string(),
                reason: "page bridge websocket disconnected".to_string(),
            },
        );
    }

    pub fn session_ws_attach(
        &self,
        session_id: &str,
        sender: mpsc::Sender<DevtoolsLiveMessage>,
    ) -> Result<(), String> {
        let snapshot = self.snapshot(session_id)?;
        self.session_senders
            .write()
            .insert(session_id.to_string(), sender.clone());
        let _ = sender.try_send(DevtoolsLiveMessage::Snapshot { snapshot });
        Ok(())
    }

    pub fn session_ws_detach(&self, session_id: &str) {
        self.session_senders.write().remove(session_id);
    }

    pub fn queue_overlay(
        &self,
        page_id: &str,
        command: BridgeOverlayCommand,
    ) -> Result<(), String> {
        if !self.pages.read().contains_key(page_id) {
            return Err("page not found".to_string());
        }
        self.overlay_pending
            .write()
            .entry(page_id.to_string())
            .or_default()
            .push(command);
        self.send_overlay_if_connected(page_id);
        Ok(())
    }

    pub fn bridge_overlay_next(
        &self,
        page_id: &str,
        payload: BridgeEvalPollPayload,
    ) -> Result<Option<BridgeOverlayCommand>, String> {
        let pages = self.pages.read();
        let page = pages
            .get(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        drop(pages);
        let mut pending = self.overlay_pending.write();
        let Some(commands) = pending.get_mut(page_id) else {
            return Ok(None);
        };
        if commands.is_empty() {
            Ok(None)
        } else {
            Ok(Some(commands.remove(0)))
        }
    }

    pub fn take_eval_result(&self, eval_id: u64) -> Option<Result<serde_json::Value, String>> {
        self.eval_results.write().remove(&eval_id)
    }

    fn send_eval_if_connected(&self, page_id: &str) {
        let Some(sender) = self.bridge_senders.read().get(page_id).cloned() else {
            return;
        };
        let command = {
            let mut pending = self.eval_pending.write();
            pending
                .get_mut(page_id)
                .and_then(|commands| (!commands.is_empty()).then(|| commands.remove(0)))
        };
        if let Some(command) = command {
            if sender
                .try_send(BridgeServerMessage::Eval { command })
                .is_err()
            {
                self.bridge_senders.write().remove(page_id);
            }
        }
    }

    fn send_overlay_if_connected(&self, page_id: &str) {
        let Some(sender) = self.bridge_senders.read().get(page_id).cloned() else {
            return;
        };
        let command = {
            let mut pending = self.overlay_pending.write();
            pending
                .get_mut(page_id)
                .and_then(|commands| (!commands.is_empty()).then(|| commands.remove(0)))
        };
        if let Some(command) = command {
            if sender
                .try_send(BridgeServerMessage::Overlay { command })
                .is_err()
            {
                self.bridge_senders.write().remove(page_id);
            }
        }
    }

    fn push_live_to_page(&self, page_id: &str, message: DevtoolsLiveMessage) {
        let session_ids: Vec<String> = self
            .sessions
            .read()
            .values()
            .filter(|session| session.page_id == page_id)
            .map(|session| session.session_id.clone())
            .collect();
        let senders = self.session_senders.read();
        let mut stale_session_ids = Vec::new();
        for session_id in session_ids {
            if let Some(sender) = senders.get(&session_id) {
                if sender.try_send(message.clone()).is_err() {
                    stale_session_ids.push(session_id);
                }
            }
        }
        drop(senders);
        if !stale_session_ids.is_empty() {
            let mut senders = self.session_senders.write();
            for session_id in stale_session_ids {
                senders.remove(&session_id);
            }
        }
    }

    pub fn record_evaluate_audit(
        &self,
        page: &DebugPage,
        expression: &str,
        world: &str,
        caller_client_id: Option<String>,
        rejected_by_allowlist: bool,
    ) -> EvaluateAuditRecord {
        let record = EvaluateAuditRecord {
            ts_unix_ms: now_ms(),
            rule_id: page.matched_rule.as_ref().map(rule_id),
            target_url: page.url.clone(),
            target_page_id: page.page_id.clone(),
            caller_client_id,
            expression_sha256: sha256_hex(expression),
            expression_preview: preview(expression, 200),
            world: world.to_string(),
            rejected_by_allowlist,
        };
        tracing::info!(
            target: "bifrost_admin::devtools::audit",
            ts_unix_ms = record.ts_unix_ms,
            rule_id = record.rule_id.as_deref().unwrap_or(""),
            target_url = %record.target_url,
            target_page_id = %record.target_page_id,
            caller_client_id = record.caller_client_id.as_deref().unwrap_or(""),
            expression_sha256 = %record.expression_sha256,
            expression_preview = %record.expression_preview,
            world = %record.world,
            rejected_by_allowlist = record.rejected_by_allowlist,
            "DevTools Runtime.evaluate audit"
        );
        let mut audit = self.evaluate_audit.write();
        audit.push_back(record.clone());
        while audit.len() > self.evaluate_audit_capacity {
            audit.pop_front();
        }
        record
    }

    pub fn list_evaluate_audit(
        &self,
        limit: Option<usize>,
        since: Option<u64>,
    ) -> Vec<EvaluateAuditRecord> {
        let limit = limit
            .unwrap_or(self.evaluate_audit_capacity)
            .min(self.evaluate_audit_capacity);
        let since = since.unwrap_or(0);
        let audit = self.evaluate_audit.read();
        let mut records: Vec<EvaluateAuditRecord> = audit
            .iter()
            .rev()
            .filter(|record| record.ts_unix_ms >= since)
            .take(limit)
            .cloned()
            .collect();
        records.reverse();
        records
    }

    pub fn evaluate_audit_capacity(&self) -> usize {
        self.evaluate_audit_capacity
    }

    pub fn expression_allowed_by_page(page: &DebugPage, expression: &str) -> bool {
        page.evaluate_allowlist.is_empty()
            || page.evaluate_allowlist.iter().any(|pattern| {
                regex::Regex::new(pattern)
                    .map(|regex| regex.is_match(expression))
                    .unwrap_or(false)
            })
    }

    pub fn bridge_network(
        &self,
        page_id: &str,
        payload: BridgeNetworkPayload,
    ) -> Result<(), String> {
        let Some(mut pages) = self.pages.try_write() else {
            debug!(
                page_id = %page_id,
                "devtools broker busy; dropping network event to avoid blocking proxy runtime"
            );
            return Ok(());
        };
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        let event = network_event_from_input(payload.event);
        page.last_seen_at_ms = now_ms();
        if let Some(event) = event.clone() {
            merge_network_event(&mut page.network_events, event);
            trim_vec_front(&mut page.network_events, PAGE_NETWORK_CACHE_CAPACITY);
        }
        drop(pages);
        if let Some(event) = event {
            self.push_live_to_page(page_id, DevtoolsLiveMessage::Network { event });
        }
        Ok(())
    }

    pub fn bridge_node_selected(
        &self,
        page_id: &str,
        payload: BridgeNodeSelectedPayload,
    ) -> Result<(), String> {
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        page.last_seen_at_ms = now_ms();
        drop(pages);
        self.push_live_to_page(
            page_id,
            DevtoolsLiveMessage::NodeSelected {
                node_id: payload.node_id,
            },
        );
        Ok(())
    }

    pub fn list_pages(&self, online_only: bool) -> Vec<DebugPage> {
        self.prune_inactive_pages();
        let cutoff = now_ms().saturating_sub(Duration::from_secs(60).as_millis() as u64);
        let connected_page_ids: HashSet<String> =
            self.bridge_senders.read().keys().cloned().collect();
        let mut pages: Vec<DebugPage> = self
            .pages
            .read()
            .values()
            .filter(|page| {
                !online_only
                    || page.last_seen_at_ms >= cutoff
                    || connected_page_ids.contains(&page.page_id)
            })
            .cloned()
            .collect();
        pages.sort_by_key(|page| std::cmp::Reverse(page.last_seen_at_ms));
        pages
    }

    pub fn list_debuggable_pages(&self, online_only: bool) -> Vec<DebugPage> {
        let mut seen = HashSet::new();
        self.list_pages(online_only)
            .into_iter()
            .filter(|page| {
                !matches!(
                    page.state,
                    DebugPageState::Candidate | DebugPageState::Stale | DebugPageState::Denied
                )
            })
            .filter(|page| {
                let key = page
                    .bridge_tab_id
                    .as_ref()
                    .map(|tab_id| format!("tab:{tab_id}"))
                    .unwrap_or_else(|| format!("url:{}", page.url));
                seen.insert(key)
            })
            .collect()
    }

    pub fn get_page(&self, page_id: &str) -> Option<DebugPage> {
        self.prune_inactive_pages();
        self.pages.read().get(page_id).cloned()
    }

    pub fn cdp_targets(&self, online_only: bool, host: &str) -> Vec<CdpTargetInfo> {
        self.list_debuggable_pages(online_only)
            .into_iter()
            .map(|page| cdp_target_info(&page, host))
            .collect()
    }

    pub fn open_session(&self, page_id: &str) -> Result<DebugSession, String> {
        // 先在 pages 作用域内完成状态更新并构造 session，释放 pages 锁后再写入 sessions，
        // 避免 pages.write() + sessions.write() 嵌套导致与 snapshot/session_page_id 死锁。
        let session = {
            let mut pages = self.pages.write();
            let page = pages
                .get_mut(page_id)
                .ok_or_else(|| "page not found".to_string())?;
            if page.state == DebugPageState::Candidate {
                return Err("page bridge has not connected".to_string());
            }
            page.state = DebugPageState::FallbackAttached;
            page.last_seen_at_ms = now_ms();

            DebugSession {
                session_id: format!("bdt_{}", uuid::Uuid::new_v4().simple()),
                page_id: page_id.to_string(),
                adapter: page.adapter.clone(),
                mode: page.mode.clone(),
                state: "attached".to_string(),
                opened_at_ms: now_ms(),
            }
        }; // pages 锁已释放
        self.sessions
            .write()
            .insert(session.session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn snapshot(&self, session_id: &str) -> Result<serde_json::Value, String> {
        // 先读取 session 中的 page_id 并释放 sessions 锁，再获取 pages 锁，
        // 避免与 open_session (pages → sessions) 形成 AB-BA 死锁。
        let page_id = {
            let sessions = self.sessions.read();
            sessions
                .get(session_id)
                .map(|s| s.page_id.clone())
                .ok_or_else(|| "session not found".to_string())?
        };
        let pages = self.pages.read();
        let page = pages
            .get(&page_id)
            .ok_or_else(|| "page not found".to_string())?;
        Ok(snapshot_for_page(page))
    }

    pub fn request_snapshot_refresh(
        &self,
        session_id: &str,
        scope: Option<String>,
    ) -> Result<(), String> {
        let page_id = self.session_page_id(session_id)?;
        let Some(sender) = self.bridge_senders.read().get(&page_id).cloned() else {
            return Err("page bridge websocket is not connected".to_string());
        };
        sender
            .try_send(BridgeServerMessage::SnapshotRequest { scope })
            .map_err(|_| "page bridge websocket is not connected".to_string())
    }

    pub async fn command(
        &self,
        session_id: &str,
        command: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match command {
            "dom.snapshot" | "console.messages" => {
                self.request_snapshot_refresh(session_id, Some(command.to_string()))?;
                self.snapshot(session_id)
            }
            "dom.highlight" => {
                let page_id = self.session_page_id(session_id)?;
                let node_id = params
                    .get("node_id")
                    .or_else(|| params.get("nodeId"))
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| "missing node_id".to_string())?;
                self.queue_overlay(&page_id, BridgeOverlayCommand::HighlightNode { node_id })?;
                Ok(serde_json::json!({"highlighted": true, "node_id": node_id}))
            }
            "dom.hide_highlight" => {
                let page_id = self.session_page_id(session_id)?;
                self.queue_overlay(&page_id, BridgeOverlayCommand::HideHighlight)?;
                Ok(serde_json::json!({"hidden": true}))
            }
            "dom.inspect" => {
                let page_id = self.session_page_id(session_id)?;
                self.queue_overlay(&page_id, BridgeOverlayCommand::StartInspect)?;
                Ok(serde_json::json!({"inspecting": true}))
            }
            "storage.set" => {
                let page_id = self.session_page_id(session_id)?;
                let area = params
                    .get("area")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing area".to_string())?;
                let key = params
                    .get("key")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing key".to_string())?;
                let value = params
                    .get("value")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing value".to_string())?;
                let expression = storage_set_expression(area, key, value)?;
                let eval_id = self.queue_eval(&page_id, expression)?;
                for _ in 0..40 {
                    if let Some(result) = self.take_eval_result(eval_id) {
                        result?;
                        return Ok(serde_json::json!({"updated": true, "area": area, "key": key}));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err("storage update timed out".to_string())
            }
            "storage.delete" | "storage.remove" => {
                let page_id = self.session_page_id(session_id)?;
                let area = params
                    .get("area")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing area".to_string())?;
                let key = params
                    .get("key")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing key".to_string())?;
                let expression = storage_delete_expression(area, key)?;
                let eval_id = self.queue_eval(&page_id, expression)?;
                for _ in 0..40 {
                    if let Some(result) = self.take_eval_result(eval_id) {
                        result?;
                        return Ok(serde_json::json!({"deleted": true, "area": area, "key": key}));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err("storage delete timed out".to_string())
            }
            "runtime.evaluate" => {
                let page_id = self.session_page_id(session_id)?;
                let expression = params
                    .get("expression")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let page = self
                    .get_page(&page_id)
                    .ok_or_else(|| "page not found".to_string())?;
                if !Self::expression_allowed_by_page(&page, &expression) {
                    self.record_evaluate_audit(&page, &expression, "main", None, true);
                    return Err("evaluate not in allowlist".to_string());
                }
                self.record_evaluate_audit(&page, &expression, "main", None, false);
                let eval_id = self.queue_eval(&page_id, expression)?;
                for _ in 0..40 {
                    if let Some(result) = self.take_eval_result(eval_id) {
                        return Ok(match result {
                            Ok(result) => result,
                            Err(exception) => serde_json::json!({
                                "type": "undefined",
                                "description": "undefined",
                                "exception": exception,
                                "exceptionDetails": {
                                    "text": exception,
                                    "exception": {
                                        "type": "string",
                                        "value": exception,
                                        "description": exception
                                    }
                                }
                            }),
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err("evaluation timed out".to_string())
            }
            other => Err(format!("unsupported command: {other}")),
        }
    }

    fn session_page_id(&self, session_id: &str) -> Result<String, String> {
        // 先读取 session 中的 page_id 并释放 sessions 锁，再获取 pages 锁，
        // 避免与 open_session (pages → sessions) 形成 AB-BA 死锁。
        let page_id = {
            let sessions = self.sessions.read();
            sessions
                .get(session_id)
                .map(|s| s.page_id.clone())
                .ok_or_else(|| "session not found".to_string())?
        };
        let pages = self.pages.read();
        let page = pages
            .get(&page_id)
            .ok_or_else(|| "page not found".to_string())?;
        Ok(page.page_id.clone())
    }

    fn migrate_page_references(&self, old_page_id: &str, new_page_id: &str) {
        if old_page_id == new_page_id {
            return;
        }
        for session in self.sessions.write().values_mut() {
            if session.page_id == old_page_id {
                session.page_id = new_page_id.to_string();
            }
        }
        move_pending_commands(&mut self.eval_pending.write(), old_page_id, new_page_id);
        move_pending_commands(&mut self.overlay_pending.write(), old_page_id, new_page_id);
        let mut bridge_senders = self.bridge_senders.write();
        if let Some(sender) = bridge_senders.remove(old_page_id) {
            bridge_senders
                .entry(new_page_id.to_string())
                .or_insert(sender);
        }
        let mut seen_seqs = self.bridge_seen_seqs.write();
        if let Some(seqs) = seen_seqs.remove(old_page_id) {
            seen_seqs.entry(new_page_id.to_string()).or_insert(seqs);
        }
    }

    fn prune_inactive_pages(&self) {
        let cutoff = now_ms().saturating_sub(Duration::from_secs(60).as_millis() as u64);
        let mut stale_page_ids = Vec::new();
        self.pages.write().retain(|page_id, page| {
            let keep = page.last_seen_at_ms >= cutoff
                || !matches!(
                    page.state,
                    DebugPageState::Candidate | DebugPageState::Stale
                );
            if !keep {
                stale_page_ids.push(page_id.clone());
            }
            keep
        });
        if !stale_page_ids.is_empty() {
            self.bridge_seen_seqs
                .write()
                .retain(|page_id, _| !stale_page_ids.contains(page_id));
        }
    }
}

pub fn bridge_command_queue_capacity() -> usize {
    BRIDGE_COMMAND_QUEUE_CAPACITY
}

pub fn session_live_queue_capacity() -> usize {
    SESSION_LIVE_QUEUE_CAPACITY
}

impl Default for BrowserDebugBroker {
    fn default() -> Self {
        Self::new()
    }
}

fn snapshot_for_page(page: &DebugPage) -> serde_json::Value {
    serde_json::json!({
        "page": page,
        "console": &page.console_messages,
        "dom_snapshot": &page.dom_snapshot,
        "dom_tree": &page.dom_tree,
        "network": &page.network_events,
        "storage": &page.storage_snapshot,
    })
}

fn trim_vec_front<T>(items: &mut Vec<T>, capacity: usize) {
    if capacity == 0 {
        items.clear();
        return;
    }
    if items.len() > capacity {
        let remove_count = items.len() - capacity;
        items.drain(0..remove_count);
    }
}

fn snapshot_from_bridge_payload(
    page: &DebugPage,
    payload: BridgeHelloPayload,
) -> serde_json::Value {
    let mut value = serde_json::json!({ "page": page });
    if let serde_json::Value::Object(ref mut object) = value {
        if let Some(scope) = payload.scope {
            object.insert("scope".to_string(), serde_json::Value::String(scope));
        }
        if payload.dom_snapshot.is_some() || payload.dom_tree.is_some() {
            object.insert(
                "dom_snapshot".to_string(),
                serde_json::to_value(payload.dom_snapshot).unwrap_or(serde_json::Value::Null),
            );
            object.insert(
                "dom_tree".to_string(),
                serde_json::to_value(payload.dom_tree).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(storage) = payload.storage {
            object.insert(
                "storage".to_string(),
                serde_json::to_value(storage).unwrap_or(serde_json::Value::Null),
            );
        }
        if !payload.console.is_empty() {
            object.insert(
                "console".to_string(),
                serde_json::to_value(payload.console).unwrap_or_else(|_| serde_json::json!([])),
            );
        }
        if !payload.network.is_empty() {
            let events: Vec<NetworkEvent> = payload
                .network
                .into_iter()
                .filter_map(network_event_from_input)
                .collect();
            object.insert(
                "network".to_string(),
                serde_json::to_value(events).unwrap_or_else(|_| serde_json::json!([])),
            );
        }
    }
    value
}

fn move_pending_commands<T>(
    pending: &mut HashMap<String, Vec<T>>,
    old_page_id: &str,
    new_page_id: &str,
) {
    let Some(mut old_commands) = pending.remove(old_page_id) else {
        return;
    };
    pending
        .entry(new_page_id.to_string())
        .or_default()
        .append(&mut old_commands);
}

fn storage_set_expression(area: &str, key: &str, value: &str) -> Result<String, String> {
    let key = serde_json::to_string(key).map_err(|err| err.to_string())?;
    let value = serde_json::to_string(value).map_err(|err| err.to_string())?;
    match area {
        "cookie" | "cookies" => Ok(format!(
            "document.cookie = {key} + '=' + encodeURIComponent({value}) + '; path=/'; window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); document.cookie"
        )),
        "local_storage" | "localStorage" => Ok(format!(
            "localStorage.setItem({key}, {value}); window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); localStorage.getItem({key})"
        )),
        "session_storage" | "sessionStorage" => Ok(format!(
            "sessionStorage.setItem({key}, {value}); window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); sessionStorage.getItem({key})"
        )),
        other => Err(format!("unsupported storage area: {other}")),
    }
}

fn storage_delete_expression(area: &str, key: &str) -> Result<String, String> {
    let key = serde_json::to_string(key).map_err(|err| err.to_string())?;
    match area {
        "cookie" | "cookies" => Ok(format!(
            "document.cookie = {key} + '=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT'; window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); document.cookie"
        )),
        "local_storage" | "localStorage" => Ok(format!(
            "localStorage.removeItem({key}); window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); null"
        )),
        "session_storage" | "sessionStorage" => Ok(format!(
            "sessionStorage.removeItem({key}); window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__ && window.__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__(); null"
        )),
        other => Err(format!("unsupported storage area: {other}")),
    }
}

fn rule_id(rule: &MatchedDevtoolsRule) -> String {
    match rule.line {
        Some(line) => format!("{}:{line}", rule.pattern),
        None => rule.pattern.clone(),
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = digest::digest(&digest::SHA256, value.as_bytes());
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn preview(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub fn cdp_target_info(page: &DebugPage, host: &str) -> CdpTargetInfo {
    let ws_path = format!("/_bifrost/api/devtools/cdp/{}", page.page_id);
    CdpTargetInfo {
        id: page.page_id.clone(),
        target_type: "page".to_string(),
        title: page
            .title
            .clone()
            .unwrap_or_else(|| "(untitled)".to_string()),
        url: page.url.clone(),
        web_socket_debugger_url: format!("ws://{host}{ws_path}"),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "devtools_tests.rs"]
mod tests;
