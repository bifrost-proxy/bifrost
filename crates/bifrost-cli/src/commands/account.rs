use std::io::Read;

use bifrost_core::{BifrostError, Result};

use crate::cli::AccountCommands;
use crate::commands::config::client::{
    ConfigApiClient, UpdateUserPassAccountRequest, UpdateUserPassRequest, UserPassAccountResponse,
    UserPassResponse,
};

pub fn handle_account_command(action: AccountCommands, host: &str, port: u16) -> Result<()> {
    let client = ConfigApiClient::new(host, port);
    match action {
        AccountCommands::List { json } => list_accounts(&client, json),
        AccountCommands::Add {
            username,
            password,
            password_stdin,
            disabled,
            enable_auth,
        } => {
            let password = read_password_arg(password, password_stdin, true)?;
            let current = current_userpass(&client)?;
            let next = add_account(current, username, password, !disabled, enable_auth)?;
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Proxy account added.");
            Ok(())
        }
        AccountCommands::Update {
            username,
            password,
            password_stdin,
            enable,
            disable,
        } => {
            let password = read_password_arg(password, password_stdin, false)?;
            let current = current_userpass(&client)?;
            let next = update_account(current, username, password, enable, disable)?;
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Proxy account updated.");
            Ok(())
        }
        AccountCommands::Remove { username } => {
            let current = current_userpass(&client)?;
            let next = remove_account(current, &username)?;
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Proxy account removed.");
            Ok(())
        }
        AccountCommands::Enable => {
            let current = current_userpass(&client)?;
            let next = set_userpass_enabled(current, true);
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Proxy account auth: enabled");
            Ok(())
        }
        AccountCommands::Disable => {
            let current = current_userpass(&client)?;
            let next = set_userpass_enabled(current, false);
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Proxy account auth: disabled");
            Ok(())
        }
        AccountCommands::SetLoopbackAuth { required } => {
            let required = parse_bool(&required)?;
            let current = current_userpass(&client)?;
            let next = set_loopback_requires_auth(current, required);
            client.set_userpass(&next).map_err(BifrostError::Config)?;
            println!("Loopback proxy account auth required: {required}");
            Ok(())
        }
    }
}

fn list_accounts(client: &ConfigApiClient, json: bool) -> Result<()> {
    let userpass = current_userpass_response(client)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&userpass)
                .map_err(|e| BifrostError::Config(e.to_string()))?
        );
        return Ok(());
    }

    println!(
        "Proxy account auth: {}",
        if userpass.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "Loopback requires auth: {}",
        userpass.loopback_requires_auth
    );
    if userpass.accounts.is_empty() {
        println!("Accounts: []");
        return Ok(());
    }
    println!("Accounts:");
    for account in userpass.accounts {
        println!(
            "  - {} (enabled: {}, has_password: {}, last_connected_at: {})",
            account.username,
            account.enabled,
            account.has_password,
            account.last_connected_at.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn current_userpass_response(client: &ConfigApiClient) -> Result<UserPassResponse> {
    Ok(client
        .get_whitelist()
        .map_err(BifrostError::Config)?
        .userpass)
}

fn current_userpass(client: &ConfigApiClient) -> Result<UserPassResponse> {
    current_userpass_response(client)
}

fn read_password_arg(
    password: Option<String>,
    password_stdin: bool,
    required: bool,
) -> Result<Option<String>> {
    match (password, password_stdin) {
        (Some(_), true) => Err(BifrostError::Config(
            "Use either --password or --password-stdin, not both".to_string(),
        )),
        (Some(value), false) => validate_password(Some(value), required),
        (None, true) => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| BifrostError::Config(format!("Failed to read stdin: {e}")))?;
            validate_password(
                Some(buf.trim_end_matches(['\n', '\r']).to_string()),
                required,
            )
        }
        (None, false) => validate_password(None, required),
    }
}

fn validate_password(value: Option<String>, required: bool) -> Result<Option<String>> {
    if let Some(password) = value {
        if password.is_empty() {
            return Err(BifrostError::Config("Password cannot be empty".to_string()));
        }
        return Ok(Some(password));
    }
    if required {
        Err(BifrostError::Config(
            "Password is required; pass --password or --password-stdin".to_string(),
        ))
    } else {
        Ok(None)
    }
}

fn add_account(
    current: UserPassResponse,
    username: String,
    password: Option<String>,
    enabled: bool,
    enable_auth: bool,
) -> Result<UpdateUserPassRequest> {
    let username = normalize_username(&username)?;
    if current
        .accounts
        .iter()
        .any(|account| account.username == username)
    {
        return Err(BifrostError::Config(format!(
            "Proxy account '{username}' already exists"
        )));
    }

    let mut accounts = response_accounts_to_update(current.accounts);
    accounts.push(UpdateUserPassAccountRequest {
        username,
        password,
        enabled,
    });

    Ok(UpdateUserPassRequest {
        enabled: current.enabled || enable_auth,
        accounts,
        loopback_requires_auth: current.loopback_requires_auth,
    })
}

fn update_account(
    current: UserPassResponse,
    username: String,
    password: Option<String>,
    enable: bool,
    disable: bool,
) -> Result<UpdateUserPassRequest> {
    let username = normalize_username(&username)?;
    let mut found = false;
    let accounts = current
        .accounts
        .into_iter()
        .map(|account| {
            if account.username != username {
                return response_account_to_update(account);
            }
            found = true;
            UpdateUserPassAccountRequest {
                username: account.username,
                password: password.clone(),
                enabled: if enable {
                    true
                } else if disable {
                    false
                } else {
                    account.enabled
                },
            }
        })
        .collect();

    if !found {
        return Err(BifrostError::Config(format!(
            "Proxy account '{username}' not found"
        )));
    }

    Ok(UpdateUserPassRequest {
        enabled: current.enabled,
        accounts,
        loopback_requires_auth: current.loopback_requires_auth,
    })
}

fn remove_account(current: UserPassResponse, username: &str) -> Result<UpdateUserPassRequest> {
    let username = normalize_username(username)?;
    let mut removed = false;
    let accounts = current
        .accounts
        .into_iter()
        .filter_map(|account| {
            if account.username == username {
                removed = true;
                None
            } else {
                Some(response_account_to_update(account))
            }
        })
        .collect();

    if !removed {
        return Err(BifrostError::Config(format!(
            "Proxy account '{username}' not found"
        )));
    }

    Ok(UpdateUserPassRequest {
        enabled: current.enabled,
        accounts,
        loopback_requires_auth: current.loopback_requires_auth,
    })
}

fn set_userpass_enabled(current: UserPassResponse, enabled: bool) -> UpdateUserPassRequest {
    UpdateUserPassRequest {
        enabled,
        accounts: response_accounts_to_update(current.accounts),
        loopback_requires_auth: current.loopback_requires_auth,
    }
}

fn set_loopback_requires_auth(
    current: UserPassResponse,
    loopback_requires_auth: bool,
) -> UpdateUserPassRequest {
    UpdateUserPassRequest {
        enabled: current.enabled,
        accounts: response_accounts_to_update(current.accounts),
        loopback_requires_auth,
    }
}

fn response_accounts_to_update(
    accounts: Vec<UserPassAccountResponse>,
) -> Vec<UpdateUserPassAccountRequest> {
    accounts
        .into_iter()
        .map(response_account_to_update)
        .collect()
}

fn response_account_to_update(account: UserPassAccountResponse) -> UpdateUserPassAccountRequest {
    UpdateUserPassAccountRequest {
        username: account.username,
        password: None,
        enabled: account.enabled,
    }
}

fn normalize_username(username: &str) -> Result<String> {
    let username = username.trim();
    if username.is_empty() {
        return Err(BifrostError::Config("Username cannot be empty".to_string()));
    }
    Ok(username.to_string())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(BifrostError::Config(format!(
            "Expected true or false, got '{value}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_userpass() -> UserPassResponse {
        UserPassResponse {
            enabled: true,
            loopback_requires_auth: false,
            accounts: vec![UserPassAccountResponse {
                username: "alice".to_string(),
                enabled: true,
                has_password: true,
                last_connected_at: Some("2026-01-01T00:00:00Z".to_string()),
            }],
        }
    }

    #[test]
    fn add_account_appends_without_requiring_json_config() {
        let req = add_account(
            sample_userpass(),
            "bob".to_string(),
            Some("secret".to_string()),
            true,
            false,
        )
        .unwrap();

        assert!(req.enabled);
        assert_eq!(req.accounts.len(), 2);
        assert_eq!(req.accounts[1].username, "bob");
        assert_eq!(req.accounts[1].password.as_deref(), Some("secret"));
        assert!(req.accounts[1].enabled);
    }

    #[test]
    fn update_account_preserves_existing_password_when_password_omitted() {
        let req =
            update_account(sample_userpass(), "alice".to_string(), None, false, true).unwrap();

        assert_eq!(req.accounts.len(), 1);
        assert_eq!(req.accounts[0].username, "alice");
        assert_eq!(req.accounts[0].password, None);
        assert!(!req.accounts[0].enabled);
    }

    #[test]
    fn remove_account_drops_target_and_keeps_flags() {
        let req = remove_account(sample_userpass(), "alice").unwrap();

        assert!(req.enabled);
        assert!(req.accounts.is_empty());
        assert!(!req.loopback_requires_auth);
    }

    #[test]
    fn add_account_rejects_duplicate_username() {
        let err = add_account(
            sample_userpass(),
            "alice".to_string(),
            Some("secret".to_string()),
            true,
            false,
        )
        .unwrap_err();

        assert!(format!("{err}").contains("already exists"));
    }
}
