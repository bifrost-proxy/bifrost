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
                        content: event
                            .message
                            .as_ref()
                            .map(|message| message.text.clone())
                            .filter(|text| !text.trim().is_empty()),
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
            content: event
                .message
                .as_ref()
                .map(|message| message.text.clone())
                .filter(|text| !text.trim().is_empty()),
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
                        let agent_message = agent_message_text_with_reference(
                            msg,
                            &event.provider_id,
                            event.source.user_id.as_deref(),
                            event.source.message_id.as_deref(),
                            &message_log_store,
                        );
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
                                    default_mode: busy_default_mode,
                                    status_context: status_context_from_agent_config(
                                        &effective_agent_config,
                                    ),
                                    default_work_dir: Some(
                                        effective_agent_config
                                            .resolve_work_dir()
                                            .display()
                                            .to_string(),
                                    ),
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
                                external_cli_config_store: &external_cli_config_store,
                                event: &event,
                                message_log_store: &message_log_store,
                                agent_session_manager: &agent_session_manager,
                            },
                        )
                        .await
                        {
                            continue;
                        }

                        let runner_id = effective_agent_config
                            .runner
                            .as_ref()
                            .and_then(|runner| runner.custom_runner_id());
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
                                images: external_cli_images_from_chat_images(
                                    resolve_event_images(&client, &provider, &event, &msg.images)
                                        .await,
                                ),
                                session_key: session_key.clone(),
                                adapter_override: None,
                                instructions_override: None,
                                delivery_override: None,
                                runner_id_override: runner_id.map(ToString::to_string),
                                runner_selected: runner_id.is_some(),
                            },
                        )
                        .await;
                        continue;
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
            ImRouteAction::AgentChat { .. } => {
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
                    .map(|message| {
                        agent_message_text_with_reference(
                            message,
                            &event.provider_id,
                            event.source.user_id.as_deref(),
                            event.source.message_id.as_deref(),
                            &message_log_store,
                        )
                    })
                    .unwrap_or_else(|| raw_message_text.to_string());
                let session_key =
                    build_session_key(&event.provider_id, event.source.user_id.as_deref());
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
                            default_mode: busy_default_mode,
                            status_context: status_context_from_agent_config(&agent_config),
                            default_work_dir: Some(
                                agent_config.resolve_work_dir().display().to_string(),
                            ),
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
                        external_cli_config_store: &external_cli_config_store,
                        event: &event,
                        message_log_store: &message_log_store,
                        agent_session_manager: &agent_session_manager,
                    },
                )
                .await
                {
                    continue;
                }

                let runner_id = agent_config
                    .runner
                    .as_ref()
                    .and_then(|runner| runner.custom_runner_id());
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
                        images: match event.message.as_ref() {
                            Some(message) => external_cli_images_from_chat_images(
                                resolve_event_images(&client, &provider, &event, &message.images)
                                    .await,
                            ),
                            None => Vec::new(),
                        },
                        session_key: session_key.clone(),
                        adapter_override: None,
                        instructions_override: None,
                        delivery_override: None,
                        runner_id_override: runner_id.map(ToString::to_string),
                        runner_selected: runner_id.is_some(),
                    },
                )
                .await;
                continue;
            }
            ImRouteAction::ExternalCliAgentChat {
                adapter,
                instructions,
                delivery_mode,
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

                if parse_im_runner_command(&message_text).is_some() {
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
                            external_cli_config_store: &external_cli_config_store,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_session_manager: &agent_session_manager,
                        },
                    )
                    .await
                    {
                        continue;
                    }
                }

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
                        images: match event.message.as_ref() {
                            Some(message) => external_cli_images_from_chat_images(
                                resolve_event_images(&client, &provider, &event, &message.images)
                                    .await,
                            ),
                            None => Vec::new(),
                        },
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
    images: Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
    session_key: String,
    adapter_override: Option<String>,
    instructions_override: Option<String>,
    delivery_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
    runner_id_override: Option<String>,
    runner_selected: bool,
}

struct AbortTaskOnDrop(tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn external_cli_images_from_chat_images(
    images: Vec<bifrost_agent::ChatImageInput>,
) -> Vec<crate::im_gateway::external_cli::ExternalCliImageInput> {
    images
        .into_iter()
        .map(
            |image| crate::im_gateway::external_cli::ExternalCliImageInput {
                mime_type: image.mime_type,
                data: image.data,
                name: None,
            },
        )
        .collect()
}

pub(super) fn resolve_external_cli_delivery_mode(
    provider: &ImProviderConfig,
    settings: &crate::im_gateway::external_cli::ExternalCliAgentSettings,
    sources: &std::collections::BTreeMap<String, String>,
    input_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
) -> crate::im_gateway::external_cli::ExternalCliDeliveryMode {
    if let Some(delivery_mode) = input_override {
        return delivery_mode;
    }
    if provider.provider_type == ImProviderType::Feishu
        && is_im_progress_card_external_adapter(&settings.adapter)
        && sources.get("deliveryMode").map(String::as_str) != Some("channel")
    {
        return crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    }
    settings.delivery_mode
}

fn is_im_progress_card_external_adapter(adapter: &str) -> bool {
    matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    )
}

async fn run_external_cli_agent_chat(ctx: ExternalCliChatContext<'_>, input: ExternalCliChatInput) {
    let config = ctx.external_cli_config_store.load();
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(&ctx.provider.id),
        input.runner_id_override.as_deref(),
    );
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

    // Intercept /clear and /reset after resolving the effective runner so the
    // reset only clears the current adapter/runner rather than every agent that
    // happens to share the same IM session key.
    let trimmed_msg = input.message_text.trim();
    if trimmed_msg == "/help" {
        let response = build_im_startup_help_for_runner(&ImHelpRunnerKind::External {
            adapter: settings.adapter.clone(),
        });
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            &response,
            ctx.message_log_store,
        )
        .await;
        return;
    }

    if trimmed_msg == "/clear" || trimmed_msg == "/reset" {
        let _ = request_agent_stop(ctx.agent_session_manager, &input.session_key).await;
        if let Some(mut session) = ctx
            .agent_session_manager
            .try_take_session(&input.session_key)
        {
            session.clear();
            ctx.agent_session_manager.return_session(session);
        } else {
            ctx.agent_session_manager.clear_session(&input.session_key);
        }
        ctx.queue_manager.clear_session(&input.session_key);
        if settings.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
            crate::im_gateway::chatgpt_web::clear_session_conversation(&input.session_key).await;
        }
        clear_persisted_agent_session_state(
            &input.session_key,
            Some(&settings.adapter),
            Some(&effective.runner_id),
        );
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

    if !settings.enabled && !input.runner_selected {
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

    let persisted_state = crate::im_gateway::session_state::load_session_state(
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    let delivery_mode = resolve_external_cli_delivery_mode(
        ctx.provider,
        &settings,
        &effective.sources,
        input.delivery_override,
    );
    let mut resolved_model_config =
        crate::im_gateway::external_cli::resolve_external_cli_model_config(
            &settings.adapter,
            &settings.adapter_config,
        );
    crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_model_config(
        &settings.adapter,
        persisted_state.as_ref(),
        &mut resolved_model_config,
    );
    let mut status_context =
        status_context_from_external_runner(&effective.runner_id, &settings.adapter);
    if let Some(model) = resolved_model_config.model.clone() {
        status_context.model = Some(model);
    }
    status_context.model_provider = resolved_model_config
        .model_provider
        .clone()
        .or_else(|| resolved_model_config.model_source.clone());
    status_context.model_reasoning_effort = resolved_model_config.reasoning_effort.clone();
    status_context.model_reasoning_summary = resolved_model_config.reasoning_summary.clone();

    let provider_agent_config =
        effective_agent_config_for_provider(&ctx.agent_config_store.load(), ctx.provider);
    let Some(mut session) = ctx
        .agent_session_manager
        .try_take_session_with_work_dir(&input.session_key, provider_agent_config.work_dir.clone())
    else {
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
                external_cli_config_store: ctx.external_cli_config_store,
                agent_config: &provider_agent_config,
                default_mode: busy_default_mode_for_external_adapter(&settings.adapter),
                status_context,
                default_work_dir: Some(
                    provider_agent_config
                        .resolve_work_dir()
                        .display()
                        .to_string(),
                ),
            },
        )
        .await;
        return;
    };

    let guide_channel = ctx
        .queue_manager
        .get_or_create_guide_channel(&input.session_key);
    restore_session_from_persisted_history(
        &mut session,
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
        provider_agent_config
            .history
            .as_ref()
            .and_then(|h| h.max_bytes),
    );
    let mut current_message = input.message_text;
    let mut current_images = input.images;
    let mut recorder = session.recorder.take();
    let mut runner_metadata = persisted_state
        .as_ref()
        .map(crate::im_gateway::session_state::metadata_from_state)
        .unwrap_or_default();

    loop {
        if let Some(command) = parse_im_cwd_command(&current_message) {
            let reply = match command {
                Ok(path) => apply_im_cwd_switch_to_session(
                    ctx.provider_store,
                    &ctx.provider.id,
                    &input.session_key,
                    &mut session,
                    &path,
                ),
                Err(reason) => format_im_cwd_error(&reason),
            };
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &reply,
                ctx.message_log_store,
            )
            .await;
            let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
            if let Some(unconsumed) =
                bifrost_agent::session::combine_guide_messages(unconsumed_guides)
            {
                if !unconsumed.trim().is_empty() {
                    let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
                }
            }
            match ctx.queue_manager.pop_queue_item(&input.session_key) {
                Some(next_item) => {
                    current_message = next_item.message;
                    current_images = external_cli_images_from_chat_images(next_item.images);
                    continue;
                }
                None => break,
            }
        }

        let mut request = crate::im_gateway::external_cli::run_request_from_settings(
            current_message.clone(),
            Some(ctx.provider.id.clone()),
            Some(input.session_key.clone()),
            &settings,
        );
        crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_run_request(
            &mut request,
            persisted_state.as_ref(),
        );
        request.images = std::mem::take(&mut current_images);
        apply_external_cli_resume_metadata(&mut request, &runner_metadata);
        consume_imported_contexts_for_im_external_runner(&mut request, &effective.runner_id);
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
        session.remember_runner_model_config(
            resolved_model_config.model.clone(),
            resolved_model_config
                .model_provider
                .clone()
                .or_else(|| resolved_model_config.model_source.clone()),
            resolved_model_config.reasoning_effort.clone(),
            resolved_model_config.reasoning_summary.clone(),
        );
        ensure_external_cli_session_recorder(
            &mut session,
            &mut recorder,
            &input.session_key,
            ctx.provider,
            &effective.runner_id,
            &request,
        );
        apply_external_cli_session_attachment_base_dir(&mut request, recorder.as_ref());
        record_external_cli_input(
            &mut session,
            &mut recorder,
            &input.session_key,
            &effective.runner_id,
            &request,
        );
        emit_external_cli_timeline_changed(
            ctx.agent_session_manager,
            recorder.as_ref(),
            &input.session_key,
            "im_turn_started",
        );
        let mut progress_enabled = false;
        let mut progress_tx_for_finish = None;
        let mut progress_task = None;
        if matches!(
            delivery_mode,
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
        ) {
            if let (Some(progress_target), Some(feishu)) = (
                build_agent_reply_target(
                    ctx.provider,
                    ctx.event,
                    "__agent_progress__",
                    "Agent Progress",
                    "interactive",
                ),
                ctx.client.feishu(),
            ) {
                let progress_result = if ctx
                    .progress_registry
                    .rollover_existing(&input.session_key, &current_message)
                    .await
                {
                    Ok(())
                } else {
                    ctx.progress_registry
                        .start_feishu(
                            &input.session_key,
                            feishu,
                            ctx.provider.clone(),
                            progress_target,
                            &current_message,
                        )
                        .await
                        .map(|_| ())
                };
                match progress_result {
                    Ok(_) => {
                        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
                            bifrost_agent::AgentTurnProgressEvent,
                        >();
                        progress_tx_for_finish = Some(progress_tx.clone());
                        let progress_registry = Arc::clone(ctx.progress_registry);
                        let session_key_for_progress = input.session_key.clone();
                        progress_task = Some(tokio::spawn(async move {
                            super::agent_chat::run_progress_event_coalescer(
                                progress_registry,
                                session_key_for_progress,
                                &mut progress_rx,
                            )
                            .await;
                        }));
                        progress_enabled = true;
                        let runner_summary = external_cli_progress_runner_summary(
                            &effective.runner_id,
                            &settings.adapter,
                            &request,
                            None,
                        );
                        let _ = ctx
                            .progress_registry
                            .update_runner_summary(&input.session_key, runner_summary)
                            .await;
                    }
                    Err(error) => {
                        warn!(
                            session_key = %input.session_key,
                            error = %error,
                            "failed to start external runner progress card; final reply will be sent when the run finishes"
                        );
                    }
                }
            }
        }
        let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
            crate::im_gateway::external_cli::default_runs_root(),
        );
        let (external_progress_tx, mut external_progress_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let request_for_progress = request.clone();
        // Keep the runner control loop independently polled while this task
        // handles a default Guide message (or a legacy inbound /g). Awaiting the
        // guide acknowledgement inline otherwise stalls `run_with_progress`,
        // which is also responsible for forwarding that guide to the worker.
        let request_for_run = request.clone();
        let mut run_task = tokio::spawn(async move {
            runtime
                .run_with_progress(request_for_run, Some(external_progress_tx))
                .await
        });
        let _run_task_guard = AbortTaskOnDrop(run_task.abort_handle());
        let result = loop {
            tokio::select! {
                result = &mut run_task => break result
                    .map_err(|error| format!("external runner task failed: {error}"))
                    .and_then(|result| result),
                Some(progress_event) = external_progress_rx.recv() => {
                    if let Some(recorder) = recorder.as_mut() {
                        if let Some(end_index) = super::chat_gateway::record_external_cli_progress_event_to_timeline(
                            recorder,
                            &input.session_key,
                            "im",
                            &effective.runner_id,
                            &settings.adapter,
                            &progress_event,
                        ) {
                            ctx.agent_session_manager.emit_timeline_changed(
                                &input.session_key,
                                &recorder.file_path().display().to_string(),
                                Some(end_index),
                                "im_progress",
                            );
                        }
                    }
                    if progress_enabled {
                        if let (Some(progress_tx), Some(agent_event)) = (
                            progress_tx_for_finish.as_ref(),
                            crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                                &input.session_key,
                                &settings.adapter,
                                crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                                    Some(&effective.runner_id),
                                    resolved_model_config.model.as_deref(),
                                    resolved_model_config
                                        .model_provider
                                        .as_deref()
                                        .or(resolved_model_config.model_source.as_deref()),
                                    resolved_model_config.reasoning_effort.as_deref(),
                                    resolved_model_config.reasoning_summary.as_deref(),
                                    request_for_progress.work_dir.as_deref(),
                                ),
                                &progress_event,
                            ),
                        ) {
                            let _ = progress_tx.send(agent_event);
                        }
                    }
                }
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
                        ctx.external_cli_config_store,
                        busy_default_mode_for_external_adapter(&settings.adapter),
                    ).await;
                }
            }
        };
        while let Ok(progress_event) = external_progress_rx.try_recv() {
            if let Some(recorder) = recorder.as_mut() {
                if let Some(end_index) =
                    super::chat_gateway::record_external_cli_progress_event_to_timeline(
                        recorder,
                        &input.session_key,
                        "im",
                        &effective.runner_id,
                        &settings.adapter,
                        &progress_event,
                    )
                {
                    ctx.agent_session_manager.emit_timeline_changed(
                        &input.session_key,
                        &recorder.file_path().display().to_string(),
                        Some(end_index),
                        "im_progress",
                    );
                }
            }
            if progress_enabled {
                if let (Some(progress_tx), Some(agent_event)) = (
                    progress_tx_for_finish.as_ref(),
                    crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                        &input.session_key,
                        &settings.adapter,
                        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                            Some(&effective.runner_id),
                            resolved_model_config.model.as_deref(),
                            resolved_model_config
                                .model_provider
                                .as_deref()
                                .or(resolved_model_config.model_source.as_deref()),
                            resolved_model_config.reasoning_effort.as_deref(),
                            resolved_model_config.reasoning_summary.as_deref(),
                            request_for_progress.work_dir.as_deref(),
                        ),
                        &progress_event,
                    ),
                ) {
                    let _ = progress_tx.send(agent_event);
                }
            }
        }
        match result {
            Ok(mut result) => {
                let run_succeeded = matches!(
                    result.status,
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
                );
                if !run_succeeded {
                    let failure_reply = external_cli_non_success_reply(&result);
                    result.response = failure_reply.clone();
                    result.responses = vec![failure_reply];
                }
                remember_external_cli_result_metadata(&mut runner_metadata, &result.metadata);
                record_external_cli_result(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &result,
                );
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_finished",
                );
                remember_session_state_values(
                    &input.session_key,
                    &settings.adapter,
                    Some(&effective.runner_id),
                    session.external_conversation_id.clone(),
                    session.external_thread_id.clone(),
                    recorder
                        .as_ref()
                        .map(|recorder| recorder.file_path().display().to_string()),
                    session.work_dir.clone(),
                );
                if progress_enabled {
                    let runner_summary = external_cli_progress_runner_summary(
                        &effective.runner_id,
                        &settings.adapter,
                        &request_for_progress,
                        Some(&result.metadata),
                    );
                    let _ = ctx
                        .progress_registry
                        .update_runner_summary(&input.session_key, runner_summary)
                        .await;
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let event = if run_succeeded {
                            bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                                content: result.response.clone(),
                            }
                        } else {
                            bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: result.response.clone(),
                            }
                        };
                        let _ = progress_tx.send(event);
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
                    }
                    let _ = ctx
                        .progress_registry
                        .finish(
                            &input.session_key,
                            Some(result.response.clone()),
                            !run_succeeded,
                        )
                        .await;
                } else if !matches!(
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
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_failed",
                );
                if progress_enabled {
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let _ =
                            progress_tx.send(bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: reply.clone(),
                            });
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
                    }
                    let _ = ctx
                        .progress_registry
                        .finish(&input.session_key, Some(reply.clone()), true)
                        .await;
                } else {
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        ctx.event,
                        &reply,
                        ctx.message_log_store,
                    )
                    .await;
                }
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
        match ctx.queue_manager.pop_queue_item(&input.session_key) {
            Some(next_item) => {
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
                current_message = next_item.message;
                current_images = external_cli_images_from_chat_images(next_item.images);
            }
            None => break,
        };
    }
    if recorder.is_some() && !session.history_cleared {
        session.recorder = recorder;
    }
    remember_session_state_from_agent_session(
        &session,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    ctx.queue_manager.clear_session(&input.session_key);
    ctx.agent_session_manager.return_session(session);
}

fn consume_imported_contexts_for_im_external_runner(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let contexts = match crate::im_gateway::session_state::take_imported_contexts(
        session_key,
        &request.adapter,
        Some(runner_id),
    ) {
        Ok(contexts) => contexts,
        Err(error) => {
            warn!(
                session_key = %session_key,
                adapter = %request.adapter,
                runner_id = %runner_id,
                error = %error,
                "failed to consume imported contexts for IM external runner"
            );
            Vec::new()
        }
    };
    let Some(rendered) = crate::im_gateway::session_state::render_imported_contexts(&contexts)
    else {
        return;
    };
    request.instructions = Some(match request.instructions.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{}\n\n{}", existing.trim(), rendered.trim())
        }
        _ => rendered,
    });
}

fn external_cli_non_success_reply(
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) -> String {
    match result.status {
        crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => format!(
            "Runner failed: external CLI timed out after {} seconds.\n\n_run: `{}`_",
            std::cmp::max(1, result.duration_ms / 1000),
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => format!(
            "Runner stopped before completion.\n\n_run: `{}`_",
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Failed => {
            let detail = if result.response.trim().is_empty() {
                match result.exit_code {
                    Some(code) => format!("exit_code={code}"),
                    None => "external CLI exited unsuccessfully".to_string(),
                }
            } else {
                truncate_str(&result.response, 260)
            };
            format!("Runner failed: {detail}\n\n_run: `{}`_", result.run_id)
        }
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => result.response.clone(),
    }
}

pub(super) fn apply_external_cli_resume_metadata(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: &std::collections::BTreeMap<String, String>,
) {
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        if request
            .params
            .get("conversationId")
            .or_else(|| request.params.get("conversation_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            return;
        }
        let Some(conversation_id) = metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        if !request.params.is_object() {
            request.params = serde_json::json!({});
        }
        if let Some(params) = request.params.as_object_mut() {
            params.insert(
                "conversationId".to_string(),
                serde_json::Value::String(conversation_id.to_string()),
            );
        }
        return;
    }
    if !matches!(
        request.adapter.as_str(),
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        return;
    }
    if request
        .params
        .get("threadId")
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return;
    }
    let Some(thread_id) = metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.insert(
            "threadId".to_string(),
            serde_json::Value::String(thread_id.to_string()),
        );
    }
}

pub(super) fn remember_external_cli_result_metadata(
    metadata: &mut std::collections::BTreeMap<String, String>,
    result_metadata: &std::collections::BTreeMap<String, String>,
) {
    if let Some(thread_id) = result_metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("threadId".to_string(), thread_id.to_string());
    }
    if let Some(conversation_id) = result_metadata
        .get("conversationId")
        .or_else(|| result_metadata.get("conversation_id"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("conversationId".to_string(), conversation_id.to_string());
    }
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
    session.mark_external_runner_runtime(runner_id, &request.adapter);
    sync_external_cli_active_status(session);
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
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_run_state(session_key, "running", Some("im"), Some(runner_id))
        {
            warn!(error = %error, "failed to record external cli running state");
        }
    }
}

fn apply_external_cli_session_attachment_base_dir(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    recorder: Option<&ConversationRecorder>,
) {
    let Some(recorder) = recorder else {
        return;
    };
    let Some(session_dir) = recorder.file_path().parent() else {
        return;
    };
    let Some(session_stem) = recorder.file_path().file_stem() else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.remove("attachment_base_dir");
        params.insert(
            "attachmentBaseDir".to_string(),
            serde_json::Value::String(
                session_dir
                    .join("attachments")
                    .join(session_stem)
                    .display()
                    .to_string(),
            ),
        );
        params.insert(
            "historyPath".to_string(),
            serde_json::Value::String(recorder.file_path().display().to_string()),
        );
    }
}

fn external_cli_request_chat_images(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) -> Vec<bifrost_agent::ChatImageInput> {
    request
        .images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .map(|image| bifrost_agent::ChatImageInput {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        })
        .collect()
}

fn record_external_cli_input(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    _runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let images = external_cli_request_chat_images(request);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::user_with_images(&request.message, &images),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_user_message_with_images(session_key, &request.message, &images)
        {
            warn!(error = %error, "failed to record external cli user message");
        }
    }
}

fn emit_external_cli_timeline_changed(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    recorder: Option<&ConversationRecorder>,
    session_key: &str,
    reason: &str,
) {
    let Some(recorder) = recorder else {
        return;
    };
    agent_session_manager.emit_timeline_changed(
        session_key,
        &recorder.file_path().display().to_string(),
        recorder.event_count(),
        reason,
    );
}

fn record_external_cli_result(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    session.remember_external_conversation_ref(
        result
            .metadata
            .get("conversationId")
            .or_else(|| result.metadata.get("conversation_id"))
            .cloned(),
        result
            .metadata
            .get("threadId")
            .or_else(|| result.metadata.get("thread_id"))
            .cloned(),
    );
    sync_external_cli_active_status(session);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::assistant(&result.response),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        let run_state = if matches!(
            result.status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
        ) {
            "completed"
        } else {
            "failed"
        };
        if let Err(error) =
            rec.record_run_state(session_key, run_state, Some("im"), Some(&result.adapter))
        {
            warn!(error = %error, "failed to record external cli run state");
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
    _error: &str,
    reply: &str,
) {
    append_session_message(session, bifrost_agent::ChatMessage::assistant(reply));
    if let Some(rec) = recorder.as_mut() {
        if let Err(record_error) =
            rec.record_run_state(session_key, "failed", Some("im"), Some(&request.adapter))
        {
            warn!(error = %record_error, "failed to record external cli failure state");
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

fn sync_external_cli_active_status(session: &bifrost_agent::session::AgentSession) {
    let Some(handle) = session.active_turn_status.as_ref() else {
        return;
    };
    let Ok(mut status) = handle.lock() else {
        return;
    };
    status.agent_type = session.agent_type.clone();
    status.runner_type = session.runner_type.clone();
    status.runner_id = session.runner_id.clone();
    status.model = session.model.clone();
    status.model_provider = session.model_provider.clone();
    status.model_reasoning_effort = session.model_reasoning_effort.clone();
    status.model_reasoning_summary = session.model_reasoning_summary.clone();
    status.external_conversation_id = session.external_conversation_id.clone();
    status.external_thread_id = session.external_thread_id.clone();
    status.user_turn_count = session.user_turn_count();
    status.message_count = session.history.len();
    status.work_dir = session.work_dir.clone();
    status.history_version = session.history_version;
    status.compaction_count = session.compaction_count;
    status.total_tokens_used = session.total_tokens_used;
    status.estimated_context_tokens = session.effective_token_count();
}

fn external_cli_adapter_label(adapter: &str) -> &'static str {
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        "ChatGPT Web"
    } else {
        "Runner"
    }
}

fn external_cli_progress_runner_summary(
    runner_id: &str,
    adapter: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: Option<&std::collections::BTreeMap<String, String>>,
) -> crate::im_gateway::progress_card::ProgressRunnerSummary {
    let resolved_model_config = crate::im_gateway::external_cli::resolve_external_cli_model_config(
        &request.adapter,
        &request.adapter_config,
    );
    let configured_model = resolved_model_config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let metadata_model = metadata
        .and_then(|metadata| metadata.get("model"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = configured_model.or(metadata_model).map(str::to_string);
    let model_source = if configured_model.is_some() {
        resolved_model_config
            .model_provider
            .clone()
            .or_else(|| resolved_model_config.model_source.clone())
            .map(|value| format_runner_model_source(&value))
    } else {
        metadata
            .and_then(|metadata| metadata.get("modelSource"))
            .map(String::as_str)
            .map(format_runner_model_source)
            .filter(|value| !value.trim().is_empty())
    };
    let external_thread_id = metadata.and_then(|metadata| {
        metadata
            .get("threadId")
            .or_else(|| metadata.get("thread_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let external_conversation_id = metadata.and_then(|metadata| {
        metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    crate::im_gateway::progress_card::ProgressRunnerSummary {
        runner_id: runner_id.trim().to_string(),
        adapter: adapter.trim().to_string(),
        model,
        model_source,
        reasoning_effort: resolved_model_config.reasoning_effort.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningEffort").cloned())
        }),
        reasoning_summary: resolved_model_config.reasoning_summary.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningSummary").cloned())
        }),
        reasoning_source: resolved_model_config
            .reasoning_source
            .map(|value| format_runner_model_source(&value)),
        token_usage: metadata.and_then(external_cli_token_usage_from_metadata),
        work_dir: request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        external_thread_id,
        external_conversation_id,
    }
}

fn external_cli_token_usage_from_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
) -> Option<crate::im_gateway::progress_card::ProgressRunnerTokenUsage> {
    let usage = crate::im_gateway::progress_card::ProgressRunnerTokenUsage {
        input_tokens: metadata_u64(metadata, "usageInputTokens"),
        cached_input_tokens: metadata_u64(metadata, "usageCachedInputTokens"),
        output_tokens: metadata_u64(metadata, "usageOutputTokens"),
        reasoning_output_tokens: metadata_u64(metadata, "usageReasoningOutputTokens"),
        total_tokens: metadata_u64(metadata, "usageTotalTokens"),
    };
    (usage.input_tokens.is_some()
        || usage.cached_input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_output_tokens.is_some()
        || usage.total_tokens.is_some())
    .then_some(usage)
}

fn metadata_u64(metadata: &std::collections::BTreeMap<String, String>, key: &str) -> Option<u64> {
    metadata.get(key)?.trim().parse().ok()
}

fn format_runner_model_source(source: &str) -> String {
    match source.trim() {
        "runner config" => "runner 配置".to_string(),
        "codex default" => "Codex 默认".to_string(),
        "trae default" => "Trae 默认".to_string(),
        "codex config" => "Codex 配置".to_string(),
        "trae config" => "Trae 配置".to_string(),
        value => value.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn abort_task_on_drop_cancels_spawned_runner_task() {
        let task = tokio::spawn(std::future::pending::<()>());
        let guard = AbortTaskOnDrop(task.abort_handle());

        drop(guard);

        let error = task.await.expect_err("task should be cancelled by guard");
        assert!(error.is_cancelled());
    }

    #[test]
    fn event_dedup_evict_expired_handles_large_ttl() {
        let mut dedup = EventDedup::new();
        dedup.ttl = std::time::Duration::MAX;

        dedup
            .window
            .push_back(("event-1".to_string(), Instant::now()));
        dedup.evict_expired();

        assert_eq!(dedup.window.len(), 1);
    }

    #[test]
    fn external_cli_images_from_chat_images_preserves_payloads() {
        let images = external_cli_images_from_chat_images(vec![
            bifrost_agent::ChatImageInput {
                mime_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            },
            bifrost_agent::ChatImageInput {
                mime_type: "image/jpeg".to_string(),
                data: "dHdv".to_string(),
            },
        ]);

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].data, "aGVsbG8=");
        assert!(images[0].name.is_none());
        assert_eq!(images[1].mime_type, "image/jpeg");
        assert_eq!(images[1].data, "dHdv");
        assert!(images[1].name.is_none());
    }

    #[test]
    fn external_cli_progress_runner_summary_uses_session_effort_override() {
        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            message: "hello".to_string(),
            images: Vec::new(),
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

    #[test]
    fn im_external_runner_consumes_proactive_outbound_context_once() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        crate::im_gateway::session_state::push_outbound_context_if_unseen(
            "weixin-main:owner",
            "codex",
            Some("codex"),
            "message-1",
            crate::im_gateway::session_state::ImImportedRunnerContext {
                call_id: "outbound-message-1".to_string(),
                source_session_key: "weixin-main:owner".to_string(),
                target_runner_id: "codex".to_string(),
                target_adapter: "proactive_outbound".to_string(),
                user_message: "automation sent this".to_string(),
                response: "2026-07-15 日报概要\n已完成日报。".to_string(),
                created_at: 1,
            },
        )
        .expect("push proactive context");
        let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "日报里说了什么？".to_string(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: Some("weixin-main".to_string()),
            runner_id: Some("codex".to_string()),
            session_key: Some("weixin-main:owner".to_string()),
            runtime: "external_cli".to_string(),
            adapter: "codex".to_string(),
            work_dir: None,
            instructions: Some("base instructions".to_string()),
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };

        consume_imported_contexts_for_im_external_runner(&mut request, "codex");
        let instructions = request.instructions.as_ref().expect("instructions");
        assert!(instructions.contains("base instructions"));
        assert!(instructions.contains("Proactive Messages Sent Through This Bot"));
        assert!(instructions.contains("2026-07-15 日报概要"));

        let mut second = request.clone();
        second.instructions = None;
        consume_imported_contexts_for_im_external_runner(&mut second, "codex");
        assert!(second.instructions.is_none());
    }
}
