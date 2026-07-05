use colored::Colorize;

use crate::cli::{Cli, EnhancedProxyCommands, StatusFormat};
use crate::config::get_bifrost_dir;
use bifrost_core::{EnhancedProxyManager, EnhancedProxyState};
use bifrost_storage::set_data_dir;

pub fn handle_enhanced_proxy_command(
    cli: &Cli,
    action: EnhancedProxyCommands,
) -> bifrost_core::Result<()> {
    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());
    let manager = EnhancedProxyManager::new(bifrost_dir);

    match action {
        EnhancedProxyCommands::Status { format } => print_status(&manager, format),
        EnhancedProxyCommands::Enable { host, port } => {
            let target_host = host.unwrap_or_else(|| "127.0.0.1".to_string());
            let target_port = port.unwrap_or(cli.port);
            manager.set_enabled(true, &target_host, target_port)?;
            print_status(&manager, StatusFormat::Text)
        }
        EnhancedProxyCommands::Disable => {
            let current = manager.load_desired_state();
            manager.set_enabled(false, &current.proxy_host, current.proxy_port)?;
            print_status(&manager, StatusFormat::Text)
        }
    }
}

fn print_status(manager: &EnhancedProxyManager, format: StatusFormat) -> bifrost_core::Result<()> {
    let status = manager.status();
    match format {
        StatusFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(&status)
                    .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?
            );
        }
        StatusFormat::JsonPretty => {
            println!(
                "{}",
                serde_json::to_string_pretty(&status)
                    .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?
            );
        }
        StatusFormat::Text => {
            println!("{}", "Enhanced Proxy".bright_cyan().bold());
            println!("  Supported:          {}", status.supported);
            println!("  Configured:         {}", status.configured_enabled);
            println!("  Active:             {}", status.enabled);
            println!("  State:              {:?}", status.state);
            println!(
                "  Target:             {}:{}",
                status.proxy_host, status.proxy_port
            );
            println!("  Helper bundle:      {}", status.helper_bundle_id);
            println!("  Extension bundle:   {}", status.extension_bundle_id);
            println!(
                "  Helper app:         {}",
                status
                    .helper_app_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!(
                "  Extension:          {}",
                status
                    .extension_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!(
                "  Controller socket:  {}",
                status.control_socket_path.display()
            );
            println!("  Controller linked:  {}", status.controller_connected);
            println!(
                "  Capture TCP:        {} {:?}",
                status.policy.capture_tcp, status.policy.tcp_ports
            );
            println!(
                "  Capture UDP:        {} {:?}",
                status.policy.capture_udp, status.policy.udp_ports
            );
            if let Some(message) = &status.message {
                println!("  Message:            {}", message);
            }
            if let Some(remediation) = &status.remediation {
                let label = if matches!(status.state, EnhancedProxyState::Running) {
                    "Note"
                } else {
                    "Action"
                };
                println!("  {}:             {}", label, remediation);
            }
        }
    }
    Ok(())
}
