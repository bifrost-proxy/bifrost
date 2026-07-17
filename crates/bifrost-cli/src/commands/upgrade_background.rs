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

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};
use bifrost_core::BifrostError;

use super::upgrade::{
    download_progress_line, handle_background_upgrade, take_deferred_install_scheduled,
    RunningProxyHint,
};
use crate::cli::Commands;

struct ProgressSink {
    data_dir: PathBuf,
    target_version: Option<String>,
    source: String,
}

static SINK: OnceLock<Mutex<Option<ProgressSink>>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<ProgressSink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

fn install_sink(sink: ProgressSink) {
    if let Ok(mut slot) = sink_slot().lock() {
        *slot = Some(sink);
    }
}

fn take_sink() -> Option<ProgressSink> {
    sink_slot().lock().ok().and_then(|mut slot| slot.take())
}

/// Emit a progress record for the active sink (no-op when none is installed).
fn emit(build: impl FnOnce(&ProgressSink) -> UpgradeProgress) {
    if let Ok(slot) = sink_slot().lock() {
        if let Some(sink) = slot.as_ref() {
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

/// Run an unattended upgrade, reporting progress to the shared file channel.
///
/// `target` is informational (the engine always resolves the latest release);
/// `source` records who initiated the upgrade (`"tray"` / `"admin"` / `"cli"`).
pub fn handle_upgrade_background(
    target: Option<String>,
    source: String,
    running_proxy_pid: Option<u32>,
    running_proxy_port: Option<u16>,
) {
    handle_upgrade_background_with(
        target,
        source,
        running_proxy_pid,
        running_proxy_port,
        crate::config::get_bifrost_dir(),
        handle_background_upgrade,
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
    let deferred_terminal_pending = take_deferred_install_scheduled();

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
    fn reporting_without_sink_is_noop() {
        let _guard = lock_tests();
        let _ = take_sink();
        // Must not panic and must not write anywhere.
        report_installing();
        report_download(1, Some(2), Instant::now());
        report_restarting();
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
    fn background_upgrade_source_delegates_windows_deferred_terminal_progress() {
        let source = include_str!("upgrade_background.rs");
        assert!(source.contains("take_deferred_install_scheduled"));
        assert!(source.contains("deferred_terminal_pending"));
        assert!(source.contains("terminal progress delegated to Windows deferred installer"));
    }
}
