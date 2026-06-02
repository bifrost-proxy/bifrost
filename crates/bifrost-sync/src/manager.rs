use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bifrost_core::{BifrostError, Result};
use bifrost_storage::{
    content_hash, ConfigChangeEvent, ConfigManager, RuleFile, RuleSyncStatus, RulesStorage,
    SyncConfig, SyncConfigUpdate,
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify, RwLock};

use crate::client::SyncHttpClient;
use crate::normalize::normalize_remote_rule;
use crate::types::{RemoteEnv, RemoteUser, SyncReason};

const TOMBSTONE_MAX_AGE_SECS: i64 = 7 * 24 * 3600;
const TOMBSTONE_MIN_AGE_SECS: i64 = 120;
const STARTUP_LOGIN_PREFLIGHT_MAX_ATTEMPTS: usize = 3;
const STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_SECS: u64 = 15;
const STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV: &str =
    "BIFROST_SYNC_STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS";
const LOGIN_BROWSER_DRY_RUN_FILE_ENV: &str = "BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE";

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
struct SyncStateFile {
    token: Option<String>,
    user: Option<RemoteUser>,
    last_sync_at: Option<String>,
    last_sync_action: Option<SyncAction>,
    startup_login_prompt: Option<StartupLoginPromptFile>,
    deleted_rules: HashMap<String, DeletedRuleTombstone>,
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
        let has_session = state
            .token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty());
        SyncStatus {
            enabled: config.sync.enabled,
            auto_sync: config.sync.auto_sync,
            remote_base_url: config.sync.remote_base_url,
            has_session,
            reachable: runtime.reachable,
            authorized: runtime.authorized,
            syncing: runtime.syncing,
            reason: runtime.reason,
            last_sync_at: state.last_sync_at,
            last_sync_action: state.last_sync_action,
            last_error: runtime.last_error,
            user: state.user,
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
        self.config_manager
            .update_sync_config(SyncConfigUpdate {
                auto_sync: Some(true),
                ..Default::default()
            })
            .await?;
        {
            let mut state = self.state.lock();
            state.token = Some(token);
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
        if !(remote_base_url.starts_with("http://") || remote_base_url.starts_with("https://")) {
            return Err(BifrostError::Config(
                "remote_base_url must start with http:// or https://".to_string(),
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

        let client = SyncHttpClient::new(&config.sync)?;
        let reachable = client.probe_reachable(&config.sync).await;
        if !reachable {
            return Ok(SyncOnceResult {
                success: false,
                message: format!("Remote server unreachable: {}", config.sync.remote_base_url),
                action: None,
                user: None,
                local_rules: 0,
                remote_rules: 0,
            });
        }

        let token = { self.state.lock().token.clone() };
        if token.as_deref().unwrap_or("").is_empty() {
            return Ok(SyncOnceResult {
                success: false,
                message: "No sync session token. Please login first via the admin UI.".to_string(),
                action: None,
                user: None,
                local_rules: 0,
                remote_rules: 0,
            });
        }

        let token = token.unwrap_or_default();
        let user = client.get_user_info(&config.sync, &token).await?;
        let Some(user) = user else {
            return Ok(SyncOnceResult {
                success: false,
                message: "Token expired or invalid. Please re-login via the admin UI.".to_string(),
                action: None,
                user: None,
                local_rules: 0,
                remote_rules: 0,
            });
        };

        {
            let mut state = self.state.lock();
            state.user = Some(user.clone());
            self.persist_state(&state)?;
        }

        let rules_storage = self.config_manager.rules_storage().await;
        let local_count = rules_storage.load_all()?.len();

        let result = self.sync_rules(&client, &config.sync, &token, &user).await;
        let state = self.state.lock().clone();
        let synced_count = rules_storage
            .load_all()?
            .iter()
            .filter(|rule| rule.sync.remote_id.is_some())
            .count();

        match result {
            Ok(()) => Ok(SyncOnceResult {
                success: true,
                message: "Sync completed successfully".to_string(),
                action: state.last_sync_action,
                user: Some(user),
                local_rules: local_count,
                remote_rules: synced_count,
            }),
            Err(error) => Ok(SyncOnceResult {
                success: false,
                message: format!("Sync failed: {error}"),
                action: None,
                user: Some(user),
                local_rules: local_count,
                remote_rules: 0,
            }),
        }
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
                        Ok(ConfigChangeEvent::RulesChanged | ConfigChangeEvent::SyncConfigChanged) => {}
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
                state.user = None;
                state.token = None;
                self.persist_state(&state)?;
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
                let _ = self.config_manager.notify(ConfigChangeEvent::RulesChanged);
            }
            return Ok(());
        };

        {
            let mut state = self.state.lock();
            state.user = Some(user.clone());
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

        let result = self.sync_rules(&client, &config.sync, &token, &user).await;
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
        let local_rules = rules_storage.load_all()?;
        let remote_rules = client.search_envs(config, token, &user.user_id).await?;
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
            .map(|t| t.remote_id.clone())
            .collect();
        let tombstone_names: HashSet<String> = state_snapshot
            .deleted_rules
            .values()
            .map(|t| t.rule_name.clone())
            .collect();

        let mut plan = Vec::new();
        let mut blocked_remote_ids: HashSet<String> = HashSet::new();
        let mut blocked_names: HashSet<String> = HashSet::new();

        for tombstone in state_snapshot.deleted_rules.values() {
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

            let remote_env = local_rule
                .sync
                .remote_id
                .as_deref()
                .and_then(|remote_id| remote_by_id.get(remote_id).copied())
                .or_else(|| {
                    if local_rule.sync.remote_id.is_some() {
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
            } else if local_rule.sync.remote_id.is_some()
                && local_rule.sync.status == RuleSyncStatus::Synced
            {
                tracing::debug!(
                    target: "bifrost_sync::manager",
                    name = %local_rule.name,
                    remote_id = ?local_rule.sync.remote_id,
                    "synced rule disappeared from remote, deleting local copy"
                );
                rules_storage.delete(&local_rule.name)?;
            } else if local_rule.sync.remote_id.is_some()
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
        current_state.last_sync_at = Some(now.to_rfc3339());
        let sync_action = match (pushed_local, pulled_remote) {
            (true, true) => SyncAction::Bidirectional,
            (true, false) => SyncAction::LocalPushed,
            (false, true) => SyncAction::RemotePulled,
            (false, false) => SyncAction::NoChange,
        };
        current_state.last_sync_action = Some(sync_action);
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
            let _ = self.config_manager.notify(ConfigChangeEvent::RulesChanged);
        }

        Ok(())
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

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    command
        .spawn()
        .map_err(|error| BifrostError::Network(format!("failed to open login browser: {error}")))?;
    Ok(())
}

fn startup_login_preflight_retry_delay() -> Duration {
    std::env::var(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_SECS))
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
                connect_timeout_ms: Some(500),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config_manager.clone(), 9900).unwrap();
        (temp_dir, config_manager, manager)
    }

    fn read_dry_run_urls(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
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
    }

    #[tokio::test]
    async fn startup_login_preflight_opens_once_when_third_probe_is_reachable() {
        let _env_lock = env_lock().lock().await;
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
    async fn startup_login_preflight_wake_interrupts_retry_wait() {
        let _env_lock = env_lock().lock().await;
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

    #[tokio::test]
    async fn startup_login_preflight_skips_when_session_token_exists() {
        let _env_lock = env_lock().lock().await;
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
}
