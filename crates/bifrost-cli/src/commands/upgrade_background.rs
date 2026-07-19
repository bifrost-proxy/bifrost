//! Unattended background upgrade entry point used by the tray and the Admin UI.
//!
//! This wraps the existing interactive upgrade engine ([`super::upgrade`]) with:
//! 1. a process-global progress *sink* that records [`UpgradeProgress`] to the
//!    shared file channel in `bifrost-core`, and
//! 2. an unattended driver equivalent to `bifrost upgrade` that never blocks on
//!    stdin and auto-restarts a running proxy after installation.
//!
//! The reporting hooks called from `upgrade.rs` are no-ops unless a background
//! upgrade has installed a sink, so the normal interactive CLI path is
//! completely unaffected.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};
use bifrost_core::BifrostError;
use fs2::FileExt;

use super::upgrade::{
    download_progress_line, handle_background_upgrade, take_deferred_install_scheduled,
    take_desktop_handoff_scheduled, RunningProxyHint,
};
use crate::cli::Commands;

struct ProgressSink {
    data_dir: PathBuf,
    target_version: Option<String>,
    source: String,
}

struct ActiveProgressSink {
    sink: ProgressSink,
    owner_thread: std::thread::ThreadId,
}

static SINK: OnceLock<Mutex<Option<ActiveProgressSink>>> = OnceLock::new();
const UPGRADE_LOCK_FILE_NAME: &str = "upgrade.lock";
const UPGRADE_LOCK_OWNER_FILE_NAME: &str = "upgrade.lock.owner.json";
pub(crate) const PARENT_UPGRADE_LOCK_TOKEN_ENV: &str = "BIFROST_PARENT_UPGRADE_LOCK_TOKEN_INTERNAL";
pub(crate) const PARENT_UPGRADE_LOCK_OWNER_PID_ENV: &str =
    "BIFROST_PARENT_UPGRADE_LOCK_OWNER_PID_INTERNAL";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct UpgradeLockOwner {
    pid: u32,
    token: String,
}

static OWNED_UPGRADE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, UpgradeLockOwner>>> = OnceLock::new();

pub(crate) enum UpgradeLockAttempt {
    Acquired(File),
    Contended,
    PendingDesktopHandoff,
}

fn sink_slot() -> &'static Mutex<Option<ActiveProgressSink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

fn owned_upgrade_lock_slot() -> &'static Mutex<HashMap<PathBuf, UpgradeLockOwner>> {
    OWNED_UPGRADE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn publish_upgrade_lock_owner(data_dir: &Path) -> Result<(), BifrostError> {
    let owner = UpgradeLockOwner {
        pid: std::process::id(),
        token: uuid::Uuid::new_v4().as_simple().to_string(),
    };
    let content = serde_json::to_vec(&owner)
        .map_err(|error| BifrostError::Config(format!("encode upgrade lock owner: {error}")))?;
    std::fs::write(data_dir.join(UPGRADE_LOCK_OWNER_FILE_NAME), content)
        .map_err(BifrostError::Io)?;
    if let Ok(mut slot) = owned_upgrade_lock_slot().lock() {
        slot.insert(data_dir.to_path_buf(), owner);
    }
    Ok(())
}

pub(crate) fn parent_upgrade_lock_child_environment(data_dir: &Path) -> Vec<(String, String)> {
    owned_upgrade_lock_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.get(data_dir).cloned())
        .map(|owner| {
            vec![
                (PARENT_UPGRADE_LOCK_TOKEN_ENV.to_string(), owner.token),
                (
                    PARENT_UPGRADE_LOCK_OWNER_PID_ENV.to_string(),
                    owner.pid.to_string(),
                ),
            ]
        })
        .unwrap_or_default()
}

#[cfg(windows)]
fn current_parent_pid() -> Option<u32> {
    let system = sysinfo::System::new_all();
    let current = sysinfo::get_current_pid().ok()?;
    system
        .process(current)
        .and_then(|process| process.parent())
        .map(|pid| pid.as_u32())
}

#[cfg(unix)]
fn current_parent_pid() -> Option<u32> {
    // SAFETY: getppid has no pointer arguments and does not mutate Rust-owned
    // memory. A positive PID is the kernel-observed direct parent.
    let pid = unsafe { libc::getppid() };
    (pid > 0).then_some(pid as u32)
}

fn parent_upgrade_lock_credential_matches(data_dir: &Path, actual_parent_pid: u32) -> bool {
    let Ok(token) = std::env::var(PARENT_UPGRADE_LOCK_TOKEN_ENV) else {
        return false;
    };
    let Some(owner_pid) = std::env::var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return false;
    };
    if owner_pid != actual_parent_pid {
        return false;
    }
    let Some(owner) = std::fs::read(data_dir.join(UPGRADE_LOCK_OWNER_FILE_NAME))
        .ok()
        .and_then(|content| serde_json::from_slice::<UpgradeLockOwner>(&content).ok())
    else {
        return false;
    };
    if owner.pid != owner_pid || owner.token != token {
        return false;
    }
    let Ok(probe) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(data_dir.join(UPGRADE_LOCK_FILE_NAME))
    else {
        return false;
    };
    match probe.try_lock_exclusive() {
        Err(error) => upgrade_lock_is_contended(&error),
        Ok(()) => {
            let _ = FileExt::unlock(&probe);
            false
        }
    }
}

pub(crate) fn parent_upgrade_lock_is_valid(data_dir: &Path) -> bool {
    current_parent_pid()
        .map(|pid| parent_upgrade_lock_credential_matches(data_dir, pid))
        .unwrap_or(false)
}

fn install_sink(sink: ProgressSink) {
    if let Ok(mut slot) = sink_slot().lock() {
        *slot = Some(ActiveProgressSink {
            sink,
            owner_thread: std::thread::current().id(),
        });
    }
}

fn take_sink() -> Option<ProgressSink> {
    sink_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .map(|active| active.sink)
}

pub(crate) fn try_acquire_upgrade_lock(data_dir: &Path) -> Result<Option<File>, BifrostError> {
    match try_acquire_upgrade_lock_attempt(data_dir)? {
        UpgradeLockAttempt::Acquired(lock) => Ok(Some(lock)),
        UpgradeLockAttempt::Contended | UpgradeLockAttempt::PendingDesktopHandoff => Ok(None),
    }
}

pub(crate) fn try_acquire_upgrade_lock_attempt(
    data_dir: &Path,
) -> Result<UpgradeLockAttempt, BifrostError> {
    std::fs::create_dir_all(data_dir).map_err(BifrostError::Io)?;
    if crate::commands::app::desktop_pending_install_guard_is_active(data_dir) {
        // The App updater has handed a Windows MSI/EXE to the desktop shell.
        // Its process-level file lock is necessarily released before the old
        // App exits, so the fresh pending marker is the cross-process guard for
        // the remainder of that deferred transaction.
        return Ok(UpgradeLockAttempt::PendingDesktopHandoff);
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(data_dir.join(UPGRADE_LOCK_FILE_NAME))
        .map_err(BifrostError::Io)?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            publish_upgrade_lock_owner(data_dir)?;
            Ok(UpgradeLockAttempt::Acquired(lock))
        }
        Err(error) if upgrade_lock_is_contended(&error) => Ok(UpgradeLockAttempt::Contended),
        Err(error) => Err(BifrostError::Io(error)),
    }
}

fn upgrade_lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        // LockFileEx reports ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
        // instead of mapping them to ErrorKind::WouldBlock.
        return true;
    }
    false
}

/// Emit a progress record for the active sink (no-op when none is installed).
fn emit(build: impl FnOnce(&ProgressSink) -> UpgradeProgress) {
    if let Ok(slot) = sink_slot().lock() {
        if let Some(active) = slot
            .as_ref()
            .filter(|active| active.owner_thread == std::thread::current().id())
        {
            let sink = &active.sink;
            let progress = build(sink);
            write_progress(&sink.data_dir, &progress);
        }
    }
}

fn base(sink: &ProgressSink, phase: UpgradePhase, message: impl Into<String>) -> UpgradeProgress {
    UpgradeProgress::new(phase, message)
        .with_target(sink.target_version.clone())
        .with_source(Some(sink.source.clone()))
}

/// Reported from the download loop in `upgrade.rs` at each render tick.
pub(crate) fn report_download(downloaded: u64, total: Option<u64>, started: Instant) {
    let percent = match total {
        Some(total) if total > 0 => {
            Some(((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0))
        }
        _ => None,
    };
    let message = download_progress_line(downloaded, total, started);
    emit(|sink| base(sink, UpgradePhase::Downloading, message.clone()).with_percent(percent));
}

/// Reported from `upgrade.rs` right before extracting/installing the binary.
pub(crate) fn report_installing() {
    emit(|sink| base(sink, UpgradePhase::Installing, "Installing new version…"));
}

/// Reported from `upgrade.rs` right before restarting the running proxy.
pub(crate) fn report_restarting() {
    emit(|sink| base(sink, UpgradePhase::Restarting, "Restarting proxy…"));
}

/// Preserve a desktop-owned handoff record while the CLI-owned core restarts.
///
/// In the hybrid WebView flow the companion updater has already delegated the
/// terminal transition to the desktop shell. Replacing its `source=desktop`
/// record with the background sink's source would prevent the WebView from
/// invoking that handoff and leave the upgrade stuck at `restarting`.
pub(crate) fn report_restarting_preserving_desktop_handoff(preserve: bool) {
    let preserve_existing = preserve
        && sink_slot()
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref()
                    .filter(|active| active.owner_thread == std::thread::current().id())
                    .map(|active| active.sink.data_dir.clone())
            })
            .map(|data_dir| {
                let progress = bifrost_core::upgrade_progress::read_progress(&data_dir);
                progress.phase == UpgradePhase::Restarting
                    && progress.source.as_deref() == Some("desktop")
            })
            .unwrap_or(false);
    if !preserve_existing {
        report_restarting();
    }
}

/// Run an unattended upgrade, reporting progress to the shared file channel.
///
/// `target` pins the release selected by the initiating version check so CLI
/// and App cannot diverge if `latest` changes while the upgrade is running.
/// `source` records who initiated the upgrade (`"tray"` / `"admin"` / `"cli"`).
pub fn handle_upgrade_background(
    target: Option<String>,
    source: String,
    running_proxy_pid: Option<u32>,
    running_proxy_port: Option<u16>,
) {
    let engine_target = target.clone();
    handle_upgrade_background_with(
        target,
        source,
        running_proxy_pid,
        running_proxy_port,
        crate::config::get_bifrost_dir(),
        move |restart_hint| handle_background_upgrade(restart_hint, engine_target),
    );
}

pub fn handle_upgrade_background_command(command: Commands) -> Result<(), BifrostError> {
    handle_upgrade_background_command_with(command, handle_upgrade_background)
}

fn handle_upgrade_background_command_with(
    command: Commands,
    handler: impl FnOnce(Option<String>, String, Option<u32>, Option<u16>),
) -> Result<(), BifrostError> {
    let Commands::SelfUpdate {
        target,
        source,
        running_proxy_pid,
        running_proxy_port,
    } = command
    else {
        return Err(BifrostError::Config(
            "Expected hidden self-update command".to_string(),
        ));
    };
    handler(target, source, running_proxy_pid, running_proxy_port);
    Ok(())
}

fn handle_upgrade_background_with(
    target: Option<String>,
    source: String,
    running_proxy_pid: Option<u32>,
    running_proxy_port: Option<u16>,
    data_dir: Result<PathBuf, BifrostError>,
    engine: impl FnOnce(Option<RunningProxyHint>) -> Result<(), BifrostError>,
) {
    let data_dir = match data_dir {
        Ok(dir) => dir,
        Err(error) => {
            tracing::error!(error = %error, "background upgrade: cannot resolve data dir");
            return;
        }
    };
    let _upgrade_lock = match try_acquire_upgrade_lock_attempt(&data_dir) {
        Ok(UpgradeLockAttempt::Acquired(lock)) => lock,
        Ok(UpgradeLockAttempt::PendingDesktopHandoff) => {
            tracing::info!(
                "background upgrade: preserving progress owned by pending desktop handoff"
            );
            return;
        }
        Ok(UpgradeLockAttempt::Contended) => {
            tracing::warn!("background upgrade: preserving progress owned by another updater");
            return;
        }
        Err(error) => {
            tracing::error!(error = %error, "background upgrade: cannot acquire upgrade lock");
            write_upgrade_lock_failure(
                &data_dir,
                target.clone(),
                source.clone(),
                "Upgrade lock could not be acquired",
                &format!("Failed to acquire the cross-process upgrade lock: {error}"),
            );
            return;
        }
    };

    install_sink(ProgressSink {
        data_dir: data_dir.clone(),
        target_version: target.clone(),
        source: source.clone(),
    });

    emit(|sink| base(sink, UpgradePhase::Checking, "Checking for updates…"));

    // This background path is fully unattended: it skips the confirmation
    // prompt and auto-restarts the running proxy. It also restarts when the
    // on-disk binary is already current but the running daemon still serves an
    // older in-memory version.
    let restart_hint = RunningProxyHint::from_parts(running_proxy_pid, running_proxy_port);
    let result = engine(restart_hint);
    let deferred_terminal_pending =
        take_deferred_install_scheduled() || take_desktop_handoff_scheduled();

    match &result {
        Ok(()) if deferred_terminal_pending => {
            tracing::info!(
                "background upgrade: terminal progress delegated to Windows deferred installer"
            );
        }
        Ok(()) => {
            let target_version = target.clone();
            let source_label = source.clone();
            write_progress(
                &data_dir,
                &UpgradeProgress::new(UpgradePhase::Completed, "Upgrade complete")
                    .with_target(target_version)
                    .with_source(Some(source_label)),
            );
        }
        Err(error) => {
            let message = error.to_string();
            write_progress(
                &data_dir,
                &UpgradeProgress::new(UpgradePhase::Failed, "Upgrade failed")
                    .with_target(target.clone())
                    .with_source(Some(source.clone()))
                    .with_error(Some(message)),
            );
        }
    }

    // Drop the sink so a subsequent in-process upgrade starts clean. The
    // terminal record (Completed/Failed) stays on disk for readers (tray/admin)
    // to consume after they refresh; they clear it on acknowledgement.
    let _ = take_sink();
}

fn write_upgrade_lock_failure(
    data_dir: &Path,
    target: Option<String>,
    source: String,
    message: &str,
    error: &str,
) {
    write_progress(
        data_dir,
        &UpgradeProgress::new(UpgradePhase::Failed, message)
            .with_target(target)
            .with_source(Some(source))
            .with_error(Some(error.to_string())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::upgrade_progress::read_progress;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "bifrost-upgrade-bg-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // These tests exercise the sink reporting helpers directly. They mutate the
    // process-global sink, so they run serially under a shared lock.
    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn download_report_writes_percent_and_phase() {
        let _guard = lock_tests();
        let dir = temp_dir();
        install_sink(ProgressSink {
            data_dir: dir.clone(),
            target_version: Some("0.0.104".to_string()),
            source: "admin".to_string(),
        });

        report_download(512, Some(1024), Instant::now());
        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Downloading);
        assert_eq!(progress.percent, Some(50.0));
        assert_eq!(progress.target_version, Some("0.0.104".to_string()));
        assert_eq!(progress.source, Some("admin".to_string()));

        let _ = take_sink();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn installing_and_restarting_report_phases() {
        let _guard = lock_tests();
        let dir = temp_dir();
        install_sink(ProgressSink {
            data_dir: dir.clone(),
            target_version: None,
            source: "tray".to_string(),
        });

        report_installing();
        assert_eq!(read_progress(&dir).phase, UpgradePhase::Installing);
        report_restarting();
        assert_eq!(read_progress(&dir).phase, UpgradePhase::Restarting);

        let _ = take_sink();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cli_core_restart_preserves_desktop_handoff_progress() {
        let _guard = lock_tests();
        let dir = temp_dir();
        install_sink(ProgressSink {
            data_dir: dir.clone(),
            target_version: Some("0.0.156".to_string()),
            source: "admin".to_string(),
        });
        write_progress(
            &dir,
            &UpgradeProgress::new(UpgradePhase::Restarting, "Restart desktop to finish update")
                .with_target(Some("0.0.156".to_string()))
                .with_source(Some("desktop".to_string())),
        );

        report_restarting_preserving_desktop_handoff(true);
        let preserved = read_progress(&dir);
        assert_eq!(preserved.source.as_deref(), Some("desktop"));
        assert_eq!(preserved.message, "Restart desktop to finish update");

        report_restarting_preserving_desktop_handoff(false);
        let ordinary_restart = read_progress(&dir);
        assert_eq!(ordinary_restart.source.as_deref(), Some("admin"));
        assert_eq!(ordinary_restart.message, "Restarting proxy…");

        let _ = take_sink();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reporting_without_sink_is_noop() {
        let _guard = lock_tests();
        let _ = take_sink();
        // Must not panic and must not write anywhere.
        report_installing();
        report_download(1, Some(2), Instant::now());
        report_restarting();
    }

    #[test]
    fn progress_sink_ignores_reports_from_non_owner_thread() {
        let _guard = lock_tests();
        let dir = temp_dir();
        install_sink(ProgressSink {
            data_dir: dir.clone(),
            target_version: Some("0.0.156".to_string()),
            source: "admin".to_string(),
        });

        report_restarting();
        std::thread::spawn(report_installing)
            .join()
            .expect("non-owner reporter thread");
        assert_eq!(read_progress(&dir).phase, UpgradePhase::Restarting);

        let _ = take_sink();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cross_process_upgrade_lock_allows_only_one_owner() {
        let dir = temp_dir();
        let first = try_acquire_upgrade_lock(&dir)
            .expect("acquire first lock")
            .expect("first owner");
        assert!(
            try_acquire_upgrade_lock(&dir)
                .expect("contended lock is not an IO error")
                .is_none(),
            "a second updater must not install or restart concurrently"
        );
        drop(first);
        assert!(try_acquire_upgrade_lock(&dir)
            .expect("reacquire released lock")
            .is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn managed_child_credential_requires_token_owner_pid_and_live_lock() {
        let _guard = lock_tests();
        let dir = temp_dir();
        let owner = try_acquire_upgrade_lock(&dir)
            .expect("acquire parent lock")
            .expect("parent owns lock");
        let previous_token = std::env::var_os(PARENT_UPGRADE_LOCK_TOKEN_ENV);
        let previous_pid = std::env::var_os(PARENT_UPGRADE_LOCK_OWNER_PID_ENV);
        for (key, value) in parent_upgrade_lock_child_environment(&dir) {
            std::env::set_var(key, value);
        }

        assert!(parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id()
        ));
        assert!(!parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id().saturating_add(1)
        ));

        std::env::set_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV, "not-a-pid");
        assert!(!parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id()
        ));
        for (key, value) in parent_upgrade_lock_child_environment(&dir) {
            std::env::set_var(key, value);
        }

        let owner_path = dir.join(UPGRADE_LOCK_OWNER_FILE_NAME);
        let owner_content = std::fs::read(&owner_path).expect("read owner sidecar");
        std::fs::remove_file(&owner_path).expect("remove owner sidecar");
        assert!(!parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id()
        ));
        std::fs::write(&owner_path, owner_content).expect("restore owner sidecar");

        std::env::set_var(PARENT_UPGRADE_LOCK_TOKEN_ENV, "forged-token");
        assert!(!parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id()
        ));
        for (key, value) in parent_upgrade_lock_child_environment(&dir) {
            std::env::set_var(key, value);
        }

        #[cfg(unix)]
        {
            let lock_path = dir.join(UPGRADE_LOCK_FILE_NAME);
            std::fs::remove_file(&lock_path).expect("unlink live lock path");
            assert!(!parent_upgrade_lock_credential_matches(
                &dir,
                std::process::id()
            ));
            std::fs::write(&lock_path, b"").expect("replace live lock path");
            assert!(!parent_upgrade_lock_credential_matches(
                &dir,
                std::process::id()
            ));
        }

        drop(owner);
        assert!(!parent_upgrade_lock_credential_matches(
            &dir,
            std::process::id()
        ));

        match previous_token {
            Some(value) => std::env::set_var(PARENT_UPGRADE_LOCK_TOKEN_ENV, value),
            None => std::env::remove_var(PARENT_UPGRADE_LOCK_TOKEN_ENV),
        }
        match previous_pid {
            Some(value) => std::env::set_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV, value),
            None => std::env::remove_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upgrade_lock_contention_normalizes_platform_error_kinds() {
        assert!(upgrade_lock_is_contended(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
        assert!(!upgrade_lock_is_contended(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        #[cfg(windows)]
        {
            assert!(upgrade_lock_is_contended(
                &std::io::Error::from_raw_os_error(32)
            ));
            assert!(upgrade_lock_is_contended(
                &std::io::Error::from_raw_os_error(33)
            ));
        }
    }

    #[test]
    fn background_upgrade_preserves_active_progress_when_another_process_owns_the_lock() {
        let _guard = lock_tests();
        let dir = temp_dir();
        let owner = try_acquire_upgrade_lock(&dir)
            .expect("acquire lock")
            .expect("first owner");
        write_progress(
            &dir,
            &UpgradeProgress::new(UpgradePhase::Downloading, "Tray upgrade in progress")
                .with_target(Some("0.0.155".to_string()))
                .with_source(Some("tray".to_string())),
        );
        let engine_called = std::cell::Cell::new(false);

        handle_upgrade_background_with(
            Some("0.0.156".to_string()),
            "admin".to_string(),
            Some(12345),
            Some(9900),
            Ok(dir.clone()),
            |_| {
                engine_called.set(true);
                Ok(())
            },
        );

        assert!(!engine_called.get());
        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Downloading);
        assert_eq!(progress.target_version.as_deref(), Some("0.0.155"));
        assert_eq!(progress.source.as_deref(), Some("tray"));
        assert_eq!(progress.message, "Tray upgrade in progress");
        assert!(progress.error.is_none());
        drop(owner);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn background_upgrade_preserves_progress_owned_by_pending_desktop_handoff() {
        let _guard = lock_tests();
        let dir = temp_dir();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        std::fs::write(
            dir.join("desktop-upgrade-pending-install.json"),
            format!(
                r#"{{"schema_version":1,"created_at_ms":{now_ms},"package_path":"Bifrost.msi","target_version":"0.0.156"}}"#
            ),
        )
        .expect("write pending desktop marker");
        write_progress(
            &dir,
            &UpgradeProgress::new(UpgradePhase::Restarting, "Desktop handoff pending")
                .with_target(Some("0.0.156".to_string()))
                .with_source(Some("desktop".to_string())),
        );
        let engine_called = std::cell::Cell::new(false);

        handle_upgrade_background_with(
            Some("0.0.157".to_string()),
            "admin".to_string(),
            None,
            None,
            Ok(dir.clone()),
            |_| {
                engine_called.set(true);
                Ok(())
            },
        );

        assert!(!engine_called.get());
        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Restarting);
        assert_eq!(progress.source.as_deref(), Some("desktop"));
        assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn background_upgrade_writes_failed_progress_when_lock_file_cannot_be_opened() {
        let _guard = lock_tests();
        let dir = temp_dir();
        std::fs::create_dir(dir.join(UPGRADE_LOCK_FILE_NAME))
            .expect("make lock path impossible to open as a file");
        let engine_called = std::cell::Cell::new(false);

        handle_upgrade_background_with(
            Some("0.0.156".to_string()),
            "admin".to_string(),
            None,
            None,
            Ok(dir.clone()),
            |_| {
                engine_called.set(true);
                Ok(())
            },
        );

        assert!(!engine_called.get());
        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Failed);
        assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));
        assert_eq!(progress.source.as_deref(), Some("admin"));
        assert!(progress
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Failed to acquire")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn background_upgrade_returns_without_running_when_data_dir_resolution_fails() {
        let engine_called = std::cell::Cell::new(false);
        handle_upgrade_background_with(
            Some("0.0.156".to_string()),
            "admin".to_string(),
            None,
            None,
            Err(BifrostError::Config("data dir unavailable".to_string())),
            |_| {
                engine_called.set(true);
                Ok(())
            },
        );
        assert!(!engine_called.get());
    }

    #[test]
    fn hidden_self_update_command_forwards_complete_restart_hint() {
        let command = Commands::SelfUpdate {
            target: Some("0.0.156".to_string()),
            source: "admin".to_string(),
            running_proxy_pid: Some(12345),
            running_proxy_port: Some(9900),
        };
        let forwarded = std::cell::RefCell::new(None);
        handle_upgrade_background_command_with(command, |target, source, pid, port| {
            forwarded.replace(Some((target, source, pid, port)));
        })
        .unwrap();
        assert_eq!(
            forwarded.into_inner(),
            Some((
                Some("0.0.156".to_string()),
                "admin".to_string(),
                Some(12345),
                Some(9900),
            ))
        );
        assert!(handle_upgrade_background_command_with(
            Commands::VersionCheck,
            |_, _, _, _| panic!("non-self-update command must not be dispatched"),
        )
        .is_err());
        assert!(handle_upgrade_background_command(Commands::VersionCheck).is_err());
    }

    #[test]
    fn background_upgrade_wrapper_forwards_hint_and_writes_terminal_progress() {
        let _guard = lock_tests();
        let dir = temp_dir();
        let seen_hint = std::cell::RefCell::new(None);
        handle_upgrade_background_with(
            Some("0.0.156".to_string()),
            "admin".to_string(),
            Some(12345),
            Some(9900),
            Ok(dir.clone()),
            |hint| {
                seen_hint.replace(hint);
                Ok(())
            },
        );
        assert_eq!(
            seen_hint.into_inner(),
            RunningProxyHint::from_parts(Some(12345), Some(9900))
        );
        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Completed);
        assert_eq!(progress.source.as_deref(), Some("admin"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn public_background_upgrade_runs_real_already_latest_engine() {
        const CHILD_ENV: &str = "BIFROST_TEST_PUBLIC_BACKGROUND_UPGRADE_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = std::process::Command::new(
                std::env::current_exe().expect("current test executable"),
            )
            .args([
                "--exact",
                "commands::upgrade_background::tests::public_background_upgrade_runs_real_already_latest_engine",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated background upgrade test");
            assert!(status.success(), "isolated background upgrade test failed");
            return;
        }

        let _sink_guard = lock_tests();
        let dir = temp_dir();
        let app_dir = dir.join("apps");
        std::fs::create_dir_all(&app_dir).expect("create empty app dir");
        let keys = [
            "BIFROST_DATA_DIR",
            "BIFROST_APP_INSTALL_DIR",
            "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        ];
        let previous: Vec<_> = keys
            .iter()
            .map(|key| ((*key).to_string(), std::env::var_os(key)))
            .collect();
        std::env::set_var("BIFROST_DATA_DIR", &dir);
        std::env::set_var("BIFROST_APP_INSTALL_DIR", &app_dir);
        std::env::set_var(
            "BIFROST_UPGRADE_TEST_LATEST_VERSION",
            env!("CARGO_PKG_VERSION"),
        );

        handle_upgrade_background(
            Some(env!("CARGO_PKG_VERSION").to_string()),
            "admin".to_string(),
            None,
            None,
        );

        let progress = read_progress(&dir);
        assert_eq!(progress.phase, UpgradePhase::Completed);
        assert_eq!(progress.source.as_deref(), Some("admin"));
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = take_sink();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn background_upgrade_source_delegates_windows_deferred_terminal_progress() {
        let source = include_str!("upgrade_background.rs");
        assert!(source.contains("take_deferred_install_scheduled"));
        assert!(source.contains("deferred_terminal_pending"));
        assert!(source.contains("terminal progress delegated to Windows deferred installer"));
    }
}
