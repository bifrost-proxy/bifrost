use crate::{ProxyInstance, TestCase};
use serde_json::{json, Value};
use std::net::TcpListener;
use tempfile::TempDir;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
        "ai_workflow_create_validate_preview_run",
        "Validate AI Workflow API create, validate, preview, run record, and backend persistence",
        "admin",
        || async move {
            let port = pick_unused_port()?;
            bifrost_storage::set_data_dir(
                std::env::temp_dir().join(format!("bifrost_e2e_test_{port}")),
            );
            let (_proxy, _admin_state) = ProxyInstance::start_with_admin(port, vec![], false, true)
                .await
                .map_err(|e| format!("Failed to start proxy with admin: {e}"))?;
            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .no_proxy()
                .build()
                .map_err(|e| format!("Failed to create client: {e}"))?;
            let base = format!("http://127.0.0.1:{port}/_bifrost/api/ai/workflows");
            let workflow_id = format!("e2e-daily-audio-{port}");
            let templates_response = client
                .get(format!("{base}/templates"))
                .send()
                .await
                .map_err(|e| format!("templates request failed: {e}"))?;
            assert_status(&templates_response, 200)?;
            let templates: Value = templates_response
                .json()
                .await
                .map_err(|e| format!("parse templates response: {e}"))?;
            let template = templates
                .get("templates")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find(|item| {
                        item.get("id").and_then(Value::as_str) == Some("default-asr-transcription")
                    })
                })
                .ok_or_else(|| format!("default ASR template missing: {templates}"))?;
            let template_draft =
                template
                    .get("draft")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!("default ASR template should include editable draft: {template}")
                    })?;
            if !template_draft.contains("transcribe_daily_audio")
                || !template_draft.contains("run_daily_agent")
            {
                return Err(format!(
                    "default ASR template should model transcription and Daily Agent nodes: {template_draft}"
                ));
            }
            let template_detail_response = client
                .get(format!("{base}/templates/default-asr-transcription"))
                .send()
                .await
                .map_err(|e| format!("template detail request failed: {e}"))?;
            assert_status(&template_detail_response, 200)?;
            let template_detail: Value = template_detail_response
                .json()
                .await
                .map_err(|e| format!("parse template detail response: {e}"))?;
            if template_detail
                .pointer("/template/workflow/metadata/id")
                .and_then(Value::as_str)
                != Some("default-asr-transcription")
            {
                return Err(format!(
                    "template detail should include workflow: {template_detail}"
                ));
            }
            let draft = template_draft.replace("default-asr-transcription", &workflow_id);

            let validate_response = client
                .post(format!("{base}/validate"))
                .json(&json!({ "draft": draft }))
                .send()
                .await
                .map_err(|e| format!("validate request failed: {e}"))?;
            assert_status(&validate_response, 200)?;
            let validation: Value = validate_response
                .json()
                .await
                .map_err(|e| format!("parse validation response: {e}"))?;
            if validation.get("valid").and_then(Value::as_bool) != Some(true) {
                return Err(format!("expected valid workflow, got {validation}"));
            }

            let preview_response = client
                .post(format!("{base}/preview"))
                .json(&json!({ "draft": draft }))
                .send()
                .await
                .map_err(|e| format!("preview request failed: {e}"))?;
            assert_status(&preview_response, 200)?;
            let preview: Value = preview_response
                .json()
                .await
                .map_err(|e| format!("parse preview response: {e}"))?;
            if !preview
                .get("markdown")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("transcribe_daily_audio")
            {
                return Err(format!("preview did not include DAG summary: {preview}"));
            }
            if preview
                .get("effectiveInputs")
                .and_then(Value::as_array)
                .map(Vec::len)
                != Some(1)
            {
                return Err(format!(
                    "preview did not expose runner effective inputs: {preview}"
                ));
            }

            let workflow: Value = serde_yaml::from_str(&draft)
                .map_err(|e| format!("sample workflow yaml should parse: {e}"))?;
            let create_response = client
                .post(&base)
                .json(&json!({ "workflow": workflow, "dryRun": false }))
                .send()
                .await
                .map_err(|e| format!("create request failed: {e}"))?;
            assert_status(&create_response, 201)?;
            let created: Value = create_response
                .json()
                .await
                .map_err(|e| format!("parse create response: {e}"))?;
            if created
                .pointer("/workflow/metadata/revision")
                .and_then(Value::as_u64)
                != Some(1)
            {
                return Err(format!(
                    "created workflow should start at revision 1: {created}"
                ));
            }

            let list_response = client
                .get(&base)
                .send()
                .await
                .map_err(|e| format!("list request failed: {e}"))?;
            assert_status(&list_response, 200)?;
            let list: Value = list_response
                .json()
                .await
                .map_err(|e| format!("parse list response: {e}"))?;
            if !list
                .get("workflows")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .any(|item| item.get("id").and_then(Value::as_str) == Some(workflow_id.as_str()))
            {
                return Err(format!("list did not include saved workflow: {list}"));
            }

            let audio_temp = TempDir::new().map_err(|e| format!("create temp audio dir: {e}"))?;
            let run_response = client
                .post(format!("{base}/{workflow_id}/run"))
                .json(&json!({ "inputs": { "audio_dir": audio_temp.path().to_string_lossy() } }))
                .send()
                .await
                .map_err(|e| format!("run request failed: {e}"))?;
            assert_status(&run_response, 201)?;
            let run: Value = run_response
                .json()
                .await
                .map_err(|e| format!("parse run response: {e}"))?;
            if run.pointer("/run/status").and_then(Value::as_str) != Some("success") {
                return Err(format!("run should finish with success: {run}"));
            }
            if run
                .pointer("/run/finishedAt")
                .and_then(Value::as_str)
                .is_none()
            {
                return Err(format!("run should include finishedAt: {run}"));
            }
            if run
                .pointer("/run/nodeStates")
                .and_then(Value::as_array)
                .map(Vec::len)
                != Some(2)
            {
                return Err(format!("run should record two node states: {run}"));
            }
            assert_run_trace_files(&run)?;
            assert_event_sequence(&run)?;
            assert_node_states(&run, &["no_update", "skipped"])?;
            let artifacts_dir = run
                .pointer("/run/artifactsDir")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("run response did not include artifacts dir: {run}"))?;
            for relative in [
                "run.json",
                "events.jsonl",
                "logs/run.log",
                "logs/index.json",
            ] {
                let path = std::path::Path::new(artifacts_dir).join(relative);
                if !path.is_file() {
                    return Err(format!(
                        "expected runtime trace file {} to exist",
                        path.display()
                    ));
                }
            }
            let run_id = run
                .pointer("/run/id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("run response did not include run id: {run}"))?;
            let logs_response = client
                .get(format!("{base}/{workflow_id}/runs/{run_id}"))
                .send()
                .await
                .map_err(|e| format!("logs request failed: {e}"))?;
            assert_status(&logs_response, 200)?;
            let logs: Value = logs_response
                .json()
                .await
                .map_err(|e| format!("parse logs response: {e}"))?;
            if logs.pointer("/run/status").and_then(Value::as_str) != Some("success") {
                return Err(format!(
                    "logs endpoint should return persisted success run: {logs}"
                ));
            }
            assert_event_sequence(&logs)?;

            let script_workflow_id = format!("e2e-script-real-{port}");
            let script_draft = format!(
                r#"apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: {script_workflow_id}
  name: E2E Script Real Execution
spec:
  resourcePolicy:
    default: deny
  triggers:
    - type: schedule
      enabled: true
      everyMs: 1000
      inputs:
        topic: scheduled
  inputs:
    - name: topic
      type: text
      required: true
  nodes:
    - id: write_report
      type: script
      retryStrategy:
        maxAttempts: 2
      command: "printf '# Report\\n\\ninput=' > \"$BIFROST_WORKFLOW_OUTPUT\"; cat \"$BIFROST_WORKFLOW_INPUT\" >> \"$BIFROST_WORKFLOW_OUTPUT\""
      inputs:
        - name: topic
          source:
            type: workflow_input
            name: topic
          as: text
      outputs:
        - name: report
          type: document
          pathTemplate: reports/script.md
  outputs:
    - name: final_report
      type: document
      from: write_report.outputs.report
"#
            );
            let script_workflow: Value = serde_yaml::from_str(&script_draft)
                .map_err(|e| format!("script workflow yaml should parse: {e}"))?;
            let script_create = client
                .post(&base)
                .json(&json!({ "workflow": script_workflow, "dryRun": false }))
                .send()
                .await
                .map_err(|e| format!("script create request failed: {e}"))?;
            assert_status(&script_create, 201)?;
            let script_run_response = client
                .post(format!("{base}/{script_workflow_id}/run"))
                .json(&json!({ "inputs": { "topic": "real-execution" } }))
                .send()
                .await
                .map_err(|e| format!("script run request failed: {e}"))?;
            assert_status(&script_run_response, 201)?;
            let script_run: Value = script_run_response
                .json()
                .await
                .map_err(|e| format!("parse script run response: {e}"))?;
            if script_run.pointer("/run/status").and_then(Value::as_str) != Some("success") {
                return Err(format!(
                    "script workflow should finish with success: {script_run}"
                ));
            }
            assert_run_trace_files(&script_run)?;
            assert_event_sequence(&script_run)?;
            assert_node_states(&script_run, &["success"])?;
            let report_path = script_run
                .pointer("/run/nodeStates/0/artifacts/0/path")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("script run should expose report artifact: {script_run}"))?;
            let report = std::fs::read_to_string(report_path)
                .map_err(|e| format!("read script report artifact {report_path}: {e}"))?;
            if !report.contains("real-execution") {
                return Err(format!(
                    "script report should contain runtime input: {report}"
                ));
            }

            let notification_workflow_id = format!("e2e-notification-real-{port}");
            let notification_draft = format!(
                r#"apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: {notification_workflow_id}
  name: E2E Notification Real Delivery
spec:
  resourcePolicy:
    default: deny
  inputs:
    - name: report
      type: text
      required: true
  nodes:
    - id: notify
      type: notification
      title: Workflow E2E Notification
      message: Workflow notification delivered locally.
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
"#
            );
            let notification_workflow: Value = serde_yaml::from_str(&notification_draft)
                .map_err(|e| format!("notification workflow yaml should parse: {e}"))?;
            let notification_create = client
                .post(&base)
                .json(&json!({ "workflow": notification_workflow, "dryRun": false }))
                .send()
                .await
                .map_err(|e| format!("notification create request failed: {e}"))?;
            assert_status(&notification_create, 201)?;
            let notification_run_response = client
                .post(format!("{base}/{notification_workflow_id}/run"))
                .json(&json!({ "inputs": { "report": "ready" } }))
                .send()
                .await
                .map_err(|e| format!("notification run request failed: {e}"))?;
            assert_status(&notification_run_response, 201)?;
            let notification_run: Value = notification_run_response
                .json()
                .await
                .map_err(|e| format!("parse notification run response: {e}"))?;
            if notification_run
                .pointer("/run/status")
                .and_then(Value::as_str)
                != Some("success")
            {
                return Err(format!(
                    "notification workflow should finish with success: {notification_run}"
                ));
            }
            assert_run_trace_files(&notification_run)?;
            assert_event_sequence(&notification_run)?;
            assert_node_states(&notification_run, &["success"])?;
            let local_status = notification_run
                .pointer("/run/nodeStates/0/metadata/localNotificationStatus")
                .and_then(Value::as_str);
            if local_status != Some("created") {
                return Err(format!(
                    "notification node should create a local notification receipt: {notification_run}"
                ));
            }
            let notifications_response = client
                .get(format!(
                    "http://127.0.0.1:{port}/_bifrost/api/notifications?type=ai_workflow&limit=5"
                ))
                .send()
                .await
                .map_err(|e| format!("notifications request failed: {e}"))?;
            assert_status(&notifications_response, 200)?;
            let notifications: Value = notifications_response
                .json()
                .await
                .map_err(|e| format!("parse notifications response: {e}"))?;
            if !notifications
                .get("items")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .any(|item| {
                    item.get("title").and_then(Value::as_str) == Some("Workflow E2E Notification")
                })
            {
                return Err(format!(
                    "notifications API should include Workflow local notification: {notifications}"
                ));
            }

            let schedule_response = client
                .get(format!("{base}/schedules"))
                .send()
                .await
                .map_err(|e| format!("workflow schedules request failed: {e}"))?;
            assert_status(&schedule_response, 200)?;
            let schedules: Value = schedule_response
                .json()
                .await
                .map_err(|e| format!("parse workflow schedules response: {e}"))?;
            if !schedules
                .get("schedules")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .any(|item| {
                    item.get("workflowId").and_then(Value::as_str)
                        == Some(script_workflow_id.as_str())
                })
            {
                return Err(format!(
                    "workflow schedules should include enabled script workflow: {schedules}"
                ));
            }
            let scheduled_run = wait_for_scheduled_run(&client, &base, &script_workflow_id).await?;
            if scheduled_run.pointer("/run/status").and_then(Value::as_str) != Some("success") {
                return Err(format!(
                    "scheduled workflow run should finish with success: {scheduled_run}"
                ));
            }
            assert_run_trace_files(&scheduled_run)?;
            assert_event_sequence(&scheduled_run)?;
            assert_node_states(&scheduled_run, &["success"])?;
            let scheduled_report_path = scheduled_run
                .pointer("/run/nodeStates/0/artifacts/0/path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("scheduled run should expose report artifact: {scheduled_run}")
                })?;
            let scheduled_report = std::fs::read_to_string(scheduled_report_path).map_err(|e| {
                format!("read scheduled report artifact {scheduled_report_path}: {e}")
            })?;
            if !scheduled_report.contains("scheduled") {
                return Err(format!(
                    "scheduled report should contain schedule trigger input: {scheduled_report}"
                ));
            }
            let wrong_workflow_response = client
                .get(format!("{base}/wrong-workflow/runs/{run_id}"))
                .send()
                .await
                .map_err(|e| format!("wrong workflow logs request failed: {e}"))?;
            assert_status(&wrong_workflow_response, 404)?;
            Ok(())
        },
    )]
}

async fn wait_for_scheduled_run(
    client: &reqwest::Client,
    base: &str,
    workflow_id: &str,
) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut last_schedules = Value::Null;
    while std::time::Instant::now() < deadline {
        let response = client
            .get(format!("{base}/schedules"))
            .send()
            .await
            .map_err(|e| format!("poll schedules request failed: {e}"))?;
        assert_status(&response, 200)?;
        let schedules: Value = response
            .json()
            .await
            .map_err(|e| format!("parse polled schedules response: {e}"))?;
        last_schedules = schedules.clone();
        if let Some((run_id, status)) = schedules
            .get("schedules")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find_map(|item| {
                    if item.get("workflowId").and_then(Value::as_str) != Some(workflow_id) {
                        return None;
                    }
                    let run_id = item.get("lastRunId").and_then(Value::as_str)?;
                    let status = item.get("lastStatus").and_then(Value::as_str)?;
                    Some((run_id.to_string(), status.to_string()))
                })
            })
        {
            if status != "success" {
                return Err(format!(
                    "scheduled workflow completed with unexpected status={status}: {schedules}"
                ));
            }
            let run_response = client
                .get(format!("{base}/{workflow_id}/runs/{run_id}"))
                .send()
                .await
                .map_err(|e| format!("scheduled logs request failed: {e}"))?;
            assert_status(&run_response, 200)?;
            return run_response
                .json()
                .await
                .map_err(|e| format!("parse scheduled run response: {e}"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(format!(
        "scheduled workflow did not execute within timeout; last schedules={last_schedules}"
    ))
}

fn assert_run_trace_files(run: &Value) -> Result<(), String> {
    for relative in [
        "run.json",
        "events.jsonl",
        "logs/run.log",
        "logs/index.json",
    ] {
        let artifacts_dir = run
            .pointer("/run/artifactsDir")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("run response did not include artifacts dir: {run}"))?;
        let path = std::path::Path::new(artifacts_dir).join(relative);
        if !path.is_file() {
            return Err(format!(
                "expected runtime trace file {} to exist",
                path.display()
            ));
        }
    }
    Ok(())
}

fn assert_event_sequence(run: &Value) -> Result<(), String> {
    let events = run
        .pointer("/run/events")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("run should include events: {run}"))?;
    for event_name in [
        "run_created",
        "topology_planned",
        "node_started",
        "node_finished",
        "run_finished",
    ] {
        if !events
            .iter()
            .any(|event| event.get("event").and_then(Value::as_str) == Some(event_name))
        {
            return Err(format!("run events missing {event_name}: {run}"));
        }
    }
    Ok(())
}

fn assert_node_states(run: &Value, allowed_statuses: &[&str]) -> Result<(), String> {
    let nodes = run
        .pointer("/run/nodeStates")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("run should include nodeStates: {run}"))?;
    for node in nodes {
        let status = node
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !allowed_statuses.contains(&status) {
            return Err(format!(
                "node status should be one of {allowed_statuses:?}: {node}"
            ));
        }
        for field in [
            "inputManifestPath",
            "outputManifestPath",
            "attemptLogPath",
            "stdoutPath",
            "stderrPath",
        ] {
            let path = node
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("node missing {field}: {node}"))?;
            if !std::path::Path::new(path).is_file() {
                return Err(format!("node {field} file should exist: {path}"));
            }
        }
        let artifacts = node
            .get("artifacts")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("node should include artifacts: {node}"))?;
        if status != "skipped" && artifacts.is_empty() {
            return Err(format!("node should include at least one artifact: {node}"));
        }
        for artifact in artifacts {
            let path = artifact
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("artifact should include path: {artifact}"))?;
            if !std::path::Path::new(path).is_file() {
                return Err(format!("artifact file should exist: {path}"));
            }
            if !artifact
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .starts_with("sha256:")
            {
                return Err(format!("artifact should include sha256: {artifact}"));
            }
        }
    }
    Ok(())
}

fn assert_status(response: &reqwest::Response, expected: u16) -> Result<(), String> {
    if response.status().as_u16() == expected {
        Ok(())
    } else {
        Err(format!(
            "expected HTTP {expected}, got {} for {}",
            response.status(),
            response.url()
        ))
    }
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind test port: {e}"))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read test port: {e}"))
}
