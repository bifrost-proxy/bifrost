use bifrost_storage::{set_data_dir, ConfigManager};

#[cfg(target_os = "macos")]
use crate::cli::SystemProxyLaunchdCommands;
use crate::cli::{Cli, SystemProxyCommands};
use crate::config::get_bifrost_dir;
use crate::process::is_process_running;

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
            poll_secs,
        } => {
            return run_system_proxy_lifecycle_helper(data_dir.clone(), *parent_pid, *poll_secs);
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
                    println!("Supported: true");
                    println!("Enabled:  {}", status.enable);
                    println!("Host:     {}", status.host);
                    println!("Port:     {}", status.port);
                    println!("Bypass:   {}", status.bypass);
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
            match manager.disable_managed() {
                Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
                    println!("✓ System proxy disabled");
                }
                Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => {
                    println!("✓ System proxy already disabled");
                }
                Ok(bifrost_core::SystemProxyDisableOutcome::OwnedByOther) => {
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
                                    match manager.disable_managed_with_privilege() {
                                        Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
                                            println!("✓ System proxy disabled via sudo");
                                        }
                                        Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => {
                                            println!("✓ System proxy already disabled");
                                        }
                                        Ok(
                                            bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
                                        ) => {
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
        SystemProxyCommands::CleanupDaemon { .. } => {
            unreachable!("hidden system-proxy helper commands are handled before config load")
        }
    }
    manager.detach();
    Ok(())
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
    bifrost_core::SystemProxyManager::recover_from_crash(data_dir)?;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        "system proxy cleanup helper restore completed"
    );
    Ok(())
}

fn run_system_proxy_lifecycle_helper(
    data_dir: std::path::PathBuf,
    parent_pid: Option<u32>,
    poll_secs: u64,
) -> bifrost_core::Result<()> {
    set_data_dir(data_dir.clone());
    let poll_interval = std::time::Duration::from_secs(poll_secs.max(1));
    let required_parent_misses = 3_u32;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        parent_pid = parent_pid.unwrap_or_default(),
        poll_secs = poll_interval.as_secs(),
        required_parent_misses,
        "system proxy lifecycle helper started"
    );

    #[cfg(unix)]
    {
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
            loop {
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGTERM");
                        return cleanup_system_proxy_state(&data_dir);
                    }
                    _ = sigint.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGINT");
                        return cleanup_system_proxy_state(&data_dir);
                    }
                    _ = sighup.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGHUP");
                        return cleanup_system_proxy_state(&data_dir);
                    }
                    _ = tokio::time::sleep(poll_interval) => {
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
                                    return cleanup_system_proxy_state(&data_dir);
                                }
                            } else {
                                consecutive_parent_misses = 0;
                            }
                        }
                    }
                }
            }
        })
    }

    #[cfg(not(unix))]
    {
        let mut consecutive_parent_misses = 0_u32;
        loop {
            std::thread::sleep(poll_interval);
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
                        return cleanup_system_proxy_state(&data_dir);
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
    match bifrost_core::system_proxy_launchd::recover_if_no_live_runtime(&data_dir) {
        Ok(true) => tracing::info!(
            target: "bifrost_cli::shutdown",
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery completed"
        ),
        Ok(false) => tracing::info!(
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
