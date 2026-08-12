use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MockInboundRequest {
    provider_id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    files: Vec<MockInboundFile>,
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
    mentioned_bot_open_id: Option<String>,
    #[serde(default)]
    mentioned_bot_name: Option<String>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    reply_to: Option<MockInboundReplyReference>,
    #[serde(default)]
    root_id: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MockInboundReplyReference {
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    created_at_ms: Option<u64>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MockInboundFile {
    #[serde(default)]
    file_key: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    data: Option<String>,
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
    if body.text.trim().is_empty()
        && body
            .files
            .iter()
            .all(|file| file.data.as_deref().unwrap_or_default().trim().is_empty())
    {
        return error_response(StatusCode::BAD_REQUEST, "text or files are required");
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
    let has_file = body
        .files
        .iter()
        .any(|file| !file.data.as_deref().unwrap_or_default().trim().is_empty());
    let raw_type = if body.text.trim().is_empty() && has_file {
        "file"
    } else {
        "text"
    };

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
                    open_id: body
                        .mentioned_bot_open_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("mock-bot")
                        .to_string()
                        .into(),
                    name: body
                        .mentioned_bot_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("Bifrost")
                        .to_string()
                        .into(),
                    tenant_key: None,
                    is_bot: body.mentioned_bot_open_id.is_none(),
                }]
            } else {
                Vec::new()
            },
            images: Vec::new(),
            files: body
                .files
                .into_iter()
                .filter_map(|file| {
                    let data = file.data?.trim().to_string();
                    if data.is_empty() {
                        return None;
                    }
                    Some(crate::im_gateway::types::ImFileAttachment {
                        file_key: file
                            .file_key
                            .unwrap_or_else(|| format!("mock-file-{}", uuid_short())),
                        name: file.name,
                        mime_type: file.mime_type,
                        size_bytes: None,
                        data_base64: Some(data),
                        download_url: None,
                        ..Default::default()
                    })
                })
                .collect(),
            reply_to: body
                .reply_to
                .map(|reply| crate::im_gateway::types::ImMessageReference {
                    message_id: reply.message_id,
                    created_at_ms: reply.created_at_ms,
                    text: reply.text,
                }),
            raw_type: Some(raw_type.to_string()),
            raw_content: body
                .chat_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|chat_name| serde_json::json!({"_bifrost_debug_chat_name": chat_name})),
            create_time: Some(now_ms()),
            update_time: None,
            root_id: body.root_id,
            parent_id: body.parent_id,
            thread_id: body.thread_id,
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
            user_id: Some(" ou_alice ".to_string()),
            chat_id: Some(" oc_engineering ".to_string()),
            chat_type: Some("group".to_string()),
            chat_name: Some(" Engineering ".to_string()),
            user_name: Some("Alice".to_string()),
            mention_bot: true,
            mentioned_bot_open_id: None,
            mentioned_bot_name: None,
            message_id: Some(" om_debug ".to_string()),
            event_id: Some(" evt_debug ".to_string()),
            root_id: Some("om_root".to_string()),
            parent_id: Some("om_parent".to_string()),
            thread_id: Some("omt_thread".to_string()),
            reply_to: None,
            files: Vec::new(),
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
        let mut runnable = request("debug-provider", "please run");
        runnable.root_id = None;
        runnable.parent_id = None;
        runnable.thread_id = None;
        assert_eq!(
            inject_mock_inbound(runnable, &service).await.status(),
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

    #[tokio::test]
    async fn mock_inbound_accepts_file_only_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp.path());
        let service = Arc::new(ImGatewayService::new(temp.path()));
        service
            .provider_store
            .add(provider("debug-file-provider"))
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        service
            .mock_event_sinks
            .write()
            .insert("debug-file-provider".to_string(), tx);
        let mut body = request("debug-file-provider", "  ");
        body.files = vec![
            MockInboundFile {
                file_key: None,
                name: Some("report.md".to_string()),
                mime_type: Some("text/markdown".to_string()),
                data: Some("  IyBSZXBvcnQ=  ".to_string()),
            },
            MockInboundFile {
                file_key: Some("ignored-empty".to_string()),
                name: Some("empty.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                data: Some(" ".to_string()),
            },
            MockInboundFile {
                file_key: Some("ignored-missing-data".to_string()),
                name: Some("missing.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                data: None,
            },
        ];

        assert_eq!(
            inject_mock_inbound(body, &service).await.status(),
            StatusCode::OK
        );
        let event = rx.try_recv().expect("mock file event");
        let message = event.message.expect("mock message");

        assert_eq!(message.raw_type.as_deref(), Some("file"));
        assert_eq!(message.files.len(), 1);
        assert!(message.files[0].file_key.starts_with("mock-file-"));
        assert_eq!(message.files[0].name.as_deref(), Some("report.md"));
        assert_eq!(message.files[0].mime_type.as_deref(), Some("text/markdown"));
        assert_eq!(
            message.files[0].data_base64.as_deref(),
            Some("IyBSZXBvcnQ=")
        );
        assert!(message.files[0].download_url.is_none());
    }
}
