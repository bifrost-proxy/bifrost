//! MCP (Model Context Protocol) server management.
//!
//! Supports stdio and Streamable HTTP transports. Manages lifecycle:
//! - Start server processes (stdio) or create HTTP session.
//! - Initialize (capabilities exchange).
//! - List tools.
//! - Call tools (per-call timeout, id-correlated responses).
//! - Shutdown (bounded, concurrent, best-effort).
//!
//! Design notes (2026/05 hardening):
//! - JSON-RPC is full-duplex: each connection runs a **reader task** that
//!   demultiplexes inbound lines by `id` into per-call `oneshot` channels, and
//!   routes `id`-less notifications to a dedicated channel. Without this the
//!   caller could read a `notifications/tools/list_changed` frame and
//!   misinterpret it as a response, desynchronising all subsequent requests.
//! - Both transports enforce **per-request timeouts** derived from
//!   `McpServerConfig::tool_timeout_sec` / `startup_timeout_sec`. Stdio
//!   previously had no timeout and a hung server would wedge the whole turn.
//! - Stdio **stderr is streamed to `warn!`** for observability instead of
//!   being swallowed.
//! - Streamable HTTP implements the spec-compliant dual-channel pattern: POSTs
//!   carry client→server frames; a long-lived `GET` SSE subscription carries
//!   server→client frames (responses and notifications).
//! - `McpManager::new` starts servers **concurrently** with bounded
//!   parallelism; a slow server no longer blocks the others.
//! - `Drop` kills any surviving stdio children so process leaks cannot
//!   outlive an aborted agent.
//! - Tool names are validated against the OpenAI function-calling constraint
//!   (`^[a-zA-Z0-9_-]{1,64}$`); duplicate registrations are rejected instead
//!   of silently overwriting.
//! - Input schemas are sanity-checked; invalid schemas degrade to
//!   `{"type":"object"}` so a misbehaving server cannot brick a whole turn.

pub mod approval;
pub mod elicitation;
pub mod oauth;
pub mod resources;
pub mod session;
pub mod types;

use crate::config::McpServerConfig;
use crate::types::{ToolDefinition, ToolResult};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use self::resources::{
    is_resource_tool_name, LIST_MCP_RESOURCES_TOOL_NAME, LIST_MCP_RESOURCE_TEMPLATES_TOOL_NAME,
    READ_MCP_RESOURCE_TOOL_NAME,
};

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
    #[serde(default)]
    #[allow(dead_code)]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// MCP tool schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct McpToolInfo {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    input_schema: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum bytes for a single JSON-RPC line from a stdio MCP server.
/// Hard-capped so a misbehaving server can not OOM the agent.
const MAX_JSONRPC_LINE_BYTES: u64 = 10 * 1024 * 1024;

/// Default per-request / tool-call timeout when config omits one.
const DEFAULT_TOOL_TIMEOUT_SEC: u64 = 600;

/// Default startup timeout (initialize + tools/list).
const DEFAULT_STARTUP_TIMEOUT_SEC: u64 = 600;

/// Graceful shutdown timeout per server.
const SHUTDOWN_TIMEOUT_MS: u64 = 500;

/// Max concurrent server startups.
const STARTUP_CONCURRENCY: usize = 8;

/// Codex threshold for switching MCP tools to deferred loading.
///
/// Codex uses `>= 100`, not `> 100`.
pub(crate) const DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD: usize = 100;

/// Tool-name regex per OpenAI function-calling constraint:
/// `^[a-zA-Z0-9_-]{1,64}$`.
fn is_valid_tool_name(name: &str) -> bool {
    let len = name.len();
    if !(1..=64).contains(&len) {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Shorten + sanitise a server name so `mcp_{server}_{tool}` stays under
/// 64 chars. Non-[A-Za-z0-9_-] chars collapse to `_`; if still too long we
/// keep the first 8 chars and append a stable 8-char hash of the full name.
fn sanitise_server_prefix(server: &str, tool_len: usize) -> String {
    let sanitised: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // 4 chars for the "mcp_" prefix + underscore between server and tool.
    let budget = 64usize.saturating_sub(4 + 1 + tool_len);
    if sanitised.len() <= budget {
        return sanitised;
    }
    // Too long: stable 8-char fnv-1a-ish hash (xor upper/lower halves, keep
    // exactly 8 hex chars).
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in server.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let hash32 = ((hash >> 32) as u32) ^ (hash as u32);
    let head_len = budget.saturating_sub(9).min(sanitised.len());
    let head = &sanitised[..head_len];
    format!("{head}_{hash32:08x}")
}

// ---------------------------------------------------------------------------
// Transport abstraction
// ---------------------------------------------------------------------------

/// Outbound writer: each transport exposes a way to send a serialized frame.
#[async_trait::async_trait]
trait FrameSink: Send {
    async fn send_frame(&mut self, bytes: Vec<u8>) -> Result<(), String>;
}

/// Stdio writer.
struct StdioSink {
    server_name: String,
    stdin: ChildStdin,
}

#[async_trait::async_trait]
impl FrameSink for StdioSink {
    async fn send_frame(&mut self, mut bytes: Vec<u8>) -> Result<(), String> {
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|e| format!("write stdio to '{}': {e}", self.server_name))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("flush stdio to '{}': {e}", self.server_name))?;
        Ok(())
    }
}

/// HTTP sink — each send is a POST; server→client frames arrive via the
/// separate SSE reader task.
///
/// NOTE: per the MCP "Streamable HTTP" spec, a server may reply to a POST with
/// an inline SSE stream carrying the correlated response. We currently consume
/// that stream with `Response::text()` (buffered), which is correct but serial:
/// a slow server that streams its reply will hold the sink `Mutex` until the
/// stream completes. Most servers return 202 Accepted and deliver the response
/// on the long-lived GET channel (already handled), so this is a latency
/// concern only for servers that exclusively use inline POST-SSE. Revisit if
/// such a server is onboarded.
struct HttpSink {
    server_name: String,
    client: reqwest::Client,
    url: String,
    auth_header: Option<String>,
    session_id: Arc<Mutex<Option<String>>>,
    /// Channel back to the demux loop so POST-returned frames can be routed
    /// alongside SSE-delivered frames.
    frame_tx: mpsc::UnboundedSender<String>,
}

#[async_trait::async_trait]
impl FrameSink for HttpSink {
    async fn send_frame(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let mut req = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        let sid_snapshot = self.session_id.lock().await.clone();
        if let Some(sid) = sid_snapshot {
            req = req.header("Mcp-Session-Id", sid);
        }
        if let Some(ref auth) = self.auth_header {
            req = req.header("Authorization", auth.clone());
        }

        let resp = req
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("HTTP POST to '{}': {e}", self.server_name))?;

        if let Some(sid_value) = resp.headers().get("mcp-session-id") {
            if let Ok(sid_str) = sid_value.to_str() {
                *self.session_id.lock().await = Some(sid_str.to_string());
            }
        }

        let status = resp.status();
        // 202 Accepted: response/notification will arrive on the SSE channel.
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok(());
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "MCP '{}' HTTP error {}: {}",
                self.server_name, status, body_text
            ));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("read HTTP body from '{}': {e}", self.server_name))?;

        if body_text.is_empty() {
            return Ok(());
        }

        if content_type.contains("text/event-stream") {
            for frame in split_sse_events(&body_text) {
                let _ = self.frame_tx.send(frame);
            }
        } else {
            let _ = self.frame_tx.send(body_text);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// Pending request → response channel.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>;

/// An active MCP server connection.
pub(crate) struct McpConnection {
    server_name: String,
    sink: Mutex<Box<dyn FrameSink>>,
    pending: PendingMap,
    next_id: std::sync::atomic::AtomicU64,
    tools: Vec<ToolDefinition>,
    per_request_timeout: Duration,
    /// Joinable lifecycle tasks. We `abort()` them on shutdown so neither the
    /// reader nor the stderr pump outlives the connection.
    tasks: Vec<JoinHandle<()>>,
    /// Only present for stdio transports — retained so we can kill the child
    /// on shutdown / drop even if the server ignores `shutdown`.
    child: Option<Child>,
}

impl McpConnection {
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.send_request_with_timeout(method, params, self.per_request_timeout)
            .await
    }

    async fn send_request_with_timeout(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        dur: Duration,
    ) -> Result<serde_json::Value, String> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let bytes = serde_json::to_vec(&request).map_err(|e| format!("serialize request: {e}"))?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let send_result = self.sink.lock().await.send_frame(bytes).await;
        if let Err(e) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        match timeout(dur, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_canceled)) => Err(format!(
                "MCP '{}' reader dropped before response",
                self.server_name
            )),
            Err(_elapsed) => {
                self.pending.lock().await.remove(&id);
                Err(format!(
                    "MCP '{}' request '{}' timed out after {}s",
                    self.server_name,
                    method,
                    dur.as_secs()
                ))
            }
        }
    }

    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };
        let bytes =
            serde_json::to_vec(&notif).map_err(|e| format!("serialize notification: {e}"))?;
        self.sink.lock().await.send_frame(bytes).await
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

/// Route a single inbound JSON frame: if it carries an `id` that matches a
/// pending request, deliver it; otherwise log it as a notification.
async fn route_inbound_frame(server_name: &str, pending: &PendingMap, line: &str) {
    // Try to parse; tolerate frames that do not match our struct shape by
    // logging them at debug rather than breaking the demux loop.
    let parsed: JsonRpcResponse = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            debug!(server = %server_name, error = %e, line = %line, "unparseable MCP frame");
            return;
        }
    };

    if let Some(method) = parsed.method.as_deref() {
        debug!(
            server = %server_name,
            method = %method,
            "MCP notification received"
        );
        return;
    }

    let Some(id) = parsed.id else {
        debug!(
            server = %server_name,
            "MCP frame without id or method (ignored)"
        );
        return;
    };

    let tx = { pending.lock().await.remove(&id) };
    let Some(tx) = tx else {
        debug!(
            server = %server_name,
            id,
            "MCP response for unknown id (dropped)"
        );
        return;
    };

    if let Some(err) = parsed.error {
        let _ = tx.send(Err(format!("MCP '{server_name}' error: {}", err.message)));
        return;
    }
    let _ = tx.send(Ok(parsed.result.unwrap_or(serde_json::Value::Null)));
}

/// Split a raw SSE body into individual event payloads (the `data:` lines of
/// each event, concatenated — `event:` / `id:` lines are ignored). Events are
/// delimited by a blank line per the SSE grammar.
fn split_sse_events(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in body.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            buf.push_str(data.trim_start());
        }
        // event:, id:, retry: lines are intentionally ignored.
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

// ---------------------------------------------------------------------------
// Reader tasks
// ---------------------------------------------------------------------------

fn spawn_stdio_reader(
    server_name: String,
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).take(MAX_JSONRPC_LINE_BYTES);
        let mut buf = Vec::with_capacity(8 * 1024);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => {
                    debug!(server = %server_name, "stdio reader: EOF");
                    break;
                }
                Ok(n) => {
                    if n as u64 >= MAX_JSONRPC_LINE_BYTES && !buf.ends_with(b"\n") {
                        warn!(
                            server = %server_name,
                            limit = MAX_JSONRPC_LINE_BYTES,
                            "MCP frame exceeded size limit; discarding and resynchronising to next newline"
                        );
                        // Drain the rest of the oversized frame so we don't
                        // mistake its tail for a fresh frame. Allow up to
                        // another MAX_JSONRPC_LINE_BYTES of junk before
                        // giving up.
                        reader.set_limit(MAX_JSONRPC_LINE_BYTES);
                        let mut junk = Vec::with_capacity(8 * 1024);
                        match reader.read_until(b'\n', &mut junk).await {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(e) => {
                                warn!(
                                    server = %server_name,
                                    error = %e,
                                    "error while resynchronising after oversized frame"
                                );
                                break;
                            }
                        }
                        reader.set_limit(MAX_JSONRPC_LINE_BYTES);
                        continue;
                    }
                    reader.set_limit(MAX_JSONRPC_LINE_BYTES);
                    let line = match std::str::from_utf8(&buf) {
                        Ok(s) => s.trim_end_matches(['\r', '\n']).to_string(),
                        Err(_) => {
                            debug!(server = %server_name, "non-utf8 frame dropped");
                            continue;
                        }
                    };
                    if line.is_empty() {
                        continue;
                    }
                    route_inbound_frame(&server_name, &pending, &line).await;
                }
                Err(e) => {
                    warn!(server = %server_name, error = %e, "stdio reader error");
                    break;
                }
            }
        }
        // Fail any still-pending requests on EOF.
        let leftover: Vec<_> = pending.lock().await.drain().collect();
        for (_, tx) in leftover {
            let _ = tx.send(Err(format!("MCP '{server_name}' closed connection")));
        }
    })
}

fn spawn_stderr_pump(server_name: String, stderr: tokio::process::ChildStderr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if !line.is_empty() {
                warn!(server = %server_name, line = %line, "mcp stderr");
            }
        }
    })
}

fn spawn_http_demux(
    server_name: String,
    mut rx: mpsc::UnboundedReceiver<String>,
    pending: PendingMap,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            route_inbound_frame(&server_name, &pending, &frame).await;
        }
    })
}

fn spawn_http_sse_reader(
    server_name: String,
    client: reqwest::Client,
    url: String,
    auth_header: Option<String>,
    session_id: Arc<Mutex<Option<String>>>,
    frame_tx: mpsc::UnboundedSender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut req = client.get(&url).header("Accept", "text/event-stream");
            let sid_snapshot = session_id.lock().await.clone();
            if let Some(sid) = sid_snapshot {
                req = req.header("Mcp-Session-Id", sid);
            }
            if let Some(ref auth) = auth_header {
                req = req.header("Authorization", auth.clone());
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    debug!(server = %server_name, error = %e, "SSE subscribe failed");
                    // Server may not support GET SSE; back off and retry.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            if !resp.status().is_success() {
                debug!(
                    server = %server_name,
                    status = %resp.status(),
                    "SSE subscribe non-success (likely no server-push support)"
                );
                // If the server explicitly rejects GET, stop retrying.
                if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
                    || resp.status() == reqwest::StatusCode::NOT_FOUND
                {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }

            // Stream line-by-line.
            let mut current = String::new();
            let mut stream = resp.bytes_stream();
            use futures::StreamExt;
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else { break };
                let Ok(s) = std::str::from_utf8(&chunk) else {
                    continue;
                };
                current.push_str(s);
                while let Some(idx) = current.find("\n\n").or_else(|| current.find("\r\n\r\n")) {
                    let (event_block, _rest) = current.split_at(idx);
                    let events = split_sse_events(&format!("{event_block}\n"));
                    for ev in events {
                        if frame_tx.send(ev).is_err() {
                            return;
                        }
                    }
                    let skip = if current[idx..].starts_with("\r\n\r\n") {
                        4
                    } else {
                        2
                    };
                    current = current[idx + skip..].to_string();
                }
            }
            // Stream closed; retry after brief pause.
            debug!(server = %server_name, "SSE stream closed; reconnecting");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
}

// ---------------------------------------------------------------------------
// McpManager
// ---------------------------------------------------------------------------

/// Manages MCP server lifecycle and tool routing.
pub struct McpManager {
    connections: HashMap<String, Arc<McpConnection>>,
    /// Map from fully-qualified tool name → server name.
    tool_routing: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct McpToolExposure {
    pub direct_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerAvailabilityStatus {
    Available,
    Unavailable,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerAvailability {
    pub name: String,
    pub enabled: bool,
    pub status: McpServerAvailabilityStatus,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn check_server_availability(
    configs: &HashMap<String, McpServerConfig>,
) -> Vec<McpServerAvailability> {
    use futures::stream::{self, StreamExt};

    let mut statuses = configs
        .iter()
        .filter(|(_, cfg)| !cfg.enabled)
        .map(|(name, _)| McpServerAvailability {
            name: name.clone(),
            enabled: false,
            status: McpServerAvailabilityStatus::Disabled,
            tool_count: 0,
            error: None,
        })
        .collect::<Vec<_>>();

    let eligible = configs
        .iter()
        .filter(|(_, cfg)| cfg.enabled)
        .map(|(name, cfg)| (name.clone(), cfg.clone()))
        .collect::<Vec<_>>();

    let outcomes = stream::iter(eligible.into_iter().map(|(name, cfg)| async move {
        let outcome = start_one_server(&name, &cfg).await;
        (name, outcome)
    }))
    .buffer_unordered(STARTUP_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    statuses.extend(outcomes.into_iter().map(|(name, outcome)| match outcome {
        Ok(conn) => McpServerAvailability {
            name,
            enabled: true,
            status: McpServerAvailabilityStatus::Available,
            tool_count: conn.tools.len(),
            error: None,
        },
        Err(error) => McpServerAvailability {
            name,
            enabled: true,
            status: McpServerAvailabilityStatus::Unavailable,
            tool_count: 0,
            error: Some(error),
        },
    }));

    statuses.sort_by(|a, b| a.name.cmp(&b.name));
    statuses
}

impl McpManager {
    /// Create a new McpManager. Enabled servers start **concurrently** with
    /// bounded parallelism; individual failures are logged and skipped without
    /// blocking peers.
    pub async fn new(configs: &HashMap<String, McpServerConfig>) -> Self {
        let mut manager = Self {
            connections: HashMap::new(),
            tool_routing: HashMap::new(),
        };

        let eligible: Vec<(String, McpServerConfig)> = configs
            .iter()
            .filter(|(name, cfg)| {
                if !cfg.enabled {
                    debug!(server = %name, "skipping disabled MCP server");
                    return false;
                }
                true
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        use futures::stream::{self, StreamExt};
        let outcomes: Vec<(String, Result<Arc<McpConnection>, String>)> =
            stream::iter(eligible.into_iter().map(|(name, cfg)| async move {
                let outcome = start_one_server(&name, &cfg).await;
                (name, outcome)
            }))
            .buffer_unordered(STARTUP_CONCURRENCY)
            .collect()
            .await;

        for (name, outcome) in outcomes {
            match outcome {
                Ok(conn) => {
                    if manager.connections.contains_key(&name) {
                        warn!(server = %name, "duplicate MCP server name; ignoring later registration");
                        continue;
                    }
                    for tool in &conn.tools {
                        let tool_name = tool.name().to_string();
                        if manager
                            .tool_routing
                            .insert(tool_name.clone(), name.clone())
                            .is_some()
                        {
                            warn!(
                                tool = %tool_name,
                                server = %name,
                                "tool name collision across MCP servers; later wins"
                            );
                        }
                    }
                    info!(
                        server = %name,
                        tool_count = conn.tools.len(),
                        "MCP server initialized"
                    );
                    manager.connections.insert(name, conn);
                }
                Err(e) => {
                    error!(server = %name, error = %e, "failed to start MCP server");
                }
            }
        }

        manager
    }

    /// List tools aggregated across all connected servers.
    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        self.connections
            .values()
            .flat_map(|conn| conn.tools.iter().cloned())
            .collect()
    }

    /// Return direct/deferred MCP exposure.
    ///
    /// Once the MCP tool count reaches the configured threshold all MCP server
    /// tools are deferred and made discoverable through `tool_search`.
    /// MCP resource tools stay direct as part of the core tool surface.
    pub fn tool_exposure(&self) -> McpToolExposure {
        mcp_tool_exposure_with_resource_tools(self.list_tools(), !self.connections.is_empty())
    }

    /// Call a tool on the routed MCP server.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<ToolResult, String> {
        if is_resource_tool_name(tool_name) {
            return self.call_resource_tool(tool_name, arguments).await;
        }

        let server_name = self
            .tool_routing
            .get(tool_name)
            .ok_or_else(|| format!("no MCP server for tool '{tool_name}'"))?
            .clone();
        let conn = self
            .connections
            .get(&server_name)
            .ok_or_else(|| format!("MCP server '{server_name}' not connected"))?
            .clone();

        // Strip our prefix to recover the server-side tool name. We register
        // tools as `mcp_{sanprefix}__{orig}`, so the original name is
        // everything after the last `__`. Fall back to the raw tool name if
        // the delimiter is absent (shouldn't happen for names produced by
        // this module, but keeps the call defensive).
        let original_name = tool_name
            .rsplit_once("__")
            .map(|(_, orig)| orig.to_string())
            .unwrap_or_else(|| tool_name.to_string());

        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let params = serde_json::json!({
            "name": original_name,
            "arguments": args,
        });

        let started = std::time::Instant::now();
        debug!(server = %server_name, tool = %original_name, "calling MCP tool");

        let result = match conn.send_request("tools/call", Some(params)).await {
            Ok(v) => v,
            Err(e) => {
                let elapsed_ms = started.elapsed().as_millis();
                warn!(
                    server = %server_name,
                    tool = %original_name,
                    elapsed_ms,
                    error = %e,
                    "MCP tool call failed"
                );
                return Err(e);
            }
        };

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| result.to_string());

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        info!(
            server = %server_name,
            tool = %original_name,
            elapsed_ms = started.elapsed().as_millis() as u64,
            success = !is_error,
            "MCP tool call completed"
        );

        Ok(ToolResult {
            success: !is_error,
            output: content,
            runtime_events: Vec::new(),
        })
    }

    pub fn is_mcp_tool(&self, tool_name: &str) -> bool {
        self.tool_routing.contains_key(tool_name)
            || (!self.connections.is_empty() && is_resource_tool_name(tool_name))
    }

    async fn call_resource_tool(
        &self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<ToolResult, String> {
        let args = parse_mcp_resource_arguments(arguments)?;
        let output = match tool_name {
            LIST_MCP_RESOURCES_TOOL_NAME => {
                self.list_mcp_resources_for_tool(args.server, args.cursor)
                    .await?
            }
            LIST_MCP_RESOURCE_TEMPLATES_TOOL_NAME => {
                self.list_mcp_resource_templates_for_tool(args.server, args.cursor)
                    .await?
            }
            READ_MCP_RESOURCE_TOOL_NAME => {
                let server = args
                    .server
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "read_mcp_resource requires non-empty `server`".to_string())?;
                let uri = args
                    .uri
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "read_mcp_resource requires non-empty `uri`".to_string())?;
                self.read_mcp_resource_for_tool(server, uri).await?
            }
            _ => return Err(format!("unknown MCP resource tool: {tool_name}")),
        };

        Ok(ToolResult {
            success: true,
            output,
            runtime_events: Vec::new(),
        })
    }

    async fn list_mcp_resources_for_tool(
        &self,
        server: Option<String>,
        cursor: Option<String>,
    ) -> Result<String, String> {
        if cursor.is_some() && server.as_ref().is_none_or(|name| name.trim().is_empty()) {
            return Err("cursor can only be used when a server is specified".to_string());
        }
        if let Some(server_name) = normalize_optional_tool_arg(server) {
            let conn = self
                .connections
                .get(&server_name)
                .ok_or_else(|| format!("MCP server '{server_name}' not connected"))?;
            let result = conn
                .send_request("resources/list", cursor_params(cursor))
                .await?;
            return Ok(list_resource_payload(&server_name, result, "resources"));
        }

        let mut resources = Vec::new();
        let mut errors = Vec::new();
        let mut server_names = self.connections.keys().cloned().collect::<Vec<_>>();
        server_names.sort();
        for server_name in server_names {
            let Some(conn) = self.connections.get(&server_name) else {
                continue;
            };
            match conn.send_request("resources/list", None).await {
                Ok(result) => {
                    resources.extend(tagged_array_entries(&server_name, result, "resources"));
                }
                Err(error) => errors.push(serde_json::json!({
                    "server": server_name,
                    "error": error,
                })),
            }
        }
        Ok(serde_json::json!({
            "resources": resources,
            "errors": errors,
        })
        .to_string())
    }

    async fn list_mcp_resource_templates_for_tool(
        &self,
        server: Option<String>,
        cursor: Option<String>,
    ) -> Result<String, String> {
        if cursor.is_some() && server.as_ref().is_none_or(|name| name.trim().is_empty()) {
            return Err("cursor can only be used when a server is specified".to_string());
        }
        if let Some(server_name) = normalize_optional_tool_arg(server) {
            let conn = self
                .connections
                .get(&server_name)
                .ok_or_else(|| format!("MCP server '{server_name}' not connected"))?;
            let result = conn
                .send_request("resources/templates/list", cursor_params(cursor))
                .await?;
            return Ok(list_resource_payload(
                &server_name,
                result,
                "resourceTemplates",
            ));
        }

        let mut resource_templates = Vec::new();
        let mut errors = Vec::new();
        let mut server_names = self.connections.keys().cloned().collect::<Vec<_>>();
        server_names.sort();
        for server_name in server_names {
            let Some(conn) = self.connections.get(&server_name) else {
                continue;
            };
            match conn.send_request("resources/templates/list", None).await {
                Ok(result) => {
                    resource_templates.extend(tagged_array_entries(
                        &server_name,
                        result,
                        "resourceTemplates",
                    ));
                }
                Err(error) => errors.push(serde_json::json!({
                    "server": server_name,
                    "error": error,
                })),
            }
        }
        Ok(serde_json::json!({
            "resourceTemplates": resource_templates,
            "errors": errors,
        })
        .to_string())
    }

    async fn read_mcp_resource_for_tool(
        &self,
        server_name: String,
        uri: String,
    ) -> Result<String, String> {
        let conn = self
            .connections
            .get(&server_name)
            .ok_or_else(|| format!("MCP server '{server_name}' not connected"))?;
        let result = conn
            .send_request(
                "resources/read",
                Some(serde_json::json!({
                    "uri": uri,
                })),
            )
            .await?;
        let mut payload = serde_json::Map::new();
        payload.insert("server".to_string(), serde_json::Value::String(server_name));
        payload.insert("uri".to_string(), serde_json::Value::String(uri));
        if let serde_json::Value::Object(map) = result {
            for (key, value) in map {
                payload.insert(key, value);
            }
        }
        Ok(serde_json::Value::Object(payload).to_string())
    }

    /// Shutdown all MCP servers. Each server gets up to `SHUTDOWN_TIMEOUT_MS`
    /// to respond to `shutdown` before we force-kill stdio children.
    pub async fn shutdown(&mut self) {
        let drained: Vec<(String, Arc<McpConnection>)> = self.connections.drain().collect();
        let grace = Duration::from_millis(SHUTDOWN_TIMEOUT_MS);
        let futs = drained.into_iter().map(|(name, conn)| async move {
            debug!(server = %name, "shutting down MCP server");
            let _ = timeout(grace, conn.send_request("shutdown", None)).await;
            // Abort reader/stderr tasks and kill child (owned in Drop path;
            // here we trigger it explicitly by dropping the last Arc).
            drop(conn);
        });
        join_all(futs).await;
        self.tool_routing.clear();
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct McpResourceToolArgs {
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

fn parse_mcp_resource_arguments(arguments: &str) -> Result<McpResourceToolArgs, String> {
    if arguments.trim().is_empty() {
        return Ok(McpResourceToolArgs::default());
    }
    serde_json::from_str(arguments).map_err(|e| format!("failed to parse function arguments: {e}"))
}

fn normalize_optional_tool_arg(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn cursor_params(cursor: Option<String>) -> Option<serde_json::Value> {
    normalize_optional_tool_arg(cursor).map(|cursor| {
        serde_json::json!({
            "cursor": cursor,
        })
    })
}

fn list_resource_payload(server_name: &str, result: serde_json::Value, array_key: &str) -> String {
    let next_cursor = result.get("nextCursor").cloned();
    let entries = tagged_array_entries(server_name, result, array_key);
    let mut payload = serde_json::Map::new();
    payload.insert(
        "server".to_string(),
        serde_json::Value::String(server_name.to_string()),
    );
    payload.insert(array_key.to_string(), serde_json::Value::Array(entries));
    if let Some(next_cursor) = next_cursor {
        payload.insert("nextCursor".to_string(), next_cursor);
    }
    serde_json::Value::Object(payload).to_string()
}

fn tagged_array_entries(
    server_name: &str,
    result: serde_json::Value,
    array_key: &str,
) -> Vec<serde_json::Value> {
    result
        .get(array_key)
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let mut object = match entry {
                        serde_json::Value::Object(map) => map.clone(),
                        other => {
                            let mut map = serde_json::Map::new();
                            map.insert("value".to_string(), other.clone());
                            map
                        }
                    };
                    object.insert(
                        "server".to_string(),
                        serde_json::Value::String(server_name.to_string()),
                    );
                    serde_json::Value::Object(object)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn mcp_tool_exposure_from_definitions(tools: Vec<ToolDefinition>) -> McpToolExposure {
    if tools.len() >= DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD {
        McpToolExposure {
            direct_tools: Vec::new(),
            deferred_tools: tools,
        }
    } else {
        McpToolExposure {
            direct_tools: tools,
            deferred_tools: Vec::new(),
        }
    }
}

pub(crate) fn mcp_tool_exposure_with_resource_tools(
    server_tools: Vec<ToolDefinition>,
    has_connections: bool,
) -> McpToolExposure {
    let mut exposure = mcp_tool_exposure_from_definitions(server_tools);
    if has_connections {
        let mut resource_tools = resources::all_resource_tool_definitions();
        resource_tools.extend(exposure.direct_tools);
        exposure.direct_tools = resource_tools;
    }
    exposure
}

impl Drop for McpManager {
    fn drop(&mut self) {
        // Dropping the Arc<McpConnection>s propagates to McpConnection::drop
        // which aborts tasks + kills children. No further work required.
    }
}

// ---------------------------------------------------------------------------
// Server bootstrap
// ---------------------------------------------------------------------------

async fn start_one_server(
    name: &str,
    config: &McpServerConfig,
) -> Result<Arc<McpConnection>, String> {
    let per_request_timeout =
        Duration::from_secs(config.tool_timeout_sec.unwrap_or(DEFAULT_TOOL_TIMEOUT_SEC));
    let startup_timeout = Duration::from_secs(
        config
            .startup_timeout_sec
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SEC),
    );
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

    let (conn_core, mut tasks) = if config.url.is_some() {
        build_http_connection(name, config, pending.clone(), per_request_timeout).await?
    } else if config.command.is_some() {
        build_stdio_connection(name, config, pending.clone(), per_request_timeout).await?
    } else {
        return Err(format!(
            "MCP server '{name}' has neither 'url' nor 'command' configured"
        ));
    };

    let (sink, child) = conn_core;
    let mut conn = McpConnection {
        server_name: name.to_string(),
        sink: Mutex::new(sink),
        pending,
        next_id: std::sync::atomic::AtomicU64::new(1),
        tools: Vec::new(),
        per_request_timeout,
        tasks: std::mem::take(&mut tasks),
        child,
    };

    // Initialize + tools/list under the startup timeout.
    timeout(startup_timeout, async {
        initialize_connection(&conn).await?;
        let tools = list_tools_from_connection(&conn, name, config).await?;
        conn.tools = tools;
        Ok::<_, String>(())
    })
    .await
    .map_err(|_| {
        format!(
            "MCP '{name}' startup timed out after {}s",
            startup_timeout.as_secs()
        )
    })??;

    Ok(Arc::new(conn))
}

async fn build_stdio_connection(
    name: &str,
    config: &McpServerConfig,
    pending: PendingMap,
    _per_request_timeout: Duration,
) -> Result<((Box<dyn FrameSink>, Option<Child>), Vec<JoinHandle<()>>), String> {
    let command = config
        .command
        .as_deref()
        .ok_or_else(|| format!("MCP '{name}' missing command"))?;
    let args = config.args.as_deref().unwrap_or(&[]);

    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ref cwd) = config.cwd {
        cmd.current_dir(cwd);
    }
    if let Some(ref env_map) = config.env {
        for (k, v) in env_map {
            cmd.env(k, v);
        }
    }

    info!(server = %name, command = %command, "starting MCP server (stdio)");

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn MCP '{name}' ({command}): {e}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("no stdin for MCP '{name}'"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("no stdout for MCP '{name}'"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("no stderr for MCP '{name}'"))?;

    let reader_task = spawn_stdio_reader(name.to_string(), stdout, pending);
    let stderr_task = spawn_stderr_pump(name.to_string(), stderr);

    let sink: Box<dyn FrameSink> = Box::new(StdioSink {
        server_name: name.to_string(),
        stdin,
    });
    Ok(((sink, Some(child)), vec![reader_task, stderr_task]))
}

async fn build_http_connection(
    name: &str,
    config: &McpServerConfig,
    pending: PendingMap,
    per_request_timeout: Duration,
) -> Result<((Box<dyn FrameSink>, Option<Child>), Vec<JoinHandle<()>>), String> {
    let url = config
        .url
        .as_deref()
        .ok_or_else(|| format!("MCP '{name}' missing url"))?
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(per_request_timeout)
        .build()
        .map_err(|e| format!("build HTTP client for '{name}': {e}"))?;

    // Read token lazily so rotation without a restart is possible. For the
    // common case (env-var), the snapshot is cheap so we refresh per send.
    let auth_header = config
        .bearer_token_env_var
        .as_deref()
        .and_then(|env_var| std::env::var(env_var).ok())
        .map(|token| format!("Bearer {token}"));

    info!(server = %name, url = %url, "starting MCP server (HTTP)");

    let session_id = Arc::new(Mutex::new(None::<String>));
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<String>();

    let demux_task = spawn_http_demux(name.to_string(), frame_rx, pending.clone());
    let sse_task = spawn_http_sse_reader(
        name.to_string(),
        client.clone(),
        url.clone(),
        auth_header.clone(),
        session_id.clone(),
        frame_tx.clone(),
    );

    let sink: Box<dyn FrameSink> = Box::new(HttpSink {
        server_name: name.to_string(),
        client,
        url,
        auth_header,
        session_id,
        frame_tx,
    });

    Ok(((sink, None), vec![demux_task, sse_task]))
}

async fn initialize_connection(conn: &McpConnection) -> Result<(), String> {
    let params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "bifrost-agent",
            "version": env!("CARGO_PKG_VERSION")
        }
    });
    conn.send_request("initialize", Some(params)).await?;
    conn.send_notification("notifications/initialized", None)
        .await?;
    Ok(())
}

async fn list_tools_from_connection(
    conn: &McpConnection,
    server_name: &str,
    config: &McpServerConfig,
) -> Result<Vec<ToolDefinition>, String> {
    let result = conn.send_request("tools/list", None).await?;
    let tools_value = result
        .get("tools")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));

    let mcp_tools: Vec<McpToolInfo> =
        serde_json::from_value(tools_value).map_err(|e| format!("parse tools list: {e}"))?;

    let mut tool_defs = Vec::new();
    let mut seen_local = std::collections::HashSet::new();
    for tool in mcp_tools {
        if let Some(ref enabled) = config.enabled_tools {
            if !enabled.contains(&tool.name) {
                continue;
            }
        }
        if let Some(ref disabled) = config.disabled_tools {
            if disabled.contains(&tool.name) {
                continue;
            }
        }

        // Validate the raw tool name; reject obvious garbage.
        if tool.name.is_empty() || tool.name.len() > 64 {
            warn!(server = %server_name, tool = %tool.name, "skipping tool with invalid name length");
            continue;
        }
        if !seen_local.insert(tool.name.clone()) {
            warn!(server = %server_name, tool = %tool.name, "skipping duplicate tool from same server");
            continue;
        }

        let sanprefix = sanitise_server_prefix(server_name, tool.name.len());
        // Use double-underscore as the split marker so arbitrary user server
        // names containing '_' can be recovered on dispatch.
        let prefixed_name = format!("mcp_{sanprefix}__{}", tool.name);
        if !is_valid_tool_name(&prefixed_name) {
            warn!(
                server = %server_name,
                tool = %tool.name,
                prefixed = %prefixed_name,
                "prefixed tool name violates function-calling constraints; skipping"
            );
            continue;
        }

        let parameters = validate_or_default_schema(server_name, &tool.name, tool.input_schema);

        tool_defs.push(ToolDefinition::function(
            prefixed_name,
            tool.description.unwrap_or_default(),
            Some(parameters),
        ));
    }
    Ok(tool_defs)
}

/// Minimal JSON Schema sanity check: must be an object with `"type":"object"`
/// at the top or a `$ref`. On failure we substitute `{"type":"object"}` so the
/// OpenAI function-calling API accepts the tool and a misbehaving server can
/// not break the whole turn.
fn validate_or_default_schema(
    server_name: &str,
    tool_name: &str,
    schema: Option<serde_json::Value>,
) -> serde_json::Value {
    let fallback = serde_json::json!({"type": "object"});
    let Some(schema) = schema else {
        return fallback;
    };
    let obj = match schema {
        serde_json::Value::Object(ref m) => m,
        _ => {
            warn!(server = %server_name, tool = %tool_name, "schema is not an object; substituting default");
            return fallback;
        }
    };
    let has_type_object = obj
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s == "object")
        .unwrap_or(false);
    let has_ref = obj.contains_key("$ref");
    if !(has_type_object || has_ref) {
        warn!(
            server = %server_name,
            tool = %tool_name,
            "schema missing type=object and $ref; substituting default"
        );
        return fallback;
    }
    schema
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
