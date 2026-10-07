// Legacy cross-platform implementation is being split incrementally: macOS
// ownership/commands, durable storage, locks and tests now live in submodules.
// Follow-up refactor: move the remaining Windows/Linux adapters and legacy recovery.
#[cfg(any(target_os = "macos", test))]
mod macos_backend;
#[cfg(any(target_os = "macos", test))]
mod macos_command;
#[cfg(target_os = "macos")]
mod macos_manager;
mod macos_owned;
#[cfg(any(all(target_os = "macos", bifrost_proxy_test_io), test))]
mod macos_test_io;
mod persistence;
mod verification;
pub use verification::ManagedSystemProxyVerification;
#[cfg(not(target_os = "macos"))]
mod retarget;
#[cfg(target_os = "macos")]
use macos_manager::*;

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use std::{
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
};
use sysproxy::Sysproxy;

use crate::{BifrostError, Result};

const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1";
const BACKUP_FILE_NAME: &str = "proxy_backup.json";
const RUNTIME_FILE_NAME: &str = "runtime.json";
const STATE_FILE_NAME: &str = "proxy_state.json";
#[cfg(target_os = "macos")]
const LOCK_FILE_NAME: &str = ".system_proxy.lock";
#[cfg(target_os = "macos")]
const DEFAULT_LOCK_WAIT_TIMEOUT_MS: u64 = 60_000;
#[cfg(target_os = "macos")]
const LOCK_WAIT_LOG_INTERVAL_MS: u64 = 5_000;
#[cfg(target_os = "macos")]
const LOCK_WAIT_POLL_MS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[cfg(any(not(target_os = "macos"), test))]
fn proxy_bypass_lists_match(actual: &str, expected: &str) -> bool {
    fn normalized(value: &str) -> std::collections::BTreeSet<String> {
        value
            .split([',', ';'])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.to_ascii_lowercase())
            .collect()
    }

    normalized(actual) == normalized(expected)
}

#[cfg(any(not(target_os = "macos"), test))]
fn proxy_state_matches_expected(
    actual: &ProxyBackup,
    host: &str,
    port: u16,
    bypass: &str,
    all_services_match: Option<bool>,
) -> bool {
    let target_matches = match all_services_match {
        Some(matches) => matches,
        None => actual.enable && actual.host == host && actual.port == port,
    };
    target_matches && proxy_bypass_lists_match(&actual.bypass, bypass)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemProxyDisableOutcome {
    Disabled,
    NotEnabled,
    OwnedByOther,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagedSystemProxyOwnership {
    pub schema_version: u32,
    pub generation: String,
    pub original: ProxyBackup,
    pub target: ProxyBackup,
    pub applied: bool,
    #[serde(default)]
    pub phase: Option<ManagedSystemProxyPhase>,
    #[serde(default)]
    pub authorization_suppressed: bool,
}

/// Durable lifecycle state. Legacy `applied: false` is only a pending apply;
/// it is never sufficient evidence to resume a deliberately suspended proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSystemProxyPhase {
    PendingApply,
    Applied,
    Suspending,
    Suspended,
    Resuming,
    Restoring,
}

impl ManagedSystemProxyOwnership {
    pub fn is_suspended(&self) -> bool {
        matches!(
            self.phase,
            Some(
                ManagedSystemProxyPhase::Suspending
                    | ManagedSystemProxyPhase::Suspended
                    | ManagedSystemProxyPhase::Resuming
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedSystemProxyTransition {
    Applied,
    AlreadyInState,
    OwnershipChanged,
    NotManaged,
}

#[cfg(any(not(target_os = "macos"), test))]
fn backup_restores_managed_target(
    backup: &ProxyBackup,
    managed_target: Option<&ProxyBackup>,
) -> bool {
    managed_target.is_some_and(|target| backup.target_matches(&target.host, target.port))
}

/// Decide whether `enable` should preserve the original proxy recorded in the
/// existing on-disk managed state instead of backing up the current OS proxy.
///
/// This guards the restart / re-adoption handoff: a brand-new
/// [`SystemProxyManager`] (`is_set == false`) is asked to enable the very same
/// target that on-disk managed state already tracks, while the OS system proxy
/// still points at that target (e.g. `bifrost restart` deliberately keeps the
/// system proxy pointing at Bifrost across the daemon swap). If we backed up the
/// *current* proxy in that situation we would overwrite the genuine pre-Bifrost
/// original with Bifrost's own `host:port`, and a later crash recovery /
/// restore would "restore" the system proxy to a dead Bifrost endpoint. In that
/// case we keep the recorded original instead.
#[cfg(any(not(target_os = "macos"), test))]
fn restart_handoff_preserved_original(
    is_set: bool,
    existing_state: Option<&ManagedProxyState>,
    current_points_at_target: bool,
    host: &str,
    port: u16,
) -> Option<ProxyBackup> {
    if is_set {
        return None;
    }
    let state = existing_state?;
    if !state.target.target_matches(host, port) {
        return None;
    }
    if !current_points_at_target {
        return None;
    }
    Some(state.original.clone())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ManagedProxyState {
    #[serde(default = "managed_proxy_state_schema_version_default")]
    schema_version: u32,
    #[serde(default)]
    generation: String,
    original: ProxyBackup,
    target: ProxyBackup,
    #[serde(default = "managed_proxy_state_applied_default")]
    applied: bool,
    #[serde(default)]
    phase: Option<ManagedSystemProxyPhase>,
    #[serde(default)]
    authorization_suppressed: bool,
    #[serde(default)]
    macos_services: Vec<macos_owned::ServiceOwnership>,
}

impl ManagedProxyState {
    fn phase(&self) -> ManagedSystemProxyPhase {
        self.phase.unwrap_or(if self.applied {
            ManagedSystemProxyPhase::Applied
        } else {
            ManagedSystemProxyPhase::PendingApply
        })
    }

    fn set_phase(&mut self, phase: ManagedSystemProxyPhase) {
        self.schema_version = 3;
        self.phase = Some(phase);
        self.applied = phase == ManagedSystemProxyPhase::Applied;
    }
}

fn managed_proxy_state_schema_version_default() -> u32 {
    1
}

fn managed_proxy_state_applied_default() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub use file_lock::repair_system_proxy_lock_permissions;
#[cfg(target_os = "macos")]
pub use file_lock::repair_system_proxy_lock_permissions;
#[cfg(target_os = "macos")]
use file_lock::*;
mod file_lock;

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
    attached_generation: Option<String>,
    data_dir: PathBuf,
    #[cfg(test)]
    skip_os_proxy_io: bool,
    #[cfg(test)]
    mock_macos_state: macos_backend::SharedMockState,
}

impl SystemProxyManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            original_proxy: None,
            is_set: false,
            attached_generation: None,
            data_dir,
            #[cfg(test)]
            skip_os_proxy_io: false,
            #[cfg(test)]
            mock_macos_state: Default::default(),
        }
    }

    pub fn is_supported() -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Sysproxy::is_support()
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    fn management_available(&self) -> bool {
        #[cfg(test)]
        {
            Self::is_supported() || self.skip_os_proxy_io
        }
        #[cfg(not(test))]
        {
            Self::is_supported()
        }
    }

    fn record_system_proxy_action(&self, event_name: &str, action: &str) {
        let ownership = self.load_managed_state().ok();
        let generation = ownership.as_ref().map(|state| state.generation.clone());
        let expected_proxy = ownership.as_ref().map(|state| state.target.clone());
        let _ = crate::update_system_proxy_owner_state(&self.data_dir, |owner| {
            owner.ownership_generation = generation.clone();
            owner.expected_proxy = expected_proxy.clone();
            owner.last_action = Some(action.into());
            owner.last_error = None;
        });
        let mut event = crate::SystemProxyLifecycleEvent::new(event_name, "system_proxy_manager");
        event.ownership_generation = generation;
        event.system_proxy_action = Some(action.into());
        let _ = crate::append_system_proxy_event(&self.data_dir, &event);
    }

    #[cfg(not(target_os = "macos"))]
    pub fn enable(&mut self, host: &str, port: u16, bypass: Option<&str>) -> Result<()> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return Err(BifrostError::Config(
                "Native aggregate proxy I/O is disabled for mock managers".into(),
            ));
        }
        let bypass_str = bypass.unwrap_or(DEFAULT_BYPASS);
        if !Self::is_supported() {
            return Err(BifrostError::Config(
                "System proxy is not supported on this platform".to_string(),
            ));
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock = acquire_system_proxy_file_lock(&self.data_dir, "enable")?;

        tracing::info!(
            requested_host = %host,
            requested_port = port,
            was_set = self.is_set,
            "System proxy enable requested"
        );

        let mut preserved_original: Option<Sysproxy> = None;
        if self.is_set {
            #[cfg(target_os = "macos")]
            let all_services_match = Some(
                macos_all_services_proxy_match(host, port).unwrap_or_else(|error| {
                    tracing::warn!(
                        error = %error,
                        expected_host = %host,
                        expected_port = port,
                        "Failed to inspect all macOS network services before system proxy re-apply"
                    );
                    false
                }),
            );

            #[cfg(not(target_os = "macos"))]
            let all_services_match = None;

            if let Ok(actual) = Self::get_current() {
                if proxy_state_matches_expected(&actual, host, port, bypass_str, all_services_match)
                {
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
        } else if let Ok(existing_state) = self.load_managed_state() {
            // Restart / re-adoption handoff: a fresh manager is asked to enable
            // the same target that on-disk state already tracks while the OS
            // proxy still points at it. Preserve the recorded original so we do
            // not clobber the user's genuine pre-Bifrost proxy with Bifrost's
            // own host:port (otherwise a later crash recovery would "restore"
            // the system proxy to a dead Bifrost endpoint).
            let current_points_at_target = {
                #[cfg(target_os = "macos")]
                {
                    macos_any_service_proxy_matches(host, port).unwrap_or_else(|error| {
                        tracing::warn!(
                            error = %error,
                            expected_host = %host,
                            expected_port = port,
                            "Failed to inspect macOS network services during restart handoff backup check"
                        );
                        false
                    })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Self::get_current()
                        .map(|actual| actual.target_matches(host, port))
                        .unwrap_or(false)
                }
            };

            if let Some(original) = restart_handoff_preserved_original(
                self.is_set,
                Some(&existing_state),
                current_points_at_target,
                host,
                port,
            ) {
                tracing::info!(
                    expected_host = %host,
                    expected_port = port,
                    original_enabled = original.enable,
                    original_host = %original.host,
                    original_port = original.port,
                    "Preserving recorded original system proxy during restart handoff re-enable"
                );
                preserved_original = Some(original.into());
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

        self.save_managed_state(
            &current,
            &Sysproxy {
                enable: true,
                host: host.to_string(),
                port,
                bypass: bypass_str.to_string(),
            },
            false,
        )?;
        let mut state = self.load_managed_state()?;
        macos_owned::begin_explicit_acquisition(&mut state);
        self.write_managed_state(&state)?;
        #[cfg(target_os = "macos")]
        {
            tracing::info!(
                requested_host = %host,
                requested_port = port,
                bypass = %bypass_str,
                "Applying Bifrost system proxy to all macOS network services"
            );
            set_macos_all_services_proxy(host, port, bypass_str)?;
            if let Err(error) = self.mark_managed_state_applied() {
                tracing::warn!(
                    error = %error,
                    "failed to mark macOS system proxy state applied after enabling"
                );
            }
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
            self.mark_managed_state_applied()?;
        }

        // Preserve the live baseline selected above. A retained same-target
        // journal can contain an older original than this explicit enable.
        self.attach_managed_generation(&state.generation);
        tracing::info!(
            "System proxy enabled: {}:{} (bypass: {})",
            host,
            port,
            bypass_str
        );
        self.record_system_proxy_action("system_proxy_enabled", "enable");

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

    #[cfg(not(target_os = "macos"))]
    pub fn force_disable(&mut self) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "force_disable")?;

        self.force_disable_without_file_lock()
    }

    #[cfg(not(target_os = "macos"))]
    fn force_disable_without_file_lock(&mut self) -> Result<()> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return Err(BifrostError::Config(
                "Native aggregate proxy I/O is disabled for mock managers".into(),
            ));
        }
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

        self.record_system_proxy_action("system_proxy_force_disabled", "force_disable");
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
        self.disable_if_matches_inner(expected_host, expected_port, false)
    }

    pub fn disable_if_matches_explicit(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_inner(expected_host, expected_port, true)
    }

    #[cfg(not(target_os = "macos"))]
    fn disable_if_matches_inner(
        &mut self,
        expected_host: &str,
        expected_port: u16,
        explicit_disable: bool,
    ) -> Result<SystemProxyDisableOutcome> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return Err(BifrostError::Config(
                "Native aggregate proxy I/O is disabled for mock managers".into(),
            ));
        }
        if !Self::is_supported() {
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock = acquire_system_proxy_file_lock(
            &self.data_dir,
            if explicit_disable {
                "disable_if_matches_explicit"
            } else {
                "disable_if_matches"
            },
        )?;

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

        if explicit_disable {
            let expected_target = ProxyBackup {
                enable: true,
                host: expected_host.to_string(),
                port: expected_port,
                bypass: String::new(),
            };
            self.restore_or_disable_current_for_explicit_disable(&expected_target)?;
        } else {
            self.restore_or_disable_current()?;
        }
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

    pub fn disable_managed_explicit(&mut self) -> Result<SystemProxyDisableOutcome> {
        if !Self::is_supported() {
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        let Some(state) = self.load_managed_state().ok() else {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        };

        self.disable_if_matches_explicit(&state.target.host, state.target.port)
    }

    pub fn is_current_managed(&self, current: &ProxyBackup) -> bool {
        let Some(state) = self.load_managed_state().ok() else {
            return false;
        };

        if current.target_matches(&state.target.host, state.target.port) {
            return true;
        }

        #[cfg(target_os = "macos")]
        let service_match = self
            .macos_backend(macos_command::Privilege::Direct)
            .and_then(|mut os| {
                macos_services_match_with_backend(&mut os, &state.target.host, state.target.port)
            });
        #[cfg(not(target_os = "macos"))]
        let service_match = Self::any_service_proxy_matches(&state.target.host, state.target.port);
        service_match.unwrap_or_else(|error| {
            tracing::warn!(
                error = %error,
                target_host = %state.target.host,
                target_port = state.target.port,
                "Failed to inspect all services for managed system proxy ownership"
            );
            false
        })
    }

    pub fn any_service_proxy_matches(host: &str, port: u16) -> Result<bool> {
        #[cfg(target_os = "macos")]
        {
            macos_any_service_proxy_matches(host, port)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (host, port);
            Ok(false)
        }
    }

    pub fn managed_target_has_live_listener(data_dir: &std::path::Path) -> bool {
        let manager = Self::new(data_dir.to_path_buf());
        let Ok(state) = manager.load_managed_state() else {
            return false;
        };

        managed_target_listener_is_alive(&state.target)
    }

    pub fn ensure_managed_ownership(&self) -> Result<Option<ManagedSystemProxyOwnership>> {
        if !self.management_available() {
            return Ok(None);
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "ensure_managed_ownership")?;

        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        if state.generation.is_empty() {
            state.schema_version = 3;
            state.generation = uuid::Uuid::now_v7().to_string();
            self.write_managed_state(&state)?;
        }
        Ok(Some(state.into()))
    }

    /// Read the persisted ownership snapshot without migrating or writing it.
    /// Diagnostics use this path so `doctor` remains observational.
    pub fn read_managed_ownership(&self) -> Result<Option<ManagedSystemProxyOwnership>> {
        if !self.management_available() {
            return Ok(None);
        }
        match self.load_managed_state() {
            Ok(state) => Ok(Some(state.into())),
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn suspend_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.suspend_managed_if_generation_guarded(expected_generation, || Ok(true))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn suspend_managed_if_generation_guarded(
        &mut self,
        expected_generation: &str,
        should_suspend: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
        };
        if expected_generation.is_empty()
            || state.generation != expected_generation
            || !should_suspend()?
        {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if state.phase() == ManagedSystemProxyPhase::Suspended {
            #[cfg(test)]
            let matches = self.skip_os_proxy_io || self.current_matches_backup(&state.original)?;
            #[cfg(not(test))]
            let matches = self.current_matches_backup(&state.original)?;
            return Ok(if matches {
                GuardedSystemProxyTransition::AlreadyInState
            } else {
                GuardedSystemProxyTransition::OwnershipChanged
            });
        }
        #[cfg(test)]
        let current_matches = if self.skip_os_proxy_io {
            true
        } else {
            #[cfg(target_os = "macos")]
            {
                macos_any_service_proxy_matches(&state.target.host, state.target.port)?
            }
            #[cfg(not(target_os = "macos"))]
            {
                let current = Self::get_current()?;
                current.target_matches(&state.target.host, state.target.port)
            }
        };
        #[cfg(not(test))]
        let current_matches = {
            #[cfg(target_os = "macos")]
            {
                macos_any_service_proxy_matches(&state.target.host, state.target.port)?
            }
            #[cfg(not(target_os = "macos"))]
            {
                let current = Self::get_current()?;
                current.target_matches(&state.target.host, state.target.port)
            }
        };
        if !guarded_suspend_allowed(&state, expected_generation, current_matches) {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        state.set_phase(ManagedSystemProxyPhase::Suspending);
        self.write_managed_state(&state)?;
        #[cfg(test)]
        if !self.skip_os_proxy_io {
            self.apply_proxy_backup_for_target(&state.original, Some(&state.target))?;
        }
        #[cfg(not(test))]
        self.apply_proxy_backup_for_target(&state.original, Some(&state.target))?;
        state.applied = false;
        state.phase = Some(ManagedSystemProxyPhase::Suspended);
        state.schema_version = 3;
        self.write_managed_state(&state)?;
        self.is_set = false;
        self.original_proxy = None;
        self.record_system_proxy_action("system_proxy_fail_open_suspended", "suspend");
        Ok(GuardedSystemProxyTransition::Applied)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn resume_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "resume_managed_if_generation")?;

        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
        };
        if state.generation != expected_generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if state.applied {
            #[cfg(test)]
            let matches = self.skip_os_proxy_io || self.current_matches_backup(&state.target)?;
            #[cfg(not(test))]
            let matches = self.current_matches_backup(&state.target)?;
            if !matches {
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            self.attach_managed_state(&state);
            return Ok(GuardedSystemProxyTransition::AlreadyInState);
        }
        #[cfg(test)]
        let current_matches_original = if self.skip_os_proxy_io {
            true
        } else {
            self.current_matches_backup(&state.original)?
                || (matches!(
                    state.phase(),
                    ManagedSystemProxyPhase::Suspending | ManagedSystemProxyPhase::Resuming
                ) && self.current_matches_backup(&state.target)?)
        };
        #[cfg(not(test))]
        let current_matches_original = self.current_matches_backup(&state.original)?
            || (matches!(
                state.phase(),
                ManagedSystemProxyPhase::Suspending | ManagedSystemProxyPhase::Resuming
            ) && self.current_matches_backup(&state.target)?);
        if !guarded_resume_allowed(&state, expected_generation, current_matches_original) {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        state.set_phase(ManagedSystemProxyPhase::Resuming);
        self.write_managed_state(&state)?;
        #[cfg(test)]
        if !self.skip_os_proxy_io {
            self.apply_proxy_backup(&state.target)?;
        }
        #[cfg(not(test))]
        self.apply_proxy_backup(&state.target)?;
        state.applied = true;
        state.phase = Some(ManagedSystemProxyPhase::Applied);
        state.schema_version = 3;
        self.write_managed_state(&state)?;
        self.attach_managed_state(&state);
        self.record_system_proxy_action("system_proxy_generation_resumed", "resume");
        Ok(GuardedSystemProxyTransition::Applied)
    }

    #[cfg(not(target_os = "macos"))]
    fn current_matches_backup(&self, expected: &ProxyBackup) -> Result<bool> {
        #[cfg(target_os = "macos")]
        {
            if expected.enable {
                return macos_all_services_proxy_match(&expected.host, expected.port);
            }
            Ok(!macos_any_service_proxy_enabled()?)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = Self::get_current()?;
            Ok(if expected.enable {
                current.target_matches(&expected.host, expected.port)
                    && proxy_bypass_lists_match(&current.bypass, &expected.bypass)
            } else {
                !current.enable
            })
        }
    }

    pub fn last_runtime_target_has_live_listener(data_dir: &std::path::Path) -> bool {
        let Some(target) = load_last_runtime_proxy_target(data_dir) else {
            return false;
        };

        managed_target_listener_is_alive(&target)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn restore(&mut self) -> Result<()> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return self.restore_mock_aggregate();
        }
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
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock = acquire_system_proxy_file_lock(&self.data_dir, "restore")?;

        #[cfg(target_os = "macos")]
        let managed_target = self.load_managed_state().ok().map(|state| state.target);

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
            let original_backup = ProxyBackup::from(&original);
            self.apply_proxy_backup_for_target(&original_backup, managed_target.as_ref())?;
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

    #[cfg(not(target_os = "macos"))]
    pub fn get_current() -> Result<ProxyBackup> {
        if !Self::is_supported() {
            return Err(BifrostError::Config(
                "System proxy is not supported on this platform".to_string(),
            ));
        }

        #[cfg(target_os = "macos")]
        {
            return Self::parse_macos_proxy()
                .map(|proxy| ProxyBackup::from(&proxy))
                .ok_or_else(|| {
                    BifrostError::Config(
                        "networksetup/scutil could not read current macOS proxy settings".into(),
                    )
                });
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
        let output = macos_command::run_bounded(
            "/usr/sbin/scutil",
            &["--proxy"],
            std::time::Duration::from_secs(10),
        )
        .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut proxy = macos_command::parse_scutil_proxy(&stdout)?;
        if !proxy.enable && (proxy.host.is_empty() || proxy.port == 0) {
            if let Some((host, port)) = macos_stored_proxy_endpoint() {
                proxy.host = host;
                proxy.port = port;
            }
        }
        Some(proxy.into())
    }

    #[cfg(target_os = "windows")]
    fn parse_windows_proxy() -> Option<Sysproxy> {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let settings = match hkcu
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        {
            Ok(key) => key,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "[SYSTEM_PROXY] Failed to open Windows Internet Settings registry key"
                );
                return Some(Sysproxy {
                    enable: false,
                    host: String::new(),
                    port: 0,
                    bypass: String::new(),
                });
            }
        };

        let enable = settings
            .get_value::<u32, _>("ProxyEnable")
            .map(|value| value != 0)
            .unwrap_or(false);
        let proxy_server = settings
            .get_value::<String, _>("ProxyServer")
            .unwrap_or_default();
        let (host, port) = Self::parse_windows_proxy_server(&proxy_server);
        let bypass = settings
            .get_value::<String, _>("ProxyOverride")
            .unwrap_or_default()
            .replace(';', ",");

        Some(Sysproxy {
            enable,
            host,
            port,
            bypass,
        })
    }

    #[cfg(target_os = "windows")]
    fn parse_windows_proxy_server(proxy_server: &str) -> (String, u16) {
        let proxy_server = proxy_server.trim();
        if proxy_server.is_empty() {
            return (String::new(), 0);
        }

        let target = proxy_server
            .split(';')
            .filter_map(|entry| entry.split_once('='))
            .find_map(|(scheme, value)| {
                let scheme = scheme.trim().to_ascii_lowercase();
                if matches!(scheme.as_str(), "http" | "https" | "socks" | "socks5") {
                    Some(value.trim())
                } else {
                    None
                }
            })
            .unwrap_or(proxy_server);

        Self::parse_windows_proxy_host_port(target)
    }

    #[cfg(target_os = "windows")]
    fn parse_windows_proxy_host_port(value: &str) -> (String, u16) {
        let value = value.trim();
        if let Some((host, port)) = value.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.trim().trim_matches(['[', ']']).to_string(), port);
            }
        }
        (value.trim_matches(['[', ']']).to_string(), 0)
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
        self.detach_in_place();
    }

    pub fn detach_in_place(&mut self) {
        self.attached_generation = None;
        self.is_set = false;
        self.original_proxy = None;
    }

    fn backup_file_path(&self) -> PathBuf {
        self.data_dir.join(BACKUP_FILE_NAME)
    }

    fn state_file_path(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE_NAME)
    }

    #[cfg(not(target_os = "macos"))]
    fn save_backup(&self, proxy: &Sysproxy) -> Result<()> {
        let backup = ProxyBackup::from(proxy);
        let content = serde_json::to_string_pretty(&backup).map_err(|e| {
            BifrostError::Config(format!("Failed to serialize proxy backup: {}", e))
        })?;

        if let Some(parent) = self.backup_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        persistence::atomic_write(&self.backup_file_path(), content.as_bytes())?;
        Ok(())
    }

    #[cfg(any(not(target_os = "macos"), test))]
    fn save_managed_state(
        &self,
        original: &Sysproxy,
        target: &Sysproxy,
        applied: bool,
    ) -> Result<()> {
        let existing = self
            .load_managed_state()
            .ok()
            .filter(|state| state.target.target_matches(&target.host, target.port));
        let state = ManagedProxyState {
            schema_version: 3,
            generation: existing
                .as_ref()
                .map(|state| state.generation.clone())
                .filter(|generation| !generation.is_empty())
                .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            original: existing
                .as_ref()
                .map(|state| state.original.clone())
                .unwrap_or_else(|| ProxyBackup::from(original)),
            target: ProxyBackup::from(target),
            applied,
            phase: Some(if applied {
                ManagedSystemProxyPhase::Applied
            } else {
                ManagedSystemProxyPhase::PendingApply
            }),
            authorization_suppressed: existing
                .as_ref()
                .is_some_and(|state| state.authorization_suppressed),
            macos_services: existing
                .map(|state| state.macos_services)
                .unwrap_or_default(),
        };
        self.write_managed_state(&state)
    }

    fn write_managed_state(&self, state: &ManagedProxyState) -> Result<()> {
        let content = serde_json::to_string_pretty(&state)
            .map_err(|e| BifrostError::Config(format!("Failed to serialize proxy state: {}", e)))?;

        if let Some(parent) = self.state_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        persistence::atomic_write(&self.state_file_path(), content.as_bytes())?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn mark_managed_state_applied(&self) -> Result<()> {
        let mut state = self.load_managed_state()?;
        if state.applied {
            return Ok(());
        }
        state.applied = true;
        state.phase = Some(ManagedSystemProxyPhase::Applied);
        self.write_managed_state(&state)
    }

    #[cfg(not(target_os = "macos"))]
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

    #[cfg(not(target_os = "macos"))]
    fn remove_backup(&self) {
        let _ = std::fs::remove_file(self.backup_file_path());
    }

    #[cfg(not(target_os = "macos"))]
    fn remove_state_files(&self) {
        self.remove_backup();
        let _ = std::fs::remove_file(self.state_file_path());
    }

    #[cfg(not(target_os = "macos"))]
    fn restore_or_disable_current(&mut self) -> Result<()> {
        let managed_state = self.load_managed_state().ok();
        let managed_target = managed_state.as_ref().map(|state| state.target.clone());
        let original = self.load_original_proxy_backup(managed_state);

        if let Some(original) = original {
            self.apply_proxy_backup_for_target(&original, managed_target.as_ref())?;
        } else {
            self.force_disable_without_file_lock()?;
            return Ok(());
        }

        self.record_system_proxy_action("system_proxy_restored", "restore_original");
        self.remove_state_files();
        self.is_set = false;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn restore_or_disable_current_for_explicit_disable(
        &mut self,
        expected_target: &ProxyBackup,
    ) -> Result<()> {
        let managed_state = self.load_managed_state().ok();
        let managed_target = managed_state
            .as_ref()
            .map(|state| state.target.clone())
            .unwrap_or_else(|| expected_target.clone());
        let original = self.load_original_proxy_backup(managed_state);

        if let Some(original) = original {
            if !backup_restores_managed_target(&original, Some(&managed_target)) {
                self.apply_proxy_backup_for_target(&original, Some(&managed_target))?;
                self.remove_state_files();
                self.is_set = false;
                return Ok(());
            }

            tracing::info!(
                original_host = %original.host,
                original_port = original.port,
                target_host = %managed_target.host,
                target_port = managed_target.port,
                "explicit system proxy disable ignored saved backup because it points back to the managed Bifrost target"
            );
        }

        let disabled = ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        };
        self.apply_proxy_backup_for_target(&disabled, Some(&managed_target))?;
        self.remove_state_files();
        self.is_set = false;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn load_original_proxy_backup(
        &mut self,
        managed_state: Option<ManagedProxyState>,
    ) -> Option<ProxyBackup> {
        self.original_proxy
            .take()
            .map(|proxy| ProxyBackup::from(&proxy))
            .or_else(|| managed_state.map(|state| state.original))
            .or_else(|| {
                self.load_backup()
                    .ok()
                    .map(|proxy| ProxyBackup::from(&proxy))
            })
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_proxy_backup(&self, proxy: &ProxyBackup) -> Result<()> {
        self.apply_proxy_backup_for_target(proxy, None)
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_proxy_backup_for_target(
        &self,
        proxy: &ProxyBackup,
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] target: Option<&ProxyBackup>,
    ) -> Result<()> {
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return Err(BifrostError::Config(
                "Native aggregate proxy I/O is disabled for mock managers".into(),
            ));
        }
        #[cfg(target_os = "macos")]
        {
            tracing::info!(
                original_enabled = proxy.enable,
                original_host = %proxy.host,
                original_port = proxy.port,
                target_host = target.map(|target| target.host.as_str()).unwrap_or(""),
                target_port = target.map(|target| target.port).unwrap_or(0),
                "Applying saved macOS system proxy backup"
            );
            let result = apply_macos_proxy_backup(proxy, target);
            if let Err(e) = result {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    apply_macos_proxy_backup_with_gui_auth(proxy, target)?;
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

    #[cfg(not(target_os = "macos"))]
    pub fn recover_from_crash(data_dir: &std::path::Path) -> Result<()> {
        if !Self::is_supported() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(data_dir, "recover_from_crash")?;

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
                    match decide_macos_managed_state_recovery(
                        &current,
                        &state,
                        macos_any_service_proxy_matches(&state.target.host, state.target.port),
                    ) {
                        Ok(decision) => decision,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                target_host = %state.target.host,
                                target_port = state.target.port,
                                "Failed to inspect all macOS network services during crash recovery; preserving managed state for retry"
                            );
                            return Err(error);
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
                    manager.apply_proxy_backup_for_target(&state.original, Some(&state.target))?;
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
                CrashRecoveryDecision::DiscardPendingApply => {
                    tracing::info!(
                        target_host = %state.target.host,
                        target_port = state.target.port,
                        "Pending system proxy state was never applied; removing stale state without changing current proxy"
                    );
                }
            }
            manager.remove_state_files();
            return Ok(());
        }

        let backup_path = data_dir.join(BACKUP_FILE_NAME);
        if !backup_path.exists() {
            if let Some(runtime_target) = load_last_runtime_proxy_target(data_dir) {
                let current = Self::get_current()?;
                let current_matches_runtime = {
                    #[cfg(target_os = "macos")]
                    {
                        match decide_macos_runtime_target_match(macos_any_service_proxy_matches(
                            &runtime_target.host,
                            runtime_target.port,
                        )) {
                            Ok(matches) => matches,
                            Err(error) => {
                                tracing::debug!(
                                    error = %error,
                                    target_host = %runtime_target.host,
                                    target_port = runtime_target.port,
                                    "Failed to inspect macOS network services for last runtime target during crash recovery; preserving runtime state for retry"
                                );
                                return Err(error);
                            }
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        current_proxy_matches_target(&current, &runtime_target)
                    }
                };

                if current_matches_runtime {
                    let disabled = ProxyBackup {
                        enable: false,
                        host: String::new(),
                        port: 0,
                        bypass: String::new(),
                    };
                    tracing::info!(
                        target_host = %runtime_target.host,
                        target_port = runtime_target.port,
                        "No managed proxy state found, but current system proxy matches last Bifrost runtime target; disabling stale Bifrost proxy"
                    );
                    manager.apply_proxy_backup_for_target(&disabled, Some(&runtime_target))?;
                    manager.remove_state_files();
                    tracing::info!("Recovered stale Bifrost system proxy from last runtime target");
                    return Ok(());
                }

                tracing::info!(
                    current_enabled = current.enable,
                    current_host = %current.host,
                    current_port = current.port,
                    target_host = %runtime_target.host,
                    target_port = runtime_target.port,
                    "No managed proxy state found and current system proxy does not match last Bifrost runtime target"
                );
            }
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

        manager.apply_proxy_backup(&backup)?;

        std::fs::remove_file(&backup_path)?;
        tracing::info!("Recovered system proxy from previous crash");

        Ok(())
    }
}

impl From<ManagedProxyState> for ManagedSystemProxyOwnership {
    fn from(state: ManagedProxyState) -> Self {
        Self {
            schema_version: state.schema_version,
            generation: state.generation,
            original: state.original,
            target: state.target,
            applied: state.applied,
            phase: state.phase,
            authorization_suppressed: state.authorization_suppressed,
        }
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn guarded_suspend_allowed(
    state: &ManagedProxyState,
    expected_generation: &str,
    current_matches_target: bool,
) -> bool {
    (matches!(
        state.phase(),
        ManagedSystemProxyPhase::Applied
            | ManagedSystemProxyPhase::Suspending
            | ManagedSystemProxyPhase::Resuming
    ) || state.phase == Some(ManagedSystemProxyPhase::PendingApply))
        && !expected_generation.is_empty()
        && state.generation == expected_generation
        && current_matches_target
}

#[cfg(any(not(target_os = "macos"), test))]
fn guarded_resume_allowed(
    state: &ManagedProxyState,
    expected_generation: &str,
    current_matches_original: bool,
) -> bool {
    matches!(
        state.phase(),
        ManagedSystemProxyPhase::Suspended
            | ManagedSystemProxyPhase::Suspending
            | ManagedSystemProxyPhase::Resuming
    ) && !expected_generation.is_empty()
        && state.generation == expected_generation
        && current_matches_original
}

#[cfg(any(not(target_os = "macos"), test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashRecoveryDecision {
    RestoreOriginal,
    PreserveExternal,
    DiscardPendingApply,
}

#[cfg(any(not(target_os = "macos"), test))]
fn decide_managed_state_recovery(
    current: &ProxyBackup,
    state: &ManagedProxyState,
) -> CrashRecoveryDecision {
    if !state.applied && !current_proxy_matches_target(current, &state.target) {
        return CrashRecoveryDecision::DiscardPendingApply;
    }

    if current_proxy_matches_target(current, &state.target) {
        CrashRecoveryDecision::RestoreOriginal
    } else {
        CrashRecoveryDecision::PreserveExternal
    }
}

#[cfg(test)]
fn decide_macos_managed_state_recovery(
    current: &ProxyBackup,
    state: &ManagedProxyState,
    service_match: Result<bool>,
) -> Result<CrashRecoveryDecision> {
    match service_match {
        Ok(true) => Ok(CrashRecoveryDecision::RestoreOriginal),
        Ok(false) => Ok(decide_managed_state_recovery(current, state)),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
fn decide_macos_runtime_target_match(service_match: Result<bool>) -> Result<bool> {
    service_match
}

#[cfg(any(not(target_os = "macos"), test))]
fn current_proxy_matches_target(current: &ProxyBackup, target: &ProxyBackup) -> bool {
    current.target_matches(&target.host, target.port)
}

fn load_last_runtime_proxy_target(data_dir: &Path) -> Option<ProxyBackup> {
    let content = std::fs::read_to_string(data_dir.join(RUNTIME_FILE_NAME)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let port = value
        .get("port")
        .and_then(|port| port.as_u64())
        .filter(|port| *port > 0 && *port <= u16::MAX as u64)
        .map(|port| port as u16)?;
    let host = value
        .get("host")
        .and_then(|host| host.as_str())
        .map(runtime_host_to_system_proxy_host)
        .unwrap_or_else(|| "127.0.0.1".to_string());

    Some(ProxyBackup {
        enable: true,
        host,
        port,
        bypass: String::new(),
    })
}

fn runtime_host_to_system_proxy_host(host: &str) -> String {
    match normalize_proxy_host(host).as_str() {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        normalized => normalized.to_string(),
    }
}

fn managed_target_listener_is_alive(target: &ProxyBackup) -> bool {
    use std::net::ToSocketAddrs;

    if !target.enable || target.port == 0 {
        return false;
    }
    let host = match normalize_proxy_host(&target.host).as_str() {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        host => host.to_string(),
    };
    let Ok(socket_addrs) = (host.as_str(), target.port).to_socket_addrs() else {
        return false;
    };
    let socket_addrs = socket_addrs.collect::<Vec<_>>();
    if socket_addrs.is_empty() {
        return false;
    }
    let timeout = std::time::Duration::from_millis(750);
    for attempt in 1..=3 {
        for socket_addr in &socket_addrs {
            if std::net::TcpStream::connect_timeout(socket_addr, timeout).is_ok() {
                tracing::info!(
                    target_host = %target.host,
                    target_port = target.port,
                    resolved_addr = %socket_addr,
                    attempt,
                    "Managed system proxy target still has a live listener"
                );
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    false
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
            if matches!(
                crate::read_system_proxy_shutdown_mode(&self.data_dir),
                Some(crate::SystemProxyShutdownMode::ForegroundCleanup)
                    | Some(crate::SystemProxyShutdownMode::PreserveForRestart)
            ) {
                tracing::info!(
                    data_dir = %self.data_dir.display(),
                    "system proxy manager drop restore skipped because shutdown marker owns cleanup"
                );
                self.detach_in_place();
                return;
            }
            if let Err(e) = self.restore() {
                tracing::error!("Failed to restore system proxy on drop: {}", e);
            }
        }
    }
}

#[cfg(test)]
fn parse_macos_networksetup_proxy(output: &str) -> (bool, String, u16) {
    let mut enabled = false;
    let mut host = String::new();
    let mut port = 0_u16;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "Enabled" => enabled = value.trim().eq_ignore_ascii_case("yes"),
            "Server" => host = value.trim().to_string(),
            "Port" => port = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    (enabled, host, port)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod guarded_suspend_tests;

#[cfg(test)]
mod authorization_tests;
