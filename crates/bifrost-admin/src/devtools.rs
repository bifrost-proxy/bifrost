use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use ring::digest;
use serde::{Deserialize, Serialize};

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
    fn page_bridge(mode: &DevtoolsMode) -> Self {
        Self {
            console_subscribe: "supported".to_string(),
            dom_snapshot: "supported".to_string(),
            runtime_evaluate: match mode {
                DevtoolsMode::Read => "requires_control".to_string(),
                DevtoolsMode::Control => "supported".to_string(),
            },
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEvent {
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub resource_type: String,
    pub at_ms: u64,
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
    pub network: Vec<NetworkEventInput>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeConsolePayload {
    pub token: String,
    pub level: Option<String>,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct BridgeClosePayload {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct NetworkEventInput {
    pub url: String,
    pub method: Option<String>,
    pub status: Option<u16>,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeNetworkPayload {
    pub token: String,
    pub event: NetworkEventInput,
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
    HideHighlight,
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
        let mut pages = self.pages.write();
        let expected_token = pages
            .get(page_id)
            .map(|page| page.bridge_token.clone())
            .ok_or_else(|| "page not found".to_string())?;
        if expected_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        let tab_id = payload.tab_id.filter(|value| !value.trim().is_empty());
        if let Some(tab_id) = &tab_id {
            pages.retain(|id, existing| {
                id == page_id
                    || existing.bridge_tab_id.as_deref() != Some(tab_id.as_str())
                    || !matches!(existing.adapter, DebugAdapterKind::PageBridge)
            });
        }
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        page.bridge_tab_id = tab_id;
        if let Some(title) = payload.title {
            if !title.trim().is_empty() {
                page.title = Some(title);
            }
        }
        if let Some(url) = payload.url {
            page.url = url;
        }
        if payload.user_agent.is_some() {
            page.user_agent = payload.user_agent;
        }
        if payload.dom_snapshot.is_some() || payload.dom_tree.is_some() {
            page.dom_snapshot = payload.dom_snapshot;
            page.dom_tree = payload.dom_tree;
            page.dom_updated_at_ms = now_ms();
        }
        if payload.storage.is_some() {
            page.storage_snapshot = payload.storage;
        }
        for event in payload.network {
            push_network_event(&mut page.network_events, event);
        }
        page.state = DebugPageState::Discoverable;
        page.last_seen_at_ms = now_ms();
        Ok(())
    }

    pub fn bridge_console(
        &self,
        page_id: &str,
        payload: BridgeConsolePayload,
    ) -> Result<(), String> {
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        page.console_messages.push(ConsoleMessage {
            level: payload.level.unwrap_or_else(|| "log".to_string()),
            text: payload.text,
            at_ms: now_ms(),
        });
        if page.console_messages.len() > 200 {
            let extra = page.console_messages.len() - 200;
            page.console_messages.drain(0..extra);
        }
        page.last_seen_at_ms = now_ms();
        Ok(())
    }

    pub fn bridge_close(&self, page_id: &str, payload: BridgeClosePayload) -> Result<(), String> {
        let mut pages = self.pages.write();
        let page = pages
            .get(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        pages.remove(page_id);
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
                expression,
            });
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
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.bridge_token != payload.token {
            return Err("bridge token mismatch".to_string());
        }
        push_network_event(&mut page.network_events, payload.event);
        page.last_seen_at_ms = now_ms();
        Ok(())
    }

    pub fn list_pages(&self, online_only: bool) -> Vec<DebugPage> {
        let cutoff = now_ms().saturating_sub(Duration::from_secs(60).as_millis() as u64);
        let mut pages: Vec<DebugPage> = self
            .pages
            .read()
            .values()
            .filter(|page| !online_only || page.last_seen_at_ms >= cutoff)
            .cloned()
            .collect();
        pages.sort_by_key(|page| std::cmp::Reverse(page.last_seen_at_ms));
        pages
    }

    pub fn get_page(&self, page_id: &str) -> Option<DebugPage> {
        self.pages.read().get(page_id).cloned()
    }

    pub fn cdp_targets(&self, online_only: bool, host: &str) -> Vec<CdpTargetInfo> {
        self.list_pages(online_only)
            .into_iter()
            .map(|page| cdp_target_info(&page, host))
            .collect()
    }

    pub fn open_session(&self, page_id: &str) -> Result<DebugSession, String> {
        let mut pages = self.pages.write();
        let page = pages
            .get_mut(page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.state == DebugPageState::Candidate {
            return Err("page bridge has not connected".to_string());
        }
        page.state = DebugPageState::FallbackAttached;
        page.last_seen_at_ms = now_ms();

        let session = DebugSession {
            session_id: format!("bdt_{}", uuid::Uuid::new_v4().simple()),
            page_id: page_id.to_string(),
            adapter: page.adapter.clone(),
            mode: page.mode.clone(),
            state: "attached".to_string(),
            opened_at_ms: now_ms(),
        };
        self.sessions
            .write()
            .insert(session.session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn snapshot(&self, session_id: &str) -> Result<serde_json::Value, String> {
        let sessions = self.sessions.read();
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "session not found".to_string())?;
        let pages = self.pages.read();
        let page = pages
            .get(&session.page_id)
            .ok_or_else(|| "page not found".to_string())?;
        Ok(serde_json::json!({
            "page": page,
            "console": page.console_messages,
            "dom_snapshot": page.dom_snapshot,
            "dom_tree": page.dom_tree,
            "network": page.network_events,
            "storage": page.storage_snapshot,
        }))
    }

    pub async fn command(
        &self,
        session_id: &str,
        command: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match command {
            "dom.snapshot" => self.snapshot(session_id),
            "console.messages" => self.snapshot(session_id),
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
            "storage.set" => {
                let page_id = self.session_control_page_id(session_id)?;
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
            "runtime.evaluate" => {
                let page_id = self.session_control_page_id(session_id)?;
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
                        return result;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err("evaluation timed out".to_string())
            }
            other => Err(format!("unsupported command: {other}")),
        }
    }

    fn session_page_id(&self, session_id: &str) -> Result<String, String> {
        let sessions = self.sessions.read();
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "session not found".to_string())?;
        let pages = self.pages.read();
        let page = pages
            .get(&session.page_id)
            .ok_or_else(|| "page not found".to_string())?;
        Ok(page.page_id.clone())
    }

    fn session_control_page_id(&self, session_id: &str) -> Result<String, String> {
        let sessions = self.sessions.read();
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "session not found".to_string())?;
        let pages = self.pages.read();
        let page = pages
            .get(&session.page_id)
            .ok_or_else(|| "page not found".to_string())?;
        if page.mode != DevtoolsMode::Control {
            return Err("requires_control".to_string());
        }
        Ok(page.page_id.clone())
    }
}

impl Default for BrowserDebugBroker {
    fn default() -> Self {
        Self::new()
    }
}

fn push_network_event(events: &mut Vec<NetworkEvent>, event: NetworkEventInput) {
    if event.url.trim().is_empty() {
        return;
    }
    events.push(NetworkEvent {
        url: event.url,
        method: event.method.unwrap_or_else(|| "GET".to_string()),
        status: event.status,
        resource_type: event.resource_type.unwrap_or_else(|| "Other".to_string()),
        at_ms: now_ms(),
    });
    if events.len() > 500 {
        let extra = events.len() - 500;
        events.drain(0..extra);
    }
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
mod tests {
    use super::*;

    fn input(url: &str) -> RegisterPageInput {
        RegisterPageInput {
            url: url.to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "101".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: Some(MatchedDevtoolsRule {
                pattern: "example.test".to_string(),
                raw: Some("example.test devtools://mode=read".to_string()),
                line: Some(1),
                evaluate_allowlist: Vec::new(),
            }),
        }
    }

    #[test]
    fn test_proxied_page_registry_records_document_request_with_devtools_rule() {
        let broker = BrowserDebugBroker::new();
        let (page_id, token) =
            broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

        let pages = broker.list_pages(true);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].page_id, page_id);
        assert_eq!(pages[0].state, DebugPageState::Candidate);
        assert_eq!(pages[0].traffic_ids, vec!["101".to_string()]);
        assert!(!token.is_empty());
    }

    #[test]
    fn test_page_bridge_hello_marks_page_discoverable() {
        let broker = BrowserDebugBroker::new();
        let (page_id, token) =
            broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

        broker
            .bridge_hello(
                &page_id,
                BridgeHelloPayload {
                    token,
                    tab_id: Some("tab-a".to_string()),
                    title: Some("Fixture".to_string()),
                    url: None,
                    user_agent: Some("Mobile Safari".to_string()),
                    dom_snapshot: Some("<html></html>".to_string()),
                    dom_tree: None,
                    storage: None,
                    network: Vec::new(),
                },
            )
            .expect("bridge hello");

        let page = broker.list_pages(true).remove(0);
        assert_eq!(page.title.as_deref(), Some("Fixture"));
        assert_eq!(page.state, DebugPageState::Discoverable);
        assert_eq!(page.user_agent.as_deref(), Some("Mobile Safari"));
    }

    #[test]
    fn test_page_bridge_rejects_token_replay_or_mismatch() {
        let broker = BrowserDebugBroker::new();
        let (page_id, _token) =
            broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

        let result = broker.bridge_hello(
            &page_id,
            BridgeHelloPayload {
                token: "wrong".to_string(),
                tab_id: Some("tab-a".to_string()),
                title: Some("Fixture".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                network: Vec::new(),
            },
        );

        assert!(result.is_err());
        assert_eq!(broker.list_pages(true)[0].state, DebugPageState::Candidate);
    }

    #[tokio::test]
    async fn test_runtime_evaluate_requires_control_scope() {
        let broker = BrowserDebugBroker::new();
        let (page_id, token) =
            broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
        broker
            .bridge_hello(
                &page_id,
                BridgeHelloPayload {
                    token,
                    tab_id: Some("tab-a".to_string()),
                    title: Some("Fixture".to_string()),
                    url: None,
                    user_agent: None,
                    dom_snapshot: None,
                    dom_tree: None,
                    storage: None,
                    network: Vec::new(),
                },
            )
            .expect("bridge hello");
        let session = broker.open_session(&page_id).expect("session");

        let result = broker
            .command(
                &session.session_id,
                "runtime.evaluate",
                serde_json::json!({"expression": "document.title"}),
            )
            .await;

        assert_eq!(result.unwrap_err(), "requires_control");
    }
}
