use super::*;

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
async fn external_cli_run_writes_image_attachments_and_injects_prompt_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runtime = ExternalCliRuntime::new(temp_dir.path());
    let request = ExternalCliRunRequest {
        images: vec![ExternalCliImageInput {
            mime_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
            name: Some("pasted.png".to_string()),
        }],
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
    assert_eq!(tokio::fs::read(&images[0].path).await.unwrap(), b"hello");
}

#[tokio::test]
async fn external_cli_runtime_marks_stopped_run_before_late_stdout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
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
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "sleep 2; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"too late\"}'"
                    .to_string(),
            ],
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
async fn external_cli_runtime_stops_active_run_by_session_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let runs_root = temp_dir.path().to_path_buf();
    let runtime = ExternalCliRuntime::new(&runs_root);
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
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "sleep 2; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"too late\"}'"
                    .to_string(),
            ],
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

#[test]
fn effective_config_marks_channel_overrides() {
    let mut config = ExternalCliGatewayConfig::default();
    let runner = config.runners.get_mut("codex").expect("codex runner");
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
