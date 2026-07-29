use super::*;

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

pub(super) async fn handle_targets(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /targets  |  POST /targets
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let targets = service.target_store.list();
                json_response(&targets)
            }
            Method::POST => {
                let mut target: ImTarget = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if target.created_at == 0 {
                    target.created_at = now;
                }
                target.updated_at = now;
                match service.target_store.add(target) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // /:id
    if let Some(id_str) = rest.strip_prefix('/') {
        let id = id_str.split('/').next().unwrap_or(id_str);
        if !id.is_empty() && !id.contains('/') {
            return handle_target_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Target endpoint not found")
}

pub(super) async fn handle_target_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.target_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Target not found");
            };
            apply_target_patch(&mut existing, &patch);
            match service.target_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.target_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct SendMessageRequest {
    #[serde(default)]
    pub(super) provider_id: Option<String>,
    #[serde(default)]
    pub(super) target_id: Option<String>,
    #[serde(default = "default_msg_type")]
    pub(super) msg_type: String,
    #[serde(default)]
    pub(super) content: serde_json::Value,
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) card: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) image: Option<SendImageRequest>,
    #[serde(default)]
    pub(super) rich_card: Option<SendRichCardRequest>,
    #[serde(default)]
    pub(super) idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub(super) struct SendImageRequest {
    #[serde(default)]
    pub(super) image_key: Option<String>,
    #[serde(default)]
    pub(super) data_base64: Option<String>,
    #[serde(default)]
    pub(super) file_name: Option<String>,
    #[serde(default)]
    pub(super) mime_type: Option<String>,
    #[serde(default = "default_feishu_image_type")]
    pub(super) image_type: String,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub(super) struct SendRichCardRequest {
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) image_key: Option<String>,
    #[serde(default)]
    pub(super) image: Option<SendImageRequest>,
    #[serde(default)]
    pub(super) image_alt: Option<String>,
}

pub(super) fn default_msg_type() -> String {
    "interactive".to_string()
}

pub(super) fn default_feishu_image_type() -> String {
    "message".to_string()
}

pub(super) async fn handle_messages_send(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body: SendMessageRequest = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let resolved = match resolve_send_message_request(service, &body) {
        Ok(v) => v,
        Err((status, message)) => return error_response(status, &message),
    };

    if !resolved.provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }

    if !resolved.target.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Target is disabled");
    }

    let feishu = service.connection_manager.feishu_provider();
    let prepared =
        match prepare_outbound_content(feishu, &resolved.provider, &body, resolved.content).await {
            Ok(content) => content,
            Err((status, message)) => return error_response(status, &message),
        };
    let content_preview = build_content_preview(&body.msg_type, &prepared);
    let log_content = (body.msg_type == "text").then(|| {
        prepared
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(&prepared).unwrap_or_default())
    });
    let log_msg_type = outbound_log_msg_type(&resolved.provider, &body.msg_type);
    let idempotency_key = body
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if idempotency_key.is_some_and(|value| value.len() > 512) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "idempotency_key must be at most 512 bytes",
        );
    }
    let outbox_send = if let Some(key) = idempotency_key {
        use sha2::{Digest, Sha256};
        let payload = serde_json::to_vec(&prepared).unwrap_or_default();
        let payload_sha256 = format!("{:x}", Sha256::digest(payload));
        match service.outbox_store.begin(
            key,
            &resolved.provider.id,
            &resolved.log_target_id,
            &body.msg_type,
            &payload_sha256,
        ) {
            Ok(crate::im_gateway::ImOutboxBegin::Replay { message_id }) => {
                return json_response(&crate::im_gateway::types::SendResult {
                    message_id,
                    request_id: Some("idempotent-replay".to_string()),
                });
            }
            Ok(crate::im_gateway::ImOutboxBegin::Send { stable_client_id }) => {
                Some((key.to_string(), stable_client_id))
            }
            Err(error) => {
                return error_response(StatusCode::CONFLICT, &error.to_string());
            }
        }
    } else {
        None
    };

    if resolved.provider.provider_type == crate::im_gateway::types::ImProviderType::Weixin
        && !service
            .connection_manager
            .weixin_provider()
            .send_ready(&resolved.provider, &resolved.target)
    {
        if let Some((key, _)) = outbox_send.as_ref() {
            if let Err(error) = service
                .outbox_store
                .mark_pending(key, "Weixin provider is not send-ready")
            {
                error!(
                    error = %error,
                    idempotency_key = %key,
                    "failed to return send-not-ready IM outbox record to pending"
                );
            }
        }
        return error_response(
            StatusCode::CONFLICT,
            "Weixin provider is connected but not send-ready; send the bot an inbound message first",
        );
    }

    // Send via the configured provider implementation.
    let client = service.provider_client(&resolved.provider);
    let result = if body.msg_type == "text" {
        let text = prepared
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(&prepared).unwrap_or_default());
        if let Some((_, stable_client_id)) = outbox_send.as_ref().filter(|_| {
            resolved.provider.provider_type == crate::im_gateway::types::ImProviderType::Weixin
        }) {
            service
                .connection_manager
                .weixin_provider()
                .send_text_with_client_id(
                    &resolved.provider,
                    &resolved.target,
                    &text,
                    stable_client_id,
                )
                .await
        } else {
            client
                .send_text(&resolved.provider, &resolved.target, &text)
                .await
        }
    } else if body.msg_type == "image" {
        let image_key = prepared
            .get("image_key")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        feishu
            .send_image(&resolved.provider, &resolved.target, image_key, None)
            .await
    } else {
        client
            .send_card(
                &resolved.provider,
                &resolved.target,
                prepared.clone(),
                Default::default(),
            )
            .await
    };

    if let Some((key, _)) = outbox_send.as_ref() {
        match &result {
            Ok(send_result) => {
                if let Err(error) = service
                    .outbox_store
                    .mark_sent(key, send_result.message_id.as_deref())
                {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!(
                            "provider acknowledged the message but the durable outbox commit failed: {error}"
                        ),
                    );
                }
            }
            Err(error) => {
                if let Err(store_error) = service.outbox_store.mark_pending(key, &error.to_string())
                {
                    error!(
                        error = %store_error,
                        idempotency_key = %key,
                        "failed to return IM outbox record to pending"
                    );
                }
            }
        }
    }

    // Record outbound message log
    let (status, message_id, error_msg) = match &result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: resolved.provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(resolved.log_target_id),
        target_name: Some(resolved.log_target_name),
        message_id,
        msg_type: Some(log_msg_type),
        content: log_content,
        content_preview,
        trigger: Some("api".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: None,
        reaction_added: None,
    };
    if let Err(e) = service.message_log_store.add(log) {
        error!(error = %e, "failed to store outbound message log");
    }

    match result {
        Ok(result) => json_response(&result),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to send message: {e}"),
        ),
    }
}

#[derive(Debug)]
pub(super) struct ResolvedSendMessage {
    pub(super) provider: ImProviderConfig,
    pub(super) target: ImTarget,
    pub(super) log_target_id: String,
    pub(super) log_target_name: String,
    pub(super) content: serde_json::Value,
}

pub(super) fn resolve_send_message_request(
    service: &ImGatewayService,
    body: &SendMessageRequest,
) -> std::result::Result<ResolvedSendMessage, (StatusCode, String)> {
    let content = normalized_send_content(body)?;
    let target_id = body.target_id.as_deref().unwrap_or("__owner__");

    if matches!(target_id, "__owner__" | "owner") {
        let provider_id = body.provider_id.as_deref().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "provider_id is required when sending to owner".to_string(),
            )
        })?;
        let provider = service.provider_store.get(provider_id).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Provider '{provider_id}' not found"),
            )
        })?;
        let owner_open_id = provider.owner_open_id.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("Provider '{provider_id}' has no owner_open_id"),
            )
        })?;
        let target = ImTarget {
            id: "__owner__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Owner".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: owner_open_id,
            default_msg_type: default_msg_type(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        return Ok(ResolvedSendMessage {
            provider,
            target,
            log_target_id: "__owner__".to_string(),
            log_target_name: "Owner".to_string(),
            content,
        });
    }

    let target = service.target_store.get(target_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Target '{target_id}' not found"),
        )
    })?;
    if let Some(provider_id) = body.provider_id.as_deref() {
        if target.provider_id != provider_id {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Target '{target_id}' does not belong to provider '{provider_id}'"),
            ));
        }
    }
    let provider = service
        .provider_store
        .get(&target.provider_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Provider not found for target".to_string(),
            )
        })?;
    let log_target_id = target.id.clone();
    let log_target_name = target.display_name.clone();
    Ok(ResolvedSendMessage {
        provider,
        target,
        log_target_id,
        log_target_name,
        content,
    })
}

pub(super) fn normalized_send_content(
    body: &SendMessageRequest,
) -> std::result::Result<serde_json::Value, (StatusCode, String)> {
    if !body.content.is_null() {
        return Ok(body.content.clone());
    }
    if let Some(text) = &body.text {
        return Ok(serde_json::Value::String(text.clone()));
    }
    if let Some(card) = &body.card {
        return Ok(card.clone());
    }
    if let Some(image) = &body.image {
        return serde_json::to_value(image).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid image payload: {e}"),
            )
        });
    }
    if let Some(rich_card) = &body.rich_card {
        return serde_json::to_value(rich_card).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid rich_card payload: {e}"),
            )
        });
    }
    Err((
        StatusCode::BAD_REQUEST,
        "content is required for messages/send".to_string(),
    ))
}

pub(super) async fn prepare_outbound_content(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    body: &SendMessageRequest,
    content: serde_json::Value,
) -> std::result::Result<serde_json::Value, (StatusCode, String)> {
    match body.msg_type.as_str() {
        "image" => {
            let image = body.image.clone().or_else(|| parse_image_content(&content));
            let image_key = resolve_image_key(feishu, provider, image.as_ref()).await?;
            Ok(serde_json::json!({ "image_key": image_key }))
        }
        "interactive" => {
            if let Some(rich_card) = &body.rich_card {
                build_rich_card_content(feishu, provider, rich_card).await
            } else {
                Ok(content)
            }
        }
        "text" => Ok(content),
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("unsupported msg_type '{other}'; supported: text, image, interactive"),
        )),
    }
}

pub(super) fn parse_image_content(content: &serde_json::Value) -> Option<SendImageRequest> {
    if content.is_null() {
        return None;
    }
    if let Some(image_key) = content.as_str().filter(|value| !value.trim().is_empty()) {
        return Some(SendImageRequest {
            image_key: Some(image_key.to_string()),
            data_base64: None,
            file_name: None,
            mime_type: None,
            image_type: default_feishu_image_type(),
        });
    }
    serde_json::from_value(content.clone()).ok()
}

pub(super) async fn resolve_image_key(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    image: Option<&SendImageRequest>,
) -> std::result::Result<String, (StatusCode, String)> {
    let image = image.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "image payload is required for msg_type=image".to_string(),
        )
    })?;

    if let Some(image_key) = image
        .image_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(image_key.to_string());
    }

    let data_base64 = image.data_base64.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "image_key or data_base64 is required for image payload".to_string(),
        )
    })?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid image data_base64: {e}"),
            )
        })?;
    if bytes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "image data_base64 decoded to empty bytes".to_string(),
        ));
    }

    let file_name = image.file_name.as_deref().unwrap_or("bifrost-image");
    let uploaded = feishu
        .upload_image(
            provider,
            &image.image_type,
            file_name,
            bytes,
            image.mime_type.as_deref(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to upload image: {e}"),
            )
        })?;
    Ok(uploaded.image_key)
}

pub(super) async fn build_rich_card_content(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    rich_card: &SendRichCardRequest,
) -> std::result::Result<serde_json::Value, (StatusCode, String)> {
    let title = rich_card
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Bifrost");
    let text = rich_card
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let image_key = if let Some(image_key) = rich_card
        .image_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(image_key.to_string())
    } else if rich_card.image.is_some() {
        Some(resolve_image_key(feishu, provider, rich_card.image.as_ref()).await?)
    } else {
        None
    };

    if text.is_none() && image_key.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "rich_card requires text, image_key, or image data".to_string(),
        ));
    }

    let mut elements = Vec::new();
    if let Some(image_key) = image_key {
        let alt = rich_card
            .image_alt
            .as_deref()
            .unwrap_or(title)
            .trim()
            .to_string();
        elements.push(serde_json::json!({
            "tag": "img",
            "img_key": image_key,
            "alt": {
                "tag": "plain_text",
                "content": if alt.is_empty() { title } else { &alt }
            }
        }));
    }
    if let Some(text) = text {
        let rendered_text = render_agent_markdown_for_feishu(
            feishu,
            provider,
            text,
            provider_agent_work_dir(provider).as_deref(),
        )
        .await;
        let rendered_text =
            crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_text);
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": rendered_text
        }));
    }

    Ok(serde_json::json!({
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": title
            }
        },
        "elements": elements
    }))
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub(super) async fn handle_routes(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /routes  |  POST /routes
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let routes = service.route_store.list();
                json_response(&routes)
            }
            Method::POST => {
                let mut route: ImRoute = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if route.created_at == 0 {
                    route.created_at = now;
                }
                route.updated_at = now;
                match service.route_store.add(route) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/pause, /:id/resume
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // /:id/pause
        if let Some(id) = extract_segment_before(id_and_rest, "/pause") {
            return handle_route_pause(req, service, id).await;
        }
        // /:id/resume
        if let Some(id) = extract_segment_before(id_and_rest, "/resume") {
            return handle_route_resume(req, service, id).await;
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_route_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Route endpoint not found")
}

pub(super) async fn handle_route_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.route_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Route not found");
            };
            apply_route_patch(&mut existing, &patch);
            match service.route_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.route_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

pub(super) async fn handle_route_pause(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut route) = service.route_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Route not found");
    };
    route.enabled = false;
    route.updated_at = now_ms();
    match service.route_store.update(route) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": false})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(super) async fn handle_route_resume(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut route) = service.route_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Route not found");
    };
    route.enabled = true;
    route.updated_at = now_ms();
    match service.route_store.update(route) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": true})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------------------------------------------------------------------------
