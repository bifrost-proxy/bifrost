mod config;
mod config_manager;
mod data_dir;
mod local_secrets;
mod remote_shell;
mod rules;
mod state;
mod unified_config;
mod values;

pub(crate) use config::BifrostConfig as LegacyBifrostConfig;
pub use config_manager::{
    ConfigChangeEvent, ConfigManager, RulesChangeOrigin, SharedConfigManager,
};
pub use data_dir::{data_dir, data_dir_fingerprint, data_dir_fingerprint_for, set_data_dir};
pub use local_secrets::LocalSecretKey;
pub use remote_shell::{
    ensure_default_ssh_key_shell_policy, RemoteShellPolicy, RemoteShellProfile, RemoteShellSet,
    RemoteShellStore, DEFAULT_SSH_KEY_SHELL_POLICY_ID,
};
pub use rules::{
    build_rule_reference_catalog, content_hash, rule_reference_key, RuleFile, RuleSummary,
    RuleSyncMetadata, RuleSyncStatus, RulesStorage, ShareEnvState, DEFAULT_RULE_NAME,
    DEFAULT_RULE_SORT_ORDER,
};
pub use state::{RuntimeState, StateManager};
pub use unified_config::{
    AccessConfig as NewAccessConfig, AccessConfigUpdate, CollapsedSections, FilterPanelConfig,
    PathsConfig, PinnedFilter, PinnedFilterType, ProxySettings, SandboxConfig, SandboxConfigUpdate,
    SandboxFileConfig, SandboxFileConfigUpdate, SandboxLimitsConfig, SandboxLimitsConfigUpdate,
    SandboxNetConfig, SandboxNetConfigUpdate, ServerConfig, ServerConfigUpdate, SocksAuthConfig,
    SyncConfig, SyncConfigUpdate, SystemProxyConfig as NewSystemProxyConfig,
    SystemProxyConfigUpdate, SystemProxyRecoveryMode, TlsConfig, TlsConfigUpdate,
    TrafficConfig as NewTrafficConfig, TrafficConfigUpdate, TrayConfig, TrayConfigUpdate,
    TraySystemStatsItems, TraySystemStatsItemsUpdate, UiConfig, UiConfigUpdate, UnifiedConfig,
    DEFAULT_BREAKPOINT_TIMEOUT_MS, DEFAULT_REMOTE_BASE_URL, DEFAULT_SYSTEM_PROXY_BYPASS,
    DEFAULT_TRAFFIC_MAX_RECORDS, LEGACY_DEFAULT_SYSTEM_PROXY_BYPASS, MAX_BREAKPOINT_TIMEOUT_MS,
    MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS, MAX_TRAFFIC_MAX_DB_SIZE_BYTES, MAX_TRAFFIC_MAX_RECORDS,
    MIN_BREAKPOINT_TIMEOUT_MS, MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS, MIN_TRAFFIC_MAX_DB_SIZE_BYTES,
    MIN_TRAFFIC_MAX_RECORDS,
};
pub use values::{ValueEntry, ValuesStorage};
