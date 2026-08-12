use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit};
use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use parking_lot::RwLock;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use bifrost_core::Result;

use crate::im_gateway::provider::{EventSink, ImProvider};
use crate::im_gateway::types::{
    ConnectionHandle, ConnectionState, ImChannelCapabilities, ImConversationCapabilities, ImEvent,
    ImEventMessage, ImEventSource, ImFileAttachment, ImFileMediaKind, ImImageAttachment,
    ImImageSource, ImInteractionCapabilities, ImMessageReference, ImProgressPresentation,
    ImProviderConfig, ImProviderType, ImSendCapabilities, ImSendPartCapability, ImSendSupportLevel,
    ImTarget, ProviderValidation, SendOptions, SendResult, UploadedImage,
};
use crate::im_gateway::weixin_context_store::WeixinContextStore;
use crate::im_gateway::weixin_sync_store::WeixinSyncCursorStore;

const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const DEFAULT_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_LONG_POLL_TIMEOUT_MS: u64 = 35_000;
const MIN_LONG_POLL_TIMEOUT_MS: u64 = 5_000;
const MAX_LONG_POLL_TIMEOUT_MS: u64 = 120_000;
const LONG_POLL_NETWORK_MARGIN_MS: u64 = 5_000;
const LOGIN_HTTP_TIMEOUT: Duration = Duration::from_secs(75);
const LOGIN_QR_EXPIRES_IN_SECONDS: u64 = 60;
const TEXT_RETRY_CHUNK_MAX_CHARS: usize = 1_000;
const TEXT_RETRY_CHUNK_MAX_BYTES: usize = 3_000;
const MAX_OUTBOUND_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_OUTBOUND_FILE_BYTES: usize = 30 * 1024 * 1024;
const MAX_PENDING_OUTBOUND_MEDIA_ITEMS: usize = 256;
const MAX_PENDING_OUTBOUND_MEDIA_BYTES: usize = 256 * 1024 * 1024;
const PENDING_OUTBOUND_MEDIA_TTL_MS: u64 = 10 * 60 * 1_000;
const MAX_INBOUND_MEDIA_BYTES: usize = 100 * 1024 * 1024;

pub(crate) struct WeixinToolProgress<'a> {
    pub channel_run_id: &'a str,
    pub client_msg_id: &'a str,
    pub tool_name: &'a str,
    pub tool_call_id: Option<&'a str>,
    pub finished_status: Option<&'a str>,
}

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
    long_poll_timeout_ms: u64,
}

#[derive(Debug)]
struct PollBatch {
    events: Vec<ImEvent>,
    next_cursor: Option<String>,
}

#[derive(Debug)]
pub(crate) struct WeixinConnectionStatusEvent {
    pub(crate) state: ConnectionState,
    pub(crate) error: Option<String>,
}

pub(crate) type WeixinConnectionStatusTx =
    tokio::sync::mpsc::UnboundedSender<WeixinConnectionStatusEvent>;

#[derive(Clone)]
struct CachedTypingTicket {
    ticket: String,
    expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutboundMediaKind {
    Image,
    File,
    Video,
}

impl OutboundMediaKind {
    fn upload_media_type(self) -> u8 {
        match self {
            Self::Image => 1,
            Self::Video => 2,
            Self::File => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Video => "video",
        }
    }
}

#[derive(Clone)]
struct PendingOutboundMedia {
    kind: OutboundMediaKind,
    file_name: String,
    bytes: Vec<u8>,
    mime_type: Option<String>,
    created_at_ms: u64,
}

struct UploadedOutboundMedia {
    pending: PendingOutboundMedia,
    download_param: String,
    aeskey_hex: String,
    ciphertext_size: usize,
}

pub struct WeixinProvider {
    http: reqwest::Client,
    poll_http: reqwest::Client,
    login_http: reqwest::Client,
    runtime: Arc<RwLock<HashMap<String, AccountRuntime>>>,
    context_store: Option<Arc<WeixinContextStore>>,
    sync_cursor_store: Option<Arc<WeixinSyncCursorStore>>,
    pending_outbound_media: Arc<RwLock<HashMap<String, PendingOutboundMedia>>>,
    typing_tickets: Arc<RwLock<HashMap<String, CachedTypingTicket>>>,
    active_channel_runs: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for WeixinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WeixinProvider {
    pub fn new() -> Self {
        Self::new_with_data_dir(&bifrost_storage::data_dir())
    }

    pub fn new_with_data_dir(data_dir: &std::path::Path) -> Self {
        Self::with_http_timeouts_and_data_dir(DEFAULT_HTTP_TIMEOUT, LOGIN_HTTP_TIMEOUT, data_dir)
    }

    #[cfg(test)]
    fn with_http_timeouts(default_timeout: Duration, login_timeout: Duration) -> Self {
        Self::with_http_timeouts_and_data_dir(
            default_timeout,
            login_timeout,
            &bifrost_storage::data_dir(),
        )
    }

    fn with_http_timeouts_and_data_dir(
        default_timeout: Duration,
        login_timeout: Duration,
        data_dir: &std::path::Path,
    ) -> Self {
        let http = bifrost_core::outbound_reqwest_client_builder()
            .timeout(default_timeout)
            .build()
            .unwrap_or_default();
        let login_http = bifrost_core::outbound_reqwest_client_builder()
            .timeout(login_timeout)
            .build()
            .unwrap_or_default();
        let poll_http = bifrost_core::outbound_reqwest_client_builder()
            .build()
            .unwrap_or_default();
        Self {
            http,
            poll_http,
            login_http,
            runtime: Arc::new(RwLock::new(HashMap::new())),
            context_store: match WeixinContextStore::new(data_dir) {
                Ok(store) => Some(Arc::new(store)),
                Err(error) => {
                    warn!(%error, "failed to initialize encrypted Weixin context store");
                    None
                }
            },
            sync_cursor_store: match WeixinSyncCursorStore::new(data_dir) {
                Ok(store) => Some(Arc::new(store)),
                Err(error) => {
                    warn!(%error, "failed to initialize encrypted Weixin sync cursor store");
                    None
                }
            },
            pending_outbound_media: Arc::new(RwLock::new(HashMap::new())),
            typing_tickets: Arc::new(RwLock::new(HashMap::new())),
            active_channel_runs: Arc::new(RwLock::new(HashMap::new())),
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

    fn account_runtime_key(config: &ImProviderConfig) -> String {
        format!("{}\0{}", config.id.trim(), Self::account_id(config))
    }

    fn context_token(&self, config: &ImProviderConfig, user_id: &str) -> Option<String> {
        let account_id = Self::account_id(config);
        self.runtime
            .read()
            .get(&Self::account_runtime_key(config))
            .and_then(|runtime| runtime.context_tokens.get(user_id).cloned())
            .or_else(|| {
                self.context_store
                    .as_ref()
                    .and_then(|store| store.get(account_id, user_id))
            })
    }

    pub fn send_ready(&self, config: &ImProviderConfig, target: &ImTarget) -> bool {
        self.send_ready_for_user(config, &target.receive_id)
    }

    pub fn send_ready_for_user(&self, config: &ImProviderConfig, user_id: &str) -> bool {
        self.context_token(config, user_id).is_some()
    }

    fn user_runtime_key(config: &ImProviderConfig, user_id: &str) -> String {
        format!(
            "{}\0{}\0{}",
            config.id.trim(),
            Self::account_id(config),
            user_id.trim()
        )
    }

    pub(crate) fn begin_channel_run(&self, config: &ImProviderConfig, target: &ImTarget) -> String {
        let run_id = uuid::Uuid::new_v4().to_string();
        self.active_channel_runs.write().insert(
            Self::user_runtime_key(config, &target.receive_id),
            run_id.clone(),
        );
        run_id
    }

    fn active_channel_run_id(&self, config: &ImProviderConfig, user_id: &str) -> Option<String> {
        self.active_channel_runs
            .read()
            .get(&Self::user_runtime_key(config, user_id))
            .cloned()
    }

    pub(crate) fn end_channel_run(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        channel_run_id: &str,
    ) {
        let key = Self::user_runtime_key(config, &target.receive_id);
        let mut runs = self.active_channel_runs.write();
        if runs
            .get(&key)
            .is_some_and(|active| active == channel_run_id)
        {
            runs.remove(&key);
        }
    }

    pub(crate) fn invalidate_typing_ticket(&self, config: &ImProviderConfig, target: &ImTarget) {
        self.typing_tickets
            .write()
            .remove(&Self::user_runtime_key(config, &target.receive_id));
    }

    pub(crate) async fn typing_ticket(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
    ) -> Result<String> {
        let key = Self::user_runtime_key(config, &target.receive_id);
        if let Some(cached) = self.typing_tickets.read().get(&key).cloned() {
            if cached.expires_at_ms > now_ms() {
                return Ok(cached.ticket);
            }
        }
        let context_token = self
            .context_token(config, &target.receive_id)
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "weixin typing requires an inbound context token".to_string(),
                )
            })?;
        let response = Self::with_common_headers(
            self.http
                .post(format!("{}/ilink/bot/getconfig", Self::base_url(config))),
            Self::bot_token(config)?,
        )
        .json(&serde_json::json!({
            "ilink_user_id": target.receive_id,
            "context_token": context_token,
            "base_info": { "channel_version": "1.0.3" }
        }))
        .send()
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!("weixin getconfig failed: {error}"))
        })?;
        let response = Self::read_json_response_or_empty(response, "getconfig").await?;
        if let Some(error) = Self::send_error_message(&response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin getconfig failed: {error}"
            )));
        }
        let ticket = response
            .get("typing_ticket")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(
                    "weixin getconfig response missing typing_ticket".to_string(),
                )
            })?
            .to_string();
        self.typing_tickets.write().insert(
            key,
            CachedTypingTicket {
                ticket: ticket.clone(),
                expires_at_ms: now_ms().saturating_add(20 * 60 * 60 * 1_000),
            },
        );
        Ok(ticket)
    }

    pub(crate) async fn send_typing_status(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        ticket: &str,
        status: u8,
    ) -> Result<()> {
        let response = Self::with_common_headers(
            self.http
                .post(format!("{}/ilink/bot/sendtyping", Self::base_url(config))),
            Self::bot_token(config)?,
        )
        .json(&serde_json::json!({
            "ilink_user_id": target.receive_id,
            "typing_ticket": ticket,
            "status": status,
            "base_info": { "channel_version": "1.0.3" }
        }))
        .send()
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!("weixin sendtyping failed: {error}"))
        })?;
        let response = Self::read_json_response_or_empty(response, "sendtyping").await?;
        if let Some(error) = Self::send_error_message(&response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin sendtyping failed: {error}"
            )));
        }
        Ok(())
    }

    pub(crate) async fn send_tool_progress(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        progress: WeixinToolProgress<'_>,
    ) -> Result<SendResult> {
        let (item_type, item_name, completed) = if progress.finished_status.is_some() {
            (12, "tool_call_result_item", true)
        } else {
            (11, "tool_call_start_item", false)
        };
        let mut tool_item = serde_json::json!({ "tool_name": progress.tool_name });
        if let Some(tool_call_id) = progress.tool_call_id {
            tool_item["tool_call_id"] = serde_json::Value::String(tool_call_id.to_string());
        }
        if let Some(status) = progress.finished_status {
            tool_item["status"] = serde_json::Value::String(status.to_string());
        }
        let mut item = serde_json::json!({
            "type": item_type,
            "create_time_ms": now_ms(),
            "is_completed": completed
        });
        item[item_name] = tool_item;
        let context_token = self
            .context_token(config, &target.receive_id)
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "weixin progress requires an inbound context token".to_string(),
                )
            })?;
        let payload = serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": target.receive_id,
                "client_id": progress.client_msg_id,
                "message_type": 2,
                "message_state": 2,
                "item_list": [item],
                "context_token": context_token,
                "run_id": progress.channel_run_id
            },
            "base_info": { "channel_version": "1.0.3" }
        });
        let response = Self::with_common_headers(
            self.http
                .post(format!("{}/ilink/bot/sendmessage", Self::base_url(config))),
            Self::bot_token(config)?,
        )
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "weixin send tool progress failed: {error}"
            ))
        })?;
        let response = Self::read_json_response_or_empty(response, "send tool progress").await?;
        if let Some(error) = Self::send_error_message(&response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin send tool progress failed: {error}"
            )));
        }
        Ok(SendResult {
            message_id: response
                .get("message_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .or_else(|| Some(progress.client_msg_id.to_string())),
            request_id: response
                .get("request_id")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        })
    }

    #[cfg(test)]
    pub(crate) fn store_context_for_test(
        &self,
        config: &ImProviderConfig,
        user_id: &str,
        token: &str,
    ) -> Result<()> {
        self.context_store
            .as_ref()
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "weixin context store is unavailable".to_string(),
                )
            })?
            .put(Self::account_id(config), user_id, token)
    }

    fn with_common_headers(
        request: reqwest::RequestBuilder,
        bot_token: &str,
    ) -> reqwest::RequestBuilder {
        request
            .header("content-type", "application/json")
            .header("iLink-App-ClientVersion", "1")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {bot_token}"))
            .header("X-WECHAT-UIN", Self::random_wechat_uin())
    }

    fn random_wechat_uin() -> String {
        let value: u32 = rand::random();
        base64::engine::general_purpose::STANDARD.encode(value.to_string())
    }

    fn normalize_long_poll_timeout_ms(timeout_ms: u64) -> u64 {
        timeout_ms.clamp(MIN_LONG_POLL_TIMEOUT_MS, MAX_LONG_POLL_TIMEOUT_MS)
    }

    fn long_poll_request_timeout_ms(server_timeout_ms: u64) -> u64 {
        server_timeout_ms.saturating_add(LONG_POLL_NETWORK_MARGIN_MS)
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
            .login_http
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
            expires_in_seconds: LOGIN_QR_EXPIRES_IN_SECONDS,
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
                .login_http
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

    async fn poll_once(&self, config: &ImProviderConfig) -> Result<PollBatch> {
        let base_url = Self::base_url(config);
        let account_id = Self::account_id(config).to_string();
        let runtime_key = Self::account_runtime_key(config);
        let bot_token = Self::bot_token(config)?;
        let (get_updates_buf, server_timeout_ms) = {
            let runtime = self.runtime.read();
            let current = runtime.get(&runtime_key);
            let cursor = current
                .map(|runtime| runtime.get_updates_buf.clone())
                .filter(|cursor| !cursor.is_empty())
                .or_else(|| {
                    self.sync_cursor_store
                        .as_ref()
                        .and_then(|store| store.get(&config.id, &account_id))
                })
                .unwrap_or_default();
            let timeout = current
                .map(|runtime| runtime.long_poll_timeout_ms)
                .filter(|timeout| *timeout > 0)
                .unwrap_or(DEFAULT_LONG_POLL_TIMEOUT_MS);
            (cursor, timeout)
        };
        let url = format!("{base_url}/ilink/bot/getupdates");
        let response = Self::with_common_headers(
            self.poll_http.post(url).timeout(Duration::from_millis(
                Self::long_poll_request_timeout_ms(server_timeout_ms),
            )),
            bot_token,
        )
        .json(&serde_json::json!({
            "get_updates_buf": get_updates_buf,
            "base_info": { "channel_version": "1.0.3" }
        }))
        .send()
        .await
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!("weixin getupdates failed: {e}"))
        })?;
        let response_status = response.status();
        if !response_status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin getupdates HTTP error: status={response_status}, body={}",
                truncate_chars(&body, 2_000)
            )));
        }
        let response: serde_json::Value = response.json().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "weixin getupdates response parse failed: {e}"
            ))
        })?;
        if let Some(error) = Self::send_error_message(&response) {
            let authentication_required = response
                .get("ret")
                .or_else(|| response.get("errcode"))
                .and_then(|value| value.as_i64())
                == Some(-14);
            let prefix = if authentication_required {
                "weixin authentication required"
            } else {
                "weixin getupdates failed"
            };
            return Err(bifrost_core::BifrostError::Network(format!(
                "{prefix}: {error}"
            )));
        }
        if let Some(timeout) = response
            .get("longpolling_timeout_ms")
            .or_else(|| response.get("long_polling_timeout_ms"))
            .and_then(|value| value.as_u64())
        {
            self.runtime
                .write()
                .entry(runtime_key.clone())
                .or_default()
                .long_poll_timeout_ms = Self::normalize_long_poll_timeout_ms(timeout);
        }
        let next_cursor = response
            .get("get_updates_buf")
            .or_else(|| response.get("next_get_updates_buf"))
            .or_else(|| response.get("sync_buf"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
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
                    self.context_store
                        .as_ref()
                        .ok_or_else(|| {
                            bifrost_core::BifrostError::Config(
                                "encrypted Weixin context store is unavailable".to_string(),
                            )
                        })?
                        .put(&account_id, &from, context_token)?;
                    self.runtime
                        .write()
                        .entry(runtime_key.clone())
                        .or_default()
                        .context_tokens
                        .insert(from, context_token.to_string());
                }
            }
            events.push(Self::normalize_update(config, &account_id, update));
        }
        Ok(PollBatch {
            events,
            next_cursor,
        })
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
        let files = Self::message_files(&update);
        let reply_to = Self::message_reference(&update);
        let mut text = Self::message_text(&update);
        let voice_transcripts = Self::message_voice_transcripts(&update);
        if !voice_transcripts.is_empty() {
            if !text.trim().is_empty() {
                text.push_str("\n\n");
            }
            text.push_str("【语音转写】\n");
            text.push_str(&voice_transcripts.join("\n"));
        } else if files
            .iter()
            .any(|file| file.media_kind == ImFileMediaKind::Voice)
        {
            if !text.trim().is_empty() {
                text.push_str("\n\n");
            }
            text.push_str("【语音消息未转写，已作为音频附件提供】");
        }
        if text.trim().is_empty() && images.is_empty() && files.is_empty() {
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
                chat_type: Some("p2p".to_string()),
                user_name: None,
                sender_type: Some("user".to_string()),
            },
            message: Some(ImEventMessage {
                text,
                mentions: Vec::new(),
                images,
                files,
                reply_to,
                raw_type: Some(raw_type),
                raw_content: Some(update),
                create_time: None,
                update_time: None,
                root_id: None,
                parent_id: None,
                thread_id: None,
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
            "/text_item/text",
            "/text_item/content",
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

    fn message_reference(value: &serde_json::Value) -> Option<ImMessageReference> {
        let items = value.get("item_list")?.as_array()?;
        items.iter().find_map(|item| {
            let reference = item.pointer("/ref_msg/message_item")?;
            let message_id = Self::string_or_number_field(
                reference,
                &["msg_id", "message_id", "id", "client_msg_id", "new_msg_id"],
            );
            let created_at_ms = Self::u64_field(
                reference,
                &["create_time_ms", "created_at_ms", "timestamp_ms"],
            );
            let text = Self::message_text(reference).trim().to_string();
            let text = (!text.is_empty()).then_some(text);
            (message_id.is_some() || created_at_ms.is_some() || text.is_some()).then_some(
                ImMessageReference {
                    message_id,
                    created_at_ms,
                    text,
                },
            )
        })
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

    fn message_voice_transcripts(value: &serde_json::Value) -> Vec<String> {
        let mut transcripts = Vec::new();
        let mut collect = |item: &serde_json::Value| {
            if item.get("type").and_then(|value| value.as_i64()) != Some(3) {
                return;
            }
            if let Some(text) = item
                .pointer("/voice_item/text")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                transcripts.push(text.to_string());
            }
        };
        if let Some(items) = value.get("item_list").and_then(|items| items.as_array()) {
            for item in items {
                collect(item);
                if let Some(reference) = item.pointer("/ref_msg/message_item") {
                    collect(reference);
                }
            }
        } else {
            collect(value);
        }
        transcripts
    }

    fn message_files(value: &serde_json::Value) -> Vec<ImFileAttachment> {
        let mut files = Vec::new();
        if let Some(items) = value.get("item_list").and_then(|items| items.as_array()) {
            for item in items {
                Self::push_file_from_item(item, &mut files);
                if let Some(reference) = item.pointer("/ref_msg/message_item") {
                    Self::push_file_from_item(reference, &mut files);
                }
            }
        } else {
            Self::push_file_from_item(value, &mut files);
        }
        files
    }

    fn push_file_from_item(item: &serde_json::Value, files: &mut Vec<ImFileAttachment>) {
        let Some(item_type) = item.get("type").and_then(|value| value.as_i64()) else {
            return;
        };
        let (media_kind, media_path, default_name, default_mime) = match item_type {
            3 => {
                let transcript = item
                    .pointer("/voice_item/text")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if transcript.is_some() {
                    return;
                }
                (
                    ImFileMediaKind::Voice,
                    "/voice_item",
                    "voice.silk",
                    "audio/silk",
                )
            }
            4 => (
                ImFileMediaKind::File,
                "/file_item",
                "attachment.bin",
                "application/octet-stream",
            ),
            5 => (
                ImFileMediaKind::Video,
                "/video_item",
                "video.mp4",
                "video/mp4",
            ),
            _ => return,
        };
        let Some(media_item) = item.pointer(media_path) else {
            return;
        };
        let media = media_item.get("media");
        let download_url = media
            .and_then(|value| value.get("full_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(str::to_string);
        let encrypted_query_param = media
            .and_then(|value| value.get("encrypt_query_param"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if download_url.is_none() && encrypted_query_param.is_none() {
            return;
        }
        let aes_key = media
            .and_then(|value| value.get("aes_key"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = media_item
            .get("file_name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_name)
            .to_string();
        let mime_type = match media_kind {
            ImFileMediaKind::File => mime_guess::from_path(&name)
                .first_raw()
                .unwrap_or(default_mime)
                .to_string(),
            _ => default_mime.to_string(),
        };
        let file_key = Self::string_or_number_field(item, &["msg_id", "message_id", "id"])
            .or_else(|| encrypted_query_param.clone())
            .or_else(|| download_url.clone())
            .unwrap_or_else(|| format!("weixin-media-{:016x}", stable_hash(&item.to_string())));
        if files.iter().any(|file| file.file_key == file_key) {
            return;
        }
        let size_bytes = match media_kind {
            ImFileMediaKind::File => Self::u64_field(media_item, &["len"]),
            ImFileMediaKind::Video => Self::u64_field(media_item, &["video_size"]),
            ImFileMediaKind::Voice => None,
        };
        let duration_ms = match media_kind {
            ImFileMediaKind::Video => Self::u64_field(media_item, &["play_length"]),
            ImFileMediaKind::Voice => Self::u64_field(media_item, &["playtime"]),
            ImFileMediaKind::File => None,
        };
        let codec = (media_kind == ImFileMediaKind::Voice).then(|| {
            Self::u64_field(media_item, &["encode_type"])
                .map(Self::voice_codec_name)
                .unwrap_or("unknown")
                .to_string()
        });
        files.push(ImFileAttachment {
            file_key,
            name: Some(name),
            mime_type: Some(mime_type),
            size_bytes,
            data_base64: None,
            download_url,
            media_kind,
            encrypted_query_param,
            aes_key,
            transcript: None,
            duration_ms,
            codec,
        });
    }

    fn voice_codec_name(encode_type: u64) -> &'static str {
        match encode_type {
            1 => "pcm",
            2 => "adpcm",
            3 => "feature",
            4 => "speex",
            5 => "amr",
            6 => "silk",
            7 => "mp3",
            8 => "ogg-speex",
            _ => "unknown",
        }
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

    fn u64_field(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
        keys.iter().find_map(|key| {
            value.get(*key).and_then(|field| {
                field
                    .as_u64()
                    .or_else(|| field.as_i64().and_then(|number| u64::try_from(number).ok()))
                    .or_else(|| field.as_str().and_then(|number| number.parse().ok()))
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

    async fn read_json_response_or_empty(
        response: reqwest::Response,
        label: &str,
    ) -> Result<serde_json::Value> {
        let status = response.status();
        let text = response.text().await.map_err(|e| {
            bifrost_core::BifrostError::Network(format!("weixin {label} response read failed: {e}"))
        })?;
        if !status.is_success() {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin {label} error: status={status}, body={text}"
            )));
        }
        if text.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        serde_json::from_str(&text).map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "weixin {label} response parse failed: {e}; body={text}"
            ))
        })
    }

    fn split_text_for_retry(text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }

        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_chars = 0usize;
        let mut current_bytes = 0usize;

        for ch in text.chars() {
            let ch_bytes = ch.len_utf8();
            if !current.is_empty()
                && (current_chars + 1 > TEXT_RETRY_CHUNK_MAX_CHARS
                    || current_bytes + ch_bytes > TEXT_RETRY_CHUNK_MAX_BYTES)
            {
                chunks.push(std::mem::take(&mut current));
                current_chars = 0;
                current_bytes = 0;
            }
            current.push(ch);
            current_chars += 1;
            current_bytes += ch_bytes;
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    fn split_text_messages_for_retry(text: &str) -> Vec<String> {
        let chunks = Self::split_text_for_retry(text);
        if chunks.len() <= 1 {
            return chunks;
        }
        let total = chunks.len();
        chunks
            .into_iter()
            .enumerate()
            .map(|(idx, chunk)| format!("[{}/{}]\n\n{}", idx + 1, total, chunk))
            .collect()
    }

    /// Weixin's native text bubble collapses a single LF as inline whitespace,
    /// while an empty line is rendered as a visible paragraph break. Promote
    /// single line breaks without expanding existing paragraph spacing.
    fn render_text_for_weixin(text: &str) -> String {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut rendered = String::with_capacity(normalized.len());
        let mut chars = normalized.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\n' {
                rendered.push(ch);
                continue;
            }
            let mut newline_count = 1;
            while chars.peek() == Some(&'\n') {
                chars.next();
                newline_count += 1;
            }
            for _ in 0..newline_count.max(2) {
                rendered.push('\n');
            }
        }
        rendered
    }

    async fn send_text_once(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
        client_msg_id: String,
    ) -> Result<SendResult> {
        let base_url = Self::base_url(config);
        let bot_token = Self::bot_token(config)?;
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
        let context_token = self
            .context_token(config, &target.receive_id)
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "weixin provider is connected but not send-ready; send the bot an inbound message first"
                        .to_string(),
                )
            })?;
        msg["context_token"] = serde_json::Value::String(context_token);
        if let Some(run_id) = self.active_channel_run_id(config, &target.receive_id) {
            msg["run_id"] = serde_json::Value::String(run_id);
        }
        let payload = serde_json::json!({
            "msg": msg,
            "base_info": {
                "channel_version": "1.0.3"
            }
        });
        let response = Self::with_common_headers(
            self.http.post(format!("{base_url}/ilink/bot/sendmessage")),
            bot_token,
        )
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!("weixin sendmessage failed: {e}"))
        })?;
        let response = Self::read_json_response_or_empty(response, "sendmessage").await?;
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

    pub async fn send_text_with_client_id(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
        client_msg_id: &str,
    ) -> Result<SendResult> {
        let rendered_text = Self::render_text_for_weixin(text);
        let original_error = match self
            .send_text_once(config, target, &rendered_text, client_msg_id.to_string())
            .await
        {
            Ok(result) => return Ok(result),
            Err(error @ bifrost_core::BifrostError::Network(_)) => error,
            Err(error) => return Err(error),
        };
        let chunks = Self::split_text_messages_for_retry(&rendered_text);
        if chunks.len() <= 1 {
            return Err(original_error);
        }

        let mut first_message_id = None;
        let mut first_request_id = None;
        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_msg_id = format!("{client_msg_id}-part-{}", idx + 1);
            let result = self
                .send_text_once(config, target, chunk, chunk_msg_id)
                .await
                .map_err(|error| {
                    bifrost_core::BifrostError::Network(format!(
                        "weixin full message failed ({original_error}); fallback chunk {}/{} failed: {error}",
                        idx + 1,
                        chunks.len()
                    ))
                })?;
            if first_message_id.is_none() {
                first_message_id = result.message_id;
            }
            if first_request_id.is_none() {
                first_request_id = result.request_id;
            }
        }
        Ok(SendResult {
            message_id: first_message_id,
            request_id: first_request_id,
        })
    }

    fn encrypt_aes_128_ecb(plaintext: &[u8], aes_key: &[u8; 16]) -> Result<Vec<u8>> {
        type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
        Ok(Aes128EcbEnc::new_from_slice(aes_key)
            .map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin outbound image AES key init failed: {e}"
                ))
            })?
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext))
    }

    fn bytes_to_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    fn cdn_upload_url(upload_full_url: Option<&str>, upload_param: &str, filekey: &str) -> String {
        upload_full_url
            .map(str::trim)
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{}/upload?encrypted_query_param={}&filekey={}",
                    DEFAULT_CDN_BASE_URL,
                    urlencoding::encode(upload_param),
                    urlencoding::encode(filekey)
                )
            })
    }

    fn classify_outbound_file(bytes: &[u8], mime_type: Option<&str>) -> OutboundMediaKind {
        let mime_type = mime_type
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        let is_mp4 = bytes.len() >= 12 && &bytes[4..8] == b"ftyp";
        let is_webm = bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]);
        match mime_type.as_deref() {
            Some("video/mp4") if is_mp4 => OutboundMediaKind::Video,
            Some("video/webm") if is_webm => OutboundMediaKind::Video,
            _ => OutboundMediaKind::File,
        }
    }

    fn insert_pending_outbound_media(&self, pending: PendingOutboundMedia) -> Result<String> {
        let now = now_ms();
        let mut store = self.pending_outbound_media.write();
        store.retain(|_, media| {
            now.saturating_sub(media.created_at_ms) <= PENDING_OUTBOUND_MEDIA_TTL_MS
        });
        let pending_bytes = store.values().map(|media| media.bytes.len()).sum::<usize>();
        if store.len() >= MAX_PENDING_OUTBOUND_MEDIA_ITEMS
            || pending_bytes.saturating_add(pending.bytes.len()) > MAX_PENDING_OUTBOUND_MEDIA_BYTES
        {
            return Err(bifrost_core::BifrostError::Config(
                "weixin pending outbound media cache is full; retry after pending sends finish"
                    .to_string(),
            ));
        }
        let key = format!(
            "weixin-outbound-{}-{}-{}",
            pending.kind.label(),
            now,
            uuid::Uuid::new_v4()
        );
        store.insert(key.clone(), pending);
        Ok(key)
    }

    async fn upload_outbound_media_for_target(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        media_key: &str,
        expected_kind: Option<OutboundMediaKind>,
    ) -> Result<UploadedOutboundMedia> {
        let pending = self
            .pending_outbound_media
            .read()
            .get(media_key)
            .cloned()
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin outbound media key not found: {media_key}"
                ))
            })?;
        if expected_kind.is_some_and(|kind| kind != pending.kind) {
            return Err(bifrost_core::BifrostError::Config(format!(
                "weixin outbound media kind mismatch for {}",
                pending.file_name
            )));
        }
        let base_url = Self::base_url(config);
        let bot_token = Self::bot_token(config)?;
        let mut aes_key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut aes_key);
        let aeskey_hex = Self::bytes_to_hex(&aes_key);
        let ciphertext = Self::encrypt_aes_128_ecb(&pending.bytes, &aes_key)?;
        let rawfilemd5 = format!("{:x}", md5::compute(&pending.bytes));
        let filekey = format!("bifrost-{}-{}", now_ms(), uuid::Uuid::new_v4());

        let get_upload_payload = serde_json::json!({
            "filekey": filekey,
            "media_type": pending.kind.upload_media_type(),
            "to_user_id": target.receive_id,
            "rawsize": pending.bytes.len(),
            "rawfilemd5": rawfilemd5,
            "filesize": ciphertext.len(),
            "no_need_thumb": true,
            "aeskey": aeskey_hex,
            "base_info": {
                "channel_version": "1.0.3"
            }
        });
        let upload_response = Self::with_common_headers(
            self.http.post(format!("{base_url}/ilink/bot/getuploadurl")),
            bot_token,
        )
        .json(&get_upload_payload)
        .send()
        .await
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!("weixin getuploadurl failed: {e}"))
        })?;
        let upload_response =
            Self::read_json_response_or_empty(upload_response, "getuploadurl").await?;
        if let Some(error) = Self::send_error_message(&upload_response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin getuploadurl failed: {error}"
            )));
        }
        let upload_param = upload_response
            .get("upload_param")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin getuploadurl response missing upload_param: {upload_response}"
                ))
            })?;
        let upload_full_url = upload_response
            .get("upload_full_url")
            .and_then(|v| v.as_str());
        let cdn_url = Self::cdn_upload_url(upload_full_url, upload_param, &filekey);
        let cdn_response = self
            .http
            .post(&cdn_url)
            .header("content-type", "application/octet-stream")
            .body(ciphertext.clone())
            .send()
            .await
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin CDN {} upload failed: {e}",
                    pending.kind.label()
                ))
            })?;
        let cdn_status = cdn_response.status();
        let download_param = cdn_response
            .headers()
            .get("x-encrypted-param")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if !cdn_status.is_success() {
            let body = cdn_response.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin CDN {} upload error: status={cdn_status}, body={body}",
                pending.kind.label()
            )));
        }
        let download_param = download_param.ok_or_else(|| {
            bifrost_core::BifrostError::Network(format!(
                "weixin CDN {} upload missing x-encrypted-param header",
                pending.kind.label()
            ))
        })?;

        debug!(
            provider_id = %config.id,
            target = %target.receive_id,
            media_kind = pending.kind.label(),
            file_name = %pending.file_name,
            mime_type = pending.mime_type.as_deref().unwrap_or("unknown"),
            rawsize = pending.bytes.len(),
            ciphertext_size = ciphertext.len(),
            "weixin outbound media uploaded to CDN"
        );

        Ok(UploadedOutboundMedia {
            pending,
            download_param,
            aeskey_hex,
            ciphertext_size: ciphertext.len(),
        })
    }

    fn outbound_media_item(uploaded: &UploadedOutboundMedia) -> serde_json::Value {
        let media = serde_json::json!({
            "encrypt_query_param": uploaded.download_param,
            "aes_key": base64::engine::general_purpose::STANDARD
                .encode(uploaded.aeskey_hex.as_bytes()),
            "encrypt_type": 1
        });
        match uploaded.pending.kind {
            OutboundMediaKind::Image => serde_json::json!({
                "type": 2,
                "image_item": {
                    "media": media,
                    "aeskey": uploaded.aeskey_hex,
                    "mid_size": uploaded.ciphertext_size
                }
            }),
            OutboundMediaKind::File => serde_json::json!({
                "type": 4,
                "file_item": {
                    "media": media,
                    "file_name": uploaded.pending.file_name,
                    "len": uploaded.pending.bytes.len().to_string()
                }
            }),
            OutboundMediaKind::Video => serde_json::json!({
                "type": 5,
                "video_item": {
                    "media": media,
                    "video_size": uploaded.ciphertext_size
                }
            }),
        }
    }

    async fn send_outbound_media(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        media_key: &str,
        expected_kind: Option<OutboundMediaKind>,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        let uploaded = self
            .upload_outbound_media_for_target(config, target, media_key, expected_kind)
            .await?;
        let item = Self::outbound_media_item(&uploaded);
        let kind = uploaded.pending.kind;
        let client_msg_id = uuid.map(str::to_string).unwrap_or_else(|| {
            format!(
                "bifrost-weixin-{}-{}-{}",
                kind.label(),
                now_ms(),
                uuid::Uuid::new_v4()
            )
        });
        let context_token = self
            .context_token(config, &target.receive_id)
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "weixin provider is connected but not send-ready; send the bot an inbound message first"
                        .to_string(),
                )
            })?;
        let mut msg = serde_json::json!({
            "from_user_id": "",
            "to_user_id": target.receive_id,
            "client_id": client_msg_id,
            "message_type": 2,
            "message_state": 2,
            "item_list": [item],
            "context_token": context_token
        });
        if let Some(run_id) = self.active_channel_run_id(config, &target.receive_id) {
            msg["run_id"] = serde_json::Value::String(run_id);
        }
        let payload = serde_json::json!({
            "msg": msg,
            "base_info": {
                "channel_version": "1.0.3"
            }
        });
        let response = Self::with_common_headers(
            self.http
                .post(format!("{}/ilink/bot/sendmessage", Self::base_url(config))),
            Self::bot_token(config)?,
        )
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!("weixin send {} failed: {e}", kind.label()))
        })?;
        let response =
            Self::read_json_response_or_empty(response, &format!("send {}", kind.label())).await?;
        if let Some(error) = Self::send_error_message(&response) {
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin send {} failed: {error}",
                kind.label()
            )));
        }
        self.pending_outbound_media.write().remove(media_key);
        debug!(
            provider_id = %config.id,
            target = %target.receive_id,
            media_kind = kind.label(),
            "weixin media message sent"
        );
        Ok(SendResult {
            message_id: response
                .get("message_id")
                .or_else(|| response.get("msgid"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .or(Some(client_msg_id)),
            request_id: response
                .get("request_id")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        })
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
        config: &ImProviderConfig,
        image: &ImImageAttachment,
    ) -> Result<(String, Vec<u8>)> {
        debug!(
            file_key = %image.file_key,
            has_aes_key = image.aes_key.is_some(),
            "downloading weixin message image resource"
        );
        let (header_mime, bytes) = self
            .download_and_decrypt_media(
                config,
                image.download_url.as_deref(),
                image.encrypted_query_param.as_deref(),
                image.aes_key.as_deref(),
                &image.file_key,
            )
            .await?;
        let mime_type = image
            .mime_type
            .clone()
            .or_else(|| header_mime.filter(|value| value.starts_with("image/")))
            .unwrap_or_else(|| "image/png".to_string());
        Ok((mime_type, bytes))
    }

    pub async fn download_message_file_resource(
        &self,
        config: &ImProviderConfig,
        file: &ImFileAttachment,
    ) -> Result<(String, Vec<u8>)> {
        debug!(
            file_key = %file.file_key,
            media_kind = ?file.media_kind,
            has_aes_key = file.aes_key.is_some(),
            "downloading weixin message file resource"
        );
        let (header_mime, bytes) = self
            .download_and_decrypt_media(
                config,
                file.download_url.as_deref(),
                file.encrypted_query_param.as_deref(),
                file.aes_key.as_deref(),
                &file.file_key,
            )
            .await?;
        let mime_type = file
            .mime_type
            .clone()
            .or(header_mime)
            .unwrap_or_else(|| "application/octet-stream".to_string());
        Ok((mime_type, bytes))
    }

    async fn download_and_decrypt_media(
        &self,
        config: &ImProviderConfig,
        download_url: Option<&str>,
        encrypted_query_param: Option<&str>,
        aes_key: Option<&str>,
        label: &str,
    ) -> Result<(Option<String>, Vec<u8>)> {
        let url = download_url
            .map(str::to_string)
            .or_else(|| {
                encrypted_query_param.map(|param| {
                    format!(
                        "{}/download?encrypted_query_param={}",
                        DEFAULT_CDN_BASE_URL,
                        urlencoding::encode(param)
                    )
                })
            })
            .ok_or_else(|| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin media {label} has no download URL or CDN query param"
                ))
            })?;
        Self::validate_media_download_url(config, &url)?;
        let response = self.http.get(&url).send().await.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "weixin media {label} download failed: {error}"
            ))
        })?;
        Self::validate_media_download_url(config, response.url().as_str())?;
        let status = response.status();
        let header_mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(bifrost_core::BifrostError::Network(format!(
                "weixin media {label} download error: status={status}, body={body}"
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_INBOUND_MEDIA_BYTES as u64)
        {
            return Err(bifrost_core::BifrostError::Config(format!(
                "weixin media {label} exceeds {} MiB download limit",
                MAX_INBOUND_MEDIA_BYTES / 1024 / 1024
            )));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin media {label} body read failed: {error}"
                ))
            })?;
            if bytes.len().saturating_add(chunk.len()) > MAX_INBOUND_MEDIA_BYTES {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "weixin media {label} exceeds {} MiB download limit",
                    MAX_INBOUND_MEDIA_BYTES / 1024 / 1024
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let bytes = if let Some(aes_key) = aes_key {
            Self::decrypt_aes_128_ecb(&bytes, aes_key, label)?
        } else {
            bytes
        };
        Ok((header_mime, bytes))
    }

    fn validate_media_download_url(config: &ImProviderConfig, url: &str) -> Result<()> {
        let parsed = reqwest::Url::parse(url).map_err(|error| {
            bifrost_core::BifrostError::Config(format!(
                "weixin media download URL is invalid: {error}"
            ))
        })?;
        let host = parsed.host_str().unwrap_or_default();
        let official_cdn = host == "novac2c.cdn.weixin.qq.com" && parsed.scheme() == "https";
        let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
        let base_is_loopback = reqwest::Url::parse(Self::base_url(config))
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"));
        if official_cdn || (loopback && base_is_loopback) {
            return Ok(());
        }
        Err(bifrost_core::BifrostError::Config(
            "weixin media download URL host is not allowed".to_string(),
        ))
    }

    fn decrypt_aes_128_ecb(ciphertext: &[u8], aes_key: &str, label: &str) -> Result<Vec<u8>> {
        type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;
        let key = Self::parse_aes_key(aes_key, label)?;
        Aes128EcbDec::new_from_slice(&key)
            .map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin media {label} AES key init failed: {e}"
                ))
            })?
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "weixin media {label} AES decrypt failed: {e}"
                ))
            })
    }

    fn parse_aes_key(aes_key: &str, label: &str) -> Result<Vec<u8>> {
        let trimmed = aes_key.trim();
        if trimmed.len() == 32 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Self::hex_to_bytes(trimmed).ok_or_else(|| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin media {label} AES hex key parse failed"
                ))
            });
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .map_err(|e| {
                bifrost_core::BifrostError::Config(format!(
                    "weixin media {label} AES key base64 decode failed: {e}"
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
            "weixin media {label} AES key must decode to 16 raw bytes or 32-char hex string, got {} bytes",
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

impl WeixinProvider {
    pub(crate) async fn connect_events_with_status(
        &self,
        config: &ImProviderConfig,
        sink: EventSink,
        status_tx: Option<WeixinConnectionStatusTx>,
    ) -> Result<ConnectionHandle> {
        Self::bot_token(config)?;
        if self.sync_cursor_store.is_none() {
            return Err(bifrost_core::BifrostError::Config(
                "encrypted Weixin sync cursor store is unavailable".to_string(),
            ));
        }
        let config = config.clone();
        let provider = Self {
            http: self.http.clone(),
            poll_http: self.poll_http.clone(),
            login_http: self.login_http.clone(),
            runtime: Arc::clone(&self.runtime),
            context_store: self.context_store.clone(),
            sync_cursor_store: self.sync_cursor_store.clone(),
            pending_outbound_media: Arc::clone(&self.pending_outbound_media),
            typing_tickets: Arc::clone(&self.typing_tickets),
            active_channel_runs: Arc::clone(&self.active_channel_runs),
        };
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut consecutive_errors = 0u32;
            'polling: loop {
                let result = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    result = provider.poll_once(&config) => result,
                };
                match result {
                    Ok(batch) => {
                        let was_reconnecting = consecutive_errors > 0;
                        for event in batch.events {
                            if sink.send(event).is_err() {
                                send_connection_status(
                                    status_tx.as_ref(),
                                    ConnectionState::Disconnected,
                                    Some(
                                        "Weixin event sink closed; inbound polling stopped"
                                            .to_string(),
                                    ),
                                );
                                break 'polling;
                            }
                        }
                        if let Some(cursor) = batch.next_cursor {
                            let account_id = Self::account_id(&config).to_string();
                            let persist_result = provider
                                .sync_cursor_store
                                .as_ref()
                                .expect("sync cursor store checked before connection start")
                                .put(&config.id, &account_id, &cursor);
                            if let Err(error) = persist_result {
                                consecutive_errors = consecutive_errors.saturating_add(1);
                                warn!(
                                    provider_id = %config.id,
                                    error = %error,
                                    "failed to persist weixin sync cursor; cursor not advanced"
                                );
                                send_connection_status(
                                    status_tx.as_ref(),
                                    ConnectionState::Reconnecting,
                                    Some(error.to_string()),
                                );
                                let delay = if consecutive_errors >= 3 {
                                    Duration::from_secs(30)
                                } else {
                                    Duration::from_secs(2)
                                };
                                tokio::select! {
                                    _ = &mut shutdown_rx => break,
                                    _ = tokio::time::sleep(delay) => {}
                                }
                                continue;
                            }
                            provider
                                .runtime
                                .write()
                                .entry(Self::account_runtime_key(&config))
                                .or_default()
                                .get_updates_buf = cursor;
                        }
                        consecutive_errors = 0;
                        if was_reconnecting {
                            send_connection_status(
                                status_tx.as_ref(),
                                ConnectionState::Connected,
                                None,
                            );
                        }
                    }
                    Err(error) => {
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        let authentication_required =
                            error.to_string().contains("authentication required");
                        warn!(
                            provider_id = %config.id,
                            consecutive_errors,
                            error = %error,
                            "weixin poll failed"
                        );
                        if authentication_required {
                            send_connection_status(
                                status_tx.as_ref(),
                                ConnectionState::AuthenticationRequired,
                                Some(
                                    "Weixin authorization expired; scan a new QR code".to_string(),
                                ),
                            );
                            break;
                        }
                        send_connection_status(
                            status_tx.as_ref(),
                            ConnectionState::Reconnecting,
                            Some(error.to_string()),
                        );
                        let delay = if consecutive_errors >= 3 {
                            Duration::from_secs(30)
                        } else {
                            Duration::from_secs(2)
                        };
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                }
            }
            info!(provider_id = %config.id, "weixin poll connection stopped");
        });
        Ok(ConnectionHandle { shutdown_tx })
    }
}

fn send_connection_status(
    status_tx: Option<&WeixinConnectionStatusTx>,
    state: ConnectionState,
    error: Option<String>,
) {
    if let Some(status_tx) = status_tx {
        let _ = status_tx.send(WeixinConnectionStatusEvent { state, error });
    }
}

#[async_trait]
impl ImProvider for WeixinProvider {
    fn provider_type(&self) -> ImProviderType {
        ImProviderType::Weixin
    }

    fn send_capabilities(&self, config: &ImProviderConfig) -> ImSendCapabilities {
        let native = |max_bytes| ImSendPartCapability {
            support: ImSendSupportLevel::Native,
            delivered_as: None,
            max_bytes,
            reason: None,
        };
        let unsupported = |reason: &str| ImSendPartCapability {
            support: ImSendSupportLevel::Unsupported,
            delivered_as: None,
            max_bytes: None,
            reason: Some(reason.to_string()),
        };
        ImSendCapabilities {
            provider_id: config.id.clone(),
            provider_type: config.provider_type,
            destinations: vec!["owner".into(), "target".into(), "direct".into()],
            receive_id_types: vec!["open_id".into()],
            parts: BTreeMap::from([
                ("text".into(), native(None)),
                (
                    "markdown".into(),
                    ImSendPartCapability {
                        support: ImSendSupportLevel::Degraded,
                        delivered_as: Some("text".into()),
                        max_bytes: None,
                        reason: Some("Weixin renders Markdown as readable plain text".to_string()),
                    },
                ),
                ("image".into(), native(Some(10 * 1024 * 1024))),
                ("file".into(), native(Some(30 * 1024 * 1024))),
                ("video".into(), native(Some(30 * 1024 * 1024))),
                (
                    "native_card".into(),
                    unsupported("Weixin does not support Feishu native cards"),
                ),
            ]),
            requires_context: true,
        }
    }

    fn channel_capabilities(&self, config: &ImProviderConfig) -> ImChannelCapabilities {
        ImChannelCapabilities {
            send: self.send_capabilities(config),
            interaction: ImInteractionCapabilities {
                typing: true,
                progress: ImProgressPresentation::StructuredEvents,
                mutable_message: false,
                native_reply: false,
                reactions: false,
                recall: false,
            },
            conversation: ImConversationCapabilities {
                direct: true,
                group: false,
                thread: false,
                mention: false,
                requires_context: true,
            },
        }
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
        self.connect_events_with_status(config, sink, None).await
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
        let client_msg_id = format!("bifrost-weixin-{}-{}", now_ms(), uuid::Uuid::new_v4());
        self.send_text_with_client_id(config, target, text, &client_msg_id)
            .await
    }

    async fn upload_image(
        &self,
        _config: &ImProviderConfig,
        _image_type: &str,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<UploadedImage> {
        if bytes.is_empty() {
            return Err(bifrost_core::BifrostError::Config(
                "weixin outbound image upload requires non-empty bytes".to_string(),
            ));
        }
        if bytes.len() > MAX_OUTBOUND_IMAGE_BYTES {
            return Err(bifrost_core::BifrostError::Config(format!(
                "weixin outbound image exceeds {} MiB limit",
                MAX_OUTBOUND_IMAGE_BYTES / 1024 / 1024
            )));
        }
        let image_key = self.insert_pending_outbound_media(PendingOutboundMedia {
            kind: OutboundMediaKind::Image,
            file_name: file_name.trim().to_string(),
            bytes,
            mime_type: mime_type.map(str::to_string),
            created_at_ms: now_ms(),
        })?;
        Ok(UploadedImage {
            image_key,
            request_id: None,
        })
    }

    async fn send_image(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        image_key: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.send_outbound_media(
            config,
            target,
            image_key,
            Some(OutboundMediaKind::Image),
            uuid,
        )
        .await
    }

    async fn upload_file(
        &self,
        _config: &ImProviderConfig,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<String> {
        if bytes.is_empty() {
            return Err(bifrost_core::BifrostError::Config(
                "weixin outbound file upload requires non-empty bytes".to_string(),
            ));
        }
        if bytes.len() > MAX_OUTBOUND_FILE_BYTES {
            return Err(bifrost_core::BifrostError::Config(format!(
                "weixin outbound file exceeds {} MiB limit",
                MAX_OUTBOUND_FILE_BYTES / 1024 / 1024
            )));
        }
        let kind = Self::classify_outbound_file(&bytes, mime_type);
        self.insert_pending_outbound_media(PendingOutboundMedia {
            kind,
            file_name: file_name
                .trim()
                .split(['/', '\\'])
                .next_back()
                .filter(|value| !value.is_empty())
                .unwrap_or("attachment.bin")
                .to_string(),
            bytes,
            mime_type: mime_type.map(str::to_string),
            created_at_ms: now_ms(),
        })
    }

    async fn send_file(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        file_key: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.send_outbound_media(config, target, file_key, None, uuid)
            .await
    }

    async fn send_native_card(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _card: serde_json::Value,
        _opts: SendOptions,
    ) -> Result<SendResult> {
        Err(bifrost_core::BifrostError::Config(
            "weixin provider does not support native cards".to_string(),
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
#[path = "weixin_tests.rs"]
mod weixin_tests;
