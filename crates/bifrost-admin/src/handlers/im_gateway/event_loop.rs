use super::*;

// ---------------------------------------------------------------------------

/// Time-windowed event deduplication filter.
///
/// During reconnection, the Feishu server may re-deliver events that were
/// already processed. This filter uses a bounded queue of recently-seen
/// event_ids with a TTL to efficiently discard duplicates.
pub(super) struct EventDedup {
    /// Ordered queue of (event_id, first_seen_at) for TTL expiry.
    window: VecDeque<(String, Instant)>,
    /// Maximum number of event_ids to retain.
    max_entries: usize,
    /// Events older than this duration are evicted.
    ttl: std::time::Duration,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EventLoopOptions {
    pub(super) send_online_notification: bool,
}

impl Default for EventLoopOptions {
    fn default() -> Self {
        Self {
            send_online_notification: true,
        }
    }
}

impl EventDedup {
    pub(super) fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(512),
            max_entries: 2048,
            ttl: std::time::Duration::from_secs(300), // 5 minutes
        }
    }

    pub(super) fn contains(&mut self, event_id: &str) -> bool {
        self.evict_expired();
        self.window.iter().any(|(id, _)| id == event_id)
    }

    pub(super) fn record(&mut self, event_id: &str) {
        if self.window.len() >= self.max_entries {
            self.window.pop_front();
        }
        self.window
            .push_back((event_id.to_string(), Instant::now()));
    }

    pub(super) fn remove(&mut self, event_id: &str) {
        self.window.retain(|(id, _)| id != event_id);
    }

    pub(super) fn evict_expired(&mut self) {
        let Some(cutoff) = Instant::now().checked_sub(self.ttl) else {
            return;
        };
        while let Some((_, ts)) = self.window.front() {
            if *ts < cutoff {
                self.window.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Event processing loop: receives events from the long connection and processes them.
///
/// Security: Feishu messages are limited to the configured bot owner.
/// Weixin is an inbound IM channel and does not apply owner filtering.
/// After provider-specific checks, matches routes and executes actions.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_event_loop(
    rx: mpsc::UnboundedReceiver<ImEvent>,
    client: ImProviderClient,
    provider: ImProviderConfig,
    event_store: Arc<ImEventStore>,
    message_log_store: Arc<ImMessageLogStore>,
    group_context_store: Arc<ImGroupContextStore>,
    route_store: Arc<ImRouteStore>,
    provider_store: Arc<ImProviderStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
    target_store: Arc<ImTargetStore>,
    connection_manager: Arc<ImConnectionManager>,
    agent_session_manager: Arc<ImAgentSessionManager>,
    external_cli_config_store: Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    queue_manager: Arc<SessionQueueManager>,
    progress_registry: Arc<ImAgentProgressRegistry>,
) {
    run_event_loop_with_options(
        rx,
        client,
        provider,
        event_store,
        message_log_store,
        group_context_store,
        route_store,
        provider_store,
        agent_config_store,
        schedule_store,
        scheduler,
        target_store,
        connection_manager,
        agent_session_manager,
        external_cli_config_store,
        queue_manager,
        progress_registry,
        EventLoopOptions::default(),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_event_loop_with_options(
    mut rx: mpsc::UnboundedReceiver<ImEvent>,
    client: ImProviderClient,
    provider: ImProviderConfig,
    event_store: Arc<ImEventStore>,
    message_log_store: Arc<ImMessageLogStore>,
    group_context_store: Arc<ImGroupContextStore>,
    route_store: Arc<ImRouteStore>,
    provider_store: Arc<ImProviderStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    _schedule_store: Arc<ImScheduleStore>,
    _scheduler: Arc<ImScheduler>,
    _target_store: Arc<ImTargetStore>,
    _connection_manager: Arc<ImConnectionManager>,
    agent_session_manager: Arc<ImAgentSessionManager>,
    external_cli_config_store: Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    queue_manager: Arc<SessionQueueManager>,
    progress_registry: Arc<ImAgentProgressRegistry>,
    options: EventLoopOptions,
) {
    info!(
        provider_id = %provider.id,
        owner_open_id = ?provider.owner_open_id,
        "event processing loop started"
    );

    // Initialize MCP manager from agent config (TOML + JSON merged)
    let init_config = agent_config_store.load();

    // Cleanup expired session files on startup if retention policy is active
    if let Some(ref history) = init_config.history {
        if history.persistence == bifrost_agent::config::HistoryPersistence::Last90Days {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cutoff = now.saturating_sub(90 * 24 * 3600);
            let data_dir = bifrost_agent::config::agent_home_dir();
            let removed = bifrost_agent::persistence::cleanup_expired_sessions(&data_dir, cutoff);
            if removed > 0 {
                info!(removed, "cleaned up expired session files (>90 days)");
            }
        }
    }

    // Send online notification to owner on connect
    if options.send_online_notification {
        if let Some(ref owner_open_id) = provider.owner_open_id {
            let online_target = ImTarget {
                id: "__online_notify__".to_string(),
                provider_id: provider.id.clone(),
                display_name: "Owner".to_string(),
                enabled: true,
                receive_id_type: "open_id".to_string(),
                receive_id: owner_open_id.clone(),
                default_msg_type: "text".to_string(),
                created_at: 0,
                updated_at: 0,
            };

            let online_context = build_online_notification_agent_context(
                &provider,
                &agent_config_store.load(),
                &external_cli_config_store.load(),
                &agent_session_manager,
            );
            let online_msg = build_online_notification_message_with_context(
                &provider,
                &current_device_name(),
                Some(&online_context),
            );
            let send_result = client
                .send_text(&provider, &online_target, &online_msg)
                .await;

            // Record outbound message log
            let (status, message_id, error_msg) = match &send_result {
                Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
                Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
            };
            let log = ImMessageLog {
                id: uuid_short(),
                provider_id: provider.id.clone(),
                direction: MessageDirection::Outbound,
                status,
                timestamp: now_ms(),
                target_id: Some(owner_open_id.clone()),
                target_name: Some("Owner".to_string()),
                message_id,
                msg_type: Some(outbound_log_msg_type(&provider, "text")),
                content_preview: Some(online_msg.to_string()),
                content: Some(online_msg.to_string()),
                trigger: Some("online".to_string()),
                error: error_msg,
                sender_open_id: None,
                event_id: None,
                reaction_added: None,
            };
            let _ = message_log_store.add(log);

            if let Err(e) = &send_result {
                error!(provider_id = %provider.id, error = %e, "failed to send online notification");
            } else {
                info!(provider_id = %provider.id, owner_open_id = %owner_open_id, "online notification sent");
            }
        }
    }

    let mut dedup = EventDedup::new();
    let mut session_mailboxes = SessionMailboxRegistry::new();
    let recovery_owner = uuid::Uuid::new_v4().to_string();
    let mut recovered_session_events = match recover_pending_feishu_thread_events(
        &provider.id,
        &group_context_store,
        &recovery_owner,
    ) {
        Ok(events) => events,
        Err(error) => {
            error!(provider_id = %provider.id, error = %error, "failed to claim pending Feishu topic recoveries");
            VecDeque::new()
        }
    };
    let mut inbound_open = true;

    loop {
        if !inbound_open && session_mailboxes.is_empty() && recovered_session_events.is_empty() {
            break;
        }
        let event = match recovered_session_events.pop_front() {
            Some(event) => event,
            None if inbound_open => {
                tokio::select! {
                    event = rx.recv() => match event {
                        Some(event) => event,
                        None => {
                            inbound_open = false;
                            continue;
                        },
                    },
                    completion = session_mailboxes.recv_completion() => {
                        if let Some(completion) = completion {
                            recover_session_completion(
                                &mut session_mailboxes,
                                &mut dedup,
                                &mut recovered_session_events,
                                completion,
                            );
                        }
                        continue;
                    }
                }
            }
            None => {
                if let Some(completion) = session_mailboxes.recv_completion().await {
                    recover_session_completion(
                        &mut session_mailboxes,
                        &mut dedup,
                        &mut recovered_session_events,
                        completion,
                    );
                }
                continue;
            }
        };
        let provider = provider_store
            .get(&event.provider_id)
            .unwrap_or_else(|| provider.clone());

        if !provider.enabled {
            info!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                "dropping inbound event because provider is disabled"
            );
            continue;
        }

        // Deduplication: per Feishu docs, use message_id for idempotency
        // ("如有幂等需求请使用 message_id 去重，不要依赖 event_id").
        // Falls back to event_id for non-message events.
        let dedup_key = event_dedup_key(&event).to_string();
        let new_group_command = feishu_new_group_command_text(&client, &provider, &event).await;
        let is_new_group_command = new_group_command.is_some();
        if !is_new_group_command && !dedup_key.is_empty() && dedup.contains(&dedup_key) {
            debug!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                message_id = ?event.source.message_id,
                "dropping duplicate event"
            );
            continue;
        }

        if is_new_group_command {
            if !dedup_key.is_empty() {
                dedup.record(&dedup_key);
            }
            if let Err(error) = event_store.add(event.clone()) {
                error!(error = %error, "failed to store /new event");
            }
            handle_im_new_group_command(
                new_group_command.as_deref().unwrap_or_default(),
                &client,
                &provider,
                &event,
                &group_context_store,
                &message_log_store,
            )
            .await;
            continue;
        }

        let dispatch_result = session_mailboxes.dispatch(event);
        let Some(mut event) = dispatch_result.unrouted_event else {
            if dispatch_result.delivered && !dedup_key.is_empty() {
                dedup.record(&dedup_key);
            }
            continue;
        };
        if !dedup_key.is_empty() {
            dedup.record(&dedup_key);
        }

        // Feishu bots use owner_open_id as a safety boundary. Weixin ClawBot is
        // an inbound IM channel, so owner_open_id only records the login user
        // and must not filter inbound messages.
        let is_group_event = crate::im_gateway::group_context::is_feishu_group_event(&event);
        if provider.provider_type == crate::im_gateway::types::ImProviderType::Feishu
            && !is_group_event
        {
            if let Some(ref owner_id) = provider.owner_open_id {
                let sender_id = event.source.user_id.as_deref().unwrap_or("");
                if sender_id != owner_id {
                    info!(
                        provider_id = %event.provider_id,
                        event_id = %event.event_id,
                        sender_open_id = %sender_id,
                        owner_open_id = %owner_id,
                        "rejecting message from non-owner user"
                    );
                    let log = ImMessageLog {
                        id: uuid_short(),
                        provider_id: event.provider_id.clone(),
                        direction: MessageDirection::Inbound,
                        status: MessageStatus::Rejected,
                        timestamp: now_ms(),
                        target_id: None,
                        target_name: None,
                        message_id: event.source.message_id.clone(),
                        msg_type: event.message.as_ref().and_then(|m| m.raw_type.clone()),
                        content_preview: event.message.as_ref().map(inbound_message_preview),
                        content: event.message.as_ref().map(|message| message.text.clone()),
                        trigger: Some("websocket".to_string()),
                        error: Some(format!("rejected: sender {} is not owner", sender_id)),
                        sender_open_id: Some(sender_id.to_string()),
                        event_id: Some(event.event_id.clone()),
                        reaction_added: None,
                    };
                    let _ = message_log_store.add(log);
                    continue;
                }
            }
        }

        info!(
            provider_id = %event.provider_id,
            event_id = %event.event_id,
            event_type = %event.event_type,
            message_type = ?event.message.as_ref().and_then(|m| m.raw_type.as_deref()),
            message_text_len = event.message.as_ref().map(|m| m.text.len()).unwrap_or(0),
            image_count = event.message.as_ref().map(|m| m.images.len()).unwrap_or(0),
            has_text = event.message.as_ref().is_some_and(|m| !m.text.trim().is_empty()),
            "received inbound event from owner"
        );

        let inbound_dispatch = if is_group_event {
            let session_busy = event.source.chat_id.as_deref().is_some_and(|chat_id| {
                agent_session_manager.is_session_active(
                    &crate::im_gateway::group_context::build_group_session_key(
                        &event.provider_id,
                        chat_id,
                    ),
                )
            });
            match prepare_group_inbound_dispatch(
                &client,
                &provider,
                &event,
                &group_context_store,
                session_busy,
            )
            .await
            {
                Ok(GroupInboundDispatch::Dispatch(dispatch)) => *dispatch,
                Ok(GroupInboundDispatch::Ambient) => {
                    if let Err(error) = event_store.add(event.clone()) {
                        error!(error = %error, "failed to store ambient group event");
                    }
                    let log = ImMessageLog {
                        id: uuid_short(),
                        provider_id: event.provider_id.clone(),
                        direction: MessageDirection::Inbound,
                        status: MessageStatus::Success,
                        timestamp: now_ms(),
                        target_id: event.source.chat_id.clone(),
                        target_name: None,
                        message_id: event.source.message_id.clone(),
                        msg_type: event.message.as_ref().and_then(|m| m.raw_type.clone()),
                        content_preview: event.message.as_ref().map(inbound_message_preview),
                        content: event.message.as_ref().map(|message| message.text.clone()),
                        trigger: Some("group_context".to_string()),
                        error: None,
                        sender_open_id: event.source.user_id.clone(),
                        event_id: Some(event.event_id.clone()),
                        reaction_added: None,
                    };
                    let _ = message_log_store.add(log);
                    continue;
                }
                Ok(GroupInboundDispatch::AddressedElsewhere) => continue,
                Err(error) => {
                    if let Err(store_error) = event_store.add(event.clone()) {
                        error!(error = %store_error, "failed to store rejected group trigger");
                    }
                    error!(
                        provider_id = %event.provider_id,
                        event_id = %event.event_id,
                        error = %error,
                        "failed to prepare Feishu group agent turn"
                    );
                    send_agent_reply(
                        &client,
                        &provider,
                        &event,
                        &format!("无法准备本次群聊上下文：{error}"),
                        &message_log_store,
                    )
                    .await;
                    continue;
                }
            }
        } else {
            if let Err(error) = event_store.add(event.clone()) {
                error!(error = %error, "failed to store event");
            }
            PreparedInboundDispatch {
                message_text: event
                    .message
                    .as_ref()
                    .map(|message| {
                        agent_message_text_with_reference(
                            message,
                            &event.provider_id,
                            event.source.user_id.as_deref(),
                            event.source.message_id.as_deref(),
                            &message_log_store,
                        )
                    })
                    .unwrap_or_default(),
                session_key: build_session_key(&event.provider_id, event.source.user_id.as_deref()),
                group_turn_id: None,
                reset_group_context: false,
                direct_reply: None,
                thread_anchor_message_id: None,
                thread_fallback_message: None,
                referenced_images: Vec::new(),
                referenced_files: Vec::new(),
                attachment_notices: Vec::new(),
            }
        };

        if is_group_event {
            if let Err(error) = event_store.add(event.clone()) {
                error!(error = %error, "failed to store accepted group event");
            }
        }

        prepend_referenced_attachments(&mut event, &inbound_dispatch);

        acknowledge_and_log_inbound_event(&client, &provider, &event, &message_log_store).await;
        if !inbound_dispatch.attachment_notices.is_empty() {
            let notice = attachment_notice_message(&inbound_dispatch.attachment_notices);
            send_agent_reply(&client, &provider, &event, &notice, &message_log_store).await;
        }
        if let Some(reply) = inbound_dispatch.direct_reply.as_deref() {
            send_agent_reply(&client, &provider, &event, reply, &message_log_store).await;
            if let Some(turn_id) = inbound_dispatch.group_turn_id.as_deref() {
                if let Err(error) = group_context_store.mark_turn_completed(turn_id, now_ms()) {
                    warn!(turn_id = %turn_id, error = %error, "failed to complete unavailable quoted-message turn");
                }
            }
            continue;
        }

        // --- Route matching & action execution ---
        let routes = route_store.list();
        // Group Agent semantics are deliberately deterministic: ambient
        // messages have already returned above, while triggers and slash
        // commands must use the same Agent command path as direct messages.
        // Legacy regex/script routes remain a p2p/event automation feature.
        let matches = if is_group_event {
            Vec::new()
        } else {
            ImEventRouter::match_routes(&event, &routes)
        };
        if matches.is_empty() {
            // No route matched — try default agent chat if agent is enabled
            let agent_config = agent_config_store.load();
            if agent_config.enabled {
                if let Some(ref msg) = event.message {
                    if !msg.text.trim().is_empty()
                        || !msg.images.is_empty()
                        || !msg.files.is_empty()
                    {
                        let session_key = inbound_dispatch.session_key.clone();
                        let agent_message = inbound_dispatch.message_text.clone();
                        let effective_agent_config =
                            effective_agent_config_for_provider(&agent_config, &provider);
                        let busy_default_mode = busy_default_mode_for_agent_config(
                            &effective_agent_config,
                            &external_cli_config_store.load(),
                            Some(provider.id.as_str()),
                        );

                        // ── Guide/Queue mode: check if session is busy ──
                        if agent_session_manager.is_session_active(&session_key) {
                            handle_busy_message(
                                &agent_message,
                                &session_key,
                                BusyMessageContext {
                                    queue_manager: &queue_manager,
                                    client: &client,
                                    provider: &provider,
                                    event: &event,
                                    message_log_store: &message_log_store,
                                    agent_session_manager: &agent_session_manager,
                                    progress_registry: &progress_registry,
                                    external_cli_config_store: &external_cli_config_store,
                                    agent_config: &effective_agent_config,
                                    group_context_store: &group_context_store,
                                    group_turn_id: inbound_dispatch.group_turn_id.as_deref(),
                                    default_mode: busy_default_mode,
                                    status_context: status_context_from_agent_config(
                                        &effective_agent_config,
                                    ),
                                    default_work_dir: group_context_store
                                        .work_dir_by_session(&session_key)
                                        .ok()
                                        .flatten()
                                        .map(|path| path.display().to_string())
                                        .or_else(|| {
                                            Some(
                                                effective_agent_config
                                                    .resolve_work_dir()
                                                    .display()
                                                    .to_string(),
                                            )
                                        }),
                                },
                            )
                            .await;
                            continue;
                        }

                        if handle_idle_im_command(
                            &agent_message,
                            &session_key,
                            &effective_agent_config,
                            IdleImCommandContext {
                                client: &client,
                                provider: &provider,
                                provider_store: &provider_store,
                                group_context_store: &group_context_store,
                                external_cli_config_store: &external_cli_config_store,
                                event: &event,
                                message_log_store: &message_log_store,
                                agent_session_manager: &agent_session_manager,
                                queue_manager: &queue_manager,
                            },
                        )
                        .await
                        {
                            external_runner::finalize_current_feishu_thread_binding(
                                &group_context_store,
                                &provider.id,
                                &event,
                                "ready",
                            );
                            continue;
                        }

                        let runner_id = group_context_store
                            .runner_id_by_session(&session_key)
                            .ok()
                            .flatten()
                            .or_else(|| {
                                effective_agent_config
                                    .runner
                                    .as_ref()
                                    .and_then(|runner| runner.custom_runner_id())
                                    .map(ToString::to_string)
                            });
                        let (images, files) =
                            resolve_initial_external_cli_attachments(&client, &provider, &event)
                                .await;
                        spawn_external_cli_agent_chat(
                            &mut session_mailboxes,
                            ExternalCliChatTaskContext {
                                client: client.clone(),
                                provider: provider.clone(),
                                provider_store: Arc::clone(&provider_store),
                                event: event.clone(),
                                message_log_store: Arc::clone(&message_log_store),
                                agent_config_store: Arc::clone(&agent_config_store),
                                external_cli_config_store: Arc::clone(&external_cli_config_store),
                                agent_session_manager: Arc::clone(&agent_session_manager),
                                queue_manager: Arc::clone(&queue_manager),
                                progress_registry: Arc::clone(&progress_registry),
                                event_store: Arc::clone(&event_store),
                                group_context_store: Arc::clone(&group_context_store),
                            },
                            ExternalCliChatInput {
                                message_text: agent_message,
                                images,
                                files,
                                session_key: session_key.clone(),
                                adapter_override: None,
                                instructions_override: None,
                                delivery_override: None,
                                runner_id_override: runner_id.clone(),
                                runner_selected: runner_id.is_some(),
                                group_turn_id: inbound_dispatch.group_turn_id.clone(),
                                reset_group_context: inbound_dispatch.reset_group_context,
                                thread_anchor_message_id: inbound_dispatch
                                    .thread_anchor_message_id
                                    .clone(),
                                thread_fallback_message: inbound_dispatch
                                    .thread_fallback_message
                                    .clone(),
                            },
                        );
                        continue;
                    }
                }
            } else if let Some(turn_id) = inbound_dispatch.group_turn_id.as_deref() {
                if let Err(error) =
                    group_context_store.release_turn(turn_id, "IM Agent is disabled", now_ms())
                {
                    warn!(turn_id = %turn_id, error = %error, "failed to release undispatched group turn");
                }
            }
            continue;
        }

        // Execute first matched route
        let route_match = &matches[0];
        info!(
            route_id = %route_match.route.id,
            route_name = %route_match.route.name,
            "executing matched route action"
        );

        match &route_match.route.action {
            ImRouteAction::RunScriptAndReply { .. } => {
                // Script execution (existing logic, kept as-is for this route type)
                info!(route_id = %route_match.route.id, "RunScriptAndReply action matched (execution handled by task executor)");
            }
            ImRouteAction::AgentChat { .. } => {
                let raw_message_text = route_match.message_text.as_deref().unwrap_or("");
                let has_attachments = event
                    .message
                    .as_ref()
                    .is_some_and(|message| !message.images.is_empty() || !message.files.is_empty());
                if raw_message_text.trim().is_empty() && !has_attachments {
                    continue;
                }
                let message_text = if is_group_event {
                    inbound_dispatch.message_text.clone()
                } else {
                    event
                        .message
                        .as_ref()
                        .map(agent_message_text)
                        .unwrap_or_else(|| raw_message_text.to_string())
                };
                let session_key = inbound_dispatch.session_key.clone();
                let agent_config =
                    effective_agent_config_for_provider(&agent_config_store.load(), &provider);
                let busy_default_mode = busy_default_mode_for_agent_config(
                    &agent_config,
                    &external_cli_config_store.load(),
                    Some(provider.id.as_str()),
                );

                // ── Guide/Queue mode: check if session is busy ──
                if agent_session_manager.is_session_active(&session_key) {
                    handle_busy_message(
                        &message_text,
                        &session_key,
                        BusyMessageContext {
                            queue_manager: &queue_manager,
                            client: &client,
                            provider: &provider,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_session_manager: &agent_session_manager,
                            progress_registry: &progress_registry,
                            external_cli_config_store: &external_cli_config_store,
                            agent_config: &agent_config,
                            group_context_store: &group_context_store,
                            group_turn_id: inbound_dispatch.group_turn_id.as_deref(),
                            default_mode: busy_default_mode,
                            status_context: status_context_from_agent_config(&agent_config),
                            default_work_dir: group_context_store
                                .work_dir_by_session(&session_key)
                                .ok()
                                .flatten()
                                .map(|path| path.display().to_string())
                                .or_else(|| {
                                    Some(agent_config.resolve_work_dir().display().to_string())
                                }),
                        },
                    )
                    .await;
                    continue;
                }

                if handle_idle_im_command(
                    &message_text,
                    &session_key,
                    &agent_config,
                    IdleImCommandContext {
                        client: &client,
                        provider: &provider,
                        provider_store: &provider_store,
                        group_context_store: &group_context_store,
                        external_cli_config_store: &external_cli_config_store,
                        event: &event,
                        message_log_store: &message_log_store,
                        agent_session_manager: &agent_session_manager,
                        queue_manager: &queue_manager,
                    },
                )
                .await
                {
                    external_runner::finalize_current_feishu_thread_binding(
                        &group_context_store,
                        &provider.id,
                        &event,
                        "ready",
                    );
                    continue;
                }

                let runner_id = group_context_store
                    .runner_id_by_session(&session_key)
                    .ok()
                    .flatten()
                    .or_else(|| {
                        agent_config
                            .runner
                            .as_ref()
                            .and_then(|runner| runner.custom_runner_id())
                            .map(ToString::to_string)
                    });
                let (images, files) =
                    resolve_initial_external_cli_attachments(&client, &provider, &event).await;
                spawn_external_cli_agent_chat(
                    &mut session_mailboxes,
                    ExternalCliChatTaskContext {
                        client: client.clone(),
                        provider: provider.clone(),
                        provider_store: Arc::clone(&provider_store),
                        event: event.clone(),
                        message_log_store: Arc::clone(&message_log_store),
                        agent_config_store: Arc::clone(&agent_config_store),
                        external_cli_config_store: Arc::clone(&external_cli_config_store),
                        agent_session_manager: Arc::clone(&agent_session_manager),
                        queue_manager: Arc::clone(&queue_manager),
                        progress_registry: Arc::clone(&progress_registry),
                        event_store: Arc::clone(&event_store),
                        group_context_store: Arc::clone(&group_context_store),
                    },
                    ExternalCliChatInput {
                        message_text,
                        images,
                        files,
                        session_key: session_key.clone(),
                        adapter_override: None,
                        instructions_override: None,
                        delivery_override: None,
                        runner_id_override: runner_id.clone(),
                        runner_selected: runner_id.is_some(),
                        group_turn_id: inbound_dispatch.group_turn_id.clone(),
                        reset_group_context: inbound_dispatch.reset_group_context,
                        thread_anchor_message_id: inbound_dispatch.thread_anchor_message_id.clone(),
                        thread_fallback_message: inbound_dispatch.thread_fallback_message.clone(),
                    },
                );
                continue;
            }
            ImRouteAction::ExternalCliAgentChat {
                adapter,
                instructions,
                delivery_mode,
                ..
            } => {
                let raw_message_text = route_match.message_text.as_deref().unwrap_or("");
                let has_attachments = event
                    .message
                    .as_ref()
                    .is_some_and(|message| !message.images.is_empty() || !message.files.is_empty());
                if raw_message_text.trim().is_empty() && !has_attachments {
                    continue;
                }
                let message_text = if is_group_event {
                    inbound_dispatch.message_text.clone()
                } else {
                    event
                        .message
                        .as_ref()
                        .map(agent_message_text)
                        .unwrap_or_else(|| raw_message_text.to_string())
                };
                let session_key = inbound_dispatch.session_key.clone();

                if matches!(message_text.trim(), "/q" | "/pwd")
                    || parse_im_runner_command(&message_text).is_some()
                {
                    let agent_config =
                        effective_agent_config_for_provider(&agent_config_store.load(), &provider);
                    if handle_idle_im_command(
                        &message_text,
                        &session_key,
                        &agent_config,
                        IdleImCommandContext {
                            client: &client,
                            provider: &provider,
                            provider_store: &provider_store,
                            group_context_store: &group_context_store,
                            external_cli_config_store: &external_cli_config_store,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_session_manager: &agent_session_manager,
                            queue_manager: &queue_manager,
                        },
                    )
                    .await
                    {
                        external_runner::finalize_current_feishu_thread_binding(
                            &group_context_store,
                            &provider.id,
                            &event,
                            "ready",
                        );
                        continue;
                    }
                }

                let (images, files) =
                    resolve_initial_external_cli_attachments(&client, &provider, &event).await;
                spawn_external_cli_agent_chat(
                    &mut session_mailboxes,
                    ExternalCliChatTaskContext {
                        client: client.clone(),
                        provider: provider.clone(),
                        provider_store: Arc::clone(&provider_store),
                        event: event.clone(),
                        message_log_store: Arc::clone(&message_log_store),
                        agent_config_store: Arc::clone(&agent_config_store),
                        external_cli_config_store: Arc::clone(&external_cli_config_store),
                        agent_session_manager: Arc::clone(&agent_session_manager),
                        queue_manager: Arc::clone(&queue_manager),
                        progress_registry: Arc::clone(&progress_registry),
                        event_store: Arc::clone(&event_store),
                        group_context_store: Arc::clone(&group_context_store),
                    },
                    ExternalCliChatInput {
                        message_text,
                        images,
                        files,
                        session_key: session_key.clone(),
                        adapter_override: adapter.clone(),
                        instructions_override: instructions.clone(),
                        delivery_override: *delivery_mode,
                        runner_id_override: None,
                        runner_selected: false,
                        group_turn_id: inbound_dispatch.group_turn_id.clone(),
                        reset_group_context: inbound_dispatch.reset_group_context,
                        thread_anchor_message_id: inbound_dispatch.thread_anchor_message_id.clone(),
                        thread_fallback_message: inbound_dispatch.thread_fallback_message.clone(),
                    },
                );
            }
        }
    }

    if let Err(error) = group_context_store.release_feishu_thread_recovery_claims(
        &provider.id,
        &recovery_owner,
        now_ms(),
    ) {
        warn!(provider_id = %provider.id, error = %error, "failed to release Feishu topic recovery claims on event-loop exit");
    }
    info!(
        provider_id = %provider.id,
        "event processing loop ended"
    );
}

async fn feishu_new_group_command_text(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> Option<String> {
    if provider.provider_type != ImProviderType::Feishu {
        return None;
    }
    let message = event.message.as_ref()?;
    if !crate::im_gateway::group_context::is_feishu_group_event(event) {
        return parse_im_new_group_command(&message.text)
            .is_some()
            .then(|| message.text.trim().to_string());
    }
    let bot_identity = if message.mentions.is_empty() {
        None
    } else {
        client.feishu()?.fetch_bot_identity(provider).await.ok()
    };
    match crate::im_gateway::group_context::classify_group_message(
        message,
        bot_identity.as_ref(),
        false,
    ) {
        crate::im_gateway::group_context::GroupMessageDisposition::SystemCommand {
            command,
            ..
        } if parse_im_new_group_command(&command).is_some() => Some(command),
        _ => None,
    }
}

fn event_dedup_key(event: &ImEvent) -> &str {
    event
        .source
        .message_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&event.event_id)
}

fn recover_session_completion(
    session_mailboxes: &mut SessionMailboxRegistry,
    dedup: &mut EventDedup,
    recovered_session_events: &mut VecDeque<ImEvent>,
    completion: SessionTaskCompletion,
) {
    for event in session_mailboxes.finish(completion) {
        let dedup_key = event_dedup_key(&event).to_string();
        if !dedup_key.is_empty() {
            dedup.remove(&dedup_key);
        }
        recovered_session_events.push_back(event);
    }
}

fn recover_pending_feishu_thread_events(
    provider_id: &str,
    group_context_store: &ImGroupContextStore,
    recovery_owner: &str,
) -> Result<VecDeque<ImEvent>, String> {
    let events = group_context_store
        .claim_pending_feishu_thread_bindings(provider_id, recovery_owner, now_ms())?
        .into_iter()
        .map(|binding| {
            if let Some(event_json) = binding.initial_event_json.as_deref() {
                if let Ok(event) = serde_json::from_str::<ImEvent>(event_json) {
                    return event;
                }
            }
            ImEvent {
                event_id: format!("recovered:{}", binding.trigger_message_id),
                provider_id: binding.provider_id,
                provider_type: ImProviderType::Feishu,
                event_type: "message.recovered".to_string(),
                source: crate::im_gateway::types::ImEventSource {
                    chat_id: Some(binding.chat_id),
                    chat_type: Some("group".to_string()),
                    user_id: None,
                    user_name: None,
                    sender_type: Some("user".to_string()),
                    message_id: Some(binding.trigger_message_id),
                },
                message: Some(crate::im_gateway::types::ImEventMessage {
                    text: binding.initial_message,
                    mentions: Vec::new(),
                    images: Vec::new(),
                    files: Vec::new(),
                    reply_to: None,
                    raw_type: Some("text".to_string()),
                    raw_content: None,
                    create_time: None,
                    update_time: None,
                    root_id: Some(binding.root_message_id.clone()),
                    parent_id: Some(binding.root_message_id),
                    thread_id: Some(binding.feishu_thread_id),
                }),
                received_at: now_ms(),
                raw_digest: Some("startup_recovery".to_string()),
            }
        })
        .collect::<VecDeque<_>>();
    Ok(events)
}

pub(super) async fn acknowledge_and_log_inbound_event(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let mut reaction_added = None;
    if let Some(ref message_id) = event.source.message_id {
        match client.add_reaction(provider, message_id, "OK").await {
            Ok(true) => {
                info!(message_id = %message_id, "added OK reaction to message");
                reaction_added = Some(true);
            }
            Ok(false) => {
                reaction_added = None;
            }
            Err(error) => {
                error!(message_id = %message_id, error = %error, "failed to add OK reaction");
                reaction_added = Some(false);
            }
        }
    }

    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: event.provider_id.clone(),
        direction: MessageDirection::Inbound,
        status: MessageStatus::Success,
        timestamp: now_ms(),
        target_id: None,
        target_name: None,
        message_id: event.source.message_id.clone(),
        msg_type: event
            .message
            .as_ref()
            .and_then(|message| message.raw_type.clone()),
        content_preview: event.message.as_ref().map(inbound_message_preview),
        content: event.message.as_ref().map(|message| message.text.clone()),
        trigger: Some("websocket".to_string()),
        error: None,
        sender_open_id: event.source.user_id.clone(),
        event_id: Some(event.event_id.clone()),
        reaction_added,
    };
    if let Err(error) = message_log_store.add(log) {
        error!(error = %error, "failed to store inbound message log");
    }
}

// ---------------------------------------------------------------------------

struct ExternalCliChatContext<'a> {
    rx: &'a mut mpsc::UnboundedReceiver<ImEvent>,
    client: &'a ImProviderClient,
    provider: &'a ImProviderConfig,
    provider_store: &'a Arc<ImProviderStore>,
    event: &'a ImEvent,
    message_log_store: &'a Arc<ImMessageLogStore>,
    agent_config_store: &'a Arc<ImAgentConfigStore>,
    external_cli_config_store: &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    agent_session_manager: &'a Arc<ImAgentSessionManager>,
    queue_manager: &'a Arc<SessionQueueManager>,
    progress_registry: &'a Arc<ImAgentProgressRegistry>,
    event_store: &'a Arc<ImEventStore>,
    group_context_store: &'a Arc<ImGroupContextStore>,
}

struct ExternalCliChatInput {
    message_text: String,
    images: Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
    files: Vec<crate::im_gateway::external_cli::ExternalCliFileInput>,
    session_key: String,
    adapter_override: Option<String>,
    instructions_override: Option<String>,
    delivery_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
    runner_id_override: Option<String>,
    runner_selected: bool,
    group_turn_id: Option<String>,
    reset_group_context: bool,
    thread_anchor_message_id: Option<String>,
    thread_fallback_message: Option<String>,
}

async fn resolve_initial_external_cli_attachments(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> (
    Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
    Vec<crate::im_gateway::external_cli::ExternalCliFileInput>,
) {
    // Weixin often emits the caption and media as adjacent events. Register the
    // session mailbox before doing any CDN I/O so the companion event can be
    // collected while the first attachment is downloading/decrypting.
    if provider.provider_type == ImProviderType::Weixin {
        return (Vec::new(), Vec::new());
    }
    let Some(message) = event.message.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let (images, files) = tokio::join!(
        resolve_event_images(client, provider, event, &message.images),
        resolve_event_files(client, provider, event, &message.files),
    );
    (external_cli_images_from_chat_images(images), files)
}

pub(super) struct PreparedInboundDispatch {
    pub(super) message_text: String,
    pub(super) session_key: String,
    pub(super) group_turn_id: Option<String>,
    pub(super) reset_group_context: bool,
    pub(super) direct_reply: Option<String>,
    pub(super) thread_anchor_message_id: Option<String>,
    pub(super) thread_fallback_message: Option<String>,
    pub(super) referenced_images: Vec<ImImageAttachment>,
    pub(super) referenced_files: Vec<ImFileAttachment>,
    pub(super) attachment_notices: Vec<String>,
}

pub(super) fn prepend_referenced_attachments(
    event: &mut ImEvent,
    dispatch: &PreparedInboundDispatch,
) {
    if dispatch.referenced_images.is_empty() && dispatch.referenced_files.is_empty() {
        return;
    }
    let Some(message) = event.message.as_mut() else {
        return;
    };
    let mut images = dispatch.referenced_images.clone();
    images.append(&mut message.images);
    message.images = images;
    let mut files = dispatch.referenced_files.clone();
    files.append(&mut message.files);
    message.files = files;
}

fn referenced_attachment_prompt_note(image_count: usize, file_count: usize) -> String {
    format!("引用附件已下载到本地；Attached Images/Files 列表最前面的 {image_count} 张图片和 {file_count} 个文件来自引用消息，请优先使用这些绝对路径。")
}

pub(super) fn attachment_notice_message(notices: &[String]) -> String {
    let notices = notices.join("\n- ");
    format!("附件处理提示（不影响任务继续执行）：\n- {notices}")
}

pub(super) enum GroupInboundDispatch {
    Dispatch(Box<PreparedInboundDispatch>),
    Ambient,
    AddressedElsewhere,
}

#[cfg(test)]
impl GroupInboundDispatch {
    fn is_none(&self) -> bool {
        !matches!(self, Self::Dispatch(_))
    }

    fn unwrap(self) -> PreparedInboundDispatch {
        match self {
            Self::Dispatch(dispatch) => *dispatch,
            Self::Ambient => panic!("expected dispatch, got ambient message"),
            Self::AddressedElsewhere => {
                panic!("expected dispatch, got addressed-elsewhere message")
            }
        }
    }

    fn expect(self, message: &str) -> PreparedInboundDispatch {
        match self {
            Self::Dispatch(dispatch) => *dispatch,
            _ => panic!("{message}"),
        }
    }
}

pub(super) async fn prepare_group_inbound_dispatch(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    store: &ImGroupContextStore,
    session_busy: bool,
) -> Result<GroupInboundDispatch, String> {
    use crate::im_gateway::group_context::{classify_group_message, GroupMessageDisposition};

    let message = event
        .message
        .as_ref()
        .ok_or_else(|| "group message event has no message body".to_string())?;
    let chat_id = event
        .source
        .chat_id
        .as_deref()
        .ok_or_else(|| "group event is missing chat_id".to_string())?;

    if let Some((thread_id, root_message_id)) =
        crate::im_gateway::group_context::feishu_thread_parts(event)
    {
        let session_key = crate::im_gateway::group_context::build_group_thread_session_key(
            &event.provider_id,
            chat_id,
            thread_id,
        );
        if let Some(binding) =
            store.feishu_thread_binding(&event.provider_id, chat_id, thread_id)?
        {
            if !message.mentions.is_empty() {
                let bot_identity = match client.feishu() {
                    Some(feishu) => feishu.fetch_bot_identity(provider).await.ok(),
                    None => None,
                };
                if matches!(
                    classify_group_message(message, bot_identity.as_ref(), session_busy),
                    GroupMessageDisposition::AddressedElsewhere
                ) {
                    return Ok(GroupInboundDispatch::AddressedElsewhere);
                }
            }
            store.record_event(event, "event")?;
            let recovering = event.source.message_id.as_deref()
                == Some(binding.trigger_message_id.as_str())
                && matches!(
                    binding.state.as_str(),
                    "waiting_source" | "initializing" | "recovering"
                );
            return Ok(GroupInboundDispatch::Dispatch(Box::new(
                PreparedInboundDispatch {
                    message_text: if recovering {
                        binding.initial_message.clone()
                    } else {
                        agent_message_text(message)
                    },
                    session_key: binding.derived_session_key,
                    group_turn_id: None,
                    reset_group_context: false,
                    direct_reply: None,
                    thread_anchor_message_id: recovering
                        .then_some(binding.source_message_id.clone()),
                    thread_fallback_message: binding.fallback_message,
                    referenced_images: Vec::new(),
                    referenced_files: Vec::new(),
                    attachment_notices: Vec::new(),
                },
            )));
        }

        let anchor = store
            .feishu_message_anchor(&event.provider_id, root_message_id)?
            .filter(|anchor| anchor.chat_id == chat_id && anchor.is_derivable());

        let needs_identity =
            !message.mentions.is_empty() && !message.mentions.iter().any(|mention| mention.is_bot);
        let bot_identity = if needs_identity {
            match client.feishu() {
                Some(feishu) => feishu.fetch_bot_identity(provider).await.ok(),
                None => None,
            }
        } else {
            None
        };
        let disposition = classify_group_message(message, bot_identity.as_ref(), session_busy);
        let (active_request, system_command) = if anchor.is_some()
            && !matches!(disposition, GroupMessageDisposition::AddressedElsewhere)
        {
            (agent_message_text(message), false)
        } else {
            match disposition {
                GroupMessageDisposition::AgentTrigger { active_request, .. } => {
                    (active_request, false)
                }
                GroupMessageDisposition::SystemCommand { command, .. } => (command, true),
                GroupMessageDisposition::AddressedElsewhere => {
                    return Ok(GroupInboundDispatch::AddressedElsewhere)
                }
                GroupMessageDisposition::Ambient => return Ok(GroupInboundDispatch::Ambient),
            }
        };
        store.record_event(event, "event")?;
        let (
            message_text,
            fallback_message,
            source_kind,
            source_adapter,
            source_thread_id,
            source_turn_id,
        ) = if system_command {
            (
                active_request,
                None,
                "message_context".to_string(),
                None,
                None,
                None,
            )
        } else if let Some(anchor) = anchor.as_ref() {
            let fallback_message = if anchor.status == "pending" {
                let feishu = client.feishu().ok_or_else(|| {
                    "Feishu topic root messages require a Feishu provider".to_string()
                })?;
                let root = feishu
                    .fetch_message(provider, root_message_id)
                    .await
                    .map_err(|error| error.to_string())?;
                if root.chat_id != chat_id {
                    return Err("话题根消息不属于当前群聊，已拒绝读取以避免跨群泄露。".to_string());
                }
                Some(
                    crate::im_gateway::group_context::build_feishu_thread_prompt(
                        &root.sender_id,
                        None,
                        &root.text,
                        event.source.user_id.as_deref().unwrap_or("unknown"),
                        event.source.user_name.as_deref(),
                        &active_request,
                    ),
                )
            } else {
                None
            };
            (
                active_request,
                fallback_message,
                "local_checkpoint".to_string(),
                Some(anchor.adapter.clone()),
                anchor
                    .checkpoint_thread_id
                    .clone()
                    .or_else(|| anchor.external_thread_id.clone()),
                anchor.external_turn_id.clone(),
            )
        } else {
            let feishu = client.feishu().ok_or_else(|| {
                "Feishu topic root messages require a Feishu provider".to_string()
            })?;
            let root = feishu
                .fetch_message(provider, root_message_id)
                .await
                .map_err(|error| error.to_string())?;
            if root.chat_id != chat_id {
                return Err("话题根消息不属于当前群聊，已拒绝读取以避免跨群泄露。".to_string());
            }
            (
                crate::im_gateway::group_context::build_feishu_thread_prompt(
                    &root.sender_id,
                    None,
                    &root.text,
                    event.source.user_id.as_deref().unwrap_or("unknown"),
                    event.source.user_name.as_deref(),
                    &active_request,
                ),
                None,
                "message_context".to_string(),
                None,
                None,
                None,
            )
        };
        let binding =
            store.claim_feishu_thread_binding(
                &crate::im_gateway::group_context::FeishuThreadBinding {
                    provider_id: event.provider_id.clone(),
                    chat_id: chat_id.to_string(),
                    feishu_thread_id: thread_id.to_string(),
                    root_message_id: root_message_id.to_string(),
                    derived_session_key: session_key.clone(),
                    source_kind,
                    source_message_id: root_message_id.to_string(),
                    source_adapter,
                    source_thread_id,
                    source_turn_id,
                    trigger_message_id: event.source.message_id.clone().unwrap_or_default(),
                    initial_message: message_text.clone(),
                    fallback_message: fallback_message.clone(),
                    initial_event_json: Some(serde_json::to_string(event).map_err(|error| {
                        format!("serialize Feishu topic recovery event: {error}")
                    })?),
                    state: if anchor
                        .as_ref()
                        .is_some_and(|value| value.status == "pending")
                    {
                        "waiting_source".to_string()
                    } else {
                        "initializing".to_string()
                    },
                },
                event.received_at,
            )?;
        store.initialize_feishu_thread_session_settings(
            &event.provider_id,
            chat_id,
            &binding.derived_session_key,
            anchor.as_ref().map(|anchor| anchor.runner_id.as_str()),
            event.received_at,
        )?;
        return Ok(GroupInboundDispatch::Dispatch(Box::new(
            PreparedInboundDispatch {
                message_text,
                session_key: binding.derived_session_key,
                group_turn_id: None,
                reset_group_context: false,
                direct_reply: None,
                thread_anchor_message_id: anchor.map(|_| root_message_id.to_string()),
                thread_fallback_message: fallback_message,
                referenced_images: Vec::new(),
                referenced_files: Vec::new(),
                attachment_notices: Vec::new(),
            },
        )));
    }
    let needs_identity =
        !message.mentions.is_empty() && !message.mentions.iter().any(|mention| mention.is_bot);
    let bot_identity = if needs_identity {
        match client.feishu() {
            Some(feishu) => match feishu.fetch_bot_identity(provider).await {
                Ok(identity) => Some(identity),
                Err(error) => {
                    warn!(
                        provider_id = %provider.id,
                        error = %error,
                        "failed to resolve Feishu bot identity for group mention"
                    );
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    let disposition = classify_group_message(message, bot_identity.as_ref(), session_busy);
    if matches!(disposition, GroupMessageDisposition::AddressedElsewhere) {
        return Ok(GroupInboundDispatch::AddressedElsewhere);
    }
    // Only the addressed bot resolves a referenced message. Every provider may
    // run on a different machine, so the Feishu message API is authoritative;
    // the local group ledger is only a per-provider cache for prompt assembly.
    let existing_turn = if matches!(disposition, GroupMessageDisposition::AgentTrigger { .. }) {
        let trigger_message_id = event
            .source
            .message_id
            .as_deref()
            .ok_or_else(|| "group event is missing message_id".to_string())?;
        store.existing_turn(&event.provider_id, trigger_message_id)?
    } else {
        None
    };
    if existing_turn.is_none()
        && matches!(disposition, GroupMessageDisposition::AgentTrigger { .. })
    {
        if let Some(parent_id) = message
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let feishu = client
                .feishu()
                .ok_or_else(|| "referenced group messages require a Feishu provider".to_string())?;
            let referenced = feishu
                .fetch_message(provider, parent_id)
                .await
                .map_err(|error| error.to_string())?;
            if referenced.chat_id != chat_id {
                return Err("被引用消息不属于当前群聊，已拒绝读取以避免跨群泄露。".to_string());
            }
            store.record_fetched_message(
                &event.provider_id,
                chat_id,
                &referenced,
                event.received_at,
            )?;
        }
    }
    store.record_event(event, "event")?;
    let supplied_chat_name = message
        .raw_content
        .as_ref()
        .and_then(|content| content.get("_bifrost_debug_chat_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(chat_name) = supplied_chat_name {
        store.set_chat_name(&event.provider_id, chat_id, chat_name, event.received_at)?;
    } else if store.chat_name(&event.provider_id, chat_id)?.is_none() {
        if let Some(feishu) = client.feishu() {
            if !store.begin_chat_name_lookup(&event.provider_id, chat_id, now_ms()) {
                debug!(
                    provider_id = %provider.id,
                    chat_id = %chat_id,
                    "skipping Feishu group name lookup during failure backoff"
                );
            } else {
                match feishu.fetch_chat_name(provider, chat_id).await {
                    Ok(chat_name) => {
                        store.set_chat_name(
                            &event.provider_id,
                            chat_id,
                            &chat_name,
                            event.received_at,
                        )?;
                    }
                    Err(error) => {
                        warn!(
                            provider_id = %provider.id,
                            chat_id = %chat_id,
                            error = %error,
                            "failed to resolve Feishu group name"
                        );
                    }
                }
            }
        }
    }
    match disposition {
        GroupMessageDisposition::Ambient => Ok(GroupInboundDispatch::Ambient),
        GroupMessageDisposition::AddressedElsewhere => Ok(GroupInboundDispatch::AddressedElsewhere),
        GroupMessageDisposition::SystemCommand {
            command,
            reset_context,
        } => Ok(GroupInboundDispatch::Dispatch(Box::new(
            PreparedInboundDispatch {
                message_text: command,
                session_key: crate::im_gateway::group_context::build_group_session_key(
                    &event.provider_id,
                    chat_id,
                ),
                group_turn_id: None,
                reset_group_context: reset_context,
                direct_reply: None,
                thread_anchor_message_id: None,
                thread_fallback_message: None,
                referenced_images: Vec::new(),
                referenced_files: Vec::new(),
                attachment_notices: Vec::new(),
            },
        ))),
        GroupMessageDisposition::AgentTrigger {
            kind,
            active_request,
            command_prefix,
        } => {
            if let Some(existing) = existing_turn.as_ref() {
                let recoverable = matches!(existing.status.as_str(), "prepared" | "dispatched");
                if session_busy || !recoverable {
                    return Ok(GroupInboundDispatch::Ambient);
                }
                warn!(
                    turn_id = %existing.turn_id,
                    status = %existing.status,
                    "recovering nonterminal group turn from a redelivered event"
                );
            }
            let trigger_message_id = existing_turn
                .as_ref()
                .map(|turn| turn.trigger_message_id.as_str())
                .or(event.source.message_id.as_deref())
                .ok_or_else(|| "group event is missing message_id".to_string())?;
            let (referenced_images, referenced_files, attachment_notices) = match store
                .referenced_attachments_for_trigger(&event.provider_id, trigger_message_id)?
            {
                Some(referenced) => {
                    hydrate_referenced_group_attachments(client, provider, referenced).await
                }
                None => (Vec::new(), Vec::new(), Vec::new()),
            };
            let prepared = match existing_turn {
                Some(mut existing) => {
                    existing.duplicate = true;
                    existing
                }
                None => store.prepare_turn(event, kind, &active_request)?,
            };
            store.mark_turn_dispatched(&prepared.turn_id, now_ms())?;
            let mut message_text = prepared.delivery_message(command_prefix);
            if !referenced_images.is_empty() || !referenced_files.is_empty() {
                message_text.push_str("\n\n");
                message_text.push_str(&referenced_attachment_prompt_note(
                    referenced_images.len(),
                    referenced_files.len(),
                ));
            }
            if !attachment_notices.is_empty() {
                message_text.push_str("\n\n");
                message_text.push_str(&attachment_notice_message(&attachment_notices));
            }
            Ok(GroupInboundDispatch::Dispatch(Box::new(PreparedInboundDispatch {
                message_text,
                session_key: prepared.session_key,
                group_turn_id: Some(prepared.turn_id),
                reset_group_context: false,
                direct_reply: prepared.quoted_message_missing.then(|| {
                    "我无法看到你引用的这条消息内容，请重新发送这条消息，或把内容补充到 @ 后面。"
                        .to_string()
                }),
                thread_anchor_message_id: None,
                thread_fallback_message: None,
                referenced_images,
                referenced_files,
                attachment_notices,
            })))
        }
    }
}

mod external_runner;
use external_runner::*;
mod session_dispatch;
#[allow(unused_imports)]
pub(super) use external_runner::{
    apply_external_cli_resume_metadata, external_cli_progress_runner_summary,
    finalize_live_guide_group_turns, resolve_external_cli_delivery_mode,
};
use session_dispatch::*;
#[cfg(test)]
#[path = "event_loop/tests.rs"]
mod tests;
