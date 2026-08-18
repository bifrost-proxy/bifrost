use super::*;

#[tokio::test]
async fn thread_query_commands_reply_in_idle_and_busy_modes() {
    fn event(provider: &ImProviderConfig, command: &str) -> ImEvent {
        ImEvent {
            event_id: format!("evt-query-{}", uuid_short()),
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                user_id: Some("query-user".to_string()),
                message_id: Some(format!("om-query-{}", uuid_short())),
                ..Default::default()
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: command.to_string(),
                ..Default::default()
            }),
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "weixin-thread-query".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.secret_ref = None;
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let agent_config = service.agent_config_store.load();
    let session_key = "weixin-thread-query:query-user";

    for command in ["/q", "/pwd", "/runner", "/stop"] {
        assert!(
            handle_idle_im_command(
                command,
                session_key,
                &agent_config,
                IdleImCommandContext {
                    client: &client,
                    provider: &provider,
                    provider_store: &service.provider_store,
                    group_context_store: &service.group_context_store,
                    external_cli_config_store: &service.external_cli_config_store,
                    event: &event(&provider, command),
                    message_log_store: &service.message_log_store,
                    agent_session_manager: &service.agent_session_manager,
                    queue_manager: &service.queue_manager,
                },
            )
            .await
        );
    }

    service
        .queue_manager
        .push_queue(session_key, "排队中的真实消息".to_string())
        .expect("queue message");
    for command in ["/q", "/pwd", "/runner", "/stop"] {
        handle_busy_message(
            command,
            session_key,
            BusyMessageContext {
                queue_manager: &service.queue_manager,
                client: &client,
                provider: &provider,
                event: &event(&provider, command),
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
            },
        )
        .await;
    }

    assert!(
        handle_idle_im_command(
            "/stop",
            "",
            &agent_config,
            IdleImCommandContext {
                client: &client,
                provider: &provider,
                provider_store: &service.provider_store,
                group_context_store: &service.group_context_store,
                external_cli_config_store: &service.external_cli_config_store,
                event: &event(&provider, "/stop"),
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                queue_manager: &service.queue_manager,
            },
        )
        .await
    );
    handle_busy_message(
        "/stop",
        "",
        BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event(&provider, "/stop"),
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
        },
    )
    .await;
    handle_busy_message(
        "guide while broker control is invalid",
        "",
        BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event(&provider, "guide while broker control is invalid"),
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

    let replies = service
        .message_log_store
        .list()
        .into_iter()
        .filter_map(|entry| entry.content)
        .collect::<Vec<_>>();
    assert!(replies.iter().any(|reply| reply.contains("排队已清空")));
    assert!(replies
        .iter()
        .any(|reply| reply.contains("排队中的真实消息")));
    assert!(
        replies
            .iter()
            .filter(|reply| reply.contains("当前线程工作目录"))
            .count()
            >= 2
    );
    assert!(
        replies
            .iter()
            .filter(|reply| reply.contains("当前 Runner："))
            .count()
            >= 2
    );
    assert!(
        replies
            .iter()
            .filter(|reply| reply.contains("当前没有正在执行的 Agent loop"))
            .count()
            >= 2
    );
    assert!(
        replies
            .iter()
            .filter(|reply| reply.contains("停止当前 Agent loop 失败"))
            .count()
            >= 2
    );
    assert!(replies
        .iter()
        .any(|reply| reply.contains("发送实时引导失败")));
}
