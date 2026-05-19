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

    /// Returns `true` if this event_id is a duplicate (already seen within the
    /// TTL window). If not a duplicate, records it for future checks.
    pub(super) fn is_duplicate(&mut self, event_id: &str) -> bool {
        self.evict_expired();

        // Check if already seen
        if self.window.iter().any(|(id, _)| id == event_id) {
            return true;
        }

        // Record new event
        if self.window.len() >= self.max_entries {
            self.window.pop_front();
        }
        self.window
            .push_back((event_id.to_string(), Instant::now()));
        false
    }

    pub(super) fn evict_expired(&mut self) {
        let cutoff = Instant::now() - self.ttl;
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
    route_store: Arc<ImRouteStore>,
    provider_store: Arc<ImProviderStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    agent_client: Arc<ImAgentClient>,
    agent_tools: Arc<ImAgentToolRegistry>,
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
        route_store,
        provider_store,
        agent_config_store,
        agent_client,
        agent_tools,
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
    route_store: Arc<ImRouteStore>,
    provider_store: Arc<ImProviderStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    agent_client: Arc<ImAgentClient>,
    agent_tools: Arc<ImAgentToolRegistry>,
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
    target_store: Arc<ImTargetStore>,
    connection_manager: Arc<ImConnectionManager>,
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

    let mut mcp_manager = ImMcpManager::new(&init_config.mcp_servers).await;
    let mcp_tool_count = mcp_manager.list_tools().len();
    if mcp_tool_count > 0 {
        info!(
            provider_id = %provider.id,
            mcp_tools = mcp_tool_count,
            "MCP manager initialized with tools"
        );
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

            let online_msg = build_online_notification_message(&provider);
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
                msg_type: Some("text".to_string()),
                content_preview: Some(online_msg.to_string()),
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

    while let Some(event) = rx.recv().await {
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
        let dedup_key = event
            .source
            .message_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&event.event_id);

        if !dedup_key.is_empty() && dedup.is_duplicate(dedup_key) {
            debug!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                message_id = ?event.source.message_id,
                "dropping duplicate event"
            );
            continue;
        }

        // Feishu bots use owner_open_id as a safety boundary. Weixin ClawBot is
        // an inbound IM channel, so owner_open_id only records the login user
        // and must not filter inbound messages.
        if provider.provider_type == crate::im_gateway::types::ImProviderType::Feishu {
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

        // Store the event in history
        if let Err(e) = event_store.add(event.clone()) {
            error!(error = %e, "failed to store event");
        }

        // Add "OK" reaction to acknowledge receipt
        let mut reaction_added = None;
        if let Some(ref message_id) = event.source.message_id {
            match client.add_reaction(&provider, message_id, "OK").await {
                Ok(true) => {
                    info!(message_id = %message_id, "added OK reaction to message");
                    reaction_added = Some(true);
                }
                Ok(false) => {
                    reaction_added = None;
                }
                Err(e) => {
                    error!(message_id = %message_id, error = %e, "failed to add OK reaction");
                    reaction_added = Some(false);
                }
            }
        }

        // Record inbound message log
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: event.provider_id.clone(),
            direction: MessageDirection::Inbound,
            status: MessageStatus::Success,
            timestamp: now_ms(),
            target_id: None,
            target_name: None,
            message_id: event.source.message_id.clone(),
            msg_type: event.message.as_ref().and_then(|m| m.raw_type.clone()),
            content_preview: event.message.as_ref().map(inbound_message_preview),
            trigger: Some("websocket".to_string()),
            error: None,
            sender_open_id: event.source.user_id.clone(),
            event_id: Some(event.event_id.clone()),
            reaction_added,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store inbound message log");
        }

        // --- Route matching & action execution ---
        let routes = route_store.list();
        let matches = ImEventRouter::match_routes(&event, &routes);
        if matches.is_empty() {
            // No route matched — try default agent chat if agent is enabled
            let agent_config = agent_config_store.load();
            if agent_config.enabled {
                if let Some(ref msg) = event.message {
                    if !msg.text.trim().is_empty() || !msg.images.is_empty() {
                        let session_key =
                            build_session_key(&event.provider_id, event.source.user_id.as_deref());
                        let agent_message = agent_message_text(msg);

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
                                },
                            )
                            .await;
                            continue;
                        }

                        let effective_agent_config =
                            effective_agent_config_for_provider(&agent_config, &provider);
                        if handle_idle_im_command(
                            &agent_message,
                            &session_key,
                            &effective_agent_config,
                            IdleImCommandContext {
                                client: &client,
                                provider: &provider,
                                event: &event,
                                message_log_store: &message_log_store,
                                agent_session_manager: &agent_session_manager,
                            },
                        )
                        .await
                        {
                            continue;
                        }

                        if let Some(runner_id) = effective_agent_config
                            .runner
                            .as_ref()
                            .and_then(|runner| runner.custom_runner_id())
                        {
                            run_external_cli_agent_chat(
                                ExternalCliChatContext {
                                    rx: &mut rx,
                                    client: &client,
                                    provider: &provider,
                                    provider_store: &provider_store,
                                    event: &event,
                                    message_log_store: &message_log_store,
                                    agent_config_store: &agent_config_store,
                                    external_cli_config_store: &external_cli_config_store,
                                    agent_session_manager: &agent_session_manager,
                                    queue_manager: &queue_manager,
                                    progress_registry: &progress_registry,
                                    event_store: &event_store,
                                },
                                ExternalCliChatInput {
                                    message_text: agent_message,
                                    session_key: session_key.clone(),
                                    adapter_override: None,
                                    instructions_override: None,
                                    delivery_override: None,
                                    runner_id_override: Some(runner_id.to_string()),
                                    runner_selected: true,
                                },
                            )
                            .await;
                            continue;
                        }

                        // Session is free — start processing with select! interleaving
                        let images =
                            resolve_event_images(&client, &provider, &event, &msg.images).await;
                        run_agent_chat_with_interleave(
                            &mut rx,
                            &client,
                            &provider,
                            &provider_store,
                            &event,
                            &agent_client,
                            &agent_config_store,
                            &agent_tools,
                            &schedule_store,
                            &scheduler,
                            &target_store,
                            &connection_manager,
                            &agent_session_manager,
                            &queue_manager,
                            &progress_registry,
                            &session_key,
                            &agent_message,
                            images,
                            None,
                            &mut mcp_manager,
                            &message_log_store,
                            &event_store,
                        )
                        .await;
                    }
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
            ImRouteAction::AgentChat {
                system_prompt,
                reply_target: _,
                ..
            } => {
                let raw_message_text = route_match.message_text.as_deref().unwrap_or("");
                let has_images = event
                    .message
                    .as_ref()
                    .is_some_and(|message| !message.images.is_empty());
                if raw_message_text.trim().is_empty() && !has_images {
                    continue;
                }
                let message_text = event
                    .message
                    .as_ref()
                    .map(agent_message_text)
                    .unwrap_or_else(|| raw_message_text.to_string());
                let session_key =
                    build_session_key(&event.provider_id, event.source.user_id.as_deref());

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
                        },
                    )
                    .await;
                    continue;
                }

                let agent_config =
                    effective_agent_config_for_provider(&agent_config_store.load(), &provider);
                if handle_idle_im_command(
                    &message_text,
                    &session_key,
                    &agent_config,
                    IdleImCommandContext {
                        client: &client,
                        provider: &provider,
                        event: &event,
                        message_log_store: &message_log_store,
                        agent_session_manager: &agent_session_manager,
                    },
                )
                .await
                {
                    continue;
                }

                if let Some(runner_id) = agent_config
                    .runner
                    .as_ref()
                    .and_then(|runner| runner.custom_runner_id())
                {
                    run_external_cli_agent_chat(
                        ExternalCliChatContext {
                            rx: &mut rx,
                            client: &client,
                            provider: &provider,
                            provider_store: &provider_store,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_config_store: &agent_config_store,
                            external_cli_config_store: &external_cli_config_store,
                            agent_session_manager: &agent_session_manager,
                            queue_manager: &queue_manager,
                            progress_registry: &progress_registry,
                            event_store: &event_store,
                        },
                        ExternalCliChatInput {
                            message_text,
                            session_key: session_key.clone(),
                            adapter_override: None,
                            instructions_override: None,
                            delivery_override: None,
                            runner_id_override: Some(runner_id.to_string()),
                            runner_selected: true,
                        },
                    )
                    .await;
                    continue;
                }

                let images = match event.message.as_ref() {
                    Some(message) => {
                        resolve_event_images(&client, &provider, &event, &message.images).await
                    }
                    None => Vec::new(),
                };
                run_agent_chat_with_interleave(
                    &mut rx,
                    &client,
                    &provider,
                    &provider_store,
                    &event,
                    &agent_client,
                    &agent_config_store,
                    &agent_tools,
                    &schedule_store,
                    &scheduler,
                    &target_store,
                    &connection_manager,
                    &agent_session_manager,
                    &queue_manager,
                    &progress_registry,
                    &session_key,
                    &message_text,
                    images,
                    system_prompt.as_deref(),
                    &mut mcp_manager,
                    &message_log_store,
                    &event_store,
                )
                .await;
            }
            ImRouteAction::ExternalCliAgentChat {
                adapter,
                instructions,
                delivery_mode,
                ..
            } => {
                let raw_message_text = route_match.message_text.as_deref().unwrap_or("");
                if raw_message_text.trim().is_empty() {
                    continue;
                }
                let message_text = event
                    .message
                    .as_ref()
                    .map(agent_message_text)
                    .unwrap_or_else(|| raw_message_text.to_string());
                let session_key =
                    build_session_key(&event.provider_id, event.source.user_id.as_deref());

                run_external_cli_agent_chat(
                    ExternalCliChatContext {
                        rx: &mut rx,
                        client: &client,
                        provider: &provider,
                        provider_store: &provider_store,
                        event: &event,
                        message_log_store: &message_log_store,
                        agent_config_store: &agent_config_store,
                        external_cli_config_store: &external_cli_config_store,
                        agent_session_manager: &agent_session_manager,
                        queue_manager: &queue_manager,
                        progress_registry: &progress_registry,
                        event_store: &event_store,
                    },
                    ExternalCliChatInput {
                        message_text,
                        session_key: session_key.clone(),
                        adapter_override: adapter.clone(),
                        instructions_override: instructions.clone(),
                        delivery_override: *delivery_mode,
                        runner_id_override: None,
                        runner_selected: false,
                    },
                )
                .await;
            }
        }
    }

    mcp_manager.shutdown().await;

    info!(
        provider_id = %provider.id,
        "event processing loop ended"
    );
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
}

struct ExternalCliChatInput {
    message_text: String,
    session_key: String,
    adapter_override: Option<String>,
    instructions_override: Option<String>,
    delivery_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
    runner_id_override: Option<String>,
    runner_selected: bool,
}

async fn run_external_cli_agent_chat(ctx: ExternalCliChatContext<'_>, input: ExternalCliChatInput) {
    // Intercept /clear and /reset — these should reset the session rather than
    // being forwarded to the external runner (e.g. ChatGPT Web) as a message.
    let trimmed_msg = input.message_text.trim();
    if trimmed_msg == "/clear" || trimmed_msg == "/reset" {
        // Clear agent session history
        if let Some(mut session) = ctx
            .agent_session_manager
            .try_take_session(&input.session_key)
        {
            session.clear();
            ctx.agent_session_manager.return_session(session);
        }
        // Clear ChatGPT Web conversation mapping so the next message starts
        // a new conversation instead of appending to the old one.
        crate::im_gateway::chatgpt_web::clear_session_conversation(&input.session_key).await;
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "会话已重置，下一条消息将开始新的对话。",
            ctx.message_log_store,
        )
        .await;
        return;
    }

    let config = ctx.external_cli_config_store.load();
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(&ctx.provider.id),
        input.runner_id_override.as_deref(),
    );
    if !effective.settings.enabled && !input.runner_selected {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "Runner is not enabled for this IM channel.",
            ctx.message_log_store,
        )
        .await;
        return;
    }

    let mut settings = effective.settings;
    if let Some(adapter) = input
        .adapter_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.adapter = adapter.to_string();
    }
    if let Some(instructions) = input
        .instructions_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.instructions = match settings.instructions.take() {
            Some(existing) if !existing.trim().is_empty() => {
                Some(format!("{}\n\n{}", existing.trim(), instructions))
            }
            _ => Some(instructions.to_string()),
        };
    }

    let delivery_mode = input.delivery_override.unwrap_or(settings.delivery_mode);

    if matches!(
        delivery_mode,
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
    ) {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "已开始处理 Runner 任务。",
            ctx.message_log_store,
        )
        .await;
    }

    let Some(mut session) = ctx.agent_session_manager.try_take_session_with_work_dir(
        &input.session_key,
        effective_agent_config_for_provider(&ctx.agent_config_store.load(), ctx.provider).work_dir,
    ) else {
        handle_busy_message(
            &input.message_text,
            &input.session_key,
            BusyMessageContext {
                queue_manager: ctx.queue_manager,
                client: ctx.client,
                provider: ctx.provider,
                event: ctx.event,
                message_log_store: ctx.message_log_store,
                agent_session_manager: ctx.agent_session_manager,
                progress_registry: ctx.progress_registry,
            },
        )
        .await;
        return;
    };

    let guide_channel = ctx
        .queue_manager
        .get_or_create_guide_channel(&input.session_key);
    let mut current_message = input.message_text;
    let mut recorder = session.recorder.take();

    loop {
        if matches!(
            delivery_mode,
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
        ) {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                "已开始处理 Runner 任务。",
                ctx.message_log_store,
            )
            .await;
        }

        let mut request = crate::im_gateway::external_cli::run_request_from_settings(
            current_message.clone(),
            Some(ctx.provider.id.clone()),
            Some(input.session_key.clone()),
            &settings,
        );
        if request.work_dir.is_none() {
            request.work_dir =
                effective_agent_work_dir_for_provider(&ctx.agent_config_store.load(), ctx.provider);
        }
        // Graceful fallback: if the configured work_dir doesn't exist on disk,
        // clear it so the runner uses a default directory instead of failing.
        if let Some(ref work_dir) = request.work_dir {
            if !work_dir.exists() {
                tracing::warn!(
                    work_dir = %work_dir.display(),
                    provider_id = %ctx.provider.id,
                    "agent work_dir does not exist, falling back to default"
                );
                request.work_dir = None;
            }
        }
        if request.allow_work_dirs.is_empty() {
            if let Some(work_dir) = request.work_dir.as_ref() {
                request.allow_work_dirs = vec![work_dir.display().to_string()];
            }
        }
        ensure_external_cli_session_recorder(
            &mut session,
            &mut recorder,
            &input.session_key,
            ctx.provider,
            &effective.runner_id,
            &request,
        );
        record_external_cli_input(
            &mut session,
            &mut recorder,
            &input.session_key,
            &effective.runner_id,
            &request,
        );
        let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
            crate::im_gateway::external_cli::default_runs_root(),
        );
        let run_future = runtime.run(request.clone());
        tokio::pin!(run_future);
        let result = loop {
            tokio::select! {
                result = &mut run_future => break result,
                Some(next_event) = ctx.rx.recv() => {
                    maybe_stop_external_cli_for_event(&next_event, &input.session_key).await;
                    handle_concurrent_event_during_chat(
                        &next_event,
                        ctx.provider,
                        &input.session_key,
                        ctx.queue_manager,
                        ctx.client,
                        ctx.message_log_store,
                        ctx.agent_session_manager,
                        ctx.progress_registry,
                        ctx.agent_config_store,
                        ctx.provider_store,
                        ctx.event_store,
                    ).await;
                }
            }
        };
        match result {
            Ok(result) => {
                record_external_cli_result(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &result,
                );
                if !matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm
                ) {
                    // Send individual response messages separately when there
                    // are multiple (e.g. ChatGPT thinking + answer).
                    let responses_to_send: Vec<String> = if result.responses.len() > 1 {
                        result
                            .responses
                            .iter()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    } else {
                        vec![result.response.trim().to_string()]
                    };
                    for (idx, reply_text) in responses_to_send.iter().enumerate() {
                        let reply = if matches!(
                            delivery_mode,
                            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                        ) && idx == responses_to_send.len() - 1
                        {
                            // Only append run_id to the last message
                            format!("{}\n\n_run: `{}`_", reply_text, result.run_id)
                        } else {
                            reply_text.clone()
                        };
                        send_agent_reply(
                            ctx.client,
                            ctx.provider,
                            ctx.event,
                            &reply,
                            ctx.message_log_store,
                        )
                        .await;
                    }
                }
            }
            Err(error) => {
                // Extract diagnostic screenshot path if present.
                let (clean_error, screenshot_path) = extract_diagnostic_screenshot_path(&error);
                let reply = format!("Runner failed: {}", truncate_str(&clean_error, 300));
                record_external_cli_failure(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &request,
                    &error,
                    &reply,
                );
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &reply,
                    ctx.message_log_store,
                )
                .await;
                // Send diagnostic screenshot via IM if available.
                if let Some(path) = screenshot_path {
                    if let Some(target) = build_agent_reply_target(
                        ctx.provider,
                        ctx.event,
                        "__diag_screenshot__",
                        "Diagnostic Screenshot",
                        "image",
                    ) {
                        let images = vec![AgentReplyLocalImage {
                            alt: "diagnostic screenshot".to_string(),
                            path,
                        }];
                        send_agent_reply_images(
                            ctx.client,
                            ctx.provider,
                            ctx.event,
                            &target,
                            &images,
                            ctx.message_log_store,
                        )
                        .await;
                    }
                }
            }
        };

        let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
        if let Some(unconsumed) = bifrost_agent::session::combine_guide_messages(unconsumed_guides)
        {
            if !unconsumed.trim().is_empty() {
                let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
            }
        }
        match ctx.queue_manager.pop_queue(&input.session_key) {
            Some(next_message) => {
                if matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                ) {
                    let remaining = ctx.queue_manager.queue_status(&input.session_key).len();
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        ctx.event,
                        &format!("开始处理排队消息，当前剩余 {remaining} 条。"),
                        ctx.message_log_store,
                    )
                    .await;
                }
                current_message = next_message;
            }
            None => break,
        };
    }
    if recorder.is_some() && !session.memory_cleared {
        session.recorder = recorder;
    }
    ctx.queue_manager.clear_session(&input.session_key);
    ctx.agent_session_manager.return_session(session);
}

fn ensure_external_cli_session_recorder(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    provider: &ImProviderConfig,
    runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let adapter_label = external_cli_adapter_label(&request.adapter);
    if session.source == "unknown" {
        session.source = request.adapter.clone();
    }
    if session.title.is_none() {
        session.title = Some(format!(
            "{}: {}",
            adapter_label,
            truncate_str(request.message.trim(), 48)
        ));
    }
    if session.work_dir.is_none() {
        session.work_dir = request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string());
    }

    if recorder.is_none() {
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut rec = ConversationRecorder::new(&data_dir, session_key);
        if let Err(error) = rec.record_session_start(
            session_key,
            serde_json::json!({
                "source": request.adapter,
                "runtime": request.runtime,
                "adapter": request.adapter,
                "runner_id": runner_id,
                "provider_id": provider.id,
                "provider_type": format!("{:?}", provider.provider_type).to_lowercase(),
                "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
            }),
        ) {
            warn!(error = %error, "failed to record external cli session start");
        }
        if let Some(title) = session.title.as_deref() {
            if let Err(error) = rec.record_title_updated(session_key, title) {
                warn!(error = %error, "failed to record external cli session title");
            }
        }
        *recorder = Some(rec);
    }
}

fn record_external_cli_input(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    append_session_message(session, bifrost_agent::ChatMessage::user(&request.message));
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) = rec.record_user_message(session_key, &request.message) {
            warn!(error = %error, "failed to record external cli user message");
        }
        let arguments = serde_json::json!({
            "runtime": request.runtime,
            "adapter": request.adapter,
            "runner_id": runner_id,
            "operation": request.operation,
            "provider_id": request.provider_id,
            "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
            "message_length": request.message.chars().count(),
        })
        .to_string();
        if let Err(error) =
            rec.record_tool_call_with_id(session_key, &request.adapter, &arguments, None)
        {
            warn!(error = %error, "failed to record external cli tool call");
        }
    }
}

fn record_external_cli_result(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    append_session_message(
        session,
        bifrost_agent::ChatMessage::assistant(&result.response),
    );
    if let Some(rec) = recorder.as_mut() {
        let tool_result = serde_json::json!({
            "run_id": result.run_id,
            "runtime": result.runtime,
            "adapter": result.adapter,
            "status": result.status,
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "artifacts": result.artifacts,
            "event_types": result.events.iter().map(|event| format!("{:?}", event.event_type)).collect::<Vec<_>>(),
            "response_preview": truncate_str(&result.response, 500),
        })
        .to_string();
        let success = matches!(
            result.status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
        );
        if let Err(error) = rec.record_tool_result_with_call_id(
            session_key,
            &result.adapter,
            &tool_result,
            success,
            Some(&result.run_id),
        ) {
            warn!(error = %error, "failed to record external cli tool result");
        }
        if let Err(error) = rec.record_assistant_message(session_key, &result.response) {
            warn!(error = %error, "failed to record external cli assistant message");
        }
    }
}

fn record_external_cli_failure(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    error: &str,
    reply: &str,
) {
    append_session_message(session, bifrost_agent::ChatMessage::assistant(reply));
    if let Some(rec) = recorder.as_mut() {
        let tool_result = serde_json::json!({
            "runtime": request.runtime,
            "adapter": request.adapter,
            "operation": request.operation,
            "provider_id": request.provider_id,
            "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
            "error": error,
        })
        .to_string();
        if let Err(record_error) = rec.record_tool_result_with_call_id(
            session_key,
            &request.adapter,
            &tool_result,
            false,
            None,
        ) {
            warn!(error = %record_error, "failed to record external cli failure result");
        }
        if let Err(record_error) = rec.record_assistant_message(session_key, reply) {
            warn!(error = %record_error, "failed to record external cli failure message");
        }
    }
}

fn append_session_message(
    session: &mut bifrost_agent::session::AgentSession,
    message: bifrost_agent::ChatMessage,
) {
    session.history.push(message);
    session.last_active_at = now_ms() / 1000;
    session.history_version = session.history_version.saturating_add(1);
}

fn external_cli_adapter_label(adapter: &str) -> &'static str {
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        "ChatGPT Web"
    } else {
        "Runner"
    }
}

async fn maybe_stop_external_cli_for_event(event: &ImEvent, active_session_key: &str) {
    let Some(message) = event.message.as_ref() else {
        return;
    };
    let msg_text = agent_message_text(message);
    if msg_text.trim() != "/stop" {
        return;
    }
    let session_key = build_session_key(&event.provider_id, event.source.user_id.as_deref());
    if session_key != active_session_key {
        return;
    }
    if let Err(error) = crate::im_gateway::external_cli::request_session_stop(
        crate::im_gateway::external_cli::default_runs_root(),
        active_session_key,
    )
    .await
    {
        debug!(session_key = %active_session_key, error = %error, "external cli session stop was not applied");
    }
}
