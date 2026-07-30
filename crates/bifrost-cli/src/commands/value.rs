use std::collections::HashMap;
use std::path::Path;

use bifrost_storage::ValuesStorage;

use crate::cli::ValueCommands;
use crate::commands::config::client::ConfigApiClient;

fn parse_values_file(path: &Path) -> bifrost_core::Result<(HashMap<String, String>, usize)> {
    let content = std::fs::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let mut values = HashMap::new();
    let mut count = 0;

    match extension {
        "json" => {
            values = serde_json::from_str(&content)
                .map_err(|e| bifrost_core::BifrostError::Parse(format!("Invalid JSON: {}", e)))?;
            count = values.len();
        }
        "kv" | "env" => {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(eq_pos) = line.find('=') else {
                    continue;
                };
                let key = line[..eq_pos].trim();
                if key.is_empty() {
                    continue;
                }
                let value = line[eq_pos + 1..].trim();
                values.insert(key.to_string(), value.to_string());
                count += 1;
            }
        }
        _ => {
            if let Some(name) = path.file_stem().and_then(|value| value.to_str()) {
                values.insert(name.to_string(), content.trim().to_string());
                count = 1;
            }
        }
    }

    Ok((values, count))
}

fn handle_online_value_command(
    action: ValueCommands,
    client: &ConfigApiClient,
) -> bifrost_core::Result<()> {
    match action {
        ValueCommands::Add { name, value } => {
            client
                .upsert_values(&HashMap::from([(name.clone(), value)]))
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Value '{}' added successfully.", name);
        }
        ValueCommands::Update { name, value } => {
            client
                .update_value(&name, &value)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Value '{}' updated successfully.", name);
        }
        ValueCommands::Delete { name } => {
            client
                .delete_value(&name)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Value '{}' deleted successfully.", name);
        }
        ValueCommands::Import { file } => {
            if !file.exists() {
                return Err(bifrost_core::BifrostError::NotFound(format!(
                    "File not found: {}",
                    file.display()
                )));
            }
            let (values, count) = parse_values_file(&file)?;
            client
                .upsert_values(&values)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Imported {} value(s) from '{}'.", count, file.display());
        }
        ValueCommands::List | ValueCommands::Show { .. } => {
            return Err(bifrost_core::BifrostError::Config(
                "internal error: read-only value command routed as an online mutation".to_string(),
            ));
        }
    }
    Ok(())
}

fn routes_value_mutation_to_api(action: &ValueCommands) -> bool {
    matches!(
        action,
        ValueCommands::Add { .. }
            | ValueCommands::Update { .. }
            | ValueCommands::Delete { .. }
            | ValueCommands::Import { .. }
    )
}

fn route_online_value_command(
    action: ValueCommands,
    client: Option<&ConfigApiClient>,
) -> bifrost_core::Result<Option<ValueCommands>> {
    if routes_value_mutation_to_api(&action) {
        if let Some(client) = client {
            handle_online_value_command(action, client)?;
            return Ok(None);
        }
    }
    Ok(Some(action))
}

pub fn handle_value_command(action: ValueCommands) -> bifrost_core::Result<()> {
    handle_value_command_with_runtime(action, super::config::runtime::live_config_api_client)
}

fn handle_value_command_with_runtime(
    action: ValueCommands,
    resolve_live_client: impl FnOnce() -> bifrost_core::Result<Option<ConfigApiClient>>,
) -> bifrost_core::Result<()> {
    let client = routes_value_mutation_to_api(&action)
        .then(resolve_live_client)
        .transpose()?
        .flatten();
    let Some(action) = route_online_value_command(action, client.as_ref())? else {
        return Ok(());
    };

    let values_dir = bifrost_storage::data_dir().join("values");
    let mut storage = ValuesStorage::with_dir(values_dir.clone())?;

    match action {
        ValueCommands::List => {
            let entries = storage.list_entries()?;
            if entries.is_empty() {
                println!("No values defined.");
                println!();
                println!("Values directory: {}", values_dir.display());
            } else {
                println!("Values ({}):", entries.len());
                println!("====================");
                for entry in entries {
                    let preview = entry.value.replace('\n', "\\n");
                    println!("  {} = {}", entry.name, preview);
                }
                println!();
                println!("Values directory: {}", values_dir.display());
            }
        }
        ValueCommands::Show { name } => {
            if let Some(value) = storage.get_value(&name) {
                println!("{}", value);
            } else {
                return Err(bifrost_core::BifrostError::NotFound(format!(
                    "Value '{}' not found",
                    name
                )));
            }
        }
        ValueCommands::Add { name, value } => {
            storage.set_value(&name, &value)?;
            println!("Value '{}' added successfully.", name);
        }
        ValueCommands::Update { name, value } => {
            storage.update(&name, &value)?;
            println!("Value '{}' updated successfully.", name);
        }
        ValueCommands::Delete { name } => {
            if storage.exists(&name) {
                storage.remove_value(&name)?;
                println!("Value '{}' deleted successfully.", name);
            } else {
                return Err(bifrost_core::BifrostError::NotFound(format!(
                    "Value '{}' not found",
                    name
                )));
            }
        }
        ValueCommands::Import { file } => {
            if !file.exists() {
                return Err(bifrost_core::BifrostError::NotFound(format!(
                    "File not found: {}",
                    file.display()
                )));
            }
            let count = storage.load_from_file(&file)?;
            println!("Imported {} value(s) from '{}'.", count, file.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> ConfigApiClient {
        ConfigApiClient::new("127.0.0.1", server.address().port())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn online_value_commands_use_admin_api_for_all_mutations() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/values"))
            .and(body_json(serde_json::json!({"values": {"CLI_ADD": "one"}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_value_command(
            ValueCommands::Add {
                name: "CLI_ADD".to_string(),
                value: "one".to_string(),
            },
            &client,
        )
        .unwrap();

        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/values/CLI_UPDATE"))
            .and(body_json(serde_json::json!({"value": "two"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_value_command(
            ValueCommands::Update {
                name: "CLI_UPDATE".to_string(),
                value: "two".to_string(),
            },
            &client,
        )
        .unwrap();

        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/values/CLI_DELETE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_value_command(
            ValueCommands::Delete {
                name: "CLI_DELETE".to_string(),
            },
            &client,
        )
        .unwrap();

        let dir = tempdir().unwrap();
        let import_path = dir.path().join("import.json");
        std::fs::write(&import_path, r#"{"CLI_IMPORT":"three"}"#).unwrap();
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/values"))
            .and(body_json(
                serde_json::json!({"values": {"CLI_IMPORT": "three"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_value_command(ValueCommands::Import { file: import_path }, &client).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn online_value_command_rejects_missing_import_and_read_only_routing() {
        let server = MockServer::start().await;
        let client = client_for(&server);
        let missing = tempdir().unwrap().path().join("missing.json");

        let missing_error =
            handle_online_value_command(ValueCommands::Import { file: missing }, &client)
                .unwrap_err();
        assert!(missing_error.to_string().contains("File not found"));

        let routing_error = handle_online_value_command(ValueCommands::List, &client).unwrap_err();
        assert!(routing_error
            .to_string()
            .contains("read-only value command"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn value_routing_uses_api_only_for_mutations() {
        let server = MockServer::start().await;
        let client = client_for(&server);
        assert!(!routes_value_mutation_to_api(&ValueCommands::List));
        assert!(
            route_online_value_command(ValueCommands::List, Some(&client))
                .unwrap()
                .is_some()
        );

        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/values/ROUTED"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        assert!(route_online_value_command(
            ValueCommands::Delete {
                name: "ROUTED".to_string(),
            },
            Some(&client),
        )
        .unwrap()
        .is_none());

        assert!(route_online_value_command(
            ValueCommands::Update {
                name: "OFFLINE".to_string(),
                value: "value".to_string(),
            },
            None,
        )
        .unwrap()
        .is_some());

        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/values/WRAPPER"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_value_command_with_runtime(
            ValueCommands::Delete {
                name: "WRAPPER".to_string(),
            },
            || Ok(Some(client_for(&server))),
        )
        .unwrap();
    }

    #[test]
    fn parse_values_file_supports_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("values.json");
        std::fs::write(&path, r#"{"A":"1","B":"two"}"#).unwrap();

        let (values, count) = parse_values_file(&path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(values.get("A").map(String::as_str), Some("1"));
        assert_eq!(values.get("B").map(String::as_str), Some("two"));
    }

    #[test]
    fn parse_values_file_supports_env_and_preserves_last_duplicate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("values.env");
        std::fs::write(
            &path,
            "# comment\nA=first\ninvalid\n\n=ignored\nB = two\nA=last\n",
        )
        .unwrap();

        let (values, count) = parse_values_file(&path).unwrap();
        assert_eq!(count, 3);
        assert_eq!(values.len(), 2);
        assert_eq!(values.get("A").map(String::as_str), Some("last"));
        assert_eq!(values.get("B").map(String::as_str), Some("two"));
    }

    #[test]
    fn parse_values_file_uses_stem_for_plain_text() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("token.txt");
        std::fs::write(&path, " secret \n").unwrap();

        let (values, count) = parse_values_file(&path).unwrap();
        assert_eq!(count, 1);
        assert_eq!(values.get("token").map(String::as_str), Some("secret"));
    }

    #[test]
    fn parse_values_file_rejects_invalid_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("values.json");
        std::fs::write(&path, "{not-json}").unwrap();

        let error = parse_values_file(&path).unwrap_err();
        assert!(error.to_string().contains("Invalid JSON"));
    }
}
