use serde::{Deserialize, Serialize};

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

pub fn run_status(format: crate::cli::StatusFormat) -> bifrost_core::Result<()> {
    let gathered = gather_status();
    match format {
        crate::cli::StatusFormat::Text => render_status_text(&gathered),
        crate::cli::StatusFormat::Json => render_status_json(&gathered, false),
        crate::cli::StatusFormat::JsonPretty => render_status_json(&gathered, true),
    }
    Ok(())
}

struct GatheredStatus {
    runtime_info: Option<RuntimeInfo>,
    is_running: bool,
    runtime_port: u16,
    system_proxy: SystemProxyStatus,
    tls_config: Result<TlsConfig, String>,
    proxy_address_info: Option<ProxyAddressInfo>,
    temporary_port_bindings: Result<Vec<TemporaryPortBinding>, String>,
    rule_groups: Result<Vec<RuleGroup>, String>,
    active_summary: Option<Result<ActiveSummaryResponse, String>>,
    data_dir: Option<std::path::PathBuf>,
}

fn gather_status() -> GatheredStatus {
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
    let rule_groups = if is_running {
        match fetch_rules_from_api(runtime_port) {
            Some(groups) => Ok(groups),
            None => Err("Unable to fetch rule information from running server".to_string()),
        }
    } else {
        Err("server not running".to_string())
    };
    let active_summary = if is_running {
        Some(fetch_active_summary_from_api(runtime_port).map_err(|e| e.to_string()))
    } else {
        None
    };
    let data_dir = crate::config::get_bifrost_dir().ok();

    GatheredStatus {
        runtime_info,
        is_running,
        runtime_port,
        system_proxy,
        tls_config,
        proxy_address_info,
        temporary_port_bindings,
        rule_groups,
        active_summary,
        data_dir,
    }
}

fn render_status_text(g: &GatheredStatus) {
    println!("Bifrost Proxy Status");
    println!("====================");

    println!();
    for line in format_service_overview_lines(
        g.is_running,
        g.runtime_info.as_ref(),
        &g.system_proxy,
        g.tls_config.as_ref().map_err(String::as_str),
        g.proxy_address_info.as_ref(),
    ) {
        println!("{}", line);
    }

    println!();
    println!("Runtime");
    println!("-------");

    let is_running = match &g.runtime_info {
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

    let runtime_port = g.runtime_port;
    println!("Default Port Rule Groups: {runtime_port}");
    println!("-------------------------");
    println!(
        "Scope: default/main proxy port {runtime_port}; temporary port bindings are listed separately at the bottom."
    );

    if is_running {
        match &g.rule_groups {
            Ok(groups) => {
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
            Err(_) => {
                println!("(Unable to fetch rule information from running server)");
            }
        }
    } else {
        println!("(Server not running, rule information unavailable)");
    }

    if is_running {
        let active_summary_lines = match &g.active_summary {
            Some(Ok(summary)) => {
                format_active_summary_status_block(true, runtime_port, Ok(summary))
            }
            Some(Err(err)) => format_active_summary_status_block(true, runtime_port, Err(err)),
            None => Vec::new(),
        };

        for line in active_summary_lines {
            println!("{}", line);
        }
    }

    for line in format_temporary_port_bindings_block(
        is_running,
        g.temporary_port_bindings.as_deref().map_err(String::as_str),
    ) {
        println!("{}", line);
    }
}

fn render_status_json(g: &GatheredStatus, pretty: bool) {
    let value = build_status_json(g);
    let serialized = if pretty {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    }
    .unwrap_or_else(|e| {
        format!(
            "{{\"schema_version\":1,\"error\":\"serialize_failed:{}\"}}",
            e
        )
    });
    println!("{}", serialized);
}

#[derive(Debug, Serialize)]
struct StatusJson {
    schema_version: u32,
    version: &'static str,
    running: bool,
    pid: Option<u32>,
    uptime_sec: Option<u64>,
    listener: Option<ListenerJson>,
    system_proxy: SystemProxyJson,
    tls: Option<TlsJson>,
    active_rules: Option<Vec<ActiveRuleGroupJson>>,
    data_dir: Option<String>,
    ports: Option<Vec<PortJson>>,
    errors: Vec<ErrorJson>,
}

#[derive(Debug, Serialize)]
struct ListenerJson {
    host: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    socks5_port: Option<u16>,
}

#[derive(Debug, Serialize)]
struct SystemProxyJson {
    supported: bool,
    enabled: bool,
    host: Option<String>,
    port: Option<u16>,
    bypass: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TlsJson {
    enabled: bool,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
    include_apps: Vec<String>,
    exclude_apps: Vec<String>,
    include_ips: Vec<String>,
    exclude_ips: Vec<String>,
    unsafe_ssl: bool,
    disconnect_on_config_change: bool,
}

#[derive(Debug, Serialize)]
struct ActiveRuleGroupJson {
    group: String,
    rule_count: usize,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct PortJson {
    port: u16,
    host: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    binding: String,
}

#[derive(Debug, Serialize)]
struct ErrorJson {
    source: String,
    message: String,
}

fn current_epoch_ms() -> Option<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn build_status_json(g: &GatheredStatus) -> StatusJson {
    let mut errors: Vec<ErrorJson> = Vec::new();

    let (pid, uptime_sec, listener) = if g.is_running {
        let info = g.runtime_info.as_ref();
        let pid = info.map(|i| i.pid);
        let listener = info.map(|i| ListenerJson {
            host: i.host.clone().unwrap_or_else(|| "127.0.0.1".to_string()),
            port: i.port,
            socks5_port: i.socks5_port,
        });
        let uptime_sec = info
            .and_then(|i| i.started_at_ms)
            .and_then(|started| current_epoch_ms().map(|now| now.saturating_sub(started) / 1000));
        (pid, uptime_sec, listener)
    } else {
        (None, None, None)
    };

    let system_proxy = SystemProxyJson {
        supported: g.system_proxy.supported,
        enabled: g.system_proxy.enabled,
        host: if g.system_proxy.supported && g.system_proxy.host.is_empty() {
            None
        } else if g.system_proxy.supported {
            Some(g.system_proxy.host.clone())
        } else {
            None
        },
        port: if g.system_proxy.supported && g.system_proxy.port != 0 {
            Some(g.system_proxy.port)
        } else {
            None
        },
        bypass: if g.system_proxy.supported {
            Some(g.system_proxy.bypass.clone())
        } else {
            None
        },
        error: g.system_proxy.error.clone(),
    };

    let tls = match &g.tls_config {
        Ok(cfg) => Some(TlsJson {
            enabled: cfg.enable_tls_interception,
            include_domains: cfg.intercept_include.clone(),
            exclude_domains: cfg.intercept_exclude.clone(),
            include_apps: cfg.app_intercept_include.clone(),
            exclude_apps: cfg.app_intercept_exclude.clone(),
            include_ips: cfg.ip_intercept_include.clone(),
            exclude_ips: cfg.ip_intercept_exclude.clone(),
            unsafe_ssl: cfg.unsafe_ssl,
            disconnect_on_config_change: cfg.disconnect_on_config_change,
        }),
        Err(message) => {
            if g.is_running {
                errors.push(ErrorJson {
                    source: "tls_config".to_string(),
                    message: message.clone(),
                });
            }
            None
        }
    };

    let active_rules = match &g.rule_groups {
        Ok(groups) => Some(
            groups
                .iter()
                .map(|gr| ActiveRuleGroupJson {
                    group: gr.name.clone(),
                    rule_count: gr.rule_count,
                    enabled: gr.enabled,
                })
                .collect(),
        ),
        Err(message) => {
            if g.is_running {
                errors.push(ErrorJson {
                    source: "rules".to_string(),
                    message: message.clone(),
                });
            }
            None
        }
    };

    let ports = match &g.temporary_port_bindings {
        Ok(bindings) => Some(
            bindings
                .iter()
                .map(|b| PortJson {
                    port: b.port,
                    host: b.host.clone(),
                    status: format_temp_port_status(&b.status).to_string(),
                    name: b.name.clone(),
                    binding: format!("{}:{}", b.host, b.port),
                })
                .collect(),
        ),
        Err(message) => {
            if g.is_running {
                errors.push(ErrorJson {
                    source: "ports".to_string(),
                    message: message.clone(),
                });
            }
            None
        }
    };

    if let Some(Err(message)) = &g.active_summary {
        errors.push(ErrorJson {
            source: "active_summary".to_string(),
            message: message.clone(),
        });
    }

    StatusJson {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION"),
        running: g.is_running,
        pid,
        uptime_sec,
        listener,
        system_proxy,
        tls,
        active_rules,
        data_dir: g.data_dir.as_ref().map(|p| p.display().to_string()),
        ports,
        errors,
    }
}
#[cfg(test)]
mod tests {
    use super::{
        build_status_json, format_active_summary_status_block, format_service_overview_lines,
        format_temporary_port_bindings_block, GatheredStatus, ProxyAddress, ProxyAddressInfo,
        RuleGroup, SystemProxyStatus, TlsConfig,
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

    fn sample_runtime() -> RuntimeInfo {
        RuntimeInfo {
            pid: 4242,
            port: 9900,
            socks5_port: Some(9901),
            host: Some("127.0.0.1".to_string()),
            started_at_ms: Some(1_700_000_000_000),
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        }
    }

    fn sample_tls() -> TlsConfig {
        TlsConfig {
            enable_tls_interception: true,
            intercept_exclude: vec!["corp.test".to_string()],
            intercept_include: vec!["api.example.test".to_string()],
            app_intercept_exclude: Vec::new(),
            app_intercept_include: vec!["Chrome".to_string()],
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            unsafe_ssl: false,
            disconnect_on_config_change: false,
        }
    }

    fn sample_system_proxy() -> SystemProxyStatus {
        SystemProxyStatus {
            supported: true,
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: "localhost".to_string(),
            error: None,
        }
    }

    #[test]
    fn build_status_json_running_serializes_schema_v1() {
        let runtime = sample_runtime();
        let tls = sample_tls();
        let system_proxy = sample_system_proxy();
        let proxy_address_info = ProxyAddressInfo {
            addresses: vec![ProxyAddress {
                address: "127.0.0.1:9900".to_string(),
            }],
        };
        let rule_groups = vec![
            RuleGroup {
                name: "default".to_string(),
                enabled: true,
                rule_count: 7,
            },
            RuleGroup {
                name: "disabled-grp".to_string(),
                enabled: false,
                rule_count: 0,
            },
        ];
        let binding = TemporaryPortBinding {
            port: 18890,
            host: "127.0.0.1".to_string(),
            name: Some("mobile".to_string()),
            status: TemporaryPortStatus::Running,
            rule_refs: vec![RuleSetRef::LocalRule {
                name: "local-rule".to_string(),
            }],
            missing_refs: Vec::new(),
            created_at: 1,
            updated_at: 2,
        };
        let gathered = GatheredStatus {
            runtime_info: Some(runtime),
            is_running: true,
            runtime_port: 9900,
            system_proxy,
            tls_config: Ok(tls),
            proxy_address_info: Some(proxy_address_info),
            temporary_port_bindings: Ok(vec![binding]),
            rule_groups: Ok(rule_groups),
            active_summary: Some(Ok(ActiveSummaryResponse {
                total: 0,
                rules: Vec::new(),
                variable_conflicts: Vec::new(),
                merged_content: String::new(),
            })),
            data_dir: Some(std::path::PathBuf::from("/tmp/bifrost")),
        };

        let json = build_status_json(&gathered);
        let v = serde_json::to_value(&json).expect("serialize");

        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["running"], true);
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["listener"]["host"], "127.0.0.1");
        assert_eq!(v["listener"]["port"], 9900);
        assert_eq!(v["listener"]["socks5_port"], 9901);
        assert_eq!(v["system_proxy"]["supported"], true);
        assert_eq!(v["system_proxy"]["enabled"], true);
        assert_eq!(v["system_proxy"]["host"], "127.0.0.1");
        assert_eq!(v["system_proxy"]["port"], 9900);
        assert_eq!(v["system_proxy"]["bypass"], "localhost");
        assert!(v["system_proxy"]["error"].is_null());
        assert_eq!(v["tls"]["enabled"], true);
        assert_eq!(v["tls"]["include_domains"][0], "api.example.test");
        assert_eq!(v["tls"]["exclude_domains"][0], "corp.test");
        assert_eq!(v["tls"]["include_apps"][0], "Chrome");
        assert_eq!(v["tls"]["unsafe_ssl"], false);
        assert_eq!(v["active_rules"][0]["group"], "default");
        assert_eq!(v["active_rules"][0]["rule_count"], 7);
        assert_eq!(v["active_rules"][0]["enabled"], true);
        assert_eq!(v["active_rules"][1]["enabled"], false);
        assert_eq!(v["data_dir"], "/tmp/bifrost");
        assert_eq!(v["ports"][0]["port"], 18890);
        assert_eq!(v["ports"][0]["host"], "127.0.0.1");
        assert_eq!(v["ports"][0]["binding"], "127.0.0.1:18890");
        assert_eq!(v["ports"][0]["status"], "running");
        assert!(v["errors"].as_array().unwrap().is_empty());
        assert!(v.get("version").is_some());
        assert!(v.get("uptime_sec").is_some());
    }

    #[test]
    fn build_status_json_stopped_omits_runtime_and_keeps_keys() {
        let system_proxy = SystemProxyStatus {
            supported: true,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            error: None,
        };
        let gathered = GatheredStatus {
            runtime_info: None,
            is_running: false,
            runtime_port: 9900,
            system_proxy,
            tls_config: Err("server not running".to_string()),
            proxy_address_info: None,
            temporary_port_bindings: Err("server not running".to_string()),
            rule_groups: Err("server not running".to_string()),
            active_summary: None,
            data_dir: None,
        };

        let json = build_status_json(&gathered);
        let v = serde_json::to_value(&json).expect("serialize");

        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["running"], false);
        assert!(v["pid"].is_null());
        assert!(v["listener"].is_null());
        assert!(v["tls"].is_null());
        assert!(v["active_rules"].is_null());
        assert!(v["ports"].is_null());
        assert_eq!(v["system_proxy"]["supported"], true);
        assert_eq!(v["system_proxy"]["enabled"], false);
        // No admin API calls were attempted while stopped → no errors recorded.
        assert!(v["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn build_status_json_partial_failures_recorded_in_errors() {
        let runtime = sample_runtime();
        let system_proxy = sample_system_proxy();
        let gathered = GatheredStatus {
            runtime_info: Some(runtime),
            is_running: true,
            runtime_port: 9900,
            system_proxy,
            tls_config: Err("tls api timeout".to_string()),
            proxy_address_info: None,
            temporary_port_bindings: Err("ports api 500".to_string()),
            rule_groups: Err("rules api closed".to_string()),
            active_summary: Some(Err("active summary 500".to_string())),
            data_dir: None,
        };

        let json = build_status_json(&gathered);
        let v = serde_json::to_value(&json).expect("serialize");

        assert_eq!(v["running"], true);
        assert!(v["tls"].is_null());
        assert!(v["active_rules"].is_null());
        assert!(v["ports"].is_null());
        let errors = v["errors"].as_array().expect("errors");
        let sources: Vec<&str> = errors
            .iter()
            .map(|e| e["source"].as_str().unwrap_or_default())
            .collect();
        assert!(sources.contains(&"tls_config"));
        assert!(sources.contains(&"rules"));
        assert!(sources.contains(&"ports"));
        assert!(sources.contains(&"active_summary"));
    }
}
