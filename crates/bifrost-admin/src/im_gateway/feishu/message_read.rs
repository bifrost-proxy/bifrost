use super::*;

#[derive(Clone, Debug)]
pub struct FeishuFetchedMessage {
    pub message_id: String,
    pub chat_id: String,
    pub sender_id: String,
    pub sender_type: Option<String>,
    pub msg_type: String,
    pub text: String,
    pub mentions: Vec<ImMention>,
    pub images: Vec<ImImageAttachment>,
    pub files: Vec<ImFileAttachment>,
    pub raw_content: serde_json::Value,
    pub create_time: Option<u64>,
    pub update_time: Option<u64>,
}

impl FeishuProvider {
    /// Fetch one referenced message from Feishu. This is intentionally the
    /// source of truth for group replies: multiple Bifrost providers may run on
    /// different machines and therefore cannot depend on shared process memory
    /// or a shared local database to understand another bot's output.
    pub async fn fetch_message(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
    ) -> Result<FeishuFetchedMessage> {
        let original = self
            .fetch_message_with_card_format(config, message_id, true)
            .await?;
        if original.msg_type == "interactive" && original.text.trim().is_empty() {
            return self
                .fetch_message_with_card_format(config, message_id, false)
                .await;
        }
        Ok(original)
    }

    async fn fetch_message_with_card_format(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        original_card_json: bool,
    ) -> Result<FeishuFetchedMessage> {
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Err(bifrost_core::BifrostError::Config(
                "Feishu referenced message_id is empty".to_string(),
            ));
        }
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let mut request = self
            .http
            .get(format!("{base_url}/im/v1/messages/{message_id}"))
            .header("Authorization", format!("Bearer {token}"))
            .query(&[("user_id_type", "open_id")]);
        if original_card_json {
            request = request.query(&[("card_msg_content_type", "user_card_content")]);
        }
        let response = request.send().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "fetch Feishu referenced message request failed: {error}"
            ))
        })?;
        let value: serde_json::Value = response.json().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "fetch Feishu referenced message response parse failed: {error}"
            ))
        })?;
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();
        if code != 0 {
            let message = value
                .get("msg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if is_message_read_permission_error(code, message) {
                return Err(bifrost_core::BifrostError::Config(
                    message_read_permission_help(config.app_id.as_deref()),
                ));
            }
            return Err(bifrost_core::BifrostError::Network(format!(
                "fetch Feishu referenced message failed: code={code}, msg={message}"
            )));
        }
        let item = value
            .get("data")
            .and_then(|data| data.get("items"))
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
            .ok_or_else(|| {
                bifrost_core::BifrostError::NotFound(format!(
                    "Feishu referenced message not found: {message_id}"
                ))
            })?;
        let raw_content = item
            .get("body")
            .and_then(|body| body.get("content"))
            .and_then(serde_json::Value::as_str)
            .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let msg_type = item
            .get("msg_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let mentions = parse_feishu_mentions(item.get("mentions"));
        let text = if msg_type == "interactive" {
            extract_card_text(&raw_content)
        } else {
            extract_feishu_message_text(&raw_content, &mentions)
        };
        let (images, files) = parse_feishu_message_attachments(&msg_type, &raw_content);
        Ok(FeishuFetchedMessage {
            message_id: item
                .get("message_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(message_id)
                .to_string(),
            chat_id: item
                .get("chat_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            sender_id: item
                .get("sender")
                .and_then(|sender| sender.get("id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            sender_type: item
                .get("sender")
                .and_then(|sender| sender.get("sender_type"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            msg_type,
            text,
            mentions,
            images,
            files,
            raw_content,
            create_time: item
                .get("create_time")
                .and_then(json_u64_from_string_or_number),
            update_time: item
                .get("update_time")
                .and_then(json_u64_from_string_or_number),
        })
    }
}

pub(super) fn is_message_read_permission_error(code: i64, message: &str) -> bool {
    matches!(code, 230027 | 99991672 | 99991679)
        || message.to_ascii_lowercase().contains("permission")
        || message.contains("权限")
        || message.contains("scope")
}

pub(super) fn message_read_permission_help(app_id: Option<&str>) -> String {
    let app = app_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("（App ID: `{value}`）"))
        .unwrap_or_default();
    format!(
        "当前飞书机器人{app}没有读取被引用群消息的权限。请在飞书开放平台进入该应用的「权限管理」，申请 `im:message:readonly`（获取单聊、群组消息）和 `im:message.group_msg`（获取群组中所有消息），然后创建并发布新版本使权限生效。权限生效后重新引用这条消息并 @ 机器人。"
    )
}

pub(super) fn extract_card_text(card: &serde_json::Value) -> String {
    const MAX_CARD_TEXT_CHARS: usize = 16_000;

    fn collect(value: &serde_json::Value, field: Option<&str>, parts: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                let tag = map
                    .get("tag")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if matches!(
                    tag,
                    "button"
                        | "action"
                        | "select_static"
                        | "multi_select_static"
                        | "overflow"
                        | "date_picker"
                        | "picker_time"
                        | "picker_datetime"
                ) {
                    return;
                }
                const VISUAL_KEYS: &[&str] = &[
                    "header", "title", "subtitle", "body", "elements", "summary", "content", "text",
                ];
                for key in VISUAL_KEYS {
                    if let Some(child) = map.get(*key) {
                        collect(child, Some(key), parts);
                    }
                }
                for (key, child) in map {
                    if VISUAL_KEYS.contains(&key.as_str())
                        || matches!(
                            key.as_str(),
                            "url"
                                | "multi_url"
                                | "behaviors"
                                | "value"
                                | "tag"
                                | "schema"
                                | "config"
                        )
                    {
                        continue;
                    }
                    collect(child, Some(key), parts);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect(item, field, parts);
                }
            }
            serde_json::Value::String(text)
                if matches!(field, Some("content" | "text" | "title" | "summary")) =>
            {
                let text = text.trim();
                if !text.is_empty() && parts.last().is_none_or(|last| last != text) {
                    parts.push(text.to_string());
                }
            }
            _ => {}
        }
    }

    let mut parts = Vec::new();
    collect(card, None, &mut parts);
    bifrost_core::text::truncate_chars(&parts.join("\n"), MAX_CARD_TEXT_CHARS)
}

#[cfg(test)]
mod tests;
