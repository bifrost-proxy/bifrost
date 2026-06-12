use std::path::PathBuf;

use serde::Deserialize;

use crate::DEFAULT_TRAFFIC_MAX_RECORDS;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct LegacyAccessConfig {
    pub mode: String,
    pub whitelist: Vec<String>,
    pub allow_lan: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct LegacySystemProxyConfig {
    pub enabled: bool,
    pub bypass: String,
}

impl Default for LegacySystemProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bypass: "localhost,127.0.0.1,::1,*.local".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct LegacyTrafficConfig {
    pub max_records: usize,
    pub max_body_memory_size: usize,
    pub max_body_buffer_size: usize,
    pub temp_dir: PathBuf,
    pub file_retention_days: u64,
    pub sse_stream_flush_bytes: usize,
    pub sse_stream_flush_interval_ms: u64,
    pub ws_payload_flush_bytes: usize,
    pub ws_payload_flush_interval_ms: u64,
    pub ws_payload_max_open_files: usize,
}

impl Default for LegacyTrafficConfig {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_TRAFFIC_MAX_RECORDS,
            max_body_memory_size: 512 * 1024,
            max_body_buffer_size: 10 * 1024 * 1024,
            temp_dir: crate::data_dir().join("traffic"),
            file_retention_days: 7,
            sse_stream_flush_bytes: 256 * 1024,
            sse_stream_flush_interval_ms: 1000,
            ws_payload_flush_bytes: 512 * 1024,
            ws_payload_flush_interval_ms: 1000,
            ws_payload_max_open_files: 128,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct BifrostConfig {
    pub rules_dir: PathBuf,
    pub values_dir: PathBuf,
    pub cert_dir: PathBuf,
    pub access: LegacyAccessConfig,
    pub traffic: LegacyTrafficConfig,
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub system_proxy: LegacySystemProxyConfig,
    pub disconnect_on_config_change: bool,
}

impl Default for BifrostConfig {
    fn default() -> Self {
        let base = crate::data_dir();
        Self {
            rules_dir: base.join("rules"),
            values_dir: base.join("values"),
            cert_dir: base.join("certs"),
            access: LegacyAccessConfig::default(),
            traffic: LegacyTrafficConfig::default(),
            enable_tls_interception: true,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            system_proxy: LegacySystemProxyConfig::default(),
            disconnect_on_config_change: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_access_config_default_is_empty() {
        let a = LegacyAccessConfig::default();
        assert_eq!(a.mode, "");
        assert!(a.whitelist.is_empty());
        assert!(!a.allow_lan);
    }

    #[test]
    fn legacy_system_proxy_config_default() {
        let p = LegacySystemProxyConfig::default();
        assert!(!p.enabled);
        assert_eq!(p.bypass, "localhost,127.0.0.1,::1,*.local");
    }

    #[test]
    fn legacy_traffic_config_default_values() {
        let t = LegacyTrafficConfig::default();
        assert_eq!(t.max_records, DEFAULT_TRAFFIC_MAX_RECORDS);
        assert_eq!(t.max_body_memory_size, 512 * 1024);
        assert_eq!(t.max_body_buffer_size, 10 * 1024 * 1024);
        assert_eq!(t.file_retention_days, 7);
        assert_eq!(t.sse_stream_flush_bytes, 256 * 1024);
        assert_eq!(t.sse_stream_flush_interval_ms, 1000);
        assert_eq!(t.ws_payload_flush_bytes, 512 * 1024);
        assert_eq!(t.ws_payload_flush_interval_ms, 1000);
        assert_eq!(t.ws_payload_max_open_files, 128);
        assert!(t.temp_dir.ends_with("traffic"));
    }

    #[test]
    fn bifrost_config_default_dirs() {
        let c = BifrostConfig::default();
        assert!(c.rules_dir.ends_with("rules"));
        assert!(c.values_dir.ends_with("values"));
        assert!(c.cert_dir.ends_with("certs"));
        assert!(c.enable_tls_interception);
        assert!(c.disconnect_on_config_change);
        assert!(c.intercept_exclude.is_empty());
        assert!(c.intercept_include.is_empty());
    }

    #[test]
    fn bifrost_config_deserializes_partial_toml_with_defaults() {
        // Only some fields present; the rest fall back to defaults via #[serde(default)].
        let toml_src = r#"
enable_tls_interception = false
disconnect_on_config_change = false
intercept_include = ["example.com"]

[access]
mode = "whitelist"
allow_lan = true
whitelist = ["10.0.0.0/8"]

[system_proxy]
enabled = true
bypass = "localhost"

[traffic]
max_records = 42
file_retention_days = 30
"#;
        let cfg: BifrostConfig = toml::from_str(toml_src).unwrap();
        assert!(!cfg.enable_tls_interception);
        assert!(!cfg.disconnect_on_config_change);
        assert_eq!(cfg.intercept_include, vec!["example.com".to_string()]);
        assert_eq!(cfg.access.mode, "whitelist");
        assert!(cfg.access.allow_lan);
        assert_eq!(cfg.access.whitelist, vec!["10.0.0.0/8".to_string()]);
        assert!(cfg.system_proxy.enabled);
        assert_eq!(cfg.system_proxy.bypass, "localhost");
        assert_eq!(cfg.traffic.max_records, 42);
        assert_eq!(cfg.traffic.file_retention_days, 30);
        // Untouched traffic field still defaulted.
        assert_eq!(cfg.traffic.ws_payload_max_open_files, 128);
    }
}
