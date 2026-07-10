use super::client::{
    PerformanceConfigResponse, ServerConfigResponse, TlsConfigResponse, WhitelistResponse,
};
use super::keys::{format_size, ConfigKey};

pub fn print_full_config(
    server: &ServerConfigResponse,
    tls: &TlsConfigResponse,
    perf: &PerformanceConfigResponse,
    whitelist: &WhitelistResponse,
) {
    println!("Bifrost Configuration");
    println!("=====================\n");

    print_server_config(server);
    println!();
    print_tls_config(tls);
    println!();
    print_traffic_config(perf);
    println!();
    print_access_config(whitelist);
}

pub fn print_tls_config(tls: &TlsConfigResponse) {
    println!("TLS Configuration");
    println!("  Enabled:              {}", tls.enable_tls_interception);
    println!("  Unsafe SSL:           {}", tls.unsafe_ssl);
    println!(
        "  Disconnect on Change: {}",
        tls.disconnect_on_config_change
    );
    println!();
    println!("  Exclude Domains:");
    if tls.intercept_exclude.is_empty() {
        println!("    (none)");
    } else {
        for domain in &tls.intercept_exclude {
            println!("    - {}", domain);
        }
    }
    println!();
    println!("  Include Domains:");
    if tls.intercept_include.is_empty() {
        println!("    (none)");
    } else {
        for domain in &tls.intercept_include {
            println!("    - {}", domain);
        }
    }
    println!();
    println!("  Exclude Apps:");
    if tls.app_intercept_exclude.is_empty() {
        println!("    (none)");
    } else {
        for app in &tls.app_intercept_exclude {
            println!("    - {}", app);
        }
    }
    println!();
    println!("  Include Apps:");
    if tls.app_intercept_include.is_empty() {
        println!("    (none)");
    } else {
        for app in &tls.app_intercept_include {
            println!("    - {}", app);
        }
    }
}

pub fn print_server_config(server: &ServerConfigResponse) {
    println!("Server Configuration");
    println!("  Timeout:              {} s", server.timeout_secs);
    println!(
        "  HTTP/1 Header Limit:  {}",
        format_size(server.http1_max_header_size)
    );
    println!(
        "  HTTP/2 Header Limit:  {}",
        format_size(server.http2_max_header_list_size)
    );
    println!(
        "  WS Handshake Limit:   {}",
        format_size(server.websocket_handshake_max_header_size)
    );
}

pub fn print_traffic_config(perf: &PerformanceConfigResponse) {
    println!("Traffic Configuration");
    println!("  Max Records:          {}", perf.traffic.max_records);
    println!(
        "  Max DB Size:          {}",
        format_size(perf.traffic.max_db_size_bytes as usize)
    );
    println!(
        "  Max Body Size:        {}",
        format_size(perf.traffic.max_body_memory_size)
    );
    println!(
        "  Max Buffer Size:      {}",
        format_size(perf.traffic.max_body_buffer_size)
    );
    println!(
        "  Super Performance:    {}",
        perf.traffic.super_performance_mode
    );
    println!(
        "  Retention Days:       {}",
        perf.traffic.file_retention_days
    );
    println!(
        "  SSE Flush Bytes:      {}",
        format_size(perf.traffic.sse_stream_flush_bytes)
    );
    println!(
        "  SSE Flush Interval:   {} ms",
        perf.traffic.sse_stream_flush_interval_ms
    );
    println!(
        "  WS Flush Bytes:       {}",
        format_size(perf.traffic.ws_payload_flush_bytes)
    );
    println!(
        "  WS Flush Interval:    {} ms",
        perf.traffic.ws_payload_flush_interval_ms
    );
    println!(
        "  WS Max Open Files:    {}",
        perf.traffic.ws_payload_max_open_files
    );

    if perf.body_store_stats.is_some()
        || perf.frame_store_stats.is_some()
        || perf.ws_payload_store_stats.is_some()
    {
        println!();
        println!("  Storage Stats:");
        if let Some(ref stats) = perf.body_store_stats {
            println!(
                "    Body Cache:         {} ({} files)",
                format_size(stats.total_size as usize),
                stats.file_count
            );
        }
        if let Some(ref stats) = perf.frame_store_stats {
            println!(
                "    Frame Store:        {} ({} connections)",
                format_size(stats.total_size as usize),
                stats.connection_count
            );
        }
        if let Some(ref stats) = perf.ws_payload_store_stats {
            println!(
                "    WS Payload Store:   {} ({} files)",
                format_size(stats.total_size as usize),
                stats.file_count
            );
        }
    }
}

pub fn print_access_config(whitelist: &WhitelistResponse) {
    println!("Access Control");
    println!("  Mode:                 {}", whitelist.mode);
    println!("  Allow LAN:            {}", whitelist.allow_lan);
    println!("  UserPass Enabled:     {}", whitelist.userpass.enabled);
    println!(
        "  Loopback Requires Auth: {}",
        whitelist.userpass.loopback_requires_auth
    );
    if whitelist.userpass.accounts.is_empty() {
        println!("  UserPass Accounts:    []");
    } else {
        println!("  UserPass Accounts:");
        for account in &whitelist.userpass.accounts {
            println!(
                "    - {} (enabled: {}, has_password: {}, last_connected_at: {})",
                account.username,
                account.enabled,
                account.has_password,
                account.last_connected_at.as_deref().unwrap_or("-")
            );
        }
    }
}

pub fn print_config_value(key: &ConfigKey, value: &serde_json::Value) {
    match value {
        serde_json::Value::Bool(b) => println!("{} = {}", key, b),
        serde_json::Value::Number(n) => {
            if key.is_size() {
                if let Some(bytes) = n.as_u64() {
                    println!("{} = {} ({})", key, bytes, format_size(bytes as usize));
                } else {
                    println!("{} = {}", key, n);
                }
            } else {
                println!("{} = {}", key, n);
            }
        }
        serde_json::Value::String(s) => println!("{} = {}", key, s),
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                println!("{} = []", key);
            } else {
                println!("{} =", key);
                for item in arr {
                    if let serde_json::Value::String(s) = item {
                        println!("  - {}", s);
                    }
                }
            }
        }
        _ => println!("{} = {}", key, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::client::{
        BodyStoreStats, FrameStoreStats, PerformanceConfigResponse, ServerConfigResponse,
        TlsConfigResponse, TrafficConfig, UserPassAccountResponse, UserPassResponse,
        WhitelistResponse, WsPayloadStoreStats,
    };
    use serde_json::json;

    fn sample_server() -> ServerConfigResponse {
        ServerConfigResponse {
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
        }
    }

    fn sample_tls_empty() -> TlsConfigResponse {
        TlsConfigResponse {
            enable_tls_interception: true,
            intercept_exclude: vec![],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
            disconnect_on_config_change: true,
        }
    }

    fn sample_tls_lists() -> TlsConfigResponse {
        TlsConfigResponse {
            intercept_exclude: vec!["example.com".into()],
            intercept_include: vec!["include.com".into()],
            app_intercept_exclude: vec!["AppA".into()],
            app_intercept_include: vec!["AppB".into()],
            ip_intercept_exclude: vec!["10.0.0.1".into()],
            ip_intercept_include: vec!["10.0.0.2".into()],
            ..sample_tls_empty()
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
            file_retention_days: 3,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 32,
        };

        let (body_store_stats, frame_store_stats, ws_payload_store_stats) = if with_stats {
            (
                Some(BodyStoreStats {
                    file_count: 10,
                    total_size: 12345,
                    temp_dir: "/tmp/body".into(),
                    max_memory_size: 512 * 1024,
                    retention_days: 3,
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
                    retention_days: 2,
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

    fn sample_whitelist_with_accounts() -> WhitelistResponse {
        WhitelistResponse {
            mode: "whitelist".into(),
            allow_lan: true,
            whitelist: vec!["127.0.0.1".into()],
            temporary_whitelist: vec![],
            userpass: UserPassResponse {
                enabled: true,
                loopback_requires_auth: true,
                accounts: vec![UserPassAccountResponse {
                    username: "demo".into(),
                    enabled: true,
                    has_password: true,
                    last_connected_at: Some("2024-01-01T00:00:00Z".into()),
                }],
            },
        }
    }

    fn sample_whitelist_empty_accounts() -> WhitelistResponse {
        WhitelistResponse {
            userpass: UserPassResponse {
                enabled: false,
                accounts: Vec::new(),
                loopback_requires_auth: false,
            },
            ..sample_whitelist_with_accounts()
        }
    }

    #[test]
    fn print_server_config_runs_without_panic() {
        let server = sample_server();
        print_server_config(&server);
    }

    #[test]
    fn print_tls_config_handles_empty_and_non_empty_lists() {
        let tls_empty = sample_tls_empty();
        let tls_lists = sample_tls_lists();

        print_tls_config(&tls_empty);
        print_tls_config(&tls_lists);
    }

    #[test]
    fn print_traffic_config_prints_basic_and_stats_sections() {
        let perf_no_stats = sample_perf(false);
        let perf_with_stats = sample_perf(true);

        print_traffic_config(&perf_no_stats);
        print_traffic_config(&perf_with_stats);
    }

    #[test]
    fn print_access_config_handles_accounts_presence() {
        let wl_empty = sample_whitelist_empty_accounts();
        let wl_with_accounts = sample_whitelist_with_accounts();

        print_access_config(&wl_empty);
        print_access_config(&wl_with_accounts);
    }

    #[test]
    fn print_config_value_handles_all_supported_json_variants() {
        // Bool
        let key = ConfigKey::TlsEnabled;
        let value = json!(true);
        print_config_value(&key, &value);

        // Number that is treated as size
        let key = ConfigKey::TrafficMaxBodySize;
        let value = json!(1024u64);
        print_config_value(&key, &value);

        // Number that is not a size key
        let key = ConfigKey::TrafficRetentionDays;
        let value = json!(7);
        print_config_value(&key, &value);

        // String
        let key = ConfigKey::AccessMode;
        let value = json!("local_only");
        print_config_value(&key, &value);

        // Empty array
        let key = ConfigKey::TlsExclude;
        let value = json!([] as [String; 0]);
        print_config_value(&key, &value);

        // Non-empty array
        let value = json!(["example.com", "test.com"]);
        print_config_value(&key, &value);

        // Fallback branch for other JSON types (object)
        let value = json!({"nested": 1});
        print_config_value(&key, &value);
    }

    #[test]
    fn print_full_config_composes_all_sections() {
        let server = sample_server();
        let tls = sample_tls_lists();
        let perf = sample_perf(true);
        let wl = sample_whitelist_with_accounts();

        print_full_config(&server, &tls, &perf, &wl);
    }
}
