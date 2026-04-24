use bifrost_core::{BifrostError, Result};
use serde_json::{json, Value};

use crate::cli::RemoteGrantCommands;
use crate::commands::config::client::ConfigApiClient;

pub fn handle_remote_grant_command(
    action: RemoteGrantCommands,
    host: &str,
    port: u16,
) -> Result<()> {
    let client = ConfigApiClient::new(host, port);
    match action {
        RemoteGrantCommands::List { json: print_json } => {
            let payload = client
                .list_remote_invoke_grants()
                .map_err(BifrostError::Config)?;
            if print_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .map_err(|error| BifrostError::Config(error.to_string()))?
                );
            } else {
                print_grant_summary(&payload);
            }
        }
        RemoteGrantCommands::Update {
            grant_id,
            access,
            policy,
            stdin,
            interactive,
        } => {
            let body = build_update_payload(&access, &policy, stdin, interactive)?;
            let payload = client
                .update_remote_invoke_grant(&grant_id, &body)
                .map_err(BifrostError::Config)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&payload)
                    .map_err(|error| BifrostError::Config(error.to_string()))?
            );
        }
        RemoteGrantCommands::Revoke { grant_id } => {
            let payload = client
                .revoke_remote_invoke_grant(&grant_id)
                .map_err(BifrostError::Config)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&payload)
                    .map_err(|error| BifrostError::Config(error.to_string()))?
            );
        }
    }

    Ok(())
}

fn build_update_payload(
    access: &str,
    policy_ids: &[String],
    stdin: Option<bool>,
    interactive: Option<bool>,
) -> Result<Value> {
    let (grant_scope, policy_binding) = match access {
        "query" => ("remote_query", Value::Null),
        "all" => ("remote_shell_exec", json!({ "mode": "all" })),
        "selected" => {
            if policy_ids.is_empty() {
                return Err(BifrostError::Config(
                    "--policy is required when --access selected".to_string(),
                ));
            }
            (
                "remote_shell_exec",
                json!({
                    "mode": "selected",
                    "policy_ids": policy_ids,
                }),
            )
        }
        _ => {
            return Err(BifrostError::Config(format!(
                "unsupported access mode '{}'",
                access
            )));
        }
    };

    let interactive_allowed = interactive.unwrap_or(false);
    let effective_scope = if interactive_allowed && grant_scope != "remote_query" {
        "remote_shell_interactive"
    } else {
        grant_scope
    };

    Ok(json!({
        "grant_scope": effective_scope,
        "policy_binding": if effective_scope == "remote_query" { Value::Null } else { policy_binding },
        "interactive_allowed": if effective_scope == "remote_query" { Value::Null } else { Value::Bool(interactive_allowed) },
        "stdin_allowed": if effective_scope == "remote_query" { Value::Null } else { stdin.map(Value::Bool).unwrap_or(Value::Bool(false)) },
    }))
}

fn print_grant_summary(payload: &Value) {
    let grants = payload
        .get("grants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    println!("Remote Grants");
    println!("  Count: {}", grants.len());
    for grant in grants {
        let grant_id = grant.get("grant_id").and_then(Value::as_str).unwrap_or("-");
        let caller = grant
            .get("caller_display_name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                grant
                    .get("caller_fingerprint")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
            });
        let scope = grant
            .get("grant_scope")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let mode = grant
            .get("grant_mode")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let binding = match grant.get("policy_binding") {
            Some(Value::Object(map)) => {
                serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string())
            }
            _ => "-".to_string(),
        };
        let interactive = grant
            .get("interactive_allowed")
            .and_then(Value::as_bool)
            .map(|value| if value { "on" } else { "off" })
            .unwrap_or("-");
        let stdin = grant
            .get("stdin_allowed")
            .and_then(Value::as_bool)
            .map(|value| if value { "on" } else { "off" })
            .unwrap_or("-");
        println!("  - {} {}", grant_id, caller);
        println!("    scope: {} | mode: {}", scope, mode);
        println!("    binding: {}", binding);
        println!("    stdin: {} | interactive: {}", stdin, interactive);
    }
}
