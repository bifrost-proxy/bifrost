use bifrost_storage::{set_data_dir, ConfigManager, SystemProxyConfigUpdate};

#[cfg(target_os = "macos")]
use crate::cli::SystemProxyLaunchdCommands;
use crate::cli::{Cli, SystemProxyCommands};
use crate::config::get_bifrost_dir;
use crate::process::{
    capture_runtime_system_proxy_snapshot, is_process_running, read_runtime_info,
    runtime_system_proxy_host, RuntimeInfo, RuntimeSystemProxySnapshot,
};
#[cfg(unix)]
use bifrost_power::PowerEvent;
#[cfg(target_os = "macos")]
use bifrost_power::PowerNotificationWatcher;

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
                    println!("Supported: true");
                    println!("Enabled:             {}", status.enable);
                    println!("Host:                {}", status.host);
                    println!("Port:                {}", status.port);
                    println!("Bypass:              {}", status.bypass);
                    println!("Managed by Bifrost:  {}", managed_by_bifrost);
                    println!(
                        "Configured enabled:  {}",
                        stored_config.system_proxy.enabled
                    );
                    println!("Configured bypass:   {}", stored_config.system_proxy.bypass);
                    if status.enable && !managed_by_bifrost {
                        println!(
                            "System proxy is enabled by another application; Bifrost will leave it unchanged."
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Failed to get system proxy: {}", e);
                }
            }
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
    bifrost_core::retry_with_policy(
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
    )?;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        "system proxy cleanup helper restore completed"
    );
    Ok(())
}

fn should_try_managed_runtime_restart(runtime: &RuntimeInfo) -> bool {
    runtime.restartable_daemon() && runtime.binary_path.is_some()
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

fn restart_managed_runtime_before_cleanup(data_dir: &std::path::Path) -> bool {
    let Some(runtime) = read_runtime_info() else {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            data_dir = %data_dir.display(),
            "runtime_restart_skipped: runtime info missing"
        );
        return false;
    };

    tracing::info!(
        target: "bifrost_cli::shutdown",
        pid = runtime.pid,
        port = runtime.port,
        start_mode = ?runtime.start_mode,
        restartable_runtime = runtime.restartable_runtime,
        "runtime_restart_considered"
    );

    if runtime_listener_is_alive(&runtime) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: listener is already alive"
        );
        return true;
    }

    if !should_try_managed_runtime_restart(&runtime) {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: runtime is not restartable"
        );
        return false;
    }

    let Some(snapshot) = capture_runtime_system_proxy_snapshot(Some(&runtime)) else {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: current system proxy is not owned by previous runtime"
        );
        return false;
    };

    let Some(binary_path) = runtime.binary_path.clone() else {
        tracing::info!(
            target: "bifrost_cli::shutdown",
            port = runtime.port,
            "runtime_restart_skipped: runtime binary path missing"
        );
        return false;
    };
    if !binary_path.exists() {
        tracing::warn!(
            target: "bifrost_cli::shutdown",
            binary_path = %binary_path.display(),
            "runtime_restart_failed: runtime binary path does not exist"
        );
        return false;
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

    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                status = %status,
                "runtime_restart_failed: start command exited unsuccessfully"
            );
            return false;
        }
        Err(error) => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                error = %error,
                "runtime_restart_failed: failed to spawn start command"
            );
            return false;
        }
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if runtime_listener_is_alive(&runtime) {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                port = runtime.port,
                "runtime_restart_succeeded"
            );
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    tracing::warn!(
        target: "bifrost_cli::shutdown",
        port = runtime.port,
        "runtime_restart_failed: listener did not become reachable before timeout"
    );
    false
}

fn cleanup_or_restart_managed_runtime(data_dir: &std::path::Path) -> bifrost_core::Result<()> {
    if restart_managed_runtime_before_cleanup(data_dir) {
        return Ok(());
    }
    cleanup_system_proxy_state(data_dir)
}

fn cleanup_after_parent_exit(data_dir: &std::path::Path) -> bifrost_core::Result<()> {
    match bifrost_core::read_system_proxy_shutdown_mode(data_dir) {
        Some(bifrost_core::SystemProxyShutdownMode::BackgroundCleanup) => {
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(data_dir);
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                "system proxy lifecycle helper running stop-requested background cleanup"
            );
            cleanup_system_proxy_state(data_dir)
        }
        Some(bifrost_core::SystemProxyShutdownMode::ForegroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                "system proxy lifecycle helper exiting because stop cleaned proxy before parent exit"
            );
            Ok(())
        }
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                "system proxy lifecycle helper skipping cleanup for restart"
            );
            Ok(())
        }
        None => cleanup_or_restart_managed_runtime(data_dir),
    }
}

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

    if pid_reuse_detected(parent_pid, parent_started_at_ms) {
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

fn pid_reuse_detected(parent_pid: Option<u32>, recorded_started_at_ms: Option<u64>) -> bool {
    let Some(pid) = parent_pid else {
        return false;
    };
    let observed = bifrost_core::get_process_start_time_ms(pid);
    matches!(
        bifrost_core::start_times_match(recorded_started_at_ms, observed),
        bifrost_core::StartTimeMatch::Mismatch { .. }
    )
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
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        parent_pid = parent_pid.unwrap_or_default(),
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        poll_secs = poll_interval.as_secs(),
        required_parent_misses,
        "system proxy lifecycle helper started"
    );

    if pid_reuse_detected(parent_pid, parent_started_at_ms) {
        tracing::warn!(
            target: "bifrost_cli::shutdown",
            parent_pid = parent_pid.unwrap_or_default(),
            "system proxy lifecycle helper detected pid_reuse_check=mismatch at startup; running guarded cleanup"
        );
        return cleanup_after_parent_exit(&data_dir);
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
            let mut power_poll = tokio::time::interval(std::time::Duration::from_millis(250));
            power_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGTERM");
                        return cleanup_after_parent_exit(&data_dir);
                    },
                    _ = sigint.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGINT");
                        return cleanup_after_parent_exit(&data_dir);
                    },
                    _ = sighup.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGHUP");
                        return cleanup_after_parent_exit(&data_dir);
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
                        if pid_reuse_detected(parent_pid, parent_started_at_ms) {
                            tracing::warn!(
                                target: "bifrost_cli::shutdown",
                                parent_pid = parent_pid.unwrap_or_default(),
                                "system proxy lifecycle helper detected pid_reuse_check=mismatch during poll; running guarded cleanup"
                            );
                            return cleanup_after_parent_exit(&data_dir);
                        }
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
                                    return cleanup_after_parent_exit(&data_dir);
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
        loop {
            std::thread::sleep(poll_interval);
            if pid_reuse_detected(parent_pid, parent_started_at_ms) {
                tracing::warn!(
                    target: "bifrost_cli::shutdown",
                    parent_pid = parent_pid.unwrap_or_default(),
                    "system proxy lifecycle helper detected pid_reuse_check=mismatch during poll; running guarded cleanup"
                );
                return cleanup_after_parent_exit(&data_dir);
            }
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
                        return cleanup_after_parent_exit(&data_dir);
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

    #[test]
    fn pid_reuse_detected_when_start_time_mismatches_current_process() {
        let recorded = bifrost_core::current_process_start_time_ms()
            .map(|started_at_ms| started_at_ms.saturating_add(10_000));

        assert!(pid_reuse_detected(Some(std::process::id()), recorded));
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
}
