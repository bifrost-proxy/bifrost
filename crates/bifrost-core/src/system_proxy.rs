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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemProxyDisableOutcome {
    Disabled,
    NotEnabled,
    OwnedByOther,
}

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
    original: ProxyBackup,
    target: ProxyBackup,
    #[serde(default = "managed_proxy_state_applied_default")]
    applied: bool,
}

fn managed_proxy_state_applied_default() -> bool {
    true
}

#[cfg(target_os = "macos")]
struct SystemProxyFileLock {
    file: File,
    context: &'static str,
}

#[cfg(target_os = "macos")]
impl Drop for SystemProxyFileLock {
    fn drop(&mut self) {
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            tracing::warn!(
                context = self.context,
                error = %std::io::Error::last_os_error(),
                "failed to release system proxy cross-process file lock"
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn acquire_system_proxy_file_lock(
    data_dir: &Path,
    context: &'static str,
) -> Result<SystemProxyFileLock> {
    std::fs::create_dir_all(data_dir)?;
    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let file = match open_system_proxy_lock_file(data_dir, true) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            repair_system_proxy_lock_permissions_with_gui_auth(data_dir)?;
            open_system_proxy_lock_file(data_dir, true)?
        }
        Err(error) => return Err(error.into()),
    };
    tracing::info!(
        data_dir = %data_dir.display(),
        lock_path = %lock_path.display(),
        context,
        "waiting for system proxy cross-process file lock"
    );
    wait_for_system_proxy_file_lock(&file, data_dir, &lock_path, context)?;
    tracing::info!(
        data_dir = %data_dir.display(),
        lock_path = %lock_path.display(),
        context,
        "acquired system proxy cross-process file lock"
    );
    Ok(SystemProxyFileLock { file, context })
}

#[cfg(target_os = "macos")]
fn wait_for_system_proxy_file_lock(
    file: &File,
    data_dir: &Path,
    lock_path: &Path,
    context: &'static str,
) -> Result<()> {
    let timeout = system_proxy_lock_wait_timeout();
    let started = Instant::now();
    let mut next_log_at = Duration::ZERO;

    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        let would_block = error.raw_os_error() == Some(libc::EWOULDBLOCK)
            || error.raw_os_error() == Some(libc::EAGAIN);
        if !would_block {
            return Err(error.into());
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(BifrostError::Config(format!(
                "Timed out after {}ms waiting for system proxy cross-process file lock (context={context}, data_dir={}, lock_path={}). Another Bifrost process may be stuck while changing macOS system proxy settings.",
                timeout.as_millis(),
                data_dir.display(),
                lock_path.display()
            )));
        }

        if elapsed >= next_log_at {
            tracing::warn!(
                data_dir = %data_dir.display(),
                lock_path = %lock_path.display(),
                context,
                elapsed_ms = elapsed.as_millis(),
                timeout_ms = timeout.as_millis(),
                "still waiting for system proxy cross-process file lock"
            );
            next_log_at = elapsed + Duration::from_millis(LOCK_WAIT_LOG_INTERVAL_MS);
        }

        let remaining = timeout.saturating_sub(elapsed);
        std::thread::sleep(std::cmp::min(
            Duration::from_millis(LOCK_WAIT_POLL_MS),
            remaining,
        ));
    }
}

#[cfg(target_os = "macos")]
fn system_proxy_lock_wait_timeout() -> Duration {
    system_proxy_lock_wait_timeout_from_env(
        std::env::var("BIFROST_SYSTEM_PROXY_LOCK_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn system_proxy_lock_wait_timeout_from_env(value: Option<&str>) -> Duration {
    let timeout_ms = value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LOCK_WAIT_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

#[cfg(target_os = "macos")]
fn open_system_proxy_lock_file(data_dir: &Path, create: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .truncate(false)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    if create {
        options.create(true).mode(0o666);
    }
    let file = options.open(&lock_path)?;
    relax_lock_file_mode_if_needed(&file, &lock_path)?;
    Ok(file)
}

#[cfg(target_os = "macos")]
pub fn repair_system_proxy_lock_permissions(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let _ = open_system_proxy_lock_file(data_dir, true)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn repair_system_proxy_lock_permissions(_data_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn relax_lock_file_mode_if_needed(file: &File, lock_path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::other(format!(
            "Refusing to use non-regular system proxy lock file: {}",
            lock_path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(std::io::Error::other(format!(
            "Refusing to use hard-linked system proxy lock file: {}",
            lock_path.display()
        )));
    }

    let current_mode = metadata.permissions().mode() & 0o777;
    if current_mode != 0o666 {
        let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o666) };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        tracing::info!(
            target: "bifrost_core::system_proxy",
            lock_path = %lock_path.display(),
            previous_mode = format!("{:o}", current_mode),
            "relaxed system proxy lock file permissions to 0666"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn repair_system_proxy_lock_permissions_with_gui_auth(data_dir: &Path) -> Result<()> {
    let program = std::env::current_exe().map_err(|error| {
        BifrostError::Config(format!("Failed to resolve current executable: {error}"))
    })?;
    let shell_command = format!(
        "{} system-proxy repair-lock --data-dir {}",
        shell_quote_path(&program),
        shell_quote_path(data_dir)
    );
    let script = format!(
        r#"do shell script "{}" with administrator privileges with prompt "{}""#,
        escape_apple_script(&shell_command),
        escape_apple_script("Bifrost needs to repair the system proxy cleanup lock file.")
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| BifrostError::Config(format!("Failed to execute osascript: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "RequiresAdmin: failed to repair system proxy lock permissions: {} {}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(target_os = "macos")]
fn shell_quote_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn escape_apple_script(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
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
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Sysproxy::is_support()
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    pub fn enable(&mut self, host: &str, port: u16, bypass: Option<&str>) -> Result<()> {
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

        let bypass_str = bypass.unwrap_or(DEFAULT_BYPASS);
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
                    if all_services_match && proxy_bypass_lists_match(&actual.bypass, bypass_str) {
                        return Ok(());
                    }
                } else if actual.enable
                    && actual.host == host
                    && actual.port == port
                    && proxy_bypass_lists_match(&actual.bypass, bypass_str)
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
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "force_disable")?;

        self.force_disable_without_file_lock()
    }

    fn force_disable_without_file_lock(&mut self) -> Result<()> {
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
        self.disable_if_matches_inner(expected_host, expected_port, false)
    }

    pub fn disable_if_matches_explicit(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_inner(expected_host, expected_port, true)
    }

    fn disable_if_matches_inner(
        &mut self,
        expected_host: &str,
        expected_port: u16,
        explicit_disable: bool,
    ) -> Result<SystemProxyDisableOutcome> {
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

        Self::any_service_proxy_matches(&state.target.host, state.target.port).unwrap_or_else(
            |error| {
                tracing::warn!(
                    error = %error,
                    target_host = %state.target.host,
                    target_port = state.target.port,
                    "Failed to inspect all services for managed system proxy ownership"
                );
                false
            },
        )
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

    pub fn last_runtime_target_has_live_listener(data_dir: &std::path::Path) -> bool {
        let Some(target) = load_last_runtime_proxy_target(data_dir) else {
            return false;
        };

        managed_target_listener_is_alive(&target)
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
        self.is_set = false;
        self.original_proxy = None;
    }

    pub fn detach_in_place(&mut self) {
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

    fn save_managed_state(
        &self,
        original: &Sysproxy,
        target: &Sysproxy,
        applied: bool,
    ) -> Result<()> {
        let state = ManagedProxyState {
            original: ProxyBackup::from(original),
            target: ProxyBackup::from(target),
            applied,
        };
        self.write_managed_state(&state)
    }

    fn write_managed_state(&self, state: &ManagedProxyState) -> Result<()> {
        let content = serde_json::to_string_pretty(&state)
            .map_err(|e| BifrostError::Config(format!("Failed to serialize proxy state: {}", e)))?;

        if let Some(parent) = self.state_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(self.state_file_path(), content)?;
        Ok(())
    }

    fn mark_managed_state_applied(&self) -> Result<()> {
        let mut state = self.load_managed_state()?;
        if state.applied {
            return Ok(());
        }
        state.applied = true;
        self.write_managed_state(&state)
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
        let managed_state = self.load_managed_state().ok();
        let managed_target = managed_state.as_ref().map(|state| state.target.clone());
        let original = self.load_original_proxy_backup(managed_state);

        if let Some(original) = original {
            self.apply_proxy_backup_for_target(&original, managed_target.as_ref())?;
        } else {
            self.force_disable_without_file_lock()?;
            return Ok(());
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(())
    }

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

    fn apply_proxy_backup(&self, proxy: &ProxyBackup) -> Result<()> {
        self.apply_proxy_backup_for_target(proxy, None)
    }

    fn apply_proxy_backup_for_target(
        &self,
        proxy: &ProxyBackup,
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] target: Option<&ProxyBackup>,
    ) -> Result<()> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashRecoveryDecision {
    RestoreOriginal,
    PreserveExternal,
    DiscardPendingApply,
}

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

#[cfg(any(target_os = "macos", test))]
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

#[cfg(any(target_os = "macos", test))]
fn decide_macos_runtime_target_match(service_match: Result<bool>) -> Result<bool> {
    service_match
}

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
    if services.is_empty() {
        return Err(BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".to_string(),
        ));
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
    Ok(!macos_services_proxy_matches(host, port)?.is_empty())
}

#[cfg(target_os = "macos")]
fn macos_services_proxy_matches(host: &str, port: u16) -> Result<Vec<String>> {
    use std::sync::mpsc;
    use std::thread;

    let services = list_macos_services()?;
    let host = host.to_string();
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::with_capacity(services.len());

    for service in services {
        let tx = tx.clone();
        let host = host.clone();
        handles.push(thread::spawn(move || {
            let matches = macos_service_proxy_matches(&service, "-getwebproxy", &host, port)
                .and_then(|web_matches| {
                    if web_matches {
                        Ok(true)
                    } else {
                        macos_service_proxy_matches(&service, "-getsecurewebproxy", &host, port)
                    }
                });
            let _ = tx.send((service, matches));
        }));
    }
    drop(tx);

    let mut matching_services = Vec::new();
    let mut first_error = None;
    for (service, result) in rx {
        match result {
            Ok(true) => matching_services.push(service),
            Ok(false) => {}
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(matching_services)
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
const MACOS_NETWORKSETUP_MAX_PARALLEL_SERVICES: usize = 4;

#[cfg(target_os = "macos")]
fn run_macos_services_parallel<F>(
    services: &[String],
    operation: &'static str,
    run_service: F,
) -> Result<()>
where
    F: Fn(&str) -> Result<()> + Send + Sync,
{
    let parallelism = MACOS_NETWORKSETUP_MAX_PARALLEL_SERVICES
        .max(1)
        .min(services.len().max(1));
    let mut first_error = None;

    for chunk in services.chunks(parallelism) {
        let run_service = &run_service;
        let results = std::thread::scope(|scope| {
            let handles = chunk
                .iter()
                .map(|service| {
                    scope.spawn(move || {
                        let service_started_at = Instant::now();
                        let result = run_service(service);
                        match &result {
                            Ok(()) => tracing::info!(
                                service = %service,
                                operation,
                                elapsed_ms = service_started_at.elapsed().as_millis(),
                                "macOS network service proxy operation completed"
                            ),
                            Err(error) => tracing::warn!(
                                service = %service,
                                operation,
                                error = %error,
                                elapsed_ms = service_started_at.elapsed().as_millis(),
                                "macOS network service proxy operation failed"
                            ),
                        }
                        result
                    })
                })
                .collect::<Vec<_>>();

            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        Err(BifrostError::Config(format!(
                            "macOS networksetup {operation} worker panicked"
                        )))
                    })
                })
                .collect::<Vec<_>>()
        });

        for result in results {
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(target_os = "macos")]
fn set_macos_all_services_proxy(host: &str, port: u16, bypass: &str) -> Result<()> {
    let services = list_macos_services()?;
    set_macos_services_proxy(&services, host, port, bypass)
}

#[cfg(target_os = "macos")]
fn set_macos_services_proxy(
    services: &[String],
    host: &str,
    port: u16,
    bypass: &str,
) -> Result<()> {
    let started_at = Instant::now();
    tracing::info!(
        service_count = services.len(),
        requested_host = %host,
        requested_port = port,
        "Setting macOS web proxies for selected network services"
    );
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    run_macos_services_parallel(services, "set", |svc| {
        tracing::info!(
            service = %svc,
            requested_host = %host,
            requested_port = port,
            "Setting macOS network service proxy to requested target"
        );
        let port = port.to_string();
        // HTTP
        run_networksetup("networksetup", &["-setwebproxy", svc, host, &port])?;
        run_networksetup("networksetup", &["-setwebproxystate", svc, "on"])?;
        // HTTPS
        run_networksetup("networksetup", &["-setsecurewebproxy", svc, host, &port])?;
        run_networksetup("networksetup", &["-setsecurewebproxystate", svc, "on"])?;
        // Bypass
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.to_string()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup("networksetup", &str_args)?;
        }
        Ok(())
    })?;
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service proxy set completed"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn disable_macos_all_services_proxy() -> Result<()> {
    let services = list_macos_services()?;
    disable_macos_services_proxy(&services)
}

#[cfg(target_os = "macos")]
fn disable_macos_services_proxy(services: &[String]) -> Result<()> {
    let started_at = Instant::now();
    tracing::info!(
        service_count = services.len(),
        "Disabling macOS web proxies for selected network services"
    );
    run_macos_services_parallel(services, "disable", |svc| {
        tracing::info!(
            service = %svc,
            "Disabling macOS network service web proxies"
        );
        run_networksetup("networksetup", &["-setwebproxystate", svc, "off"])?;
        run_networksetup("networksetup", &["-setsecurewebproxystate", svc, "off"])?;
        Ok(())
    })?;
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service proxy disable completed"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_macos_proxy_backup(proxy: &ProxyBackup, target: Option<&ProxyBackup>) -> Result<()> {
    let services = if let Some(target) = target {
        let services = macos_services_proxy_matches(&target.host, target.port)?;
        tracing::info!(
            target_host = %target.host,
            target_port = target.port,
            matching_service_count = services.len(),
            "Selected macOS network services still pointing at Bifrost target for restore"
        );
        services
    } else {
        list_macos_services()?
    };

    if services.is_empty() {
        tracing::info!(
            target_host = target.map(|target| target.host.as_str()).unwrap_or(""),
            target_port = target.map(|target| target.port).unwrap_or(0),
            "No macOS network services require system proxy restore"
        );
        return Ok(());
    }

    if proxy.enable {
        set_macos_services_proxy(&services, &proxy.host, proxy.port, &proxy.bypass)
    } else {
        disable_macos_services_proxy(&services)
    }
}

#[cfg(target_os = "macos")]
fn apply_macos_proxy_backup_with_gui_auth(
    proxy: &ProxyBackup,
    target: Option<&ProxyBackup>,
) -> Result<()> {
    let services = if let Some(target) = target {
        let services = macos_services_proxy_matches(&target.host, target.port)?;
        tracing::info!(
            target_host = %target.host,
            target_port = target.port,
            matching_service_count = services.len(),
            "Selected macOS network services still pointing at Bifrost target for GUI-auth restore"
        );
        services
    } else {
        list_macos_services()?
    };

    if services.is_empty() {
        tracing::info!("No macOS network services require GUI-auth system proxy restore");
        return Ok(());
    }

    if proxy.enable {
        set_macos_services_proxy_with_gui_auth(&services, &proxy.host, proxy.port, &proxy.bypass)
    } else {
        disable_macos_services_proxy_with_gui_auth(&services)
    }
}

#[cfg(target_os = "macos")]
fn set_macos_services_proxy_with_gui_auth(
    services: &[String],
    host: &str,
    port: u16,
    bypass: &str,
) -> Result<()> {
    let started_at = Instant::now();
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for svc in services {
        let service_started_at = Instant::now();
        run_networksetup_with_gui_auth(&["-setwebproxy", svc, host, &port.to_string()])?;
        run_networksetup_with_gui_auth(&["-setwebproxystate", svc, "on"])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxy", svc, host, &port.to_string()])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxystate", svc, "on"])?;
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.clone()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup_with_gui_auth(&str_args)?;
        }
        tracing::info!(
            service = %svc,
            elapsed_ms = service_started_at.elapsed().as_millis(),
            "macOS network service GUI-auth proxy set completed"
        );
    }
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service GUI-auth proxy set completed"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn disable_macos_services_proxy_with_gui_auth(services: &[String]) -> Result<()> {
    let started_at = Instant::now();
    for svc in services {
        let service_started_at = Instant::now();
        run_networksetup_with_gui_auth(&["-setwebproxystate", svc, "off"])?;
        run_networksetup_with_gui_auth(&["-setsecurewebproxystate", svc, "off"])?;
        tracing::info!(
            service = %svc,
            elapsed_ms = service_started_at.elapsed().as_millis(),
            "macOS network service GUI-auth proxy disable completed"
        );
    }
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service GUI-auth proxy disable completed"
    );
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
    set_macos_services_proxy_with_gui_auth(&services, host, port, bypass)
}

#[cfg(target_os = "macos")]
pub fn disable_macos_all_services_proxy_with_gui_auth() -> Result<()> {
    let services = list_macos_services()?;
    disable_macos_services_proxy_with_gui_auth(&services)
}

#[cfg(target_os = "macos")]
fn disable_macos_matching_services_proxy_with_gui_auth(target: &ProxyBackup) -> Result<()> {
    let services = macos_services_proxy_matches(&target.host, target.port)?;
    disable_macos_services_proxy_with_gui_auth(&services)
}

#[cfg(target_os = "macos")]
pub fn set_macos_all_services_proxy_with_sudo(host: &str, port: u16, bypass: &str) -> Result<()> {
    let services = list_macos_services()?;
    let started_at = Instant::now();
    tracing::info!(
        service_count = services.len(),
        requested_host = %host,
        requested_port = port,
        "Setting macOS web proxies with sudo for selected network services"
    );
    let bypass_domains: Vec<String> = bypass
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    run_macos_services_parallel(&services, "sudo_set", |svc| {
        let port = port.to_string();
        // HTTP
        run_networksetup_with_sudo(&["-setwebproxy", svc, host, &port])?;
        run_networksetup_with_sudo(&["-setwebproxystate", svc, "on"])?;
        // HTTPS
        run_networksetup_with_sudo(&["-setsecurewebproxy", svc, host, &port])?;
        run_networksetup_with_sudo(&["-setsecurewebproxystate", svc, "on"])?;
        // Bypass
        if !bypass_domains.is_empty() {
            let mut args = vec!["-setproxybypassdomains".to_string(), svc.to_string()];
            args.extend(bypass_domains.iter().cloned());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_networksetup_with_sudo(&str_args)?;
        }
        Ok(())
    })?;
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service sudo proxy set completed"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn disable_macos_all_services_proxy_with_sudo() -> Result<()> {
    let services = list_macos_services()?;
    disable_macos_services_proxy_with_sudo(&services)
}

#[cfg(target_os = "macos")]
fn disable_macos_matching_services_proxy_with_sudo(target: &ProxyBackup) -> Result<()> {
    let services = macos_services_proxy_matches(&target.host, target.port)?;
    disable_macos_services_proxy_with_sudo(&services)
}

#[cfg(target_os = "macos")]
fn disable_macos_services_proxy_with_sudo(services: &[String]) -> Result<()> {
    let started_at = Instant::now();
    run_macos_services_parallel(services, "sudo_disable", |svc| {
        run_networksetup_with_sudo(&["-setwebproxystate", svc, "off"])?;
        run_networksetup_with_sudo(&["-setsecurewebproxystate", svc, "off"])?;
        Ok(())
    })?;
    tracing::info!(
        service_count = services.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "macOS selected network service sudo proxy disable completed"
    );
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
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "enable_with_privilege")?;

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
            false,
        )?;
        set_macos_all_services_proxy_with_sudo(host, port, bypass_str)?;
        if let Err(error) = self.mark_managed_state_applied() {
            tracing::warn!(
                error = %error,
                "failed to mark macOS system proxy state applied after privileged enable"
            );
        }
        self.is_set = true;
        Ok(())
    }

    pub fn disable_managed_with_privilege(&mut self) -> Result<SystemProxyDisableOutcome> {
        let Some(state) = self.load_managed_state().ok() else {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        };

        self.disable_if_matches_with_privilege(&state.target.host, state.target.port)
    }

    pub fn disable_managed_explicit_with_privilege(&mut self) -> Result<SystemProxyDisableOutcome> {
        let Some(state) = self.load_managed_state().ok() else {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        };

        self.disable_if_matches_explicit_with_privilege(&state.target.host, state.target.port)
    }

    pub fn disable_if_matches_with_privilege(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_with_privilege_inner(expected_host, expected_port, false)
    }

    pub fn disable_if_matches_explicit_with_privilege(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_with_privilege_inner(expected_host, expected_port, true)
    }

    fn disable_if_matches_with_privilege_inner(
        &mut self,
        expected_host: &str,
        expected_port: u16,
        explicit_disable: bool,
    ) -> Result<SystemProxyDisableOutcome> {
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock = acquire_system_proxy_file_lock(
            &self.data_dir,
            if explicit_disable {
                "disable_if_matches_explicit_with_privilege"
            } else {
                "disable_if_matches_with_privilege"
            },
        )?;

        let current = Self::get_current()?;

        let any_macos_service_matches =
            macos_any_service_proxy_matches(expected_host, expected_port).unwrap_or_else(|error| {
                tracing::warn!(
                    error = %error,
                    expected_host = %expected_host,
                    expected_port,
                    "Failed to inspect all macOS network services before privileged system proxy disable"
                );
                false
            });

        if !current.enable && !any_macos_service_matches {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        let matches_expected =
            any_macos_service_matches || current.target_matches(expected_host, expected_port);

        if !matches_expected {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }

        let managed_state = self.load_managed_state().ok();
        let expected_target = ProxyBackup {
            enable: true,
            host: expected_host.to_string(),
            port: expected_port,
            bypass: String::new(),
        };
        let managed_target = managed_state
            .as_ref()
            .map(|state| state.target.clone())
            .unwrap_or(expected_target);
        let original = self
            .load_original_proxy_backup(managed_state)
            .unwrap_or(ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });

        let dirty_backup_restores_managed_target =
            explicit_disable && backup_restores_managed_target(&original, Some(&managed_target));

        if original.enable && !dirty_backup_restores_managed_target {
            set_macos_all_services_proxy_with_sudo(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else if explicit_disable {
            if dirty_backup_restores_managed_target {
                tracing::info!(
                    original_host = %original.host,
                    original_port = original.port,
                    target_host = %managed_target.host,
                    target_port = managed_target.port,
                    "explicit system proxy disable ignored saved backup because it points back to the managed Bifrost target"
                );
            }
            disable_macos_matching_services_proxy_with_sudo(&managed_target)?;
        } else {
            disable_macos_all_services_proxy_with_sudo()?;
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(SystemProxyDisableOutcome::Disabled)
    }

    pub fn disable_with_privilege(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "disable_with_privilege")?;

        disable_macos_all_services_proxy_with_sudo()?;
        self.is_set = false;
        Ok(())
    }

    pub fn restore_with_privilege(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "restore_with_privilege")?;

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
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "enable_with_gui_auth")?;

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
            false,
        )?;
        set_macos_all_services_proxy_with_gui_auth(host, port, bypass_str)?;
        if let Err(error) = self.mark_managed_state_applied() {
            tracing::warn!(
                error = %error,
                "failed to mark macOS system proxy state applied after GUI-auth enable"
            );
        }
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
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "disable_with_gui_auth")?;

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
        self.disable_if_matches_with_gui_auth_inner(expected_host, expected_port, false)
    }

    pub fn disable_if_matches_explicit_with_gui_auth(
        &mut self,
        expected_host: &str,
        expected_port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_with_gui_auth_inner(expected_host, expected_port, true)
    }

    fn disable_if_matches_with_gui_auth_inner(
        &mut self,
        expected_host: &str,
        expected_port: u16,
        explicit_disable: bool,
    ) -> Result<SystemProxyDisableOutcome> {
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock = acquire_system_proxy_file_lock(
            &self.data_dir,
            if explicit_disable {
                "disable_if_matches_explicit_with_gui_auth"
            } else {
                "disable_if_matches_with_gui_auth"
            },
        )?;

        let current = Self::get_current()?;

        let any_macos_service_matches = macos_any_service_proxy_matches(
            expected_host,
            expected_port,
        )
        .unwrap_or_else(|error| {
            tracing::warn!(
                error = %error,
                expected_host = %expected_host,
                expected_port,
                "Failed to inspect all macOS network services before GUI-auth system proxy disable"
            );
            false
        });

        if !current.enable && !any_macos_service_matches {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::NotEnabled);
        }

        let matches_expected =
            any_macos_service_matches || current.target_matches(expected_host, expected_port);

        if !matches_expected {
            self.remove_state_files();
            self.is_set = false;
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }

        let managed_state = self.load_managed_state().ok();
        let expected_target = ProxyBackup {
            enable: true,
            host: expected_host.to_string(),
            port: expected_port,
            bypass: String::new(),
        };
        let managed_target = managed_state
            .as_ref()
            .map(|state| state.target.clone())
            .unwrap_or(expected_target);
        let original = self
            .load_original_proxy_backup(managed_state)
            .unwrap_or(ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            });

        let dirty_backup_restores_managed_target =
            explicit_disable && backup_restores_managed_target(&original, Some(&managed_target));

        if original.enable && !dirty_backup_restores_managed_target {
            set_macos_all_services_proxy_with_gui_auth(
                &original.host,
                original.port,
                &original.bypass,
            )?;
        } else if explicit_disable {
            if dirty_backup_restores_managed_target {
                tracing::info!(
                    original_host = %original.host,
                    original_port = original.port,
                    target_host = %managed_target.host,
                    target_port = managed_target.port,
                    "explicit system proxy disable ignored saved backup because it points back to the managed Bifrost target"
                );
            }
            disable_macos_matching_services_proxy_with_gui_auth(&managed_target)?;
        } else {
            disable_macos_all_services_proxy_with_gui_auth()?;
        }

        self.remove_state_files();
        self.is_set = false;
        Ok(SystemProxyDisableOutcome::Disabled)
    }

    pub fn restore_with_gui_auth(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _system_proxy_file_lock =
            acquire_system_proxy_file_lock(&self.data_dir, "restore_with_gui_auth")?;

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
    fn normalize_proxy_host_trims_brackets_and_lowercases() {
        assert_eq!(normalize_proxy_host("  [::1] "), "::1");
        assert_eq!(normalize_proxy_host("LOCALHOST"), "localhost");
        assert_eq!(normalize_proxy_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(normalize_proxy_host(""), "");
    }

    #[test]
    fn proxy_hosts_match_loopback_aliases() {
        assert!(proxy_hosts_match("localhost", "127.0.0.1"));
        assert!(proxy_hosts_match("127.0.0.1", "localhost"));
        assert!(proxy_hosts_match("::1", "127.0.0.1"));
        assert!(proxy_hosts_match("127.0.0.1", "::1"));
        assert!(proxy_hosts_match("::1", "localhost"));
        assert!(proxy_hosts_match("localhost", "::1"));
        // Exact normalized match.
        assert!(proxy_hosts_match("[::1]", "::1"));
        assert!(proxy_hosts_match("EXAMPLE.com", "example.com"));
        // Non-matching distinct hosts.
        assert!(!proxy_hosts_match("10.0.0.1", "10.0.0.2"));
    }

    #[test]
    fn runtime_host_to_system_proxy_host_maps_wildcards() {
        assert_eq!(runtime_host_to_system_proxy_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(runtime_host_to_system_proxy_host("::"), "127.0.0.1");
        assert_eq!(runtime_host_to_system_proxy_host(""), "127.0.0.1");
        assert_eq!(runtime_host_to_system_proxy_host("10.1.2.3"), "10.1.2.3");
        assert_eq!(runtime_host_to_system_proxy_host("[::1]"), "::1");
    }

    #[test]
    fn current_proxy_matches_target_delegates_to_target_matches() {
        let current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        let target = ProxyBackup {
            enable: true,
            host: "localhost".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        assert!(current_proxy_matches_target(&current, &target));

        let other = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 1111,
            bypass: String::new(),
        };
        assert!(!current_proxy_matches_target(&current, &other));
    }

    fn make_state(applied: bool, target_port: u16) -> ManagedProxyState {
        ManagedProxyState {
            original: ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: target_port,
                bypass: String::new(),
            },
            applied,
        }
    }

    #[test]
    fn decide_managed_state_recovery_branches() {
        let matching_current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        let mismatching_current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        };

        // applied + current matches target -> restore.
        assert_eq!(
            decide_managed_state_recovery(&matching_current, &make_state(true, 9900)),
            CrashRecoveryDecision::RestoreOriginal
        );
        // applied + current does NOT match -> preserve external.
        assert_eq!(
            decide_managed_state_recovery(&mismatching_current, &make_state(true, 9900)),
            CrashRecoveryDecision::PreserveExternal
        );
        // not applied + current does NOT match -> discard pending apply.
        assert_eq!(
            decide_managed_state_recovery(&mismatching_current, &make_state(false, 9900)),
            CrashRecoveryDecision::DiscardPendingApply
        );
        // not applied + current matches -> restore.
        assert_eq!(
            decide_managed_state_recovery(&matching_current, &make_state(false, 9900)),
            CrashRecoveryDecision::RestoreOriginal
        );
    }

    #[test]
    fn decide_macos_managed_state_recovery_uses_service_match() {
        let current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        let state = make_state(true, 9900);

        assert_eq!(
            decide_macos_managed_state_recovery(&current, &state, Ok(true)).unwrap(),
            CrashRecoveryDecision::RestoreOriginal
        );
        // service_match false -> delegate to decide_managed_state_recovery.
        assert_eq!(
            decide_macos_managed_state_recovery(&current, &state, Ok(false)).unwrap(),
            CrashRecoveryDecision::RestoreOriginal
        );
        // Err propagates.
        assert!(decide_macos_managed_state_recovery(
            &current,
            &state,
            Err(BifrostError::Config("boom".to_string()))
        )
        .is_err());
    }

    #[test]
    fn decide_macos_runtime_target_match_passthrough() {
        assert!(decide_macos_runtime_target_match(Ok(true)).unwrap());
        assert!(!decide_macos_runtime_target_match(Ok(false)).unwrap());
        assert!(
            decide_macos_runtime_target_match(Err(BifrostError::Config("x".to_string()))).is_err()
        );
    }

    #[test]
    fn restart_handoff_preserved_original_default_applied_field() {
        // ManagedProxyState deserialized without `applied` defaults to true.
        let json = r#"{
            "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
            "target": {"enable": true, "host": "127.0.0.1", "port": 9900, "bypass": ""}
        }"#;
        let state: ManagedProxyState = serde_json::from_str(json).unwrap();
        assert!(state.applied);
    }

    #[test]
    fn load_last_runtime_proxy_target_reads_runtime_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(RUNTIME_FILE_NAME),
            r#"{"host": "0.0.0.0", "port": 9911}"#,
        )
        .unwrap();
        let target = load_last_runtime_proxy_target(dir.path()).unwrap();
        assert_eq!(target.port, 9911);
        // 0.0.0.0 wildcard mapped to loopback.
        assert_eq!(target.host, "127.0.0.1");
        assert!(target.enable);
    }

    #[test]
    fn load_last_runtime_proxy_target_missing_or_invalid() {
        let dir = tempfile::tempdir().unwrap();
        // No file at all.
        assert!(load_last_runtime_proxy_target(dir.path()).is_none());
        // Port out of range / zero -> None.
        std::fs::write(
            dir.path().join(RUNTIME_FILE_NAME),
            r#"{"host": "127.0.0.1", "port": 0}"#,
        )
        .unwrap();
        assert!(load_last_runtime_proxy_target(dir.path()).is_none());
        // Missing host -> defaults to 127.0.0.1.
        std::fs::write(dir.path().join(RUNTIME_FILE_NAME), r#"{"port": 8080}"#).unwrap();
        let t = load_last_runtime_proxy_target(dir.path()).unwrap();
        assert_eq!(t.host, "127.0.0.1");
        assert_eq!(t.port, 8080);
    }

    #[test]
    fn managed_target_listener_is_alive_false_when_disabled_or_no_listener() {
        // Disabled target -> false immediately.
        let disabled = ProxyBackup {
            enable: false,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        assert!(!managed_target_listener_is_alive(&disabled));

        // Port 0 -> false.
        let zero_port = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 0,
            bypass: String::new(),
        };
        assert!(!managed_target_listener_is_alive(&zero_port));
    }

    #[test]
    fn system_proxy_disable_outcome_eq() {
        assert_eq!(
            SystemProxyDisableOutcome::Disabled,
            SystemProxyDisableOutcome::Disabled
        );
        assert_ne!(
            SystemProxyDisableOutcome::Disabled,
            SystemProxyDisableOutcome::OwnedByOther
        );
        assert!(format!("{:?}", SystemProxyDisableOutcome::NotEnabled).contains("NotEnabled"));
    }

    #[test]
    fn enable_disable_unsupported_on_non_macos_windows() {
        // On Linux is_supported() is false, so enable returns a Config error.
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let dir = tempfile::tempdir().unwrap();
            let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
            assert!(manager.enable("127.0.0.1", 9900, None).is_err());
        }
    }

    #[test]
    fn test_is_supported() {
        let supported = SystemProxyManager::is_supported();
        println!("System proxy supported: {}", supported);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(supported, Sysproxy::is_support());
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert!(!supported);
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
    fn drop_skips_restore_when_restart_shutdown_marker_owns_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        crate::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            crate::SystemProxyShutdownMode::PreserveForRestart,
        )
        .unwrap();

        let mut manager = SystemProxyManager::new(temp_dir.path().to_path_buf());
        manager.original_proxy = Some(Sysproxy {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        });
        manager.is_set = true;

        drop(manager);

        assert!(matches!(
            crate::read_system_proxy_shutdown_mode(temp_dir.path()),
            Some(crate::SystemProxyShutdownMode::PreserveForRestart)
        ));
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
    fn proxy_bypass_match_ignores_order_case_and_empty_entries() {
        assert!(proxy_bypass_lists_match(
            " localhost;*.LOCAL,127.0.0.1,,",
            "127.0.0.1,localhost,*.local"
        ));
        assert!(!proxy_bypass_lists_match(
            "localhost,127.0.0.1",
            "localhost,127.0.0.1,corp.example"
        ));
    }

    #[test]
    fn explicit_disable_detects_backup_that_restores_managed_target() {
        let backup = ProxyBackup {
            enable: true,
            host: "localhost".to_string(),
            port: 9900,
            bypass: "different.example".to_string(),
        };
        let target = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: "localhost,127.0.0.1".to_string(),
        };

        assert!(backup_restores_managed_target(&backup, Some(&target)));
    }

    #[test]
    fn explicit_disable_preserves_backup_for_external_proxy() {
        let backup = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        };
        let target = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };

        assert!(!backup_restores_managed_target(&backup, Some(&target)));
    }

    #[test]
    fn restart_handoff_preserves_recorded_original_when_all_conditions_met() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "10.0.0.1".to_string(),
                port: 7070,
                bypass: "corp.example".to_string(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: true,
        };

        let preserved =
            restart_handoff_preserved_original(false, Some(&state), true, "127.0.0.1", 9900)
                .expect("should preserve recorded original");

        assert!(preserved.enable);
        assert_eq!(preserved.host, "10.0.0.1");
        assert_eq!(preserved.port, 7070);
        assert_eq!(preserved.bypass, "corp.example");
    }

    #[test]
    fn restart_handoff_does_not_preserve_when_manager_already_set() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "10.0.0.1".to_string(),
                port: 7070,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: true,
        };

        // is_set == true means the live manager owns its own original; the
        // existing in-process backup path handles preservation instead.
        assert!(
            restart_handoff_preserved_original(true, Some(&state), true, "127.0.0.1", 9900)
                .is_none()
        );
    }

    #[test]
    fn restart_handoff_does_not_preserve_without_existing_state() {
        assert!(restart_handoff_preserved_original(false, None, true, "127.0.0.1", 9900).is_none());
    }

    #[test]
    fn restart_handoff_does_not_preserve_when_target_mismatches() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "10.0.0.1".to_string(),
                port: 7070,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: true,
        };

        // The requested host:port does not match the on-disk managed target, so
        // this is a genuine fresh enable and the current proxy must be backed up.
        assert!(
            restart_handoff_preserved_original(false, Some(&state), true, "127.0.0.1", 6152)
                .is_none()
        );
    }

    #[test]
    fn restart_handoff_does_not_preserve_when_current_not_pointing_at_target() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "10.0.0.1".to_string(),
                port: 7070,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: true,
        };

        // The OS proxy no longer points at the managed target, so we cannot
        // assume this is a restart handoff; fall back to backing up the current
        // proxy rather than blindly trusting stale recorded state.
        assert!(
            restart_handoff_preserved_original(false, Some(&state), false, "127.0.0.1", 9900)
                .is_none()
        );
    }

    #[test]
    fn managed_target_listener_detects_live_loopback_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let target = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port,
            bypass: String::new(),
        };

        assert!(managed_target_listener_is_alive(&target));
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
            applied: true,
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
            applied: true,
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

    #[test]
    fn crash_recovery_discards_pending_apply_when_target_was_never_set() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 6152,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: false,
        };
        let current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        };

        assert_eq!(
            decide_managed_state_recovery(&current, &state),
            CrashRecoveryDecision::DiscardPendingApply
        );
    }

    #[test]
    fn crash_recovery_restores_pending_apply_when_target_is_visible() {
        let state = ManagedProxyState {
            original: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 6152,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".to_string(),
                port: 9900,
                bypass: String::new(),
            },
            applied: false,
        };
        let current = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };

        assert_eq!(
            decide_managed_state_recovery(&current, &state),
            CrashRecoveryDecision::RestoreOriginal
        );
    }

    #[test]
    fn macos_recovery_keeps_managed_state_when_services_are_not_ready() {
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
            applied: true,
        };
        let current = ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        };
        let error = BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".to_string(),
        );

        let result = decide_macos_managed_state_recovery(&current, &state, Err(error));

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No enabled macOS network services"));
    }

    #[test]
    fn macos_runtime_recovery_keeps_runtime_state_when_services_are_not_ready() {
        let error = BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".to_string(),
        );

        let result = decide_macos_runtime_target_match(Err(error));

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No enabled macOS network services"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_proxy_lock_timeout_env_defaults_and_overrides() {
        assert_eq!(
            system_proxy_lock_wait_timeout_from_env(None),
            std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
        );
        assert_eq!(
            system_proxy_lock_wait_timeout_from_env(Some("250")),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            system_proxy_lock_wait_timeout_from_env(Some("0")),
            std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
        );
        assert_eq!(
            system_proxy_lock_wait_timeout_from_env(Some("not-a-number")),
            std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_proxy_lock_is_world_writable_after_creation() {
        use std::os::unix::fs::PermissionsExt;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-system-proxy-lock-mode-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        // Drop the lock before chmod-checking; releasing flock is fine, but
        // we want to inspect the persisted mode bits.
        {
            let _lock = acquire_system_proxy_file_lock(&data_dir, "test_mode_create")
                .expect("acquire fresh lock");
        }
        let lock_path = data_dir.join(LOCK_FILE_NAME);
        let mode = std::fs::metadata(&lock_path)
            .expect("stat lock")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o666, "lock file mode should be 0o666 on creation");

        // Tighten the mode and re-acquire: the helper must heal it back to 0o666.
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))
            .expect("tighten lock mode");
        {
            let _lock = acquire_system_proxy_file_lock(&data_dir, "test_mode_relax")
                .expect("re-acquire lock");
        }
        let mode = std::fs::metadata(&lock_path)
            .expect("stat lock")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o666, "lock file mode should be relaxed to 0o666");
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_proxy_lock_rejects_symlink() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-system-proxy-lock-symlink-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        let target = data_dir.join("target");
        std::fs::write(&target, "target").expect("write target");
        let lock_path = data_dir.join(LOCK_FILE_NAME);
        std::os::unix::fs::symlink(&target, &lock_path).expect("create symlink");

        let result = acquire_system_proxy_file_lock(&data_dir, "test_symlink");

        assert!(result.is_err(), "symlink lock must be rejected");
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_proxy_file_lock_serializes_cross_process_entries() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!("bifrost-system-proxy-lock-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        let first =
            acquire_system_proxy_file_lock(&data_dir, "test_first").expect("acquire first lock");
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_data_dir = data_dir.clone();

        let handle = std::thread::spawn(move || {
            let _second = acquire_system_proxy_file_lock(&thread_data_dir, "test_second")
                .expect("acquire second lock");
            tx.send(()).expect("send acquired");
        });

        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "second lock acquired before first lock was released"
        );
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .expect("second lock acquired after first lock release");
        handle.join().expect("join lock thread");
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn last_runtime_proxy_target_reads_runtime_host_and_port() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-target-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        std::fs::write(
            data_dir.join(RUNTIME_FILE_NAME),
            r#"{"pid":12345,"host":"localhost","port":18889}"#,
        )
        .expect("write runtime");

        let target = load_last_runtime_proxy_target(&data_dir).expect("runtime target");

        assert_eq!(target.host, "localhost");
        assert_eq!(target.port, 18889);
        assert!(target.enable);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn last_runtime_proxy_target_maps_wildcard_host_to_loopback() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-runtime-wildcard-target-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        std::fs::write(
            data_dir.join(RUNTIME_FILE_NAME),
            r#"{"pid":12345,"host":"0.0.0.0","port":9900}"#,
        )
        .expect("write runtime");

        let target = load_last_runtime_proxy_target(&data_dir).expect("runtime target");

        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 9900);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn last_runtime_proxy_target_ignores_invalid_or_missing_port() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-runtime-invalid-target-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        std::fs::write(
            data_dir.join(RUNTIME_FILE_NAME),
            r#"{"pid":12345,"host":"127.0.0.1","port":70000}"#,
        )
        .expect("write runtime");

        assert!(load_last_runtime_proxy_target(&data_dir).is_none());
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn current_proxy_matches_last_runtime_target_with_loopback_alias() {
        let current = ProxyBackup {
            enable: true,
            host: "localhost".to_string(),
            port: 9900,
            bypass: String::new(),
        };
        let target = ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 9900,
            bypass: String::new(),
        };

        assert!(current_proxy_matches_target(&current, &target));
    }

    #[test]
    fn last_runtime_target_has_live_listener_detects_runtime_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-runtime-listener-target-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        std::fs::write(
            data_dir.join(RUNTIME_FILE_NAME),
            format!(r#"{{"pid":12345,"host":"127.0.0.1","port":{port}}}"#),
        )
        .expect("write runtime");

        assert!(SystemProxyManager::last_runtime_target_has_live_listener(
            &data_dir
        ));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn last_runtime_target_has_live_listener_resolves_localhost() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("bifrost-runtime-localhost-target-{unique}"));
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        std::fs::write(
            data_dir.join(RUNTIME_FILE_NAME),
            format!(r#"{{"pid":12345,"host":"localhost","port":{port}}}"#),
        )
        .expect("write runtime");

        assert!(SystemProxyManager::last_runtime_target_has_live_listener(
            &data_dir
        ));
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
