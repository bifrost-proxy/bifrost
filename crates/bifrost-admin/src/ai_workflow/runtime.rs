use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use bifrost_agent::tools::ToolHandler;
use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as Sha2Digest, Sha256};
use tokio::process::Command;
use uuid::Uuid;

use super::{sanitize_id, WorkflowDocument, WorkflowNode, WorkflowOutput, WorkflowRunRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowArtifactRecord {
    name: String,
    kind: String,
    path: String,
    size_bytes: u64,
    sha256: String,
    summary: String,
}

#[derive(Debug, Clone)]
struct NodeExecutionResult {
    status: NodeStatus,
    artifacts: Vec<WorkflowArtifactRecord>,
    message: Option<String>,
    metadata: Value,
    attempts: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeStatus {
    Success,
    NoUpdate,
    Skipped,
    Failed,
}

impl NodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NoUpdate => "no_update",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

pub(super) async fn create_run(
    workflow: &WorkflowDocument,
    inputs: Value,
    runs_dir: &Path,
) -> Result<WorkflowRunRecord, String> {
    let run_id = format!("run-{}", Uuid::new_v4());
    let run_dir = runs_dir.join(&run_id);
    fs::create_dir_all(&run_dir).map_err(|error| error.to_string())?;
    let created_at = Utc::now().to_rfc3339();
    let mut events = vec![
        json!({
            "ts": created_at,
            "event": "run_created",
            "workflowId": workflow.metadata.id,
            "workflowRevision": workflow.metadata.revision,
            "inputs": inputs,
        }),
        json!({
            "ts": Utc::now().to_rfc3339(),
            "event": "topology_planned",
            "nodeCount": workflow.spec.nodes.len(),
            "edgeCount": workflow.spec.edges.len(),
        }),
    ];
    write_log_line(
        &run_dir.join("logs").join("run.log"),
        &format!(
            "run {} for workflow {} revision {} created",
            run_id, workflow.metadata.id, workflow.metadata.revision
        ),
    )?;

    let mut node_states = Vec::new();
    let mut produced_outputs = BTreeMap::<String, BTreeMap<String, WorkflowArtifactRecord>>::new();
    let mut skipped_nodes = BTreeSet::<String>::new();
    let mut run_failed = false;
    let outgoing = outgoing_edges(workflow);
    for node in topological_nodes(workflow) {
        let node_started_at = Utc::now();
        let node_dir = run_dir.join("nodes").join(&node.id);
        fs::create_dir_all(&node_dir).map_err(|error| error.to_string())?;
        let input_snapshot = resolve_node_inputs(node, &inputs, &produced_outputs);
        write_pretty_json(&node_dir.join("input_manifest.json"), &input_snapshot)?;

        let result = if skipped_nodes.contains(&node.id) {
            let attempt_dir = node_dir.join("attempts").join("1");
            fs::create_dir_all(&attempt_dir).map_err(|error| error.to_string())?;
            events.push(json!({
                "ts": node_started_at.to_rfc3339(),
                "event": "node_started",
                "nodeId": node.id,
                "kind": node.node_type,
                "attempt": 1,
            }));
            write_log_line(
                &run_dir.join("logs").join("run.log"),
                &format!("node {} ({}) started attempt=1", node.id, node.node_type),
            )?;
            Ok(NodeExecutionResult {
                status: NodeStatus::Skipped,
                artifacts: Vec::new(),
                message: Some("skipped because upstream returned no_update".to_string()),
                metadata: json!({ "skipReason": "upstream_no_update" }),
                attempts: Vec::new(),
            })
        } else {
            execute_node_with_retries(
                workflow,
                node,
                &inputs,
                &input_snapshot,
                &node_dir,
                &run_dir,
                &mut events,
            )
            .await
        };
        let finished_at = Utc::now();
        let elapsed_ms = (finished_at - node_started_at).num_milliseconds().max(0);
        let final_attempt = result
            .as_ref()
            .ok()
            .and_then(final_attempt_number)
            .unwrap_or(1);
        let attempt_dir = node_dir.join("attempts").join(final_attempt.to_string());
        match result {
            Ok(result) if result.status == NodeStatus::Failed => {
                run_failed = true;
                finish_node(
                    node,
                    result,
                    &mut produced_outputs,
                    &mut events,
                    &mut node_states,
                    &node_dir,
                    &attempt_dir,
                    &run_dir,
                    node_started_at,
                    finished_at,
                    elapsed_ms,
                    final_attempt,
                )?;
                break;
            }
            Ok(result) => {
                let should_skip_downstream =
                    result.status == NodeStatus::NoUpdate && node_no_update_skips_downstream(node);
                finish_node(
                    node,
                    result,
                    &mut produced_outputs,
                    &mut events,
                    &mut node_states,
                    &node_dir,
                    &attempt_dir,
                    &run_dir,
                    node_started_at,
                    finished_at,
                    elapsed_ms,
                    final_attempt,
                )?;
                if should_skip_downstream {
                    mark_downstream_skipped(&node.id, &outgoing, &mut skipped_nodes);
                }
            }
            Err(error) => {
                run_failed = true;
                finish_node(
                    node,
                    NodeExecutionResult {
                        status: NodeStatus::Failed,
                        artifacts: Vec::new(),
                        message: Some(error),
                        metadata: Value::Null,
                        attempts: Vec::new(),
                    },
                    &mut produced_outputs,
                    &mut events,
                    &mut node_states,
                    &node_dir,
                    &attempt_dir,
                    &run_dir,
                    node_started_at,
                    finished_at,
                    elapsed_ms,
                    final_attempt,
                )?;
                break;
            }
        }
    }

    let finished_at = Utc::now().to_rfc3339();
    let status = if run_failed { "failed" } else { "success" };
    events.push(json!({
        "ts": finished_at,
        "event": "run_finished",
        "status": status,
        "nodeCount": node_states.len(),
    }));
    write_log_line(
        &run_dir.join("logs").join("run.log"),
        &format!(
            "run {} finished status={} nodes={}",
            run_id,
            status,
            node_states.len()
        ),
    )?;
    write_events_jsonl(&run_dir.join("events.jsonl"), &events)?;
    write_log_index(&run_dir, workflow, &events, &node_states)?;
    let record = WorkflowRunRecord {
        id: run_id,
        workflow_id: workflow.metadata.id.clone(),
        workflow_revision: workflow.metadata.revision,
        status: status.to_string(),
        created_at,
        finished_at: Some(finished_at),
        node_states,
        events,
        artifacts_dir: run_dir.to_string_lossy().to_string(),
    };
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
fn finish_node(
    node: &WorkflowNode,
    result: NodeExecutionResult,
    produced_outputs: &mut BTreeMap<String, BTreeMap<String, WorkflowArtifactRecord>>,
    events: &mut Vec<Value>,
    node_states: &mut Vec<Value>,
    node_dir: &Path,
    attempt_dir: &Path,
    run_dir: &Path,
    node_started_at: chrono::DateTime<Utc>,
    finished_at: chrono::DateTime<Utc>,
    elapsed_ms: i64,
    final_attempt: u64,
) -> Result<(), String> {
    let artifact_values = result
        .artifacts
        .iter()
        .map(|artifact| serde_json::to_value(artifact).unwrap_or_default())
        .collect::<Vec<_>>();
    write_pretty_json(
        &node_dir.join("output_manifest.json"),
        &json!({
            "nodeId": node.id,
            "status": result.status.as_str(),
            "message": result.message,
            "metadata": result.metadata,
            "attempts": result.attempts,
            "artifacts": artifact_values,
        }),
    )?;
    write_pretty_json(
        &attempt_dir.join("attempt.json"),
        &json!({
            "attempt": final_attempt,
            "status": result.status.as_str(),
            "startedAt": node_started_at.to_rfc3339(),
            "finishedAt": finished_at.to_rfc3339(),
            "elapsedMs": elapsed_ms,
            "artifactCount": result.artifacts.len(),
            "message": result.message,
            "metadata": result.metadata,
            "attempts": result.attempts,
        }),
    )?;
    let stderr = if result.status == NodeStatus::Failed {
        result.message.clone().unwrap_or_default()
    } else {
        String::new()
    };
    let stdout_path = attempt_dir.join("stdout.log");
    if !stdout_path.is_file() {
        fs::write(&stdout_path, result.message.clone().unwrap_or_default())
            .map_err(|error| error.to_string())?;
    }
    fs::write(attempt_dir.join("stderr.log"), stderr).map_err(|error| error.to_string())?;
    if !matches!(result.status, NodeStatus::Failed | NodeStatus::Skipped) {
        produced_outputs.insert(
            node.id.clone(),
            result
                .artifacts
                .iter()
                .map(|artifact| (artifact.name.clone(), artifact.clone()))
                .collect(),
        );
    }
    events.push(json!({
        "ts": finished_at.to_rfc3339(),
        "event": "node_finished",
        "nodeId": node.id,
        "kind": node.node_type,
        "status": result.status.as_str(),
        "attempt": final_attempt,
        "elapsedMs": elapsed_ms,
        "artifactCount": result.artifacts.len(),
        "message": result.message,
        "metadata": result.metadata,
        "attempts": result.attempts,
    }));
    write_log_line(
        &run_dir.join("logs").join("run.log"),
        &format!(
            "node {} ({}) finished status={} elapsed_ms={} artifacts={} message={}",
            node.id,
            node.node_type,
            result.status.as_str(),
            elapsed_ms,
            result.artifacts.len(),
            result.message.clone().unwrap_or_default()
        ),
    )?;
    node_states.push(json!({
        "nodeId": node.id,
        "kind": node.node_type,
        "status": result.status.as_str(),
        "attempt": final_attempt,
        "startedAt": node_started_at.to_rfc3339(),
        "finishedAt": finished_at.to_rfc3339(),
        "elapsedMs": elapsed_ms,
        "message": result.message,
        "metadata": result.metadata,
        "attempts": result.attempts,
        "inputManifestPath": node_dir.join("input_manifest.json").to_string_lossy().to_string(),
        "outputManifestPath": node_dir.join("output_manifest.json").to_string_lossy().to_string(),
        "attemptLogPath": attempt_dir.join("attempt.json").to_string_lossy().to_string(),
        "stdoutPath": attempt_dir.join("stdout.log").to_string_lossy().to_string(),
        "stderrPath": attempt_dir.join("stderr.log").to_string_lossy().to_string(),
        "artifacts": artifact_values,
    }));
    Ok(())
}

async fn execute_node(
    workflow: &WorkflowDocument,
    node: &WorkflowNode,
    workflow_inputs: &Value,
    input_snapshot: &Value,
    node_dir: &Path,
    attempt_dir: &Path,
) -> Result<NodeExecutionResult, String> {
    match node.node_type.as_str() {
        "asr_transcription" => {
            execute_asr_node(workflow, node, workflow_inputs, node_dir, attempt_dir).await
        }
        "runner" => {
            execute_runner_node(workflow, node, input_snapshot, node_dir, attempt_dir).await
        }
        "script" => execute_script_node(node, input_snapshot, node_dir, attempt_dir).await,
        "notification" => {
            execute_notification_node(node, input_snapshot, node_dir, attempt_dir).await
        }
        other => Err(format!("unsupported workflow node type: {other}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_node_with_retries(
    workflow: &WorkflowDocument,
    node: &WorkflowNode,
    workflow_inputs: &Value,
    input_snapshot: &Value,
    node_dir: &Path,
    run_dir: &Path,
    events: &mut Vec<Value>,
) -> Result<NodeExecutionResult, String> {
    let max_attempts = node_retry_max_attempts(node);
    let mut attempt_records = Vec::new();
    let mut last_error = None;
    for attempt in 1..=max_attempts {
        let attempt_started_at = Utc::now();
        let attempt_dir = node_dir.join("attempts").join(attempt.to_string());
        fs::create_dir_all(&attempt_dir).map_err(|error| error.to_string())?;
        events.push(json!({
            "ts": attempt_started_at.to_rfc3339(),
            "event": "node_started",
            "nodeId": node.id,
            "kind": node.node_type,
            "attempt": attempt,
        }));
        write_log_line(
            &run_dir.join("logs").join("run.log"),
            &format!(
                "node {} ({}) started attempt={}",
                node.id, node.node_type, attempt
            ),
        )?;

        let result = execute_node(
            workflow,
            node,
            workflow_inputs,
            input_snapshot,
            node_dir,
            &attempt_dir,
        )
        .await;
        let attempt_finished_at = Utc::now();
        let elapsed_ms = (attempt_finished_at - attempt_started_at)
            .num_milliseconds()
            .max(0);
        match result {
            Ok(mut result) => {
                let attempt_record = json!({
                    "attempt": attempt,
                    "status": result.status.as_str(),
                    "startedAt": attempt_started_at.to_rfc3339(),
                    "finishedAt": attempt_finished_at.to_rfc3339(),
                    "elapsedMs": elapsed_ms,
                    "message": result.message,
                    "metadata": result.metadata,
                });
                attempt_records.push(attempt_record.clone());
                if result.attempts.is_empty() {
                    result.attempts = attempt_records;
                } else {
                    result.attempts.extend(attempt_records);
                }
                write_pretty_json(&attempt_dir.join("attempt.json"), &attempt_record)?;
                return Ok(result);
            }
            Err(error) => {
                last_error = Some(error.clone());
                let attempt_record = json!({
                    "attempt": attempt,
                    "status": "failed",
                    "startedAt": attempt_started_at.to_rfc3339(),
                    "finishedAt": attempt_finished_at.to_rfc3339(),
                    "elapsedMs": elapsed_ms,
                    "message": error,
                });
                write_pretty_json(&attempt_dir.join("attempt.json"), &attempt_record)?;
                if !attempt_dir.join("stdout.log").is_file() {
                    fs::write(attempt_dir.join("stdout.log"), "")
                        .map_err(|error| error.to_string())?;
                }
                if !attempt_dir.join("stderr.log").is_file() {
                    fs::write(attempt_dir.join("stderr.log"), error.as_bytes())
                        .map_err(|error| error.to_string())?;
                }
                events.push(json!({
                    "ts": attempt_finished_at.to_rfc3339(),
                    "event": "node_attempt_failed",
                    "nodeId": node.id,
                    "kind": node.node_type,
                    "attempt": attempt,
                    "elapsedMs": elapsed_ms,
                    "message": error,
                }));
                write_log_line(
                    &run_dir.join("logs").join("run.log"),
                    &format!(
                        "node {} ({}) attempt={} failed elapsed_ms={} message={}",
                        node.id, node.node_type, attempt, elapsed_ms, error
                    ),
                )?;
                attempt_records.push(attempt_record);
            }
        }
    }

    Ok(NodeExecutionResult {
        status: NodeStatus::Failed,
        artifacts: Vec::new(),
        message: last_error.or_else(|| Some("node failed".to_string())),
        metadata: json!({ "maxAttempts": max_attempts }),
        attempts: attempt_records,
    })
}

async fn execute_asr_node(
    workflow: &WorkflowDocument,
    node: &WorkflowNode,
    workflow_inputs: &Value,
    node_dir: &Path,
    attempt_dir: &Path,
) -> Result<NodeExecutionResult, String> {
    let audio_dir = workflow_input_string(node, workflow_inputs, "audio_dir")
        .or_else(|| {
            workflow_inputs
                .get("audio_dir")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| format!("ASR node {} requires audio_dir workflow input", node.id))?;
    let task_id = node
        .extra
        .get("taskId")
        .or_else(|| node.extra.get("task_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("workflow-{}-{}", workflow.metadata.id, node.id));
    let config = crate::handlers::asr_jobs::WorkflowAsrDirectoryRunConfig {
        task_id: sanitize_id(&task_id),
        task_name: node
            .extra
            .get("taskName")
            .or_else(|| node.extra.get("task_name"))
            .and_then(Value::as_str)
            .unwrap_or(&workflow.metadata.name)
            .to_string(),
        audio_dir: PathBuf::from(audio_dir),
        recursive: node
            .extra
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        language: node
            .extra
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_string),
        model: node
            .extra
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        runtime_strategy: node
            .extra
            .get("runtimeStrategy")
            .or_else(|| node.extra.get("runtime_strategy"))
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let result = crate::handlers::asr_jobs::run_workflow_asr_directory_node(config).await?;
    write_pretty_json(&node_dir.join("asr_result.json"), &json!(result))?;
    let mut artifacts = Vec::new();
    for output in &node.outputs {
        match output.name.as_str() {
            "daily_markdown" => {
                if let Some(document) = result.daily_documents.first() {
                    artifacts.push(artifact_record_from_existing(
                        &output.name,
                        &output.output_type,
                        &document.path,
                    )?);
                } else {
                    let path = materialize_output(
                        output,
                        node_dir,
                        &format!(
                            "# No ASR Updates\n\nWorkflow ASR task `{}` found no new audio files.\n",
                            result.task_id
                        ),
                    )?;
                    artifacts.push(artifact_record(&output.name, &output.output_type, &path)?);
                }
            }
            "transcription_manifest" => {
                let path = materialize_json_output(output, node_dir, &json!(result))?;
                artifacts.push(artifact_record(&output.name, &output.output_type, &path)?);
            }
            _ => {
                let path = materialize_json_output(output, node_dir, &json!(result))?;
                artifacts.push(artifact_record(&output.name, &output.output_type, &path)?);
            }
        }
    }
    fs::write(
        attempt_dir.join("stdout.log"),
        format!(
            "ASR workflow node completed processed_now={} failed_now={} no_update={} daily_documents={}\n",
            result.processed_now,
            result.failed_now,
            result.no_update,
            result.daily_documents.len()
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(NodeExecutionResult {
        status: if result.no_update {
            NodeStatus::NoUpdate
        } else {
            NodeStatus::Success
        },
        artifacts,
        message: Some(format!(
            "processed {}, failed {}, daily documents {}",
            result.processed_now,
            result.failed_now,
            result.daily_documents.len()
        )),
        metadata: json!(result),
        attempts: Vec::new(),
    })
}

async fn execute_runner_node(
    workflow: &WorkflowDocument,
    node: &WorkflowNode,
    input_snapshot: &Value,
    node_dir: &Path,
    attempt_dir: &Path,
) -> Result<NodeExecutionResult, String> {
    let prompt = build_runner_prompt(workflow, node, input_snapshot);
    let runner_id = node
        .extra
        .get("runner")
        .or_else(|| node.extra.get("runnerId"))
        .or_else(|| node.extra.get("runner_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(default_workflow_runner_id);
    let timeout_ms = node
        .extra
        .get("timeoutMs")
        .or_else(|| node.extra.get("timeout_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(900_000);
    let mut selected_runner_id = runner_id.to_string();
    let mut fallback_error = None;
    let mut result =
        run_external_runner(&runner_id, node, prompt.clone(), node_dir, timeout_ms).await;
    if result.is_err() {
        if let Some(fallback) = node
            .extra
            .get("fallbackRunner")
            .or_else(|| node.extra.get("fallback_runner"))
            .and_then(Value::as_str)
        {
            fallback_error = result.as_ref().err().cloned();
            selected_runner_id = fallback.to_string();
            result = run_external_runner(fallback, node, prompt, node_dir, timeout_ms).await;
        }
    }
    let result = result?;
    let mut artifacts = Vec::new();
    for output in &node.outputs {
        let body = if result.response.trim().is_empty() {
            format!(
                "# Workflow Runner Output\n\nRunner `{}` completed without a final response.\n",
                selected_runner_id
            )
        } else {
            result.response.clone()
        };
        let path = materialize_output(output, node_dir, &body)?;
        artifacts.push(artifact_record(&output.name, &output.output_type, &path)?);
    }
    fs::write(
        attempt_dir.join("stdout.log"),
        format!(
            "runner {} completed status={:?} run_id={} response_len={}\n",
            selected_runner_id,
            result.status,
            result.run_id,
            result.response.len()
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(NodeExecutionResult {
        status: NodeStatus::Success,
        artifacts,
        message: Some(format!(
            "runner {selected_runner_id} completed run {}",
            result.run_id
        )),
        metadata: json!({
            "runnerId": selected_runner_id,
            "primaryRunnerId": runner_id,
            "fallbackError": fallback_error,
            "runId": result.run_id,
            "adapter": result.adapter,
            "externalStatus": format!("{:?}", result.status),
            "artifacts": result.artifacts,
            "events": result.events,
        }),
        attempts: Vec::new(),
    })
}

fn default_workflow_runner_id() -> String {
    let config_store =
        crate::im_gateway::external_cli::ExternalCliConfigStore::new(&bifrost_storage::data_dir());
    let config = config_store.load();
    let default_runner_id = config.default_runner_id.trim();
    if default_runner_id.is_empty() {
        "codex".to_string()
    } else {
        default_runner_id.to_string()
    }
}

async fn execute_script_node(
    node: &WorkflowNode,
    input_snapshot: &Value,
    node_dir: &Path,
    attempt_dir: &Path,
) -> Result<NodeExecutionResult, String> {
    let script = node
        .extra
        .get("script")
        .or_else(|| node.extra.get("command"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("script node {} requires script or command", node.id))?;
    let input_path = node_dir.join("script_input.json");
    write_pretty_json(&input_path, input_snapshot)?;
    let output_path = node_dir.join("script_output.txt");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(script)
        .env("BIFROST_WORKFLOW_INPUT", &input_path)
        .env("BIFROST_WORKFLOW_OUTPUT", &output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .map_err(|_| format!("script node {} timed out", node.id))?
        .map_err(|error| format!("script node {} failed to spawn: {error}", node.id))?;
    fs::write(attempt_dir.join("stdout.log"), &output.stdout).map_err(|error| error.to_string())?;
    fs::write(attempt_dir.join("stderr.log"), &output.stderr).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "script node {} exited with status {:?}: {}",
            node.id,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if !output_path.exists() {
        fs::write(&output_path, &output.stdout).map_err(|error| error.to_string())?;
    }
    let mut artifacts = Vec::new();
    if node.outputs.is_empty() {
        artifacts.push(artifact_record("script_output", "file", &output_path)?);
    } else {
        for output in &node.outputs {
            let path = materialize_output(
                output,
                node_dir,
                &String::from_utf8_lossy(
                    &fs::read(&output_path).map_err(|error| error.to_string())?,
                ),
            )?;
            artifacts.push(artifact_record(&output.name, &output.output_type, &path)?);
        }
    }
    Ok(NodeExecutionResult {
        status: NodeStatus::Success,
        artifacts,
        message: Some("script completed".to_string()),
        metadata: json!({ "exitCode": output.status.code() }),
        attempts: Vec::new(),
    })
}

async fn execute_notification_node(
    node: &WorkflowNode,
    input_snapshot: &Value,
    node_dir: &Path,
    attempt_dir: &Path,
) -> Result<NodeExecutionResult, String> {
    let path = node_dir.join("notification_request.json");
    let channel = node
        .extra
        .get("channel")
        .ok_or_else(|| format!("notification node {} requires channel", node.id))?;
    let title = notification_title(node);
    let body = notification_body(node, input_snapshot);
    write_pretty_json(
        &path,
        &json!({
            "channel": channel,
            "input": input_snapshot,
            "title": title,
            "message": body,
            "recordedAt": Utc::now().to_rfc3339(),
        }),
    )?;

    let notification_id =
        crate::notification_db::create_notification(&crate::notification_db::CreateNotification {
            notification_type: "ai_workflow".to_string(),
            title: title.clone(),
            message: body.clone(),
            metadata: Some(
                serde_json::to_string(&json!({
                    "nodeId": node.id,
                    "channel": channel,
                    "requestPath": path,
                }))
                .map_err(|error| error.to_string())?,
            ),
        })
        .map_err(|error| error.to_string())?;

    let mut delivery = json!({
        "localNotificationId": notification_id,
        "localNotificationStatus": "created",
    });
    if notification_channel_requires_im(channel) {
        let im_result = send_workflow_notification_im(channel, &title, &body, node_dir).await?;
        delivery["im"] = im_result;
    }
    let receipt_path = node_dir.join("notification_receipt.json");
    write_pretty_json(&receipt_path, &delivery)?;
    fs::write(
        attempt_dir.join("stdout.log"),
        format!(
            "notification node created local notification id={} im_delivery={}\n",
            notification_id,
            delivery.get("im").is_some()
        ),
    )
    .map_err(|error| error.to_string())?;
    let artifacts = if node.outputs.is_empty() {
        vec![
            artifact_record("notification_request", "json", &path)?,
            artifact_record("notification_receipt", "json", &receipt_path)?,
        ]
    } else {
        let mut artifacts = Vec::new();
        for output in &node.outputs {
            let source_path = if output.name.contains("request") {
                &path
            } else {
                &receipt_path
            };
            artifacts.push(artifact_record(
                &output.name,
                &output.output_type,
                source_path,
            )?);
        }
        artifacts
    };
    Ok(NodeExecutionResult {
        status: NodeStatus::Success,
        artifacts,
        message: Some("notification delivered".to_string()),
        metadata: delivery,
        attempts: Vec::new(),
    })
}

fn notification_title(node: &WorkflowNode) -> String {
    node.extra
        .get("title")
        .or_else(|| node.extra.get("cardTitle"))
        .or_else(|| node.extra.get("card_title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Bifrost AI Workflow")
        .to_string()
}

fn notification_body(node: &WorkflowNode, input_snapshot: &Value) -> String {
    if let Some(message) = node.extra.get("message") {
        if let Some(text) = message
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return text.to_string();
        }
        if let Some(text) = message
            .get("markdown")
            .or_else(|| message.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return text.to_string();
        }
    }
    if let Some(text) = node
        .extra
        .get("markdown")
        .or_else(|| node.extra.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return text.to_string();
    }
    format!(
        "Workflow notification node `{}` completed.\n\n```json\n{}\n```",
        node.id,
        serde_json::to_string_pretty(input_snapshot).unwrap_or_default()
    )
}

fn notification_channel_requires_im(channel: &Value) -> bool {
    let channel_type = channel
        .get("type")
        .or_else(|| channel.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(channel_type, "" | "local" | "in_app" | "notification_db") {
        return channel
            .get("providerId")
            .or_else(|| channel.get("provider_id"))
            .or_else(|| channel.get("provider"))
            .is_some();
    }
    true
}

async fn send_workflow_notification_im(
    channel: &Value,
    title: &str,
    body: &str,
    node_dir: &Path,
) -> Result<Value, String> {
    let provider_id = channel
        .get("providerId")
        .or_else(|| channel.get("provider_id"))
        .or_else(|| channel.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "notification IM channel requires providerId".to_string())?;
    let target_id = channel
        .get("targetId")
        .or_else(|| channel.get("target_id"))
        .or_else(|| channel.get("target"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "owner".to_string());
    let target_mode = channel
        .get("targetMode")
        .or_else(|| channel.get("target_mode"))
        .and_then(Value::as_str)
        .unwrap_or("configured_target");
    let message_kind = channel
        .get("messageType")
        .or_else(|| channel.get("message_type"))
        .and_then(Value::as_str)
        .unwrap_or("markdown");
    let args = if message_kind == "text" {
        json!({
            "provider_id": provider_id,
            "target_id": target_id,
            "target_mode": target_mode,
            "text": body,
        })
    } else {
        json!({
            "provider_id": provider_id,
            "target_id": target_id,
            "target_mode": target_mode,
            "markdown": body,
            "card_title": title,
        })
    };
    let service = crate::handlers::im_gateway::ImGatewayService::new(&bifrost_storage::data_dir());
    let tool = crate::im_gateway::send_msg_tool::SendMsgTool::new(
        service.provider_store.clone(),
        service.target_store.clone(),
        service.message_log_store.clone(),
        service.connection_manager.clone(),
        crate::im_gateway::send_msg_tool::SendMsgToolContext::default(),
    );
    let result = tool
        .execute(
            &serde_json::to_string(&args).map_err(|error| error.to_string())?,
            node_dir,
        )
        .await;
    if !result.success {
        return Err(format!(
            "notification IM delivery failed: {}",
            result.output
        ));
    }
    serde_json::from_str(&result.output).map_err(|error| error.to_string())
}

async fn run_external_runner(
    runner_id: &str,
    node: &WorkflowNode,
    prompt: String,
    work_dir: &Path,
    timeout_ms: u64,
) -> Result<crate::im_gateway::external_cli::ExternalCliRunResult, String> {
    let (adapter, adapter_config, inject_bifrost_tools, skill_paths) = if runner_id == "mock" {
        (
            "mock".to_string(),
            crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_delta\",\"delta\":\"workflow runner working\"}' '{\"type\":\"assistant_final\",\"content\":\"# Mock Workflow Runner Report\\n\\nWorkflow runner executed successfully.\"}'".to_string(),
                ],
                timeout_secs: Some((timeout_ms / 1000).max(1)),
                ..Default::default()
            },
            false,
            Vec::new(),
        )
    } else {
        let config_store = crate::im_gateway::external_cli::ExternalCliConfigStore::new(
            &bifrost_storage::data_dir(),
        );
        let config = config_store.load();
        let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
            &config,
            None,
            Some(runner_id),
        );
        if !effective.settings.enabled {
            return Err(format!("workflow runner `{runner_id}` is not enabled"));
        }
        (
            effective.settings.adapter,
            effective.settings.adapter_config,
            effective.settings.inject_bifrost_tools,
            effective.settings.skill_paths,
        )
    };
    let operation = if adapter == "codex" { "run" } else { "send" }.to_string();
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        message: prompt,
        operation,
        params: Value::Null,
        provider_id: None,
        runner_id: Some(runner_id.to_string()),
        session_key: node
            .extra
            .get("sessionKey")
            .or_else(|| node.extra.get("session_key"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(format!("workflow:{}", node.id))),
        runtime: "external_cli".to_string(),
        adapter,
        work_dir: Some(work_dir.to_path_buf()),
        instructions: node
            .extra
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_string),
        adapter_config,
        allow_work_dirs: vec![work_dir.to_string_lossy().to_string()],
        inject_bifrost_tools,
        skill_paths,
    };
    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        bifrost_storage::data_dir().join("im_gateway/runs"),
    );
    let run = tokio::time::timeout(Duration::from_millis(timeout_ms), runtime.run(request))
        .await
        .map_err(|_| format!("runner `{runner_id}` timed out after {timeout_ms}ms"))??;
    if run.status != crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded {
        return Err(format!(
            "runner `{runner_id}` failed with status {:?}: {}",
            run.status, run.response
        ));
    }
    Ok(run)
}

fn build_runner_prompt(
    workflow: &WorkflowDocument,
    node: &WorkflowNode,
    input_snapshot: &Value,
) -> String {
    let mut prompt = String::new();
    if let Some(base) = node.prompt.as_deref() {
        prompt.push_str(base);
        prompt.push_str("\n\n");
    }
    prompt.push_str("You are executing a Bifrost AI Workflow runner node.\n");
    prompt.push_str(&format!("Workflow: {}\n", workflow.metadata.id));
    prompt.push_str(&format!("Node: {}\n\n", node.id));
    prompt.push_str("Use only the declared effective inputs below. Write the requested report as the final answer.\n\n");
    prompt.push_str("```json\n");
    prompt.push_str(&serde_json::to_string_pretty(input_snapshot).unwrap_or_default());
    prompt.push_str("\n```\n");
    prompt
}

fn workflow_input_string(
    node: &WorkflowNode,
    workflow_inputs: &Value,
    input_name: &str,
) -> Option<String> {
    node.inputs.iter().find_map(|input| {
        if input.name != input_name {
            return None;
        }
        let source = &input.source;
        if source.get("type").and_then(Value::as_str) != Some("workflow_input") {
            return None;
        }
        let name = source.get("name").and_then(Value::as_str)?;
        workflow_inputs
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn topological_nodes(workflow: &WorkflowDocument) -> Vec<&WorkflowNode> {
    let by_id = workflow
        .spec
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut indegree = workflow
        .spec
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<&str, Vec<&str>>::new();
    for edge in &workflow.spec.edges {
        if indegree.contains_key(edge.from.as_str()) && indegree.contains_key(edge.to.as_str()) {
            *indegree.entry(edge.to.as_str()).or_default() += 1;
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
    }
    let mut queue = indegree
        .iter()
        .filter_map(|(node_id, count)| (*count == 0).then_some(*node_id))
        .collect::<VecDeque<_>>();
    let mut ordered = Vec::new();
    while let Some(node_id) = queue.pop_front() {
        if let Some(node) = by_id.get(node_id) {
            ordered.push(*node);
        }
        if let Some(targets) = outgoing.get(node_id) {
            for target in targets {
                if let Some(count) = indegree.get_mut(target) {
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(target);
                    }
                }
            }
        }
    }
    if ordered.len() == workflow.spec.nodes.len() {
        ordered
    } else {
        workflow.spec.nodes.iter().collect()
    }
}

fn outgoing_edges(workflow: &WorkflowDocument) -> BTreeMap<String, Vec<String>> {
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for edge in &workflow.spec.edges {
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    outgoing
}

fn mark_downstream_skipped(
    node_id: &str,
    outgoing: &BTreeMap<String, Vec<String>>,
    skipped_nodes: &mut BTreeSet<String>,
) {
    if let Some(targets) = outgoing.get(node_id) {
        for target in targets {
            if skipped_nodes.insert(target.clone()) {
                mark_downstream_skipped(target, outgoing, skipped_nodes);
            }
        }
    }
}

fn node_no_update_skips_downstream(node: &WorkflowNode) -> bool {
    node.extra
        .get("noUpdatePolicy")
        .or_else(|| node.extra.get("no_update_policy"))
        .and_then(|policy| {
            policy
                .get("skipDownstream")
                .or_else(|| policy.get("skip_downstream"))
        })
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn node_retry_max_attempts(node: &WorkflowNode) -> u64 {
    node.extra
        .get("retryStrategy")
        .or_else(|| node.extra.get("retry_strategy"))
        .and_then(|strategy| {
            strategy
                .get("maxAttempts")
                .or_else(|| strategy.get("max_attempts"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 5)
}

fn final_attempt_number(result: &NodeExecutionResult) -> Option<u64> {
    result
        .attempts
        .last()
        .and_then(|attempt| attempt.get("attempt"))
        .and_then(Value::as_u64)
}

fn resolve_node_inputs(
    node: &WorkflowNode,
    workflow_inputs: &Value,
    produced_outputs: &BTreeMap<String, BTreeMap<String, WorkflowArtifactRecord>>,
) -> Value {
    let inputs = node
        .inputs
        .iter()
        .map(|input| {
            json!({
                "name": input.name,
                "source": input.source,
                "as": input.as_type,
                "resolved": resolve_input_source(&input.source, workflow_inputs, produced_outputs),
            })
        })
        .collect::<Vec<_>>();
    json!({ "nodeId": node.id, "kind": node.node_type, "inputs": inputs })
}

fn resolve_input_source(
    source: &Value,
    workflow_inputs: &Value,
    produced_outputs: &BTreeMap<String, BTreeMap<String, WorkflowArtifactRecord>>,
) -> Value {
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "workflow_input" => {
            let name = source
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({
                "kind": "workflow_input",
                "name": name,
                "value": workflow_inputs.get(name).cloned().unwrap_or(Value::Null),
            })
        }
        "node_output" => {
            let node_id = source
                .get("nodeId")
                .or_else(|| source.get("node_id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let output = source
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let artifact = produced_outputs
                .get(node_id)
                .and_then(|outputs| outputs.get(output))
                .and_then(|artifact| serde_json::to_value(artifact).ok())
                .unwrap_or(Value::Null);
            json!({ "kind": "node_output", "nodeId": node_id, "output": output, "artifact": artifact })
        }
        "literal_text" | "literal_script" => json!({
            "kind": source.get("type").and_then(Value::as_str).unwrap_or_default(),
            "content": source.get("content").cloned().unwrap_or(Value::Null),
        }),
        "file_ref" => json!({
            "kind": "file_ref",
            "path": source.get("path").cloned().unwrap_or(Value::Null),
        }),
        "artifact_query" => json!({ "kind": "artifact_query", "query": source }),
        other => json!({ "kind": other, "source": source }),
    }
}

fn materialize_json_output(
    output: &WorkflowOutput,
    node_dir: &Path,
    value: &Value,
) -> Result<PathBuf, String> {
    let path = output_path(output, node_dir);
    write_pretty_json(&path, value)?;
    Ok(path)
}

fn materialize_output(
    output: &WorkflowOutput,
    node_dir: &Path,
    body: &str,
) -> Result<PathBuf, String> {
    let path = output_path(output, node_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&path, body).map_err(|error| error.to_string())?;
    Ok(path)
}

fn output_path(output: &WorkflowOutput, node_dir: &Path) -> PathBuf {
    let relative = output.path_template.clone().unwrap_or_else(|| {
        let extension = match output.output_type.as_str() {
            "json" => "json",
            "file" | "file_set" => "txt",
            _ => "md",
        };
        format!("outputs/{}.{}", sanitize_id(&output.name), extension)
    });
    let rendered = relative
        .replace("{{run.date}}", &Local::now().format("%Y-%m-%d").to_string())
        .replace("{{node.id}}", "node");
    node_dir.join(rendered)
}

fn artifact_record_from_existing(
    name: &str,
    kind: &str,
    path: &Path,
) -> Result<WorkflowArtifactRecord, String> {
    artifact_record(name, kind, path)
}

fn artifact_record(name: &str, kind: &str, path: &Path) -> Result<WorkflowArtifactRecord, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(&bytes);
    let summary = String::from_utf8_lossy(&bytes)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect::<String>();
    Ok(WorkflowArtifactRecord {
        name: name.to_string(),
        kind: kind.to_string(),
        path: path.to_string_lossy().to_string(),
        size_bytes: bytes.len() as u64,
        sha256: format!(
            "sha256:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        summary,
    })
}

fn write_pretty_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, body).map_err(|error| error.to_string())
}

fn write_events_jsonl(path: &Path, events: &[Value]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = fs::File::create(path).map_err(|error| error.to_string())?;
    for event in events {
        let line = serde_json::to_string(event).map_err(|error| error.to_string())?;
        writeln!(file, "{line}").map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_log_line(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    writeln!(file, "{} {line}", Utc::now().to_rfc3339()).map_err(|error| error.to_string())
}

fn write_log_index(
    run_dir: &Path,
    workflow: &WorkflowDocument,
    events: &[Value],
    node_states: &[Value],
) -> Result<(), String> {
    let index = json!({
        "workflowId": workflow.metadata.id,
        "workflowRevision": workflow.metadata.revision,
        "eventsPath": run_dir.join("events.jsonl").to_string_lossy().to_string(),
        "runLogPath": run_dir.join("logs").join("run.log").to_string_lossy().to_string(),
        "nodeCount": node_states.len(),
        "eventCount": events.len(),
        "nodes": node_states.iter().map(|node| json!({
            "nodeId": node.get("nodeId").cloned().unwrap_or(Value::Null),
            "status": node.get("status").cloned().unwrap_or(Value::Null),
            "inputManifestPath": node.get("inputManifestPath").cloned().unwrap_or(Value::Null),
            "outputManifestPath": node.get("outputManifestPath").cloned().unwrap_or(Value::Null),
            "attemptLogPath": node.get("attemptLogPath").cloned().unwrap_or(Value::Null),
        })).collect::<Vec<_>>(),
    });
    write_pretty_json(&run_dir.join("logs").join("index.json"), &index)
}
