use std::collections::HashMap;
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
    ConnectionHandle, ConnectionState, ImEvent, ImEventMessage, ImEventSource, ImImageAttachment,
    ImImageSource, ImProviderConfig, ImProviderType, ImTarget, ProviderValidation, SendOptions,
    SendResult, UploadedImage,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_BASE_URL: &str = "https://open.feishu.cn/open-apis";
const TOKEN_REFRESH_AHEAD_SECS: u64 = 300; // 5 minutes before expiry
const MAX_BACKOFF_SECS: u64 = 60;
const INITIAL_BACKOFF_SECS: u64 = 1;
const WS_PING_INTERVAL_SECS: u64 = 90;
/// Maximum time we're willing to wait for *any* server-originated traffic
/// (event / pong / server-initiated ping) before considering the connection
/// silently dead and forcing a reconnect. Must be strictly larger than the
/// server-advertised ping interval so a single dropped pong doesn't trip it.
const WS_SERVER_SILENCE_TIMEOUT_SECS: u64 = 180;
/// How often the silence watchdog wakes up to re-check `last_server_msg_at`.
const WS_SILENCE_CHECK_INTERVAL_SECS: u64 = 15;
const DEFAULT_TEXT_CARD_TITLE: &str = "Bifrost";

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
        }
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
}

pub(crate) fn build_default_text_card(text: &str) -> serde_json::Value {
    build_markdown_card(DEFAULT_TEXT_CARD_TITLE, text, "blue")
}

pub(crate) fn build_markdown_card(
    title: &str,
    markdown: &str,
    template: &str,
) -> serde_json::Value {
    let title = title.trim();
    let title = if title.is_empty() {
        DEFAULT_TEXT_CARD_TITLE
    } else {
        title
    };
    let template = template.trim();
    let template = if template.is_empty() {
        "blue"
    } else {
        template
    };
    let markdown = crate::im_gateway::markdown_converter::convert_to_feishu_markdown(markdown);
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": template,
            "title": {
                "tag": "plain_text",
                "content": title
            }
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

// ---------------------------------------------------------------------------
// ImProvider trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ImProvider for FeishuProvider {
    fn provider_type(&self) -> ImProviderType {
        ImProviderType::Feishu
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
        });

        Ok(ConnectionHandle { shutdown_tx })
    }

    async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let app_secret = config.secret_ref.as_deref().unwrap_or_default();
        let token = self.get_tenant_token(config, app_secret).await?;

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

    async fn send_text(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> Result<SendResult> {
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
            None,
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
}

// ---------------------------------------------------------------------------
// Additional Feishu-specific methods
// ---------------------------------------------------------------------------

impl FeishuProvider {
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
        let bytes = resp.bytes().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "feishu message image body read failed: {}",
                e
            ))
        })?;
        Ok((mime_type, bytes.to_vec()))
    }
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
                                                    handle_ws_message(text, &config.id, sink);
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

/// Handle a text message from the Feishu WebSocket.
fn handle_ws_message(text: &str, provider_id: &str, sink: &EventSink) {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!(provider_id = provider_id, error = %e, "failed to parse feishu ws message");
            return;
        }
    };

    // Check if this is a heartbeat/pong from Feishu's protocol layer
    if let Some(msg_type) = parsed.get("type").and_then(|v| v.as_str()) {
        if msg_type == "pong" || msg_type == "heartbeat" {
            debug!(
                provider_id = provider_id,
                "received feishu protocol heartbeat"
            );
            return;
        }
    }

    // Normalize and dispatch event
    if let Some(event) = normalize_feishu_event(&parsed, provider_id) {
        if let Err(e) = sink.send(event) {
            error!(
                provider_id = provider_id,
                error = %e,
                "failed to send event to sink, receiver may be dropped"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Event Normalization
// ---------------------------------------------------------------------------

/// Normalize a raw Feishu event into the unified ImEvent model.
///
/// Handles `im.message.receive_v1` events.
pub fn normalize_feishu_event(raw: &serde_json::Value, provider_id: &str) -> Option<ImEvent> {
    let header = raw.get("header")?;
    let event_id = header.get("event_id").and_then(|v| v.as_str())?.to_string();
    let event_type_raw = header.get("event_type").and_then(|v| v.as_str())?;

    // Only handle im.message.receive_v1 for V1
    let normalized_event_type = match event_type_raw {
        "im.message.receive_v1" => "message.receive",
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

    let content_obj = message
        .get("content")
        .and_then(|v| v.as_str())
        .and_then(|content_str| serde_json::from_str::<serde_json::Value>(content_str).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // Extract text content from message.content. Feishu rich text (`post`)
    // stores plain text in nested `content` arrays rather than top-level text.
    let text = extract_feishu_message_text(&content_obj);

    let mut images = Vec::new();
    if message_type.as_deref() == Some("image") {
        if let Some(image_key) = content_obj.get("image_key").and_then(|v| v.as_str()) {
            images.push(ImImageAttachment {
                file_key: image_key.to_string(),
                source: ImImageSource::MessageResource,
                mime_type: None,
                data_base64: None,
                download_url: None,
                encrypted_query_param: None,
                aes_key: None,
            });
        }
    }
    collect_rich_text_image_keys(&content_obj, &mut images);

    info!(
        provider_id = %provider_id,
        event_id = %event_id,
        message_id = ?message_id,
        message_type = ?message_type,
        text_len = text.len(),
        image_count = images.len(),
        image_keys = %images
            .iter()
            .map(|image| image.file_key.as_str())
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
            user_id,
            message_id,
        },
        message: Some(ImEventMessage {
            text,
            mentions: Vec::new(),
            images,
            raw_type: message_type,
        }),
        received_at: now,
        raw_digest: Some(raw_digest),
    })
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
            if let Some(image_key) = map.get("image_key").and_then(|v| v.as_str()) {
                if !images.iter().any(|image| image.file_key == image_key) {
                    images.push(ImImageAttachment {
                        file_key: image_key.to_string(),
                        source: ImImageSource::MessageResource,
                        mime_type: None,
                        data_base64: None,
                        download_url: None,
                        encrypted_query_param: None,
                        aes_key: None,
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

fn extract_feishu_message_text(value: &serde_json::Value) -> String {
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    let mut parts = Vec::new();
    collect_feishu_text_nodes(value, &mut parts);
    parts.join("").trim().to_string()
}

fn collect_feishu_text_nodes(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str());
            if matches!(tag, Some("text" | "a" | "code_block")) {
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            if tag == Some("at") {
                if let Some(text) = map
                    .get("user_name")
                    .or_else(|| map.get("user_id"))
                    .and_then(|v| v.as_str())
                {
                    parts.push(text.to_string());
                }
            }
            if tag == Some("br") {
                parts.push("\n".to_string());
            }
            for child in map.values() {
                collect_feishu_text_nodes(child, parts);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_feishu_text_nodes(item, parts);
            }
        }
        _ => {}
    }
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

    #[test]
    fn test_default_text_card_wraps_plain_text_as_markdown_card() {
        let card = build_default_text_card("**hello**\n\n- from Bifrost");

        assert_eq!(card["schema"], "2.0");
        assert_eq!(card["header"]["title"]["content"], "Bifrost");
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
        let text = extract_feishu_message_text(&rich);
        assert_eq!(text, "Hello world\nAlice");

        let plain = serde_json::json!({"text": "plain text"});
        assert_eq!(extract_feishu_message_text(&plain), "plain text");
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
}
