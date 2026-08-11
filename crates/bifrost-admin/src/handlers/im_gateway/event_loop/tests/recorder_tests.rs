use super::*;

#[test]
fn ensure_external_cli_recorder_creates_metadata_once() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-created";
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    let path = recorder.as_ref().unwrap().file_path().to_path_buf();
    drop(recorder);
    let events = bifrost_agent::persistence::load_conversation_events(&path).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["session_start", "title_updated", "run_state_changed"]
    );
}

#[test]
fn ensure_external_cli_recorder_rejects_an_unremovable_canonical_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-invalid-path";
    let data_dir = bifrost_agent::config::agent_home_dir();
    let canonical = bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key);
    std::fs::create_dir_all(&canonical).unwrap();
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    assert!(recorder.is_none());
}

#[cfg(unix)]
#[test]
fn ensure_external_cli_recorder_handles_metadata_write_failures() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-readonly";
    let data_dir = bifrost_agent::config::agent_home_dir();
    let parent = bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key)
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::create_dir_all(&parent).unwrap();
    let original_mode = std::fs::metadata(&parent).unwrap().permissions().mode();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(recorder.is_some());
    assert!(!recorder.unwrap().file_path().exists());
}

#[test]
fn external_cli_progress_runner_summary_uses_session_effort_override() {
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        message: "hello".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("feishu-main".to_string()),
        runner_id: Some("Traex".to_string()),
        session_key: Some("feishu-main:owner-open-id".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        work_dir: Some(std::path::PathBuf::from("/tmp/bifrost")),
        instructions: None,
        adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
            model: Some("GPT-5.5".to_string()),
            reasoning_effort: Some("high".to_string()),
            config_overrides: vec![
                "model_reasoning_effort=\"xhigh\"".to_string(),
                "model_provider=\"trae\"".to_string(),
            ],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("modelReasoningEffort".to_string(), "xhigh".to_string());

    let summary = external_cli_progress_runner_summary(
        "Traex",
        crate::im_gateway::external_cli::TRAEX_ADAPTER,
        &request,
        Some(&metadata),
    );

    assert_eq!(summary.model.as_deref(), Some("GPT-5.5"));
    assert_eq!(summary.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(summary.reasoning_source.as_deref(), Some("runner 配置"));
}

#[test]
fn external_cli_progress_runner_summary_reads_weekly_usage_metadata() {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("codexWeeklyUsedPercent".to_string(), "140".to_string());
    metadata.insert("codexWeeklyWindowMinutes".to_string(), "10080".to_string());
    metadata.insert("codexWeeklyResetsAt".to_string(), "1784490086".to_string());

    let usage = external_cli_weekly_usage_from_metadata(&metadata)
        .expect("valid weekly metadata should be rendered");

    assert_eq!(usage.used_percent, 100);
    assert_eq!(usage.window_minutes, 10_080);
    assert_eq!(usage.resets_at, Some(1_784_490_086));
    metadata.remove("codexWeeklyWindowMinutes");
    assert!(external_cli_weekly_usage_from_metadata(&metadata).is_none());
}

#[test]
fn external_cli_progress_session_id_flows_from_live_event_to_feishu_card() {
    let request = recorder_test_request("feishu-main:owner-open-id");
    let event = crate::im_gateway::external_cli::parse_progress_events(
        r#"{"type":"thread.started","thread_id":"thread-live-card"}"#,
    )
    .pop()
    .expect("thread started event");
    let mut metadata = std::collections::BTreeMap::new();
    assert!(
        crate::im_gateway::external_cli::merge_external_cli_progress_metadata(
            "codex",
            &event,
            &mut metadata,
        )
    );
    let runner = external_cli_progress_runner_summary("codex", "codex", &request, Some(&metadata));
    let mut snapshot =
        crate::im_gateway::progress_card::ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(runner);

    let card = crate::im_gateway::progress_card::build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).expect("serialize progress card");

    assert!(
        serialized.contains("Runner：`codex` · Adapter：`codex` · Session ID：`thread-live-card`")
    );
}

fn external_cli_result_with_status(
    status: crate::im_gateway::external_cli::ExternalCliRunStatus,
) -> crate::im_gateway::external_cli::ExternalCliRunResult {
    crate::im_gateway::external_cli::ExternalCliRunResult {
        run_id: "run-timeout".to_string(),
        session_key: Some("s1".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "traex".to_string(),
        status,
        exit_code: None,
        response: "early agent message".to_string(),
        responses: Vec::new(),
        started_at: 1,
        finished_at: 181_000,
        duration_ms: 180_999,
        artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
            run_dir: String::new(),
            prompt: String::new(),
            command_snapshot: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            normalized_events: String::new(),
            last_message: String::new(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::new(),
    }
}

#[test]
fn timed_out_external_cli_result_reports_failure_reply() {
    let result = external_cli_result_with_status(
        crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut,
    );

    let reply = external_cli_non_success_reply(&result);

    assert!(reply.contains("timed out after 180 seconds"));
    assert!(reply.contains("run-timeout"));
    assert!(!reply.contains("early agent message"));
}

async fn exercise_external_runner_terminal_notification(
    server: &crate::im_gateway::progress_card::tests::MockFeishuProgressServer,
    final_text: &str,
    failed: bool,
) -> (
    Arc<tokio::sync::Mutex<crate::im_gateway::progress_card::FeishuProgressCardSession>>,
    Arc<ImMessageLogStore>,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().expect("terminal notification message store");
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));
    let progress_registry = Arc::new(ImAgentProgressRegistry::new());
    let group_context_store = Arc::new(ImGroupContextStore::new(temp.path()));
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let feishu = Arc::new(crate::im_gateway::feishu::FeishuProvider::new());
    let client = ImProviderClient::Feishu(Arc::clone(&feishu));
    let event = group_test_event(
        &provider.id,
        "terminal-notification-trigger",
        "run a long task",
        false,
        1,
    );
    let session = progress_registry
        .start_feishu_replying_to(
            "terminal-notification-session",
            feishu,
            provider.clone(),
            crate::im_gateway::progress_card::tests::mock_progress_target(),
            "run a long task",
            event.source.message_id.as_deref(),
        )
        .await
        .expect("start progress card");

    finish_external_runner_progress_and_notify(
        ExternalRunnerProgressFinishContext {
            progress_registry: &progress_registry,
            client: &client,
            provider: &provider,
            message_log_store: &message_log_store,
            group_context_store: &group_context_store,
            event: &event,
        },
        ExternalRunnerProgressFinish {
            session_key: "terminal-notification-session",
            final_text,
            failed,
            work_dir: None,
            anchor: None,
        },
    )
    .await;

    (session, message_log_store, temp)
}

#[tokio::test(flavor = "current_thread")]
async fn external_runner_success_finishes_progress_card_and_sends_terminal_summary() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let final_summary = "FINAL_SUMMARY_MARKER：全部验证通过。";
    let (session, message_log_store, _temp) =
        exercise_external_runner_terminal_notification(&server, final_summary, false).await;

    assert_eq!(
        session.lock().await.snapshot().phase,
        crate::im_gateway::progress_card::ImProgressPhase::Finished
    );
    let updates = server
        .card_update_payloads
        .lock()
        .expect("card updates lock")
        .clone();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].contains(final_summary));

    let messages = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    assert_eq!(messages.len(), 2, "progress card plus terminal reply");
    let terminal_card: serde_json::Value = serde_json::from_str(
        messages[1]["content"]
            .as_str()
            .expect("terminal card content"),
    )
    .expect("terminal card json");
    assert_eq!(terminal_card["header"]["template"], "green");
    assert_eq!(
        terminal_card["header"]["title"]["content"],
        "Task completed"
    );
    assert_eq!(
        terminal_card["header"]["title"]["i18n_content"]["zh_cn"],
        "任务执行结束"
    );
    assert_eq!(
        terminal_card["header"]["title"]["i18n_content"]["ja_jp"],
        "タスク実行完了"
    );
    let terminal_body = serde_json::to_string(&terminal_card["body"]).unwrap();
    assert!(terminal_body.contains(final_summary));
    let paths = server
        .message_paths
        .lock()
        .expect("message paths lock")
        .clone();
    assert_eq!(
        paths[1], "/open-apis/im/v1/messages/om_1/reply",
        "terminal card must directly quote the progress card"
    );

    let logs = message_log_store.list();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].status, MessageStatus::Success);
    assert!(logs[0]
        .content
        .as_deref()
        .is_some_and(|content| content.contains(final_summary)));
}

#[tokio::test(flavor = "current_thread")]
async fn external_runner_failure_finishes_progress_card_and_sends_terminal_reason() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let failure_reason = "Runner failed: permission denied for workspace";
    let (session, message_log_store, _temp) =
        exercise_external_runner_terminal_notification(&server, failure_reason, true).await;

    assert_eq!(
        session.lock().await.snapshot().phase,
        crate::im_gateway::progress_card::ImProgressPhase::Failed
    );
    let updates = server
        .card_update_payloads
        .lock()
        .expect("card updates lock")
        .clone();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].contains(failure_reason));

    let messages = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    assert_eq!(messages.len(), 2, "progress card plus failure reply");
    let terminal_card: serde_json::Value = serde_json::from_str(
        messages[1]["content"]
            .as_str()
            .expect("failure card content"),
    )
    .expect("failure card json");
    assert_eq!(terminal_card["header"]["template"], "red");
    assert_eq!(terminal_card["header"]["title"]["content"], "Task failed");
    assert_eq!(
        terminal_card["header"]["title"]["i18n_content"]["zh_cn"],
        "任务执行失败"
    );
    let terminal_body = serde_json::to_string(&terminal_card["body"]).unwrap();
    assert!(terminal_body.contains(failure_reason));
    let paths = server
        .message_paths
        .lock()
        .expect("message paths lock")
        .clone();
    assert_eq!(paths[1], "/open-apis/im/v1/messages/om_1/reply");
    assert_eq!(message_log_store.list()[0].status, MessageStatus::Success);
}

#[tokio::test(flavor = "current_thread")]
async fn external_runner_terminal_send_failure_does_not_rollback_finished_progress_card() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server_with_send_failure(
        Some(2),
    )
    .await;
    let final_summary = "CARD_FINISHED_BEFORE_NOTIFICATION_FAILURE";
    let (session, message_log_store, _temp) =
        exercise_external_runner_terminal_notification(&server, final_summary, false).await;

    assert_eq!(
        session.lock().await.snapshot().phase,
        crate::im_gateway::progress_card::ImProgressPhase::Finished
    );
    let updates = server
        .card_update_payloads
        .lock()
        .expect("card updates lock")
        .clone();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].contains(final_summary));
    let logs = message_log_store.list();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].status, MessageStatus::Failed);
    assert!(logs[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("send denied")));
}

#[tokio::test(flavor = "current_thread")]
async fn external_runner_terminal_without_reply_id_uses_provider_send_fallback() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().expect("terminal fallback message store");
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut event = group_test_event(
        &provider.id,
        "terminal-fallback-trigger",
        "run a task",
        false,
        1,
    );
    event.source.message_id = None;

    send_external_runner_terminal_reply_from_work_dir(
        &client,
        &provider,
        &event,
        ExternalRunnerTerminalReply {
            text: "   ",
            failed: false,
            progress_message_id: None,
            work_dir: None,
        },
        &message_log_store,
    )
    .await;

    let paths = server
        .message_paths
        .lock()
        .expect("message paths lock")
        .clone();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "/open-apis/im/v1/messages");
    let payloads = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    let card: serde_json::Value = serde_json::from_str(
        payloads[0]["content"]
            .as_str()
            .expect("fallback card content"),
    )
    .expect("fallback card json");
    assert_eq!(card["header"]["title"]["content"], "Task completed");
    assert_eq!(
        card["header"]["title"]["i18n_content"]["zh_cn"],
        "任务执行结束"
    );
    assert_eq!(card["body"]["elements"][0]["content"], "—");
    assert_eq!(message_log_store.list()[0].status, MessageStatus::Success);
}

#[cfg(unix)]
async fn exercise_external_runner_control_flow_with_progress(
    server: &crate::im_gateway::progress_card::tests::MockFeishuProgressServer,
    executable: &str,
    args: Vec<String>,
    session_key: &str,
) -> (
    crate::handlers::im_gateway::ImGatewayService,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().expect("external runner control flow data dir");
    let guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let mut provider =
        crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    provider.id = format!("provider-{session_key}");
    service.provider_store.add(provider.clone()).unwrap();

    let mut config = service.external_cli_config_store.load();
    let runner_id = config.default_runner_id.clone();
    let runner = config.runners.get_mut(&runner_id).expect("default runner");
    runner.enabled = true;
    runner.adapter = "custom".to_string();
    runner.inject_bifrost_tools = false;
    runner.delivery_mode = crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    runner.adapter_config.executable = Some(executable.to_string());
    runner.adapter_config.args = args;
    runner.adapter_config.timeout_secs = Some(10);
    service.external_cli_config_store.save(config).unwrap();

    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let event = group_test_event(
        &provider.id,
        &format!("trigger-{session_key}"),
        "run the terminal notification test",
        false,
        1,
    );
    let (_tx, mut rx) = mpsc::unbounded_channel();
    run_external_cli_agent_chat(
        ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_config_store: &service.agent_config_store,
            external_cli_config_store: &service.external_cli_config_store,
            agent_session_manager: &service.agent_session_manager,
            queue_manager: &service.queue_manager,
            progress_registry: &service.progress_registry,
            event_store: &service.event_store,
            group_context_store: &service.group_context_store,
        },
        ExternalCliChatInput {
            message_text: "run the terminal notification test".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key: session_key.to_string(),
            adapter_override: None,
            instructions_override: None,
            delivery_override: Some(
                crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard,
            ),
            runner_id_override: None,
            runner_selected: true,
            group_turn_id: None,
            reset_group_context: false,
            thread_anchor_message_id: None,
            thread_fallback_message: None,
        },
    )
    .await;
    drop(guard);

    (service, temp)
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn external_runner_success_control_flow_sends_terminal_card() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let script = "cat >/dev/null; printf '%s\\n' '{\"type\":\"assistant_final\",\"content\":\"CONTROL_FLOW_FINAL_SUMMARY\"}'";
    let (service, _temp) = exercise_external_runner_control_flow_with_progress(
        &server,
        "/bin/sh",
        vec!["-c".to_string(), script.to_string()],
        "terminal-success-control-flow",
    )
    .await;

    let messages = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    assert_eq!(messages.len(), 2, "progress card plus terminal card");
    assert!(messages[1]["content"]
        .as_str()
        .is_some_and(|content| content.contains("CONTROL_FLOW_FINAL_SUMMARY")));
    assert_eq!(
        service.message_log_store.list()[0].status,
        MessageStatus::Success
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn external_runner_spawn_error_control_flow_sends_failure_terminal_card() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let (service, _temp) = exercise_external_runner_control_flow_with_progress(
        &server,
        "/definitely/missing/bifrost-terminal-runner",
        Vec::new(),
        "terminal-spawn-error-control-flow",
    )
    .await;

    let messages = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    assert_eq!(
        messages.len(),
        2,
        "progress card plus failure terminal card"
    );
    let terminal = messages[1]["content"]
        .as_str()
        .expect("terminal failure card content");
    assert!(terminal.contains("Task failed"));
    assert!(terminal.contains("Runner failed:"));
    assert_eq!(
        service.message_log_store.list()[0].status,
        MessageStatus::Success
    );
}

#[tokio::test]
async fn external_runner_small_branches_keep_safe_defaults() {
    assert_eq!(
        external_cli_adapter_label(crate::im_gateway::chatgpt_web::ADAPTER_ID),
        "ChatGPT Web"
    );
    assert_eq!(external_cli_adapter_label("traex"), "Runner");

    let mut without_message = group_test_event("provider", "missing", "", false, 1);
    without_message.message = None;
    maybe_stop_external_cli_for_event(&without_message, "unrelated").await;

    let non_stop = group_test_event("provider", "continue", "continue", false, 2);
    maybe_stop_external_cli_for_event(&non_stop, "unrelated").await;

    let other_session = group_test_event("provider", "stop", "/stop", false, 3);
    maybe_stop_external_cli_for_event(&other_session, "unrelated").await;
}
