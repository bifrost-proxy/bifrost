use super::*;

/// Handle an inbound event while an external runner is active.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_concurrent_event_during_chat(
    event: &ImEvent,
    provider: &ImProviderConfig,
    active_session_key: &str,
    queue_manager: &Arc<SessionQueueManager>,
    client: &ImProviderClient,
    message_log_store: &Arc<ImMessageLogStore>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    provider_store: &Arc<ImProviderStore>,
    event_store: &Arc<ImEventStore>,
    group_context_store: &Arc<ImGroupContextStore>,
    external_cli_config_store: &Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    active_session_default_mode: BusyMessageDefaultMode,
) {
    let provider = provider_store
        .get(&event.provider_id)
        .unwrap_or_else(|| provider.clone());
    if !provider.enabled {
        return;
    }
    let is_group_event = crate::im_gateway::group_context::is_feishu_group_event(event);
    if provider.provider_type == ImProviderType::Feishu && !is_group_event {
        if let Some(ref owner_id) = provider.owner_open_id {
            if event.source.user_id.as_deref().unwrap_or("") != owner_id {
                return;
            }
        }
    }
    if let Err(error) = event_store.add(event.clone()) {
        error!(error = %error, "failed to store concurrent event");
    }
    let message = match event.message.as_ref() {
        Some(message)
            if !message.text.trim().is_empty()
                || !message.images.is_empty()
                || !message.files.is_empty() =>
        {
            message
        }
        _ => return,
    };
    let dispatch = if is_group_event {
        match prepare_group_inbound_dispatch(
            client,
            &provider,
            event,
            group_context_store,
            agent_session_manager.is_session_active(
                &crate::im_gateway::group_context::build_group_session_key(
                    &event.provider_id,
                    event.source.chat_id.as_deref().unwrap_or_default(),
                ),
            ),
        )
        .await
        {
            Ok(Some(dispatch)) => dispatch,
            Ok(None) => return,
            Err(error) => {
                send_agent_reply(
                    client,
                    &provider,
                    event,
                    &format!("无法准备本次群聊上下文：{error}"),
                    message_log_store,
                )
                .await;
                return;
            }
        }
    } else {
        PreparedInboundDispatch {
            message_text: agent_message_text_with_reference(
                message,
                &event.provider_id,
                event.source.user_id.as_deref(),
                event.source.message_id.as_deref(),
                message_log_store,
            ),
            session_key: build_session_key(&event.provider_id, event.source.user_id.as_deref()),
            group_turn_id: None,
            reset_group_context: false,
            direct_reply: None,
        }
    };
    let direct_reply = dispatch.direct_reply.clone();
    let message_text = dispatch.message_text;
    let session_key = dispatch.session_key;
    let group_turn_id = dispatch.group_turn_id;
    // Ambient group messages return above. Accepted triggers retain the same
    // acknowledgement and inbound audit side effects as the normal path even
    // though their session mailbox already has a runner in flight.
    acknowledge_and_log_inbound_event(client, &provider, event, message_log_store).await;
    if let Some(reply) = direct_reply {
        send_agent_reply(client, &provider, event, &reply, message_log_store).await;
        if let Some(turn_id) = group_turn_id.as_deref() {
            if let Err(error) = group_context_store.mark_turn_completed(turn_id, now_ms()) {
                warn!(turn_id = %turn_id, error = %error, "failed to complete unavailable quoted-message turn");
            }
        }
        return;
    }
    let agent_config = effective_agent_config_for_provider(&agent_config_store.load(), &provider);
    if session_key == active_session_key {
        if message_text.trim() == "/help" {
            let config = external_cli_config_store.load();
            let response = build_im_help_text_for_agent_config(
                &agent_config,
                &config,
                Some(provider.id.as_str()),
            );
            send_agent_reply(client, &provider, event, &response, message_log_store).await;
            return;
        }
        handle_busy_message(
            &message_text,
            &session_key,
            BusyMessageContext {
                queue_manager,
                client,
                provider: &provider,
                event,
                message_log_store,
                agent_session_manager,
                progress_registry,
                external_cli_config_store,
                agent_config: &agent_config,
                group_context_store,
                group_turn_id: group_turn_id.as_deref(),
                default_mode: active_session_default_mode,
                status_context: status_context_from_agent_config(&agent_config),
                default_work_dir: group_context_store
                    .work_dir_by_session(&session_key)
                    .ok()
                    .flatten()
                    .map(|path| path.display().to_string())
                    .or_else(|| Some(agent_config.resolve_work_dir().display().to_string())),
            },
        )
        .await;
    } else if agent_session_manager.is_session_active(&session_key) {
        let busy_default_mode = busy_default_mode_for_agent_config(
            &agent_config,
            &external_cli_config_store.load(),
            Some(provider.id.as_str()),
        );
        handle_busy_message(
            &message_text,
            &session_key,
            BusyMessageContext {
                queue_manager,
                client,
                provider: &provider,
                event,
                message_log_store,
                agent_session_manager,
                progress_registry,
                external_cli_config_store,
                agent_config: &agent_config,
                group_context_store,
                group_turn_id: group_turn_id.as_deref(),
                default_mode: busy_default_mode,
                status_context: status_context_from_agent_config(&agent_config),
                default_work_dir: group_context_store
                    .work_dir_by_session(&session_key)
                    .ok()
                    .flatten()
                    .map(|path| path.display().to_string())
                    .or_else(|| Some(agent_config.resolve_work_dir().display().to_string())),
            },
        )
        .await;
    } else {
        let images = resolve_event_images(client, &provider, event, &message.images).await;
        let files = resolve_event_files(client, &provider, event, &message.files).await;
        let context = group_turn_id.clone().map(|group_turn_id| {
            crate::im_gateway::queue_manager::QueueItemContext {
                event_id: event.event_id.clone(),
                message_id: event.source.message_id.clone(),
                user_id: event.source.user_id.clone(),
                user_name: event.source.user_name.clone(),
                group_turn_id: Some(group_turn_id),
            }
        });
        let _ = queue_manager.push_queue_with_attachments_and_context(
            &session_key,
            message_text,
            images,
            files,
            context,
        );
        send_agent_reply(
            client,
            &provider,
            event,
            "⏳ 消息已排队，将在当前任务完成后处理。",
            message_log_store,
        )
        .await;
    }
}
