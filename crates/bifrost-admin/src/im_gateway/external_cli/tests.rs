use super::*;
use std::sync::{Mutex, OnceLock};

static EXTERNAL_CLI_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn external_cli_env_guard() -> std::sync::MutexGuard<'static, ()> {
    EXTERNAL_CLI_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn delayed_final_command(content: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec![
                "/C".to_string(),
                format!(
                    "ping -n 3 127.0.0.1 >nul & echo {{\"type\":\"assistant_final\",\"content\":\"{content}\"}}"
                ),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                format!(
                    "sleep 2; printf '%s\\n' '{{\"type\":\"assistant_final\",\"content\":\"{content}\"}}'"
                ),
            ],
        )
    }
}

#[cfg(unix)]
#[test]
fn terminate_process_group_force_kills_sigterm_ignoring_process() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;
    use std::time::Instant;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("trap '' TERM; while true; do sleep 1; done")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn SIGTERM-ignoring process");
    let pid = child.id();

    terminate_process_group(pid).expect("terminate process group");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                panic!("process ignored SIGTERM and was not force-killed");
            }
            Err(error) => panic!("wait SIGTERM-ignoring process: {error}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn signal_process_group_reports_not_found_after_child_exits() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn short-lived process");
    let pid = child.id();
    child.wait().expect("wait short-lived process");

    assert_eq!(
        signal_process_group_or_child(pid, nix::sys::signal::Signal::SIGTERM)
            .expect("signal missing process group"),
        ProcessSignalOutcome::NotFound
    );
    terminate_process_group(pid).expect("terminate missing process group is a no-op");
}

#[cfg(unix)]
#[tokio::test]
async fn stale_external_worker_entry_does_not_kill_pid_when_stop_receiver_is_gone() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn protected external worker process");
    let pid = child.id();
    let (control_tx, control_rx) = tokio::sync::mpsc::channel(1);
    drop(control_rx);
    ACTIVE_WORKER_SESSIONS.insert(
        "stale-external-worker".to_string(),
        ExternalCliWorkerControlHandle { pid, control_tx },
    );

    assert!(!request_worker_session_stop("stale-external-worker").await);
    assert!(
        child
            .try_wait()
            .expect("poll protected external worker process")
            .is_none(),
        "stale external worker registry entry must not terminate a pid when stop receiver is gone"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[tokio::test]
async fn acknowledged_external_worker_stop_does_not_kill_pid() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as StdProcessCommand;

    let mut child = StdProcessCommand::new("sh")
        .arg("-c")
        .arg("sleep 5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn protected external worker process");
    let pid = child.id();
    let (control_tx, mut control_rx) = tokio::sync::mpsc::channel(1);
    ACTIVE_WORKER_SESSIONS.insert(
        "acked-external-worker".to_string(),
        ExternalCliWorkerControlHandle { pid, control_tx },
    );
    let ack_task = tokio::spawn(async move {
        if let Some(ExternalCliWorkerControlRequest::Stop { ack_tx }) = control_rx.recv().await {
            let _ = ack_tx.send(());
        }
    });

    assert!(request_worker_session_stop("acked-external-worker").await);
    ack_task.await.expect("ack task");
    assert!(
        child
            .try_wait()
            .expect("poll protected external worker process")
            .is_none(),
        "acknowledged external worker stop must not be followed by pid termination"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn external_cli_adapter_parser_maps_progress_events() {
    let stdout = r#"{"type":"run_started","content":"start"}
{"type":"assistant_delta","delta":"hello"}
not json
{"type":"tool_started","tool_name":"exec_command","content":"running"}
{"type":"assistant_final","content":"done"}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(events[1].content, "hello");
    assert_eq!(events[2].title.as_deref(), Some("exec_command"));
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
}

#[test]
fn codex_cli_parser_maps_real_jsonl_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"deprecated config warning"}}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_REAL_CODEX_OK"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[1].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[1].content, "deprecated config warning");
    assert_eq!(events[2].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[3].content, "BIFROST_REAL_CODEX_OK");
    assert_eq!(
        events[4].event_type,
        ExternalCliProgressEventType::RunFinished
    );
}

#[test]
fn codex_cli_parser_maps_real_todo_list_events_to_plan_updates() {
    let stdout = r#"{"type":"item.started","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":false},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}
{"type":"item.updated","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}
{"type":"item.completed","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false},{"text":"verify UI","completed":false}]}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .all(|event| event.event_type == ExternalCliProgressEventType::PlanUpdated));
    assert!(events.iter().all(|event| event.title.is_none()));
    let initial_steps = external_progress_plan_steps(&events[0]);
    assert_eq!(initial_steps.len(), 3);
    assert_eq!(initial_steps[0].step, "inspect output");
    assert_eq!(initial_steps[0].status, PlanStepStatus::Pending);
    let updated_steps = external_progress_plan_steps(&events[1]);
    assert_eq!(updated_steps[0].status, PlanStepStatus::Completed);
    assert_eq!(
        updated_steps[1].status,
        PlanStepStatus::Pending,
        "Codex todo_list currently exposes completed=true/false, not in_progress"
    );
}

#[test]
fn generic_plan_updated_parser_accepts_status_fields() {
    let events = parse_progress_events(
        r#"{"type":"plan_updated","title":"Runner plan","items":[{"text":"inspect","status":"completed"},{"text":"map","status":"in_progress"},{"text":"verify","status":"pending"}]}"#,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::PlanUpdated
    );
    assert_eq!(events[0].title.as_deref(), Some("Runner plan"));
    let steps = external_progress_plan_steps(&events[0]);
    assert_eq!(steps[0].status, PlanStepStatus::Completed);
    assert_eq!(steps[1].status, PlanStepStatus::InProgress);
    assert_eq!(steps[2].status, PlanStepStatus::Pending);
}

#[test]
fn codex_cli_parser_maps_real_command_execution_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"019ea049-6138-7303-ab6e-dacccbd437a7"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"/Users/eden/work/github/bifrost-traex-runner\n","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_CODEX_REALTIME_DIRECT_OK"}}
{"type":"turn.completed","usage":{"input_tokens":59589,"cached_input_tokens":6912,"output_tokens":221,"reasoning_output_tokens":156}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 6);
    assert_eq!(
        events[2].event_type,
        ExternalCliProgressEventType::ToolStarted
    );
    assert_eq!(events[2].title.as_deref(), Some("exec_command"));
    assert_eq!(events[2].content, "/bin/zsh -lc pwd");
    assert_eq!(
        events[2]
            .raw
            .get("arguments")
            .and_then(|value| value.get("command"))
            .and_then(serde_json::Value::as_str),
        Some("/bin/zsh -lc pwd")
    );
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[3].title.as_deref(), Some("exec_command"));
    assert_eq!(
        events[3].content,
        "/Users/eden/work/github/bifrost-traex-runner\n"
    );
    assert_eq!(
        events[3]
            .raw
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        events[4].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[4].content, "BIFROST_CODEX_REALTIME_DIRECT_OK");
    assert_eq!(
        events[5].event_type,
        ExternalCliProgressEventType::RunFinished
    );
}

#[test]
fn file_change_detail_counts_added_deleted_and_modified_lines() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "type": "fileChange",
        "changes": [
            {
                "path": "src/updated.rs",
                "kind": {"type": "update", "move_path": null},
                "diff": "--- a/src/updated.rs\n+++ b/src/updated.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n context\n"
            },
            {
                "path": "src/new.rs",
                "kind": {"type": "add"},
                "diff": "--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n+one\n+two\n"
            },
            {
                "path": "src/old.rs",
                "kind": {"type": "delete"},
                "diff": "--- a/src/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n"
            }
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: src/updated.rs (修改 1 行 · 新增 1 行)"));
    assert!(detail.contains("file: src/new.rs (新增 2 行)"));
    assert!(detail.contains("file: src/old.rs (删除 2 行)"));
    assert!(!detail.contains("修改 2 行 · 新增 1 行 · 删除 1 行"));
}

#[test]
fn file_change_detail_keeps_action_when_diff_has_no_changed_lines() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "path": "src/renamed.rs",
        "status": "completed",
        "kind": {"type": "move", "move_path": "src/original.rs"}
    }))
    .expect("file change detail");

    assert_eq!(detail, "file: src/renamed.rs (移动)");
}

#[test]
fn file_change_detail_counts_plain_added_and_deleted_content() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "type": "fileChange",
        "changes": [
            {
                "path": "/workspace/src/new.rs",
                "kind": {"type": "add"},
                "diff": "first\n+literal content\n-third\n"
            },
            {
                "path": "/workspace/src/old.rs",
                "kind": {"type": "delete"},
                "diff": "first\n\nthird\n"
            }
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: /workspace/src/new.rs (新增 3 行)"));
    assert!(detail.contains("file: /workspace/src/old.rs (删除 3 行)"));
}

#[test]
fn file_change_detail_uses_workspace_relative_paths_and_indents_every_diff_line() {
    let detail = file_change_detail_from_value_with_work_dir(
        &serde_json::json!({
            "type": "fileChange",
            "changes": [{
                "path": "/workspace/project/target/demo.txt",
                "kind": {"type": "add"},
                "diff": "first\nsecond\nthird\n"
            }]
        }),
        Some(Path::new("/workspace/project")),
    )
    .expect("file change detail");

    assert_eq!(
        detail,
        "changes:\n- file: target/demo.txt (新增 3 行)\n  first\n  second\n  third"
    );
}

#[test]
fn file_change_detail_preserves_paths_outside_workspace() {
    let detail = file_change_detail_from_value_with_work_dir(
        &serde_json::json!({
            "type": "fileChange",
            "changes": [{
                "path": "/shared/demo.txt",
                "kind": {"type": "add"},
                "diff": "one\n"
            }]
        }),
        Some(Path::new("/workspace/project")),
    )
    .expect("file change detail");

    assert!(detail.contains("file: /shared/demo.txt (新增 1 行)"));
}

#[test]
fn file_change_detail_preserves_unknown_actions_and_path_only_changes() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "changes": [
            {"path": "scripts/tool.sh", "action": "chmod"},
            {"path": "assets/empty.txt"}
        ]
    }))
    .expect("file change detail");

    assert!(detail.contains("file: scripts/tool.sh (chmod)"));
    assert!(detail.contains("file: assets/empty.txt"));
}

#[test]
fn file_change_line_stats_do_not_pair_changes_across_hunks() {
    let diff = "@@ -1 +1 @@\n-old\n context\n@@ -8 +8,2 @@\n context\n+new\n";

    assert_eq!(unified_diff_line_stats(diff), (1, 1, 0));
}

#[test]
fn external_progress_result_prefers_file_detail_and_keeps_structured_fallbacks() {
    let nested_file_change = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: "stale absolute-path detail".to_string(),
        title: Some("fileChange".to_string()),
        raw: serde_json::json!({
            "params": {
                "item": {
                    "type": "fileChange",
                    "path": "/workspace/project/src/main.rs",
                    "kind": {"type": "update"}
                }
            }
        }),
    };
    assert_eq!(
        external_progress_result_text(&nested_file_change, Some(Path::new("/workspace/project"))),
        "file: src/main.rs (修改)"
    );

    let detail_free_file_change = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: String::new(),
        title: Some("fileChange".to_string()),
        raw: serde_json::json!({"item": {"type": "fileChange"}}),
    };
    assert!(
        external_progress_result_text(&detail_free_file_change, None)
            .contains(r#""type": "fileChange""#)
    );

    let empty_regular_tool = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: String::new(),
        title: Some("exec_command".to_string()),
        raw: serde_json::json!({}),
    };
    assert!(external_progress_result_text(&empty_regular_tool, None).is_empty());
}

#[test]
fn file_change_detail_covers_top_level_diff_and_header_only_unified_diff() {
    let detail = file_change_detail_from_value(&serde_json::json!({
        "diff": "--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n"
    }))
    .expect("top-level diff detail");
    assert_eq!(
        detail,
        "diff:\n  --- a/src/main.rs\n  +++ b/src/main.rs\n  -old\n  +new"
    );
    assert!(looks_like_unified_diff(
        "--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n"
    ));

    assert_eq!(
        format_file_change_path("src/main.rs", Some("修改"), Some("context only"), None),
        "file: src/main.rs (修改)"
    );
}

#[test]
fn traex_cli_parser_maps_real_jsonl_events() {
    let stdout = r#"{"type":"thread.started","thread_id":"019e9f78-traex"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"model rerouted"}}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"BIFROST_TRAEX_OK"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[0].content, "019e9f78-traex");
    assert_eq!(events[1].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[2].event_type, ExternalCliProgressEventType::Status);
    assert_eq!(events[2].content, "model rerouted");
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[3].content, "BIFROST_TRAEX_OK");
}

#[test]
fn claude_code_parser_maps_stream_json_events() {
    let stdout = r#"{"type":"system","subtype":"init","session_id":"claude-session-1"}
{"type":"assistant","message":{"content":[{"type":"text","text":"BIFROST_CLAUDE_CODE_OK"}],"usage":{"input_tokens":10,"output_tokens":4}}}
{"type":"result","subtype":"success","is_error":false,"result":"BIFROST_CLAUDE_CODE_OK","session_id":"claude-session-1","usage":{"input_tokens":10,"output_tokens":4}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::RunStarted
    );
    assert_eq!(events[0].content, "claude-session-1");
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[1].content, "BIFROST_CLAUDE_CODE_OK");
    assert_eq!(
        events[2].event_type,
        ExternalCliProgressEventType::RunFinished
    );

    let mut metadata = std::collections::BTreeMap::new();
    append_external_cli_metadata(CLAUDE_CODE_ADAPTER, &events, &mut metadata);

    assert_eq!(
        metadata.get("threadId").map(String::as_str),
        Some("claude-session-1")
    );
    assert_eq!(
        metadata.get("usageInputTokens").map(String::as_str),
        Some("10")
    );
    assert_eq!(
        metadata.get("usageOutputTokens").map(String::as_str),
        Some("4")
    );
    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("14")
    );
}

#[test]
fn claude_code_parser_maps_tool_use_and_tool_result() {
    let stdout = r#"{"type":"system","subtype":"init","session_id":"claude-session-tool"}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tooluse_1","name":"Bash","input":{"command":"pwd"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"tooluse_1","type":"tool_result","content":"/Users/bytedance/project/bifrost","is_error":false}]},"tool_use_result":{"stdout":"/Users/bytedance/project/bifrost","stderr":"","interrupted":false}}
{"type":"assistant","message":{"content":[{"type":"text","text":"BIFROST_CLAUDE_TOOL_OK"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"BIFROST_CLAUDE_TOOL_OK","session_id":"claude-session-tool","usage":{"input_tokens":20,"output_tokens":6}}"#;

    let events = parse_progress_events(stdout);

    assert_eq!(
        events
            .iter()
            .map(|event| &event.event_type)
            .collect::<Vec<_>>(),
        vec![
            &ExternalCliProgressEventType::RunStarted,
            &ExternalCliProgressEventType::ToolStarted,
            &ExternalCliProgressEventType::ToolFinished,
            &ExternalCliProgressEventType::AssistantFinal,
            &ExternalCliProgressEventType::RunFinished,
        ]
    );
    assert_eq!(events[1].title.as_deref(), Some("Bash"));
    assert_eq!(events[1].content, "pwd");
    assert_eq!(events[2].title.as_deref(), Some("Bash"));
    assert_eq!(events[2].content, "/Users/bytedance/project/bifrost");
    assert_eq!(external_progress_arguments_text(&events[2]), "pwd");
    assert_eq!(
        events[2]
            .raw
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    let context = ExternalCliProgressStatusContext::new(
        Some(DEFAULT_CLAUDE_CODE_RUNNER_ID),
        None,
        None,
        None,
        None,
        None,
    );
    let turn_started =
        external_progress_to_agent_turn_event("session", CLAUDE_CODE_ADAPTER, context, &events[1])
            .expect("tool started event");
    assert!(matches!(
        turn_started,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
    ));
    let turn_finished =
        external_progress_to_agent_turn_event("session", CLAUDE_CODE_ADAPTER, context, &events[2])
            .expect("tool finished event");
    match turn_finished {
        bifrost_agent::AgentTurnProgressEvent::ToolFinished { log, .. } => {
            assert_eq!(log.tool_name, "Bash");
            assert_eq!(log.arguments, "pwd");
            assert_eq!(log.result, "/Users/bytedance/project/bifrost");
            assert!(log.success);
        }
        other => panic!("expected tool finished event, got {other:?}"),
    }
}

#[test]
fn codex_adapter_builds_exec_command_with_prompt_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: Some("be concise".to_string()),
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile: Some("bifrost".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("never".to_string()),
            search: Some(true),
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "codex");
    assert_eq!(spec.args[0], "exec");
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(&spec.args, "--profile", "bifrost"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec.args.contains(&"--ask-for-approval".to_string()));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_defaults_to_danger_full_access_for_headless_runs() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_respects_explicit_sandbox_without_danger_full_access() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            sandbox: Some("workspace-write".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_builds_exec_command_with_prompt_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: Some("be concise".to_string()),
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            profile: Some("bifrost".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            permission_mode: Some("auto".to_string()),
            skip_git_repo_check: Some(true),
            ignore_user_config: Some(true),
            ignore_rules: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            config_overrides: vec!["shell_environment_policy.inherit=all".to_string()],
            enable_features: vec!["web_search".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "traex");
    assert_eq!(spec.timeout_secs, None);
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(spec.args.contains(&"exec".to_string()));
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert!(has_arg_pair(&spec.args, "--profile", "bifrost"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(has_arg_pair(&spec.args, "--permission-mode", "auto"));
    assert!(has_arg_pair(&spec.args, "--add-dir", "/tmp/extra"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "shell_environment_policy.inherit=all"
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec.args.contains(&"--skip-git-repo-check".to_string()));
    assert!(spec.args.contains(&"--ignore-user-config".to_string()));
    assert!(spec.args.contains(&"--ignore-rules".to_string()));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_defaults_to_headless_full_access_for_exec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(!spec.args.contains(&"default".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_maps_default_permission_mode_to_headless_full_access() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            permission_mode: Some("default".to_string()),
            sandbox: Some("workspace-write".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(!spec.args.contains(&"default".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_respects_explicit_non_bypass_permission_mode() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            permission_mode: Some("plan".to_string()),
            sandbox: Some("workspace-write".to_string()),
            skip_git_repo_check: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--permission-mode", "plan"));
    assert!(has_arg_pair(&spec.args, "--sandbox", "workspace-write"));
    assert!(!spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn traex_adapter_builds_resume_command_from_thread_id() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello again".to_string(),
        operation: default_operation(),
        params: serde_json::json!({ "threadId": "thread-existing" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("traex".to_string()),
            profile: Some("not-supported-by-resume".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            permission_mode: Some("bypass_permissions".to_string()),
            danger_full_access: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "traex");
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(spec.args.contains(&"exec".to_string()));
    assert!(spec.args.contains(&"resume".to_string()));
    assert!(spec.args.contains(&"--json".to_string()));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(!spec.args.contains(&"--permission-mode".to_string()));
    assert!(!spec.args.contains(&"bypass_permissions".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(spec.args.contains(&"thread-existing".to_string()));
    assert!(!spec.args.contains(&"--profile".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(!spec.args.contains(&"--add-dir".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_builds_current_cli_config_flags() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile_v2: Some("team".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("never".to_string()),
            reasoning_effort: Some("high".to_string()),
            reasoning_summary: Some("auto".to_string()),
            dangerously_bypass_hook_trust: Some(true),
            strict_config: Some(true),
            skip_git_repo_check: Some(true),
            ignore_user_config: Some(true),
            ignore_rules: Some(true),
            oss: Some(true),
            local_provider: Some("ollama".to_string()),
            output_schema: Some("/tmp/schema.json".to_string()),
            color: Some("never".to_string()),
            add_dirs: vec!["/tmp/extra".to_string()],
            config_overrides: vec!["shell_environment_policy.inherit=all".to_string()],
            enable_features: vec!["web_search".to_string()],
            disable_features: vec!["legacy_mode".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--profile-v2", "team"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "shell_environment_policy.inherit=all"
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "approval_policy=\"never\""
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_summary=\"auto\""
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(has_arg_pair(&spec.args, "--disable", "legacy_mode"));
    assert!(has_arg_pair(&spec.args, "--add-dir", "/tmp/extra"));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-hook-trust".to_string()));
    assert!(spec.args.contains(&"--strict-config".to_string()));
    assert!(spec.args.contains(&"--oss".to_string()));
    assert!(has_arg_pair(&spec.args, "--local-provider", "ollama"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-schema",
        "/tmp/schema.json"
    ));
    assert!(has_arg_pair(&spec.args, "--color", "never"));
    assert!(spec.args.contains(&"--skip-git-repo-check".to_string()));
    assert!(spec.args.contains(&"--ignore-user-config".to_string()));
    assert!(spec.args.contains(&"--ignore-rules".to_string()));
    assert!(!spec.args.contains(&"--search".to_string()));
}

#[test]
fn codex_adapter_respects_configured_service_tier_override() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            config_overrides: vec!["service_tier=\"flex\"".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"flex\""
    ));
    assert!(!has_arg_pair(
        &spec.args,
        "--config",
        "service_tier=\"fast\""
    ));
}

#[test]
fn codex_adapter_maps_legacy_search_to_web_search_feature() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            search: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(!spec.args.contains(&"--search".to_string()));
}

#[test]
fn codex_adapter_danger_full_access_suppresses_sandbox() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            sandbox: Some("workspace-write".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
}

#[test]
fn codex_adapter_builds_resume_command_from_thread_id() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello again".to_string(),
        operation: default_operation(),
        params: serde_json::json!({ "threadId": "thread-existing" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            profile: Some("not-supported-by-resume".to_string()),
            model: Some("gpt-test".to_string()),
            sandbox: Some("workspace-write".to_string()),
            danger_full_access: Some(true),
            add_dirs: vec!["/tmp/extra".to_string()],
            ephemeral: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "codex");
    assert_eq!(spec.args[0], "exec");
    assert!(spec.args.contains(&"resume".to_string()));
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-test"));
    assert!(spec.args.contains(&"--ephemeral".to_string()));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(spec.args.contains(&"thread-existing".to_string()));
    assert!(!spec.args.contains(&"--profile".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert!(!spec.args.contains(&"--add-dir".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn codex_adapter_injects_work_dir_with_custom_args() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:feishu:chat-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            args: vec!["exec".to_string(), "--json".to_string(), "-".to_string()],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.args[0], "exec");
    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert_eq!(spec.work_dir.as_deref(), Some(Path::new("/tmp/work")));
}

#[test]
fn codex_adapter_applies_config_flags_to_custom_args() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("schedule:one".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: DEFAULT_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("codex".to_string()),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                "gpt-runner".to_string(),
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                "-".to_string(),
            ],
            model: Some("gpt-schedule".to_string()),
            reasoning_effort: Some("high".to_string()),
            enable_features: vec!["web_search".to_string()],
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--cd", "/tmp/work"));
    assert!(has_arg_pair(
        &spec.args,
        "--output-last-message",
        "/tmp/last.md"
    ));
    assert!(has_arg_pair(&spec.args, "--model", "gpt-schedule"));
    assert!(!has_arg_pair(&spec.args, "--model", "gpt-runner"));
    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
    assert!(has_arg_pair(&spec.args, "--enable", "web_search"));
    assert!(spec
        .args
        .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    assert!(!spec.args.contains(&"--sandbox".to_string()));
    assert_eq!(spec.args.last().map(String::as_str), Some("-"));
}

#[test]
fn claude_code_adapter_applies_session_model_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            model: Some("claude-opus-4-5-20251101".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert_eq!(spec.executable, "claude");
    assert!(spec.args.contains(&"-p".to_string()));
    assert!(has_arg_pair(&spec.args, "--input-format", "stream-json"));
    assert!(spec.args.contains(&"--replay-user-messages".to_string()));
    assert!(has_arg_pair(
        &spec.args,
        "--model",
        "claude-opus-4-5-20251101"
    ));
    assert!(spec
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn claude_code_explicit_exec_transport_keeps_text_stdin() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude-exec".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            transport: Some(ExternalCliTransport::Exec),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--input-format", "text"));
    assert!(!spec.args.contains(&"--replay-user-messages".to_string()));
}

#[test]
fn claude_code_adapter_applies_session_effort_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("session:claude".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            reasoning_effort: Some("xhigh".to_string()),
            danger_full_access: Some(true),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(&spec.args, "--effort", "xhigh"));
}

#[test]
fn traex_adapter_applies_session_effort_to_command_spec() {
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_TRAEX_RUNNER_ID.to_string()),
        session_key: Some("session:traex".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: TRAEX_ADAPTER.to_string(),
        work_dir: Some(PathBuf::from("/tmp/work")),
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            reasoning_effort: Some("high".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let spec = build_command_spec(&request, Path::new("/tmp/last.md")).unwrap();

    assert!(has_arg_pair(
        &spec.args,
        "--config",
        "model_reasoning_effort=\"high\""
    ));
}

#[tokio::test]
async fn final_response_prefers_assistant_message_over_run_finished() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_last_message = temp_dir.path().join("last.md");
    let events = parse_progress_events(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"real final"}}
{"type":"turn.completed"}"#,
    );

    let response = final_response(&missing_last_message, "raw stdout", &events)
        .await
        .unwrap();

    assert_eq!(response, "real final");
}

#[tokio::test]
async fn external_cli_runtime_runs_mock_command_and_writes_artifacts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello from api".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_delta\",\"delta\":\"working\"}' '{\"type\":\"assistant_final\",\"content\":\"mock final\"}'"
                    .to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let result = runtime.run(request).await.unwrap();

    assert_eq!(result.status, ExternalCliRunStatus::Succeeded);
    assert_eq!(result.response, "mock final");
    assert_eq!(result.events.len(), 2);
    assert!(Path::new(&result.artifacts.command_snapshot).exists());
    assert!(Path::new(&result.artifacts.normalized_events).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn external_cli_runtime_dispatches_default_claude_stream_json_transport() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let executable = temp_dir.path().join("mock-claude-runtime");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json
import sys

if "--version" in sys.argv:
    print("mock claude 1.0")
    raise SystemExit(0)

first = json.loads(sys.stdin.readline())
print(json.dumps({"type":"system","subtype":"init","session_id":"runtime-stream-session"}), flush=True)
print(json.dumps(first), flush=True)
print(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":"runtime stream final"}]},"session_id":"runtime-stream-session"}), flush=True)
print(json.dumps({"type":"result","subtype":"success","is_error":False,"result":"runtime stream final","session_id":"runtime-stream-session"}), flush=True)
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let runtime = ExternalCliRuntime::new(temp_dir.path().join("runs"));
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello stream runtime".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        session_key: Some("runtime-stream-session-key".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable.display().to_string()),
            timeout_secs: Some(5),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    assert!(ExternalCliTransport::AppServer.supports_live_guide());
    assert!(ExternalCliTransport::StreamJson.supports_live_guide());
    assert!(!ExternalCliTransport::Exec.supports_live_guide());
    let result = runtime.run(request).await.unwrap();
    assert_eq!(result.status, ExternalCliRunStatus::Succeeded);
    assert_eq!(result.response, "runtime stream final");
    assert!(result
        .events
        .iter()
        .any(|event| event.event_type == ExternalCliProgressEventType::AssistantFinal));
}

#[tokio::test]
async fn external_cli_runtime_persists_chatgpt_web_adapter_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello from daily agent".to_string(),
        operation: "unsupported-test-operation".to_string(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("web".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            timeout_secs: Some(1),
            extra: BTreeMap::from([("browser".to_string(), serde_json::json!("invalid"))]),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let result = runtime.run(request).await.unwrap();

    assert_eq!(result.status, ExternalCliRunStatus::Failed);
    assert!(result.response.contains("ChatGPT Web run failed"));
    assert!(result
        .response
        .contains("parse chatgpt_web adapter config failed"));
    assert!(Path::new(&result.artifacts.command_snapshot).exists());
    assert!(Path::new(&result.artifacts.stdout).exists());
    assert!(Path::new(&result.artifacts.stderr).exists());
    assert!(Path::new(&result.artifacts.normalized_events).exists());
    assert!(Path::new(&result.artifacts.last_message).exists());
    let stderr = tokio::fs::read_to_string(&result.artifacts.stderr)
        .await
        .unwrap();
    assert!(stderr.contains("parse chatgpt_web adapter config failed"));
    let last_message = tokio::fs::read_to_string(&result.artifacts.last_message)
        .await
        .unwrap();
    assert_eq!(last_message, result.response);
    assert!(Path::new(&result.artifacts.run_dir)
        .join("result.json")
        .exists());
    assert!(result.metadata.contains_key("failureDiagnostics"));
    assert_eq!(
        result.events[0].event_type,
        ExternalCliProgressEventType::RunFailed
    );
}

#[tokio::test]
async fn external_cli_runtime_streams_stdout_before_process_exit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello stream".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-stream-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_delta\",\"delta\":\"streaming now\"}'; sleep 1; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"stream final\"}'".to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let run = tokio::spawn(async move {
        runtime
            .run_with_progress(request, Some(progress_tx))
            .await
            .unwrap()
    });

    let first = tokio::time::timeout(Duration::from_secs(10), progress_rx.recv())
        .await
        .expect("progress event should arrive before process exit")
        .expect("progress channel open");

    assert_eq!(
        first.event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(first.content, "streaming now");
    assert!(
        !run.is_finished(),
        "mock command sleeps after first event, so runtime must still be active"
    );
    let result = run.await.unwrap();
    assert_eq!(result.response, "stream final");
    assert_eq!(result.events.len(), 2);
}

#[test]
fn external_progress_maps_to_agent_turn_progress_events() {
    let event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::AssistantDelta,
        content: "thinking out loud".to_string(),
        title: None,
        raw: serde_json::json!({"type":"assistant_delta","delta":"thinking out loud"}),
    };

    let mapped = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &event,
    )
    .expect("mapped event");

    match mapped {
        bifrost_agent::AgentTurnProgressEvent::AssistantDelta { content } => {
            assert_eq!(content, "thinking out loud");
        }
        other => panic!("unexpected mapped event: {other:?}"),
    }

    let status_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "running".to_string(),
        title: None,
        raw: serde_json::json!({"type":"status","content":"running"}),
    };
    let mapped_status = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &status_event,
    )
    .expect("mapped status event");
    match mapped_status {
        bifrost_agent::AgentTurnProgressEvent::Status(status) => {
            assert_eq!(status.runner_type.as_deref(), Some(TRAEX_ADAPTER));
            assert_eq!(status.runner_id.as_deref(), Some("traex"));
            assert_eq!(status.model.as_deref(), Some("trae-model"));
            assert_eq!(status.model_provider.as_deref(), Some("runner config"));
            assert_eq!(status.model_reasoning_effort.as_deref(), Some("high"));
            assert_eq!(status.model_reasoning_summary.as_deref(), Some("auto"));
        }
        other => panic!("unexpected mapped status event: {other:?}"),
    }

    let plan_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::PlanUpdated,
        content: "plan updated (2 steps)".to_string(),
        title: None,
        raw: serde_json::json!({
            "type": "item.updated",
            "item": {
                "type": "todo_list",
                "items": [
                    {"text": "inspect output", "completed": true},
                    {"text": "map parser", "completed": false}
                ]
            }
        }),
    };
    let mapped_plan = external_progress_to_agent_turn_event(
        "session-a",
        TRAEX_ADAPTER,
        ExternalCliProgressStatusContext::new(
            Some("traex"),
            Some("trae-model"),
            Some("runner config"),
            Some("high"),
            Some("auto"),
            Some(Path::new("/tmp/work")),
        ),
        &plan_event,
    )
    .expect("mapped plan event");
    match mapped_plan {
        bifrost_agent::AgentTurnProgressEvent::PlanUpdated { steps, title } => {
            assert!(title.is_none());
            assert_eq!(steps.len(), 2);
            assert_eq!(steps[0].status, PlanStepStatus::Completed);
            assert_eq!(steps[1].status, PlanStepStatus::Pending);
        }
        other => panic!("unexpected mapped plan event: {other:?}"),
    }
}

#[tokio::test]
async fn external_cli_run_writes_image_attachments_and_injects_prompt_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _data_dir_guard = crate::test_env::BifrostDataDirGuard::set(temp_dir.path());
    let runs_root = temp_dir.path().join("runs");
    let runtime = ExternalCliRuntime::new(&runs_root);
    let request = ExternalCliRunRequest {
        images: vec![
            ExternalCliImageInput {
                mime_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
                name: Some("pasted.png".to_string()),
            },
            ExternalCliImageInput {
                mime_type: "image/jpeg".to_string(),
                data: "dHdv".to_string(),
                name: Some("second.jpg".to_string()),
            },
        ],
        message: String::new(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("chat-gateway-image-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"saw image\"}'".to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut second_request = request.clone();
    second_request.images = vec![ExternalCliImageInput {
        mime_type: "image/png".to_string(),
        data: "d29ybGQ=".to_string(),
        name: Some("second.png".to_string()),
    }];

    let result = runtime.run(request).await.unwrap();

    let prompt = tokio::fs::read_to_string(&result.artifacts.prompt)
        .await
        .unwrap();
    assert!(prompt.contains("## Attached Images"));
    assert!(prompt.contains("image-1.png"));
    assert!(prompt.contains("image-2.jpg"));
    let images: Vec<ExternalCliSavedImageAttachment> = serde_json::from_str(
        result
            .metadata
            .get("attachments.images")
            .expect("attachments metadata"),
    )
    .unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[1].mime_type, "image/jpeg");
    let first_image_path = std::path::PathBuf::from(&images[0].path);
    let second_saved_image_path = std::path::PathBuf::from(&images[1].path);
    assert_eq!(
        first_image_path.parent(),
        Some(
            runs_root
                .join(&result.run_id)
                .join("attachments")
                .join("images")
                .as_path()
        )
    );
    assert_eq!(
        first_image_path.file_name().and_then(|v| v.to_str()),
        Some("image-1.png")
    );
    assert_eq!(
        second_saved_image_path.file_name().and_then(|v| v.to_str()),
        Some("image-2.jpg")
    );
    assert_eq!(tokio::fs::read(&images[0].path).await.unwrap(), b"hello");
    assert_eq!(tokio::fs::read(&images[1].path).await.unwrap(), b"two");

    let second_result = runtime.run(second_request).await.unwrap();
    let second_images: Vec<ExternalCliSavedImageAttachment> = serde_json::from_str(
        second_result
            .metadata
            .get("attachments.images")
            .expect("attachments metadata"),
    )
    .unwrap();
    assert_eq!(second_images.len(), 1);
    let second_image_path = std::path::PathBuf::from(&second_images[0].path);
    assert_ne!(first_image_path, second_image_path);
    assert_eq!(
        second_image_path.parent(),
        Some(
            runs_root
                .join(&second_result.run_id)
                .join("attachments")
                .join("images")
                .as_path()
        )
    );
    assert_eq!(
        tokio::fs::read(&first_image_path).await.unwrap(),
        b"hello",
        "second run must not overwrite first run attachment"
    );
    assert_eq!(tokio::fs::read(&second_image_path).await.unwrap(), b"world");
}

#[tokio::test]
async fn external_cli_runtime_marks_stopped_run_before_late_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
    let (executable, args) = delayed_final_command("too late");
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "stop me".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("stop-test".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    let run_id = wait_for_single_run_dir(&runs_root).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    request_run_stop(&runs_root, &run_id).await.unwrap();

    let result = handle.await.unwrap();

    assert_eq!(result.run_id, run_id);
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert_eq!(result.response, "External CLI run was stopped by request.");
    assert_eq!(result.exit_code, None);
    assert_eq!(
        result.events[0].event_type,
        ExternalCliProgressEventType::RunFailed
    );
}

#[tokio::test]
async fn read_run_detail_prefers_persisted_result_response_over_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let run_id = "detail-response-run";
    let run_dir = runs_root.join(run_id);
    tokio::fs::create_dir_all(&run_dir).await.unwrap();
    tokio::fs::write(run_dir.join("runtime_snapshot.json"), "{}")
        .await
        .unwrap();
    tokio::fs::write(
        run_dir.join("normalized_events.jsonl"),
        r#"{"eventType":"run_failed","content":"External CLI run was stopped by request.","title":"Stopped","raw":{"type":"run_stopped"}}"#,
    )
    .await
    .unwrap();
    tokio::fs::write(run_dir.join("cli.stdout.log"), "raw streaming stdout\n")
        .await
        .unwrap();
    tokio::fs::write(
        run_dir.join("cli.stderr.log"),
        "external cli stopped by request\n",
    )
    .await
    .unwrap();
    let result = ExternalCliRunResult {
        run_id: run_id.to_string(),
        session_key: Some("detail-response-session".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "traex".to_string(),
        status: ExternalCliRunStatus::Stopped,
        exit_code: None,
        response: "External CLI run was stopped by request.".to_string(),
        responses: vec!["External CLI run was stopped by request.".to_string()],
        started_at: 1,
        finished_at: 2,
        duration_ms: 1,
        artifacts: ExternalCliRunArtifacts {
            run_dir: run_dir.display().to_string(),
            prompt: run_dir.join("prompt.md").display().to_string(),
            command_snapshot: run_dir.join("runtime_snapshot.json").display().to_string(),
            stdout: run_dir.join("cli.stdout.log").display().to_string(),
            stderr: run_dir.join("cli.stderr.log").display().to_string(),
            normalized_events: run_dir
                .join("normalized_events.jsonl")
                .display()
                .to_string(),
            last_message: run_dir.join("last_message.md").display().to_string(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::new(),
    };
    tokio::fs::write(
        run_dir.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .await
    .unwrap();

    let detail = read_run_detail(&runs_root, run_id).await.unwrap();

    assert_eq!(detail.response, "External CLI run was stopped by request.");
}

#[test]
fn visible_terminal_response_uses_stderr_for_empty_failed_result() {
    let response = visible_terminal_response(
        ExternalCliRunStatus::Failed,
        String::new(),
        "",
        "Error loading config.toml: unknown variant `default`, expected `fast` or `flex`\n",
        &[],
    );

    assert_eq!(
        response,
        "Error loading config.toml: unknown variant `default`, expected `fast` or `flex`"
    );
}

#[tokio::test]
async fn external_cli_runtime_stops_active_run_by_session_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
    let (executable, args) = delayed_final_command("too late");
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "stop by session".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: None,
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            executable: Some(executable),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    let run_id = wait_for_single_run_dir(&runs_root).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    request_session_stop(&runs_root, "im:provider-a:user-a")
        .await
        .unwrap();

    let result = handle.await.unwrap();

    assert_eq!(result.run_id, run_id);
    assert_eq!(result.status, ExternalCliRunStatus::Stopped);
    assert_eq!(result.response, "External CLI run was stopped by request.");
}

#[tokio::test]
async fn worker_guide_rejects_saturated_control_channel_without_waiting() {
    let session_key = format!("saturated-guide-session-{}", uuid::Uuid::new_v4());
    let (control_tx, _control_rx) = tokio::sync::mpsc::channel(1);
    let (stop_ack_tx, _stop_ack_rx) = oneshot::channel();
    control_tx
        .try_send(ExternalCliWorkerControlRequest::Stop {
            ack_tx: stop_ack_tx,
        })
        .expect("fill control channel");
    ACTIVE_WORKER_SESSIONS.insert(
        session_key.clone(),
        ExternalCliWorkerControlHandle { pid: 1, control_tx },
    );

    let error = request_worker_session_guide(
        &session_key,
        "guide-over-capacity".to_string(),
        "do not wait for a saturated worker".to_string(),
    )
    .await
    .expect_err("saturated guide should fail fast");

    assert!(error.contains("too many pending guide requests"));
    ACTIVE_WORKER_SESSIONS.remove(&session_key);
}

#[test]
fn stale_run_session_cleanup_preserves_replacement_owner() {
    let session_key = format!("replacement-session-{}", uuid::Uuid::new_v4());
    let old_run_id = format!("old-run-{}", uuid::Uuid::new_v4());
    let new_run_id = format!("new-run-{}", uuid::Uuid::new_v4());
    ACTIVE_SESSIONS.insert(session_key.clone(), new_run_id.clone());

    assert!(!remove_active_session_if_owned(&session_key, &old_run_id));
    assert_eq!(
        ACTIVE_SESSIONS
            .get(&session_key)
            .map(|entry| entry.value().clone()),
        Some(new_run_id.clone())
    );

    assert!(remove_active_session_if_owned(&session_key, &new_run_id));
    assert!(!ACTIVE_SESSIONS.contains_key(&session_key));
}

#[test]
fn terminate_process_rejects_pid_zero() {
    let error = terminate_process(0).unwrap_err();

    assert_eq!(error, "refusing to terminate pid 0");
}

#[tokio::test]
async fn request_run_stop_treats_missing_active_pid_as_stopped() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let run_id = "missing-active-pid-stop";
    tokio::fs::create_dir_all(runs_root.join(run_id))
        .await
        .unwrap();
    ACTIVE_RUNS.insert(run_id.to_string(), 999_999_999);

    request_run_stop(&runs_root, run_id).await.unwrap();

    assert!(
        tokio::fs::try_exists(runs_root.join(run_id).join("stop_requested"))
            .await
            .unwrap()
    );
    assert!(
        ACTIVE_RUNS.get(run_id).is_none(),
        "missing active pid should still be removed after a stop request"
    );
}

#[test]
fn taskkill_missing_process_messages_are_idempotent() {
    assert!(taskkill_message_indicates_missing_process(
        b"ERROR: The process \"999999999\" not found.",
        b""
    ));
    assert!(taskkill_message_indicates_missing_process(
        b"",
        b"ERROR: The process with PID 999999999 could not be terminated.\r\nReason: There is no running instance of the task.\r\n"
    ));
    assert!(!taskkill_message_indicates_missing_process(
        b"",
        b"ERROR: The process with PID 999999999 could not be terminated.\r\nReason: Access is denied.\r\n"
    ));
}

#[test]
fn effective_config_marks_channel_overrides() {
    let mut config = ExternalCliGatewayConfig::default();
    let runner = config
        .runners
        .get_mut(DEFAULT_CODEX_RUNNER_ID)
        .expect("Codex runner");
    runner.enabled = true;
    runner.adapter = "codex".to_string();
    runner.inject_bifrost_tools = true;
    config.runners.insert(
        "mock-runner".to_string(),
        ExternalCliAgentSettings {
            enabled: true,
            adapter: "mock".to_string(),
            inject_bifrost_tools: false,
            ..Default::default()
        },
    );
    config.channels.insert(
        "feishu-main".to_string(),
        ExternalCliChannelSettings {
            runner_id: Some("mock-runner".to_string()),
            ..Default::default()
        },
    );

    let effective = effective_config_for_provider(&config, Some("feishu-main"));

    assert!(effective.settings.enabled);
    assert_eq!(effective.settings.adapter, "mock");
    assert!(!effective.settings.inject_bifrost_tools);
    assert_eq!(
        effective.sources.get("runnerId").map(String::as_str),
        Some("channel")
    );
    assert_eq!(effective.runner_id, "mock-runner");
}

#[test]
fn default_gateway_config_contains_enabled_codex_and_traex_runners() {
    let config = ExternalCliGatewayConfig::default();

    assert_eq!(config.default_runner_id, DEFAULT_CODEX_RUNNER_ID);
    let codex = config
        .runners
        .get(DEFAULT_CODEX_RUNNER_ID)
        .expect("Codex default runner");
    assert!(codex.enabled);
    assert_eq!(codex.adapter, DEFAULT_ADAPTER);
    let traex_runner = config
        .runners
        .get(DEFAULT_TRAEX_RUNNER_ID)
        .expect("Traex default runner");
    assert!(traex_runner.enabled);
    assert_eq!(traex_runner.adapter, TRAEX_ADAPTER);
    let claude_code = config
        .runners
        .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
        .expect("Claude Code default runner");
    assert!(claude_code.enabled);
    assert_eq!(claude_code.adapter, CLAUDE_CODE_ADAPTER);
}

#[test]
fn normalized_gateway_config_adds_named_defaults_without_overwriting_existing_runners() {
    let mut config = ExternalCliGatewayConfig {
        default_runner_id: "custom".to_string(),
        runners: BTreeMap::from([(
            "custom".to_string(),
            ExternalCliAgentSettings {
                enabled: false,
                adapter: "mock".to_string(),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    config.runners.remove(DEFAULT_CODEX_RUNNER_ID);
    config.runners.remove(DEFAULT_TRAEX_RUNNER_ID);
    config.runners.remove(DEFAULT_CLAUDE_CODE_RUNNER_ID);

    let normalized = normalized_gateway_config(config);

    assert_eq!(normalized.default_runner_id, "custom");
    assert_eq!(
        normalized
            .runners
            .get("custom")
            .map(|settings| settings.adapter.as_str()),
        Some("mock")
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CODEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, DEFAULT_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_TRAEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, CLAUDE_CODE_ADAPTER))
    );
}

#[test]
fn normalized_gateway_config_migrates_legacy_claude_code_runner_id() {
    let config = ExternalCliGatewayConfig {
        default_runner_id: "Claude Code".to_string(),
        runners: BTreeMap::from([(
            "Claude Code".to_string(),
            ExternalCliAgentSettings {
                enabled: true,
                adapter: CLAUDE_CODE_ADAPTER.to_string(),
                ..Default::default()
            },
        )]),
        channels: BTreeMap::from([(
            "feishu-main".to_string(),
            ExternalCliChannelSettings {
                runner_id: Some("Claude Code".to_string()),
                ..Default::default()
            },
        )]),
        version: 1,
    };

    let normalized = normalized_gateway_config(config);

    assert!(!normalized.runners.contains_key("Claude Code"));
    assert_eq!(normalized.default_runner_id, DEFAULT_CLAUDE_CODE_RUNNER_ID);
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| settings.adapter.as_str()),
        Some(CLAUDE_CODE_ADAPTER)
    );
    assert_eq!(
        normalized
            .channels
            .get("feishu-main")
            .and_then(|channel| channel.runner_id.as_deref()),
        Some(DEFAULT_CLAUDE_CODE_RUNNER_ID)
    );
    assert_eq!(
        canonical_external_cli_runner_id(&normalized, "claude code"),
        DEFAULT_CLAUDE_CODE_RUNNER_ID
    );
}

#[test]
fn normalized_gateway_config_empty_runners_uses_enabled_named_defaults() {
    let normalized = normalized_gateway_config(ExternalCliGatewayConfig {
        default_runner_id: "codex".to_string(),
        runners: BTreeMap::new(),
        channels: BTreeMap::new(),
        version: 0,
    });

    assert_eq!(normalized.default_runner_id, DEFAULT_CODEX_RUNNER_ID);
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CODEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, DEFAULT_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_TRAEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
    );
    assert_eq!(
        normalized
            .runners
            .get(DEFAULT_CLAUDE_CODE_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, CLAUDE_CODE_ADAPTER))
    );
    assert!(!normalized.runners.contains_key("codex"));
}

#[test]
fn effective_config_resolves_legacy_runner_aliases_to_named_defaults() {
    let config = ExternalCliGatewayConfig::default();

    let codex = effective_config_for_provider_and_runner(&config, None, Some("codex"));
    assert_eq!(codex.runner_id, DEFAULT_CODEX_RUNNER_ID);
    assert_eq!(codex.settings.adapter, DEFAULT_ADAPTER);
    assert!(codex.settings.enabled);

    let traex = effective_config_for_provider_and_runner(&config, None, Some("traex"));
    assert_eq!(traex.runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert_eq!(traex.settings.adapter, TRAEX_ADAPTER);
    assert!(traex.settings.enabled);

    let legacy_alias = ["Tree", "X"].concat();
    let legacy_traex = effective_config_for_provider_and_runner(&config, None, Some(&legacy_alias));
    assert_eq!(legacy_traex.runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert_eq!(legacy_traex.settings.adapter, TRAEX_ADAPTER);
    assert!(legacy_traex.settings.enabled);

    let claude_code = effective_config_for_provider_and_runner(&config, None, Some("claude-code"));
    assert_eq!(claude_code.runner_id, DEFAULT_CLAUDE_CODE_RUNNER_ID);
    assert_eq!(claude_code.settings.adapter, CLAUDE_CODE_ADAPTER);
    assert!(claude_code.settings.enabled);
}

#[test]
fn normalized_gateway_config_migrates_legacy_traex_runner_id() {
    let legacy_alias = ["Tree", "X"].concat();
    let normalized = normalized_gateway_config(ExternalCliGatewayConfig {
        default_runner_id: legacy_alias.clone(),
        runners: BTreeMap::from([(
            legacy_alias.clone(),
            ExternalCliAgentSettings {
                enabled: true,
                adapter: TRAEX_ADAPTER.to_string(),
                ..Default::default()
            },
        )]),
        channels: BTreeMap::from([(
            "feishu-main".to_string(),
            ExternalCliChannelSettings {
                runner_id: Some(legacy_alias.clone()),
                ..Default::default()
            },
        )]),
        version: 1,
    });

    assert_eq!(normalized.default_runner_id, DEFAULT_TRAEX_RUNNER_ID);
    assert!(normalized.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID));
    assert!(!normalized.runners.contains_key(&legacy_alias));
    assert_eq!(
        normalized
            .channels
            .get("feishu-main")
            .and_then(|channel| channel.runner_id.as_deref()),
        Some(DEFAULT_TRAEX_RUNNER_ID)
    );
}

#[test]
fn config_store_new_persists_missing_default_runners_on_startup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir
        .path()
        .join("admin")
        .join("im_gateway_external_cli_agent.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"{"version":1,"defaultRunnerId":"legacy","runners":{"legacy":{"enabled":true,"adapter":"mock","adapterConfig":{},"injectBifrostTools":true,"skillPaths":[],"deliveryMode":"final_reply"}},"channels":{}}"#,
    )
    .unwrap();

    let store = ExternalCliConfigStore::new(temp_dir.path());
    let loaded = store.load();
    let persisted: ExternalCliGatewayConfig =
        serde_json::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();

    for config in [loaded, persisted] {
        assert!(config.runners.contains_key("legacy"));
        assert!(config.runners.contains_key(DEFAULT_CODEX_RUNNER_ID));
        assert!(config.runners.contains_key(DEFAULT_TRAEX_RUNNER_ID));
        assert!(config.runners.contains_key(DEFAULT_CLAUDE_CODE_RUNNER_ID));
    }
}

#[test]
fn codex_request_metadata_includes_configured_or_default_model_label() {
    let _env_lock = external_cli_env_guard();
    let codex_home = tempfile::tempdir().unwrap();
    let trae_home = tempfile::tempdir().unwrap();
    let _codex_home = EnvGuard::set("CODEX_HOME", codex_home.path());
    let _trae_home = EnvGuard::set("TRAE_HOME", trae_home.path());
    let configured_request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("codex".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            model: Some("gpt-test".to_string()),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut configured_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&configured_request, &mut configured_metadata);

    assert_eq!(
        configured_metadata.get("model").map(String::as_str),
        Some("gpt-test")
    );
    assert_eq!(
        configured_metadata.get("modelSource").map(String::as_str),
        Some("runner config")
    );
    assert_eq!(
        configured_metadata.get("modelLabel").map(String::as_str),
        Some("gpt-test")
    );

    let default_request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: default_operation(),
        params: serde_json::Value::Null,
        provider_id: None,
        runner_id: Some("codex".to_string()),
        session_key: None,
        runtime: DEFAULT_RUNTIME.to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut default_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&default_request, &mut default_metadata);

    assert_eq!(default_metadata.get("model"), None);
    assert_eq!(
        default_metadata.get("modelSource").map(String::as_str),
        Some("codex default")
    );
    assert_eq!(
        default_metadata.get("modelLabel").map(String::as_str),
        Some("Codex default model (not explicitly configured)")
    );

    let trae_request = ExternalCliRunRequest {
        adapter: TRAEX_ADAPTER.to_string(),
        runner_id: Some("traex".to_string()),
        ..default_request.clone()
    };
    let mut trae_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&trae_request, &mut trae_metadata);

    assert_eq!(trae_metadata.get("model"), None);
    assert_eq!(
        trae_metadata.get("modelSource").map(String::as_str),
        Some("trae default")
    );
    assert_eq!(
        trae_metadata.get("modelLabel").map(String::as_str),
        Some("Trae default model (not explicitly configured)")
    );

    let claude_code_request = ExternalCliRunRequest {
        adapter: CLAUDE_CODE_ADAPTER.to_string(),
        runner_id: Some(DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string()),
        ..default_request
    };
    let mut claude_code_metadata = std::collections::BTreeMap::new();

    append_external_cli_request_metadata(&claude_code_request, &mut claude_code_metadata);

    assert_eq!(claude_code_metadata.get("model"), None);
    assert_eq!(
        claude_code_metadata.get("modelSource").map(String::as_str),
        Some("claude code default")
    );
    assert_eq!(
        claude_code_metadata.get("modelLabel").map(String::as_str),
        Some("Claude Code default model (not explicitly configured)")
    );
}

#[test]
fn codex_and_traex_model_config_resolves_user_defaults_and_overrides() {
    let _env_lock = external_cli_env_guard();
    let codex_home = tempfile::tempdir().unwrap();
    let trae_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"
model = "gpt-codex-default"
model_reasoning_effort = "high"
model_reasoning_summary = "auto"
"#,
    )
    .unwrap();
    std::fs::write(
        codex_home.path().join("work.config.toml"),
        r#"
model = "gpt-codex-profile"
model_reasoning_effort = "medium"
"#,
    )
    .unwrap();
    std::fs::write(
        trae_home.path().join("traecli.toml"),
        r#"
model = "GPT-Trae"
model_provider = "trae"
"#,
    )
    .unwrap();
    let _codex_home = EnvGuard::set("CODEX_HOME", codex_home.path());
    let _trae_home = EnvGuard::set("TRAE_HOME", trae_home.path());

    let codex = resolve_external_cli_model_config(
        DEFAULT_ADAPTER,
        &ExternalCliAdapterConfig {
            profile: Some("work".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(codex.model.as_deref(), Some("gpt-codex-profile"));
    assert_eq!(codex.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(codex.reasoning_summary.as_deref(), Some("auto"));
    assert_eq!(codex.model_source.as_deref(), Some("codex config"));

    let trae =
        resolve_external_cli_model_config(TRAEX_ADAPTER, &ExternalCliAdapterConfig::default());
    assert_eq!(trae.model.as_deref(), Some("GPT-Trae"));
    assert_eq!(trae.model_provider.as_deref(), Some("trae"));

    let overridden = resolve_external_cli_model_config(
        DEFAULT_ADAPTER,
        &ExternalCliAdapterConfig {
            model: Some("gpt-runner".to_string()),
            reasoning_effort: Some("low".to_string()),
            config_overrides: vec!["model_reasoning_summary=\"detailed\"".to_string()],
            ..Default::default()
        },
    );
    assert_eq!(overridden.model.as_deref(), Some("gpt-runner"));
    assert_eq!(overridden.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(overridden.reasoning_summary.as_deref(), Some("detailed"));
    assert_eq!(overridden.model_source.as_deref(), Some("runner config"));
    assert_eq!(
        overridden.reasoning_source.as_deref(),
        Some("runner config")
    );
}

#[test]
fn claude_code_model_config_ignores_settings_model_but_resolves_effort() {
    let _env_lock = external_cli_env_guard();
    let home = tempfile::tempdir().unwrap();
    let claude_home = home.path().join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    std::fs::write(
        claude_home.join("settings.json"),
        r#"{
          "model": "sonnet",
          "effortLevel": "low",
          "env": {
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-custom",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-opus-4-7",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-custom",
            "CLAUDE_CODE_EFFORT_LEVEL": "medium"
          }
        }"#,
    )
    .unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let _claude_config_dir = EnvGuard::unset("CLAUDE_CONFIG_DIR");
    let _claude_home = EnvGuard::unset("CLAUDE_HOME");
    let _anthropic_model = EnvGuard::unset("ANTHROPIC_MODEL");
    let _default_sonnet = EnvGuard::unset("ANTHROPIC_DEFAULT_SONNET_MODEL");
    let _default_opus = EnvGuard::unset("ANTHROPIC_DEFAULT_OPUS_MODEL");
    let _default_haiku = EnvGuard::unset("ANTHROPIC_DEFAULT_HAIKU_MODEL");
    let _claude_effort = EnvGuard::unset("CLAUDE_CODE_EFFORT_LEVEL");
    let _claude_effort_short = EnvGuard::unset("CLAUDE_EFFORT");

    let claude = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(claude.model, None);
    assert_eq!(claude.model_provider, None);
    assert_eq!(claude.model_source, None);
    assert_eq!(claude.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(claude.reasoning_source.as_deref(), Some("claude settings"));

    let runner_model = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig {
            env: BTreeMap::from([(
                "ANTHROPIC_MODEL".to_string(),
                "custom-direct-model".to_string(),
            )]),
            ..Default::default()
        },
    );
    assert_eq!(runner_model.model, None);
    assert_eq!(runner_model.reasoning_effort.as_deref(), Some("medium"));

    let runner_effort = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig {
            env: BTreeMap::from([("CLAUDE_CODE_EFFORT_LEVEL".to_string(), "high".to_string())]),
            ..Default::default()
        },
    );
    assert_eq!(runner_effort.model, None);
    assert_eq!(runner_effort.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        runner_effort.reasoning_source.as_deref(),
        Some("runner config")
    );

    let status = resolve_external_cli_status_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(status.model.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(status.model_provider.as_deref(), Some("sonnet"));
    assert_eq!(status.model_source.as_deref(), Some("claude settings"));
    assert_eq!(status.reasoning_effort.as_deref(), Some("medium"));
}

#[test]
fn claude_code_status_model_config_reads_plain_settings_model_without_catalog_coupling() {
    let _env_lock = external_cli_env_guard();
    let home = tempfile::tempdir().unwrap();
    let claude_home = home.path().join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    std::fs::write(
        claude_home.join("settings.json"),
        r#"{
          "model": "opus",
          "effortLevel": "low"
        }"#,
    )
    .unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let _claude_config_dir = EnvGuard::unset("CLAUDE_CONFIG_DIR");
    let _claude_home = EnvGuard::unset("CLAUDE_HOME");
    let _anthropic_model = EnvGuard::unset("ANTHROPIC_MODEL");
    let _default_sonnet = EnvGuard::unset("ANTHROPIC_DEFAULT_SONNET_MODEL");
    let _default_opus = EnvGuard::unset("ANTHROPIC_DEFAULT_OPUS_MODEL");
    let _default_haiku = EnvGuard::unset("ANTHROPIC_DEFAULT_HAIKU_MODEL");
    let _claude_effort = EnvGuard::unset("CLAUDE_CODE_EFFORT_LEVEL");
    let _claude_effort_short = EnvGuard::unset("CLAUDE_EFFORT");

    let runtime = resolve_external_cli_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(runtime.model, None);
    assert_eq!(runtime.reasoning_effort.as_deref(), Some("low"));

    let status = resolve_external_cli_status_model_config(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
    );
    assert_eq!(status.model.as_deref(), Some("opus"));
    assert_eq!(status.model_provider, None);
    assert_eq!(status.model_source.as_deref(), Some("claude settings"));
    assert_eq!(status.reasoning_effort.as_deref(), Some("low"));
}

#[test]
fn codex_like_metadata_includes_turn_usage_tokens() {
    let events = parse_progress_events(
        r#"{"type":"thread.started","thread_id":"thread-usage"}
{"type":"turn.completed","usage":{"input_tokens":59589,"cached_input_tokens":6912,"output_tokens":221,"reasoning_output_tokens":156}}"#,
    );
    let mut metadata = std::collections::BTreeMap::new();

    append_external_cli_metadata(TRAEX_ADAPTER, &events, &mut metadata);

    assert_eq!(
        metadata.get("threadId").map(String::as_str),
        Some("thread-usage")
    );
    assert_eq!(
        metadata.get("usageInputTokens").map(String::as_str),
        Some("59589")
    );
    assert_eq!(
        metadata.get("usageCachedInputTokens").map(String::as_str),
        Some("6912")
    );
    assert_eq!(
        metadata.get("usageOutputTokens").map(String::as_str),
        Some("221")
    );
    assert_eq!(
        metadata
            .get("usageReasoningOutputTokens")
            .map(String::as_str),
        Some("156")
    );
    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("59810")
    );
}

#[test]
fn codex_progress_metadata_merges_thread_total_and_weekly_window() {
    let usage_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "token usage updated".to_string(),
        title: Some("token_usage".to_string()),
        raw: serde_json::json!({
            "usage": {
                "input_tokens": 1200,
                "cached_input_tokens": 300,
                "output_tokens": 80,
                "reasoning_output_tokens": 20,
                "total_tokens": 1280
            }
        }),
    };
    let limits_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "usage updated".to_string(),
        title: Some("rate_limits".to_string()),
        raw: serde_json::json!({
            "params": {
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 63,
                        "windowDurationMins": 10080,
                        "resetsAt": 1784490086
                    },
                    "secondary": {
                        "usedPercent": 5,
                        "windowDurationMins": 300,
                        "resetsAt": 1784000000
                    }
                }
            }
        }),
    };
    let mut metadata = std::collections::BTreeMap::new();

    assert!(merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &usage_event,
        &mut metadata
    ));
    assert!(merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &limits_event,
        &mut metadata
    ));

    assert_eq!(
        metadata.get("usageTotalTokens").map(String::as_str),
        Some("1280")
    );
    assert_eq!(
        metadata.get("codexWeeklyUsedPercent").map(String::as_str),
        Some("63")
    );
    assert_eq!(
        metadata.get("codexWeeklyWindowMinutes").map(String::as_str),
        Some("10080")
    );
    assert_eq!(
        metadata.get("codexWeeklyResetsAt").map(String::as_str),
        Some("1784490086")
    );
}

#[test]
fn codex_progress_metadata_ignores_short_windows_and_non_codex_adapters() {
    let event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "usage updated".to_string(),
        title: Some("rate_limits".to_string()),
        raw: serde_json::json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 20,
                    "windowDurationMins": 300,
                    "resetsAt": 1784000000
                }
            }
        }),
    };
    let mut metadata = std::collections::BTreeMap::new();

    assert!(!merge_external_cli_progress_metadata(
        DEFAULT_ADAPTER,
        &event,
        &mut metadata
    ));
    assert!(!merge_external_cli_progress_metadata(
        CLAUDE_CODE_ADAPTER,
        &event,
        &mut metadata
    ));
    assert!(metadata.is_empty());
}

#[test]
fn codex_and_traex_metadata_include_runner_observability() {
    for adapter in [DEFAULT_ADAPTER, TRAEX_ADAPTER] {
        let request = ExternalCliRunRequest {
            images: Vec::new(),
            message: "inspect image".to_string(),
            operation: default_operation(),
            params: serde_json::json!({"threadId": "thread-existing"}),
            provider_id: Some("web".to_string()),
            runner_id: Some(adapter.to_string()),
            session_key: Some("session-observe".to_string()),
            runtime: DEFAULT_RUNTIME.to_string(),
            adapter: adapter.to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/work")),
            instructions: None,
            adapter_config: ExternalCliAdapterConfig {
                approval_policy: Some("never".to_string()),
                sandbox: Some("danger-full-access".to_string()),
                permission_mode: Some("bypassPermissions".to_string()),
                danger_full_access: Some(true),
                add_dirs: vec!["/tmp/extra".to_string()],
                enable_features: vec!["network".to_string()],
                timeout_secs: Some(30),
                ..Default::default()
            },
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: true,
            skill_paths: Vec::new(),
        };
        let spec = CommandSpec {
            executable: adapter.to_string(),
            args: vec!["exec".to_string(), "--json".to_string()],
            env: std::collections::BTreeMap::new(),
            work_dir: request.work_dir.clone(),
            timeout_secs: Some(30),
        };
        let events = vec![
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::Status,
                content: "retrying capacity error".to_string(),
                title: Some("Codex capacity retry".to_string()),
                raw: serde_json::json!({
                    "type": "capacity_retry",
                    "retryAttempt": 1,
                    "maxRetries": 3,
                    "delayMs": 1000
                }),
            },
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::ToolFinished,
                content: "tool output".to_string(),
                title: Some("Shell".to_string()),
                raw: serde_json::json!({
                    "type": "item.completed",
                    "observedAtMs": 1120,
                    "durationMs": 120,
                    "item": {
                        "id": "tool-1",
                        "type": "command_execution",
                        "command": "pwd",
                        "exit_code": 0,
                        "status": "completed"
                    }
                }),
            },
            ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::AssistantFinal,
                content: "done".to_string(),
                title: None,
                raw: serde_json::json!({"type": "assistant_final", "observedAtMs": 1150}),
            },
        ];
        let saved_images = vec![ExternalCliSavedImageAttachment {
            path: "/tmp/session/run/images/image-1.png".to_string(),
            mime_type: "image/png".to_string(),
            size_bytes: 42,
            name: Some("image.png".to_string()),
        }];
        let mut metadata = std::collections::BTreeMap::new();

        append_external_cli_observability_metadata(
            ExternalCliObservabilityInput {
                request: &request,
                spec: &spec,
                prompt: "## Attached Images\n- /tmp/session/run/images/image-1.png\n",
                saved_images: &saved_images,
                stdout: b"{\"type\":\"assistant_final\"}\n",
                stderr: b"warning\n",
                events: &events,
                timings: ExternalCliObservabilityTimings {
                    started_at: 1000,
                    command_started_at: Some(1010),
                    command_finished_at: Some(1200),
                    finished_at: 1250,
                },
                cli_version: Some("runner 1.2.3"),
            },
            &mut metadata,
        );

        assert_eq!(
            metadata.get("runner.adapter").map(String::as_str),
            Some(adapter)
        );
        assert_eq!(
            metadata.get("cli.version").map(String::as_str),
            Some("runner 1.2.3")
        );
        assert_eq!(
            metadata.get("prompt.attachmentPathCount"),
            Some(&"1".to_string())
        );
        assert_eq!(
            metadata.get("attachments.totalBytes"),
            Some(&"42".to_string())
        );
        assert_eq!(metadata.get("io.stdoutLines"), Some(&"1".to_string()));
        assert_eq!(
            metadata.get("timing.commandDurationMs"),
            Some(&"190".to_string())
        );
        assert_eq!(
            metadata.get("timing.firstEventLatencyMs"),
            Some(&"120".to_string())
        );
        assert_eq!(metadata.get("tools.count"), Some(&"1".to_string()));
        assert_eq!(
            metadata.get("runner.capacityRetryCount"),
            Some(&"1".to_string())
        );
        assert_eq!(
            metadata.get("tools.totalDurationMs"),
            Some(&"120".to_string())
        );
        assert_eq!(
            metadata.get("resume.requested").map(String::as_str),
            Some("true")
        );
    }
}

#[test]
fn progress_event_observation_adds_tool_duration() {
    let mut starts = std::collections::HashMap::new();
    let mut started = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolStarted,
        content: "pwd".to_string(),
        title: None,
        raw: serde_json::json!({"type": "item.started", "item": {"id": "tool-1"}}),
    };
    let mut finished = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::ToolFinished,
        content: "/tmp".to_string(),
        title: None,
        raw: serde_json::json!({"type": "item.completed", "item": {"id": "tool-1"}}),
    };

    enrich_progress_event_observation(&mut started, 2000, &mut starts);
    enrich_progress_event_observation(&mut finished, 2125, &mut starts);

    assert_eq!(
        started
            .raw
            .get("observedAtMs")
            .and_then(serde_json::Value::as_u64),
        Some(2000)
    );
    assert_eq!(
        finished
            .raw
            .get("observedAtMs")
            .and_then(serde_json::Value::as_u64),
        Some(2125)
    );
    assert_eq!(
        finished
            .raw
            .get("durationMs")
            .and_then(serde_json::Value::as_u64),
        Some(125)
    );
}

#[test]
fn codex_cli_parser_maps_reasoning_summary_to_assistant_delta() {
    let events = parse_progress_events(
        r#"{"type":"item.completed","item":{"id":"reasoning_0","type":"reasoning_summary","summary":"I checked the workspace and will run the focused tests."}}"#,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(
        events[0].content,
        "I checked the workspace and will run the focused tests."
    );
}

#[test]
fn traex_model_slash_command_parser_handles_list_show_set_and_clear() {
    assert_eq!(
        parse_external_cli_model_slash_command("/models"),
        Some(Ok(ExternalCliModelSlashCommand::List))
    );
    assert!(matches!(
        parse_external_cli_model_slash_command("/models extra"),
        Some(Err(_))
    ));
    assert_eq!(
        parse_external_cli_model_slash_command(" /model "),
        Some(Ok(ExternalCliModelSlashCommand::Show))
    );
    assert_eq!(
        parse_external_cli_model_slash_command("/model gpt-5.5"),
        Some(Ok(ExternalCliModelSlashCommand::Set("gpt-5.5".to_string())))
    );
    assert_eq!(
        parse_external_cli_model_slash_command("/model clear"),
        Some(Ok(ExternalCliModelSlashCommand::Clear))
    );
    assert!(matches!(
        parse_external_cli_model_slash_command("/model bad model"),
        Some(Err(_))
    ));
    assert_eq!(parse_external_cli_model_slash_command("/modelish"), None);
}

#[test]
fn external_cli_effort_slash_command_parser_handles_list_show_set_and_clear() {
    assert_eq!(
        parse_external_cli_effort_slash_command("/efforts"),
        Some(Ok(ExternalCliEffortSlashCommand::List))
    );
    assert!(matches!(
        parse_external_cli_effort_slash_command("/efforts extra"),
        Some(Err(_))
    ));
    assert_eq!(
        parse_external_cli_effort_slash_command(" /effort "),
        Some(Ok(ExternalCliEffortSlashCommand::Show))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort xhigh"),
        Some(Ok(ExternalCliEffortSlashCommand::Set("xhigh".to_string())))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort clear"),
        Some(Ok(ExternalCliEffortSlashCommand::Clear))
    );
    assert_eq!(
        parse_external_cli_effort_slash_command("/effort auto"),
        Some(Ok(ExternalCliEffortSlashCommand::Clear))
    );
    assert!(matches!(
        parse_external_cli_effort_slash_command("/effort bad value"),
        Some(Err(_))
    ));
    assert_eq!(parse_external_cli_effort_slash_command("/effortish"), None);
}

#[test]
fn external_cli_model_catalog_parser_filters_raw_catalog_to_safe_public_fields() {
    let models = parse_external_cli_model_catalog(
        TRAEX_ADAPTER,
        r#"{
          "models": [
            {
              "slug": "hidden-model",
              "visibility": "hidden",
              "base_instructions": "do not leak"
            },
            {
              "slug": "Doubao-Seed-2.1-Pro",
              "description": "flagship",
              "default_reasoning_level": "high",
              "supported_reasoning_levels": [{"effort": "low", "description": "fast"}],
              "visibility": "list",
              "supported_in_api": true,
              "model_load": 115,
              "priority": 2,
              "additional_speed_tiers": ["fast"],
              "service_tiers": [{"id": "default", "name": "Default", "description": "standard"}],
              "base_instructions": "do not leak"
            },
            {
              "slug": "DeepSeek-V4-Flash",
              "visibility": "list",
              "priority": 1
            }
          ]
        }"#,
    )
    .expect("parse catalog");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].slug, "DeepSeek-V4-Flash");
    assert_eq!(models[1].slug, "Doubao-Seed-2.1-Pro");
    assert_eq!(models[1].default_reasoning_level.as_deref(), Some("high"));
    assert_eq!(models[1].additional_speed_tiers, vec!["fast"]);
    assert_eq!(models[1].model_load.as_deref(), Some("115%"));
    let serialized = serde_json::to_string(&models).expect("serialize sanitized catalog");
    assert!(!serialized.contains("base_instructions"));
    assert!(!serialized.contains("do not leak"));
    let rendered = format_external_cli_model_catalog(TRAEX_ADAPTER, &models);
    assert!(rendered.contains("Model load: 115%"));
}

#[test]
fn external_cli_model_catalog_parser_accepts_codex_catalog() {
    let models = parse_external_cli_model_catalog(
        DEFAULT_ADAPTER,
        r#"{
          "models": [
            {
              "slug": "gpt-5.5",
              "description": "Frontier model",
              "default_reasoning_level": "medium",
              "visibility": "list",
              "priority": 0,
              "additional_speed_tiers": ["fast"],
              "base_instructions": "do not leak"
            }
          ]
        }"#,
    )
    .expect("parse codex catalog");

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "gpt-5.5");
    assert_eq!(models[0].default_reasoning_level.as_deref(), Some("medium"));
    let serialized = serde_json::to_string(&models).expect("serialize sanitized catalog");
    assert!(!serialized.contains("base_instructions"));
    assert!(!serialized.contains("do not leak"));
}

#[tokio::test]
async fn claude_code_model_slash_uses_builtin_catalog_and_accepts_full_model_slug() {
    assert!(supports_external_cli_model_slash(CLAUDE_CODE_ADAPTER));
    assert_eq!(
        external_cli_model_adapter_label(CLAUDE_CODE_ADAPTER),
        "Claude Code"
    );

    let models = load_external_cli_model_catalog(
        CLAUDE_CODE_ADAPTER,
        &ExternalCliAdapterConfig::default(),
        None,
    )
    .await
    .expect("load claude code catalog");

    assert!(models.iter().any(|model| model.slug == "sonnet"));
    assert!(models.iter().any(|model| model.slug == "opus"));
    assert!(models.iter().any(|model| model.slug == "haiku"));
    assert!(models.iter().any(|model| model.slug == "fable"));
    let rendered = format_external_cli_model_catalog(CLAUDE_CODE_ADAPTER, &models);
    assert!(rendered.contains("Sonnet 4.6"));
    assert!(rendered.contains("Opus 4.8"));
    assert!(rendered.contains("Haiku 4.5"));
    assert_eq!(
        validate_external_cli_model_selection(CLAUDE_CODE_ADAPTER, "sonnet", &models)
            .expect("known alias"),
        "sonnet"
    );
    assert_eq!(
        validate_external_cli_model_selection(
            CLAUDE_CODE_ADAPTER,
            "claude-opus-4-5-20251101",
            &models,
        )
        .expect("full model name"),
        "claude-opus-4-5-20251101"
    );
    assert!(
        validate_external_cli_model_selection(CLAUDE_CODE_ADAPTER, "bad model", &models,).is_err()
    );
}

#[test]
fn external_cli_effort_validation_uses_runner_specific_options() {
    assert_eq!(
        validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "xhigh").unwrap(),
        "xhigh"
    );
    assert_eq!(
        validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "MAX").unwrap(),
        "max"
    );
    assert!(validate_external_cli_effort_selection(CLAUDE_CODE_ADAPTER, "minimal").is_err());
    assert_eq!(
        validate_external_cli_effort_selection(DEFAULT_ADAPTER, "minimal").unwrap(),
        "minimal"
    );
    assert!(validate_external_cli_effort_selection(DEFAULT_ADAPTER, "max").is_err());
    assert_eq!(
        validate_external_cli_effort_selection(TRAEX_ADAPTER, "high").unwrap(),
        "high"
    );
}

#[test]
fn external_cli_effort_validation_honors_current_model_supported_levels() {
    let models = vec![ExternalCliModelInfo {
        slug: "thinking-model".to_string(),
        default_reasoning_level: Some("medium".to_string()),
        supported_reasoning_levels: vec![
            ExternalCliReasoningLevelInfo {
                effort: "low".to_string(),
                description: None,
            },
            ExternalCliReasoningLevelInfo {
                effort: "medium".to_string(),
                description: None,
            },
        ],
        ..Default::default()
    }];

    assert_eq!(
        validate_external_cli_effort_selection_for_model(
            TRAEX_ADAPTER,
            "low",
            Some("thinking-model"),
            &models,
        )
        .unwrap(),
        "low"
    );
    assert!(validate_external_cli_effort_selection_for_model(
        TRAEX_ADAPTER,
        "high",
        Some("thinking-model"),
        &models,
    )
    .is_err());
    assert_eq!(
        validate_external_cli_effort_selection_for_model(
            TRAEX_ADAPTER,
            "high",
            Some("unknown-model"),
            &models,
        )
        .unwrap(),
        "high"
    );
    let rendered = format_external_cli_effort_catalog_for_model(
        TRAEX_ADAPTER,
        Some("thinking-model"),
        &models,
    );
    assert!(rendered.contains("当前模型 `thinking-model`"));
    assert!(rendered.contains("`low`"));
    assert!(!rendered.contains("`high`"));
}

fn has_arg_pair(args: &[String], left: &str, right: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == left && pair[1] == right)
}

async fn wait_for_single_run_dir(runs_root: &Path) -> String {
    for _ in 0..100 {
        let mut entries = tokio::fs::read_dir(runs_root).await.unwrap();
        if let Some(entry) = entries.next_entry().await.unwrap() {
            return entry.file_name().to_string_lossy().to_string();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run dir was not created");
}
