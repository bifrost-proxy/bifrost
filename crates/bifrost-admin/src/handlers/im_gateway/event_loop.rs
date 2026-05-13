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
    queue_manager: Arc<SessionQueueManager>,
    progress_registry: Arc<ImAgentProgressRegistry>,
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
        }
    }

    mcp_manager.shutdown().await;

    info!(
        provider_id = %provider.id,
        "event processing loop ended"
    );
}

// ---------------------------------------------------------------------------
