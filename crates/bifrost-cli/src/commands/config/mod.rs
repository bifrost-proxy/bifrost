pub(crate) mod client;
mod display;
mod keys;
pub(crate) mod runtime;

use std::io::Write;
use std::path::PathBuf;

use bifrost_core::{BifrostError, Result};
use bifrost_storage::{
    DEFAULT_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS, MIN_TRAFFIC_MAX_RECORDS,
};

use crate::cli::ConfigCommands;
use client::{
    ConfigApiClient, PerformanceConfigResponse, ServerConfigResponse, TlsConfigResponse,
    UpdatePerformanceConfigRequest, UpdateServerConfigRequest, UpdateTlsConfigRequest,
    UpdateUserPassAccountRequest, UpdateUserPassRequest, WhitelistResponse,
};
use keys::{format_size, parse_bool, parse_list, parse_size, ConfigKey};

pub fn handle_config_command(action: Option<ConfigCommands>, host: &str, port: u16) -> Result<()> {
    let client = ConfigApiClient::new(host, port);

    match action {
        None => show_all_config(&client, false),
        Some(ConfigCommands::Show { json, section }) => {
            if let Some(sec) = section {
                show_section_config(&client, &sec, json)
            } else {
                show_all_config(&client, json)
            }
        }
        Some(ConfigCommands::Get { key, json }) => get_config_value(&client, &key, json),
        Some(ConfigCommands::Set { key, value }) => set_config_value(&client, &key, &value),
        Some(ConfigCommands::Add { key, value }) => add_list_item(&client, &key, &value),
        Some(ConfigCommands::Remove { key, value }) => remove_list_item(&client, &key, &value),
        Some(ConfigCommands::Reset { key, yes }) => reset_config(&client, &key, yes),
        Some(ConfigCommands::ClearCache { yes }) => clear_cache(&client, yes),
        Some(ConfigCommands::Disconnect { domain }) => disconnect_domain(&client, &domain),
        Some(ConfigCommands::Export { output, format }) => export_config(&client, output, &format),
        Some(ConfigCommands::DisconnectByApp { app }) => {
            let client = client::ConfigApiClient::new(host, port);
            client
                .disconnect_by_app(&app)
                .map_err(BifrostError::Config)?;
            println!("Disconnected connections for app: {}", app);
            Ok(())
        }
        Some(ConfigCommands::Performance) => {
            let client = client::ConfigApiClient::new(host, port);
            let overview = client.get_system_overview().map_err(BifrostError::Config)?;
            let sandbox = client.get_sandbox_config().map_err(BifrostError::Config)?;

            println!("Performance Overview");
            println!("====================");
            println!();

            if let Some(obj) = overview.as_object() {
                println!("System:");
                for (key, value) in obj {
                    if key.contains("store")
                        || key.contains("body")
                        || key.contains("frame")
                        || key.contains("writer")
                        || key.contains("payload")
                        || key.contains("memory")
                        || key.contains("size")
                        || key.contains("count")
                        || key.contains("db")
                    {
                        if let Some(n) = value.as_u64() {
                            println!("  {}: {}", key.replace('_', " "), n);
                        } else if let Some(s) = value.as_str() {
                            println!("  {}: {}", key.replace('_', " "), s);
                        }
                    }
                }
            }

            println!();
            println!("Sandbox Configuration:");
            println!(
                "{}",
                serde_json::to_string_pretty(&sandbox).unwrap_or_default()
            );

            Ok(())
        }
        Some(ConfigCommands::Websocket) => {
            let client = client::ConfigApiClient::new(host, port);
            let connections = client
                .get_websocket_connections()
                .map_err(BifrostError::Config)?;

            if let Some(arr) = connections.as_array() {
                if arr.is_empty() {
                    println!("No active WebSocket connections.");
                } else {
                    println!("Active WebSocket Connections ({}):", arr.len());
                    println!();
                    for conn in arr {
                        if let Some(url) = conn.get("url").and_then(|v| v.as_str()) {
                            println!("  URL: {}", url);
                        }
                        if let Some(id) = conn.get("id").and_then(|v| v.as_str()) {
                            println!("  ID: {}", id);
                        }
                        if let Some(state) = conn.get("state").and_then(|v| v.as_str()) {
                            println!("  State: {}", state);
                        }
                        if let Some(created) = conn.get("created_at").and_then(|v| v.as_str()) {
                            println!("  Created: {}", created);
                        }
                        println!();
                    }
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&connections).unwrap_or_default()
                );
            }
            Ok(())
        }
        Some(ConfigCommands::Connections) => {
            let client = client::ConfigApiClient::new(host, port);
            let result = client.list_connections().map_err(BifrostError::Config)?;

            if let Some(obj) = result.as_object() {
                let total = obj.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                let connections = obj.get("connections").and_then(|v| v.as_array());

                println!("Active Connections ({}):", total);
                println!();

                if let Some(conns) = connections {
                    if conns.is_empty() {
                        println!("  No active connections.");
                    } else {
                        let header = format!(
                            "  {:<36}  {:<40}  {:<6}  {:<10}  {}",
                            "REQ ID", "HOST", "PORT", "INTERCEPT", "APP"
                        );
                        println!("{header}");
                        println!("  {}", "-".repeat(110));
                        for conn in conns {
                            let req_id = conn.get("req_id").and_then(|v| v.as_str()).unwrap_or("-");
                            let conn_host =
                                conn.get("host").and_then(|v| v.as_str()).unwrap_or("-");
                            let conn_port = conn.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
                            let intercept = conn
                                .get("intercept_mode")
                                .and_then(|v| v.as_bool())
                                .map(|b| if b { "yes" } else { "no" })
                                .unwrap_or("-");
                            let app = conn
                                .get("client_app")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-");
                            println!(
                                "  {:<36}  {:<40}  {:<6}  {:<10}  {}",
                                req_id, conn_host, conn_port, intercept, app
                            );
                        }
                    }
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            }
            Ok(())
        }
        Some(ConfigCommands::Memory) => {
            let client = client::ConfigApiClient::new(host, port);
            let result = client
                .get_memory_diagnostics()
                .map_err(BifrostError::Config)?;

            if let Some(obj) = result.as_object() {
                println!("Memory Diagnostics");
                println!("==================");
                println!();

                if let Some(process) = obj.get("process") {
                    println!("Process:");
                    if let Some(pid) = process.get("pid").and_then(|v| v.as_u64()) {
                        println!("  PID:         {}", pid);
                    }
                    if let Some(rss) = process.get("rss_kib").and_then(|v| v.as_u64()) {
                        println!(
                            "  RSS:         {:.1} MiB ({:.2} GiB)",
                            rss as f64 / 1024.0 / 1024.0,
                            rss as f64 / 1024.0 / 1024.0 / 1024.0
                        );
                    }
                    if let Some(vms) = process.get("vms_kib").and_then(|v| v.as_u64()) {
                        println!(
                            "  Virtual:     {:.1} MiB ({:.2} GiB)",
                            vms as f64 / 1024.0 / 1024.0,
                            vms as f64 / 1024.0 / 1024.0 / 1024.0
                        );
                    }
                    if let Some(cpu) = process.get("cpu_usage_percent").and_then(|v| v.as_f64()) {
                        println!("  CPU:         {:.1}%", cpu);
                    }
                    if let Some(total) = process.get("system_total_kib").and_then(|v| v.as_u64()) {
                        println!(
                            "  System RAM:  {:.1} GiB",
                            total as f64 / 1024.0 / 1024.0 / 1024.0
                        );
                    }
                    println!();
                }

                if let Some(connections) = obj.get("connections") {
                    println!("Connections:");
                    if let Some(tunnel) = connections
                        .get("tunnel_registry_active")
                        .and_then(|v| v.as_u64())
                    {
                        println!("  Tunnel active:  {}", tunnel);
                    }
                    if let Some(sse) = connections.get("sse") {
                        if let Some(total) = sse.get("connections").and_then(|v| v.as_u64()) {
                            println!("  SSE total:      {}", total);
                        }
                        if let Some(open) = sse.get("open").and_then(|v| v.as_u64()) {
                            println!("  SSE open:       {}", open);
                        }
                    }
                    println!();
                }

                if let Some(stores) = obj.get("stores") {
                    println!("Stores:");
                    if let Some(body) = stores.get("body_store") {
                        if let Some(count) = body.get("file_count").and_then(|v| v.as_u64()) {
                            let size = body.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0);
                            println!(
                                "  Body store:     {} files, {} bytes ({:.1} MiB)",
                                count,
                                size,
                                size as f64 / 1024.0 / 1024.0
                            );
                        }
                    }
                    if let Some(frame) = stores.get("frame_store") {
                        if let Some(disk) = frame.get("disk") {
                            if let Some(count) =
                                disk.get("connection_count").and_then(|v| v.as_u64())
                            {
                                let size =
                                    disk.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0);
                                println!(
                                    "  Frame store:    {} connections, {} bytes ({:.1} MiB)",
                                    count,
                                    size,
                                    size as f64 / 1024.0 / 1024.0
                                );
                            }
                        }
                    }
                    if let Some(ws) = stores.get("ws_payload_store") {
                        if let Some(disk) = ws.get("disk") {
                            if let Some(count) = disk.get("file_count").and_then(|v| v.as_u64()) {
                                let size =
                                    disk.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0);
                                println!(
                                    "  WS payload:     {} files, {} bytes ({:.1} MiB)",
                                    count,
                                    size,
                                    size as f64 / 1024.0 / 1024.0
                                );
                            }
                        }
                    }
                    println!();
                }

                if let Some(traffic_db) = obj.get("traffic_db") {
                    println!("Traffic DB:");
                    if let Some(db) = traffic_db.get("db") {
                        if let Some(count) = db.get("record_count").and_then(|v| v.as_u64()) {
                            println!("  Records:     {}", count);
                        }
                        if let Some(size) = db.get("db_size_bytes").and_then(|v| v.as_u64()) {
                            println!(
                                "  DB size:     {} bytes ({:.1} MiB)",
                                size,
                                size as f64 / 1024.0 / 1024.0
                            );
                        }
                    }
                    if let Some(cache) = traffic_db.get("recent_cache") {
                        if let Some(size) = cache.get("entries").and_then(|v| v.as_u64()) {
                            println!("  Cache entries: {}", size);
                        }
                    }
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

fn show_all_config(client: &ConfigApiClient, json: bool) -> Result<()> {
    let server = client.get_server_config().map_err(BifrostError::Config)?;
    let tls = client.get_tls_config().map_err(BifrostError::Config)?;
    let perf = client
        .get_performance_config()
        .map_err(BifrostError::Config)?;
    let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;

    if json {
        let combined = serde_json::json!({
            "server": server,
            "tls": tls,
            "traffic": perf.traffic,
            "access": {
                "mode": whitelist.mode,
                "allow_lan": whitelist.allow_lan,
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&combined)
                .map_err(|e| BifrostError::Config(e.to_string()))?
        );
    } else {
        display::print_full_config(&server, &tls, &perf, &whitelist);
    }
    Ok(())
}

fn show_section_config(client: &ConfigApiClient, section: &str, json: bool) -> Result<()> {
    match section.to_lowercase().as_str() {
        "server" => {
            let server = client.get_server_config().map_err(BifrostError::Config)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&server)
                        .map_err(|e| BifrostError::Config(e.to_string()))?
                );
            } else {
                display::print_server_config(&server);
            }
        }
        "tls" => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&tls)
                        .map_err(|e| BifrostError::Config(e.to_string()))?
                );
            } else {
                display::print_tls_config(&tls);
            }
        }
        "traffic" => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&perf)
                        .map_err(|e| BifrostError::Config(e.to_string()))?
                );
            } else {
                display::print_traffic_config(&perf);
            }
        }
        "access" => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            if json {
                let access = serde_json::json!({
                    "mode": whitelist.mode,
                    "allow_lan": whitelist.allow_lan,
                    "whitelist": whitelist.whitelist,
                    "temporary_whitelist": whitelist.temporary_whitelist,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&access)
                        .map_err(|e| BifrostError::Config(e.to_string()))?
                );
            } else {
                display::print_access_config(&whitelist);
            }
        }
        _ => {
            return Err(BifrostError::Config(format!(
                "Unknown section: '{}'. Available sections: server, tls, traffic, access",
                section
            )));
        }
    }
    Ok(())
}

fn get_config_value(client: &ConfigApiClient, key: &str, json: bool) -> Result<()> {
    let config_key: ConfigKey = key.parse().map_err(BifrostError::Config)?;

    let value = match config_key {
        ConfigKey::ServerTimeoutSecs => {
            let server = client.get_server_config().map_err(BifrostError::Config)?;
            serde_json::Value::Number(server.timeout_secs.into())
        }
        ConfigKey::ServerHttp1MaxHeaderSize => {
            let server = client.get_server_config().map_err(BifrostError::Config)?;
            serde_json::Value::Number(server.http1_max_header_size.into())
        }
        ConfigKey::ServerHttp2MaxHeaderListSize => {
            let server = client.get_server_config().map_err(BifrostError::Config)?;
            serde_json::Value::Number(server.http2_max_header_list_size.into())
        }
        ConfigKey::ServerWebSocketHandshakeMaxHeaderSize => {
            let server = client.get_server_config().map_err(BifrostError::Config)?;
            serde_json::Value::Number(server.websocket_handshake_max_header_size.into())
        }
        ConfigKey::TlsEnabled => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(tls.enable_tls_interception)
        }
        ConfigKey::TlsUnsafeSsl => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(tls.unsafe_ssl)
        }
        ConfigKey::TlsDisconnectOnChange => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(tls.disconnect_on_config_change)
        }
        ConfigKey::TlsExclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.intercept_exclude)
        }
        ConfigKey::TlsInclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.intercept_include)
        }
        ConfigKey::TlsAppExclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.app_intercept_exclude)
        }
        ConfigKey::TlsAppInclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.app_intercept_include)
        }
        ConfigKey::TlsIpExclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.ip_intercept_exclude)
        }
        ConfigKey::TlsIpInclude => {
            let tls = client.get_tls_config().map_err(BifrostError::Config)?;
            serde_json::json!(tls.ip_intercept_include)
        }
        ConfigKey::TrafficMaxRecords => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.max_records.into())
        }
        ConfigKey::TrafficMaxDbSize => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.max_db_size_bytes.into())
        }
        ConfigKey::TrafficMaxBodySize => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.max_body_memory_size.into())
        }
        ConfigKey::TrafficMaxBufferSize => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.max_body_buffer_size.into())
        }
        ConfigKey::TrafficSuperPerformanceMode => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Bool(perf.traffic.super_performance_mode)
        }
        ConfigKey::TrafficRetentionDays => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.file_retention_days.into())
        }
        ConfigKey::TrafficSseStreamFlushBytes => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.sse_stream_flush_bytes.into())
        }
        ConfigKey::TrafficSseStreamFlushIntervalMs => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.sse_stream_flush_interval_ms.into())
        }
        ConfigKey::TrafficWsPayloadFlushBytes => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.ws_payload_flush_bytes.into())
        }
        ConfigKey::TrafficWsPayloadFlushIntervalMs => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.ws_payload_flush_interval_ms.into())
        }
        ConfigKey::TrafficWsPayloadMaxOpenFiles => {
            let perf = client
                .get_performance_config()
                .map_err(BifrostError::Config)?;
            serde_json::Value::Number(perf.traffic.ws_payload_max_open_files.into())
        }
        ConfigKey::AccessMode => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            serde_json::Value::String(whitelist.mode)
        }
        ConfigKey::AccessAllowLan => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(whitelist.allow_lan)
        }
        ConfigKey::AccessUserPassEnabled => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(whitelist.userpass.enabled)
        }
        ConfigKey::AccessUserPassAccounts => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            serde_json::to_value(whitelist.userpass.accounts)
                .map_err(|error| BifrostError::Config(error.to_string()))?
        }
        ConfigKey::AccessUserPassLoopbackRequiresAuth => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            serde_json::Value::Bool(whitelist.userpass.loopback_requires_auth)
        }
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|e| BifrostError::Config(e.to_string()))?
        );
    } else {
        display::print_config_value(&config_key, &value);
    }
    Ok(())
}

fn parse_userpass_accounts(
    value: &str,
) -> std::result::Result<Vec<UpdateUserPassAccountRequest>, String> {
    serde_json::from_str(value).map_err(|error| {
        format!(
            "access.userpass.accounts expects a JSON array like \
[{{\"username\":\"demo\",\"password\":\"secret\",\"enabled\":true}}]: {}",
            error
        )
    })
}

fn set_config_value(client: &ConfigApiClient, key: &str, value: &str) -> Result<()> {
    let config_key: ConfigKey = key.parse().map_err(BifrostError::Config)?;

    match config_key {
        ConfigKey::ServerTimeoutSecs => {
            let timeout_secs = value
                .parse::<u64>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            let req = UpdateServerConfigRequest {
                timeout_secs: Some(timeout_secs),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ server.timeout-secs set to {}", timeout_secs);
        }
        ConfigKey::ServerHttp1MaxHeaderSize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdateServerConfigRequest {
                http1_max_header_size: Some(size),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!(
                "✓ server.http1-max-header-size set to {}",
                format_size(size)
            );
        }
        ConfigKey::ServerHttp2MaxHeaderListSize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdateServerConfigRequest {
                http2_max_header_list_size: Some(size),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!(
                "✓ server.http2-max-header-list-size set to {}",
                format_size(size)
            );
        }
        ConfigKey::ServerWebSocketHandshakeMaxHeaderSize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdateServerConfigRequest {
                websocket_handshake_max_header_size: Some(size),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!(
                "✓ server.websocket-handshake-max-header-size set to {}",
                format_size(size)
            );
        }
        ConfigKey::TlsEnabled => {
            let enabled = parse_bool(value).map_err(BifrostError::Config)?;
            let req = UpdateTlsConfigRequest {
                enable_tls_interception: Some(enabled),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!(
                "✓ TLS interception {}",
                if enabled { "enabled" } else { "disabled" }
            );
        }
        ConfigKey::TlsUnsafeSsl => {
            let unsafe_ssl = parse_bool(value).map_err(BifrostError::Config)?;
            let req = UpdateTlsConfigRequest {
                unsafe_ssl: Some(unsafe_ssl),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ unsafe-ssl set to {}", unsafe_ssl);
        }
        ConfigKey::TlsDisconnectOnChange => {
            let disconnect = parse_bool(value).map_err(BifrostError::Config)?;
            let req = UpdateTlsConfigRequest {
                disconnect_on_config_change: Some(disconnect),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ disconnect-on-change set to {}", disconnect);
        }
        ConfigKey::TlsExclude => {
            let patterns = parse_list(value);
            let req = UpdateTlsConfigRequest {
                intercept_exclude: Some(patterns.clone()),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ TLS exclude list set to: {:?}", patterns);
        }
        ConfigKey::TlsInclude => {
            let patterns = parse_list(value);
            let req = UpdateTlsConfigRequest {
                intercept_include: Some(patterns.clone()),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ TLS include list set to: {:?}", patterns);
        }
        ConfigKey::TlsAppExclude => {
            let apps = parse_list(value);
            let req = UpdateTlsConfigRequest {
                app_intercept_exclude: Some(apps.clone()),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ TLS app exclude list set to: {:?}", apps);
        }
        ConfigKey::TlsAppInclude => {
            let apps = parse_list(value);
            let req = UpdateTlsConfigRequest {
                app_intercept_include: Some(apps.clone()),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ TLS app include list set to: {:?}", apps);
        }
        ConfigKey::TlsIpExclude => {
            let ips = parse_list(value);
            client
                .update_tls_config(&UpdateTlsConfigRequest {
                    ip_intercept_exclude: Some(ips.clone()),
                    ..Default::default()
                })
                .map_err(BifrostError::Config)?;
            println!("Set tls.ip-exclude = {:?}", ips);
        }
        ConfigKey::TlsIpInclude => {
            let ips = parse_list(value);
            client
                .update_tls_config(&UpdateTlsConfigRequest {
                    ip_intercept_include: Some(ips.clone()),
                    ..Default::default()
                })
                .map_err(BifrostError::Config)?;
            println!("Set tls.ip-include = {:?}", ips);
        }
        ConfigKey::TrafficMaxRecords => {
            let max = value
                .parse::<usize>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            if !(MIN_TRAFFIC_MAX_RECORDS..=MAX_TRAFFIC_MAX_RECORDS).contains(&max) {
                return Err(BifrostError::Config(format!(
                    "traffic.max-records must be between {} and {}",
                    MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS
                )));
            }
            let req = UpdatePerformanceConfigRequest {
                max_records: Some(max),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ max-records set to {}", max);
        }
        ConfigKey::TrafficMaxDbSize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                max_db_size_bytes: Some(size as u64),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ max-db-size set to {}", format_size(size));
        }
        ConfigKey::TrafficMaxBodySize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                max_body_memory_size: Some(size),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ max-body-size set to {}", format_size(size));
        }
        ConfigKey::TrafficMaxBufferSize => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                max_body_buffer_size: Some(size),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ max-buffer-size set to {}", format_size(size));
        }
        ConfigKey::TrafficSuperPerformanceMode => {
            let enabled = parse_bool(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                super_performance_mode: Some(enabled),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ super-performance-mode set to {}", enabled);
        }
        ConfigKey::TrafficRetentionDays => {
            let days = value
                .parse::<u64>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            if days > 7 {
                return Err(BifrostError::Config(
                    "retention-days cannot exceed 7 days".to_string(),
                ));
            }
            let req = UpdatePerformanceConfigRequest {
                file_retention_days: Some(days),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ retention-days set to {} days", days);
        }
        ConfigKey::TrafficSseStreamFlushBytes => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                sse_stream_flush_bytes: Some(size),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ sse-stream-flush-bytes set to {}", format_size(size));
        }
        ConfigKey::TrafficSseStreamFlushIntervalMs => {
            let ms = value
                .parse::<u64>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            let req = UpdatePerformanceConfigRequest {
                sse_stream_flush_interval_ms: Some(ms),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ sse-stream-flush-interval-ms set to {} ms", ms);
        }
        ConfigKey::TrafficWsPayloadFlushBytes => {
            let size = parse_size(value).map_err(BifrostError::Config)?;
            let req = UpdatePerformanceConfigRequest {
                ws_payload_flush_bytes: Some(size),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ ws-payload-flush-bytes set to {}", format_size(size));
        }
        ConfigKey::TrafficWsPayloadFlushIntervalMs => {
            let ms = value
                .parse::<u64>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            let req = UpdatePerformanceConfigRequest {
                ws_payload_flush_interval_ms: Some(ms),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ ws-payload-flush-interval-ms set to {} ms", ms);
        }
        ConfigKey::TrafficWsPayloadMaxOpenFiles => {
            let count = value
                .parse::<usize>()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            let req = UpdatePerformanceConfigRequest {
                ws_payload_max_open_files: Some(count),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ ws-payload-max-open-files set to {}", count);
        }
        ConfigKey::AccessMode => {
            let valid_modes = ["allow_all", "local_only", "whitelist", "interactive"];
            if !valid_modes.contains(&value) {
                return Err(BifrostError::Config(format!(
                    "Invalid access mode: '{}'. Valid modes: {}",
                    value,
                    valid_modes.join(", ")
                )));
            }
            client
                .set_access_mode(value)
                .map_err(BifrostError::Config)?;
            println!("✓ access mode set to {}", value);
        }
        ConfigKey::AccessAllowLan => {
            let allow = parse_bool(value).map_err(BifrostError::Config)?;
            client.set_allow_lan(allow).map_err(BifrostError::Config)?;
            println!("✓ allow-lan set to {}", allow);
        }
        ConfigKey::AccessUserPassEnabled => {
            let enabled = parse_bool(value).map_err(BifrostError::Config)?;
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled,
                    accounts: whitelist
                        .userpass
                        .accounts
                        .into_iter()
                        .map(|account| UpdateUserPassAccountRequest {
                            username: account.username,
                            password: None,
                            enabled: account.enabled,
                        })
                        .collect(),
                    loopback_requires_auth: whitelist.userpass.loopback_requires_auth,
                })
                .map_err(BifrostError::Config)?;
            println!("✓ access.userpass.enabled set to {}", enabled);
        }
        ConfigKey::AccessUserPassAccounts => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            let accounts = parse_userpass_accounts(value).map_err(BifrostError::Config)?;
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled: whitelist.userpass.enabled,
                    accounts,
                    loopback_requires_auth: whitelist.userpass.loopback_requires_auth,
                })
                .map_err(BifrostError::Config)?;
            println!("✓ access.userpass.accounts updated");
        }
        ConfigKey::AccessUserPassLoopbackRequiresAuth => {
            let enabled = parse_bool(value).map_err(BifrostError::Config)?;
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled: whitelist.userpass.enabled,
                    accounts: whitelist
                        .userpass
                        .accounts
                        .into_iter()
                        .map(|account| UpdateUserPassAccountRequest {
                            username: account.username,
                            password: None,
                            enabled: account.enabled,
                        })
                        .collect(),
                    loopback_requires_auth: enabled,
                })
                .map_err(BifrostError::Config)?;
            println!(
                "✓ access.userpass.loopback-requires-auth set to {}",
                enabled
            );
        }
    }
    Ok(())
}

fn add_list_item(client: &ConfigApiClient, key: &str, value: &str) -> Result<()> {
    let config_key: ConfigKey = key.parse().map_err(BifrostError::Config)?;

    if !config_key.is_list() {
        return Err(BifrostError::Config(format!(
            "'{}' is not a list configuration. Use 'set' instead.\n\nList configurations: tls.exclude, tls.include, tls.app-exclude, tls.app-include",
            key
        )));
    }

    let tls = client.get_tls_config().map_err(BifrostError::Config)?;
    let mut list = match config_key {
        ConfigKey::TlsExclude => tls.intercept_exclude,
        ConfigKey::TlsInclude => tls.intercept_include,
        ConfigKey::TlsAppExclude => tls.app_intercept_exclude,
        ConfigKey::TlsAppInclude => tls.app_intercept_include,
        ConfigKey::TlsIpExclude => tls.ip_intercept_exclude,
        ConfigKey::TlsIpInclude => tls.ip_intercept_include,
        _ => unreachable!(),
    };

    if list.contains(&value.to_string()) {
        println!("⚠ '{}' already exists in {}", value, key);
        return Ok(());
    }

    list.push(value.to_string());

    let req = match config_key {
        ConfigKey::TlsExclude => UpdateTlsConfigRequest {
            intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsInclude => UpdateTlsConfigRequest {
            intercept_include: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsAppExclude => UpdateTlsConfigRequest {
            app_intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsAppInclude => UpdateTlsConfigRequest {
            app_intercept_include: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsIpExclude => UpdateTlsConfigRequest {
            ip_intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsIpInclude => UpdateTlsConfigRequest {
            ip_intercept_include: Some(list),
            ..Default::default()
        },
        _ => unreachable!(),
    };

    client
        .update_tls_config(&req)
        .map_err(BifrostError::Config)?;
    println!("✓ Added '{}' to {}", value, key);
    Ok(())
}

fn remove_list_item(client: &ConfigApiClient, key: &str, value: &str) -> Result<()> {
    let config_key: ConfigKey = key.parse().map_err(BifrostError::Config)?;

    if !config_key.is_list() {
        return Err(BifrostError::Config(format!(
            "'{}' is not a list configuration.\n\nList configurations: tls.exclude, tls.include, tls.app-exclude, tls.app-include",
            key
        )));
    }

    let tls = client.get_tls_config().map_err(BifrostError::Config)?;
    let mut list = match config_key {
        ConfigKey::TlsExclude => tls.intercept_exclude,
        ConfigKey::TlsInclude => tls.intercept_include,
        ConfigKey::TlsAppExclude => tls.app_intercept_exclude,
        ConfigKey::TlsAppInclude => tls.app_intercept_include,
        ConfigKey::TlsIpExclude => tls.ip_intercept_exclude,
        ConfigKey::TlsIpInclude => tls.ip_intercept_include,
        _ => unreachable!(),
    };

    let original_len = list.len();
    list.retain(|x| x != value);

    if list.len() == original_len {
        println!("⚠ '{}' not found in {}", value, key);
        return Ok(());
    }

    let req = match config_key {
        ConfigKey::TlsExclude => UpdateTlsConfigRequest {
            intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsInclude => UpdateTlsConfigRequest {
            intercept_include: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsAppExclude => UpdateTlsConfigRequest {
            app_intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsAppInclude => UpdateTlsConfigRequest {
            app_intercept_include: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsIpExclude => UpdateTlsConfigRequest {
            ip_intercept_exclude: Some(list),
            ..Default::default()
        },
        ConfigKey::TlsIpInclude => UpdateTlsConfigRequest {
            ip_intercept_include: Some(list),
            ..Default::default()
        },
        _ => unreachable!(),
    };

    client
        .update_tls_config(&req)
        .map_err(BifrostError::Config)?;
    println!("✓ Removed '{}' from {}", value, key);
    Ok(())
}

fn reset_config(client: &ConfigApiClient, key: &str, yes: bool) -> Result<()> {
    if key == "all" {
        if !yes {
            print!("This will reset ALL configurations to default values. Continue? [y/N] ");
            std::io::stdout()
                .flush()
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .map_err(|e| BifrostError::Config(e.to_string()))?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Cancelled.");
                return Ok(());
            }
        }

        let tls_req = UpdateTlsConfigRequest {
            enable_tls_interception: Some(false),
            intercept_exclude: Some(vec![]),
            intercept_include: Some(vec![]),
            app_intercept_exclude: Some(vec![]),
            app_intercept_include: Some(vec![]),
            ip_intercept_exclude: Some(vec![]),
            ip_intercept_include: Some(vec![]),
            unsafe_ssl: Some(false),
            disconnect_on_config_change: Some(true),
        };
        client
            .update_tls_config(&tls_req)
            .map_err(BifrostError::Config)?;

        let server_req = UpdateServerConfigRequest {
            timeout_secs: Some(30),
            http1_max_header_size: Some(64 * 1024),
            http2_max_header_list_size: Some(256 * 1024),
            websocket_handshake_max_header_size: Some(64 * 1024),
        };
        client
            .update_server_config(&server_req)
            .map_err(BifrostError::Config)?;

        let perf_req = UpdatePerformanceConfigRequest {
            max_records: Some(DEFAULT_TRAFFIC_MAX_RECORDS),
            max_db_size_bytes: Some(2 * 1024 * 1024 * 1024),
            max_body_memory_size: Some(512 * 1024),
            max_body_buffer_size: Some(10 * 1024 * 1024),
            max_body_probe_size: Some(64 * 1024),
            binary_traffic_performance_mode: Some(true),
            file_retention_days: Some(7),
            sse_stream_flush_bytes: Some(256 * 1024),
            sse_stream_flush_interval_ms: Some(1000),
            ws_payload_flush_bytes: Some(512 * 1024),
            ws_payload_flush_interval_ms: Some(1000),
            ws_payload_max_open_files: Some(128),
            super_performance_mode: Some(false),
        };
        client
            .update_performance_config(&perf_req)
            .map_err(BifrostError::Config)?;

        client
            .set_access_mode("local_only")
            .map_err(BifrostError::Config)?;
        client.set_allow_lan(false).map_err(BifrostError::Config)?;

        println!("✓ All configurations reset to defaults");
        return Ok(());
    }

    let config_key: ConfigKey = key.parse().map_err(BifrostError::Config)?;

    if !yes {
        print!("Reset '{}' to default value? [y/N] ", key);
        std::io::stdout()
            .flush()
            .map_err(|e| BifrostError::Config(e.to_string()))?;
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| BifrostError::Config(e.to_string()))?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    match config_key {
        ConfigKey::ServerTimeoutSecs => {
            let req = UpdateServerConfigRequest {
                timeout_secs: Some(30),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ server.timeout-secs reset to 30");
        }
        ConfigKey::ServerHttp1MaxHeaderSize => {
            let req = UpdateServerConfigRequest {
                http1_max_header_size: Some(64 * 1024),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ server.http1-max-header-size reset to 64 KB");
        }
        ConfigKey::ServerHttp2MaxHeaderListSize => {
            let req = UpdateServerConfigRequest {
                http2_max_header_list_size: Some(256 * 1024),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ server.http2-max-header-list-size reset to 256 KB");
        }
        ConfigKey::ServerWebSocketHandshakeMaxHeaderSize => {
            let req = UpdateServerConfigRequest {
                websocket_handshake_max_header_size: Some(64 * 1024),
                ..Default::default()
            };
            client
                .update_server_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ server.websocket-handshake-max-header-size reset to 64 KB");
        }
        ConfigKey::TlsEnabled => {
            let req = UpdateTlsConfigRequest {
                enable_tls_interception: Some(false),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.enabled reset to true");
        }
        ConfigKey::TlsUnsafeSsl => {
            let req = UpdateTlsConfigRequest {
                unsafe_ssl: Some(false),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.unsafe-ssl reset to false");
        }
        ConfigKey::TlsDisconnectOnChange => {
            let req = UpdateTlsConfigRequest {
                disconnect_on_config_change: Some(true),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.disconnect-on-change reset to true");
        }
        ConfigKey::TlsExclude => {
            let req = UpdateTlsConfigRequest {
                intercept_exclude: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.exclude reset to []");
        }
        ConfigKey::TlsInclude => {
            let req = UpdateTlsConfigRequest {
                intercept_include: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.include reset to []");
        }
        ConfigKey::TlsAppExclude => {
            let req = UpdateTlsConfigRequest {
                app_intercept_exclude: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.app-exclude reset to []");
        }
        ConfigKey::TlsAppInclude => {
            let req = UpdateTlsConfigRequest {
                app_intercept_include: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.app-include reset to []");
        }
        ConfigKey::TlsIpExclude => {
            let req = UpdateTlsConfigRequest {
                ip_intercept_exclude: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.ip-exclude reset to []");
        }
        ConfigKey::TlsIpInclude => {
            let req = UpdateTlsConfigRequest {
                ip_intercept_include: Some(vec![]),
                ..Default::default()
            };
            client
                .update_tls_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ tls.ip-include reset to []");
        }
        ConfigKey::TrafficMaxRecords => {
            let req = UpdatePerformanceConfigRequest {
                max_records: Some(DEFAULT_TRAFFIC_MAX_RECORDS),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!(
                "✓ traffic.max-records reset to {}",
                DEFAULT_TRAFFIC_MAX_RECORDS
            );
        }
        ConfigKey::TrafficMaxDbSize => {
            let req = UpdatePerformanceConfigRequest {
                max_db_size_bytes: Some(2 * 1024 * 1024 * 1024),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.max-db-size reset to 2 GB");
        }
        ConfigKey::TrafficMaxBodySize => {
            let req = UpdatePerformanceConfigRequest {
                max_body_memory_size: Some(512 * 1024),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.max-body-size reset to 512 KB");
        }
        ConfigKey::TrafficMaxBufferSize => {
            let req = UpdatePerformanceConfigRequest {
                max_body_buffer_size: Some(10 * 1024 * 1024),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.max-buffer-size reset to 10 MB");
        }
        ConfigKey::TrafficSuperPerformanceMode => {
            let req = UpdatePerformanceConfigRequest {
                super_performance_mode: Some(false),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.super-performance-mode reset to false");
        }
        ConfigKey::TrafficRetentionDays => {
            let req = UpdatePerformanceConfigRequest {
                file_retention_days: Some(7),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.retention-days reset to 7");
        }
        ConfigKey::TrafficSseStreamFlushBytes => {
            let req = UpdatePerformanceConfigRequest {
                sse_stream_flush_bytes: Some(256 * 1024),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.sse-stream-flush-bytes reset to 256 KB");
        }
        ConfigKey::TrafficSseStreamFlushIntervalMs => {
            let req = UpdatePerformanceConfigRequest {
                sse_stream_flush_interval_ms: Some(1000),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.sse-stream-flush-interval-ms reset to 1000");
        }
        ConfigKey::TrafficWsPayloadFlushBytes => {
            let req = UpdatePerformanceConfigRequest {
                ws_payload_flush_bytes: Some(512 * 1024),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.ws-payload-flush-bytes reset to 512 KB");
        }
        ConfigKey::TrafficWsPayloadFlushIntervalMs => {
            let req = UpdatePerformanceConfigRequest {
                ws_payload_flush_interval_ms: Some(1000),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.ws-payload-flush-interval-ms reset to 1000");
        }
        ConfigKey::TrafficWsPayloadMaxOpenFiles => {
            let req = UpdatePerformanceConfigRequest {
                ws_payload_max_open_files: Some(128),
                ..Default::default()
            };
            client
                .update_performance_config(&req)
                .map_err(BifrostError::Config)?;
            println!("✓ traffic.ws-payload-max-open-files reset to 128");
        }
        ConfigKey::AccessMode => {
            client
                .set_access_mode("local_only")
                .map_err(BifrostError::Config)?;
            println!("✓ access.mode reset to local_only");
        }
        ConfigKey::AccessAllowLan => {
            client.set_allow_lan(false).map_err(BifrostError::Config)?;
            println!("✓ access.allow-lan reset to false");
        }
        ConfigKey::AccessUserPassEnabled => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled: false,
                    accounts: whitelist
                        .userpass
                        .accounts
                        .into_iter()
                        .map(|account| UpdateUserPassAccountRequest {
                            username: account.username,
                            password: None,
                            enabled: account.enabled,
                        })
                        .collect(),
                    loopback_requires_auth: whitelist.userpass.loopback_requires_auth,
                })
                .map_err(BifrostError::Config)?;
            println!("✓ access.userpass.enabled reset to false");
        }
        ConfigKey::AccessUserPassAccounts => {
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled: false,
                    accounts: Vec::new(),
                    loopback_requires_auth: false,
                })
                .map_err(BifrostError::Config)?;
            println!("✓ access.userpass.accounts reset");
        }
        ConfigKey::AccessUserPassLoopbackRequiresAuth => {
            let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;
            client
                .set_userpass(&UpdateUserPassRequest {
                    enabled: whitelist.userpass.enabled,
                    accounts: whitelist
                        .userpass
                        .accounts
                        .into_iter()
                        .map(|account| UpdateUserPassAccountRequest {
                            username: account.username,
                            password: None,
                            enabled: account.enabled,
                        })
                        .collect(),
                    loopback_requires_auth: false,
                })
                .map_err(BifrostError::Config)?;
            println!("✓ access.userpass.loopback-requires-auth reset to false");
        }
    }
    Ok(())
}

fn clear_cache(client: &ConfigApiClient, yes: bool) -> Result<()> {
    if !yes {
        print!("This will clear all cached data. Continue? [y/N] ");
        std::io::stdout()
            .flush()
            .map_err(|e| BifrostError::Config(e.to_string()))?;
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| BifrostError::Config(e.to_string()))?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let result = client.clear_cache().map_err(BifrostError::Config)?;
    println!("✓ {}", result.message);
    Ok(())
}

fn disconnect_domain(client: &ConfigApiClient, domain: &str) -> Result<()> {
    let result = client
        .disconnect_by_domain(domain)
        .map_err(BifrostError::Config)?;
    println!("✓ {}", result.message);
    Ok(())
}

fn export_config(client: &ConfigApiClient, output: Option<PathBuf>, format: &str) -> Result<()> {
    let server = client.get_server_config().map_err(BifrostError::Config)?;
    let tls = client.get_tls_config().map_err(BifrostError::Config)?;
    let perf = client
        .get_performance_config()
        .map_err(BifrostError::Config)?;
    let whitelist = client.get_whitelist().map_err(BifrostError::Config)?;

    let content = match format.to_lowercase().as_str() {
        "json" => export_as_json(&server, &tls, &perf, &whitelist)?,
        "toml" => export_as_toml(&server, &tls, &perf, &whitelist),
        _ => {
            return Err(BifrostError::Config(format!(
                "Unsupported format: '{}'. Use 'json' or 'toml'",
                format
            )));
        }
    };

    if let Some(path) = output {
        std::fs::write(&path, &content).map_err(|e| BifrostError::Config(e.to_string()))?;
        println!("✓ Configuration exported to {}", path.display());
    } else {
        println!("{}", content);
    }

    Ok(())
}

fn export_as_json(
    server: &ServerConfigResponse,
    tls: &TlsConfigResponse,
    perf: &PerformanceConfigResponse,
    whitelist: &WhitelistResponse,
) -> Result<String> {
    let combined = serde_json::json!({
        "server": {
            "timeout_secs": server.timeout_secs,
            "http1_max_header_size": server.http1_max_header_size,
            "http2_max_header_list_size": server.http2_max_header_list_size,
            "websocket_handshake_max_header_size": server.websocket_handshake_max_header_size,
        },
        "tls": {
            "enabled": tls.enable_tls_interception,
            "unsafe_ssl": tls.unsafe_ssl,
            "disconnect_on_change": tls.disconnect_on_config_change,
            "exclude": tls.intercept_exclude,
            "include": tls.intercept_include,
            "app_exclude": tls.app_intercept_exclude,
            "app_include": tls.app_intercept_include,
        },
        "traffic": {
            "max_records": perf.traffic.max_records,
            "max_db_size_bytes": perf.traffic.max_db_size_bytes,
            "max_body_size": perf.traffic.max_body_memory_size,
            "max_buffer_size": perf.traffic.max_body_buffer_size,
            "super_performance_mode": perf.traffic.super_performance_mode,
            "retention_days": perf.traffic.file_retention_days,
        },
        "access": {
            "mode": whitelist.mode,
            "allow_lan": whitelist.allow_lan,
        }
    });
    serde_json::to_string_pretty(&combined).map_err(|e| BifrostError::Config(e.to_string()))
}

fn export_as_toml(
    server: &ServerConfigResponse,
    tls: &TlsConfigResponse,
    perf: &PerformanceConfigResponse,
    whitelist: &WhitelistResponse,
) -> String {
    let mut output = String::new();

    output.push_str("[server]\n");
    output.push_str(&format!("timeout_secs = {}\n", server.timeout_secs));
    output.push_str(&format!(
        "http1_max_header_size = {}\n",
        server.http1_max_header_size
    ));
    output.push_str(&format!(
        "http2_max_header_list_size = {}\n",
        server.http2_max_header_list_size
    ));
    output.push_str(&format!(
        "websocket_handshake_max_header_size = {}\n",
        server.websocket_handshake_max_header_size
    ));
    output.push('\n');

    output.push_str("[tls]\n");
    output.push_str(&format!(
        "enable_interception = {}\n",
        tls.enable_tls_interception
    ));
    output.push_str(&format!("unsafe_ssl = {}\n", tls.unsafe_ssl));
    output.push_str(&format!(
        "disconnect_on_change = {}\n",
        tls.disconnect_on_config_change
    ));
    if !tls.intercept_exclude.is_empty() {
        output.push_str(&format!(
            "intercept_exclude = {:?}\n",
            tls.intercept_exclude
        ));
    }
    if !tls.intercept_include.is_empty() {
        output.push_str(&format!(
            "intercept_include = {:?}\n",
            tls.intercept_include
        ));
    }
    if !tls.app_intercept_exclude.is_empty() {
        output.push_str(&format!(
            "app_intercept_exclude = {:?}\n",
            tls.app_intercept_exclude
        ));
    }
    if !tls.app_intercept_include.is_empty() {
        output.push_str(&format!(
            "app_intercept_include = {:?}\n",
            tls.app_intercept_include
        ));
    }
    output.push('\n');

    output.push_str("[traffic]\n");
    output.push_str(&format!("max_records = {}\n", perf.traffic.max_records));
    output.push_str(&format!(
        "max_db_size_bytes = {}\n",
        perf.traffic.max_db_size_bytes
    ));
    output.push_str(&format!(
        "max_body_memory_size = {}\n",
        perf.traffic.max_body_memory_size
    ));
    output.push_str(&format!(
        "max_body_buffer_size = {}\n",
        perf.traffic.max_body_buffer_size
    ));
    output.push_str(&format!(
        "super_performance_mode = {}\n",
        perf.traffic.super_performance_mode
    ));
    output.push_str(&format!(
        "file_retention_days = {}\n",
        perf.traffic.file_retention_days
    ));
    output.push('\n');

    output.push_str("[access]\n");
    output.push_str(&format!("mode = \"{}\"\n", whitelist.mode));
    output.push_str(&format!("allow_lan = {}\n", whitelist.allow_lan));

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::client::{
        BodyStoreStats, FrameStoreStats, PerformanceConfigResponse, ServerConfigResponse,
        TlsConfigResponse, TrafficConfig, UserPassAccountResponse, UserPassResponse,
        WhitelistResponse, WsPayloadStoreStats,
    };

    fn sample_server() -> ServerConfigResponse {
        ServerConfigResponse {
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
        }
    }

    fn sample_tls() -> TlsConfigResponse {
        TlsConfigResponse {
            enable_tls_interception: true,
            intercept_exclude: vec!["example.com".into()],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec!["AppA".into()],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
            disconnect_on_config_change: true,
        }
    }

    fn sample_perf(with_stats: bool) -> PerformanceConfigResponse {
        let traffic = TrafficConfig {
            max_records: 1000,
            max_db_size_bytes: 1024 * 1024 * 1024,
            max_body_memory_size: 512 * 1024,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            super_performance_mode: false,
            binary_traffic_performance_mode: true,
            file_retention_days: 7,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 64,
        };

        let (body_store_stats, frame_store_stats, ws_payload_store_stats) = if with_stats {
            (
                Some(BodyStoreStats {
                    file_count: 10,
                    total_size: 12345,
                    temp_dir: "/tmp/body".into(),
                    max_memory_size: 512 * 1024,
                    retention_days: 7,
                }),
                Some(FrameStoreStats {
                    connection_count: 2,
                    total_size: 6789,
                    frames_dir: "/tmp/frames".into(),
                    retention_hours: 24,
                }),
                Some(WsPayloadStoreStats {
                    file_count: 3,
                    total_size: 1111,
                    payload_dir: "/tmp/ws".into(),
                    retention_days: 3,
                }),
            )
        } else {
            (None, None, None)
        };

        PerformanceConfigResponse {
            traffic,
            body_store_stats,
            frame_store_stats,
            ws_payload_store_stats,
        }
    }

    fn sample_whitelist() -> WhitelistResponse {
        WhitelistResponse {
            mode: "local_only".into(),
            allow_lan: false,
            whitelist: vec!["127.0.0.1".into()],
            temporary_whitelist: vec![],
            userpass: UserPassResponse {
                enabled: true,
                accounts: vec![UserPassAccountResponse {
                    username: "demo".into(),
                    enabled: true,
                    has_password: true,
                    last_connected_at: None,
                }],
                loopback_requires_auth: true,
            },
        }
    }

    #[test]
    fn parse_userpass_accounts_parses_valid_json_array() {
        let json = r#"[
            {"username":"alice","password":"secret","enabled":true},
            {"username":"bob","password":null,"enabled":false}
        ]"#;

        let accounts = parse_userpass_accounts(json).expect("valid accounts json");
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].username, "alice");
        assert_eq!(accounts[0].password.as_deref(), Some("secret"));
        assert!(accounts[0].enabled);
        assert_eq!(accounts[1].username, "bob");
        assert!(accounts[1].password.is_none());
        assert!(!accounts[1].enabled);
    }

    #[test]
    fn parse_userpass_accounts_returns_helpful_error_for_invalid_json() {
        let err = parse_userpass_accounts("not-json").unwrap_err();
        assert!(err.starts_with("access.userpass.accounts expects a JSON array like",));
    }

    #[test]
    fn export_as_json_produces_structured_output() {
        let server = sample_server();
        let tls = sample_tls();
        let perf = sample_perf(false);
        let whitelist = sample_whitelist();

        let json_str = export_as_json(&server, &tls, &perf, &whitelist).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(v["server"]["timeout_secs"].as_u64(), Some(30));
        assert_eq!(
            v["server"]["http1_max_header_size"].as_u64(),
            Some(64 * 1024)
        );
        assert_eq!(v["tls"]["enabled"].as_bool(), Some(true));
        assert_eq!(
            v["traffic"]["max_records"].as_u64(),
            Some(perf.traffic.max_records as u64)
        );
        assert_eq!(v["access"]["mode"].as_str(), Some("local_only"));
        assert_eq!(v["access"]["allow_lan"].as_bool(), Some(false));
    }

    #[test]
    fn export_as_toml_contains_expected_sections_and_keys() {
        let server = sample_server();
        let tls = sample_tls();
        let perf = sample_perf(true);
        let whitelist = sample_whitelist();

        let toml = export_as_toml(&server, &tls, &perf, &whitelist);

        assert!(toml.contains("[server]"));
        assert!(toml.contains("timeout_secs = 30"));
        assert!(toml.contains("[tls]"));
        assert!(toml.contains("enable_interception"));
        assert!(toml.contains("[traffic]"));
        assert!(toml.contains("max_records"));
        assert!(toml.contains("[access]"));
        assert!(toml.contains("mode = \"local_only\""));
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::cli::ConfigCommands;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn start_mock_server() -> (MockServer, String, u16) {
        let mock_server = MockServer::start().await;
        let base_uri = mock_server.uri(); // e.g. http://127.0.0.1:12345
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().expect("mock server host").to_string();
        let port: u16 = parts
            .next()
            .expect("mock server port")
            .parse()
            .expect("valid port number");
        (mock_server, host, port)
    }

    async fn setup_basic_config_mocks(server: &MockServer) {
        // Server config (GET/PUT)
        let server_body = json!({
            "timeout_secs": 30u64,
            "http1_max_header_size": 64_000usize,
            "http2_max_header_list_size": 256_000usize,
            "websocket_handshake_max_header_size": 64_000usize,
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&server_body))
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&server_body))
            .mount(server)
            .await;

        // TLS config (GET/PUT)
        let tls_body = json!({
            "enable_tls_interception": true,
            "intercept_exclude": ["example.com"],
            "intercept_include": ["include.com"],
            "app_intercept_exclude": ["AppA"],
            "app_intercept_include": ["AppB"],
            "ip_intercept_exclude": ["127.0.0.1"],
            "ip_intercept_include": ["10.0.0.1"],
            "unsafe_ssl": false,
            "disconnect_on_config_change": true,
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/tls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tls_body))
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/tls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tls_body))
            .mount(server)
            .await;

        // Performance config (GET/PUT)
        let perf_body = json!({
            "traffic": {
                "max_records": DEFAULT_TRAFFIC_MAX_RECORDS,
                "max_db_size_bytes": 2_000_000_000u64,
                "max_body_memory_size": 512 * 1024usize,
                "max_body_buffer_size": 10 * 1024 * 1024usize,
                "max_body_probe_size": 64 * 1024usize,
                "super_performance_mode": false,
                "binary_traffic_performance_mode": true,
                "file_retention_days": 7u64,
                "sse_stream_flush_bytes": 256 * 1024usize,
                "sse_stream_flush_interval_ms": 1000u64,
                "ws_payload_flush_bytes": 512 * 1024usize,
                "ws_payload_flush_interval_ms": 1000u64,
                "ws_payload_max_open_files": 128usize,
            },
            "body_store_stats": null,
            "frame_store_stats": null,
            "ws_payload_store_stats": null,
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/performance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&perf_body))
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/performance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&perf_body))
            .mount(server)
            .await;

        // Clear cache
        let clear_body = json!({
            "body_cache_removed": 1usize,
            "traffic_cache_removed": 2usize,
            "frame_cache_removed": 3usize,
            "ws_payload_cache_removed": 4usize,
            "message": "cleared",
        });
        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/config/performance/clear-cache"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&clear_body))
            .mount(server)
            .await;

        // Whitelist (GET)
        let whitelist_body = json!({
            "mode": "local_only",
            "allow_lan": false,
            "whitelist": ["127.0.0.1"],
            "temporary_whitelist": [],
            "userpass": {
                "enabled": true,
                "accounts": [
                    {
                        "username": "demo",
                        "enabled": true,
                        "has_password": true,
                        "last_connected_at": null,
                    }
                ],
                "loopback_requires_auth": true,
            }
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/whitelist"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&whitelist_body))
            .mount(server)
            .await;

        // Whitelist updates
        let ok_body = json!({"ok": true});
        for (m, p) in [
            ("PUT", "/_bifrost/api/whitelist/allow-lan"),
            ("PUT", "/_bifrost/api/whitelist/userpass"),
            ("PUT", "/_bifrost/api/whitelist/mode"),
        ] {
            Mock::given(method(m))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_json(&ok_body))
                .mount(server)
                .await;
        }

        // Disconnect by domain
        let disconnect_body = json!({
            "success": true,
            "disconnected_count": 1usize,
            "message": "disconnected",
        });
        Mock::given(method("POST"))
            .and(path("/_bifrost/api/config/connections/disconnect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&disconnect_body))
            .mount(server)
            .await;

        // Disconnect by app
        Mock::given(method("POST"))
            .and(path("/_bifrost/api/config/connections/disconnect-by-app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&ok_body))
            .mount(server)
            .await;

        // List connections
        let connections_body = json!({
            "total": 1usize,
            "connections": [
                {
                    "req_id": "req-1",
                    "host": "example.com",
                    "port": 443u16,
                    "intercept_mode": true,
                    "client_app": "UnitTest",
                }
            ],
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&connections_body))
            .mount(server)
            .await;

        // System overview
        let overview_body = json!({
            "body_store_used": 10u64,
            "frame_store_used": 20u64,
            "other": "ignored",
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/system/overview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&overview_body))
            .mount(server)
            .await;

        // Sandbox config
        let sandbox_body = json!({"net": {"enabled": true}});
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/sandbox"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&sandbox_body))
            .mount(server)
            .await;

        // Websocket connections
        let ws_body = json!([]);
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/websocket/connections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&ws_body))
            .mount(server)
            .await;

        // Memory diagnostics
        let memory_body = json!({
            "process": {
                "pid": 1234u64,
                "rss_kib": 1024u64,
                "vms_kib": 2048u64,
                "cpu_usage_percent": 1.5f64,
                "system_total_kib": 8 * 1024 * 1024u64,
            },
            "connections": {
                "tunnel_registry_active": 1u64,
                "sse": {"connections": 2u64, "open": 1u64},
            },
            "stores": {
                "body_store": {"file_count": 4u64, "total_size": 4096u64},
                "frame_store": {"disk": {"connection_count": 2u64, "total_size": 2048u64}},
                "ws_payload_store": {"disk": {"file_count": 3u64, "total_size": 1024u64}},
            },
            "traffic_db": {
                "db": {"record_count": 10u64, "db_size_bytes": 10_000u64},
                "recent_cache": {"entries": 5u64},
            },
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/system/memory"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&memory_body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn handle_config_command_covers_happy_paths_with_mock_server() {
        let (server, host, port) = start_mock_server().await;
        setup_basic_config_mocks(&server).await;

        // Default (None) -> show_all_config
        handle_config_command(None, &host, port).unwrap();

        // Show section variants
        handle_config_command(
            Some(ConfigCommands::Show {
                json: false,
                section: Some("server".to_string()),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Show {
                json: true,
                section: Some("tls".to_string()),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Show {
                json: false,
                section: Some("traffic".to_string()),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Show {
                json: true,
                section: Some("access".to_string()),
            }),
            &host,
            port,
        )
        .unwrap();

        // Get / set / list style commands
        handle_config_command(
            Some(ConfigCommands::Get {
                key: "tls.enabled".to_string(),
                json: true,
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Set {
                key: "server.timeout-secs".to_string(),
                value: "45".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Set {
                key: "traffic.max-records".to_string(),
                value: MIN_TRAFFIC_MAX_RECORDS.to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Set {
                key: "traffic.super-performance-mode".to_string(),
                value: "true".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Add {
                key: "tls.exclude".to_string(),
                value: "extra.com".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Remove {
                key: "tls.exclude".to_string(),
                value: "extra.com".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Reset {
                key: "server.timeout-secs".to_string(),
                yes: true,
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(Some(ConfigCommands::ClearCache { yes: true }), &host, port).unwrap();

        handle_config_command(
            Some(ConfigCommands::Disconnect {
                domain: "example.com".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::Export {
                output: None,
                format: "toml".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(
            Some(ConfigCommands::DisconnectByApp {
                app: "UnitTest".to_string(),
            }),
            &host,
            port,
        )
        .unwrap();

        handle_config_command(Some(ConfigCommands::Performance), &host, port).unwrap();
        handle_config_command(Some(ConfigCommands::Websocket), &host, port).unwrap();
        handle_config_command(Some(ConfigCommands::Connections), &host, port).unwrap();
        handle_config_command(Some(ConfigCommands::Memory), &host, port).unwrap();
    }

    #[tokio::test]
    async fn export_config_writes_toml_to_file_when_output_provided() {
        let (server, host, port) = start_mock_server().await;
        setup_basic_config_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        export_config(&client, Some(tmp.path().to_path_buf()), "toml").unwrap();

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("[server]"));
        assert!(contents.contains("[tls]"));
        assert!(contents.contains("[traffic]"));
        assert!(contents.contains("super_performance_mode = false"));
        assert!(contents.contains("[access]"));
    }

    #[test]
    fn show_section_config_rejects_unknown_section() {
        let client = ConfigApiClient::new("localhost", 0);
        let err = show_section_config(&client, "unknown-section", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown section"));
        assert!(msg.contains("server, tls, traffic, access"));
    }

    #[test]
    fn get_config_value_rejects_unknown_key() {
        let client = ConfigApiClient::new("localhost", 0);
        let err = get_config_value(&client, "does.not.exist", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown config key"));
        assert!(msg.contains("traffic.max-records"));
    }

    #[test]
    fn set_config_value_rejects_invalid_bool_and_ranges_before_network() {
        let client = ConfigApiClient::new("localhost", 0);

        // Invalid boolean for tls.enabled
        let err = set_config_value(&client, "tls.enabled", "maybe").unwrap_err();
        assert!(err.to_string().contains("Invalid boolean value"));

        // traffic.retention-days > 7
        let err = set_config_value(&client, "traffic.retention-days", "10").unwrap_err();
        assert!(err
            .to_string()
            .contains("retention-days cannot exceed 7 days"));

        // traffic.max-records outside allowed range
        let below_min = MIN_TRAFFIC_MAX_RECORDS - 1;
        let err =
            set_config_value(&client, "traffic.max-records", &below_min.to_string()).unwrap_err();
        assert!(err
            .to_string()
            .contains("traffic.max-records must be between"));
    }

    #[test]
    fn add_and_remove_list_item_report_non_list_keys() {
        let client = ConfigApiClient::new("localhost", 0);

        let err = add_list_item(&client, "server.timeout-secs", "1").unwrap_err();
        assert!(err.to_string().contains("is not a list configuration"));

        let err = remove_list_item(&client, "server.timeout-secs", "1").unwrap_err();
        assert!(err.to_string().contains("is not a list configuration"));
    }

    #[test]
    fn reset_config_rejects_unknown_key_without_network() {
        let client = ConfigApiClient::new("localhost", 0);
        let err = reset_config(&client, "does.not.exist", true).unwrap_err();
        assert!(err.to_string().contains("Unknown config key"));
    }

    #[test]
    fn export_config_rejects_unsupported_format() {
        let client = ConfigApiClient::new("localhost", 0);
        let _ = export_config(&client, None, "yaml").unwrap_err();
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::cli::ConfigCommands;
    use crate::commands::config::client::ConfigApiClient;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn start_mock_server() -> (MockServer, String, u16) {
        let mock_server = MockServer::start().await;
        let base_uri = mock_server.uri();
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().expect("mock server host").to_string();
        let port: u16 = parts
            .next()
            .expect("mock server port")
            .parse()
            .expect("valid port number");
        (mock_server, host, port)
    }

    async fn setup_tls_and_whitelist_mocks(server: &MockServer) {
        let tls_body = json!({
            "enable_tls_interception": false,
            "intercept_exclude": [],
            "intercept_include": [],
            "app_intercept_exclude": [],
            "app_intercept_include": [],
            "ip_intercept_exclude": [],
            "ip_intercept_include": [],
            "unsafe_ssl": false,
            "disconnect_on_config_change": true,
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/tls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tls_body))
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/tls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tls_body))
            .mount(server)
            .await;

        let whitelist_body = json!({
            "mode": "local_only",
            "allow_lan": false,
            "whitelist": ["127.0.0.1"],
            "temporary_whitelist": [],
            "userpass": {
                "enabled": false,
                "accounts": [],
                "loopback_requires_auth": false,
            }
        });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/whitelist"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&whitelist_body))
            .mount(server)
            .await;

        let ok_body = json!({"ok": true});
        for (m, p) in [
            ("PUT", "_bifrost/api/whitelist/allow-lan"),
            ("PUT", "_bifrost/api/whitelist/userpass"),
            ("PUT", "_bifrost/api/whitelist/mode"),
        ] {
            Mock::given(method(m))
                .and(path(format!("/{p}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(&ok_body))
                .mount(server)
                .await;
        }
    }

    #[tokio::test]
    async fn set_config_value_updates_tls_unsafe_ssl_via_mock_server() {
        let (server, host, port) = start_mock_server().await;
        setup_tls_and_whitelist_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        set_config_value(&client, "tls.unsafe-ssl", "true").unwrap();
    }

    #[tokio::test]
    async fn set_config_value_updates_tls_disconnect_on_change_via_mock_server() {
        let (server, host, port) = start_mock_server().await;
        setup_tls_and_whitelist_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        set_config_value(&client, "tls.disconnect-on-change", "false").unwrap();
    }

    #[tokio::test]
    async fn set_config_value_accepts_valid_access_modes() {
        let (server, host, port) = start_mock_server().await;
        setup_tls_and_whitelist_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        for mode in ["allow_all", "local_only", "whitelist", "interactive"] {
            set_config_value(&client, "access.mode", mode).unwrap();
        }
    }

    #[test]
    fn set_config_value_rejects_invalid_access_mode_before_network() {
        let client = ConfigApiClient::new("localhost", 0);
        let err = set_config_value(&client, "access.mode", "invalid-mode").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Invalid access mode"));
    }

    #[tokio::test]
    async fn add_list_item_adds_to_tls_include_list() {
        let (server, host, port) = start_mock_server().await;
        setup_tls_and_whitelist_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        add_list_item(&client, "tls.include", "example.com").unwrap();
    }

    #[tokio::test]
    async fn remove_list_item_handles_missing_value_gracefully() {
        let (server, host, port) = start_mock_server().await;
        setup_tls_and_whitelist_mocks(&server).await;

        let client = ConfigApiClient::new(&host, port);
        remove_list_item(&client, "tls.include", "missing.com").unwrap();
    }

    #[tokio::test]
    async fn reset_config_all_uses_mock_server_without_prompt() {
        let (server, host, port) = start_mock_server().await;

        // Minimal mocks for TLS / server / performance / whitelist reset sequence.
        let server_body = json!({
            "timeout_secs": 30u64,
            "http1_max_header_size": 64_000usize,
            "http2_max_header_list_size": 256_000usize,
            "websocket_handshake_max_header_size": 64_000usize,
        });
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&server_body))
            .mount(&server)
            .await;

        let tls_body = json!({
            "enable_tls_interception": false,
            "intercept_exclude": [],
            "intercept_include": [],
            "app_intercept_exclude": [],
            "app_intercept_include": [],
            "ip_intercept_exclude": [],
            "ip_intercept_include": [],
            "unsafe_ssl": false,
            "disconnect_on_config_change": true,
        });
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/tls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tls_body))
            .mount(&server)
            .await;

        let perf_body = json!({
            "traffic": {
                "max_records": DEFAULT_TRAFFIC_MAX_RECORDS,
                "max_db_size_bytes": 2_000_000_000u64,
                "max_body_memory_size": 512 * 1024usize,
                "max_body_buffer_size": 10 * 1024 * 1024usize,
                "max_body_probe_size": 64 * 1024usize,
                "super_performance_mode": false,
                "binary_traffic_performance_mode": true,
                "file_retention_days": 7u64,
                "sse_stream_flush_bytes": 256 * 1024usize,
                "sse_stream_flush_interval_ms": 1000u64,
                "ws_payload_flush_bytes": 512 * 1024usize,
                "ws_payload_flush_interval_ms": 1000u64,
                "ws_payload_max_open_files": 128usize,
            },
            "body_store_stats": null,
            "frame_store_stats": null,
            "ws_payload_store_stats": null,
        });
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/config/performance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&perf_body))
            .mount(&server)
            .await;

        let whitelist_body = json!({
            "mode": "local_only",
            "allow_lan": false,
            "whitelist": [],
            "temporary_whitelist": [],
            "userpass": {
                "enabled": false,
                "accounts": [],
                "loopback_requires_auth": false,
            }
        });
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/whitelist/mode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&whitelist_body))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/whitelist/allow-lan"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&whitelist_body))
            .mount(&server)
            .await;

        let client = ConfigApiClient::new(&host, port);
        reset_config(&client, "all", true).unwrap();
    }

    #[tokio::test]
    async fn clear_cache_uses_wiremock_response() {
        let (server, host, port) = start_mock_server().await;
        let clear_body = json!({
            "body_cache_removed": 1usize,
            "traffic_cache_removed": 0usize,
            "frame_cache_removed": 0usize,
            "ws_payload_cache_removed": 0usize,
            "message": "cleared",
        });
        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/config/performance/clear-cache"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&clear_body))
            .mount(&server)
            .await;

        let client = ConfigApiClient::new(&host, port);
        clear_cache(&client, true).unwrap();
    }

    #[tokio::test]
    async fn handle_config_command_reports_error_on_server_failure() {
        let (server, host, port) = start_mock_server().await;
        let error_body = json!({ "error": "boom" });
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/server"))
            .respond_with(ResponseTemplate::new(500).set_body_json(&error_body))
            .mount(&server)
            .await;

        let result = handle_config_command(
            Some(ConfigCommands::Show {
                json: true,
                section: Some("server".to_string()),
            }),
            &host,
            port,
        );
        assert!(result.is_err());
    }
}
