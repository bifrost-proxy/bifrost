use super::*;

#[test]
pub(super) fn im_status_runtime_context_reads_persisted_runner_overrides_and_session_id() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let agent_config = bifrost_agent::config::AgentConfig::default();
    let external_config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    crate::im_gateway::session_state::upsert_session_state(
        "feishu-main:owner",
        "codex",
        Some("Codex"),
        |state| {
            state.external_thread_id = Some("thread-persisted-status".to_string());
            state.model_override = Some("gpt-status".to_string());
            state.model_override_source = Some("session override".to_string());
            state.reasoning_effort_override = Some("xhigh".to_string());
        },
    )
    .expect("persist status state");

    let context = resolve_im_status_runtime_context(
        &agent_config,
        &external_config,
        "feishu-main",
        "feishu-main:owner",
        Some("Codex"),
    );

    assert_eq!(context.runner_type.as_deref(), Some("codex"));
    assert_eq!(context.runner_id.as_deref(), Some("Codex"));
    assert_eq!(context.model.as_deref(), Some("gpt-status"));
    assert_eq!(context.model_provider.as_deref(), Some("session override"));
    assert_eq!(context.model_reasoning_effort.as_deref(), Some("xhigh"));
    assert_eq!(
        context.external_thread_id.as_deref(),
        Some("thread-persisted-status")
    );
}

#[tokio::test]
pub(super) async fn idle_weixin_status_reply_uses_shared_complete_overview() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "weixin-main".to_string();
    provider.display_name = "Weixin Main".to_string();
    provider.provider_type = ImProviderType::Weixin;
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let session_key = "weixin-main:user-status";
    let event = ImEvent {
        event_id: "evt-weixin-status".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            user_id: Some("user-status".to_string()),
            message_id: Some("msg-weixin-status".to_string()),
            ..Default::default()
        },
        message: None,
        received_at: now_ms(),
        raw_digest: None,
    };
    crate::im_gateway::session_state::upsert_session_state(
        session_key,
        "codex",
        Some("Codex"),
        |state| {
            state.external_thread_id = Some("thread-weixin-status".to_string());
            state.model_override = Some("gpt-5.6-sol".to_string());
            state.model_override_source = Some("aidp_local".to_string());
            state.reasoning_effort_override = Some("high".to_string());
        },
    )
    .expect("persist status state");

    assert!(
        handle_idle_im_command(
            "/status",
            session_key,
            &service.agent_config_store.load(),
            IdleImCommandContext {
                client: &client,
                provider: &provider,
                provider_store: &service.provider_store,
                group_context_store: &service.group_context_store,
                external_cli_config_store: &service.external_cli_config_store,
                event: &event,
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                queue_manager: &service.queue_manager,
            },
        )
        .await
    );

    let reply = service
        .message_log_store
        .list()
        .into_iter()
        .find_map(|entry| entry.content)
        .expect("status reply should be recorded");
    assert!(reply.contains("- **Provider**: Weixin Main (`weixin-main`)"));
    assert!(reply.contains("- **Runner Type**: `codex`"));
    assert!(reply.contains("- **Runner ID**: `Codex`"));
    assert!(reply.contains("- **Model**: `gpt-5.6-sol（aidp_local）`"));
    assert!(reply.contains("- **Reasoning Effort**: `high`"));
    assert!(reply.contains("- **Bound Session**: `weixin-main:user-status`"));
    assert!(reply.contains("- **External Session ID**: `thread-weixin-status`"));
    assert!(reply.contains("- **Completed User Turns**: 0"));
    assert!(reply.contains("- **Queue**: 无排队消息"));
    assert!(reply.contains("- **Status**: Ready（新会话）"));
}

#[tokio::test]
pub(super) async fn busy_status_reply_covers_detail_live_and_processing_fallbacks() {
    fn status_event(provider: &ImProviderConfig, suffix: &str) -> ImEvent {
        ImEvent {
            event_id: format!("evt-busy-status-{suffix}"),
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                user_id: Some(format!("user-{suffix}")),
                message_id: Some(format!("msg-busy-status-{suffix}")),
                ..Default::default()
            },
            message: None,
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    async fn invoke_status(
        service: &ImGatewayService,
        client: &ImProviderClient,
        provider: &ImProviderConfig,
        agent_config: &crate::im_gateway::agent::ImAgentConfig,
        session_key: &str,
        event: &ImEvent,
        status_context: bifrost_agent::StatusRuntimeContext,
    ) {
        handle_busy_message(
            "/status",
            session_key,
            BusyMessageContext {
                queue_manager: &service.queue_manager,
                client,
                provider,
                event,
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                progress_registry: &service.progress_registry,
                external_cli_config_store: &service.external_cli_config_store,
                agent_config,
                group_context_store: &service.group_context_store,
                group_turn_id: None,
                default_mode: BusyMessageDefaultMode::Queue,
                status_context,
                default_work_dir: Some("/tmp/busy-status".to_string()),
            },
        )
        .await;
    }

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "weixin-busy-status".to_string();
    provider.display_name = "Weixin Busy Status".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.secret_ref = None;
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let agent_config = service.agent_config_store.load();

    let detail_key = "weixin-busy-status:user-detail";
    let mut detail_session = service
        .agent_session_manager
        .take_session_with_work_dir(detail_key, Some("/tmp/detail-status".to_string()));
    detail_session.mark_external_runner_runtime("codex", "Codex");
    detail_session.remember_external_conversation_ref(None, Some("thread-busy-detail".to_string()));
    service.agent_session_manager.return_session(detail_session);
    invoke_status(
        &service,
        &client,
        &provider,
        &agent_config,
        detail_key,
        &status_event(&provider, "detail"),
        Default::default(),
    )
    .await;

    let live_key = "weixin-busy-status:user-live";
    let _live_session = service
        .agent_session_manager
        .take_session_with_work_dir(live_key, Some("/tmp/live-status".to_string()));
    let mut live_status = service
        .agent_session_manager
        .get_active_turn_status(live_key)
        .expect("live status");
    live_status.runner_type = Some("codex".to_string());
    live_status.runner_id = Some("Codex".to_string());
    live_status.model = Some("live-model".to_string());
    live_status.model_provider = Some("live-provider".to_string());
    live_status.external_thread_id = Some("thread-busy-live".to_string());
    service
        .agent_session_manager
        .update_active_turn_status_from_worker(live_status);
    service
        .queue_manager
        .push_queue(live_key, "queued after live status".to_string())
        .expect("queue live message");
    invoke_status(
        &service,
        &client,
        &provider,
        &agent_config,
        live_key,
        &status_event(&provider, "live"),
        Default::default(),
    )
    .await;

    let fallback_key = "weixin-busy-status:user-fallback";
    assert!(service
        .agent_session_manager
        .try_start_external_session_preview(
            fallback_key,
            Some("fallback".to_string()),
            Some("/tmp/fallback-status".to_string()),
            Some("im".to_string()),
            Some("codex".to_string()),
            Some("Codex".to_string()),
        ));
    let fallback_context = bifrost_agent::StatusRuntimeContext {
        model: Some("fallback-model".to_string()),
        model_provider: Some("fallback-provider".to_string()),
        external_thread_id: Some("thread-busy-fallback".to_string()),
        external_conversation_id: Some("conversation-busy-fallback".to_string()),
        ..Default::default()
    };
    invoke_status(
        &service,
        &client,
        &provider,
        &agent_config,
        fallback_key,
        &status_event(&provider, "fallback"),
        fallback_context,
    )
    .await;

    let replies: Vec<String> = service
        .message_log_store
        .list()
        .into_iter()
        .filter_map(|entry| entry.content)
        .collect();
    assert!(replies.iter().any(|reply| {
        reply.contains(detail_key)
            && reply.contains("thread-busy-detail")
            && reply.contains("会话诊断:")
    }));
    assert!(replies.iter().any(|reply| {
        reply.contains(live_key)
            && reply.contains("thread-busy-live")
            && reply.contains("1 条排队消息")
            && reply.contains("Running")
    }));
    assert!(
        replies.iter().any(|reply| {
            reply.contains(fallback_key)
                && reply.contains("thread-busy-fallback")
                && reply.contains("Running")
        }),
        "fallback reply missing from {replies:#?}"
    );
}
