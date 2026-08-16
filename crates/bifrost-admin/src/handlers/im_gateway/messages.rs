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
                if let Err(message) = validate_target(service, &target) {
                    return error_response(StatusCode::BAD_REQUEST, &message);
                }
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
            if let Err(message) = validate_target(service, &existing) {
                return error_response(StatusCode::BAD_REQUEST, &message);
            }
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

fn validate_target(
    service: &ImGatewayService,
    target: &ImTarget,
) -> std::result::Result<(), String> {
    if target.id.trim().is_empty() {
        return Err("target id is required".to_string());
    }
    if target.display_name.trim().is_empty() {
        return Err("target display_name is required".to_string());
    }
    if target.receive_id.trim().is_empty() {
        return Err("target receive_id is required".to_string());
    }
    let provider = service
        .provider_store
        .get(&target.provider_id)
        .ok_or_else(|| format!("Provider '{}' not found", target.provider_id))?;
    let capabilities = service
        .provider_client(&provider)
        .send_capabilities(&provider);
    if !capabilities
        .receive_id_types
        .iter()
        .any(|value| value == &target.receive_id_type)
    {
        return Err(format!(
            "receive_id_type '{}' is not supported by provider '{}'; supported: {}",
            target.receive_id_type,
            provider.id,
            capabilities.receive_id_types.join(", ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, serde::Serialize)]
pub(crate) struct SendMessageRequest {
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
    pub(super) destination: Option<SendDestinationRequest>,
    #[serde(default)]
    pub(super) parts: Vec<SendPartRequest>,
    #[serde(default)]
    pub(super) idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum SendDestinationRequest {
    Owner,
    Target {
        target_id: String,
    },
    Direct {
        receive_id_type: String,
        receive_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SendPartRequest {
    Text {
        text: String,
    },
    Markdown {
        text: String,
    },
    Image {
        image_key: String,
    },
    File {
        file_key: String,
        #[serde(default)]
        file_name: Option<String>,
    },
    NativeCard {
        card: serde_json::Value,
    },
}

impl SendPartRequest {
    fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Markdown { .. } => "markdown",
            Self::Image { .. } => "image",
            Self::File { .. } => "file",
            Self::NativeCard { .. } => "native_card",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct SendPartReceipt {
    pub(super) index: usize,
    pub(super) requested_kind: String,
    pub(super) delivered_kind: String,
    pub(super) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct SendBundleResponse {
    pub(super) bundle_id: String,
    pub(super) provider_id: String,
    pub(super) destination: String,
    pub(super) status: String,
    pub(super) receipts: Vec<SendPartReceipt>,
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
    handle_messages_send_with_delegation(req, service, true).await
}

#[cfg(test)]
pub(super) async fn handle_messages_send_in_process(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    handle_messages_send_with_delegation(req, service, false).await
}

async fn handle_messages_send_with_delegation(
    req: Request<Incoming>,
    service: &ImGatewayService,
    delegate_to_worker: bool,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body: SendMessageRequest = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if delegate_to_worker
        && crate::worker_runtime::worker_execution_enabled(
            crate::worker_runtime::WorkerKind::ImGateway,
        )
        && !crate::worker_runtime::im_gateway::is_im_gateway_worker_process()
    {
        return crate::worker_runtime::im_gateway::send_message(body).await;
    }

    handle_messages_send_body(service, body).await
}

pub(crate) async fn handle_messages_send_body(
    service: &ImGatewayService,
    body: SendMessageRequest,
) -> Response<BoxBody> {
    if !body.parts.is_empty() || body.destination.is_some() {
        return handle_message_bundle_send(service, body).await;
    }

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

    let client = service.provider_client(&resolved.provider);
    let prepared = match prepare_outbound_content(
        &client,
        &resolved.provider,
        &body,
        resolved.content,
    )
    .await
    {
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
        client
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
pub(crate) struct UploadMessageMetadata {
    pub(crate) provider_id: String,
    pub(crate) kind: String,
    pub(crate) file_name: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) image_type: String,
}

#[derive(Debug)]
pub(crate) struct UploadMessageRequest {
    pub(crate) metadata: UploadMessageMetadata,
    pub(crate) body: Vec<u8>,
}

pub(super) async fn handle_messages_upload(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    handle_messages_upload_with_delegation(req, service, true).await
}

#[cfg(test)]
pub(super) async fn handle_messages_upload_in_process(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    handle_messages_upload_with_delegation(req, service, false).await
}

async fn handle_messages_upload_with_delegation(
    req: Request<Incoming>,
    service: &ImGatewayService,
    delegate_to_worker: bool,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let query = req.uri().query().unwrap_or_default();
    let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let required = |name: &str| {
        params
            .get(name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("query parameter '{name}' is required"))
    };
    let provider_id = match required("provider_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let kind = match required("kind") {
        Ok("image") => "image",
        Ok("file") => "file",
        Ok(other) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("unsupported upload kind '{other}'; supported: image, file"),
            )
        }
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let file_name =
        match required("file_name") {
            Ok(value) if is_safe_upload_file_name(value) => value,
            Ok(_) => return error_response(
                StatusCode::BAD_REQUEST,
                "file_name must be a plain file name without path components or control characters",
            ),
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };

    let Some(provider) = service.provider_store.get(provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if !provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }
    let client = service.provider_client(&provider);
    let capabilities = client.send_capabilities(&provider);
    let Some(capability) = capabilities.part(kind) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("provider '{provider_id}' does not support {kind} uploads"),
        );
    };
    if capability.support == crate::im_gateway::types::ImSendSupportLevel::Unsupported {
        return error_response(
            StatusCode::BAD_REQUEST,
            capability
                .reason
                .as_deref()
                .unwrap_or("upload type is unsupported by this provider"),
        );
    }
    let max_bytes = capability.max_bytes.unwrap_or(10 * 1024 * 1024);
    if req
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > max_bytes)
    {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("{kind} upload exceeds the {max_bytes} byte limit"),
        );
    }
    let mime_type = params
        .get("mime_type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            req.headers()
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        });
    let metadata = UploadMessageMetadata {
        provider_id: provider_id.to_string(),
        kind: kind.to_string(),
        file_name: file_name.to_string(),
        mime_type,
        image_type: params
            .get("image_type")
            .cloned()
            .unwrap_or_else(|| "message".to_string()),
    };
    if delegate_to_worker
        && crate::worker_runtime::worker_execution_enabled(
            crate::worker_runtime::WorkerKind::ImGateway,
        )
        && !crate::worker_runtime::im_gateway::is_im_gateway_worker_process()
    {
        return crate::worker_runtime::im_gateway::upload_message_stream(
            metadata,
            req.into_body(),
            max_bytes,
        )
        .await;
    }

    let body = match http_body_util::Limited::new(req.into_body(), max_bytes as usize)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("{kind} upload exceeded the {max_bytes} byte streaming limit: {error}"),
            )
        }
    };
    if body.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "upload body must not be empty");
    }
    if body.len() as u64 > max_bytes {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("{kind} upload exceeds the {max_bytes} byte limit"),
        );
    }

    let upload = UploadMessageRequest {
        metadata,
        body: body.to_vec(),
    };
    handle_messages_upload_body(service, upload).await
}

pub(crate) async fn handle_messages_upload_body(
    service: &ImGatewayService,
    upload: UploadMessageRequest,
) -> Response<BoxBody> {
    let metadata = upload.metadata;
    let Some(provider) = service.provider_store.get(&metadata.provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if !provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }
    let client = service.provider_client(&provider);
    let capabilities = client.send_capabilities(&provider);
    let Some(capability) = capabilities.part(&metadata.kind) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "provider '{}' does not support {} uploads",
                metadata.provider_id, metadata.kind
            ),
        );
    };
    if capability.support == crate::im_gateway::types::ImSendSupportLevel::Unsupported {
        return error_response(
            StatusCode::BAD_REQUEST,
            capability
                .reason
                .as_deref()
                .unwrap_or("upload type is unsupported by this provider"),
        );
    }
    let max_bytes = capability.max_bytes.unwrap_or(10 * 1024 * 1024);
    if upload.body.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "upload body must not be empty");
    }
    if upload.body.len() as u64 > max_bytes {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "{} upload exceeds the {max_bytes} byte limit",
                metadata.kind
            ),
        );
    }

    let result = if metadata.kind == "image" {
        client
            .upload_image(
                &provider,
                &metadata.image_type,
                &metadata.file_name,
                upload.body,
                metadata.mime_type.as_deref(),
            )
            .await
            .map(|uploaded| {
                serde_json::json!({
                    "kind": "image",
                    "key": uploaded.image_key,
                    "request_id": uploaded.request_id,
                })
            })
    } else {
        client
            .upload_file(
                &provider,
                &metadata.file_name,
                upload.body,
                metadata.mime_type.as_deref(),
            )
            .await
            .map(|file_key| serde_json::json!({ "kind": "file", "key": file_key }))
    };

    match result {
        Ok(value) => json_response(&value),
        Err(error) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("failed to upload {}: {error}", metadata.kind),
        ),
    }
}

fn is_safe_upload_file_name(file_name: &str) -> bool {
    !file_name.is_empty()
        && !file_name.chars().any(char::is_control)
        && !file_name.contains('/')
        && !file_name.contains('\\')
        && Path::new(file_name)
            .file_name()
            .and_then(|value| value.to_str())
            == Some(file_name)
}

pub(super) async fn handle_message_bundle_send(
    service: &ImGatewayService,
    body: SendMessageRequest,
) -> Response<BoxBody> {
    if body.parts.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "parts must not be empty");
    }
    if body.parts.len() > 16 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "parts must contain at most 16 items",
        );
    }
    for (index, part) in body.parts.iter().enumerate() {
        if let Err(message) = validate_send_part(part) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid part at index {index}: {message}"),
            );
        }
    }
    let provider_id = match body
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value,
        None => return error_response(StatusCode::BAD_REQUEST, "provider_id is required"),
    };
    let Some(provider) = service.provider_store.get(provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    if !provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }
    let (target, log_target_id, log_target_name, destination) =
        match resolve_bundle_destination(service, &provider, &body) {
            Ok(value) => value,
            Err((status, message)) => return error_response(status, &message),
        };
    if !target.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Target is disabled");
    }
    let idempotency_key = body
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if idempotency_key.is_some_and(|value| value.len() > 480) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "bundle idempotency_key must be at most 480 bytes",
        );
    }

    let client = service.provider_client(&provider);
    let capabilities = client.send_capabilities(&provider);
    let bundle_id = idempotency_key
        .map(str::to_string)
        .unwrap_or_else(|| format!("im-bundle-{}", uuid_short()));
    let mut receipts = Vec::with_capacity(body.parts.len());

    for (index, part) in body.parts.iter().enumerate() {
        let requested_kind = part.kind();
        let capability = capabilities.part(requested_kind);
        let delivered_kind = capability
            .and_then(|value| value.delivered_as.as_deref())
            .unwrap_or(requested_kind)
            .to_string();
        let warning = capability.and_then(|value| {
            (value.support == crate::im_gateway::types::ImSendSupportLevel::Degraded)
                .then(|| value.reason.clone())
                .flatten()
        });
        if capability.is_none_or(|value| {
            value.support == crate::im_gateway::types::ImSendSupportLevel::Unsupported
        }) {
            let error = capability
                .and_then(|value| value.reason.clone())
                .unwrap_or_else(|| format!("{requested_kind} is unsupported by this provider"));
            receipts.push(SendPartReceipt {
                index,
                requested_kind: requested_kind.to_string(),
                delivered_kind,
                status: "failed".to_string(),
                message_id: None,
                request_id: None,
                warning: None,
                error: Some(error),
            });
            continue;
        }

        if provider.provider_type == crate::im_gateway::types::ImProviderType::Weixin
            && !service
                .connection_manager
                .weixin_provider()
                .send_ready(&provider, &target)
        {
            receipts.push(SendPartReceipt {
                index,
                requested_kind: requested_kind.to_string(),
                delivered_kind,
                status: "failed".to_string(),
                message_id: None,
                request_id: None,
                warning,
                error: Some(
                    "Weixin provider is connected but not send-ready; send the bot an inbound message first"
                        .to_string(),
                ),
            });
            continue;
        }

        use sha2::{Digest, Sha256};
        let part_payload = serde_json::to_vec(part).unwrap_or_default();
        let payload_sha256 = format!("{:x}", Sha256::digest(part_payload));
        let part_key = idempotency_key.map(|key| format!("{key}:{index:03}"));
        let stable_client_id = if let Some(key) = part_key.as_deref() {
            match service.outbox_store.begin(
                key,
                &provider.id,
                &log_target_id,
                requested_kind,
                &payload_sha256,
            ) {
                Ok(crate::im_gateway::ImOutboxBegin::Replay { message_id }) => {
                    receipts.push(SendPartReceipt {
                        index,
                        requested_kind: requested_kind.to_string(),
                        delivered_kind,
                        status: "success".to_string(),
                        message_id,
                        request_id: Some("idempotent-replay".to_string()),
                        warning,
                        error: None,
                    });
                    continue;
                }
                Ok(crate::im_gateway::ImOutboxBegin::Send { stable_client_id }) => {
                    Some(stable_client_id)
                }
                Err(error) => {
                    receipts.push(SendPartReceipt {
                        index,
                        requested_kind: requested_kind.to_string(),
                        delivered_kind,
                        status: "failed".to_string(),
                        message_id: None,
                        request_id: None,
                        warning,
                        error: Some(error.to_string()),
                    });
                    continue;
                }
            }
        } else {
            None
        };

        let send_result = send_bundle_part(
            service,
            &client,
            &provider,
            &target,
            part,
            stable_client_id.as_deref(),
        )
        .await;
        let (status, message_id, request_id, error) = match send_result {
            Ok(result) => {
                if let Some(key) = part_key.as_deref() {
                    if let Err(error) = service
                        .outbox_store
                        .mark_sent(key, result.message_id.as_deref())
                    {
                        receipts.push(SendPartReceipt {
                            index,
                            requested_kind: requested_kind.to_string(),
                            delivered_kind,
                            status: "failed".to_string(),
                            message_id: result.message_id,
                            request_id: result.request_id,
                            warning,
                            error: Some(format!(
                                "provider acknowledged the part but outbox commit failed: {error}"
                            )),
                        });
                        continue;
                    }
                }
                ("success", result.message_id, result.request_id, None)
            }
            Err(error) => {
                if let Some(key) = part_key.as_deref() {
                    let _ = service.outbox_store.mark_pending(key, &error.to_string());
                }
                ("failed", None, None, Some(error.to_string()))
            }
        };

        let preview = bundle_part_preview(part);
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status: if status == "success" {
                MessageStatus::Success
            } else {
                MessageStatus::Failed
            },
            timestamp: now_ms(),
            target_id: Some(log_target_id.clone()),
            target_name: Some(log_target_name.clone()),
            message_id: message_id.clone(),
            msg_type: Some(delivered_kind.clone()),
            content: matches!(part, SendPartRequest::Text { .. }).then(|| preview.clone()),
            content_preview: Some(preview),
            trigger: Some(format!("api:{bundle_id}")),
            error: error.clone(),
            sender_open_id: None,
            event_id: None,
            reaction_added: None,
        };
        if let Err(log_error) = service.message_log_store.add(log) {
            error!(error = %log_error, "failed to store outbound bundle part log");
        }
        receipts.push(SendPartReceipt {
            index,
            requested_kind: requested_kind.to_string(),
            delivered_kind,
            status: status.to_string(),
            message_id,
            request_id,
            warning,
            error,
        });
    }

    let success_count = receipts
        .iter()
        .filter(|receipt| receipt.status == "success")
        .count();
    let status = if success_count == receipts.len() {
        "success"
    } else if success_count == 0 {
        "failed"
    } else {
        "partial_success"
    };
    let response = SendBundleResponse {
        bundle_id,
        provider_id: provider.id,
        destination,
        status: status.to_string(),
        receipts,
    };
    let http_status = match status {
        "success" => StatusCode::OK,
        "partial_success" => StatusCode::MULTI_STATUS,
        _ => StatusCode::BAD_GATEWAY,
    };
    json_response_with_status(http_status, &response)
}

pub(super) fn validate_send_part(part: &SendPartRequest) -> std::result::Result<(), String> {
    match part {
        SendPartRequest::Text { text } | SendPartRequest::Markdown { text } => {
            if text.trim().is_empty() {
                return Err("text must not be empty".to_string());
            }
        }
        SendPartRequest::Image { image_key } => {
            if image_key.trim().is_empty() {
                return Err("image_key must not be empty".to_string());
            }
        }
        SendPartRequest::File {
            file_key,
            file_name,
        } => {
            if file_key.trim().is_empty() {
                return Err("file_key must not be empty".to_string());
            }
            if file_name
                .as_deref()
                .is_some_and(|name| !is_safe_upload_file_name(name))
            {
                return Err("file_name must be a plain file name".to_string());
            }
        }
        SendPartRequest::NativeCard { card } => {
            if !card.is_object() {
                return Err("card must be a JSON object".to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn resolve_bundle_destination(
    service: &ImGatewayService,
    provider: &ImProviderConfig,
    body: &SendMessageRequest,
) -> std::result::Result<(ImTarget, String, String, String), (StatusCode, String)> {
    let destination = body.destination.clone().unwrap_or_else(|| {
        body.target_id
            .as_deref()
            .filter(|value| !matches!(*value, "__owner__" | "owner"))
            .map(|target_id| SendDestinationRequest::Target {
                target_id: target_id.to_string(),
            })
            .unwrap_or(SendDestinationRequest::Owner)
    });
    match destination {
        SendDestinationRequest::Owner => {
            let owner = provider
                .owner_open_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Provider '{}' has no owner_open_id", provider.id),
                    )
                })?;
            Ok((
                ImTarget {
                    id: "__owner__".to_string(),
                    provider_id: provider.id.clone(),
                    display_name: "Owner".to_string(),
                    receive_id_type: "open_id".to_string(),
                    receive_id: owner,
                    default_msg_type: "text".to_string(),
                    enabled: true,
                    created_at: 0,
                    updated_at: 0,
                },
                "__owner__".to_string(),
                "Owner".to_string(),
                "owner".to_string(),
            ))
        }
        SendDestinationRequest::Target { target_id } => {
            let target = service.target_store.get(&target_id).ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("Target '{target_id}' not found"),
                )
            })?;
            if target.provider_id != provider.id {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Target '{target_id}' does not belong to provider '{}'",
                        provider.id
                    ),
                ));
            }
            let display_name = target.display_name.clone();
            Ok((
                target,
                target_id.clone(),
                display_name,
                format!("target:{target_id}"),
            ))
        }
        SendDestinationRequest::Direct {
            receive_id_type,
            receive_id,
        } => {
            let receive_id_type = receive_id_type.trim().to_string();
            let capabilities = service
                .provider_client(provider)
                .send_capabilities(provider);
            if !capabilities
                .receive_id_types
                .iter()
                .any(|value| value == &receive_id_type)
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "receive_id_type '{}' is not supported by provider '{}'; supported: {}",
                        receive_id_type,
                        provider.id,
                        capabilities.receive_id_types.join(", ")
                    ),
                ));
            }
            let receive_id = receive_id.trim();
            if receive_id.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "receive_id must not be empty".to_string(),
                ));
            }
            let masked = if receive_id.chars().count() > 8 {
                format!("{}***", receive_id.chars().take(8).collect::<String>())
            } else {
                receive_id.to_string()
            };
            let summary = format!("direct:{receive_id_type}:{masked}");
            Ok((
                ImTarget {
                    id: summary.clone(),
                    provider_id: provider.id.clone(),
                    display_name: "Direct recipient".to_string(),
                    receive_id_type,
                    receive_id: receive_id.to_string(),
                    default_msg_type: "text".to_string(),
                    enabled: true,
                    created_at: 0,
                    updated_at: 0,
                },
                summary.clone(),
                "Direct recipient".to_string(),
                summary,
            ))
        }
    }
}

async fn send_bundle_part(
    service: &ImGatewayService,
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    target: &ImTarget,
    part: &SendPartRequest,
    stable_client_id: Option<&str>,
) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
    match part {
        SendPartRequest::Text { text } | SendPartRequest::Markdown { text } => {
            if provider.provider_type == crate::im_gateway::types::ImProviderType::Weixin {
                if let Some(client_id) = stable_client_id {
                    return service
                        .connection_manager
                        .weixin_provider()
                        .send_text_with_client_id(provider, target, text, client_id)
                        .await;
                }
            }
            client
                .send_text_with_uuid(provider, target, text, stable_client_id)
                .await
        }
        SendPartRequest::Image { image_key } => {
            client
                .send_image(provider, target, image_key, stable_client_id)
                .await
        }
        SendPartRequest::File { file_key, .. } => {
            client
                .send_file(provider, target, file_key, stable_client_id)
                .await
        }
        SendPartRequest::NativeCard { card } => {
            client
                .send_native_card(
                    provider,
                    target,
                    card.clone(),
                    crate::im_gateway::types::SendOptions {
                        uuid: stable_client_id.map(str::to_string),
                        msg_type: "interactive".to_string(),
                    },
                )
                .await
        }
    }
}

fn bundle_part_preview(part: &SendPartRequest) -> String {
    match part {
        SendPartRequest::Text { text } | SendPartRequest::Markdown { text } => {
            truncate_str(text, 200)
        }
        SendPartRequest::Image { .. } => "[image]".to_string(),
        SendPartRequest::File { file_name, .. } => file_name
            .as_deref()
            .map(|name| format!("[file:{name}]"))
            .unwrap_or_else(|| "[file]".to_string()),
        SendPartRequest::NativeCard { card } => card
            .get("header")
            .and_then(|header| header.get("title"))
            .and_then(|title| title.get("content"))
            .and_then(|content| content.as_str())
            .map(|title| truncate_str(title, 200))
            .unwrap_or_else(|| "[native_card]".to_string()),
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
        let owner_open_id = provider
            .owner_open_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
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
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    body: &SendMessageRequest,
    content: serde_json::Value,
) -> std::result::Result<serde_json::Value, (StatusCode, String)> {
    match body.msg_type.as_str() {
        "image" => {
            let image = body.image.clone().or_else(|| parse_image_content(&content));
            let image_key = resolve_image_key(client, provider, image.as_ref()).await?;
            Ok(serde_json::json!({ "image_key": image_key }))
        }
        "interactive" => {
            if let Some(rich_card) = &body.rich_card {
                build_rich_card_content(client, provider, rich_card).await
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
    client: &ImProviderClient,
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
    let uploaded = client
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
    client: &ImProviderClient,
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
        Some(resolve_image_key(client, provider, rich_card.image.as_ref()).await?)
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
        let rendered_text = if let Some(feishu) = client.feishu() {
            render_agent_markdown_for_feishu(
                &feishu,
                provider,
                text,
                provider_agent_work_dir(provider).as_deref(),
            )
            .await
        } else {
            text.to_string()
        };
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
