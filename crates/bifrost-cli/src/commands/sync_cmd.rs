use super::config::client::ConfigApiClient;
use crate::cli::SyncCommands;

pub fn handle_sync_command(
    action: SyncCommands,
    host: &str,
    port: u16,
) -> bifrost_core::Result<()> {
    let client = ConfigApiClient::new(host, port);

    match action {
        SyncCommands::Status => show_status(&client),
        SyncCommands::Login { token, url } => sync_login(&client, token, url),
        SyncCommands::Logout => sync_logout(&client),
        SyncCommands::Run => sync_run(&client),
        SyncCommands::Config {
            enabled,
            auto_sync,
            remote_url,
        } => sync_config(&client, enabled, auto_sync, remote_url),
    }
}

fn show_status(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    let status = client
        .get_sync_status()
        .map_err(bifrost_core::BifrostError::Config)?;

    println!("Sync Status");
    println!("============");
    println!("Enabled: {}", status.enabled);
    println!("Auto sync: {}", status.auto_sync);
    println!("Remote URL: {}", status.remote_base_url);
    println!("Has session: {}", status.has_session);
    println!("Reachable: {}", status.reachable);
    println!("Authorized: {}", status.authorized);
    println!("Syncing: {}", status.syncing);
    println!("Reason: {}", status.reason);

    if let Some(ref last_sync) = status.last_sync_at {
        println!("Last sync at: {}", last_sync);
    }
    if let Some(ref action) = status.last_sync_action {
        println!("Last sync action: {}", action);
    }
    if let Some(ref error) = status.last_error {
        println!("Last error: {}", error);
    }

    if let Some(ref user) = status.user {
        println!();
        println!("User:");
        println!("  ID: {}", user.user_id);
        println!("  Nickname: {}", user.nickname);
        println!("  Email: {}", user.email);
    }

    Ok(())
}

fn sync_login(
    client: &ConfigApiClient,
    token: Option<String>,
    url: Option<String>,
) -> bifrost_core::Result<()> {
    match (&token, &url) {
        (Some(token), _) if token.trim().is_empty() => {
            return Err(bifrost_core::BifrostError::Config(
                "--token must not be empty".to_string(),
            ));
        }
        (_, Some(url)) if url.trim().is_empty() => {
            return Err(bifrost_core::BifrostError::Config(
                "--url must not be empty".to_string(),
            ));
        }
        (Some(_), Some(_)) => println!("Saving sync login token..."),
        (Some(_), None) => println!("Saving sync login token with the configured remote URL..."),
        (None, None) => println!("Initiating sync login..."),
        (None, Some(_)) => {
            return Err(bifrost_core::BifrostError::Config(
                "--url requires --token".to_string(),
            ));
        }
    }

    let status = client
        .sync_login(token.as_deref(), url.as_deref())
        .map_err(bifrost_core::BifrostError::Config)?;

    if status.has_session && status.authorized {
        println!("Login successful.");
        if let Some(ref user) = status.user {
            println!("Logged in as: {} ({})", user.nickname, user.email);
        }
    } else if token.is_some() {
        println!("Login token saved.");
        println!("Remote URL: {}", status.remote_base_url);
        println!("Status: {}", status.reason);
    } else {
        println!("Login initiated. Please complete authentication in the browser.");
        println!("Status: {}", status.reason);
    }

    Ok(())
}

fn sync_logout(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    let status = client
        .sync_logout()
        .map_err(bifrost_core::BifrostError::Config)?;

    if !status.has_session {
        println!("Logged out successfully.");
    } else {
        println!("Logout request sent. Status: {}", status.reason);
    }

    Ok(())
}

fn sync_run(client: &ConfigApiClient) -> bifrost_core::Result<()> {
    println!("Triggering sync...");
    let status = client
        .sync_run()
        .map_err(bifrost_core::BifrostError::Config)?;

    if let Some(ref action) = status.last_sync_action {
        println!("Sync completed. Action: {}", action);
    } else {
        println!("Sync triggered. Status: {}", status.reason);
    }

    if let Some(ref error) = status.last_error {
        println!("Warning: {}", error);
    }

    Ok(())
}

fn sync_config(
    client: &ConfigApiClient,
    enabled: Option<bool>,
    auto_sync: Option<bool>,
    remote_url: Option<String>,
) -> bifrost_core::Result<()> {
    use super::config::client::UpdateSyncConfigRequest;

    if enabled.is_none() && auto_sync.is_none() && remote_url.is_none() {
        let status = client
            .get_sync_status()
            .map_err(bifrost_core::BifrostError::Config)?;
        println!("Sync Configuration");
        println!("==================");
        println!("Enabled: {}", status.enabled);
        println!("Auto sync: {}", status.auto_sync);
        println!("Remote URL: {}", status.remote_base_url);
        return Ok(());
    }

    let req = UpdateSyncConfigRequest {
        enabled,
        auto_sync,
        remote_base_url: remote_url,
    };

    let status = client
        .update_sync_config(&req)
        .map_err(bifrost_core::BifrostError::Config)?;

    println!("Sync configuration updated.");
    println!("Enabled: {}", status.enabled);
    println!("Auto sync: {}", status.auto_sync);
    println!("Remote URL: {}", status.remote_base_url);

    Ok(())
}
