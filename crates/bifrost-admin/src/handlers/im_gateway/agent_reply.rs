use super::*;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AgentReplyImageCacheKey {
    provider_id: String,
    path: PathBuf,
    len: u64,
    modified_ms: u128,
}

/// Convert agent Markdown before it is placed into a Feishu card.
///
/// Local image references such as `![chart](./chart.png)` or `![chart](/tmp/chart.png)`
/// are uploaded once per provider/file fingerprint and rewritten to Feishu's
/// `![chart](image_key)` form. The later Markdown converter preserves image_key
/// references so Feishu cards can render them as inline images.
pub(super) async fn render_agent_markdown_for_feishu(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    markdown: &str,
    base_dir: Option<&Path>,
) -> String {
    if !markdown.contains("![") {
        return markdown.to_string();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            output.push_str(line);
            output.push('\n');
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            output.push_str(line);
            output.push('\n');
            continue;
        }

        output.push_str(
            &rewrite_agent_markdown_images_in_line(feishu, provider, line, base_dir).await,
        );
        output.push('\n');
    }

    if output.ends_with('\n') && !markdown.ends_with('\n') {
        output.pop();
    }
    output
}

pub(super) fn detect_markdown_code_fence(trimmed: &str) -> Option<String> {
    let fence_char = trimmed.chars().next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }
    let fence_len = trimmed.chars().take_while(|&c| c == fence_char).count();
    if fence_len < 3 {
        return None;
    }
    Some(trimmed[..fence_len].to_string())
}

pub(super) async fn rewrite_agent_markdown_images_in_line(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    line: &str,
    base_dir: Option<&Path>,
) -> String {
    if !line.contains("![") {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut pos = 0;
    while pos < line.len() {
        if line.as_bytes()[pos] == b'!' && pos + 1 < line.len() && line.as_bytes()[pos + 1] == b'['
        {
            if let Some((alt, url, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
            {
                if is_local_markdown_image_candidate(&url) {
                    if let Some(image_path) = resolve_agent_reply_image_path(&url, base_dir) {
                        match upload_agent_reply_image_cached(feishu, provider, &image_path).await {
                            Ok(image_key) => {
                                result.push_str(&format!("![{}]({})", alt, image_key));
                                pos = end;
                                continue;
                            }
                            Err(error) => {
                                warn!(
                                    provider_id = %provider.id,
                                    path = %image_path.display(),
                                    error = %error,
                                    "failed to upload local image referenced by agent markdown"
                                );
                            }
                        }
                    } else {
                        warn!(
                            provider_id = %provider.id,
                            image_url = %url,
                            "failed to resolve local image referenced by agent markdown"
                        );
                    }
                    result.push_str(&local_image_fallback_markdown(&alt, &url));
                    pos = end;
                    continue;
                }
            }
        }

        let ch = line[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }
    result
}

pub(super) fn local_image_fallback_markdown(alt: &str, _url: &str) -> String {
    let label = if alt.trim().is_empty() {
        "图片".to_string()
    } else {
        alt.trim().to_string()
    };
    format!("[{} 未能上传]", label)
}

pub(super) async fn upload_agent_reply_image_cached(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    image_path: &Path,
) -> bifrost_core::Result<String> {
    let metadata = tokio::fs::metadata(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to stat agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    if !metadata.is_file() {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image is not a file: {}",
            image_path.display()
        )));
    }
    if metadata.len() > MAX_AGENT_REPLY_IMAGE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image exceeds {} bytes: {}",
            MAX_AGENT_REPLY_IMAGE_BYTES,
            image_path.display()
        )));
    }

    let cache_key = AgentReplyImageCacheKey {
        provider_id: provider.id.clone(),
        path: image_path
            .canonicalize()
            .unwrap_or_else(|_| image_path.to_path_buf()),
        len: metadata.len(),
        modified_ms: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
    };

    if let Some(image_key) = agent_reply_image_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        return Ok(image_key);
    }

    let bytes = tokio::fs::read(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to read agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    let file_name = image_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-reply-image.png");
    let uploaded = feishu
        .upload_image(
            provider,
            "message",
            file_name,
            bytes,
            mime_type_for_image_path(image_path),
        )
        .await?;
    let image_key = uploaded.image_key;

    if let Ok(mut cache) = agent_reply_image_cache().lock() {
        cache.insert(cache_key, image_key.clone());
    }
    Ok(image_key)
}

pub(super) fn agent_reply_image_cache() -> &'static Mutex<HashMap<AgentReplyImageCacheKey, String>>
{
    AGENT_REPLY_IMAGE_UPLOAD_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn provider_agent_work_dir(provider: &ImProviderConfig) -> Option<PathBuf> {
    provider
        .agent_config
        .as_ref()
        .and_then(|config| config.work_dir.as_deref())
        .map(str::trim)
        .filter(|work_dir| !work_dir.is_empty())
        .map(PathBuf::from)
}

pub(super) fn resolve_agent_reply_image_path(
    raw_url: &str,
    base_dir: Option<&Path>,
) -> Option<PathBuf> {
    let destination = markdown_image_destination(raw_url);
    if !is_local_markdown_image_candidate(destination) {
        return None;
    }

    if let Some(path) = destination.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }

    let path = PathBuf::from(destination);
    if path.is_absolute() {
        Some(path)
    } else {
        base_dir.map(|base_dir| base_dir.join(path))
    }
}

pub(super) fn is_local_markdown_image_candidate(raw_url: &str) -> bool {
    let destination = markdown_image_destination(raw_url);
    !destination.is_empty()
        && !destination.starts_with("http://")
        && !destination.starts_with("https://")
        && !looks_like_feishu_image_key(destination)
}

pub(super) fn markdown_image_destination(raw_url: &str) -> &str {
    let trimmed = raw_url.trim();
    let trimmed = trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed);
    if let Some((path, _title)) = trimmed.split_once(" \"") {
        path.trim()
    } else if let Some((path, _title)) = trimmed.split_once(" '") {
        path.trim()
    } else {
        trimmed
    }
}

pub(super) fn looks_like_feishu_image_key(value: &str) -> bool {
    value.starts_with("img_")
}

pub(super) fn mime_type_for_image_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

/// Send an agent reply text via Feishu card and log the outbound message.
///
/// Extracted helper to share between the main turn loop and session-free command fast path.
pub(super) async fn send_agent_reply(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    send_agent_reply_with_title(client, provider, event, reply_text, message_log_store, None).await;
}

pub(super) fn agent_reply_target_id(
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> Option<String> {
    let target = match provider.provider_type {
        crate::im_gateway::types::ImProviderType::Weixin => event
            .source
            .chat_id
            .as_deref()
            .or(event.source.user_id.as_deref())
            .or(provider.owner_open_id.as_deref()),
        _ => provider
            .owner_open_id
            .as_deref()
            .or(event.source.user_id.as_deref())
            .or(event.source.chat_id.as_deref()),
    };
    target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn should_send_plain_im_task_start_notice(
    provider: &ImProviderConfig,
    progress_enabled: bool,
) -> bool {
    provider.provider_type == ImProviderType::Weixin && !progress_enabled
}

pub(super) fn build_agent_reply_target(
    provider: &ImProviderConfig,
    event: &ImEvent,
    id: &str,
    display_name: &str,
    default_msg_type: &str,
) -> Option<crate::im_gateway::types::ImTarget> {
    let receive_id = agent_reply_target_id(provider, event)?;
    Some(crate::im_gateway::types::ImTarget {
        id: id.to_string(),
        provider_id: provider.id.clone(),
        display_name: display_name.to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id,
        default_msg_type: default_msg_type.to_string(),
        created_at: 0,
        updated_at: 0,
    })
}

/// Send an agent reply with a custom card title.
pub(super) async fn send_agent_reply_with_title(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
    title: Option<&str>,
) {
    let Some(reply_target) = build_agent_reply_target(
        provider,
        event,
        "__agent_reply__",
        "Agent Reply",
        "interactive",
    ) else {
        error!("no target open_id to send agent reply");
        return;
    };

    let card_title = title.unwrap_or("Bifrost AI");
    let rendered_text = if let Some(feishu) = client.feishu() {
        render_agent_markdown_for_feishu(
            &feishu,
            provider,
            reply_text,
            provider_agent_work_dir(provider).as_deref(),
        )
        .await
    } else {
        reply_text.to_string()
    };
    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_text);
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": card_title
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": converted_text,
                    "element_id": "agent_reply"
                }
            ]
        }
    });

    let send_result = client
        .send_card(
            provider,
            &reply_target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

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
        target_id: Some(reply_target.receive_id.clone()),
        target_name: Some(reply_target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(reply_text, 200)),
        trigger: Some("agent".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply"),
    }
}

/// Send an agent reply with plan progress and tool calls panel (for goal continuation).
/// This sends a card similar to the final response rendering but can be called mid-continuation.
#[allow(clippy::too_many_arguments)]
pub(super) async fn send_agent_reply_with_plan(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    plan_steps: Option<&[PlanStep]>,
    tool_calls_log: &[ToolCallLog],
    title: Option<&str>,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let Some(reply_target) = build_agent_reply_target(
        provider,
        event,
        "__agent_reply__",
        "Agent Reply",
        "interactive",
    ) else {
        error!("no target open_id to send agent reply");
        return;
    };

    let rendered_text = if let Some(feishu) = client.feishu() {
        render_agent_markdown_for_feishu(
            &feishu,
            provider,
            reply_text,
            provider_agent_work_dir(provider).as_deref(),
        )
        .await
    } else {
        reply_text.to_string()
    };
    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_text);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": converted_text,
        "element_id": "agent_reply"
    })];

    // Add plan progress panel if present
    if let Some(steps) = plan_steps {
        let completed = steps
            .iter()
            .filter(|s| matches!(s.status, bifrost_agent::PlanStepStatus::Completed))
            .count();
        let total = steps.len();
        let mut plan_md = String::new();
        for s in steps {
            plan_md.push_str(&format!("{} {}\n", s.status.emoji(), s.step));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": true,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("📋 任务计划（{}/{}）", completed, total)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": plan_md
            }]
        }));
    }

    // Add tool calls panel if present
    if !tool_calls_log.is_empty() {
        let mut tool_md = String::new();
        for log in tool_calls_log {
            let icon = if log.success { "✅" } else { "❌" };
            tool_md.push_str(&format!("{} `{}`\n", icon, log.tool_name));
            let result_preview = truncate_str(&log.result, 500);
            tool_md.push_str(&format!("```\n{}\n```\n", result_preview));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": false,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("🔧 工具调用记录（{}次）", tool_calls_log.len())
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": tool_md
            }]
        }));
    }

    let card_title = title.unwrap_or("Bifrost AI");
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": card_title
            }
        },
        "body": {
            "elements": elements
        }
    });

    let send_result = client
        .send_card(
            provider,
            &reply_target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

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
        target_id: Some(reply_target.receive_id.clone()),
        target_name: Some(reply_target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(reply_text, 200)),
        trigger: Some("agent_continuation".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply with plan sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply with plan"),
    }
}

/// Best-effort helper to send an error notification card to the provider owner.
#[allow(dead_code)]
pub(super) async fn send_error_card_to_owner(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    error_message: &str,
) {
    let target_open_id = match provider.owner_open_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return,
    };

    let target = crate::im_gateway::types::ImTarget {
        id: "__error_notify__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Error Notify".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: target_open_id.to_string(),
        default_msg_type: "interactive".to_string(),
        created_at: 0,
        updated_at: 0,
    };

    let converted_error =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(error_message);
    let card = serde_json::json!({
        "schema": "2.0",
        "config": { "width_mode": "fill" },
        "header": {
            "template": "red",
            "title": { "tag": "plain_text", "content": "Bifrost Agent Error" }
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": converted_error
            }]
        }
    });

    if let Err(e) = feishu
        .send_card(
            provider,
            &target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
    {
        warn!(error = %e, "failed to send error card to owner");
    }
}

pub(super) async fn handle_provider_policy(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => {
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Return policy placeholder — policy store will be integrated in future
            json_response(&serde_json::json!({
                "provider_id": id,
                "permissions": [],
                "script_policy_binding": null,
            }))
        }
        Method::PATCH => {
            let _patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Policy update placeholder
            json_response(&serde_json::json!({"success": true}))
        }
        _ => method_not_allowed(),
    }
}

pub(super) async fn handle_provider_policy_bind_shell(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let _body: serde_json::Value = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(_provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    // Bind-shell placeholder
    json_response(&serde_json::json!({"success": true}))
}
