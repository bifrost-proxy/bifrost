use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bifrost_core::{BifrostError, Result};
use bifrost_storage::{
    content_hash, ConfigChangeEvent, ConfigManager, RuleFile, RuleSyncStatus, RulesChangeOrigin,
    RulesStorage, SyncConfig, SyncConfigUpdate, TlsConfigUpdate, DEFAULT_REMOTE_BASE_URL,
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify, RwLock};

use crate::client::SyncHttpClient;
use crate::client::{SyncBasicConfigCheckItem, SyncBasicConfigUpdateItem};
use crate::github_gist::{
    GitHubGistBasicConfig, GitHubGistClient, GitHubGistRule, GitHubGistSnapshot,
};
use crate::normalize::normalize_remote_rule;
use crate::types::{RemoteBasicConfig, RemoteEnv, RemoteUser, SyncReason};

const TOMBSTONE_MAX_AGE_SECS: i64 = 7 * 24 * 3600;
const TOMBSTONE_MIN_AGE_SECS: i64 = 120;
const STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS: usize = 3;
const STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_SECS: u64 = 15;
const STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV: &str =
    "BIFROST_SYNC_STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS";
const DISABLE_AUTO_LOGIN_PROMPT_ENV: &str = "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT";
const LOGIN_BROWSER_DRY_RUN_FILE_ENV: &str = "BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_GIST_REMOTE_BASE_URL: &str = "https://api.github.com/gists";
const GITHUB_GIST_PROVIDER_ID: &str = "github_gist";

pub type SharedSyncManager = Arc<SyncManager>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DeletedRuleTombstone {
    rule_id: String,
    rule_name: String,
    remote_id: String,
    remote_user_id: String,
    base_remote_updated_at: Option<String>,
    base_content_hash: Option<String>,
    deleted_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StartupLoginPromptFile {
    auto_prompted_at: String,
    remote_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BasicConfigSyncMeta {
    remote_updated_at: Option<String>,
    hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProviderSyncMeta {
    last_sync_at: Option<String>,
    last_sync_action: Option<SyncAction>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SyncStateFile {
    token: Option<String>,
    user: Option<RemoteUser>,
    #[serde(default)]
    provider_sessions: HashMap<String, SyncProviderSession>,
    #[serde(default)]
    provider_sync: HashMap<String, ProviderSyncMeta>,
    last_sync_at: Option<String>,
    last_sync_action: Option<SyncAction>,
    startup_login_prompt: Option<StartupLoginPromptFile>,
    deleted_rules: HashMap<String, DeletedRuleTombstone>,
    basic_configs: HashMap<String, BasicConfigSyncMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SyncProviderSession {
    token: String,
    remote_base_url: String,
    user: Option<RemoteUser>,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Clone)]
enum SyncPlanStep {
    DeleteLocal {
        tombstone: DeletedRuleTombstone,
    },
    DeleteRemote {
        tombstone: DeletedRuleTombstone,
        remote_env: RemoteEnv,
    },
    UpdateRemote {
        local_rule: RuleFile,
        remote_env: RemoteEnv,
    },
    CreateRemote {
        local_rule: RuleFile,
    },
    UpdateLocal {
        local_rule: RuleFile,
        remote_env: RemoteEnv,
    },
    CreateLocal {
        remote_env: RemoteEnv,
    },
}

#[derive(Debug, Clone, Default)]
struct LoginPromptState {
    last_opened_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncStatus {
    pub enabled: bool,
    pub auto_sync: bool,
    pub remote_base_url: String,
    pub has_session: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub syncing: bool,
    pub reason: SyncReason,
    pub last_sync_at: Option<String>,
    pub last_sync_action: Option<SyncAction>,
    pub last_error: Option<String>,
    pub user: Option<RemoteUser>,
    pub providers: Vec<SyncProviderStatus>,
    pub first_run_prompt_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncProviderCapabilities {
    pub rules_sync: bool,
    pub config_sync: bool,
    pub remote_invoke: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncProviderStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub remote_base_url: Option<String>,
    pub connected: bool,
    pub enabled: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub reason: SyncReason,
    pub last_error: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_sync_action: Option<SyncAction>,
    pub user: Option<RemoteUser>,
    pub capabilities: SyncProviderCapabilities,
    pub remote_invoke_registered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteInvokeProviderTarget {
    pub provider_id: String,
    pub remote_base_url: String,
    pub session_token: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    LocalPushed,
    RemotePulled,
    Bidirectional,
    NoChange,
}

#[derive(Debug, Clone)]
pub struct SyncOnceResult {
    pub success: bool,
    pub message: String,
    pub action: Option<SyncAction>,
    pub user: Option<RemoteUser>,
    pub local_rules: usize,
    pub remote_rules: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SyncRuntimeState {
    pub reachable: bool,
    pub authorized: bool,
    pub syncing: bool,
    pub reason: SyncReason,
    pub last_error: Option<String>,
}

fn is_bytedance_remote(remote_base_url: &str) -> bool {
    remote_base_url.contains("bytedance.net")
}

fn normalize_remote_base_url(remote_base_url: &str) -> String {
    remote_base_url.trim().trim_end_matches('/').to_string()
}

fn provider_id_for_remote(remote_base_url: &str) -> Option<&'static str> {
    let normalized = normalize_remote_base_url(remote_base_url);
    if normalized.is_empty() {
        return None;
    }
    if is_bytedance_remote(&normalized) {
        Some("bytedance_internal")
    } else {
        Some("bifrost_cloud")
    }
}

fn is_github_gist_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("github gist") || normalized.contains("github token")
}

fn is_github_gist_auth_error(message: &str) -> bool {
    if !is_github_gist_error(message) {
        return false;
    }
    let normalized = message.to_ascii_lowercase();
    normalized.contains("invalid")
        || normalized.contains("missing the gist scope")
        || normalized.contains("does not have access")
        || normalized.contains("unauthorized")
        || normalized.contains("status 401")
        || normalized.contains("status 403")
}

fn session_for_provider<'a>(
    provider_sessions: &'a HashMap<String, SyncProviderSession>,
    provider_id: &str,
) -> Option<&'a SyncProviderSession> {
    provider_sessions
        .get(provider_id)
        .filter(|session| !session.token.trim().is_empty())
}

fn provider_meta_for<'a>(
    provider_sync: &'a HashMap<String, ProviderSyncMeta>,
    provider_id: &str,
) -> Option<&'a ProviderSyncMeta> {
    provider_sync.get(provider_id)
}

fn record_provider_sync_success(
    state: &mut SyncStateFile,
    provider_id: &str,
    at: String,
    action: Option<SyncAction>,
) {
    let meta = state
        .provider_sync
        .entry(provider_id.to_string())
        .or_default();
    meta.last_sync_at = Some(at);
    meta.last_sync_action = action;
    meta.last_error = None;
}

fn record_provider_sync_error(state: &mut SyncStateFile, provider_id: &str, error: String) {
    let meta = state
        .provider_sync
        .entry(provider_id.to_string())
        .or_default();
    meta.last_error = Some(error);
}

fn update_provider_session_user(
    sync_config: &SyncConfig,
    state: &mut SyncStateFile,
    user: &RemoteUser,
) {
    if let Some(provider_id) = provider_id_for_remote(&sync_config.remote_base_url) {
        if let Some(session) = state.provider_sessions.get_mut(provider_id) {
            session.user = Some(user.clone());
        }
    }
}

fn build_provider_statuses(
    sync_config: &SyncConfig,
    runtime: &SyncRuntimeState,
    user: &Option<RemoteUser>,
    has_session: bool,
    provider_sessions: &HashMap<String, SyncProviderSession>,
    provider_sync: &HashMap<String, ProviderSyncMeta>,
) -> Vec<SyncProviderStatus> {
    let current_url = sync_config
        .remote_base_url
        .trim_end_matches('/')
        .to_string();
    let bytedance_selected = is_bytedance_remote(&current_url);
    let cloud_selected = !bytedance_selected && !current_url.is_empty();
    let current_connected = has_session && runtime.authorized;
    let bytedance_session = session_for_provider(provider_sessions, "bytedance_internal");
    let cloud_session = session_for_provider(provider_sessions, "bifrost_cloud");
    let github_gist_session = session_for_provider(provider_sessions, "github_gist");
    let bytedance_meta = provider_meta_for(provider_sync, "bytedance_internal");
    let cloud_meta = provider_meta_for(provider_sync, "bifrost_cloud");
    let github_gist_meta = provider_meta_for(provider_sync, "github_gist");
    let github_gist_last_error = runtime
        .last_error
        .as_ref()
        .filter(|error| is_github_gist_error(error))
        .cloned()
        .or_else(|| github_gist_meta.and_then(|meta| meta.last_error.clone()));
    let github_gist_auth_error = github_gist_last_error
        .as_deref()
        .is_some_and(is_github_gist_auth_error);
    let bytedance_connected =
        bytedance_session.is_some() || (bytedance_selected && current_connected);
    let cloud_connected = cloud_session.is_some() || (cloud_selected && current_connected);
    let github_gist_connected = github_gist_session.is_some();
    let github_gist_authorized = github_gist_connected && !github_gist_auth_error;

    vec![
        SyncProviderStatus {
            id: "bytedance_internal".to_string(),
            name: "ByteDance Internal".to_string(),
            description: "Internal trusted sync and Remote Invoke provider.".to_string(),
            remote_base_url: Some(DEFAULT_REMOTE_BASE_URL.to_string()),
            connected: bytedance_connected,
            enabled: sync_config.enabled && (bytedance_selected || bytedance_session.is_some()),
            reachable: bytedance_session.is_some() || (bytedance_selected && runtime.reachable),
            authorized: bytedance_session.is_some() || (bytedance_selected && runtime.authorized),
            reason: if bytedance_selected {
                runtime.reason.clone()
            } else if bytedance_session.is_some() {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: (bytedance_selected
                && !runtime
                    .last_error
                    .as_deref()
                    .is_some_and(is_github_gist_error))
            .then(|| runtime.last_error.clone())
            .flatten()
            .or_else(|| bytedance_meta.and_then(|meta| meta.last_error.clone())),
            last_sync_at: bytedance_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: bytedance_meta.and_then(|meta| meta.last_sync_action),
            user: bytedance_session
                .and_then(|session| session.user.clone())
                .or_else(|| {
                    (bytedance_selected && has_session)
                        .then(|| user.clone())
                        .flatten()
                }),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: true,
            },
            remote_invoke_registered: bytedance_connected,
        },
        SyncProviderStatus {
            id: "bifrost_cloud".to_string(),
            name: "Bifrost Cloud".to_string(),
            description: "Custom Bifrost sync service for teams and self-hosting.".to_string(),
            remote_base_url: cloud_session
                .map(|session| session.remote_base_url.clone())
                .or_else(|| cloud_selected.then(|| current_url.clone())),
            connected: cloud_connected,
            enabled: sync_config.enabled && (cloud_selected || cloud_session.is_some()),
            reachable: cloud_session.is_some() || (cloud_selected && runtime.reachable),
            authorized: cloud_session.is_some() || (cloud_selected && runtime.authorized),
            reason: if cloud_selected {
                runtime.reason.clone()
            } else if cloud_session.is_some() {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: (cloud_selected
                && !runtime
                    .last_error
                    .as_deref()
                    .is_some_and(is_github_gist_error))
            .then(|| runtime.last_error.clone())
            .flatten()
            .or_else(|| cloud_meta.and_then(|meta| meta.last_error.clone())),
            last_sync_at: cloud_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: cloud_meta.and_then(|meta| meta.last_sync_action),
            user: cloud_session
                .and_then(|session| session.user.clone())
                .or_else(|| {
                    (cloud_selected && has_session)
                        .then(|| user.clone())
                        .flatten()
                }),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: true,
            },
            remote_invoke_registered: cloud_connected,
        },
        SyncProviderStatus {
            id: "github_gist".to_string(),
            name: "GitHub Gist".to_string(),
            description: "Public GitHub Gist-backed portable sync provider.".to_string(),
            remote_base_url: Some(GITHUB_GIST_REMOTE_BASE_URL.to_string()),
            connected: github_gist_connected,
            enabled: sync_config.enabled && github_gist_connected,
            reachable: github_gist_connected,
            authorized: github_gist_authorized,
            reason: if github_gist_last_error.is_some() {
                SyncReason::Error
            } else if github_gist_connected {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: github_gist_last_error,
            last_sync_at: github_gist_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: github_gist_meta.and_then(|meta| meta.last_sync_action),
            user: github_gist_session.and_then(|session| session.user.clone()),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: false,
            },
            remote_invoke_registered: false,
        },
    ]
}

fn github_fallback_user() -> RemoteUser {
    RemoteUser {
        user_id: "github".to_string(),
        nickname: "GitHub".to_string(),
        avatar: String::new(),
        email: String::new(),
    }
}

fn gist_remote_id_for_rule(rule: &RuleFile) -> String {
    rule.sync
        .remote_id
        .clone()
        .filter(|remote_id| remote_id.starts_with("gist:"))
        .unwrap_or_else(|| format!("gist:{}", rule.sync.rule_id))
}

fn is_github_gist_remote_id(remote_id: &str) -> bool {
    remote_id.starts_with("gist:")
}

fn server_remote_id_for_rule(rule: &RuleFile) -> Option<&str> {
    rule.sync
        .remote_id
        .as_deref()
        .filter(|remote_id| !is_github_gist_remote_id(remote_id))
}

fn github_gist_basic_config_meta_key(key: &str) -> String {
    format!("{GITHUB_GIST_PROVIDER_ID}:{key}")
}

fn gist_rule_to_remote_env(rule: GitHubGistRule) -> RemoteEnv {
    RemoteEnv {
        id: rule.id,
        user_id: rule.user_id,
        name: rule.name,
        rule: rule.rule,
        create_time: rule.create_time,
        update_time: rule.update_time,
    }
}

fn gist_config_to_remote(config: &GitHubGistBasicConfig) -> RemoteBasicConfig {
    RemoteBasicConfig {
        id: config.id.clone(),
        user_id: config.user_id.clone(),
        config_key: config.config_key.clone(),
        value_json: config.value_json.clone(),
        hash: config.hash.clone(),
        create_time: config.create_time.clone(),
        update_time: config.update_time.clone(),
    }
}

fn upsert_gist_rule(
    snapshot: &mut GitHubGistSnapshot,
    local_rule: &RuleFile,
    remote_env: Option<&RemoteEnv>,
    user: &RemoteUser,
    now: &str,
) {
    let remote_id = remote_env
        .map(|env| env.id.clone())
        .unwrap_or_else(|| gist_remote_id_for_rule(local_rule));
    let create_time = remote_env
        .map(|env| env.create_time.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| now.to_string());
    let item = GitHubGistRule {
        id: remote_id.clone(),
        user_id: user.user_id.clone(),
        name: local_rule.name.clone(),
        rule: local_rule.content.clone(),
        create_time,
        update_time: now.to_string(),
    };
    if let Some(existing) = snapshot.rules.iter_mut().find(|rule| rule.id == remote_id) {
        *existing = item;
    } else {
        snapshot.rules.push(item);
    }
}

#[derive(Clone)]
pub struct SyncManagerHandle {
    inner: SharedSyncManager,
}

impl SyncManagerHandle {
    pub fn new(inner: SharedSyncManager) -> Self {
        Self { inner }
    }

    pub async fn status(&self) -> SyncStatus {
        self.inner.status().await
    }

    pub async fn save_token(&self, token: String) -> Result<SyncStatus> {
        self.inner.save_token(token).await?;
        Ok(self.inner.status().await)
    }

    pub async fn save_github_gist_session(&self, token: String) -> Result<SyncStatus> {
        self.inner.save_github_gist_session(token).await?;
        Ok(self.inner.status().await)
    }

    pub async fn logout(&self) -> Result<SyncStatus> {
        self.inner.logout().await?;
        Ok(self.inner.status().await)
    }

    pub fn trigger_sync(&self) {
        self.inner.trigger_sync();
    }

    pub async fn login_url(&self, callback_url: &str) -> Result<String> {
        self.inner.login_url(callback_url).await
    }

    pub async fn remote_sample(&self, limit: usize) -> Result<Vec<RemoteEnv>> {
        self.inner.remote_sample(limit).await
    }

    pub fn session_token(&self) -> Option<String> {
        self.inner.session_token()
    }

    pub fn session_token_for_remote(&self, remote_base_url: &str) -> Option<String> {
        self.inner.session_token_for_remote(remote_base_url)
    }

    pub async fn remote_invoke_registration_targets(&self) -> Vec<RemoteInvokeProviderTarget> {
        self.inner.remote_invoke_registration_targets().await
    }

    pub async fn record_deleted_rule(&self, rule: &RuleFile) -> Result<()> {
        self.inner.record_deleted_rule(rule).await
    }

    pub async fn clear_deleted_rule(&self, rule_name: &str) -> Result<()> {
        self.inner.clear_deleted_rule(rule_name).await
    }

    pub async fn proxy_forward(
        &self,
        method: reqwest::Method,
        path: &str,
        query: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, String, Vec<u8>)> {
        self.inner.proxy_forward(method, path, query, body).await
    }
}

pub struct SyncManager {
    config_manager: Arc<ConfigManager>,
    local_callback_url: String,
    state_file: PathBuf,
    state: Mutex<SyncStateFile>,
    sync_lock: AsyncMutex<()>,
    login_prompt: Mutex<LoginPromptState>,
    runtime: RwLock<SyncRuntimeState>,
    wake: Notify,
}

impl SyncManager {
    pub fn new(config_manager: Arc<ConfigManager>, admin_port: u16) -> Result<Self> {
        let state_file = config_manager.data_dir().join("sync-state.json");
        let state = if state_file.exists() {
            const MAX_STATE_FILE_BYTES: u64 = 256 * 1024 * 1024;
            if let Ok(meta) = std::fs::metadata(&state_file) {
                if meta.len() > MAX_STATE_FILE_BYTES {
                    return Err(BifrostError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("sync state file too large ({} bytes)", meta.len()),
                    )));
                }
            }
            let content = fs::read_to_string(&state_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            SyncStateFile::default()
        };
        Ok(Self {
            config_manager,
            local_callback_url: format!("http://127.0.0.1:{admin_port}/login.html"),
            state_file,
            state: Mutex::new(state),
            sync_lock: AsyncMutex::new(()),
            login_prompt: Mutex::new(LoginPromptState::default()),
            runtime: RwLock::new(SyncRuntimeState::default()),
            wake: Notify::new(),
        })
    }

    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }

    pub fn trigger_sync(&self) {
        self.wake.notify_one();
    }

    pub async fn status(&self) -> SyncStatus {
        let config = self.config_manager.config().await;
        let runtime = self.runtime.read().await.clone();
        let state = self.state.lock().clone();
        let server_has_session = state
            .token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty());
        let has_any_session = server_has_session
            || state
                .provider_sessions
                .values()
                .any(|session| !session.token.trim().is_empty());
        let providers = build_provider_statuses(
            &config.sync,
            &runtime,
            &state.user,
            server_has_session,
            &state.provider_sessions,
            &state.provider_sync,
        );
        let has_ready_provider = providers
            .iter()
            .any(|provider| provider.connected && provider.reachable && provider.authorized);
        let provider_last_sync_at = providers
            .iter()
            .filter(|provider| provider.connected)
            .filter_map(|provider| provider.last_sync_at.clone())
            .max();
        let provider_last_sync_action = provider_last_sync_at.as_ref().and_then(|last_at| {
            providers
                .iter()
                .find(|provider| provider.last_sync_at.as_ref() == Some(last_at))
                .and_then(|provider| provider.last_sync_action)
        });
        SyncStatus {
            enabled: config.sync.enabled,
            auto_sync: config.sync.auto_sync,
            remote_base_url: config.sync.remote_base_url,
            has_session: has_any_session,
            reachable: runtime.reachable || has_ready_provider,
            authorized: runtime.authorized || has_ready_provider,
            syncing: runtime.syncing,
            reason: if has_ready_provider
                && matches!(
                    runtime.reason,
                    SyncReason::Unreachable | SyncReason::Unauthorized
                ) {
                SyncReason::Ready
            } else {
                runtime.reason
            },
            last_sync_at: provider_last_sync_at.or(state.last_sync_at),
            last_sync_action: provider_last_sync_action.or(state.last_sync_action),
            last_error: runtime.last_error,
            user: state.user,
            first_run_prompt_required: !providers.iter().any(|provider| provider.connected),
            providers,
        }
    }

    pub fn current_user_id(&self) -> Option<String> {
        let state = self.state.lock();
        state.user.as_ref().map(|u| u.user_id.clone())
    }

    pub fn has_session(&self) -> bool {
        let state = self.state.lock();
        state.token.as_deref().is_some_and(|t| !t.trim().is_empty())
    }

    pub fn session_token(&self) -> Option<String> {
        self.state.lock().token.clone()
    }

    pub fn session_token_for_remote(&self, remote_base_url: &str) -> Option<String> {
        let normalized = normalize_remote_base_url(remote_base_url);
        let state = self.state.lock();
        state
            .provider_sessions
            .values()
            .find(|session| normalize_remote_base_url(&session.remote_base_url) == normalized)
            .map(|session| session.token.clone())
            .filter(|token| !token.trim().is_empty())
            .or_else(|| {
                self.config_manager.try_config().and_then(|config| {
                    (normalize_remote_base_url(&config.sync.remote_base_url) == normalized)
                        .then(|| state.token.clone())
                        .flatten()
                })
            })
    }

    pub async fn remote_invoke_registration_targets(&self) -> Vec<RemoteInvokeProviderTarget> {
        let config = self.config_manager.config().await;
        let state = self.state.lock().clone();
        let mut targets = Vec::new();
        for provider_id in ["bytedance_internal", "bifrost_cloud"] {
            if let Some(session) = session_for_provider(&state.provider_sessions, provider_id) {
                targets.push(RemoteInvokeProviderTarget {
                    provider_id: provider_id.to_string(),
                    remote_base_url: session.remote_base_url.clone(),
                    session_token: session.token.clone(),
                });
            }
        }
        if targets.is_empty() {
            if let Some(provider_id) = provider_id_for_remote(&config.sync.remote_base_url) {
                if let Some(token) = state.token.clone().filter(|token| !token.trim().is_empty()) {
                    targets.push(RemoteInvokeProviderTarget {
                        provider_id: provider_id.to_string(),
                        remote_base_url: normalize_remote_base_url(&config.sync.remote_base_url),
                        session_token: token,
                    });
                }
            }
        }
        targets
    }

    pub async fn login_url(&self, callback_url: &str) -> Result<String> {
        let config = self.config_manager.config().await;
        let client = SyncHttpClient::new(&config.sync)?;
        Ok(client.login_url(&config.sync, callback_url))
    }

    pub async fn request_login(&self) -> Result<()> {
        let config = self.config_manager.config().await;
        self.open_login_browser(&config.sync, true).await?;
        Ok(())
    }

    pub async fn save_token(&self, token: String) -> Result<()> {
        let config = self.config_manager.config().await;
        self.config_manager
            .update_sync_config(SyncConfigUpdate {
                auto_sync: Some(true),
                ..Default::default()
            })
            .await?;
        {
            let mut state = self.state.lock();
            let token = token.trim().to_string();
            let user = state.user.clone();
            state.token = Some(token.clone());
            if let Some(provider_id) = provider_id_for_remote(&config.sync.remote_base_url) {
                state.provider_sessions.insert(
                    provider_id.to_string(),
                    SyncProviderSession {
                        token,
                        remote_base_url: normalize_remote_base_url(&config.sync.remote_base_url),
                        user,
                    },
                );
            }
            self.persist_state(&state)?;
        }
        self.wake.notify_one();
        Ok(())
    }

    pub async fn save_login_session(&self, token: String, remote_base_url: String) -> Result<()> {
        let token = token.trim().to_string();
        let remote_base_url = remote_base_url.trim().trim_end_matches('/').to_string();
        if token.is_empty() {
            return Err(BifrostError::Config("token is required".to_string()));
        }
        if remote_base_url.is_empty() {
            return Err(BifrostError::Config(
                "remote_base_url is required".to_string(),
            ));
        }
        if !remote_base_url.starts_with("https://") {
            return Err(BifrostError::Config(
                "remote_base_url must start with https://".to_string(),
            ));
        }

        self.config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(true),
                remote_base_url: Some(remote_base_url),
                ..Default::default()
            })
            .await?;
        self.save_token(token).await
    }

    pub async fn save_github_gist_session(&self, token: String) -> Result<()> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(BifrostError::Config("GitHub token is required".to_string()));
        }
        let user = self.github_user_for_token(&token).await?;
        self.config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(true),
                ..Default::default()
            })
            .await?;
        {
            let mut state = self.state.lock();
            state.provider_sessions.insert(
                GITHUB_GIST_PROVIDER_ID.to_string(),
                SyncProviderSession {
                    token,
                    remote_base_url: GITHUB_GIST_REMOTE_BASE_URL.to_string(),
                    user: Some(user),
                },
            );
            self.persist_state(&state)?;
        }
        self.wake.notify_one();
        Ok(())
    }

    fn github_gist_session(&self) -> Option<SyncProviderSession> {
        self.state
            .lock()
            .provider_sessions
            .get(GITHUB_GIST_PROVIDER_ID)
            .cloned()
            .filter(|session| !session.token.trim().is_empty())
    }

    async fn sync_github_gist_once(
        &self,
        token: &str,
        cached_user: Option<RemoteUser>,
    ) -> Result<SyncOnceResult> {
        let user = match cached_user {
            Some(user) if !user.user_id.trim().is_empty() => user,
            _ => self.github_user_for_token(token).await?,
        };
        {
            let mut state = self.state.lock();
            if let Some(session) = state.provider_sessions.get_mut(GITHUB_GIST_PROVIDER_ID) {
                session.user = Some(user.clone());
            }
            self.persist_state(&state)?;
        }

        let rules_storage = self.config_manager.rules_storage().await;
        let local_count = rules_storage
            .load_all()?
            .into_iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .count();
        let result = self.sync_github_gist_provider(token, &user).await;
        let state = self.state.lock().clone();
        let synced_count = rules_storage
            .load_all()?
            .iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .filter(|rule| {
                rule.sync
                    .remote_id
                    .as_deref()
                    .is_some_and(|remote_id| remote_id.starts_with("gist:"))
            })
            .count();

        match result {
            Ok(()) => Ok(SyncOnceResult {
                success: true,
                message: "GitHub Gist sync completed successfully".to_string(),
                action: state
                    .provider_sync
                    .get(GITHUB_GIST_PROVIDER_ID)
                    .and_then(|meta| meta.last_sync_action)
                    .or(state.last_sync_action),
                user: Some(user),
                local_rules: local_count,
                remote_rules: synced_count,
            }),
            Err(error) => {
                let mut state = self.state.lock();
                record_provider_sync_error(&mut state, GITHUB_GIST_PROVIDER_ID, error.to_string());
                self.persist_state(&state)?;
                Ok(SyncOnceResult {
                    success: false,
                    message: format!("GitHub Gist sync failed: {error}"),
                    action: None,
                    user: Some(user),
                    local_rules: local_count,
                    remote_rules: 0,
                })
            }
        }
    }

    async fn sync_github_gist_provider(&self, token: &str, user: &RemoteUser) -> Result<()> {
        let client = GitHubGistClient::new()?;
        let mut remote = client.load_snapshot(token).await?;
        let now = Utc::now().to_rfc3339();
        remote.snapshot.version = 1;
        remote.snapshot.user_id = user.user_id.clone();

        let mut outcome = self
            .sync_github_gist_rules(&mut remote.snapshot, user, &now)
            .await?;
        let config_outcome = self
            .sync_github_gist_basic_configs(&mut remote.snapshot, user, &now)
            .await?;
        outcome.0 |= config_outcome.0;
        outcome.1 |= config_outcome.1;
        if outcome.0 {
            remote.snapshot.updated_at = now.clone();
            let gist_id = client.save_snapshot(token, &remote).await?;
            if remote.gist_id.as_deref() != Some(gist_id.as_str()) {
                tracing::info!(
                    target: "bifrost_sync::manager",
                    gist_id = %gist_id,
                    "GitHub Gist sync snapshot created"
                );
            }
        }

        {
            let mut state = self.state.lock();
            let sync_at = Utc::now().to_rfc3339();
            let sync_action = match outcome {
                (true, true) => SyncAction::Bidirectional,
                (true, false) => SyncAction::LocalPushed,
                (false, true) => SyncAction::RemotePulled,
                (false, false) => SyncAction::NoChange,
            };
            state.last_sync_at = Some(sync_at.clone());
            state.last_sync_action = Some(sync_action);
            record_provider_sync_success(
                &mut state,
                GITHUB_GIST_PROVIDER_ID,
                sync_at,
                Some(sync_action),
            );
            self.persist_state(&state)?;
        }
        Ok(())
    }

    async fn mirror_github_gist_provider(&self, token: &str, user: &RemoteUser) -> Result<()> {
        let client = GitHubGistClient::new()?;
        let mut remote = client.load_snapshot(token).await?;
        let now = Utc::now().to_rfc3339();
        remote.snapshot.version = 1;
        remote.snapshot.user_id = user.user_id.clone();

        let rules_changed = self
            .mirror_github_gist_rules(&mut remote.snapshot, user, &now)
            .await?;
        let configs_changed = self
            .mirror_github_gist_basic_configs(&mut remote.snapshot, user, &now)
            .await?;
        if rules_changed || configs_changed {
            remote.snapshot.updated_at = now.clone();
            client.save_snapshot(token, &remote).await?;
        }
        let mut state = self.state.lock();
        let sync_at = Utc::now().to_rfc3339();
        state.last_sync_at = Some(sync_at.clone());
        record_provider_sync_success(
            &mut state,
            GITHUB_GIST_PROVIDER_ID,
            sync_at,
            Some(SyncAction::NoChange),
        );
        self.persist_state(&state)?;
        Ok(())
    }

    async fn mirror_github_gist_rules(
        &self,
        snapshot: &mut GitHubGistSnapshot,
        user: &RemoteUser,
        now: &str,
    ) -> Result<bool> {
        let rules_storage = self.config_manager.rules_storage().await;
        let local_rules = rules_storage
            .load_all()?
            .into_iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .collect::<Vec<_>>();
        let existing_by_name: HashMap<&str, &GitHubGistRule> = snapshot
            .rules
            .iter()
            .map(|rule| (rule.name.as_str(), rule))
            .collect();
        let mut next_rules = Vec::new();
        for local_rule in local_rules {
            let existing = existing_by_name.get(local_rule.name.as_str()).copied();
            let id = existing
                .map(|rule| rule.id.clone())
                .unwrap_or_else(|| format!("gist:{}", local_rule.sync.rule_id));
            let create_time = existing
                .map(|rule| rule.create_time.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| now.to_string());
            next_rules.push(GitHubGistRule {
                id,
                user_id: user.user_id.clone(),
                name: local_rule.name,
                rule: local_rule.content,
                create_time,
                update_time: now.to_string(),
            });
        }
        next_rules.sort_by(|left, right| left.name.cmp(&right.name));
        let changed = serde_json::to_value(&snapshot.rules)
            .ok()
            .zip(serde_json::to_value(&next_rules).ok())
            .is_none_or(|(current, next)| current != next);
        if changed {
            snapshot.rules = next_rules;
        }
        Ok(changed)
    }

    async fn mirror_github_gist_basic_configs(
        &self,
        snapshot: &mut GitHubGistSnapshot,
        user: &RemoteUser,
        now: &str,
    ) -> Result<bool> {
        let configs = self.current_basic_config_payloads().await?;
        let mut changed = false;
        for (key, value_json, hash) in configs {
            let should_update = snapshot
                .basic_configs
                .get(key)
                .is_none_or(|remote| remote.hash != hash || remote.value_json != value_json);
            if should_update {
                snapshot.basic_configs.insert(
                    key.to_string(),
                    GitHubGistBasicConfig {
                        id: key.to_string(),
                        user_id: user.user_id.clone(),
                        config_key: key.to_string(),
                        value_json,
                        hash,
                        create_time: snapshot
                            .basic_configs
                            .get(key)
                            .map(|item| item.create_time.clone())
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| now.to_string()),
                        update_time: now.to_string(),
                    },
                );
                changed = true;
            }
        }
        Ok(changed)
    }

    async fn sync_github_gist_rules(
        &self,
        snapshot: &mut GitHubGistSnapshot,
        user: &RemoteUser,
        now: &str,
    ) -> Result<(bool, bool)> {
        let rules_storage = self.config_manager.rules_storage().await;
        let local_rules = rules_storage
            .load_all()?
            .into_iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .collect::<Vec<_>>();
        let remote_rules = snapshot
            .rules
            .iter()
            .cloned()
            .map(gist_rule_to_remote_env)
            .collect::<Vec<_>>();
        let remote_by_id: HashMap<&str, &RemoteEnv> = remote_rules
            .iter()
            .map(|env| (env.id.as_str(), env))
            .collect();
        let remote_name_counts: HashMap<&str, usize> =
            remote_rules.iter().fold(HashMap::new(), |mut counts, env| {
                *counts.entry(env.name.as_str()).or_default() += 1;
                counts
            });
        let remote_by_unique_name: HashMap<&str, &RemoteEnv> = remote_rules
            .iter()
            .filter(|env| remote_name_counts.get(env.name.as_str()) == Some(&1))
            .map(|env| (env.name.as_str(), env))
            .collect();

        let state_snapshot = self.state.lock().clone();
        let mut pushed = false;
        let mut pulled = false;
        let mut local_storage_changed = false;
        let mut consumed_remote_ids = HashSet::new();

        for tombstone in state_snapshot.deleted_rules.values() {
            if rules_storage.exists(&tombstone.rule_name) {
                rules_storage.delete(&tombstone.rule_name)?;
                local_storage_changed = true;
                pulled = true;
            }
            let before = snapshot.rules.len();
            snapshot
                .rules
                .retain(|rule| rule.id != tombstone.remote_id && rule.name != tombstone.rule_name);
            if snapshot.rules.len() != before {
                pushed = true;
            }
        }

        for local_rule in &local_rules {
            let remote_env = local_rule
                .sync
                .remote_id
                .as_deref()
                .and_then(|remote_id| remote_by_id.get(remote_id).copied())
                .or_else(|| {
                    if local_rule
                        .sync
                        .remote_id
                        .as_deref()
                        .is_some_and(|remote_id| remote_id.starts_with("gist:"))
                    {
                        None
                    } else {
                        remote_by_unique_name.get(local_rule.name.as_str()).copied()
                    }
                });

            if let Some(remote_env) = remote_env {
                consumed_remote_ids.insert(remote_env.id.clone());
                match local_rule.sync.status {
                    RuleSyncStatus::Modified | RuleSyncStatus::LocalOnly => {
                        upsert_gist_rule(snapshot, local_rule, Some(remote_env), user, now);
                        let mut synced_rule = local_rule.clone();
                        synced_rule.mark_synced(
                            remote_env.id.clone(),
                            user.user_id.clone(),
                            remote_env.create_time.clone(),
                            now.to_string(),
                        );
                        rules_storage.save(&synced_rule)?;
                        pushed = true;
                        local_storage_changed = true;
                    }
                    RuleSyncStatus::Synced => {
                        let remote_hash = content_hash(&remote_env.rule);
                        let remote_changed = local_rule.sync.remote_updated_at.as_deref()
                            != Some(remote_env.update_time.as_str())
                            || local_rule.sync.last_synced_content_hash.as_deref()
                                != Some(remote_hash.as_str());
                        if remote_changed {
                            let mut updated = local_rule.clone();
                            updated.content = remote_env.rule.clone();
                            updated.updated_at = remote_env.update_time.clone();
                            updated.mark_synced(
                                remote_env.id.clone(),
                                remote_env.user_id.clone(),
                                remote_env.create_time.clone(),
                                remote_env.update_time.clone(),
                            );
                            rules_storage.save(&updated)?;
                            pulled = true;
                            local_storage_changed = true;
                        }
                    }
                }
            } else if local_rule
                .sync
                .remote_id
                .as_deref()
                .is_some_and(|remote_id| remote_id.starts_with("gist:"))
                && local_rule.sync.status == RuleSyncStatus::Synced
            {
                rules_storage.delete(&local_rule.name)?;
                pulled = true;
                local_storage_changed = true;
            } else {
                let remote_id = gist_remote_id_for_rule(local_rule);
                upsert_gist_rule(snapshot, local_rule, None, user, now);
                let mut synced_rule = local_rule.clone();
                synced_rule.mark_synced(
                    remote_id,
                    user.user_id.clone(),
                    now.to_string(),
                    now.to_string(),
                );
                rules_storage.save(&synced_rule)?;
                pushed = true;
                local_storage_changed = true;
            }
        }

        for remote_env in &remote_rules {
            if consumed_remote_ids.contains(&remote_env.id) {
                continue;
            }
            if state_snapshot.deleted_rules.values().any(|tombstone| {
                tombstone.remote_id == remote_env.id || tombstone.rule_name == remote_env.name
            }) {
                continue;
            }
            let mut remote_rule = RuleFile::new(&remote_env.name, &remote_env.rule);
            remote_rule.enabled = false;
            remote_rule.description = Some("Synced from GitHub Gist".to_string());
            remote_rule.created_at = remote_env.create_time.clone();
            remote_rule.updated_at = remote_env.update_time.clone();
            remote_rule.mark_synced(
                remote_env.id.clone(),
                remote_env.user_id.clone(),
                remote_env.create_time.clone(),
                remote_env.update_time.clone(),
            );
            rules_storage.save(&remote_rule)?;
            pulled = true;
            local_storage_changed = true;
        }

        if local_storage_changed {
            let _ = self.config_manager.notify(ConfigChangeEvent::rules_changed(
                RulesChangeOrigin::RemoteSync,
            ));
        }
        Ok((pushed, pulled))
    }

    async fn sync_github_gist_basic_configs(
        &self,
        snapshot: &mut GitHubGistSnapshot,
        user: &RemoteUser,
        now: &str,
    ) -> Result<(bool, bool)> {
        let snapshots = self.current_basic_config_payloads().await?;
        let state_snapshot = self.state.lock().clone();
        let mut pushed = false;
        let mut pulled = false;
        let mut meta_updates = Vec::new();
        for (key, value_json, hash) in snapshots {
            let meta_key = github_gist_basic_config_meta_key(key);
            let local_changed = state_snapshot
                .basic_configs
                .get(&meta_key)
                .and_then(|meta| meta.hash.as_deref())
                != Some(hash.as_str());
            if local_changed {
                snapshot.basic_configs.insert(
                    key.to_string(),
                    GitHubGistBasicConfig {
                        id: key.to_string(),
                        user_id: user.user_id.clone(),
                        config_key: key.to_string(),
                        value_json,
                        hash: hash.clone(),
                        create_time: snapshot
                            .basic_configs
                            .get(key)
                            .map(|item| item.create_time.clone())
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| now.to_string()),
                        update_time: now.to_string(),
                    },
                );
                meta_updates.push((
                    meta_key,
                    BasicConfigSyncMeta {
                        remote_updated_at: Some(now.to_string()),
                        hash: Some(hash),
                    },
                ));
                pushed = true;
            } else if let Some(remote) = snapshot.basic_configs.get(key) {
                let meta_key = github_gist_basic_config_meta_key(key);
                let known_hash = state_snapshot
                    .basic_configs
                    .get(&meta_key)
                    .and_then(|meta| meta.hash.as_deref());
                if known_hash != Some(remote.hash.as_str())
                    && self
                        .apply_remote_basic_config(&gist_config_to_remote(remote))
                        .await?
                {
                    meta_updates.push((
                        meta_key,
                        BasicConfigSyncMeta {
                            remote_updated_at: Some(remote.update_time.clone()),
                            hash: Some(remote.hash.clone()),
                        },
                    ));
                    pulled = true;
                }
            }
        }
        if !meta_updates.is_empty() {
            let mut state = self.state.lock();
            for (key, meta) in meta_updates {
                state.basic_configs.insert(key, meta);
            }
            self.persist_state(&state)?;
        }
        Ok((pushed, pulled))
    }

    async fn current_basic_config_payloads(&self) -> Result<[(&'static str, String, String); 3]> {
        let unified = self.config_manager.config().await;
        let app_allowlist = serde_json::to_string(&unified.tls.app_intercept_include)
            .map_err(|e| BifrostError::Config(format!("encode app allowlist: {e}")))?;
        let domain_allowlist = serde_json::to_string(&unified.tls.intercept_include)
            .map_err(|e| BifrostError::Config(format!("encode domain allowlist: {e}")))?;
        let blacklist = serde_json::to_string(&unified.tls.intercept_exclude)
            .map_err(|e| BifrostError::Config(format!("encode blacklist: {e}")))?;
        Ok([
            (
                "app_allowlist",
                app_allowlist.clone(),
                content_hash(&app_allowlist),
            ),
            (
                "domain_allowlist",
                domain_allowlist.clone(),
                content_hash(&domain_allowlist),
            ),
            ("blacklist", blacklist.clone(), content_hash(&blacklist)),
        ])
    }

    async fn github_user_for_token(&self, token: &str) -> Result<RemoteUser> {
        let client = bifrost_core::outbound_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| BifrostError::Network(format!("failed to build GitHub client: {e}")))?;
        let response = client
            .get(format!("{GITHUB_API_BASE_URL}/user"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "bifrost-sync")
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("GitHub user request failed: {e}")))?;

        if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
            return Err(BifrostError::Config(
                "GitHub token is invalid or does not have access".to_string(),
            ));
        }
        if !response.status().is_success() {
            return Err(BifrostError::Network(format!(
                "GitHub user request returned HTTP {}",
                response.status()
            )));
        }
        let user = response
            .json::<GitHubUser>()
            .await
            .map_err(|e| BifrostError::Network(format!("invalid GitHub user response: {e}")))?;
        Ok(RemoteUser {
            user_id: if user.id == 0 {
                user.login.clone()
            } else {
                format!("github:{}", user.id)
            },
            nickname: user.name.unwrap_or_else(|| user.login.clone()),
            avatar: user.avatar_url.unwrap_or_default(),
            email: user.email.unwrap_or_default(),
        })
    }

    pub async fn remote_sample(&self, limit: usize) -> Result<Vec<RemoteEnv>> {
        let config = self.config_manager.config().await;
        let token = self
            .state
            .lock()
            .token
            .clone()
            .ok_or_else(|| BifrostError::Config("sync session token missing".to_string()))?;
        let user = self
            .state
            .lock()
            .user
            .clone()
            .ok_or_else(|| BifrostError::Config("sync user missing".to_string()))?;
        let client = SyncHttpClient::new(&config.sync)?;
        let mut envs = client
            .search_envs(&config.sync, &token, &user.user_id)
            .await?;
        envs.sort_by(|a, b| b.update_time.cmp(&a.update_time));
        envs.truncate(limit.max(1));
        Ok(envs)
    }

    pub async fn record_deleted_rule(&self, rule: &RuleFile) -> Result<()> {
        let Some(remote_id) = rule.sync.remote_id.clone() else {
            return Ok(());
        };
        let remote_user_id = rule.sync.remote_user_id.clone().ok_or_else(|| {
            BifrostError::Config(format!(
                "rule '{}' is missing remote_user_id in sync metadata",
                rule.name
            ))
        })?;

        {
            let mut state = self.state.lock();
            state.deleted_rules.insert(
                rule.sync.rule_id.clone(),
                DeletedRuleTombstone {
                    rule_id: rule.sync.rule_id.clone(),
                    rule_name: rule.name.clone(),
                    remote_id,
                    remote_user_id,
                    base_remote_updated_at: rule.sync.remote_updated_at.clone(),
                    base_content_hash: rule.sync.last_synced_content_hash.clone(),
                    deleted_at: Utc::now().to_rfc3339(),
                },
            );
            tracing::info!(
                target: "bifrost_sync::manager",
                name = %rule.name,
                rule_id = %rule.sync.rule_id,
                deleted_rules = state.deleted_rules.len(),
                "recorded delete tombstone"
            );
            self.persist_state(&state)?;
        }

        Ok(())
    }

    pub async fn clear_deleted_rule(&self, rule_name: &str) -> Result<()> {
        let mut state = self.state.lock();
        let before = state.deleted_rules.len();
        state
            .deleted_rules
            .retain(|_, tombstone| tombstone.rule_name != rule_name);
        if state.deleted_rules.len() != before {
            self.persist_state(&state)?;
        }
        Ok(())
    }

    pub async fn proxy_forward(
        &self,
        method: reqwest::Method,
        path: &str,
        query: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, String, Vec<u8>)> {
        let config = self.config_manager.config().await;
        let token = self
            .state
            .lock()
            .token
            .clone()
            .ok_or_else(|| BifrostError::Config("sync session token missing".to_string()))?;
        let client = SyncHttpClient::new(&config.sync)?;
        client
            .proxy_forward(&config.sync, &token, method, path, query, body)
            .await
    }

    pub async fn logout(&self) -> Result<()> {
        let config = self.config_manager.config().await;
        let token = { self.state.lock().token.clone() };
        if let Some(token) = token {
            let client = SyncHttpClient::new(&config.sync)?;
            let _ = client.logout(&config.sync, &token).await;
        }
        {
            let mut state = self.state.lock();
            state.token = None;
            state.user = None;
            state.provider_sessions.clear();
            self.persist_state(&state)?;
        }
        self.login_prompt.lock().last_opened_at = None;
        {
            let mut runtime = self.runtime.write().await;
            runtime.authorized = false;
            runtime.reason = SyncReason::Unauthorized;
            runtime.last_error = None;
        }
        Ok(())
    }

    pub async fn sync_once(&self) -> Result<SyncOnceResult> {
        let _sync_guard = self.sync_lock.lock().await;
        let config = self.config_manager.config().await;
        if !config.sync.enabled {
            return Ok(SyncOnceResult {
                success: false,
                message: "Sync is disabled in configuration".to_string(),
                action: None,
                user: None,
                local_rules: 0,
                remote_rules: 0,
            });
        }

        let state_snapshot = self.state.lock().clone();
        let mut server_candidates = Vec::new();
        if let Some(token) = state_snapshot
            .token
            .clone()
            .filter(|token| !token.trim().is_empty())
        {
            server_candidates.push((config.sync.clone(), token));
        }
        for provider_id in ["bytedance_internal", "bifrost_cloud"] {
            let Some(session) =
                session_for_provider(&state_snapshot.provider_sessions, provider_id)
            else {
                continue;
            };
            let normalized = normalize_remote_base_url(&session.remote_base_url);
            if server_candidates.iter().any(|(candidate, _)| {
                normalize_remote_base_url(&candidate.remote_base_url) == normalized
            }) {
                continue;
            }
            let mut sync_config = config.sync.clone();
            sync_config.remote_base_url = normalized;
            server_candidates.push((sync_config, session.token.clone()));
        }
        if server_candidates.is_empty() && self.github_gist_session().is_none() {
            let client = SyncHttpClient::new(&config.sync)?;
            if !client.probe_reachable(&config.sync).await {
                return Ok(SyncOnceResult {
                    success: false,
                    message: format!("Remote server unreachable: {}", config.sync.remote_base_url),
                    action: None,
                    user: None,
                    local_rules: 0,
                    remote_rules: 0,
                });
            }
        }

        let mut last_server_failure: Option<String> = None;
        for (sync_config, token) in server_candidates {
            let Some(provider_id) = provider_id_for_remote(&sync_config.remote_base_url) else {
                continue;
            };
            let client = SyncHttpClient::new(&sync_config)?;
            let reachable = client.probe_reachable(&sync_config).await;
            if !reachable {
                let message = format!("Remote server unreachable: {}", sync_config.remote_base_url);
                let mut state = self.state.lock();
                record_provider_sync_error(&mut state, provider_id, message.clone());
                self.persist_state(&state)?;
                last_server_failure = Some(message);
                continue;
            }

            let user = client.get_user_info(&sync_config, &token).await?;
            let Some(user) = user else {
                let message = "Token expired or invalid. Please re-login via the admin UI.";
                let mut state = self.state.lock();
                record_provider_sync_error(&mut state, provider_id, message.to_string());
                self.persist_state(&state)?;
                last_server_failure = Some(message.to_string());
                continue;
            };

            {
                let mut state = self.state.lock();
                state.user = Some(user.clone());
                update_provider_session_user(&sync_config, &mut state, &user);
                self.persist_state(&state)?;
            }

            let rules_storage = self.config_manager.rules_storage().await;
            let local_count = rules_storage
                .load_all()?
                .into_iter()
                .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
                .count();

            let result = match self.sync_rules(&client, &sync_config, &token, &user).await {
                Ok(()) => {
                    self.sync_basic_configs(&client, &sync_config, &token, &user)
                        .await
                }
                Err(error) => Err(error),
            };
            if result.is_ok() {
                if let Some(session) = self.github_gist_session() {
                    let gist_user = session.user.unwrap_or_else(|| user.clone());
                    if let Err(error) = self
                        .mirror_github_gist_provider(&session.token, &gist_user)
                        .await
                    {
                        let mut state = self.state.lock();
                        record_provider_sync_error(
                            &mut state,
                            GITHUB_GIST_PROVIDER_ID,
                            error.to_string(),
                        );
                        self.persist_state(&state)?;
                    }
                }
            }
            let state = self.state.lock().clone();
            let synced_count = rules_storage
                .load_all()?
                .iter()
                .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
                .filter(|rule| rule.sync.remote_id.is_some())
                .count();

            match result {
                Ok(()) => {
                    return Ok(SyncOnceResult {
                        success: true,
                        message: "Sync completed successfully".to_string(),
                        action: state.last_sync_action,
                        user: Some(user),
                        local_rules: local_count,
                        remote_rules: synced_count,
                    });
                }
                Err(error) => {
                    let message = format!("Sync failed: {error}");
                    let mut state = self.state.lock();
                    record_provider_sync_error(&mut state, provider_id, error.to_string());
                    self.persist_state(&state)?;
                    last_server_failure = Some(message.clone());
                }
            }
        }

        if let Some(session) = self.github_gist_session() {
            return self
                .sync_github_gist_once(&session.token, session.user.clone())
                .await;
        }

        Ok(SyncOnceResult {
            success: false,
            message: last_server_failure.unwrap_or_else(|| {
                "No sync session token. Please login first via the admin UI.".to_string()
            }),
            action: None,
            user: None,
            local_rules: 0,
            remote_rules: 0,
        })
    }

    async fn run(self: &Arc<Self>) {
        let mut receiver = self.config_manager.subscribe();
        if let Err(error) = self.startup_login_preflight().await {
            tracing::warn!(
                target: "bifrost_sync::manager",
                error = %error,
                "sync startup login preflight failed"
            );
        }
        loop {
            let config = self.config_manager.config().await;
            let interval = Duration::from_secs(config.sync.probe_interval_secs.max(2));
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = self.wake.notified() => {}
                event = receiver.recv() => {
                    match event {
                        Ok(ConfigChangeEvent::RulesChanged(origin))
                            if origin.should_wake_sync() => {}
                        Ok(ConfigChangeEvent::SyncConfigChanged) => {}
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
            if let Err(error) = self.tick().await {
                let mut runtime = self.runtime.write().await;
                runtime.syncing = false;
                runtime.reason = SyncReason::Error;
                runtime.last_error = Some(error.to_string());
            }
        }
    }

    async fn tick(&self) -> Result<()> {
        let _sync_guard = self.sync_lock.lock().await;
        let config = self.config_manager.config().await;
        if !config.sync.enabled {
            let mut runtime = self.runtime.write().await;
            runtime.reachable = false;
            runtime.authorized = false;
            runtime.syncing = false;
            runtime.reason = SyncReason::Disabled;
            runtime.last_error = None;
            return Ok(());
        }

        let token = { self.state.lock().token.clone() };
        if token.as_deref().unwrap_or("").is_empty() {
            if let Some(session) = self.github_gist_session() {
                let user = session.user.clone().unwrap_or_else(github_fallback_user);
                {
                    let mut runtime = self.runtime.write().await;
                    runtime.reachable = true;
                    runtime.authorized = true;
                    runtime.syncing = config.sync.auto_sync;
                    runtime.reason = if config.sync.auto_sync {
                        SyncReason::Syncing
                    } else {
                        SyncReason::Ready
                    };
                    runtime.last_error = None;
                }
                if !config.sync.auto_sync {
                    return Ok(());
                }
                let result = self.sync_github_gist_provider(&session.token, &user).await;
                let mut runtime = self.runtime.write().await;
                runtime.reachable = true;
                runtime.authorized = true;
                runtime.syncing = false;
                return match result {
                    Ok(()) => {
                        runtime.reason = SyncReason::Ready;
                        runtime.last_error = None;
                        Ok(())
                    }
                    Err(error) => {
                        let mut state = self.state.lock();
                        record_provider_sync_error(
                            &mut state,
                            GITHUB_GIST_PROVIDER_ID,
                            error.to_string(),
                        );
                        self.persist_state(&state)?;
                        runtime.reason = SyncReason::Error;
                        runtime.last_error = Some(error.to_string());
                        Err(error)
                    }
                };
            }
            let mut runtime = self.runtime.write().await;
            runtime.authorized = false;
            runtime.syncing = false;
            if runtime.reason != SyncReason::Unreachable {
                runtime.reason = SyncReason::Unauthorized;
            }
            runtime.last_error = None;
            return Ok(());
        }

        let client = SyncHttpClient::new(&config.sync)?;
        let reachable = client.probe_reachable(&config.sync).await;
        tracing::debug!(
            target: "bifrost_sync::manager",
            enabled = config.sync.enabled,
            auto_sync = config.sync.auto_sync,
            reachable,
            "sync tick evaluated connectivity"
        );
        if !reachable {
            if let Some(session) = self.github_gist_session() {
                let user = session.user.clone().unwrap_or_else(github_fallback_user);
                if config.sync.auto_sync {
                    if let Err(error) = self.sync_github_gist_provider(&session.token, &user).await
                    {
                        let mut state = self.state.lock();
                        record_provider_sync_error(
                            &mut state,
                            GITHUB_GIST_PROVIDER_ID,
                            error.to_string(),
                        );
                        self.persist_state(&state)?;
                    }
                }
                let mut runtime = self.runtime.write().await;
                runtime.reachable = true;
                runtime.authorized = true;
                runtime.syncing = false;
                runtime.reason = SyncReason::Ready;
                runtime.last_error = None;
                return Ok(());
            }
            let mut runtime = self.runtime.write().await;
            runtime.reachable = false;
            runtime.authorized = false;
            runtime.syncing = false;
            runtime.reason = SyncReason::Unreachable;
            runtime.last_error = None;
            return Ok(());
        }

        let token = token.unwrap_or_default();
        let user = client.get_user_info(&config.sync, &token).await?;
        let Some(user) = user else {
            let was_authorized = {
                let runtime = self.runtime.read().await;
                runtime.authorized
            };
            {
                let mut state = self.state.lock();
                if let Some(provider_id) = provider_id_for_remote(&config.sync.remote_base_url) {
                    record_provider_sync_error(
                        &mut state,
                        provider_id,
                        "Token expired or invalid. Please re-login via the admin UI.".to_string(),
                    );
                }
                state.user = None;
                state.token = None;
                self.persist_state(&state)?;
            }
            if let Some(session) = self.github_gist_session() {
                let user = session.user.clone().unwrap_or_else(github_fallback_user);
                if config.sync.auto_sync {
                    if let Err(error) = self.sync_github_gist_provider(&session.token, &user).await
                    {
                        let mut state = self.state.lock();
                        record_provider_sync_error(
                            &mut state,
                            GITHUB_GIST_PROVIDER_ID,
                            error.to_string(),
                        );
                        self.persist_state(&state)?;
                    }
                }
                let mut runtime = self.runtime.write().await;
                runtime.reachable = true;
                runtime.authorized = true;
                runtime.syncing = false;
                runtime.reason = SyncReason::Ready;
                runtime.last_error = None;
                return Ok(());
            }
            let mut runtime = self.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = false;
            runtime.syncing = false;
            runtime.reason = SyncReason::Unauthorized;
            runtime.last_error = None;
            drop(runtime);
            if was_authorized {
                tracing::info!(
                    target: "bifrost_sync::manager",
                    "session expired, triggering rules reload to disable group rules"
                );
                let _ = self.config_manager.notify(ConfigChangeEvent::rules_changed(
                    RulesChangeOrigin::RemoteSync,
                ));
            }
            return Ok(());
        };

        {
            let mut state = self.state.lock();
            state.user = Some(user.clone());
            update_provider_session_user(&config.sync, &mut state, &user);
            self.persist_state(&state)?;
        }

        if !config.sync.auto_sync {
            let mut runtime = self.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = true;
            runtime.syncing = false;
            runtime.reason = SyncReason::Ready;
            runtime.last_error = None;
            return Ok(());
        }

        {
            let mut runtime = self.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = true;
            runtime.syncing = true;
            runtime.reason = SyncReason::Syncing;
            runtime.last_error = None;
        }

        let result = match self.sync_rules(&client, &config.sync, &token, &user).await {
            Ok(()) => {
                self.sync_basic_configs(&client, &config.sync, &token, &user)
                    .await
            }
            Err(error) => Err(error),
        };
        if result.is_ok() {
            if let Some(session) = self.github_gist_session() {
                let gist_user = session.user.unwrap_or_else(|| user.clone());
                if let Err(error) = self
                    .mirror_github_gist_provider(&session.token, &gist_user)
                    .await
                {
                    let mut state = self.state.lock();
                    record_provider_sync_error(
                        &mut state,
                        GITHUB_GIST_PROVIDER_ID,
                        error.to_string(),
                    );
                    self.persist_state(&state)?;
                }
            }
        }
        let mut runtime = self.runtime.write().await;
        runtime.reachable = true;
        runtime.authorized = true;
        runtime.syncing = false;
        match result {
            Ok(()) => {
                runtime.reason = SyncReason::Ready;
                runtime.last_error = None;
                Ok(())
            }
            Err(error) => {
                tracing::error!(
                    target: "bifrost_sync::manager",
                    error = %error,
                    user_id = %user.user_id,
                    "sync tick failed"
                );
                runtime.reason = SyncReason::Error;
                runtime.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    async fn startup_login_preflight(&self) -> Result<()> {
        self.startup_login_preflight_with_delay(startup_login_preflight_retry_delay())
            .await
    }

    async fn startup_login_preflight_with_delay(&self, retry_delay: Duration) -> Result<()> {
        if startup_login_preflight_disabled_by_env() {
            tracing::info!(
                target: "bifrost_sync::manager",
                env = DISABLE_AUTO_LOGIN_PROMPT_ENV,
                "sync startup login preflight skipped because auto login prompt is disabled by environment"
            );
            return Ok(());
        }

        for attempt in 1..=STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS {
            let config = self.config_manager.config().await;
            if !config.sync.enabled {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    "sync startup login preflight skipped because sync is disabled"
                );
                return Ok(());
            }
            if self.has_session() {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    "sync startup login preflight skipped because session token exists"
                );
                return Ok(());
            }
            if self.startup_login_already_prompted() {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    "sync startup login preflight skipped because login was already auto prompted"
                );
                let mut runtime = self.runtime.write().await;
                runtime.authorized = false;
                runtime.syncing = false;
                runtime.reason = SyncReason::Unauthorized;
                runtime.last_error = None;
                return Ok(());
            }

            let client = SyncHttpClient::new(&config.sync)?;
            let reachable = client.probe_reachable(&config.sync).await;
            tracing::debug!(
                target: "bifrost_sync::manager",
                attempt,
                max_attempts = STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS,
                reachable,
                remote_base_url = %config.sync.remote_base_url,
                "sync startup login preflight probed remote"
            );
            if reachable {
                let opened = self.open_startup_login_browser(&config.sync).await?;
                let mut runtime = self.runtime.write().await;
                runtime.reachable = true;
                runtime.authorized = false;
                runtime.syncing = false;
                runtime.reason = SyncReason::Unauthorized;
                runtime.last_error = None;
                drop(runtime);
                tracing::info!(
                    target: "bifrost_sync::manager",
                    opened,
                    remote_base_url = %config.sync.remote_base_url,
                    "sync startup login preflight completed with reachable remote"
                );
                return Ok(());
            }

            {
                let mut runtime = self.runtime.write().await;
                runtime.reachable = false;
                runtime.authorized = false;
                runtime.syncing = false;
                runtime.reason = SyncReason::Unreachable;
                runtime.last_error = None;
            }

            if attempt < STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS {
                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {}
                    _ = self.wake.notified() => {
                        tracing::debug!(
                            target: "bifrost_sync::manager",
                            attempt,
                            "sync startup login preflight interrupted by sync wake"
                        );
                        return Ok(());
                    }
                }
            }
        }

        tracing::info!(
            target: "bifrost_sync::manager",
            attempts = STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS,
            "sync startup login preflight stopped because remote stayed unreachable"
        );
        Ok(())
    }

    fn startup_login_already_prompted(&self) -> bool {
        self.state
            .lock()
            .startup_login_prompt
            .as_ref()
            .is_some_and(|prompt| !prompt.auto_prompted_at.trim().is_empty())
    }

    async fn open_startup_login_browser(&self, sync_config: &SyncConfig) -> Result<bool> {
        if self.startup_login_already_prompted() {
            return Ok(false);
        }
        let opened = self.open_login_browser(sync_config, false).await?;
        if opened {
            let mut state = self.state.lock();
            state.startup_login_prompt = Some(StartupLoginPromptFile {
                auto_prompted_at: Utc::now().to_rfc3339(),
                remote_base_url: sync_config
                    .remote_base_url
                    .trim_end_matches('/')
                    .to_string(),
            });
            self.persist_state(&state)?;
        }
        Ok(opened)
    }

    async fn open_login_browser(&self, sync_config: &SyncConfig, force: bool) -> Result<bool> {
        let should_open = {
            let prompt = self.login_prompt.lock();
            force || prompt.last_opened_at.is_none()
        };
        if !should_open {
            return Ok(false);
        }

        let client = SyncHttpClient::new(sync_config)?;
        let login_url = client.login_url_with_reauth(sync_config, &self.local_callback_url);
        open_url_in_browser(&login_url)?;
        self.login_prompt.lock().last_opened_at = Some(Utc::now());
        Ok(true)
    }

    async fn sync_rules(
        &self,
        client: &SyncHttpClient,
        config: &SyncConfig,
        token: &str,
        user: &RemoteUser,
    ) -> Result<()> {
        let rules_storage = self.config_manager.rules_storage().await;
        let local_rules = rules_storage
            .load_all()?
            .into_iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .collect::<Vec<_>>();
        let remote_rules = client
            .search_envs(config, token, &user.user_id)
            .await?
            .into_iter()
            .filter(|rule| !RulesStorage::is_default_rule_name(&rule.name))
            .collect::<Vec<_>>();
        let now = Utc::now();
        tracing::debug!(
            target: "bifrost_sync::manager",
            local_rules = local_rules.len(),
            remote_rules = remote_rules.len(),
            user_id = %user.user_id,
            "starting rules sync"
        );

        let state_snapshot = self.state.lock().clone();

        let remote_by_id: HashMap<&str, &RemoteEnv> = remote_rules
            .iter()
            .map(|env| (env.id.as_str(), env))
            .collect();
        let remote_name_counts: HashMap<&str, usize> =
            remote_rules.iter().fold(HashMap::new(), |mut counts, env| {
                *counts.entry(env.name.as_str()).or_default() += 1;
                counts
            });
        let remote_by_unique_name: HashMap<&str, &RemoteEnv> = remote_rules
            .iter()
            .filter(|env| remote_name_counts.get(env.name.as_str()) == Some(&1))
            .map(|env| (env.name.as_str(), env))
            .collect();

        let tombstone_remote_ids: HashSet<String> = state_snapshot
            .deleted_rules
            .values()
            .filter(|t| !is_github_gist_remote_id(&t.remote_id))
            .map(|t| t.remote_id.clone())
            .collect();
        let tombstone_names: HashSet<String> = state_snapshot
            .deleted_rules
            .values()
            .filter(|t| !is_github_gist_remote_id(&t.remote_id))
            .map(|t| t.rule_name.clone())
            .collect();

        let mut plan = Vec::new();
        let mut blocked_remote_ids: HashSet<String> = HashSet::new();
        let mut blocked_names: HashSet<String> = HashSet::new();

        for tombstone in state_snapshot
            .deleted_rules
            .values()
            .filter(|tombstone| !is_github_gist_remote_id(&tombstone.remote_id))
        {
            blocked_remote_ids.insert(tombstone.remote_id.clone());
            blocked_names.insert(tombstone.rule_name.clone());

            if rules_storage.exists(&tombstone.rule_name) {
                plan.push(SyncPlanStep::DeleteLocal {
                    tombstone: tombstone.clone(),
                });
            }

            let matching_remote_envs: Vec<&RemoteEnv> = remote_rules
                .iter()
                .filter(|env| env.id == tombstone.remote_id || env.name == tombstone.rule_name)
                .collect();

            for remote_env in matching_remote_envs {
                blocked_remote_ids.insert(remote_env.id.clone());
                plan.push(SyncPlanStep::DeleteRemote {
                    tombstone: tombstone.clone(),
                    remote_env: remote_env.clone(),
                });
            }
        }

        let mut consumed_remote_ids = blocked_remote_ids.clone();
        for local_rule in &local_rules {
            if blocked_names.contains(&local_rule.name) {
                continue;
            }

            let server_remote_id = server_remote_id_for_rule(local_rule);
            let remote_env = server_remote_id
                .and_then(|remote_id| remote_by_id.get(remote_id).copied())
                .or_else(|| {
                    if server_remote_id.is_some() {
                        None
                    } else {
                        remote_by_unique_name.get(local_rule.name.as_str()).copied()
                    }
                });

            if let Some(remote_env) = remote_env {
                consumed_remote_ids.insert(remote_env.id.clone());
                match local_rule.sync.status {
                    RuleSyncStatus::Modified | RuleSyncStatus::LocalOnly => {
                        plan.push(SyncPlanStep::UpdateRemote {
                            local_rule: local_rule.clone(),
                            remote_env: remote_env.clone(),
                        });
                    }
                    RuleSyncStatus::Synced => {
                        let normalized_remote_content =
                            normalize_remote_rule(remote_env, &remote_rules);
                        let remote_hash = content_hash(&normalized_remote_content);
                        let remote_changed = local_rule.sync.remote_updated_at.as_deref()
                            != Some(remote_env.update_time.as_str())
                            || local_rule.sync.last_synced_content_hash.as_deref()
                                != Some(remote_hash.as_str());
                        if remote_changed {
                            plan.push(SyncPlanStep::UpdateLocal {
                                local_rule: local_rule.clone(),
                                remote_env: remote_env.clone(),
                            });
                        }
                    }
                }
            } else if server_remote_id.is_some() && local_rule.sync.status == RuleSyncStatus::Synced
            {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %local_rule.name,
                    remote_id = ?local_rule.sync.remote_id,
                    "synced rule disappeared from remote, deleting local copy"
                );
                rules_storage.delete(&local_rule.name)?;
            } else if server_remote_id.is_some()
                && local_rule.sync.status == RuleSyncStatus::Modified
            {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %local_rule.name,
                    remote_id = ?local_rule.sync.remote_id,
                    "modified rule's remote disappeared, re-creating on remote"
                );
                plan.push(SyncPlanStep::CreateRemote {
                    local_rule: local_rule.clone(),
                });
            } else {
                plan.push(SyncPlanStep::CreateRemote {
                    local_rule: local_rule.clone(),
                });
            }
        }

        for remote_env in &remote_rules {
            if consumed_remote_ids.contains(&remote_env.id) {
                continue;
            }

            if tombstone_names.contains(remote_env.name.as_str())
                || tombstone_remote_ids.contains(remote_env.id.as_str())
            {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %remote_env.name,
                    remote_id = %remote_env.id,
                    "skipping remote rule blocked by tombstone"
                );
                continue;
            }

            tracing::debug!(
                target: "bifrost_sync::manager",
                name = %remote_env.name,
                remote_id = %remote_env.id,
                "pulling remote rule into local storage"
            );
            plan.push(SyncPlanStep::CreateLocal {
                remote_env: remote_env.clone(),
            });
        }

        let mut pulled_remote = false;
        let mut pushed_local = false;
        let mut local_storage_changed = false;
        let mut tombstones_to_remove: HashSet<String> = HashSet::new();
        let mut tombstone_delete_success: HashMap<String, bool> = HashMap::new();
        let mut tombstone_deleted_remote_count: usize = 0;
        let mut tombstone_deleted_local_count: usize = 0;
        let mut tombstone_delete_failed_count: usize = 0;
        for step in plan {
            match step {
                SyncPlanStep::DeleteLocal { tombstone } => {
                    if rules_storage.exists(&tombstone.rule_name) {
                        rules_storage.delete(&tombstone.rule_name)?;
                        tracing::debug!(
                            target: "bifrost_sync::manager",
                            name = %tombstone.rule_name,
                            rule_id = %tombstone.rule_id,
                            "deleted local rule via tombstone"
                        );
                        tombstone_deleted_local_count += 1;
                        local_storage_changed = true;
                    }
                }
                SyncPlanStep::DeleteRemote {
                    tombstone,
                    remote_env,
                } => match client
                    .delete_env_by_id(config, token, &remote_env.id, &remote_env.user_id)
                    .await
                {
                    Ok(()) => {
                        tombstone_delete_success
                            .entry(tombstone.rule_id.clone())
                            .and_modify(|success| *success &= true)
                            .or_insert(true);
                        tracing::debug!(
                            target: "bifrost_sync::manager",
                            name = %tombstone.rule_name,
                            remote_id = %remote_env.id,
                            "deleted remote rule via tombstone"
                        );
                        tombstone_deleted_remote_count += 1;
                        pushed_local = true;
                    }
                    Err(error) => {
                        tombstone_delete_success.insert(tombstone.rule_id.clone(), false);
                        tombstone_delete_failed_count += 1;
                        tracing::warn!(
                            target: "bifrost_sync::manager",
                            name = %tombstone.rule_name,
                            rule_id = %tombstone.rule_id,
                            remote_id = %remote_env.id,
                            error = %error,
                            "failed to delete remote rule via tombstone, will retry later"
                        );
                    }
                },
                SyncPlanStep::UpdateRemote {
                    local_rule,
                    remote_env,
                } => {
                    let updated_remote = client
                        .update_env(config, token, &remote_env, &local_rule.content)
                        .await?;
                    let mut synced_rule = local_rule.clone();
                    synced_rule.mark_synced(
                        updated_remote.id.clone(),
                        updated_remote.user_id.clone(),
                        updated_remote.create_time.clone(),
                        updated_remote.update_time.clone(),
                    );
                    rules_storage.save(&synced_rule)?;
                    pushed_local = true;
                    local_storage_changed = true;
                }
                SyncPlanStep::CreateRemote { local_rule } => {
                    let created = client
                        .create_env(
                            config,
                            token,
                            &user.user_id,
                            &local_rule.name,
                            &local_rule.content,
                        )
                        .await?;
                    let mut synced_rule = local_rule.clone();
                    synced_rule.mark_synced(
                        created.id.clone(),
                        created.user_id.clone(),
                        created.create_time.clone(),
                        created.update_time.clone(),
                    );
                    rules_storage.save(&synced_rule)?;
                    pushed_local = true;
                    local_storage_changed = true;
                }
                SyncPlanStep::UpdateLocal {
                    local_rule,
                    remote_env,
                } => {
                    self.save_remote_as_local(
                        &rules_storage,
                        &local_rule,
                        &remote_env,
                        &remote_rules,
                    )?;
                    pulled_remote = true;
                    local_storage_changed = true;
                }
                SyncPlanStep::CreateLocal { remote_env } => {
                    let remote_content = normalize_remote_rule(&remote_env, &remote_rules);
                    let mut remote_placeholder = RuleFile {
                        name: remote_env.name.clone(),
                        content: remote_content,
                        enabled: false,
                        sort_order: 0,
                        description: Some("Synced from remote".to_string()),
                        group: None,
                        version: "1.0.0".to_string(),
                        created_at: remote_env.create_time.clone(),
                        updated_at: remote_env.update_time.clone(),
                        sync: bifrost_storage::RuleSyncMetadata::default(),
                    };
                    remote_placeholder.mark_synced(
                        remote_env.id.clone(),
                        remote_env.user_id.clone(),
                        remote_env.create_time.clone(),
                        remote_env.update_time.clone(),
                    );
                    rules_storage.save(&remote_placeholder)?;
                    pulled_remote = true;
                    local_storage_changed = true;
                }
            }
        }

        if tombstone_deleted_remote_count > 0
            || tombstone_deleted_local_count > 0
            || tombstone_delete_failed_count > 0
        {
            tracing::info!(
                target: "bifrost_sync::manager",
                remote_deleted = tombstone_deleted_remote_count,
                local_deleted = tombstone_deleted_local_count,
                failed = tombstone_delete_failed_count,
                "tombstone enforcement summary"
            );
        }

        let mut current_state = self.state.lock();
        for (deleted_rule_id, tombstone) in &state_snapshot.deleted_rules {
            if rules_storage.exists(&tombstone.rule_name) {
                continue;
            }

            let tombstone_age = tombstone
                .deleted_at
                .parse::<DateTime<Utc>>()
                .ok()
                .map(|deleted_at| (now - deleted_at).num_seconds())
                .unwrap_or(0);

            let remote_has_matching = remote_rules
                .iter()
                .any(|env| env.id == tombstone.remote_id || env.name == tombstone.rule_name);

            if tombstone_age > TOMBSTONE_MAX_AGE_SECS {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %tombstone.rule_name,
                    rule_id = %tombstone.rule_id,
                    deleted_at = %tombstone.deleted_at,
                    "tombstone expired after max age, removing"
                );
                tombstones_to_remove.insert(deleted_rule_id.clone());
                continue;
            }

            if !remote_has_matching && tombstone_age >= TOMBSTONE_MIN_AGE_SECS {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %tombstone.rule_name,
                    remote_id = %tombstone.remote_id,
                    age_secs = tombstone_age,
                    "tombstone cleared: remote has no matching rules and min age reached"
                );
                tombstones_to_remove.insert(deleted_rule_id.clone());
            }
        }
        for deleted_rule_id in &tombstones_to_remove {
            current_state.deleted_rules.remove(deleted_rule_id);
        }
        if !tombstones_to_remove.is_empty() {
            tracing::info!(
                target: "bifrost_sync::manager",
                cleared = tombstones_to_remove.len(),
                remaining = current_state.deleted_rules.len(),
                "tombstones cleared"
            );
        }
        let sync_at = now.to_rfc3339();
        current_state.last_sync_at = Some(sync_at.clone());
        let sync_action = match (pushed_local, pulled_remote) {
            (true, true) => SyncAction::Bidirectional,
            (true, false) => SyncAction::LocalPushed,
            (false, true) => SyncAction::RemotePulled,
            (false, false) => SyncAction::NoChange,
        };
        current_state.last_sync_action = Some(sync_action);
        if let Some(provider_id) = provider_id_for_remote(&config.remote_base_url) {
            record_provider_sync_success(
                &mut current_state,
                provider_id,
                sync_at,
                Some(sync_action),
            );
        }
        if sync_action != SyncAction::NoChange {
            tracing::info!(
                target: "bifrost_sync::manager",
                tombstones = current_state.deleted_rules.len(),
                last_sync_action = ?sync_action,
                "sync cycle completed"
            );
        } else {
            tracing::debug!(
                target: "bifrost_sync::manager",
                tombstones = current_state.deleted_rules.len(),
                "sync cycle completed with no changes"
            );
        }
        self.persist_state(&current_state)?;
        drop(current_state);

        if local_storage_changed {
            let _ = self.config_manager.notify(ConfigChangeEvent::rules_changed(
                RulesChangeOrigin::RemoteSync,
            ));
        }

        Ok(())
    }

    async fn sync_basic_configs(
        &self,
        client: &SyncHttpClient,
        config: &SyncConfig,
        token: &str,
        user: &RemoteUser,
    ) -> Result<()> {
        let unified = self.config_manager.config().await;
        let snapshots = [
            (
                "app_allowlist",
                serde_json::to_string(&unified.tls.app_intercept_include)
                    .map_err(|e| BifrostError::Config(format!("encode app allowlist: {e}")))?,
            ),
            (
                "domain_allowlist",
                serde_json::to_string(&unified.tls.intercept_include)
                    .map_err(|e| BifrostError::Config(format!("encode domain allowlist: {e}")))?,
            ),
            (
                "blacklist",
                serde_json::to_string(&unified.tls.intercept_exclude)
                    .map_err(|e| BifrostError::Config(format!("encode blacklist: {e}")))?,
            ),
        ];

        let state_snapshot = self.state.lock().clone();
        let mut check_list = Vec::new();
        let mut update_list = Vec::new();
        for (key, value_json) in snapshots {
            let hash = content_hash(&value_json);
            match state_snapshot.basic_configs.get(key) {
                Some(meta) if meta.hash.as_deref() == Some(hash.as_str()) => {
                    check_list.push(SyncBasicConfigCheckItem {
                        id: key.to_string(),
                        user_id: user.user_id.clone(),
                        config_key: key.to_string(),
                        update_time: meta.remote_updated_at.clone().unwrap_or_default(),
                        hash,
                    });
                }
                _ => update_list.push(SyncBasicConfigUpdateItem {
                    user_id: user.user_id.clone(),
                    config_key: key.to_string(),
                    value_json,
                    hash,
                }),
            }
        }

        let data = match client
            .sync_basic_configs(config, token, &user.user_id, check_list, update_list)
            .await
        {
            Ok(data) => data,
            Err(error)
                if error
                    .to_string()
                    .contains("sync basic config returned empty data")
                    || error.to_string().contains("status 404") =>
            {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    error = %error,
                    "remote sync provider does not expose basic config sync; skipping"
                );
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        let mut remote_applied = false;
        for remote in &data.local_update_list {
            if self.apply_remote_basic_config(remote).await? {
                remote_applied = true;
            }
        }

        {
            let mut state = self.state.lock();
            for remote in data.local_update_list {
                if !remote.config_key.trim().is_empty() {
                    state.basic_configs.insert(
                        remote.config_key.clone(),
                        BasicConfigSyncMeta {
                            remote_updated_at: Some(remote.update_time),
                            hash: Some(remote.hash),
                        },
                    );
                }
            }
            for item in data.result_list {
                if item.status != 0 {
                    tracing::warn!(
                        target: "bifrost_sync::manager",
                        config_key = %item.config_key,
                        message = ?item.msg,
                        "remote rejected basic config sync item"
                    );
                    continue;
                }
                let key = if !item.config_key.trim().is_empty() {
                    item.config_key
                } else {
                    item.id
                };
                if key.trim().is_empty() {
                    continue;
                }
                state.basic_configs.insert(
                    key,
                    BasicConfigSyncMeta {
                        remote_updated_at: Some(item.update_time),
                        hash: Some(item.hash),
                    },
                );
            }
            for key in data.local_delete_list {
                state.basic_configs.remove(&key);
            }
            self.persist_state(&state)?;
        }

        if remote_applied {
            tracing::info!(
                target: "bifrost_sync::manager",
                "remote basic config changes applied"
            );
        }

        Ok(())
    }

    async fn apply_remote_basic_config(&self, remote: &RemoteBasicConfig) -> Result<bool> {
        let values: Vec<String> = serde_json::from_str(&remote.value_json).map_err(|e| {
            BifrostError::Config(format!(
                "remote basic config '{}' is not a string list: {e}",
                remote.config_key
            ))
        })?;
        let update = match remote.config_key.as_str() {
            "app_allowlist" => TlsConfigUpdate {
                app_intercept_include: Some(values),
                ..Default::default()
            },
            "domain_allowlist" => TlsConfigUpdate {
                intercept_include: Some(values),
                ..Default::default()
            },
            "blacklist" => TlsConfigUpdate {
                intercept_exclude: Some(values),
                ..Default::default()
            },
            _ => return Ok(false),
        };
        self.config_manager.update_tls_config(update).await?;
        Ok(true)
    }

    fn save_remote_as_local(
        &self,
        rules_storage: &RulesStorage,
        existing_rule: &RuleFile,
        remote_env: &RemoteEnv,
        remote_envs: &[RemoteEnv],
    ) -> Result<()> {
        let rule = merge_remote_into_local_rule(existing_rule, remote_env, remote_envs);
        rules_storage.save(&rule)
    }

    fn persist_state(&self, state: &SyncStateFile) -> Result<()> {
        let content = serde_json::to_string_pretty(state)
            .map_err(|e| BifrostError::Config(format!("failed to serialize sync state: {e}")))?;
        fs::write(&self.state_file, content)?;
        // The sync state file embeds SSO tokens; restrict it to the owner on
        // Unix. Best-effort: log a warning on failure rather than failing sync.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&self.state_file, fs::Permissions::from_mode(0o600))
            {
                tracing::warn!(
                    "failed to set 0600 permissions on {}: {e}",
                    self.state_file.display()
                );
            }
        }
        Ok(())
    }
}

fn merge_remote_into_local_rule(
    existing_rule: &RuleFile,
    remote_env: &RemoteEnv,
    remote_envs: &[RemoteEnv],
) -> RuleFile {
    let mut rule = RuleFile {
        name: existing_rule.name.clone(),
        content: normalize_remote_rule(remote_env, remote_envs),
        enabled: existing_rule.enabled,
        sort_order: existing_rule.sort_order,
        description: existing_rule.description.clone(),
        group: existing_rule.group.clone(),
        version: existing_rule.version.clone(),
        created_at: existing_rule.created_at.clone(),
        updated_at: remote_env.update_time.clone(),
        sync: existing_rule.sync.clone(),
    };
    rule.mark_synced(
        remote_env.id.clone(),
        remote_env.user_id.clone(),
        remote_env.create_time.clone(),
        remote_env.update_time.clone(),
    );
    rule
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn open_url_in_browser(url: &str) -> Result<()> {
    if let Ok(path) = std::env::var(LOGIN_BROWSER_DRY_RUN_FILE_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(file, "{url}")?;
            return Ok(());
        }
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map_err(|error| BifrostError::Network(format!("failed to open login browser: {error}")))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_url_in_browser(url: &str) -> Result<()> {
    if let Ok(path) = std::env::var(LOGIN_BROWSER_DRY_RUN_FILE_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(file, "{url}")?;
            return Ok(());
        }
    }

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let operation = wide("open");
    let url = wide(url);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(BifrostError::Network(format!(
            "failed to open login browser: ShellExecuteW failed with code {result}"
        )));
    }
    Ok(())
}

fn startup_login_preflight_retry_delay() -> Duration {
    std::env::var(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_SECS))
}

fn startup_login_preflight_disabled_by_env() -> bool {
    std::env::var(DISABLE_AUTO_LOGIN_PROMPT_ENV)
        .ok()
        .is_some_and(|value| is_truthy_env_value(&value))
}

fn is_truthy_env_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as TokioMutex;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn env_lock() -> &'static TokioMutex<()> {
        static LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| TokioMutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        old_value: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old_value = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old_value }
        }

        fn unset(key: &'static str) -> Self {
            let old_value = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, old_value }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.old_value.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    async fn spawn_sso_check_server(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_task = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let statuses = statuses.clone();
                let hits = hits_for_task.clone();
                tokio::spawn(async move {
                    let mut buf = [0_u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let hit = hits.fetch_add(1, Ordering::SeqCst);
                    let status = statuses
                        .get(hit)
                        .copied()
                        .or_else(|| statuses.last().copied())
                        .unwrap_or(200);
                    let reason = if status == 200 {
                        "OK"
                    } else {
                        "Service Unavailable"
                    };
                    let response =
                        format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    async fn sync_manager_for_remote(
        remote_base_url: &str,
    ) -> (TempDir, Arc<ConfigManager>, SyncManager) {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some(remote_base_url.to_string()),
                connect_timeout_ms: Some(5_000),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager.clone(), 9900).unwrap();
        (temp_dir, config_manager, manager)
    }

    fn test_user(user_id: &str) -> RemoteUser {
        RemoteUser {
            user_id: user_id.to_string(),
            nickname: user_id.to_string(),
            avatar: String::new(),
            email: format!("{user_id}@example.test"),
        }
    }

    #[test]
    fn provider_and_github_gist_helpers_cover_routing_edges() {
        assert_eq!(provider_id_for_remote("   "), None);
        assert_eq!(
            provider_id_for_remote("https://bifrost.bytedance.net/"),
            Some("bytedance_internal")
        );
        assert_eq!(
            provider_id_for_remote("https://sync.example.test/"),
            Some("bifrost_cloud")
        );

        let fallback = github_fallback_user();
        assert_eq!(fallback.user_id, "github");
        assert_eq!(fallback.nickname, "GitHub");

        let local_rule = RuleFile::new("local", "a.example.com host://127.0.0.1:3000");
        assert!(gist_remote_id_for_rule(&local_rule).starts_with("gist:"));
        assert!(server_remote_id_for_rule(&local_rule).is_none());

        let mut gist_rule = local_rule.clone();
        gist_rule.sync.remote_id = Some("gist:existing".to_string());
        assert_eq!(gist_remote_id_for_rule(&gist_rule), "gist:existing");
        assert!(server_remote_id_for_rule(&gist_rule).is_none());

        let mut server_rule = local_rule;
        server_rule.sync.remote_id = Some("server-env".to_string());
        assert_eq!(server_remote_id_for_rule(&server_rule), Some("server-env"));

        let gist_config = GitHubGistBasicConfig {
            id: "domain_allowlist".to_string(),
            user_id: "github:1".to_string(),
            config_key: "domain_allowlist".to_string(),
            value_json: "[\"example.com\"]".to_string(),
            hash: "hash".to_string(),
            create_time: "2026-07-07T00:00:00Z".to_string(),
            update_time: "2026-07-07T00:01:00Z".to_string(),
        };
        let remote_config = gist_config_to_remote(&gist_config);
        assert_eq!(remote_config.id, "domain_allowlist");
        assert_eq!(remote_config.hash, "hash");
    }

    #[tokio::test]
    async fn status_marks_remote_invoke_registered_for_bytedance_and_cloud_sessions() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        {
            let mut state = manager.state.lock();
            state.provider_sessions.insert(
                "bytedance_internal".to_string(),
                SyncProviderSession {
                    token: "byte-token".to_string(),
                    remote_base_url: DEFAULT_REMOTE_BASE_URL.to_string(),
                    user: Some(test_user("byte-user")),
                },
            );
            state.provider_sessions.insert(
                "bifrost_cloud".to_string(),
                SyncProviderSession {
                    token: "cloud-token".to_string(),
                    remote_base_url: "https://sync.example.test".to_string(),
                    user: Some(test_user("cloud-user")),
                },
            );
            manager.persist_state(&state).unwrap();
        }

        let status = manager.status().await;
        let bytedance = status
            .providers
            .iter()
            .find(|provider| provider.id == "bytedance_internal")
            .expect("bytedance provider");
        let cloud = status
            .providers
            .iter()
            .find(|provider| provider.id == "bifrost_cloud")
            .expect("cloud provider");
        assert!(bytedance.connected);
        assert!(bytedance.remote_invoke_registered);
        assert_eq!(
            bytedance.user.as_ref().map(|user| user.user_id.as_str()),
            Some("byte-user")
        );
        assert!(cloud.connected);
        assert!(cloud.remote_invoke_registered);
        assert_eq!(
            cloud.user.as_ref().map(|user| user.user_id.as_str()),
            Some("cloud-user")
        );
        assert_eq!(
            cloud.remote_base_url.as_deref(),
            Some("https://sync.example.test")
        );
        assert!(!status.first_run_prompt_required);
    }

    #[tokio::test]
    async fn status_leaves_unconfigured_bifrost_cloud_url_empty() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;

        let status = manager.status().await;
        let cloud = status
            .providers
            .iter()
            .find(|provider| provider.id == "bifrost_cloud")
            .expect("cloud provider");
        assert!(!cloud.connected);
        assert!(cloud.remote_base_url.is_none());
    }

    #[tokio::test]
    async fn status_marks_github_gist_connected_from_provider_session() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        {
            let mut state = manager.state.lock();
            state.provider_sessions.insert(
                "github_gist".to_string(),
                SyncProviderSession {
                    token: "ghp_test".to_string(),
                    remote_base_url: GITHUB_GIST_REMOTE_BASE_URL.to_string(),
                    user: Some(test_user("github:12345")),
                },
            );
            manager.persist_state(&state).unwrap();
        }

        let status = manager.status().await;
        let gist = status
            .providers
            .iter()
            .find(|provider| provider.id == "github_gist")
            .expect("github gist provider");
        assert!(gist.connected);
        assert!(gist.authorized);
        assert_eq!(gist.reason, SyncReason::Ready);
        assert!(gist.last_error.is_none());
        assert!(gist.capabilities.rules_sync);
        assert!(gist.capabilities.config_sync);
        assert!(!gist.capabilities.remote_invoke);
        assert!(!gist.remote_invoke_registered);
        assert_eq!(
            gist.remote_base_url.as_deref(),
            Some(GITHUB_GIST_REMOTE_BASE_URL)
        );
        assert_eq!(
            gist.user.as_ref().map(|user| user.user_id.as_str()),
            Some("github:12345")
        );
        assert!(!status.first_run_prompt_required);
    }

    #[tokio::test]
    async fn status_marks_github_gist_session_unauthorized_after_token_error() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        {
            let mut state = manager.state.lock();
            state.provider_sessions.insert(
                "github_gist".to_string(),
                SyncProviderSession {
                    token: "expired-ghp-token".to_string(),
                    remote_base_url: GITHUB_GIST_REMOTE_BASE_URL.to_string(),
                    user: Some(test_user("github:12345")),
                },
            );
            manager.persist_state(&state).unwrap();
        }
        {
            let mut runtime = manager.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = true;
            runtime.reason = SyncReason::Error;
            runtime.last_error =
                Some("GitHub token is invalid or missing the gist scope".to_string());
        }

        let status = manager.status().await;
        let gist = status
            .providers
            .iter()
            .find(|provider| provider.id == "github_gist")
            .expect("github gist provider");
        assert!(
            gist.connected,
            "saved session should keep reconnect/sign-out actions visible"
        );
        assert!(!gist.authorized);
        assert_eq!(gist.reason, SyncReason::Error);
        assert_eq!(
            gist.last_error.as_deref(),
            Some("GitHub token is invalid or missing the gist scope")
        );
        assert!(!status.first_run_prompt_required);
    }

    #[tokio::test]
    async fn status_keeps_blank_provider_session_disconnected() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote("https://sync.example.test").await;
        {
            let mut state = manager.state.lock();
            state.provider_sessions.insert(
                "bifrost_cloud".to_string(),
                SyncProviderSession {
                    token: "   ".to_string(),
                    remote_base_url: "https://sync.example.test".to_string(),
                    user: Some(test_user("cloud-user")),
                },
            );
            manager.persist_state(&state).unwrap();
        }

        let status = manager.status().await;
        let cloud = status
            .providers
            .iter()
            .find(|provider| provider.id == "bifrost_cloud")
            .expect("cloud provider");
        assert!(!cloud.connected);
        assert!(!cloud.remote_invoke_registered);
        assert_eq!(
            cloud.remote_base_url.as_deref(),
            Some("https://sync.example.test")
        );
        assert!(status.first_run_prompt_required);
    }

    #[tokio::test]
    async fn github_gist_snapshot_syncs_rule_create_update_and_delete() {
        let (_temp_dir, config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        let user = test_user("github:12345");
        let rules_storage = config_manager.rules_storage().await;
        rules_storage
            .save(&RuleFile::new(
                "gist-rule",
                "example.com host://127.0.0.1:3000",
            ))
            .unwrap();
        let mut snapshot = GitHubGistSnapshot::default();

        let outcome = manager
            .sync_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(outcome, (true, false));
        assert_eq!(snapshot.rules.len(), 1);
        assert_eq!(snapshot.rules[0].name, "gist-rule");
        assert_eq!(snapshot.rules[0].rule, "example.com host://127.0.0.1:3000");
        let mut local = rules_storage.load("gist-rule").unwrap();
        assert_eq!(local.sync.status, RuleSyncStatus::Synced);
        assert!(local
            .sync
            .remote_id
            .as_deref()
            .is_some_and(|remote_id| remote_id.starts_with("gist:")));

        local.content = "example.com host://127.0.0.1:3001".to_string();
        local.touch_local_change();
        rules_storage.save(&local).unwrap();
        let outcome = manager
            .sync_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();
        assert_eq!(outcome, (true, false));
        assert_eq!(snapshot.rules[0].rule, "example.com host://127.0.0.1:3001");

        let synced = rules_storage.load("gist-rule").unwrap();
        manager.record_deleted_rule(&synced).await.unwrap();
        rules_storage.delete("gist-rule").unwrap();
        let outcome = manager
            .sync_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:02:00Z")
            .await
            .unwrap();
        assert_eq!(outcome, (true, false));
        assert!(snapshot.rules.is_empty());
    }

    #[tokio::test]
    async fn github_gist_snapshot_syncs_basic_config_updates() {
        let (_temp_dir, config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        let user = test_user("github:12345");
        config_manager
            .update_tls_config(TlsConfigUpdate {
                app_intercept_include: Some(vec!["com.example.App".to_string()]),
                intercept_include: Some(vec!["example.com".to_string()]),
                intercept_exclude: Some(vec!["skip.example.com".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut snapshot = GitHubGistSnapshot::default();

        let outcome = manager
            .sync_github_gist_basic_configs(&mut snapshot, &user, "2026-07-07T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(outcome, (true, false));
        assert_eq!(
            snapshot
                .basic_configs
                .get("app_allowlist")
                .map(|item| item.value_json.as_str()),
            Some("[\"com.example.App\"]")
        );
        assert_eq!(
            snapshot
                .basic_configs
                .get("domain_allowlist")
                .map(|item| item.value_json.as_str()),
            Some("[\"example.com\"]")
        );
        assert_eq!(
            snapshot
                .basic_configs
                .get("blacklist")
                .map(|item| item.value_json.as_str()),
            Some("[\"skip.example.com\"]")
        );

        config_manager
            .update_tls_config(TlsConfigUpdate {
                intercept_include: Some(vec!["changed.example.com".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
        let outcome = manager
            .sync_github_gist_basic_configs(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();
        assert_eq!(outcome, (true, false));
        assert_eq!(
            snapshot
                .basic_configs
                .get("domain_allowlist")
                .map(|item| item.value_json.as_str()),
            Some("[\"changed.example.com\"]")
        );
        let state = manager.state.lock();
        assert!(state
            .basic_configs
            .contains_key("github_gist:domain_allowlist"));
        assert!(!state.basic_configs.contains_key("domain_allowlist"));
    }

    #[tokio::test]
    async fn github_gist_mirror_does_not_overwrite_server_rule_metadata() {
        let (_temp_dir, config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        let user = test_user("github:12345");
        let rules_storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("server-rule", "example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "server-env-1",
            "server-user",
            "2026-07-07T00:00:00Z",
            "2026-07-07T00:00:00Z",
        );
        rules_storage.save(&rule).unwrap();
        let mut snapshot = GitHubGistSnapshot::default();

        let changed = manager
            .mirror_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();

        assert!(changed);
        assert_eq!(snapshot.rules.len(), 1);
        assert_eq!(snapshot.rules[0].name, "server-rule");
        assert_eq!(snapshot.rules[0].rule, "example.com host://127.0.0.1:3000");
        let local = rules_storage.load("server-rule").unwrap();
        assert_eq!(local.sync.remote_id.as_deref(), Some("server-env-1"));
        assert_eq!(local.sync.status, RuleSyncStatus::Synced);
    }

    #[tokio::test]
    async fn github_gist_mirror_replaces_snapshot_and_preserves_remote_create_time() {
        let (_temp_dir, config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        let user = test_user("github:12345");
        let rules_storage = config_manager.rules_storage().await;
        rules_storage
            .save(&RuleFile::new(
                "beta",
                "beta.example.com host://127.0.0.1:3000",
            ))
            .unwrap();
        rules_storage
            .save(&RuleFile::new(
                "alpha",
                "alpha.example.com host://127.0.0.1:3000",
            ))
            .unwrap();
        let mut snapshot = GitHubGistSnapshot {
            rules: vec![
                GitHubGistRule {
                    id: "gist:old-beta".to_string(),
                    user_id: "github:old".to_string(),
                    name: "beta".to_string(),
                    rule: "old.example.com host://127.0.0.1:3000".to_string(),
                    create_time: "2026-01-01T00:00:00Z".to_string(),
                    update_time: "2026-01-01T00:00:00Z".to_string(),
                },
                GitHubGistRule {
                    id: "gist:remote-only".to_string(),
                    user_id: "github:old".to_string(),
                    name: "remote-only".to_string(),
                    rule: "remote.example.com host://127.0.0.1:3000".to_string(),
                    create_time: "2026-01-01T00:00:00Z".to_string(),
                    update_time: "2026-01-01T00:00:00Z".to_string(),
                },
            ],
            ..GitHubGistSnapshot::default()
        };

        let changed = manager
            .mirror_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();
        assert!(changed);
        assert_eq!(
            snapshot
                .rules
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        let beta = snapshot
            .rules
            .iter()
            .find(|rule| rule.name == "beta")
            .expect("beta rule");
        assert_eq!(beta.id, "gist:old-beta");
        assert_eq!(beta.create_time, "2026-01-01T00:00:00Z");

        let changed_again = manager
            .mirror_github_gist_rules(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();
        assert!(!changed_again);
    }

    #[tokio::test]
    async fn github_gist_mirror_basic_configs_preserves_create_time_and_becomes_idle() {
        let (_temp_dir, config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        let user = test_user("github:12345");
        config_manager
            .update_tls_config(TlsConfigUpdate {
                intercept_include: Some(vec!["example.com".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut snapshot = GitHubGistSnapshot::default();
        snapshot.basic_configs.insert(
            "domain_allowlist".to_string(),
            GitHubGistBasicConfig {
                id: "domain_allowlist".to_string(),
                user_id: "github:old".to_string(),
                config_key: "domain_allowlist".to_string(),
                value_json: "[]".to_string(),
                hash: "old".to_string(),
                create_time: "2026-01-01T00:00:00Z".to_string(),
                update_time: "2026-01-01T00:00:00Z".to_string(),
            },
        );

        let changed = manager
            .mirror_github_gist_basic_configs(&mut snapshot, &user, "2026-07-07T00:01:00Z")
            .await
            .unwrap();
        assert!(changed);
        let domain_config = snapshot
            .basic_configs
            .get("domain_allowlist")
            .expect("domain config");
        assert_eq!(domain_config.value_json, "[\"example.com\"]");
        assert_eq!(domain_config.create_time, "2026-01-01T00:00:00Z");
        assert_eq!(domain_config.update_time, "2026-07-07T00:01:00Z");

        let changed_again = manager
            .mirror_github_gist_basic_configs(&mut snapshot, &user, "2026-07-07T00:02:00Z")
            .await
            .unwrap();
        assert!(!changed_again);
    }

    #[tokio::test]
    async fn server_sync_ignores_github_gist_remote_metadata() {
        let server = MockServer::start().await;
        let (_temp_dir, config_manager, manager) = sync_manager_for_remote(&server.uri()).await;
        let rules_storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("gist-origin", "example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "gist:rule-1",
            "github:12345",
            "2026-07-07T00:00:00Z",
            "2026-07-07T00:00:00Z",
        );
        rules_storage.save(&rule).unwrap();

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "message": "ok",
                "data": { "list": [] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "message": "ok",
                "data": {
                    "id": "server-env-1",
                    "user_id": "server-user",
                    "name": "gist-origin",
                    "rule": "example.com host://127.0.0.1:3000",
                    "create_time": "2026-07-07T00:01:00Z",
                    "update_time": "2026-07-07T00:01:00Z"
                }
            })))
            .mount(&server)
            .await;

        let config = config_manager.config().await;
        let sync_config = config.sync.clone();
        let client = SyncHttpClient::new(&sync_config).unwrap();
        manager
            .sync_rules(
                &client,
                &sync_config,
                "server-token",
                &test_user("server-user"),
            )
            .await
            .unwrap();

        let local = rules_storage.load("gist-origin").unwrap();
        assert_eq!(local.sync.remote_id.as_deref(), Some("server-env-1"));
        assert_eq!(local.sync.remote_user_id.as_deref(), Some("server-user"));
    }

    #[tokio::test]
    async fn remote_invoke_registration_targets_cover_single_and_dual_channels() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        manager.save_token("byte-token".to_string()).await.unwrap();
        let targets = manager.remote_invoke_registration_targets().await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].provider_id, "bytedance_internal");
        assert_eq!(targets[0].session_token, "byte-token");
        assert_eq!(
            manager
                .session_token_for_remote(DEFAULT_REMOTE_BASE_URL)
                .as_deref(),
            Some("byte-token")
        );

        {
            let mut state = manager.state.lock();
            state.provider_sessions.insert(
                "bifrost_cloud".to_string(),
                SyncProviderSession {
                    token: "cloud-token".to_string(),
                    remote_base_url: "https://sync.example.test".to_string(),
                    user: Some(test_user("cloud-user")),
                },
            );
            manager.persist_state(&state).unwrap();
        }

        let targets = manager.remote_invoke_registration_targets().await;
        assert_eq!(
            targets
                .iter()
                .map(|target| (target.provider_id.as_str(), target.session_token.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("bytedance_internal", "byte-token"),
                ("bifrost_cloud", "cloud-token")
            ]
        );
        assert_eq!(
            manager
                .session_token_for_remote("https://sync.example.test/")
                .as_deref(),
            Some("cloud-token")
        );
    }

    #[tokio::test]
    async fn remote_invoke_registration_targets_fall_back_to_legacy_token() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote("https://sync.example.test/").await;
        {
            let mut state = manager.state.lock();
            state.token = Some("legacy-cloud-token".to_string());
            manager.persist_state(&state).unwrap();
        }

        let targets = manager.remote_invoke_registration_targets().await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].provider_id, "bifrost_cloud");
        assert_eq!(targets[0].remote_base_url, "https://sync.example.test");
        assert_eq!(targets[0].session_token, "legacy-cloud-token");
        assert_eq!(
            manager
                .session_token_for_remote("https://sync.example.test")
                .as_deref(),
            Some("legacy-cloud-token")
        );
    }

    #[tokio::test]
    async fn logout_clears_provider_sessions() {
        let (_temp_dir, _config_manager, manager) =
            sync_manager_for_remote(DEFAULT_REMOTE_BASE_URL).await;
        {
            let mut state = manager.state.lock();
            state.token = Some("legacy-token".to_string());
            state.provider_sessions.insert(
                "bytedance_internal".to_string(),
                SyncProviderSession {
                    token: "byte-token".to_string(),
                    remote_base_url: DEFAULT_REMOTE_BASE_URL.to_string(),
                    user: Some(test_user("byte-user")),
                },
            );
            state.provider_sessions.insert(
                "bifrost_cloud".to_string(),
                SyncProviderSession {
                    token: "cloud-token".to_string(),
                    remote_base_url: "https://sync.example.test".to_string(),
                    user: Some(test_user("cloud-user")),
                },
            );
            manager.persist_state(&state).unwrap();
        }

        manager.logout().await.unwrap();

        let state = manager.state.lock();
        assert!(state.token.is_none());
        assert!(state.provider_sessions.is_empty());
    }

    fn read_dry_run_urls(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn auto_login_prompt_env_accepts_truthy_values_only() {
        for value in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert!(
                is_truthy_env_value(value),
                "{value:?} should disable prompt"
            );
        }
        for value in ["", "0", "false", "no", "off", "disable"] {
            assert!(
                !is_truthy_env_value(value),
                "{value:?} should not disable prompt"
            );
        }
    }

    #[test]
    fn merge_remote_into_local_preserves_local_metadata() {
        let existing_rule = RuleFile::new("demo", "old.example.com host://127.0.0.1:3000")
            .with_enabled(false)
            .with_sort_order(7)
            .with_description(Some("Pinned locally".to_string()));
        let remote_env = RemoteEnv {
            id: "remote-id".to_string(),
            user_id: "user-1".to_string(),
            name: "demo".to_string(),
            rule: "new.example.com host://127.0.0.1:3200".to_string(),
            create_time: "2026-03-20T09:00:00Z".to_string(),
            update_time: "2026-03-20T12:00:00Z".to_string(),
        };

        let merged = merge_remote_into_local_rule(
            &existing_rule,
            &remote_env,
            std::slice::from_ref(&remote_env),
        );

        assert_eq!(merged.name, "demo");
        assert_eq!(merged.content, "new.example.com host://127.0.0.1:3200");
        assert!(!merged.enabled);
        assert_eq!(merged.sort_order, 7);
        assert_eq!(merged.description.as_deref(), Some("Pinned locally"));
        assert_eq!(merged.version, existing_rule.version);
        assert_eq!(merged.created_at, existing_rule.created_at);
        assert_eq!(merged.updated_at, "2026-03-20T12:00:00Z");
    }

    #[test]
    fn sync_action_summarizes_push_pull_and_idle_results() {
        let action = |pushed_local: bool, pulled_remote: bool| match (pushed_local, pulled_remote) {
            (true, true) => SyncAction::Bidirectional,
            (true, false) => SyncAction::LocalPushed,
            (false, true) => SyncAction::RemotePulled,
            (false, false) => SyncAction::NoChange,
        };

        assert_eq!(action(true, false), SyncAction::LocalPushed);
        assert_eq!(action(false, true), SyncAction::RemotePulled);
        assert_eq!(action(true, true), SyncAction::Bidirectional);
        assert_eq!(action(false, false), SyncAction::NoChange);
    }

    #[test]
    fn synced_rule_is_deleted_when_remote_disappears() {
        let mut local_rule = RuleFile::new("demo", "local.example.com host://127.0.0.1:3000");
        local_rule.mark_synced(
            "remote-id",
            "user-1",
            "2026-03-20T09:00:00Z",
            "2026-03-20T11:00:00Z",
        );

        assert_eq!(local_rule.sync.status, RuleSyncStatus::Synced);
        assert!(local_rule.sync.remote_id.is_some());
    }

    #[test]
    fn modified_rule_is_not_deleted_when_remote_disappears() {
        let mut local_rule = RuleFile::new("demo", "local.example.com host://127.0.0.1:3000");
        local_rule.mark_synced(
            "remote-id",
            "user-1",
            "2026-03-20T09:00:00Z",
            "2026-03-20T11:00:00Z",
        );
        local_rule.touch_local_change();

        assert_eq!(local_rule.sync.status, RuleSyncStatus::Modified);
    }

    #[tokio::test]
    async fn save_token_reenables_auto_sync_after_login() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager.clone(), 9900).unwrap();

        manager.save_token("login-token".to_string()).await.unwrap();

        let config = config_manager.config().await;
        let status = manager.status().await;
        assert!(config.sync.auto_sync);
        assert!(status.has_session);
        assert_eq!(manager.state.lock().token.as_deref(), Some("login-token"));
    }

    #[tokio::test]
    async fn save_login_session_updates_remote_url_and_token() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(false),
                remote_base_url: Some("https://old.example.test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager.clone(), 9900).unwrap();

        manager
            .save_login_session(
                "  ci-token  ".to_string(),
                "https://bifrost.bytedance.net/".to_string(),
            )
            .await
            .unwrap();

        let config = config_manager.config().await;
        let status = manager.status().await;
        assert!(config.sync.auto_sync);
        assert!(config.sync.enabled);
        assert_eq!(config.sync.remote_base_url, "https://bifrost.bytedance.net");
        assert!(status.has_session);
        assert_eq!(manager.state.lock().token.as_deref(), Some("ci-token"));
    }

    #[tokio::test]
    async fn save_login_session_rejects_empty_or_invalid_input() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        assert!(manager
            .save_login_session("   ".to_string(), "https://relay.test".to_string())
            .await
            .is_err());
        assert!(manager
            .save_login_session("token".to_string(), "relay.test".to_string())
            .await
            .is_err());
        assert!(manager
            .save_login_session("token".to_string(), "   ".to_string())
            .await
            .unwrap_err()
            .to_string()
            .contains("remote_base_url"));
        assert!(manager
            .save_login_session("token".to_string(), "http://relay.test".to_string())
            .await
            .unwrap_err()
            .to_string()
            .contains("https://"));
    }

    #[tokio::test]
    async fn startup_login_preflight_opens_once_when_third_probe_is_reachable() {
        let _env_lock = env_lock().lock().await;
        let _disable_guard = EnvVarGuard::unset(DISABLE_AUTO_LOGIN_PROMPT_ENV);
        let (remote_base_url, hits) = spawn_sso_check_server(vec![503, 503, 200]).await;
        let (temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        let dry_run_file = temp_dir.path().join("opened-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(hits.load(Ordering::SeqCst), 3);
        let opened = read_dry_run_urls(&dry_run_file);
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains("/v4/sso/logout?next="));
        assert!(manager.state.lock().startup_login_prompt.is_some());

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(read_dry_run_urls(&dry_run_file).len(), 1);
    }

    #[tokio::test]
    async fn startup_login_preflight_stops_after_three_unreachable_probes() {
        let _env_lock = env_lock().lock().await;
        let _disable_guard = EnvVarGuard::unset(DISABLE_AUTO_LOGIN_PROMPT_ENV);
        let (remote_base_url, hits) = spawn_sso_check_server(vec![503, 503, 503]).await;
        let (temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        let dry_run_file = temp_dir.path().join("opened-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(
            hits.load(Ordering::SeqCst),
            STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS
        );
        assert!(read_dry_run_urls(&dry_run_file).is_empty());
        assert!(manager.state.lock().startup_login_prompt.is_none());
        assert_eq!(manager.runtime.read().await.reason, SyncReason::Unreachable);
    }

    #[tokio::test]
    async fn startup_login_preflight_skips_when_disabled_by_env() {
        let _env_lock = env_lock().lock().await;
        let (remote_base_url, hits) = spawn_sso_check_server(vec![200]).await;
        let (temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        let dry_run_file = temp_dir.path().join("opened-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );
        let _disable_guard = EnvVarGuard::set(DISABLE_AUTO_LOGIN_PROMPT_ENV, "1");

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(read_dry_run_urls(&dry_run_file).is_empty());
        assert!(manager.state.lock().startup_login_prompt.is_none());
    }

    #[tokio::test]
    async fn startup_login_preflight_wake_interrupts_retry_wait() {
        let _env_lock = env_lock().lock().await;
        let _disable_guard = EnvVarGuard::unset(DISABLE_AUTO_LOGIN_PROMPT_ENV);
        let (remote_base_url, hits) = spawn_sso_check_server(vec![503, 503, 503]).await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        let manager = Arc::new(manager);
        let preflight_manager = manager.clone();

        let preflight = tokio::spawn(async move {
            preflight_manager
                .startup_login_preflight_with_delay(Duration::from_secs(60))
                .await
        });

        for _ in 0..50 {
            if hits.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        manager.save_token("login-token".to_string()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), preflight)
            .await
            .expect("preflight should be interrupted by sync wake")
            .expect("preflight task should not panic")
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn startup_login_preflight_skips_when_auto_prompt_was_persisted() {
        let _env_lock = env_lock().lock().await;
        let _disable_guard = EnvVarGuard::unset(DISABLE_AUTO_LOGIN_PROMPT_ENV);
        let (remote_base_url, hits) = spawn_sso_check_server(vec![200]).await;
        let (temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        {
            let mut state = manager.state.lock();
            state.startup_login_prompt = Some(StartupLoginPromptFile {
                auto_prompted_at: "2026-06-02T00:00:00Z".to_string(),
                remote_base_url: remote_base_url.clone(),
            });
            manager.persist_state(&state).unwrap();
        }
        let dry_run_file = temp_dir.path().join("opened-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(read_dry_run_urls(&dry_run_file).is_empty());
        assert_eq!(
            manager.runtime.read().await.reason,
            SyncReason::Unauthorized
        );
    }

    // ----------------------------------------------------------------------
    // A small routing fixture server that mimics the real sync API surface so
    // sync_once / sync_rules / tick can be exercised end-to-end against a
    // loopback ephemeral listener (same pattern as spawn_sso_check_server).
    // ----------------------------------------------------------------------
    #[derive(Clone, Default)]
    struct FakeApi {
        // List returned by GET /v4/env.
        envs: Vec<RemoteEnv>,
        // user_id returned by GET /v4/sso/info (None => 401-style empty data).
        user_id: Option<String>,
    }

    async fn spawn_api_server(api: FakeApi) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_task = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let api = api.clone();
                let hits = hits_for_task.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0_u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    hits.fetch_add(1, Ordering::SeqCst);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first = req.lines().next().unwrap_or("");
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");

                    let json_body: String = if path.starts_with("/v4/sso/check") {
                        String::new()
                    } else if path.starts_with("/v4/sso/info") {
                        match &api.user_id {
                            Some(uid) => format!(
                                r#"{{"code":0,"message":"ok","data":{{"user_id":"{uid}","nickname":"N","avatar":"","email":""}}}}"#
                            ),
                            None => r#"{"code":0,"message":"ok","data":null}"#.to_string(),
                        }
                    } else if path.starts_with("/v4/env") && method == "GET" {
                        let list = api
                            .envs
                            .iter()
                            .map(|e| {
                                format!(
                                    r#"{{"id":"{}","user_id":"{}","name":"{}","rule":"{}","create_time":"{}","update_time":"{}"}}"#,
                                    e.id, e.user_id, e.name, e.rule, e.create_time, e.update_time
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(",");
                        format!(r#"{{"code":0,"message":"ok","data":{{"list":[{list}]}}}}"#)
                    } else if path.starts_with("/v4/env") && method == "POST" {
                        // create_env returns a fresh env.
                        r#"{"code":0,"message":"ok","data":{"id":"new-remote","user_id":"user-1","name":"created","rule":"r","create_time":"2026-01-01T00:00:00Z","update_time":"2026-01-01T00:00:00Z"}}"#.to_string()
                    } else if path.starts_with("/v4/env") && method == "PATCH" {
                        r#"{"code":0,"message":"ok","data":{"id":"upd-remote","user_id":"user-1","name":"updated","rule":"r","create_time":"2026-01-01T00:00:00Z","update_time":"2026-01-02T00:00:00Z"}}"#.to_string()
                    } else {
                        // DELETE and everything else: empty envelope.
                        r#"{"code":0,"message":"ok","data":null}"#.to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        json_body.len(),
                        json_body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    fn re(id: &str, name: &str, rule: &str, update: &str) -> RemoteEnv {
        RemoteEnv {
            id: id.into(),
            user_id: "user-1".into(),
            name: name.into(),
            rule: rule.into(),
            create_time: "2026-01-01T00:00:00Z".into(),
            update_time: update.into(),
        }
    }

    #[tokio::test]
    async fn handle_delegates_to_inner_manager() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some("https://example.test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = Arc::new(SyncManager::new(config_manager, 9900).unwrap());
        let handle = SyncManagerHandle::new(manager.clone());

        // status / session accessors flow through the handle.
        let status = handle.status().await;
        assert!(status.enabled);
        assert!(!status.has_session);
        assert!(handle.session_token().is_none());

        // login_url goes through the handle to the client builder.
        let url = handle.login_url("http://cb").await.unwrap();
        assert!(url.contains("/v4/sso/login?next="));

        // save_token returns updated status with a session.
        let after = handle.save_token("tok".to_string()).await.unwrap();
        assert!(after.has_session);
        assert_eq!(handle.session_token().as_deref(), Some("tok"));

        // trigger_sync just notifies; should not panic.
        handle.trigger_sync();

        // logout clears the session.
        let after_logout = handle.logout().await.unwrap();
        assert!(!after_logout.has_session);
    }

    #[tokio::test]
    async fn accessors_reflect_state() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        assert!(!manager.has_session());
        assert!(manager.current_user_id().is_none());
        assert!(manager.session_token().is_none());

        {
            let mut state = manager.state.lock();
            state.token = Some("tk".to_string());
            state.user = Some(RemoteUser {
                user_id: "u-9".to_string(),
                ..RemoteUser::default()
            });
            manager.persist_state(&state).unwrap();
        }
        assert!(manager.has_session());
        assert_eq!(manager.current_user_id().as_deref(), Some("u-9"));
        assert_eq!(manager.session_token().as_deref(), Some("tk"));
    }

    #[tokio::test]
    async fn record_and_clear_deleted_rule_tombstones() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        // A rule with no remote_id is a no-op (returns Ok, records nothing).
        let unsynced = RuleFile::new("local-only", "a.example.com host://127.0.0.1:3000");
        manager.record_deleted_rule(&unsynced).await.unwrap();
        assert!(manager.state.lock().deleted_rules.is_empty());

        // A synced rule records a tombstone.
        let mut synced = RuleFile::new("demo", "a.example.com host://127.0.0.1:3000");
        synced.mark_synced(
            "remote-1",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
        );
        manager.record_deleted_rule(&synced).await.unwrap();
        assert_eq!(manager.state.lock().deleted_rules.len(), 1);

        // Clearing by a non-matching name keeps the tombstone.
        manager.clear_deleted_rule("nope").await.unwrap();
        assert_eq!(manager.state.lock().deleted_rules.len(), 1);
        // Clearing by the rule name removes it.
        manager.clear_deleted_rule("demo").await.unwrap();
        assert!(manager.state.lock().deleted_rules.is_empty());
    }

    #[tokio::test]
    async fn record_deleted_rule_requires_remote_user_id() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let mut rule = RuleFile::new("demo", "a.example.com host://127.0.0.1:3000");
        // Set remote_id but leave remote_user_id None -> error.
        rule.sync.remote_id = Some("remote-1".to_string());
        rule.sync.remote_user_id = None;
        let err = manager.record_deleted_rule(&rule).await.unwrap_err();
        assert!(matches!(err, BifrostError::Config(_)));
    }

    #[tokio::test]
    async fn remote_sample_requires_token_and_user() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        // No token -> Config error.
        assert!(matches!(
            manager.remote_sample(5).await.unwrap_err(),
            BifrostError::Config(_)
        ));
        // Token but no user -> Config error.
        manager.state.lock().token = Some("tk".to_string());
        assert!(matches!(
            manager.remote_sample(5).await.unwrap_err(),
            BifrostError::Config(_)
        ));
    }

    #[tokio::test]
    async fn remote_sample_returns_sorted_truncated_envs() {
        let api = FakeApi {
            envs: vec![
                re("e1", "alpha", "r1", "2026-01-01T00:00:00Z"),
                re("e2", "beta", "r2", "2026-03-01T00:00:00Z"),
                re("e3", "gamma", "r3", "2026-02-01T00:00:00Z"),
            ],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, _cm, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("tk".to_string());
        manager.state.lock().user = Some(RemoteUser {
            user_id: "user-1".to_string(),
            ..RemoteUser::default()
        });

        let sample = manager.remote_sample(2).await.unwrap();
        assert_eq!(sample.len(), 2);
        // Newest update_time first.
        assert_eq!(sample[0].id, "e2");
        assert_eq!(sample[1].id, "e3");
    }

    #[tokio::test]
    async fn proxy_forward_requires_token() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        let err = manager
            .proxy_forward(reqwest::Method::GET, "/v4/env", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Config(_)));
    }

    #[tokio::test]
    async fn proxy_forward_passes_through_to_remote() {
        let (base, _hits) = spawn_api_server(FakeApi::default()).await;
        let (_temp, _cm, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("tk".to_string());
        let (status, content_type, body) = manager
            .proxy_forward(reqwest::Method::GET, "/v4/env", Some("a=1"), None)
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert!(content_type.contains("application/json"));
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn logout_clears_session_and_runtime() {
        let (base, _hits) = spawn_api_server(FakeApi::default()).await;
        let (_temp, _cm, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("tk".to_string());
        manager.state.lock().user = Some(RemoteUser::default());
        {
            let mut rt = manager.runtime.write().await;
            rt.authorized = true;
        }
        manager.logout().await.unwrap();
        assert!(manager.state.lock().token.is_none());
        assert!(manager.state.lock().user.is_none());
        let rt = manager.runtime.read().await;
        assert!(!rt.authorized);
        assert_eq!(rt.reason, SyncReason::Unauthorized);
    }

    #[tokio::test]
    async fn sync_once_returns_disabled_when_sync_off() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        // Explicitly disable sync (it defaults to enabled).
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("disabled"));
    }

    #[tokio::test]
    async fn sync_once_returns_unreachable_when_remote_down() {
        let (_temp, _cm, manager) = sync_manager_for_remote("http://192.0.2.1:9").await;
        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("unreachable"));
    }

    #[tokio::test]
    async fn sync_once_requires_session_token() {
        let (base, _hits) = spawn_api_server(FakeApi {
            user_id: Some("user-1".to_string()),
            ..FakeApi::default()
        })
        .await;
        let (_temp, _cm, manager) = sync_manager_for_remote(&base).await;
        // Reachable but no token.
        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("token"));
    }

    #[tokio::test]
    async fn sync_once_reports_expired_token() {
        // Server reachable; /v4/sso/info returns null data => token invalid.
        let (base, _hits) = spawn_api_server(FakeApi {
            user_id: None,
            ..FakeApi::default()
        })
        .await;
        let (_temp, _cm, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("expired".to_string());
        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("expired") || result.message.contains("invalid"));
    }

    #[tokio::test]
    async fn sync_once_pulls_remote_rule_into_local_storage() {
        let api = FakeApi {
            envs: vec![re(
                "remote-x",
                "pulled-rule",
                "remote.example.com host://127.0.0.1:3000",
                "2026-04-01T00:00:00Z",
            )],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        assert_eq!(result.action, Some(SyncAction::RemotePulled));

        // The remote rule should now exist locally.
        let storage = config_manager.rules_storage().await;
        assert!(storage.exists("pulled-rule"));
    }

    #[tokio::test]
    async fn sync_once_creates_remote_for_local_only_rule() {
        let api = FakeApi {
            envs: vec![],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        // Seed a purely local rule.
        let storage = config_manager.rules_storage().await;
        storage
            .save(&RuleFile::new(
                "local-rule",
                "a.example.com host://127.0.0.1:3000",
            ))
            .unwrap();

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        assert_eq!(result.action, Some(SyncAction::LocalPushed));

        // After push, the local rule is marked synced (remote_id populated).
        let saved = storage.load_all().unwrap();
        let rule = saved.iter().find(|r| r.name == "local-rule").unwrap();
        assert!(rule.sync.remote_id.is_some());
    }

    #[tokio::test]
    async fn sync_once_updates_remote_for_modified_local_rule() {
        // Remote has a matching env; local rule is Modified -> UpdateRemote.
        let api = FakeApi {
            envs: vec![re(
                "remote-x",
                "shared",
                "old.example.com host://127.0.0.1:3000",
                "2026-01-01T00:00:00Z",
            )],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        let storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("shared", "new.example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "remote-x",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        rule.touch_local_change(); // -> Modified
        storage.save(&rule).unwrap();

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        assert_eq!(result.action, Some(SyncAction::LocalPushed));
        // After update the rule is synced again with the returned remote id.
        let saved = storage.load_all().unwrap();
        let updated = saved.iter().find(|r| r.name == "shared").unwrap();
        assert_eq!(updated.sync.status, RuleSyncStatus::Synced);
    }

    #[tokio::test]
    async fn sync_once_updates_local_when_remote_changed() {
        // Local is Synced but the remote update_time/content differ -> UpdateLocal.
        let api = FakeApi {
            envs: vec![re(
                "remote-x",
                "shared",
                "remote-fresh.example.com host://127.0.0.1:3000",
                "2026-09-09T00:00:00Z",
            )],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        let storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("shared", "stale.example.com host://127.0.0.1:3000");
        // Mark synced with an OLD remote update time so remote_changed == true.
        rule.mark_synced(
            "remote-x",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        storage.save(&rule).unwrap();

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        assert_eq!(result.action, Some(SyncAction::RemotePulled));
        let saved = storage.load_all().unwrap();
        let updated = saved.iter().find(|r| r.name == "shared").unwrap();
        assert!(updated.content.contains("remote-fresh.example.com"));
    }

    #[tokio::test]
    async fn sync_once_deletes_local_when_synced_rule_vanishes_from_remote() {
        // Local rule is Synced with a remote_id, but remote returns no envs ->
        // the local copy is deleted directly inside sync_rules.
        let api = FakeApi {
            envs: vec![],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        let storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("ghost", "a.example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "gone-remote",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        storage.save(&rule).unwrap();

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        assert!(!storage.exists("ghost"));
    }

    #[tokio::test]
    async fn sync_once_enforces_tombstone_delete_remote_and_local() {
        // A tombstone for a rule that still exists locally and remotely ->
        // DeleteLocal + DeleteRemote plan steps both execute.
        let api = FakeApi {
            envs: vec![re(
                "tomb-remote",
                "to-delete",
                "x.example.com host://127.0.0.1:3000",
                "2026-01-01T00:00:00Z",
            )],
            user_id: Some("user-1".to_string()),
        };
        let (base, _hits) = spawn_api_server(api).await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good-token".to_string());

        let storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("to-delete", "x.example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "tomb-remote",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        storage.save(&rule).unwrap();

        // Record a tombstone for it.
        manager.record_deleted_rule(&rule).await.unwrap();
        assert_eq!(manager.state.lock().deleted_rules.len(), 1);

        let result = manager.sync_once().await.unwrap();
        assert!(result.success, "message: {}", result.message);
        // Local copy removed via DeleteLocal.
        assert!(!storage.exists("to-delete"));
        assert_eq!(result.action, Some(SyncAction::LocalPushed));
    }

    #[tokio::test]
    async fn tick_marks_disabled_when_sync_off() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Disabled);
        assert!(!runtime.reachable);
        assert!(!runtime.authorized);
        assert!(!runtime.syncing);
    }

    #[tokio::test]
    async fn tick_marks_unauthorized_when_token_missing() {
        let (base, _hits) = spawn_api_server(FakeApi {
            envs: vec![],
            user_id: Some("user-1".to_string()),
        })
        .await;
        let (_temp, _config_manager, manager) = sync_manager_for_remote(&base).await;
        // No token set.
        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Unauthorized);
        assert!(!runtime.authorized);
    }

    #[tokio::test]
    async fn tick_marks_unreachable_when_remote_down() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some("http://192.0.2.1:9".to_string()),
                connect_timeout_ms: Some(500),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        manager.state.lock().token = Some("tk".to_string());

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Unreachable);
        assert!(!runtime.reachable);
    }

    #[tokio::test]
    async fn tick_clears_session_when_user_info_expired() {
        // Server is reachable but /v4/sso/info returns null data → session expired.
        let (base, _hits) = spawn_api_server(FakeApi {
            envs: vec![],
            user_id: None,
        })
        .await;
        let (_temp, _config_manager, manager) = sync_manager_for_remote(&base).await;
        {
            let mut state = manager.state.lock();
            state.token = Some("stale".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-1".to_string(),
                ..RemoteUser::default()
            });
            manager.persist_state(&state).unwrap();
        }

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Unauthorized);
        assert!(runtime.reachable);
        assert!(!runtime.authorized);
        // Session is cleared.
        assert!(manager.state.lock().token.is_none());
        assert!(manager.state.lock().user.is_none());
    }

    #[tokio::test]
    async fn tick_marks_ready_without_sync_when_auto_sync_off() {
        let (base, _hits) = spawn_api_server(FakeApi {
            envs: vec![],
            user_id: Some("user-1".to_string()),
        })
        .await;
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(false),
                remote_base_url: Some(base.clone()),
                connect_timeout_ms: Some(500),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        manager.state.lock().token = Some("good".to_string());

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Ready);
        assert!(runtime.reachable);
        assert!(runtime.authorized);
        assert!(!runtime.syncing);
    }

    #[tokio::test]
    async fn tick_runs_full_sync_and_marks_ready() {
        let (base, _hits) = spawn_api_server(FakeApi {
            envs: vec![re(
                "remote-y",
                "ticked-rule",
                "ticked.example.com host://127.0.0.1:3000",
                "2026-04-01T00:00:00Z",
            )],
            user_id: Some("user-1".to_string()),
        })
        .await;
        let (_temp, config_manager, manager) = sync_manager_for_remote(&base).await;
        manager.state.lock().token = Some("good".to_string());

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Ready);
        assert!(runtime.authorized);
        assert!(!runtime.syncing);
        assert!(runtime.last_error.is_none());
        drop(runtime);

        // The remote rule was pulled during the sync.
        let storage = config_manager.rules_storage().await;
        assert!(storage.exists("ticked-rule"));
    }

    #[test]
    fn startup_login_preflight_retry_delay_reads_env() {
        let _guard = EnvVarGuard::set(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV, "250");
        assert_eq!(
            startup_login_preflight_retry_delay(),
            Duration::from_millis(250)
        );
        let _guard2 = EnvVarGuard::unset(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV);
        assert_eq!(
            startup_login_preflight_retry_delay(),
            Duration::from_secs(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_SECS)
        );
    }

    #[test]
    fn open_url_in_browser_dry_run_appends_to_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("urls.txt");
        let _guard = EnvVarGuard::set(LOGIN_BROWSER_DRY_RUN_FILE_ENV, path.to_str().unwrap());
        open_url_in_browser("http://example.test/login").unwrap();
        open_url_in_browser("http://example.test/again").unwrap();
        let urls = read_dry_run_urls(&path);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "http://example.test/login");
    }

    #[tokio::test]
    async fn new_loads_existing_state_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        // First manager writes a state file with a token.
        let m1 = SyncManager::new(config_manager.clone(), 9900).unwrap();
        m1.state.lock().token = Some("persisted".to_string());
        {
            let state = m1.state.lock().clone();
            m1.persist_state(&state).unwrap();
        }
        // Second manager loads it back from disk.
        let m2 = SyncManager::new(config_manager, 9900).unwrap();
        assert_eq!(m2.session_token().as_deref(), Some("persisted"));
    }

    #[tokio::test]
    async fn startup_login_preflight_skips_when_session_token_exists() {
        let _env_lock = env_lock().lock().await;
        let _disable_guard = EnvVarGuard::unset(DISABLE_AUTO_LOGIN_PROMPT_ENV);
        let (remote_base_url, hits) = spawn_sso_check_server(vec![200]).await;
        let (temp_dir, _config_manager, manager) = sync_manager_for_remote(&remote_base_url).await;
        {
            let mut state = manager.state.lock();
            state.token = Some("existing-token".to_string());
            manager.persist_state(&state).unwrap();
        }
        let dry_run_file = temp_dir.path().join("opened-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );

        manager
            .startup_login_preflight_with_delay(Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(read_dry_run_urls(&dry_run_file).is_empty());
        assert!(manager.state.lock().startup_login_prompt.is_none());
    }

    #[test]
    fn session_helpers_expose_state_from_mutex() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        assert!(!manager.has_session());
        assert_eq!(manager.session_token(), None);
        assert_eq!(manager.current_user_id(), None);

        {
            let mut state = manager.state.lock();
            state.token = Some("session-token".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-1".to_string(),
                nickname: "nick".to_string(),
                avatar: String::new(),
                email: "user-1@example.test".to_string(),
            });
        }

        assert!(manager.has_session());
        assert_eq!(manager.session_token().as_deref(), Some("session-token"));
        assert_eq!(manager.current_user_id().as_deref(), Some("user-1"));
    }

    #[tokio::test]
    async fn status_reflects_runtime_and_state() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(false),
                remote_base_url: Some("https://status.example.test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager.clone(), 9900).unwrap();

        {
            let mut state = manager.state.lock();
            state.token = Some("status-token".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-42".to_string(),
                nickname: "status-nick".to_string(),
                avatar: String::new(),
                email: "user-42@example.test".to_string(),
            });
            state.last_sync_at = Some("2026-06-12T00:00:00Z".to_string());
            state.last_sync_action = Some(SyncAction::RemotePulled);
        }

        {
            let mut runtime = manager.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = true;
            runtime.syncing = false;
            runtime.reason = SyncReason::Ready;
            runtime.last_error = Some("previous error".to_string());
        }

        let status = manager.status().await;
        assert!(status.enabled);
        assert!(!status.auto_sync);
        assert_eq!(status.remote_base_url, "https://status.example.test");
        assert!(status.has_session);
        assert!(status.reachable);
        assert!(status.authorized);
        assert!(!status.syncing);
        assert_eq!(status.reason, SyncReason::Ready);
        assert_eq!(status.last_sync_at.as_deref(), Some("2026-06-12T00:00:00Z"));
        assert_eq!(status.last_sync_action, Some(SyncAction::RemotePulled));
        assert_eq!(status.last_error.as_deref(), Some("previous error"));
        assert_eq!(status.user.unwrap().user_id, "user-42");
    }

    #[tokio::test]
    async fn record_and_clear_deleted_rules_manage_tombstones() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let mut rule = RuleFile::new("demo", "rule-content");
        rule.mark_synced(
            "remote-id-1",
            "remote-user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );

        manager.record_deleted_rule(&rule).await.unwrap();

        {
            let state = manager.state.lock();
            assert_eq!(state.deleted_rules.len(), 1);
            let tombstone = state.deleted_rules.values().next().unwrap();
            assert_eq!(tombstone.rule_id, rule.sync.rule_id);
            assert_eq!(tombstone.rule_name, "demo");
            assert_eq!(tombstone.remote_id, "remote-id-1");
            assert_eq!(tombstone.remote_user_id, "remote-user-1");
        }

        manager.clear_deleted_rule("demo").await.unwrap();
        assert!(manager.state.lock().deleted_rules.is_empty());

        // A rule without remote_id should be ignored.
        let local_only = RuleFile::new("local-only", "content");
        manager.record_deleted_rule(&local_only).await.unwrap();
        assert!(manager.state.lock().deleted_rules.is_empty());
    }

    #[tokio::test]
    async fn sync_once_returns_disabled_when_sync_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(result.action.is_none());
        assert!(result.user.is_none());
        assert_eq!(result.local_rules, 0);
        assert_eq!(result.remote_rules, 0);
        assert_eq!(result.message, "Sync is disabled in configuration");
    }

    #[tokio::test]
    async fn sync_once_reports_unreachable_remote() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        // No mock for /v4/sso/check -> 404 -> unreachable.
        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert!(
            result.message.starts_with("Remote server unreachable: "),
            "unexpected message: {}",
            result.message
        );
        assert!(result.user.is_none());
    }

    #[tokio::test]
    async fn sync_once_reports_missing_token_when_remote_reachable() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert_eq!(
            result.message,
            "No sync session token. Please login first via the admin UI."
        );
        assert!(result.user.is_none());
    }

    #[tokio::test]
    async fn sync_once_reports_invalid_token_when_user_info_missing() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("expired-token".to_string());
        }

        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v4/sso/info"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = manager.sync_once().await.unwrap();
        assert!(!result.success);
        assert_eq!(
            result.message,
            "Token expired or invalid. Please re-login via the admin UI."
        );
        assert!(result.user.is_none());
    }

    #[tokio::test]
    async fn sync_once_performs_successful_sync_with_local_rule() {
        let server = MockServer::start().await;
        let (_temp_dir, config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        // Prepare one local rule.
        let rules_storage = config_manager.rules_storage().await;
        let rule = RuleFile::new("demo", "example.com proxy://localhost:3000");
        rules_storage.save(&rule).unwrap();

        // Session token.
        {
            let mut state = manager.state.lock();
            state.token = Some("session-token".to_string());
        }

        // Remote is reachable.
        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // User info.
        let user_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "user_id": "user-1",
                "nickname": "nick",
                "avatar": "",
                "email": "user-1@example.test"
            }
        });

        Mock::given(method("GET"))
            .and(path("/v4/sso/info"))
            .and(header("x-bifrost-token", "session-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user_body))
            .mount(&server)
            .await;

        // Empty remote env list.
        let list_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": { "list": [] }
        });

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(list_body))
            .mount(&server)
            .await;

        // Create remote env.
        let created_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "id": 100,
                "user_id": "user-1",
                "name": "demo",
                "rule": "example.com proxy://localhost:3000",
                "create_time": "2026-01-01T00:00:00Z",
                "update_time": "2026-01-01T00:00:00Z"
            }
        });

        Mock::given(method("POST"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(created_body))
            .mount(&server)
            .await;

        let result = manager.sync_once().await.unwrap();
        assert!(result.success);
        assert_eq!(result.message, "Sync completed successfully");
        assert_eq!(result.local_rules, 1);
        assert_eq!(result.remote_rules, 1);
        assert_eq!(result.action, Some(SyncAction::LocalPushed));
        assert_eq!(result.user.unwrap().user_id, "user-1");
    }

    #[tokio::test]
    async fn tick_sets_reason_disabled_when_sync_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert!(!runtime.reachable);
        assert!(!runtime.authorized);
        assert!(!runtime.syncing);
        assert_eq!(runtime.reason, SyncReason::Disabled);
        assert!(runtime.last_error.is_none());
    }

    #[tokio::test]
    async fn tick_sets_reason_unauthorized_when_missing_token() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Unauthorized);
        assert!(!runtime.authorized);
        assert!(!runtime.syncing);
        assert!(runtime.last_error.is_none());
    }

    #[tokio::test]
    async fn tick_sets_reason_unreachable_when_remote_unreachable() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("token".to_string());
        }

        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        manager.tick().await.unwrap();

        let runtime = manager.runtime.read().await;
        assert_eq!(runtime.reason, SyncReason::Unreachable);
        assert!(!runtime.authorized);
        assert!(!runtime.syncing);
        assert!(runtime.last_error.is_none());
    }

    #[tokio::test]
    async fn tick_clears_session_when_user_info_returns_none() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("token".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-1".to_string(),
                nickname: String::new(),
                avatar: String::new(),
                email: String::new(),
            });
        }

        {
            let mut runtime = manager.runtime.write().await;
            runtime.authorized = true;
        }

        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v4/sso/info"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        manager.tick().await.unwrap();

        {
            let state = manager.state.lock();
            assert!(state.token.is_none());
            assert!(state.user.is_none());
        }

        let runtime = manager.runtime.read().await;
        assert!(runtime.reachable);
        assert!(!runtime.authorized);
        assert!(!runtime.syncing);
        assert_eq!(runtime.reason, SyncReason::Unauthorized);
        assert!(runtime.last_error.is_none());
    }

    #[tokio::test]
    async fn sync_rules_updates_local_when_remote_changed() {
        let server = MockServer::start().await;
        let (_temp_dir, config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        // Local rule is already synced to a remote env.
        let rules_storage = config_manager.rules_storage().await;
        let mut local_rule = RuleFile::new("sync-me", "old.example.test proxy://localhost:3000");
        local_rule.mark_synced(
            "env-1",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        rules_storage.save(&local_rule).unwrap();

        let user = RemoteUser {
            user_id: "user-1".to_string(),
            ..Default::default()
        };
        let token = "token-sync";

        let config = config_manager.config().await;
        let sync_config = config.sync.clone();
        let client = SyncHttpClient::new(&sync_config).unwrap();

        // Remote env has a newer update_time and different rule content.
        let env_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "list": [
                    {
                        "id": "env-1",
                        "user_id": "user-1",
                        "name": "sync-me",
                        "rule": "new.example.test proxy://localhost:3100",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-02T00:00:00Z"
                    }
                ]
            }
        });

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(env_body))
            .mount(&server)
            .await;

        manager
            .sync_rules(&client, &sync_config, token, &user)
            .await
            .unwrap();

        let updated_rules = rules_storage.load_all().unwrap();
        assert_eq!(updated_rules.len(), 1);
        let updated = &updated_rules[0];
        assert_eq!(updated.name, "sync-me");
        assert!(updated.content.contains("new.example.test"));

        let state = manager.state.lock();
        assert_eq!(state.last_sync_action, Some(SyncAction::RemotePulled));
    }

    #[tokio::test]
    async fn sync_rules_filters_global_default_from_local_and_remote() {
        let server = MockServer::start().await;
        let (_temp_dir, config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        let rules_storage = config_manager.rules_storage().await;
        let default_rule = rules_storage.ensure_default_rule().unwrap();
        assert_eq!(default_rule.name, bifrost_storage::DEFAULT_RULE_NAME);

        let user = RemoteUser {
            user_id: "user-1".to_string(),
            ..Default::default()
        };
        let token = "token-sync";

        let config = config_manager.config().await;
        let sync_config = config.sync.clone();
        let client = SyncHttpClient::new(&sync_config).unwrap();

        let env_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "list": [
                    {
                        "id": "env-default",
                        "user_id": "user-1",
                        "name": "default",
                        "rule": "remote-default.example.test status://218",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-02T00:00:00Z"
                    }
                ]
            }
        });

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(env_body))
            .mount(&server)
            .await;

        manager
            .sync_rules(&client, &sync_config, token, &user)
            .await
            .unwrap();

        let default_rule = rules_storage
            .load(bifrost_storage::DEFAULT_RULE_NAME)
            .unwrap();
        assert!(default_rule.sync.remote_id.is_none());
        assert!(!default_rule.content.contains("remote-default.example.test"));
        assert_eq!(rules_storage.load_all().unwrap().len(), 1);

        let state = manager.state.lock();
        assert_eq!(state.last_sync_action, Some(SyncAction::NoChange));
    }

    #[tokio::test]
    async fn sync_rules_enforces_and_clears_tombstones() {
        let server = MockServer::start().await;
        let (_temp_dir, config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        let rules_storage = config_manager.rules_storage().await;
        let mut rule = RuleFile::new("tombstoned", "example.com proxy://localhost:3000");
        rule.mark_synced(
            "env-2",
            "user-1",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        rules_storage.save(&rule).unwrap();

        {
            let mut state = manager.state.lock();
            state.deleted_rules.insert(
                "rule-id-1".to_string(),
                DeletedRuleTombstone {
                    rule_id: "rule-id-1".to_string(),
                    rule_name: "tombstoned".to_string(),
                    remote_id: "env-2".to_string(),
                    remote_user_id: "user-1".to_string(),
                    base_remote_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
                    base_content_hash: None,
                    deleted_at: "2000-01-01T00:00:00Z".to_string(),
                },
            );
        }

        let user = RemoteUser {
            user_id: "user-1".to_string(),
            ..Default::default()
        };
        let token = "token-tombstone";

        let config = config_manager.config().await;
        let sync_config = config.sync.clone();
        let client = SyncHttpClient::new(&sync_config).unwrap();

        let env_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "list": [
                    {
                        "id": "env-2",
                        "user_id": "user-1",
                        "name": "tombstoned",
                        "rule": "remote-content",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-01T00:00:00Z"
                    }
                ]
            }
        });

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(env_body))
            .mount(&server)
            .await;

        let delete_body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {}
        });

        Mock::given(method("DELETE"))
            .and(path("/v4/env/env-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(delete_body))
            .mount(&server)
            .await;

        manager
            .sync_rules(&client, &sync_config, token, &user)
            .await
            .unwrap();

        assert!(rules_storage.load_all().unwrap().is_empty());
        let state = manager.state.lock();
        assert!(state.deleted_rules.is_empty());
        assert_eq!(state.last_sync_action, Some(SyncAction::LocalPushed));
    }

    #[tokio::test]
    async fn remote_sample_returns_error_when_session_token_missing() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let err = manager.remote_sample(10).await.unwrap_err();
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("sync session token missing"));
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remote_sample_returns_error_when_user_missing() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        {
            let mut state = manager.state.lock();
            state.token = Some("token".to_string());
        }

        let err = manager.remote_sample(10).await.unwrap_err();
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("sync user missing"));
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remote_sample_sorts_and_truncates_envs() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("session-token".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-1".to_string(),
                ..Default::default()
            });
        }

        let body = serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": {
                "list": [
                    {
                        "id": "env-old",
                        "user_id": "user-1",
                        "name": "old",
                        "rule": "rule-old",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-01T00:00:00Z"
                    },
                    {
                        "id": "env-new",
                        "user_id": "user-1",
                        "name": "new",
                        "rule": "rule-new",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-02T00:00:00Z"
                    }
                ]
            }
        });

        Mock::given(method("GET"))
            .and(path("/v4/env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let envs = manager.remote_sample(0).await.unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "env-new");
    }

    #[tokio::test]
    async fn proxy_forward_returns_error_when_session_token_missing() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let err = manager
            .proxy_forward(reqwest::Method::GET, "/proxy/test", None, None)
            .await
            .unwrap_err();
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("sync session token missing"));
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn proxy_forward_forwards_requests_using_session_token() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("proxy-token".to_string());
        }

        Mock::given(method("POST"))
            .and(path("/proxy/through"))
            .and(header("x-bifrost-token", "proxy-token"))
            .respond_with(ResponseTemplate::new(202).set_body_raw("proxied-body", "text/plain"))
            .mount(&server)
            .await;

        let (status, content_type, body) = manager
            .proxy_forward(
                reqwest::Method::POST,
                "/proxy/through",
                Some("q=1"),
                Some(b"body".to_vec()),
            )
            .await
            .unwrap();

        assert_eq!(status, 202);
        assert_eq!(content_type, "text/plain");
        assert_eq!(body, b"proxied-body");
    }

    #[tokio::test]
    async fn logout_clears_state_and_marks_runtime_unauthorized() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        {
            let mut state = manager.state.lock();
            state.token = None;
            state.user = Some(RemoteUser {
                user_id: "user-1".to_string(),
                ..Default::default()
            });
        }
        manager.login_prompt.lock().last_opened_at = Some(Utc::now());
        {
            let mut runtime = manager.runtime.write().await;
            runtime.reachable = true;
            runtime.authorized = true;
            runtime.reason = SyncReason::Ready;
            runtime.last_error = Some("some error".to_string());
        }

        manager.logout().await.unwrap();

        {
            let state = manager.state.lock();
            assert!(state.token.is_none());
            assert!(state.user.is_none());
        }
        assert!(manager.login_prompt.lock().last_opened_at.is_none());
        let runtime = manager.runtime.read().await;
        assert!(!runtime.authorized);
        assert_eq!(runtime.reason, SyncReason::Unauthorized);
        assert!(runtime.last_error.is_none());
    }

    #[tokio::test]
    async fn logout_sends_remote_logout_when_token_exists() {
        let server = MockServer::start().await;
        let (_temp_dir, _config_manager, manager) = sync_manager_for_remote(&server.uri()).await;

        {
            let mut state = manager.state.lock();
            state.token = Some("logout-token".to_string());
            state.user = Some(RemoteUser {
                user_id: "user-logout".to_string(),
                ..Default::default()
            });
        }

        Mock::given(method("GET"))
            .and(path("/v4/sso/logout"))
            .and(header("x-bifrost-token", "logout-token"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        manager.logout().await.unwrap();

        let state = manager.state.lock();
        assert!(state.token.is_none());
        assert!(state.user.is_none());
    }

    #[tokio::test]
    async fn login_url_uses_sync_config_remote_base_url() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some("https://login.example.test/".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();

        let callback = "http://127.0.0.1:9900/callback";
        let url = manager.login_url(callback).await.unwrap();
        assert!(url.starts_with("https://login.example.test/v4/sso/login?next="));
        assert!(url.contains("callback"));
    }

    #[tokio::test]
    async fn request_login_opens_browser_even_if_prompt_already_opened() {
        let _env_lock = env_lock().lock().await;
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some("https://login.example.test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager, 9900).unwrap();
        let dry_run_file = temp_dir.path().join("request-login-urls.txt");
        let _dry_run_guard = EnvVarGuard::set(
            LOGIN_BROWSER_DRY_RUN_FILE_ENV,
            dry_run_file.to_str().unwrap(),
        );

        manager.login_prompt.lock().last_opened_at = Some(Utc::now());

        manager.request_login().await.unwrap();

        let opened = read_dry_run_urls(&dry_run_file);
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains("/v4/sso/logout?next="));
    }

    #[tokio::test]
    async fn sync_manager_handle_delegates_to_inner_methods() {
        let temp_dir = TempDir::new().unwrap();
        let config_manager = Arc::new(ConfigManager::new(temp_dir.path().to_path_buf()).unwrap());
        config_manager
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                remote_base_url: Some("https://handle.example.test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = Arc::new(SyncManager::new(config_manager.clone(), 9900).unwrap());
        let handle = SyncManagerHandle::new(manager.clone());

        let status = handle.status().await;
        assert!(!status.has_session);

        handle.save_token("handle-token".to_string()).await.unwrap();
        let status = handle.status().await;
        assert!(status.has_session);
        assert_eq!(handle.session_token().as_deref(), Some("handle-token"));

        let status_after_logout = handle.logout().await.unwrap();
        assert!(!status_after_logout.has_session);
        assert!(!handle.status().await.has_session);

        let err = handle.remote_sample(5).await.unwrap_err();
        match err {
            BifrostError::Config(_) => {}
            other => panic!("expected config error, got {other:?}"),
        }

        let rule = RuleFile::new("handle-demo", "example.com proxy://localhost:3000");
        handle.record_deleted_rule(&rule).await.unwrap();
        handle.clear_deleted_rule("handle-demo").await.unwrap();

        let err = handle
            .proxy_forward(reqwest::Method::GET, "/proxy/test", None, None)
            .await
            .unwrap_err();
        match err {
            BifrostError::Config(_) => {}
            other => panic!("expected config error, got {other:?}"),
        }

        handle.trigger_sync();

        let url = handle.login_url("http://127.0.0.1:9900/cb").await.unwrap();
        assert!(url.contains("/v4/sso/login"));
    }
}
