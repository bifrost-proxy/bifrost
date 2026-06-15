use std::io::IsTerminal;

use bifrost_core::{BifrostError, Result};
use bifrost_storage::DEFAULT_SSH_KEY_SHELL_POLICY_ID;
use chrono::{DateTime, Local};
use dialoguer::{theme::ColorfulTheme, Select};
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
            grant_id_option,
            device,
            level,
            access,
            scope,
            policy,
            stdin,
            interactive,
            file_access,
        } => {
            let grants_payload = client
                .list_remote_invoke_grants()
                .map_err(BifrostError::Config)?;
            let grant_id = resolve_grant_id(
                &grants_payload,
                grant_id_option.as_deref().or(grant_id.as_deref()),
                device.as_deref(),
            )?;
            let level = resolve_level(
                &grants_payload,
                level.as_deref(),
                access.as_deref(),
                scope.as_deref(),
                file_access.as_deref(),
            )?;
            let body = if let Some(level) = level {
                build_level_payload(level)
            } else {
                build_update_payload(
                    access.as_deref(),
                    scope.as_deref(),
                    &policy,
                    stdin,
                    interactive,
                    file_access.as_deref(),
                )?
            };
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionLevel {
    FullTrust,
    CommandsAndFiles,
    FilesOnly,
    ReadOnlyWatch,
}

impl PermissionLevel {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "full" | "full-trust" => Ok(Self::FullTrust),
            "shell" | "commands-files" | "commands-and-files" => Ok(Self::CommandsAndFiles),
            "files" | "files-only" => Ok(Self::FilesOnly),
            "query" | "read-only" | "read-only-watch" => Ok(Self::ReadOnlyWatch),
            other => Err(BifrostError::Config(format!(
                "unsupported permission level '{}'",
                other
            ))),
        }
    }
}

fn build_level_payload(level: PermissionLevel) -> Value {
    match level {
        PermissionLevel::FullTrust => json!({
            "grant_scope": "remote_shell_interactive",
            "file_access": "read_write",
            "policy_binding": {
                "mode": "selected",
                "policy_ids": [DEFAULT_SSH_KEY_SHELL_POLICY_ID],
            },
            "interactive_allowed": true,
            "stdin_allowed": true,
        }),
        PermissionLevel::CommandsAndFiles => json!({
            "grant_scope": "remote_shell_exec",
            "file_access": "read_write",
            "policy_binding": {
                "mode": "selected",
                "policy_ids": [DEFAULT_SSH_KEY_SHELL_POLICY_ID],
            },
            "interactive_allowed": false,
            "stdin_allowed": false,
        }),
        PermissionLevel::FilesOnly => json!({
            "grant_scope": "remote_query",
            "file_access": "read_write",
            "policy_binding": Value::Null,
            "interactive_allowed": Value::Null,
            "stdin_allowed": Value::Null,
        }),
        PermissionLevel::ReadOnlyWatch => json!({
            "grant_scope": "remote_query",
            "file_access": "none",
            "policy_binding": Value::Null,
            "interactive_allowed": Value::Null,
            "stdin_allowed": Value::Null,
        }),
    }
}

fn resolve_level(
    grants_payload: &Value,
    level: Option<&str>,
    access: Option<&str>,
    scope: Option<&str>,
    file_access: Option<&str>,
) -> Result<Option<PermissionLevel>> {
    if let Some(level) = level {
        if access.is_some() || scope.is_some() || file_access.is_some() {
            return Err(BifrostError::Config(
                "use either --level or low-level scope/access flags, not both".to_string(),
            ));
        }
        return PermissionLevel::parse(level).map(Some);
    }

    if access.is_some() || scope.is_some() || file_access.is_some() {
        return Ok(None);
    }

    if !std::io::stdin().is_terminal() {
        return Err(BifrostError::Config(
            "permission level is required in non-interactive mode; pass --level full|shell|files|query"
                .to_string(),
        ));
    }

    let choices = [
        "Full trust - commands, files, stdin, interactive terminals",
        "Run commands & read/write files - no interactive terminals",
        "Files only - read/write files and inspect traffic",
        "Read-only watch - status and traffic only",
    ];
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Select permission level for {} grant(s)",
            grants_payload
                .get("grants")
                .and_then(Value::as_array)
                .map(|grants| grants.len())
                .unwrap_or(0)
        ))
        .items(&choices)
        .default(0)
        .interact()
        .map_err(|error| BifrostError::Config(format!("select permission level: {error}")))?;
    Ok(Some(match selection {
        0 => PermissionLevel::FullTrust,
        1 => PermissionLevel::CommandsAndFiles,
        2 => PermissionLevel::FilesOnly,
        _ => PermissionLevel::ReadOnlyWatch,
    }))
}

fn resolve_grant_id(
    payload: &Value,
    explicit: Option<&str>,
    device: Option<&str>,
) -> Result<String> {
    if explicit.is_some() && device.is_some() {
        return Err(BifrostError::Config(
            "use either grant id or --device, not both".to_string(),
        ));
    }
    if let Some(grant_id) = explicit {
        return Ok(grant_id.to_string());
    }

    let grants = payload
        .get("grants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if grants.is_empty() {
        return Err(BifrostError::Config("no active grants found".to_string()));
    }

    if let Some(selector) = device {
        let selector = selector.trim();
        let matches = grants
            .iter()
            .filter(|grant| grant_matches_selector(grant, selector))
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [grant] => grant_id_from_value(grant),
            [] => Err(BifrostError::Config(format!(
                "no grant matched device selector '{}'",
                selector
            ))),
            _ => Err(BifrostError::Config(format!(
                "device selector '{}' matched multiple grants; use --grant-id",
                selector
            ))),
        };
    }

    if !std::io::stdin().is_terminal() {
        return Err(BifrostError::Config(
            "grant id is required in non-interactive mode; pass <grant-id>, --grant-id, or --device"
                .to_string(),
        ));
    }

    let labels = grants.iter().map(format_grant_choice).collect::<Vec<_>>();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select grant/device")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|error| BifrostError::Config(format!("select grant: {error}")))?;
    grant_id_from_value(&grants[selection])
}

fn grant_id_from_value(grant: &Value) -> Result<String> {
    grant
        .get("grant_id")
        .and_then(Value::as_str)
        .or_else(|| grant.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| BifrostError::Config("grant entry is missing grant_id".to_string()))
}

fn grant_matches_selector(grant: &Value, selector: &str) -> bool {
    [
        "grant_id",
        "id",
        "caller_display_name",
        "caller_fingerprint",
        "label",
        "device_name",
        "ssh_key_fingerprint",
    ]
    .iter()
    .filter_map(|key| grant.get(*key).and_then(Value::as_str))
    .any(|value| value.starts_with(selector) || value.contains(selector))
}

fn format_grant_choice(grant: &Value) -> String {
    let grant_id = grant
        .get("grant_id")
        .and_then(Value::as_str)
        .or_else(|| grant.get("id").and_then(Value::as_str))
        .unwrap_or("-");
    let caller = grant
        .get("caller_display_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| grant.get("label").and_then(Value::as_str))
        .or_else(|| grant.get("caller_fingerprint").and_then(Value::as_str))
        .unwrap_or("-");
    let scope = grant
        .get("grant_scope")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let file_access = grant
        .get("file_access")
        .and_then(Value::as_str)
        .unwrap_or("-");
    format!("{grant_id} | {caller} | scope={scope} | file={file_access}")
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
        let file_access = grant
            .get("file_access")
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
        let first_connected = grant
            .get("first_connected_at")
            .or_else(|| grant.get("first_authorized_at"))
            .and_then(Value::as_u64)
            .map(format_millis)
            .unwrap_or_else(|| "-".to_string());
        let last_command = grant
            .get("last_command_at")
            .and_then(Value::as_u64)
            .map(format_millis)
            .unwrap_or_else(|| "-".to_string());
        println!("  - {} {}", grant_id, caller);
        println!(
            "    scope: {} | file: {} | mode: {}",
            scope, file_access, mode
        );
        println!(
            "    first connected: {} | last command: {}",
            first_connected, last_command
        );
        println!("    binding: {}", binding);
        println!("    stdin: {} | interactive: {}", stdin, interactive);
    }
}

fn format_millis(value: u64) -> String {
    DateTime::from_timestamp_millis(value as i64)
        .map(|date_time| {
            date_time
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| value.to_string())
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

    #[test]
    fn build_level_payload_full_trust_uses_interactive_shell_and_default_policy() {
        let payload = build_level_payload(PermissionLevel::FullTrust);

        assert_eq!(payload["grant_scope"], "remote_shell_interactive");
        assert_eq!(payload["file_access"], "read_write");
        assert_eq!(payload["interactive_allowed"], true);
        assert_eq!(payload["stdin_allowed"], true);
        assert_eq!(payload["policy_binding"]["mode"], "selected");
        assert_eq!(
            payload["policy_binding"]["policy_ids"][0],
            DEFAULT_SSH_KEY_SHELL_POLICY_ID
        );
    }

    #[test]
    fn build_level_payload_files_only_keeps_shell_query_but_file_write() {
        let payload = build_level_payload(PermissionLevel::FilesOnly);

        assert_eq!(payload["grant_scope"], "remote_query");
        assert_eq!(payload["file_access"], "read_write");
        assert!(payload["policy_binding"].is_null());
    }

    #[test]
    fn permission_level_aliases_parse() {
        assert_eq!(
            PermissionLevel::parse("full-trust").unwrap(),
            PermissionLevel::FullTrust
        );
        assert_eq!(
            PermissionLevel::parse("commands-and-files").unwrap(),
            PermissionLevel::CommandsAndFiles
        );
        assert_eq!(
            PermissionLevel::parse("read-only-watch").unwrap(),
            PermissionLevel::ReadOnlyWatch
        );
    }
}
