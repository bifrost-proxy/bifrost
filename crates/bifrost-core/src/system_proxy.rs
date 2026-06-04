use std::path::PathBuf;
use sysproxy::Sysproxy;

use crate::{BifrostError, Result};

const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1,*.local";
const BACKUP_FILE_NAME: &str = "proxy_backup.json";
const STATE_FILE_NAME: &str = "proxy_state.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxyBackup {
    pub enable: bool,
    pub host: String,
    pub port: u16,
    pub bypass: String,
}

impl ProxyBackup {
    pub fn target_matches(&self, host: &str, port: u16) -> bool {
        self.enable && self.port == port && proxy_hosts_match(&self.host, host)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemProxyDisableOutcome {
    Disabled,
    NotEnabled,
    OwnedByOther,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ManagedProxyState {
    original: ProxyBackup,
    target: ProxyBackup,
}

impl From<&Sysproxy> for ProxyBackup {
    fn from(proxy: &Sysproxy) -> Self {
        Self {
            enable: proxy.enable,
            host: proxy.host.clone(),
            port: proxy.port,
            bypass: proxy.bypass.clone(),
        }
    }
}

impl From<ProxyBackup> for Sysproxy {
    fn from(backup: ProxyBackup) -> Self {
        Self {
            enable: backup.enable,
            host: backup.host,
            port: backup.port,
            bypass: backup.bypass,
        }
    }
}

pub struct SystemProxyManager {
    original_proxy: Option<Sysproxy>,
    is_set: bool,
    data_dir: PathBuf,
}

impl SystemProxyManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            original_proxy: None,
            is_set: false,
            data_dir,
        }
    }

    pub fn is_supported() -> bool {
        Sysproxy::is_support()
    }

    pub fn enable(&mut self, host: &str, port: u16, bypass: Option<&str>) -> Result<()> {
        if !Self::is_supported() {
            return Err(BifrostError::Config(
                "System proxy is not supported on this platform".to_string(),
            ));
        }
        tracing::info!(
            requested_host = %host,
            requested_port = port,
            was_set = self.is_set,
            "System proxy enable requested"
        );

        let mut preserved_original: Option<Sysproxy> = None;
        if self.is_set {
            #[cfg(target_os = "macos")]
            let all_services_match =
                macos_all_services_proxy_match(host, port).unwrap_or_else(|error| {
                    tracing::warn!(
                        error = %error,
                        expected_host = %host,
                        expected_port = port,
                        "Failed to inspect all macOS network services before system proxy re-apply"
                    );
                    false
                });

            #[cfg(not(target_os = "macos"))]
            let all_services_match = false;

            if let Ok(actual) = Self::get_current() {
                if cfg!(target_os = "macos") {
                    if all_services_match {
                        return Ok(());
                    }
                } else if actual.enable && actual.host == host && actual.port == port {
                    return Ok(());
                }
                tracing::info!(
                    actual_enabled = actual.enable,
                    actual_host = %actual.host,
                    actual_port = actual.port,
                    expected_host = %host,
                    expected_port = port,
                    "System proxy was externally changed, re-applying"
                );
                preserved_original = self
                    .original_proxy
                    .clone()
                    .or_else(|| {
                        self.load_managed_state()
                            .ok()
                            .map(|state| state.original.into())
                    })
                    .or_else(|| Some(actual.into()));
            }
        }

        #[cfg(target_os = "macos")]
        let current = match preserved_original {
            Some(original) => original,
            None => match Self::parse_macos_proxy() {
                Some(proxy) => proxy,
                None => Sysproxy::get_system_proxy().map_err(|e| {
                    BifrostError::Config(format!(
                        "Failed to get current system proxy for backup: {}",
                        e
                    ))
                })?,
            },
        };

        #[cfg(target_os = "windows")]
        let current = match preserved_original {
            Some(original) => original,
            None => match Self::parse_windows_proxy() {
                Some(proxy) => proxy,
                None => Sysproxy::get_system_proxy().unwrap_or_else(|e| {
                    tracing::debug!(error = %e, "[SYSTEM_PROXY] Failed to get system proxy via winreg, using default");
                    Sysproxy {
                        enable: false,
                        host: String::new(),
                        port: 0,
                        bypass: String::new(),
                    }
                }),
            },
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let current = match preserved_original {
            Some(original) => original,
            None => Sysproxy::get_system_proxy().map_err(|e| {
                BifrostError::Config(format!(
                    "Failed to get current system proxy for backup: {}",
                    e
                ))
            })?,
        };

        self.original_proxy = Some(current.clone());
        self.save_backup(&current)?;

        let bypass_str = bypass.unwrap_or(DEFAULT_BYPASS);
        self.save_managed_state(
            &current,
            &Sysproxy {
                enable: true,
                host: host.to_string(),
                port,
                bypass: bypass_str.to_string(),
            },
        )?;
        #[cfg(target_os = "macos")]
        {
            tracing::info!(
                requested_host = %host,
                requested_port = port,
                bypass = %bypass_str,
                "Applying Bifrost system proxy to all macOS network services"
            );
            set_macos_all_services_proxy(host, port, bypass_str)?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let proxy = Sysproxy {
                enable: true,
                host: host.to_string(),
                port,
                bypass: bypass_str.to_string(),
            };

            proxy
                .set_system_proxy()
                .map_err(|e| BifrostError::Config(format!("Failed to set system proxy: {}", e)))?;
        }

        self.is_set = true;
        tracing::info!(
            "System proxy enabled: {}:{} (bypass: {})",
            host,
            port,
            bypass_str
        );

        Ok(())
    }

    pub fn disable(&mut self) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }

        if !self.is_set {
            return Ok(());
        }

        self.force_disable()
    }

    pub fn force_disable(&mut self) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            disable_macos_all_services_proxy()?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let proxy = Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            };

            proxy.set_system_proxy().map_err(|e| {
                BifrostError::Config(format!("Failed to disable system proxy: {}", e))
            })?;
        }

        self.is_set = false;
        self.original_proxy = None;
        self.remove_state_files();
        tracing::info!("System proxy force disabled");

        Ok(())
    }

    pub fn disable_if_matches(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        if !Self::is_supported() {
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        let current = Self::get_current()?;

        #[cfg(target_os = "macos")]
        let any_macos_service_matches =
            macos_any_service_proxy_matches(expected_host, expected_port).unwrap_or_else(|error| {
                tracing::warn!(
                    error = %error,
                    expected_host = %expected_host,
                    expected_port,
                    "Failed to inspect all macOS network services before system proxy disable"
                );
                false
            });

        #[cfg(not(target_os = "macos"))]
        let any_macos_service_matches = false;

        if !current.enable && !any_macos_service_matches {
            self.is_set = false;
            self.original_proxy = None;
            self.remove_state_files();
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        #[cfg(target_os = "macos")]
        let matches_expected =
            any_macos_service_matches || current.target_matches(expected_host, expected_port);

        #[cfg(not(target_os = "macos"))]
        let matches_expected = current.target_matches(expected_host, expected_port);

        if !matches_expected {
            self.is_set = false;
            self.original_proxy = None;
            self.remove_state_files();
            tracing::info!(
                current_host = %current.host,
                current_port = current.port,
                expected_host = %expected_host,
                expected_port,
                "System proxy points to another proxy; leaving it unchanged"
            );
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }

        self.restore_or_disable_current()?;
        Ok(SystemProxyDisableOutcome::Disabled)
    }

    pub fn disable_managed(&mut self) -> Result<SystemProxyDisableOutcome> {
        if !Self::is_supported() {
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        let Some(state) = self.load_managed_state().ok() else {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        };

        self.disable_if_matches(&state.target.host, state.target.port)
    }

    pub fn is_current_managed(&self, current: &ProxyBackup) -> bool {
        self.load_managed_state()
            .ok()
            .is_some_and(|state| current.target_matches(&state.target.host, state.target.port))
    }

    pub fn restore(&mut self) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }
        tracing::info!(
            data_dir = %self.data_dir.display(),
            was_set = self.is_set,
            "System proxy restore requested"
        );

        if !self.is_set {
            return Self::recover_from_crash(&self.data_dir);
        }

        let original = match self
            .original_proxy
            .take()
            .or_else(|| self.load_backup().ok())
        {
            Some(original) => original,
            None => {
                #[cfg(target_os = "macos")]
                {
                    if let Err(e) = disable_macos_all_services_proxy() {
                        let msg = e.to_string();
                        if msg.contains("RequiresAdmin") {
                            disable_macos_all_services_proxy_with_gui_auth()?;
                        } else {
                            return Err(e);
                        }
                    }
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let proxy = Sysproxy {
                        enable: false,
                        host: String::new(),
                        port: 0,
                        bypass: String::new(),
                    };
                    proxy.set_system_proxy().map_err(|e| {
                        BifrostError::Config(format!("Failed to disable system proxy: {}", e))
                    })?;
                }

                self.remove_backup();
                self.is_set = false;
                return Err(BifrostError::Config(
                    "Missing original system proxy state; disabled system proxy as failsafe"
                        .to_string(),
                ));
            }
        };

        #[cfg(target_os = "macos")]
        {
            tracing::info!(
                original_enabled = original.enable,
                original_host = %original.host,
                original_port = original.port,
                "Restoring macOS system proxy to saved original state"
            );
            let result = if original.enable {
                set_macos_all_services_proxy(&original.host, original.port, &original.bypass)
            } else {
                disable_macos_all_services_proxy()
            };
            if let Err(e) = result {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    if original.enable {
                        set_macos_all_services_proxy_with_gui_auth(
                            &original.host,
                            original.port,
                            &original.bypass,
                        )?;
                    } else {
                        disable_macos_all_services_proxy_with_gui_auth()?;
                    }
                } else {
                    return Err(e);
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            original.set_system_proxy().map_err(|e| {
                BifrostError::Config(format!("Failed to restore system proxy: {}", e))
            })?;
        }

        self.remove_state_files();
        self.is_set = false;
        tracing::info!(
            "System proxy restored to original state (enabled: {}, host: {}, port: {})",
            original.enable,
            original.host,
            original.port
        );

        Ok(())
    }

    pub fn get_current() -> Result<ProxyBackup> {
        if !Self::is_supported() {
            return Err(BifrostError::Config(
                "System proxy is not supported on this platform".to_string(),
            ));
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(proxy) = Self::parse_macos_proxy() {
                return Ok(ProxyBackup::from(&proxy));
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(proxy) = Self::parse_windows_proxy() {
                return Ok(ProxyBackup::from(&proxy));
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(proxy) = Self::parse_linux_proxy() {
                return Ok(ProxyBackup::from(&proxy));
            }
        }

        let current = Sysproxy::get_system_proxy().unwrap_or_else(|e| {
            tracing::debug!(error = %e, "[SYSTEM_PROXY] Failed to get system proxy");
            Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            }
        });

        Ok(ProxyBackup::from(&current))
    }

    #[cfg(target_os = "macos")]
    fn parse_macos_proxy() -> Option<Sysproxy> {
        let output = std::process::Command::new("scutil")
            .arg("--proxy")
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut http_enable = false;
        let mut https_enable = false;
        let mut socks_enable = false;
        let mut host = String::new();
        let mut port: u16 = 0;
        let mut bypass_list: Vec<String> = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if let Some((key, value)) = line.split_once(" : ") {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "HTTPEnable" => http_enable = value == "1",
                    "HTTPSEnable" => https_enable = value == "1",
                    "SOCKSEnable" => socks_enable = value == "1",
                    "HTTPProxy" | "HTTPSProxy" | "SOCKSProxy" if host.is_empty() => {
                        host = value.to_string();
                    }
                    "HTTPPort" | "HTTPSPort" | "SOCKSPort" if port == 0 => {
                        port = value.parse().unwrap_or(0);
                    }
                    _ => {}
                }
            } else if line.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                if let Some((_, value)) = line.split_once(" : ") {
                    bypass_list.push(value.trim().to_string());
                }
            }
        }

        let enable = http_enable || https_enable || socks_enable;
        let bypass = bypass_list.join(",");

        Some(Sysproxy {
            enable,
            host,
            port,
            bypass,
        })
    }

    #[cfg(target_os = "windows")]
    fn parse_windows_proxy() -> Option<Sysproxy> {
        use std::process::Command;

        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                "ProxyEnable",
            ])
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let enable = stdout.contains("0x1");

        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                "ProxyServer",
            ])
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (host, port) = if let Some(line) = stdout.lines().find(|l| l.contains("ProxyServer")) {
            if let Some(value) = line.split_whitespace().last() {
                if let Some((h, p)) = value.split_once(':') {
                    (h.to_string(), p.parse().unwrap_or(0))
                } else {
                    (value.to_string(), 0)
                }
            } else {
                (String::new(), 0)
            }
        } else {
            (String::new(), 0)
        };

        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                "ProxyOverride",
            ])
            .output()
            .ok();

        let bypass = output
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout
                    .lines()
                    .find(|l| l.contains("ProxyOverride"))
                    .and_then(|line| line.split_whitespace().last())
                    .map(|v| v.replace(';', ","))
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        Some(Sysproxy {
            enable,
            host,
            port,
            bypass,
        })
    }

    #[cfg(target_os = "linux")]
    fn parse_linux_proxy() -> Option<Sysproxy> {
        use std::process::Command;

        let mode_output = Command::new("gsettings")
            .args(["get", "org.gnome.system.proxy", "mode"])
            .output()
            .ok()?;

        let mode = String::from_utf8_lossy(&mode_output.stdout)
            .trim()
            .trim_matches('\'')
            .to_string();

        let enable = mode == "manual";

        if !enable {
            return Some(Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });
        }

        let host_output = Command::new("gsettings")
            .args(["get", "org.gnome.system.proxy.http", "host"])
            .output()
            .ok()?;

        let host = String::from_utf8_lossy(&host_output.stdout)
            .trim()
            .trim_matches('\'')
            .to_string();

        let port_output = Command::new("gsettings")
            .args(["get", "org.gnome.system.proxy.http", "port"])
            .output()
            .ok()?;

        let port: u16 = String::from_utf8_lossy(&port_output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);

        let bypass_output = Command::new("gsettings")
            .args(["get", "org.gnome.system.proxy", "ignore-hosts"])
            .output()
            .ok();

        let bypass = bypass_output
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let s = stdout.trim();
                if s.starts_with('[') && s.ends_with(']') {
                    s[1..s.len() - 1]
                        .split(',')
                        .map(|v| v.trim().trim_matches('\'').to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();

        Some(Sysproxy {
            enable,
            host,
            port,
            bypass,
        })
    }

    pub fn is_set(&self) -> bool {
        self.is_set
    }

    pub fn detach(mut self) {
        self.is_set = false;
        self.original_proxy = None;
    }

    fn backup_file_path(&self) -> PathBuf {
        self.data_dir.join(BACKUP_FILE_NAME)
    }

    fn state_file_path(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE_NAME)
    }

    fn save_backup(&self, proxy: &Sysproxy) -> Result<()> {
        let backup = ProxyBackup::from(proxy);
        let content = serde_json::to_string_pretty(&backup).map_err(|e| {
            BifrostError::Config(format!("Failed to serialize proxy backup: {}", e))
        })?;

        if let Some(parent) = self.backup_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(self.backup_file_path(), content)?;
        Ok(())
    }

    fn save_managed_state(&self, original: &Sysproxy, target: &Sysproxy) -> Result<()> {
        let state = ManagedProxyState {
            original: ProxyBackup::from(original),
            target: ProxyBackup::from(target),
        };
        let content = serde_json::to_string_pretty(&state)
            .map_err(|e| BifrostError::Config(format!("Failed to serialize proxy state: {}", e)))?;

        if let Some(parent) = self.state_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(self.state_file_path(), content)?;
        Ok(())
    }

    fn load_backup(&self) -> Result<Sysproxy> {
        let content = std::fs::read_to_string(self.backup_file_path())?;
        let backup: ProxyBackup = serde_json::from_str(&content).map_err(|e| {
            BifrostError::Config(format!("Failed to deserialize proxy backup: {}", e))
        })?;

        Ok(backup.into())
    }

    fn load_managed_state(&self) -> Result<ManagedProxyState> {
        let content = std::fs::read_to_string(self.state_file_path())?;
        serde_json::from_str(&content)
            .map_err(|e| BifrostError::Config(format!("Failed to deserialize proxy state: {}", e)))
    }

    fn remove_backup(&self) {
        let _ = std::fs::remove_file(self.backup_file_path());
    }

    fn remove_state_files(&self) {
        self.remove_backup();
        let _ = std::fs::remove_file(self.state_file_path());
    }

    fn restore_or_disable_current(&mut self) -> Result<()> {
        let original = self
            .original_proxy
            .take()
            .map(|proxy| ProxyBackup::from(&proxy))
            .or_else(|| self.load_managed_state().ok().map(|state| state.original))
            .or_else(|| {
                self.load_backup()
                    .ok()
                    .map(|proxy| ProxyBackup::from(&proxy))
            });

        if let Some(original) = original {
            self.apply_proxy_backup(&original)?;
        } else {
            self.force_disable()?;
            return Ok(());
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(())
    }

    fn apply_proxy_backup(&self, proxy: &ProxyBackup) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            tracing::info!(
                original_enabled = proxy.enable,
                original_host = %proxy.host,
                original_port = proxy.port,
                "Applying saved macOS system proxy backup"
            );
            let result = if proxy.enable {
                set_macos_all_services_proxy(&proxy.host, proxy.port, &proxy.bypass)
            } else {
                disable_macos_all_services_proxy()
            };
            if let Err(e) = result {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    if proxy.enable {
                        set_macos_all_services_proxy_with_gui_auth(
                            &proxy.host,
                            proxy.port,
                            &proxy.bypass,
                        )?;
                    } else {
                        disable_macos_all_services_proxy_with_gui_auth()?;
                    }
                } else {
                    return Err(e);
                }
            }
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            let proxy: Sysproxy = proxy.clone().into();
            proxy
                .set_system_proxy()
                .map_err(|e| BifrostError::Config(format!("Failed to restore system proxy: {}", e)))
        }
    }

    pub fn recover_from_crash(data_dir: &std::path::Path) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }
        tracing::info!(
            data_dir = %data_dir.display(),
            "System proxy crash recovery check starting"
        );

        let manager = Self::new(data_dir.to_path_buf());
        let state_path = data_dir.join(STATE_FILE_NAME);
        if state_path.exists() {
            let state = manager.load_managed_state()?;
            tracing::info!(
                target_host = %state.target.host,
                target_port = state.target.port,
                original_enabled = state.original.enable,
                original_host = %state.original.host,
                original_port = state.original.port,
                "Managed system proxy state found during crash recovery"
            );
            let current = Self::get_current()?;
            let decision = {
                #[cfg(target_os = "macos")]
                {
                    match macos_any_service_proxy_matches(&state.target.host, state.target.port) {
                        Ok(true) => CrashRecoveryDecision::RestoreOriginal,
                        Ok(false) => decide_managed_state_recovery(&current, &state),
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                target_host = %state.target.host,
                                target_port = state.target.port,
                                "Failed to inspect all macOS network services during crash recovery"
                            );
                            decide_managed_state_recovery(&current, &state)
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    decide_managed_state_recovery(&current, &state)
                }
            };
            match decision {
                CrashRecoveryDecision::RestoreOriginal => {
                    tracing::info!(
                        target_host = %state.target.host,
                        target_port = state.target.port,
                        original_enabled = state.original.enable,
                        original_host = %state.original.host,
                        original_port = state.original.port,
                        "Restoring original system proxy because current proxy still matches Bifrost managed target"
                    );
                    manager.apply_proxy_backup(&state.original)?;
                    tracing::info!("Recovered Bifrost-managed system proxy from previous crash");
                }
                CrashRecoveryDecision::PreserveExternal => {
                    tracing::info!(
                        current_enabled = current.enable,
                        current_host = %current.host,
                        current_port = current.port,
                        target_host = %state.target.host,
                        target_port = state.target.port,
                        "System proxy no longer points to Bifrost; preserving external proxy during crash recovery"
                    );
                }
            }
            manager.remove_state_files();
            return Ok(());
        }

        let backup_path = data_dir.join(BACKUP_FILE_NAME);
        if !backup_path.exists() {
            tracing::info!(
                data_dir = %data_dir.display(),
                "System proxy crash recovery check completed without managed state"
            );
            return Ok(());
        }

        let content = std::fs::read_to_string(&backup_path)?;
        let backup: ProxyBackup = serde_json::from_str(&content).map_err(|e| {
            BifrostError::Config(format!("Failed to deserialize proxy backup: {}", e))
        })?;
        tracing::info!(
            original_enabled = backup.enable,
            original_host = %backup.host,
            original_port = backup.port,
            "Legacy system proxy backup found during crash recovery"
        );

        let proxy: Sysproxy = backup.into();
        #[cfg(target_os = "macos")]
        {
            let result = if proxy.enable {
                set_macos_all_services_proxy(&proxy.host, proxy.port, &proxy.bypass)
            } else {
                disable_macos_all_services_proxy()
            };
            if let Err(e) = result {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    if proxy.enable {
                        set_macos_all_services_proxy_with_gui_auth(
                            &proxy.host,
                            proxy.port,
                            &proxy.bypass,
                        )?;
                    } else {
                        disable_macos_all_services_proxy_with_gui_auth()?;
                    }
                } else {
                    return Err(e);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            proxy.set_system_proxy().map_err(|e| {
                BifrostError::Config(format!("Failed to restore system proxy from crash: {}", e))
            })?;
        }

        std::fs::remove_file(&backup_path)?;
        tracing::info!("Recovered system proxy from previous crash");

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashRecoveryDecision {
    RestoreOriginal,
    PreserveExternal,
}

fn decide_managed_state_recovery(
    current: &ProxyBackup,
    state: &ManagedProxyState,
) -> CrashRecoveryDecision {
    if current.target_matches(&state.target.host, state.target.port) {
        CrashRecoveryDecision::RestoreOriginal
    } else {
        CrashRecoveryDecision::PreserveExternal
    }
}

fn proxy_hosts_match(left: &str, right: &str) -> bool {
    let left = normalize_proxy_host(left);
    let right = normalize_proxy_host(right);

    if left == right {
        return true;
    }

    matches!(
        (left.as_str(), right.as_str()),
        ("localhost", "127.0.0.1")
            | ("127.0.0.1", "localhost")
            | ("::1", "127.0.0.1")
            | ("127.0.0.1", "::1")
            | ("::1", "localhost")
            | ("localhost", "::1")
    )
}

fn normalize_proxy_host(host: &str) -> String {
    host.trim().trim_matches(['[', ']']).to_ascii_lowercase()
}

impl Drop for SystemProxyManager {
    fn drop(&mut self) {
        if self.is_set {
            if let Err(e) = self.restore() {
                tracing::error!("Failed to restore system proxy on drop: {}", e);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn list_macos_services() -> Result<Vec<String>> {
    use std::process::Command;
    let output = Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .map_err(|e| BifrostError::Config(format!("Failed to list network services: {}", e)))?;
    if !output.status.success() {
        return Err(BifrostError::Config(
            "networksetup -listallnetworkservices failed".to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut services = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if idx == 0 {
            // Skip header line
            continue;
        }
        let l = line.trim();
        if l.is_empty() || l.starts_with('*') {
            continue;
        }
        services.push(l.to_string());
    }
    Ok(services)
}

#[cfg(target_os = "macos")]
fn macos_service_proxy_matches(service: &str, getter: &str, host: &str, port: u16) -> Result<bool> {
    use std::process::Command;
    let output = Command::new("networksetup")
        .args([getter, service])
        .output()
        .map_err(|e| BifrostError::Config(format!("Failed to execute networksetup: {}", e)))?;
    if !output.status.success() {
        return Err(BifrostError::Config(format!(
            "networksetup {} failed for {}",
            getter, service
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut enabled = false;
    let mut actual_host = String::new();
    let mut actual_port = 0_u16;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Enabled" => enabled = value.eq_ignore_ascii_case("yes"),
            "Server" => actual_host = value.to_string(),
            "Port" => actual_port = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    Ok(enabled && actual_port == port && proxy_hosts_match(&actual_host, host))
}

#[cfg(target_os = "macos")]
fn macos_any_service_proxy_matches(host: &str, port: u16) -> Result<bool> {
    for service in list_macos_services()? {
        if macos_service_proxy_matches(&service, "-getwebproxy", host, port)?
            || macos_service_proxy_matches(&service, "-getsecurewebproxy", host, port)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn macos_all_services_proxy_match(host: &str, port: u16) -> Result<bool> {
    let services = list_macos_services()?;
    if services.is_empty() {
        return Ok(false);
    }

    for service in services {
        if !macos_service_proxy_matches(&service, "-getwebproxy", host, port)?
            || !macos_service_proxy_matches(&service, "-getsecurewebproxy", host, port)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(target_os = "macos")]
fn set_macos_all_services_proxy(host: &str, port: u16, bypass: &str) -> Result<()> {
    let services = list_macos_services()?;
    tracing::info!(
        service_count = services.len(),
        requested_host = %host,
        requested_port = port,
        "Setting macOS web proxies for all network services"
    );
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for svc in services {
        tracing::info!(
            service = %svc,
            requested_host = %host,
            requested_port = port,
            "Setting macOS network service proxy to requested target"
        );
        // HTTP
        run_networksetup(
            "networksetup",
            &["-setwebproxy", &svc, host, &port.to_string()],
        )?;
        run_networksetup("networksetup", &["-setwebproxystate", &svc, "on"])?;
        // HTTPS
        run_networksetup(
            "networksetup",
            &["-setsecurewebproxy", &svc, host, &port.to_string()],
        )?;
        run_networksetup("networksetup", &["-setsecurewebproxystate", &svc, "on"])?;
        // Bypass
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.clone()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup("networksetup", &str_args)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn disable_macos_all_services_proxy() -> Result<()> {
    let services = list_macos_services()?;
    tracing::info!(
        service_count = services.len(),
        "Disabling macOS web proxies for all network services"
    );
    for svc in services {
        tracing::info!(
            service = %svc,
            "Disabling macOS network service web proxies"
        );
        run_networksetup("networksetup", &["-setwebproxystate", &svc, "off"])?;
        run_networksetup("networksetup", &["-setsecurewebproxystate", &svc, "off"])?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_networksetup(cmd: &str, args: &[&str]) -> Result<()> {
    use std::process::Command;
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| BifrostError::Config(format!("Failed to execute {}: {}", cmd, e)))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msg = format!(
        "networksetup failed (code {:?}): {} {}",
        output.status.code(),
        stdout.trim(),
        stderr.trim()
    );
    if is_permission_error(&stderr) || is_permission_error(&stdout) {
        return Err(BifrostError::Config(format!("RequiresAdmin: {}", msg)));
    }
    tracing::warn!("{}", msg);
    Err(BifrostError::Config(msg))
}

#[cfg(target_os = "macos")]
fn is_permission_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("administrator")
        || lower.contains("not authorized")
        || lower.contains("permission")
        || lower.contains("require")
}

#[cfg(target_os = "macos")]
fn run_networksetup_with_gui_auth(args: &[&str]) -> Result<()> {
    use std::process::Command;

    let cmd = format!(
        "/usr/sbin/networksetup {}",
        args.iter()
            .map(|a| {
                if a.contains(' ') || a.contains('"') {
                    format!("\\\"{}\\\"", a.replace('"', "\\\\\\\""))
                } else {
                    a.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    );

    let script = format!(r#"do shell script "{}" with administrator privileges"#, cmd);

    tracing::debug!("Running osascript with command: {}", script);

    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| BifrostError::Config(format!("Failed to execute osascript: {}", e)))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Err(BifrostError::Config(
            "UserCancelled: User cancelled authorization".to_string(),
        ));
    }

    Err(BifrostError::Config(format!(
        "osascript failed: {} {}",
        stdout.trim(),
        stderr.trim()
    )))
}

#[cfg(target_os = "macos")]
pub fn set_macos_all_services_proxy_with_gui_auth(
    host: &str,
    port: u16,
    bypass: &str,
) -> Result<()> {
    let services = list_macos_services()?;
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for svc in services {
        run_networksetup_with_gui_auth(&["-setwebproxy", &svc, host, &port.to_string()])?;
        run_networksetup_with_gui_auth(&["-setwebproxystate", &svc, "on"])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxy", &svc, host, &port.to_string()])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxystate", &svc, "on"])?;
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.clone()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup_with_gui_auth(&str_args)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn disable_macos_all_services_proxy_with_gui_auth() -> Result<()> {
    let services = list_macos_services()?;
    for svc in services {
        run_networksetup_with_gui_auth(&["-setwebproxystate", &svc, "off"])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxystate", &svc, "off"])?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn set_macos_all_services_proxy_with_sudo(host: &str, port: u16, bypass: &str) -> Result<()> {
    let services = list_macos_services()?;
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for svc in services {
        // HTTP
        run_networksetup_with_sudo(&["-setwebproxy", &svc, host, &port.to_string()])?;
        run_networksetup_with_sudo(&["-setwebproxystate", &svc, "on"])?;
        // HTTPS
        run_networksetup_with_sudo(&["-setsecurewebproxy", &svc, host, &port.to_string()])?;
        run_networksetup_with_sudo(&["-setsecurewebproxystate", &svc, "on"])?;
        // Bypass
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.clone()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup_with_sudo(&str_args)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn disable_macos_all_services_proxy_with_sudo() -> Result<()> {
    let services = list_macos_services()?;
    for svc in services {
        run_networksetup_with_sudo(&["-setwebproxystate", &svc, "off"])?;
        run_networksetup_with_sudo(&["-setsecurewebproxystate", &svc, "off"])?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_networksetup_with_sudo(args: &[&str]) -> Result<()> {
    use std::process::Command;
    let output = Command::new("/usr/bin/sudo")
        .arg("networksetup")
        .args(args)
        .output()
        .map_err(|e| BifrostError::Config(format!("Failed to execute sudo networksetup: {}", e)))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msg = format!(
        "sudo networksetup failed (code {:?}): {} {}",
        output.status.code(),
        stdout.trim(),
        stderr.trim()
    );
    Err(BifrostError::Config(msg))
}

#[cfg(target_os = "macos")]
impl SystemProxyManager {
    pub fn enable_with_privilege(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<()> {
        let bypass_str = bypass.unwrap_or(DEFAULT_BYPASS);
        let current = Sysproxy::get_system_proxy().unwrap_or_else(|e| {
            tracing::warn!("Failed to get current system proxy, using default: {}", e);
            Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            }
        });
        self.original_proxy = Some(current.clone());
        self.save_backup(&current)?;
        self.save_managed_state(
            &current,
            &Sysproxy {
                enable: true,
                host: host.to_string(),
                port,
                bypass: bypass_str.to_string(),
            },
        )?;
        set_macos_all_services_proxy_with_sudo(host, port, bypass_str)?;
        self.is_set = true;
        Ok(())
    }

    pub fn disable_managed_with_privilege(&mut self) -> Result<SystemProxyDisableOutcome> {
        let Some(state) = self.load_managed_state().ok() else {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        };

        self.disable_if_matches_with_privilege(&state.target.host, state.target.port)
    }

    pub fn disable_if_matches_with_privilege(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        let current = Self::get_current()?;
        if !current.enable {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        if !current.target_matches(expected_host, expected_port) {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }

        let original = self
            .original_proxy
            .take()
            .map(|proxy| ProxyBackup::from(&proxy))
            .or_else(|| self.load_managed_state().ok().map(|state| state.original))
            .or_else(|| {
                self.load_backup()
                    .ok()
                    .map(|proxy| ProxyBackup::from(&proxy))
            })
            .unwrap_or(ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });

        if original.enable {
            set_macos_all_services_proxy_with_sudo(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else {
            disable_macos_all_services_proxy_with_sudo()?;
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(SystemProxyDisableOutcome::Disabled)
    }

    pub fn disable_with_privilege(&mut self) -> Result<()> {
        disable_macos_all_services_proxy_with_sudo()?;
        self.is_set = false;
        Ok(())
    }

    pub fn restore_with_privilege(&mut self) -> Result<()> {
        let original = self
            .original_proxy
            .take()
            .or_else(|| self.load_backup().ok())
            .unwrap_or_else(|| Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });
        if original.enable {
            set_macos_all_services_proxy_with_sudo(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else {
            disable_macos_all_services_proxy_with_sudo()?;
        }
        self.remove_backup();
        self.is_set = false;
        Ok(())
    }

    pub fn enable_with_gui_auth(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<()> {
        let bypass_str = bypass.unwrap_or(DEFAULT_BYPASS);
        let current = Sysproxy::get_system_proxy().unwrap_or_else(|e| {
            tracing::warn!("Failed to get current system proxy, using default: {}", e);
            Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            }
        });
        self.original_proxy = Some(current.clone());
        self.save_backup(&current)?;
        self.save_managed_state(
            &current,
            &Sysproxy {
                enable: true,
                host: host.to_string(),
                port,
                bypass: bypass_str.to_string(),
            },
        )?;
        set_macos_all_services_proxy_with_gui_auth(host, port, bypass_str)?;
        self.is_set = true;
        tracing::info!(
            "System proxy enabled with GUI auth: {}:{} (bypass: {})",
            host,
            port,
            bypass_str
        );
        Ok(())
    }

    pub fn disable_with_gui_auth(&mut self) -> Result<()> {
        disable_macos_all_services_proxy_with_gui_auth()?;
        self.is_set = false;
        self.remove_backup();
        tracing::info!("System proxy disabled with GUI auth");
        Ok(())
    }

    pub fn disable_if_matches_with_gui_auth(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        let current = Self::get_current()?;
        if !current.enable {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        if !current.target_matches(expected_host, expected_port) {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }

        let original = self
            .original_proxy
            .take()
            .map(|proxy| ProxyBackup::from(&proxy))
            .or_else(|| self.load_managed_state().ok().map(|state| state.original))
            .or_else(|| {
                self.load_backup()
                    .ok()
                    .map(|proxy| ProxyBackup::from(&proxy))
            })
            .unwrap_or(ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });

        if original.enable {
            set_macos_all_services_proxy_with_gui_auth(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else {
            disable_macos_all_services_proxy_with_gui_auth()?;
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(SystemProxyDisableOutcome::Disabled)
    }

    pub fn restore_with_gui_auth(&mut self) -> Result<()> {
        let original = self
            .original_proxy
            .take()
            .or_else(|| self.load_backup().ok())
            .unwrap_or_else(|| Sysproxy {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });
        if original.enable {
            set_macos_all_services_proxy_with_gui_auth(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else {
            disable_macos_all_services_proxy_with_gui_auth()?;
        }
        self.remove_backup();
        self.is_set = false;
        tracing::info!("System proxy restored with GUI auth");
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported() {
        let supported = SystemProxyManager::is_supported();
        println!("System proxy supported: {}", supported);
    }

    #[test]
    fn test_proxy_backup_serialization() {
        let backup = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: "localhost".to_string(),
        };

        let json = serde_json::to_string(&backup).unwrap();
        let restored: ProxyBackup = serde_json::from_str(&json).unwrap();

        assert_eq!(backup.enable, restored.enable);
        assert_eq!(backup.host, restored.host);
        assert_eq!(backup.port, restored.port);
        assert_eq!(backup.bypass, restored.bypass);
    }

    #[test]
    fn proxy_backup_target_matches_loopback_aliases() {
        let backup = ProxyBackup {
            enable: true,
            host: "localhost".to_string(),
            port: 8800,
            bypass: String::new(),
        };

        assert!(backup.target_matches("127.0.0.1", 8800));
        assert!(backup.target_matches("[::1]", 8800));
        assert!(!backup.target_matches("127.0.0.1", 6152));
    }

    #[test]
    fn proxy_backup_target_does_not_match_when_disabled() {
        let backup = ProxyBackup {
            enable: false,
            host: "127.0.0.1".to_string(),
            port: 8800,
            bypass: String::new(),
        };

        assert!(!backup.target_matches("127.0.0.1", 8800));
    }

    #[test]
    fn crash_recovery_restores_when_current_points_to_managed_target() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
        };
        let current = ProxyBackup {
            enable: true,
            host: "localhost".to_string(),
            port: 9900,
            bypass: String::new(),
        };

        assert_eq!(
            decide_managed_state_recovery(&current, &state),
            CrashRecoveryDecision::RestoreOriginal
        );
    }

    #[test]
    fn crash_recovery_preserves_external_proxy_on_different_port() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
        };
        let current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        };

        assert_eq!(
            decide_managed_state_recovery(&current, &state),
            CrashRecoveryDecision::PreserveExternal
        );
    }
}
