mod admin;
pub mod agent;
mod asr;
mod asr_tui;
mod bifrost_file;
mod ca;
mod caller_stream_frame;
mod capture;
mod completions;
pub(crate) mod config;
mod group;
mod im;
mod install_skill;
pub mod keepawake;
mod metrics;
pub(crate) mod port;
pub mod remote;
mod remote_grant;
mod remote_shell;
mod remote_ssh_key;
mod restart;
mod rule;
mod script;
mod search;
mod start;
mod status;
mod status_tui;
mod stop;
mod sync_cmd;
mod system_proxy;
mod traffic;
pub mod tray;
pub(crate) mod tray_launcher;
mod update_check;
mod upgrade;
mod upgrade_background;
mod value;
mod voice;
mod voice_wake_worker;
mod whitelist;

use colored::Colorize;
use serde_json::Value;
use tracing::debug;

pub use admin::*;
pub use asr::handle_ai_command;
pub use ca::*;
pub use capture::{parse_duration, run_capture_wait, CaptureOutputFormat, CaptureWaitOptions};
pub use config::handle_config_command;
pub use group::handle_group_command;
pub use install_skill::handle_install_skill;
pub use restart::{run_restart, RestartOptions};
pub use rule::*;
pub use script::*;
pub use search::{parse_include_tokens, run_search, OutputFormat, SearchOptions};
pub use start::*;
pub use status::*;
pub use status_tui::*;
pub use stop::*;
pub use system_proxy::*;
pub use traffic::{
    render_traffic_detail_body, render_traffic_list_body, run_traffic_auth_status,
    run_traffic_clear, run_traffic_export, run_traffic_get, run_traffic_list, run_traffic_replay,
    TrafficAuthStatusOptions, TrafficExportOptions, TrafficGetOptions, TrafficListOptions,
    TrafficReplayOptions,
};
pub use update_check::*;
pub use upgrade::*;
pub use upgrade_background::handle_upgrade_background;
pub use value::*;
pub use whitelist::*;

pub use bifrost_file::{handle_export_command, handle_import_command};
pub use im::handle_im_command;
pub use metrics::handle_metrics_command;
pub use port::handle_port_command;
pub use remote_grant::handle_remote_grant_command;
pub use remote_shell::handle_remote_shell_command;
pub use remote_ssh_key::handle_remote_ssh_key_command;
pub use sync_cmd::handle_sync_command;

pub fn handle_version_check(host: &str, port: u16) -> bifrost_core::Result<()> {
    let client = config::client::ConfigApiClient::new(host, port);
    let info = match client.version_check() {
        Ok(info) => info,
        Err(e) => {
            debug!(error = %e, "failed to reach running proxy, falling back to direct check");
            return handle_version_check_standalone();
        }
    };

    print_version_check_output(&info);
    Ok(())
}

fn handle_version_check_standalone() -> bifrost_core::Result<()> {
    println!(
        "{}",
        "Checking for updates directly from GitHub...".dimmed()
    );

    match update_check::get_latest_version_fresh_with_diagnostics() {
        Ok(cache) => {
            let current_version = env!("CARGO_PKG_VERSION");
            let has_update = bifrost_core::version_check::is_newer_version(
                current_version,
                &cache.latest_version,
            );
            let release_url = bifrost_core::version_check::release_page_url(&cache.latest_version);

            let info = serde_json::json!({
                "current_version": current_version,
                "latest_version": cache.latest_version,
                "has_update": has_update,
                "release_highlights": cache.release_highlights,
                "release_url": release_url,
                "checked_at": cache.checked_at.to_rfc3339(),
            });
            print_version_check_output(&info);
            Ok(())
        }
        Err(msg) => {
            let current_version = env!("CARGO_PKG_VERSION");
            println!("Current version: {}", current_version.bright_cyan().bold());
            println!(
                "{}",
                format!("Failed to check for updates: {}", msg).bright_red()
            );
            Ok(())
        }
    }
}

fn print_version_check_output(info: &Value) {
    let current = info.get("current_version").and_then(|v| v.as_str());
    let latest = info.get("latest_version").and_then(|v| v.as_str());
    let has_update = info.get("has_update").and_then(|v| v.as_bool());
    let highlights = info
        .get("release_highlights")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let release_url = info.get("release_url").and_then(|v| v.as_str());

    let separator = "─".repeat(64);

    println!();
    println!("{}", separator.bright_yellow());

    if let Some(current) = current {
        println!("  Current version: {}", current.bright_cyan().bold());
    }

    if let Some(latest) = latest {
        let colored_latest = if has_update == Some(true) {
            latest.bright_green().bold()
        } else {
            latest.bright_cyan().bold()
        };
        println!("  Latest version:  {}", colored_latest);
    }

    println!();

    match (has_update, latest) {
        (Some(true), Some(_)) => {
            println!("  {}", "🚀 Update available!".bright_yellow().bold());

            if !highlights.is_empty() {
                println!();
                println!("  {}", "What's new:".bright_white().bold());
                for highlight in &highlights {
                    println!("    {} {}", "•".bright_cyan(), highlight.bright_white());
                }
            }

            if let Some(url) = release_url {
                println!(
                    "    {} {}",
                    "→".dimmed(),
                    format!("Full release notes: {}", url).dimmed()
                );
            }

            println!();
            println!("  {}", "To upgrade, run:".bright_white());
            println!("    {}", "bifrost upgrade".bright_cyan().bold());
        }
        (Some(false), Some(_)) => {
            println!(
                "  {}",
                "✅ You are running the latest version.".bright_green()
            );
        }
        (_, None) => {
            println!(
                "  {}",
                "⚠ Could not determine the latest version. Check your network connection."
                    .bright_red()
            );
        }
        _ => {}
    }

    println!("{}", separator.bright_yellow());
    println!();
}

#[cfg(test)]
mod tests {
    use super::print_version_check_output;
    use serde_json::json;

    #[test]
    fn version_check_prints_update_available() {
        print_version_check_output(&json!({
            "current_version": "1.0.0",
            "latest_version": "1.1.0",
            "has_update": true,
            "release_highlights": ["New feature A", "Improved performance"],
            "release_url": "https://github.com/bifrost-proxy/bifrost/releases/tag/v1.1.0",
        }));
    }

    #[test]
    fn version_check_prints_latest() {
        print_version_check_output(&json!({
            "current_version": "1.1.0",
            "latest_version": "1.1.0",
            "has_update": false,
            "release_highlights": [],
        }));
    }

    #[test]
    fn version_check_prints_missing_latest() {
        print_version_check_output(&json!({
            "current_version": "1.1.0",
            "latest_version": null,
            "has_update": false,
        }));
    }

    #[test]
    fn version_check_prints_update_no_highlights() {
        print_version_check_output(&json!({
            "current_version": "1.0.0",
            "latest_version": "1.1.0",
            "has_update": true,
            "release_highlights": [],
            "release_url": "https://github.com/bifrost-proxy/bifrost/releases/tag/v1.1.0",
        }));
    }
}
