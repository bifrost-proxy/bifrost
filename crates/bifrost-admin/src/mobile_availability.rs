use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::net::IpAddr;
use std::time::Duration;

use chrono::Utc;
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::handlers::trust_probe::{
    get_or_create_terminal_probe_session, infer_device_platform_hint, list_active_sessions,
    TrustProbeProxyAccessStatus, TrustProbeSessionView, TrustProbeStatus,
};
use crate::network;
use crate::push::{
    SharedPushManager, SETTINGS_SCOPE_PENDING_AUTHORIZATIONS, SETTINGS_SCOPE_TRUST_PROBE,
    SETTINGS_SCOPE_WHITELIST_STATUS,
};
use crate::state::{SharedAccessControl, SharedAdminState};

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const DEFAULT_DEVICE_ACTIVE_WINDOW: Duration = Duration::from_secs(75);
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_RESET: &str = "\x1b[0m";

#[derive(Debug, Clone)]
pub struct MobileAvailabilityTerminalOptions {
    pub refresh_interval: Duration,
    pub device_active_window: Duration,
}

impl Default for MobileAvailabilityTerminalOptions {
    fn default() -> Self {
        Self {
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            device_active_window: DEFAULT_DEVICE_ACTIVE_WINDOW,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAvailabilitySnapshot {
    pub entries: Vec<MobileAvailabilityEntry>,
    pub devices: Vec<MobileAvailabilityDevice>,
    pub pending_authorizations: Vec<MobileAvailabilityPendingAuth>,
    pub access_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAvailabilityEntry {
    pub ip: String,
    pub is_preferred: bool,
    pub landing_url: Option<String>,
    pub terminal_qr: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAvailabilityDevice {
    pub label: String,
    pub client_ip: Option<String>,
    pub source_host: String,
    pub user_agent: Option<String>,
    pub certificate_status: String,
    pub proxy_access_status: String,
    pub proxy_config_status: String,
    pub network_status: String,
    pub last_seen_seconds_ago: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAvailabilityPendingAuth {
    pub ip: String,
    pub attempt_count: u32,
    pub first_seen_seconds_ago: u64,
}

#[derive(Default)]
struct TerminalRenderState {
    rendered_line_count: usize,
    force_render: bool,
}

pub fn spawn_terminal_panel(
    state: SharedAdminState,
    access_control: SharedAccessControl,
    push_manager: SharedPushManager,
) -> Vec<tokio::task::JoinHandle<()>> {
    if !should_render_terminal_panel(io::stdout().is_terminal()) {
        return Vec::new();
    }

    let options = MobileAvailabilityTerminalOptions::default();
    let is_dynamic_terminal = io::stdout().is_terminal();
    let render_state = std::sync::Arc::new(tokio::sync::Mutex::new(TerminalRenderState::default()));
    let render_task = tokio::spawn(render_terminal_panel_loop(
        state,
        access_control.clone(),
        options,
        render_state.clone(),
    ));
    let mut tasks = vec![render_task];
    if is_dynamic_terminal && io::stdin().is_terminal() {
        tasks.push(tokio::spawn(handle_access_control_input_loop(
            access_control,
            push_manager,
            render_state,
        )));
    }
    tasks
}

fn should_render_terminal_panel(stdout_is_terminal: bool) -> bool {
    stdout_is_terminal || std::env::var_os("BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE").is_some()
}

async fn render_terminal_panel_loop(
    state: SharedAdminState,
    access_control: SharedAccessControl,
    options: MobileAvailabilityTerminalOptions,
    render_state: std::sync::Arc<tokio::sync::Mutex<TerminalRenderState>>,
) {
    let mut last_content = String::new();
    let mut last_rendered_pending_signature: Option<String> = None;
    let is_terminal = io::stdout().is_terminal();

    loop {
        let snapshot = build_mobile_availability_snapshot(
            &state,
            &access_control,
            options.device_active_window,
        )
        .await;
        let content = render_mobile_availability_panel_for_terminal(&snapshot, is_terminal);
        let pending_signature = pending_authorization_signature(&snapshot);
        {
            let mut render_state = render_state.lock().await;
            let should_render = should_render_panel_update(
                is_terminal,
                content != last_content,
                render_state.force_render,
                pending_signature.as_deref(),
                last_rendered_pending_signature.as_deref(),
            );
            if should_render {
                if is_terminal && render_state.rendered_line_count > 0 {
                    print!("\x1b[{}A\x1b[J", render_state.rendered_line_count);
                }
                println!("{content}");
                let _ = io::stdout().flush();
                render_state.rendered_line_count = content.lines().count() + 1;
                render_state.force_render = false;
                last_content = content;
                last_rendered_pending_signature = pending_signature;
            }
        }

        if !is_terminal {
            return;
        }

        tokio::time::sleep(options.refresh_interval).await;
    }
}

fn should_render_panel_update(
    is_terminal: bool,
    content_changed: bool,
    force_render: bool,
    pending_signature: Option<&str>,
    last_rendered_pending_signature: Option<&str>,
) -> bool {
    if force_render {
        return true;
    }
    if !content_changed {
        return false;
    }
    if is_terminal
        && pending_signature.is_some()
        && pending_signature == last_rendered_pending_signature
    {
        return false;
    }
    true
}

async fn handle_access_control_input_loop(
    access_control: SharedAccessControl,
    push_manager: SharedPushManager,
    render_state: std::sync::Arc<tokio::sync::Mutex<TerminalRenderState>>,
) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => break,
        };
        let message = match parse_access_control_command(&line) {
            Ok(Some(command)) => {
                execute_access_control_command(command, &access_control, &push_manager).await
            }
            Ok(None) => continue,
            Err(error) => format!("Access Control: {error}"),
        };
        let mut render_state = render_state.lock().await;
        println!("{message}");
        let _ = io::stdout().flush();
        render_state.rendered_line_count = 0;
        render_state.force_render = true;
    }
}

pub async fn build_mobile_availability_snapshot(
    state: &SharedAdminState,
    access_control: &SharedAccessControl,
    device_active_window: Duration,
) -> MobileAvailabilitySnapshot {
    let local_ips = network::get_effective_local_ips();
    let mut entries = Vec::new();
    let mut active_hosts = HashSet::new();
    for local_ip in local_ips {
        active_hosts.insert(local_ip.ip.clone());
        match get_or_create_terminal_probe_session(state, &local_ip.ip).await {
            Ok(session) => entries.push(MobileAvailabilityEntry {
                terminal_qr: render_terminal_qr(&short_trust_probe_url(&local_ip.ip, state.port()))
                    .ok(),
                ip: local_ip.ip,
                is_preferred: local_ip.is_preferred,
                landing_url: Some(session.landing_url().to_string()),
                error: None,
            }),
            Err(error) => entries.push(MobileAvailabilityEntry {
                ip: local_ip.ip,
                is_preferred: local_ip.is_preferred,
                landing_url: None,
                terminal_qr: None,
                error: Some(error),
            }),
        }
    }

    let (access_mode, pending_authorizations, temporary_whitelist) = {
        let access_control = access_control.read().await;
        let now = unix_timestamp_seconds();
        let mut pending = access_control.get_pending_authorizations();
        pending.sort_by_key(|pending| pending.first_seen);
        let pending = pending
            .into_iter()
            .map(|pending| MobileAvailabilityPendingAuth {
                ip: pending.ip,
                attempt_count: pending.attempt_count,
                first_seen_seconds_ago: now.saturating_sub(pending.first_seen),
            })
            .collect::<Vec<_>>();
        let temporary = access_control
            .temporary_whitelist_entries()
            .into_iter()
            .collect::<HashSet<_>>();
        (access_control.mode().to_string(), pending, temporary)
    };
    let pending_by_ip = pending_authorizations
        .iter()
        .map(|pending| pending.ip.as_str())
        .collect::<HashSet<_>>();

    let devices = collect_recent_devices(
        list_active_sessions(),
        &active_hosts,
        &pending_by_ip,
        &temporary_whitelist,
        device_active_window,
    );

    MobileAvailabilitySnapshot {
        entries,
        devices,
        pending_authorizations,
        access_mode,
    }
}

fn collect_recent_devices(
    sessions: Vec<TrustProbeSessionView>,
    active_hosts: &HashSet<String>,
    pending_by_ip: &HashSet<&str>,
    temporary_whitelist: &HashSet<IpAddr>,
    active_window: Duration,
) -> Vec<MobileAvailabilityDevice> {
    let now = Utc::now();
    let mut devices = Vec::new();

    for session in sessions {
        if !active_hosts.contains(session.host()) {
            continue;
        }
        for device in session.devices() {
            let age = now.signed_duration_since(device.last_seen()).num_seconds();
            if age < 0 || age as u64 > active_window.as_secs() {
                continue;
            }
            let mut label = recent_device_label(
                device.platform_hint(),
                device.user_agent(),
                device.device_id(),
            );
            if label.chars().count() > 64 {
                label = label.chars().take(61).collect::<String>();
                label.push_str("...");
            }
            let client_ip = device.client_ip().map(str::to_string);
            devices.push(MobileAvailabilityDevice {
                label,
                client_ip: client_ip.clone(),
                source_host: session.host().to_string(),
                user_agent: device.user_agent().map(str::to_string),
                certificate_status: certificate_status_text(
                    device.status(),
                    device.tls_trusted(),
                    device.opened(),
                ),
                proxy_access_status: proxy_access_status_text(
                    device.proxy_access_status(),
                    client_ip.as_deref(),
                    pending_by_ip,
                    temporary_whitelist,
                ),
                proxy_config_status: proxy_config_status_text(device.proxy_configured()),
                network_status: network_status_text(device.status(), device.network_reachable()),
                last_seen_seconds_ago: age,
            });
        }
    }

    dedupe_recent_devices(devices)
}

fn dedupe_recent_devices(devices: Vec<MobileAvailabilityDevice>) -> Vec<MobileAvailabilityDevice> {
    let mut by_key = HashMap::<String, MobileAvailabilityDevice>::new();
    for device in devices {
        let key = mobile_device_dedupe_key(&device);
        match by_key.get_mut(&key) {
            Some(existing) if device.last_seen_seconds_ago < existing.last_seen_seconds_ago => {
                *existing = device;
            }
            Some(_) => {}
            None => {
                by_key.insert(key, device);
            }
        }
    }
    let mut devices = by_key.into_values().collect::<Vec<_>>();
    devices.sort_by(compare_mobile_devices);
    devices
}

fn compare_mobile_devices(
    left: &MobileAvailabilityDevice,
    right: &MobileAvailabilityDevice,
) -> std::cmp::Ordering {
    compare_optional_ip(left.client_ip.as_deref(), right.client_ip.as_deref())
        .then_with(|| left.source_host.cmp(&right.source_host))
        .then_with(|| left.label.cmp(&right.label))
}

fn compare_optional_ip(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    match (left.and_then(parse_ip_addr), right.and_then(parse_ip_addr)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.unwrap_or_default().cmp(right.unwrap_or_default()),
    }
}

fn parse_ip_addr(value: &str) -> Option<IpAddr> {
    value.parse::<IpAddr>().ok()
}

fn recent_device_label(
    platform_hint: Option<&str>,
    user_agent: Option<&str>,
    device_id: &str,
) -> String {
    platform_hint
        .filter(|value| !is_unknown_platform_label(value))
        .map(str::to_string)
        .or_else(|| user_agent.and_then(infer_device_platform_hint))
        .or_else(|| user_agent.map(str::to_string))
        .filter(|value| !is_unknown_platform_label(value))
        .unwrap_or_else(|| device_id.to_string())
}

fn is_unknown_platform_label(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.is_empty()
        || value == "unknown"
        || value == "unknown browser"
        || value == "unknown device"
        || value == "unknown platform"
}

fn mobile_device_dedupe_key(device: &MobileAvailabilityDevice) -> String {
    if let Some(client_ip) = &device.client_ip {
        return format!("{}|{}", device.source_host, client_ip);
    }
    format!(
        "{}|{}|{}",
        device.source_host,
        device.label.to_ascii_lowercase(),
        device.user_agent.as_deref().unwrap_or_default()
    )
}

pub fn render_mobile_availability_panel(snapshot: &MobileAvailabilitySnapshot) -> String {
    render_mobile_availability_panel_for_terminal(snapshot, false)
}

fn render_mobile_availability_panel_for_terminal(
    snapshot: &MobileAvailabilitySnapshot,
    use_color: bool,
) -> String {
    let mut output = String::new();
    output.push_str("MOBILE AVAILABILITY CHECK\n");
    output.push_str("────────────────────────────────────────────────────────────────────────\n");

    if snapshot.entries.is_empty() {
        output.push_str("   No effective LAN/public IPv4 address detected.\n");
    } else {
        for entry in &snapshot.entries {
            let preferred = if entry.is_preferred {
                " (preferred)"
            } else {
                ""
            };
            output.push_str(&format!("   Target:        {}{}\n", entry.ip, preferred));
            if let Some(error) = &entry.error {
                output.push_str(&format!("   Status:        unavailable - {error}\n"));
                continue;
            }
            if let Some(url) = &entry.landing_url {
                output.push_str(&format!("   Open:          {url}\n"));
            }
            if let Some(qr) = &entry.terminal_qr {
                output.push_str("   QR:\n");
                for line in qr.lines() {
                    output.push_str("     ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }
    }

    output.push_str("\n   Recent devices:\n");
    if snapshot.devices.is_empty() {
        output.push_str("     none\n");
    } else {
        for device in &snapshot.devices {
            let client = device.client_ip.as_deref().unwrap_or("unknown ip");
            output.push_str(&format!(
                "     - {} from {} via {} (last seen {}s ago)\n",
                device.label, client, device.source_host, device.last_seen_seconds_ago
            ));
            output.push_str(&format!(
                "       network: {}; certificate: {}; proxy access: {}; proxy config: {}\n",
                render_status_value(&device.network_status, use_color),
                render_status_value(&device.certificate_status, use_color),
                render_status_value(&device.proxy_access_status, use_color),
                render_status_value(&device.proxy_config_status, use_color)
            ));
        }
    }

    output.push_str("\n   Access Control:\n");
    output.push_str(&format!("     mode: {}\n", snapshot.access_mode));
    if snapshot.pending_authorizations.is_empty() {
        output.push_str("     pending: none\n");
    } else {
        for (index, pending) in snapshot.pending_authorizations.iter().enumerate() {
            let marker = if index == 0 { "current" } else { "queued" };
            output.push_str(&format!(
                "     pending: {} ({marker}, attempts {}, first seen {}s ago)\n",
                pending.ip, pending.attempt_count, pending.first_seen_seconds_ago
            ));
        }
        output.push_str("     approve current device? Yes: y | No: n\n");
        output.push_str("     auto refresh: paused while waiting for y/n input\n");
    }
    output.push_str("     shortcut: y = allow current, n = deny current\n");
    output
}

fn render_status_value(value: &str, use_color: bool) -> String {
    if !use_color {
        return value.to_string();
    }
    let color = match value {
        "allowed" | "configured" | "reachable" | "trusted" => ANSI_GREEN,
        "denied" | "failed" => ANSI_RED,
        _ => ANSI_YELLOW,
    };
    format!("{color}{value}{ANSI_RESET}")
}

fn pending_authorization_signature(snapshot: &MobileAvailabilitySnapshot) -> Option<String> {
    if snapshot.pending_authorizations.is_empty() {
        return None;
    }
    Some(
        snapshot
            .pending_authorizations
            .iter()
            .map(|pending| pending.ip.as_str())
            .collect::<Vec<_>>()
            .join("|"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessControlTerminalCommand {
    Allow(IpAddr),
    Deny(IpAddr),
    AllowFirstPending,
    DenyFirstPending,
    Help,
}

pub fn parse_access_control_command(
    input: &str,
) -> Result<Option<AccessControlTerminalCommand>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or_default().to_ascii_lowercase();
    match command.as_str() {
        "help" | "h" | "?" => {
            parse_no_arg_command(parts.next(), AccessControlTerminalCommand::Help)
        }
        "yes" | "y" => parse_no_arg_command(
            parts.next(),
            AccessControlTerminalCommand::AllowFirstPending,
        ),
        "no" | "n" => {
            parse_no_arg_command(parts.next(), AccessControlTerminalCommand::DenyFirstPending)
        }
        "allow" | "approve" | "a" => parse_ip_command(parts.next(), parts.next())
            .map(AccessControlTerminalCommand::Allow)
            .map(Some),
        "deny" | "reject" | "d" => parse_ip_command(parts.next(), parts.next())
            .map(AccessControlTerminalCommand::Deny)
            .map(Some),
        _ => Err(
            "unknown command. Use y to allow or n to deny the current pending device".to_string(),
        ),
    }
}

fn parse_no_arg_command(
    extra: Option<&str>,
    command: AccessControlTerminalCommand,
) -> Result<Option<AccessControlTerminalCommand>, String> {
    if extra.is_some() {
        return Err("this command does not take an IP address or extra arguments".to_string());
    }
    Ok(Some(command))
}

fn parse_ip_command(first: Option<&str>, extra: Option<&str>) -> Result<IpAddr, String> {
    if extra.is_some() {
        return Err("expected exactly one IP address".to_string());
    }
    let Some(value) = first else {
        return Err("missing IP address".to_string());
    };
    value
        .parse::<IpAddr>()
        .map_err(|_| format!("invalid IP address: {value}"))
}

async fn execute_access_control_command(
    command: AccessControlTerminalCommand,
    access_control: &SharedAccessControl,
    push_manager: &SharedPushManager,
) -> String {
    match command {
        AccessControlTerminalCommand::Help => {
            "Access Control: press y to allow or n to deny the current pending device".to_string()
        }
        AccessControlTerminalCommand::AllowFirstPending => {
            let Some(ip) = first_pending_authorization_ip(access_control).await else {
                return "Access Control: no pending device to allow".to_string();
            };
            allow_access_control_ip(ip, access_control, push_manager).await
        }
        AccessControlTerminalCommand::DenyFirstPending => {
            let Some(ip) = first_pending_authorization_ip(access_control).await else {
                return "Access Control: no pending device to deny".to_string();
            };
            deny_access_control_ip(ip, access_control, push_manager).await
        }
        AccessControlTerminalCommand::Allow(ip) => {
            allow_access_control_ip(ip, access_control, push_manager).await
        }
        AccessControlTerminalCommand::Deny(ip) => {
            deny_access_control_ip(ip, access_control, push_manager).await
        }
    }
}

async fn first_pending_authorization_ip(access_control: &SharedAccessControl) -> Option<IpAddr> {
    let access_control = access_control.read().await;
    let mut pending = access_control.get_pending_authorizations();
    pending.sort_by_key(|pending| pending.first_seen);
    pending
        .first()
        .and_then(|pending| pending.ip.parse::<IpAddr>().ok())
}

async fn allow_access_control_ip(
    ip: IpAddr,
    access_control: &SharedAccessControl,
    push_manager: &SharedPushManager,
) -> String {
    let message = {
        let access_control = access_control.read().await;
        if access_control.is_in_temporary_whitelist(&ip) {
            format!("Access Control: {ip} is already allowed")
        } else if access_control.approve_pending(&ip) {
            format!("Access Control: allowed pending client {ip}")
        } else {
            access_control.add_temporary(ip);
            format!("Access Control: allowed {ip}")
        }
    };
    broadcast_access_control_updates(push_manager).await;
    message
}

async fn deny_access_control_ip(
    ip: IpAddr,
    access_control: &SharedAccessControl,
    push_manager: &SharedPushManager,
) -> String {
    let message = {
        let access_control = access_control.read().await;
        if access_control.reject_pending(&ip) {
            format!("Access Control: denied pending client {ip}")
        } else {
            access_control.deny_session(ip);
            format!("Access Control: denied {ip}")
        }
    };
    broadcast_access_control_updates(push_manager).await;
    message
}

async fn broadcast_access_control_updates(push_manager: &SharedPushManager) {
    push_manager
        .broadcast_settings_scope(SETTINGS_SCOPE_PENDING_AUTHORIZATIONS)
        .await;
    push_manager
        .broadcast_settings_scope(SETTINGS_SCOPE_WHITELIST_STATUS)
        .await;
    push_manager
        .broadcast_settings_scope(SETTINGS_SCOPE_TRUST_PROBE)
        .await;
}

fn render_terminal_qr(url: &str) -> Result<String, String> {
    QrCode::new(url.as_bytes())
        .map_err(|error| format!("Failed to generate QR code: {error}"))
        .map(|code| {
            code.render::<Dense1x2>()
                .quiet_zone(false)
                .module_dimensions(1, 1)
                .build()
        })
}

fn short_trust_probe_url(host: &str, port: u16) -> String {
    format!("http://{host}:{port}/_bifrost/tp")
}

fn certificate_status_text(status: TrustProbeStatus, tls_trusted: bool, opened: bool) -> String {
    if tls_trusted || matches!(status, TrustProbeStatus::TlsTrusted) {
        "trusted".to_string()
    } else if matches!(status, TrustProbeStatus::TlsFailed) {
        "not trusted".to_string()
    } else if opened {
        "checking".to_string()
    } else {
        "not opened".to_string()
    }
}

fn network_status_text(status: TrustProbeStatus, network_reachable: bool) -> String {
    if network_reachable || matches!(status, TrustProbeStatus::NetworkReachable) {
        "reachable".to_string()
    } else if matches!(status, TrustProbeStatus::NetworkFailed) {
        "failed".to_string()
    } else {
        "checking".to_string()
    }
}

fn proxy_access_status_text(
    status: Option<TrustProbeProxyAccessStatus>,
    client_ip: Option<&str>,
    pending_by_ip: &HashSet<&str>,
    temporary_whitelist: &HashSet<IpAddr>,
) -> String {
    if let Some(client_ip) = client_ip.and_then(|value| value.parse::<IpAddr>().ok()) {
        if temporary_whitelist.contains(&client_ip) {
            return "allowed".to_string();
        }
    }
    if let Some(client_ip) = client_ip {
        if pending_by_ip.contains(client_ip) {
            return "pending approval".to_string();
        }
    }
    match status {
        Some(TrustProbeProxyAccessStatus::Allowed) => "allowed".to_string(),
        Some(TrustProbeProxyAccessStatus::Pending) => "pending approval".to_string(),
        Some(TrustProbeProxyAccessStatus::Denied) => "denied".to_string(),
        Some(TrustProbeProxyAccessStatus::Unavailable) => "unavailable".to_string(),
        None => "not requested".to_string(),
    }
}

fn proxy_config_status_text(proxy_configured: bool) -> String {
    if proxy_configured {
        "configured".to_string()
    } else {
        "not confirmed".to_string()
    }
}

fn unix_timestamp_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_access_control_commands() {
        let ip = "192.168.1.9".parse::<IpAddr>().unwrap();
        assert_eq!(
            parse_access_control_command("allow 192.168.1.9").unwrap(),
            Some(AccessControlTerminalCommand::Allow(ip))
        );
        assert_eq!(
            parse_access_control_command("d 192.168.1.9").unwrap(),
            Some(AccessControlTerminalCommand::Deny(ip))
        );
        assert_eq!(
            parse_access_control_command("y").unwrap(),
            Some(AccessControlTerminalCommand::AllowFirstPending)
        );
        assert_eq!(
            parse_access_control_command("no").unwrap(),
            Some(AccessControlTerminalCommand::DenyFirstPending)
        );
        assert_eq!(parse_access_control_command("").unwrap(), None);
        assert!(parse_access_control_command("allow").is_err());
        assert!(parse_access_control_command("allow 1.1.1.1 extra").is_err());
        assert!(parse_access_control_command("y 192.168.1.9").is_err());
    }

    #[test]
    fn render_panel_shows_empty_effective_ip_state() {
        let snapshot = MobileAvailabilitySnapshot {
            entries: vec![],
            devices: vec![],
            pending_authorizations: vec![],
            access_mode: "interactive".to_string(),
        };
        let panel = render_mobile_availability_panel(&snapshot);
        assert!(panel.contains("No effective LAN/public IPv4 address detected."));
        assert!(panel.contains("pending: none"));
    }

    #[test]
    fn render_panel_shows_yes_no_access_control_prompt_for_pending_device() {
        let snapshot = MobileAvailabilitySnapshot {
            entries: vec![],
            devices: vec![],
            pending_authorizations: vec![MobileAvailabilityPendingAuth {
                ip: "192.168.1.9".to_string(),
                attempt_count: 3,
                first_seen_seconds_ago: 12,
            }],
            access_mode: "interactive".to_string(),
        };
        let panel = render_mobile_availability_panel(&snapshot);
        assert!(panel.contains("pending: 192.168.1.9 (current, attempts 3"));
        assert!(panel.contains("approve current device? Yes: y | No: n"));
        assert!(panel.contains("auto refresh: paused while waiting for y/n input"));
        assert!(panel.contains("shortcut: y = allow current, n = deny current"));
    }

    #[test]
    fn render_panel_colors_status_values_only_for_terminal_output() {
        let snapshot = MobileAvailabilitySnapshot {
            entries: vec![],
            devices: vec![MobileAvailabilityDevice {
                label: "ios".to_string(),
                client_ip: Some("10.4.224.68".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: None,
                certificate_status: "trusted".to_string(),
                proxy_access_status: "allowed".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 0,
            }],
            pending_authorizations: vec![],
            access_mode: "interactive".to_string(),
        };

        let plain = render_mobile_availability_panel(&snapshot);
        assert!(plain.contains("network: reachable; certificate: trusted; proxy access: allowed; proxy config: not confirmed"));
        assert!(!plain.contains(ANSI_GREEN));

        let colored = render_mobile_availability_panel_for_terminal(&snapshot, true);
        assert!(colored.contains(&format!("{ANSI_GREEN}reachable{ANSI_RESET}")));
        assert!(colored.contains(&format!("{ANSI_GREEN}trusted{ANSI_RESET}")));
        assert!(colored.contains(&format!("{ANSI_GREEN}allowed{ANSI_RESET}")));
        assert!(colored.contains(&format!("{ANSI_YELLOW}not confirmed{ANSI_RESET}")));
    }

    #[test]
    fn recent_device_status_texts_are_device_scoped() {
        let pending_by_ip = HashSet::new();
        let temporary_whitelist = HashSet::new();

        assert_eq!(
            certificate_status_text(TrustProbeStatus::PageOpened, false, true),
            "checking"
        );
        assert_eq!(
            network_status_text(TrustProbeStatus::PageOpened, false),
            "checking"
        );
        assert_eq!(
            proxy_access_status_text(
                None,
                Some("10.4.224.68"),
                &pending_by_ip,
                &temporary_whitelist
            ),
            "not requested"
        );
        assert_eq!(proxy_config_status_text(false), "not confirmed");
    }

    #[test]
    fn terminal_qr_uses_compact_short_url_without_quiet_zone() {
        let long_url = "http://10.71.185.109:18890/_bifrost/public/trust-probe";
        let short_url = short_trust_probe_url("10.71.185.109", 18890);
        assert_eq!(short_url, "http://10.71.185.109:18890/_bifrost/tp");

        let long_qr = render_terminal_qr(long_url).unwrap();
        let short_qr = render_terminal_qr(&short_url).unwrap();
        let long_height = long_qr.lines().count();
        let short_height = short_qr.lines().count();
        let long_width = long_qr.lines().map(str::len).max().unwrap_or_default();
        let short_width = short_qr.lines().map(str::len).max().unwrap_or_default();

        assert!(short_height < long_height);
        assert!(short_width < long_width);
        assert!(!short_qr
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .is_empty());
    }

    #[test]
    fn terminal_panel_skips_non_tty_unless_forced() {
        let _guard = EnvVarGuard::remove("BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE");
        assert!(should_render_terminal_panel(true));
        assert!(!should_render_terminal_panel(false));

        std::env::set_var("BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE", "1");
        assert!(should_render_terminal_panel(false));
    }

    #[test]
    fn pending_panel_suppresses_automatic_rerender_with_same_pending_ip() {
        assert!(!should_render_panel_update(
            true,
            true,
            false,
            Some("192.168.1.9"),
            Some("192.168.1.9")
        ));
        assert!(should_render_panel_update(
            true,
            true,
            false,
            Some("192.168.1.10"),
            Some("192.168.1.9")
        ));
        assert!(should_render_panel_update(
            true,
            true,
            true,
            Some("192.168.1.9"),
            Some("192.168.1.9")
        ));
        assert!(should_render_panel_update(
            false,
            true,
            false,
            Some("192.168.1.9"),
            Some("192.168.1.9")
        ));
    }

    #[test]
    fn recent_device_label_infers_platform_from_user_agent_when_hint_is_unknown() {
        let label = recent_device_label(
            Some("unknown"),
            Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 Edg/149.0.0.0"),
            "dev-local",
        );
        assert_eq!(label, "macos edge");

        let android = recent_device_label(
            None,
            Some("Mozilla/5.0 (Linux; Android 15; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Mobile Safari/537.36"),
            "dev-android",
        );
        assert_eq!(android, "android chrome");
    }

    #[test]
    fn dedupe_recent_devices_collapses_refreshes_from_same_client_ip_and_host() {
        let devices = dedupe_recent_devices(vec![
            MobileAvailabilityDevice {
                label: "ios".to_string(),
                client_ip: Some("10.4.224.68".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: Some("Mobile Safari".to_string()),
                certificate_status: "not trusted".to_string(),
                proxy_access_status: "pending approval".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 48,
            },
            MobileAvailabilityDevice {
                label: "ios".to_string(),
                client_ip: Some("10.4.224.68".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: Some("Mobile Safari".to_string()),
                certificate_status: "not trusted".to_string(),
                proxy_access_status: "pending approval".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 0,
            },
        ]);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].label, "ios");
        assert_eq!(devices[0].client_ip.as_deref(), Some("10.4.224.68"));
        assert_eq!(devices[0].last_seen_seconds_ago, 0);
    }

    #[test]
    fn dedupe_recent_devices_keeps_different_client_ips() {
        let devices = dedupe_recent_devices(vec![
            MobileAvailabilityDevice {
                label: "ios".to_string(),
                client_ip: Some("10.4.224.68".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: None,
                certificate_status: "checking".to_string(),
                proxy_access_status: "pending approval".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 0,
            },
            MobileAvailabilityDevice {
                label: "ios".to_string(),
                client_ip: Some("10.4.224.69".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: None,
                certificate_status: "checking".to_string(),
                proxy_access_status: "pending approval".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 1,
            },
        ]);

        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn recent_devices_are_sorted_by_client_ip_not_last_seen() {
        let devices = dedupe_recent_devices(vec![
            MobileAvailabilityDevice {
                label: "ios-newer".to_string(),
                client_ip: Some("10.4.224.68".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: None,
                certificate_status: "trusted".to_string(),
                proxy_access_status: "allowed".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 0,
            },
            MobileAvailabilityDevice {
                label: "android-older".to_string(),
                client_ip: Some("10.71.219.32".to_string()),
                source_host: "10.71.185.109".to_string(),
                user_agent: None,
                certificate_status: "trusted".to_string(),
                proxy_access_status: "allowed".to_string(),
                proxy_config_status: "not confirmed".to_string(),
                network_status: "reachable".to_string(),
                last_seen_seconds_ago: 30,
            },
        ]);

        assert_eq!(
            devices
                .iter()
                .map(|device| device.client_ip.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["10.4.224.68", "10.71.219.32"]
        );
    }
}
