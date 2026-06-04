mod config;
mod config_manager;
mod data_dir;
mod remote_shell;
mod rules;
mod state;
mod unified_config;
mod values;

pub(crate) use config::BifrostConfig as LegacyBifrostConfig;
pub use config_manager::{ConfigChangeEvent, ConfigManager, SharedConfigManager};
pub use data_dir::{data_dir, set_data_dir};
pub use remote_shell::{RemoteShellPolicy, RemoteShellProfile, RemoteShellSet, RemoteShellStore};
pub use rules::{
    content_hash, RuleFile, RuleSummary, RuleSyncMetadata, RuleSyncStatus, RulesStorage,
};
pub use state::{RuntimeState, StateManager};
pub use unified_config::{
    AccessConfig as NewAccessConfig, AccessConfigUpdate, CollapsedSections, FilterPanelConfig,
    PathsConfig, PinnedFilter, PinnedFilterType, ProxySettings, SandboxConfig, SandboxConfigUpdate,
    SandboxFileConfig, SandboxFileConfigUpdate, SandboxLimitsConfig, SandboxLimitsConfigUpdate,
    SandboxNetConfig, SandboxNetConfigUpdate, ServerConfig, ServerConfigUpdate, SocksAuthConfig,
    SyncConfig, SyncConfigUpdate, SystemProxyConfig as NewSystemProxyConfig,
    SystemProxyConfigUpdate, TlsConfig, TlsConfigUpdate, TrafficConfig as NewTrafficConfig,
    TrafficConfigUpdate, UiConfig, UiConfigUpdate, UnifiedConfig, DEFAULT_BREAKPOINT_TIMEOUT_MS,
    DEFAULT_REMOTE_BASE_URL, DEFAULT_TRAFFIC_MAX_RECORDS, MAX_BREAKPOINT_TIMEOUT_MS,
    MAX_TRAFFIC_MAX_DB_SIZE_BYTES, MAX_TRAFFIC_MAX_RECORDS, MIN_BREAKPOINT_TIMEOUT_MS,
    MIN_TRAFFIC_MAX_DB_SIZE_BYTES, MIN_TRAFFIC_MAX_RECORDS,
};
pub use values::{ValueEntry, ValuesStorage};
