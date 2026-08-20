use bifrost_storage::{
    set_data_dir, ConfigManager, SystemProxyConfigUpdate, SystemProxyRecoveryMode,
    MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS, MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
};

#[cfg(target_os = "macos")]
use crate::cli::SystemProxyLaunchdCommands;
use crate::cli::{Cli, SystemProxyCommands};
use crate::config::get_bifrost_dir;
use crate::process::{
    capture_runtime_system_proxy_snapshot, inspect_process_identity, is_process_running,
    read_runtime_info, runtime_system_proxy_host, ProcessIdentityStatus, RuntimeInfo,
    RuntimeSystemProxySnapshot,
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
            let bypass_str = bypass.unwrap_or_else(|| stored_config.system_proxy.bypass.clone());
            if runtime_target_matches_request(&proxy_host, proxy_port)
                && try_admin_api_set_system_proxy(true, Some(&bypass_str))
            {
                println!(
                    "✓ System proxy enabled via running Bifrost: {}:{} (bypass: {})",
                    proxy_host, proxy_port, bypass_str
                );
                manager.detach();
                return Ok(());
            }
            if let Err(e) = manager.enable(&proxy_host, proxy_port, Some(&bypass_str)) {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    println!("System proxy requires administrator privileges.");
                    let proceed = dialoguer::Confirm::new()
                        .with_prompt("Try enabling via sudo now?")
                        .default(true)
                        .interact();
                    match proceed {
                        Ok(true) => {
                            #[cfg(target_os = "macos")]
                            {
                                if let Err(se) = manager.enable_with_privilege(
                                    &proxy_host,
                                    proxy_port,
                                    Some(&bypass_str),
                                ) {
                                    eprintln!("Failed to enable with sudo: {}", se);
                                } else {
                                    persist_system_proxy_config(
                                        &config_manager,
                                        true,
                                        Some(bypass_str.clone()),
                                    )?;
                                    println!("✓ System proxy enabled via sudo");
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                eprintln!("Privilege escalation is only applicable on macOS.");
                            }
                        }
                        _ => {
                            println!("Cancelled.");
                        }
                    }
                } else {
                    eprintln!("Failed to enable system proxy: {}", e);
                }
            } else {
                persist_system_proxy_config(&config_manager, true, Some(bypass_str.clone()))?;
                println!(
                    "✓ System proxy enabled: {}:{} (bypass: {})",
                    proxy_host, proxy_port, bypass_str
                );
            }
        }
        SystemProxyCommands::Disable => {
            if !bifrost_core::SystemProxyManager::is_supported() {
                println!("System proxy not supported on this platform");
                return Ok(());
            }
            if try_admin_api_set_system_proxy(false, None) {
                println!("✓ System proxy disabled via running Bifrost");
                manager.detach();
                return Ok(());
            }
            match disable_system_proxy_explicit(&mut manager) {
                Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
                    persist_system_proxy_config(&config_manager, false, None)?;
                    println!("✓ System proxy disabled");
                }
                Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => {
                    persist_system_proxy_config(&config_manager, false, None)?;
                    println!("✓ System proxy already disabled");
                }
                Ok(bifrost_core::SystemProxyDisableOutcome::OwnedByOther) => {
                    persist_system_proxy_config(&config_manager, false, None)?;
                    println!("System proxy is enabled by another application; left unchanged.");
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("RequiresAdmin") {
                        println!("System proxy disable requires administrator privileges.");
                        let proceed = dialoguer::Confirm::new()
                            .with_prompt("Try disabling via sudo now?")
                            .default(true)
                            .interact();
                        match proceed {
                            Ok(true) => {
                                #[cfg(target_os = "macos")]
                                {
                                    match disable_system_proxy_explicit_with_privilege(&mut manager)
                                    {
                                        Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
                                            persist_system_proxy_config(
                                                &config_manager,
                                                false,
                                                None,
                                            )?;
                                            println!("✓ System proxy disabled via sudo");
                                        }
                                        Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => {
                                            persist_system_proxy_config(
                                                &config_manager,
                                                false,
                                                None,
                                            )?;
                                            println!("✓ System proxy already disabled");
                                        }
                                        Ok(
                                            bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
                                        ) => {
                                            persist_system_proxy_config(
                                                &config_manager,
                                                false,
                                                None,
                                            )?;
                                            println!("System proxy is enabled by another application; left unchanged.");
                                        }
                                        Err(se) => eprintln!("Failed to disable with sudo: {}", se),
                                    }
                                }
                                #[cfg(not(target_os = "macos"))]
                                {
                                    eprintln!("Privilege escalation is only applicable on macOS.");
                                }
                            }
                            _ => {
                                println!("Cancelled.");
                            }
                        }
                    } else {
                        eprintln!("Failed to disable system proxy: {}", e);
                    }
                }
            }
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
    let (current_proxy, current_proxy_error) = match bifrost_core::SystemProxyManager::get_current()
    {
        Ok(proxy) => (Some(proxy), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (managed_ownership, managed_ownership_error) = match manager.read_managed_ownership() {
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
    if let (Some(ownership), Some(current)) = (managed_ownership.as_ref(), current_proxy.as_ref()) {
        let current_matches_target = current
            .target_matches(&ownership.target.host, ownership.target.port)
            || bifrost_core::SystemProxyManager::any_service_proxy_matches(
                &ownership.target.host,
                ownership.target.port,
            )
            .unwrap_or(false);
        let current_matches_original = current == &ownership.original;
        if ownership.applied && !current_matches_target {
            findings.push("managed state says applied but OS proxy ownership changed".into());
        }
        if !ownership.applied && !current_matches_original {
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

fn runtime_target_matches_request(host: &str, port: u16) -> bool {
    running_runtime_info()
        .map(|runtime| runtime_info_system_proxy_target(&runtime))
        .is_some_and(|target| {
            bifrost_core::ProxyBackup {
                enable: true,
                host: target.host,
                port: target.port,
                bypass: String::new(),
            }
            .target_matches(host, port)
        })
}

fn try_admin_api_set_system_proxy(enabled: bool, bypass: Option<&str>) -> bool {
    let Some(port) = running_runtime_admin_port() else {
        return false;
    };
    let url = format!("http://127.0.0.1:{port}/_bifrost/api/proxy/system");
    let mut body = serde_json::json!({ "enabled": enabled });
    if let Some(bypass) = bypass {
        body["bypass"] = serde_json::Value::String(bypass.to_string());
    }

    bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .put(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map(|response| response.status() < 400)
        .unwrap_or(false)
}

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

fn should_retry_disable_with_runtime_target(
    outcome: bifrost_core::SystemProxyDisableOutcome,
    runtime_target: Option<&RuntimeSystemProxyTarget>,
) -> bool {
    matches!(
        outcome,
        bifrost_core::SystemProxyDisableOutcome::OwnedByOther
    ) && runtime_target.is_some()
}

fn disable_system_proxy_explicit(
    manager: &mut bifrost_core::SystemProxyManager,
) -> bifrost_core::Result<bifrost_core::SystemProxyDisableOutcome> {
    let outcome = manager.disable_managed_explicit()?;
    let runtime_target = read_valid_runtime_system_proxy_target();
    if should_retry_disable_with_runtime_target(outcome, runtime_target.as_ref()) {
        if let Some(target) = runtime_target {
            return manager.disable_if_matches_explicit(&target.host, target.port);
        }
    }

    Ok(outcome)
}

#[cfg(target_os = "macos")]
fn disable_system_proxy_explicit_with_privilege(
    manager: &mut bifrost_core::SystemProxyManager,
) -> bifrost_core::Result<bifrost_core::SystemProxyDisableOutcome> {
    let outcome = manager.disable_managed_explicit_with_privilege()?;
    let runtime_target = read_valid_runtime_system_proxy_target();
    if should_retry_disable_with_runtime_target(outcome, runtime_target.as_ref()) {
        if let Some(target) = runtime_target {
            return manager.disable_if_matches_explicit_with_privilege(&target.host, target.port);
        }
    }

    Ok(outcome)
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

fn cleanup_system_proxy_state(data_dir: &std::path::Path) -> bifrost_core::Result<()> {
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        "system proxy cleanup helper restore starting"
    );
    let started_at = std::time::Instant::now();
    let system_proxy_result = bifrost_core::retry_with_policy(
        bifrost_core::RECOVERY_RETRY_WINDOW,
        bifrost_core::RECOVERY_RETRY_INTERVAL,
        |attempt| {
            tracing::debug!(
                target: "bifrost_cli::shutdown",
                attempt,
                "system proxy cleanup helper invoking recover_from_crash"
            );
            bifrost_core::SystemProxyManager::recover_from_crash(data_dir)
        },
    );
    // CLI profile removal is independent from OS proxy recovery. Always attempt both so a
    // temporary networksetup/WinINET failure cannot leave shell proxy variables behind.
    let cli_proxy_result = bifrost_core::CliProxyEnvironmentManager::disable_all_managed();
    let cli_proxy_profile_count =
        combine_proxy_cleanup_results(system_proxy_result, cli_proxy_result)?;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        cli_proxy_profiles = cli_proxy_profile_count,
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        "proxy cleanup helper restore completed"
    );
    Ok(())
}

fn combine_proxy_cleanup_results(
    system_proxy_result: bifrost_core::Result<()>,
    cli_proxy_result: bifrost_core::Result<Vec<std::path::PathBuf>>,
) -> bifrost_core::Result<usize> {
    match (system_proxy_result, cli_proxy_result) {
        (Ok(()), Ok(paths)) => Ok(paths.len()),
        (Err(system_error), Ok(_)) => Err(system_error),
        (Ok(()), Err(cli_error)) => Err(cli_error),
        (Err(system_error), Err(cli_error)) => Err(bifrost_core::BifrostError::Config(format!(
            "System proxy cleanup failed: {system_error}; CLI proxy environment cleanup failed: {cli_error}"
        ))),
    }
}

fn should_try_managed_runtime_restart(runtime: &RuntimeInfo) -> bool {
    runtime.restartable_daemon()
        && runtime.binary_path.is_some()
        && runtime.system_proxy_enabled != Some(false)
}

fn build_managed_runtime_restart_args(
    runtime: &RuntimeInfo,
    snapshot: &RuntimeSystemProxySnapshot,
) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "--daemon".to_string(),
        "--yes".to_string(),
        "--port".to_string(),
        runtime.port.to_string(),
    ];
    if let Some(host) = runtime.host.as_deref().filter(|host| !host.is_empty()) {
        args.push("--host".to_string());
        args.push(host.to_string());
    }
    if let Some(socks5_port) = runtime.socks5_port {
        args.push("--socks5-port".to_string());
        args.push(socks5_port.to_string());
    }
    args.push("--system-proxy".to_string());
    args.push("--proxy-bypass".to_string());
    args.push(snapshot.bypass.clone());
    args
}

fn runtime_listener_is_alive(runtime: &RuntimeInfo) -> bool {
    use std::net::ToSocketAddrs;

    let host = runtime_system_proxy_host(runtime.host.as_deref());
    let Ok(addrs) = (host, runtime.port).to_socket_addrs() else {
        return false;
    };
    let timeout = std::time::Duration::from_millis(750);
    for addr in addrs {
        if std::net::TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return true;
        }
    }
    false
}

fn runtime_data_plane_is_ready(runtime: &RuntimeInfo) -> bool {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;

    let host = runtime_system_proxy_host(runtime.host.as_deref());
    let Ok(addrs) = (host, runtime.port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        let Ok(mut stream) =
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
        else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(750)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
        if stream
            .write_all(b"GET http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary HTTP/1.1\r\nHost: bifrost-runtime-canary.invalid\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            continue;
        }
        let mut response = [0_u8; 128];
        if let Ok(read) = stream.read(&mut response) {
            if response[..read].starts_with(b"HTTP/1.1 204") {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedRuntimeRestartOutcome {
    NotAttempted,
    Ready,
    FailOpenSuspended,
    FailClosedPreserved,
}

fn system_proxy_recovery_policy(
    data_dir: &std::path::Path,
) -> (SystemProxyRecoveryMode, std::time::Duration) {
    ConfigManager::new(data_dir.to_path_buf())
        .ok()
        .map(|manager| futures::executor::block_on(manager.config()).system_proxy)
        .map(|config| {
            (
                config.recovery_mode,
                std::time::Duration::from_secs(config.recovery_grace_secs.clamp(
                    MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
                    MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
                )),
            )
        })
        .unwrap_or((
            SystemProxyRecoveryMode::FailOpen,
            std::time::Duration::from_secs(MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS),
        ))
}

fn recovery_mode_name(mode: SystemProxyRecoveryMode) -> &'static str {
    match mode {
        SystemProxyRecoveryMode::FailOpen => "fail_open",
        SystemProxyRecoveryMode::FailClosed => "fail_closed",
    }
}

fn read_runtime_info_from(data_dir: &std::path::Path) -> Option<RuntimeInfo> {
    let content = std::fs::read_to_string(data_dir.join("runtime.json")).ok()?;
    serde_json::from_str(&content).ok()
}

fn restart_managed_runtime_before_cleanup(
    data_dir: &std::path::Path,
) -> ManagedRuntimeRestartOutcome {
    restart_managed_runtime_before_cleanup_with_timeout(
        data_dir,
        std::time::Duration::from_secs(30),
    )
}

fn restart_managed_runtime_before_cleanup_with_timeout(
    data_dir: &std::path::Path,
    ready_timeout: std::time::Duration,
) -> ManagedRuntimeRestartOutcome {
    let Some(runtime) = read_runtime_info_from(data_dir) else {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            data_dir = %data_dir.display(),
            "runtime_restart_skipped: runtime info missing"
        );
        return ManagedRuntimeRestartOutcome::NotAttempted;
    };

    tracing::info!(
        target: "bifrost_cli::shutdown",
        pid = runtime.pid,
        port = runtime.port,
        start_mode = ?runtime.start_mode,
        restartable_runtime = runtime.restartable_runtime,
        "runtime_restart_considered"
    );

    if runtime_data_plane_is_ready(&runtime) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: listener is already alive"
        );
        return ManagedRuntimeRestartOutcome::Ready;
    }

    if !should_try_managed_runtime_restart(&runtime) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: runtime is not restartable"
        );
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }

    let Some(binary_path) = runtime.binary_path.clone() else {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: runtime binary path missing"
        );
        return ManagedRuntimeRestartOutcome::NotAttempted;
    };
    if !binary_path.exists() {
        tracing::warn!(
            target: "bifrost_cli::shutdown",
            binary_path = %binary_path.display(),
            "runtime_restart_failed: runtime binary path does not exist"
        );
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }

    let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
    let ownership = match manager.ensure_managed_ownership() {
        Ok(Some(ownership)) => ownership,
        Ok(None) => return ManagedRuntimeRestartOutcome::NotAttempted,
        Err(error) => {
            tracing::warn!(error = %error, "runtime restart could not load proxy ownership");
            return ManagedRuntimeRestartOutcome::NotAttempted;
        }
    };
    let runtime_host = runtime_system_proxy_host(runtime.host.as_deref());
    if !ownership.target.target_matches(runtime_host, runtime.port) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            ownership_target_host = %ownership.target.host,
            ownership_target_port = ownership.target.port,
            "runtime_restart_skipped: persisted system proxy ownership belongs to another runtime target"
        );
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }
    // A previous recovery may already have fail-opened to the recorded
    // original proxy. The daemon must still be restarted in that state; the
    // generation-guarded resume path will re-enable only if nobody changed the
    // proxy afterward.
    let snapshot = capture_runtime_system_proxy_snapshot(Some(&runtime)).unwrap_or(
        RuntimeSystemProxySnapshot {
            bypass: ownership.target.bypass.clone(),
        },
    );
    let generation = ownership.generation.clone();
    let (recovery_mode, grace) = system_proxy_recovery_policy(data_dir);
    let recovery_started_at = std::time::Instant::now();
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.ownership_generation = Some(generation.clone());
        state.helper_pid = Some(std::process::id());
        state.helper_started_at_ms = bifrost_core::current_process_start_time_ms();
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.recovery_mode = Some(recovery_mode_name(recovery_mode).into());
        state.recovery_grace_secs = Some(grace.as_secs());
        state.phase = Some("restarting_daemon".into());
        state.last_action = Some("spawn_replacement".into());
    });
    let mut started_event = bifrost_core::SystemProxyLifecycleEvent::new(
        "helper_runtime_restart_started",
        "system_proxy_helper",
    );
    started_event.old_pid = Some(runtime.pid);
    started_event.ownership_generation = Some(generation.clone());
    started_event.trigger = Some(recovery_mode_name(recovery_mode).into());
    let _ = bifrost_core::append_system_proxy_event(data_dir, &started_event);

    if let Err(error) = bifrost_core::write_system_proxy_shutdown_mode(
        data_dir,
        bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
    ) {
        tracing::warn!(error = %error, "runtime restart failed to persist restart handoff marker");
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }

    let args = build_managed_runtime_restart_args(&runtime, &snapshot);
    tracing::info!(
        target: "bifrost_cli::shutdown",
        binary_path = %binary_path.display(),
        args = ?args,
        data_dir = %data_dir.display(),
        "runtime_restart_started"
    );

    let mut command = std::process::Command::new(&binary_path);
    command
        .args(&args)
        .env("BIFROST_DATA_DIR", data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    let _start_launcher = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                error = %error,
                "runtime_restart_failed: failed to spawn start command"
            );
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(data_dir);
            return apply_recovery_policy_after_failed_restart(
                data_dir,
                &mut manager,
                &generation,
                recovery_mode,
                recovery_started_at,
                Some(error.to_string()),
            );
        }
    };

    let deadline = std::time::Instant::now() + ready_timeout;
    let grace_deadline = std::time::Instant::now() + grace;
    let mut grace_action_applied = false;
    while std::time::Instant::now() < deadline {
        if runtime_data_plane_is_ready(&runtime) {
            let transition =
                if grace_action_applied && recovery_mode == SystemProxyRecoveryMode::FailOpen {
                    manager.resume_managed_if_generation(&generation)
                } else {
                    Ok(bifrost_core::GuardedSystemProxyTransition::AlreadyInState)
                };
            tracing::info!(
                target: "bifrost_cli::shutdown",
                port = runtime.port,
                transition = ?transition,
                "runtime_restart_succeeded"
            );
            let new_pid = read_runtime_info_from(data_dir).map(|runtime| runtime.pid);
            let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
                "helper_runtime_restart_ready",
                "system_proxy_helper",
            );
            event.old_pid = Some(runtime.pid);
            event.new_pid = new_pid;
            event.ownership_generation = Some(generation.clone());
            event.system_proxy_action = Some(format!("{transition:?}").to_ascii_lowercase());
            event.recovery_elapsed_ms = Some(recovery_started_at.elapsed().as_millis() as u64);
            let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
            let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
                state.pid = new_pid;
                state.phase = Some("running".into());
                state.last_action = Some("replacement_ready".into());
                state.last_error = transition.as_ref().err().map(ToString::to_string);
            });
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(data_dir);
            return ManagedRuntimeRestartOutcome::Ready;
        }
        if !grace_action_applied && std::time::Instant::now() >= grace_deadline {
            grace_action_applied = true;
            match recovery_mode {
                SystemProxyRecoveryMode::FailOpen => {
                    let transition = manager.suspend_managed_if_generation(&generation);
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        transition = ?transition,
                        "runtime restart exceeded grace period; fail-open restored direct connectivity"
                    );
                    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
                        "helper_fail_open_applied",
                        "system_proxy_helper",
                    );
                    event.old_pid = Some(runtime.pid);
                    event.ownership_generation = Some(generation.clone());
                    event.system_proxy_action =
                        Some(format!("{transition:?}").to_ascii_lowercase());
                    event.recovery_elapsed_ms =
                        Some(recovery_started_at.elapsed().as_millis() as u64);
                    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
                    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
                        state.phase = Some("recovering_fail_open".into());
                        state.last_action = Some("fail_open_after_grace".into());
                        state.last_error = transition.as_ref().err().map(ToString::to_string);
                    });
                }
                SystemProxyRecoveryMode::FailClosed => {
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        "runtime restart exceeded grace period; fail-closed preserved the managed proxy"
                    );
                    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
                        "helper_fail_closed_preserved",
                        "system_proxy_helper",
                    );
                    event.old_pid = Some(runtime.pid);
                    event.ownership_generation = Some(generation.clone());
                    event.system_proxy_action = Some("preserved".into());
                    event.recovery_elapsed_ms =
                        Some(recovery_started_at.elapsed().as_millis() as u64);
                    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
                    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
                        state.phase = Some("recovering_fail_closed".into());
                        state.last_action = Some("fail_closed_after_grace".into());
                    });
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    tracing::warn!(
        target: "bifrost_cli::shutdown",
        port = runtime.port,
        "runtime_restart_failed: listener did not become reachable before timeout"
    );
    // The replacement has already consumed the handoff decision during its
    // startup. Do not leave a stale preserve marker behind if it never reaches
    // readiness, otherwise a later helper invocation could skip recovery.
    let _ = bifrost_core::consume_system_proxy_shutdown_mode(data_dir);
    apply_recovery_policy_after_failed_restart(
        data_dir,
        &mut manager,
        &generation,
        recovery_mode,
        recovery_started_at,
        None,
    )
}

fn apply_recovery_policy_after_failed_restart(
    data_dir: &std::path::Path,
    manager: &mut bifrost_core::SystemProxyManager,
    generation: &str,
    recovery_mode: SystemProxyRecoveryMode,
    recovery_started_at: std::time::Instant,
    error: Option<String>,
) -> ManagedRuntimeRestartOutcome {
    let (outcome, action) = match recovery_mode {
        SystemProxyRecoveryMode::FailOpen => (
            ManagedRuntimeRestartOutcome::FailOpenSuspended,
            manager.suspend_managed_if_generation(generation),
        ),
        SystemProxyRecoveryMode::FailClosed => (
            ManagedRuntimeRestartOutcome::FailClosedPreserved,
            Ok(bifrost_core::GuardedSystemProxyTransition::AlreadyInState),
        ),
    };
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "helper_runtime_restart_not_ready",
        "system_proxy_helper",
    );
    event.ownership_generation = Some(generation.into());
    event.system_proxy_action = Some(format!("{action:?}").to_ascii_lowercase());
    event.error = error
        .clone()
        .or_else(|| action.as_ref().err().map(ToString::to_string));
    event.recovery_elapsed_ms = Some(recovery_started_at.elapsed().as_millis() as u64);
    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.phase = Some(
            match outcome {
                ManagedRuntimeRestartOutcome::FailOpenSuspended => "recovering_fail_open",
                ManagedRuntimeRestartOutcome::FailClosedPreserved => "recovering_fail_closed",
                _ => "recovering",
            }
            .into(),
        );
        state.last_action = Some(recovery_mode_name(recovery_mode).into());
        state.last_error = event.error.clone();
    });
    outcome
}

fn cleanup_or_restart_managed_runtime(data_dir: &std::path::Path) -> bifrost_core::Result<()> {
    match restart_managed_runtime_before_cleanup(data_dir) {
        ManagedRuntimeRestartOutcome::NotAttempted => cleanup_system_proxy_state(data_dir),
        ManagedRuntimeRestartOutcome::Ready
        | ManagedRuntimeRestartOutcome::FailOpenSuspended
        | ManagedRuntimeRestartOutcome::FailClosedPreserved => Ok(()),
    }
}

fn cleanup_after_parent_exit(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
    trigger: LifecycleRecoveryTrigger,
) -> bifrost_core::Result<()> {
    let recovery_started_at = std::time::Instant::now();
    let helper_pid = std::process::id();
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.helper_pid = Some(helper_pid);
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.phase = Some("parent_exit_recovery".into());
        state.last_action = Some(trigger.as_str().into());
    });
    tracing::info!(
        target: "bifrost_cli::shutdown",
        helper_pid,
        parent_pid = parent_pid.unwrap_or_default(),
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        detection_method = trigger.as_str(),
        data_dir = %data_dir.display(),
        "system proxy lifecycle recovery started"
    );
    let (action, result) = match bifrost_core::read_system_proxy_shutdown_mode(data_dir) {
        Some(bifrost_core::SystemProxyShutdownMode::BackgroundCleanup) => {
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(data_dir);
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper running stop-requested background cleanup"
            );
            ("background_cleanup", cleanup_system_proxy_state(data_dir))
        }
        Some(bifrost_core::SystemProxyShutdownMode::ForegroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper exiting because stop cleaned proxy before parent exit"
            );
            ("already_cleaned", Ok(()))
        }
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper skipping cleanup for restart"
            );
            ("preserve_for_restart", Ok(()))
        }
        None => (
            "restart_or_restore",
            cleanup_or_restart_managed_runtime(data_dir),
        ),
    };

    match &result {
        Ok(()) => tracing::info!(
            target: "bifrost_cli::shutdown",
            helper_pid,
            parent_pid = parent_pid.unwrap_or_default(),
            parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
            detection_method = trigger.as_str(),
            recovery_action = action,
            elapsed_ms = recovery_started_at.elapsed().as_millis() as u64,
            "system proxy lifecycle recovery completed"
        ),
        Err(error) => tracing::warn!(
            target: "bifrost_cli::shutdown",
            helper_pid,
            parent_pid = parent_pid.unwrap_or_default(),
            parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
            detection_method = trigger.as_str(),
            recovery_action = action,
            elapsed_ms = recovery_started_at.elapsed().as_millis() as u64,
            error = %error,
            "system proxy lifecycle recovery failed; managed proxy state may require manual repair"
        ),
    }
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "lifecycle_helper_recovery_completed",
        "system_proxy_helper",
    );
    event.old_pid = parent_pid;
    event.new_pid = read_runtime_info_from(data_dir).map(|runtime| runtime.pid);
    event.trigger = Some(trigger.as_str().into());
    event.decision = Some(action.into());
    event.error = result.as_ref().err().map(ToString::to_string);
    event.recovery_elapsed_ms = Some(recovery_started_at.elapsed().as_millis() as u64);
    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
    result
}

#[cfg(unix)]
fn stored_system_proxy_desired_state(data_dir: &std::path::Path) -> (bool, String) {
    match ConfigManager::new(data_dir.to_path_buf()) {
        Ok(config_manager) => {
            let config = futures::executor::block_on(config_manager.config());
            (
                config.system_proxy.enabled,
                config.system_proxy.bypass.clone(),
            )
        }
        Err(error) => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                error = %error,
                "system proxy wake reconcile could not read stored desired state"
            );
            (false, String::new())
        }
    }
}

#[cfg(unix)]
fn reapply_system_proxy_for_live_runtime(
    data_dir: &std::path::Path,
    runtime: &RuntimeInfo,
    bypass: &str,
    trigger: &str,
) -> bifrost_core::Result<()> {
    let target_host = runtime_system_proxy_host(runtime.host.as_deref());
    let target_port = runtime.port;
    let current = bifrost_core::SystemProxyManager::get_current()?;
    if current.enable && !current.target_matches(target_host, target_port) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            trigger,
            current_host = %current.host,
            current_port = current.port,
            target_host,
            target_port,
            "system proxy wake reconcile detected external owner; leaving it unchanged"
        );
        return Ok(());
    }

    let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
    manager.enable(target_host, target_port, Some(bypass))?;
    manager.detach();
    tracing::info!(
        target: "bifrost_cli::shutdown",
        trigger,
        target_host,
        target_port,
        "system proxy wake reconcile reapplied proxy for live runtime"
    );
    Ok(())
}

#[cfg(unix)]
fn reconcile_system_proxy_after_power_wake(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
) -> bifrost_core::Result<()> {
    const TRIGGER: &str = "power_notification";
    tracing::info!(
        target: "bifrost_cli::shutdown",
        trigger = TRIGGER,
        data_dir = %data_dir.display(),
        "system proxy wake reconcile starting"
    );

    if matches!(
        parent_identity_status(parent_pid, parent_started_at_ms),
        ProcessIdentityStatus::Reused
    ) {
        tracing::warn!(
            target: "bifrost_cli::shutdown",
            trigger = TRIGGER,
            parent_pid = parent_pid.unwrap_or_default(),
            "system proxy wake reconcile detected pid_reuse_check=mismatch; entering restart-before-cleanup"
        );
        return cleanup_or_restart_managed_runtime(data_dir);
    }

    if let Some(runtime) = read_runtime_info() {
        if !runtime_identity_is_current(&runtime) {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                trigger = TRIGGER,
                pid = runtime.pid,
                port = runtime.port,
                "system proxy wake reconcile found stale runtime identity; entering restart-before-cleanup"
            );
            return cleanup_or_restart_managed_runtime(data_dir);
        }

        if runtime_listener_is_alive(&runtime) {
            let (desired_enabled, bypass) = stored_system_proxy_desired_state(data_dir);
            if !desired_enabled {
                tracing::info!(
                    target: "bifrost_cli::shutdown",
                    trigger = TRIGGER,
                    pid = runtime.pid,
                    port = runtime.port,
                    "system proxy wake reconcile skipped because stored desired state is disabled and runtime is alive"
                );
                return Ok(());
            }
            return reapply_system_proxy_for_live_runtime(data_dir, &runtime, &bypass, TRIGGER);
        }

        tracing::warn!(
            target: "bifrost_cli::shutdown",
            trigger = TRIGGER,
            pid = runtime.pid,
            port = runtime.port,
            "system proxy wake reconcile found runtime without live listener; entering restart-before-cleanup"
        );
        return cleanup_or_restart_managed_runtime(data_dir);
    }

    tracing::warn!(
        target: "bifrost_cli::shutdown",
        trigger = TRIGGER,
        "system proxy wake reconcile found no runtime info; entering guarded cleanup"
    );
    cleanup_or_restart_managed_runtime(data_dir)
}

fn parent_identity_status(
    parent_pid: Option<u32>,
    recorded_started_at_ms: Option<u64>,
) -> ProcessIdentityStatus {
    parent_pid.map_or(ProcessIdentityStatus::Unknown, |pid| {
        inspect_process_identity(pid, recorded_started_at_ms)
    })
}

fn immediate_parent_exit_trigger(
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
) -> Option<LifecycleRecoveryTrigger> {
    match parent_identity_status(parent_pid, parent_started_at_ms) {
        ProcessIdentityStatus::Exited => Some(LifecycleRecoveryTrigger::PidMissing),
        ProcessIdentityStatus::Reused => Some(LifecycleRecoveryTrigger::PidReused),
        ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => None,
    }
}

fn record_lifecycle_helper_heartbeat(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    phase: &str,
) {
    if let Err(error) = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.helper_pid = Some(std::process::id());
        state.helper_started_at_ms = state
            .helper_started_at_ms
            .or_else(bifrost_core::current_process_start_time_ms);
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.pid = parent_pid.or(state.pid);
        state.phase = Some(phase.into());
    }) {
        tracing::warn!(error = %error, "failed to persist lifecycle helper heartbeat");
    }
}

fn run_system_proxy_lifecycle_helper(
    data_dir: std::path::PathBuf,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
    poll_secs: u64,
) -> bifrost_core::Result<()> {
    set_data_dir(data_dir.clone());
    let poll_interval = std::time::Duration::from_secs(poll_secs.max(1));
    let required_parent_misses = 3_u32;
    record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "lifecycle_helper_started",
        "system_proxy_helper",
    );
    event.old_pid = parent_pid;
    event.new_pid = Some(std::process::id());
    let _ = bifrost_core::append_system_proxy_event(&data_dir, &event);
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        parent_pid = parent_pid.unwrap_or_default(),
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        poll_secs = poll_interval.as_secs(),
        fast_identity_poll_ms = 250_u64,
        required_parent_misses,
        "system proxy lifecycle helper started; fast process-identity checks do not use listener or HTTP readiness"
    );

    match parent_identity_status(parent_pid, parent_started_at_ms) {
        ProcessIdentityStatus::Reused => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                parent_pid = parent_pid.unwrap_or_default(),
                detection_method = LifecycleRecoveryTrigger::PidReused.as_str(),
                "system proxy lifecycle helper detected parent PID reuse at startup; running guarded recovery"
            );
            return cleanup_after_parent_exit(
                &data_dir,
                parent_pid,
                parent_started_at_ms,
                LifecycleRecoveryTrigger::PidReused,
            );
        }
        ProcessIdentityStatus::Exited => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                parent_pid = parent_pid.unwrap_or_default(),
                detection_method = LifecycleRecoveryTrigger::PidMissing.as_str(),
                "system proxy lifecycle helper observed parent PID missing at startup; running immediate guarded recovery"
            );
            return cleanup_after_parent_exit(
                &data_dir,
                parent_pid,
                parent_started_at_ms,
                LifecycleRecoveryTrigger::PidMissing,
            );
        }
        ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => {}
    }

    #[cfg(unix)]
    {
        let (power_tx, power_rx) = std::sync::mpsc::channel::<PowerEvent>();
        #[cfg(target_os = "macos")]
        let _power_watcher = {
            match PowerNotificationWatcher::start(power_tx) {
                Ok(watcher) => {
                    tracing::info!(
                        target: "bifrost_cli::shutdown",
                        "system proxy lifecycle helper power watcher started"
                    );
                    Some(watcher)
                }
                Err(error) => {
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        error = %error,
                        "system proxy lifecycle helper power watcher failed to start"
                    );
                    None
                }
            }
        };
        #[cfg(not(target_os = "macos"))]
        let _power_watcher = {
            drop(power_tx);
            None::<()>
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to start system proxy lifecycle helper runtime: {error}"
                ))
            })?;
        runtime.block_on(async move {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm = signal(SignalKind::terminate()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGTERM handler for system proxy lifecycle helper: {error}"
                ))
            })?;
            let mut sigint = signal(SignalKind::interrupt()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGINT handler for system proxy lifecycle helper: {error}"
                ))
            })?;
            let mut sighup = signal(SignalKind::hangup()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGHUP handler for system proxy lifecycle helper: {error}"
                ))
            })?;

            let mut consecutive_parent_misses = 0_u32;
            let mut parent_poll = tokio::time::interval(poll_interval);
            parent_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // A direct process-instance disappearance is a strong liveness
            // signal. Check it much more frequently than the conservative
            // zombie/legacy fallback, without involving the proxy port or
            // Admin readiness endpoint.
            let mut parent_identity_poll =
                tokio::time::interval(std::time::Duration::from_millis(250));
            parent_identity_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut power_poll = tokio::time::interval(std::time::Duration::from_millis(250));
            power_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut helper_heartbeat_poll =
                tokio::time::interval(std::time::Duration::from_secs(5));
            helper_heartbeat_poll
                .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = helper_heartbeat_poll.tick() => {
                        record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
                    },
                    _ = sigterm.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGTERM");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sigterm"));
                    },
                    _ = sigint.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGINT");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sigint"));
                    },
                    _ = sighup.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGHUP");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sighup"));
                    },
                    _ = parent_identity_poll.tick() => {
                        if let Some(trigger) = immediate_parent_exit_trigger(
                            parent_pid,
                            parent_started_at_ms,
                        ) {
                            tracing::info!(
                                target: "bifrost_cli::shutdown",
                                parent_pid = parent_pid.unwrap_or_default(),
                                detection_method = trigger.as_str(),
                                "system proxy lifecycle helper observed confirmed parent-instance exit during fast identity check"
                            );
                            return cleanup_after_parent_exit(
                                &data_dir,
                                parent_pid,
                                parent_started_at_ms,
                                trigger,
                            );
                        }
                    },
                    _ = power_poll.tick() => {
                        while let Ok(event) = power_rx.try_recv() {
                            tracing::info!(
                                target: "bifrost_cli::shutdown",
                                event = ?event,
                                "system proxy lifecycle helper received power notification"
                            );
                            if event == PowerEvent::SystemHasPoweredOn {
                                if let Err(error) = reconcile_system_proxy_after_power_wake(
                                    &data_dir,
                                    parent_pid,
                                    parent_started_at_ms,
                                ) {
                                    tracing::warn!(
                                        target: "bifrost_cli::shutdown",
                                        error = %error,
                                        "system proxy wake reconcile failed after power notification"
                                    );
                                }
                            }
                        }
                    },
                    _ = parent_poll.tick() => {
                        if let Some(pid) = parent_pid {
                            if !is_process_running(pid) {
                                consecutive_parent_misses += 1;
                                tracing::warn!(
                                    target: "bifrost_cli::shutdown",
                                    parent_pid = pid,
                                    consecutive_parent_misses,
                                    required_parent_misses,
                                    "system proxy lifecycle helper parent process not visible"
                                );
                                if consecutive_parent_misses >= required_parent_misses {
                                    tracing::info!(
                                        target: "bifrost_cli::shutdown",
                                        parent_pid = pid,
                                        "system proxy lifecycle helper confirmed parent exit"
                                    );
                                    return cleanup_after_parent_exit(
                                        &data_dir,
                                        parent_pid,
                                        parent_started_at_ms,
                                        LifecycleRecoveryTrigger::PollConfirmedExit,
                                    );
                                }
                            } else {
                                consecutive_parent_misses = 0;
                            }
                        }
                    },
                }
            }
        })
    }

    #[cfg(not(unix))]
    {
        let mut consecutive_parent_misses = 0_u32;
        let fast_identity_interval = std::time::Duration::from_millis(250);
        let mut next_parent_poll = std::time::Instant::now() + poll_interval;
        let mut next_helper_heartbeat =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            std::thread::sleep(fast_identity_interval);
            if std::time::Instant::now() >= next_helper_heartbeat {
                record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
                next_helper_heartbeat =
                    std::time::Instant::now() + std::time::Duration::from_secs(5);
            }
            match parent_identity_status(parent_pid, parent_started_at_ms) {
                ProcessIdentityStatus::Reused => {
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = parent_pid.unwrap_or_default(),
                        detection_method = LifecycleRecoveryTrigger::PidReused.as_str(),
                        "system proxy lifecycle helper detected parent PID reuse; running guarded recovery"
                    );
                    return cleanup_after_parent_exit(
                        &data_dir,
                        parent_pid,
                        parent_started_at_ms,
                        LifecycleRecoveryTrigger::PidReused,
                    );
                }
                ProcessIdentityStatus::Exited => {
                    tracing::info!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = parent_pid.unwrap_or_default(),
                        detection_method = LifecycleRecoveryTrigger::PidMissing.as_str(),
                        "system proxy lifecycle helper observed parent PID missing; running immediate guarded recovery"
                    );
                    return cleanup_after_parent_exit(
                        &data_dir,
                        parent_pid,
                        parent_started_at_ms,
                        LifecycleRecoveryTrigger::PidMissing,
                    );
                }
                ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => {}
            }

            // Keep the historical boolean probe at its conservative cadence.
            // It still handles zombie and platform-specific fallback cases, but
            // it must not delay an explicit PID-instance disappearance.
            if std::time::Instant::now() < next_parent_poll {
                continue;
            }
            next_parent_poll = std::time::Instant::now() + poll_interval;
            if let Some(pid) = parent_pid {
                if !is_process_running(pid) {
                    consecutive_parent_misses += 1;
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = pid,
                        consecutive_parent_misses,
                        required_parent_misses,
                        "system proxy lifecycle helper parent process not visible"
                    );
                    if consecutive_parent_misses >= required_parent_misses {
                        tracing::info!(
                            target: "bifrost_cli::shutdown",
                            parent_pid = pid,
                            "system proxy lifecycle helper confirmed parent exit"
                        );
                        return cleanup_after_parent_exit(
                            &data_dir,
                            parent_pid,
                            parent_started_at_ms,
                            LifecycleRecoveryTrigger::PollConfirmedExit,
                        );
                    }
                } else {
                    consecutive_parent_misses = 0;
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run_system_proxy_cleanup_daemon(
    data_dir: std::path::PathBuf,
    installed_version: Option<String>,
) -> bifrost_core::Result<()> {
    set_data_dir(data_dir.clone());
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        installed_version = installed_version.as_deref().unwrap_or(""),
        current_version = bifrost_core::system_proxy_launchd::CURRENT_VERSION,
        "system proxy launchd cleanup daemon started"
    );

    let startup_started_at = std::time::Instant::now();
    match bifrost_core::system_proxy_launchd::recover_if_no_live_runtime_with_startup_retry(
        &data_dir,
    ) {
        Ok(bifrost_core::SystemProxyLaunchdRecoveryOutcome::Recovered) => tracing::info!(
            target: "bifrost_cli::shutdown",
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery completed"
        ),
        Ok(bifrost_core::SystemProxyLaunchdRecoveryOutcome::Skipped) => tracing::info!(
            target: "bifrost_cli::shutdown",
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery skipped"
        ),
        Err(error) => tracing::warn!(
            target: "bifrost_cli::shutdown",
            error = %error,
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery failed"
        ),
    }

    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        installed_version = installed_version.as_deref().unwrap_or(""),
        current_version = bifrost_core::system_proxy_launchd::CURRENT_VERSION,
        "system proxy launchd cleanup daemon exiting after one-shot recovery check"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RuntimeStartMode;
    use std::path::PathBuf;

    fn cleanup_error(message: &str) -> bifrost_core::BifrostError {
        bifrost_core::BifrostError::Config(message.to_string())
    }

    fn unused_loopback_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn write_restart_fixture(
        data_dir: &std::path::Path,
        port: u16,
        binary_path: PathBuf,
        restartable: bool,
        target_port: u16,
        applied: bool,
    ) {
        let runtime = RuntimeInfo {
            pid: 424_242,
            port,
            socks5_port: None,
            host: Some("127.0.0.1".into()),
            started_at_ms: Some(1),
            start_mode: if restartable {
                RuntimeStartMode::Daemon
            } else {
                RuntimeStartMode::Foreground
            },
            restartable_runtime: restartable,
            binary_path: Some(binary_path),
            system_proxy_enabled: Some(true),
            system_proxy_bypass: Some("localhost,127.0.0.1".into()),
            health_port: None,
        };
        std::fs::write(
            data_dir.join("runtime.json"),
            serde_json::to_vec_pretty(&runtime).unwrap(),
        )
        .unwrap();
        std::fs::write(
            data_dir.join("proxy_state.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 2,
                "generation": "generation-fixture",
                "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
                "target": {
                    "enable": true,
                    "host": "127.0.0.1",
                    "port": target_port,
                    "bypass": "localhost,127.0.0.1"
                },
                "applied": applied
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn combined_proxy_cleanup_reports_success_and_each_failure_shape() {
        assert_eq!(
            combine_proxy_cleanup_results(
                Ok(()),
                Ok(vec![PathBuf::from(".zshrc"), PathBuf::from(".bashrc")])
            )
            .unwrap(),
            2
        );

        let system_only =
            combine_proxy_cleanup_results(Err(cleanup_error("system failed")), Ok(Vec::new()))
                .unwrap_err()
                .to_string();
        assert!(system_only.contains("system failed"));

        let cli_only = combine_proxy_cleanup_results(Ok(()), Err(cleanup_error("profile failed")))
            .unwrap_err()
            .to_string();
        assert!(cli_only.contains("profile failed"));

        let both = combine_proxy_cleanup_results(
            Err(cleanup_error("system failed")),
            Err(cleanup_error("profile failed")),
        )
        .unwrap_err()
        .to_string();
        assert!(both.contains("System proxy cleanup failed"));
        assert!(both.contains("system failed"));
        assert!(both.contains("CLI proxy environment cleanup failed"));
        assert!(both.contains("profile failed"));
    }

    #[test]
    fn parent_identity_status_detects_pid_reuse_from_start_time_mismatch() {
        let recorded = bifrost_core::current_process_start_time_ms()
            .map(|started_at_ms| started_at_ms.saturating_add(10_000));

        assert_eq!(
            parent_identity_status(Some(std::process::id()), recorded),
            ProcessIdentityStatus::Reused
        );
        assert_eq!(
            immediate_parent_exit_trigger(Some(std::process::id()), recorded),
            Some(LifecycleRecoveryTrigger::PidReused)
        );
    }

    #[test]
    fn runtime_identity_rejects_start_time_mismatch() {
        let runtime = RuntimeInfo {
            pid: std::process::id(),
            port: 18889,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: bifrost_core::current_process_start_time_ms()
                .map(|started_at_ms| started_at_ms.saturating_add(10_000)),
            start_mode: RuntimeStartMode::Foreground,
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        };

        assert!(!runtime_identity_is_current(&runtime));
    }

    #[test]
    fn managed_runtime_restart_skips_foreground_runtime() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Foreground,
            restartable_runtime: false,
            binary_path: Some(PathBuf::from("/tmp/bifrost")),
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        };

        assert!(!should_try_managed_runtime_restart(&runtime));
    }

    #[test]
    fn managed_runtime_restart_requires_binary_path() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        };

        assert!(!should_try_managed_runtime_restart(&runtime));
    }

    #[test]
    fn managed_runtime_restart_skips_explicitly_disabled_system_proxy() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: Some(PathBuf::from("/tmp/bifrost")),
            system_proxy_enabled: Some(false),
            system_proxy_bypass: None,
            health_port: None,
        };

        assert!(!should_try_managed_runtime_restart(&runtime));
    }

    #[test]
    fn managed_runtime_restart_args_preserve_runtime_and_system_proxy() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 18889,
            socks5_port: Some(18890),
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: Some(PathBuf::from("/tmp/bifrost")),
            system_proxy_enabled: Some(true),
            system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
            health_port: None,
        };
        let snapshot = RuntimeSystemProxySnapshot {
            bypass: "localhost,127.0.0.1,*.local".to_string(),
        };

        assert_eq!(
            build_managed_runtime_restart_args(&runtime, &snapshot),
            vec![
                "start",
                "--daemon",
                "--yes",
                "--port",
                "18889",
                "--host",
                "0.0.0.0",
                "--socks5-port",
                "18890",
                "--system-proxy",
                "--proxy-bypass",
                "localhost,127.0.0.1,*.local"
            ]
        );
    }

    #[test]
    fn runtime_info_system_proxy_target_maps_wildcard_to_loopback() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 18889,
            socks5_port: None,
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Foreground,
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        };

        assert_eq!(
            runtime_info_system_proxy_target(&runtime),
            RuntimeSystemProxyTarget {
                host: "127.0.0.1".to_string(),
                port: 18889
            }
        );
    }

    #[test]
    fn cli_disable_retries_with_runtime_target_only_for_owned_by_other() {
        let target = RuntimeSystemProxyTarget {
            host: "127.0.0.1".to_string(),
            port: 18889,
        };

        assert!(should_retry_disable_with_runtime_target(
            bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
            Some(&target)
        ));
        assert!(!should_retry_disable_with_runtime_target(
            bifrost_core::SystemProxyDisableOutcome::Disabled,
            Some(&target)
        ));
        assert!(!should_retry_disable_with_runtime_target(
            bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
            None
        ));
    }

    #[test]
    fn cli_disable_does_not_retry_for_non_owned_by_other_outcomes() {
        let target = RuntimeSystemProxyTarget {
            host: "127.0.0.1".to_string(),
            port: 18889,
        };

        assert!(!should_retry_disable_with_runtime_target(
            bifrost_core::SystemProxyDisableOutcome::Disabled,
            Some(&target)
        ));
        assert!(!should_retry_disable_with_runtime_target(
            bifrost_core::SystemProxyDisableOutcome::NotEnabled,
            Some(&target)
        ));
    }

    #[test]
    fn parent_identity_status_is_unknown_without_parent_pid() {
        assert_eq!(
            parent_identity_status(None, Some(123)),
            ProcessIdentityStatus::Unknown
        );
        assert_eq!(
            parent_identity_status(None, None),
            ProcessIdentityStatus::Unknown
        );
        assert_eq!(immediate_parent_exit_trigger(None, None), None);
    }

    #[test]
    fn lifecycle_recovery_trigger_names_are_diagnostic_stable() {
        assert_eq!(LifecycleRecoveryTrigger::PidMissing.as_str(), "pid_missing");
        assert_eq!(LifecycleRecoveryTrigger::PidReused.as_str(), "pid_reused");
        assert_eq!(
            LifecycleRecoveryTrigger::PollConfirmedExit.as_str(),
            "poll_confirmed_exit"
        );
        assert_eq!(
            LifecycleRecoveryTrigger::Signal("sigterm").as_str(),
            "sigterm"
        );
    }

    #[test]
    fn runtime_info_system_proxy_target_preserves_specific_host() {
        let runtime = RuntimeInfo {
            pid: 123,
            port: 18889,
            socks5_port: None,
            host: Some("example.com".to_string()),
            started_at_ms: None,
            start_mode: RuntimeStartMode::Foreground,
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        };

        assert_eq!(
            runtime_info_system_proxy_target(&runtime),
            RuntimeSystemProxyTarget {
                host: "example.com".to_string(),
                port: 18889,
            }
        );
    }

    #[test]
    fn doctor_report_is_read_only_and_explains_missing_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let manager = bifrost_core::SystemProxyManager::new(dir.path().to_path_buf());
        let report = build_system_proxy_doctor_report(dir.path(), &manager);

        assert!(report.runtime.is_none());
        assert_eq!(report.runtime_identity, "Missing");
        assert!(report
            .findings
            .iter()
            .any(|finding| finding == "runtime marker is missing"));
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.contains("health_port")));
        let text =
            render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::Text).unwrap();
        assert!(text.contains("Runtime identity:    Missing"));
        assert!(
            render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::Json)
                .unwrap()
                .starts_with('{')
        );
        assert!(
            render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::JsonPretty)
                .unwrap()
                .contains("\n  \"runtime\"")
        );
        assert!(!dir.path().join("system_proxy_owner.json").exists());
        assert!(!dir.path().join("system_proxy_events.jsonl").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn doctor_report_covers_healthy_stale_invalid_and_ownership_findings() {
        fn health_server(body: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
            use std::io::{Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 512];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            });
            (port, thread)
        }

        let dir = tempfile::tempdir().unwrap();
        let manager = bifrost_core::SystemProxyManager::new(dir.path().to_path_buf());
        let mut runtime = RuntimeInfo {
            pid: std::process::id(),
            port: unused_loopback_port(),
            socks5_port: None,
            host: Some("127.0.0.1".into()),
            started_at_ms: bifrost_core::current_process_start_time_ms(),
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: std::env::current_exe().ok(),
            system_proxy_enabled: Some(true),
            system_proxy_bypass: Some("localhost".into()),
            health_port: None,
        };
        let healthy = bifrost_core::RuntimeHealthSnapshot {
            scheduler_heartbeat_age_ms: 1,
            ..Default::default()
        };
        let (health_port, health_thread) = health_server(serde_json::to_vec(&healthy).unwrap());
        runtime.health_port = Some(health_port);
        std::fs::write(
            dir.path().join("runtime.json"),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        let report = build_system_proxy_doctor_report(dir.path(), &manager);
        health_thread.join().unwrap();
        assert_eq!(report.runtime_identity, "Alive");
        assert!(report.health.is_some());
        assert!(report
            .findings
            .iter()
            .any(|finding| finding == "no blocking ownership or runtime health issue detected"));

        let stale = bifrost_core::RuntimeHealthSnapshot {
            scheduler_heartbeat_age_ms: 5_001,
            ..Default::default()
        };
        let (health_port, health_thread) = health_server(serde_json::to_vec(&stale).unwrap());
        runtime.health_port = Some(health_port);
        std::fs::write(
            dir.path().join("runtime.json"),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("proxy_state.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "generation": "doctor-generation",
                "original": {"enable": true, "host": "unrelated-original.invalid", "port": 54321, "bypass": ""},
                "target": {"enable": true, "host": "unrelated-target.invalid", "port": 54322, "bypass": ""},
                "applied": true
            }))
            .unwrap(),
        )
        .unwrap();
        let report = build_system_proxy_doctor_report(dir.path(), &manager);
        health_thread.join().unwrap();
        assert!(report
            .findings
            .iter()
            .any(|finding| finding == "scheduler heartbeat is stale"));
        assert!(report.findings.iter().any(|finding| {
            finding == "managed state says applied but OS proxy ownership changed"
        }));

        let (health_port, health_thread) = health_server(b"not-json".to_vec());
        runtime.health_port = Some(health_port);
        std::fs::write(
            dir.path().join("runtime.json"),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        let invalid = build_system_proxy_doctor_report(dir.path(), &manager);
        health_thread.join().unwrap();
        assert!(invalid
            .health_error
            .as_deref()
            .is_some_and(|error| error.contains("invalid health response")));

        runtime.health_port = Some(unused_loopback_port());
        std::fs::write(
            dir.path().join("proxy_state.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "generation": "doctor-generation",
                "original": {"enable": true, "host": "unrelated-original.invalid", "port": 54321, "bypass": ""},
                "target": {"enable": true, "host": "unrelated-target.invalid", "port": 54322, "bypass": ""},
                "applied": false
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("runtime.json"),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        let unavailable = build_system_proxy_doctor_report(dir.path(), &manager);
        assert!(unavailable
            .health_error
            .as_deref()
            .is_some_and(|error| error.contains("health lane unavailable")));
        assert!(unavailable.findings.iter().any(|finding| {
            finding == "fail-open state no longer matches the recorded original proxy"
        }));
    }

    #[test]
    fn failed_restart_fail_closed_persists_diagnostics_without_touching_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = bifrost_core::SystemProxyManager::new(dir.path().to_path_buf());

        let outcome = apply_recovery_policy_after_failed_restart(
            dir.path(),
            &mut manager,
            "generation-closed",
            SystemProxyRecoveryMode::FailClosed,
            std::time::Instant::now(),
            Some("spawn failed".into()),
        );

        assert_eq!(outcome, ManagedRuntimeRestartOutcome::FailClosedPreserved);
        let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(owner.phase.as_deref(), Some("recovering_fail_closed"));
        assert_eq!(owner.last_error.as_deref(), Some("spawn failed"));
        let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 5).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "helper_runtime_restart_not_ready");
        assert_eq!(
            events[0].ownership_generation.as_deref(),
            Some("generation-closed")
        );
    }

    #[test]
    fn recovery_policy_helper_persists_both_modes() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            persist_system_proxy_recovery_policy(&manager, "fail-closed", 3).unwrap(),
            SystemProxyRecoveryMode::FailClosed
        );
        assert_eq!(
            futures::executor::block_on(manager.config())
                .system_proxy
                .recovery_mode,
            SystemProxyRecoveryMode::FailClosed
        );
        assert_eq!(
            persist_system_proxy_recovery_policy(&manager, "anything-else", 5).unwrap(),
            SystemProxyRecoveryMode::FailOpen
        );
        let config = futures::executor::block_on(manager.config());
        assert_eq!(
            config.system_proxy.recovery_mode,
            SystemProxyRecoveryMode::FailOpen
        );
        assert_eq!(config.system_proxy.recovery_grace_secs, 5);
    }

    #[test]
    fn parent_exit_markers_complete_without_proxy_mutation() {
        for mode in [
            bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        ] {
            let dir = tempfile::tempdir().unwrap();
            bifrost_core::write_system_proxy_shutdown_mode(dir.path(), mode).unwrap();
            cleanup_after_parent_exit(
                dir.path(),
                Some(424_242),
                Some(1),
                LifecycleRecoveryTrigger::Signal("test"),
            )
            .unwrap();

            let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(owner.phase.as_deref(), Some("parent_exit_recovery"));
            let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 5).unwrap();
            assert_eq!(
                events.last().unwrap().event,
                "lifecycle_helper_recovery_completed"
            );
            assert_eq!(events.last().unwrap().trigger.as_deref(), Some("test"));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_restart_launches_replacement_and_waits_for_data_canary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let fake_binary = dir.path().join("fake-bifrost.py");
        std::fs::write(
            &fake_binary,
            r#"#!/usr/bin/env python3
import os, socket, sys
port = int(sys.argv[sys.argv.index('--port') + 1])
data_dir = os.environ['BIFROST_DATA_DIR']
with open(os.path.join(data_dir, 'replacement.pid'), 'w') as output:
    output.write(str(os.getpid()))
server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(('127.0.0.1', port))
server.listen(8)
while True:
    connection, _ = server.accept()
    connection.recv(4096)
    connection.sendall(b'HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
    connection.close()
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_binary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_binary, permissions).unwrap();

        let runtime = RuntimeInfo {
            pid: 424_242,
            port,
            socks5_port: None,
            host: Some("127.0.0.1".into()),
            started_at_ms: Some(1),
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: Some(fake_binary),
            system_proxy_enabled: Some(true),
            system_proxy_bypass: Some("localhost,127.0.0.1".into()),
            health_port: None,
        };
        std::fs::write(
            dir.path().join("runtime.json"),
            serde_json::to_vec_pretty(&runtime).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("proxy_state.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 2,
                "generation": "generation-ready",
                "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
                "target": {
                    "enable": true,
                    "host": "127.0.0.1",
                    "port": port,
                    "bypass": "localhost,127.0.0.1"
                },
                "applied": true
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::Ready
        );
        let child_pid = std::fs::read_to_string(dir.path().join("replacement.pid"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &child_pid.to_string()])
            .status();
        let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 10).unwrap();
        assert!(events
            .iter()
            .any(|event| event.event == "helper_runtime_restart_started"));
        assert!(events
            .iter()
            .any(|event| event.event == "helper_runtime_restart_ready"));
        assert!(bifrost_core::read_system_proxy_shutdown_mode(dir.path()).is_none());
    }

    #[test]
    fn managed_restart_preflight_rejects_incomplete_or_foreign_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );

        let port = unused_loopback_port();
        write_restart_fixture(
            dir.path(),
            port,
            PathBuf::from("/usr/bin/true"),
            false,
            port,
            true,
        );
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );

        write_restart_fixture(
            dir.path(),
            port,
            dir.path().join("missing-binary"),
            true,
            port,
            true,
        );
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );

        write_restart_fixture(
            dir.path(),
            port,
            PathBuf::from("/usr/bin/true"),
            true,
            port + 1,
            true,
        );
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );

        std::fs::write(dir.path().join("proxy_state.json"), "invalid").unwrap();
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );
        std::fs::remove_file(dir.path().join("proxy_state.json")).unwrap();
        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_restart_spawn_failure_applies_fail_open_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let port = unused_loopback_port();
        let not_executable = dir.path().join("not-executable");
        std::fs::create_dir(&not_executable).unwrap();
        write_restart_fixture(dir.path(), port, not_executable, true, port, false);

        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::FailOpenSuspended
        );
        let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(owner.phase.as_deref(), Some("recovering_fail_open"));
        assert!(owner.last_error.is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_restart_grace_applies_both_recovery_policies() {
        for (mode, applied, expected) in [
            (
                SystemProxyRecoveryMode::FailOpen,
                false,
                ManagedRuntimeRestartOutcome::FailOpenSuspended,
            ),
            (
                SystemProxyRecoveryMode::FailClosed,
                true,
                ManagedRuntimeRestartOutcome::FailClosedPreserved,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let port = unused_loopback_port();
            write_restart_fixture(
                dir.path(),
                port,
                PathBuf::from("/usr/bin/true"),
                true,
                port,
                applied,
            );
            let config_manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
            futures::executor::block_on(config_manager.update_system_proxy_config(
                SystemProxyConfigUpdate {
                    enabled: None,
                    bypass: None,
                    auto_enable: None,
                    recovery_mode: Some(mode),
                    recovery_grace_secs: Some(MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS),
                },
            ))
            .unwrap();

            assert_eq!(
                restart_managed_runtime_before_cleanup_with_timeout(
                    dir.path(),
                    std::time::Duration::from_millis(3_250),
                ),
                expected
            );
            let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 20).unwrap();
            assert!(events.iter().any(|event| {
                event.event
                    == match mode {
                        SystemProxyRecoveryMode::FailOpen => "helper_fail_open_applied",
                        SystemProxyRecoveryMode::FailClosed => "helper_fail_closed_preserved",
                    }
            }));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn managed_restart_is_not_attempted_when_system_proxy_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let port = unused_loopback_port();
        write_restart_fixture(
            dir.path(),
            port,
            std::env::current_exe().unwrap(),
            true,
            port,
            true,
        );

        assert_eq!(
            restart_managed_runtime_before_cleanup(dir.path()),
            ManagedRuntimeRestartOutcome::NotAttempted
        );
    }

    #[test]
    fn status_renderer_reports_recovery_policy_and_external_owner_warning() {
        let status = bifrost_core::ProxyBackup {
            enable: true,
            host: "external.proxy".into(),
            port: 8080,
            bypass: "localhost".into(),
        };
        let configured = bifrost_storage::NewSystemProxyConfig {
            enabled: true,
            bypass: "localhost,127.0.0.1".into(),
            auto_enable: true,
            recovery_mode: SystemProxyRecoveryMode::FailClosed,
            recovery_grace_secs: 3,
        };

        let external = render_system_proxy_status(&status, false, &configured);
        assert!(external.contains("Recovery policy:     fail_closed (3s)"));
        assert!(external.contains("enabled by another application"));

        let managed = render_system_proxy_status(&status, true, &configured);
        assert!(managed.contains("Managed by Bifrost:  true"));
        assert!(!managed.contains("another application"));
    }
}
