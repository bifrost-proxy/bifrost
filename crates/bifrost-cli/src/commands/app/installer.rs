use super::*;

#[cfg(any(target_os = "windows", test))]
pub(super) struct WindowsDesktopInstallSnapshot {
    pub(super) install_dir: PathBuf,
    pub(super) backup: tempfile::TempDir,
    pub(super) had_previous_install: bool,
}

#[cfg(any(target_os = "windows", test))]
impl WindowsDesktopInstallSnapshot {
    fn capture(install_dir: &Path) -> Result<Self, BifrostError> {
        let backup =
            tempfile::tempdir().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
        let had_previous_install = install_dir.exists();
        if had_previous_install {
            copy_dir_recursive(install_dir, backup.path())?;
        }
        Ok(Self {
            install_dir: install_dir.to_path_buf(),
            backup,
            had_previous_install,
        })
    }

    pub(super) fn restore(self) -> Result<(), BifrostError> {
        let parent = self.install_dir.parent().ok_or_else(|| {
            BifrostError::Config(format!(
                "desktop app install directory has no parent: {}",
                self.install_dir.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        let failed = parent.join(format!(".Bifrost.failed-upgrade-{}", std::process::id()));
        remove_path_if_exists(&failed)?;
        if self.install_dir.exists() {
            fs::rename(&self.install_dir, &failed)?;
        }

        let restore_result = if self.had_previous_install {
            copy_dir_recursive(self.backup.path(), &self.install_dir)
        } else {
            Ok(())
        };
        if let Err(error) = restore_result {
            let _ = remove_path_if_exists(&self.install_dir);
            if failed.exists() {
                let _ = fs::rename(&failed, &self.install_dir);
            }
            return Err(error);
        }
        remove_path_if_exists(&failed)
    }

    fn installed_tree_is_unchanged(&self) -> bool {
        self.had_previous_install
            && paths_have_same_contents(&self.install_dir, self.backup.path()).unwrap_or(false)
    }
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn paths_have_same_contents(left: &Path, right: &Path) -> io::Result<bool> {
    let left_metadata = fs::symlink_metadata(left)?;
    let right_metadata = fs::symlink_metadata(right)?;
    if left_metadata.file_type() != right_metadata.file_type() {
        return Ok(false);
    }

    if left_metadata.is_file() {
        if left_metadata.len() != right_metadata.len() {
            return Ok(false);
        }
        let mut left_file = fs::File::open(left)?;
        let mut right_file = fs::File::open(right)?;
        let mut left_buffer = [0_u8; 64 * 1024];
        let mut right_buffer = [0_u8; 64 * 1024];
        loop {
            let left_read = left_file.read(&mut left_buffer)?;
            let right_read = right_file.read(&mut right_buffer)?;
            if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
                return Ok(false);
            }
            if left_read == 0 {
                return Ok(true);
            }
        }
    }

    if left_metadata.is_dir() {
        let mut left_entries = fs::read_dir(left)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        let mut right_entries = fs::read_dir(right)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        left_entries.sort();
        right_entries.sort();
        if left_entries != right_entries {
            return Ok(false);
        }
        for name in left_entries {
            if !paths_have_same_contents(&left.join(&name), &right.join(name))? {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    Ok(false)
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn remove_path_if_exists(path: &Path) -> Result<(), BifrostError> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn run_windows_desktop_install_transaction<T>(
    install_dir: &Path,
    install_and_verify: impl FnOnce() -> Result<T, BifrostError>,
) -> Result<T, BifrostError> {
    let snapshot = WindowsDesktopInstallSnapshot::capture(install_dir)?;
    finish_windows_desktop_install_transaction(snapshot, install_and_verify())
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn finish_windows_desktop_install_transaction<T>(
    snapshot: WindowsDesktopInstallSnapshot,
    install_result: Result<T, BifrostError>,
) -> Result<T, BifrostError> {
    let had_previous_install = snapshot.had_previous_install;
    match install_result {
        Ok(value) => Ok(value),
        Err(install_error) if snapshot.installed_tree_is_unchanged() => Err(BifrostError::Config(
            format!("{install_error}; previous desktop app unchanged"),
        )),
        Err(install_error) => match snapshot.restore() {
            Ok(()) => Err(BifrostError::Config(format!(
                "{install_error}; {}",
                if had_previous_install {
                    "previous desktop app restored"
                } else {
                    "failed desktop install removed"
                }
            ))),
            Err(rollback_error) => Err(BifrostError::Config(format!(
                "{install_error}; failed to restore previous desktop app: {rollback_error}"
            ))),
        },
    }
}

pub(super) fn install_desktop_package_verified(
    package: &Path,
    install_dir: &Path,
    install_path: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let install_and_verify = || {
        install_desktop_package(
            package,
            install_dir,
            install_path,
            target_version,
            progress_source,
        )?;
        verify_installed_desktop_target_version(install_path, target_version)
    };
    #[cfg(target_os = "windows")]
    {
        run_windows_desktop_install_transaction(install_dir, install_and_verify)
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_and_verify()
    }
}

pub(super) fn acquire_top_level_app_upgrade_lock(
    progress_source: &str,
    target_version: &str,
) -> Result<Option<fs::File>, BifrostError> {
    let dir = data_dir();
    if crate::commands::upgrade_background::parent_upgrade_lock_is_valid(&dir)
        && (progress_source == CALLER_MANAGED_PROGRESS_SOURCE
            || desktop_upgrade_handoff_managed(progress_source))
    {
        return Ok(None);
    }
    use crate::commands::upgrade_background::UpgradeLockAttempt;
    match crate::commands::upgrade_background::try_acquire_upgrade_lock_attempt(&dir) {
        Ok(UpgradeLockAttempt::Acquired(lock)) => Ok(Some(lock)),
        Ok(UpgradeLockAttempt::PendingDesktopHandoff) => Err(BifrostError::Config(
            "Desktop app update handoff is already pending".to_string(),
        )),
        Ok(UpgradeLockAttempt::Contended) => Err(BifrostError::Config(
            "Upgrade is already running in another process".to_string(),
        )),
        Err(error) => {
            let error = BifrostError::Config(format!(
                "Failed to acquire the cross-process upgrade lock: {error}"
            ));
            write_app_failed_progress(target_version, progress_source, &error);
            Err(error)
        }
    }
}

pub(super) fn write_app_failed_progress(
    target_version: &str,
    progress_source: &str,
    error: &BifrostError,
) {
    write_app_progress(
        UpgradePhase::Failed,
        "Desktop app update failed",
        Some(target_version.to_string()),
        progress_source,
        None,
        Some(error.to_string()),
    );
}

pub(super) fn skip_desktop_restart() -> bool {
    env::var("BIFROST_APP_SKIP_RESTART")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(any(target_os = "windows", test))]
pub(super) const DESKTOP_PENDING_INSTALL_FILE: &str = "desktop-upgrade-pending-install.json";
#[cfg(any(target_os = "windows", test))]
pub(super) const DESKTOP_PENDING_INSTALL_SCHEMA_VERSION: u8 = 1;

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(super) struct PendingDesktopInstall {
    pub(super) schema_version: u8,
    pub(super) created_at_ms: u64,
    pub(super) package_path: String,
    pub(super) target_version: String,
    #[serde(default)]
    pub(super) package_owned_by_updater: bool,
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn should_defer_desktop_install(
    progress_source: &str,
    package: &Path,
    windows: bool,
    handoff_managed: bool,
) -> bool {
    if !windows || progress_source != "desktop" || !handoff_managed {
        return false;
    }
    matches!(
        package.extension().and_then(|value| value.to_str()),
        Some(extension) if extension.eq_ignore_ascii_case("msi") || extension.eq_ignore_ascii_case("exe")
    )
}

#[cfg(target_os = "windows")]
pub(super) fn should_defer_current_desktop_install(progress_source: &str, package: &Path) -> bool {
    should_defer_desktop_install(
        progress_source,
        package,
        true,
        desktop_upgrade_handoff_managed(progress_source),
    )
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub(super) fn should_defer_current_desktop_install(
    _progress_source: &str,
    _package: &Path,
) -> bool {
    false
}

pub(super) fn desktop_upgrade_handoff_managed(source: &str) -> bool {
    source == "desktop"
        && env::var(DESKTOP_UPGRADE_HANDOFF_ENV)
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
}

pub(super) fn should_write_app_progress(source: &str) -> bool {
    source != CALLER_MANAGED_PROGRESS_SOURCE
}

#[cfg(any(target_os = "windows", test))]
// The Windows helper may wait 30s for the App, 30s for its core, and 10m for
// the installer. Keep both the cross-process guard and relaunch marker alive
// beyond that 11-minute execution budget, with room for WebView polling and
// process scheduling before the helper starts.
pub(super) const DESKTOP_PENDING_INSTALL_STALE_AFTER_MS: u64 = 15 * 60 * 1000;

#[cfg(any(target_os = "windows", test))]
pub(crate) fn desktop_pending_install_guard_is_active(data_dir: &Path) -> bool {
    let path = data_dir.join(DESKTOP_PENDING_INSTALL_FILE);
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(pending) = serde_json::from_str::<PendingDesktopInstall>(&content) else {
        return false;
    };
    if pending.schema_version != DESKTOP_PENDING_INSTALL_SCHEMA_VERSION {
        return false;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    now_ms
        .checked_sub(pending.created_at_ms)
        .map(|age| age <= DESKTOP_PENDING_INSTALL_STALE_AFTER_MS)
        .unwrap_or(true)
}

#[cfg(not(any(target_os = "windows", test)))]
pub(crate) fn desktop_pending_install_guard_is_active(_data_dir: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(super) fn defer_desktop_install_to_handoff(
    package: &Path,
    target_version: &str,
    package_owned_by_updater: bool,
) -> Result<(), BifrostError> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let pending = PendingDesktopInstall {
        schema_version: DESKTOP_PENDING_INSTALL_SCHEMA_VERSION,
        created_at_ms,
        package_path: package.to_string_lossy().into_owned(),
        target_version: target_version.to_string(),
        package_owned_by_updater,
    };
    let content = serde_json::to_string_pretty(&pending).map_err(|error| {
        BifrostError::Config(format!(
            "failed to encode deferred desktop installer: {error}"
        ))
    })?;
    fs::write(
        dir.join(DESKTOP_PENDING_INSTALL_FILE),
        format!("{content}\n"),
    )?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug)]
pub(super) struct DesktopInstallCommandOutput {
    pub(super) status: std::process::ExitStatus,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn run_desktop_install_command(
    command: Command,
    target_version: &str,
    progress_source: &str,
) -> Result<std::process::ExitStatus, BifrostError> {
    Ok(run_desktop_install_command_output(command, target_version, progress_source)?.status)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn run_desktop_install_command_output(
    command: Command,
    target_version: &str,
    progress_source: &str,
) -> Result<DesktopInstallCommandOutput, BifrostError> {
    run_desktop_install_command_output_with_timeout(
        command,
        target_version,
        progress_source,
        DESKTOP_INSTALL_COMMAND_TIMEOUT,
        DESKTOP_INSTALL_COMMAND_HEARTBEAT,
        DESKTOP_INSTALL_TERMINAL_HEARTBEAT,
    )
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn run_desktop_install_command_output_with_timeout(
    mut command: Command,
    target_version: &str,
    progress_source: &str,
    timeout: Duration,
    heartbeat: Duration,
    terminal_heartbeat: Duration,
) -> Result<DesktopInstallCommandOutput, BifrostError> {
    let mut output_capture =
        crate::commands::streamed_output::StreamedOutputCapture::new().map_err(BifrostError::Io)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output_capture.stdout_stdio().map_err(BifrostError::Io)?)
        .stderr(output_capture.stderr_stdio().map_err(BifrostError::Io)?)
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;
    let started = Instant::now();
    let mut next_terminal_heartbeat = Instant::now() + terminal_heartbeat;
    let status = loop {
        output_capture.forward_available();
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                output_capture.forward_available();
                return Err(BifrostError::Config(format!(
                    "desktop package installer timed out after {} seconds",
                    timeout.as_secs_f64()
                )));
            }
            Ok(None) => {
                if Instant::now() >= next_heartbeat {
                    write_app_progress(
                        UpgradePhase::Installing,
                        "Installing desktop app…",
                        Some(target_version.to_string()),
                        progress_source,
                        None,
                        None,
                    );
                    next_heartbeat = Instant::now() + heartbeat;
                }
                if Instant::now() >= next_terminal_heartbeat {
                    println!(
                        "  Installing desktop app... ({}s elapsed)",
                        started.elapsed().as_secs()
                    );
                    next_terminal_heartbeat = Instant::now() + terminal_heartbeat;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    };
    output_capture.forward_available();
    let (stdout_text, stderr_text) = output_capture.read_all();
    Ok(DesktopInstallCommandOutput {
        status,
        stdout: stdout_text,
        stderr: stderr_text,
    })
}
