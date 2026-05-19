use super::*;

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

pub(super) async fn handle_providers(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /providers  |  POST /providers
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let providers = service.provider_store.list();
                let safe: Vec<_> = providers.iter().map(sanitize_provider).collect();
                json_response(&safe)
            }
            Method::POST => {
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let mut config = match parse_provider_create_payload(payload) {
                    Ok(v) => v,
                    Err(e) => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &format!("Invalid request body: {e}"),
                        );
                    }
                };
                let now = now_ms();
                if config.created_at == 0 {
                    config.created_at = now;
                }
                config.updated_at = now;
                normalize_provider_agent_config(&mut config);
                match service.provider_store.add(config) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/status, /:id/policy, /:id/policy/bind-shell
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // Check for /:id/policy/bind-shell
        if let Some(id) = extract_segment_before(id_and_rest, "/policy/bind-shell") {
            return handle_provider_policy_bind_shell(req, service, id).await;
        }
        // Check for /:id/policy
        if let Some(id) = extract_segment_before(id_and_rest, "/policy") {
            return handle_provider_policy(req, service, id).await;
        }
        // Check for /:id/status
        if let Some(id) = extract_segment_before(id_and_rest, "/status") {
            return handle_provider_status(&req, service, id);
        }
        // Check for /:id/connect
        if let Some(id) = extract_segment_before(id_and_rest, "/connect") {
            return handle_provider_connect(req, service, id).await;
        }
        // Check for /:id/disconnect
        if let Some(id) = extract_segment_before(id_and_rest, "/disconnect") {
            return handle_provider_disconnect(req, service, id).await;
        }
        // Check for /:id/weixin-login/start
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/start") {
            return handle_provider_weixin_login_start(req, service, id).await;
        }
        // Check for /:id/weixin-login/status
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/status") {
            return handle_provider_weixin_login_status(req, service, id).await;
        }
        // Check for /:id/weixin-login/complete
        if let Some(id) = extract_segment_before(id_and_rest, "/weixin-login/complete") {
            return handle_provider_weixin_login_complete(req, service, id).await;
        }
        // Check for /:id/messages
        if let Some(id) = extract_segment_before(id_and_rest, "/messages") {
            return handle_provider_messages(req, service, id).await;
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_provider_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Provider endpoint not found")
}

pub(super) async fn handle_provider_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => match service.provider_store.get(id) {
            Some(p) => json_response(&sanitize_provider(&p)),
            None => error_response(StatusCode::NOT_FOUND, "Provider not found"),
        },
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            apply_provider_patch(&mut existing, &patch);
            match service.provider_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.provider_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

pub(super) fn handle_provider_status(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let status = service.connection_manager.get_status(id);
    match status {
        Some(s) => json_response(&s),
        None => {
            if service.provider_store.get(id).is_some() {
                json_response(&crate::im_gateway::types::ConnectionStatus::default())
            } else {
                error_response(StatusCode::NOT_FOUND, "Provider not found")
            }
        }
    }
}

pub(super) async fn handle_provider_weixin_login_start(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    match service
        .connection_manager
        .weixin_provider()
        .start_login(provider.base_url.as_deref())
        .await
    {
        Ok(login) => {
            service.weixin_login_pending.write().insert(
                provider.id.clone(),
                PendingWeixinLogin {
                    login: login.clone(),
                    created_at_ms: now_ms(),
                },
            );
            json_response(&serde_json::json!({
                "success": true,
                "scan_url": login.scan_url,
                "expires_in_seconds": login.expires_in_seconds,
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to start weixin login: {e}"),
        ),
    }
}

pub(super) async fn handle_provider_weixin_login_status(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    let Some(pending) = service.weixin_login_pending.read().get(id).cloned() else {
        return json_response(&serde_json::json!({
            "success": true,
            "status": if provider.secret_ref.is_some() { "authorized" } else { "idle" },
            "provider": sanitize_provider(&provider),
        }));
    };
    let expires_at_ms =
        pending.created_at_ms + pending.login.expires_in_seconds.saturating_mul(1000);
    if now_ms() >= expires_at_ms {
        service.weixin_login_pending.write().remove(id);
        return json_response(&serde_json::json!({
            "success": true,
            "status": "expired",
            "expires_at": expires_at_ms,
        }));
    }
    match service
        .connection_manager
        .weixin_provider()
        .complete_login(
            &pending.login.poll_key,
            provider.base_url.as_deref(),
            1,
            std::time::Duration::ZERO,
        )
        .await
    {
        Ok(account) => {
            provider.app_id = Some(account.account_id.clone());
            provider.owner_open_id = Some(account.user_id.clone());
            provider.base_url = Some(account.base_url.clone());
            provider.secret_ref = Some(account.bot_token);
            provider.enabled = true;
            provider.event_connection_enabled = true;
            if provider.event_types.is_empty() {
                provider.event_types = vec!["message.receive".to_string()];
            }
            provider.updated_at = now_ms();
            normalize_provider_agent_config(&mut provider);
            if let Err(e) = service.provider_store.update(provider.clone()) {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            service.weixin_login_pending.write().remove(id);
            json_response(&serde_json::json!({
                "success": true,
                "status": "confirmed",
                "provider": sanitize_provider(&provider),
                "account": {
                    "account_id": account.account_id,
                    "user_id": account.user_id,
                    "base_url": account.base_url,
                }
            }))
        }
        Err(e) => {
            let error = e.to_string();
            if error.contains("expired") {
                service.weixin_login_pending.write().remove(id);
                return json_response(&serde_json::json!({
                    "success": true,
                    "status": "expired",
                    "expires_at": expires_at_ms,
                }));
            }
            json_response(&serde_json::json!({
                "success": true,
                "status": "pending",
                "expires_at": expires_at_ms,
            }))
        }
    }
}

pub(super) async fn handle_provider_weixin_login_complete(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if provider.provider_type != crate::im_gateway::types::ImProviderType::Weixin {
        return error_response(StatusCode::BAD_REQUEST, "Provider is not a weixin provider");
    }
    let Some(pending) = service.weixin_login_pending.read().get(id).cloned() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "No pending weixin login; start QR login first",
        );
    };
    match service
        .connection_manager
        .weixin_provider()
        .complete_login(
            &pending.login.poll_key,
            provider.base_url.as_deref(),
            20,
            std::time::Duration::from_secs(2),
        )
        .await
    {
        Ok(account) => {
            provider.app_id = Some(account.account_id.clone());
            provider.owner_open_id = Some(account.user_id.clone());
            provider.base_url = Some(account.base_url.clone());
            provider.secret_ref = Some(account.bot_token);
            provider.enabled = true;
            provider.event_connection_enabled = true;
            if provider.event_types.is_empty() {
                provider.event_types = vec!["message.receive".to_string()];
            }
            provider.updated_at = now_ms();
            normalize_provider_agent_config(&mut provider);
            if let Err(e) = service.provider_store.update(provider.clone()) {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            service.weixin_login_pending.write().remove(id);
            json_response(&serde_json::json!({
                "success": true,
                "provider": sanitize_provider(&provider),
                "account": {
                    "account_id": account.account_id,
                    "user_id": account.user_id,
                    "base_url": account.base_url,
                }
            }))
        }
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to complete weixin login: {e}"),
        ),
    }
}

/// POST /providers/:id/connect — start event long connection for a provider.
///
/// If `owner_open_id` is not configured, this will auto-detect it from the
/// Feishu Application Info API and persist it to the provider store.
pub(super) async fn handle_provider_connect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };

    let app_secret = match provider.secret_ref.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return error_response(StatusCode::BAD_REQUEST, "Provider has no secret configured"),
    };

    // Auto-detect owner_open_id if not set for Feishu. Weixin gets owner from QR login.
    let feishu = service.connection_manager.feishu_provider().clone();
    if provider.provider_type == crate::im_gateway::types::ImProviderType::Feishu
        && provider
            .owner_open_id
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        match feishu.fetch_bot_owner_open_id(&provider).await {
            Ok(owner_id) => {
                info!(
                    provider_id = id,
                    owner_open_id = %owner_id,
                    "auto-detected bot owner"
                );
                provider.owner_open_id = Some(owner_id);
                // Persist to store
                if let Err(e) = service.provider_store.update(provider.clone()) {
                    warn!(provider_id = id, error = %e, "failed to persist auto-detected owner_open_id");
                }
            }
            Err(e) => {
                warn!(
                    provider_id = id,
                    error = %e,
                    "failed to auto-detect owner, connection will proceed without owner filter"
                );
            }
        }
    }

    // Create event channel
    let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();

    // Spawn the event processing loop
    let client = service.provider_client(&provider);
    let provider_for_loop = provider.clone();
    let event_store = service.event_store.clone();
    let message_log_store = service.message_log_store.clone();
    let route_store = service.route_store.clone();
    let provider_store = service.provider_store.clone();
    let agent_config_store = service.agent_config_store.clone();
    let agent_client = service.agent_client.clone();
    let agent_tools = service.agent_tools.clone();
    let schedule_store = service.schedule_store.clone();
    let scheduler = service.scheduler.clone();
    let target_store = service.target_store.clone();
    let connection_manager = service.connection_manager.clone();
    let agent_session_manager = service.agent_session_manager.clone();
    let external_cli_config_store = service.external_cli_config_store.clone();
    let queue_manager = service.queue_manager.clone();
    let progress_registry = service.progress_registry.clone();
    tokio::spawn(async move {
        run_event_loop(
            rx,
            client,
            provider_for_loop,
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
        )
        .await;
    });

    // Start the long connection
    match service
        .connection_manager
        .start_connection(&provider, &app_secret, tx)
        .await
    {
        Ok(()) => {
            info!(provider_id = id, "provider event connection started");
            json_response(&serde_json::json!({"success": true, "message": "Connection started"}))
        }
        Err(e) => {
            service.connection_manager.mark_failed(id, e.to_string());
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to start connection: {e}"),
            )
        }
    }
}

/// POST /providers/:id/disconnect — stop event long connection.
pub(super) async fn handle_provider_disconnect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    service.connection_manager.stop_connection(id);
    info!(provider_id = id, "provider event connection stopped");
    json_response(&serde_json::json!({"success": true, "message": "Connection stopped"}))
}

/// GET /providers/:id/messages — list message logs for a provider.
///   Query params: ?direction=inbound|outbound  &source=user|bot
/// DELETE /providers/:id/messages — clear message logs for a provider.
pub(super) async fn handle_provider_messages(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    match *req.method() {
        Method::GET => {
            let query = req.uri().query().unwrap_or("");
            let params = parse_query_params(query);
            let direction_filter = params.get("direction").map(|s| s.as_str());
            let source_filter = params.get("source").map(|s| s.as_str());

            let mut messages = service.message_log_store.list_by_provider(id);

            // Filter by direction
            if let Some(dir) = direction_filter {
                messages.retain(|m| match dir {
                    "inbound" => matches!(m.direction, MessageDirection::Inbound),
                    "outbound" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            // Filter by source: "user" = inbound from user, "bot" = outbound from bot
            if let Some(src) = source_filter {
                messages.retain(|m| match src {
                    "user" => matches!(m.direction, MessageDirection::Inbound),
                    "bot" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            json_response(&messages)
        }
        Method::DELETE => match service.message_log_store.clear_by_provider(id) {
            Ok(()) => {
                json_response(&serde_json::json!({"success": true, "message": "Messages cleared"}))
            }
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

/// Build a session key that is unique per (provider, user) pair.
///
/// This ensures that the same user chatting through different bots gets
/// independent agent sessions, preventing cross-bot history contamination.
pub(super) fn build_session_key(provider_id: &str, user_id: Option<&str>) -> String {
    let user = user_id.unwrap_or("unknown");
    format!("{provider_id}:{user}")
}

pub(super) fn effective_agent_config_for_provider(
    base: &crate::im_gateway::agent::ImAgentConfig,
    provider: &ImProviderConfig,
) -> crate::im_gateway::agent::ImAgentConfig {
    let mut config = base.clone();
    if let Some(agent_config) = provider.agent_config.as_ref() {
        if let Some(runner) = agent_config.runner.as_ref() {
            config.runner = Some(runner.clone());
        }
        if let Some(work_dir) = agent_config
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.work_dir = Some(work_dir.to_string());
        }
        if let Some(instructions) = agent_config
            .base_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.base_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .developer_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.developer_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .user_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.user_instructions = Some(instructions.to_string());
        }
    }
    config
}

pub(super) fn effective_agent_work_dir_for_provider(
    base: &crate::im_gateway::agent::ImAgentConfig,
    provider: &ImProviderConfig,
) -> Option<std::path::PathBuf> {
    effective_agent_config_for_provider(base, provider)
        .work_dir
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------------
