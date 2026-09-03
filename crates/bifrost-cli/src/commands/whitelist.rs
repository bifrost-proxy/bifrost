use bifrost_core::AccessMode;
use bifrost_storage::{set_data_dir, AccessConfigUpdate, ConfigManager};

use super::config::client::ConfigApiClient;
use crate::cli::WhitelistCommands;
use crate::config::get_bifrost_dir;

pub fn handle_whitelist_command(action: WhitelistCommands) -> bifrost_core::Result<()> {
    if super::client::is_active() {
        return handle_client_whitelist_command(action, &ConfigApiClient::new("127.0.0.1", 9900));
    }
    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());

    let config_manager = ConfigManager::new(bifrost_dir)?;
    let config = futures::executor::block_on(config_manager.config());

    match action {
        WhitelistCommands::List => {
            println!("Client IP Whitelist");
            println!("===================");
            if config.access.whitelist.is_empty() {
                println!("No entries in whitelist.");
            } else {
                for entry in &config.access.whitelist {
                    println!("  - {}", entry);
                }
            }
            println!();
            println!(
                "LAN (private network) access: {}",
                if config.access.allow_lan {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        WhitelistCommands::Add { ip_or_cidr } => {
            if ip_or_cidr.contains('/') {
                if ip_or_cidr.parse::<ipnet::IpNet>().is_err() {
                    return Err(bifrost_core::BifrostError::Config(format!(
                        "Invalid CIDR notation: {}",
                        ip_or_cidr
                    )));
                }
            } else if ip_or_cidr.parse::<std::net::IpAddr>().is_err() {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "Invalid IP address: {}",
                    ip_or_cidr
                )));
            }

            if config.access.whitelist.contains(&ip_or_cidr) {
                println!("'{}' is already in the whitelist.", ip_or_cidr);
            } else {
                let mut new_whitelist = config.access.whitelist.clone();
                new_whitelist.push(ip_or_cidr.clone());

                let update = AccessConfigUpdate {
                    whitelist: Some(new_whitelist),
                    ..Default::default()
                };
                futures::executor::block_on(config_manager.update_access_config(update))?;

                println!("Added '{}' to whitelist.", ip_or_cidr);
                println!("Note: Restart the proxy server for changes to take effect.");
            }
        }
        WhitelistCommands::Remove { ip_or_cidr } => {
            if let Some(pos) = config
                .access
                .whitelist
                .iter()
                .position(|x| x == &ip_or_cidr)
            {
                let mut new_whitelist = config.access.whitelist.clone();
                new_whitelist.remove(pos);

                let update = AccessConfigUpdate {
                    whitelist: Some(new_whitelist),
                    ..Default::default()
                };
                futures::executor::block_on(config_manager.update_access_config(update))?;

                println!("Removed '{}' from whitelist.", ip_or_cidr);
                println!("Note: Restart the proxy server for changes to take effect.");
            } else {
                println!("'{}' is not in the whitelist.", ip_or_cidr);
            }
        }
        WhitelistCommands::AllowLan { enable } => {
            let enable_bool = match enable.to_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => {
                    return Err(bifrost_core::BifrostError::Config(format!(
                        "Invalid value '{}'. Use 'true' or 'false'.",
                        enable
                    )));
                }
            };

            let update = AccessConfigUpdate {
                allow_lan: Some(enable_bool),
                ..Default::default()
            };
            futures::executor::block_on(config_manager.update_access_config(update))?;

            if enable_bool {
                println!("LAN (private network) access enabled.");
            } else {
                println!("LAN (private network) access disabled.");
            }
            println!("Note: Restart the proxy server for changes to take effect.");
        }
        WhitelistCommands::Status => {
            println!("Access Control Settings");
            println!("=======================");
            println!("Mode: {}", config.access.mode);
            println!(
                "LAN access: {}",
                if config.access.allow_lan {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!();
            println!("Whitelist entries: {}", config.access.whitelist.len());
            if !config.access.whitelist.is_empty() {
                for entry in &config.access.whitelist {
                    println!("  - {}", entry);
                }
            }
            println!();
            println!("Access mode options:");
            println!(
                "  {} - Only allow connections from localhost",
                AccessMode::LocalOnly
            );
            println!(
                "  {} - Allow localhost + whitelisted IPs/CIDRs",
                AccessMode::Whitelist
            );
            println!(
                "  {} - Prompt for confirmation on unknown IPs (default)",
                AccessMode::Interactive
            );
            println!(
                "  {} - Allow all connections (not recommended)",
                AccessMode::AllowAll
            );
        }
        WhitelistCommands::Mode { mode } => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            match mode {
                Some(m) => {
                    client
                        .set_access_mode(&m)
                        .map_err(bifrost_core::BifrostError::Config)?;
                    println!("Access mode set to: {}", m);
                }
                None => {
                    let result = client
                        .get_access_mode()
                        .map_err(bifrost_core::BifrostError::Config)?;
                    if let Some(mode) = result
                        .get("mode")
                        .and_then(|v: &serde_json::Value| v.as_str())
                    {
                        println!("Current access mode: {}", mode);
                    } else {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        );
                    }
                }
            }
        }
        WhitelistCommands::Pending => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            let pending = client
                .get_pending()
                .map_err(bifrost_core::BifrostError::Config)?;

            if pending.is_empty() {
                println!("No pending access requests.");
            } else {
                println!("Pending Access Requests ({}):", pending.len());
                for item in &pending {
                    if let Some(ip) = item.get("ip").and_then(|v: &serde_json::Value| v.as_str()) {
                        let ts = item
                            .get("timestamp")
                            .or_else(|| item.get("requested_at"))
                            .and_then(|v: &serde_json::Value| v.as_str())
                            .unwrap_or("-");
                        println!("  {} (requested: {})", ip, ts);
                    }
                }
            }
        }
        WhitelistCommands::Approve { ip } => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            client
                .approve_pending(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Approved access for: {}", ip);
        }
        WhitelistCommands::Reject { ip } => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            client
                .reject_pending(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Rejected access for: {}", ip);
        }
        WhitelistCommands::ClearPending => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            client
                .clear_pending()
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("All pending access requests cleared.");
        }
        WhitelistCommands::AddTemporary { ip } => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            client
                .add_temporary(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Temporary access granted for: {}", ip);
        }
        WhitelistCommands::RemoveTemporary { ip } => {
            let port = crate::process::read_runtime_port().unwrap_or(9900);
            let client = ConfigApiClient::new("127.0.0.1", port);

            client
                .remove_temporary(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Temporary access removed for: {}", ip);
        }
    }

    Ok(())
}

fn handle_client_whitelist_command(
    action: WhitelistCommands,
    client: &ConfigApiClient,
) -> bifrost_core::Result<()> {
    match action {
        WhitelistCommands::List | WhitelistCommands::Status => {
            let value = client
                .get_whitelist()
                .map_err(bifrost_core::BifrostError::Config)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
        }
        WhitelistCommands::Add { ip_or_cidr } => {
            let _: serde_json::Value = client
                .post("/whitelist", &serde_json::json!({"ip_or_cidr": ip_or_cidr}))
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Added '{}' to whitelist.", ip_or_cidr);
        }
        WhitelistCommands::Remove { ip_or_cidr } => {
            let _: serde_json::Value = client
                .delete_with_body_public(
                    "/whitelist",
                    &serde_json::json!({"ip_or_cidr": ip_or_cidr}),
                )
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Removed '{}' from whitelist.", ip_or_cidr);
        }
        WhitelistCommands::AllowLan { enable } => {
            let allow = enable.parse::<bool>().map_err(|_| {
                bifrost_core::BifrostError::Config(format!(
                    "Invalid value '{enable}'. Use 'true' or 'false'."
                ))
            })?;
            client
                .set_allow_lan(allow)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!(
                "LAN (private network) access {}.",
                if allow { "enabled" } else { "disabled" }
            );
        }
        WhitelistCommands::Mode { mode } => match mode {
            Some(mode) => {
                client
                    .set_access_mode(&mode)
                    .map_err(bifrost_core::BifrostError::Config)?;
                println!("Access mode set to: {mode}");
            }
            None => {
                let value = client
                    .get_access_mode()
                    .map_err(bifrost_core::BifrostError::Config)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            }
        },
        WhitelistCommands::Pending => {
            let pending = client
                .get_pending()
                .map_err(bifrost_core::BifrostError::Config)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&pending).unwrap_or_default()
            );
        }
        WhitelistCommands::Approve { ip } => {
            client
                .approve_pending(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Approved access for: {ip}");
        }
        WhitelistCommands::Reject { ip } => {
            client
                .reject_pending(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Rejected access for: {ip}");
        }
        WhitelistCommands::ClearPending => {
            client
                .clear_pending()
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("All pending access requests cleared.");
        }
        WhitelistCommands::AddTemporary { ip } => {
            client
                .add_temporary(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Temporary access granted for: {ip}");
        }
        WhitelistCommands::RemoveTemporary { ip } => {
            client
                .remove_temporary(&ip)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Temporary access removed for: {ip}");
        }
    }
    Ok(())
}
