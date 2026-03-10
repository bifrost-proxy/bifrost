use std::path::{Path, PathBuf};
use std::sync::Arc;

use bifrost_core::{BifrostError, Result};
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use crate::rules::{RuleFile, RulesStorage};
use crate::state::StateManager;
use crate::unified_config::{
    AccessConfigUpdate, SystemProxyConfigUpdate, TlsConfig, TlsConfigUpdate, TrafficConfig,
    TrafficConfigUpdate, UiConfig, UiConfigUpdate, UnifiedConfig,
};
use crate::values::ValuesStorage;
use crate::LegacyBifrostConfig;

pub type SharedConfigManager = Arc<ConfigManager>;

#[derive(Debug, Clone)]
pub enum ConfigChangeEvent {
    TlsConfigChanged(TlsConfig),
    AccessConfigChanged,
    SystemProxyConfigChanged,
    RulesChanged,
    ValuesChanged(String),
    StateChanged,
}

pub struct ConfigManager {
    data_dir: PathBuf,
    config: RwLock<UnifiedConfig>,
    rules_storage: RwLock<RulesStorage>,
    values_storage: RwLock<ValuesStorage>,
    state_manager: RwLock<StateManager>,
    change_notifier: broadcast::Sender<ConfigChangeEvent>,
}

impl ConfigManager {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        Self::init_data_dir(&data_dir)?;

        let config = Self::load_config_with_migration(&data_dir)?;
        let rules_dir = data_dir.join("rules");
        let values_dir = data_dir.join("values");
        let rules_storage = RulesStorage::with_dir(rules_dir)?;
        let values_storage = ValuesStorage::with_dir(values_dir)?;
        let state_manager = StateManager::with_file(data_dir.join("state.json"))?;

        let (change_notifier, _) = broadcast::channel(100);

        Ok(Self {
            data_dir,
            config: RwLock::new(config),
            rules_storage: RwLock::new(rules_storage),
            values_storage: RwLock::new(values_storage),
            state_manager: RwLock::new(state_manager),
            change_notifier,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub async fn config(&self) -> UnifiedConfig {
        self.config.read().await.clone()
    }

    pub async fn update_config<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut UnifiedConfig),
    {
        let mut config = self.config.write().await;
        f(&mut config);
        self.save_config(&config)?;
        Ok(())
    }

    pub async fn update_tls_config(&self, update: TlsConfigUpdate) -> Result<TlsConfig> {
        let mut config = self.config.write().await;

        if let Some(enable) = update.enable_interception {
            config.tls.enable_interception = enable;
        }
        if let Some(exclude) = update.intercept_exclude {
            config.tls.intercept_exclude = exclude;
        }
        if let Some(include) = update.intercept_include {
            config.tls.intercept_include = include;
        }
        if let Some(app_exclude) = update.app_intercept_exclude {
            config.tls.app_intercept_exclude = app_exclude;
        }
        if let Some(app_include) = update.app_intercept_include {
            config.tls.app_intercept_include = app_include;
        }
        if let Some(unsafe_ssl) = update.unsafe_ssl {
            config.tls.unsafe_ssl = unsafe_ssl;
        }
        if let Some(disconnect) = update.disconnect_on_change {
            config.tls.disconnect_on_change = disconnect;
        }

        self.save_config(&config)?;

        let tls_config = config.tls.clone();
        let _ = self
            .change_notifier
            .send(ConfigChangeEvent::TlsConfigChanged(tls_config.clone()));

        Ok(tls_config)
    }

    pub async fn update_access_config(&self, update: AccessConfigUpdate) -> Result<()> {
        let mut config = self.config.write().await;

        if let Some(mode) = update.mode {
            config.access.mode = mode;
        }
        if let Some(whitelist) = update.whitelist {
            config.access.whitelist = whitelist;
        }
        if let Some(allow_lan) = update.allow_lan {
            config.access.allow_lan = allow_lan;
        }

        self.save_config(&config)?;
        let _ = self
            .change_notifier
            .send(ConfigChangeEvent::AccessConfigChanged);

        Ok(())
    }

    pub async fn update_system_proxy_config(&self, update: SystemProxyConfigUpdate) -> Result<()> {
        let mut config = self.config.write().await;

        if let Some(enabled) = update.enabled {
            config.system_proxy.enabled = enabled;
        }
        if let Some(bypass) = update.bypass {
            config.system_proxy.bypass = bypass;
        }
        if let Some(auto_enable) = update.auto_enable {
            config.system_proxy.auto_enable = auto_enable;
        }

        self.save_config(&config)?;
        let _ = self
            .change_notifier
            .send(ConfigChangeEvent::SystemProxyConfigChanged);

        Ok(())
    }

    pub async fn update_traffic_config(
        &self,
        update: TrafficConfigUpdate,
    ) -> Result<TrafficConfig> {
        let mut config = self.config.write().await;

        if let Some(max_records) = update.max_records {
            config.traffic.max_records = max_records;
        }
        if let Some(max_db_size_bytes) = update.max_db_size_bytes {
            config.traffic.max_db_size_bytes = max_db_size_bytes;
        }
        if let Some(max_body_memory_size) = update.max_body_memory_size {
            config.traffic.max_body_memory_size = max_body_memory_size;
        }
        if let Some(max_body_buffer_size) = update.max_body_buffer_size {
            config.traffic.max_body_buffer_size = max_body_buffer_size;
        }
        if let Some(max_body_probe_size) = update.max_body_probe_size {
            config.traffic.max_body_probe_size = max_body_probe_size;
        }
        if let Some(enable_body_index) = update.enable_body_index {
            config.traffic.enable_body_index = enable_body_index;
        }
        if let Some(file_retention_days) = update.file_retention_days {
            config.traffic.file_retention_days = file_retention_days;
        }
        if let Some(sse_stream_flush_bytes) = update.sse_stream_flush_bytes {
            config.traffic.sse_stream_flush_bytes = sse_stream_flush_bytes;
        }
        if let Some(sse_stream_flush_interval_ms) = update.sse_stream_flush_interval_ms {
            config.traffic.sse_stream_flush_interval_ms = sse_stream_flush_interval_ms;
        }
        if let Some(ws_payload_flush_bytes) = update.ws_payload_flush_bytes {
            config.traffic.ws_payload_flush_bytes = ws_payload_flush_bytes;
        }
        if let Some(ws_payload_flush_interval_ms) = update.ws_payload_flush_interval_ms {
            config.traffic.ws_payload_flush_interval_ms = ws_payload_flush_interval_ms;
        }
        if let Some(ws_payload_max_open_files) = update.ws_payload_max_open_files {
            config.traffic.ws_payload_max_open_files = ws_payload_max_open_files;
        }

        self.save_config(&config)?;

        Ok(config.traffic.clone())
    }

    pub async fn get_ui_config(&self) -> UiConfig {
        let config = self.config.read().await;
        config.ui.clone()
    }

    pub async fn update_ui_config(&self, update: UiConfigUpdate) -> Result<UiConfig> {
        let mut config = self.config.write().await;

        if let Some(pinned_filters) = update.pinned_filters {
            config.ui.pinned_filters = pinned_filters;
        }
        if let Some(filter_panel) = update.filter_panel {
            config.ui.filter_panel = filter_panel;
        }
        if let Some(detail_panel_collapsed) = update.detail_panel_collapsed {
            config.ui.detail_panel_collapsed = detail_panel_collapsed;
        }

        self.save_config(&config)?;

        Ok(config.ui.clone())
    }

    pub async fn save_rule(&self, rule: &RuleFile) -> Result<()> {
        let storage = self.rules_storage.write().await;
        storage.save(rule)?;
        let _ = self.change_notifier.send(ConfigChangeEvent::RulesChanged);
        Ok(())
    }

    pub async fn load_rule(&self, name: &str) -> Result<RuleFile> {
        let storage = self.rules_storage.read().await;
        storage.load(name)
    }

    pub async fn list_rules(&self) -> Result<Vec<String>> {
        let storage = self.rules_storage.read().await;
        storage.list()
    }

    pub async fn delete_rule(&self, name: &str) -> Result<()> {
        let storage = self.rules_storage.write().await;
        storage.delete(name)?;
        let _ = self.change_notifier.send(ConfigChangeEvent::RulesChanged);
        Ok(())
    }

    pub async fn load_all_rules(&self) -> Result<Vec<RuleFile>> {
        let storage = self.rules_storage.read().await;
        storage.load_all()
    }

    pub async fn load_enabled_rules(&self) -> Result<Vec<RuleFile>> {
        let storage = self.rules_storage.read().await;
        storage.load_enabled()
    }

    pub async fn set_rule_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        let storage = self.rules_storage.write().await;
        storage.set_enabled(name, enabled)?;
        let _ = self.change_notifier.send(ConfigChangeEvent::RulesChanged);
        Ok(())
    }

    pub async fn rules_storage(&self) -> RulesStorage {
        self.rules_storage.read().await.clone()
    }

    pub async fn set_value(&self, key: &str, value: &str) -> Result<()> {
        let mut storage = self.values_storage.write().await;
        storage.set_value(key, value)?;
        let _ = self
            .change_notifier
            .send(ConfigChangeEvent::ValuesChanged(key.to_string()));
        Ok(())
    }

    pub async fn get_value(&self, key: &str) -> Option<String> {
        let storage = self.values_storage.read().await;
        storage.get_value(key)
    }

    pub async fn list_values(&self) -> Vec<(String, String)> {
        use bifrost_core::ValueStore;
        let storage = self.values_storage.read().await;
        storage.list()
    }

    pub async fn delete_value(&self, key: &str) -> Result<()> {
        let mut storage = self.values_storage.write().await;
        storage.remove_value(key)?;
        let _ = self
            .change_notifier
            .send(ConfigChangeEvent::ValuesChanged(key.to_string()));
        Ok(())
    }

    pub async fn values_as_hashmap(&self) -> std::collections::HashMap<String, String> {
        use bifrost_core::ValueStore;
        let storage = self.values_storage.read().await;
        storage.as_hashmap()
    }

    pub async fn values_storage(&self) -> ValuesStorage {
        self.values_storage.read().await.clone()
    }

    pub async fn enable_rule_group(&self, name: &str) -> Result<()> {
        let mut state = self.state_manager.write().await;
        state.enable_group(name);
        state.save()?;
        let _ = self.change_notifier.send(ConfigChangeEvent::StateChanged);
        Ok(())
    }

    pub async fn disable_rule_group(&self, name: &str) -> Result<()> {
        let mut state = self.state_manager.write().await;
        state.disable_group(name);
        state.save()?;
        let _ = self.change_notifier.send(ConfigChangeEvent::StateChanged);
        Ok(())
    }

    pub async fn is_rule_group_enabled(&self, name: &str) -> bool {
        let state = self.state_manager.read().await;
        state.is_group_enabled(name)
    }

    pub async fn enabled_rule_groups(&self) -> Vec<String> {
        let state = self.state_manager.read().await;
        state.enabled_groups()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChangeEvent> {
        self.change_notifier.subscribe()
    }

    pub fn notify(
        &self,
        event: ConfigChangeEvent,
    ) -> std::result::Result<usize, broadcast::error::SendError<ConfigChangeEvent>> {
        self.change_notifier.send(event)
    }

    fn init_data_dir(dir: &Path) -> Result<()> {
        let is_new = !dir.exists();
        std::fs::create_dir_all(dir)?;
        for subdir in ["rules", "values", "certs", "traffic", "body_cache"] {
            std::fs::create_dir_all(dir.join(subdir))?;
        }
        if is_new {
            info!("Initialized data directory: {}", dir.display());
        }
        Ok(())
    }

    fn load_config_with_migration(data_dir: &Path) -> Result<UnifiedConfig> {
        let config_path = data_dir.join("config.toml");

        if !config_path.exists() {
            info!("Creating default configuration: {}", config_path.display());
            let default = UnifiedConfig::default_for_data_dir(data_dir);
            Self::save_config_to_file(&config_path, &default)?;
            return Ok(default);
        }

        let content = std::fs::read_to_string(&config_path)?;

        if let Ok(config) = toml::from_str::<UnifiedConfig>(&content) {
            return Ok(config.with_data_dir(data_dir));
        }

        if let Ok(legacy) = toml::from_str::<LegacyBifrostConfig>(&content) {
            info!("Detected legacy config format, migrating to new format...");
            let new_config = Self::migrate_from_legacy(&legacy, data_dir);

            let backup_path = data_dir.join("config.toml.bak");
            if let Err(e) = std::fs::copy(&config_path, &backup_path) {
                tracing::warn!("Failed to backup old config: {}", e);
            }

            Self::save_config_to_file(&config_path, &new_config)?;
            info!(
                "Config migrated successfully (backup: {})",
                backup_path.display()
            );

            return Ok(new_config);
        }

        Err(BifrostError::Config(
            "Failed to parse config.toml".to_string(),
        ))
    }

    fn migrate_from_legacy(legacy: &LegacyBifrostConfig, data_dir: &Path) -> UnifiedConfig {
        use crate::unified_config::*;

        UnifiedConfig {
            server: ServerConfig {
                socks5_auth: None,
                timeout_secs: 30,
            },
            tls: TlsConfig {
                enable_interception: legacy.enable_tls_interception,
                intercept_exclude: legacy.intercept_exclude.clone(),
                intercept_include: legacy.intercept_include.clone(),
                app_intercept_exclude: Vec::new(),
                app_intercept_include: Vec::new(),
                unsafe_ssl: false,
                disconnect_on_change: legacy.disconnect_on_config_change,
            },
            access: AccessConfig {
                mode: legacy
                    .access
                    .mode
                    .parse()
                    .unwrap_or(bifrost_core::AccessMode::LocalOnly),
                whitelist: legacy.access.whitelist.clone(),
                allow_lan: legacy.access.allow_lan,
            },
            proxy: ProxySettings::default(),
            system_proxy: SystemProxyConfig {
                enabled: legacy.system_proxy.enabled,
                bypass: legacy.system_proxy.bypass.clone(),
                auto_enable: false,
            },
            traffic: TrafficConfig {
                max_records: legacy.traffic.max_records,
                max_db_size_bytes: 2 * 1024 * 1024 * 1024,
                max_body_memory_size: legacy.traffic.max_body_memory_size,
                max_body_buffer_size: legacy.traffic.max_body_buffer_size,
                max_body_probe_size: 64 * 1024,
                enable_body_index: false,
                file_retention_days: legacy.traffic.file_retention_days,
                sse_stream_flush_bytes: legacy.traffic.sse_stream_flush_bytes,
                sse_stream_flush_interval_ms: legacy.traffic.sse_stream_flush_interval_ms,
                ws_payload_flush_bytes: legacy.traffic.ws_payload_flush_bytes,
                ws_payload_flush_interval_ms: legacy.traffic.ws_payload_flush_interval_ms,
                ws_payload_max_open_files: legacy.traffic.ws_payload_max_open_files,
            },
            paths: PathsConfig::for_data_dir(data_dir),
            ui: UiConfig::default(),
        }
    }

    fn save_config(&self, config: &UnifiedConfig) -> Result<()> {
        let config_path = self.data_dir.join("config.toml");
        Self::save_config_to_file(&config_path, config)
    }

    fn save_config_to_file(path: &Path, config: &UnifiedConfig) -> Result<()> {
        let content =
            toml::to_string_pretty(config).map_err(|e| BifrostError::Config(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl Clone for ConfigManager {
    fn clone(&self) -> Self {
        let config = futures::executor::block_on(async { self.config.read().await.clone() });
        let rules_storage =
            futures::executor::block_on(async { self.rules_storage.read().await.clone() });
        let values_storage =
            futures::executor::block_on(async { self.values_storage.read().await.clone() });
        let state_manager =
            futures::executor::block_on(async { self.state_manager.read().await.clone() });

        let (change_notifier, _) = broadcast::channel(100);

        Self {
            data_dir: self.data_dir.clone(),
            config: RwLock::new(config),
            rules_storage: RwLock::new(rules_storage),
            values_storage: RwLock::new(values_storage),
            state_manager: RwLock::new(state_manager),
            change_notifier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ConfigManager) {
        let temp_dir = TempDir::new().unwrap();
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
        (temp_dir, manager)
    }

    #[tokio::test]
    async fn test_config_manager_new() {
        let (_temp_dir, manager) = setup();
        let config = manager.config().await;

        assert_eq!(config.server.timeout_secs, 30);
        assert!(!config.tls.enable_interception);
    }

    #[tokio::test]
    async fn test_update_tls_config() {
        let (_temp_dir, manager) = setup();

        let update = TlsConfigUpdate {
            enable_interception: Some(false),
            unsafe_ssl: Some(true),
            ..Default::default()
        };

        manager.update_tls_config(update).await.unwrap();

        let config = manager.config().await;
        assert!(!config.tls.enable_interception);
        assert!(config.tls.unsafe_ssl);
    }

    #[tokio::test]
    async fn test_config_persistence() {
        let temp_dir = TempDir::new().unwrap();

        {
            let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
            let update = TlsConfigUpdate {
                enable_interception: Some(false),
                ..Default::default()
            };
            manager.update_tls_config(update).await.unwrap();
        }

        {
            let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();
            let config = manager.config().await;
            assert!(!config.tls.enable_interception);
        }
    }

    #[tokio::test]
    async fn test_values_operations() {
        let (_temp_dir, manager) = setup();

        manager.set_value("test_key", "test_value").await.unwrap();
        let value = manager.get_value("test_key").await;
        assert_eq!(value, Some("test_value".to_string()));

        manager.delete_value("test_key").await.unwrap();
        let value = manager.get_value("test_key").await;
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn test_rules_operations() {
        let (_temp_dir, manager) = setup();

        let rule = RuleFile::new("test_rule", "example.com host://localhost");
        manager.save_rule(&rule).await.unwrap();

        let loaded = manager.load_rule("test_rule").await.unwrap();
        assert_eq!(loaded.name, "test_rule");
        assert_eq!(loaded.content, "example.com host://localhost");

        let rules = manager.list_rules().await.unwrap();
        assert!(rules.contains(&"test_rule".to_string()));

        manager.delete_rule("test_rule").await.unwrap();
        let rules = manager.list_rules().await.unwrap();
        assert!(!rules.contains(&"test_rule".to_string()));
    }

    #[tokio::test]
    async fn test_rule_groups() {
        let (_temp_dir, manager) = setup();

        manager.enable_rule_group("group1").await.unwrap();
        assert!(manager.is_rule_group_enabled("group1").await);

        manager.disable_rule_group("group1").await.unwrap();
        assert!(!manager.is_rule_group_enabled("group1").await);
    }

    #[tokio::test]
    async fn test_change_notification() {
        let (_temp_dir, manager) = setup();
        let mut receiver = manager.subscribe();

        let update = TlsConfigUpdate {
            enable_interception: Some(false),
            ..Default::default()
        };
        manager.update_tls_config(update).await.unwrap();

        let event = receiver.try_recv().unwrap();
        assert!(matches!(event, ConfigChangeEvent::TlsConfigChanged(_)));
    }
}
