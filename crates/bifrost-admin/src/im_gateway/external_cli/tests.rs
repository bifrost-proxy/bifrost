use super::*;

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
    let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(stop_rx);
    ACTIVE_WORKER_SESSIONS.insert(
        "stale-external-worker".to_string(),
        ExternalCliWorkerStopHandle { pid, stop_tx },
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
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    ACTIVE_WORKER_SESSIONS.insert(
        "acked-external-worker".to_string(),
        ExternalCliWorkerStopHandle { pid, stop_tx },
    );
    let ack_task = tokio::spawn(async move {
        if let Some(stop_request) = stop_rx.recv().await {
            stop_request.ack();
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
        Some("traex"),
        Some("trae-model"),
        Some("runner config"),
        Some(Path::new("/tmp/work")),
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
        Some("traex"),
        Some("trae-model"),
        Some("runner config"),
        Some(Path::new("/tmp/work")),
        &status_event,
    )
    .expect("mapped status event");
    match mapped_status {
        bifrost_agent::AgentTurnProgressEvent::Status(status) => {
            assert_eq!(status.runner_type.as_deref(), Some(TRAEX_ADAPTER));
            assert_eq!(status.runner_id.as_deref(), Some("traex"));
            assert_eq!(status.model.as_deref(), Some("trae-model"));
            assert_eq!(status.model_provider.as_deref(), Some("runner config"));
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
        Some("traex"),
        Some("trae-model"),
        Some("runner config"),
        Some(Path::new("/tmp/work")),
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
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let session_attachment_dir = temp_dir
        .path()
        .join("sessions")
        .join("2026")
        .join("06")
        .join("25")
        .join("attachments");
    let request = ExternalCliRunRequest {
        images: vec![ExternalCliImageInput {
            mime_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
            name: Some("pasted.png".to_string()),
        }],
        message: String::new(),
        operation: default_operation(),
        params: serde_json::json!({
            "attachmentBaseDir": session_attachment_dir.display().to_string()
        }),
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

    let result = runtime.run(request).await.unwrap();

    let prompt = tokio::fs::read_to_string(&result.artifacts.prompt)
        .await
        .unwrap();
    assert!(prompt.contains("## Attached Images"));
    assert!(prompt.contains("image-1.png"));
    let images: Vec<ExternalCliSavedImageAttachment> = serde_json::from_str(
        result
            .metadata
            .get("attachments.images")
            .expect("attachments metadata"),
    )
    .unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime_type, "image/png");
    assert!(std::path::Path::new(&images[0].path).starts_with(&session_attachment_dir));
    assert_eq!(tokio::fs::read(&images[0].path).await.unwrap(), b"hello");
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
fn default_gateway_config_contains_enabled_codex_and_treex_runners() {
    let config = ExternalCliGatewayConfig::default();

    assert_eq!(config.default_runner_id, DEFAULT_CODEX_RUNNER_ID);
    let codex = config
        .runners
        .get(DEFAULT_CODEX_RUNNER_ID)
        .expect("Codex default runner");
    assert!(codex.enabled);
    assert_eq!(codex.adapter, DEFAULT_ADAPTER);
    let treex = config
        .runners
        .get(DEFAULT_TREEX_RUNNER_ID)
        .expect("TreeX default runner");
    assert!(treex.enabled);
    assert_eq!(treex.adapter, TRAEX_ADAPTER);
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
    config.runners.remove(DEFAULT_TREEX_RUNNER_ID);

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
            .get(DEFAULT_TREEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
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
            .get(DEFAULT_TREEX_RUNNER_ID)
            .map(|settings| (settings.enabled, settings.adapter.as_str())),
        Some((true, TRAEX_ADAPTER))
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

    let treex = effective_config_for_provider_and_runner(&config, None, Some("traex"));
    assert_eq!(treex.runner_id, DEFAULT_TREEX_RUNNER_ID);
    assert_eq!(treex.settings.adapter, TRAEX_ADAPTER);
    assert!(treex.settings.enabled);
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
        assert!(config.runners.contains_key(DEFAULT_TREEX_RUNNER_ID));
    }
}

#[test]
fn codex_request_metadata_includes_configured_or_default_model_label() {
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
        ..default_request
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
