use bifrost_storage::{
    set_data_dir, ConfigManager, SystemProxyConfigUpdate, SystemProxyRecoveryMode,
    MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS, MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
};

#[cfg(target_os = "macos")]
use crate::cli::SystemProxyLaunchdCommands;
use crate::cli::{Cli, SystemProxyCommands};
use crate::config::get_bifrost_dir;
use crate::process::{
    inspect_process_identity, is_process_running, read_runtime_info, runtime_system_proxy_host,
    ProcessIdentityStatus, RuntimeInfo,
};
#[cfg(unix)]
use bifrost_power::PowerEvent;
#[cfg(target_os = "macos")]
use bifrost_power::PowerNotificationWatcher;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleRecoveryTrigger {
    PidMissing,
    PidReused,
    PollConfirmedExit,
    Signal(&'static str),
}

impl LifecycleRecoveryTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::PidMissing => "pid_missing",
            Self::PidReused => "pid_reused",
            Self::PollConfirmedExit => "poll_confirmed_exit",
            Self::Signal(signal) => signal,
        }
    }
}

pub fn handle_system_proxy_command(
    cli: &Cli,
    action: SystemProxyCommands,
) -> bifrost_core::Result<()> {
    match &action {
        SystemProxyCommands::Cleanup { data_dir } => {
            let cleanup_dir = match data_dir.clone() {
                Some(data_dir) => data_dir,
                None => get_bifrost_dir()?,
            };
            set_data_dir(cleanup_dir.clone());
            return cleanup_system_proxy_state(&cleanup_dir);
        }
        SystemProxyCommands::LifecycleHelper {
            data_dir,
            parent_pid,
            parent_started_at_ms,
            poll_secs,
        } => {
            return run_system_proxy_lifecycle_helper(
                data_dir.clone(),
                *parent_pid,
                *parent_started_at_ms,
                *poll_secs,
            );
        }
        #[cfg(target_os = "macos")]
        SystemProxyCommands::RepairLock { data_dir } => {
            return bifrost_core::repair_system_proxy_lock_permissions(data_dir);
        }
        #[cfg(target_os = "macos")]
        SystemProxyCommands::CleanupDaemon {
            data_dir,
            installed_version,
        } => {
            return run_system_proxy_cleanup_daemon(data_dir.clone(), installed_version.clone());
        }
        _ => {}
    }

    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());

    let config_manager = ConfigManager::new(bifrost_dir.clone())?;
    let stored_config = futures::executor::block_on(config_manager.config());

    let mut manager = bifrost_core::SystemProxyManager::new(bifrost_dir.clone());
    match action {
        SystemProxyCommands::Status => {
            if !bifrost_core::SystemProxyManager::is_supported() {
                println!("System proxy not supported on this platform");
                return Ok(());
            }
            match bifrost_core::SystemProxyManager::get_current() {
                Ok(status) => {
                    let runtime_target = read_valid_runtime_system_proxy_target();
                    let managed_by_bifrost = manager.is_current_managed(&status)
                        || runtime_target.as_ref().is_some_and(|target| {
                            status.target_matches(&target.host, target.port)
                                || bifrost_core::SystemProxyManager::any_service_proxy_matches(
                                    &target.host,
                                    target.port,
                                )
                                .unwrap_or(false)
                        });
                    print!(
                        "{}",
                        render_system_proxy_status(
                            &status,
                            managed_by_bifrost,
                            &stored_config.system_proxy,
                        )
                    );
                }
                Err(e) => {
                    eprintln!("Failed to get system proxy: {}", e);
                }
            }
        }
        SystemProxyCommands::Doctor { format } => {
            let report = build_system_proxy_doctor_report(&bifrost_dir, &manager);
            println!("{}", render_system_proxy_doctor_report(&report, format)?);
        }
        SystemProxyCommands::RecoveryPolicy { mode, grace_secs } => {
            let recovery_mode =
                persist_system_proxy_recovery_policy(&config_manager, mode.as_str(), grace_secs)?;
            println!(
                "✓ Recovery policy configured: {} ({}s grace)",
                recovery_mode_name(recovery_mode),
                grace_secs
            );
        }
        SystemProxyCommands::Enable { bypass, host, port } => {
            if !bifrost_core::SystemProxyManager::is_supported() {
                println!("System proxy not supported on this platform");
                return Ok(());
            }
            let proxy_host = host.unwrap_or_else(|| "127.0.0.1".to_string());
            let proxy_port = port.unwrap_or(cli.port);
            let live_port = running_runtime_admin_port_for_target(&proxy_host, proxy_port);
            let result = route_explicit_proxy_command(
                live_port,
                |port| request_admin_proxy_change(port, true, bypass.as_deref()),
                || {
                    direct_enable(
                        &mut manager,
                        &config_manager,
                        &proxy_host,
                        proxy_port,
                        bypass.clone(),
                    )
                },
            );
            manager.detach_in_place();
            result?;
        }
        SystemProxyCommands::Disable => {
            if !bifrost_core::SystemProxyManager::is_supported() {
                println!("System proxy not supported on this platform");
                return Ok(());
            }
            let result = route_explicit_proxy_command(
                running_runtime_admin_port(),
                |port| request_admin_proxy_change(port, false, None),
                || direct_disable(&mut manager, &config_manager),
            );
            manager.detach_in_place();
            result?;
        }
        #[cfg(target_os = "macos")]
        SystemProxyCommands::Launchd { action } => {
            handle_system_proxy_launchd_command(&action, Some(bifrost_dir.clone()))?;
        }
        SystemProxyCommands::Cleanup { .. } | SystemProxyCommands::LifecycleHelper { .. } => {
            unreachable!("hidden system-proxy helper commands are handled before config load")
        }
        #[cfg(target_os = "macos")]
        SystemProxyCommands::RepairLock { .. } => {
            unreachable!("hidden system-proxy helper commands are handled before config load")
        }
        #[cfg(target_os = "macos")]
        SystemProxyCommands::CleanupDaemon { .. } => {
            unreachable!("hidden system-proxy helper commands are handled before config load")
        }
    }
    manager.detach();
    Ok(())
}

fn render_system_proxy_status(
    status: &bifrost_core::ProxyBackup,
    managed_by_bifrost: bool,
    configured: &bifrost_storage::NewSystemProxyConfig,
) -> String {
    let mut lines = vec![
        "Supported: true".to_string(),
        format!("Enabled:             {}", status.enable),
        format!("Host:                {}", status.host),
        format!("Port:                {}", status.port),
        format!("Bypass:              {}", status.bypass),
        format!("Managed by Bifrost:  {managed_by_bifrost}"),
        format!("Configured enabled:  {}", configured.enabled),
        format!("Configured bypass:   {}", configured.bypass),
        format!(
            "Recovery policy:     {} ({}s)",
            recovery_mode_name(configured.recovery_mode),
            configured.recovery_grace_secs
        ),
    ];
    if status.enable && !managed_by_bifrost {
        lines.push(
            "System proxy is enabled by another application; Bifrost will leave it unchanged."
                .to_string(),
        );
    }
    format!("{}\n", lines.join("\n"))
}

#[derive(Debug, serde::Serialize)]
struct SystemProxyDoctorReport {
    runtime: Option<RuntimeInfo>,
    runtime_identity: String,
    health: Option<bifrost_core::RuntimeHealthSnapshot>,
    health_error: Option<String>,
    current_proxy: Option<bifrost_core::ProxyBackup>,
    current_proxy_error: Option<String>,
    managed_ownership: Option<bifrost_core::ManagedSystemProxyOwnership>,
    managed_ownership_error: Option<String>,
    owner_state: Option<bifrost_core::SystemProxyOwnerState>,
    recent_events: Vec<bifrost_core::SystemProxyLifecycleEvent>,
    findings: Vec<String>,
}

fn build_system_proxy_doctor_report(
    data_dir: &std::path::Path,
    manager: &bifrost_core::SystemProxyManager,
) -> SystemProxyDoctorReport {
    build_system_proxy_doctor_report_with_observations(
        data_dir,
        bifrost_core::SystemProxyManager::get_current,
        || manager.read_managed_ownership(),
        bifrost_core::SystemProxyManager::any_service_proxy_matches,
    )
}

fn build_system_proxy_doctor_report_with_observations(
    data_dir: &std::path::Path,
    read_current: impl FnOnce() -> bifrost_core::Result<bifrost_core::ProxyBackup>,
    read_ownership: impl FnOnce() -> bifrost_core::Result<
        Option<bifrost_core::ManagedSystemProxyOwnership>,
    >,
    any_service_matches: impl FnOnce(&str, u16) -> bifrost_core::Result<bool>,
) -> SystemProxyDoctorReport {
    let runtime = read_runtime_info_from(data_dir);
    let runtime_identity = runtime
        .as_ref()
        .map(|runtime| {
            format!(
                "{:?}",
                inspect_process_identity(runtime.pid, runtime.started_at_ms)
            )
        })
        .unwrap_or_else(|| "Missing".into());
    let (health, health_error) = match runtime.as_ref().and_then(|runtime| runtime.health_port) {
        Some(port) => {
            let url = format!("http://127.0.0.1:{port}/health");
            match bifrost_core::direct_ureq_agent_builder()
                .timeout(std::time::Duration::from_millis(750))
                .build()
                .get(&url)
                .call()
            {
                Ok(response) => match response.into_json::<bifrost_core::RuntimeHealthSnapshot>() {
                    Ok(snapshot) => (Some(snapshot), None),
                    Err(error) => (None, Some(format!("invalid health response: {error}"))),
                },
                Err(error) => (None, Some(format!("health lane unavailable: {error}"))),
            }
        }
        None => (None, Some("runtime marker has no health_port".into())),
    };
    let (current_proxy, current_proxy_error) = match read_current() {
        Ok(proxy) => (Some(proxy), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (managed_ownership, managed_ownership_error) = match read_ownership() {
        Ok(ownership) => (ownership, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let owner_state = bifrost_core::read_system_proxy_owner_state(data_dir)
        .ok()
        .flatten();
    let recent_events =
        bifrost_core::read_recent_system_proxy_events(data_dir, 30).unwrap_or_default();
    let mut findings = Vec::new();
    if runtime.is_none() {
        findings.push("runtime marker is missing".into());
    } else if runtime_identity != "Alive" {
        findings.push(format!("runtime process identity is {runtime_identity}"));
    }
    if let Some(error) = health_error.as_ref() {
        findings.push(error.clone());
    }
    if health
        .as_ref()
        .is_some_and(|snapshot| snapshot.scheduler_heartbeat_age_ms >= 5_000)
    {
        findings.push("scheduler heartbeat is stale".into());
    }
    if managed_ownership
        .as_ref()
        .is_some_and(|ownership| ownership.authorization_suppressed)
    {
        findings.push("system proxy authorization was cancelled; an explicit enable is required before retrying".into());
    }
    if let (Some(ownership), Some(current)) = (managed_ownership.as_ref(), current_proxy.as_ref()) {
        let current_matches_target = current
            .target_matches(&ownership.target.host, ownership.target.port)
            || any_service_matches(&ownership.target.host, ownership.target.port).unwrap_or(false);
        let current_matches_original = current == &ownership.original;
        if ownership.applied && !current_matches_target {
            findings.push("managed state says applied but OS proxy ownership changed".into());
        }
        if ownership.is_suspended() && !current_matches_original {
            findings.push("fail-open state no longer matches the recorded original proxy".into());
        }
    }
    if findings.is_empty() {
        findings.push("no blocking ownership or runtime health issue detected".into());
    }

    SystemProxyDoctorReport {
        runtime,
        runtime_identity,
        health,
        health_error,
        current_proxy,
        current_proxy_error,
        managed_ownership,
        managed_ownership_error,
        owner_state,
        recent_events,
        findings,
    }
}

fn render_system_proxy_doctor_report(
    report: &SystemProxyDoctorReport,
    format: crate::cli::StatusFormat,
) -> bifrost_core::Result<String> {
    let serialization_error = |error: serde_json::Error| {
        bifrost_core::BifrostError::Config(format!("Failed to serialize doctor report: {error}"))
    };
    match format {
        crate::cli::StatusFormat::Json => {
            return serde_json::to_string(report).map_err(serialization_error)
        }
        crate::cli::StatusFormat::JsonPretty => {
            return serde_json::to_string_pretty(report).map_err(serialization_error)
        }
        crate::cli::StatusFormat::Text => {}
    }

    let mut lines = vec![
        format!("Runtime identity:    {}", report.runtime_identity),
        format!(
            "Runtime PID/port:    {}",
            report
                .runtime
                .as_ref()
                .map(|runtime| format!("{}/{}", runtime.pid, runtime.port))
                .unwrap_or_else(|| "-".into())
        ),
        format!(
            "Health lane:         {}",
            report
                .health
                .as_ref()
                .map(|health| format!(
                    "ok (heartbeat={}ms pressure={:?} rss={} fd={}/{})",
                    health.scheduler_heartbeat_age_ms,
                    health.pressure,
                    health.rss_bytes,
                    health.fd_count,
                    health.fd_limit
                ))
                .or_else(|| report.health_error.clone())
                .unwrap_or_else(|| "-".into())
        ),
        format!(
            "Ownership generation: {}",
            report
                .managed_ownership
                .as_ref()
                .map(|ownership| ownership.generation.as_str())
                .unwrap_or("-")
        ),
        format!("Recent events:       {}", report.recent_events.len()),
        "Findings:".into(),
    ];
    for finding in &report.findings {
        lines.push(format!("  - {finding}"));
    }
    Ok(lines.join("\n"))
}

fn persist_system_proxy_recovery_policy(
    config_manager: &ConfigManager,
    mode: &str,
    grace_secs: u64,
) -> bifrost_core::Result<SystemProxyRecoveryMode> {
    let recovery_mode = match mode {
        "fail-closed" => SystemProxyRecoveryMode::FailClosed,
        _ => SystemProxyRecoveryMode::FailOpen,
    };
    futures::executor::block_on(config_manager.update_system_proxy_config(
        SystemProxyConfigUpdate {
            enabled: None,
            bypass: None,
            auto_enable: None,
            recovery_mode: Some(recovery_mode),
            recovery_grace_secs: Some(grace_secs),
        },
    ))?;
    Ok(recovery_mode)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeSystemProxyTarget {
    host: String,
    port: u16,
}

fn runtime_info_system_proxy_target(runtime: &RuntimeInfo) -> RuntimeSystemProxyTarget {
    RuntimeSystemProxyTarget {
        host: runtime_system_proxy_host(runtime.host.as_deref()).to_string(),
        port: runtime.port,
    }
}

fn runtime_identity_is_current(runtime: &RuntimeInfo) -> bool {
    // The boolean Unix probe accepts process-group sentinels such as PID 0.
    // Only a concrete, current process instance may accumulate ready canaries.
    if inspect_process_identity(runtime.pid, runtime.started_at_ms) != ProcessIdentityStatus::Alive
    {
        return false;
    }
    if !is_process_running(runtime.pid) {
        return false;
    }

    let observed_started_at_ms = bifrost_core::get_process_start_time_ms(runtime.pid);
    match bifrost_core::start_times_match(runtime.started_at_ms, observed_started_at_ms) {
        bifrost_core::StartTimeMatch::Mismatch { recorded, observed } => {
            tracing::debug!(
                pid = runtime.pid,
                recorded_started_at_ms = recorded,
                observed_started_at_ms = observed,
                "ignoring stale runtime system proxy target because process start time mismatched"
            );
            false
        }
        bifrost_core::StartTimeMatch::Match | bifrost_core::StartTimeMatch::Unknown => true,
    }
}

fn read_valid_runtime_system_proxy_target() -> Option<RuntimeSystemProxyTarget> {
    running_runtime_info().map(|runtime| runtime_info_system_proxy_target(&runtime))
}

fn running_runtime_info() -> Option<RuntimeInfo> {
    let runtime = read_runtime_info()?;
    if runtime_identity_is_current(&runtime) {
        Some(runtime)
    } else {
        None
    }
}

fn running_runtime_admin_port() -> Option<u16> {
    running_runtime_info().map(|runtime| runtime.port)
}

fn running_runtime_admin_port_for_target(host: &str, port: u16) -> Option<u16> {
    let runtime = running_runtime_info()?;
    let target = runtime_info_system_proxy_target(&runtime);
    let matches = bifrost_core::ProxyBackup {
        enable: true,
        host: target.host,
        port: target.port,
        bypass: String::new(),
    }
    .target_matches(host, port);
    matches.then_some(runtime.port)
}

#[cfg(test)]
fn persist_system_proxy_config(
    config_manager: &ConfigManager,
    enabled: bool,
    bypass: Option<String>,
) -> bifrost_core::Result<()> {
    futures::executor::block_on(config_manager.update_system_proxy_config(
        SystemProxyConfigUpdate {
            enabled: Some(enabled),
            bypass,
            auto_enable: None,
            recovery_mode: None,
            recovery_grace_secs: None,
        },
    ))
    .map_err(|error| {
        bifrost_core::BifrostError::Config(format!(
            "Failed to persist system proxy config: {error}"
        ))
    })
}

fn with_accepted_system_proxy_intent<T>(
    config_manager: &ConfigManager,
    enabled: bool,
    bypass: Option<String>,
    attempt: impl FnOnce(&bifrost_storage::NewSystemProxyConfig) -> bifrost_core::Result<T>,
) -> bifrost_core::Result<T> {
    let accepted = futures::executor::block_on(
        config_manager.update_system_proxy_config_with_snapshot(SystemProxyConfigUpdate {
            enabled: Some(enabled),
            bypass,
            auto_enable: None,
            recovery_mode: None,
            recovery_grace_secs: None,
        }),
    )?;
    attempt(&accepted)
}

#[cfg(test)]
fn should_retry_disable_with_runtime_target(
    outcome: bifrost_core::SystemProxyDisableOutcome,
    runtime_target: Option<&RuntimeSystemProxyTarget>,
) -> bool {
    matches!(
        outcome,
        bifrost_core::SystemProxyDisableOutcome::OwnedByOther
    ) && runtime_target.is_some()
}

#[cfg(target_os = "macos")]
pub(crate) fn handle_system_proxy_launchd_command(
    action: &SystemProxyLaunchdCommands,
    default_data_dir: Option<std::path::PathBuf>,
) -> bifrost_core::Result<()> {
    match action {
        SystemProxyLaunchdCommands::Status { label, plist_path } => {
            let label = label
                .as_deref()
                .unwrap_or(bifrost_core::system_proxy_launchd::DEFAULT_LABEL);
            let status = bifrost_core::launchd_status(label, plist_path.clone())?;
            print_launchd_status(&status);
        }
        SystemProxyLaunchdCommands::Install {
            data_dir,
            program,
            label,
            plist_path,
            dry_run,
        } => {
            let data_dir = data_dir
                .clone()
                .or(default_data_dir)
                .unwrap_or(get_bifrost_dir()?);
            let config = bifrost_core::SystemProxyLaunchdConfig::new(
                label.clone(),
                program.clone(),
                data_dir,
                plist_path.clone(),
            )?;
            if *dry_run {
                print!("{}", bifrost_core::render_launchd_plist(&config));
                return Ok(());
            }
            let status = match bifrost_core::install_launchd_cleanup(&config) {
                Ok(status) => status,
                Err(error) if error.to_string().contains("RequiresAdmin") => {
                    bifrost_core::install_launchd_cleanup_with_gui_auth(&config)?
                }
                Err(error) => return Err(error),
            };
            println!("✓ macOS system proxy cleanup LaunchDaemon installed");
            print_launchd_status(&status);
        }
        SystemProxyLaunchdCommands::Uninstall { label, plist_path } => {
            let status =
                match bifrost_core::uninstall_launchd_cleanup(label.as_deref(), plist_path.clone())
                {
                    Ok(status) => status,
                    Err(error) if error.to_string().contains("RequiresAdmin") => {
                        bifrost_core::uninstall_launchd_cleanup_with_gui_auth(
                            label.as_deref(),
                            plist_path.clone(),
                            None,
                        )?
                    }
                    Err(error) => return Err(error),
                };
            println!("✓ macOS system proxy cleanup LaunchDaemon uninstalled");
            print_launchd_status(&status);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn print_launchd_status(status: &bifrost_core::SystemProxyLaunchdStatus) {
    println!("Supported:         {}", status.supported);
    println!("Installed:         {}", status.installed);
    println!("Loaded:            {}", status.loaded);
    println!("Label:             {}", status.label);
    println!("Plist:             {}", status.plist_path.display());
    println!(
        "Program:           {}",
        status
            .program
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "Data dir:          {}",
        status
            .data_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "Installed version: {}",
        status.installed_version.as_deref().unwrap_or("-")
    );
    println!(
        "Installed mode:    {}",
        match status.installed_mode {
            Some(bifrost_core::SystemProxyLaunchdMode::OneShot) => "one-shot",
            Some(bifrost_core::SystemProxyLaunchdMode::KeepAlive) => "keep-alive",
            Some(bifrost_core::SystemProxyLaunchdMode::Unknown) => "unknown",
            None => "-",
        }
    );
    println!("Current version:   {}", status.current_version);
    println!("Needs upgrade:     {}", status.needs_upgrade);
    if let Some(reason) = &status.needs_upgrade_reason {
        println!("Upgrade reason:    {reason}");
    }
    if let Some(message) = &status.message {
        println!("Message:           {message}");
    }
}

mod explicit_command;
use explicit_command::*;
mod cleanup_fence;
use cleanup_fence::*;
mod readiness;
use readiness::*;
mod lifecycle;
mod recovery;
use lifecycle::*;
use recovery::*;

#[cfg(test)]
mod tests;
