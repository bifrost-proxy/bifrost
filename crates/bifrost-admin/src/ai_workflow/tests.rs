use super::*;
use tempfile::TempDir;

fn sample_workflow() -> WorkflowDocument {
    parse_workflow_document(
        r#"
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: daily-audio
  name: Daily Audio
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: audio_dir
      type: file_set
  nodes:
    - id: transcribe
      type: asr_transcription
      noUpdatePolicy:
        skipDownstream: true
      outputs:
        - name: daily_markdown
          type: document
    - id: summarize
      type: runner
      inputs:
        - name: daily
          source:
            type: node_output
            nodeId: transcribe
            output: daily_markdown
          as: document
      outputs:
        - name: report
          type: document
          pathTemplate: reports/summary.md
  edges:
    - from: transcribe
      to: summarize
  outputs:
    - name: final_report
      type: document
      from: summarize.outputs.report
"#,
    )
    .map(normalize_workflow)
    .unwrap()
}

#[test]
fn default_asr_template_is_valid_and_editable() {
    let template = default_asr_workflow_template();
    assert_eq!(template.id, DEFAULT_ASR_WORKFLOW_TEMPLATE_ID);
    assert!(template.draft.contains("audio_dir"));
    assert!(template.draft.contains("focus_topics"));
    let report = validate_workflow(&template.workflow);
    assert!(report.valid, "template diagnostics: {report:?}");
    assert_eq!(
        template.workflow.metadata.id,
        DEFAULT_ASR_WORKFLOW_TEMPLATE_ID
    );
    assert_eq!(template.workflow.spec.nodes.len(), 2);
    assert!(template
        .workflow
        .spec
        .nodes
        .iter()
        .any(|node| node.id == "transcribe_daily_audio"
            && node.node_type == "asr_transcription"
            && node.extra.contains_key("noUpdatePolicy")));
    assert!(template.workflow.spec.nodes.iter().any(|node| {
        node.id == "run_daily_agent"
            && node.node_type == "runner"
            && node.extra.get("runner").and_then(Value::as_str) == Some("codex")
            && node.inputs.iter().any(|input| {
                input.source.get("nodeId").and_then(Value::as_str) == Some("transcribe_daily_audio")
            })
    }));
}

#[test]
fn runner_without_explicit_id_uses_external_cli_default_runner() {
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let config_store = crate::im_gateway::external_cli::ExternalCliConfigStore::new(temp.path());
    let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
        default_runner_id: "workflow-real-runner".to_string(),
        ..Default::default()
    };
    config.runners.insert(
        "workflow-real-runner".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: true,
            adapter: "mock".to_string(),
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"real default runner\"}'".to_string(),
                ],
                timeout_secs: Some(10),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    config_store.save(config).unwrap();
    let workflow = parse_workflow_document(
        r#"
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: default-runner
  name: Default Runner
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: topic
      type: text
  nodes:
    - id: summarize
      type: runner
      inputs:
        - name: topic
          source:
            type: workflow_input
            name: topic
          as: text
      outputs:
        - name: report
          type: document
  outputs:
    - name: final_report
      type: document
      from: summarize.outputs.report
"#,
    )
    .map(normalize_workflow)
    .unwrap();
    let store = WorkflowStore::new(temp.path().join("agent/workflows"));
    store.save(workflow, None).unwrap();
    let run = store
        .create_run("default-runner", json!({ "topic": "release" }))
        .unwrap();
    let node = run.node_states.first().unwrap();
    assert_eq!(
        node.pointer("/metadata/runnerId").and_then(Value::as_str),
        Some("workflow-real-runner")
    );
    assert_ne!(
        node.pointer("/metadata/runnerId").and_then(Value::as_str),
        Some("mock")
    );
}

#[test]
fn validates_explicit_runner_inputs_and_dag() {
    let workflow = sample_workflow();
    let report = validate_workflow(&workflow);
    assert!(report.valid, "unexpected errors: {:?}", report.errors);
    let preview = preview_workflow(&workflow);
    assert_eq!(preview.effective_inputs.len(), 1);
    assert!(preview.markdown.contains("transcribe"));
    assert_eq!(preview.react_flow.nodes.len(), 2);
}

#[test]
fn rejects_implicit_runner_inputs_and_unsafe_paths() {
    let mut workflow = sample_workflow();
    workflow.spec.nodes[1].inputs.clear();
    workflow.spec.nodes[1].outputs[0].path_template = Some("..\\escape.md".to_string());
    let report = validate_workflow(&workflow);
    let codes = report
        .errors
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"runner_requires_explicit_inputs"));
    assert!(codes.contains(&"unsafe_path_template"));
}

#[test]
fn rejects_workflow_ids_that_are_not_url_and_file_safe() {
    let mut workflow = sample_workflow();
    workflow.metadata.id = "daily/audio".to_string();
    let report = validate_workflow(&workflow);
    let codes = report
        .errors
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"invalid_metadata_id"));
}

#[test]
fn rejects_cycles_and_missing_outputs() {
    let mut workflow = sample_workflow();
    workflow.spec.edges.push(WorkflowEdge {
        from: "summarize".to_string(),
        to: "transcribe".to_string(),
        extra: BTreeMap::new(),
    });
    workflow.spec.nodes[1].inputs[0].source["output"] = Value::String("missing".to_string());
    let report = validate_workflow(&workflow);
    let codes = report
        .errors
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"dag_cycle"));
    assert!(codes.contains(&"node_output_not_found"));
}

fn store_persists_definition_and_run_record_impl() {
    let temp = TempDir::new().unwrap();
    let store = WorkflowStore::new(temp.path().join("agent/workflows"));
    let saved = store.save(sample_workflow(), None).unwrap();
    assert_eq!(saved.metadata.revision, 1);
    let loaded = store.get("daily-audio").unwrap();
    assert_eq!(loaded.metadata.id, "daily-audio");
    let audio_dir = temp.path().join("audio");
    fs::create_dir_all(&audio_dir).unwrap();
    let run = store
        .create_run(
            "daily-audio",
            json!({ "audio_dir": audio_dir.to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(run.status, "success");
    assert!(run.finished_at.is_some());
    assert_eq!(run.node_states.len(), 2);
    assert_eq!(
        run.events
            .iter()
            .filter_map(|event| event.get("event").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        vec![
            "run_created",
            "topology_planned",
            "node_started",
            "node_finished",
            "node_started",
            "node_finished",
            "run_finished",
        ]
    );
    let run_dir = PathBuf::from(&run.artifacts_dir);
    assert!(run_dir.join("run.json").is_file());
    assert!(run_dir.join("events.jsonl").is_file());
    assert!(run_dir.join("logs/run.log").is_file());
    assert!(run_dir.join("logs/index.json").is_file());
    for node_id in ["transcribe", "summarize"] {
        let node_dir = run_dir.join("nodes").join(node_id);
        assert!(node_dir.join("input_manifest.json").is_file());
        assert!(node_dir.join("output_manifest.json").is_file());
        assert!(node_dir.join("attempts/1/attempt.json").is_file());
        assert!(node_dir.join("attempts/1/stdout.log").is_file());
        assert!(node_dir.join("attempts/1/stderr.log").is_file());
    }
    let summarize = run
        .node_states
        .iter()
        .find(|state| state.get("nodeId").and_then(Value::as_str) == Some("summarize"))
        .unwrap();
    assert_eq!(
        summarize.get("status").and_then(Value::as_str),
        Some("skipped")
    );
    let artifacts = summarize
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(artifacts.len(), 0);
    let input_manifest =
        fs::read_to_string(run_dir.join("nodes/summarize/input_manifest.json")).unwrap();
    assert!(input_manifest.contains("daily_markdown"));
    let run_log = fs::read_to_string(run_dir.join("logs/run.log")).unwrap();
    assert!(run_log.contains("node transcribe"));
    assert!(run_log.contains("finished status=no_update"));
    assert!(store.get_run(&run.id).is_ok());
}

fn script_node_retries_and_preserves_attempt_logs_impl() {
    let temp = TempDir::new().unwrap();
    let marker = temp.path().join("retry-marker");
    let command = format!(
        "if [ ! -f {marker:?} ]; then touch {marker:?}; echo first-failure >&2; exit 7; fi; printf retry-ok > \"$BIFROST_WORKFLOW_OUTPUT\""
    );
    let workflow = parse_workflow_document(&format!(
        r#"
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: retry-script
  name: Retry Script
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: topic
      type: text
  nodes:
    - id: write_report
      type: script
      retryStrategy:
        maxAttempts: 2
      command: {command:?}
      inputs:
        - name: topic
          source:
            type: workflow_input
            name: topic
          as: text
      outputs:
        - name: report
          type: document
  outputs:
    - name: final_report
      type: document
      from: write_report.outputs.report
"#
    ))
    .map(normalize_workflow)
    .unwrap();
    let store = WorkflowStore::new(temp.path().join("agent/workflows"));
    store.save(workflow, None).unwrap();
    let run = store
        .create_run("retry-script", json!({ "topic": "release" }))
        .unwrap();

    assert_eq!(run.status, "success");
    let node = run.node_states.first().unwrap();
    assert_eq!(node.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(node.get("attempt").and_then(Value::as_u64), Some(2));
    assert_eq!(
        node.get("attempts").and_then(Value::as_array).map(Vec::len),
        Some(2)
    );
    let run_dir = PathBuf::from(&run.artifacts_dir);
    assert!(run_dir
        .join("nodes/write_report/attempts/1/stderr.log")
        .is_file());
    assert!(run_dir
        .join("nodes/write_report/attempts/2/stdout.log")
        .is_file());
    let report = fs::read_to_string(
        node.pointer("/artifacts/0/path")
            .and_then(Value::as_str)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(report, "retry-ok");
}

fn runner_node_uses_fallback_runner_and_records_primary_error_impl() {
    let temp = TempDir::new().unwrap();
    let workflow = parse_workflow_document(
        r#"
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: fallback-runner
  name: Fallback Runner
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: topic
      type: text
  nodes:
    - id: summarize
      type: runner
      runner: missing-runner
      fallbackRunner: mock
      prompt: Summarize the declared topic.
      inputs:
        - name: topic
          source:
            type: workflow_input
            name: topic
          as: text
      outputs:
        - name: report
          type: document
  outputs:
    - name: final_report
      type: document
      from: summarize.outputs.report
"#,
    )
    .map(normalize_workflow)
    .unwrap();
    let store = WorkflowStore::new(temp.path().join("agent/workflows"));
    store.save(workflow, None).unwrap();
    let run = store
        .create_run("fallback-runner", json!({ "topic": "release" }))
        .unwrap();

    assert_eq!(run.status, "success");
    let node = run.node_states.first().unwrap();
    assert_eq!(node.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(
        node.pointer("/metadata/runnerId").and_then(Value::as_str),
        Some("mock")
    );
    assert_eq!(
        node.pointer("/metadata/primaryRunnerId")
            .and_then(Value::as_str),
        Some("missing-runner")
    );
    assert!(node
        .pointer("/metadata/fallbackError")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .contains("not enabled"));
}

fn notification_node_creates_local_notification_and_receipt_impl() {
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let workflow = parse_workflow_document(
        r#"
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: notify-local
  name: Notify Local
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: report
      type: text
  nodes:
    - id: notify
      type: notification
      title: Workflow Ready
      message: Report is ready.
      channel:
        type: local
      inputs:
        - name: report
          source:
            type: workflow_input
            name: report
          as: text
      outputs:
        - name: receipt
          type: json
  outputs:
    - name: receipt
      type: json
      from: notify.outputs.receipt
"#,
    )
    .map(normalize_workflow)
    .unwrap();
    let store = WorkflowStore::new(temp.path().join("agent/workflows"));
    store.save(workflow, None).unwrap();
    let run = store
        .create_run("notify-local", json!({ "report": "done" }))
        .unwrap();

    assert_eq!(run.status, "success");
    let node = run.node_states.first().unwrap();
    assert_eq!(node.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(
        node.pointer("/metadata/localNotificationStatus")
            .and_then(Value::as_str),
        Some("created")
    );
    assert!(
        node.pointer("/metadata/localNotificationId")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            > 0
    );
    assert!(PathBuf::from(&run.artifacts_dir)
        .join("nodes/notify/notification_receipt.json")
        .is_file());
    let notifications =
        crate::notification_db::list_notifications(Some("ai_workflow"), Some("unread"), 10, 0)
            .unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].title, "Workflow Ready");
}

struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn set_data_dir(path: &Path) -> Self {
        let previous = std::env::var("BIFROST_DATA_DIR").ok();
        std::env::set_var("BIFROST_DATA_DIR", path);
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => std::env::set_var("BIFROST_DATA_DIR", previous),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
}

#[test]
fn workflow_runtime_executes_real_nodes_with_isolated_environment() {
    store_persists_definition_and_run_record_impl();
    script_node_retries_and_preserves_attempt_logs_impl();
    runner_node_uses_fallback_runner_and_records_primary_error_impl();
    notification_node_creates_local_notification_and_receipt_impl();
}
