use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use bifrost_agent::tools::ToolHandler;
use bifrost_agent::types::ToolResult;
use serde::Deserialize;
use serde_json::Value;

use super::connection::ImConnectionManager;
use super::message_log_store::ImMessageLogStore;
use super::provider::ImProvider;
use super::provider_store::ImProviderStore;
use super::target_store::ImTargetStore;
use super::types::{
    ImEvent, ImMessageChannelBinding, ImMessageLog, ImProviderConfig, ImProviderType, ImTarget,
    MessageDirection, MessageStatus, MessageTargetMode, SendOptions,
};

#[derive(Clone, Debug, Default)]
pub struct SendMsgToolContext {
    pub message_channel: Option<ImMessageChannelBinding>,
}

pub struct SendMsgTool {
    provider_store: Arc<ImProviderStore>,
    target_store: Arc<ImTargetStore>,
    message_log_store: Arc<ImMessageLogStore>,
    connection_manager: Arc<ImConnectionManager>,
    context: SendMsgToolContext,
    description: String,
}

impl SendMsgTool {
    pub fn new(
        provider_store: Arc<ImProviderStore>,
        target_store: Arc<ImTargetStore>,
        message_log_store: Arc<ImMessageLogStore>,
        connection_manager: Arc<ImConnectionManager>,
        context: SendMsgToolContext,
    ) -> Self {
        let capability = context
            .message_channel
            .as_ref()
            .map(|channel| {
                format!(
                    "Default target: provider_id={}, target_mode={:?}, target_id={}.",
                    channel.provider_id, channel.target_mode, channel.target_id
                )
            })
            .unwrap_or_else(|| "No default IM target is available in this turn.".to_string());
        Self {
            provider_store,
            target_store,
            message_log_store,
            connection_manager,
            context,
            description: format!(
                "Send a message to the current IM channel or an explicit IM target. Use this for direct user-visible replies or notifications. {capability}"
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SendMsgArgs {
    provider_id: Option<String>,
    target_id: Option<String>,
    target_mode: Option<MessageTargetMode>,
    text: Option<String>,
    markdown: Option<String>,
    card: Option<Value>,
    image_key: Option<String>,
    image: Option<SendMsgImageArgs>,
    image_alt: Option<String>,
    card_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendMsgImageArgs {
    image_key: Option<String>,
    data_base64: Option<String>,
    file_name: Option<String>,
    mime_type: Option<String>,
    #[serde(default = "default_image_type")]
    image_type: String,
}

#[async_trait]
impl ToolHandler for SendMsgTool {
    fn name(&self) -> &str {
        "send_msg"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        let default_provider_type = self
            .context
            .message_channel
            .as_ref()
            .and_then(|channel| self.provider_store.get(&channel.provider_id))
            .map(|provider| provider.provider_type);
        let mut properties = serde_json::json!({
            "provider_id": {"type": "string", "description": "Optional IM provider id. Omit to use the injected default channel."},
            "target_id": {"type": "string", "description": "Optional target id or raw source id. Omit to use the injected default channel."},
            "target_mode": {"type": "string", "enum": ["source_thread", "source_user", "owner", "configured_target"], "description": "How target_id should be resolved. Defaults to configured_target when target_id is provided."},
            "text": {"type": "string", "description": "Plain text message body."},
            "markdown": {"type": "string", "description": "Markdown body. Channels without markdown support receive it as text."}
        });
        if default_provider_type != Some(ImProviderType::Weixin) {
            properties["card"] = serde_json::json!({
                "type": "object",
                "description": "Feishu interactive card JSON. If omitted on Feishu, markdown/image fields are wrapped into a JSON 2.0 card. Weixin degrades cards to text."
            });
            properties["card_title"] = serde_json::json!({
                "type": "string",
                "description": "Optional Feishu card header title when markdown or image_key is wrapped into a card."
            });
            properties["image_key"] = serde_json::json!({
                "type": "string",
                "description": "Feishu image_key to render as an image element inside the generated card."
            });
            properties["image_alt"] = serde_json::json!({
                "type": "string",
                "description": "Alt text for the generated Feishu card image element."
            });
            properties["image"] = serde_json::json!({
                "type": "object",
                "description": "Image payload for Feishu card generation. Use image_key or data_base64 plus optional file_name/mime_type.",
                "properties": {
                    "image_key": {"type": "string"},
                    "data_base64": {"type": "string"},
                    "file_name": {"type": "string"},
                    "mime_type": {"type": "string"},
                    "image_type": {"type": "string", "description": "Feishu upload image_type; defaults to message."}
                }
            });
        }
        serde_json::json!({
            "type": "object",
            "properties": properties
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args = match parse_args::<SendMsgArgs>(arguments) {
            Ok(args) => args,
            Err(error) => return error_tool_result(error),
        };
        let channel = match self.resolve_channel(&args) {
            Ok(channel) => channel,
            Err(error) => return error_tool_result(error),
        };
        let provider = match self.provider_store.get(&channel.provider_id) {
            Some(provider) => provider,
            None => {
                return error_tool_result(format!("provider '{}' not found", channel.provider_id))
            }
        };
        let target = match self.resolve_target(&provider, &channel) {
            Ok(target) => target,
            Err(error) => return error_tool_result(error),
        };

        let prepared = match self.prepare_send_payload(&provider, args).await {
            Ok(prepared) => prepared,
            Err(error) => return error_tool_result(error),
        };
        let msg_type = prepared.msg_type.clone();
        let content_preview = prepared.content_preview.clone();
        let send_result = match prepared.payload {
            PreparedSendPayloadKind::Text(text) => self.send_text(&provider, &target, &text).await,
            PreparedSendPayloadKind::Card(card) => self.send_card(&provider, &target, card).await,
        };

        let (success, message_id, error) = match send_result {
            Ok(result) => (true, result.message_id, None),
            Err(error) => (false, None, Some(error.to_string())),
        };
        let status = if success {
            MessageStatus::Success
        } else {
            MessageStatus::Failed
        };
        let _ = self.message_log_store.add(ImMessageLog {
            id: uuid::Uuid::new_v4().as_simple().to_string(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status,
            timestamp: now_ms(),
            target_id: Some(target.id.clone()),
            target_name: Some(target.display_name.clone()),
            message_id: message_id.clone(),
            msg_type: Some(msg_type.clone()),
            content_preview: Some(content_preview),
            trigger: Some("agent_tool:send_msg".to_string()),
            error: error.clone(),
            sender_open_id: None,
            event_id: None,
            reaction_added: None,
        });

        if success {
            json_tool_result(&serde_json::json!({
                "success": true,
                "provider_id": provider.id,
                "target_id": target.id,
                "msg_type": msg_type,
                "message_id": message_id
            }))
        } else {
            error_tool_result(error.unwrap_or_else(|| "send_msg failed".to_string()))
        }
    }
}

impl SendMsgTool {
    async fn prepare_send_payload(
        &self,
        provider: &ImProviderConfig,
        args: SendMsgArgs,
    ) -> Result<PreparedSendPayload, String> {
        let markdown = trimmed(args.markdown);
        let text = trimmed(args.text);
        let has_image = args
            .image_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
            || args.image.is_some();

        if let Some(card) = args.card {
            return Ok(PreparedSendPayload::card(card, "[interactive card]"));
        }

        if provider.provider_type == ImProviderType::Feishu && (markdown.is_some() || has_image) {
            let card_markdown =
                markdown
                    .as_deref()
                    .or(if has_image { text.as_deref() } else { None });
            let card = self
                .build_feishu_card(
                    provider,
                    args.card_title.as_deref(),
                    card_markdown,
                    args.image_key.as_deref(),
                    args.image.as_ref(),
                    args.image_alt.as_deref(),
                )
                .await?;
            let preview = markdown
                .as_deref()
                .or(text.as_deref())
                .map(|value| truncate_chars(value, 200))
                .unwrap_or_else(|| "[interactive card image]".to_string());
            return Ok(PreparedSendPayload::card(card, preview));
        }

        let Some(text) = text.or(markdown) else {
            return Err("text, markdown, card, or image_key is required".to_string());
        };
        let preview = truncate_chars(&text, 200);
        Ok(PreparedSendPayload::text(text, preview))
    }

    async fn build_feishu_card(
        &self,
        provider: &ImProviderConfig,
        title: Option<&str>,
        markdown: Option<&str>,
        image_key: Option<&str>,
        image: Option<&SendMsgImageArgs>,
        image_alt: Option<&str>,
    ) -> Result<Value, String> {
        let title = title
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Bifrost AI");
        let mut elements = Vec::new();
        if let Some(image_key) = self
            .resolve_card_image_key(provider, image_key, image)
            .await?
        {
            let alt = image_alt
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(title);
            elements.push(serde_json::json!({
                "tag": "img",
                "img_key": image_key,
                "alt": {
                    "tag": "plain_text",
                    "content": alt
                }
            }));
        }
        if let Some(markdown) = markdown.map(str::trim).filter(|value| !value.is_empty()) {
            let markdown =
                crate::im_gateway::markdown_converter::convert_to_feishu_markdown(markdown);
            elements.push(serde_json::json!({
                "tag": "markdown",
                "content": markdown,
                "element_id": "send_msg_markdown"
            }));
        }
        if elements.is_empty() {
            return Err("Feishu card requires markdown, image_key, or image data".to_string());
        }
        Ok(serde_json::json!({
            "schema": "2.0",
            "config": {
                "width_mode": "fill",
                "update_multi": true
            },
            "header": {
                "template": "blue",
                "title": {
                    "tag": "plain_text",
                    "content": title
                }
            },
            "body": {
                "elements": elements
            }
        }))
    }

    async fn resolve_card_image_key(
        &self,
        provider: &ImProviderConfig,
        image_key: Option<&str>,
        image: Option<&SendMsgImageArgs>,
    ) -> Result<Option<String>, String> {
        if let Some(image_key) = image_key.map(str::trim).filter(|value| !value.is_empty()) {
            return Ok(Some(image_key.to_string()));
        }
        let Some(image) = image else {
            return Ok(None);
        };
        if let Some(image_key) = image
            .image_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(image_key.to_string()));
        }
        let data_base64 = image
            .data_base64
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "image_key or image.data_base64 is required for Feishu card image".to_string()
            })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .map_err(|error| format!("invalid image.data_base64: {error}"))?;
        if bytes.is_empty() {
            return Err("image.data_base64 decoded to empty bytes".to_string());
        }
        let uploaded = self
            .connection_manager
            .feishu_provider()
            .upload_image(
                provider,
                &image.image_type,
                image.file_name.as_deref().unwrap_or("send-msg-image"),
                bytes,
                image.mime_type.as_deref(),
            )
            .await
            .map_err(|error| format!("failed to upload Feishu card image: {error}"))?;
        Ok(Some(uploaded.image_key))
    }

    fn resolve_channel(&self, args: &SendMsgArgs) -> Result<ImMessageChannelBinding, String> {
        if args.provider_id.is_none() && args.target_id.is_none() && args.target_mode.is_none() {
            return self
                .context
                .message_channel
                .clone()
                .ok_or_else(|| "send_msg has no default IM channel in this turn".to_string());
        }

        let target_mode = args
            .target_mode
            .clone()
            .unwrap_or(MessageTargetMode::ConfiguredTarget);
        let target_id = args
            .target_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.context
                    .message_channel
                    .as_ref()
                    .map(|channel| channel.target_id.clone())
            })
            .ok_or_else(|| "target_id is required when no default IM channel exists".to_string())?;
        let provider_id = args
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.target_store
                    .get(&target_id)
                    .map(|target| target.provider_id)
            })
            .or_else(|| {
                self.context
                    .message_channel
                    .as_ref()
                    .map(|channel| channel.provider_id.clone())
            })
            .ok_or_else(|| "provider_id is required for this target".to_string())?;
        Ok(ImMessageChannelBinding {
            provider_id,
            target_id,
            target_mode,
        })
    }

    fn resolve_target(
        &self,
        provider: &ImProviderConfig,
        channel: &ImMessageChannelBinding,
    ) -> Result<ImTarget, String> {
        match channel.target_mode {
            MessageTargetMode::ConfiguredTarget => {
                if channel.target_id == "__owner__" || channel.target_id == "owner" {
                    return self.owner_target(provider);
                }
                let Some(target) = self.target_store.get(&channel.target_id) else {
                    return Err(format!("target '{}' not found", channel.target_id));
                };
                if target.provider_id != provider.id {
                    return Err(format!(
                        "target '{}' belongs to provider '{}', not '{}'",
                        target.id, target.provider_id, provider.id
                    ));
                }
                Ok(target)
            }
            MessageTargetMode::Owner => self.owner_target(provider),
            MessageTargetMode::SourceThread => Ok(transient_target(
                provider,
                "__source_thread__",
                "Source Thread",
                source_receive_id_type(provider, MessageTargetMode::SourceThread),
                &channel.target_id,
            )),
            MessageTargetMode::SourceUser => Ok(transient_target(
                provider,
                "__source_user__",
                "Source User",
                source_receive_id_type(provider, MessageTargetMode::SourceUser),
                &channel.target_id,
            )),
        }
    }

    fn owner_target(&self, provider: &ImProviderConfig) -> Result<ImTarget, String> {
        let owner_id = provider
            .owner_open_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("provider '{}' has no owner_open_id", provider.id))?;
        Ok(transient_target(
            provider,
            "__owner__",
            "Owner",
            source_receive_id_type(provider, MessageTargetMode::Owner),
            owner_id,
        ))
    }

    async fn send_text(
        &self,
        provider: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> bifrost_core::Result<super::types::SendResult> {
        match provider.provider_type {
            ImProviderType::Weixin => {
                self.connection_manager
                    .weixin_provider()
                    .send_text(provider, target, text)
                    .await
            }
            _ => {
                self.connection_manager
                    .feishu_provider()
                    .send_text(provider, target, text)
                    .await
            }
        }
    }

    async fn send_card(
        &self,
        provider: &ImProviderConfig,
        target: &ImTarget,
        card: Value,
    ) -> bifrost_core::Result<super::types::SendResult> {
        match provider.provider_type {
            ImProviderType::Weixin => {
                self.connection_manager
                    .weixin_provider()
                    .send_card(provider, target, card, SendOptions::default())
                    .await
            }
            _ => {
                self.connection_manager
                    .feishu_provider()
                    .send_card(provider, target, card, SendOptions::default())
                    .await
            }
        }
    }
}

struct PreparedSendPayload {
    payload: PreparedSendPayloadKind,
    msg_type: String,
    content_preview: String,
}

impl PreparedSendPayload {
    fn text(text: String, content_preview: String) -> Self {
        Self {
            payload: PreparedSendPayloadKind::Text(text),
            msg_type: "text".to_string(),
            content_preview,
        }
    }

    fn card(card: Value, content_preview: impl Into<String>) -> Self {
        Self {
            payload: PreparedSendPayloadKind::Card(card),
            msg_type: "interactive".to_string(),
            content_preview: content_preview.into(),
        }
    }
}

enum PreparedSendPayloadKind {
    Text(String),
    Card(Value),
}

fn transient_target(
    provider: &ImProviderConfig,
    id: &str,
    display_name: &str,
    receive_id_type: &str,
    receive_id: &str,
) -> ImTarget {
    ImTarget {
        id: id.to_string(),
        provider_id: provider.id.clone(),
        display_name: display_name.to_string(),
        receive_id_type: receive_id_type.to_string(),
        receive_id: receive_id.to_string(),
        default_msg_type: "text".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

fn source_receive_id_type(provider: &ImProviderConfig, mode: MessageTargetMode) -> &'static str {
    match (provider.provider_type, mode) {
        (ImProviderType::Feishu, MessageTargetMode::SourceThread) => "chat_id",
        _ => "open_id",
    }
}

pub fn message_channel_from_event(
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> Option<ImMessageChannelBinding> {
    if let Some(chat_id) = event
        .source
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: chat_id.to_string(),
            target_mode: MessageTargetMode::SourceThread,
        });
    }
    if let Some(user_id) = event
        .source
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: user_id.to_string(),
            target_mode: MessageTargetMode::SourceUser,
        });
    }
    provider
        .owner_open_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|owner_id| ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: owner_id.to_string(),
            target_mode: MessageTargetMode::Owner,
        })
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T, String> {
    if arguments.trim().is_empty() {
        serde_json::from_str("{}").map_err(|error| format!("invalid arguments: {error}"))
    } else {
        serde_json::from_str(arguments).map_err(|error| format!("invalid arguments: {error}"))
    }
}

fn json_tool_result(value: &Value) -> ToolResult {
    ToolResult {
        success: true,
        output: serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()),
        runtime_events: Vec::new(),
    }
}

fn error_tool_result(error: String) -> ToolResult {
    ToolResult {
        success: false,
        output: error,
        runtime_events: Vec::new(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_image_type() -> String {
    "message".to_string()
}

#[cfg(test)]
mod tests {
    use bifrost_agent::tools::ToolHandler;

    use super::*;

    #[test]
    fn send_msg_schema_uses_plain_object_for_model_compatibility() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let provider_store = Arc::new(ImProviderStore::new(temp_dir.path()));
        provider_store
            .add(ImProviderConfig {
                id: "bifrost".to_string(),
                provider_type: ImProviderType::Feishu,
                display_name: "Bifrost".to_string(),
                enabled: true,
                base_url: None,
                app_id: None,
                secret_ref: None,
                owner_open_id: Some("owner".to_string()),
                event_connection_enabled: false,
                event_types: Vec::new(),
                agent_config: None,
                created_at: 0,
                updated_at: 0,
            })
            .expect("add provider");
        let tool = SendMsgTool::new(
            provider_store,
            Arc::new(ImTargetStore::new(temp_dir.path())),
            Arc::new(ImMessageLogStore::new(temp_dir.path())),
            Arc::new(ImConnectionManager::new()),
            SendMsgToolContext {
                message_channel: Some(ImMessageChannelBinding {
                    provider_id: "bifrost".to_string(),
                    target_id: "owner".to_string(),
                    target_mode: MessageTargetMode::Owner,
                }),
            },
        );

        let schema = tool.parameters_schema();
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("allOf").is_none());
        assert!(schema.get("not").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn feishu_markdown_send_msg_builds_interactive_card() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let provider_store = Arc::new(ImProviderStore::new(temp_dir.path()));
        let provider = ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: None,
            secret_ref: None,
            owner_open_id: Some("owner".to_string()),
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        provider_store.add(provider.clone()).expect("add provider");
        let tool = SendMsgTool::new(
            provider_store,
            Arc::new(ImTargetStore::new(temp_dir.path())),
            Arc::new(ImMessageLogStore::new(temp_dir.path())),
            Arc::new(ImConnectionManager::new()),
            SendMsgToolContext::default(),
        );

        let payload = tool
            .prepare_send_payload(
                &provider,
                SendMsgArgs {
                    provider_id: None,
                    target_id: None,
                    target_mode: None,
                    text: None,
                    markdown: Some("**hello**".to_string()),
                    card: None,
                    image_key: None,
                    image: None,
                    image_alt: None,
                    card_title: Some("Report".to_string()),
                },
            )
            .await
            .expect("payload");

        assert_eq!(payload.msg_type, "interactive");
        let PreparedSendPayloadKind::Card(card) = payload.payload else {
            panic!("expected card payload");
        };
        assert_eq!(card["schema"], "2.0");
        assert_eq!(card["header"]["title"]["content"], "Report");
        assert_eq!(card["body"]["elements"][0]["tag"], "markdown");
        assert_eq!(card["body"]["elements"][0]["content"], "**hello**");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn feishu_image_key_send_msg_builds_card_image_element() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let provider = ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: None,
            secret_ref: None,
            owner_open_id: Some("owner".to_string()),
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        let tool = SendMsgTool::new(
            Arc::new(ImProviderStore::new(temp_dir.path())),
            Arc::new(ImTargetStore::new(temp_dir.path())),
            Arc::new(ImMessageLogStore::new(temp_dir.path())),
            Arc::new(ImConnectionManager::new()),
            SendMsgToolContext::default(),
        );

        let payload = tool
            .prepare_send_payload(
                &provider,
                SendMsgArgs {
                    provider_id: None,
                    target_id: None,
                    target_mode: None,
                    text: None,
                    markdown: Some("caption".to_string()),
                    card: None,
                    image_key: Some("img_v3_chart".to_string()),
                    image: None,
                    image_alt: Some("Chart".to_string()),
                    card_title: None,
                },
            )
            .await
            .expect("payload");

        let PreparedSendPayloadKind::Card(card) = payload.payload else {
            panic!("expected card payload");
        };
        assert_eq!(card["body"]["elements"][0]["tag"], "img");
        assert_eq!(card["body"]["elements"][0]["img_key"], "img_v3_chart");
        assert_eq!(card["body"]["elements"][0]["alt"]["content"], "Chart");
        assert_eq!(card["body"]["elements"][1]["tag"], "markdown");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn feishu_text_with_image_key_keeps_text_in_generated_card() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let provider = ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: None,
            secret_ref: None,
            owner_open_id: Some("owner".to_string()),
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        let tool = SendMsgTool::new(
            Arc::new(ImProviderStore::new(temp_dir.path())),
            Arc::new(ImTargetStore::new(temp_dir.path())),
            Arc::new(ImMessageLogStore::new(temp_dir.path())),
            Arc::new(ImConnectionManager::new()),
            SendMsgToolContext::default(),
        );

        let payload = tool
            .prepare_send_payload(
                &provider,
                SendMsgArgs {
                    provider_id: None,
                    target_id: None,
                    target_mode: None,
                    text: Some("plain caption".to_string()),
                    markdown: None,
                    card: None,
                    image_key: Some("img_v3_chart".to_string()),
                    image: None,
                    image_alt: None,
                    card_title: None,
                },
            )
            .await
            .expect("payload");

        let PreparedSendPayloadKind::Card(card) = payload.payload else {
            panic!("expected card payload");
        };
        assert_eq!(card["body"]["elements"][0]["tag"], "img");
        assert_eq!(card["body"]["elements"][1]["tag"], "markdown");
        assert_eq!(card["body"]["elements"][1]["content"], "plain caption");
    }
}
