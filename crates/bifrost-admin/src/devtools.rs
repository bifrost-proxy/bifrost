use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::process::{Child, Command};
use thiserror::Error;

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
    pub system_chrome_frontend_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFrontendOpenResult {
    pub opened: bool,
    pub url: String,
    pub command: String,
}

pub const CHROME_DEVTOOLS_FRONTEND_VERSION: &str = "1.0.666106";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendInstallState {
    NotInstalled,
    Installed,
    Broken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeDevToolsFrontendStatus {
    pub state: FrontendInstallState,
    pub version: String,
    pub source: String,
    pub installed: bool,
    pub install_path: String,
    pub inspector_path: String,
    pub download_url: String,
    pub total_size_bytes: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegisterPageInput {
    pub url: String,
    pub origin: String,
    pub traffic_id: String,
    pub mode: DevtoolsMode,
    pub matched_rule: Option<MatchedDevtoolsRule>,
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

#[derive(Default)]
pub struct BrowserDebugBroker {
    pages: RwLock<HashMap<String, DebugPage>>,
    sessions: RwLock<HashMap<String, DebugSession>>,
    eval_next_id: AtomicU64,
    eval_pending: RwLock<HashMap<String, Vec<BridgeEvalCommand>>>,
    eval_results: RwLock<HashMap<u64, Result<serde_json::Value, String>>>,
    overlay_pending: RwLock<HashMap<String, Vec<BridgeOverlayCommand>>>,
}

pub type SharedBrowserDebugBroker = Arc<BrowserDebugBroker>;

impl BrowserDebugBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_page_candidate(&self, input: RegisterPageInput) -> (String, String) {
        let page_id = format!("pg_{}", uuid::Uuid::new_v4().simple());
        let bridge_token = format!("bdt_{}", uuid::Uuid::new_v4().simple());
        let capabilities = CapabilityMatrix::page_bridge(&input.mode);
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
            "runtime.evaluate" => {
                let page_id = self.session_control_page_id(session_id)?;
                let expression = params
                    .get("expression")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
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
        system_chrome_frontend_url: format!(
            "devtools://devtools/bundled/inspector.html?ws={host}{ws_path}"
        ),
    }
}

pub async fn open_system_chrome_frontend(
    page: &DebugPage,
    host: &str,
) -> Result<SystemFrontendOpenResult, String> {
    let url = cdp_target_info(page, host).system_chrome_frontend_url;
    let command = launch_system_browser(&url).await?;
    Ok(SystemFrontendOpenResult {
        opened: true,
        url,
        command,
    })
}

#[derive(Debug, Clone)]
struct BrowserCandidate {
    label: String,
    binary: String,
}

async fn launch_system_browser(url: &str) -> Result<String, String> {
    let candidates = resolve_system_browser_candidates();
    if candidates.is_empty() {
        return Err("Chrome, Edge, or Chromium was not found".to_string());
    }
    let requested_port = std::env::var("BIFROST_DEVTOOLS_CHROME_DEBUG_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let mut errors = Vec::new();
    for candidate in candidates {
        match launch_browser_candidate(&candidate, url, requested_port).await {
            Ok(()) => return Ok(candidate.label),
            Err(err) => errors.push(format!("{}: {err}", candidate.label)),
        }
    }
    Err(format!(
        "failed to launch a DevTools browser; tried {}",
        errors.join(" | ")
    ))
}

async fn launch_browser_candidate(
    candidate: &BrowserCandidate,
    url: &str,
    requested_port: u16,
) -> Result<(), String> {
    let profile_dir = system_browser_profile_dir(&candidate.label);
    fs::create_dir_all(&profile_dir)
        .map_err(|err| format!("failed to create browser profile dir: {err}"))?;

    let mut command = Command::new(&candidate.binary);
    command
        .arg(format!("--remote-debugging-port={requested_port}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("about:blank");
    for arg in split_env_args("BIFROST_DEVTOOLS_CHROME_ARGS") {
        command.arg(arg);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to launch {}: {err}", candidate.binary))?;

    let port = wait_for_chrome_debug_port(&profile_dir, requested_port, &mut child).await?;
    open_chrome_debug_target(port, url).await
}

fn system_browser_profile_dir(label: &str) -> PathBuf {
    if let Ok(profile) = std::env::var("BIFROST_DEVTOOLS_CHROME_PROFILE") {
        return PathBuf::from(profile);
    }
    let safe_label = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    bifrost_storage::data_dir()
        .join("admin/devtools-system-browser-profiles")
        .join(format!("{}-{}", safe_label, uuid::Uuid::new_v4().simple()))
}

fn resolve_system_browser_candidates() -> Vec<BrowserCandidate> {
    let mut candidates = Vec::new();
    if let Ok(binary) = std::env::var("BIFROST_DEVTOOLS_CHROME") {
        candidates.push(BrowserCandidate {
            label: browser_label_from_path(&binary, "env-browser"),
            binary,
        });
    }
    #[cfg(target_os = "macos")]
    {
        let discovered = [
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ];
        for candidate in discovered {
            if std::path::Path::new(candidate).exists() {
                let binary = candidate.to_string();
                candidates.push(BrowserCandidate {
                    label: browser_label_from_path(&binary, "mac-browser"),
                    binary,
                });
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        for binary in ["msedge", "chrome"] {
            candidates.push(BrowserCandidate {
                label: binary.to_string(),
                binary: binary.to_string(),
            });
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let discovered = [
            "microsoft-edge",
            "microsoft-edge-stable",
            "msedge",
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ];
        for candidate in discovered {
            if Command::new("sh")
                .arg("-c")
                .arg(format!("command -v {candidate}"))
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
            {
                candidates.push(BrowserCandidate {
                    label: candidate.to_string(),
                    binary: candidate.to_string(),
                });
            }
        }
    }
    dedupe_browser_candidates(candidates)
}

fn browser_label_from_path(path: &str, fallback: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn dedupe_browser_candidates(candidates: Vec<BrowserCandidate>) -> Vec<BrowserCandidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.binary.clone()))
        .collect()
}

async fn wait_for_chrome_debug_port(
    profile_dir: &Path,
    requested_port: u16,
    child: &mut Child,
) -> Result<u16, String> {
    let active_port_file = profile_dir.join("DevToolsActivePort");
    for _ in 0..80 {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "browser exited before remote debugging was ready: {status}"
            ));
        }
        let port = if requested_port != 0 {
            requested_port
        } else {
            match tokio::fs::read_to_string(&active_port_file).await {
                Ok(content) => content
                    .lines()
                    .next()
                    .and_then(|line| line.trim().parse::<u16>().ok())
                    .unwrap_or(0),
                Err(_) => 0,
            }
        };
        if port != 0
            && reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
                .await
                .map(|response| response.status().is_success())
                .unwrap_or(false)
        {
            return Ok(port);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Chrome remote debugging endpoint did not become ready".to_string())
}

async fn open_chrome_debug_target(port: u16, url: &str) -> Result<(), String> {
    let encoded = urlencoding::encode(url);
    let response = reqwest::Client::new()
        .put(format!("http://127.0.0.1:{port}/json/new?{encoded}"))
        .send()
        .await
        .map_err(|err| format!("failed to ask Chrome to open DevTools URL: {err}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Chrome rejected DevTools URL open request with HTTP {}",
            response.status()
        ))
    }
}

fn split_env_args(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split_whitespace()
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn frontend_status() -> ChromeDevToolsFrontendStatus {
    frontend_status_for_root(&frontend_package_root())
}

pub async fn install_frontend() -> Result<ChromeDevToolsFrontendStatus, FrontendInstallError> {
    let version = CHROME_DEVTOOLS_FRONTEND_VERSION.to_string();
    let download_url = frontend_download_url();
    let data_dir = bifrost_storage::data_dir();
    let cache_root = frontend_cache_root_in(&data_dir);
    let package_root = frontend_package_root_in(&data_dir);
    tokio::fs::create_dir_all(&cache_root).await?;

    let response = reqwest::get(&download_url).await?;
    if !response.status().is_success() {
        return Err(FrontendInstallError::HttpStatus(response.status().as_u16()));
    }
    let bytes = response.bytes().await?;
    let archive_path = cache_root.join(format!("chrome-devtools-frontend-{version}.tgz"));
    tokio::fs::write(&archive_path, &bytes).await?;

    let unpack_root = package_root.clone();
    tokio::task::spawn_blocking(move || unpack_frontend_archive(&archive_path, &unpack_root))
        .await
        .map_err(|err| FrontendInstallError::Join(err.to_string()))??;

    Ok(frontend_status_for_root(&package_root))
}

pub fn frontend_file_path(request_path: &str) -> Result<PathBuf, FrontendFileError> {
    let root = frontend_package_root();
    let relative = request_path
        .strip_prefix("/api/devtools/frontend/")
        .unwrap_or(request_path)
        .trim_start_matches('/');
    let relative = if relative.is_empty() {
        "inspector.html"
    } else {
        relative
    };
    if relative
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(FrontendFileError::InvalidPath);
    }

    let mut normalized = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            _ => return Err(FrontendFileError::InvalidPath),
        }
    }
    let path = root.join(normalized);
    if !path.starts_with(&root) {
        return Err(FrontendFileError::InvalidPath);
    }
    Ok(path)
}

pub fn frontend_package_root() -> PathBuf {
    frontend_package_root_in(&bifrost_storage::data_dir())
}

fn frontend_status_for_root(package_root: &Path) -> ChromeDevToolsFrontendStatus {
    let inspector = package_root.join("inspector.html");
    let installed = inspector.is_file();
    let (state, reason) = if installed {
        (FrontendInstallState::Installed, None)
    } else if package_root.exists() {
        (
            FrontendInstallState::Broken,
            Some("cached directory exists but inspector.html is missing".to_string()),
        )
    } else {
        (FrontendInstallState::NotInstalled, None)
    };

    ChromeDevToolsFrontendStatus {
        state,
        version: CHROME_DEVTOOLS_FRONTEND_VERSION.to_string(),
        source: "npm_on_demand_cache".to_string(),
        installed,
        install_path: package_root.display().to_string(),
        inspector_path: "/_bifrost/api/devtools/frontend/inspector.html".to_string(),
        download_url: frontend_download_url(),
        total_size_bytes: installed.then(|| dir_size(package_root).unwrap_or(0)),
        reason,
    }
}

fn frontend_cache_root_in(data_dir: &Path) -> PathBuf {
    data_dir.join("admin").join("devtools-frontend")
}

fn frontend_package_root_in(data_dir: &Path) -> PathBuf {
    frontend_cache_root_in(data_dir).join(format!(
        "chrome-devtools-frontend-{}",
        CHROME_DEVTOOLS_FRONTEND_VERSION
    ))
}

fn frontend_download_url() -> String {
    std::env::var("BIFROST_DEVTOOLS_FRONTEND_TARBALL_URL").unwrap_or_else(|_| {
        format!("https://registry.npmjs.org/chrome-devtools-frontend/-/chrome-devtools-frontend-{CHROME_DEVTOOLS_FRONTEND_VERSION}.tgz")
    })
}

fn unpack_frontend_archive(
    archive_path: &Path,
    package_root: &Path,
) -> Result<(), FrontendInstallError> {
    let tmp_root = package_root.with_extension("tmp");
    if tmp_root.exists() {
        fs::remove_dir_all(&tmp_root)?;
    }
    if package_root.exists() {
        fs::remove_dir_all(package_root)?;
    }
    fs::create_dir_all(&tmp_root)?;

    let file = fs::File::open(archive_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(&tmp_root)?;

    let npm_package_root = tmp_root.join("package");
    let compiled_frontend_root = npm_package_root.join("front_end");
    if !compiled_frontend_root.join("inspector.html").is_file() {
        return Err(FrontendInstallError::InvalidArchive(
            "chrome-devtools-frontend package did not contain inspector.html".to_string(),
        ));
    }
    fs::rename(&compiled_frontend_root, package_root)?;
    let _ = fs::remove_dir_all(&tmp_root);
    Ok(())
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

#[derive(Debug, Error)]
pub enum FrontendInstallError {
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("download returned HTTP {0}")]
    HttpStatus(u16),
    #[error("unpack task failed: {0}")]
    Join(String),
    #[error("invalid archive: {0}")]
    InvalidArchive(String),
}

#[derive(Debug, Error)]
pub enum FrontendFileError {
    #[error("invalid frontend asset path")]
    InvalidPath,
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
