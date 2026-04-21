use std::collections::HashSet;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use bifrost_core::{direct_reqwest_client_builder, BifrostError};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use futures::StreamExt;
use ring::digest::{digest, SHA256};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::cli::{RemoteCommands, RemoteTrafficCommands};

const PAIRING_WATCH_TIMEOUT_SECS: u64 = 180;
const CALL_EVENT_TIMEOUT_SECS: u64 = 120;
const CANCEL_SETTLE_TIMEOUT_SECS: u64 = 10;
const CANCEL_SETTLE_TOTAL_TIMEOUT_SECS: u64 = 15;
const CALLER_USER_AGENT: &str = "bifrost-cli-remote";
const CONNECTIONS_FILE: &str = "remote-connections.json";
const START_PAIRING_OVERLOAD_RETRY_DELAYS_MS: [u64; 3] = [300, 700, 1500];
const CANCEL_SETTLE_RETRY_DELAYS_MS: [u64; 4] = [200, 500, 1000, 2000];
const SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
const BIFROST_KEY_BEGIN: &str = "-----BEGIN BIFROST KEY-----";
const BIFROST_KEY_END: &str = "-----END BIFROST KEY-----";
const PKCS8_KEY_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PKCS8_KEY_END: &str = "-----END PRIVATE KEY-----";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_key_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_key_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_code: Option<String>,
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
    let hostname = get_hostname();
    let username = get_username();
    let caller_fingerprint = generate_caller_fingerprint(&username, &hostname);
    let caller_info = CallerInfo {
        fingerprint: caller_fingerprint.clone(),
        display_name: Some(hostname.clone()),
        user_agent: Some(CALLER_USER_AGENT.to_string()),
        platform: Some(std::env::consts::OS.to_string()),
        hostname: Some(hostname),
        username: Some(username),
    };

    if let RemoteCommands::Connect {
        pair_code,
        ssh_key,
        device_code,
    } = &opts.action
    {
        if let Some(ssh_key) = ssh_key {
            return handle_connect_with_ssh(
                &caller,
                ssh_key,
                device_code.as_deref(),
                &caller_info,
                &opts.relay_url,
            )
            .await;
        }

        let pair_code = pair_code.as_deref().ok_or_else(|| {
            BifrostError::Config(
                "either <pair_code> or --ssh-key is required for `bifrost remote connect`"
                    .to_string(),
            )
        })?;
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
        )
        .await;
    }

    let conn = resolve_local_connection(&connections, opts.client_id.as_deref())?;
    let caller_fingerprint = if conn.caller_fingerprint.is_empty() {
        caller_fingerprint
    } else {
        conn.caller_fingerprint.clone()
    };

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

    let stream_stdout = should_stream_remote_command(&command);
    let result = tokio::select! {
        result = caller.subscribe_call_events(
            &call_result.call_id,
            &call_result.relay_token,
            stream_stdout,
            CALL_EVENT_TIMEOUT_SECS,
        ) => result?,
        _ = wait_for_remote_call_cancel_signal() => {
            eprintln!("{}", "→ Cancellation requested, notifying remote device...".bright_yellow());
            let cancel_requested = match caller.cancel_call(&call_result.call_id, &call_result.relay_token).await {
                Ok(()) => true,
                Err(err) => {
                    warn!(call_id = %call_result.call_id, error = %err, "failed to send remote call cancel");
                    false
                }
            };
            match tokio::time::timeout(
                Duration::from_secs(CANCEL_SETTLE_TOTAL_TIMEOUT_SECS),
                caller.settle_cancelled_call(
                    &call_result.call_id,
                    &call_result.relay_token,
                    stream_stdout,
                    cancel_requested,
                ),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!(
                        call_id = %call_result.call_id,
                        "cancel settle exceeded total timeout, synthesizing cancelled result"
                    );
                    synthesized_cancelled_result()
                }
            }
        }
    };

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

    let pairing_result = start_pairing_with_retry(
        caller,
        &StartPairingRequest {
            pair_code: pair_code.to_string(),
            caller_info: caller_info.clone(),
        },
    )
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
                auth_method: Some("pair_code".to_string()),
                ssh_key_fingerprint: None,
                ssh_key_source: None,
                device_code: None,
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
            println!(
                "{}",
                "  Pair codes are one-time only. After connect succeeds, use remote status or other read-only remote commands instead of reconnecting with the same code."
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

async fn handle_connect_with_ssh(
    caller: &CallerRelayClient,
    ssh_key: &str,
    device_code_override: Option<&str>,
    caller_info: &CallerInfo,
    relay_url: &str,
) -> bifrost_core::Result<()> {
    let loaded_key = load_ssh_key(ssh_key, device_code_override)?;

    println!(
        "{}",
        format!(
            "→ Initiating SSH authorization with device code {}...",
            loaded_key.device_code.bright_cyan()
        )
        .dimmed()
    );

    let challenge = caller
        .request_ssh_challenge(&loaded_key.device_code)
        .await?;
    let timestamp = now_millis();
    let payload = build_ssh_connect_signature_payload(
        &challenge.challenge,
        &challenge.challenge_id,
        &loaded_key.device_code,
        timestamp,
    );
    let signature = base64::engine::general_purpose::STANDARD
        .encode(loaded_key.key_pair.sign(payload.as_bytes()).as_ref());

    let response = caller
        .ssh_connect(&SshConnectRequest {
            device_code: loaded_key.device_code.clone(),
            challenge_id: challenge.challenge_id,
            signature,
            timestamp,
            caller_info: Some(caller_info.clone()),
        })
        .await?;

    println!(
        "{}",
        "⏳ Waiting for SSH authorization confirmation on the remote device...".bright_yellow()
    );

    let result = caller
        .watch_ssh_connect_result(&response.connect_id, &response.relay_token)
        .await?;

    match result.status.as_str() {
        "approved" => {
            let grant_id = result.grant_id.unwrap_or_else(|| "unknown".to_string());
            let client_instance_id = result.client_instance_id.ok_or_else(|| {
                BifrostError::Config(
                    "ssh connect succeeded but relay did not return client_instance_id".to_string(),
                )
            })?;
            let grant_mode = result.grant_mode.unwrap_or_else(|| "permanent".to_string());
            let caller_fingerprint = result
                .caller_fingerprint
                .unwrap_or_else(|| loaded_key.ssh_key_fingerprint.clone());

            let new_conn = LocalConnection {
                client_instance_id: client_instance_id.clone(),
                device_name: format!("SSH {}", loaded_key.device_code),
                platform: "unknown".to_string(),
                relay_url: relay_url.to_string(),
                grant_id: grant_id.clone(),
                grant_mode,
                caller_fingerprint,
                connected_at: now_millis(),
                auth_method: Some("ssh_publickey".to_string()),
                ssh_key_fingerprint: Some(loaded_key.ssh_key_fingerprint.clone()),
                ssh_key_source: Some(loaded_key.source_label.clone()),
                device_code: Some(loaded_key.device_code.clone()),
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
                    "✓ Connected with SSH key (grant: {})",
                    &grant_id[..grant_id.len().min(8)]
                )
                .bright_green()
            );
            println!(
                "{}",
                format!("  Device Code: {}", loaded_key.device_code).dimmed()
            );
            println!(
                "{}",
                format!("  Key Source: {}", loaded_key.source_label).dimmed()
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
        "rejected" => Err(BifrostError::Config(
            result
                .reason
                .unwrap_or_else(|| "ssh connect rejected".to_string()),
        )),
        other => Err(BifrostError::Config(format!(
            "ssh connect failed with status: {other}"
        ))),
    }
}

async fn start_pairing_with_retry(
    caller: &CallerRelayClient,
    req: &StartPairingRequest,
) -> bifrost_core::Result<StartPairingResponse> {
    let mut retry_idx = 0usize;

    loop {
        match caller.start_pairing(req).await {
            Ok(result) => return Ok(result),
            Err(err) if is_retryable_start_pairing_overload(&err) => {
                if retry_idx >= START_PAIRING_OVERLOAD_RETRY_DELAYS_MS.len() {
                    return Err(start_pairing_overload_error());
                }

                let delay_ms = START_PAIRING_OVERLOAD_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;

                warn!(
                    attempt = retry_idx,
                    delay_ms, "relay overload protection triggered during start_pairing, retrying"
                );
                println!(
                    "{}",
                    format!("↻ Relay is temporarily busy, retrying pairing in {delay_ms}ms...")
                        .dimmed()
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_retryable_start_pairing_overload(err: &BifrostError) -> bool {
    let BifrostError::Network(message) = err else {
        return false;
    };

    let lower = message.to_ascii_lowercase();
    lower.contains("start_pairing failed with status 503") && lower.contains("overload-protect")
}

fn start_pairing_overload_error() -> BifrostError {
    BifrostError::Network(
        "pairing service is temporarily busy (relay overload protection triggered). Please wait a few seconds and run `bifrost remote connect <pair-code>` again.".to_string(),
    )
}

async fn handle_disconnect(
    caller: &CallerRelayClient,
    connections: &[LocalConnection],
    client_id: Option<&str>,
    all: bool,
    grant_id: Option<&str>,
) -> bifrost_core::Result<()> {
    if let Some(gid) = grant_id {
        let caller_fingerprint = connections
            .iter()
            .find(|conn| conn.grant_id == gid)
            .map(|conn| conn.caller_fingerprint.as_str())
            .unwrap_or("");
        let outcome = caller.delete_grant(gid, caller_fingerprint).await?;
        let mut conns = connections.to_vec();
        conns.retain(|c| c.grant_id != gid);
        save_connections(&conns)?;
        let short_id = &gid[..gid.len().min(8)];
        match outcome {
            DeleteGrantOutcome::Deleted => {
                println!("{}", format!("✓ Grant {short_id} revoked.").bright_green())
            }
            DeleteGrantOutcome::AlreadyMissing => println!(
                "{}",
                format!("✓ Grant {short_id} was already missing on relay; local record removed.")
                    .bright_green()
            ),
        }
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
                .delete_grant(&conn.grant_id, &conn.caller_fingerprint)
                .await
            {
                Ok(DeleteGrantOutcome::Deleted) => {
                    deleted += 1;
                    to_remove.push(i);
                    println!(
                        "  {} {} ({})",
                        "✓".bright_green(),
                        short_id,
                        conn.device_name
                    );
                }
                Ok(DeleteGrantOutcome::AlreadyMissing) => {
                    deleted += 1;
                    to_remove.push(i);
                    println!(
                        "  {} {} ({}) — relay grant already missing, local record removed",
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

    let outcome = caller
        .delete_grant(&conn.grant_id, &conn.caller_fingerprint)
        .await?;

    let mut conns = connections.to_vec();
    conns.retain(|c| {
        !(c.client_instance_id == conn.client_instance_id && c.relay_url == conn.relay_url)
    });
    save_connections(&conns)?;

    match outcome {
        DeleteGrantOutcome::Deleted => println!(
            "{}",
            format!(
                "✓ Disconnected from {} (grant: {short_id})",
                conn.device_name
            )
            .bright_green()
        ),
        DeleteGrantOutcome::AlreadyMissing => println!(
            "{}",
            format!(
                "✓ Disconnected from {} (grant: {short_id}, already missing on relay; local record removed)",
                conn.device_name
            )
            .bright_green()
        ),
    }
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

fn should_stream_remote_command(command: &str) -> bool {
    matches!(command, "search.get" | "traffic.search")
}

async fn wait_for_remote_call_cancel_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sighup = signal(SignalKind::hangup()).expect("failed to install SIGHUP handler");

        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
            _ = sighup.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn generate_caller_fingerprint(user: &str, machine_id: &str) -> String {
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

fn classify_delete_grant_failure(
    status: reqwest::StatusCode,
    body: &str,
) -> Option<DeleteGrantOutcome> {
    if status == reqwest::StatusCode::NOT_FOUND || body.contains("grant_not_found") {
        return Some(DeleteGrantOutcome::AlreadyMissing);
    }
    None
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

    if result.cancelled {
        eprintln!(
            "{}",
            format!("Remote command '{}' cancelled by caller.", command).bright_yellow()
        );
        return;
    }

    if result.exit_code != 0 {
        eprintln!(
            "{}",
            format!(
                "Remote command '{}' exited with code {}{}",
                command,
                result.exit_code,
                if result.stderr.is_none() {
                    " (no error details received from remote)"
                } else {
                    ""
                }
            )
            .bright_red()
        );
    }
}

struct CallerRelayClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteGrantOutcome {
    Deleted,
    AlreadyMissing,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
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
struct SshChallengeRequest {
    device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshChallengeResponse {
    challenge_id: String,
    challenge: String,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectRequest {
    device_code: String,
    challenge_id: String,
    signature: String,
    timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller_info: Option<CallerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectResponse {
    connect_id: String,
    relay_token: String,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectResult {
    status: String,
    #[serde(default)]
    grant_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    caller_fingerprint: Option<String>,
    #[serde(default)]
    grant_mode: Option<String>,
    #[serde(default)]
    client_instance_id: Option<String>,
}

struct LoadedSshKey {
    key_pair: Ed25519KeyPair,
    device_code: String,
    ssh_key_fingerprint: String,
    source_label: String,
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
    cancelled: bool,
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
    ) -> bifrost_core::Result<DeleteGrantOutcome> {
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
        if status.is_success() {
            return Ok(DeleteGrantOutcome::Deleted);
        }

        let body = response.text().await.unwrap_or_default();
        if classify_delete_grant_failure(status, &body) == Some(DeleteGrantOutcome::AlreadyMissing)
        {
            return Ok(DeleteGrantOutcome::AlreadyMissing);
        }

        Err(BifrostError::Network(format!(
            "delete grant failed with status {status}: {}",
            truncate(&body, 500)
        )))
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

    async fn cancel_call(&self, call_id: &str, relay_token: &str) -> bifrost_core::Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/cancel",
            self.base_url, call_id
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("cancel call failed: {e}")))?;

        let _ = self.parse_response_data(response, "cancel_call").await?;
        Ok(())
    }

    async fn request_ssh_challenge(
        &self,
        device_code: &str,
    ) -> bifrost_core::Result<SshChallengeResponse> {
        let url = format!("{}/v4/remote-invoke/ssh/challenge", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&SshChallengeRequest {
                device_code: device_code.to_string(),
            })
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("ssh challenge failed: {e}")))?;

        self.parse_response_typed(response, "ssh_challenge").await
    }

    async fn ssh_connect(
        &self,
        req: &SshConnectRequest,
    ) -> bifrost_core::Result<SshConnectResponse> {
        let url = format!("{}/v4/remote-invoke/ssh/connect", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("ssh connect failed: {e}")))?;

        self.parse_response_typed(response, "ssh_connect").await
    }

    async fn watch_ssh_connect_result(
        &self,
        connect_id: &str,
        relay_token: &str,
    ) -> bifrost_core::Result<SshConnectResult> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, connect_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build ssh connect sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("watch ssh connect failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "watch_ssh_connect_result returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let timeout = tokio::time::sleep(Duration::from_secs(SSH_CONNECT_TIMEOUT_SECS));
        tokio::pin!(timeout);

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    return Err(BifrostError::Config("ssh connect approval timed out".to_string()));
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
                                    if event_name == "ssh_connect_result" && !data_buf.is_empty() {
                                        return serde_json::from_str(&data_buf).map_err(|e| {
                                            BifrostError::Network(format!(
                                                "parse ssh_connect_result failed: {e}"
                                            ))
                                        });
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
                            return Err(BifrostError::Network(format!("ssh connect SSE error: {e}")));
                        }
                        None => {
                            return Err(BifrostError::Network(
                                "ssh connect stream closed unexpectedly".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn subscribe_call_events(
        &self,
        call_id: &str,
        relay_token: &str,
        stream_stdout: bool,
        timeout_secs: u64,
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
        let mut timeout = Box::pin(tokio::time::sleep(Duration::from_secs(timeout_secs)));

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
                        if !stream_stdout {
                            result.stdout = Some(stdout_parts.join(""));
                        }
                        return Ok(result);
                    }
                    warn!("call events timed out");
                    if !stream_stdout && stdout_parts.is_empty() {
                        return Err(BifrostError::Config("remote call timed out waiting for response".to_string()));
                    }
                    if !stream_stdout {
                        result.stdout = Some(stdout_parts.join(""));
                    }
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
                                                                    if stream_stdout {
                                                                        print!("{ct}");
                                                                        std::io::stdout().flush().ok();
                                                                    } else {
                                                                        stdout_parts.push(ct.to_string());
                                                                    }
                                                                }
                                                            } else {
                                                                debug!(seq = seq, "skipping duplicate frame");
                                                            }
                                                        }
                                                    } else if let Some(ct) = v.get("ciphertext").and_then(|c| c.as_str()) {
                                                        if stream_stdout {
                                                            print!("{ct}");
                                                            std::io::stdout().flush().ok();
                                                        } else {
                                                            stdout_parts.push(ct.to_string());
                                                        }
                                                    }
                                                }
                                                if exit_received {
                                                    debug!("late frame received after exit");
                                                    timeout = Box::pin(tokio::time::sleep(Duration::from_secs(3)));
                                                }
                                            }
                                            "exit" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    result.exit_code = v.get("exit_code")
                                                        .and_then(|c| c.as_i64())
                                                        .unwrap_or(0) as i32;
                                                    result.duration_ms = v.get("duration_ms")
                                                        .and_then(|d| d.as_u64());
                                                    result.stderr = v.get("stderr")
                                                        .and_then(|s| s.as_str())
                                                        .filter(|s| !s.is_empty())
                                                        .map(|s| s.to_string());
                                                }
                                                if !stream_stdout && !stdout_parts.is_empty() {
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
                                                if !stream_stdout {
                                                    result.stdout = Some(stdout_parts.join(""));
                                                }
                                                return Ok(result);
                                            }
                                            "status" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    if let Some(status) = parse_call_terminal_status(&v) {
                                                        if status == "cancelled" {
                                                            result.exit_code = 130;
                                                            result.stderr = Some("remote call cancelled by caller".to_string());
                                                            result.cancelled = true;
                                                            if !stream_stdout {
                                                                result.stdout = Some(stdout_parts.join(""));
                                                            }
                                                            return Ok(result);
                                                        }
                                                    }
                                                }
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
                            if !stream_stdout {
                                result.stdout = Some(stdout_parts.join(""));
                            }
                            return Ok(result);
                        }
                    }
                }
            }
        }
    }

    async fn settle_cancelled_call(
        &self,
        call_id: &str,
        relay_token: &str,
        stream_stdout: bool,
        cancel_requested: bool,
    ) -> bifrost_core::Result<CallResult> {
        if !cancel_requested {
            return self
                .subscribe_call_events(call_id, relay_token, stream_stdout, CALL_EVENT_TIMEOUT_SECS)
                .await;
        }

        for (attempt, delay_ms) in std::iter::once(0)
            .chain(CANCEL_SETTLE_RETRY_DELAYS_MS)
            .enumerate()
        {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            match self
                .subscribe_call_events(
                    call_id,
                    relay_token,
                    stream_stdout,
                    CANCEL_SETTLE_TIMEOUT_SECS,
                )
                .await
            {
                Ok(result) if result.cancelled => return Ok(result),
                Ok(result) if is_ambiguous_cancel_settle_result(&result) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        "cancel settle timed out without terminal event, synthesizing cancelled result"
                    );
                    return Ok(synthesized_cancelled_result());
                }
                Ok(result) => return Ok(result),
                Err(err) if is_retryable_cancel_settle_error(&err) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        error = %err,
                        "cancel settle hit retryable relay throttling, retrying"
                    );
                }
                Err(err) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        error = %err,
                        "cancel settle failed after relay accepted cancel, synthesizing cancelled result"
                    );
                    return Ok(synthesized_cancelled_result());
                }
            }
        }

        warn!(
            call_id = %call_id,
            "cancel settle exhausted retries, synthesizing cancelled result"
        );
        Ok(synthesized_cancelled_result())
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

fn load_ssh_key(
    spec: &str,
    device_code_override: Option<&str>,
) -> bifrost_core::Result<LoadedSshKey> {
    let (raw, source_label, file_path) = read_ssh_key_source(spec)?;
    if let Some(path) = file_path.as_deref() {
        warn_if_ssh_key_permissions_are_too_open(path);
    }

    let (pkcs8, embedded_device_code) = parse_ssh_private_key_bytes(&raw)?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| BifrostError::Config("parse ed25519 private key failed".to_string()))?;
    let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
    let device_code = device_code_override
        .map(ToOwned::to_owned)
        .or(embedded_device_code)
        .unwrap_or_else(|| derive_device_code(&public_key_der));
    let ssh_key_fingerprint = sha256_hex(&public_key_der);

    Ok(LoadedSshKey {
        key_pair,
        device_code,
        ssh_key_fingerprint,
        source_label,
    })
}

fn read_ssh_key_source(spec: &str) -> bifrost_core::Result<(Vec<u8>, String, Option<PathBuf>)> {
    if let Some(env_name) = spec.strip_prefix("env:") {
        let value = std::env::var(env_name).map_err(|_| {
            BifrostError::Config(format!("environment variable `{env_name}` is not set"))
        })?;
        return Ok((value.into_bytes(), format!("env:{env_name}"), None));
    }

    if spec == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read ssh key from stdin: {e}"
            )))
        })?;
        return Ok((buf, "stdin".to_string(), None));
    }

    let path = PathBuf::from(spec);
    let bytes = std::fs::read(&path).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "read ssh key {}: {e}",
            path.display()
        )))
    })?;
    Ok((bytes, path.display().to_string(), Some(path)))
}

fn parse_ssh_private_key_bytes(raw: &[u8]) -> bifrost_core::Result<(Vec<u8>, Option<String>)> {
    if let Ok(text) = std::str::from_utf8(raw) {
        if let Some((pkcs8, device_code)) = parse_bifrost_key_file(text)? {
            return Ok((pkcs8, Some(device_code)));
        }
        if let Some(pkcs8) = parse_pem_block(text, PKCS8_KEY_BEGIN, PKCS8_KEY_END)? {
            return Ok((pkcs8, None));
        }
    }

    Ok((raw.to_vec(), None))
}

fn parse_bifrost_key_file(text: &str) -> bifrost_core::Result<Option<(Vec<u8>, String)>> {
    let Some(body) = text
        .split_once(BIFROST_KEY_BEGIN)
        .and_then(|(_, rest)| rest.split_once(BIFROST_KEY_END).map(|(body, _)| body))
    else {
        return Ok(None);
    };

    let mut device_code = None;
    let mut base64_lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Device-Code:") {
            device_code = Some(value.trim().to_string());
            continue;
        }
        base64_lines.push(trimmed);
    }

    let device_code = device_code.ok_or_else(|| {
        BifrostError::Config("Bifrost SSH key file is missing `Device-Code:` header".to_string())
    })?;
    let pkcs8 = base64::engine::general_purpose::STANDARD
        .decode(base64_lines.join(""))
        .map_err(|e| BifrostError::Config(format!("decode Bifrost SSH key file failed: {e}")))?;
    Ok(Some((pkcs8, device_code)))
}

fn parse_pem_block(
    text: &str,
    begin_marker: &str,
    end_marker: &str,
) -> bifrost_core::Result<Option<Vec<u8>>> {
    let Some(body) = text
        .split_once(begin_marker)
        .and_then(|(_, rest)| rest.split_once(end_marker).map(|(body, _)| body))
    else {
        return Ok(None);
    };

    let encoded: String = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| BifrostError::Config(format!("decode PEM private key failed: {e}")))?;
    Ok(Some(decoded))
}

fn build_ssh_connect_signature_payload(
    challenge: &str,
    challenge_id: &str,
    device_code: &str,
    timestamp: u64,
) -> String {
    format!(
        "{{\"challenge\":{},\"challenge_id\":{},\"device_code\":{},\"timestamp\":{timestamp}}}",
        serde_json::to_string(challenge).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(challenge_id).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(device_code).unwrap_or_else(|_| "\"\"".to_string()),
    )
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn ed25519_public_key_to_spki_der(public_key: &[u8]) -> Vec<u8> {
    const ED25519_SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];

    let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + public_key.len());
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(public_key);
    der
}

fn derive_device_code(public_key_der: &[u8]) -> String {
    let digest = digest(&SHA256, public_key_der);
    let prefix = &digest.as_ref()[..8];
    let mut out = String::from("BF-");
    for byte in prefix {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = digest(&SHA256, data);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn warn_if_ssh_key_permissions_are_too_open(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "{}",
                    format!(
                        "Warning: SSH key file {} has permissions {:o}; consider chmod 600.",
                        path.display(),
                        mode
                    )
                    .bright_yellow()
                );
            }
        }
    }
}

fn parse_call_terminal_status(payload: &Value) -> Option<&str> {
    payload
        .get("status")
        .and_then(|status| status.as_str())
        .filter(|status| matches!(*status, "cancelled" | "completed" | "failed" | "timeout"))
}

fn synthesized_cancelled_result() -> CallResult {
    CallResult {
        exit_code: 130,
        stdout: None,
        stderr: Some("remote call cancelled by caller".to_string()),
        duration_ms: None,
        cancelled: true,
    }
}

fn is_retryable_cancel_settle_error(err: &BifrostError) -> bool {
    match err {
        BifrostError::Network(message) => {
            message.contains("call events returned 429")
                || message.contains("cancel_call failed with status 429")
                || message.contains("too many requests, retry after")
        }
        _ => false,
    }
}

fn is_ambiguous_cancel_settle_result(result: &CallResult) -> bool {
    !result.cancelled
        && result.exit_code == 0
        && result.stderr.is_none()
        && result.duration_ms.is_none()
        && result.stdout.is_none()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...(truncated)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_remote_command_for_search_uses_streaming_command() {
        let (command, args_json) = build_remote_command(&RemoteCommands::Search {
            keyword: "nextoncall".to_string(),
            limit: 7,
        });

        assert_eq!(command, "search.get");
        let args = args_json.expect("search args_json should exist");
        let parsed: Value = serde_json::from_str(&args).expect("search args_json should be valid");
        assert_eq!(parsed["query"], "nextoncall");
        assert_eq!(parsed["limit"], 7);
    }

    #[test]
    fn test_build_remote_command_for_traffic_search_uses_streaming_command() {
        let (command, args_json) = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Search {
                keyword: "token".to_string(),
                limit: 3,
            },
        });

        assert_eq!(command, "traffic.search");
        let args = args_json.expect("traffic search args_json should exist");
        let parsed: Value =
            serde_json::from_str(&args).expect("traffic search args_json should be valid");
        assert_eq!(parsed["query"], "token");
        assert_eq!(parsed["limit"], 3);
    }

    #[test]
    fn test_should_stream_remote_command_marks_search_variants_only() {
        assert!(should_stream_remote_command("search.get"));
        assert!(should_stream_remote_command("traffic.search"));
        assert!(!should_stream_remote_command("status"));
        assert!(!should_stream_remote_command("traffic.get"));
    }

    #[test]
    fn test_retryable_start_pairing_overload_matches_503_overload_protect() {
        let err = BifrostError::Network(
            "start_pairing failed with status 503 Service Unavailable: overload-protect triggered"
                .to_string(),
        );

        assert!(is_retryable_start_pairing_overload(&err));
    }

    #[test]
    fn test_retryable_start_pairing_overload_rejects_non_overload_errors() {
        let invalid_code = BifrostError::Network(
            "start_pairing failed with status 400 Bad Request: pair_code_expired".to_string(),
        );
        let unrelated_503 = BifrostError::Network(
            "watch_pairing failed with status 503 Service Unavailable: upstream maintenance"
                .to_string(),
        );

        assert!(!is_retryable_start_pairing_overload(&invalid_code));
        assert!(!is_retryable_start_pairing_overload(&unrelated_503));
    }

    #[test]
    fn test_classify_delete_grant_failure_treats_404_as_already_missing() {
        let outcome = classify_delete_grant_failure(
            reqwest::StatusCode::NOT_FOUND,
            "{\"code\":404,\"message\":\"grant_not_found\"}",
        );

        assert_eq!(outcome, Some(DeleteGrantOutcome::AlreadyMissing));
    }

    #[test]
    fn test_classify_delete_grant_failure_rejects_other_errors() {
        let outcome = classify_delete_grant_failure(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "{\"code\":500,\"message\":\"relay_internal_error\"}",
        );

        assert_eq!(outcome, None);
    }

    #[test]
    fn test_start_pairing_overload_error_is_actionable() {
        let err = start_pairing_overload_error();
        let BifrostError::Network(message) = err else {
            panic!("expected network error");
        };

        assert!(message.contains("pairing service is temporarily busy"));
        assert!(message.contains("bifrost remote connect <pair-code>"));
    }

    #[test]
    fn test_parse_call_terminal_status_accepts_cancelled() {
        let payload = serde_json::json!({
            "call_id": "call-1",
            "status": "cancelled"
        });

        assert_eq!(parse_call_terminal_status(&payload), Some("cancelled"));
    }

    #[test]
    fn test_parse_call_terminal_status_rejects_streaming() {
        let payload = serde_json::json!({
            "call_id": "call-1",
            "status": "streaming"
        });

        assert_eq!(parse_call_terminal_status(&payload), None);
    }

    #[test]
    fn test_retryable_cancel_settle_error_matches_429_call_events() {
        let err = BifrostError::Network(
            "call events returned 429 Too Many Requests: {\"code\":-1,\"message\":\"too many requests, retry after 15s\"}".to_string(),
        );

        assert!(is_retryable_cancel_settle_error(&err));
    }

    #[test]
    fn test_ambiguous_cancel_settle_result_requires_synthesis() {
        let result = CallResult::default();

        assert!(is_ambiguous_cancel_settle_result(&result));
    }

    #[test]
    fn test_build_ssh_connect_signature_payload_matches_relay_shape() {
        let payload = build_ssh_connect_signature_payload(
            "challenge-value",
            "challenge-id",
            "BF-0123456789ABCDEF",
            1700000000000,
        );

        assert_eq!(
            payload,
            "{\"challenge\":\"challenge-value\",\"challenge_id\":\"challenge-id\",\"device_code\":\"BF-0123456789ABCDEF\",\"timestamp\":1700000000000}"
        );
    }

    #[test]
    fn test_parse_bifrost_key_file_extracts_pkcs8_and_device_code() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate pkcs8");
        let rendered = format!(
            "{BIFROST_KEY_BEGIN}\nDevice-Code: BF-0123456789ABCDEF\n{}\n{BIFROST_KEY_END}\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );

        let (parsed_pkcs8, device_code) = parse_bifrost_key_file(&rendered)
            .expect("parse should succeed")
            .expect("bifrost key should be detected");

        assert_eq!(device_code, "BF-0123456789ABCDEF");
        assert_eq!(parsed_pkcs8, pkcs8.as_ref());
    }
}
