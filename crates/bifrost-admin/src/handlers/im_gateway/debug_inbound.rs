use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MockInboundRequest {
    provider_id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    raw_feishu_event: Option<serde_json::Value>,
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
    inject_mock_inbound(body, service).await
}

async fn inject_mock_inbound(
    body: MockInboundRequest,
    service: &SharedImGatewayService,
) -> Response<BoxBody> {
    if body.provider_id.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "providerId is required");
    }
    if body.text.trim().is_empty() && body.raw_feishu_event.is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "text or rawFeishuEvent is required",
        );
    }

    let Some(provider) = service.provider_store.get(&body.provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "provider not found");
    };

    if let Some(raw_event) = body.raw_feishu_event {
        if provider.provider_type != ImProviderType::Feishu {
            return error_response(
                StatusCode::BAD_REQUEST,
                "rawFeishuEvent requires a Feishu provider",
            );
        }
        let Some(event) =
            crate::im_gateway::feishu::normalize_feishu_event(&raw_event, provider.id.as_str())
        else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "rawFeishuEvent is unsupported or invalid",
            );
        };
        let event_id = event.event_id.clone();
        let sender_id = event.source.user_id.clone();
        let tx = ensure_mock_event_sink(service, &provider);
        if tx.send(event).is_err() {
            service.mock_event_sinks.write().remove(&provider.id);
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "mock inbound sink is closed",
            );
        }
        return json_response(&serde_json::json!({
            "success": true,
            "providerId": provider.id,
            "eventId": event_id,
            "senderId": sender_id,
            "rawFeishuEvent": true
        }));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider_id: &str, text: &str) -> MockInboundRequest {
        MockInboundRequest {
            provider_id: provider_id.to_string(),
            text: text.to_string(),
            raw_feishu_event: None,
            user_id: Some(" ou_alice ".to_string()),
            chat_id: Some(" oc_engineering ".to_string()),
            chat_type: Some("group".to_string()),
            chat_name: Some(" Engineering ".to_string()),
            user_name: Some("Alice".to_string()),
            mention_bot: true,
            message_id: Some(" om_debug ".to_string()),
            event_id: Some(" evt_debug ".to_string()),
        }
    }

    fn provider(id: &str) -> ImProviderConfig {
        ImProviderConfig {
            id: id.to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Debug Feishu".to_string(),
            enabled: true,
            base_url: None,
            app_id: Some("app".to_string()),
            secret_ref: Some("secret".to_string()),
            owner_open_id: None,
            event_connection_enabled: true,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn mock_inbound_validates_and_injects_group_message() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
        let service = Arc::new(ImGatewayService::new(temp.path()));
        let mut provider = provider("debug-provider");
        provider.base_url = Some("http://127.0.0.1:9".to_string());
        service.provider_store.add(provider).unwrap();
        let mut config = service.agent_config_store.load();
        config.enabled = false;
        service.agent_config_store.save(&config).unwrap();

        assert_eq!(
            inject_mock_inbound(request("", "hello"), &service)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            inject_mock_inbound(request("missing", "hello"), &service)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            inject_mock_inbound(request("debug-provider", "  "), &service)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            inject_mock_inbound(request("debug-provider", "please run"), &service)
                .await
                .status(),
            StatusCode::OK
        );

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if service
                    .group_context_store
                    .message_count("debug-provider", "oc_engineering")
                    .unwrap_or_default()
                    == 1
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mock inbound event was not consumed");
        assert_eq!(
            service
                .group_context_store
                .chat_name("debug-provider", "oc_engineering")
                .unwrap()
                .as_deref(),
            Some("Engineering")
        );
    }

    #[tokio::test]
    async fn mock_inbound_accepts_only_valid_raw_feishu_events() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
        let service = Arc::new(ImGatewayService::new(temp.path()));
        let mut feishu = provider("raw-feishu");
        feishu.owner_open_id = Some("ou_owner".to_string());
        service.provider_store.add(feishu).unwrap();

        let raw_event = serde_json::json!({
            "header": {
                "event_id": "evt_menu_help",
                "event_type": "application.bot.menu_v6"
            },
            "event": {
                "operator": {"operator_id": {"open_id": "ou_owner"}},
                "event_key": "bf_help",
                "timestamp": 1710000000
            }
        });
        let mut body = request("raw-feishu", "");
        body.raw_feishu_event = Some(raw_event);
        assert_eq!(
            inject_mock_inbound(body, &service).await.status(),
            StatusCode::OK
        );

        let mut invalid = request("raw-feishu", "");
        invalid.raw_feishu_event = Some(serde_json::json!({"unsupported": true}));
        assert_eq!(
            inject_mock_inbound(invalid, &service).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn mock_inbound_applies_identity_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
        let service = Arc::new(ImGatewayService::new(temp.path()));
        let mut provider = provider("debug-fallback-provider");
        provider.owner_open_id = Some("owner".to_string());
        service.provider_store.add(provider).unwrap();

        let mut body = request("debug-fallback-provider", "ambient");
        body.user_id = Some(" ".to_string());
        body.chat_id = Some(" ".to_string());
        body.event_id = Some(" ".to_string());
        body.message_id = Some(" ".to_string());
        body.chat_name = None;
        body.mention_bot = false;
        assert_eq!(
            inject_mock_inbound(body, &service).await.status(),
            StatusCode::OK
        );
    }
}
