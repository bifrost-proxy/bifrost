use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use prost::Message as ProstMessage;
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, error, info, warn};

use bifrost_core::Result;

use crate::im_gateway::provider::{EventSink, ImProvider};
use crate::im_gateway::types::{
    ConnectionHandle, ConnectionState, ImChannelCapabilities, ImConversationCapabilities, ImEvent,
    ImEventMessage, ImEventSource, ImFileAttachment, ImImageAttachment, ImImageSource,
    ImInteractionCapabilities, ImMention, ImProgressPresentation, ImProviderConfig, ImProviderType,
    ImSendCapabilities, ImSendPartCapability, ImSendSupportLevel, ImTarget, ProviderValidation,
    SendOptions, SendResult, UploadedImage,
};

#[path = "feishu_card_action.rs"]
pub(crate) mod card_action;

mod message_read;
pub use message_read::FeishuFetchedMessage;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_BASE_URL: &str = "https://open.feishu.cn/open-apis";
const TOKEN_REFRESH_AHEAD_SECS: u64 = 300; // 5 minutes before expiry
const MAX_BACKOFF_SECS: u64 = 60;
const INITIAL_BACKOFF_SECS: u64 = 1;
const MAX_MESSAGE_IMAGE_RESOURCE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_MESSAGE_FILE_RESOURCE_BYTES: u64 = 100 * 1024 * 1024;
pub(crate) const MAX_OUTBOUND_FILE_BYTES: u64 = 100 * 1024 * 1024;
const WS_PING_INTERVAL_SECS: u64 = 90;
/// Maximum time we're willing to wait for *any* server-originated traffic
/// (event / pong / server-initiated ping) before considering the connection
/// silently dead and forcing a reconnect. Must be strictly larger than the
/// server-advertised ping interval so a single dropped pong doesn't trip it.
const WS_SERVER_SILENCE_TIMEOUT_SECS: u64 = 180;
/// How often the silence watchdog wakes up to re-check `last_server_msg_at`.
const WS_SILENCE_CHECK_INTERVAL_SECS: u64 = 15;

#[derive(Clone, Copy)]
struct FeishuReplyOptions<'a> {
    uuid: Option<&'a str>,
    reply_in_thread: bool,
}

// ---------------------------------------------------------------------------
// Protobuf Frame types for Feishu WebSocket binary protocol
// ---------------------------------------------------------------------------

/// Protobuf Frame header (key-value pair)
#[derive(Clone, prost::Message)]
struct PbHeader {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

/// Protobuf Frame for Feishu WebSocket binary protocol
#[derive(Clone, prost::Message)]
struct PbFrame {
    #[prost(uint64, tag = "1")]
    seq_id: u64,
    #[prost(uint64, tag = "2")]
    log_id: u64,
    #[prost(int32, tag = "3")]
    service: i32,
    #[prost(int32, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<PbHeader>,
    #[prost(string, optional, tag = "6")]
    payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    log_id_new: Option<String>,
}

/// WS endpoint result containing the WebSocket URL, service_id and ping interval.
struct WsEndpointResult {
    url: String,
    service_id: i32,
    ping_interval_secs: u64,
}

#[derive(Debug)]
pub(crate) struct FeishuConnectionStatusEvent {
    pub state: ConnectionState,
    pub error: Option<String>,
}

pub(crate) type FeishuConnectionStatusTx =
    tokio::sync::mpsc::UnboundedSender<FeishuConnectionStatusEvent>;

// ---------------------------------------------------------------------------
// Protobuf frame helpers
// ---------------------------------------------------------------------------

fn get_header_value<'a>(headers: &'a [PbHeader], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.key == key)
        .map(|h| h.value.as_str())
}

fn build_ping_frame(service_id: i32) -> Vec<u8> {
    let frame = PbFrame {
        seq_id: 0,
        log_id: 0,
        service: service_id,
        method: 0, // CONTROL
        headers: vec![PbHeader {
            key: "type".to_string(),
            value: "ping".to_string(),
        }],
        payload_encoding: None,
        payload_type: None,
        payload: None,
        log_id_new: None,
    };
    frame.encode_to_vec()
}

fn build_response_frame(original: &PbFrame, success: bool) -> Vec<u8> {
    let code = if success { 200 } else { 500 };
    let resp_json = serde_json::json!({"code": code}).to_string();

    let mut resp_frame = original.clone();
    resp_frame.payload = Some(resp_json.into_bytes());
    resp_frame.encode_to_vec()
}

// ---------------------------------------------------------------------------
// Token Cache
// ---------------------------------------------------------------------------

struct TokenCache {
    token: String,
    expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeishuBotIdentity {
    pub open_id: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeishuCreatedChat {
    pub chat_id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeishuScopeGrantStatus {
    Granted,
    Missing,
}

#[derive(Debug)]
struct ScopeGrantPage {
    granted: bool,
    has_more: bool,
    page_token: Option<String>,
}

fn parse_scope_grant_page(
    value: &serde_json::Value,
    required_scope: &str,
) -> std::result::Result<ScopeGrantPage, String> {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Err(format!(
            "query Feishu application scopes failed: code={code}, msg={message}"
        ));
    }
    let data = value
        .get("data")
        .ok_or_else(|| "Feishu application scopes response missing data".to_string())?;
    let scopes = data
        .get("scopes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Feishu application scopes response missing data.scopes".to_string())?;
    Ok(ScopeGrantPage {
        granted: scopes.iter().any(|scope| {
            scope.get("scope_name").and_then(serde_json::Value::as_str) == Some(required_scope)
                && scope
                    .get("grant_status")
                    .and_then(serde_json::Value::as_i64)
                    == Some(1)
        }),
        has_more: data
            .get("has_more")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        page_token: data
            .get("page_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TokenCacheKey {
    base_url: String,
    app_id: String,
    secret_digest: String,
}

// ---------------------------------------------------------------------------
// Feishu Provider
// ---------------------------------------------------------------------------

pub struct FeishuProvider {
    http: reqwest::Client,
    token_cache: RwLock<HashMap<TokenCacheKey, TokenCache>>,
    bot_identity_cache: RwLock<HashMap<TokenCacheKey, FeishuBotIdentity>>,
}

impl Default for FeishuProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FeishuProvider {
    pub fn new() -> Self {
        let http = build_feishu_http_client();

        Self {
            http,
            token_cache: RwLock::new(HashMap::new()),
            bot_identity_cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_group_chat(
        &self,
        config: &ImProviderConfig,
        name: &str,
        owner_open_id: &str,
        uuid: &str,
    ) -> Result<FeishuCreatedChat> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!(
            "{base_url}/im/v1/chats?user_id_type=open_id&set_bot_manager=true&uuid={}",
            urlencoding::encode(uuid)
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "name": name,
                "owner_id": owner_open_id,
                "chat_mode": "group",
                "chat_type": "private"
            }))
            .send()
            .await
            .map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "Feishu create group request failed: {error}"
                ))
            })?;
        let request_id = response
            .headers()
            .get("x-tt-logid")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let value: serde_json::Value = response.json().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "Feishu create group response parse failed: {error}"
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
                .unwrap_or("unknown error");
            let request_suffix = request_id
                .as_deref()
                .map(|id| format!(", request_id={id}"))
                .unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "Feishu create group failed: code={code}, msg={message}{request_suffix}"
            )));
        }
        let data = value.get("data").unwrap_or(&value);
        let chat_id = data
            .get("chat_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "Feishu create group response missing chat_id".to_string(),
                )
            })?
            .to_string();
        let response_name = data
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(name)
            .to_string();
        Ok(FeishuCreatedChat {
            chat_id,
            name: response_name,
        })
    }

    /// Get the base URL from config, falling back to the default Feishu API URL.
    fn base_url(config: &ImProviderConfig) -> &str {
        config
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
    }

    fn token_cache_key(base_url: &str, app_id: &str, app_secret: &str) -> TokenCacheKey {
        let secret_digest = digest(&SHA256, app_secret.as_bytes());

        TokenCacheKey {
            base_url: base_url.to_string(),
            app_id: app_id.to_string(),
            secret_digest: hex_encode(secret_digest.as_ref()),
        }
    }

    /// Get tenant_access_token with caching and early refresh.
    pub async fn get_tenant_token(
        &self,
        config: &ImProviderConfig,
        app_secret: &str,
    ) -> Result<String> {
        let now = current_timestamp_secs();
        let base_url = Self::base_url(config);
        let app_id = config.app_id.as_deref().unwrap_or_default();
        let cache_key = Self::token_cache_key(base_url, app_id, app_secret);

        // Check cache
        {
            let cache = self.token_cache.read();
            if let Some(c) = cache.get(&cache_key) {
                if now + TOKEN_REFRESH_AHEAD_SECS < c.expires_at {
                    return Ok(c.token.clone());
                }
            }
        }

        // Refresh needed
        let (token, expire) = self.refresh_token(base_url, app_id, app_secret).await?;

        let expires_at = now + expire;
        let result = token.clone();

        let mut cache = self.token_cache.write();
        cache.insert(cache_key, TokenCache { token, expires_at });

        Ok(result)
    }

    /// Query the application's tenant-level grant status for one permission.
    /// The application scopes endpoint itself does not require an extra scope.
    pub async fn scope_grant_status(
        &self,
        config: &ImProviderConfig,
        required_scope: &str,
    ) -> Result<FeishuScopeGrantStatus> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let mut page_token: Option<String> = None;
        loop {
            let mut request = self
                .http
                .get(format!("{base_url}/application/v6/scopes"))
                .header("Authorization", format!("Bearer {token}"))
                .query(&[("page_size", "100")]);
            if let Some(token) = page_token.as_deref() {
                request = request.query(&[("page_token", token)]);
            }
            let response = request.send().await.map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "query Feishu application scopes failed: {error}"
                ))
            })?;
            let status = response.status();
            let value: serde_json::Value = response.json().await.map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "parse Feishu application scopes response failed: {error}"
                ))
            })?;
            if !status.is_success() {
                return Err(bifrost_core::BifrostError::Network(format!(
                    "query Feishu application scopes failed: status={status}"
                )));
            }
            let page = parse_scope_grant_page(&value, required_scope)
                .map_err(bifrost_core::BifrostError::Network)?;
            if page.granted {
                return Ok(FeishuScopeGrantStatus::Granted);
            }
            if !page.has_more {
                return Ok(FeishuScopeGrantStatus::Missing);
            }
            page_token = Some(page.page_token.ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "Feishu application scopes response has_more without page_token".to_string(),
                )
            })?);
        }
    }

    /// Refresh token from Feishu API.
    async fn refresh_token(
        &self,
        base_url: &str,
        app_id: &str,
        app_secret: &str,
    ) -> Result<(String, u64)> {
        let url = format!("{}/auth/v3/tenant_access_token/internal", base_url);

        #[derive(Serialize)]
        struct TokenRequest<'a> {
            app_id: &'a str,
            app_secret: &'a str,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            tenant_access_token: Option<String>,
            expire: Option<u64>,
            code: Option<i64>,
            msg: Option<String>,
        }

        debug!(app_id = app_id, "refreshing feishu tenant access token");

        let resp = self
            .http
            .post(&url)
            .json(&TokenRequest { app_id, app_secret })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu token request failed: {}",
                    reqwest_error_with_sources(e)
                ))
            })?;

        let status = resp.status();
        let body: TokenResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu token response parse failed: {}",
                e
            ))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "feishu token error: code={}, msg={}, status={}",
                    code, msg, status
                )));
            }
        }

        let token = body.tenant_access_token.ok_or_else(|| {
            bifrost_core::BifrostError::Network(
                "feishu token response missing tenant_access_token".to_string(),
            )
        })?;

        let expire = body.expire.unwrap_or(7200);

        info!(
            "feishu tenant access token refreshed, expires in {}s",
            expire
        );

        Ok((token, expire))
    }

    /// Internal implementation for sending messages.
    #[allow(clippy::too_many_arguments)]
    async fn send_message_internal(
        &self,
        base_url: &str,
        token: &str,
        receive_id_type: &str,
        receive_id: &str,
        msg_type: &str,
        content: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let url = format!(
            "{}/im/v1/messages?receive_id_type={}",
            base_url, receive_id_type
        );

        #[derive(Serialize)]
        struct SendRequest<'a> {
            receive_id: &'a str,
            msg_type: &'a str,
            content: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            uuid: Option<&'a str>,
        }

        #[derive(Deserialize)]
        struct SendResponse {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<SendResponseData>,
        }

        #[derive(Deserialize)]
        struct SendResponseData {
            message_id: Option<String>,
        }

        debug!(
            receive_id_type = receive_id_type,
            receive_id = receive_id,
            msg_type = msg_type,
            "sending feishu message"
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&SendRequest {
                receive_id,
                msg_type,
                content,
                uuid,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("feishu send request failed: {}", e))
            })?;

        let request_id = resp
            .headers()
            .get("x-tt-logid")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body: SendResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!("feishu send response parse failed: {}", e))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "feishu send error: code={}, msg={}",
                    code, msg
                )));
            }
        }

        let message_id = body.data.and_then(|d| d.message_id);

        Ok(SendResult {
            message_id,
            request_id,
        })
    }

    /// Reply to an existing Feishu message. Feishu renders this relationship as
    /// a compact native quote above the reply instead of requiring a card title.
    async fn reply_message_internal(
        &self,
        base_url: &str,
        token: &str,
        message_id: &str,
        msg_type: &str,
        content: &str,
        options: FeishuReplyOptions<'_>,
    ) -> Result<SendResult> {
        let url = format!("{}/im/v1/messages/{}/reply", base_url, message_id);

        #[derive(Deserialize)]
        struct ReplyResponse {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<ReplyResponseData>,
        }

        #[derive(Deserialize)]
        struct ReplyResponseData {
            message_id: Option<String>,
        }

        debug!(message_id, msg_type, "replying to feishu message");
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&build_reply_request(
                msg_type,
                content,
                options.uuid,
                options.reply_in_thread,
            ))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("feishu reply request failed: {}", e))
            })?;

        let request_id = resp
            .headers()
            .get("x-tt-logid")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body: ReplyResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu reply response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu reply error: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }

        Ok(SendResult {
            message_id: body.data.and_then(|data| data.message_id),
            request_id,
        })
    }
}

fn build_reply_request(
    msg_type: &str,
    content: &str,
    uuid: Option<&str>,
    reply_in_thread: bool,
) -> serde_json::Value {
    let mut request = serde_json::json!({
        "msg_type": msg_type,
        "content": content,
        "reply_in_thread": reply_in_thread,
    });
    if let Some(uuid) = uuid {
        request["uuid"] = serde_json::json!(uuid);
    }
    request
}

pub(crate) fn build_default_text_card(text: &str) -> serde_json::Value {
    build_markdown_card(text)
}

pub(crate) fn build_markdown_card(markdown: &str) -> serde_json::Value {
    let markdown = crate::im_gateway::markdown_converter::convert_to_feishu_markdown(markdown);
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": markdown,
                "element_id": "bifrost_text_message"
            }]
        }
    })
}

/// Enforce Bifrost's compact Feishu card style at the provider boundary so
/// callers cannot accidentally reintroduce the large root title band.
fn without_root_card_header(mut card: serde_json::Value) -> serde_json::Value {
    if let Some(object) = card.as_object_mut() {
        object.remove("header");
    }
    card
}

// ---------------------------------------------------------------------------
// ImProvider trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ImProvider for FeishuProvider {
    fn provider_type(&self) -> ImProviderType {
        ImProviderType::Feishu
    }

    fn send_capabilities(&self, config: &ImProviderConfig) -> ImSendCapabilities {
        let native = |max_bytes| ImSendPartCapability {
            support: ImSendSupportLevel::Native,
            delivered_as: None,
            max_bytes,
            reason: None,
        };
        ImSendCapabilities {
            provider_id: config.id.clone(),
            provider_type: config.provider_type,
            destinations: vec!["owner".into(), "target".into(), "direct".into()],
            receive_id_types: vec![
                "chat_id".into(),
                "open_id".into(),
                "user_id".into(),
                "union_id".into(),
                "email".into(),
            ],
            parts: BTreeMap::from([
                ("text".into(), native(None)),
                ("markdown".into(), native(None)),
                ("image".into(), native(Some(10 * 1024 * 1024))),
                ("file".into(), native(Some(MAX_OUTBOUND_FILE_BYTES))),
                ("native_card".into(), native(None)),
            ]),
            requires_context: false,
        }
    }

    fn channel_capabilities(&self, config: &ImProviderConfig) -> ImChannelCapabilities {
        ImChannelCapabilities {
            send: self.send_capabilities(config),
            interaction: ImInteractionCapabilities {
                typing: false,
                progress: ImProgressPresentation::MutableCard,
                mutable_message: true,
                native_reply: true,
                reactions: true,
                recall: true,
            },
            conversation: ImConversationCapabilities {
                direct: true,
                group: true,
                thread: true,
                mention: true,
                requires_context: false,
            },
        }
    }

    async fn validate_config(&self, config: &ImProviderConfig) -> Result<ProviderValidation> {
        let mut errors = Vec::new();

        if config.app_id.is_none() || config.app_id.as_deref() == Some("") {
            errors.push("app_id is required for feishu provider".to_string());
        }

        if config.secret_ref.is_none() || config.secret_ref.as_deref() == Some("") {
            errors.push("secret_ref is required for feishu provider".to_string());
        }

        Ok(ProviderValidation {
            valid: errors.is_empty(),
            errors,
        })
    }

    async fn connect_events(
        &self,
        config: &ImProviderConfig,
        sink: EventSink,
    ) -> Result<ConnectionHandle> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel();

        let config = config.clone();
        let http = self.http.clone();

        tokio::spawn(async move {
            // NOTE: In production, app_secret would be resolved from the local secret store
            // using config.secret_ref. Here we pass empty and expect the caller to have
            // provided it through the connection manager which calls start_long_connection directly.
            warn!(
                provider_id = config.id,
                "connect_events spawned via trait - use ImConnectionManager.start_connection for proper secret handling"
            );
            start_long_connection(config, String::new(), sink, shutdown_rx, http, None).await;
            let _ = stopped_tx.send(());
        });

        Ok(ConnectionHandle {
            shutdown_tx,
            stopped_rx: Some(stopped_rx),
        })
    }

    async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult> {
        self.send_card_with_header_policy(config, target, card, opts, false)
            .await
    }

    async fn send_text(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> Result<SendResult> {
        self.send_text_with_uuid(config, target, text, None).await
    }

    #[rustfmt::skip]    async fn send_text_with_uuid(&self, config: &ImProviderConfig, target: &ImTarget, text: &str, uuid: Option<&str>) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let card = build_default_text_card(text);
        let content = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize text card: {}", e))
        })?;

        self.send_message_internal(
            base_url,
            &token,
            &target.receive_id_type,
            &target.receive_id,
            "interactive",
            &content,
            uuid,
        )
        .await
    }

    async fn upload_image(
        &self,
        config: &ImProviderConfig,
        image_type: &str,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<UploadedImage> {
        self.upload_image(config, image_type, file_name, bytes, mime_type)
            .await
    }

    async fn send_image(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        image_key: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let content = serde_json::json!({ "image_key": image_key }).to_string();
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        self.send_message_internal(
            base_url,
            &token,
            &target.receive_id_type,
            &target.receive_id,
            "image",
            &content,
            uuid,
        )
        .await
    }

    #[rustfmt::skip]    async fn upload_file(&self, config: &ImProviderConfig, file_name: &str, bytes: Vec<u8>, mime_type: Option<&str>) -> Result<String> {
        FeishuProvider::upload_file(self, config, file_name, bytes, mime_type).await
    }

    #[rustfmt::skip]    async fn send_file(&self, config: &ImProviderConfig, target: &ImTarget, file_key: &str, uuid: Option<&str>) -> Result<SendResult> {
        FeishuProvider::send_file(self, config, target, file_key, uuid).await
    }

    #[rustfmt::skip]    async fn send_native_card(&self, config: &ImProviderConfig, target: &ImTarget, card: serde_json::Value, opts: SendOptions) -> Result<SendResult> {
        self.send_card_preserving_header(config, target, card, opts).await
    }
}

// ---------------------------------------------------------------------------
// Additional Feishu-specific methods
// ---------------------------------------------------------------------------

impl FeishuProvider {
    pub(crate) async fn send_card_preserving_header(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult> {
        self.send_card_with_header_policy(config, target, card, opts, true)
            .await
    }

    async fn send_card_with_header_policy(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
        preserve_header: bool,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let card = if preserve_header {
            card
        } else {
            without_root_card_header(card)
        };
        let content = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card: {}", e))
        })?;

        self.send_message_internal(
            base_url,
            &token,
            &target.receive_id_type,
            &target.receive_id,
            &opts.msg_type,
            &content,
            opts.uuid.as_deref(),
        )
        .await
    }

    pub async fn reply_card(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card: serde_json::Value,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.reply_card_with_header_policy(config, message_id, card, uuid, false, false)
            .await
    }

    pub(crate) async fn reply_card_preserving_header(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card: serde_json::Value,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.reply_card_with_header_policy(config, message_id, card, uuid, true, false)
            .await
    }

    pub(crate) async fn reply_card_preserving_header_in_thread(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card: serde_json::Value,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.reply_card_with_header_policy(config, message_id, card, uuid, true, true)
            .await
    }

    async fn reply_card_with_header_policy(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card: serde_json::Value,
        uuid: Option<&str>,
        preserve_header: bool,
        reply_in_thread: bool,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let card = if preserve_header {
            card
        } else {
            without_root_card_header(card)
        };
        let content = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize reply card: {}", e))
        })?;
        self.reply_message_internal(
            base_url,
            &token,
            message_id,
            "interactive",
            &content,
            FeishuReplyOptions {
                uuid,
                reply_in_thread,
            },
        )
        .await
    }

    /// Upload an image and return the Feishu image_key that can be used in
    /// image messages and card image elements.
    pub async fn upload_image(
        &self,
        config: &ImProviderConfig,
        image_type: &str,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<UploadedImage> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/im/v1/images", base_url);

        #[derive(Deserialize)]
        struct UploadResponse {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<UploadResponseData>,
        }

        #[derive(Deserialize)]
        struct UploadResponseData {
            image_key: Option<String>,
        }

        let mut part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string());
        if let Some(mime_type) = mime_type.filter(|value| !value.trim().is_empty()) {
            part = part.mime_str(mime_type).map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "invalid image mime type '{}': {}",
                    mime_type, e
                ))
            })?;
        }

        let form = reqwest::multipart::Form::new()
            .text("image_type", image_type.to_string())
            .part("image", part);

        debug!(
            image_type = image_type,
            file_name = file_name,
            "uploading feishu image"
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("feishu image upload failed: {}", e))
            })?;

        let request_id = resp
            .headers()
            .get("x-tt-logid")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body: UploadResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu image upload response parse failed: {}",
                e
            ))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "feishu image upload error: code={}, msg={}",
                    code, msg
                )));
            }
        }

        let image_key = body.data.and_then(|data| data.image_key).ok_or_else(|| {
            bifrost_core::BifrostError::Network(
                "feishu image upload response missing image_key".to_string(),
            )
        })?;

        Ok(UploadedImage {
            image_key,
            request_id,
        })
    }

    pub async fn upload_file(
        &self,
        config: &ImProviderConfig,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<String> {
        validate_outbound_file_size(bytes.len() as u64)?;
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/im/v1/files", base_url);

        #[derive(Deserialize)]
        struct UploadResponse {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<UploadResponseData>,
        }

        #[derive(Deserialize)]
        struct UploadResponseData {
            file_key: Option<String>,
        }

        let mut part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string());
        if let Some(mime_type) = mime_type.filter(|value| !value.trim().is_empty()) {
            part = part.mime_str(mime_type).map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "invalid file mime type '{}': {}",
                    mime_type, e
                ))
            })?;
        }

        let form = reqwest::multipart::Form::new()
            .text(
                "file_type",
                feishu_file_type_for_name(file_name).to_string(),
            )
            .text("file_name", file_name.to_string())
            .part("file", part);

        debug!(file_name = file_name, "uploading feishu file");

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("feishu file upload failed: {}", e))
            })?;

        let body: UploadResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu file upload response parse failed: {}",
                e
            ))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "feishu file upload error: code={}, msg={}",
                    code, msg
                )));
            }
        }

        body.data.and_then(|data| data.file_key).ok_or_else(|| {
            bifrost_core::BifrostError::Network(
                "feishu file upload response missing file_key".to_string(),
            )
        })
    }

    pub async fn send_file(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        file_key: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let content = serde_json::json!({ "file_key": file_key }).to_string();
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        self.send_message_internal(
            base_url,
            &token,
            &target.receive_id_type,
            &target.receive_id,
            "file",
            &content,
            uuid,
        )
        .await
    }

    /// Update an existing interactive card message.
    ///
    /// Uses Feishu PATCH /im/v1/messages/{message_id} API to update card content.
    pub async fn patch_card(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card: serde_json::Value,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let url = format!("{}/im/v1/messages/{}", base_url, message_id);

        let card = without_root_card_header(card);
        let content = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card: {}", e))
        })?;

        #[derive(Serialize)]
        struct PatchRequest<'a> {
            msg_type: &'a str,
            content: &'a str,
        }

        #[derive(Deserialize)]
        struct PatchResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        debug!(message_id = message_id, "patching feishu card message");

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&PatchRequest {
                msg_type: "interactive",
                content: &content,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("feishu patch request failed: {}", e))
            })?;

        let body: PatchResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu patch response parse failed: {}",
                e
            ))
        })?;

        if body.code.unwrap_or(0) != 0 {
            let err = bifrost_core::BifrostError::Network(format!(
                "feishu patch_card failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.as_deref().unwrap_or("")
            ));
            tracing::warn!(error = %err, message_id = message_id);
            return Err(err);
        }

        Ok(())
    }

    /// Create a CardKit card entity from JSON 2.0 card data.
    pub async fn create_card_entity(
        &self,
        config: &ImProviderConfig,
        card: serde_json::Value,
    ) -> Result<String> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/cardkit/v1/cards", base_url);
        let card = without_root_card_header(card);
        let data = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card entity: {}", e))
        })?;

        #[derive(Serialize)]
        struct CreateCardRequest<'a> {
            #[serde(rename = "type")]
            card_type: &'a str,
            data: &'a str,
        }

        #[derive(Deserialize)]
        struct CreateCardResponse {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<CreateCardData>,
        }

        #[derive(Deserialize)]
        struct CreateCardData {
            card_id: Option<String>,
        }

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&CreateCardRequest {
                card_type: "card_json",
                data: &data,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu create card entity request failed: {}",
                    e
                ))
            })?;

        let body: CreateCardResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu create card entity response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu create card entity failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        body.data.and_then(|data| data.card_id).ok_or_else(|| {
            bifrost_core::BifrostError::Network(
                "feishu create card entity response missing card_id".to_string(),
            )
        })
    }

    /// Send a previously created CardKit card entity.
    pub async fn send_card_entity(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card_id: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let content = serde_json::json!({
            "type": "card",
            "data": {
                "card_id": card_id
            }
        })
        .to_string();
        self.send_message_internal(
            base_url,
            &token,
            &target.receive_id_type,
            &target.receive_id,
            "interactive",
            &content,
            uuid,
        )
        .await
    }

    /// Reply with a previously created CardKit entity so progress cards also
    /// use Feishu's native compact quote for the triggering message.
    pub async fn reply_card_entity(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card_id: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let content = serde_json::json!({
            "type": "card",
            "data": {
                "card_id": card_id
            }
        })
        .to_string();
        self.reply_message_internal(
            base_url,
            &token,
            message_id,
            "interactive",
            &content,
            FeishuReplyOptions {
                uuid,
                reply_in_thread: false,
            },
        )
        .await
    }

    pub async fn reply_card_entity_in_thread(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        card_id: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let content = serde_json::json!({
            "type": "card",
            "data": { "card_id": card_id }
        })
        .to_string();
        self.reply_message_internal(
            base_url,
            &token,
            message_id,
            "interactive",
            &content,
            FeishuReplyOptions {
                uuid,
                reply_in_thread: true,
            },
        )
        .await
    }

    /// Replace the full JSON 2.0 payload of a CardKit card entity.
    pub async fn update_card_entity(
        &self,
        config: &ImProviderConfig,
        card_id: &str,
        card: serde_json::Value,
        sequence: u64,
        uuid: &str,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/cardkit/v1/cards/{}", base_url, card_id);
        let card = without_root_card_header(card);
        let data = serde_json::to_string(&card).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card entity: {}", e))
        })?;

        #[derive(Serialize)]
        struct UpdateCardPayload<'a> {
            #[serde(rename = "type")]
            card_type: &'a str,
            data: &'a str,
        }

        #[derive(Serialize)]
        struct UpdateCardRequest<'a> {
            card: UpdateCardPayload<'a>,
            sequence: u64,
            uuid: &'a str,
        }

        #[derive(Deserialize)]
        struct UpdateCardResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&UpdateCardRequest {
                card: UpdateCardPayload {
                    card_type: "card_json",
                    data: &data,
                },
                sequence,
                uuid,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu update card entity request failed: {}",
                    e
                ))
            })?;
        let body: UpdateCardResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu update card entity response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu update card entity failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Update a text/markdown element content in a CardKit card entity.
    pub async fn update_card_element_content(
        &self,
        config: &ImProviderConfig,
        card_id: &str,
        element_id: &str,
        content: &str,
        sequence: u64,
        uuid: &str,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!(
            "{}/cardkit/v1/cards/{}/elements/{}/content",
            base_url, card_id, element_id
        );

        #[derive(Serialize)]
        struct UpdateContentRequest<'a> {
            content: &'a str,
            sequence: u64,
            uuid: &'a str,
        }

        #[derive(Deserialize)]
        struct UpdateContentResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&UpdateContentRequest {
                content,
                sequence,
                uuid,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu update card element request failed: {}",
                    e
                ))
            })?;
        let body: UpdateContentResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu update card element response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu update card element failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Replace a CardKit card element with a full JSON 2.0 element.
    pub async fn update_card_element(
        &self,
        config: &ImProviderConfig,
        card_id: &str,
        element_id: &str,
        element: serde_json::Value,
        sequence: u64,
        uuid: &str,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!(
            "{}/cardkit/v1/cards/{}/elements/{}",
            base_url, card_id, element_id
        );
        let element = serde_json::to_string(&element).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card element: {}", e))
        })?;

        #[derive(Serialize)]
        struct UpdateElementRequest<'a> {
            element: &'a str,
            sequence: u64,
            uuid: &'a str,
        }

        #[derive(Deserialize)]
        struct UpdateElementResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&UpdateElementRequest {
                element: &element,
                sequence,
                uuid,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu update card element request failed: {}",
                    e
                ))
            })?;
        let body: UpdateElementResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu update card element response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu update card element failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Update CardKit card settings, used to close streaming mode.
    pub async fn update_card_settings(
        &self,
        config: &ImProviderConfig,
        card_id: &str,
        settings: serde_json::Value,
        sequence: u64,
        uuid: &str,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/cardkit/v1/cards/{}/settings", base_url, card_id);
        let settings = serde_json::to_string(&settings).map_err(|e| {
            bifrost_core::BifrostError::Parse(format!("failed to serialize card settings: {}", e))
        })?;

        #[derive(Serialize)]
        struct UpdateSettingsRequest<'a> {
            settings: &'a str,
            sequence: u64,
            uuid: &'a str,
        }

        #[derive(Deserialize)]
        struct UpdateSettingsResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&UpdateSettingsRequest {
                settings: &settings,
                sequence,
                uuid,
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu update card settings request failed: {}",
                    e
                ))
            })?;
        let body: UpdateSettingsResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu update card settings response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu update card settings failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Recall a bot-sent message, best-effort used when moving the active progress card.
    pub async fn recall_message(&self, config: &ImProviderConfig, message_id: &str) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{}/im/v1/messages/{}", base_url, message_id);

        #[derive(Deserialize)]
        struct RecallResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        let resp = self
            .http
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu recall message request failed: {}",
                    e
                ))
            })?;
        let body: RecallResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu recall message response parse failed: {}",
                e
            ))
        })?;
        if body.code.unwrap_or(0) != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu recall message failed: code={}, msg={}",
                body.code.unwrap_or(0),
                body.msg.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Fetch the bot owner's open_id from the Feishu Application Info API.
    ///
    /// Calls `GET /open-apis/application/v6/applications/:app_id?user_id_type=open_id`
    /// and extracts `owner.owner_id` from the response.
    pub async fn fetch_bot_owner_open_id(&self, config: &ImProviderConfig) -> Result<String> {
        let base_url = Self::base_url(config);
        let app_id = config.app_id.as_deref().unwrap_or_default();
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let url = format!(
            "{}/application/v6/applications/{}?user_id_type=open_id&lang=zh_cn",
            base_url, app_id
        );

        #[derive(Deserialize)]
        struct Resp {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<AppData>,
        }

        #[derive(Deserialize)]
        struct AppData {
            app: Option<AppInfo>,
        }

        #[derive(Deserialize)]
        struct AppInfo {
            owner: Option<AppOwner>,
        }

        #[derive(Deserialize)]
        struct AppOwner {
            #[serde(rename = "owner_id")]
            open_id: Option<String>,
        }

        debug!(
            app_id = app_id,
            "fetching bot owner open_id from application API"
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("fetch app info request failed: {}", e))
            })?;

        let body: Resp = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "fetch app info response parse failed: {}",
                e
            ))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "fetch app info error: code={}, msg={}",
                    code, msg
                )));
            }
        }

        let open_id = body
            .data
            .and_then(|d| d.app)
            .and_then(|a| a.owner)
            .and_then(|o| o.open_id)
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "app info response missing owner.open_id".to_string(),
                )
            })?;

        info!(
            app_id = app_id,
            owner_open_id = %open_id,
            "fetched bot owner open_id"
        );

        Ok(open_id)
    }

    /// Resolve the current application bot identity used to distinguish an
    /// explicit @bot mention from mentions of ordinary group members.
    pub async fn fetch_bot_identity(&self, config: &ImProviderConfig) -> Result<FeishuBotIdentity> {
        let base_url = Self::base_url(config);
        let app_id = config.app_id.as_deref().unwrap_or_default();
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let cache_key = Self::token_cache_key(base_url, app_id, app_secret);
        if let Some(identity) = self.bot_identity_cache.read().get(&cache_key).cloned() {
            return Ok(identity);
        }

        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!("{base_url}/bot/v3/info");
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "fetch bot identity request failed: {error}"
                ))
            })?;
        let value: serde_json::Value = response.json().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "fetch bot identity response parse failed: {error}"
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
            return Err(bifrost_core::BifrostError::Network(format!(
                "fetch bot identity failed: code={code}, msg={message}"
            )));
        }
        let bot = value
            .get("bot")
            .or_else(|| value.get("data").and_then(|data| data.get("bot")))
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network("bot identity response missing bot".to_string())
            })?;
        let open_id = bot
            .get("open_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "bot identity response missing bot.open_id".to_string(),
                )
            })?
            .to_string();
        let name = bot
            .get("app_name")
            .or_else(|| bot.get("bot_name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let identity = FeishuBotIdentity { open_id, name };
        self.bot_identity_cache
            .write()
            .insert(cache_key, identity.clone());
        Ok(identity)
    }

    /// Resolve the display name for a group that this provider's bot belongs
    /// to. Message receive events only contain `chat_id`, so initialization
    /// must enrich the group session through the chat information API.
    pub async fn fetch_chat_name(
        &self,
        config: &ImProviderConfig,
        chat_id: &str,
    ) -> Result<String> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;
        let url = format!(
            "{base_url}/im/v1/chats/{}?user_id_type=open_id",
            chat_id.trim()
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "fetch Feishu chat information request failed: {error}"
                ))
            })?;
        let value: serde_json::Value = response.json().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "fetch Feishu chat information response parse failed: {error}"
            ))
        })?;
        parse_feishu_chat_name(&value).map_err(bifrost_core::BifrostError::Network)
    }

    /// Add a reaction emoji to a message.
    ///
    /// `emoji_type` should be a Feishu emoji code, e.g. `"THUMBSUP"`.
    pub async fn add_reaction(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        emoji_type: &str,
    ) -> Result<()> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let url = format!("{}/im/v1/messages/{}/reactions", base_url, message_id);

        #[derive(Serialize)]
        struct EmojiTypeField<'a> {
            emoji_type: &'a str,
        }

        #[derive(Serialize)]
        struct ReactionRequest<'a> {
            reaction_type: EmojiTypeField<'a>,
        }

        #[derive(Deserialize)]
        struct ReactionResponse {
            code: Option<i64>,
            msg: Option<String>,
        }

        debug!(
            message_id = message_id,
            emoji_type = emoji_type,
            "adding reaction to feishu message"
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&ReactionRequest {
                reaction_type: EmojiTypeField { emoji_type },
            })
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("add_reaction request failed: {}", e))
            })?;

        let body: ReactionResponse = resp.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "add_reaction response parse failed: {}",
                e
            ))
        })?;

        if let Some(code) = body.code {
            if code != 0 {
                let msg = body.msg.unwrap_or_default();
                return Err(bifrost_core::BifrostError::Network(format!(
                    "add_reaction error: code={}, msg={}",
                    code, msg
                )));
            }
        }

        info!(
            message_id = message_id,
            emoji_type = emoji_type,
            "reaction added to message"
        );
        Ok(())
    }

    /// Download an image resource embedded in a Feishu message.
    pub async fn download_message_image_resource(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        file_key: &str,
    ) -> Result<(String, Vec<u8>)> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let url = format!(
            "{}/im/v1/messages/{}/resources/{}?type=image",
            base_url, message_id, file_key
        );
        debug!(
            message_id = message_id,
            file_key = file_key,
            "downloading feishu message image resource"
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu message image download failed: {}",
                    e
                ))
            })?;

        let status = resp.status();
        let mime_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("image/png")
            .to_string();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu message image download error: status={}, body={}",
                status, body
            )));
        }
        let bytes =
            read_feishu_message_resource_body(resp, "image", MAX_MESSAGE_IMAGE_RESOURCE_BYTES)
                .await?;
        Ok((mime_type, bytes))
    }

    /// Download a generic file resource embedded in a Feishu message.
    pub async fn download_message_file_resource(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        file_key: &str,
    ) -> Result<(String, Vec<u8>)> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

        let url = format!(
            "{}/im/v1/messages/{}/resources/{}?type=file",
            base_url, message_id, file_key
        );
        debug!(
            message_id = message_id,
            file_key = file_key,
            "downloading feishu message file resource"
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "feishu message file download failed: {}",
                    e
                ))
            })?;

        let status = resp.status();
        let mime_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("application/octet-stream")
            .to_string();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "feishu message file download error: status={}, body={}",
                status, body
            )));
        }
        let bytes =
            read_feishu_message_resource_body(resp, "file", MAX_MESSAGE_FILE_RESOURCE_BYTES)
                .await?;
        Ok((mime_type, bytes))
    }
}

fn validate_outbound_file_size(size: u64) -> Result<()> {
    if size == 0 {
        return Err(bifrost_core::BifrostError::Config(
            "feishu outbound file upload requires non-empty bytes".to_string(),
        ));
    }
    if size > MAX_OUTBOUND_FILE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "feishu outbound file exceeds {} MiB limit",
            MAX_OUTBOUND_FILE_BYTES / 1024 / 1024
        )));
    }
    Ok(())
}

async fn read_feishu_message_resource_body(
    response: reqwest::Response,
    resource_kind: &str,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let header_too_large = response
        .content_length()
        .is_some_and(|length| length > max_bytes);
    if header_too_large {
        return Err(feishu_resource_size_error(resource_kind, max_bytes));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "feishu message {resource_kind} body read failed: {error}"
            ))
        })?;
        let buffered = bytes.len() as u64;
        if buffered.saturating_add(chunk.len() as u64) > max_bytes {
            return Err(feishu_resource_size_error(resource_kind, max_bytes));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn feishu_resource_size_error(resource_kind: &str, max_bytes: u64) -> bifrost_core::BifrostError {
    let max_mib = max_bytes / 1024 / 1024;
    bifrost_core::BifrostError::Config(format!(
        "飞书消息 {resource_kind} 资源超过 {max_mib} MiB 上限"
    ))
}

fn parse_feishu_chat_name(value: &serde_json::Value) -> std::result::Result<String, String> {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Err(format!(
            "fetch Feishu chat information failed: code={code}, msg={message}"
        ));
    }
    value
        .get("data")
        .and_then(|data| data.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "fetch Feishu chat information response missing data.name".to_string())
}

// ---------------------------------------------------------------------------
// Long Connection
// ---------------------------------------------------------------------------

/// Start and maintain a Feishu long connection with auto-reconnect.
///
/// This function runs until `shutdown_rx` fires or is dropped.
pub(crate) async fn start_long_connection(
    config: ImProviderConfig,
    app_secret: String,
    sink: EventSink,
    mut shutdown_rx: oneshot::Receiver<()>,
    http: reqwest::Client,
    status_tx: Option<FeishuConnectionStatusTx>,
) {
    let provider_id = config.id.clone();
    let mut backoff_secs = INITIAL_BACKOFF_SECS;
    let mut reconnect_count: u32 = 0;
    let mut total_connects: u32 = 0;

    loop {
        info!(provider_id = %provider_id, reconnect_count, "starting feishu long connection");

        match run_connection_loop(
            &config,
            &app_secret,
            &sink,
            &mut shutdown_rx,
            &http,
            status_tx.as_ref(),
        )
        .await
        {
            ConnectionLoopResult::Shutdown => {
                info!(provider_id = %provider_id, "feishu long connection shutdown requested");
                return;
            }
            ConnectionLoopResult::ConnectedThenDisconnected(err) => {
                // We successfully entered the ws event loop at least once, so
                // reset backoff — treat this as the "first failure" of a
                // freshly-connected session rather than compounding onto the
                // previous reconnect streak.
                total_connects = total_connects.saturating_add(1);
                backoff_secs = INITIAL_BACKOFF_SECS;
                reconnect_count += 1;
                warn!(
                    provider_id = %provider_id,
                    error = %err,
                    backoff_secs,
                    reconnect_count,
                    total_connects,
                    "feishu long connection dropped after being connected, will reconnect"
                );
                publish_connection_status(
                    status_tx.as_ref(),
                    ConnectionState::Reconnecting,
                    Some(err.clone()),
                );
                if wait_with_shutdown(&mut shutdown_rx, Duration::from_secs(backoff_secs)).await {
                    info!(provider_id = %provider_id, "shutdown during reconnect backoff");
                    return;
                }
            }
            ConnectionLoopResult::Disconnected(err) => {
                reconnect_count += 1;
                warn!(
                    provider_id = %provider_id,
                    error = %err,
                    backoff_secs,
                    reconnect_count,
                    "feishu long connection disconnected, will reconnect"
                );
                publish_connection_status(
                    status_tx.as_ref(),
                    ConnectionState::Reconnecting,
                    Some(err.clone()),
                );

                if reconnect_count == 5 || reconnect_count.is_multiple_of(20) {
                    // Elevate visibility for operators watching tail -f logs —
                    // repeated early failures often indicate bad credentials,
                    // DNS issues, or network egress being blocked.
                    error!(
                        provider_id = %provider_id,
                        reconnect_count,
                        last_error = %err,
                        "feishu long connection has failed to establish repeatedly"
                    );
                }
                if wait_with_shutdown(&mut shutdown_rx, Duration::from_secs(backoff_secs)).await {
                    info!(provider_id = %provider_id, "shutdown during reconnect backoff");
                    return;
                }

                // Exponential backoff: 1, 2, 4, 8, ..., max 60s
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
            }
        }
    }
}

/// Sleep for `delay`, waking early only if an explicit `send(())` arrives on
/// `shutdown_rx`. Dropping the sender is **not** treated as a shutdown; in
/// that case the function simply finishes the sleep. Returns `true` iff an
/// explicit shutdown was received.
async fn wait_with_shutdown(shutdown_rx: &mut oneshot::Receiver<()>, delay: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        res = &mut *shutdown_rx => {
            match res {
                Ok(()) => true,
                Err(_) => {
                    // Sender was dropped. Replace the receiver with a never-
                    // ready one so subsequent waits don't busy-loop returning
                    // Err immediately, then fall through.
                    let (_leak, rx) = oneshot::channel::<()>();
                    *shutdown_rx = rx;
                    false
                }
            }
        }
    }
}

enum ConnectionLoopResult {
    Shutdown,
    /// The ws successfully entered the message loop and later dropped.
    /// Backoff should be reset when handling this variant.
    ConnectedThenDisconnected(String),
    /// The ws never made it into the message loop this iteration
    /// (endpoint fetch / handshake / auth failed).
    Disconnected(String),
}

async fn run_connection_loop(
    config: &ImProviderConfig,
    app_secret: &str,
    sink: &EventSink,
    shutdown_rx: &mut oneshot::Receiver<()>,
    http: &reqwest::Client,
    status_tx: Option<&FeishuConnectionStatusTx>,
) -> ConnectionLoopResult {
    let app_id = config.app_id.as_deref().unwrap_or_default();
    let domain = ws_domain(config);

    // Step 1: Get WebSocket endpoint (authenticates with AppID/AppSecret)
    let endpoint = match fetch_ws_endpoint(http, &domain, app_id, app_secret).await {
        Ok(ep) => ep,
        Err(e) => {
            return ConnectionLoopResult::Disconnected(format!("ws endpoint fetch failed: {}", e));
        }
    };

    let service_id = endpoint.service_id;

    // Step 2: Connect WebSocket
    debug!(provider_id = %config.id, url = %endpoint.url, "connecting to feishu websocket");

    let ws_stream = match tokio_tungstenite::connect_async(&endpoint.url).await {
        Ok((stream, _)) => stream,
        Err(e) => {
            return ConnectionLoopResult::Disconnected(format!("websocket connect failed: {}", e));
        }
    };

    info!(provider_id = %config.id, service_id, ping_interval = endpoint.ping_interval_secs, "feishu websocket connected");
    publish_connection_status(status_tx, ConnectionState::Connected, None);

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Track the last moment we observed any server-originated traffic. Used
    // by the silence watchdog below to detect a silently-dead TCP connection
    // that neither errors out nor closes cleanly.
    let mut last_server_msg_at = std::time::Instant::now();

    // Step 3: Message loop with protobuf ping heartbeat
    let mut ping_interval = tokio::time::interval(Duration::from_secs(endpoint.ping_interval_secs));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut silence_check =
        tokio::time::interval(Duration::from_secs(WS_SILENCE_CHECK_INTERVAL_SECS));
    silence_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            res = &mut *shutdown_rx => {
                match res {
                    Ok(()) => {
                        let _ = ws_write.close().await;
                        return ConnectionLoopResult::Shutdown;
                    }
                    Err(_) => {
                        // Sender was dropped — NOT a genuine shutdown. Swap
                        // in a never-ready receiver so this select arm doesn't
                        // busy-loop on repeated Err returns, then continue
                        // servicing the ws.
                        let (_leak, rx) = oneshot::channel::<()>();
                        *shutdown_rx = rx;
                        debug!(
                            provider_id = %config.id,
                            "shutdown_tx was dropped; keeping connection alive and ignoring"
                        );
                        continue;
                    }
                }
            }
            _ = ping_interval.tick() => {
                // Send protobuf-encoded ping frame
                let ping_data = build_ping_frame(service_id);
                if let Err(e) = ws_write.send(Message::Binary(ping_data.into())).await {
                    return ConnectionLoopResult::ConnectedThenDisconnected(
                        format!("ping send failed: {}", e)
                    );
                }
            }
            _ = silence_check.tick() => {
                let elapsed = last_server_msg_at.elapsed();
                if elapsed > Duration::from_secs(WS_SERVER_SILENCE_TIMEOUT_SECS) {
                    warn!(
                        provider_id = %config.id,
                        silence_secs = elapsed.as_secs(),
                        "no server traffic within watchdog window; forcing reconnect"
                    );
                    let _ = ws_write.close().await;
                    return ConnectionLoopResult::ConnectedThenDisconnected(
                        "server silence watchdog timeout".to_string(),
                    );
                }
            }
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        last_server_msg_at = std::time::Instant::now();
                        match PbFrame::decode(data.as_ref()) {
                            Ok(frame) => {
                                let method = frame.method;
                                let type_val = get_header_value(&frame.headers, "type")
                                    .unwrap_or("")
                                    .to_string();

                                if method == 0 {
                                    // CONTROL frame
                                    if type_val == "pong" {
                                        debug!(provider_id = %config.id, "received pong from server");
                                    }
                                } else if method == 1 {
                                    // DATA frame
                                    if type_val == "event" {
                                        let success = if let Some(ref payload) = frame.payload {
                                            match std::str::from_utf8(payload) {
                                                Ok(text) => {
                                                    card_action::handle_ws_message(text, &config.id, sink);
                                                    true
                                                }
                                                Err(e) => {
                                                    warn!(provider_id = %config.id, error = %e, "invalid utf8 in event payload");
                                                    false
                                                }
                                            }
                                        } else {
                                            warn!(provider_id = %config.id, "DATA frame with type=event but no payload");
                                            false
                                        };

                                        // Send response frame back
                                        let resp = build_response_frame(&frame, success);
                                        if let Err(e) = ws_write.send(Message::Binary(resp.into())).await {
                                            return ConnectionLoopResult::ConnectedThenDisconnected(
                                                format!("response send failed: {}", e)
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(provider_id = %config.id, error = %e, "failed to decode protobuf frame");
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        last_server_msg_at = std::time::Instant::now();
                        debug!(provider_id = %config.id, len = text.len(), "received unexpected text message");
                    }
                    Some(Ok(Message::Ping(data))) => {
                        last_server_msg_at = std::time::Instant::now();
                        if let Err(e) = ws_write.send(Message::Pong(data)).await {
                            return ConnectionLoopResult::ConnectedThenDisconnected(
                                format!("pong send failed: {}", e)
                            );
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_server_msg_at = std::time::Instant::now();
                        // WebSocket-level pong, ignore
                    }
                    Some(Ok(Message::Close(_))) => {
                        return ConnectionLoopResult::ConnectedThenDisconnected(
                            "server sent close frame".to_string()
                        );
                    }
                    Some(Ok(Message::Frame(_))) => {
                        // Raw frame, ignore
                    }
                    Some(Err(e)) => {
                        return ConnectionLoopResult::ConnectedThenDisconnected(
                            format!("websocket error: {}", e)
                        );
                    }
                    None => {
                        return ConnectionLoopResult::ConnectedThenDisconnected(
                            "websocket stream ended".to_string()
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event Normalization
// ---------------------------------------------------------------------------

/// Normalize a raw Feishu event into the unified ImEvent model.
///
/// Handles message receive and bot-added lifecycle events.
pub fn normalize_feishu_event(raw: &serde_json::Value, provider_id: &str) -> Option<ImEvent> {
    let header = raw.get("header")?;
    let event_id = header.get("event_id").and_then(|v| v.as_str())?.to_string();
    let event_type_raw = header.get("event_type").and_then(|v| v.as_str())?;

    let normalized_event_type = match event_type_raw {
        "im.message.receive_v1" => "message.receive",
        "im.chat.member.bot.added_v1" => {
            let event = raw.get("event")?;
            let chat_id = event
                .get("chat_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let raw_bytes = raw.to_string();
            let digest_value = digest(&SHA256, raw_bytes.as_bytes());
            return Some(ImEvent {
                event_id,
                provider_id: provider_id.to_string(),
                provider_type: ImProviderType::Feishu,
                event_type: crate::im_gateway::feishu_group_permission::BOT_JOINED_EVENT_TYPE
                    .to_string(),
                source: ImEventSource {
                    chat_id: Some(chat_id),
                    chat_type: Some("group".to_string()),
                    ..ImEventSource::default()
                },
                message: None,
                received_at: current_timestamp_ms(),
                raw_digest: Some(format!("sha256:{}", hex_encode(digest_value.as_ref()))),
            });
        }
        _ => {
            debug!(
                provider_id = provider_id,
                event_type = event_type_raw,
                "ignoring unsupported feishu event type"
            );
            return None;
        }
    };

    let event = raw.get("event")?;

    // Extract message info
    let message = event.get("message")?;
    let chat_id = message
        .get("chat_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let message_id = message
        .get("message_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let message_type = message
        .get("message_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let chat_type = message
        .get("chat_type")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let create_time = message
        .get("create_time")
        .and_then(json_u64_from_string_or_number);
    let update_time = message
        .get("update_time")
        .and_then(json_u64_from_string_or_number);
    let root_id = json_non_empty_string(message.get("root_id"));
    let parent_id = json_non_empty_string(message.get("parent_id"));
    let thread_id = json_non_empty_string(message.get("thread_id"));

    let content_obj = message
        .get("content")
        .and_then(|v| v.as_str())
        .and_then(|content_str| serde_json::from_str::<serde_json::Value>(content_str).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let mentions = parse_feishu_mentions(message.get("mentions"));

    // Extract text content from message.content. Feishu rich text (`post`)
    // stores plain text in nested `content` arrays rather than top-level text.
    // Its `at` nodes contain display names/IDs instead of the stable mention
    // placeholders supplied alongside the message, so normalize them back to
    // those placeholders before group classification and prompt rendering.
    let text = extract_feishu_message_text(&content_obj, &mentions);

    let (images, files) = parse_feishu_message_attachments(
        message_type.as_deref().unwrap_or("unknown"),
        &content_obj,
    );

    info!(
        provider_id = %provider_id,
        event_id = %event_id,
        message_id = ?message_id,
        message_type = ?message_type,
        text_len = text.len(),
        image_count = images.len(),
        file_count = files.len(),
        image_keys = %images
            .iter()
            .map(|image| image.file_key.as_str())
            .collect::<Vec<_>>()
            .join(","),
        file_keys = %files
            .iter()
            .map(|file| file.file_key.as_str())
            .collect::<Vec<_>>()
            .join(","),
        content_keys = %json_object_keys(&content_obj).join(","),
        content_preview = %bifrost_core::text::truncate_bytes_with_suffix(&content_obj.to_string(), 500, "..."),
        "normalized feishu inbound message"
    );

    // Extract sender info
    let sender = event.get("sender");
    let user_id = sender
        .and_then(|s| s.get("sender_id"))
        .and_then(|sid| {
            sid.get("open_id")
                .or_else(|| sid.get("user_id"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());
    let sender_type = sender
        .and_then(|sender| sender.get("sender_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    // Compute raw digest (sha256 of the raw JSON)
    let raw_bytes = raw.to_string();
    let digest_value = digest(&SHA256, raw_bytes.as_bytes());
    let raw_digest = format!("sha256:{}", hex_encode(digest_value.as_ref()));

    let now = current_timestamp_ms();

    Some(ImEvent {
        event_id,
        provider_id: provider_id.to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: normalized_event_type.to_string(),
        source: ImEventSource {
            chat_id,
            chat_type,
            user_id,
            user_name: None,
            sender_type,
            message_id,
        },
        message: Some(ImEventMessage {
            text,
            mentions,
            images,
            files,
            reply_to: None,
            raw_type: message_type,
            raw_content: Some(content_obj),
            create_time,
            update_time,
            root_id,
            parent_id,
            thread_id,
        }),
        received_at: now,
        raw_digest: Some(raw_digest),
    })
}

fn json_u64_from_string_or_number(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn parse_feishu_mentions(value: Option<&serde_json::Value>) -> Vec<ImMention> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|mention| ImMention {
            key: mention
                .get("key")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            open_id: mention
                .get("id")
                .and_then(|id| {
                    id.as_str()
                        .or_else(|| id.get("open_id").and_then(serde_json::Value::as_str))
                })
                .map(str::to_string),
            name: mention
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            tenant_key: mention
                .get("tenant_key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            is_bot: false,
        })
        .collect()
}

fn json_non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// HTTP helpers for long connection setup
// ---------------------------------------------------------------------------

/// Extract the domain (without `/open-apis`) from the provider config base_url.
/// e.g. "https://open.feishu.cn/open-apis" -> "https://open.feishu.cn"
fn ws_domain(config: &ImProviderConfig) -> String {
    let base = config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
    base.trim_end_matches('/')
        .trim_end_matches("/open-apis")
        .to_string()
}

fn feishu_file_type_for_name(file_name: &str) -> &'static str {
    match std::path::Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "pdf",
        "doc" | "docx" => "doc",
        "xls" | "xlsx" | "csv" => "xls",
        "ppt" | "pptx" => "ppt",
        _ => "stream",
    }
}

async fn fetch_ws_endpoint(
    http: &reqwest::Client,
    domain: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<WsEndpointResult> {
    let url = format!("{}/callback/ws/endpoint", domain);

    #[derive(Serialize)]
    #[allow(non_snake_case)]
    struct Req<'a> {
        AppID: &'a str,
        AppSecret: &'a str,
    }

    #[derive(Deserialize)]
    struct Resp {
        code: Option<i64>,
        msg: Option<String>,
        data: Option<WsEndpointData>,
    }

    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct WsEndpointData {
        URL: Option<String>,
        ClientConfig: Option<ClientConfig>,
    }

    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct ClientConfig {
        PingInterval: Option<u64>,
    }

    let resp = http
        .post(&url)
        .json(&Req {
            AppID: app_id,
            AppSecret: app_secret,
        })
        .send()
        .await
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "ws endpoint request failed: {}",
                reqwest_error_with_sources(e)
            ))
        })?;

    let body: Resp = resp.json().await.map_err(|e| {
        bifrost_core::BifrostError::Network(format!("ws endpoint parse failed: {}", e))
    })?;

    if let Some(code) = body.code {
        if code != 0 {
            return Err(bifrost_core::BifrostError::Network(format!(
                "ws endpoint error: code={}, msg={}",
                code,
                body.msg.unwrap_or_default()
            )));
        }
    }

    let data = body.data.ok_or_else(|| {
        bifrost_core::BifrostError::Network("ws endpoint response missing data".to_string())
    })?;

    let ws_url = data.URL.ok_or_else(|| {
        bifrost_core::BifrostError::Network("ws endpoint response missing URL".to_string())
    })?;

    // Extract service_id from URL query parameters
    let service_id = url::Url::parse(&ws_url)
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "service_id")
                .and_then(|(_, v)| v.parse::<i32>().ok())
        })
        .unwrap_or(0);

    let ping_interval_secs = data
        .ClientConfig
        .and_then(|c| c.PingInterval)
        .unwrap_or(WS_PING_INTERVAL_SECS);

    Ok(WsEndpointResult {
        url: ws_url,
        service_id,
        ping_interval_secs,
    })
}

pub(crate) fn build_feishu_http_client() -> reqwest::Client {
    bifrost_core::outbound_reqwest_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("static Feishu HTTP client configuration should be valid")
}

fn publish_connection_status(
    status_tx: Option<&FeishuConnectionStatusTx>,
    state: ConnectionState,
    error: Option<String>,
) {
    if let Some(tx) = status_tx {
        let _ = tx.send(FeishuConnectionStatusEvent { state, error });
    }
}

fn reqwest_error_with_sources(error: reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn collect_rich_text_image_keys(value: &serde_json::Value, images: &mut Vec<ImImageAttachment>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(image_key) = map
                .get("image_key")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if !images.iter().any(|image| image.file_key == image_key) {
                    images.push(ImImageAttachment {
                        file_key: image_key.to_string(),
                        source: ImImageSource::MessageResource,
                        ..Default::default()
                    });
                }
            }
            for child in map.values() {
                collect_rich_text_image_keys(child, images);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_rich_text_image_keys(item, images);
            }
        }
        _ => {}
    }
}

pub(super) fn parse_feishu_message_attachments(
    message_type: &str,
    content: &serde_json::Value,
) -> (Vec<ImImageAttachment>, Vec<ImFileAttachment>) {
    let mut images = Vec::new();
    let mut files = Vec::new();
    if message_type == "image" {
        if let Some(image_key) = content
            .get("image_key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            images.push(ImImageAttachment {
                file_key: image_key.to_string(),
                source: ImImageSource::MessageResource,
                ..Default::default()
            });
        }
    }
    if message_type == "file" {
        if let Some(file_key) = content
            .get("file_key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            files.push(ImFileAttachment {
                file_key: file_key.to_string(),
                name: content
                    .get("file_name")
                    .or_else(|| content.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
                mime_type: content
                    .get("mime_type")
                    .or_else(|| content.get("mimeType"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
                size_bytes: content
                    .get("file_size")
                    .or_else(|| content.get("size"))
                    .or_else(|| content.get("size_bytes"))
                    .and_then(serde_json::Value::as_u64),
                ..Default::default()
            });
        }
    }
    collect_rich_text_image_keys(content, &mut images);
    (images, files)
}

fn extract_feishu_message_text(value: &serde_json::Value, mentions: &[ImMention]) -> String {
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    let mut parts = Vec::new();
    collect_feishu_text_nodes(value, mentions, &mut parts);
    parts.join("").trim().to_string()
}

fn collect_feishu_text_nodes(
    value: &serde_json::Value,
    mentions: &[ImMention],
    parts: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str());
            if matches!(tag, Some("text" | "a" | "code_block")) {
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            if tag == Some("at") {
                if let Some(text) = rich_text_mention_placeholder(map, mentions).or_else(|| {
                    map.get("user_name")
                        .or_else(|| map.get("user_id"))
                        .and_then(|v| v.as_str())
                }) {
                    parts.push(text.to_string());
                }
            }
            if tag == Some("br") {
                parts.push("\n".to_string());
            }
            for child in map.values() {
                collect_feishu_text_nodes(child, mentions, parts);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_feishu_text_nodes(item, mentions, parts);
            }
        }
        _ => {}
    }
}

fn rich_text_mention_placeholder<'a>(
    node: &serde_json::Map<String, serde_json::Value>,
    mentions: &'a [ImMention],
) -> Option<&'a str> {
    let user_id = node
        .get("open_id")
        .or_else(|| node.get("user_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let user_name = node
        .get("user_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    user_id
        .and_then(|id| {
            mentions.iter().find(|mention| {
                !mention.key.trim().is_empty() && mention.open_id.as_deref() == Some(id)
            })
        })
        .or_else(|| {
            user_name.and_then(|name| {
                mentions.iter().find(|mention| {
                    !mention.key.trim().is_empty() && mention.name.as_deref() == Some(name)
                })
            })
        })
        .map(|mention| mention.key.as_str())
}

fn json_object_keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn spawn_feishu_api_server(
        bot_body: &'static str,
        chat_body: &'static str,
        reply_body: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Feishu API fixture");
        let address = listener.local_addr().expect("Feishu API fixture address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| async move {
                        let path = request.uri().path();
                        let (status, body) =
                            if path.ends_with("/auth/v3/tenant_access_token/internal") {
                                (
                                    StatusCode::OK,
                                    r#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                                )
                            } else if path.ends_with("/bot/v3/info") {
                                (StatusCode::OK, bot_body)
                            } else if path.contains("/im/v1/chats/") {
                                (StatusCode::OK, chat_body)
                            } else if path.contains("/im/v1/messages/") {
                                (StatusCode::OK, reply_body)
                            } else {
                                (StatusCode::NOT_FOUND, r#"{"code":404}"#)
                            };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(status)
                                .header("Content-Type", "application/json")
                                .body(Full::new(Bytes::from_static(body.as_bytes())))
                                .unwrap(),
                        )
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn create_group_chat_sends_private_owner_managed_idempotent_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind create group fixture");
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(None));
        let captured_server = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let captured = Arc::clone(&captured_server);
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let captured = Arc::clone(&captured);
                        async move {
                            let path = request.uri().path().to_string();
                            let query = request.uri().query().unwrap_or_default().to_string();
                            let body = request.into_body().collect().await.unwrap().to_bytes();
                            let response = if path
                                .ends_with("/auth/v3/tenant_access_token/internal")
                            {
                                r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                            } else {
                                *captured.lock().await = Some((path, query, body.to_vec()));
                                r#"{"code":0,"data":{"chat_id":"oc_created","name":"发布 项目群"}}"#
                            };
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "application/json")
                                    .body(Full::new(Bytes::from_static(response.as_bytes())))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let provider = FeishuProvider::new();
        let mut config = provider_with_base_url(Some(&format!("http://{address}/open-apis")));
        config.secret_ref = Some("secret".to_string());
        let created = provider
            .create_group_chat(&config, "发布 项目群", "ou_owner", "stable uuid")
            .await
            .unwrap();
        assert_eq!(created.chat_id, "oc_created");
        assert_eq!(created.name, "发布 项目群");
        let (path, query, body) = captured.lock().await.clone().expect("create request");
        assert_eq!(path, "/open-apis/im/v1/chats");
        let query: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(
            query.get("user_id_type").map(String::as_str),
            Some("open_id")
        );
        assert_eq!(
            query.get("set_bot_manager").map(String::as_str),
            Some("true")
        );
        assert_eq!(query.get("uuid").map(String::as_str), Some("stable uuid"));
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["name"], "发布 项目群");
        assert_eq!(body["owner_id"], "ou_owner");
        assert_eq!(body["chat_mode"], "group");
        assert_eq!(body["chat_type"], "private");
        assert!(body.get("bot_id_list").is_none());
        task.abort();
    }

    async fn create_group_error_fixture(
        body: &'static str,
        request_id: Option<&'static str>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind create group error fixture");
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(move |_request: Request<Incoming>| async move {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json");
                if let Some(request_id) = request_id {
                    response = response.header("x-tt-logid", request_id);
                }
                Ok::<_, hyper::Error>(
                    response
                        .body(Full::new(Bytes::from_static(body.as_bytes())))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        (format!("http://{address}/open-apis"), task)
    }

    fn cache_create_group_token(provider: &FeishuProvider, config: &ImProviderConfig) {
        let base_url = FeishuProvider::base_url(config);
        let cache_key = FeishuProvider::token_cache_key(
            base_url,
            config.app_id.as_deref().unwrap_or_default(),
            config.secret_ref.as_deref().unwrap_or_default(),
        );
        provider.token_cache.write().insert(
            cache_key,
            TokenCache {
                token: "cached".to_string(),
                expires_at: u64::MAX,
            },
        );
    }

    #[tokio::test]
    async fn new_group_chat_reports_api_parse_shape_and_network_errors() {
        for (body, request_id, expected) in [
            ("not-json", None, "response parse failed"),
            (
                r#"{"code":999,"msg":"denied"}"#,
                Some("log-create-1"),
                "code=999, msg=denied, request_id=log-create-1",
            ),
            (r#"{"code":0,"data":{}}"#, None, "missing chat_id"),
        ] {
            let (base_url, server) = create_group_error_fixture(body, request_id).await;
            let mut config = provider_with_base_url(Some(&base_url));
            config.secret_ref = Some("secret".to_string());
            let provider = FeishuProvider::new();
            cache_create_group_token(&provider, &config);
            let error = provider
                .create_group_chat(&config, "发布群", "ou_owner", "uuid")
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "unexpected error: {error}");
            server.abort();
        }

        let (base_url, server) = create_group_error_fixture(
            r#"{"code":0,"data":{"chat_id":"oc_created","name":" "}}"#,
            None,
        )
        .await;
        let mut config = provider_with_base_url(Some(&base_url));
        config.secret_ref = Some("secret".to_string());
        let provider = FeishuProvider::new();
        cache_create_group_token(&provider, &config);
        let created = provider
            .create_group_chat(&config, "回退群名", "ou_owner", "uuid")
            .await
            .unwrap();
        assert_eq!(created.name, "回退群名");
        server.abort();

        let mut unreachable = provider_with_base_url(Some("http://127.0.0.1:9/open-apis"));
        unreachable.secret_ref = Some("secret".to_string());
        let provider = FeishuProvider::new();
        cache_create_group_token(&provider, &unreachable);
        assert!(provider
            .create_group_chat(&unreachable, "失败群", "ou_owner", "uuid")
            .await
            .unwrap_err()
            .to_string()
            .contains("request failed"));
    }

    #[test]
    fn test_default_text_card_wraps_plain_text_as_markdown_card() {
        let card = build_default_text_card("**hello**\n\n- from Bifrost");

        assert_eq!(card["schema"], "2.0");
        assert!(card.get("header").is_none());
        assert_eq!(card["body"]["elements"][0]["tag"], "markdown");
        assert_eq!(
            card["body"]["elements"][0]["content"],
            "**hello**\n\n- from Bifrost"
        );
    }

    #[test]
    fn test_token_cache_key_isolated_by_app_id() {
        let first = FeishuProvider::token_cache_key(DEFAULT_BASE_URL, "cli_first", "secret");
        let second = FeishuProvider::token_cache_key(DEFAULT_BASE_URL, "cli_second", "secret");

        assert_ne!(first, second);
    }

    #[test]
    fn test_token_cache_key_isolated_by_secret() {
        let first = FeishuProvider::token_cache_key(DEFAULT_BASE_URL, "cli_same", "secret_first");
        let second = FeishuProvider::token_cache_key(DEFAULT_BASE_URL, "cli_same", "secret_second");

        assert_ne!(first, second);
    }

    #[test]
    fn test_token_cache_key_isolated_by_base_url() {
        let first = FeishuProvider::token_cache_key(DEFAULT_BASE_URL, "cli_same", "secret_same");
        let second = FeishuProvider::token_cache_key(
            "https://open.larksuite.com/open-apis",
            "cli_same",
            "secret_same",
        );

        assert_ne!(first, second);
    }

    #[test]
    fn test_parse_feishu_chat_name_requires_successful_non_empty_name() {
        assert_eq!(
            parse_feishu_chat_name(&serde_json::json!({
                "code": 0,
                "msg": "success",
                "data": {"name": " 我的工作空间。 "}
            }))
            .unwrap(),
            "我的工作空间。"
        );
        assert!(parse_feishu_chat_name(&serde_json::json!({
            "code": 99991672,
            "msg": "Access denied"
        }))
        .unwrap_err()
        .contains("code=99991672"));
        assert!(parse_feishu_chat_name(&serde_json::json!({
            "code": 0,
            "data": {"name": " "}
        }))
        .unwrap_err()
        .contains("missing data.name"));
    }

    #[tokio::test]
    async fn fetch_group_identity_and_chat_name_use_api_and_cache_successfully() {
        let (base_url, server) = spawn_feishu_api_server(
            r#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Bifrost"}}"#,
            r#"{"code":0,"data":{"name":" Engineering "}}"#,
            r#"{"code":0,"data":{"message_id":"om_reply"}}"#,
        )
        .await;
        let mut config = provider_with_base_url(Some(&base_url));
        config.secret_ref = Some("secret".to_string());
        let provider = FeishuProvider::new();

        let identity = provider.fetch_bot_identity(&config).await.unwrap();
        assert_eq!(identity.open_id, "ou_bot");
        assert_eq!(identity.name.as_deref(), Some("Bifrost"));
        assert_eq!(
            provider.fetch_bot_identity(&config).await.unwrap(),
            identity
        );
        assert_eq!(
            provider.fetch_chat_name(&config, "oc_group").await.unwrap(),
            "Engineering"
        );
        let reply = provider
            .reply_card(
                &config,
                "om_source",
                serde_json::json!({"header":{"title":{"content":"remove"}},"elements":[]}),
                Some("reply-uuid"),
            )
            .await
            .unwrap();
        assert_eq!(reply.message_id.as_deref(), Some("om_reply"));
        provider
            .patch_card(
                &config,
                "om_reply",
                serde_json::json!({"header":{"title":{"content":"remove"}},"elements":[]}),
            )
            .await
            .unwrap();
        server.abort();

        let (base_url, server) = spawn_feishu_api_server(
            r#"{"code":0,"bot":{"open_id":"ou_bot"}}"#,
            r#"{"code":0,"data":{"name":"Engineering"}}"#,
            "not-json",
        )
        .await;
        let mut config = provider_with_base_url(Some(&base_url));
        config.secret_ref = Some("secret".to_string());
        assert!(FeishuProvider::new()
            .reply_card(
                &config,
                "om_source",
                serde_json::json!({"elements":[]}),
                None,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("reply response parse failed"));
        server.abort();
    }

    #[tokio::test]
    async fn fetch_group_identity_reports_api_shape_parse_and_network_errors() {
        for (bot_body, expected) in [
            (r#"{"code":999,"msg":"denied"}"#, "code=999"),
            (r#"{"code":0,"data":{}}"#, "missing bot"),
            (
                r#"{"code":0,"bot":{"open_id":" ","bot_name":"Bifrost"}}"#,
                "missing bot.open_id",
            ),
            ("not-json", "response parse failed"),
        ] {
            let (base_url, server) = spawn_feishu_api_server(
                bot_body,
                r#"{"code":0,"data":{"name":"Group"}}"#,
                r#"{"code":0}"#,
            )
            .await;
            let mut config = provider_with_base_url(Some(&base_url));
            config.secret_ref = Some("secret".to_string());
            let error = FeishuProvider::new()
                .fetch_bot_identity(&config)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            server.abort();
        }

        let (base_url, server) = spawn_feishu_api_server(
            r#"{"code":0,"bot":{"open_id":"ou_bot","bot_name":"Fallback Name"}}"#,
            "not-json",
            r#"{"code":0}"#,
        )
        .await;
        let mut config = provider_with_base_url(Some(&base_url));
        config.secret_ref = Some("secret".to_string());
        let provider = FeishuProvider::new();
        let identity = provider.fetch_bot_identity(&config).await.unwrap();
        assert_eq!(identity.name.as_deref(), Some("Fallback Name"));
        assert!(provider
            .fetch_chat_name(&config, "oc_group")
            .await
            .unwrap_err()
            .to_string()
            .contains("response parse failed"));
        server.abort();

        let mut unreachable = provider_with_base_url(Some("http://127.0.0.1:9"));
        unreachable.secret_ref = Some("secret".to_string());
        let provider = FeishuProvider::new();
        let cache_key = FeishuProvider::token_cache_key(
            FeishuProvider::base_url(&unreachable),
            unreachable.app_id.as_deref().unwrap(),
            "secret",
        );
        provider.token_cache.write().insert(
            cache_key,
            TokenCache {
                token: "cached".to_string(),
                expires_at: u64::MAX,
            },
        );
        assert!(provider
            .fetch_bot_identity(&unreachable)
            .await
            .unwrap_err()
            .to_string()
            .contains("request failed"));
        assert!(provider
            .fetch_chat_name(&unreachable, "oc_group")
            .await
            .unwrap_err()
            .to_string()
            .contains("request failed"));
        assert!(provider
            .reply_card(
                &unreachable,
                "om_source",
                serde_json::json!({"elements":[]}),
                None,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("reply request failed"));
    }

    #[test]
    fn test_normalize_feishu_message_receive_event() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_001",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_xxx",
                    "message_type": "text",
                    "content": "{\"text\":\"/check bifrost\"}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc",
                        "user_id": "uid_123"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();

        assert_eq!(event.event_id, "evt_001");
        assert_eq!(event.provider_id, "feishu-main");
        assert_eq!(event.provider_type, ImProviderType::Feishu);
        assert_eq!(event.event_type, "message.receive");
        assert_eq!(event.source.chat_id.as_deref(), Some("oc_xxx"));
        assert_eq!(event.source.user_id.as_deref(), Some("ou_abc"));
        assert_eq!(event.source.message_id.as_deref(), Some("om_xxx"));

        let msg = event.message.unwrap();
        assert_eq!(msg.text, "/check bifrost");
        assert_eq!(msg.raw_type.as_deref(), Some("text"));
        assert!(event.raw_digest.unwrap().starts_with("sha256:"));
    }

    #[test]
    fn test_normalize_feishu_bot_added_event() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt-bot-added",
                "event_type": "im.chat.member.bot.added_v1"
            },
            "event": {
                "chat_id": "oc_permission_group",
                "operator_id": {"open_id": "ou_admin"}
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").expect("normalized event");
        assert_eq!(
            event.event_type,
            crate::im_gateway::feishu_group_permission::BOT_JOINED_EVENT_TYPE
        );
        assert_eq!(event.event_id, "evt-bot-added");
        assert_eq!(event.source.chat_id.as_deref(), Some("oc_permission_group"));
        assert_eq!(event.source.chat_type.as_deref(), Some("group"));
        assert!(event.message.is_none());
        assert!(event.raw_digest.is_some());
    }

    #[test]
    fn test_normalize_feishu_bot_added_event_requires_chat_id() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt-bot-added",
                "event_type": "im.chat.member.bot.added_v1"
            },
            "event": {}
        });

        assert!(normalize_feishu_event(&raw, "feishu-main").is_none());
    }

    #[test]
    fn test_parse_scope_grant_page_distinguishes_granted_and_missing() {
        let granted = serde_json::json!({
            "code": 0,
            "data": {
                "scopes": [
                    {"scope_name": "im:message.group_msg", "grant_status": 1}
                ],
                "has_more": false
            }
        });
        assert!(
            parse_scope_grant_page(&granted, "im:message.group_msg")
                .unwrap()
                .granted
        );

        let missing = serde_json::json!({
            "code": 0,
            "data": {
                "scopes": [
                    {"scope_name": "im:message.group_msg", "grant_status": 2}
                ],
                "has_more": true,
                "page_token": "next-page"
            }
        });
        let page = parse_scope_grant_page(&missing, "im:message.group_msg").unwrap();
        assert!(!page.granted);
        assert!(page.has_more);
        assert_eq!(page.page_token.as_deref(), Some("next-page"));
    }

    #[test]
    fn test_parse_scope_grant_page_rejects_api_error_and_missing_scopes() {
        assert!(parse_scope_grant_page(
            &serde_json::json!({"code": 999, "msg": "denied"}),
            "im:message.group_msg"
        )
        .unwrap_err()
        .contains("code=999"));
        assert!(parse_scope_grant_page(
            &serde_json::json!({"code": 0, "data": {}}),
            "im:message.group_msg"
        )
        .unwrap_err()
        .contains("data.scopes"));
    }

    #[tokio::test]
    async fn scope_grant_status_follows_pagination_and_rejects_invalid_responses() {
        async fn fixture(
            responses: Vec<(StatusCode, &'static str)>,
        ) -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let responses = Arc::new(parking_lot::Mutex::new(responses.into_iter()));
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let responses = Arc::clone(&responses);
                    tokio::spawn(async move {
                        let service = service_fn(move |request: Request<Incoming>| {
                            let responses = Arc::clone(&responses);
                            async move {
                                let (status, body) = if request
                                    .uri()
                                    .path()
                                    .ends_with("/auth/v3/tenant_access_token/internal")
                                {
                                    (
                                        StatusCode::OK,
                                        r#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                                    )
                                } else {
                                    responses
                                        .lock()
                                        .next()
                                        .unwrap_or((StatusCode::NOT_FOUND, r#"{"code":404}"#))
                                };
                                Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(status)
                                        .header("Content-Type", "application/json")
                                        .body(Full::new(Bytes::from_static(body.as_bytes())))
                                        .unwrap(),
                                )
                            }
                        });
                        let _ = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
            (format!("http://{address}/open-apis"), task)
        }

        let (base_url, server) = fixture(vec![
            (
                StatusCode::OK,
                r#"{"code":0,"data":{"scopes":[],"has_more":true,"page_token":"next"}}"#,
            ),
            (
                StatusCode::OK,
                r#"{"code":0,"data":{"scopes":[{"scope_name":"im:message.group_msg","grant_status":1}],"has_more":false}}"#,
            ),
        ])
        .await;
        let mut config = provider_with_base_url(Some(&base_url));
        config.secret_ref = Some("secret".to_string());
        assert_eq!(
            FeishuProvider::new()
                .scope_grant_status(&config, "im:message.group_msg")
                .await
                .unwrap(),
            FeishuScopeGrantStatus::Granted
        );
        server.abort();

        for (status, body, expected) in [
            (
                StatusCode::OK,
                r#"{"code":0,"data":{"scopes":[],"has_more":true}}"#,
                "has_more without page_token",
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"code":0}"#,
                "status=500",
            ),
            (
                StatusCode::OK,
                "not-json",
                "parse Feishu application scopes response",
            ),
        ] {
            let (base_url, server) = fixture(vec![(status, body)]).await;
            let mut config = provider_with_base_url(Some(&base_url));
            config.secret_ref = Some("secret".to_string());
            let error = FeishuProvider::new()
                .scope_grant_status(&config, "im:message.group_msg")
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "unexpected error: {error}");
            server.abort();
        }
    }

    #[test]
    fn test_normalize_feishu_group_message_preserves_mentions_and_thread_metadata() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_group",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000999"
            },
            "event": {
                "message": {
                    "chat_id": "oc_group",
                    "chat_type": "group",
                    "message_id": "om_group",
                    "message_type": "text",
                    "create_time": "1710000000111",
                    "update_time": "1710000000222",
                    "root_id": "om_root",
                    "parent_id": "om_parent",
                    "thread_id": "omt_thread",
                    "content": "{\"text\":\"@_user_1 inspect this\"}",
                    "mentions": [{
                        "key": "@_user_1",
                        "id": {"open_id": "ou_bot"},
                        "name": "Bifrost",
                        "tenant_key": "tenant-a"
                    }]
                },
                "sender": {
                    "sender_type": "user",
                    "sender_id": {"open_id": "ou_alice"}
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        assert_eq!(event.source.chat_type.as_deref(), Some("group"));
        assert_eq!(event.source.sender_type.as_deref(), Some("user"));
        let message = event.message.unwrap();
        assert_eq!(message.create_time, Some(1_710_000_000_111));
        assert_eq!(message.update_time, Some(1_710_000_000_222));
        assert_eq!(message.root_id.as_deref(), Some("om_root"));
        assert_eq!(message.parent_id.as_deref(), Some("om_parent"));
        assert_eq!(message.thread_id.as_deref(), Some("omt_thread"));
        assert_eq!(message.mentions.len(), 1);
        assert_eq!(message.mentions[0].key, "@_user_1");
        assert_eq!(message.mentions[0].open_id.as_deref(), Some("ou_bot"));
        assert_eq!(message.mentions[0].name.as_deref(), Some("Bifrost"));
        assert!(!message.mentions[0].is_bot);
    }

    #[test]
    fn test_normalize_feishu_post_restores_stable_mention_placeholders() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_group_post",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000999"
            },
            "event": {
                "message": {
                    "chat_id": "oc_group",
                    "chat_type": "group",
                    "message_id": "om_group_post",
                    "message_type": "post",
                    "content": serde_json::json!({
                        "content": [[
                            {"tag": "at", "user_id": "ou_bot", "user_name": "Bifrost"},
                            {"tag": "text", "text": " inspect this"},
                            {"tag": "at", "user_id": "ou_alice", "user_name": "Alice"}
                        ]]
                    }).to_string(),
                    "mentions": [
                        {
                            "key": "@_user_1",
                            "id": {"open_id": "ou_bot"},
                            "name": "Bifrost"
                        },
                        {
                            "key": "@_user_2",
                            "id": {"open_id": "ou_alice"},
                            "name": "Alice"
                        }
                    ]
                },
                "sender": {
                    "sender_type": "user",
                    "sender_id": {"open_id": "ou_alice"}
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let message = event.message.unwrap();
        assert_eq!(message.text, "@_user_1 inspect this@_user_2");
        assert_eq!(message.mentions.len(), 2);
    }

    #[test]
    fn test_normalize_feishu_image_message_extracts_resource_key() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_img",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_img",
                    "message_type": "image",
                    "content": "{\"image_key\":\"img_v3_abc\"}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let msg = event.message.unwrap();
        assert_eq!(msg.raw_type.as_deref(), Some("image"));
        assert_eq!(msg.images.len(), 1);
        assert_eq!(msg.images[0].file_key, "img_v3_abc");
        assert_eq!(msg.images[0].source, ImImageSource::MessageResource);
    }

    #[test]
    fn test_normalize_feishu_file_message_extracts_attachment() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_file",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_file",
                    "message_type": "file",
                    "content": "{\"file_key\":\"file_v3_abc\",\"file_name\":\"需求.md\",\"file_size\":12}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let msg = event.message.unwrap();
        assert_eq!(msg.raw_type.as_deref(), Some("file"));
        assert!(msg.images.is_empty());
        assert_eq!(msg.files.len(), 1);
        assert_eq!(msg.files[0].file_key, "file_v3_abc");
        assert_eq!(msg.files[0].name.as_deref(), Some("需求.md"));
        assert_eq!(msg.files[0].size_bytes, Some(12));
    }

    #[test]
    fn test_normalize_feishu_file_message_supports_alias_metadata_fields() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_file_alias",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_file_alias",
                    "message_type": "file",
                    "content": "{\"file_key\":\"file_v3_alias\",\"name\":\"report.txt\",\"mimeType\":\"text/plain\",\"size_bytes\":34}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let msg = event.message.unwrap();
        assert_eq!(msg.files.len(), 1);
        assert_eq!(msg.files[0].file_key, "file_v3_alias");
        assert_eq!(msg.files[0].name.as_deref(), Some("report.txt"));
        assert_eq!(msg.files[0].mime_type.as_deref(), Some("text/plain"));
        assert_eq!(msg.files[0].size_bytes, Some(34));
    }

    #[test]
    fn test_normalize_feishu_file_message_ignores_missing_file_key() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_file_missing_key",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_file_missing_key",
                    "message_type": "file",
                    "content": "{\"file_name\":\"report.txt\",\"mime_type\":\"text/plain\",\"size\":34}"
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let msg = event.message.unwrap();
        assert_eq!(msg.raw_type.as_deref(), Some("file"));
        assert!(msg.files.is_empty());
    }

    #[tokio::test]
    async fn test_download_feishu_file_resource_fetches_message_resource() {
        use http_body_util::{BodyExt, Full};
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Method, Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu file server");
        let port = listener.local_addr().expect("mock local addr").port();
        let saw_file_request = Arc::new(AtomicBool::new(false));
        let saw_file_request_for_server = Arc::clone(&saw_file_request);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let saw_file_request = Arc::clone(&saw_file_request_for_server);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let saw_file_request = Arc::clone(&saw_file_request);
                        async move {
                            let method = req.method().clone();
                            let path = req.uri().path().to_string();
                            let query = req.uri().query().unwrap_or_default().to_string();
                            let auth = req
                                .headers()
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string);
                            let _body = req
                                .into_body()
                                .collect()
                                .await
                                .expect("collect request body")
                                .to_bytes();
                            if method == Method::POST
                                && path == "/open-apis/auth/v3/tenant_access_token/internal"
                            {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(bytes::Bytes::from_static(
                                            br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            if method == Method::GET
                                && path == "/open-apis/im/v1/messages/om_file/resources/file_v3_abc"
                            {
                                assert_eq!(query, "type=file");
                                assert_eq!(auth.as_deref(), Some("Bearer tenant-token"));
                                saw_file_request.store(true, Ordering::SeqCst);
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("content-type", "text/markdown; charset=utf-8")
                                        .body(Full::new(bytes::Bytes::from_static(
                                            b"# Report\n\nhello\n",
                                        )))
                                        .unwrap(),
                                );
                            }
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Full::new(bytes::Bytes::from_static(b"not found")))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let provider = FeishuProvider::new();
        let config = provider_with_base_url(Some(&format!("http://127.0.0.1:{port}/open-apis")));
        let (mime_type, bytes) = provider
            .download_message_file_resource(&config, "om_file", "file_v3_abc")
            .await
            .expect("download file resource");

        assert_eq!(mime_type, "text/markdown");
        assert_eq!(bytes, b"# Report\n\nhello\n");
        assert!(saw_file_request.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_download_feishu_file_resource_reports_http_errors() {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Method, Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu file error server");
        let port = listener.local_addr().expect("mock local addr").port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(
                        move |req: Request<hyper::body::Incoming>| async move {
                            let method = req.method().clone();
                            let path = req.uri().path().to_string();
                            if method == Method::POST
                                && path == "/open-apis/auth/v3/tenant_access_token/internal"
                            {
                                return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(bytes::Bytes::from_static(
                                        br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                    )))
                                    .unwrap(),
                            );
                            }
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::BAD_GATEWAY)
                                    .header("content-type", "application/json")
                                    .body(Full::new(bytes::Bytes::from_static(
                                        br#"{"code":999,"msg":"file unavailable"}"#,
                                    )))
                                    .unwrap(),
                            )
                        },
                    );
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let provider = FeishuProvider::new();
        let config = provider_with_base_url(Some(&format!("http://127.0.0.1:{port}/open-apis")));
        let err = provider
            .download_message_file_resource(&config, "om_file", "file_v3_abc")
            .await
            .expect_err("non-success file resource response is an error");

        let message = err.to_string();
        assert!(message.contains("502 Bad Gateway"));
        assert!(message.contains("file unavailable"));
    }

    #[tokio::test]
    async fn test_download_feishu_file_resource_reports_request_send_errors() {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Method, Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu token-only server");
        let port = listener.local_addr().expect("mock local addr").port();

        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| async move {
                let method = req.method().clone();
                let path = req.uri().path().to_string();
                if method == Method::POST
                    && path == "/open-apis/auth/v3/tenant_access_token/internal"
                {
                    return Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("connection", "close")
                            .body(Full::new(bytes::Bytes::from_static(
                                br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                            )))
                            .unwrap(),
                    );
                }
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Full::new(bytes::Bytes::from_static(b"not found")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new().serve_connection(io, service).await;
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            drop(stream);
        });

        let provider = FeishuProvider::new();
        let config = provider_with_base_url(Some(&format!("http://127.0.0.1:{port}/open-apis")));
        let err = provider
            .download_message_file_resource(&config, "om_file", "file_v3_abc")
            .await
            .expect_err("closed mock server should fail resource request send");

        let message = err.to_string();
        assert!(message.contains("feishu message file download failed"));
    }

    #[tokio::test]
    async fn test_download_feishu_file_resource_reports_truncated_response_errors() {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Method, Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu truncated file server");
        let port = listener.local_addr().expect("mock local addr").port();

        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| async move {
                let method = req.method().clone();
                let path = req.uri().path().to_string();
                if method == Method::POST
                    && path == "/open-apis/auth/v3/tenant_access_token/internal"
                {
                    return Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("connection", "close")
                            .body(Full::new(bytes::Bytes::from_static(
                                br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                            )))
                            .unwrap(),
                    );
                }
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Full::new(bytes::Bytes::from_static(b"not found")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new().serve_connection(io, service).await;

            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/pdf\r\n",
                "Content-Length: 64\r\n",
                "\r\n",
                "short"
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });

        let provider = FeishuProvider::new();
        let config = provider_with_base_url(Some(&format!("http://127.0.0.1:{port}/open-apis")));
        let err = provider
            .download_message_file_resource(&config, "om_file", "file_v3_abc")
            .await
            .expect_err("truncated response body should fail body read");

        let message = err.to_string();
        assert!(
            message.contains("feishu message file body read failed")
                || message.contains("feishu message file download failed"),
            "unexpected truncated response error: {message}"
        );
    }

    #[test]
    fn test_normalize_feishu_post_extracts_text_and_images() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_post_img",
                "event_type": "im.message.receive_v1",
                "create_time": "1710000000000"
            },
            "event": {
                "message": {
                    "chat_id": "oc_xxx",
                    "message_id": "om_post",
                    "message_type": "post",
                    "content": serde_json::json!({
                        "title": "标题",
                        "content": [[
                            {"tag": "text", "text": "请看这张图"},
                            {"tag": "img", "image_key": "img_v3_post"},
                            {"tag": "img", "image_key": "img_v3_post_2"}
                        ]]
                    }).to_string()
                },
                "sender": {
                    "sender_id": {
                        "open_id": "ou_abc"
                    }
                }
            }
        });

        let event = normalize_feishu_event(&raw, "feishu-main").unwrap();
        let msg = event.message.unwrap();
        assert_eq!(msg.raw_type.as_deref(), Some("post"));
        assert_eq!(msg.text, "请看这张图");
        assert_eq!(msg.images.len(), 2);
        assert_eq!(msg.images[0].file_key, "img_v3_post");
        assert_eq!(msg.images[1].file_key, "img_v3_post_2");
    }

    #[test]
    fn test_normalize_unknown_event_type_returns_none() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt_002",
                "event_type": "card.action.trigger",
                "create_time": "1710000000000"
            },
            "event": {}
        });

        let result = normalize_feishu_event(&raw, "feishu-main");
        assert!(result.is_none());
    }

    #[test]
    fn test_normalize_missing_header_returns_none() {
        let raw = serde_json::json!({ "event": {} });
        let result = normalize_feishu_event(&raw, "feishu-main");
        assert!(result.is_none());
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_encode(&[]), "");
    }

    fn provider_with_base_url(base_url: Option<&str>) -> ImProviderConfig {
        ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu".to_string(),
            enabled: true,
            base_url: base_url.map(|s| s.to_string()),
            app_id: Some("cli_test".to_string()),
            secret_ref: None,
            owner_open_id: None,
            event_connection_enabled: true,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn test_ws_domain_trims_open_apis_suffix_and_defaults() {
        let config = provider_with_base_url(Some("https://open.feishu.cn/open-apis"));
        assert_eq!(ws_domain(&config), "https://open.feishu.cn");

        let config_lark = provider_with_base_url(Some("https://open.larksuite.com"));
        assert_eq!(ws_domain(&config_lark), "https://open.larksuite.com");

        let config_default = provider_with_base_url(None);
        assert_eq!(ws_domain(&config_default), "https://open.feishu.cn");
    }

    #[test]
    fn test_feishu_file_type_for_name_maps_extensions() {
        assert_eq!(feishu_file_type_for_name("doc.pdf"), "pdf");
        assert_eq!(feishu_file_type_for_name("report.DOCX"), "doc");
        assert_eq!(feishu_file_type_for_name("data.csv"), "xls");
        assert_eq!(feishu_file_type_for_name("table.XLS"), "xls");
        assert_eq!(feishu_file_type_for_name("slides.PPT"), "ppt");
        assert_eq!(feishu_file_type_for_name("unknown.bin"), "stream");
        assert_eq!(feishu_file_type_for_name("noext"), "stream");
    }

    #[test]
    fn outbound_file_size_accepts_100_mib_and_rejects_larger_or_empty_files() {
        assert!(validate_outbound_file_size(MAX_OUTBOUND_FILE_BYTES).is_ok());

        let oversized = validate_outbound_file_size(MAX_OUTBOUND_FILE_BYTES + 1)
            .expect_err("files above 100 MiB must be rejected before upload");
        assert!(oversized.to_string().contains("100 MiB limit"));

        let empty =
            validate_outbound_file_size(0).expect_err("empty files must be rejected before upload");
        assert!(empty.to_string().contains("requires non-empty bytes"));
    }

    #[test]
    fn test_collect_rich_text_image_keys_recurses_and_deduplicates() {
        let mut images = Vec::new();
        let value = serde_json::json!({
            "tag": "root",
            "content": [
                {"tag": "img", "image_key": "img_v3_1"},
                {
                    "tag": "paragraph",
                    "children": [
                        {"image_key": "img_v3_1"},
                        {"image_key": "img_v3_2"}
                    ]
                }
            ]
        });

        collect_rich_text_image_keys(&value, &mut images);
        let keys: Vec<_> = images.iter().map(|img| img.file_key.as_str()).collect();
        assert_eq!(keys, vec!["img_v3_1", "img_v3_2"]);
    }

    #[test]
    fn test_extract_feishu_message_text_from_rich_nodes() {
        let rich = serde_json::json!({
            "content": [[
                {"tag": "text", "text": "Hello"},
                {"tag": "a", "text": " world"},
                {"tag": "br"},
                {"tag": "at", "user_name": "Alice"}
            ]]
        });
        let mentions = vec![ImMention {
            key: "@_user_1".to_string(),
            open_id: Some("ou_alice".to_string()),
            name: Some("Alice".to_string()),
            tenant_key: None,
            is_bot: false,
        }];
        let text = extract_feishu_message_text(&rich, &mentions);
        assert_eq!(text, "Hello world\n@_user_1");

        let unmatched = extract_feishu_message_text(&rich, &[]);
        assert_eq!(unmatched, "Hello world\nAlice");

        let duplicate_name_mentions = vec![
            ImMention {
                key: "@_wrong_name_match".to_string(),
                open_id: Some("ou_other".to_string()),
                name: Some("Alice".to_string()),
                tenant_key: None,
                is_bot: false,
            },
            mentions[0].clone(),
        ];
        let rich_with_id = serde_json::json!({
            "content": [[{"tag": "at", "user_id": "ou_alice", "user_name": "Alice"}]]
        });
        assert_eq!(
            extract_feishu_message_text(&rich_with_id, &duplicate_name_mentions),
            "@_user_1"
        );

        let plain = serde_json::json!({"text": "plain text"});
        assert_eq!(extract_feishu_message_text(&plain, &mentions), "plain text");
    }

    #[test]
    fn test_json_object_keys_extracts_top_level_keys() {
        let value = serde_json::json!({"a": 1, "b": 2});
        let mut keys = json_object_keys(&value);
        keys.sort();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn test_publish_connection_status_sends_event_when_channel_present() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FeishuConnectionStatusEvent>();
        publish_connection_status(
            Some(&tx),
            ConnectionState::Connected,
            Some("ok".to_string()),
        );
        let event = rx.try_recv().expect("status event");
        assert!(matches!(event.state, ConnectionState::Connected));
        assert_eq!(event.error.as_deref(), Some("ok"));

        // No panic when channel is absent
        publish_connection_status(None, ConnectionState::Connected, None);
    }

    #[test]
    fn test_get_header_value_and_ping_response_frames() {
        let headers = vec![
            PbHeader {
                key: "type".to_string(),
                value: "ping".to_string(),
            },
            PbHeader {
                key: "x-request-id".to_string(),
                value: "req-1".to_string(),
            },
        ];
        assert_eq!(get_header_value(&headers, "type"), Some("ping"));
        assert_eq!(get_header_value(&headers, "missing"), None);

        let ping = build_ping_frame(42);
        let decoded = PbFrame::decode(&*ping).expect("decode ping frame");
        assert_eq!(decoded.service, 42);
        assert_eq!(get_header_value(&decoded.headers, "type"), Some("ping"));

        let ok_bytes = build_response_frame(&decoded, true);
        let ok_frame = PbFrame::decode(&*ok_bytes).expect("decode ok frame");
        let ok_body: serde_json::Value =
            serde_json::from_slice(ok_frame.payload.as_deref().unwrap()).unwrap();
        assert_eq!(ok_body["code"], 200);

        let err_bytes = build_response_frame(&decoded, false);
        let err_frame = PbFrame::decode(&*err_bytes).expect("decode err frame");
        let err_body: serde_json::Value =
            serde_json::from_slice(err_frame.payload.as_deref().unwrap()).unwrap();
        assert_eq!(err_body["code"], 500);
    }

    #[test]
    fn topic_reply_request_sets_reply_in_thread() {
        let request = build_reply_request("interactive", "{}", Some("uuid-1"), true);
        assert_eq!(request["reply_in_thread"], true);
        assert_eq!(request["uuid"], "uuid-1");
        let ordinary = build_reply_request("interactive", "{}", None, false);
        assert_eq!(ordinary["reply_in_thread"], false);
        assert!(ordinary.get("uuid").is_none());
    }

    #[tokio::test]
    async fn message_resource_body_enforces_header_and_stream_limits() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind message resource server");
        let address = listener.local_addr().expect("message resource address");
        let server = tokio::spawn(async move {
            for response in [
                "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept().await.expect("accept resource request");
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request).await;
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write resource response");
            }
        });
        let http = reqwest::Client::new();

        let header_response = http
            .get(format!("http://{address}/header"))
            .send()
            .await
            .expect("header response");
        let header_error = read_feishu_message_resource_body(header_response, "file", 4)
            .await
            .expect_err("content-length must be checked before buffering");
        assert!(header_error.to_string().contains("0 MiB 上限"));

        let stream_response = http
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .expect("stream response");
        let stream_error = read_feishu_message_resource_body(stream_response, "image", 4)
            .await
            .expect_err("streamed cumulative size must be checked");
        assert!(stream_error.to_string().contains("image"));
        server.await.expect("message resource server");
    }

    #[test]
    fn parse_message_attachments_ignores_blank_resource_keys() {
        let (images, files) =
            parse_feishu_message_attachments("image", &serde_json::json!({"image_key": "   "}));
        assert!(images.is_empty());
        assert!(files.is_empty());

        let (images, files) = parse_feishu_message_attachments(
            "file",
            &serde_json::json!({"file_key": "", "file_name": "blank.txt"}),
        );
        assert!(images.is_empty());
        assert!(files.is_empty());
    }
}
