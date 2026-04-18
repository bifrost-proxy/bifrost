use std::time::Duration;

use bifrost_core::{direct_reqwest_client_builder, BifrostError};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::cli::{RemoteCommands, RemoteTrafficCommands};

const PAIRING_WATCH_TIMEOUT_SECS: u64 = 180;
const CALL_EVENT_TIMEOUT_SECS: u64 = 120;
const CALLER_USER_AGENT: &str = "bifrost-cli-remote";

#[derive(Debug)]
pub struct RemoteOptions {
    pub relay_url: String,
    pub token: String,
    pub client_id: Option<String>,
    pub action: RemoteCommands,
}

pub fn handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| BifrostError::Config(format!("failed to build tokio runtime: {e}")))?;

    rt.block_on(async_handle_remote_command(opts))
}

async fn async_handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let caller = CallerRelayClient::new(&opts.relay_url, &opts.token);
    let client_instance_id = resolve_client_id(&caller, opts.client_id.as_deref()).await?;
    let caller_fingerprint = generate_caller_fingerprint();
    let caller_info = CallerInfo {
        fingerprint: caller_fingerprint.clone(),
        display_name: Some(get_hostname()),
        user_agent: Some(CALLER_USER_AGENT.to_string()),
        platform: Some(std::env::consts::OS.to_string()),
    };

    if let RemoteCommands::Connect { pair_code } = &opts.action {
        return handle_connect(&caller, &client_instance_id, pair_code, &caller_info).await;
    }

    let (command, args_json) = build_remote_command(&opts.action);
    let command_summary = CommandSummary {
        command_preview: command.clone(),
        masked_args_json: args_json.clone(),
    };

    let grant = caller
        .find_reusable_grant(&client_instance_id, &caller_fingerprint)
        .await?;

    let grant = match grant {
        Some(g) => g,
        None => {
            eprintln!(
                "{}",
                "✗ No existing authorization found. Please run `bifrost remote connect <pair-code>` first."
                    .bright_red()
            );
            std::process::exit(1);
        }
    };

    info!(grant_id = %grant.grant_id, "found reusable grant");
    println!(
        "{}",
        format!(
            "✓ Using authorization (grant: {})",
            &grant.grant_id[..grant.grant_id.len().min(8)]
        )
        .bright_green()
    );

    let call_result = caller
        .open_call(&OpenCallRequest {
            grant_id: grant.grant_id.clone(),
            client_instance_id: client_instance_id.clone(),
            command: RemoteCommand {
                command: command.clone(),
                args_json: args_json.clone(),
            },
            command_summary,
            caller_pubkey: String::new(),
        })
        .await?;

    debug!(call_id = %call_result.call_id, grant_id = %grant.grant_id, "call opened, subscribing to events");
    println!("{}", "→ Executing command on remote device...".dimmed());

    let result = caller
        .subscribe_call_events(&call_result.call_id, &call_result.relay_token)
        .await?;

    print_remote_result(&command, &result);

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}

async fn handle_connect(
    caller: &CallerRelayClient,
    client_instance_id: &str,
    pair_code: &str,
    caller_info: &CallerInfo,
) -> bifrost_core::Result<()> {
    let command_summary = CommandSummary {
        command_preview: "connect".to_string(),
        masked_args_json: None,
    };

    println!(
        "{}",
        format!(
            "→ Initiating pairing with code {}...",
            pair_code.bright_cyan()
        )
        .dimmed()
    );

    let pairing_result = caller
        .start_pairing(&StartPairingRequest {
            client_instance_id: client_instance_id.to_string(),
            pair_code: pair_code.to_string(),
            caller_pubkey: String::new(),
            caller_info: caller_info.clone(),
            command_summary,
            command: RemoteCommand {
                command: "connect".to_string(),
                args_json: None,
            },
        })
        .await?;

    println!(
        "{}",
        "⏳ Waiting for approval on the remote device...".bright_yellow()
    );

    let approval = caller.watch_pairing(&pairing_result.pairing_id).await?;

    match approval.status.as_str() {
        "approved" => {
            let grant_id = approval.grant_id.unwrap_or_else(|| "unknown".to_string());
            println!(
                "{}",
                format!(
                    "✓ Connected! Authorization granted (grant: {})",
                    &grant_id[..grant_id.len().min(8)]
                )
                .bright_green()
            );
            println!(
                "{}",
                "  You can now run commands like: bifrost remote status".dimmed()
            );
            Ok(())
        }
        "rejected" => {
            println!("{}", "✗ Pairing was rejected.".bright_red());
            Err(BifrostError::Config("pairing rejected".to_string()))
        }
        other => {
            println!(
                "{}",
                format!("✗ Pairing ended with status: {other}").bright_red()
            );
            Err(BifrostError::Config(format!(
                "pairing failed with status: {other}"
            )))
        }
    }
}

fn build_remote_command(action: &RemoteCommands) -> (String, Option<String>) {
    match action {
        RemoteCommands::Connect { .. } => unreachable!("connect handled separately"),
        RemoteCommands::Status => ("status".to_string(), None),
        RemoteCommands::Search { keyword, limit } => {
            let args = serde_json::json!({
                "query": keyword,
                "limit": limit,
            });
            ("search.get".to_string(), Some(args.to_string()))
        }
        RemoteCommands::Traffic { action } => match action {
            RemoteTrafficCommands::List {
                limit,
                cursor,
                method,
                status,
            } => {
                let mut args = serde_json::json!({
                    "limit": limit,
                });
                if let Some(c) = cursor {
                    args["cursor"] = serde_json::json!(c);
                }
                if let Some(m) = method {
                    args["method"] = serde_json::json!(m);
                }
                if let Some(s) = status {
                    args["status"] = serde_json::json!(s);
                }
                ("traffic.list".to_string(), Some(args.to_string()))
            }
            RemoteTrafficCommands::Get {
                id,
                request_body,
                response_body,
            } => {
                let args = serde_json::json!({
                    "id": id,
                    "request_body": request_body,
                    "response_body": response_body,
                });
                ("traffic.get".to_string(), Some(args.to_string()))
            }
            RemoteTrafficCommands::Search { keyword, limit } => {
                let args = serde_json::json!({
                    "keyword": keyword,
                    "limit": limit,
                });
                ("traffic.search".to_string(), Some(args.to_string()))
            }
        },
    }
}

fn generate_caller_fingerprint() -> String {
    let machine_id = get_hostname();
    let user = get_username();
    let raw = format!("bifrost-cli:{}:{}", user, machine_id);
    format!("{:x}", simple_hash(raw.as_bytes()))
}

fn simple_hash(data: &[u8]) -> u128 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let h1 = hasher.finish();
    let mut hasher2 = DefaultHasher::new();
    h1.hash(&mut hasher2);
    let h2 = hasher2.finish();
    (h1 as u128) << 64 | (h2 as u128)
}

fn get_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn get_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

async fn resolve_client_id(
    caller: &CallerRelayClient,
    explicit_id: Option<&str>,
) -> bifrost_core::Result<String> {
    if let Some(id) = explicit_id {
        return Ok(id.to_string());
    }

    let clients = caller.list_online_clients().await?;
    match clients.len() {
        0 => Err(BifrostError::Config(
            "no online clients found on relay server".to_string(),
        )),
        1 => {
            let id = clients[0]
                .get("client_instance_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    BifrostError::Config("client response missing client_instance_id".to_string())
                })?;
            println!(
                "{}",
                format!("→ Found online client: {}", &id[..id.len().min(12)]).dimmed()
            );
            Ok(id.to_string())
        }
        n => {
            println!("{}", format!("Found {} online clients:", n).bright_yellow());
            for (i, c) in clients.iter().enumerate() {
                let id = c
                    .get("client_instance_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let name = c
                    .get("device_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                println!(
                    "  {} {} ({})",
                    format!("[{}]", i + 1).bright_cyan(),
                    name,
                    id
                );
            }
            Err(BifrostError::Config(
                "multiple clients online, please specify --client-id".to_string(),
            ))
        }
    }
}

fn print_remote_result(command: &str, result: &CallResult) {
    if let Some(ref stdout) = result.stdout {
        if !stdout.is_empty() {
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
        }
    }

    if let Some(ref stderr) = result.stderr {
        if !stderr.is_empty() {
            eprint!("{stderr}");
            if !stderr.ends_with('\n') {
                eprintln!();
            }
        }
    }

    if result.exit_code != 0 {
        eprintln!(
            "{}",
            format!(
                "Remote command '{}' exited with code {}",
                command, result.exit_code
            )
            .bright_red()
        );
    }
}

// ---------------------------------------------------------------------------
// Caller Relay Client
// ---------------------------------------------------------------------------

struct CallerRelayClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallerInfo {
    fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandSummary {
    command_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    masked_args_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteCommand {
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingRequest {
    client_instance_id: String,
    pair_code: String,
    caller_pubkey: String,
    caller_info: CallerInfo,
    command_summary: CommandSummary,
    command: RemoteCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingResponse {
    pairing_id: String,
    #[serde(default)]
    approval_sse_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingWatchResult {
    status: String,
    #[serde(default)]
    grant_id: Option<String>,
    #[serde(default)]
    relay_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantInfo {
    grant_id: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallRequest {
    grant_id: String,
    client_instance_id: String,
    command: RemoteCommand,
    command_summary: CommandSummary,
    caller_pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallResponse {
    call_id: String,
    relay_token: String,
}

#[derive(Debug, Clone, Default)]
struct CallResult {
    exit_code: i32,
    stdout: Option<String>,
    stderr: Option<String>,
    duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RelayApiResponse<T> {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    data: Option<T>,
}

impl CallerRelayClient {
    fn new(base_url: &str, token: &str) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build caller http client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if !self.token.is_empty() {
            headers.insert(
                "x-bifrost-token",
                reqwest::header::HeaderValue::from_str(&self.token)
                    .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("")),
            );
        }
        headers
    }

    async fn list_online_clients(&self) -> bifrost_core::Result<Vec<Value>> {
        let url = format!("{}/v4/remote-invoke/clients", self.base_url);
        let response = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("list clients failed: {e}")))?;

        let data: Value = self.parse_response_data(response, "list_clients").await?;
        match data {
            Value::Array(arr) => Ok(arr),
            _ => Ok(vec![]),
        }
    }

    async fn find_reusable_grant(
        &self,
        client_instance_id: &str,
        caller_fingerprint: &str,
    ) -> bifrost_core::Result<Option<GrantInfo>> {
        let url = format!(
            "{}/v4/remote-invoke/grants/reusable?client_instance_id={}&caller_fingerprint={}",
            self.base_url,
            urlencoding::encode(client_instance_id),
            urlencoding::encode(caller_fingerprint),
        );

        let response = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("find reusable grant failed: {e}")))?;

        let data: Value = self
            .parse_response_data(response, "find_reusable_grant")
            .await?;
        if data.is_null() {
            return Ok(None);
        }
        let grant: GrantInfo = serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("parse grant failed: {e}")))?;
        if grant.grant_id.is_empty() {
            return Ok(None);
        }
        Ok(Some(grant))
    }

    async fn start_pairing(
        &self,
        req: &StartPairingRequest,
    ) -> bifrost_core::Result<StartPairingResponse> {
        let url = format!("{}/v4/remote-invoke/pairings/start", self.base_url);
        let response = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("start pairing failed: {e}")))?;

        self.parse_response_typed(response, "start_pairing").await
    }

    async fn watch_pairing(&self, pairing_id: &str) -> bifrost_core::Result<PairingWatchResult> {
        let url = format!(
            "{}/v4/remote-invoke/pairings/{}/watch",
            self.base_url, pairing_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("watch pairing failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "watch pairing returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let timeout = tokio::time::sleep(Duration::from_secs(PAIRING_WATCH_TIMEOUT_SECS));
        tokio::pin!(timeout);

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    return Err(BifrostError::Config("pairing approval timed out".to_string()));
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        debug!(event = %event_name, "pairing SSE event");
                                        match event_name.as_str() {
                                            "decision" | "approved" | "rejected" | "status" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    let status = v.get("status")
                                                        .or_else(|| v.get("decision"))
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or(&event_name)
                                                        .to_string();
                                                    let grant_id = v.get("grant_id")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let relay_token = v.get("relay_token")
                                                        .and_then(|t| t.as_str())
                                                        .map(|s| s.to_string());

                                                    if status == "approved" || status == "rejected" || status == "expired" || status == "cancelled" {
                                                        return Ok(PairingWatchResult { status, grant_id, relay_token });
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    event_name.clear();
                                    data_buf.clear();
                                } else if let Some(ev) = line.strip_prefix("event:") {
                                    event_name = ev.trim().to_string();
                                } else if let Some(d) = line.strip_prefix("data:") {
                                    if !data_buf.is_empty() {
                                        data_buf.push('\n');
                                    }
                                    data_buf.push_str(d.trim());
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("pairing watch SSE error: {e}")));
                        }
                        None => {
                            return Err(BifrostError::Network("pairing watch stream closed unexpectedly".to_string()));
                        }
                    }
                }
            }
        }
    }

    async fn open_call(&self, req: &OpenCallRequest) -> bifrost_core::Result<OpenCallResponse> {
        let url = format!("{}/v4/remote-invoke/calls/open", self.base_url);
        let response = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("open call failed: {e}")))?;

        self.parse_response_typed(response, "open_call").await
    }

    async fn subscribe_call_events(
        &self,
        call_id: &str,
        relay_token: &str,
    ) -> bifrost_core::Result<CallResult> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, call_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build call events sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("subscribe call events failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "call events returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let timeout = tokio::time::sleep(Duration::from_secs(CALL_EVENT_TIMEOUT_SECS));
        tokio::pin!(timeout);

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();
        let mut result = CallResult::default();
        let mut stdout_parts: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    warn!("call events timed out");
                    if stdout_parts.is_empty() {
                        return Err(BifrostError::Config("remote call timed out waiting for response".to_string()));
                    }
                    result.stdout = Some(stdout_parts.join(""));
                    return Ok(result);
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        debug!(event = %event_name, "call event");
                                        match event_name.as_str() {
                                            "frame" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    if let Some(envelope_json) = v.get("envelope_json").and_then(|e| e.as_str()) {
                                                        if let Ok(envelope) = serde_json::from_str::<Value>(envelope_json) {
                                                            if let Some(ct) = envelope.get("ciphertext").and_then(|c| c.as_str()) {
                                                                stdout_parts.push(ct.to_string());
                                                            }
                                                        }
                                                    } else if let Some(ct) = v.get("ciphertext").and_then(|c| c.as_str()) {
                                                        stdout_parts.push(ct.to_string());
                                                    }
                                                }
                                            }
                                            "exit" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    result.exit_code = v.get("exit_code")
                                                        .and_then(|c| c.as_i64())
                                                        .unwrap_or(0) as i32;
                                                    result.duration_ms = v.get("duration_ms")
                                                        .and_then(|d| d.as_u64());
                                                }
                                                result.stdout = Some(stdout_parts.join(""));
                                                return Ok(result);
                                            }
                                            "error" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    let msg = v.get("message")
                                                        .or_else(|| v.get("error"))
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("unknown error");
                                                    error!(error = %msg, "call error from relay");
                                                    result.exit_code = -1;
                                                    result.stderr = Some(msg.to_string());
                                                }
                                                result.stdout = Some(stdout_parts.join(""));
                                                return Ok(result);
                                            }
                                            _ => {
                                                debug!(event = %event_name, "unhandled call event");
                                            }
                                        }
                                    }
                                    event_name.clear();
                                    data_buf.clear();
                                } else if let Some(ev) = line.strip_prefix("event:") {
                                    event_name = ev.trim().to_string();
                                } else if let Some(d) = line.strip_prefix("data:") {
                                    if !data_buf.is_empty() {
                                        data_buf.push('\n');
                                    }
                                    data_buf.push_str(d.trim());
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("call events SSE error: {e}")));
                        }
                        None => {
                            info!("call events stream closed");
                            result.stdout = Some(stdout_parts.join(""));
                            return Ok(result);
                        }
                    }
                }
            }
        }
    }

    async fn parse_response_data(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> bifrost_core::Result<Value> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| BifrostError::Network(format!("{operation} response read failed: {e}")))?;

        if !status.is_success() {
            return Err(BifrostError::Network(format!(
                "{operation} failed with status {status}: {}",
                truncate(&body, 500)
            )));
        }

        let envelope: RelayApiResponse<Value> = serde_json::from_str(&body).map_err(|e| {
            BifrostError::Network(format!(
                "{operation} invalid JSON: {e} body={}",
                truncate(&body, 500)
            ))
        })?;

        if envelope.code != 0 {
            let msg = envelope.message.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "{operation} error code {}: {msg}",
                envelope.code
            )));
        }

        Ok(envelope.data.unwrap_or(Value::Null))
    }

    async fn parse_response_typed<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> bifrost_core::Result<T> {
        let data = self.parse_response_data(response, operation).await?;
        serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("{operation} parse failed: {e}")))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...(truncated)")
    }
}
