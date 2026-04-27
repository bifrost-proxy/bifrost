//! `bifrost keepawake ...` and `bifrost remote keepawake ...` CLI commands.

use clap::Subcommand;
use colored::Colorize;
use serde_json::json;

#[derive(Subcommand, Debug, Clone)]
pub enum KeepAwakeAction {
    /// Show current keep-awake status
    Status,
    /// Activate keep-awake (set mode = force_on)
    On,
    /// Deactivate keep-awake (set mode = off)
    Off,
    /// Change the keep-awake mode
    Mode {
        #[command(subcommand)]
        action: KeepAwakeModeAction,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum KeepAwakeModeAction {
    /// Set the keep-awake mode
    Set {
        /// Mode value: `off`, `auto`, or `force_on`
        mode: String,
    },
    /// Show the current keep-awake mode
    Get,
}

/// Handle `bifrost keepawake <action>` locally via admin HTTP API.
pub fn handle_local(action: KeepAwakeAction, port: u16) -> bifrost_core::Result<()> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| bifrost_core::BifrostError::Network(format!("runtime: {e}")))?;
    rt.block_on(async move { run_local(action, port).await })
}

async fn run_local(action: KeepAwakeAction, port: u16) -> bifrost_core::Result<()> {
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}/api/power");
    let resp: reqwest::Response = match action {
        KeepAwakeAction::Status => client
            .get(format!("{base}/status"))
            .send()
            .await
            .map_err(|e| bifrost_core::BifrostError::Network(format!("http: {e}")))?,
        KeepAwakeAction::On => client
            .post(format!("{base}/on"))
            .send()
            .await
            .map_err(|e| bifrost_core::BifrostError::Network(format!("http: {e}")))?,
        KeepAwakeAction::Off => client
            .post(format!("{base}/off"))
            .send()
            .await
            .map_err(|e| bifrost_core::BifrostError::Network(format!("http: {e}")))?,
        KeepAwakeAction::Mode {
            action: KeepAwakeModeAction::Set { mode },
        } => client
            .post(format!("{base}/mode"))
            .json(&json!({ "mode": mode }))
            .send()
            .await
            .map_err(|e| bifrost_core::BifrostError::Network(format!("http: {e}")))?,
        KeepAwakeAction::Mode {
            action: KeepAwakeModeAction::Get,
        } => client
            .get(format!("{base}/status"))
            .send()
            .await
            .map_err(|e| bifrost_core::BifrostError::Network(format!("http: {e}")))?,
    };
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| bifrost_core::BifrostError::Network(format!("body: {e}")))?;
    if !status.is_success() {
        eprintln!("{} {body}", format!("HTTP {status}").red());
        return Err(bifrost_core::BifrostError::Network(format!(
            "keepawake request failed: {status}"
        )));
    }
    println!("{body}");
    Ok(())
}

/// Build the JSON args for a PowerMgmt remote invoke.
pub fn build_remote_args(action: &KeepAwakeAction) -> (String, serde_json::Value) {
    match action {
        KeepAwakeAction::Status => ("status".to_string(), json!({})),
        KeepAwakeAction::On => ("on".to_string(), json!({})),
        KeepAwakeAction::Off => ("off".to_string(), json!({})),
        KeepAwakeAction::Mode {
            action: KeepAwakeModeAction::Set { mode },
        } => ("set_mode".to_string(), json!({ "mode": mode.clone() })),
        KeepAwakeAction::Mode {
            action: KeepAwakeModeAction::Get,
        } => ("status".to_string(), json!({})),
    }
}
