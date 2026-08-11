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

#[tokio::test(flavor = "current_thread")]
async fn standard_feishu_reply_keeps_uploaded_image_inline_without_image_message() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().expect("standard reply image store");
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let event = group_test_event(&provider.id, "standard-image-trigger", "render", false, 1);
    tokio::fs::write(temp.path().join("standard.png"), b"standard-image-bytes")
        .await
        .expect("write standard image");

    send_agent_reply_from_work_dir(
        &client,
        &provider,
        &event,
        "before ![standard](./standard.png) after",
        &message_log_store,
        Some(temp.path()),
    )
    .await;

    assert_eq!(
        server
            .image_upload_payloads
            .lock()
            .expect("image upload payloads lock")
            .len(),
        1
    );
    let payloads = server
        .message_payloads
        .lock()
        .expect("message payloads lock")
        .clone();
    assert_eq!(payloads.len(), 1, "only the interactive card is sent");
    assert_eq!(payloads[0]["msg_type"], "interactive");
    let card: serde_json::Value =
        serde_json::from_str(payloads[0]["content"].as_str().expect("card content"))
            .expect("card json");
    assert_eq!(
        card["body"]["elements"][0]["content"],
        "before ![standard](img_v3_progress_inline) after"
    );
    assert!(message_log_store
        .list()
        .iter()
        .all(|log| log.msg_type.as_deref() != Some("image")));
}

#[tokio::test(flavor = "current_thread")]
async fn planned_feishu_reply_keeps_uploaded_image_inline_without_image_message() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().expect("planned reply image store");
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let event = group_test_event(&provider.id, "planned-image-trigger", "render", false, 1);
    let image_path = temp.path().join("planned.png");
    tokio::fs::write(&image_path, b"planned-image-bytes")
        .await
        .expect("write planned image");
    let reply = format!("planned ![chart]({})", image_path.display());
    let plan = [PlanStep {
        step: "Render chart".to_string(),
        status: bifrost_agent::PlanStepStatus::Completed,
    }];
    let tool_calls = [ToolCallLog {
        tool_name: "render".to_string(),
        arguments: "{}".to_string(),
        result: "done".to_string(),
        success: true,
    }];

    send_agent_reply_with_plan(
        &client,
        &provider,
        &event,
        &reply,
        Some(&plan),
        &tool_calls,
        Some("Planned reply"),
        &message_log_store,
    )
    .await;

    assert_eq!(server.image_upload_payloads.lock().unwrap().len(), 1);
    let payloads = server.message_payloads.lock().unwrap().clone();
    assert_eq!(payloads.len(), 1, "only the interactive card is sent");
    let card: serde_json::Value =
        serde_json::from_str(payloads[0]["content"].as_str().expect("card content"))
            .expect("card json");
    assert_eq!(
        card["body"]["elements"][0]["content"],
        "planned ![chart](img_v3_progress_inline)"
    );
    assert_eq!(card["body"]["elements"][1]["tag"], "collapsible_panel");
    assert_eq!(card["body"]["elements"][2]["tag"], "collapsible_panel");
    assert!(message_log_store
        .list()
        .iter()
        .all(|log| log.msg_type.as_deref() != Some("image")));
}

#[tokio::test(flavor = "current_thread")]
async fn topic_terminal_without_progress_card_replies_in_thread_instead_of_main_group() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().unwrap();
    let logs = Arc::new(ImMessageLogStore::new(temp.path()));
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut event = group_test_event(&provider.id, "topic-trigger", "run", false, 1);
    let message = event.message.as_mut().unwrap();
    message.root_id = Some("root-card".to_string());
    message.parent_id = Some("root-card".to_string());
    message.thread_id = Some("topic-1".to_string());

    send_external_runner_terminal_reply_from_work_dir(
        &client,
        &provider,
        &event,
        ExternalRunnerTerminalReply {
            text: "done",
            failed: false,
            progress_message_id: None,
            work_dir: None,
        },
        &logs,
    )
    .await;

    let paths = server.message_paths.lock().unwrap().clone();
    assert_eq!(paths, ["/open-apis/im/v1/messages/topic-trigger/reply"]);
    let payloads = server.message_payloads.lock().unwrap().clone();
    assert_eq!(payloads[0]["reply_in_thread"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn progress_and_terminal_cards_are_both_persisted_as_derivation_anchors() {
    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().unwrap();
    let logs = Arc::new(ImMessageLogStore::new(temp.path()));
    let group_store = Arc::new(ImGroupContextStore::new(temp.path()));
    let registry = Arc::new(ImAgentProgressRegistry::new());
    let provider = crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    let feishu = Arc::new(crate::im_gateway::feishu::FeishuProvider::new());
    let client = ImProviderClient::Feishu(Arc::clone(&feishu));
    let event = group_test_event(&provider.id, "anchor-trigger", "run", false, 1);
    registry
        .start_feishu_replying_to(
            "anchor-session",
            feishu,
            provider.clone(),
            crate::im_gateway::progress_card::tests::mock_progress_target(),
            "run",
            event.source.message_id.as_deref(),
        )
        .await
        .unwrap();
    let anchor = crate::im_gateway::group_context::FeishuMessageAnchor {
        provider_id: provider.id.clone(),
        chat_id: "oc_group".to_string(),
        message_id: String::new(),
        source_session_key: "anchor-session".to_string(),
        run_id: Some("run-1".to_string()),
        runner_id: "Codex".to_string(),
        adapter: "codex".to_string(),
        transport: "app_server".to_string(),
        external_thread_id: Some("thread-1".to_string()),
        external_turn_id: Some("turn-1".to_string()),
        checkpoint_thread_id: None,
        status: "ready".to_string(),
    };

    finish_external_runner_progress_and_notify(
        ExternalRunnerProgressFinishContext {
            progress_registry: &registry,
            client: &client,
            provider: &provider,
            message_log_store: &logs,
            group_context_store: &group_store,
            event: &event,
        },
        ExternalRunnerProgressFinish {
            session_key: "anchor-session",
            final_text: "done",
            failed: false,
            work_dir: None,
            anchor: Some(anchor),
        },
    )
    .await;

    assert!(group_store
        .feishu_message_anchor(&provider.id, "om_1")
        .unwrap()
        .is_some());
    assert!(group_store
        .feishu_message_anchor(&provider.id, "om_2")
        .unwrap()
        .is_some());
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
async fn external_runner_derives_ready_codex_anchor_and_keeps_cards_in_topic() {
    use std::os::unix::fs::PermissionsExt;

    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().expect("derived external runner data dir");
    let guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let executable = temp.path().join("mock-codex-topic-fork");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value, separators=(",", ":")), flush=True)
for line in sys.stdin:
    frame = json.loads(line); method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/fork":
        assert frame["params"] == {"threadId":"thread-source","lastTurnId":"turn-source"}
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"thread-derived"}}})
    elif method == "turn/start":
        assert frame["params"]["threadId"] == "thread-derived"
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"turn-derived"}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-derived","turnId":"turn-derived","item":{"id":"message","type":"agentMessage","text":"TOPIC_DERIVED_OK"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-derived","turn":{"id":"turn-derived","status":"completed"}}})
    elif method == "account/rateLimits/read": send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let mut provider =
        crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    provider.id = "provider-derived-topic".to_string();
    service.provider_store.add(provider.clone()).unwrap();
    let mut config = service.external_cli_config_store.load();
    let runner_id = config.default_runner_id.clone();
    let runner = config.runners.get_mut(&runner_id).unwrap();
    runner.enabled = true;
    runner.adapter = "codex".to_string();
    runner.inject_bifrost_tools = false;
    runner.delivery_mode = crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    runner.adapter_config.transport =
        Some(crate::im_gateway::external_cli::ExternalCliTransport::AppServer);
    runner.adapter_config.executable = Some(executable.display().to_string());
    runner.adapter_config.args.clear();
    runner.adapter_config.timeout_secs = Some(10);
    service.external_cli_config_store.save(config).unwrap();

    let source_anchor = crate::im_gateway::group_context::FeishuMessageAnchor {
        provider_id: provider.id.clone(),
        chat_id: "oc_group".to_string(),
        message_id: "source-card".to_string(),
        source_session_key: "source-session".to_string(),
        run_id: Some("source-run".to_string()),
        runner_id: runner_id.clone(),
        adapter: "codex".to_string(),
        transport: "app_server".to_string(),
        external_thread_id: Some("thread-source".to_string()),
        external_turn_id: Some("turn-source".to_string()),
        checkpoint_thread_id: None,
        status: "ready".to_string(),
    };
    service
        .group_context_store
        .upsert_feishu_message_anchor(&source_anchor, 1)
        .unwrap();

    let session_key = crate::im_gateway::group_context::build_group_thread_session_key(
        &provider.id,
        "oc_group",
        "topic-derived",
    );
    let mut event = group_test_event(
        &provider.id,
        "topic-trigger",
        "continue from the card",
        false,
        2,
    );
    let message = event.message.as_mut().unwrap();
    message.root_id = Some("source-card".to_string());
    message.parent_id = Some("source-card".to_string());
    message.thread_id = Some("topic-derived".to_string());
    service
        .group_context_store
        .claim_feishu_thread_binding(
            &crate::im_gateway::group_context::FeishuThreadBinding {
                provider_id: provider.id.clone(),
                chat_id: "oc_group".to_string(),
                feishu_thread_id: "topic-derived".to_string(),
                root_message_id: "source-card".to_string(),
                derived_session_key: session_key.clone(),
                source_kind: "local_checkpoint".to_string(),
                source_message_id: "source-card".to_string(),
                source_adapter: Some("codex".to_string()),
                source_thread_id: Some("thread-source".to_string()),
                source_turn_id: Some("turn-source".to_string()),
                trigger_message_id: "topic-trigger".to_string(),
                initial_message: "continue from the card".to_string(),
                fallback_message: None,
                initial_event_json: Some(serde_json::to_string(&event).unwrap()),
                state: "initializing".to_string(),
            },
            2,
        )
        .unwrap();

    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
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
            message_text: "continue from the card".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key: session_key.clone(),
            adapter_override: Some("codex".to_string()),
            instructions_override: None,
            delivery_override: Some(
                crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard,
            ),
            runner_id_override: Some(runner_id),
            runner_selected: true,
            group_turn_id: None,
            reset_group_context: false,
            thread_anchor_message_id: Some("source-card".to_string()),
            thread_fallback_message: None,
        },
    )
    .await;

    let binding = service
        .group_context_store
        .feishu_thread_binding(&provider.id, "oc_group", "topic-derived")
        .unwrap()
        .unwrap();
    assert_eq!(binding.state, "ready");
    let paths = server.message_paths.lock().unwrap().clone();
    assert!(paths.len() >= 2, "progress and terminal replies: {paths:?}");
    let payloads = server.message_payloads.lock().unwrap().clone();
    assert!(payloads.iter().all(|payload| {
        payload["reply_in_thread"].as_bool() == Some(true)
            || payload["content"]
                .as_str()
                .is_some_and(|content| content.contains("TOPIC_DERIVED_OK"))
    }));
    assert!(service
        .group_context_store
        .feishu_message_anchor(&provider.id, "om_1")
        .unwrap()
        .is_some());
    drop(guard);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn external_runner_traex_completion_creates_immutable_topic_checkpoint() {
    use std::os::unix::fs::PermissionsExt;

    let server = crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
    let temp = tempfile::tempdir().expect("Traex checkpoint data dir");
    let guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let executable = temp.path().join("mock-traex-topic-checkpoint");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value, separators=(",", ":")), flush=True)
for line in sys.stdin:
    frame = json.loads(line); method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":"traex-finished"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"traex-finished"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"traex-turn"}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"traex-finished","turnId":"traex-turn","item":{"id":"message","type":"agentMessage","text":"TRAEX_TOPIC_OK"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"traex-finished","turn":{"id":"traex-turn","status":"completed"}}})
    elif method == "thread/fork":
        assert frame["params"] == {"threadId":"traex-finished"}
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"traex-immutable-checkpoint"}}})
    elif method == "account/rateLimits/read": send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let mut provider =
        crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
    provider.id = "provider-traex-topic".to_string();
    service.provider_store.add(provider.clone()).unwrap();
    let mut config = service.external_cli_config_store.load();
    let runner_id = config.default_runner_id.clone();
    let runner = config.runners.get_mut(&runner_id).unwrap();
    runner.enabled = true;
    runner.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
    runner.inject_bifrost_tools = false;
    runner.delivery_mode = crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    runner.adapter_config.transport =
        Some(crate::im_gateway::external_cli::ExternalCliTransport::AppServer);
    runner.adapter_config.executable = Some(executable.display().to_string());
    runner.adapter_config.args.clear();
    runner.adapter_config.timeout_secs = Some(10);
    service.external_cli_config_store.save(config).unwrap();

    let session_key = crate::im_gateway::group_context::build_group_thread_session_key(
        &provider.id,
        "oc_group",
        "topic-traex",
    );
    let mut event = group_test_event(
        &provider.id,
        "topic-traex-trigger",
        "run Traex in this topic",
        false,
        2,
    );
    let message = event.message.as_mut().unwrap();
    message.root_id = Some("ordinary-root".to_string());
    message.parent_id = Some("ordinary-root".to_string());
    message.thread_id = Some("topic-traex".to_string());
    service
        .group_context_store
        .claim_feishu_thread_binding(
            &crate::im_gateway::group_context::FeishuThreadBinding {
                provider_id: provider.id.clone(),
                chat_id: "oc_group".to_string(),
                feishu_thread_id: "topic-traex".to_string(),
                root_message_id: "ordinary-root".to_string(),
                derived_session_key: session_key.clone(),
                source_kind: "message_context".to_string(),
                source_message_id: "ordinary-root".to_string(),
                source_adapter: None,
                source_thread_id: None,
                source_turn_id: None,
                trigger_message_id: "topic-traex-trigger".to_string(),
                initial_message: "run Traex in this topic".to_string(),
                fallback_message: None,
                initial_event_json: Some(serde_json::to_string(&event).unwrap()),
                state: "initializing".to_string(),
            },
            2,
        )
        .unwrap();

    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
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
            message_text: "run Traex in this topic".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key,
            adapter_override: Some(crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string()),
            instructions_override: None,
            delivery_override: Some(
                crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard,
            ),
            runner_id_override: Some(runner_id),
            runner_selected: true,
            group_turn_id: None,
            reset_group_context: false,
            thread_anchor_message_id: None,
            thread_fallback_message: None,
        },
    )
    .await;

    let binding = service
        .group_context_store
        .feishu_thread_binding(&provider.id, "oc_group", "topic-traex")
        .unwrap()
        .unwrap();
    assert_eq!(binding.state, "ready");
    let anchor = service
        .group_context_store
        .feishu_message_anchor(&provider.id, "om_1")
        .unwrap()
        .expect("Traex topic card anchor");
    assert_eq!(anchor.external_thread_id.as_deref(), Some("traex-finished"));
    assert_eq!(
        anchor.checkpoint_thread_id.as_deref(),
        Some("traex-immutable-checkpoint")
    );
    assert_eq!(anchor.status, "ready");
    drop(guard);
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

#[test]
fn thread_derivation_anchor_is_consumed_once_for_queued_turns() {
    let mut anchor = Some("source-card".to_string());
    assert_eq!(
        take_thread_derivation_anchor(&mut anchor).as_deref(),
        Some("source-card")
    );
    assert!(take_thread_derivation_anchor(&mut anchor).is_none());
}

#[test]
fn traex_checkpoint_requires_app_server_fork_capability() {
    let mut app_server = recorder_test_request("traex-checkpoint-capability");
    app_server.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
    assert!(should_create_traex_checkpoint(true, "traex", &app_server));

    let mut exec = app_server.clone();
    exec.adapter_config.transport =
        Some(crate::im_gateway::external_cli::ExternalCliTransport::Exec);
    assert!(!should_create_traex_checkpoint(true, "traex", &exec));
    assert!(!should_create_traex_checkpoint(false, "traex", &app_server));
    assert!(!should_create_traex_checkpoint(true, "codex", &app_server));
}

#[tokio::test]
async fn thread_anchor_request_planning_covers_active_wait_fallback_and_missing_sources() {
    fn anchor(
        provider_id: &str,
        message_id: &str,
        source_session_key: &str,
        status: &str,
    ) -> crate::im_gateway::group_context::FeishuMessageAnchor {
        crate::im_gateway::group_context::FeishuMessageAnchor {
            provider_id: provider_id.to_string(),
            chat_id: "oc_group".to_string(),
            message_id: message_id.to_string(),
            source_session_key: source_session_key.to_string(),
            run_id: Some("run".to_string()),
            runner_id: "Codex".to_string(),
            adapter: "codex".to_string(),
            transport: "app_server".to_string(),
            external_thread_id: Some("source-thread".to_string()),
            external_turn_id: Some("source-turn".to_string()),
            checkpoint_thread_id: None,
            status: status.to_string(),
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider_id = "thread-plan-provider";

    let mut active = anchor(provider_id, "active", "source-active", "active_ready");
    service
        .group_context_store
        .upsert_feishu_message_anchor(&active, 1)
        .unwrap();
    let mut request = recorder_test_request("active-derived");
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "active",
        &mut request,
        Some("unused fallback"),
    )
    .await;
    assert_eq!(request.operation, "fork");
    assert_eq!(request.params["threadId"], "source-thread");
    assert!(request.params["lastTurnId"].is_null());

    active.message_id = "no-thread".to_string();
    active.external_thread_id = None;
    active.external_turn_id = None;
    service
        .group_context_store
        .upsert_feishu_message_anchor(&active, 2)
        .unwrap();
    let mut no_thread = recorder_test_request("no-thread-derived");
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "no-thread",
        &mut no_thread,
        None,
    )
    .await;
    assert_eq!(no_thread.operation, "chat");

    let pending = anchor(provider_id, "pending", "source-pending", "pending");
    service
        .group_context_store
        .upsert_feishu_message_anchor(&pending, 3)
        .unwrap();
    let mut fallback = recorder_test_request("pending-fallback");
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "pending",
        &mut fallback,
        Some("root plus current"),
    )
    .await;
    assert_eq!(fallback.operation, "chat");
    assert_eq!(fallback.message, "root plus current");

    let failed = anchor(provider_id, "failed", "source-failed", "failed");
    service
        .group_context_store
        .upsert_feishu_message_anchor(&failed, 4)
        .unwrap();
    let mut failed_fallback = recorder_test_request("failed-anchor");
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "failed",
        &mut failed_fallback,
        Some("unused failed fallback"),
    )
    .await;
    assert_eq!(failed_fallback.operation, "chat");
    assert_eq!(failed_fallback.message, "unused failed fallback");

    let held_source = service
        .agent_session_manager
        .try_take_session("source-running")
        .unwrap();
    let waiting = anchor(provider_id, "waiting", "source-running", "pending");
    service
        .group_context_store
        .upsert_feishu_message_anchor(&waiting, 4)
        .unwrap();
    let store = Arc::clone(&service.group_context_store);
    let manager = Arc::clone(&service.agent_session_manager);
    let wait_task = tokio::spawn(async move {
        let mut request = recorder_test_request("waiting-derived");
        apply_thread_anchor_to_request(
            &store,
            &manager,
            provider_id,
            "waiting",
            &mut request,
            Some("must not fall back"),
        )
        .await;
        request
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let mut ready = waiting;
    ready.status = "ready".to_string();
    ready.checkpoint_thread_id = Some("immutable-checkpoint".to_string());
    service
        .group_context_store
        .upsert_feishu_message_anchor(&ready, 5)
        .unwrap();
    let derived = wait_task.await.unwrap();
    assert_eq!(derived.operation, "fork");
    assert_eq!(derived.params["threadId"], "immutable-checkpoint");
    assert_eq!(derived.params["lastTurnId"], "source-turn");
    service.agent_session_manager.return_session(held_source);

    let held_failed_source = service
        .agent_session_manager
        .try_take_session("source-running-failed")
        .unwrap();
    let pending_then_failed = anchor(
        provider_id,
        "pending-then-failed",
        "source-running-failed",
        "pending",
    );
    service
        .group_context_store
        .upsert_feishu_message_anchor(&pending_then_failed, 6)
        .unwrap();
    let store = Arc::clone(&service.group_context_store);
    let manager = Arc::clone(&service.agent_session_manager);
    let failed_wait_task = tokio::spawn(async move {
        let mut request = recorder_test_request("waiting-for-failed-source");
        apply_thread_anchor_to_request(
            &store,
            &manager,
            provider_id,
            "pending-then-failed",
            &mut request,
            Some("root plus current after failure"),
        )
        .await;
        request
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let mut failed_update = pending_then_failed;
    failed_update.status = "failed".to_string();
    service
        .group_context_store
        .upsert_feishu_message_anchor(&failed_update, 7)
        .unwrap();
    let failed_after_wait = failed_wait_task.await.unwrap();
    assert_eq!(failed_after_wait.operation, "chat");
    assert_eq!(failed_after_wait.message, "root plus current after failure");
    service
        .agent_session_manager
        .return_session(held_failed_source);

    let held_missing_source = service
        .agent_session_manager
        .try_take_session("source-running-missing")
        .unwrap();
    let pending_then_missing = anchor(
        provider_id,
        "pending-then-missing",
        "source-running-missing",
        "pending",
    );
    service
        .group_context_store
        .upsert_feishu_message_anchor(&pending_then_missing, 8)
        .unwrap();
    let store = Arc::clone(&service.group_context_store);
    let manager = Arc::clone(&service.agent_session_manager);
    let missing_wait_task = tokio::spawn(async move {
        let mut request = recorder_test_request("waiting-for-missing-source");
        apply_thread_anchor_to_request(
            &store,
            &manager,
            provider_id,
            "pending-then-missing",
            &mut request,
            Some("root plus current after anchor removal"),
        )
        .await;
        request
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .execute(
            "DELETE FROM im_feishu_message_anchors WHERE provider_id = ?1 AND message_id = ?2",
            rusqlite::params![provider_id, "pending-then-missing"],
        )
        .unwrap();
    let missing_after_wait =
        tokio::time::timeout(std::time::Duration::from_secs(1), missing_wait_task)
            .await
            .expect("removed anchor must not wait for the active source session")
            .unwrap();
    assert_eq!(missing_after_wait.operation, "chat");
    assert_eq!(
        missing_after_wait.message,
        "root plus current after anchor removal"
    );
    service
        .agent_session_manager
        .return_session(held_missing_source);

    let mut missing = recorder_test_request("missing-anchor");
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "missing",
        &mut missing,
        Some("unused"),
    )
    .await;
    assert_eq!(missing.operation, "chat");
    assert_eq!(missing.message, "unused");

    rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .execute_batch("DROP TABLE im_feishu_message_anchors;")
        .unwrap();
    apply_thread_anchor_to_request(
        &service.group_context_store,
        &service.agent_session_manager,
        provider_id,
        "database-error",
        &mut missing,
        Some("database error fallback"),
    )
    .await;
    assert_eq!(missing.operation, "chat");
    assert_eq!(missing.message, "database error fallback");
}
