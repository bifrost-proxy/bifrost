use std::collections::HashMap;
use std::path::Path;

use bifrost_script::{
    RequestData, ResponseData, ScriptContext, ScriptEngine, ScriptEngineConfig,
    ScriptExecutionResult, ScriptLogEntry, ScriptType,
};
use bifrost_storage::{ConfigManager, ValuesStorage};

use crate::cli::ScriptCommands;
use crate::commands::config::client::ConfigApiClient;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptSelection {
    script_type: Option<ScriptType>,
    name: String,
}

fn parse_script_type(s: &str) -> bifrost_core::Result<ScriptType> {
    match s.to_lowercase().as_str() {
        "request" | "req" => Ok(ScriptType::Request),
        "response" | "res" => Ok(ScriptType::Response),
        "decode" | "dec" => Ok(ScriptType::Decode),
        "parser" | "bp" => Ok(ScriptType::Parser),
        _ => Err(bifrost_core::BifrostError::Config(format!(
            "Invalid script type '{}'. Expected: request, response, decode, parser",
            s
        ))),
    }
}

fn parse_lookup_args(args: &[String], command_name: &str) -> bifrost_core::Result<ScriptSelection> {
    match args {
        [name] => Ok(ScriptSelection {
            script_type: None,
            name: name.clone(),
        }),
        [script_type, name] => Ok(ScriptSelection {
            script_type: Some(parse_script_type(script_type)?),
            name: name.clone(),
        }),
        _ => Err(bifrost_core::BifrostError::Config(format!(
            "script {} expects either <name> or <type> <name>",
            command_name
        ))),
    }
}

fn read_script_content(
    content: Option<String>,
    file: Option<std::path::PathBuf>,
) -> bifrost_core::Result<String> {
    if let Some(content) = content {
        Ok(content)
    } else if let Some(path) = file {
        Ok(std::fs::read_to_string(&path)?)
    } else {
        Err(bifrost_core::BifrostError::Config(
            "Either --content or --file must be provided".to_string(),
        ))
    }
}

fn list_all_scripts(
    engine: &ScriptEngine,
    rt: &tokio::runtime::Runtime,
) -> bifrost_core::Result<Vec<(ScriptType, String)>> {
    let mut all_scripts = Vec::new();
    for script_type in [
        ScriptType::Request,
        ScriptType::Response,
        ScriptType::Decode,
        ScriptType::Parser,
    ] {
        let scripts = rt.block_on(engine.list_scripts(script_type)).map_err(|e| {
            bifrost_core::BifrostError::Config(format!(
                "failed to list {} scripts: {e}",
                script_type
            ))
        })?;
        all_scripts.extend(scripts.into_iter().map(|info| (script_type, info.name)));
    }
    Ok(all_scripts)
}

fn find_matching_script(
    engine: &ScriptEngine,
    rt: &tokio::runtime::Runtime,
    name: &str,
) -> bifrost_core::Result<(ScriptType, String)> {
    let all_scripts = list_all_scripts(engine, rt)?;

    let needle = name.to_lowercase();
    let exact_matches: Vec<_> = all_scripts
        .iter()
        .filter(|(_, script_name)| script_name.eq_ignore_ascii_case(name))
        .cloned()
        .collect();

    if let [matched] = exact_matches.as_slice() {
        return Ok(matched.clone());
    }

    if exact_matches.len() > 1 {
        return Err(ambiguous_script_error(name, &exact_matches));
    }

    let fuzzy_matches: Vec<_> = all_scripts
        .into_iter()
        .filter(|(_, script_name)| script_name.to_lowercase().contains(&needle))
        .collect();

    match fuzzy_matches.as_slice() {
        [] => Err(bifrost_core::BifrostError::Config(format!(
            "script '{}' not found in any type",
            name
        ))),
        [matched] => Ok(matched.clone()),
        _ => Err(ambiguous_script_error(name, &fuzzy_matches)),
    }
}

fn ambiguous_script_error(
    query: &str,
    candidates: &[(ScriptType, String)],
) -> bifrost_core::BifrostError {
    let candidate_list = candidates
        .iter()
        .map(|(script_type, name)| format!("{} {}", script_type, name))
        .collect::<Vec<_>>()
        .join(", ");
    bifrost_core::BifrostError::Config(format!(
        "script '{}' matched multiple scripts: {}. Please specify the type explicitly.",
        query, candidate_list
    ))
}

fn load_values(data_dir: &Path) -> HashMap<String, String> {
    let Ok(storage) = ValuesStorage::with_dir(data_dir.join("values")) else {
        return HashMap::new();
    };

    let Ok(keys) = storage.list_keys() else {
        return HashMap::new();
    };

    keys.into_iter()
        .filter_map(|key| storage.get_value(&key).map(|value| (key, value)))
        .collect()
}

fn build_mock_request() -> RequestData {
    RequestData {
        url: "https://example.com/api".to_string(),
        method: "GET".to_string(),
        host: "example.com".to_string(),
        path: "/api".to_string(),
        protocol: "https".to_string(),
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("cli".to_string()),
        headers: HashMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-bifrost-source".to_string(), "cli".to_string()),
        ]),
        body: Some("{\"message\":\"hello from bifrost cli\"}".to_string()),
    }
}

fn build_mock_response(request: &RequestData) -> ResponseData {
    ResponseData {
        status: 200,
        status_text: "OK".to_string(),
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some("{\"ok\":true,\"source\":\"bifrost-cli\"}".to_string()),
        request: request.clone(),
    }
}

fn print_logs(logs: &[ScriptLogEntry]) {
    println!("Logs:");
    if logs.is_empty() {
        println!("No logs.");
        return;
    }

    for log in logs {
        print!("[{}] {}", log.level, log.message);
        if let Some(args) = &log.args {
            if !args.is_empty() {
                let rendered_args = args
                    .iter()
                    .map(|arg| match arg {
                        serde_json::Value::String(text) => text.clone(),
                        _ => arg.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                print!(" {}", rendered_args);
            }
        }
        println!();
    }
}

fn print_run_result(result: &ScriptExecutionResult) -> bifrost_core::Result<()> {
    println!("Script: {} ({})", result.script_name, result.script_type);
    println!("Success: {}", result.success);
    println!("Duration: {} ms", result.duration_ms);
    println!();

    println!("Output:");
    if let Some(error) = &result.error {
        println!("Error: {}", error);
    } else if let Some(output) = &result.decode_output {
        println!(
            "{}",
            serde_json::to_string_pretty(output)
                .map_err(|e| bifrost_core::BifrostError::Config(e.to_string()))?
        );
    } else if let Some(mods) = &result.request_modifications {
        println!(
            "{}",
            serde_json::to_string_pretty(mods)
                .map_err(|e| bifrost_core::BifrostError::Config(e.to_string()))?
        );
    } else if let Some(mods) = &result.response_modifications {
        println!(
            "{}",
            serde_json::to_string_pretty(mods)
                .map_err(|e| bifrost_core::BifrostError::Config(e.to_string()))?
        );
    } else {
        println!("null");
    }
    println!();

    print_logs(&result.logs);
    Ok(())
}

fn handle_online_script_command(
    action: ScriptCommands,
    client: &ConfigApiClient,
) -> bifrost_core::Result<()> {
    match action {
        ScriptCommands::Add {
            r#type,
            name,
            content,
            file,
        } => {
            let script_type = parse_script_type(&r#type)?;
            let script_content = read_script_content(content, file)?;
            client
                .save_script(&r#type, &name, &script_content)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Script '{}' ({}) saved successfully.", name, script_type);
        }
        ScriptCommands::Update {
            r#type,
            name,
            content,
            file,
        } => {
            let script_type = parse_script_type(&r#type)?;
            let script_content = read_script_content(content, file)?;
            client
                .get_script(&r#type, &name)
                .map_err(bifrost_core::BifrostError::Config)?;
            client
                .save_script(&r#type, &name, &script_content)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Script '{}' ({}) updated successfully.", name, script_type);
        }
        ScriptCommands::Delete { r#type, name } => {
            let script_type = parse_script_type(&r#type)?;
            client
                .delete_script(&r#type, &name)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Script '{}' ({}) deleted successfully.", name, script_type);
        }
        ScriptCommands::Rename {
            r#type,
            name,
            new_name,
        } => {
            parse_script_type(&r#type)?;
            client
                .rename_script(&r#type, &name, &new_name)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("Script '{}/{}' renamed to '{}'.", r#type, name, new_name);
        }
        ScriptCommands::List { .. } | ScriptCommands::Show { .. } | ScriptCommands::Run { .. } => {
            return Err(bifrost_core::BifrostError::Config(
                "internal error: read-only script command routed as an online mutation".to_string(),
            ));
        }
    }
    Ok(())
}

fn routes_script_mutation_to_api(action: &ScriptCommands) -> bool {
    matches!(
        action,
        ScriptCommands::Add { .. }
            | ScriptCommands::Update { .. }
            | ScriptCommands::Delete { .. }
            | ScriptCommands::Rename { .. }
    )
}

fn route_online_script_command(
    action: ScriptCommands,
    client: Option<&ConfigApiClient>,
) -> bifrost_core::Result<Option<ScriptCommands>> {
    if routes_script_mutation_to_api(&action) {
        if let Some(client) = client {
            handle_online_script_command(action, client)?;
            return Ok(None);
        }
    }
    Ok(Some(action))
}

pub fn handle_script_command(action: ScriptCommands) -> bifrost_core::Result<()> {
    let client = routes_script_mutation_to_api(&action)
        .then(super::config::runtime::live_config_api_client)
        .transpose()?
        .flatten();
    let Some(action) = route_online_script_command(action, client.as_ref())? else {
        return Ok(());
    };

    let data_dir = bifrost_storage::data_dir();
    let scripts_dir = data_dir.join("scripts");
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: scripts_dir.clone(),
        ..Default::default()
    });

    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("failed to create tokio runtime: {e}"))
    })?;

    rt.block_on(engine.init())
        .map_err(|e| bifrost_core::BifrostError::Config(format!("failed to init scripts: {e}")))?;

    match action {
        ScriptCommands::List { r#type } => {
            let types: Vec<ScriptType> = if let Some(ref t) = r#type {
                vec![parse_script_type(t)?]
            } else {
                vec![
                    ScriptType::Request,
                    ScriptType::Response,
                    ScriptType::Decode,
                    ScriptType::Parser,
                ]
            };

            let mut total = 0;
            for script_type in &types {
                let scripts = rt
                    .block_on(engine.list_scripts(*script_type))
                    .map_err(|e| {
                        bifrost_core::BifrostError::Config(format!(
                            "failed to list {} scripts: {e}",
                            script_type
                        ))
                    })?;

                if !scripts.is_empty() {
                    println!("{} scripts ({}):", script_type, scripts.len());
                    for info in &scripts {
                        println!("  {}", info.name);
                    }
                    total += scripts.len();
                }
            }

            if total == 0 {
                println!("No scripts found.");
            }

            println!();
            println!("Scripts directory: {}", scripts_dir.display());
        }
        ScriptCommands::Add {
            r#type,
            name,
            content,
            file,
        } => {
            let script_type = parse_script_type(&r#type)?;
            let script_content = read_script_content(content, file)?;

            rt.block_on(engine.save_script(script_type, &name, &script_content))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to save {} script '{}': {e}",
                        script_type, name
                    ))
                })?;
            println!("Script '{}' ({}) saved successfully.", name, script_type);
        }
        ScriptCommands::Update {
            r#type,
            name,
            content,
            file,
        } => {
            let script_type = parse_script_type(&r#type)?;
            let script_content = read_script_content(content, file)?;

            rt.block_on(engine.load_script(script_type, &name))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to load existing {} script '{}': {e}",
                        script_type, name
                    ))
                })?;

            rt.block_on(engine.save_script(script_type, &name, &script_content))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to update {} script '{}': {e}",
                        script_type, name
                    ))
                })?;
            println!("Script '{}' ({}) updated successfully.", name, script_type);
        }
        ScriptCommands::Delete { r#type, name } => {
            let script_type = parse_script_type(&r#type)?;

            rt.block_on(engine.delete_script(script_type, &name))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to delete {} script '{}': {e}",
                        script_type, name
                    ))
                })?;
            println!("Script '{}' ({}) deleted successfully.", name, script_type);
        }
        ScriptCommands::Show { args } => {
            let selection = parse_lookup_args(&args, "show/get")?;
            let (script_type, name) = match selection.script_type {
                Some(script_type) => (script_type, selection.name),
                None => find_matching_script(&engine, &rt, &selection.name)?,
            };

            let content = rt
                .block_on(engine.load_script(script_type, &name))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to load {} script '{}': {e}",
                        script_type, name
                    ))
                })?;
            println!("Script: {} ({})", name, script_type);
            println!("Content:");
            println!("{}", content);
        }
        ScriptCommands::Run { args } => {
            let selection = parse_lookup_args(&args, "run")?;
            let (script_type, name) = match selection.script_type {
                Some(script_type) => (script_type, selection.name),
                None => find_matching_script(&engine, &rt, &selection.name)?,
            };

            let content = rt
                .block_on(engine.load_script(script_type, &name))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to load {} script '{}': {e}",
                        script_type, name
                    ))
                })?;

            let values = load_values(&data_dir);
            let request = build_mock_request();
            let response = build_mock_response(&request);
            let ctx = ScriptContext {
                request_id: "cli-test".to_string(),
                script_name: name.clone(),
                script_type,
                values,
                matched_rules: vec![],
            };

            let config = ConfigManager::new(data_dir.clone())
                .ok()
                .map(|manager| rt.block_on(manager.config()));

            let mut result = if let Some(config) = config.as_ref() {
                rt.block_on(engine.test_script_with_config(
                    script_type,
                    &content,
                    Some(&request),
                    Some(&response),
                    &ctx,
                    config,
                ))
            } else {
                rt.block_on(engine.test_script(
                    script_type,
                    &content,
                    Some(&request),
                    Some(&response),
                    &ctx,
                ))
            };
            result.script_name = name;

            print_run_result(&result)?;
        }
        ScriptCommands::Rename {
            r#type,
            name,
            new_name,
        } => {
            let script_type = parse_script_type(&r#type)?;
            rt.block_on(engine.rename_script(script_type, &name, &new_name))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "failed to rename {} script '{}' to '{}': {e}",
                        script_type, name, new_name
                    ))
                })?;

            println!("Script '{}/{}' renamed to '{}'.", r#type, name, new_name);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::NamedTempFile;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> ConfigApiClient {
        ConfigApiClient::new("127.0.0.1", server.address().port())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn online_script_commands_use_admin_api_for_all_mutations() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/scripts/request/add-script"))
            .and(body_json(json!({"content": "function onRequest() {}"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_script_command(
            ScriptCommands::Add {
                r#type: "request".to_string(),
                name: "add-script".to_string(),
                content: Some("function onRequest() {}".to_string()),
                file: None,
            },
            &client,
        )
        .unwrap();

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/scripts/response/update-script"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": "function onResponse() {}"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/scripts/response/update-script"))
            .and(body_json(
                json!({"content": "function onResponse(response) { return response; }"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_script_command(
            ScriptCommands::Update {
                r#type: "response".to_string(),
                name: "update-script".to_string(),
                content: Some("function onResponse(response) { return response; }".to_string()),
                file: None,
            },
            &client,
        )
        .unwrap();

        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/scripts/decode/delete-script"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_script_command(
            ScriptCommands::Delete {
                r#type: "decode".to_string(),
                name: "delete-script".to_string(),
            },
            &client,
        )
        .unwrap();

        Mock::given(method("POST"))
            .and(path("/_bifrost/api/scripts/rename/request/old-script"))
            .and(body_json(json!({"new_name": "new-script"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        handle_online_script_command(
            ScriptCommands::Rename {
                r#type: "request".to_string(),
                name: "old-script".to_string(),
                new_name: "new-script".to_string(),
            },
            &client,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn online_script_command_rejects_read_only_routing() {
        let server = MockServer::start().await;
        let error = handle_online_script_command(
            ScriptCommands::List { r#type: None },
            &client_for(&server),
        )
        .unwrap_err();
        assert!(error.to_string().contains("read-only script command"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn script_routing_uses_api_only_for_mutations() {
        let server = MockServer::start().await;
        let client = client_for(&server);
        let list = ScriptCommands::List { r#type: None };
        assert!(!routes_script_mutation_to_api(&list));
        assert!(route_online_script_command(list, Some(&client))
            .unwrap()
            .is_some());

        Mock::given(method("DELETE"))
            .and(path("/_bifrost/api/scripts/request/routed-script"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        let routed = route_online_script_command(
            ScriptCommands::Delete {
                r#type: "request".to_string(),
                name: "routed-script".to_string(),
            },
            Some(&client),
        )
        .unwrap();
        assert!(routed.is_none());

        let offline = route_online_script_command(
            ScriptCommands::Rename {
                r#type: "request".to_string(),
                name: "old".to_string(),
                new_name: "new".to_string(),
            },
            None,
        )
        .unwrap();
        assert!(offline.is_some());
    }

    #[test]
    fn parse_lookup_args_supports_name_only() {
        let args = vec!["demo".to_string()];
        let selection = parse_lookup_args(&args, "show").unwrap();
        assert_eq!(
            selection,
            ScriptSelection {
                script_type: None,
                name: "demo".to_string()
            }
        );
    }

    #[test]
    fn parse_lookup_args_supports_type_and_name() {
        let args = vec!["request".to_string(), "demo".to_string()];
        let selection = parse_lookup_args(&args, "show").unwrap();
        assert_eq!(
            selection,
            ScriptSelection {
                script_type: Some(ScriptType::Request),
                name: "demo".to_string()
            }
        );
    }

    #[test]
    fn parse_lookup_args_rejects_invalid_arity() {
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let error = parse_lookup_args(&args, "run").unwrap_err();
        assert!(error.to_string().contains("script run expects"));
    }

    #[test]
    fn ambiguous_script_error_lists_candidates() {
        let error = ambiguous_script_error(
            "demo",
            &[
                (ScriptType::Request, "foo/demo".to_string()),
                (ScriptType::Response, "bar/demo".to_string()),
            ],
        );
        let message = error.to_string();
        assert!(message.contains("demo"));
        assert!(message.contains("request foo/demo"));
        assert!(message.contains("response bar/demo"));
    }

    #[test]
    fn parse_script_type_accepts_aliases_and_rejects_invalid() {
        assert_eq!(parse_script_type("request").unwrap(), ScriptType::Request);
        assert_eq!(parse_script_type("REQ").unwrap(), ScriptType::Request);
        assert_eq!(parse_script_type("res").unwrap(), ScriptType::Response);
        assert_eq!(parse_script_type("dec").unwrap(), ScriptType::Decode);
        assert_eq!(parse_script_type("parser").unwrap(), ScriptType::Parser);
        let err = parse_script_type("unknown").unwrap_err();
        assert!(err.to_string().contains("Invalid script type"));
    }

    #[test]
    fn read_script_content_prefers_inline_content() {
        let result = read_script_content(Some("inline".to_string()), None).unwrap();
        assert_eq!(result, "inline");
    }

    #[test]
    fn read_script_content_reads_from_file_when_no_inline_content() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "from-file").unwrap();
        let result = read_script_content(None, Some(file.path().to_path_buf())).unwrap();
        assert_eq!(result, "from-file");
    }

    #[test]
    fn read_script_content_errors_when_missing_sources() {
        let err = read_script_content(None, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("Either --content or --file must be provided"));
    }

    #[test]
    fn build_mock_request_and_response_have_consistent_fields() {
        let req = build_mock_request();
        assert_eq!(req.method, "GET");
        assert!(req.headers.contains_key("content-type"));
        let resp = build_mock_response(&req);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.request.url, req.url);
    }

    #[test]
    fn print_logs_handles_empty_and_non_empty_args() {
        let logs = vec![
            ScriptLogEntry {
                timestamp: 0,
                level: bifrost_script::ScriptLogLevel::Info,
                message: "plain".to_string(),
                args: None,
            },
            ScriptLogEntry {
                timestamp: 1,
                level: bifrost_script::ScriptLogLevel::Error,
                message: "with args".to_string(),
                args: Some(vec![json!("one"), json!({"k": "v"})]),
            },
        ];
        print_logs(&logs);
    }

    #[test]
    fn print_run_result_handles_error_branch() {
        let result = ScriptExecutionResult {
            script_name: "demo".to_string(),
            script_type: ScriptType::Decode,
            success: false,
            error: Some("boom".to_string()),
            duration_ms: 1,
            logs: Vec::new(),
            request_modifications: None,
            response_modifications: None,
            decode_output: None,
        };
        print_run_result(&result).unwrap();
    }

    #[test]
    fn print_run_result_handles_decode_output_branch() {
        let result = ScriptExecutionResult {
            script_name: "demo".to_string(),
            script_type: ScriptType::Decode,
            success: true,
            error: None,
            duration_ms: 1,
            logs: Vec::new(),
            request_modifications: None,
            response_modifications: None,
            decode_output: Some(bifrost_script::DecodeOutput {
                data: "decoded".to_string(),
                code: "0".to_string(),
                msg: "".to_string(),
            }),
        };
        print_run_result(&result).unwrap();
    }

    #[test]
    fn print_run_result_handles_request_modifications_branch() {
        let mods = bifrost_script::TestRequestModifications {
            method: Some("POST".to_string()),
            headers: Some(HashMap::from([(String::from("x"), String::from("y"))])),
            body: Some("body".to_string()),
        };
        let result = ScriptExecutionResult {
            script_name: "demo".to_string(),
            script_type: ScriptType::Request,
            success: true,
            error: None,
            duration_ms: 1,
            logs: Vec::new(),
            request_modifications: Some(mods),
            response_modifications: None,
            decode_output: None,
        };
        print_run_result(&result).unwrap();
    }

    #[test]
    fn print_run_result_handles_response_modifications_branch() {
        let mods = bifrost_script::TestResponseModifications {
            status: Some(201),
            status_text: Some("Created".to_string()),
            headers: Some(HashMap::from([(String::from("x"), String::from("y"))])),
            body: Some("body".to_string()),
        };
        let result = ScriptExecutionResult {
            script_name: "demo".to_string(),
            script_type: ScriptType::Response,
            success: true,
            error: None,
            duration_ms: 1,
            logs: Vec::new(),
            request_modifications: None,
            response_modifications: Some(mods),
            decode_output: None,
        };
        print_run_result(&result).unwrap();
    }
}
