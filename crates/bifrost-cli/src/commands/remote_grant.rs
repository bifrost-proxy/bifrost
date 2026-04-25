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
            scope,
            policy,
            stdin,
            interactive,
            file_access,
        } => {
            let body = build_update_payload(
                access.as_deref(),
                scope.as_deref(),
                &policy,
                stdin,
                interactive,
                file_access.as_deref(),
            )?;
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
    access: Option<&str>,
    scope: Option<&str>,
    policy_ids: &[String],
    stdin: Option<bool>,
    interactive: Option<bool>,
    file_access: Option<&str>,
) -> Result<Value> {
    if access.is_some() && scope.is_some() {
        return Err(BifrostError::Config(
            "use either --access or --scope, not both".to_string(),
        ));
    }

    let file_access_value = file_access
        .map(|fa| Value::String(fa.to_string()))
        .unwrap_or(Value::Null);

    if let Some(scope) = scope {
        return match scope {
            "remote_query" => Ok(json!({
                "grant_scope": "remote_query",
                "file_access": file_access_value,
                "policy_binding": Value::Null,
                "interactive_allowed": Value::Null,
                "stdin_allowed": Value::Null,
            })),
            "remote_shell_exec" | "remote_shell_interactive" => {
                let interactive_allowed = scope == "remote_shell_interactive";
                Ok(json!({
                    "grant_scope": scope,
                    "file_access": file_access_value,
                    "policy_binding": json!({ "mode": "all" }),
                    "interactive_allowed": Value::Bool(interactive_allowed),
                    "stdin_allowed": stdin.map(Value::Bool).unwrap_or(Value::Bool(false)),
                }))
            }
            _ => Err(BifrostError::Config(format!(
                "unsupported grant scope '{}'",
                scope
            ))),
        };
    }

    // If only --file-access is given without --access or --scope, just update file_access
    if access.is_none() && file_access.is_some() {
        return Ok(json!({
            "file_access": file_access_value,
        }));
    }

    let access = access.ok_or_else(|| {
        BifrostError::Config("either --access, --scope, or --file-access is required".to_string())
    })?;

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
        "file_access": file_access_value,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_update_payload_with_file_access() {
        let payload = build_update_payload(Some("all"), None, &[], None, None, Some("read_write"))
            .expect("payload");

        assert_eq!(payload["grant_scope"], "remote_shell_exec");
        assert_eq!(payload["file_access"], "read_write");
    }

    #[test]
    fn build_update_payload_file_access_only() {
        let payload =
            build_update_payload(None, None, &[], None, None, Some("read")).expect("payload");

        assert_eq!(payload["file_access"], "read");
        assert!(payload.get("grant_scope").is_none());
    }

    #[test]
    fn build_update_payload_scope_with_file_access() {
        let payload = build_update_payload(
            None,
            Some("remote_shell_interactive"),
            &[],
            None,
            None,
            Some("read_write"),
        )
        .expect("payload");

        assert_eq!(payload["grant_scope"], "remote_shell_interactive");
        assert_eq!(payload["file_access"], "read_write");
        assert_eq!(payload["interactive_allowed"], true);
    }

    #[test]
    fn build_update_payload_rejects_access_and_scope_together() {
        let err = build_update_payload(Some("all"), Some("remote_query"), &[], None, None, None)
            .expect_err("conflicting flags should fail");

        assert!(err.to_string().contains("either --access or --scope"));
    }

    #[test]
    fn build_update_payload_requires_some_flag() {
        let err = build_update_payload(None, None, &[], None, None, None)
            .expect_err("no flags should fail");

        assert!(err.to_string().contains("--file-access"));
    }
}
