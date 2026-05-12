use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
use async_trait::async_trait;
use base64::Engine as _;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use bifrost_core::Result;

use crate::im_gateway::provider::{EventSink, ImProvider};
use crate::im_gateway::types::{
    ConnectionHandle, ImEvent, ImEventMessage, ImEventSource, ImImageAttachment, ImImageSource,
    ImProviderConfig, ImProviderType, ImTarget, ProviderValidation, SendOptions, SendResult,
    UploadedImage,
};

const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const DEFAULT_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
const DEFAULT_POLL_INTERVAL_MS: u64 = 3_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinLoginStart {
    pub poll_key: String,
    pub scan_url: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinLoginAccount {
    pub account_id: String,
    pub user_id: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub bot_token: String,
}

#[derive(Default)]
struct AccountRuntime {
    get_updates_buf: String,
    context_tokens: HashMap<String, String>,
}

pub struct WeixinProvider {
    http: reqwest::Client,
    runtime: Arc<RwLock<HashMap<String, AccountRuntime>>>,
}

impl Default for WeixinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WeixinProvider {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            http,
            runtime: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn base_url(config: &ImProviderConfig) -> &str {
        config
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
    }

    fn account_id(config: &ImProviderConfig) -> &str {
        config.app_id.as_deref().unwrap_or("weixin-bot")
    }

    fn bot_token(config: &ImProviderConfig) -> Result<&str> {
        config.secret_ref.as_deref().ok_or_else(|| {
            bifrost_core::BifrostError::Config(
                "weixin provider requires bot token; complete QR login first".to_string(),
            )
        })
    }

    pub async fn start_login(&self, base_url: Option<&str>) -> Result<WeixinLoginStart> {
        let base_url = base_url.unwrap_or(DEFAULT_BASE_URL).trim_end_matches('/');
        let url = format!("{base_url}/ilink/bot/get_bot_qrcode?bot_type=3");
        #[derive(Deserialize)]
        struct QrResponse {
            qrcode: Option<String>,
            qrcode_img_content: Option<String>,
            qrcode_img_url: Option<String>,
            url: Option<String>,
        }
        let body: QrResponse = self
            .http
            .get(url)
            .header("iLink-App-ClientVersion", "1")
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("weixin qr request failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("weixin qr response parse failed: {e}"))
            })?;
        let poll_key = body.qrcode.ok_or_else(|| {
            bifrost_core::BifrostError::Network("weixin qr response missing qrcode key".to_string())
        })?;
        let scan_url = body
            .qrcode_img_content
            .or(body.qrcode_img_url)
            .or(body.url)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "weixin qr response missing qrcode_img_content URL".to_string(),
                )
            })?;
        Ok(WeixinLoginStart {
            poll_key,
            scan_url,
            expires_in_seconds: 60,
        })
    }

    pub async fn complete_login(
        &self,
        poll_key: &str,
        base_url: Option<&str>,
        max_attempts: usize,
        interval: Duration,
    ) -> Result<WeixinLoginAccount> {
        let mut host = base_url
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
            .to_string();
        for attempt in 0..max_attempts {
            let url = format!(
                "{host}/ilink/bot/get_qrcode_status?qrcode={}",
                urlencoding::encode(poll_key)
            );
            let status: serde_json::Value = self
                .http
                .get(url)
                .header("iLink-App-ClientVersion", "1")
                .send()
                .await
                .map_err(|e| {
                    bifrost_core::BifrostError::Network(format!(
                        "weixin qr status request failed: {e}"
                    ))
                })?
                .json()
                .await
                .map_err(|e| {
                    bifrost_core::BifrostError::Network(format!(
                        "weixin qr status response parse failed: {e}"
                    ))
                })?;
            let state = status
                .get("status")
                .or_else(|| status.get("state"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if state == "expired" {
                return Err(bifrost_core::BifrostError::Network(
                    "weixin QR code expired; start a new login".to_string(),
                ));
            }
            if state == "scaned_but_redirect" {
                if let Some(redirect_host) = status.get("redirect_host").and_then(|v| v.as_str()) {
                    host = redirect_host.trim_end_matches('/').to_string();
                }
            }
            if state == "confirmed"
                || status.get("bot_token").is_some()
                || status.get("access_token").is_some()
            {
                return Self::normalize_login_account(status, &host);
            }
            if attempt + 1 < max_attempts {
                tokio::time::sleep(interval).await;
            }
        }
        Err(bifrost_core::BifrostError::Network(
            "weixin QR login timed out".to_string(),
        ))
    }

    fn normalize_login_account(raw: serde_json::Value, host: &str) -> Result<WeixinLoginAccount> {
        let bot_token = raw
            .get("bot_token")
            .or_else(|| raw.get("access_token"))
            .or_else(|| raw.get("token"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "confirmed weixin login missing bot token".to_string(),
                )
            })?
            .to_string();
        let account_id = raw
            .get("ilink_bot_id")
            .or_else(|| raw.get("account_id"))
            .or_else(|| raw.get("bot_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("weixin-bot")
            .to_string();
        let user_id = raw
            .get("ilink_user_id")
            .or_else(|| raw.get("user_id"))
            .or_else(|| raw.get("wxid"))
            .and_then(|v| v.as_str())
            .unwrap_or(&account_id)
            .to_string();
        let base_url = raw
            .get("baseurl")
            .or_else(|| raw.get("base_url"))
            .and_then(|v| v.as_str())
            .unwrap_or(host)
            .trim_end_matches('/')
            .to_string();
        Ok(WeixinLoginAccount {
            account_id,
            user_id,
            base_url,
            bot_token,
        })
    }

    async fn poll_once(&self, config: &ImProviderConfig) -> Result<Vec<ImEvent>> {
        let base_url = Self::base_url(config);
        let account_id = Self::account_id(config).to_string();
        let bot_token = Self::bot_token(config)?;
        let get_updates_buf = self
            .runtime
            .read()
            .get(&account_id)
            .map(|runtime| runtime.get_updates_buf.clone())
            .unwrap_or_default();
        let url = format!("{base_url}/ilink/bot/getupdates");
        let response: serde_json::Value = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .header("iLink-App-ClientVersion", "1")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&serde_json::json!({ "get_updates_buf": get_updates_buf }))
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("weixin getupdates failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin getupdates response parse failed: {e}"
                ))
            })?;

        if let Some(next_buf) = response
            .get("get_updates_buf")
            .or_else(|| response.get("next_get_updates_buf"))
            .or_else(|| response.get("sync_buf"))
            .and_then(|v| v.as_str())
        {
            self.runtime
                .write()
                .entry(account_id.clone())
                .or_default()
                .get_updates_buf = next_buf.to_string();
        }
        let updates = response
            .get("updates")
            .or_else(|| response.get("messages"))
            .or_else(|| response.get("msgs"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut events = Vec::new();
        for update in updates {
            if let Some(context_token) = update
                .get("context_token")
                .or_else(|| update.get("contextToken"))
                .and_then(|v| v.as_str())
            {
                if let Some(from) = Self::sender_field(&update) {
                    self.runtime
                        .write()
                        .entry(account_id.clone())
                        .or_default()
                        .context_tokens
                        .insert(from, context_token.to_string());
                }
            }
            events.push(Self::normalize_update(config, &account_id, update));
        }
        Ok(events)
    }

    fn normalize_update(
        config: &ImProviderConfig,
        account_id: &str,
        update: serde_json::Value,
    ) -> ImEvent {
        let from = Self::sender_field(&update).unwrap_or_else(|| "unknown@im.wechat".to_string());
        let raw_json = serde_json::to_string(&update).unwrap_or_default();
        let message_id = Self::string_or_number_field(
            &update,
            &[
                "message_id",
                "msgid",
                "msg_id",
                "id",
                "client_msg_id",
                "new_msg_id",
            ],
        )
        .unwrap_or_else(|| format!("weixin-{}-{:016x}", account_id, stable_hash(&raw_json)));
        let images = Self::message_images(&update);
        let mut text = Self::message_text(&update);
        if text.trim().is_empty() && images.is_empty() {
            text = truncate_chars(&raw_json, 2_000);
        }
        let raw_type = Self::string_or_number_field(
            &update,
            &[
                "raw_type",
                "msg_type",
                "message_type",
                "type",
                "content_type",
            ],
        )
        .unwrap_or_else(|| "text".to_string());
        ImEvent {
            event_id: message_id.clone(),
            provider_id: config.id.clone(),
            provider_type: ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: ImEventSource {
                chat_id: Some(from.clone()),
                user_id: Some(from),
                message_id: Some(message_id),
            },
            message: Some(ImEventMessage {
                text,
                mentions: Vec::new(),
                images,
                raw_type: Some(raw_type),
            }),
            received_at: now_ms(),
            raw_digest: Some(truncate_chars(&raw_json, 2_000)),
        }
    }

    fn sender_field(value: &serde_json::Value) -> Option<String> {
        Self::string_field(
            value,
            &[
                "from_user_id",
                "from",
                "sender",
                "sender_user_id",
                "talker",
                "from_username",
                "from_user",
                "user_id",
                "wxid",
            ],
        )
    }

    fn message_text(value: &serde_json::Value) -> String {
        if let Some(text) = Self::string_field(value, &["text", "content", "msg", "message"]) {
            return Self::extract_text_from_string(&text);
        }
        for pointer in [
            "/content/text",
            "/content/content",
            "/message/text",
            "/message/content",
            "/msg/text",
            "/msg/content",
        ] {
            if let Some(text) = value.pointer(pointer).and_then(|v| v.as_str()) {
                return Self::extract_text_from_string(text);
            }
        }
        if let Some(items) = value.get("item_list").and_then(|v| v.as_array()) {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.pointer("/text_item/text")
                        .or_else(|| item.pointer("/text_item/content"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return text;
            }
        }
        String::new()
    }

    fn message_images(value: &serde_json::Value) -> Vec<ImImageAttachment> {
        let mut images = Vec::new();
        if let Some(items) = value.get("item_list").and_then(|v| v.as_array()) {
            for item in items {
                Self::push_image_from_item(item, &mut images);
                if let Some(ref_item) = item.pointer("/ref_msg/message_item") {
                    Self::push_image_from_item(ref_item, &mut images);
                }
            }
        }
        if images.is_empty() {
            Self::push_image_from_item(value, &mut images);
        }
        images
    }

    fn push_image_from_item(item: &serde_json::Value, images: &mut Vec<ImImageAttachment>) {
        let item_type = item.get("type").and_then(|v| v.as_i64());
        if item_type != Some(2) && item.get("image_item").is_none() {
            return;
        }
        let image = item.get("image_item").unwrap_or(item);
        let media = image.get("media");
        let download_url = media
            .and_then(|v| v.get("full_url"))
            .or_else(|| image.get("url"))
            .or_else(|| item.get("url"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(str::to_string);
        let encrypted_query_param = media
            .and_then(|v| v.get("encrypt_query_param"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let aes_key = image
            .get("aeskey")
            .and_then(|v| v.as_str())
            .and_then(Self::hex_aes_key_to_base64)
            .or_else(|| {
                media
                    .and_then(|v| v.get("aes_key"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let data_base64 =
            Self::inline_image_base64(image).or_else(|| Self::inline_image_base64(item));
        if download_url.is_none() && encrypted_query_param.is_none() && data_base64.is_none() {
            return;
        }
        let file_key = Self::string_or_number_field(item, &["msg_id", "message_id", "id"])
            .or_else(|| encrypted_query_param.clone())
            .or_else(|| download_url.clone())
            .unwrap_or_else(|| format!("weixin-image-{:016x}", stable_hash(&item.to_string())));
        if images.iter().any(|image| image.file_key == file_key) {
            return;
        }
        images.push(ImImageAttachment {
            file_key,
            source: ImImageSource::MessageResource,
            mime_type: Self::string_field(image, &["mime_type", "content_type"])
                .or_else(|| Some("image/png".to_string())),
            data_base64,
            download_url,
            encrypted_query_param,
            aes_key,
        });
    }

    fn inline_image_base64(value: &serde_json::Value) -> Option<String> {
        Self::string_field(
            value,
            &[
                "data_base64",
                "image_base64",
                "base64",
                "content_base64",
                "data",
            ],
        )
        .and_then(|value| {
            value
                .split_once(";base64,")
                .map(|(_, data)| data.to_string())
                .or(Some(value))
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    }

    fn hex_aes_key_to_base64(hex: &str) -> Option<String> {
        let hex = hex.trim();
        if hex.len() != 32 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = Vec::with_capacity(16);
        for idx in (0..hex.len()).step_by(2) {
            let byte = u8::from_str_radix(&hex[idx..idx + 2], 16).ok()?;
            bytes.push(byte);
        }
        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    fn extract_text_from_string(raw: &str) -> String {
        let trimmed = raw.trim();
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(text) = json
                .get("text")
                .or_else(|| json.get("content"))
                .or_else(|| json.get("message"))
                .and_then(|v| v.as_str())
            {
                return text.to_string();
            }
        }
        raw.to_string()
    }

    fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
            .map(str::to_string)
    }

    fn string_or_number_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| {
            value.get(*key).and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
        })
    }

    fn send_error_message(response: &serde_json::Value) -> Option<String> {
        for key in ["errcode", "error_code", "code", "ret"] {
            if let Some(code) = response.get(key).and_then(|v| v.as_i64()) {
                if code != 0 {
                    let message = response
                        .get("errmsg")
                        .or_else(|| response.get("error_message"))
                        .or_else(|| response.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    return Some(format!("{key}={code}: {message}"));
                }
            }
        }
        if let Some(status) = response.get("status").and_then(|v| v.as_str()) {
            if matches!(status, "error" | "failed" | "fail") {
                return Some(status.to_string());
            }
        }
        None
    }

    fn card_to_text(card: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(elements) = card.pointer("/body/elements").and_then(|v| v.as_array()) {
            for element in elements {
                if let Some(content) = element.get("content").and_then(|v| v.as_str()) {
                    parts.push(content.to_string());
                }
            }
        }
        if parts.is_empty() {
            if let Some(title) = card
                .pointer("/header/title/content")
                .and_then(|v| v.as_str())
            {
                parts.push(title.to_string());
            }
        }
        parts.join("\n\n")
    }

    pub async fn download_message_image_resource(
        &self,
        _config: &ImProviderConfig,
        image: &ImImageAttachment,
    ) -> Result<(String, Vec<u8>)> {
        let url = image
            .download_url
            .clone()
            .or_else(|| {
                image.encrypted_query_param.as_ref().map(|param| {
                    format!(
                        "{}/download?encrypted_query_param={}",
                        DEFAULT_CDN_BASE_URL,
                        urlencoding::encode(param)
                    )
                })
            })
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin image {} has no download URL or CDN query param",
                    image.file_key
                ))
            })?;
        debug!(
            file_key = %image.file_key,
            has_aes_key = image.aes_key.is_some(),
            "downloading weixin message image resource"
        );
        let resp = self.http.get(&url).send().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "weixin message image download failed: {e}"
            ))
        })?;
        let status = resp.status();
        let header_mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .filter(|value| value.starts_with("image/"))
            .map(str::to_string);
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin message image download error: status={status}, body={body}"
            )));
        }
        let bytes = resp.bytes().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "weixin message image body read failed: {e}"
            ))
        })?;
        let bytes = if let Some(aes_key) = image.aes_key.as_deref() {
            Self::decrypt_aes_128_ecb(bytes.as_ref(), aes_key, &image.file_key)?
        } else {
            bytes.to_vec()
        };
        let mime_type = image
            .mime_type
            .clone()
            .or(header_mime)
            .unwrap_or_else(|| "image/png".to_string());
        Ok((mime_type, bytes))
    }

    fn decrypt_aes_128_ecb(ciphertext: &[u8], aes_key: &str, label: &str) -> Result<Vec<u8>> {
        type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;
        let key = Self::parse_aes_key(aes_key, label)?;
        Aes128EcbDec::new_from_slice(&key)
            .map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin image {label} AES key init failed: {e}"
                ))
            })?
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin image {label} AES decrypt failed: {e}"
                ))
            })
    }

    fn parse_aes_key(aes_key: &str, label: &str) -> Result<Vec<u8>> {
        let trimmed = aes_key.trim();
        if trimmed.len() == 32 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Self::hex_to_bytes(trimmed).ok_or_else(|| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin image {label} AES hex key parse failed"
                ))
            });
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin image {label} AES key base64 decode failed: {e}"
                ))
            })?;
        if decoded.len() == 16 {
            return Ok(decoded);
        }
        if decoded.len() == 32 {
            if let Ok(hex) = std::str::from_utf8(&decoded) {
                if hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    if let Some(bytes) = Self::hex_to_bytes(hex) {
                        return Ok(bytes);
                    }
                }
            }
        }
        Err(bifrost_core::BifrostError::Config(format!(
            "weixin image {label} AES key must decode to 16 raw bytes or 32-char hex string, got {} bytes",
            decoded.len()
        )))
    }

    fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
        if !hex.len().is_multiple_of(2) {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for idx in (0..hex.len()).step_by(2) {
            bytes.push(u8::from_str_radix(&hex[idx..idx + 2], 16).ok()?);
        }
        Some(bytes)
    }
}

#[async_trait]
impl ImProvider for WeixinProvider {
    fn provider_type(&self) -> ImProviderType {
        ImProviderType::Weixin
    }

    async fn validate_config(&self, config: &ImProviderConfig) -> Result<ProviderValidation> {
        let mut errors = Vec::new();
        if config.secret_ref.as_deref().unwrap_or_default().is_empty() {
            errors.push("missing bot token; complete QR login first".to_string());
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
        Self::bot_token(config)?;
        let config = config.clone();
        let provider = Self {
            http: self.http.clone(),
            runtime: Arc::clone(&self.runtime),
        };
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = interval.tick() => {
                        match provider.poll_once(&config).await {
                            Ok(events) => {
                                for event in events {
                                    let _ = sink.send(event);
                                }
                            }
                            Err(error) => {
                                warn!(
                                    provider_id = %config.id,
                                    error = %error,
                                    "weixin poll failed"
                                );
                            }
                        }
                    }
                }
            }
            info!(provider_id = %config.id, "weixin poll connection stopped");
        });
        Ok(ConnectionHandle { shutdown_tx })
    }

    async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        _opts: SendOptions,
    ) -> Result<SendResult> {
        self.send_text(config, target, &Self::card_to_text(&card))
            .await
    }

    async fn send_text(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let account_id = Self::account_id(config);
        let bot_token = Self::bot_token(config)?;
        let client_msg_id = format!("bifrost-weixin-{}-{}", now_ms(), uuid::Uuid::new_v4());
        let mut msg = serde_json::json!({
            "from_user_id": "",
            "to_user_id": target.receive_id,
            "client_id": client_msg_id,
            "message_type": 2,
            "message_state": 2,
            "item_list": [{
                "type": 1,
                "text_item": {
                    "text": text
                }
            }],
        });
        if let Some(context_token) = self
            .runtime
            .read()
            .get(account_id)
            .and_then(|runtime| runtime.context_tokens.get(&target.receive_id).cloned())
        {
            msg["context_token"] = serde_json::Value::String(context_token);
        }
        let payload = serde_json::json!({
            "msg": msg,
            "base_info": {
                "channel_version": "1.0.3"
            }
        });
        let response: serde_json::Value = self
            .http
            .post(format!("{base_url}/ilink/bot/sendmessage"))
            .header("content-type", "application/json")
            .header("iLink-App-ClientVersion", "1")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!("weixin sendmessage failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin sendmessage response parse failed: {e}"
                ))
            })?;
        if let Some(error) = Self::send_error_message(&response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin sendmessage failed: {error}"
            )));
        }
        debug!(provider_id = %config.id, target = %target.receive_id, "weixin message sent");
        Ok(SendResult {
            message_id: response
                .get("message_id")
                .or_else(|| response.get("msgid"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(Some(client_msg_id)),
            request_id: response
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }

    async fn upload_image(
        &self,
        _config: &ImProviderConfig,
        _image_type: &str,
        _file_name: &str,
        _bytes: Vec<u8>,
        _mime_type: Option<&str>,
    ) -> Result<UploadedImage> {
        Err(bifrost_core::BifrostError::Config(
            "weixin provider does not support outbound image upload yet".to_string(),
        ))
    }

    async fn send_image(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _image_key: &str,
        _uuid: Option<&str>,
    ) -> Result<SendResult> {
        Err(bifrost_core::BifrostError::Config(
            "weixin provider does not support outbound image messages yet".to_string(),
        ))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> ImProviderConfig {
        ImProviderConfig {
            id: "weixin-main".to_string(),
            provider_type: ImProviderType::Weixin,
            display_name: "Weixin Main".to_string(),
            enabled: true,
            base_url: Some("http://127.0.0.1:12345/".to_string()),
            app_id: Some("mock-bot@im.bot".to_string()),
            secret_ref: Some("mock-token".to_string()),
            owner_open_id: Some("owner@im.wechat".to_string()),
            event_connection_enabled: true,
            event_types: vec!["message.receive".to_string()],
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn normalize_login_account_uses_ilink_fields_and_redirected_base_url() {
        let account = WeixinProvider::normalize_login_account(
            serde_json::json!({
                "bot_token": "token-1",
                "ilink_bot_id": "bot-1@im.bot",
                "ilink_user_id": "user-1@im.wechat",
                "baseurl": "https://ilink.example.com/"
            }),
            "https://fallback.example.com",
        )
        .expect("normalize account");

        assert_eq!(account.account_id, "bot-1@im.bot");
        assert_eq!(account.user_id, "user-1@im.wechat");
        assert_eq!(account.base_url, "https://ilink.example.com");
        assert_eq!(account.bot_token, "token-1");
    }

    #[test]
    fn normalize_update_converts_weixin_message_to_im_event() {
        let provider = test_provider();
        let event = WeixinProvider::normalize_update(
            &provider,
            "mock-bot@im.bot",
            serde_json::json!({
                "message_id": "msg-1",
                "from_user_id": "user-1@im.wechat",
                "text": "hello bifrost"
            }),
        );

        assert_eq!(event.provider_id, "weixin-main");
        assert_eq!(event.provider_type, ImProviderType::Weixin);
        assert_eq!(event.event_type, "message.receive");
        assert_eq!(event.source.chat_id.as_deref(), Some("user-1@im.wechat"));
        assert_eq!(event.source.user_id.as_deref(), Some("user-1@im.wechat"));
        assert_eq!(event.source.message_id.as_deref(), Some("msg-1"));
        assert_eq!(event.message.as_ref().unwrap().text, "hello bifrost");
    }

    #[test]
    fn normalize_update_extracts_ilink_item_list_text_and_numeric_message_id() {
        let provider = test_provider();
        let event = WeixinProvider::normalize_update(
            &provider,
            "mock-bot@im.bot",
            serde_json::json!({
                "message_id": 7459890488013826184u64,
                "from_user_id": "user-2@im.wechat",
                "message_type": 1,
                "item_list": [
                    {
                        "type": 1,
                        "text_item": {
                            "text": "哈哈"
                        }
                    }
                ]
            }),
        );

        assert_eq!(
            event.source.message_id.as_deref(),
            Some("7459890488013826184")
        );
        assert_eq!(event.message.as_ref().unwrap().text, "哈哈");
        assert_eq!(
            event.message.as_ref().unwrap().raw_type.as_deref(),
            Some("1")
        );
    }

    #[test]
    fn normalize_update_extracts_ilink_image_item_for_multimodal_agent() {
        let provider = test_provider();
        let event = WeixinProvider::normalize_update(
            &provider,
            "mock-bot@im.bot",
            serde_json::json!({
                "message_id": 7459903000000000000u64,
                "from_user_id": "user-image@im.wechat",
                "message_type": 1,
                "item_list": [
                    {
                        "type": 2,
                        "msg_id": "img-msg-1",
                        "image_item": {
                            "aeskey": "00112233445566778899aabbccddeeff",
                            "media": {
                                "encrypt_query_param": "encrypted-param",
                                "full_url": "https://cdn.example.test/image.enc"
                            },
                            "thumb_width": 120
                        }
                    }
                ]
            }),
        );

        let msg = event.message.as_ref().unwrap();
        assert_eq!(msg.text, "");
        assert_eq!(msg.images.len(), 1);
        assert_eq!(msg.images[0].file_key, "img-msg-1");
        assert_eq!(
            msg.images[0].download_url.as_deref(),
            Some("https://cdn.example.test/image.enc")
        );
        assert_eq!(
            msg.images[0].encrypted_query_param.as_deref(),
            Some("encrypted-param")
        );
        assert_eq!(
            msg.images[0].aes_key.as_deref(),
            Some("ABEiM0RVZneImaq7zN3u/w==")
        );
        assert_eq!(msg.images[0].mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn decrypt_aes_128_ecb_accepts_base64_raw_key() {
        use aes::cipher::BlockEncryptMut;

        type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
        let key = [7u8; 16];
        let plaintext = b"fake-png-bytes";
        let ciphertext = Aes128EcbEnc::new_from_slice(&key)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
        let key_b64 = base64::engine::general_purpose::STANDARD.encode(key);

        let decrypted = WeixinProvider::decrypt_aes_128_ecb(&ciphertext, &key_b64, "unit").unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[tokio::test]
    async fn download_message_image_resource_fetches_plain_full_url() {
        use bytes::Bytes;
        use http_body_util::Full;
        use hyper::body::Incoming;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Request, Response};
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock image server");
        let port = listener.local_addr().expect("mock local addr").port();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let io = TokioIo::new(stream);
            let service = service_fn(|_req: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .header("content-type", "image/png")
                        .body(Full::new(Bytes::from_static(b"plain-image-bytes")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new().serve_connection(io, service).await;
        });

        let provider = WeixinProvider::new();
        let image = ImImageAttachment {
            file_key: "img-full-url".to_string(),
            source: ImImageSource::MessageResource,
            mime_type: None,
            data_base64: None,
            download_url: Some(format!("http://127.0.0.1:{port}/image.png")),
            encrypted_query_param: None,
            aes_key: None,
        };

        let (mime_type, bytes) = provider
            .download_message_image_resource(&test_provider(), &image)
            .await
            .expect("download image");

        assert_eq!(mime_type, "image/png");
        assert_eq!(bytes, b"plain-image-bytes");
    }

    #[test]
    fn send_error_message_detects_non_zero_ilink_code() {
        let error = WeixinProvider::send_error_message(&serde_json::json!({
            "errcode": 40001,
            "errmsg": "invalid token"
        }));

        assert_eq!(error.as_deref(), Some("errcode=40001: invalid token"));
        assert!(WeixinProvider::send_error_message(&serde_json::json!({"errcode": 0})).is_none());
    }

    #[test]
    fn send_error_message_detects_non_zero_ilink_ret() {
        let error = WeixinProvider::send_error_message(&serde_json::json!({
            "ret": -2,
            "errmsg": "invalid payload"
        }));

        assert_eq!(error.as_deref(), Some("ret=-2: invalid payload"));
        assert!(WeixinProvider::send_error_message(&serde_json::json!({"ret": 0})).is_none());
    }

    #[test]
    fn card_to_text_omits_header_when_body_has_content() {
        let text = WeixinProvider::card_to_text(&serde_json::json!({
            "header": {
                "title": {
                    "content": "Bifrost AI"
                }
            },
            "body": {
                "elements": [{
                    "tag": "markdown",
                    "content": "最终回复内容"
                }]
            }
        }));

        assert_eq!(text, "最终回复内容");
    }

    #[tokio::test]
    async fn validate_config_requires_completed_qr_login() {
        let provider = WeixinProvider::new();
        let mut config = test_provider();
        config.secret_ref = None;
        let validation = provider.validate_config(&config).await.expect("validate");
        assert!(!validation.valid);
        assert!(validation
            .errors
            .iter()
            .any(|error| error.contains("QR login")));
    }
}
