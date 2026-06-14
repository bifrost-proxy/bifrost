use serde::Deserialize;

use bifrost_admin::{RuleSetRef, TemporaryPortBinding, TemporaryPortStatus};

use super::rule::{
    fetch_active_summary_from_api, format_active_summary_lines, ActiveSummaryResponse,
};
use crate::process::{is_process_running, read_runtime_info, RuntimeInfo};

#[derive(Debug, Clone, Deserialize)]
struct TlsConfig {
    enable_tls_interception: bool,
    intercept_exclude: Vec<String>,
    intercept_include: Vec<String>,
    app_intercept_exclude: Vec<String>,
    app_intercept_include: Vec<String>,
    ip_intercept_exclude: Vec<String>,
    ip_intercept_include: Vec<String>,
    unsafe_ssl: bool,
    disconnect_on_config_change: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ProxyAddressInfo {
    addresses: Vec<ProxyAddress>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProxyAddress {
    address: String,
}

#[derive(Debug, Clone)]
struct SystemProxyStatus {
    supported: bool,
    enabled: bool,
    host: String,
    port: u16,
    bypass: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuleGroup {
    name: String,
    enabled: bool,
    rule_count: usize,
}

fn fetch_rules_from_api(port: u16) -> Option<Vec<RuleGroup>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/rules", port);
    let response = bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .get(&url)
        .call();
    match response {
        Ok(resp) => resp.into_json().ok(),
        Err(_) => None,
    }
}

fn fetch_json_from_api<T>(port: u16, path: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let url = format!("http://127.0.0.1:{}/_bifrost/api{}", port, path);
    let response = bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?;

    response.into_json().map_err(|e| e.to_string())
}

fn fetch_tls_config_from_api(port: u16) -> Result<TlsConfig, String> {
    fetch_json_from_api(port, "/config/tls")
}

fn fetch_proxy_address_info_from_api(port: u16) -> Result<ProxyAddressInfo, String> {
    fetch_json_from_api(port, "/proxy/address")
}

fn fetch_temporary_port_bindings_from_api(port: u16) -> Result<Vec<TemporaryPortBinding>, String> {
    fetch_json_from_api(port, "/ports")
}

fn read_system_proxy_status() -> SystemProxyStatus {
    if !bifrost_core::SystemProxyManager::is_supported() {
        return SystemProxyStatus {
            supported: false,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            error: None,
        };
    }

    match bifrost_core::SystemProxyManager::get_current() {
        Ok(proxy) => SystemProxyStatus {
            supported: true,
            enabled: proxy.enable,
            host: proxy.host,
            port: proxy.port,
            bypass: proxy.bypass,
            error: None,
        },
        Err(e) => SystemProxyStatus {
            supported: true,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            error: Some(e.to_string()),
        },
    }
}

fn client_proxy_host(runtime_info: Option<&RuntimeInfo>) -> String {
    match runtime_info.and_then(|info| info.host.as_deref()) {
        Some("0.0.0.0") | Some("[::]") | Some("::") | None | Some("") => "127.0.0.1".to_string(),
        Some(host) => host.to_string(),
    }
}

fn fallback_proxy_address(runtime_info: Option<&RuntimeInfo>) -> Option<String> {
    runtime_info.map(|info| format!("http://{}:{}", client_proxy_host(Some(info)), info.port))
}

fn listen_proxy_address(runtime_info: Option<&RuntimeInfo>) -> Option<String> {
    runtime_info.map(|info| {
        format!(
            "{}:{}",
            info.host.as_deref().unwrap_or("127.0.0.1"),
            info.port
        )
    })
}

fn format_api_proxy_addresses(address_info: &ProxyAddressInfo) -> Vec<String> {
    let mut addresses = address_info
        .addresses
        .iter()
        .map(|entry| format!("http://{}", entry.address))
        .collect::<Vec<_>>();

    addresses.sort();
    addresses.dedup();
    addresses
}

fn local_proxy_address(runtime_info: Option<&RuntimeInfo>) -> String {
    runtime_info
        .map(|info| format!("http://127.0.0.1:{}", info.port))
        .unwrap_or_else(|| "unavailable (server not running)".to_string())
}

fn runtime_binds_lan(runtime_info: Option<&RuntimeInfo>) -> bool {
    matches!(
        runtime_info.and_then(|info| info.host.as_deref()),
        Some("0.0.0.0") | Some("[::]") | Some("::")
    )
}

fn format_lan_proxy_addresses(
    runtime_info: Option<&RuntimeInfo>,
    proxy_address_info: Option<&ProxyAddressInfo>,
) -> String {
    let Some(info) = runtime_info else {
        return "unavailable (server not running)".to_string();
    };

    if runtime_binds_lan(Some(info)) {
        if let Some(address_info) = proxy_address_info {
            let addresses = format_api_proxy_addresses(address_info)
                .into_iter()
                .filter(|address| {
                    !address.starts_with("http://127.")
                        && !address.starts_with("http://localhost:")
                        && !address.starts_with("http://[::1]:")
                })
                .collect::<Vec<_>>();
            if !addresses.is_empty() {
                return addresses.join(", ");
            }
        }

        return "none detected".to_string();
    }

    let host = info.host.as_deref().unwrap_or("127.0.0.1");
    if host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]" {
        "unavailable (service is listening on localhost only)".to_string()
    } else {
        format!("http://{}:{}", host, info.port)
    }
}

fn list_preview(entries: &[String]) -> String {
    if entries.is_empty() {
        return "none".to_string();
    }

    let shown = entries
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} [{}]", entries.len(), shown)
}

fn format_tls_config_line(tls_config: Result<&TlsConfig, &str>) -> String {
    match tls_config {
        Ok(config) => format!(
            "TLS Interception: {} (upstream cert verification: {}; disconnect on config change: {})",
            if config.enable_tls_interception {
                "Enabled"
            } else {
                "Disabled"
            },
            if config.unsafe_ssl {
                "skipped"
            } else {
                "strict"
            },
            if config.disconnect_on_config_change {
                "enabled"
            } else {
                "disabled"
            }
        ),
        Err(message) => format!("TLS Interception: Unknown ({message})"),
    }
}

fn format_tls_whitelist_lines(tls_config: Result<&TlsConfig, &str>) -> Vec<String> {
    match tls_config {
        Ok(config) => vec![
            format!(
                "TLS Domain Whitelist: {}",
                list_preview(&config.intercept_include)
            ),
            format!(
                "TLS App Whitelist: {}",
                list_preview(&config.app_intercept_include)
            ),
            format!(
                "TLS IP Whitelist: {}",
                list_preview(&config.ip_intercept_include)
            ),
        ],
        Err(message) => vec![
            format!("TLS Domain Whitelist: Unknown ({message})"),
            format!("TLS App Whitelist: Unknown ({message})"),
            format!("TLS IP Whitelist: Unknown ({message})"),
        ],
    }
}

fn format_tls_boundary_lines(tls_config: Result<&TlsConfig, &str>) -> Vec<String> {
    match tls_config {
        Ok(config) => vec![
            format!(
                "TLS Domain Passthrough: {}",
                list_preview(&config.intercept_exclude)
            ),
            format!(
                "TLS App Passthrough: {}",
                list_preview(&config.app_intercept_exclude)
            ),
            format!(
                "TLS IP Passthrough: {}",
                list_preview(&config.ip_intercept_exclude)
            ),
        ],
        Err(message) => vec![
            format!("TLS Domain Passthrough: Unknown ({message})"),
            format!("TLS App Passthrough: Unknown ({message})"),
            format!("TLS IP Passthrough: Unknown ({message})"),
        ],
    }
}

fn host_matches_system_proxy(proxy_host: &str, runtime_host: &str) -> bool {
    let proxy_host = proxy_host
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let runtime_host = runtime_host
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();

    if proxy_host == runtime_host {
        return true;
    }

    matches!(
        (proxy_host.as_str(), runtime_host.as_str()),
        ("localhost", "127.0.0.1")
            | ("127.0.0.1", "localhost")
            | ("::1", "127.0.0.1")
            | ("127.0.0.1", "::1")
            | ("::1", "localhost")
            | ("localhost", "::1")
    )
}

fn format_system_proxy_line(
    system_proxy: &SystemProxyStatus,
    runtime_info: Option<&RuntimeInfo>,
) -> String {
    if !system_proxy.supported {
        return "System Proxy: Unsupported on this platform".to_string();
    }

    if let Some(error) = &system_proxy.error {
        return format!("System Proxy: Unknown ({error})");
    }

    if !system_proxy.enabled {
        return "System Proxy: Disabled".to_string();
    }

    let target = format!("{}:{}", system_proxy.host, system_proxy.port);
    let matches_bifrost = runtime_info.is_some_and(|info| {
        system_proxy.port == info.port
            && host_matches_system_proxy(&system_proxy.host, &client_proxy_host(Some(info)))
    });

    let mut line = if matches_bifrost {
        format!("System Proxy: Enabled -> {target} (points to this Bifrost service)")
    } else {
        format!("System Proxy: Enabled -> {target} (does not point to this Bifrost service)")
    };

    if !system_proxy.bypass.is_empty() {
        line.push_str(&format!("; bypass={}", system_proxy.bypass));
    }

    line
}

fn format_service_overview_lines(
    is_running: bool,
    runtime_info: Option<&RuntimeInfo>,
    system_proxy: &SystemProxyStatus,
    tls_config: Result<&TlsConfig, &str>,
    proxy_address_info: Option<&ProxyAddressInfo>,
) -> Vec<String> {
    let running_runtime_info = is_running.then_some(()).and(runtime_info);
    let mut lines = vec![
        "Service Overview".to_string(),
        "----------------".to_string(),
        format!(
            "Proxy Local Address: {}",
            local_proxy_address(running_runtime_info)
        ),
        format!(
            "Proxy LAN Addresses: {}",
            format_lan_proxy_addresses(running_runtime_info, proxy_address_info)
        ),
    ];

    if let Some(address) = listen_proxy_address(running_runtime_info) {
        lines.push(format!("Proxy Listen Address: {address}"));
    }

    if let Some(address) = fallback_proxy_address(running_runtime_info) {
        lines.push(format!("Proxy Client Fallback Address: {address}"));
    } else if !is_running {
        lines.push("Proxy Listen Address: unavailable (server not running)".to_string());
    }

    lines.push(format_system_proxy_line(system_proxy, running_runtime_info));
    lines.push(format_tls_config_line(tls_config));
    lines.extend(format_tls_whitelist_lines(tls_config));
    lines.extend(format_tls_boundary_lines(tls_config));
    lines
}

fn inline_rule_label(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = if compact.chars().count() > 80 {
        let prefix = compact.chars().take(80).collect::<String>();
        format!("{prefix}...")
    } else {
        compact
    };
    format!("inline:{preview}")
}

fn format_rule_ref(rule_ref: &RuleSetRef) -> String {
    match rule_ref {
        RuleSetRef::LocalRule { name } => format!("local:{name}"),
        RuleSetRef::GroupRule { group_id, name } => format!("group:{group_id}/{name}"),
        RuleSetRef::RuleFile { path } => format!("file:{path}"),
        RuleSetRef::InlineRule { content } => inline_rule_label(content),
    }
}

fn format_temp_port_status(status: &TemporaryPortStatus) -> &'static str {
    match status {
        TemporaryPortStatus::Running => "running",
        TemporaryPortStatus::Degraded => "degraded",
    }
}

fn format_temporary_port_bindings_block(
    is_running: bool,
    bindings: Result<&[TemporaryPortBinding], &str>,
) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        "Temporary Port Bindings".to_string(),
        "-----------------------".to_string(),
    ];

    if !is_running {
        lines.push("(Server not running, temporary port bindings unavailable)".to_string());
        return lines;
    }

    let bindings = match bindings {
        Ok(bindings) => bindings,
        Err(message) => {
            lines.push(format!(
                "(Unable to fetch temporary port bindings from running server: {message})"
            ));
            return lines;
        }
    };

    if bindings.is_empty() {
        lines.push("No temporary port bindings.".to_string());
        return lines;
    }

    for binding in bindings {
        let name = binding
            .name
            .as_deref()
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        lines.push(format!(
            "- {}:{} [{}]{}",
            binding.host,
            binding.port,
            format_temp_port_status(&binding.status),
            name
        ));
        lines.push("  Rules:".to_string());
        for rule_ref in &binding.rule_refs {
            lines.push(format!("    - {}", format_rule_ref(rule_ref)));
        }
        if !binding.missing_refs.is_empty() {
            lines.push("  Missing Rules:".to_string());
            for rule_ref in &binding.missing_refs {
                lines.push(format!("    - {}", format_rule_ref(rule_ref)));
            }
        }
    }

    lines
}

fn format_active_summary_status_block(
    is_running: bool,
    default_port: u16,
    active_summary: Result<&ActiveSummaryResponse, &str>,
) -> Vec<String> {
    if !is_running {
        return Vec::new();
    }

    let mut lines = vec![String::new()];
    match active_summary {
        Ok(summary) => {
            let mut summary_lines = format_active_summary_lines(summary);
            if let Some(title) = summary_lines.first_mut() {
                *title = format!("Default Port Active Rules: {default_port}");
            }
            if let Some(underline) = summary_lines.get_mut(1) {
                *underline = "==========================".to_string();
            }
            summary_lines.insert(
                3,
                format!(
                    "Scope: default/main proxy port {default_port}; temporary port bindings are listed separately at the bottom."
                ),
            );
            summary_lines.insert(4, String::new());
            for line in &mut summary_lines {
                if line == "Merged Rules (in parsing order)" {
                    *line = format!("Default Port Merged Rules (in parsing order): {default_port}");
                }
            }
            lines.extend(summary_lines);
        }
        Err(message) => {
            lines.push(format!("Default Port Active Rules: {default_port}"));
            lines.push("==========================".to_string());
            lines.push(String::new());
            lines.push(format!(
                "(Unable to fetch default port active rule summary from running server: {message})"
            ));
        }
    }

    lines
}

pub fn run_status() -> bifrost_core::Result<()> {
    println!("Bifrost Proxy Status");
    println!("====================");

    let runtime_info = read_runtime_info();

    let is_running = runtime_info
        .as_ref()
        .is_some_and(|info| is_process_running(info.pid));
    let runtime_port = runtime_info.as_ref().map(|info| info.port).unwrap_or(9900);
    let system_proxy = read_system_proxy_status();
    let tls_config = if is_running {
        fetch_tls_config_from_api(runtime_port)
    } else {
        Err("server not running".to_string())
    };
    let proxy_address_info = if is_running {
        fetch_proxy_address_info_from_api(runtime_port).ok()
    } else {
        None
    };
    let temporary_port_bindings = if is_running {
        fetch_temporary_port_bindings_from_api(runtime_port)
    } else {
        Err("server not running".to_string())
    };

    println!();
    for line in format_service_overview_lines(
        is_running,
        runtime_info.as_ref(),
        &system_proxy,
        tls_config.as_ref().map_err(String::as_str),
        proxy_address_info.as_ref(),
    ) {
        println!("{}", line);
    }

    println!();
    println!("Runtime");
    println!("-------");

    let is_running = match &runtime_info {
        Some(info) => {
            if is_process_running(info.pid) {
                println!("Status: Running");
                println!("PID: {}", info.pid);
                println!("Port: {}", info.port);
                if let Some(ref host) = info.host {
                    println!("Host: {}", host);
                }
                if let Some(socks5_port) = info.socks5_port {
                    println!("SOCKS5 Port: {}", socks5_port);
                }
                true
            } else {
                println!("Status: Stopped (stale PID file exists)");
                println!("Stale PID: {}", info.pid);
                false
            }
        }
        None => {
            println!("Status: Stopped");
            false
        }
    };

    println!();

    println!("Default Port Rule Groups: {runtime_port}");
    println!("-------------------------");
    println!(
        "Scope: default/main proxy port {runtime_port}; temporary port bindings are listed separately at the bottom."
    );

    if is_running {
        match fetch_rules_from_api(runtime_port) {
            Some(groups) => {
                let enabled_groups: Vec<_> = groups.iter().filter(|g| g.enabled).collect();
                let disabled_groups: Vec<_> = groups.iter().filter(|g| !g.enabled).collect();

                println!("Enabled rule groups: {}", enabled_groups.len());
                for group in &enabled_groups {
                    println!("  - {} ({} rules)", group.name, group.rule_count);
                }

                if !disabled_groups.is_empty() {
                    println!("Disabled rule groups: {}", disabled_groups.len());
                    for group in &disabled_groups {
                        println!("  - {} ({} rules)", group.name, group.rule_count);
                    }
                }
            }
            None => {
                println!("(Unable to fetch rule information from running server)");
            }
        }
    } else {
        println!("(Server not running, rule information unavailable)");
    }

    if is_running {
        let active_summary_lines = match fetch_active_summary_from_api(runtime_port) {
            Ok(summary) => format_active_summary_status_block(true, runtime_port, Ok(&summary)),
            Err(err) => {
                let err_message = err.to_string();
                format_active_summary_status_block(true, runtime_port, Err(&err_message))
            }
        };

        for line in active_summary_lines {
            println!("{}", line);
        }
    }

    for line in format_temporary_port_bindings_block(
        is_running,
        temporary_port_bindings.as_deref().map_err(String::as_str),
    ) {
        println!("{}", line);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        format_active_summary_status_block, format_service_overview_lines,
        format_temporary_port_bindings_block, ProxyAddress, ProxyAddressInfo, SystemProxyStatus,
        TlsConfig,
    };
    use crate::commands::rule::{ActiveRuleItem, ActiveSummaryResponse};
    use crate::process::RuntimeInfo;
    use bifrost_admin::{RuleSetRef, TemporaryPortBinding, TemporaryPortStatus};

    #[test]
    fn status_running_includes_active_summary_block() {
        let summary = ActiveSummaryResponse {
            total: 1,
            rules: vec![ActiveRuleItem {
                name: "demo".to_string(),
                rule_count: 2,
                group_id: None,
                group_name: None,
            }],
            variable_conflicts: Vec::new(),
            merged_content: "example.com statusCode://200".to_string(),
        };

        let lines = format_active_summary_status_block(true, 9900, Ok(&summary));
        let output = lines.join("\n");

        assert!(output.contains("Default Port Active Rules: 9900"));
        assert!(output.contains("Scope: default/main proxy port 9900; temporary port bindings"));
        assert!(output.contains("Default Port Merged Rules (in parsing order): 9900"));
        assert!(output.contains("example.com statusCode://200"));
    }

    #[test]
    fn status_stopped_does_not_include_active_summary_block() {
        let summary = ActiveSummaryResponse {
            total: 1,
            rules: vec![ActiveRuleItem {
                name: "demo".to_string(),
                rule_count: 1,
                group_id: None,
                group_name: None,
            }],
            variable_conflicts: Vec::new(),
            merged_content: "example.com statusCode://200".to_string(),
        };

        let lines = format_active_summary_status_block(false, 9900, Ok(&summary));

        assert!(lines.is_empty());
    }

    #[test]
    fn service_overview_includes_proxy_system_proxy_and_tls_boundaries() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 18888,
            socks5_port: Some(18889),
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        };
        let system_proxy = SystemProxyStatus {
            supported: true,
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 18888,
            bypass: "localhost,127.0.0.1".to_string(),
            error: None,
        };
        let tls = TlsConfig {
            enable_tls_interception: true,
            intercept_exclude: vec!["*.internal.test".to_string()],
            intercept_include: vec![
                "api.example.test".to_string(),
                "admin.example.test".to_string(),
                "search.example.test".to_string(),
                "docs.example.test".to_string(),
                "cloud.example.test".to_string(),
                "status.example.test".to_string(),
            ],
            app_intercept_exclude: vec!["Safari".to_string()],
            app_intercept_include: vec![
                "Chrome".to_string(),
                "Microsoft Edge".to_string(),
                "Postman".to_string(),
                "Safari".to_string(),
                "Firefox".to_string(),
                "server".to_string(),
            ],
            ip_intercept_exclude: vec!["10.0.0.2".to_string()],
            ip_intercept_include: vec!["10.0.0.1".to_string()],
            unsafe_ssl: true,
            disconnect_on_config_change: true,
        };

        let output =
            format_service_overview_lines(true, Some(&runtime), &system_proxy, Ok(&tls), None)
                .join("\n");

        assert!(output.contains("Proxy Local Address: http://127.0.0.1:18888"));
        assert!(output.contains("Proxy LAN Addresses: none detected"));
        assert!(output.contains("System Proxy: Enabled -> 127.0.0.1:18888"));
        assert!(output.contains("points to this Bifrost service"));
        assert!(output.contains("TLS Interception: Enabled"));
        assert!(output.contains(
            "TLS Domain Whitelist: 6 [api.example.test, admin.example.test, search.example.test, docs.example.test, cloud.example.test, status.example.test]"
        ));
        assert!(output.contains(
            "TLS App Whitelist: 6 [Chrome, Microsoft Edge, Postman, Safari, Firefox, server]"
        ));
        assert!(output.contains("TLS Domain Passthrough: 1 [*.internal.test]"));
        assert!(output.contains("TLS App Passthrough: 1 [Safari]"));
        assert!(!output.contains("... +"));
    }

    #[test]
    fn service_overview_stopped_marks_runtime_fields_unknown_without_hiding_system_proxy() {
        let system_proxy = SystemProxyStatus {
            supported: true,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            error: None,
        };

        let output = format_service_overview_lines(
            false,
            None,
            &system_proxy,
            Err("server not running"),
            None,
        )
        .join("\n");

        assert!(output.contains("Proxy Local Address: unavailable (server not running)"));
        assert!(output.contains("Proxy LAN Addresses: unavailable (server not running)"));
        assert!(output.contains("System Proxy: Disabled"));
        assert!(output.contains("TLS Interception: Unknown (server not running)"));
        assert!(output.contains("TLS Domain Whitelist: Unknown (server not running)"));
        assert!(output.contains("TLS App Whitelist: Unknown (server not running)"));
    }

    #[test]
    fn service_overview_lists_localhost_and_lan_addresses_separately() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 18888,
            socks5_port: None,
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        };
        let addresses = ProxyAddressInfo {
            addresses: vec![
                ProxyAddress {
                    address: "127.0.0.1:18888".to_string(),
                },
                ProxyAddress {
                    address: "192.168.1.22:18888".to_string(),
                },
            ],
        };
        let system_proxy = SystemProxyStatus {
            supported: true,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            error: None,
        };
        let tls = TlsConfig {
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            unsafe_ssl: false,
            disconnect_on_config_change: true,
        };

        let output = format_service_overview_lines(
            true,
            Some(&runtime),
            &system_proxy,
            Ok(&tls),
            Some(&addresses),
        )
        .join("\n");

        assert!(output.contains("Proxy Local Address: http://127.0.0.1:18888"));
        assert!(output.contains("Proxy LAN Addresses: http://192.168.1.22:18888"));
    }

    #[test]
    fn temporary_port_bindings_block_lists_bound_rules_per_port() {
        let binding = TemporaryPortBinding {
            port: 18890,
            host: "127.0.0.1".to_string(),
            name: Some("mobile-debug".to_string()),
            status: TemporaryPortStatus::Running,
            rule_refs: vec![
                RuleSetRef::LocalRule {
                    name: "mobile-local".to_string(),
                },
                RuleSetRef::InlineRule {
                    content: "mobile.test status://218 resBody://(mobile)".to_string(),
                },
            ],
            missing_refs: vec![RuleSetRef::GroupRule {
                group_id: "group-a".to_string(),
                name: "missing-rule".to_string(),
            }],
            created_at: 1,
            updated_at: 2,
        };

        let output = format_temporary_port_bindings_block(true, Ok(&[binding])).join("\n");

        assert!(output.contains("Temporary Port Bindings"));
        assert!(output.contains("- 127.0.0.1:18890 [running] (mobile-debug)"));
        assert!(output.contains("    - local:mobile-local"));
        assert!(output.contains("    - inline:mobile.test status://218 resBody://(mobile)"));
        assert!(output.contains("  Missing Rules:"));
        assert!(output.contains("    - group:group-a/missing-rule"));
    }

    #[test]
    fn temporary_port_bindings_block_handles_empty_and_stopped_states() {
        let empty = format_temporary_port_bindings_block(true, Ok(&[])).join("\n");
        assert!(empty.contains("No temporary port bindings."));

        let stopped =
            format_temporary_port_bindings_block(false, Err("server not running")).join("\n");
        assert!(stopped.contains("temporary port bindings unavailable"));
    }
}
