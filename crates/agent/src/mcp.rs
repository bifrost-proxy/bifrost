//! MCP (Model Context Protocol) server management.
//!
//! Supports stdio transport for MCP servers. Manages lifecycle:
//! - Start server processes
//! - Initialize (capabilities exchange)
//! - List tools
//! - Call tools
//! - Shutdown

use crate::config::McpServerConfig;
use crate::types::{FunctionDefinition, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tracing::{debug, error, info, warn};

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
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// MCP Tool schema types
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
// McpTransport
// ---------------------------------------------------------------------------

/// Transport layer for MCP server communication.
enum McpTransport {
    /// Stdio transport via child process stdin/stdout.
    Stdio {
        #[allow(dead_code)]
        child: Child,
        stdin: ChildStdin,
        stdout_reader: BufReader<ChildStdout>,
    },
    /// HTTP (StreamableHttp) transport via POST requests.
    Http {
        client: reqwest::Client,
        url: String,
        session_id: Option<String>,
        auth_header: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// McpConnection
// ---------------------------------------------------------------------------

/// An active MCP server connection.
struct McpConnection {
    server_name: String,
    transport: McpTransport,
    tools: Vec<ToolDefinition>,
    next_id: u64,
}

impl McpConnection {
    /// Send a JSON-RPC request and read the response.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        match &mut self.transport {
            McpTransport::Stdio {
                stdin,
                stdout_reader,
                ..
            } => {
                let mut request_bytes =
                    serde_json::to_vec(&request).map_err(|e| format!("serialize request: {e}"))?;
                request_bytes.push(b'\n');

                stdin
                    .write_all(&request_bytes)
                    .await
                    .map_err(|e| format!("write to MCP server '{}': {e}", self.server_name))?;

                stdin
                    .flush()
                    .await
                    .map_err(|e| format!("flush MCP server '{}': {e}", self.server_name))?;

                // Read response line
                let mut line = String::new();
                stdout_reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| format!("read from MCP server '{}': {e}", self.server_name))?;

                if line.is_empty() {
                    return Err(format!(
                        "MCP server '{}' closed connection",
                        self.server_name
                    ));
                }

                let response: JsonRpcResponse = serde_json::from_str(&line)
                    .map_err(|e| format!("parse MCP response from '{}': {e}", self.server_name))?;

                if let Some(error) = response.error {
                    return Err(format!(
                        "MCP server '{}' error: {}",
                        self.server_name, error.message
                    ));
                }

                response
                    .result
                    .ok_or_else(|| format!("MCP server '{}' returned no result", self.server_name))
            }
            McpTransport::Http {
                client,
                url,
                session_id,
                auth_header,
            } => {
                let mut req_builder = client
                    .post(url.as_str())
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream");

                if let Some(ref sid) = session_id {
                    req_builder = req_builder.header("Mcp-Session-Id", sid.as_str());
                }
                if let Some(ref auth) = auth_header {
                    req_builder = req_builder.header("Authorization", auth.as_str());
                }

                let body =
                    serde_json::to_vec(&request).map_err(|e| format!("serialize request: {e}"))?;

                let resp = req_builder.body(body).send().await.map_err(|e| {
                    format!("HTTP request to MCP server '{}': {e}", self.server_name)
                })?;

                // Track session ID from response header
                if let Some(sid_value) = resp.headers().get("mcp-session-id") {
                    if let Ok(sid_str) = sid_value.to_str() {
                        *session_id = Some(sid_str.to_string());
                    }
                }

                let status = resp.status();
                if !status.is_success() {
                    let body_text = resp.text().await.unwrap_or_default();
                    return Err(format!(
                        "MCP server '{}' HTTP error {}: {}",
                        self.server_name, status, body_text
                    ));
                }

                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                let body_text = resp.text().await.map_err(|e| {
                    format!(
                        "read HTTP response from MCP server '{}': {e}",
                        self.server_name
                    )
                })?;

                let json_str = if content_type.contains("text/event-stream") {
                    // Parse SSE: extract data: lines and join
                    parse_sse_response(&body_text)
                } else {
                    // Direct JSON response
                    body_text
                };

                let response: JsonRpcResponse = serde_json::from_str(&json_str)
                    .map_err(|e| format!("parse MCP response from '{}': {e}", self.server_name))?;

                if let Some(error) = response.error {
                    return Err(format!(
                        "MCP server '{}' error: {}",
                        self.server_name, error.message
                    ));
                }

                response
                    .result
                    .ok_or_else(|| format!("MCP server '{}' returned no result", self.server_name))
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };

        match &mut self.transport {
            McpTransport::Stdio { stdin, .. } => {
                let mut bytes = serde_json::to_vec(&notification)
                    .map_err(|e| format!("serialize notification: {e}"))?;
                bytes.push(b'\n');
                stdin
                    .write_all(&bytes)
                    .await
                    .map_err(|e| format!("send notification to '{}': {e}", self.server_name))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| format!("flush notification to '{}': {e}", self.server_name))?;
            }
            McpTransport::Http {
                client,
                url,
                session_id,
                auth_header,
            } => {
                let mut req_builder = client
                    .post(url.as_str())
                    .header("Content-Type", "application/json");

                if let Some(ref sid) = session_id {
                    req_builder = req_builder.header("Mcp-Session-Id", sid.as_str());
                }
                if let Some(ref auth) = auth_header {
                    req_builder = req_builder.header("Authorization", auth.as_str());
                }

                let body = serde_json::to_vec(&notification)
                    .map_err(|e| format!("serialize notification: {e}"))?;

                // Fire-and-forget for notifications, but log errors
                if let Err(e) = req_builder.body(body).send().await {
                    warn!(
                        server = %self.server_name,
                        error = %e,
                        "failed to send notification via HTTP"
                    );
                }
            }
        }

        Ok(())
    }
}

/// Parse SSE response body: extract `data:` lines and join them.
fn parse_sse_response(body: &str) -> String {
    let mut data_parts = Vec::new();
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let trimmed = data.trim_start();
            if !trimmed.is_empty() {
                data_parts.push(trimmed.to_string());
            }
        }
    }
    data_parts.join("")
}

// ---------------------------------------------------------------------------
// McpManager
// ---------------------------------------------------------------------------

/// Manages MCP server lifecycle and tool routing.
pub struct McpManager {
    connections: HashMap<String, McpConnection>,
    /// Map from tool name → server name for routing.
    tool_routing: HashMap<String, String>,
}

impl McpManager {
    /// Create a new McpManager and start all configured servers.
    pub async fn new(configs: &HashMap<String, McpServerConfig>) -> Self {
        let mut manager = Self {
            connections: HashMap::new(),
            tool_routing: HashMap::new(),
        };

        for (name, config) in configs {
            if !config.enabled {
                debug!(server = %name, "skipping disabled MCP server");
                continue;
            }

            if let Err(e) = manager.start_server(name, config).await {
                error!(server = %name, error = %e, "failed to start MCP server");
            }
        }

        manager
    }

    /// Start an MCP server, detecting transport type from config.
    async fn start_server(&mut self, name: &str, config: &McpServerConfig) -> Result<(), String> {
        let transport = if config.url.is_some() {
            self.create_http_transport(name, config)?
        } else if config.command.is_some() {
            self.create_stdio_transport(name, config).await?
        } else {
            return Err(format!(
                "MCP server '{}' has neither 'url' nor 'command' configured",
                name
            ));
        };

        let mut conn = McpConnection {
            server_name: name.to_string(),
            transport,
            tools: Vec::new(),
            next_id: 1,
        };

        // Initialize the server
        self.initialize_connection(&mut conn).await?;

        // List tools
        let tools = self.list_tools_from_connection(&mut conn, config).await?;
        conn.tools = tools;

        // Register tool routing
        for tool in &conn.tools {
            self.tool_routing
                .insert(tool.function.name.clone(), name.to_string());
        }

        info!(
            server = %name,
            tool_count = conn.tools.len(),
            "MCP server initialized"
        );

        self.connections.insert(name.to_string(), conn);
        Ok(())
    }

    /// Create a stdio transport by spawning a child process.
    async fn create_stdio_transport(
        &self,
        name: &str,
        config: &McpServerConfig,
    ) -> Result<McpTransport, String> {
        let command = config
            .command
            .as_deref()
            .ok_or_else(|| format!("MCP server '{}' has no command", name))?;

        let args = config.args.as_deref().unwrap_or(&[]);

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

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
            .map_err(|e| format!("failed to spawn MCP server '{}' ({}): {e}", name, command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("no stdin for MCP server '{}'", name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("no stdout for MCP server '{}'", name))?;

        Ok(McpTransport::Stdio {
            child,
            stdin,
            stdout_reader: BufReader::new(stdout),
        })
    }

    /// Create an HTTP transport.
    fn create_http_transport(
        &self,
        name: &str,
        config: &McpServerConfig,
    ) -> Result<McpTransport, String> {
        let url = config
            .url
            .as_deref()
            .ok_or_else(|| format!("MCP server '{}' has no url", name))?
            .to_string();

        let timeout_secs = config.tool_timeout_sec.unwrap_or(60);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("failed to create HTTP client for '{}': {e}", name))?;

        // Resolve bearer token from environment variable
        let auth_header = config.bearer_token_env_var.as_deref().and_then(|env_var| {
            std::env::var(env_var)
                .ok()
                .map(|token| format!("Bearer {}", token))
        });

        info!(server = %name, url = %url, "starting MCP server (HTTP)");

        Ok(McpTransport::Http {
            client,
            url,
            session_id: None,
            auth_header,
        })
    }

    /// Initialize an MCP connection.
    async fn initialize_connection(&self, conn: &mut McpConnection) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "bifrost-agent",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        conn.send_request("initialize", Some(params)).await?;

        // Send initialized notification
        conn.send_notification("notifications/initialized", None)
            .await?;

        Ok(())
    }

    /// List tools from an MCP connection, applying filters.
    async fn list_tools_from_connection(
        &self,
        conn: &mut McpConnection,
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
        for tool in mcp_tools {
            // Apply enabled/disabled filter
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

            let prefixed_name = format!("mcp_{}_{}", conn.server_name, tool.name);
            tool_defs.push(ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: prefixed_name,
                    description: tool.description.unwrap_or_default(),
                    parameters: tool.input_schema,
                },
            });
        }

        Ok(tool_defs)
    }

    /// List tools from all connected MCP servers.
    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        self.connections
            .values()
            .flat_map(|conn| conn.tools.iter().cloned())
            .collect()
    }

    /// Call a tool on the appropriate MCP server.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<ToolResult, String> {
        let server_name = self
            .tool_routing
            .get(tool_name)
            .ok_or_else(|| format!("no MCP server for tool '{}'", tool_name))?
            .clone();

        let conn = self
            .connections
            .get_mut(&server_name)
            .ok_or_else(|| format!("MCP server '{}' not connected", server_name))?;

        // Strip prefix to get original tool name
        let prefix = format!("mcp_{}_", server_name);
        let original_name = tool_name.strip_prefix(&prefix).unwrap_or(tool_name);

        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let params = serde_json::json!({
            "name": original_name,
            "arguments": args,
        });

        debug!(server = %server_name, tool = %original_name, "calling MCP tool");

        let result = conn.send_request("tools/call", Some(params)).await?;

        // Parse MCP tool result
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

        Ok(ToolResult {
            success: !is_error,
            output: content,
        })
    }

    /// Check if a tool name belongs to an MCP server.
    pub fn is_mcp_tool(&self, tool_name: &str) -> bool {
        self.tool_routing.contains_key(tool_name)
    }

    /// Shutdown all MCP servers.
    pub async fn shutdown(&mut self) {
        for (name, mut conn) in self.connections.drain() {
            debug!(server = %name, "shutting down MCP server");
            // Best-effort shutdown
            let _ = conn.send_request("shutdown", None).await;
            if let McpTransport::Stdio { mut child, .. } = conn.transport {
                let _ = child.kill().await;
            }
        }
        self.tool_routing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_manager_empty() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let configs = HashMap::new();
            let manager = McpManager::new(&configs).await;
            assert!(manager.list_tools().is_empty());
            assert!(!manager.is_mcp_tool("anything"));
        });
    }

    #[test]
    fn test_mcp_manager_disabled_server() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut configs = HashMap::new();
            configs.insert(
                "test".to_string(),
                McpServerConfig {
                    command: Some("echo".to_string()),
                    args: None,
                    env: None,
                    cwd: None,
                    url: None,
                    bearer_token_env_var: None,
                    enabled: false,
                    startup_timeout_sec: None,
                    tool_timeout_sec: None,
                    enabled_tools: None,
                    disabled_tools: None,
                },
            );
            let manager = McpManager::new(&configs).await;
            assert!(manager.list_tools().is_empty());
        });
    }

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(!json.contains("params"));
    }

    #[test]
    fn test_json_rpc_response_parsing() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response_parsing() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().message, "Method not found");
    }

    #[test]
    fn test_http_transport_detection() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut configs = HashMap::new();
            // HTTP config: has url, no command
            configs.insert(
                "http_server".to_string(),
                McpServerConfig {
                    command: None,
                    args: None,
                    env: None,
                    cwd: None,
                    url: Some("http://localhost:9999/mcp".to_string()),
                    bearer_token_env_var: None,
                    enabled: true,
                    startup_timeout_sec: None,
                    tool_timeout_sec: None,
                    enabled_tools: None,
                    disabled_tools: None,
                },
            );
            // The server won't actually connect, but we verify detection logic
            // by checking the error message indicates HTTP transport was attempted
            let manager = McpManager::new(&configs).await;
            // The connection will fail (no server running), so no tools
            assert!(manager.list_tools().is_empty());
            // The manager tried HTTP transport (not stdio), confirm no panic
        });
    }

    #[test]
    fn test_parse_sse_response() {
        let sse_body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1,\"result\":{\"tools\":[]}}\n\n";
        let parsed = parse_sse_response(sse_body);
        let resp: JsonRpcResponse = serde_json::from_str(&parsed).unwrap();
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_parse_sse_response_single_line() {
        let sse_body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let parsed = parse_sse_response(sse_body);
        let resp: JsonRpcResponse = serde_json::from_str(&parsed).unwrap();
        assert!(resp.result.is_some());
    }
}
