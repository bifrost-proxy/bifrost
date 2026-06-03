use bifrost_storage::{set_data_dir, ConfigManager};

use crate::cli::{Cli, SystemProxyCommands};
use crate::config::get_bifrost_dir;

pub fn handle_system_proxy_command(
    cli: &Cli,
    action: SystemProxyCommands,
) -> bifrost_core::Result<()> {
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
    }
    manager.detach();
    Ok(())
}
