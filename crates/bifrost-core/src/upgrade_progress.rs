//! Cross-process upgrade progress channel.
//!
//! The upgrade engine lives in `bifrost-cli`, but both the tray (also in
//! `bifrost-cli`) and the Admin server (in `bifrost-admin`) need to observe
//! upgrade progress. Because `bifrost-admin` cannot depend on `bifrost-cli`
//! (the dependency flows `bifrost-cli -> bifrost-admin -> bifrost-core`), the
//! shared state type and its file-backed channel live here in `bifrost-core`.
//!
//! The progress is persisted to `<data_dir>/upgrade-progress.json` using an
//! atomic `write tmp + rename` so readers never observe a half-written file.
//! Readers degrade to [`UpgradeProgress::idle`] on any missing/corrupt file so
//! the channel can never deadlock the UI.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Name of the progress file written under the data directory.
pub const PROGRESS_FILE_NAME: &str = "upgrade-progress.json";

/// Default staleness threshold (seconds). An active progress whose `updated_at`
/// is older than this is considered an abandoned/crashed upgrade.
pub const DEFAULT_STALE_SECS: i64 = 120;

/// Lifecycle phases of a background upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradePhase {
    /// No upgrade in progress.
    Idle,
    /// Validating version / preparing.
    Checking,
    /// Downloading the release archive.
    Downloading,
    /// Extracting and atomically swapping the binary.
    Installing,
    /// Stopping the old proxy, waiting for ports, starting the new one.
    Restarting,
    /// Upgrade succeeded (new process started or awaiting UI refresh).
    Completed,
    /// Upgrade failed.
    Failed,
}

/// A snapshot of upgrade progress shared across processes via a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpgradeProgress {
    pub phase: UpgradePhase,
    /// Download percentage in `0.0..=100.0`. Only meaningful while
    /// [`UpgradePhase::Downloading`]; `None` otherwise.
    #[serde(default)]
    pub percent: Option<f64>,
    /// Human-readable status line (English).
    #[serde(default)]
    pub message: String,
    /// Target version, e.g. `"0.0.104"`.
    #[serde(default)]
    pub target_version: Option<String>,
    /// Who initiated the upgrade: `"tray"` / `"admin"` / `"cli"`.
    #[serde(default)]
    pub source: Option<String>,
    /// Failure reason when `phase == Failed`.
    #[serde(default)]
    pub error: Option<String>,
    /// Last update timestamp (RFC3339).
    #[serde(default)]
    pub updated_at: String,
}

impl UpgradeProgress {
    /// An idle progress with the current timestamp.
    pub fn idle() -> Self {
        Self {
            phase: UpgradePhase::Idle,
            percent: None,
            message: String::new(),
            target_version: None,
            source: None,
            error: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    /// Build a fresh progress for the given phase, stamping `updated_at` now.
    pub fn new(phase: UpgradePhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            percent: None,
            message: message.into(),
            target_version: None,
            source: None,
            error: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    pub fn with_target(mut self, target: Option<String>) -> Self {
        self.target_version = target;
        self
    }

    pub fn with_source(mut self, source: Option<String>) -> Self {
        self.source = source;
        self
    }

    pub fn with_percent(mut self, percent: Option<f64>) -> Self {
        self.percent = percent;
        self
    }

    pub fn with_error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    /// True when an upgrade is mid-flight (i.e. not a terminal/idle state).
    pub fn is_active(&self) -> bool {
        matches!(
            self.phase,
            UpgradePhase::Checking
                | UpgradePhase::Downloading
                | UpgradePhase::Installing
                | UpgradePhase::Restarting
        )
    }
}

impl Default for UpgradeProgress {
    fn default() -> Self {
        Self::idle()
    }
}

/// Path of the progress file under `data_dir`.
pub fn progress_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PROGRESS_FILE_NAME)
}

/// Read the current progress. Missing or corrupt files degrade to
/// [`UpgradeProgress::idle`].
pub fn read_progress(data_dir: &Path) -> UpgradeProgress {
    let path = progress_file_path(data_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return UpgradeProgress::idle();
    };
    serde_json::from_str(&content).unwrap_or_else(|_| UpgradeProgress::idle())
}

/// Atomically write progress to `<data_dir>/upgrade-progress.json`.
///
/// Failures are swallowed: progress reporting must never break the upgrade.
pub fn write_progress(data_dir: &Path, progress: &UpgradeProgress) {
    if let Err(error) = write_progress_inner(data_dir, progress) {
        tracing::warn!(error = %error, "failed to write upgrade progress");
    }
}

fn write_progress_inner(data_dir: &Path, progress: &UpgradeProgress) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = progress_file_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(progress)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the progress file (best-effort).
pub fn clear_progress(data_dir: &Path) {
    let _ = std::fs::remove_file(progress_file_path(data_dir));
}

/// True when an active progress has not advanced within `max_age_secs`,
/// indicating an abandoned or crashed upgrade process.
pub fn is_stale(progress: &UpgradeProgress, max_age_secs: i64) -> bool {
    if !progress.is_active() {
        return false;
    }
    let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&progress.updated_at) else {
        // Unparseable timestamp on an active record => treat as stale.
        return true;
    };
    let age = Utc::now().signed_duration_since(updated.with_timezone(&Utc));
    age.num_seconds() > max_age_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "bifrost-upgrade-progress-{}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        dir.push(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_read_round_trip_preserves_all_fields() {
        let dir = temp_dir();
        let progress = UpgradeProgress::new(UpgradePhase::Downloading, "Downloading… 42.0%")
            .with_target(Some("0.0.104".to_string()))
            .with_source(Some("admin".to_string()))
            .with_percent(Some(42.0));
        write_progress(&dir, &progress);

        let read = read_progress(&dir);
        assert_eq!(read.phase, UpgradePhase::Downloading);
        assert_eq!(read.percent, Some(42.0));
        assert_eq!(read.message, "Downloading… 42.0%");
        assert_eq!(read.target_version, Some("0.0.104".to_string()));
        assert_eq!(read.source, Some("admin".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_reads_as_idle() {
        let dir = temp_dir();
        let read = read_progress(&dir);
        assert_eq!(read.phase, UpgradePhase::Idle);
        assert!(!read.is_active());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_reads_as_idle() {
        let dir = temp_dir();
        std::fs::write(progress_file_path(&dir), "{not valid json").unwrap();
        let read = read_progress(&dir);
        assert_eq!(read.phase, UpgradePhase::Idle);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_progress_removes_file() {
        let dir = temp_dir();
        write_progress(&dir, &UpgradeProgress::new(UpgradePhase::Installing, "Installing"));
        assert!(progress_file_path(&dir).exists());
        clear_progress(&dir);
        assert!(!progress_file_path(&dir).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_active_covers_phases() {
        for (phase, active) in [
            (UpgradePhase::Idle, false),
            (UpgradePhase::Checking, true),
            (UpgradePhase::Downloading, true),
            (UpgradePhase::Installing, true),
            (UpgradePhase::Restarting, true),
            (UpgradePhase::Completed, false),
            (UpgradePhase::Failed, false),
        ] {
            let p = UpgradeProgress::new(phase, "");
            assert_eq!(p.is_active(), active, "phase {phase:?}");
        }
    }

    #[test]
    fn stale_detection_respects_threshold_and_phase() {
        // Fresh active => not stale.
        let fresh = UpgradeProgress::new(UpgradePhase::Downloading, "x");
        assert!(!is_stale(&fresh, DEFAULT_STALE_SECS));

        // Old active => stale.
        let mut old = UpgradeProgress::new(UpgradePhase::Downloading, "x");
        old.updated_at = (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 10))
            .to_rfc3339();
        assert!(is_stale(&old, DEFAULT_STALE_SECS));

        // Old but terminal => never stale.
        let mut done = UpgradeProgress::new(UpgradePhase::Completed, "x");
        done.updated_at = (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 10))
            .to_rfc3339();
        assert!(!is_stale(&done, DEFAULT_STALE_SECS));

        // Unparseable timestamp on active => stale.
        let mut bad = UpgradeProgress::new(UpgradePhase::Installing, "x");
        bad.updated_at = "not-a-date".to_string();
        assert!(is_stale(&bad, DEFAULT_STALE_SECS));
    }
}
