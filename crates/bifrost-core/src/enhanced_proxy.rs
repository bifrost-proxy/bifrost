use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const STATE_FILE_NAME: &str = "enhanced_proxy_state.json";
const DEFAULT_HELPER_BUNDLE_ID: &str = "com.bifrost.proxy.enhanced";
const DEFAULT_EXTENSION_BUNDLE_ID: &str = "com.bifrost.proxy.enhanced.network-extension";
const DEFAULT_HELPER_APP_NAME: &str = "Bifrost Enhanced Proxy.app";
const CONTROL_SOCKET_NAME: &str = "enhanced-proxy.sock";
const DEFAULT_BYPASS_APPS: &[&str] = &[
    "bifrost",
    "bifrost-desktop",
    "Bifrost",
    "Bifrost Enhanced Proxy",
    "com.bifrost.proxy",
    "com.bifrost.proxy.enhanced",
];
const DEFAULT_BYPASS_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "bifrost.local"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnhancedProxyState {
    Unsupported,
    Disabled,
    HelperMissing,
    ExtensionMissing,
    ApprovalRequired,
    Running,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhancedProxyPolicy {
    #[serde(default)]
    pub include_apps: Vec<String>,
    #[serde(default)]
    pub exclude_apps: Vec<String>,
    #[serde(default)]
    pub include_hosts: Vec<String>,
    #[serde(default)]
    pub exclude_hosts: Vec<String>,
    #[serde(default = "default_tcp_ports")]
    pub tcp_ports: Vec<u16>,
    #[serde(default)]
    pub udp_ports: Vec<u16>,
    #[serde(default = "default_capture_tcp")]
    pub capture_tcp: bool,
    #[serde(default)]
    pub capture_udp: bool,
}

impl Default for EnhancedProxyPolicy {
    fn default() -> Self {
        Self {
            include_apps: Vec::new(),
            exclude_apps: DEFAULT_BYPASS_APPS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            include_hosts: Vec::new(),
            exclude_hosts: DEFAULT_BYPASS_HOSTS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            tcp_ports: default_tcp_ports(),
            udp_ports: Vec::new(),
            capture_tcp: true,
            capture_udp: false,
        }
    }
}

fn default_tcp_ports() -> Vec<u16> {
    vec![80, 443]
}

fn default_capture_tcp() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhancedProxyDesiredState {
    pub enabled: bool,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub helper_bundle_id: String,
    pub extension_bundle_id: String,
    pub policy: EnhancedProxyPolicy,
}

impl Default for EnhancedProxyDesiredState {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 9900,
            helper_bundle_id: DEFAULT_HELPER_BUNDLE_ID.to_string(),
            extension_bundle_id: DEFAULT_EXTENSION_BUNDLE_ID.to_string(),
            policy: EnhancedProxyPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhancedProxyStatus {
    pub supported: bool,
    pub state: EnhancedProxyState,
    pub enabled: bool,
    pub configured_enabled: bool,
    pub platform: String,
    pub helper_bundle_id: String,
    pub extension_bundle_id: String,
    pub helper_app_path: Option<PathBuf>,
    pub extension_path: Option<PathBuf>,
    pub control_socket_path: PathBuf,
    pub controller_connected: bool,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub policy: EnhancedProxyPolicy,
    pub message: Option<String>,
    pub remediation: Option<String>,
}

pub struct EnhancedProxyManager {
    data_dir: PathBuf,
    helper_app_path_override: Option<PathBuf>,
}

struct EnhancedProxyProbe {
    helper_app_path: Option<PathBuf>,
    extension_path: Option<PathBuf>,
    controller_connected: bool,
}

impl EnhancedProxyManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            helper_app_path_override: std::env::var_os("BIFROST_ENHANCED_PROXY_APP")
                .map(PathBuf::from),
        }
    }

    pub fn with_helper_app_path(mut self, path: PathBuf) -> Self {
        self.helper_app_path_override = Some(path);
        self
    }

    pub fn is_supported() -> bool {
        cfg!(target_os = "macos")
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE_NAME)
    }

    pub fn control_socket_path(&self) -> PathBuf {
        self.data_dir.join(CONTROL_SOCKET_NAME)
    }

    pub fn load_desired_state(&self) -> EnhancedProxyDesiredState {
        let path = self.state_path();
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<EnhancedProxyDesiredState>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save_desired_state(&self, state: &EnhancedProxyDesiredState) -> crate::Result<()> {
        fs::create_dir_all(&self.data_dir)?;
        let body = serde_json::to_string_pretty(state)
            .map_err(|error| crate::BifrostError::Config(error.to_string()))?;
        fs::write(self.state_path(), body)?;
        Ok(())
    }

    pub fn set_enabled(
        &self,
        enabled: bool,
        proxy_host: &str,
        proxy_port: u16,
    ) -> crate::Result<()> {
        let mut state = self.load_desired_state();
        state.enabled = enabled;
        state.proxy_host = proxy_host.to_string();
        state.proxy_port = proxy_port;
        self.save_desired_state(&state)
    }

    pub fn status(&self) -> EnhancedProxyStatus {
        let desired = self.load_desired_state();
        let helper_app_path = self.resolve_helper_app_path();
        let extension_path = helper_app_path.as_ref().map(|path| {
            path.join("Contents")
                .join("Library")
                .join("SystemExtensions")
                .join(format!("{}.systemextension", desired.extension_bundle_id))
        });
        let controller_connected = self.control_socket_path().exists();

        if !Self::is_supported() {
            return self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path,
                    extension_path,
                    controller_connected,
                },
                EnhancedProxyState::Unsupported,
                Some("Enhanced proxy is only supported on macOS.".to_string()),
                Some("Use system-proxy or cli-proxy on this platform.".to_string()),
            );
        }

        if !desired.enabled {
            return self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path,
                    extension_path,
                    controller_connected,
                },
                EnhancedProxyState::Disabled,
                Some("Enhanced proxy is configured off.".to_string()),
                Some("Run `bifrost enhanced-proxy enable` to request local capture.".to_string()),
            );
        }

        let Some(ref helper_path) = helper_app_path else {
            return self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path: None,
                    extension_path,
                    controller_connected,
                },
                EnhancedProxyState::HelperMissing,
                Some("Enhanced proxy helper app was not found.".to_string()),
                Some("Install the signed Bifrost Enhanced Proxy.app or set BIFROST_ENHANCED_PROXY_APP.".to_string()),
            );
        };

        if !helper_path.exists() {
            return self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path: Some(helper_path.clone()),
                    extension_path,
                    controller_connected,
                },
                EnhancedProxyState::HelperMissing,
                Some(format!(
                    "Enhanced proxy helper app does not exist: {}",
                    helper_path.display()
                )),
                Some("Install the signed helper app, then retry enable.".to_string()),
            );
        }

        if !extension_path.as_ref().is_some_and(|path| path.exists()) {
            return self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path: Some(helper_path.clone()),
                    extension_path,
                    controller_connected,
                },
                EnhancedProxyState::ExtensionMissing,
                Some("Network Extension bundle is missing from the helper app.".to_string()),
                Some(
                    "Rebuild or reinstall the helper app with its SystemExtensions payload."
                        .to_string(),
                ),
            );
        }

        if controller_connected {
            self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path: Some(helper_path.clone()),
                    extension_path,
                    controller_connected: true,
                },
                EnhancedProxyState::Running,
                Some("Enhanced proxy controller socket is available.".to_string()),
                None,
            )
        } else {
            self.status_with(
                desired,
                EnhancedProxyProbe {
                    helper_app_path: Some(helper_path.clone()),
                    extension_path,
                    controller_connected: false,
                },
                EnhancedProxyState::ApprovalRequired,
                Some("Helper app is installed, but the Network Extension controller is not connected.".to_string()),
                Some("Open the helper app and approve the Network Extension in macOS System Settings.".to_string()),
            )
        }
    }

    fn status_with(
        &self,
        desired: EnhancedProxyDesiredState,
        probe: EnhancedProxyProbe,
        state: EnhancedProxyState,
        message: Option<String>,
        remediation: Option<String>,
    ) -> EnhancedProxyStatus {
        let enabled = matches!(state, EnhancedProxyState::Running);
        EnhancedProxyStatus {
            supported: Self::is_supported(),
            state,
            enabled,
            configured_enabled: desired.enabled,
            platform: std::env::consts::OS.to_string(),
            helper_bundle_id: desired.helper_bundle_id,
            extension_bundle_id: desired.extension_bundle_id,
            helper_app_path: probe.helper_app_path,
            extension_path: probe.extension_path,
            control_socket_path: self.control_socket_path(),
            controller_connected: probe.controller_connected,
            proxy_host: desired.proxy_host,
            proxy_port: desired.proxy_port,
            policy: desired.policy,
            message,
            remediation,
        }
    }

    fn resolve_helper_app_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.helper_app_path_override {
            return Some(path.clone());
        }
        default_applications_dir().map(|dir| dir.join(DEFAULT_HELPER_APP_NAME))
    }
}

fn default_applications_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from("/Applications"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::home_dir().map(|home| home.join("Applications"))
    }
}

pub fn enhanced_proxy_should_capture(
    policy: &EnhancedProxyPolicy,
    app: Option<&str>,
    host: Option<&str>,
    port: u16,
    is_udp: bool,
) -> bool {
    if is_udp {
        if !policy.capture_udp {
            return false;
        }
        if !policy.udp_ports.is_empty() && !policy.udp_ports.contains(&port) {
            return false;
        }
    } else {
        if !policy.capture_tcp {
            return false;
        }
        if !policy.tcp_ports.is_empty() && !policy.tcp_ports.contains(&port) {
            return false;
        }
    }

    if let Some(app) = app {
        if pattern_list_matches(&policy.exclude_apps, app) {
            return false;
        }
        if !policy.include_apps.is_empty() && !pattern_list_matches(&policy.include_apps, app) {
            return false;
        }
    } else if !policy.include_apps.is_empty() {
        return false;
    }

    if let Some(host) = host {
        if pattern_list_matches(&policy.exclude_hosts, host) {
            return false;
        }
        if !policy.include_hosts.is_empty() && !pattern_list_matches(&policy.include_hosts, host) {
            return false;
        }
    } else if !policy.include_hosts.is_empty() {
        return false;
    }

    true
}

fn pattern_list_matches(patterns: &[String], value: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_match(pattern, value))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern.eq_ignore_ascii_case(value);
    }

    let pattern_lower = pattern.to_ascii_lowercase();
    let value_lower = value.to_ascii_lowercase();
    let mut remainder = value_lower.as_str();
    let anchored_start = !pattern_lower.starts_with('*');
    let anchored_end = !pattern_lower.ends_with('*');
    let parts: Vec<&str> = pattern_lower
        .split('*')
        .filter(|part| !part.is_empty())
        .collect();

    if parts.is_empty() {
        return true;
    }
    if anchored_start && !remainder.starts_with(parts[0]) {
        return false;
    }

    for (index, part) in parts.iter().enumerate() {
        if let Some(pos) = remainder.find(part) {
            if index == 0 && anchored_start && pos != 0 {
                return false;
            }
            remainder = &remainder[pos + part.len()..];
        } else {
            return false;
        }
    }

    if anchored_end {
        if let Some(last) = parts.last() {
            return value_lower.ends_with(last);
        }
    }
    true
}

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_bypasses_bifrost_itself() {
        let policy = EnhancedProxyPolicy::default();
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("bifrost"),
            Some("example.com"),
            443,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("com.bifrost.proxy.enhanced"),
            Some("example.com"),
            443,
            false
        ));
    }

    #[test]
    fn default_policy_captures_web_tcp_ports_only() {
        let policy = EnhancedProxyPolicy::default();
        assert!(enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("example.com"),
            443,
            false
        ));
        assert!(enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("example.com"),
            80,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("example.com"),
            22,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("example.com"),
            443,
            true
        ));
    }

    #[test]
    fn include_apps_are_strict_when_present() {
        let policy = EnhancedProxyPolicy {
            include_apps: vec!["curl*".to_string()],
            ..EnhancedProxyPolicy::default()
        };
        assert!(enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("example.com"),
            443,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("Safari"),
            Some("example.com"),
            443,
            false
        ));
    }

    #[test]
    fn host_excludes_win_before_includes() {
        let policy = EnhancedProxyPolicy {
            include_hosts: vec!["*.example.com".to_string()],
            exclude_hosts: vec!["admin.example.com".to_string()],
            ..EnhancedProxyPolicy::default()
        };
        assert!(enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("api.example.com"),
            443,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("admin.example.com"),
            443,
            false
        ));
        assert!(!enhanced_proxy_should_capture(
            &policy,
            Some("curl"),
            Some("other.test"),
            443,
            false
        ));
    }

    #[test]
    fn desired_state_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = EnhancedProxyManager::new(tmp.path().to_path_buf());
        manager.set_enabled(true, "127.0.0.1", 18888).unwrap();
        let loaded = manager.load_desired_state();
        assert!(loaded.enabled);
        assert_eq!(loaded.proxy_port, 18888);
    }

    #[test]
    fn missing_helper_is_reported_when_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = tmp.path().join("Missing.app");
        let manager = EnhancedProxyManager::new(tmp.path().to_path_buf())
            .with_helper_app_path(helper.clone());
        manager.set_enabled(true, "127.0.0.1", 9900).unwrap();
        let status = manager.status();

        if EnhancedProxyManager::is_supported() {
            assert_eq!(status.state, EnhancedProxyState::HelperMissing);
            assert_eq!(status.helper_app_path, Some(helper));
        } else {
            assert_eq!(status.state, EnhancedProxyState::Unsupported);
        }
    }
}
