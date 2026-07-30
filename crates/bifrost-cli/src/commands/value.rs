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
                if let Some(eq_pos) = line.find('=') {
                    let key = line[..eq_pos].trim();
                    let value = line[eq_pos + 1..].trim();
                    if !key.is_empty() {
                        values.insert(key.to_string(), value.to_string());
                        count += 1;
                    }
                }
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

pub fn handle_value_command(action: ValueCommands) -> bifrost_core::Result<()> {
    let mutates = matches!(
        &action,
        ValueCommands::Add { .. }
            | ValueCommands::Update { .. }
            | ValueCommands::Delete { .. }
            | ValueCommands::Import { .. }
    );
    if mutates {
        if let Some(client) = super::config::runtime::live_config_api_client()? {
            return handle_online_value_command(action, &client);
        }
    }

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
        std::fs::write(&path, "# comment\nA=first\n\nB = two\nA=last\n").unwrap();

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
