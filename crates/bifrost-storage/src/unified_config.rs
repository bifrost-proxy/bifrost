use std::path::{Path, PathBuf};

use bifrost_core::{AccessMode, UserPassAuthConfig};
use serde::{Deserialize, Serialize};

pub const MIN_TRAFFIC_MAX_RECORDS: usize = 1_000;
pub const DEFAULT_TRAFFIC_MAX_RECORDS: usize = 5_000;
pub const MAX_TRAFFIC_MAX_RECORDS: usize = 100_000;

pub const MIN_TRAFFIC_MAX_DB_SIZE_BYTES: u64 = 100 * 1024;
pub const MAX_TRAFFIC_MAX_DB_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const DEFAULT_BREAKPOINT_TIMEOUT_MS: u64 = 30_000;
pub const MIN_BREAKPOINT_TIMEOUT_MS: u64 = 5_000;
pub const MAX_BREAKPOINT_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_SYSTEM_PROXY_BYPASS: &str = "localhost,127.0.0.1,::1";
pub const LEGACY_DEFAULT_SYSTEM_PROXY_BYPASS: &str = "localhost,127.0.0.1,::1,*.local";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnifiedConfig {
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub access: AccessConfig,
    pub proxy: ProxySettings,
    pub tray: TrayConfig,
    pub system_proxy: SystemProxyConfig,
    pub sync: SyncConfig,
    pub traffic: TrafficConfig,
    pub sandbox: SandboxConfig,
    #[serde(skip)]
    pub paths: PathsConfig,
    pub ui: UiConfig,
    pub keepawake: KeepAwakeConfig,
}

impl UnifiedConfig {
    pub fn default_for_data_dir(data_dir: &Path) -> Self {
        Self {
            server: ServerConfig::default(),
            tls: TlsConfig::default(),
            access: AccessConfig::default(),
            proxy: ProxySettings::default(),
            tray: TrayConfig::default(),
            system_proxy: SystemProxyConfig::default(),
            sync: SyncConfig::default(),
            traffic: TrafficConfig::default_for_data_dir(data_dir),
            sandbox: SandboxConfig::default(),
            paths: PathsConfig::for_data_dir(data_dir),
            ui: UiConfig::default(),
            keepawake: KeepAwakeConfig::default(),
        }
    }

    pub fn with_data_dir(mut self, data_dir: &Path) -> Self {
        self.paths = PathsConfig::for_data_dir(data_dir);
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub file: SandboxFileConfig,
    pub net: SandboxNetConfig,
    pub limits: SandboxLimitsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxFileConfig {
    /// 脚本沙箱默认工作目录（相对 `~/.bifrost/scripts/`，或绝对路径）
    pub sandbox_dir: String,
    /// 允许脚本访问的系统目录（绝对路径）
    pub allowed_dirs: Vec<String>,
    /// 单次文件读写最大字节数
    pub max_bytes: usize,
}

impl Default for SandboxFileConfig {
    fn default() -> Self {
        Self {
            sandbox_dir: "_sandbox".to_string(),
            allowed_dirs: Vec::new(),
            max_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxNetConfig {
    pub enabled: bool,
    pub allow_private_network: bool,
    pub timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

impl Default for SandboxNetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_private_network: false,
            timeout_ms: 5_000,
            max_request_bytes: 256 * 1024,
            max_response_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxLimitsConfig {
    pub timeout_ms: u64,
    pub max_memory_bytes: usize,
    /// decode:// 脚本的输入最大字节数（解压后 bytes）。超过则跳过 decode，避免性能/内存风险。
    pub max_decode_input_bytes: usize,
    /// HTTP body 解压后的最大输出字节数（用于防止压缩炸弹）。超过则放弃解压并回退到原始压缩数据。
    pub max_decompress_output_bytes: usize,
}

impl Default for SandboxLimitsConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_memory_bytes: 32 * 1024 * 1024,
            max_decode_input_bytes: 2 * 1024 * 1024,
            max_decompress_output_bytes: 10 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SandboxConfigUpdate {
    pub file: Option<SandboxFileConfigUpdate>,
    pub net: Option<SandboxNetConfigUpdate>,
    pub limits: Option<SandboxLimitsConfigUpdate>,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxFileConfigUpdate {
    pub sandbox_dir: Option<String>,
    pub allowed_dirs: Option<Vec<String>>,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxNetConfigUpdate {
    pub enabled: Option<bool>,
    pub allow_private_network: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub max_request_bytes: Option<usize>,
    pub max_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxLimitsConfigUpdate {
    pub timeout_ms: Option<u64>,
    pub max_memory_bytes: Option<usize>,
    pub max_decode_input_bytes: Option<usize>,
    pub max_decompress_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub socks5_auth: Option<SocksAuthConfig>,
    pub timeout_secs: u64,
    pub http1_max_header_size: usize,
    pub http2_max_header_list_size: usize,
    pub websocket_handshake_max_header_size: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ServerConfigUpdate {
    pub timeout_secs: Option<u64>,
    pub http1_max_header_size: Option<usize>,
    pub http2_max_header_list_size: Option<usize>,
    pub websocket_handshake_max_header_size: Option<usize>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            socks5_auth: None,
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SocksAuthConfig {
    pub enabled: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub const DEFAULT_APP_INTERCEPT_INCLUDE: &[&str] = &[
    "Google Chrome*",
    "Microsoft Edge*",
    "*Safari*",
    "*Firefox*",
    "*Opera*",
    "*Brave*",
    "*Arc*",
    "*Vivaldi*",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub enable_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub ip_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,
    pub unsafe_ssl: bool,
    pub disconnect_on_change: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enable_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: DEFAULT_APP_INTERCEPT_INCLUDE
                .iter()
                .map(|app| (*app).to_string())
                .collect(),
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            unsafe_ssl: false,
            disconnect_on_change: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessConfig {
    pub mode: AccessMode,
    pub whitelist: Vec<String>,
    pub allow_lan: bool,
    pub userpass: Option<UserPassAuthConfig>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            mode: AccessMode::Interactive,
            whitelist: Vec::new(),
            allow_lan: false,
            userpass: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxySettings {
    pub upstream_proxy: Option<String>,
    pub no_proxy: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    pub enabled: bool,
    pub show_system_stats: bool,
    pub system_stats_items: TraySystemStatsItems,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_system_stats: true,
            system_stats_items: TraySystemStatsItems::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraySystemStatsItems {
    #[serde(default = "default_true")]
    pub cpu: bool,
    #[serde(default = "default_true")]
    pub memory: bool,
    #[serde(default = "default_true")]
    pub disk: bool,
    #[serde(default = "default_true")]
    pub upload: bool,
    #[serde(default = "default_true")]
    pub download: bool,
}

impl TraySystemStatsItems {
    pub fn any_enabled(&self) -> bool {
        self.cpu || self.memory || self.disk || self.upload || self.download
    }
}

impl Default for TraySystemStatsItems {
    fn default() -> Self {
        Self {
            cpu: true,
            memory: true,
            disk: true,
            upload: true,
            download: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default)]
pub struct TrayConfigUpdate {
    pub enabled: Option<bool>,
    pub show_system_stats: Option<bool>,
    pub system_stats_items: Option<TraySystemStatsItemsUpdate>,
}

#[derive(Debug, Clone, Default)]
pub struct TraySystemStatsItemsUpdate {
    pub cpu: Option<bool>,
    pub memory: Option<bool>,
    pub disk: Option<bool>,
    pub upload: Option<bool>,
    pub download: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemProxyConfig {
    pub enabled: bool,
    pub bypass: String,
    pub auto_enable: bool,
}

impl Default for SystemProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bypass: DEFAULT_SYSTEM_PROXY_BYPASS.to_string(),
            auto_enable: false,
        }
    }
}

impl SystemProxyConfig {
    pub fn normalize_legacy_default_bypass(&mut self) -> bool {
        if self.bypass == LEGACY_DEFAULT_SYSTEM_PROXY_BYPASS {
            self.bypass = DEFAULT_SYSTEM_PROXY_BYPASS.to_string();
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    pub enabled: bool,
    pub auto_sync: bool,
    pub remote_base_url: String,
    pub probe_interval_secs: u64,
    pub connect_timeout_ms: u64,
}

pub const DEFAULT_REMOTE_BASE_URL: &str = "https://bifrost.bytedance.net";

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_sync: true,
            remote_base_url: DEFAULT_REMOTE_BASE_URL.to_string(),
            probe_interval_secs: 5,
            connect_timeout_ms: 3_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncConfigUpdate {
    pub enabled: Option<bool>,
    pub auto_sync: Option<bool>,
    pub remote_base_url: Option<String>,
    pub probe_interval_secs: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrafficConfig {
    pub max_records: usize,
    pub max_db_size_bytes: u64,
    pub max_body_memory_size: usize,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub super_performance_mode: bool,
    pub binary_traffic_performance_mode: bool,
    pub inject_bifrost_badge: bool,
    pub file_retention_days: u64,
    pub sse_stream_flush_bytes: usize,
    pub sse_stream_flush_interval_ms: u64,
    pub ws_payload_flush_bytes: usize,
    pub ws_payload_flush_interval_ms: u64,
    pub ws_payload_max_open_files: usize,
    pub breakpoint_timeout_ms: u64,
}

impl Default for TrafficConfig {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_TRAFFIC_MAX_RECORDS,
            max_db_size_bytes: 2 * 1024 * 1024 * 1024,
            max_body_memory_size: 512 * 1024,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            super_performance_mode: false,
            binary_traffic_performance_mode: true,
            inject_bifrost_badge: true,
            file_retention_days: 7,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 128,
            breakpoint_timeout_ms: DEFAULT_BREAKPOINT_TIMEOUT_MS,
        }
    }
}

impl TrafficConfig {
    pub fn default_for_data_dir(_data_dir: &Path) -> Self {
        Self::default()
    }

    pub fn normalize(&mut self) {
        self.max_records = normalize_max_records(self.max_records);
        self.max_db_size_bytes = self
            .max_db_size_bytes
            .clamp(MIN_TRAFFIC_MAX_DB_SIZE_BYTES, MAX_TRAFFIC_MAX_DB_SIZE_BYTES);
        self.breakpoint_timeout_ms = self
            .breakpoint_timeout_ms
            .clamp(MIN_BREAKPOINT_TIMEOUT_MS, MAX_BREAKPOINT_TIMEOUT_MS);
    }
}

pub fn normalize_max_records(value: usize) -> usize {
    value.clamp(MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS)
}

#[derive(Debug, Clone, Default)]
pub struct PathsConfig {
    pub rules_dir: PathBuf,
    pub values_dir: PathBuf,
    pub cert_dir: PathBuf,
    pub traffic_dir: PathBuf,
}

impl PathsConfig {
    pub fn for_data_dir(data_dir: &Path) -> Self {
        Self {
            rules_dir: data_dir.join("rules"),
            values_dir: data_dir.join("values"),
            cert_dir: data_dir.join("certs"),
            traffic_dir: data_dir.join("traffic"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TlsConfigUpdate {
    pub enable_interception: Option<bool>,
    pub intercept_exclude: Option<Vec<String>>,
    pub intercept_include: Option<Vec<String>>,
    pub app_intercept_exclude: Option<Vec<String>>,
    pub app_intercept_include: Option<Vec<String>>,
    pub ip_intercept_exclude: Option<Vec<String>>,
    pub ip_intercept_include: Option<Vec<String>>,
    pub unsafe_ssl: Option<bool>,
    pub disconnect_on_change: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct AccessConfigUpdate {
    pub mode: Option<AccessMode>,
    pub whitelist: Option<Vec<String>>,
    pub allow_lan: Option<bool>,
    pub userpass: Option<Option<UserPassAuthConfig>>,
}

#[derive(Debug, Clone, Default)]
pub struct SystemProxyConfigUpdate {
    pub enabled: Option<bool>,
    pub bypass: Option<String>,
    pub auto_enable: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct TrafficConfigUpdate {
    pub max_records: Option<usize>,
    pub max_db_size_bytes: Option<u64>,
    pub max_body_memory_size: Option<usize>,
    pub max_body_buffer_size: Option<usize>,
    pub max_body_probe_size: Option<usize>,
    pub super_performance_mode: Option<bool>,
    pub binary_traffic_performance_mode: Option<bool>,
    pub inject_bifrost_badge: Option<bool>,
    pub file_retention_days: Option<u64>,
    pub sse_stream_flush_bytes: Option<usize>,
    pub sse_stream_flush_interval_ms: Option<u64>,
    pub ws_payload_flush_bytes: Option<usize>,
    pub ws_payload_flush_interval_ms: Option<u64>,
    pub ws_payload_max_open_files: Option<usize>,
    pub breakpoint_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinnedFilterType {
    ClientIp,
    ClientApp,
    AccountName,
    #[default]
    Domain,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedFilter {
    pub id: String,
    #[serde(rename = "type")]
    pub filter_type: PinnedFilterType,
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilterPanelConfig {
    pub collapsed: bool,
    pub width: u32,
    pub collapsed_sections: CollapsedSections,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CollapsedSections {
    pub pinned: bool,
    pub client_ip: bool,
    pub client_app: bool,
    pub account_name: bool,
    pub domain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub pinned_filters: Vec<PinnedFilter>,
    pub filter_panel: FilterPanelConfig,
    pub detail_panel_collapsed: bool,
    pub rules_sort_mode: String,
}

#[derive(Debug, Clone, Default)]
pub struct UiConfigUpdate {
    pub pinned_filters: Option<Vec<PinnedFilter>>,
    pub filter_panel: Option<FilterPanelConfig>,
    pub detail_panel_collapsed: Option<bool>,
    pub rules_sort_mode: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            pinned_filters: Vec::new(),
            filter_panel: FilterPanelConfig::default(),
            detail_panel_collapsed: false,
            rules_sort_mode: "manual".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_unified_config_default() {
        let config = UnifiedConfig::default();
        assert_eq!(config.server.timeout_secs, 30);
        assert!(!config.tls.enable_interception);
        assert!(config.tls.intercept_exclude.is_empty());
        assert_eq!(config.access.mode, AccessMode::Interactive);
        assert!(config.system_proxy.enabled);
        assert_eq!(config.system_proxy.bypass, DEFAULT_SYSTEM_PROXY_BYPASS);
        assert!(!config.system_proxy.bypass.contains("*.local"));
        assert!(config.tray.enabled);
        assert!(config.tray.show_system_stats);
        assert!(config.tray.system_stats_items.cpu);
        assert!(config.tray.system_stats_items.memory);
        assert!(config.tray.system_stats_items.disk);
        assert!(config.tray.system_stats_items.upload);
        assert!(config.tray.system_stats_items.download);
        assert!(config.sync.enabled);
        assert!(!config.traffic.super_performance_mode);
        assert_eq!(config.sync.remote_base_url, DEFAULT_REMOTE_BASE_URL);
        assert_eq!(config.ui.rules_sort_mode, "manual");
        assert!(!config.sandbox.net.allow_private_network);
    }

    #[test]
    fn test_unified_config_for_data_dir() {
        let temp_dir = TempDir::new().unwrap();
        let config = UnifiedConfig::default_for_data_dir(temp_dir.path());

        assert_eq!(config.paths.rules_dir, temp_dir.path().join("rules"));
        assert_eq!(config.paths.values_dir, temp_dir.path().join("values"));
        assert_eq!(config.paths.cert_dir, temp_dir.path().join("certs"));
        assert_eq!(config.paths.traffic_dir, temp_dir.path().join("traffic"));
    }

    #[test]
    fn test_paths_for_data_dir() {
        let temp_dir = TempDir::new().unwrap();
        let paths = PathsConfig::for_data_dir(temp_dir.path());

        assert_eq!(paths.rules_dir, temp_dir.path().join("rules"));
        assert_eq!(paths.values_dir, temp_dir.path().join("values"));
        assert_eq!(paths.cert_dir, temp_dir.path().join("certs"));
        assert_eq!(paths.traffic_dir, temp_dir.path().join("traffic"));
    }

    #[test]
    fn system_proxy_config_migrates_only_legacy_default_bypass() {
        let mut legacy_default = SystemProxyConfig {
            bypass: LEGACY_DEFAULT_SYSTEM_PROXY_BYPASS.to_string(),
            ..Default::default()
        };
        assert!(legacy_default.normalize_legacy_default_bypass());
        assert_eq!(legacy_default.bypass, DEFAULT_SYSTEM_PROXY_BYPASS);

        let mut custom = SystemProxyConfig {
            bypass: "localhost,127.0.0.1,*.local,10.0.0.0/8".to_string(),
            ..Default::default()
        };
        assert!(!custom.normalize_legacy_default_bypass());
        assert_eq!(custom.bypass, "localhost,127.0.0.1,*.local,10.0.0.0/8");
    }

    #[test]
    fn test_tls_config_default() {
        let config = TlsConfig::default();
        assert!(!config.enable_interception);
        assert!(!config.unsafe_ssl);
        assert!(config.disconnect_on_change);
        assert_eq!(config.app_intercept_include, DEFAULT_APP_INTERCEPT_INCLUDE);
        assert!(!config
            .app_intercept_include
            .iter()
            .any(|app| app.to_ascii_lowercase().contains("codex")));
    }

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert!(config.socks5_auth.is_none());
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_serialization() {
        let config = UnifiedConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: UnifiedConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.server.timeout_secs, parsed.server.timeout_secs);
        assert_eq!(
            config.tls.enable_interception,
            parsed.tls.enable_interception
        );
        assert_eq!(config.ui.rules_sort_mode, parsed.ui.rules_sort_mode);
        assert_eq!(config.tray.show_system_stats, parsed.tray.show_system_stats);
        assert_eq!(
            config.tray.system_stats_items.cpu,
            parsed.tray.system_stats_items.cpu
        );
        assert_eq!(
            config.tray.system_stats_items.download,
            parsed.tray.system_stats_items.download
        );
    }

    #[test]
    fn test_partial_tray_system_stats_items_default_to_enabled() {
        let parsed: UnifiedConfig = toml::from_str(
            r#"
            [tray]
            enabled = true
            show_system_stats = true

            [tray.system_stats_items]
            download = false
            "#,
        )
        .unwrap();

        assert!(parsed.tray.system_stats_items.cpu);
        assert!(parsed.tray.system_stats_items.memory);
        assert!(parsed.tray.system_stats_items.disk);
        assert!(parsed.tray.system_stats_items.upload);
        assert!(!parsed.tray.system_stats_items.download);
    }
}

/// Configuration for the keep-awake (prevent-sleep) feature.
/// Only the `mode` is persisted; runtime assertion state is volatile
/// and released when the process exits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeepAwakeConfig {
    /// One of `off`, `auto`, `force_on`. Default is `auto`:
    /// inactive until the first remote call arrives, then active
    /// until manually turned off or the process exits.
    pub mode: String,
}

impl Default for KeepAwakeConfig {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
        }
    }
}
