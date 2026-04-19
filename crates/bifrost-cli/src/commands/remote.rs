use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use bifrost_core::{direct_reqwest_client_builder, BifrostError};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::cli::{RemoteCommands, RemoteTrafficCommands};

const PAIRING_WATCH_TIMEOUT_SECS: u64 = 180;
const CALL_EVENT_TIMEOUT_SECS: u64 = 120;
const CALLER_USER_AGENT: &str = "bifrost-cli-remote";
const CONNECTIONS_FILE: &str = "remote-connections.json";

#[derive(Debug)]
pub struct RemoteOptions {
    pub relay_url: String,
    pub client_id: Option<String>,
    pub action: RemoteCommands,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalConnection {
    client_instance_id: String,
    device_name: String,
    platform: String,
    relay_url: String,
    grant_id: String,
    grant_mode: String,
    caller_fingerprint: String,
    connected_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionsFile {
    version: u32,
    connections: Vec<LocalConnection>,
}

fn connections_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONNECTIONS_FILE)
}

fn load_connections() -> bifrost_core::Result<Vec<LocalConnection>> {
    let path = connections_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "read {}: {e}",
            path.display()
        )))
    })?;
    let file: ConnectionsFile = serde_json::from_str(&content)
        .map_err(|e| BifrostError::Config(format!("parse {}: {e}", path.display())))?;
    Ok(file.connections)
}

fn save_connections(connections: &[LocalConnection]) -> bifrost_core::Result<()> {
    let path = connections_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }
    let file = ConnectionsFile {
        version: 1,
        connections: connections.to_vec(),
    };
    let content = serde_json::to_string_pretty(&file)
        .map_err(|e| BifrostError::Config(format!("serialize connections: {e}")))?;
    std::fs::write(&path, content).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    Ok(())
}

fn resolve_local_connection(
    connections: &[LocalConnection],
    explicit_id: Option<&str>,
) -> bifrost_core::Result<LocalConnection> {
    if let Some(prefix) = explicit_id {
        let matches: Vec<&LocalConnection> = connections
            .iter()
            .filter(|c| c.client_instance_id.starts_with(prefix))
            .collect();

        match matches.len() {
            0 => {
                return Err(BifrostError::Config(
                    "no saved connection matching that prefix, please run `bifrost remote connect <pair-code>` first".to_string(),
                ));
            }
            1 => {
                let conn = matches[0];
                if conn.client_instance_id != prefix {
                    debug!(prefix = %prefix, full_id = %conn.client_instance_id, "resolved short client id from local connections");
                }
                return Ok(conn.clone());
            }
            n => {
                if !std::io::stdin().is_terminal() {
                    return Err(BifrostError::Config(format!(
                        "ambiguous client id prefix '{prefix}' matches {n} saved connections, please be more specific"
                    )));
                }

                let items: Vec<String> = matches
                    .iter()
                    .map(|c| {
                        let short_id = &c.client_instance_id[..c.client_instance_id.len().min(12)];
                        format!("{} ({short_id})", c.device_name)
                    })
                    .collect();

                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!(
                        "Prefix '{prefix}' matches {n} connections, select one"
                    ))
                    .items(&items)
                    .default(0)
                    .interact()
                    .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

                return Ok(matches[selection].clone());
            }
        }
    }

    match connections.len() {
        0 => Err(BifrostError::Config(
            "no saved connection, please run `bifrost remote connect <pair-code>` first"
                .to_string(),
        )),
        1 => {
            let conn = &connections[0];
            let short_id = &conn.client_instance_id[..conn.client_instance_id.len().min(12)];
            println!(
                "{}",
                format!(
                    "→ Using saved connection: {} ({short_id})",
                    conn.device_name
                )
                .dimmed()
            );
            Ok(conn.clone())
        }
        n => {
            if !std::io::stdin().is_terminal() {
                return Err(BifrostError::Config(
                    "multiple saved connections, please specify --client-id".to_string(),
                ));
            }

            let items: Vec<String> = connections
                .iter()
                .map(|c| {
                    let short_id = &c.client_instance_id[..c.client_instance_id.len().min(12)];
                    format!("{} ({short_id})", c.device_name)
                })
                .collect();

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Found {n} saved connections, select one"))
                .items(&items)
                .default(0)
                .interact()
                .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

            Ok(connections[selection].clone())
        }
    }
}

pub fn handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| BifrostError::Config(format!("failed to build tokio runtime: {e}")))?;

    rt.block_on(async_handle_remote_command(opts))
}

async fn async_handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let caller = CallerRelayClient::new(&opts.relay_url);
    let caller_fingerprint = generate_caller_fingerprint();
    let caller_info = CallerInfo {
        fingerprint: caller_fingerprint.clone(),
        display_name: Some(get_hostname()),
        user_agent: Some(CALLER_USER_AGENT.to_string()),
        platform: Some(std::env::consts::OS.to_string()),
    };

    if let RemoteCommands::Connect { pair_code } = &opts.action {
        return handle_connect(&caller, pair_code, &caller_info, &opts.relay_url).await;
    }

    let connections = load_connections()?;

    if let RemoteCommands::Disconnect { all, grant_id } = &opts.action {
        return handle_disconnect(
            &caller,
            &connections,
            opts.client_id.as_deref(),
            *all,
            grant_id.as_deref(),
            &caller_fingerprint,
        )
        .await;
    }

    let conn = resolve_local_connection(&connections, opts.client_id.as_deref())?;

    let (command, args_json) = build_remote_command(&opts.action);
    let command_summary = CommandSummary {
        command_preview: command.clone(),
        masked_args_json: args_json.clone(),
    };

    let grant = caller
        .find_reusable_grant(&conn.client_instance_id, &caller_fingerprint)
        .await?;

    let grant = match grant {
        Some(g) => g,
        None => {
            eprintln!(
                "{}",
                "✗ Authorization expired or revoked. Please run `bifrost remote connect <pair-code>` again."
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
            client_instance_id: conn.client_instance_id.clone(),
            caller_fingerprint: caller_fingerprint.clone(),
            command: RemoteCommand {
                command: command.clone(),
                args_json: args_json.clone(),
            },
            command_summary,
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
    pair_code: &str,
    caller_info: &CallerInfo,
    relay_url: &str,
) -> bifrost_core::Result<()> {
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
            pair_code: pair_code.to_string(),
            caller_info: caller_info.clone(),
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
            let client_instance_id = approval.client_instance_id.unwrap_or_default();
            let device_name = approval
                .device_name
                .unwrap_or_else(|| "unknown".to_string());
            let platform = approval.platform.unwrap_or_else(|| "unknown".to_string());
            let grant_mode = approval.grant_mode.unwrap_or_else(|| "unknown".to_string());

            let new_conn = LocalConnection {
                client_instance_id: client_instance_id.clone(),
                device_name: device_name.clone(),
                platform: platform.clone(),
                relay_url: relay_url.to_string(),
                grant_id: grant_id.clone(),
                grant_mode,
                caller_fingerprint: caller_info.fingerprint.clone(),
                connected_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            };

            let mut connections = load_connections().unwrap_or_default();
            if let Some(existing) = connections
                .iter_mut()
                .find(|c| c.client_instance_id == client_instance_id && c.relay_url == relay_url)
            {
                *existing = new_conn;
            } else {
                connections.push(new_conn);
            }
            save_connections(&connections)?;

            let short_id = &client_instance_id[..client_instance_id.len().min(12)];
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
                format!("  Device: {device_name} ({platform})").dimmed()
            );
            println!(
                "{}",
                format!(
                    "  You can now run commands like: bifrost remote status --client-id {short_id}"
                )
                .dimmed()
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

async fn handle_disconnect(
    caller: &CallerRelayClient,
    connections: &[LocalConnection],
    client_id: Option<&str>,
    all: bool,
    grant_id: Option<&str>,
    caller_fingerprint: &str,
) -> bifrost_core::Result<()> {
    if let Some(gid) = grant_id {
        caller.delete_grant(gid, caller_fingerprint).await?;
        let mut conns = connections.to_vec();
        conns.retain(|c| c.grant_id != gid);
        save_connections(&conns)?;
        println!(
            "{}",
            format!("✓ Grant {} revoked.", &gid[..gid.len().min(8)]).bright_green()
        );
        return Ok(());
    }

    if all {
        if connections.is_empty() {
            println!("{}", "No saved connections.".dimmed());
            return Ok(());
        }

        println!(
            "{}",
            format!("Revoking {} connection(s)…", connections.len()).bright_yellow()
        );

        let mut remaining = connections.to_vec();
        let mut deleted = 0usize;
        let total = remaining.len();
        let mut to_remove = Vec::new();

        for (i, conn) in connections.iter().enumerate() {
            let short_id = &conn.grant_id[..conn.grant_id.len().min(12)];
            match caller
                .delete_grant(&conn.grant_id, caller_fingerprint)
                .await
            {
                Ok(()) => {
                    deleted += 1;
                    to_remove.push(i);
                    println!(
                        "  {} {} ({})",
                        "✓".bright_green(),
                        short_id,
                        conn.device_name
                    );
                }
                Err(e) => {
                    eprintln!(
                        "  {} {} ({}) — {}",
                        "✗".bright_red(),
                        short_id,
                        conn.device_name,
                        e
                    );
                }
            }
        }

        for i in to_remove.into_iter().rev() {
            remaining.remove(i);
        }
        save_connections(&remaining)?;

        println!(
            "{}",
            format!("Revoked {deleted}/{total} connection(s).").bright_green()
        );
        return Ok(());
    }

    let conn = resolve_local_connection(connections, client_id)?;
    let short_id = &conn.grant_id[..conn.grant_id.len().min(12)];

    caller
        .delete_grant(&conn.grant_id, caller_fingerprint)
        .await?;

    let mut conns = connections.to_vec();
    conns.retain(|c| {
        !(c.client_instance_id == conn.client_instance_id && c.relay_url == conn.relay_url)
    });
    save_connections(&conns)?;

    println!(
        "{}",
        format!(
            "✓ Disconnected from {} (grant: {short_id})",
            conn.device_name
        )
        .bright_green()
    );
    Ok(())
}

fn build_remote_command(action: &RemoteCommands) -> (String, Option<String>) {
    match action {
        RemoteCommands::Connect { .. } => unreachable!("connect handled separately"),
        RemoteCommands::Disconnect { .. } => unreachable!("disconnect handled separately"),
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
                    "query": keyword,
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

struct CallerRelayClient {
    http: reqwest::Client,
    base_url: String,
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
    pair_code: String,
    caller_info: CallerInfo,
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
    client_instance_id: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    grant_mode: Option<String>,
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
    caller_fingerprint: String,
    command: RemoteCommand,
    command_summary: CommandSummary,
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
    fn new(base_url: &str) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build caller http client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    async fn delete_grant(
        &self,
        grant_id: &str,
        caller_fingerprint: &str,
    ) -> bifrost_core::Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/grants/{}?caller_fingerprint={}",
            self.base_url,
            grant_id,
            urlencoding::encode(caller_fingerprint),
        );
        let response = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("delete grant failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "delete grant failed with status {status}: {}",
                truncate(&body, 500)
            )));
        }
        Ok(())
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
                                                    let client_instance_id = v.get("client_instance_id")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let device_name = v.get("device_name")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let platform = v.get("platform")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let grant_mode = v.get("grant_mode")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());

                                                    if status == "approved" || status == "rejected" || status == "expired" || status == "cancelled" {
                                                        return Ok(PairingWatchResult {
                                                            status,
                                                            grant_id,
                                                            client_instance_id,
                                                            device_name,
                                                            platform,
                                                            grant_mode,
                                                        });
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
        let mut timeout = Box::pin(tokio::time::sleep(Duration::from_secs(
            CALL_EVENT_TIMEOUT_SECS,
        )));

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();
        let mut result = CallResult::default();
        let mut stdout_parts: Vec<String> = Vec::new();
        let mut seen_frame_seqs: HashSet<u64> = HashSet::new();
        let mut exit_received = false;

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    if exit_received {
                        debug!("grace timeout after exit, no late frame arrived");
                        result.stdout = Some(stdout_parts.join(""));
                        return Ok(result);
                    }
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
                                                            let seq = envelope.get("seq").and_then(|s| s.as_u64()).unwrap_or(0);
                                                            if seen_frame_seqs.insert(seq) {
                                                                if let Some(ct) = envelope.get("ciphertext").and_then(|c| c.as_str()) {
                                                                    stdout_parts.push(ct.to_string());
                                                                }
                                                            } else {
                                                                debug!(seq = seq, "skipping duplicate frame");
                                                            }
                                                        }
                                                    } else if let Some(ct) = v.get("ciphertext").and_then(|c| c.as_str()) {
                                                        stdout_parts.push(ct.to_string());
                                                    }
                                                }
                                                if exit_received {
                                                    debug!("late frame received after exit, returning");
                                                    result.stdout = Some(stdout_parts.join(""));
                                                    return Ok(result);
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
                                                if !stdout_parts.is_empty() {
                                                    result.stdout = Some(stdout_parts.join(""));
                                                    return Ok(result);
                                                }
                                                debug!("exit received with empty stdout, waiting for delayed frame");
                                                exit_received = true;
                                                timeout = Box::pin(tokio::time::sleep(Duration::from_secs(3)));
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
