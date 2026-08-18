use super::*;
use rusqlite::OptionalExtension;

fn busy_group_event(message_id: &str, text: &str, received_at: u64) -> ImEvent {
    ImEvent {
        event_id: format!("event-{message_id}"),
        provider_id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("busy-group".to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some("ou_sender".to_string()),
            user_name: Some("Alice".to_string()),
            sender_type: Some("user".to_string()),
            message_id: Some(message_id.to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: text.to_string(),
            raw_type: Some("text".to_string()),
            create_time: Some(received_at),
            ..Default::default()
        }),
        received_at,
        raw_digest: None,
    }
}

fn busy_group_image_event(
    message_id: &str,
    text: &str,
    received_at: u64,
    bytes_base64: &str,
) -> ImEvent {
    let mut event = busy_group_event(message_id, text, received_at);
    let message = event.message.as_mut().expect("busy event message");
    message.raw_type = Some("image".to_string());
    message
        .images
        .push(crate::im_gateway::types::ImImageAttachment {
            file_key: format!("image-{message_id}"),
            mime_type: Some("image/png".to_string()),
            data_base64: Some(bytes_base64.to_string()),
            ..Default::default()
        });
    event
}

fn persisted_group_turn_status(store: &ImGroupContextStore, turn_id: &str) -> Option<String> {
    rusqlite::Connection::open(store.file_path())
        .unwrap()
        .query_row(
            "SELECT status FROM im_group_turns WHERE turn_id = ?1",
            rusqlite::params![turn_id],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn runner_config(
    runner_id: &str,
    adapter: &str,
) -> crate::im_gateway::external_cli::ExternalCliGatewayConfig {
    let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    config.runners.insert(
        runner_id.to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: true,
            adapter: adapter.to_string(),
            ..Default::default()
        },
    );
    config
}

#[test]
fn busy_default_mode_guides_external_runners_except_chatgpt_web() {
    for (runner_id, adapter) in [
        ("codex-main", "codex"),
        ("traex-main", "traex"),
        ("claude-main", "claude_code"),
    ] {
        let config = runner_config(runner_id, adapter);
        assert_eq!(
            busy_default_mode_for_agent_config(
                &bifrost_agent::AgentConfig {
                    runner: Some(bifrost_agent::AgentRunnerMode::Custom(
                        runner_id.to_string(),
                    )),
                    ..Default::default()
                },
                &config,
                Some("feishu-main"),
            ),
            BusyMessageDefaultMode::ExternalGuide,
            "adapter {adapter} should default to external guide",
        );
    }

    let config = runner_config("chatgpt-web", crate::im_gateway::chatgpt_web::ADAPTER_ID);
    assert_eq!(
        busy_default_mode_for_agent_config(
            &bifrost_agent::AgentConfig {
                runner: Some(bifrost_agent::AgentRunnerMode::Custom(
                    "chatgpt-web".to_string(),
                )),
                ..Default::default()
            },
            &config,
            Some("feishu-main"),
        ),
        BusyMessageDefaultMode::Queue
    );

    let mut default_config = runner_config("codex-main", "codex");
    default_config.default_runner_id = "codex-main".to_string();
    let implicit_agent_config = bifrost_agent::AgentConfig {
        runner: None,
        ..Default::default()
    };
    assert_eq!(
        busy_default_mode_for_agent_config(
            &implicit_agent_config,
            &default_config,
            Some("feishu-main"),
        ),
        BusyMessageDefaultMode::ExternalGuide,
        "an implicit default runner must use the same live-guide semantics as an explicit runner",
    );

    let mut default_chatgpt =
        runner_config("chatgpt-web", crate::im_gateway::chatgpt_web::ADAPTER_ID);
    default_chatgpt.default_runner_id = "chatgpt-web".to_string();
    assert_eq!(
        busy_default_mode_for_agent_config(
            &implicit_agent_config,
            &default_chatgpt,
            Some("feishu-main"),
        ),
        BusyMessageDefaultMode::Queue,
        "an implicit ChatGPT Web default must retain queue semantics",
    );
}

#[test]
fn apply_busy_message_default_guides_messages_without_queueing() {
    let manager = SessionQueueManager::new();

    let result = apply_busy_message_default(
        &manager,
        "busy-default-guide",
        "继续看最新日志",
        BusyMessageDefaultMode::Guide,
    )
    .expect("guide default should be accepted");

    assert_eq!(result, BusyMessageDefaultResult::Guide { pending_count: 1 });
    assert_eq!(
        manager.guide_status("busy-default-guide"),
        vec!["继续看最新日志".to_string()]
    );
    assert!(manager.queue_status("busy-default-guide").is_empty());
}

#[test]
fn merge_pending_guide_messages_keeps_worker_guides_and_deduplicates_queue_guides() {
    let merged = merge_pending_guide_messages(
        &["worker-guide".to_string(), "shared-guide".to_string()],
        vec!["shared-guide".to_string(), "queue-guide".to_string()],
    );

    assert_eq!(
        merged,
        vec![
            "worker-guide".to_string(),
            "shared-guide".to_string(),
            "queue-guide".to_string()
        ]
    );
}

#[test]
fn apply_busy_message_default_queues_custom_runner_messages() {
    let manager = SessionQueueManager::new();

    let result = apply_busy_message_default(
        &manager,
        "busy-default-queue",
        "下一条给 ChatGPT Web",
        BusyMessageDefaultMode::Queue,
    )
    .expect("queue default should be accepted");

    match result {
        BusyMessageDefaultResult::Queue { items } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].message, "下一条给 ChatGPT Web");
        }
        BusyMessageDefaultResult::Guide { .. } => panic!("custom runner default should queue"),
    }
    assert!(manager.guide_status("busy-default-queue").is_empty());
    assert_eq!(
        manager.pop_queue("busy-default-queue").as_deref(),
        Some("下一条给 ChatGPT Web")
    );
}

#[test]
fn queue_query_formats_empty_and_busy_thread_contents() {
    let manager = SessionQueueManager::new();
    let empty = format_queue_status("📋 当前线程排队消息", &manager.queue_status("queue-query"));
    assert!(empty.contains("当前线程排队消息"));
    assert!(empty.contains("排队已清空"));

    manager
        .push_queue("queue-query", "第一条等待处理的消息".to_string())
        .expect("queue first item");
    manager
        .push_queue("queue-query", "第二条等待处理的消息".to_string())
        .expect("queue second item");
    let busy = format_queue_status("📋 当前线程排队消息", &manager.queue_status("queue-query"));
    assert!(busy.contains("当前排队（2条）"));
    assert!(busy.contains("第一条等待处理的消息"));
    assert!(busy.contains("第二条等待处理的消息"));
}

#[tokio::test]
async fn busy_group_turns_complete_or_release_with_their_queue_outcome() {
    let temp = tempfile::tempdir().unwrap();
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider = test_provider();
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let agent_config = service.agent_config_store.load();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "busy-group");

    let completed_event = busy_group_event("complete", "guide", 1);
    service
        .group_context_store
        .record_event(&completed_event, "test")
        .unwrap();
    let completed_turn = service
        .group_context_store
        .prepare_turn(
            &completed_event,
            crate::im_gateway::group_context::GroupTriggerKind::Guide,
            "guide",
        )
        .unwrap();
    service
        .group_context_store
        .mark_turn_dispatched(&completed_turn.turn_id, 1)
        .unwrap();
    handle_busy_default_message(
        "guide",
        &session_key,
        &BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &completed_event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: Some(&completed_turn.turn_id),
            default_mode: BusyMessageDefaultMode::Guide,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;
    assert_eq!(
        persisted_group_turn_status(&service.group_context_store, &completed_turn.turn_id),
        Some("dispatched".to_string())
    );
    let deferred = service
        .queue_manager
        .pop_queue_item(&session_key)
        .expect("busy group guide should retain its turn in the queue");
    assert_eq!(deferred.message, "guide");
    assert_eq!(
        deferred.context.and_then(|context| context.group_turn_id),
        Some(completed_turn.turn_id.clone())
    );

    for index in 0..10 {
        service
            .queue_manager
            .push_queue(&session_key, format!("queued-{index}"))
            .unwrap();
    }
    for (message_id, text, guide_command, received_at) in [
        ("overflow", "overflow", false, 2),
        ("guide-overflow", "guide overflow", true, 3),
        ("empty", "empty", false, 4),
    ] {
        let event = busy_group_event(message_id, text, received_at);
        service
            .group_context_store
            .record_event(&event, "test")
            .unwrap();
        let turn = service
            .group_context_store
            .prepare_turn(
                &event,
                crate::im_gateway::group_context::GroupTriggerKind::Queue,
                text,
            )
            .unwrap();
        let context = BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: Some(&turn.turn_id),
            default_mode: BusyMessageDefaultMode::Queue,
            status_context: Default::default(),
            default_work_dir: None,
        };
        if guide_command {
            handle_busy_guide_command(text, &session_key, &context).await;
        } else if message_id == "empty" {
            handle_busy_default_message("", &session_key, &context).await;
        } else {
            handle_busy_default_message(text, &session_key, &context).await;
        }
        assert_eq!(
            persisted_group_turn_status(&service.group_context_store, &turn.turn_id),
            None
        );
    }
}

#[tokio::test]
async fn external_busy_image_is_persisted_and_guided_into_the_active_runner() {
    let _external_cli_guard = crate::im_gateway::external_cli::external_cli_test_env_lock().await;
    let temp = tempfile::tempdir().unwrap();
    let _data_dir_guard = EnvGuard::set_data_dir(temp.path());
    let _worker_env = EnvVarGuard::remove("BIFROST_IM_GATEWAY_WORKER");
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider = test_provider();
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let agent_config = service.agent_config_store.load();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "image-guide");
    let event = busy_group_image_event(
        "image-guide-accepted",
        "请结合截图继续定位",
        10,
        "cG5nLWd1aWRlLWJ5dGVz",
    );
    let mut active =
        crate::im_gateway::external_cli::install_test_active_guide_session(&session_key);
    let response = tokio::spawn(async move { active.respond_next(true).await });

    handle_busy_default_message(
        "请结合截图继续定位",
        &session_key,
        &BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: None,
            default_mode: BusyMessageDefaultMode::ExternalGuide,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;

    let (guide_id, prompt) = response
        .await
        .expect("join test active runner")
        .expect("respond to image guide");
    assert!(guide_id.starts_with("guide-"), "{guide_id}");
    assert!(prompt.contains("## Attached Images"), "{prompt}");
    assert!(prompt.ends_with("请结合截图继续定位\n"), "{prompt}");
    let image_path = prompt
        .lines()
        .find(|line| line.starts_with("1. `"))
        .and_then(|line| line.split('`').nth(1))
        .map(std::path::PathBuf::from)
        .expect("absolute image path in guide prompt");
    assert!(image_path.is_absolute(), "{}", image_path.display());
    assert_eq!(
        tokio::fs::read(&image_path).await.unwrap(),
        b"png-guide-bytes"
    );
    assert!(service.queue_manager.queue_status(&session_key).is_empty());
}

#[tokio::test]
async fn rejected_external_busy_image_keeps_original_text_and_bytes_in_fifo() {
    let _external_cli_guard = crate::im_gateway::external_cli::external_cli_test_env_lock().await;
    let temp = tempfile::tempdir().unwrap();
    let _data_dir_guard = EnvGuard::set_data_dir(temp.path());
    let _worker_env = EnvVarGuard::remove("BIFROST_IM_GATEWAY_WORKER");
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider = test_provider();
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let agent_config = service.agent_config_store.load();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "image-reject");
    let event = busy_group_image_event(
        "image-guide-rejected",
        "保留这张截图稍后处理",
        11,
        "cmVqZWN0ZWQtaW1hZ2UtYnl0ZXM=",
    );
    let mut active =
        crate::im_gateway::external_cli::install_test_active_guide_session(&session_key);
    let response = tokio::spawn(async move { active.respond_next(false).await });

    handle_busy_default_message(
        "保留这张截图稍后处理",
        &session_key,
        &BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: None,
            default_mode: BusyMessageDefaultMode::ExternalGuide,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;

    let (_, prompt) = response
        .await
        .expect("join rejecting active runner")
        .expect("reject image guide");
    assert!(prompt.contains("保留这张截图稍后处理"), "{prompt}");
    let queued = service
        .queue_manager
        .pop_queue_item(&session_key)
        .expect("rejected image guide must be queued");
    assert_eq!(queued.message, "保留这张截图稍后处理");
    assert_eq!(queued.images.len(), 1);
    assert_eq!(queued.images[0].mime_type, "image/png");
    assert_eq!(queued.images[0].data, "cmVqZWN0ZWQtaW1hZ2UtYnl0ZXM=");

    for index in 0..10 {
        service
            .queue_manager
            .push_queue(&session_key, format!("already-full-{index}"))
            .unwrap();
    }
    let mut active =
        crate::im_gateway::external_cli::install_test_active_guide_session(&session_key);
    let response = tokio::spawn(async move { active.respond_next(false).await });
    handle_busy_default_message(
        "保留这张截图稍后处理",
        &session_key,
        &BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: None,
            default_mode: BusyMessageDefaultMode::ExternalGuide,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;
    response
        .await
        .expect("join rejecting full-queue runner")
        .expect("reject image guide with full queue");
    assert_eq!(service.queue_manager.queue_status(&session_key).len(), 10);
}

#[tokio::test]
async fn external_busy_image_preparation_failure_keeps_original_attachment_in_fifo() {
    let _external_cli_guard = crate::im_gateway::external_cli::external_cli_test_env_lock().await;
    let temp = tempfile::tempdir().unwrap();
    let _data_dir_guard = EnvGuard::set_data_dir(temp.path());
    let _worker_env = EnvVarGuard::remove("BIFROST_IM_GATEWAY_WORKER");
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider = test_provider();
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let agent_config = service.agent_config_store.load();
    let session_key = crate::im_gateway::group_context::build_group_session_key(
        &provider.id,
        "image-prepare-failure",
    );
    let event = busy_group_image_event(
        "image-guide-prepare-failure",
        "附件准备失败也不能丢",
        12,
        "cHJlcGFyZS1mYWlsdXJlLWJ5dGVz",
    );
    let blocked_data_dir = temp.path().join("not-a-directory");
    std::fs::write(&blocked_data_dir, b"block directory creation").unwrap();
    let _blocked_env = EnvVarGuard::set(
        "BIFROST_DATA_DIR",
        blocked_data_dir.to_str().expect("utf-8 blocked data dir"),
    );

    handle_busy_default_message(
        "附件准备失败也不能丢",
        &session_key,
        &BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: None,
            default_mode: BusyMessageDefaultMode::ExternalGuide,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;

    let queued = service
        .queue_manager
        .pop_queue_item(&session_key)
        .expect("attachment preparation failure must queue the original input");
    assert_eq!(queued.message, "附件准备失败也不能丢");
    assert_eq!(queued.images.len(), 1);
    assert_eq!(queued.images[0].data, "cHJlcGFyZS1mYWlsdXJlLWJ5dGVz");
}

#[tokio::test]
async fn explicit_queue_mode_preserves_busy_images_and_reports_queue_overflow() {
    let temp = tempfile::tempdir().unwrap();
    let _data_dir_guard = EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let provider = test_provider();
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let agent_config = service.agent_config_store.load();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "image-queue");
    let event = busy_group_image_event(
        "image-explicit-queue",
        "明确排队图片",
        13,
        "cXVldWVkLWltYWdlLWJ5dGVz",
    );
    let context = BusyMessageContext {
        queue_manager: &service.queue_manager,
        client: &client,
        provider: &provider,
        event: &event,
        message_log_store: &service.message_log_store,
        agent_session_manager: &service.agent_session_manager,
        progress_registry: &service.progress_registry,
        external_cli_config_store: &service.external_cli_config_store,
        agent_config: &agent_config,
        group_context_store: &service.group_context_store,
        group_turn_id: None,
        default_mode: BusyMessageDefaultMode::Queue,
        status_context: Default::default(),
        default_work_dir: None,
    };

    handle_busy_default_message("明确排队图片", &session_key, &context).await;
    let queued = service
        .queue_manager
        .pop_queue_item(&session_key)
        .expect("queue mode image");
    assert_eq!(queued.message, "明确排队图片");
    assert_eq!(queued.images[0].data, "cXVldWVkLWltYWdlLWJ5dGVz");

    for index in 0..10 {
        service
            .queue_manager
            .push_queue(&session_key, format!("full-{index}"))
            .unwrap();
    }
    handle_busy_default_message("溢出图片", &session_key, &context).await;
    assert_eq!(service.queue_manager.queue_status(&session_key).len(), 10);
}

#[test]
fn live_guide_group_turns_follow_the_external_run_outcome() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let queue_manager = SessionQueueManager::new();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key("feishu-main", "busy-group");

    let success_event = busy_group_event("live-guide-success", "/g continue", 1);
    store.record_event(&success_event, "test").unwrap();
    let success_turn = store
        .prepare_turn(
            &success_event,
            crate::im_gateway::group_context::GroupTriggerKind::Guide,
            "continue",
        )
        .unwrap();
    store
        .mark_turn_dispatched(&success_turn.turn_id, 2)
        .unwrap();
    queue_manager.track_live_guide_turn(&session_key, success_turn.turn_id.clone());
    assert_eq!(
        persisted_group_turn_status(&store, &success_turn.turn_id).as_deref(),
        Some("dispatched")
    );
    finalize_live_guide_group_turns(&queue_manager, &store, &session_key, Ok(()));
    assert_eq!(
        persisted_group_turn_status(&store, &success_turn.turn_id).as_deref(),
        Some("completed")
    );

    let failed_event = busy_group_event("live-guide-failed", "/g fail", 3);
    store.record_event(&failed_event, "test").unwrap();
    let failed_turn = store
        .prepare_turn(
            &failed_event,
            crate::im_gateway::group_context::GroupTriggerKind::Guide,
            "fail",
        )
        .unwrap();
    store.mark_turn_dispatched(&failed_turn.turn_id, 4).unwrap();
    queue_manager.track_live_guide_turn(&session_key, failed_turn.turn_id.clone());
    finalize_live_guide_group_turns(&queue_manager, &store, &session_key, Err("runner failed"));
    assert_eq!(
        persisted_group_turn_status(&store, &failed_turn.turn_id).as_deref(),
        Some("failed")
    );
}

#[test]
fn ordinary_busy_messages_always_queue_without_creating_guides() {
    let manager = SessionQueueManager::new();

    assert_eq!(
        enqueue_busy_default_message(&manager, "empty", "   ", Vec::new()),
        Err("消息内容不能为空")
    );

    for session in ["builtin-busy", "external-busy", "web-busy"] {
        let items =
            enqueue_busy_default_message(&manager, session, "  下一条独立问题  ", Vec::new())
                .expect("ordinary busy message should queue");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message, "下一条独立问题");
        assert!(manager.guide_status(session).is_empty());
    }
}

#[test]
fn codex_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("codex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "thread-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("thread-existing")
    );
}

#[test]
fn traex_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "trae-thread-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("trae-thread-existing")
    );
}

#[test]
fn claude_code_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("claude-code".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "threadId".to_string(),
        "claude-session-existing".to_string(),
    );

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("claude-session-existing")
    );
}

#[test]
fn codex_runner_metadata_does_not_override_explicit_thread() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::json!({ "threadId": "explicit-thread" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("codex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "remembered-thread".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("explicit-thread")
    );
}

#[test]
fn chatgpt_web_metadata_resumes_persisted_conversation() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "continue".to_string(),
        operation: "ask".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("chatgpt-web".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("conversationId".to_string(), "conv-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("conversationId")
            .and_then(|value| value.as_str()),
        Some("conv-existing")
    );
}
