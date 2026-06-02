use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use bifrost_core::{BifrostError, Result};
use serde_json::Value;

use crate::cli::AiWorkflowCommands;

pub fn handle_workflow_command(
    action: AiWorkflowCommands,
    admin_host: &str,
    admin_port: u16,
) -> Result<()> {
    match action {
        AiWorkflowCommands::Schema { json } => {
            let value = serde_json::to_value(bifrost_admin::ai_workflow::schema_payload())?;
            if json {
                print_json(&value)?;
            } else {
                println!("AI Workflow schema: bifrost.ai.workflow/v1alpha1");
                println!("Node types: script, runner, asr_transcription, notification");
                println!("Default template: default-asr-transcription");
                println!("Flow: draft -> validate -> preview -> apply -> execute -> logs");
            }
            Ok(())
        }
        AiWorkflowCommands::Templates { json } => {
            let value = bifrost_admin::ai_workflow::workflow_templates_payload();
            if json {
                print_json(&value)?;
            } else {
                let templates = value
                    .get("templates")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for template in templates {
                    println!(
                        "{}\t{}\t{}",
                        template.get("id").and_then(Value::as_str).unwrap_or(""),
                        template.get("name").and_then(Value::as_str).unwrap_or(""),
                        template
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    );
                }
            }
            Ok(())
        }
        AiWorkflowCommands::Template {
            template_id,
            format,
            output,
        } => {
            let template =
                bifrost_admin::ai_workflow::workflow_template(&template_id).ok_or_else(|| {
                    BifrostError::Config(format!("unknown workflow template: {template_id}"))
                })?;
            let body = if format == "json" {
                format!("{}\n", serde_json::to_string_pretty(&template.workflow)?)
            } else {
                template.draft
            };
            if let Some(path) = output {
                fs::write(&path, body).map_err(|error| {
                    BifrostError::Io(io::Error::other(format!(
                        "write workflow template {}: {error}",
                        path.display()
                    )))
                })?;
                println!(
                    "Workflow template `{}` written to {}.",
                    template.id,
                    path.display()
                );
                Ok(())
            } else {
                print_text(&body)
            }
        }
        AiWorkflowCommands::Validate { file, json } => {
            let workflow = read_workflow_file(&file)?;
            let report = bifrost_admin::ai_workflow::validate_workflow(&workflow);
            let value = serde_json::to_value(&report)?;
            if json {
                print_json(&value)?;
            } else if report.valid {
                println!("Workflow `{}` is valid.", workflow.metadata.id);
            } else {
                println!("Workflow `{}` is invalid.", workflow.metadata.id);
                for diagnostic in report.errors {
                    println!(
                        "- [{}] {}: {}",
                        diagnostic.code, diagnostic.path, diagnostic.message
                    );
                }
            }
            Ok(())
        }
        AiWorkflowCommands::Preview { file, format } => {
            let workflow = read_workflow_file(&file)?;
            let preview = bifrost_admin::ai_workflow::preview_workflow(&workflow);
            if format == "json" {
                print_json(&serde_json::to_value(preview)?)
            } else {
                print_text(&preview.markdown)
            }
        }
        AiWorkflowCommands::Render { file } => {
            let workflow = read_workflow_file(&file)?;
            print_json(&serde_json::json!({
                "reactFlow": bifrost_admin::ai_workflow::render_workflow(&workflow)
            }))
        }
        AiWorkflowCommands::Apply {
            file,
            base_revision,
            dry_run,
            json,
        } => {
            let workflow = read_workflow_file(&file)?;
            let client = WorkflowApiClient::new(admin_host, admin_port);
            let response = client.post_json_body(
                "/ai/workflows",
                &serde_json::json!({
                    "workflow": workflow,
                    "baseRevision": base_revision,
                    "dryRun": dry_run,
                }),
            )?;
            if json || dry_run {
                print_json(&response)
            } else {
                let id = response
                    .get("workflow")
                    .and_then(|workflow| workflow.get("metadata"))
                    .and_then(|metadata| metadata.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                println!("Workflow `{id}` saved.");
                Ok(())
            }
        }
        AiWorkflowCommands::List { json } => {
            let client = WorkflowApiClient::new(admin_host, admin_port);
            let response = client.get_json("/ai/workflows")?;
            if json {
                print_json(&response)
            } else {
                let workflows = response
                    .get("workflows")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if workflows.is_empty() {
                    println!("No AI Workflows.");
                } else {
                    for workflow in workflows {
                        println!(
                            "{}\t{}\trev {}",
                            workflow.get("id").and_then(Value::as_str).unwrap_or(""),
                            workflow.get("name").and_then(Value::as_str).unwrap_or(""),
                            workflow
                                .get("revision")
                                .and_then(Value::as_u64)
                                .unwrap_or(0)
                        );
                    }
                }
                Ok(())
            }
        }
        AiWorkflowCommands::Export {
            workflow_id,
            format,
        } => {
            let client = WorkflowApiClient::new(admin_host, admin_port);
            let response =
                client.get_json(&format!("/ai/workflows/{}", url_encode(&workflow_id)))?;
            let workflow = response.get("workflow").cloned().unwrap_or(response);
            if format == "json" {
                print_json(&workflow)
            } else {
                print_text(&format!(
                    "{}\n",
                    serde_yaml::to_string(&workflow)
                        .map_err(|error| BifrostError::Config(error.to_string()))?
                ))
            }
        }
        AiWorkflowCommands::Run {
            workflow_id,
            inputs,
            json,
        } => {
            let client = WorkflowApiClient::new(admin_host, admin_port);
            let response = client.post_json_body(
                &format!("/ai/workflows/{}/run", url_encode(&workflow_id)),
                &serde_json::json!({ "inputs": parse_key_values(inputs)? }),
            )?;
            if json {
                print_json(&response)
            } else {
                let run_id = response
                    .get("run")
                    .and_then(|run| run.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                let status = response
                    .get("run")
                    .and_then(|run| run.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let event_count = response
                    .get("run")
                    .and_then(|run| run.get("events"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                let node_count = response
                    .get("run")
                    .and_then(|run| run.get("nodeStates"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                println!(
                    "Workflow executed: {run_id} status={status} nodes={node_count} events={event_count}"
                );
                Ok(())
            }
        }
        AiWorkflowCommands::Logs {
            workflow_id,
            run_id,
            json,
        } => {
            let client = WorkflowApiClient::new(admin_host, admin_port);
            let response = client.get_json(&format!(
                "/ai/workflows/{}/runs/{}",
                url_encode(&workflow_id),
                url_encode(&run_id)
            ))?;
            if json {
                print_json(&response)
            } else {
                let run = response.get("run").unwrap_or(&response);
                println!(
                    "Run: {}",
                    run.get("id").and_then(Value::as_str).unwrap_or("")
                );
                println!(
                    "Status: {}",
                    run.get("status").and_then(Value::as_str).unwrap_or("")
                );
                println!(
                    "Artifacts: {}",
                    run.get("artifactsDir")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                );
                let events = run
                    .get("events")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                println!("Events: {}", events.len());
                for event in events.iter().take(8) {
                    println!(
                        "- {} {} {}",
                        event.get("ts").and_then(Value::as_str).unwrap_or(""),
                        event.get("event").and_then(Value::as_str).unwrap_or(""),
                        event.get("nodeId").and_then(Value::as_str).unwrap_or("")
                    );
                }
                let nodes = run
                    .get("nodeStates")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                println!("Nodes: {}", nodes.len());
                for node in nodes {
                    println!(
                        "- {} [{}] status={} attempts={} artifacts={}",
                        node.get("nodeId").and_then(Value::as_str).unwrap_or(""),
                        node.get("kind").and_then(Value::as_str).unwrap_or(""),
                        node.get("status").and_then(Value::as_str).unwrap_or(""),
                        node.get("attempt")
                            .and_then(Value::as_u64)
                            .unwrap_or_default(),
                        node.get("artifacts")
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or_default()
                    );
                    if let Some(path) = node.get("attemptLogPath").and_then(Value::as_str) {
                        println!("  attempt: {path}");
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
struct WorkflowApiClient {
    base_url: String,
    agent: ureq::Agent,
}

impl WorkflowApiClient {
    fn new(host: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}/_bifrost/api", host, port),
            agent: bifrost_core::direct_ureq_agent_builder()
                .timeout(Duration::from_secs(30 * 60))
                .build(),
        }
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|error| api_error("GET", &url, error))?;
        read_json_response("GET", &url, response)
    }

    fn post_json_body(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .post(&url)
            .set("content-type", "application/json")
            .send_string(&body.to_string())
            .map_err(|error| api_error("POST", &url, error))?;
        read_json_response("POST", &url, response)
    }
}

fn read_workflow_file(path: &Path) -> Result<bifrost_admin::ai_workflow::WorkflowDocument> {
    let body = fs::read_to_string(path).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read workflow file {}: {error}",
            path.display()
        )))
    })?;
    bifrost_admin::ai_workflow::parse_workflow_document(&body)
        .map(bifrost_admin::ai_workflow::normalize_workflow)
        .map_err(BifrostError::Config)
}

fn parse_key_values(inputs: Vec<String>) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for item in inputs {
        let Some((key, value)) = item.split_once('=') else {
            return Err(BifrostError::Config(format!(
                "workflow input must use KEY=VALUE: {item}"
            )));
        };
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Ok(Value::Object(map))
}

fn read_json_response(method: &str, url: &str, response: ureq::Response) -> Result<Value> {
    let body = response.into_string().map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read Workflow API response from {url}: {error}"
        )))
    })?;
    serde_json::from_str(&body).map_err(|error| {
        BifrostError::Config(format!(
            "{method} {url} returned invalid JSON: {error}; body: {}",
            truncate(&body, 300)
        ))
    })
}

fn api_error(method: &str, url: &str, error: ureq::Error) -> BifrostError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            BifrostError::Config(format!(
                "{method} {url} failed with HTTP {status}: {}",
                truncate(&body, 500)
            ))
        }
        other => BifrostError::Config(format!(
            "Failed to connect to Bifrost admin API at {url}\n\
             Start Bifrost first, or pass -p/--port for a non-default admin port.\n\
             Cause: {other}"
        )),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn print_json(value: &Value) -> Result<()> {
    print_text(&format!(
        "{}\n",
        serde_json::to_string_pretty(value)
            .map_err(|error| BifrostError::Config(error.to_string()))?
    ))
}

fn print_text(text: &str) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn url_encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}
