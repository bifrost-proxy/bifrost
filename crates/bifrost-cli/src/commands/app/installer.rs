use super::*;

pub(super) const DESKTOP_UPGRADE_HANDOFF_ENV: &str = "BIFROST_DESKTOP_UPGRADE_HANDOFF";

fn parent_upgrade_lock_is_held() -> bool {
    env::var(PARENT_UPGRADE_LOCK_HELD_ENV)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub(super) fn acquire_top_level_app_upgrade_lock(
    progress_source: &str,
    target_version: &str,
) -> Result<Option<fs::File>, BifrostError> {
    if progress_source == CALLER_MANAGED_PROGRESS_SOURCE && parent_upgrade_lock_is_held() {
        return Ok(None);
    }
    let dir = data_dir();
    match crate::commands::upgrade_background::try_acquire_upgrade_lock(&dir) {
        Ok(Some(lock)) => Ok(Some(lock)),
        Ok(None) => {
            let error =
                BifrostError::Config("Upgrade is already running in another process".to_string());
            write_app_failed_progress(target_version, progress_source, &error);
            Err(error)
        }
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
const DESKTOP_PENDING_INSTALL_STALE_AFTER_MS: u64 = 10 * 60 * 1000;

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
    )
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn run_desktop_install_command_output_with_timeout(
    mut command: Command,
    target_version: &str,
    progress_source: &str,
    timeout: Duration,
    heartbeat: Duration,
) -> Result<DesktopInstallCommandOutput, BifrostError> {
    let mut stdout =
        tempfile::tempfile().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
    let mut stderr =
        tempfile::tempfile().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
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
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    };
    let _ = stdout.seek(SeekFrom::Start(0));
    let _ = stderr.seek(SeekFrom::Start(0));
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let _ = stdout.read_to_string(&mut stdout_text);
    let _ = stderr.read_to_string(&mut stderr_text);
    Ok(DesktopInstallCommandOutput {
        status,
        stdout: stdout_text,
        stderr: stderr_text,
    })
}
