use crate::cli::MetricsCommands;

use bifrost_core::text::truncate_chars;

use super::config::client::ConfigApiClient;

pub fn handle_metrics_command(
    action: MetricsCommands,
    host: &str,
    port: u16,
) -> bifrost_core::Result<()> {
    let client = ConfigApiClient::new(host, port);

    match action {
        MetricsCommands::Summary => show_summary(&client),
        MetricsCommands::Apps => show_apps(&client),
        MetricsCommands::Hosts => show_hosts(&client),
        MetricsCommands::History { limit } => show_history(&client, limit),
    }
}

fn show_summary(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    let metrics = client
        .get_metrics()
        .map_err(bifrost_core::BifrostError::Config)?;
    let overview = client
        .get_system_overview()
        .map_err(bifrost_core::BifrostError::Config)?;

    println!("Bifrost Metrics Summary");
    println!("=======================");

    if let Some(obj) = overview.as_object() {
        if let Some(version) = obj.get("version").and_then(|v| v.as_str()) {
            println!("Version: {}", version);
        }
        if let Some(uptime) = obj.get("uptime_seconds").and_then(|v| v.as_u64()) {
            let hours = uptime / 3600;
            let minutes = (uptime % 3600) / 60;
            let seconds = uptime % 60;
            println!("Uptime: {}h {}m {}s", hours, minutes, seconds);
        }
        if let Some(port) = obj.get("port").and_then(|v| v.as_u64()) {
            println!("Port: {}", port);
        }
    }

    println!();

    if let Some(obj) = metrics.as_object() {
        println!("Traffic:");
        if let Some(total) = obj.get("total_requests").and_then(|v| v.as_u64()) {
            println!("  Total requests: {}", total);
        }
        if let Some(active) = obj.get("active_connections").and_then(|v| v.as_u64()) {
            println!("  Active connections: {}", active);
        }
        if let Some(bytes_in) = obj.get("bytes_received").and_then(|v| v.as_u64()) {
            println!("  Bytes received: {}", format_bytes(bytes_in));
        }
        if let Some(bytes_out) = obj.get("bytes_sent").and_then(|v| v.as_u64()) {
            println!("  Bytes sent: {}", format_bytes(bytes_out));
        }
        if let Some(tls) = obj.get("tls_connections").and_then(|v| v.as_u64()) {
            println!("  TLS connections: {}", tls);
        }
        if let Some(ws) = obj.get("websocket_connections").and_then(|v| v.as_u64()) {
            println!("  WebSocket connections: {}", ws);
        }

        for (key, value) in obj {
            if ![
                "total_requests",
                "active_connections",
                "bytes_received",
                "bytes_sent",
                "tls_connections",
                "websocket_connections",
                "timestamp",
            ]
            .contains(&key.as_str())
            {
                if let Some(n) = value.as_u64() {
                    println!("  {}: {}", key.replace('_', " "), n);
                } else if let Some(n) = value.as_f64() {
                    println!("  {}: {:.2}", key.replace('_', " "), n);
                }
            }
        }
    }

    Ok(())
}

fn show_apps(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    let apps = client
        .get_app_metrics()
        .map_err(bifrost_core::BifrostError::Config)?;

    if apps.is_empty() {
        println!("No application metrics available.");
        return Ok(());
    }

    println!(
        "{:<30} {:>10} {:>12} {:>12}",
        "APPLICATION", "REQUESTS", "BYTES IN", "BYTES OUT"
    );
    println!("{}", "-".repeat(66));

    for app in &apps {
        let name = app
            .get("app_name")
            .or_else(|| app.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let requests = app
            .get("request_count")
            .or_else(|| app.get("requests"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_in = app
            .get("bytes_received")
            .or_else(|| app.get("bytes_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_out = app
            .get("bytes_sent")
            .or_else(|| app.get("bytes_out"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let display_name = truncate_chars(name, 28);
        println!(
            "{:<30} {:>10} {:>12} {:>12}",
            display_name,
            requests,
            format_bytes(bytes_in),
            format_bytes(bytes_out)
        );
    }

    println!();
    println!("Total: {} applications", apps.len());

    Ok(())
}

fn show_hosts(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    let hosts = client
        .get_host_metrics()
        .map_err(bifrost_core::BifrostError::Config)?;

    if hosts.is_empty() {
        println!("No host metrics available.");
        return Ok(());
    }

    println!(
        "{:<40} {:>10} {:>12} {:>12}",
        "HOST", "REQUESTS", "BYTES IN", "BYTES OUT"
    );
    println!("{}", "-".repeat(76));

    for host in &hosts {
        let name = host
            .get("host")
            .or_else(|| host.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let requests = host
            .get("request_count")
            .or_else(|| host.get("requests"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_in = host
            .get("bytes_received")
            .or_else(|| host.get("bytes_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_out = host
            .get("bytes_sent")
            .or_else(|| host.get("bytes_out"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let display_name = truncate_chars(name, 38);
        println!(
            "{:<40} {:>10} {:>12} {:>12}",
            display_name,
            requests,
            format_bytes(bytes_in),
            format_bytes(bytes_out)
        );
    }

    println!();
    println!("Total: {} hosts", hosts.len());

    Ok(())
}

fn show_history(client: &ConfigApiClient, limit: Option<usize>) -> bifrost_core::Result<()> {
    let history = client
        .get_metrics_history(limit)
        .map_err(bifrost_core::BifrostError::Config)?;

    if history.is_empty() {
        println!("No metrics history available.");
        return Ok(());
    }

    println!(
        "{:<24} {:>10} {:>10} {:>12} {:>12}",
        "TIMESTAMP", "REQUESTS", "ACTIVE", "BYTES IN", "BYTES OUT"
    );
    println!("{}", "-".repeat(70));

    for snapshot in &history {
        let ts = snapshot
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let requests = snapshot
            .get("total_requests")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let active = snapshot
            .get("active_connections")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_in = snapshot
            .get("bytes_received")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let bytes_out = snapshot
            .get("bytes_sent")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let display_ts = truncate_chars(ts, 22);
        println!(
            "{:<24} {:>10} {:>10} {:>12} {:>12}",
            display_ts,
            requests,
            active,
            format_bytes(bytes_in),
            format_bytes(bytes_out)
        );
    }

    println!();
    println!("Showing {} snapshots", history.len());

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
