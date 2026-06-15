use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use bifrost_admin::remote_invoke::file_policy_store::{
    full_file_ops, full_trust_file_roots, upsert_ssh_fingerprint_grant,
};
use bifrost_admin::remote_invoke::ssh_keys::{SshKeyMaterial, SshKeyRecord, SshKeyStore};
use bifrost_admin::remote_invoke::types::GrantMode;
use bifrost_core::{BifrostError, Result};
use bifrost_storage::ensure_default_ssh_key_shell_policy;
use serde_json::{json, Value};

use crate::cli::RemoteSshKeyCommands;
use crate::commands::config::client::ConfigApiClient;

pub fn handle_remote_ssh_key_command(
    action: RemoteSshKeyCommands,
    host: &str,
    port: u16,
) -> Result<()> {
    let client = ConfigApiClient::new(host, port);
    match action {
        RemoteSshKeyCommands::Create {
            label,
            grant_mode,
            output,
            force,
            json: print_json,
            offline,
        } => {
            let grant_mode = parse_grant_mode(&grant_mode)?;
            let material = if offline {
                create_local_ssh_key(&label, grant_mode)?
            } else {
                create_via_api_or_local(&client, &label, grant_mode)?
            };
            emit_key_material(&material, output.as_deref(), force, print_json, "created")
        }
        RemoteSshKeyCommands::Export {
            output,
            force,
            json: print_json,
            offline,
        } => {
            let material = if offline {
                export_local_ssh_key()?
            } else {
                export_via_api_or_local(&client)?
            };
            let Some(material) = material else {
                return Err(BifrostError::Config(
                    "no active remote-invoke SSH key; run `bifrost setting ssh-key create` first"
                        .to_string(),
                ));
            };
            emit_key_material(&material, output.as_deref(), force, print_json, "exported")
        }
        RemoteSshKeyCommands::Status {
            json: print_json,
            offline,
        } => {
            let record = if offline {
                get_local_ssh_key()?
            } else {
                get_via_api_or_local(&client)?
            };
            emit_key_record(record.as_ref(), print_json)
        }
        RemoteSshKeyCommands::Revoke {
            json: print_json,
            offline,
        } => {
            let payload = if offline {
                let record = SshKeyStore::new(&bifrost_storage::data_dir()).revoke_active_key()?;
                match record {
                    Some(record) => json!({ "success": true, "record": record }),
                    None => json!({ "success": true, "record": null }),
                }
            } else {
                revoke_via_api_or_local(&client)?
            };
            if print_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .map_err(|error| BifrostError::Config(error.to_string()))?
                );
            } else {
                println!("Remote-invoke SSH key revoked.");
            }
            Ok(())
        }
    }
}

fn create_via_api_or_local(
    client: &ConfigApiClient,
    label: &str,
    grant_mode: GrantMode,
) -> Result<SshKeyMaterial> {
    let body = json!({
        "label": label,
        "grant_mode": grant_mode,
    });
    match client.create_remote_invoke_ssh_key(&body) {
        Ok(payload) => material_from_value(payload),
        Err(error) if is_admin_api_unavailable(&error) => create_local_ssh_key(label, grant_mode),
        Err(error) => Err(BifrostError::Config(error)),
    }
}

fn export_via_api_or_local(client: &ConfigApiClient) -> Result<Option<SshKeyMaterial>> {
    match client.export_remote_invoke_ssh_key() {
        Ok(Value::Null) => Ok(None),
        Ok(payload) => material_from_value(payload).map(Some),
        Err(error) if is_admin_api_unavailable(&error) => export_local_ssh_key(),
        Err(error) => Err(BifrostError::Config(error)),
    }
}

fn get_via_api_or_local(client: &ConfigApiClient) -> Result<Option<SshKeyRecord>> {
    match client.get_remote_invoke_ssh_key() {
        Ok(Value::Null) => Ok(None),
        Ok(payload) => record_from_value(payload).map(Some),
        Err(error) if is_admin_api_unavailable(&error) => get_local_ssh_key(),
        Err(error) => Err(BifrostError::Config(error)),
    }
}

fn revoke_via_api_or_local(client: &ConfigApiClient) -> Result<Value> {
    match client.revoke_remote_invoke_ssh_key() {
        Ok(payload) => Ok(payload),
        Err(error) if is_admin_api_unavailable(&error) => {
            let record = SshKeyStore::new(&bifrost_storage::data_dir()).revoke_active_key()?;
            Ok(json!({ "success": true, "record": record }))
        }
        Err(error) => Err(BifrostError::Config(error)),
    }
}

fn create_local_ssh_key(label: &str, grant_mode: GrantMode) -> Result<SshKeyMaterial> {
    let material = SshKeyStore::new(&bifrost_storage::data_dir())
        .create_or_replace_key(label.to_string(), grant_mode)?;
    seed_default_file_access_policy(&material.record)?;
    ensure_default_ssh_key_shell_policy()?;
    Ok(material)
}

fn export_local_ssh_key() -> Result<Option<SshKeyMaterial>> {
    SshKeyStore::new(&bifrost_storage::data_dir()).export_active_key_material()
}

fn get_local_ssh_key() -> Result<Option<SshKeyRecord>> {
    Ok(SshKeyStore::new(&bifrost_storage::data_dir())
        .get_active_key()?
        .map(|key| key.record))
}

fn seed_default_file_access_policy(record: &SshKeyRecord) -> Result<()> {
    upsert_ssh_fingerprint_grant(
        &record.ssh_key_fingerprint,
        Some(format!("ssh-key:{}", record.label)),
        full_trust_file_roots(),
        full_file_ops(),
        None,
        None,
    )
    .map_err(BifrostError::Config)
}

fn emit_key_material(
    material: &SshKeyMaterial,
    output: Option<&str>,
    force: bool,
    print_json: bool,
    verb: &str,
) -> Result<()> {
    if print_json {
        let payload = json!({
            "record": material.record,
            "bifrost_key_file": material.bifrost_key_file,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload)
                .map_err(|error| BifrostError::Config(error.to_string()))?
        );
        return Ok(());
    }

    if let Some(output) = output {
        write_secret(output, &material.bifrost_key_file, force)?;
        println!(
            "Remote-invoke SSH key {}: {}",
            verb, material.record.ssh_key_fingerprint
        );
        println!("Device code: {}", material.record.device_code);
        println!("Key file: {}", output);
        println!("Use on caller: bifrost remote conn up --ssh-key {}", output);
    } else {
        print!("{}", material.bifrost_key_file);
        if !material.bifrost_key_file.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

fn emit_key_record(record: Option<&SshKeyRecord>, print_json: bool) -> Result<()> {
    if print_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&record)
                .map_err(|error| BifrostError::Config(error.to_string()))?
        );
        return Ok(());
    }

    let Some(record) = record else {
        println!("No active remote-invoke SSH key.");
        return Ok(());
    };

    println!("Remote-invoke SSH key");
    println!("  ID: {}", record.id);
    println!("  Label: {}", record.label);
    println!("  Device code: {}", record.device_code);
    println!("  Fingerprint: {}", record.ssh_key_fingerprint);
    println!("  Grant mode: {}", format_grant_mode(record.grant_mode));
    println!("  Status: {:?}", record.status);
    Ok(())
}

fn write_secret(output: &str, content: &str, force: bool) -> Result<()> {
    if output == "-" {
        print!("{}", content);
        if !content.ends_with('\n') {
            println!();
        }
        return Ok(());
    }

    let path = Path::new(output);
    if path.exists() && !force {
        return Err(BifrostError::Config(format!(
            "{} already exists; pass --force to overwrite",
            path.display()
        )));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(BifrostError::Io)?;
    }

    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if force {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path).map_err(BifrostError::Io)?;
    file.write_all(content.as_bytes())
        .map_err(BifrostError::Io)?;
    file.flush().map_err(BifrostError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(BifrostError::Io)?;
    }
    Ok(())
}

fn material_from_value(value: Value) -> Result<SshKeyMaterial> {
    serde_json::from_value(value)
        .map_err(|error| BifrostError::Config(format!("parse SSH key material: {error}")))
}

fn record_from_value(value: Value) -> Result<SshKeyRecord> {
    serde_json::from_value(value)
        .map_err(|error| BifrostError::Config(format!("parse SSH key record: {error}")))
}

fn is_admin_api_unavailable(error: &str) -> bool {
    error.contains("Failed to connect to Bifrost admin API")
}

fn parse_grant_mode(value: &str) -> Result<GrantMode> {
    match value {
        "once" => Ok(GrantMode::Once),
        "30m" => Ok(GrantMode::ThirtyMinutes),
        "1h" => Ok(GrantMode::OneHour),
        "1d" => Ok(GrantMode::OneDay),
        "permanent" => Ok(GrantMode::Permanent),
        other => Err(BifrostError::Config(format!(
            "unsupported grant mode '{}'",
            other
        ))),
    }
}

fn format_grant_mode(value: GrantMode) -> &'static str {
    match value {
        GrantMode::Once => "once",
        GrantMode::ThirtyMinutes => "30m",
        GrantMode::OneHour => "1h",
        GrantMode::OneDay => "1d",
        GrantMode::Permanent => "permanent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_grant_mode_accepts_cli_values() {
        assert_eq!(parse_grant_mode("once").unwrap(), GrantMode::Once);
        assert_eq!(parse_grant_mode("30m").unwrap(), GrantMode::ThirtyMinutes);
        assert_eq!(parse_grant_mode("1h").unwrap(), GrantMode::OneHour);
        assert_eq!(parse_grant_mode("1d").unwrap(), GrantMode::OneDay);
        assert_eq!(parse_grant_mode("permanent").unwrap(), GrantMode::Permanent);
        assert!(parse_grant_mode("forever").is_err());
    }

    #[test]
    fn write_secret_refuses_existing_file_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.key");
        std::fs::write(&path, "old").unwrap();

        let err = write_secret(path.to_str().unwrap(), "new", false).unwrap_err();
        assert!(err.to_string().contains("--force"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn write_secret_sets_private_permissions_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.key");

        write_secret(path.to_str().unwrap(), "secret", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn admin_api_unavailable_detection_is_specific() {
        assert!(is_admin_api_unavailable(
            "Failed to connect to Bifrost admin API at http://127.0.0.1"
        ));
        assert!(!is_admin_api_unavailable("Failed to create ssh key"));
    }
}
