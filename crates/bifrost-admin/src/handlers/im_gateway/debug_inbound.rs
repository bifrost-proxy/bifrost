use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MockInboundRequest {
    provider_id: String,
    text: String,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    chat_type: Option<String>,
    #[serde(default)]
    chat_name: Option<String>,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    mention_bot: bool,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    event_id: Option<String>,
}

pub(super) async fn handle_debug(
    req: Request<Incoming>,
    service: &SharedImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');
    match rest {
        "/mock-inbound" => handle_mock_inbound(req, service).await,
        _ => error_response(StatusCode::NOT_FOUND, "IM Gateway debug endpoint not found"),
    }
}

async fn handle_mock_inbound(
    req: Request<Incoming>,
    service: &SharedImGatewayService,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body: MockInboundRequest = match read_body_json(req).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.provider_id.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "providerId is required");
    }
    if body.text.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "text is required");
    }

    let Some(provider) = service.provider_store.get(&body.provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "provider not found");
    };

    let sender_id = body
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| provider.owner_open_id.clone())
        .unwrap_or_else(|| "mock-user".to_string());
    let chat_id = body
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| sender_id.clone());
    let event_id = body
        .event_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("mock-{}", uuid_short()));
    let message_id = body
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("mock-msg-{}", uuid_short()));

    let tx = ensure_mock_event_sink(service, &provider);
    let event = ImEvent {
        event_id: event_id.clone(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some(chat_id.clone()),
            user_id: Some(sender_id.clone()),
            message_id: Some(message_id.clone()),
            chat_type: body.chat_type,
            user_name: body.user_name,
            sender_type: Some("user".to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: body.text,
            mentions: if body.mention_bot {
                vec![crate::im_gateway::types::ImMention {
                    key: "@_user_1".to_string(),
                    open_id: Some("mock-bot".to_string()),
                    name: Some("Bifrost".to_string()),
                    tenant_key: None,
                    is_bot: true,
                }]
            } else {
                Vec::new()
            },
            images: Vec::new(),
            raw_type: Some("text".to_string()),
            raw_content: body
                .chat_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|chat_name| serde_json::json!({"_bifrost_debug_chat_name": chat_name})),
            create_time: Some(now_ms()),
            update_time: None,
            root_id: None,
            parent_id: None,
            thread_id: None,
        }),
        received_at: now_ms(),
        raw_digest: Some("mock_inbound".to_string()),
    };

    if tx.send(event).is_err() {
        service.mock_event_sinks.write().remove(&provider.id);
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "mock inbound sink is closed",
        );
    }

    info!(
        provider_id = %provider.id,
        event_id = %event_id,
        message_id = %message_id,
        sender_id = %sender_id,
        chat_id = %chat_id,
        "mock inbound IM event injected"
    );

    json_response(&serde_json::json!({
        "success": true,
        "providerId": provider.id,
        "eventId": event_id,
        "messageId": message_id,
        "senderId": sender_id,
        "chatId": chat_id
    }))
}

fn ensure_mock_event_sink(
    service: &SharedImGatewayService,
    provider: &ImProviderConfig,
) -> mpsc::UnboundedSender<ImEvent> {
    if let Some(tx) = service.mock_event_sinks.read().get(&provider.id).cloned() {
        return tx;
    }

    let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();
    service
        .mock_event_sinks
        .write()
        .insert(provider.id.clone(), tx.clone());

    let client = service.provider_client(provider);
    let provider_for_loop = provider.clone();
    let event_store = service.event_store.clone();
    let message_log_store = service.message_log_store.clone();
    let group_context_store = service.group_context_store.clone();
    let route_store = service.route_store.clone();
    let provider_store = service.provider_store.clone();
    let agent_config_store = service.agent_config_store.clone();
    let schedule_store = service.schedule_store.clone();
    let scheduler = service.scheduler.clone();
    let target_store = service.target_store.clone();
    let connection_manager = service.connection_manager.clone();
    let agent_session_manager = service.agent_session_manager.clone();
    let external_cli_config_store = service.external_cli_config_store.clone();
    let queue_manager = service.queue_manager.clone();
    let progress_registry = service.progress_registry.clone();
    tokio::spawn(async move {
        run_event_loop_with_options(
            rx,
            client,
            provider_for_loop,
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
            EventLoopOptions {
                send_online_notification: false,
            },
        )
        .await;
    });
    tx
}
