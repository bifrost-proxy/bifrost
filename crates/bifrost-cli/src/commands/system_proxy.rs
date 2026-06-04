use bifrost_storage::{set_data_dir, ConfigManager};

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
        SystemProxyCommands::Cleanup { .. } | SystemProxyCommands::LifecycleHelper { .. } => {
            unreachable!("hidden system-proxy cleanup commands are handled before config load")
        }
    }
    manager.detach();
    Ok(())
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
