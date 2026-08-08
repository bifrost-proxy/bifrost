use super::*;

#[tokio::test]
async fn feishu_new_group_command_text_accepts_direct_and_group_commands_only() {
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-new-command-routing".to_string();
    provider.provider_type = crate::im_gateway::types::ImProviderType::Feishu;

    let mut direct = group_test_event(&provider.id, "direct", " /new 发布群 ", false, 1);
    direct.source.chat_type = Some("p2p".to_string());
    assert_eq!(
        feishu_new_group_command_text(&client, &provider, &direct).await,
        Some("/new 发布群".to_string())
    );

    let group = group_test_event(&provider.id, "group", "/new 项目 群", false, 2);
    assert_eq!(
        feishu_new_group_command_text(&client, &provider, &group).await,
        Some("/new 项目 群".to_string())
    );

    let (base_url, identity_server) = spawn_group_lookup_server().await;
    provider.base_url = Some(base_url);
    provider.app_id = Some("cli_test".to_string());
    provider.secret_ref = Some("secret".to_string());
    let mentioned = group_test_event(&provider.id, "mentioned", "@_user_1 /new 讨论群", true, 3);
    assert_eq!(
        feishu_new_group_command_text(&client, &provider, &mentioned).await,
        Some("/new 讨论群".to_string())
    );
    identity_server.abort();

    let ordinary = group_test_event(&provider.id, "ordinary", "hello", false, 4);
    assert!(feishu_new_group_command_text(&client, &provider, &ordinary)
        .await
        .is_none());

    let mut empty = direct.clone();
    empty.message = None;
    assert!(feishu_new_group_command_text(&client, &provider, &empty)
        .await
        .is_none());

    let mut weixin_provider = provider.clone();
    weixin_provider.provider_type = crate::im_gateway::types::ImProviderType::Weixin;
    assert!(
        feishu_new_group_command_text(&client, &weixin_provider, &direct)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn prepare_group_dispatch_covers_ambient_commands_triggers_and_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-dispatch".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());

    let ambient = group_test_event(&provider.id, "m1", "hello", false, 1);
    assert!(
        prepare_group_inbound_dispatch(&client, &provider, &ambient, &store, false)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .chat_name(&provider.id, "oc_group")
            .unwrap()
            .as_deref(),
        Some("Engineering")
    );

    let clear = group_test_event(&provider.id, "m2", "/clear", false, 2);
    let clear_dispatch = prepare_group_inbound_dispatch(&client, &provider, &clear, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(clear_dispatch.message_text, "/clear");
    assert!(clear_dispatch.group_turn_id.is_none());
    assert!(clear_dispatch.reset_group_context);

    let trigger = group_test_event(&provider.id, "m3", "@_user_1 investigate", true, 3);
    let dispatch = prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        dispatch.session_key,
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group")
    );
    assert!(dispatch.message_text.contains("investigate"));
    assert!(dispatch.message_text.contains("hello"));
    let turn_id = dispatch.group_turn_id.unwrap();
    store.mark_turn_completed(&turn_id, 4).unwrap();
    assert!(
        prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, false)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn prepare_group_dispatch_surfaces_quoted_message_read_failures() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-missing-quote".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    let mut trigger = group_test_event(&provider.id, "reply", "@_user_1", true, 1);
    trigger.message.as_mut().unwrap().parent_id = Some("invisible-parent".to_string());

    let error =
        match prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, false).await {
            Err(error) => error,
            Ok(_) => panic!("authoritative Feishu read failure must be visible"),
        };
    assert!(error.contains("feishu token request failed"));
}

#[tokio::test]
async fn unmentioned_bot_neither_fetches_nor_records_addressed_reference() {
    use std::sync::atomic::Ordering;

    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let (base_url, message_reads, server) = spawn_reference_routing_server("oc_group").await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-unmentioned-reference".to_string();
    provider.base_url = Some(base_url);
    let mut event = group_test_event(
        &provider.id,
        "addressed-elsewhere",
        "@_user_1 /q inspect",
        true,
        1,
    );
    let message = event.message.as_mut().unwrap();
    message.parent_id = Some("om_parent".to_string());
    message.mentions[0].is_bot = false;
    message.mentions[0].open_id = Some("ou_other_bot".to_string());
    message.mentions[0].name = Some("Other Bot".to_string());

    assert!(
        prepare_group_inbound_dispatch(&client, &provider, &event, &store, false)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(message_reads.load(Ordering::SeqCst), 0);
    assert_eq!(store.message_count(&provider.id, "oc_group").unwrap(), 0);
    server.abort();
}

#[tokio::test]
async fn prepare_group_dispatch_rejects_cross_group_reference() {
    use std::sync::atomic::Ordering;

    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let (base_url, message_reads, server) = spawn_reference_routing_server("oc_other_group").await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-cross-group-reference".to_string();
    provider.base_url = Some(base_url);
    let mut event = group_test_event(&provider.id, "cross-group", "@_user_1 inspect", true, 1);
    event.message.as_mut().unwrap().parent_id = Some("om_parent".to_string());

    let error =
        match prepare_group_inbound_dispatch(&client, &provider, &event, &store, false).await {
            Err(error) => error,
            Ok(_) => panic!("cross-group reference must be rejected"),
        };
    assert!(error.contains("不属于当前群聊"));
    assert_eq!(message_reads.load(Ordering::SeqCst), 1);
    assert_eq!(store.message_count(&provider.id, "oc_group").unwrap(), 0);
    server.abort();
}

#[tokio::test]
async fn prepare_group_dispatch_records_same_group_reference_for_prompt() {
    use std::sync::atomic::Ordering;

    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let (base_url, message_reads, server) = spawn_reference_routing_server("oc_group").await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-same-group-reference".to_string();
    provider.base_url = Some(base_url);
    let mut event = group_test_event(&provider.id, "same-group", "@_user_1", true, 2);
    event.message.as_mut().unwrap().parent_id = Some("om_parent".to_string());

    let dispatch = prepare_group_inbound_dispatch(&client, &provider, &event, &store, false)
        .await
        .unwrap()
        .expect("same-group reference dispatch");
    assert!(dispatch
        .message_text
        .contains("<at id=ou_alice>Alice</at> quoted content"));
    assert_eq!(message_reads.load(Ordering::SeqCst), 1);
    assert_eq!(store.message_count(&provider.id, "oc_group").unwrap(), 2);
    server.abort();
}

#[tokio::test]
async fn prepare_group_dispatch_recovers_nonterminal_turn_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-recovery".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    let trigger = group_test_event(
        &provider.id,
        "recover-trigger",
        "@_user_1 resume this",
        true,
        1,
    );

    let first_turn_id = {
        let store = ImGroupContextStore::new(temp.path());
        let dispatch = prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, false)
            .await
            .unwrap()
            .unwrap();
        let turn_id = dispatch.group_turn_id.unwrap();
        assert!(
            prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, true)
                .await
                .unwrap()
                .is_none(),
            "an active process must still deduplicate its in-flight turn"
        );
        turn_id
    };

    let reopened = ImGroupContextStore::new(temp.path());
    let recovered = prepare_group_inbound_dispatch(&client, &provider, &trigger, &reopened, false)
        .await
        .unwrap()
        .expect("a redelivered nonterminal turn should resume after restart");
    assert_eq!(
        recovered.group_turn_id.as_deref(),
        Some(first_turn_id.as_str())
    );
    assert!(recovered.message_text.contains("resume this"));
}

#[tokio::test]
async fn prepare_group_dispatch_recovers_quoted_turn_without_refetching_after_restart() {
    use std::sync::atomic::Ordering;

    let temp = tempfile::tempdir().unwrap();
    let (base_url, message_reads, server) = spawn_reference_routing_server("oc_group").await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-quoted-recovery".to_string();
    provider.base_url = Some(base_url);
    let mut trigger = group_test_event(&provider.id, "quoted-recovery", "@_user_1", true, 2);
    trigger.message.as_mut().unwrap().parent_id = Some("om_parent".to_string());

    let first_turn_id = {
        let store = ImGroupContextStore::new(temp.path());
        prepare_group_inbound_dispatch(&client, &provider, &trigger, &store, false)
            .await
            .unwrap()
            .unwrap()
            .group_turn_id
            .unwrap()
    };
    assert_eq!(message_reads.load(Ordering::SeqCst), 1);
    server.abort();

    let reopened = ImGroupContextStore::new(temp.path());
    let recovered = prepare_group_inbound_dispatch(&client, &provider, &trigger, &reopened, false)
        .await
        .unwrap()
        .expect("frozen quoted turn must recover from SQLite while Feishu API is offline");
    assert_eq!(
        recovered.group_turn_id.as_deref(),
        Some(first_turn_id.as_str())
    );
    assert!(recovered
        .message_text
        .contains("<at id=ou_alice>Alice</at> quoted content"));
    assert_eq!(message_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prepare_group_dispatch_resolves_chat_and_bot_from_feishu_api() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-api".to_string();
    provider.base_url = Some(base_url);
    let mut event = group_test_event(&provider.id, "api-trigger", "@_user_1 inspect", true, 1);
    let message = event.message.as_mut().unwrap();
    message.raw_content = Some(serde_json::json!({"text":"@_user_1 inspect"}));
    message.mentions[0].is_bot = false;

    let dispatch = prepare_group_inbound_dispatch(&client, &provider, &event, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert!(dispatch.message_text.contains("API Engineering"));
    assert_eq!(
        store
            .chat_name(&provider.id, "oc_group")
            .unwrap()
            .as_deref(),
        Some("API Engineering")
    );
    server.abort();
}

#[tokio::test]
async fn prepare_group_dispatch_tolerates_feishu_lookup_failures() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-api-error".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    let mut event = group_test_event(&provider.id, "api-error", "@_user_1 inspect", true, 1);
    let message = event.message.as_mut().unwrap();
    message.raw_content = Some(serde_json::json!({"text":"@_user_1 inspect"}));
    message.mentions[0].is_bot = false;

    assert!(
        prepare_group_inbound_dispatch(&client, &provider, &event, &store, false)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_group_dispatch_reports_malformed_group_context() {
    let temp = tempfile::tempdir().unwrap();
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-concurrent-malformed".to_string();
    provider.base_url = Some(base_url);
    service.provider_store.add(provider.clone()).unwrap();
    rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_concurrent_group_message
                 BEFORE INSERT ON im_group_messages
                 BEGIN SELECT RAISE(FAIL, 'test insert failure'); END;",
        )
        .unwrap();
    let event = group_test_event(
        &provider.id,
        "concurrent-malformed",
        "@_user_1 continue",
        true,
        1,
    );

    handle_concurrent_event_during_chat(
        &event,
        &provider,
        "active-session",
        &service.queue_manager,
        &ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        &service.message_log_store,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.agent_config_store,
        &service.provider_store,
        &service.event_store,
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Guide,
    )
    .await;
    assert_eq!(service.event_store.list().len(), 1);
    server.abort();
}

#[test]
fn event_session_keys_separate_group_and_direct_conversations() {
    let group = group_test_event("provider-a", "group-message", "hello", false, 1);
    assert_eq!(
        session_key_for_event(&group),
        crate::im_gateway::group_context::build_group_session_key("provider-a", "oc_group")
    );

    let mut direct = group;
    direct.source.chat_type = Some("p2p".to_string());
    assert_eq!(
        session_key_for_event(&direct),
        build_session_key("provider-a", Some("ou_sender"))
    );
}

#[tokio::test]
async fn group_event_loop_records_ambient_and_releases_turn_when_agent_is_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-loop".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service.provider_store.add(provider.clone()).unwrap();
    let mut agent_config = service.agent_config_store.load();
    agent_config.enabled = false;
    service.agent_config_store.save(&agent_config).unwrap();

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    let mut malformed = group_test_event(&provider.id, "malformed", "ignored", false, 0);
    malformed.message = None;
    tx.send(malformed).unwrap();
    tx.send(group_test_event(&provider.id, "ambient", "hello", false, 1))
        .unwrap();
    tx.send(group_test_event(
        &provider.id,
        "trigger",
        "@_user_1 run",
        true,
        2,
    ))
    .unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("group event loop timed out")
        .expect("group event loop panicked");

    let remaining_turns: i64 = rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM im_group_turns", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining_turns, 0);

    assert_eq!(
        service
            .group_context_store
            .message_count(&provider.id, "oc_group")
            .unwrap(),
        2
    );
    let retry = group_test_event(&provider.id, "retry", "@_user_1 retry", true, 3);
    service
        .group_context_store
        .record_event(&retry, "test")
        .unwrap();
    let turn = service
        .group_context_store
        .prepare_turn(
            &retry,
            crate::im_gateway::group_context::GroupTriggerKind::Mention,
            "retry",
        )
        .unwrap();
    assert_eq!(turn.message_count, 3);
}

#[tokio::test]
async fn new_group_command_event_loop_runs_before_dedup_and_runner_dispatch() {
    use std::sync::atomic::Ordering;
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, creates, server) = spawn_new_group_event_loop_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-new-group-loop".to_string();
    provider.base_url = Some(base_url);
    provider.app_id = Some("cli_test".to_string());
    provider.secret_ref = Some("secret".to_string());
    provider.owner_open_id = Some("ou_sender".to_string());

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    let event = group_test_event(&provider.id, "new-loop", "/new 事件循环群", false, 1);
    tx.send(event.clone()).unwrap();
    tx.send(event).unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("new group event loop timed out")
        .expect("new group event loop panicked");

    assert_eq!(creates.load(Ordering::SeqCst), 1);
    assert_eq!(service.event_store.list().len(), 2);
    assert!(service.message_log_store.list().iter().any(|log| log
        .content
        .as_deref()
        .is_some_and(|text| text.contains("未重复创建"))));
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn group_clear_reports_baseline_persistence_failure_after_runtime_reset() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-clear-failure".to_string();
    provider.base_url = Some(base_url);
    service.provider_store.add(provider.clone()).unwrap();

    let mut agent = service.agent_config_store.load();
    agent.enabled = true;
    agent.runner = Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()));
    service.agent_config_store.save(&agent).unwrap();
    let mut cli = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    let runner = cli
        .runners
        .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
        .unwrap();
    runner.enabled = true;
    runner.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
    service.external_cli_config_store.save(cli).unwrap();
    rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_group_baseline
             BEFORE UPDATE OF last_assigned_seq ON im_group_bindings
             BEGIN SELECT RAISE(FAIL, 'test baseline failure'); END;",
        )
        .unwrap();

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    tx.send(group_test_event(
        &provider.id,
        "before-clear",
        "keep this context",
        false,
        1,
    ))
    .unwrap();
    tx.send(group_test_event(
        &provider.id,
        "clear-failure",
        "/clear",
        false,
        2,
    ))
    .unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("group clear event loop timed out")
        .expect("group clear event loop panicked");

    let last_assigned_seq: u64 = rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .query_row(
            "SELECT last_assigned_seq FROM im_group_bindings WHERE provider_id = ?1 AND chat_id = 'oc_group'",
            rusqlite::params![provider.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(last_assigned_seq, 0);
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn group_event_loop_uses_group_workdir_for_busy_default_and_route_paths() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-busy-workdir".to_string();
    provider.base_url = Some(base_url);
    service.provider_store.add(provider.clone()).unwrap();
    service
        .route_store
        .add(crate::im_gateway::types::ImRoute {
            id: "busy-group-agent-route".to_string(),
            provider_id: provider.id.clone(),
            name: "Busy group agent route".to_string(),
            enabled: true,
            event_type: crate::im_gateway::types::ImEventType::MessageReceive,
            matcher: crate::im_gateway::types::ImEventMatcher {
                chat_ids: vec!["oc_group".to_string()],
                user_ids: Vec::new(),
                keyword: Some("route".to_string()),
                regex: None,
            },
            action: crate::im_gateway::types::ImRouteAction::AgentChat {
                system_prompt: None,
                model: None,
                reply_target: crate::im_gateway::types::ReplyTarget::OriginalChat,
            },
            timeout_ms: 30_000,
            max_output_bytes: 1_048_576,
            created_at: now_ms(),
            updated_at: now_ms(),
        })
        .unwrap();
    let mut agent = service.agent_config_store.load();
    agent.enabled = true;
    agent.runner = Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()));
    service.agent_config_store.save(&agent).unwrap();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group");
    let bootstrap = group_test_event(
        &provider.id,
        "status-bootstrap",
        "@_user_1 bootstrap",
        true,
        0,
    );
    service
        .group_context_store
        .record_event(&bootstrap, "test")
        .unwrap();
    let bootstrap_turn = service
        .group_context_store
        .prepare_turn(
            &bootstrap,
            crate::im_gateway::group_context::GroupTriggerKind::Mention,
            "bootstrap",
        )
        .unwrap();
    service
        .group_context_store
        .release_turn(&bootstrap_turn.turn_id, "test setup", 0)
        .unwrap();
    assert!(service
        .group_context_store
        .set_work_dir_by_session(&session_key, temp.path().to_str().unwrap())
        .unwrap());
    let active_session = service
        .agent_session_manager
        .try_take_session_with_work_dir(&session_key, None)
        .unwrap();

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    tx.send(group_test_event(
        &provider.id,
        "busy-default",
        "@_user_1 first busy message",
        true,
        1,
    ))
    .unwrap();
    tx.send(group_test_event(
        &provider.id,
        "busy-route",
        "@_user_1 route busy message",
        true,
        2,
    ))
    .unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("busy group event loop timed out")
        .expect("busy group event loop panicked");
    service.agent_session_manager.return_session(active_session);
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn group_event_loop_busy_session_falls_back_to_agent_workdir() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-busy-fallback".to_string();
    provider.base_url = Some(base_url);
    service.provider_store.add(provider.clone()).unwrap();
    let mut agent = service.agent_config_store.load();
    agent.enabled = true;
    agent.runner = Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()));
    agent.work_dir = Some(temp.path().display().to_string());
    service.agent_config_store.save(&agent).unwrap();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group");
    let active_session = service
        .agent_session_manager
        .try_take_session_with_work_dir(&session_key, None)
        .unwrap();

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    tx.send(group_test_event(
        &provider.id,
        "busy-fallback",
        "@_user_1 busy fallback",
        true,
        1,
    ))
    .unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("busy fallback event loop timed out")
        .expect("busy fallback event loop panicked");
    service.agent_session_manager.return_session(active_session);
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn group_event_loop_status_uses_bound_group_workdir() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let (base_url, server) = spawn_group_lookup_server().await;
    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-status".to_string();
    provider.base_url = Some(base_url);
    service.provider_store.add(provider.clone()).unwrap();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group");
    let bootstrap = group_test_event(
        &provider.id,
        "status-bootstrap",
        "@_user_1 bootstrap",
        true,
        0,
    );
    service
        .group_context_store
        .record_event(&bootstrap, "test")
        .unwrap();
    let bootstrap_turn = service
        .group_context_store
        .prepare_turn(
            &bootstrap,
            crate::im_gateway::group_context::GroupTriggerKind::Mention,
            "bootstrap",
        )
        .unwrap();
    service
        .group_context_store
        .release_turn(&bootstrap_turn.turn_id, "test setup", 0)
        .unwrap();
    assert!(service
        .group_context_store
        .set_work_dir_by_session(&session_key, temp.path().to_str().unwrap())
        .unwrap());

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    tx.send(group_test_event(
        &provider.id,
        "group-status",
        "/status",
        false,
        1,
    ))
    .unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("group status event loop timed out")
        .expect("group status event loop panicked");
    assert_eq!(
        service
            .group_context_store
            .message_count(&provider.id, "oc_group")
            .unwrap(),
        2
    );
    server.abort();
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn group_event_loop_routes_concurrent_context_to_the_active_external_session() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let runner_path = temp.path().join("slow-traex");
    std::fs::write(
            &runner_path,
            "#!/usr/bin/env sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"group-thread\"}'\nsleep 2\nprintf '%s\\n' '{\"type\":\"assistant_final\",\"content\":\"GROUP_RUNNER_OK\"}'\nprintf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n",
        )
        .unwrap();
    let mut permissions = std::fs::metadata(&runner_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&runner_path, permissions).unwrap();

    let mut agent = service.agent_config_store.load();
    agent.enabled = true;
    agent.runner = Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()));
    service.agent_config_store.save(&agent).unwrap();
    let mut cli = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    let runner = cli
        .runners
        .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
        .unwrap();
    runner.enabled = true;
    runner.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
    runner.inject_bifrost_tools = false;
    runner.adapter_config.executable = Some(runner_path.display().to_string());
    service.external_cli_config_store.save(cli).unwrap();

    let mut provider = recorder_test_provider();
    provider.id = "feishu-group-concurrent".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service.provider_store.add(provider.clone()).unwrap();
    let group_a_session =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group");
    let group_b_session =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group_b");
    let direct_a_session = build_session_key(&provider.id, Some("ou_direct_a"));
    let direct_b_session = build_session_key(&provider.id, Some("ou_direct_b"));

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop_with_options(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.group_context_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
        EventLoopOptions {
            send_online_notification: false,
        },
    ));
    tx.send(group_test_event(
        &provider.id,
        "first-trigger",
        "@_user_1 start",
        true,
        1,
    ))
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if service
                .agent_session_manager
                .is_session_active(&group_a_session)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group runner did not become active");

    let mut group_b = group_test_event(
        &provider.id,
        "second-group-trigger",
        "@_user_1 start group B",
        true,
        2,
    );
    group_b.source.chat_id = Some("oc_group_b".to_string());
    tx.send(group_b).unwrap();

    let mut direct_a =
        group_test_event(&provider.id, "direct-a-trigger", "start direct A", false, 3);
    direct_a.source.chat_type = Some("p2p".to_string());
    direct_a.source.user_id = Some("ou_direct_a".to_string());
    tx.send(direct_a).unwrap();

    let mut direct_b =
        group_test_event(&provider.id, "direct-b-trigger", "start direct B", false, 4);
    direct_b.source.chat_type = Some("p2p".to_string());
    direct_b.source.user_id = Some("ou_direct_b".to_string());
    tx.send(direct_b).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let all_active = [
                &group_a_session,
                &group_b_session,
                &direct_a_session,
                &direct_b_session,
            ]
            .into_iter()
            .all(|session_key| service.agent_session_manager.is_session_active(session_key));
            if all_active {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group and direct runners did not become active independently");
    assert!(service
        .agent_session_manager
        .is_session_active(&group_a_session));

    tx.send(group_test_event(
        &provider.id,
        "ambient-during-run",
        "the deployment is red",
        false,
        5,
    ))
    .unwrap();
    let guide_during_run = group_test_event(
        &provider.id,
        "guide-during-run",
        "/g include the deployment failure",
        false,
        6,
    );
    tx.send(guide_during_run.clone()).unwrap();
    let mut duplicate_guide = guide_during_run;
    duplicate_guide.event_id = "redelivery-guide-during-run".to_string();
    tx.send(duplicate_guide).unwrap();
    tx.send(group_test_event(
        &provider.id,
        "cwd-during-run",
        &format!("/cwd {}", temp.path().display()),
        false,
        7,
    ))
    .unwrap();
    tx.send(group_test_event(
        &provider.id,
        "queue-during-run",
        "/q verify the queued follow-up",
        false,
        8,
    ))
    .unwrap();
    tx.send(group_test_event(
        &provider.id,
        "remove-queue-during-run",
        "/rq 1",
        false,
        9,
    ))
    .unwrap();
    let mut direct_follow_up = group_test_event(
        &provider.id,
        "direct-a-guide-during-run",
        "/g include the direct follow-up",
        false,
        10,
    );
    direct_follow_up.source.chat_type = Some("p2p".to_string());
    direct_follow_up.source.user_id = Some("ou_direct_a".to_string());
    tx.send(direct_follow_up.clone()).unwrap();
    let mut duplicate_direct_follow_up = direct_follow_up;
    duplicate_direct_follow_up.event_id = "redelivery-direct-a-guide-during-run".to_string();
    tx.send(duplicate_direct_follow_up).unwrap();

    // Full-workspace LLVM coverage instrumentation can make the four independent
    // runner processes take longer than the normal test build. Keep the final
    // state assertions unchanged, but allow enough time for an instrumented run.
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let any_active = [
                &group_a_session,
                &group_b_session,
                &direct_a_session,
                &direct_b_session,
            ]
            .into_iter()
            .any(|session_key| service.agent_session_manager.is_session_active(session_key));
            if !any_active {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("independent runners did not finish");
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(60), handle)
        .await
        .expect("group runner event loop timed out")
        .expect("group runner event loop panicked");

    assert_eq!(
        service
            .group_context_store
            .message_count(&provider.id, "oc_group")
            .unwrap(),
        6
    );
    assert_eq!(
        service
            .group_context_store
            .work_dir_by_session(&group_a_session)
            .unwrap()
            .as_deref(),
        Some(std::fs::canonicalize(temp.path()).unwrap().as_path())
    );
    assert!(service
        .queue_manager
        .queue_status(&group_a_session)
        .is_empty());
    let inbound_logs: Vec<_> = service
        .message_log_store
        .list_by_provider(&provider.id)
        .into_iter()
        .filter(|log| log.direction == MessageDirection::Inbound)
        .collect();
    for message_id in [
        "guide-during-run",
        "cwd-during-run",
        "queue-during-run",
        "remove-queue-during-run",
        "direct-a-guide-during-run",
    ] {
        let matching: Vec<_> = inbound_logs
            .iter()
            .filter(|log| log.message_id.as_deref() == Some(message_id))
            .collect();
        assert_eq!(matching.len(), 1, "inbound log for {message_id}");
        assert_eq!(matching[0].reaction_added, Some(false));
    }
    assert!(inbound_logs
        .iter()
        .all(|log| log.message_id.as_deref() != Some("ambient-during-run")));
    for message_id in ["guide-during-run", "direct-a-guide-during-run"] {
        assert_eq!(
            service
                .event_store
                .list()
                .iter()
                .filter(|event| event.source.message_id.as_deref() == Some(message_id))
                .count(),
            1,
            "deduplicated event history for {message_id}"
        );
    }
    for session_key in [
        group_a_session,
        group_b_session,
        direct_a_session,
        direct_b_session,
    ] {
        let detail = service
            .agent_session_manager
            .get_session_detail(&session_key)
            .expect("independent external runner session");
        assert!(detail
            .messages
            .iter()
            .any(|message| message.content == "GROUP_RUNNER_OK"));
    }
}

#[tokio::test]
async fn disabled_and_busy_external_runner_paths_preserve_group_queue_state() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let mut provider = recorder_test_provider();
    provider.id = "feishu-disabled-runner".to_string();
    service.provider_store.add(provider.clone()).unwrap();
    let mut external_cli_config = service.external_cli_config_store.load();
    let default_runner_id = external_cli_config.default_runner_id.clone();
    external_cli_config
        .runners
        .get_mut(&default_runner_id)
        .unwrap()
        .enabled = false;
    service
        .external_cli_config_store
        .save(external_cli_config)
        .unwrap();
    let event = group_test_event(
        &provider.id,
        "disabled-runner-trigger",
        "@_user_1 investigate",
        true,
        1,
    );
    service
        .group_context_store
        .record_event(&event, "test")
        .unwrap();
    let turn = service
        .group_context_store
        .prepare_turn(
            &event,
            crate::im_gateway::group_context::GroupTriggerKind::Mention,
            "investigate",
        )
        .unwrap();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "oc_group");
    let mut reply_event = event.clone();
    reply_event.source.chat_id = None;
    reply_event.source.user_id = None;
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let (_tx, mut rx) = mpsc::unbounded_channel();
    run_external_cli_agent_chat(
        ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &reply_event,
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
            message_text: "investigate".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key: session_key.clone(),
            adapter_override: None,
            instructions_override: None,
            delivery_override: None,
            runner_id_override: None,
            runner_selected: false,
            group_turn_id: Some(turn.turn_id.clone()),
            reset_group_context: false,
        },
    )
    .await;

    let remaining: i64 = rusqlite::Connection::open(service.group_context_store.file_path())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM im_group_turns WHERE turn_id = ?1",
            rusqlite::params![turn.turn_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);

    let mut external_cli_config = service.external_cli_config_store.load();
    let default_runner_id = external_cli_config.default_runner_id.clone();
    let default_runner = external_cli_config
        .runners
        .get_mut(&default_runner_id)
        .unwrap();
    default_runner.enabled = true;
    default_runner.adapter = crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string();
    service
        .external_cli_config_store
        .save(external_cli_config)
        .unwrap();
    let held_session = service
        .agent_session_manager
        .try_take_session(&session_key)
        .unwrap();

    run_external_cli_agent_chat(
        ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &reply_event,
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
            message_text: "queued while busy".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key: session_key.clone(),
            adapter_override: None,
            instructions_override: None,
            delivery_override: None,
            runner_id_override: None,
            runner_selected: false,
            group_turn_id: None,
            reset_group_context: false,
        },
    )
    .await;

    assert_eq!(service.queue_manager.queue_status(&session_key).len(), 1);
    service.agent_session_manager.return_session(held_session);
}
