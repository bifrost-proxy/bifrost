use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::{Mutex as ParkingMutex, RwLock};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};

use super::feishu::FeishuProvider;
use super::types::{ImEvent, ImEventMessage, ImEventSource, ImProviderConfig, ImProviderType};

pub const FEISHU_BOT_MENU_EVENT: &str = "application.bot.menu_v6";
pub const BIFROST_MENU_PRESET: &str = "bifrost-default-v1";
const MENU_STATE_VERSION: u32 = 1;
const MENU_STATE_FILENAME: &str = "im_gateway_feishu_menu_state.json";
const MENU_CONTENT_EVENT: i32 = 2;
const MENU_CONTENT_SUBMENU: i32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuBotMenuSpec {
    pub preset: String,
    pub enabled: bool,
    pub nodes: Vec<FeishuBotMenuNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuBotMenuNode {
    pub menu_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_menu_id: Option<String>,
    pub sort: i32,
    pub default_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_key: Option<String>,
    pub menu_content_type: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeishuMenuApplyStatus {
    #[default]
    NotApplied,
    DraftApplied,
    Published,
    UnsupportedAppType,
    UnderReview,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuMenuState {
    pub provider_id: String,
    pub status: FeishuMenuApplyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desired_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_applied_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_published_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub updated_at: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct FeishuMenuStateData {
    version: u32,
    providers: BTreeMap<String, FeishuMenuState>,
}

pub struct FeishuMenuStateStore {
    path: PathBuf,
    data: RwLock<FeishuMenuStateData>,
    save_lock: ParkingMutex<()>,
}

impl FeishuMenuStateStore {
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("admin").join(MENU_STATE_FILENAME);
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<FeishuMenuStateData>(&bytes).ok())
            .filter(|data| data.version == MENU_STATE_VERSION)
            .unwrap_or_else(|| FeishuMenuStateData {
                version: MENU_STATE_VERSION,
                providers: BTreeMap::new(),
            });
        Self {
            path,
            data: RwLock::new(data),
            save_lock: ParkingMutex::new(()),
        }
    }

    pub fn get(&self, provider_id: &str) -> Option<FeishuMenuState> {
        self.data.read().providers.get(provider_id).cloned()
    }

    pub fn save(&self, state: FeishuMenuState) -> Result<(), String> {
        // Serialize saves so every snapshot includes the preceding successful write. Keep the
        // visible in-memory state unchanged until its corresponding snapshot reaches disk.
        let _save_guard = self.save_lock.lock();
        let mut next = self.data.read().clone();
        next.providers.insert(state.provider_id.clone(), state);
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|error| format!("serialize Feishu menu state: {error}"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create Feishu menu state directory: {error}"))?;
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| format!("create Feishu menu state temp file: {error}"))?;
        temp.write_all(&bytes)
            .map_err(|error| format!("write Feishu menu state: {error}"))?;
        temp.as_file()
            .sync_all()
            .map_err(|error| format!("sync Feishu menu state: {error}"))?;
        temp.persist(&self.path).map_err(|error| {
            format!(
                "replace Feishu menu state {}: {}",
                self.path.display(),
                error.error
            )
        })?;
        *self.data.write() = next;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuMenuSyncOptions {
    #[serde(default)]
    pub publish: bool,
    #[serde(default = "default_bot_ability")]
    pub mobile_default_ability: String,
    #[serde(default = "default_bot_ability")]
    pub pc_default_ability: String,
}

impl Default for FeishuMenuSyncOptions {
    fn default() -> Self {
        Self {
            publish: false,
            mobile_default_ability: default_bot_ability(),
            pc_default_ability: default_bot_ability(),
        }
    }
}

fn default_bot_ability() -> String {
    "bot".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeishuMenuPreview {
    pub provider_id: String,
    pub preset: String,
    pub desired_digest: String,
    pub ability: serde_json::Value,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeishuMenuSyncResult {
    pub success: bool,
    pub provider_id: String,
    pub desired_digest: String,
    pub ability_updated: bool,
    pub event_subscription_updated: bool,
    pub published: bool,
    pub skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeishuProvisionError {
    pub error: String,
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feishu_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl fmt::Display for FeishuProvisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Feishu menu {} failed ({}): {}",
            self.stage, self.error, self.message
        )?;
        if let Some(code) = self.feishu_code {
            write!(formatter, ", code={code}")?;
        }
        if let Some(request_id) = self.request_id.as_deref() {
            write!(formatter, ", request_id={request_id}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FeishuProvisionError {}

pub struct FeishuAppProvisioner<'a> {
    provider: &'a FeishuProvider,
    state_store: &'a FeishuMenuStateStore,
}

impl<'a> FeishuAppProvisioner<'a> {
    pub fn new(provider: &'a FeishuProvider, state_store: &'a FeishuMenuStateStore) -> Self {
        Self {
            provider,
            state_store,
        }
    }

    pub fn preview(
        &self,
        config: &ImProviderConfig,
    ) -> Result<FeishuMenuPreview, FeishuProvisionError> {
        validate_provider(config)?;
        let menu = bifrost_default_menu();
        validate_menu(&menu).map_err(|message| FeishuProvisionError {
            error: "menu_validation_failed".to_string(),
            stage: "validate".to_string(),
            message,
            feishu_code: None,
            http_status: None,
            request_id: None,
        })?;
        let ability = menu_ability_payload(&menu);
        let event_config = menu_event_config_payload();
        Ok(FeishuMenuPreview {
            provider_id: config.id.clone(),
            preset: menu.preset,
            desired_digest: desired_digest(config, &ability, &event_config),
            ability,
            config: event_config,
        })
    }

    pub async fn reconcile(
        &self,
        config: &ImProviderConfig,
        options: &FeishuMenuSyncOptions,
    ) -> Result<FeishuMenuSyncResult, FeishuProvisionError> {
        let preview = self.preview(config)?;
        validate_publish_options(options)?;
        let previous = self.state_store.get(&config.id);
        let already_applied = previous.as_ref().is_some_and(|state| {
            state.last_applied_digest.as_deref() == Some(preview.desired_digest.as_str())
        });
        let already_published = previous.as_ref().is_some_and(|state| {
            state.last_published_digest.as_deref() == Some(preview.desired_digest.as_str())
        });
        if already_applied && (!options.publish || already_published) {
            return Ok(FeishuMenuSyncResult {
                success: true,
                provider_id: config.id.clone(),
                desired_digest: preview.desired_digest,
                ability_updated: false,
                event_subscription_updated: false,
                published: already_published,
                skipped: true,
                version_id: previous.as_ref().and_then(|state| state.version_id.clone()),
                version: previous.as_ref().and_then(|state| state.version.clone()),
                request_ids: Vec::new(),
            });
        }

        let app_secret = match config
            .secret_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(app_secret) => app_secret,
            None => {
                let error = simple_error(
                    "missing_app_credentials",
                    "validate",
                    "Feishu provider has no app secret",
                );
                self.record_error(config, &preview.desired_digest, previous, &error);
                return Err(error);
            }
        };
        let token = match self.provider.get_tenant_token(config, app_secret).await {
            Ok(token) => token,
            Err(source) => {
                let error = simple_error("token_failed", "token", &source.to_string());
                self.record_error(config, &preview.desired_digest, previous, &error);
                return Err(error);
            }
        };
        let app_id = config.app_id.as_deref().unwrap_or_default();
        let base_url = FeishuProvider::base_url(config);
        let mut request_ids = Vec::new();
        let mut ability_updated = false;
        let mut event_subscription_updated = false;

        if !already_applied {
            match self
                .patch(
                    &format!("{base_url}/application/v7/applications/{app_id}/ability"),
                    &token,
                    &preview.ability,
                    "ability",
                )
                .await
            {
                Ok(response) => {
                    ability_updated = true;
                    push_request_id(&mut request_ids, response.request_id);
                }
                Err(error) => {
                    self.record_error(config, &preview.desired_digest, previous.clone(), &error);
                    return Err(error);
                }
            }
            match self
                .patch(
                    &format!("{base_url}/application/v7/applications/{app_id}/config"),
                    &token,
                    &preview.config,
                    "config",
                )
                .await
            {
                Ok(response) => {
                    event_subscription_updated = true;
                    push_request_id(&mut request_ids, response.request_id);
                }
                Err(error) => {
                    self.record_error(config, &preview.desired_digest, previous.clone(), &error);
                    return Err(error);
                }
            }
        }

        let mut version_id = previous.as_ref().and_then(|state| state.version_id.clone());
        let mut version = previous.as_ref().and_then(|state| state.version.clone());
        let mut published = already_published;
        if options.publish && !already_published {
            let body = serde_json::json!({
                "mobile_default_ability": options.mobile_default_ability,
                "pc_default_ability": options.pc_default_ability,
                "remark": "Sync Bifrost bot command menu",
                "changelog": "Sync Bifrost bot command menu"
            });
            match self
                .post(
                    &format!("{base_url}/application/v7/applications/{app_id}/publish"),
                    &token,
                    &body,
                    "publish",
                )
                .await
            {
                Ok(response) => {
                    push_request_id(&mut request_ids, response.request_id);
                    version_id = response
                        .value
                        .pointer("/data/version_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    version = response
                        .value
                        .pointer("/data/version")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    published = true;
                }
                Err(error) => {
                    let mut state = previous.clone().unwrap_or_default();
                    state.provider_id = config.id.clone();
                    state.desired_digest = Some(preview.desired_digest.clone());
                    state.last_applied_digest = Some(preview.desired_digest.clone());
                    apply_error_to_state(&mut state, &error);
                    let _ = self.state_store.save(state);
                    return Err(error);
                }
            }
        }

        let status = if published {
            FeishuMenuApplyStatus::Published
        } else {
            FeishuMenuApplyStatus::DraftApplied
        };
        let state = FeishuMenuState {
            provider_id: config.id.clone(),
            status,
            desired_digest: Some(preview.desired_digest.clone()),
            last_applied_digest: Some(preview.desired_digest.clone()),
            last_published_digest: published.then(|| preview.desired_digest.clone()),
            version_id: version_id.clone(),
            version: version.clone(),
            last_error_stage: None,
            last_error_kind: None,
            last_error: None,
            request_id: request_ids.last().cloned(),
            updated_at: current_timestamp_ms(),
        };
        self.state_store
            .save(state)
            .map_err(|message| simple_error("state_persist_failed", "persist", &message))?;

        Ok(FeishuMenuSyncResult {
            success: true,
            provider_id: config.id.clone(),
            desired_digest: preview.desired_digest,
            ability_updated,
            event_subscription_updated,
            published,
            skipped: false,
            version_id,
            version,
            request_ids,
        })
    }

    fn record_error(
        &self,
        config: &ImProviderConfig,
        digest: &str,
        previous: Option<FeishuMenuState>,
        error: &FeishuProvisionError,
    ) {
        let mut state = previous.unwrap_or_default();
        state.provider_id = config.id.clone();
        state.desired_digest = Some(digest.to_string());
        apply_error_to_state(&mut state, error);
        let _ = self.state_store.save(state);
    }

    async fn patch(
        &self,
        url: &str,
        token: &str,
        body: &serde_json::Value,
        stage: &str,
    ) -> Result<FeishuApplicationResponse, FeishuProvisionError> {
        let response = self
            .provider
            .http_client()
            .patch(url)
            .header("Authorization", format!("Bearer {token}"))
            .json(body)
            .send()
            .await
            .map_err(|error| {
                simple_error(
                    &format!("{stage}_request_failed"),
                    stage,
                    &error.to_string(),
                )
            })?;
        parse_application_response(response, stage).await
    }

    async fn post(
        &self,
        url: &str,
        token: &str,
        body: &serde_json::Value,
        stage: &str,
    ) -> Result<FeishuApplicationResponse, FeishuProvisionError> {
        let response = self
            .provider
            .http_client()
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .json(body)
            .send()
            .await
            .map_err(|error| {
                simple_error(
                    &format!("{stage}_request_failed"),
                    stage,
                    &error.to_string(),
                )
            })?;
        parse_application_response(response, stage).await
    }
}

#[derive(Debug)]
struct FeishuApplicationResponse {
    value: serde_json::Value,
    request_id: Option<String>,
}

async fn parse_application_response(
    response: reqwest::Response,
    stage: &str,
) -> Result<FeishuApplicationResponse, FeishuProvisionError> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-tt-logid")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|error| FeishuProvisionError {
            error: format!("{stage}_response_invalid"),
            stage: stage.to_string(),
            message: error.to_string(),
            feishu_code: None,
            http_status: Some(status.as_u16()),
            request_id: request_id.clone(),
        })?;
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    if status.is_success() && code == 0 {
        return Ok(FeishuApplicationResponse { value, request_id });
    }
    let message = value
        .get("msg")
        .or_else(|| value.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown Feishu application error")
        .to_string();
    let lower = message.to_ascii_lowercase();
    let error = if lower.contains("review") || message.contains("审核") {
        "app_under_review"
    } else if lower.contains("not support")
        || lower.contains("unsupported")
        || message.contains("机器人助手")
        || message.contains("不支持")
    {
        "unsupported_app_type"
    } else {
        match stage {
            "ability" => "ability_update_failed",
            "config" => "event_update_failed",
            "publish" => "publish_failed",
            _ => "provision_failed",
        }
    };
    Err(FeishuProvisionError {
        error: error.to_string(),
        stage: stage.to_string(),
        message,
        feishu_code: (code != 0).then_some(code),
        http_status: Some(status.as_u16()),
        request_id,
    })
}

fn apply_error_to_state(state: &mut FeishuMenuState, error: &FeishuProvisionError) {
    state.status = match error.error.as_str() {
        "unsupported_app_type" => FeishuMenuApplyStatus::UnsupportedAppType,
        "app_under_review" => FeishuMenuApplyStatus::UnderReview,
        _ => FeishuMenuApplyStatus::Failed,
    };
    state.last_error_stage = Some(error.stage.clone());
    state.last_error_kind = Some(error.error.clone());
    state.last_error = Some(error.message.clone());
    state.request_id = error.request_id.clone();
    state.updated_at = current_timestamp_ms();
}

fn validate_provider(config: &ImProviderConfig) -> Result<(), FeishuProvisionError> {
    if config.provider_type != ImProviderType::Feishu {
        return Err(simple_error(
            "invalid_provider",
            "validate",
            "bot menus are only supported for Feishu providers",
        ));
    }
    if config
        .app_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(simple_error(
            "missing_app_credentials",
            "validate",
            "Feishu provider has no app id",
        ));
    }
    Ok(())
}

fn validate_publish_options(options: &FeishuMenuSyncOptions) -> Result<(), FeishuProvisionError> {
    if !options.publish {
        return Ok(());
    }
    let allowed = |value: &str| matches!(value, "bot" | "web_app" | "gadget");
    if !allowed(options.mobile_default_ability.trim())
        || !allowed(options.pc_default_ability.trim())
    {
        return Err(simple_error(
            "invalid_publish_options",
            "validate",
            "default ability must be one of: bot, web_app, gadget",
        ));
    }
    Ok(())
}

fn simple_error(error: &str, stage: &str, message: &str) -> FeishuProvisionError {
    FeishuProvisionError {
        error: error.to_string(),
        stage: stage.to_string(),
        message: message.to_string(),
        feishu_code: None,
        http_status: None,
        request_id: None,
    }
}

fn push_request_id(request_ids: &mut Vec<String>, request_id: Option<String>) {
    if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
        request_ids.push(request_id);
    }
}

pub fn bifrost_default_menu() -> FeishuBotMenuSpec {
    FeishuBotMenuSpec {
        preset: BIFROST_MENU_PRESET.to_string(),
        enabled: true,
        nodes: vec![
            submenu("session", 10, "会话"),
            action("status", "session", 10, "状态", "bifrost.status"),
            action("resume", "session", 20, "恢复会话", "bifrost.resume"),
            action("queue", "session", 30, "排队状态", "bifrost.queue.status"),
            action("stop", "session", 40, "停止任务", "bifrost.stop"),
            submenu("agent", 20, "Agent"),
            action("model", "agent", 10, "选择模型", "bifrost.model.select"),
            action(
                "runner",
                "agent",
                20,
                "选择 Runner",
                "bifrost.runner.select",
            ),
            action("effort", "agent", 30, "推理强度", "bifrost.effort.select"),
            action("fast", "agent", 40, "Fast 模式", "bifrost.fast.manage"),
            submenu("tools", 30, "工具"),
            action("pwd", "tools", 10, "当前目录", "bifrost.cwd.show"),
            action("help", "tools", 20, "使用帮助", "bifrost.help"),
        ],
    }
}

fn submenu(menu_id: &str, sort: i32, name: &str) -> FeishuBotMenuNode {
    FeishuBotMenuNode {
        menu_id: menu_id.to_string(),
        parent_menu_id: None,
        sort,
        default_name: name.to_string(),
        event_key: None,
        menu_content_type: MENU_CONTENT_SUBMENU,
    }
}

fn action(
    menu_id: &str,
    parent_menu_id: &str,
    sort: i32,
    name: &str,
    event_key: &str,
) -> FeishuBotMenuNode {
    FeishuBotMenuNode {
        menu_id: menu_id.to_string(),
        parent_menu_id: Some(parent_menu_id.to_string()),
        sort,
        default_name: name.to_string(),
        event_key: Some(event_key.to_string()),
        menu_content_type: MENU_CONTENT_EVENT,
    }
}

pub fn menu_event_key_to_command(event_key: &str) -> Option<&'static str> {
    match event_key {
        "bifrost.status" => Some("/status"),
        "bifrost.resume" => Some("/resume"),
        "bifrost.queue.status" => Some("/q"),
        "bifrost.stop" => Some("/stop"),
        "bifrost.model.select" => Some("/model"),
        "bifrost.runner.select" => Some("/runner"),
        "bifrost.effort.select" => Some("/effort"),
        "bifrost.fast.manage" => Some("/fast status"),
        "bifrost.cwd.show" => Some("/pwd"),
        "bifrost.help" => Some("/help"),
        _ => None,
    }
}

pub fn normalize_feishu_menu_event(raw: &serde_json::Value, provider_id: &str) -> Option<ImEvent> {
    let header = raw.get("header")?;
    if header.get("event_type")?.as_str()? != FEISHU_BOT_MENU_EVENT {
        return None;
    }
    let event_id = header.get("event_id")?.as_str()?.trim();
    if event_id.is_empty() {
        return None;
    }
    let event = raw.get("event")?;
    let user_id = event
        .pointer("/operator/operator_id/open_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let event_key = event
        .get("event_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)?;
    let command = menu_event_key_to_command(event_key)?;
    let received_at = event
        .get("timestamp")
        .and_then(serde_json::Value::as_u64)
        .filter(|timestamp| *timestamp > 0)
        .map(normalize_feishu_event_timestamp)
        .unwrap_or_else(current_timestamp_ms);
    Some(ImEvent {
        event_id: event_id.to_string(),
        provider_id: provider_id.to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: ImEventSource {
            chat_id: None,
            chat_type: Some("p2p".to_string()),
            user_id: Some(user_id.to_string()),
            user_name: None,
            sender_type: Some("user".to_string()),
            message_id: None,
        },
        message: Some(ImEventMessage {
            text: command.to_string(),
            raw_type: Some(FEISHU_BOT_MENU_EVENT.to_string()),
            raw_content: Some(serde_json::json!({
                "event_key": event_key,
                "timestamp": event.get("timestamp").cloned().unwrap_or_default()
            })),
            ..ImEventMessage::default()
        }),
        received_at,
        raw_digest: Some(raw_digest(raw)),
    })
}

fn normalize_feishu_event_timestamp(timestamp: u64) -> u64 {
    // Feishu documents menu-event timestamps in Unix seconds, while ImEvent uses milliseconds.
    // Retain compatibility if the platform already sends a millisecond timestamp.
    if timestamp < 100_000_000_000 {
        timestamp.saturating_mul(1_000)
    } else {
        timestamp
    }
}

pub fn validate_menu(menu: &FeishuBotMenuSpec) -> Result<(), String> {
    if !menu.enabled {
        return Ok(());
    }
    let mut ids = BTreeSet::new();
    for node in &menu.nodes {
        if node.menu_id.trim().is_empty() || !ids.insert(node.menu_id.as_str()) {
            return Err(format!(
                "menu_id must be non-empty and unique: {}",
                node.menu_id
            ));
        }
    }
    let roots = menu
        .nodes
        .iter()
        .filter(|node| node.parent_menu_id.is_none())
        .collect::<Vec<_>>();
    if roots.len() > 3 {
        return Err("ordinary Feishu bot menus allow at most 3 root nodes".to_string());
    }
    for root in roots {
        if root.menu_content_type != MENU_CONTENT_SUBMENU || root.event_key.is_some() {
            return Err(format!("root menu {} must be a submenu", root.menu_id));
        }
        let children = menu
            .nodes
            .iter()
            .filter(|node| node.parent_menu_id.as_deref() == Some(root.menu_id.as_str()))
            .count();
        if children > 5 {
            return Err(format!(
                "root menu {} has more than 5 children",
                root.menu_id
            ));
        }
    }
    for node in menu
        .nodes
        .iter()
        .filter(|node| node.parent_menu_id.is_some())
    {
        let parent_id = node.parent_menu_id.as_deref().unwrap_or_default();
        if !menu
            .nodes
            .iter()
            .any(|parent| parent.menu_id == parent_id && parent.parent_menu_id.is_none())
        {
            return Err(format!("menu {} has an unknown parent", node.menu_id));
        }
        if node.menu_content_type != MENU_CONTENT_EVENT {
            return Err(format!("menu {} must be an event action", node.menu_id));
        }
        let event_key = node
            .event_key
            .as_deref()
            .ok_or_else(|| format!("menu {} has no event_key", node.menu_id))?;
        if menu_event_key_to_command(event_key).is_none() {
            return Err(format!("menu {} has an unknown event_key", node.menu_id));
        }
    }
    Ok(())
}

pub fn menu_ability_payload(menu: &FeishuBotMenuSpec) -> serde_json::Value {
    serde_json::json!({
        "bot": {
            "bot_menu_enable": menu.enabled,
            "bot_menus": menu.nodes
        }
    })
}

pub fn menu_event_config_payload() -> serde_json::Value {
    serde_json::json!({
        "event": {
            "add_events": [FEISHU_BOT_MENU_EVENT]
        }
    })
}

fn desired_digest(
    provider: &ImProviderConfig,
    ability: &serde_json::Value,
    config: &serde_json::Value,
) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "preset": BIFROST_MENU_PRESET,
        "application": {
            "app_id": provider.app_id.as_deref().unwrap_or_default().trim(),
            "base_url": FeishuProvider::base_url(provider),
        },
        "ability": ability,
        "config": config
    }))
    .unwrap_or_default();
    let value = digest(&SHA256, &bytes);
    format!("sha256:{}", hex_encode(value.as_ref()))
}

fn raw_digest(raw: &serde_json::Value) -> String {
    let value = digest(&SHA256, raw.to_string().as_bytes());
    format!("sha256:{}", hex_encode(value.as_ref()))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "feishu_menu_coverage_tests.rs"]
mod coverage_tests;

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

    #[derive(Clone, Debug)]
    struct CapturedApplicationRequest {
        method: String,
        path: String,
        authorization: Option<String>,
        body: serde_json::Value,
    }

    async fn spawn_application_api_fixture(
        failure_stage: Option<&'static str>,
    ) -> (
        String,
        Arc<Mutex<Vec<CapturedApplicationRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Feishu application fixture");
        let address = listener
            .local_addr()
            .expect("Feishu application fixture address");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_server = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let captured = Arc::clone(&captured_server);
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let captured = Arc::clone(&captured);
                        async move {
                            let method = request.method().to_string();
                            let path = request.uri().path().to_string();
                            let authorization = request
                                .headers()
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string);
                            let body = request.into_body().collect().await?.to_bytes();
                            let body = if body.is_empty() {
                                serde_json::Value::Null
                            } else {
                                serde_json::from_slice(&body).expect("application request JSON")
                            };
                            captured.lock().await.push(CapturedApplicationRequest {
                                method,
                                path: path.clone(),
                                authorization,
                                body,
                            });

                            let stage = if path.ends_with("/ability") {
                                Some("ability")
                            } else if path.ends_with("/config") {
                                Some("config")
                            } else if path.ends_with("/publish") {
                                Some("publish")
                            } else {
                                None
                            };
                            let (status, response, request_id) = if path
                                .ends_with("/auth/v3/tenant_access_token/internal")
                            {
                                (
                                    StatusCode::OK,
                                    r#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                    "token-request",
                                )
                            } else if failure_stage == stage {
                                (
                                    StatusCode::CONFLICT,
                                    if stage == Some("publish") {
                                        r#"{"code":12345,"msg":"unsupported PersonalAgent app type"}"#
                                    } else {
                                        r#"{"code":23456,"msg":"application is under review"}"#
                                    },
                                    "failed-request",
                                )
                            } else if stage == Some("publish") {
                                (
                                    StatusCode::OK,
                                    r#"{"code":0,"data":{"version_id":"v-id-1","version":"1.0.1"}}"#,
                                    "publish-request",
                                )
                            } else if stage.is_some() {
                                (
                                    StatusCode::OK,
                                    r#"{"code":0,"data":{}}"#,
                                    if stage == Some("ability") {
                                        "ability-request"
                                    } else {
                                        "config-request"
                                    },
                                )
                            } else {
                                (
                                    StatusCode::NOT_FOUND,
                                    r#"{"code":404,"msg":"not found"}"#,
                                    "not-found",
                                )
                            };
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(status)
                                    .header("Content-Type", "application/json")
                                    .header("x-tt-logid", request_id)
                                    .body(Full::new(Bytes::copy_from_slice(response.as_bytes())))
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
        (format!("http://{address}/open-apis"), captured, task)
    }

    fn menu_provider(base_url: String) -> ImProviderConfig {
        ImProviderConfig {
            id: "feishu-menu-unit".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Menu".to_string(),
            enabled: true,
            base_url: Some(base_url),
            app_id: Some("cli_menu".to_string()),
            secret_ref: Some("app-secret".to_string()),
            owner_open_id: Some("ou_owner".to_string()),
            event_connection_enabled: true,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn default_menu_is_valid_and_maps_every_action() {
        let menu = bifrost_default_menu();
        validate_menu(&menu).unwrap();
        assert_eq!(
            menu.nodes
                .iter()
                .filter(|node| node.parent_menu_id.is_none())
                .count(),
            3
        );
        assert_eq!(menu_event_key_to_command("bifrost.status"), Some("/status"));
        assert_eq!(
            menu_event_key_to_command("bifrost.fast.manage"),
            Some("/fast status")
        );
        assert_eq!(menu_event_key_to_command("/stop"), None);
    }

    #[test]
    fn menu_validation_rejects_bad_shape_and_unknown_actions() {
        let mut menu = bifrost_default_menu();
        menu.nodes[1].event_key = Some("/arbitrary".to_string());
        assert!(validate_menu(&menu)
            .unwrap_err()
            .contains("unknown event_key"));

        let mut menu = bifrost_default_menu();
        menu.nodes[1].parent_menu_id = Some("missing".to_string());
        assert!(validate_menu(&menu).unwrap_err().contains("unknown parent"));

        let mut menu = bifrost_default_menu();
        menu.nodes.push(submenu("fourth", 40, "More"));
        assert!(validate_menu(&menu).unwrap_err().contains("at most 3"));
    }

    #[test]
    fn menu_event_normalizes_to_private_canonical_command() {
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt-menu-1",
                "event_type": "application.bot.menu_v6"
            },
            "event": {
                "operator": {
                    "operator_id": {"open_id": "ou_owner"}
                },
                "event_key": "bifrost.model.select",
                "timestamp": 1787000000000u64
            }
        });
        let event = normalize_feishu_menu_event(&raw, "feishu-main").unwrap();
        assert_eq!(event.event_type, "message.receive");
        assert_eq!(event.source.user_id.as_deref(), Some("ou_owner"));
        assert_eq!(event.source.chat_id, None);
        assert_eq!(event.source.chat_type.as_deref(), Some("p2p"));
        assert_eq!(event.message.unwrap().text, "/model");
        assert_eq!(event.received_at, 1787000000000);
    }

    #[test]
    fn menu_event_normalizes_second_timestamp_and_preserves_milliseconds() {
        let event = |timestamp: u64| {
            serde_json::json!({
                "header": {
                    "event_id": format!("evt-menu-{timestamp}"),
                    "event_type": FEISHU_BOT_MENU_EVENT
                },
                "event": {
                    "operator": {
                        "operator_id": {"open_id": "ou_owner"}
                    },
                    "event_key": "bifrost.status",
                    "timestamp": timestamp
                }
            })
        };

        assert_eq!(
            normalize_feishu_menu_event(&event(1_669_364_458), "feishu-main")
                .unwrap()
                .received_at,
            1_669_364_458_000
        );
        assert_eq!(
            normalize_feishu_menu_event(&event(1_787_000_000_000), "feishu-main")
                .unwrap()
                .received_at,
            1_787_000_000_000
        );
    }

    #[test]
    fn menu_event_rejects_unknown_key_or_missing_operator() {
        let base = serde_json::json!({
            "header": {"event_id": "evt", "event_type": FEISHU_BOT_MENU_EVENT},
            "event": {
                "operator": {"operator_id": {"open_id": "ou_owner"}},
                "event_key": "unknown"
            }
        });
        assert!(normalize_feishu_menu_event(&base, "feishu-main").is_none());
        let mut missing = base;
        missing["event"]["event_key"] = serde_json::json!("bifrost.help");
        missing["event"]["operator"] = serde_json::Value::Null;
        assert!(normalize_feishu_menu_event(&missing, "feishu-main").is_none());
    }

    #[test]
    fn state_store_round_trips_and_preview_is_stable() {
        let temp = tempfile::tempdir().unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let state = FeishuMenuState {
            provider_id: "feishu-main".to_string(),
            status: FeishuMenuApplyStatus::DraftApplied,
            updated_at: 7,
            ..FeishuMenuState::default()
        };
        store.save(state.clone()).unwrap();
        assert_eq!(store.get("feishu-main"), Some(state));
        assert_eq!(
            FeishuMenuStateStore::new(temp.path())
                .get("feishu-main")
                .unwrap()
                .status,
            FeishuMenuApplyStatus::DraftApplied
        );
    }

    #[test]
    fn state_store_does_not_update_memory_when_persistence_fails() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("admin"), b"blocks directory creation").unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let state = FeishuMenuState {
            provider_id: "feishu-main".to_string(),
            status: FeishuMenuApplyStatus::DraftApplied,
            updated_at: 7,
            ..FeishuMenuState::default()
        };

        assert!(store.save(state).is_err());
        assert_eq!(store.get("feishu-main"), None);
    }

    #[test]
    fn preview_digest_tracks_target_application_without_hashing_the_secret() {
        let provider = FeishuProvider::new();
        let temp = tempfile::tempdir().unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let provisioner = FeishuAppProvisioner::new(&provider, &store);
        let original = menu_provider("https://open.feishu.cn/open-apis".to_string());
        let original_digest = provisioner.preview(&original).unwrap().desired_digest;

        let mut other_app = original.clone();
        other_app.app_id = Some("cli_other".to_string());
        assert_ne!(
            provisioner.preview(&other_app).unwrap().desired_digest,
            original_digest
        );

        let mut rotated_secret = original.clone();
        rotated_secret.secret_ref = Some("rotated-secret".to_string());
        assert_eq!(
            provisioner.preview(&rotated_secret).unwrap().desired_digest,
            original_digest,
            "credential rotation must not leak into or invalidate desired state"
        );
    }

    #[tokio::test]
    async fn reconcile_updates_menu_event_and_publish_then_skips_same_digest() {
        let (base_url, captured, server) = spawn_application_api_fixture(None).await;
        let provider_config = menu_provider(base_url);
        let provider = FeishuProvider::new();
        let temp = tempfile::tempdir().unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let provisioner = FeishuAppProvisioner::new(&provider, &store);

        let result = provisioner
            .reconcile(
                &provider_config,
                &FeishuMenuSyncOptions {
                    publish: true,
                    ..FeishuMenuSyncOptions::default()
                },
            )
            .await
            .unwrap();
        assert!(result.ability_updated);
        assert!(result.event_subscription_updated);
        assert!(result.published);
        assert!(!result.skipped);
        assert_eq!(result.version_id.as_deref(), Some("v-id-1"));
        assert_eq!(result.version.as_deref(), Some("1.0.1"));
        assert_eq!(
            result.request_ids,
            ["ability-request", "config-request", "publish-request"]
        );

        let requests = captured.lock().await.clone();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(
            requests[0].path,
            "/open-apis/auth/v3/tenant_access_token/internal"
        );
        assert_eq!(requests[0].body["app_id"], "cli_menu");
        assert_eq!(requests[0].body["app_secret"], "app-secret");

        assert_eq!(requests[1].method, "PATCH");
        assert_eq!(
            requests[1].path,
            "/open-apis/application/v7/applications/cli_menu/ability"
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer tenant-token")
        );
        assert_eq!(requests[1].body["bot"]["bot_menu_enable"], true);
        assert!(requests[1].body["bot"].get("enable").is_none());
        assert_eq!(
            requests[1].body["bot"]["bot_menus"]
                .as_array()
                .unwrap()
                .len(),
            13
        );

        assert_eq!(requests[2].method, "PATCH");
        assert_eq!(
            requests[2].path,
            "/open-apis/application/v7/applications/cli_menu/config"
        );
        assert_eq!(
            requests[2].body,
            serde_json::json!({"event": {"add_events": [FEISHU_BOT_MENU_EVENT]}})
        );
        assert_eq!(requests[3].method, "POST");
        assert_eq!(
            requests[3].path,
            "/open-apis/application/v7/applications/cli_menu/publish"
        );
        assert_eq!(requests[3].body["mobile_default_ability"], "bot");
        assert_eq!(requests[3].body["pc_default_ability"], "bot");

        let state = store.get(&provider_config.id).unwrap();
        assert_eq!(state.status, FeishuMenuApplyStatus::Published);
        assert_eq!(state.last_applied_digest, state.last_published_digest);
        let skipped = provisioner
            .reconcile(
                &provider_config,
                &FeishuMenuSyncOptions {
                    publish: true,
                    ..FeishuMenuSyncOptions::default()
                },
            )
            .await
            .unwrap();
        assert!(skipped.skipped);
        assert!(skipped.published);
        assert_eq!(captured.lock().await.len(), 4);
        server.abort();
    }

    #[tokio::test]
    async fn reconcile_classifies_publish_app_type_and_preserves_applied_draft() {
        let (base_url, captured, server) = spawn_application_api_fixture(Some("publish")).await;
        let provider_config = menu_provider(base_url);
        let provider = FeishuProvider::new();
        let temp = tempfile::tempdir().unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let provisioner = FeishuAppProvisioner::new(&provider, &store);
        let options = FeishuMenuSyncOptions {
            publish: true,
            ..FeishuMenuSyncOptions::default()
        };

        let error = provisioner
            .reconcile(&provider_config, &options)
            .await
            .unwrap_err();
        assert_eq!(error.error, "unsupported_app_type");
        assert_eq!(error.stage, "publish");
        assert_eq!(error.feishu_code, Some(12345));
        assert_eq!(error.http_status, Some(409));
        assert_eq!(error.request_id.as_deref(), Some("failed-request"));
        let state = store.get(&provider_config.id).unwrap();
        assert_eq!(state.status, FeishuMenuApplyStatus::UnsupportedAppType);
        assert!(state.last_applied_digest.is_some());
        assert!(state.last_published_digest.is_none());

        let retry_error = provisioner
            .reconcile(&provider_config, &options)
            .await
            .unwrap_err();
        assert_eq!(retry_error.error, "unsupported_app_type");
        let requests = captured.lock().await.clone();
        assert_eq!(requests.len(), 5);
        assert_eq!(requests[4].path, requests[3].path);
        server.abort();
    }

    #[tokio::test]
    async fn reconcile_classifies_review_failure_without_marking_draft_applied() {
        let (base_url, _captured, server) = spawn_application_api_fixture(Some("ability")).await;
        let provider_config = menu_provider(base_url);
        let provider = FeishuProvider::new();
        let temp = tempfile::tempdir().unwrap();
        let store = FeishuMenuStateStore::new(temp.path());
        let error = FeishuAppProvisioner::new(&provider, &store)
            .reconcile(&provider_config, &FeishuMenuSyncOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.error, "app_under_review");
        assert_eq!(error.stage, "ability");
        let state = store.get(&provider_config.id).unwrap();
        assert_eq!(state.status, FeishuMenuApplyStatus::UnderReview);
        assert!(state.last_applied_digest.is_none());
        server.abort();
    }
}
