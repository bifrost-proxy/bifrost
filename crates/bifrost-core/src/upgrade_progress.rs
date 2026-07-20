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

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Name of the progress file written under the data directory.
pub const PROGRESS_FILE_NAME: &str = "upgrade-progress.json";

const DESKTOP_UPGRADE_ORIGIN_PREFIX: &str = ".desktop-upgrade-origin-";
const DESKTOP_UPGRADE_ORIGIN_MAX_AGE_SECS: i64 = 30;

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
    read_progress_inner(data_dir).unwrap_or_else(|_| UpgradeProgress::idle())
}

fn read_progress_inner(data_dir: &Path) -> std::io::Result<UpgradeProgress> {
    let content = read_progress_content(&progress_file_path(data_dir))?;
    serde_json::from_str(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(not(windows))]
fn read_progress_content(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(windows)]
fn read_progress_content(path: &Path) -> std::io::Result<String> {
    for attempt in 0..WINDOWS_FILE_CONFLICT_MAX_ATTEMPTS {
        match std::fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(error) => {
                if !is_transient_windows_file_conflict(&error)
                    || attempt + 1 == WINDOWS_FILE_CONFLICT_MAX_ATTEMPTS
                {
                    return Err(error);
                }
                sleep_for_windows_file_conflict(attempt);
            }
        }
    }
    unreachable!("bounded Windows progress read loop must return")
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
    let content = serde_json::to_string_pretty(progress)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Admin, CLI, the Windows replacement helper, and the relaunched desktop
    // core can briefly overlap while ownership is handed off. A fixed temp
    // name lets one writer rename another writer's file (or makes the rename
    // fail), leaving the Web UI with stale progress. NamedTempFile gives every
    // writer a unique file in the destination directory and `persist` performs
    // the platform-specific atomic replacement of an existing destination.
    let mut tmp = tempfile::NamedTempFile::new_in(data_dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.as_file().sync_all()?;
    persist_progress_temp(tmp, &path)?;
    Ok(())
}

#[cfg(not(windows))]
fn persist_progress_temp(tmp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    tmp.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[cfg(windows)]
fn persist_progress_temp(mut tmp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    // MoveFileExW(MOVEFILE_REPLACE_EXISTING), used by tempfile::persist, can
    // transiently fail while another process is replacing or opening the same
    // progress file. Keep the unique source file and retry only Windows sharing
    // conflicts; permanent permission/path failures still surface immediately.
    for attempt in 0..WINDOWS_FILE_CONFLICT_MAX_ATTEMPTS {
        match tmp.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if !is_transient_windows_file_conflict(&error.error)
                    || attempt + 1 == WINDOWS_FILE_CONFLICT_MAX_ATTEMPTS
                {
                    return Err(error.error);
                }
                tmp = error.file;
                sleep_for_windows_file_conflict(attempt);
            }
        }
    }
    unreachable!("bounded Windows progress replacement loop must return")
}

#[cfg(windows)]
const WINDOWS_FILE_CONFLICT_MAX_ATTEMPTS: usize = 100;

#[cfg(windows)]
fn is_transient_windows_file_conflict(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(windows)]
fn sleep_for_windows_file_conflict(attempt: usize) {
    std::thread::sleep(std::time::Duration::from_millis(2 + (attempt as u64 % 7)));
}

/// Remove the progress file (best-effort).
pub fn clear_progress(data_dir: &Path) {
    let _ = std::fs::remove_file(progress_file_path(data_dir));
}

/// Issue a short-lived, one-time credential proving that a desktop Tauri
/// command initiated the next Admin upgrade request.
pub fn issue_desktop_upgrade_origin_token(data_dir: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(data_dir)?;
    clear_expired_desktop_upgrade_origin_tokens(data_dir);

    let token = uuid::Uuid::new_v4().to_string();
    let path = desktop_upgrade_origin_token_path(data_dir, &token);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(Utc::now().to_rfc3339().as_bytes())?;
    file.sync_all()?;
    Ok(token)
}

/// Atomically consume a desktop-issued origin credential. Invalid, expired,
/// or already-consumed credentials are rejected.
pub fn consume_desktop_upgrade_origin_token(data_dir: &Path, token: &str) -> bool {
    let Ok(token) = uuid::Uuid::parse_str(token) else {
        return false;
    };
    let token = token.to_string();
    let path = desktop_upgrade_origin_token_path(data_dir, &token);
    let claimed = path.with_extension(format!("claimed-{}", uuid::Uuid::new_v4()));
    if std::fs::rename(&path, &claimed).is_err() {
        return false;
    }

    let content = std::fs::read_to_string(&claimed);
    let _ = std::fs::remove_file(&claimed);
    let Ok(content) = content else {
        return false;
    };
    let Ok(issued_at) = chrono::DateTime::parse_from_rfc3339(content.trim()) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(issued_at.with_timezone(&Utc));
    (0..=DESKTOP_UPGRADE_ORIGIN_MAX_AGE_SECS).contains(&age.num_seconds())
}

fn desktop_upgrade_origin_token_path(data_dir: &Path, token: &str) -> PathBuf {
    data_dir.join(format!("{DESKTOP_UPGRADE_ORIGIN_PREFIX}{token}.json"))
}

fn clear_expired_desktop_upgrade_origin_tokens(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(DESKTOP_UPGRADE_ORIGIN_PREFIX) && name.ends_with(".json") {
            let valid = std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value.trim()).ok())
                .is_some_and(|issued_at| {
                    let age = Utc::now().signed_duration_since(issued_at.with_timezone(&Utc));
                    (0..=DESKTOP_UPGRADE_ORIGIN_MAX_AGE_SECS).contains(&age.num_seconds())
                });
            if !valid {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
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
        write_progress(
            &dir,
            &UpgradeProgress::new(UpgradePhase::Installing, "Installing"),
        );
        assert!(progress_file_path(&dir).exists());
        clear_progress(&dir);
        assert!(!progress_file_path(&dir).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_progress_writers_never_publish_partial_json_or_leave_temp_files() {
        let dir = temp_dir();
        let mut writers = Vec::new();
        for writer in 0..16usize {
            let dir = dir.clone();
            writers.push(std::thread::spawn(move || {
                for iteration in 0..32usize {
                    let progress = UpgradeProgress::new(
                        UpgradePhase::Downloading,
                        format!("writer-{writer}-iteration-{iteration}"),
                    )
                    .with_target(Some("0.0.156".to_string()))
                    .with_source(Some(format!("writer-{writer}")));
                    write_progress_inner(&dir, &progress).expect("write progress");
                    let published = read_progress_inner(&dir).expect("read published progress");
                    assert_eq!(published.phase, UpgradePhase::Downloading);
                    assert_eq!(published.target_version.as_deref(), Some("0.0.156"));
                }
            }));
        }
        for writer in writers {
            writer.join().expect("progress writer thread");
        }

        let persisted = read_progress(&dir);
        assert_eq!(persisted.phase, UpgradePhase::Downloading);
        assert_eq!(persisted.target_version.as_deref(), Some("0.0.156"));
        let leftovers = std::fs::read_dir(&dir)
            .expect("read temp directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != progress_file_path(&dir))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn desktop_upgrade_origin_token_is_one_time_and_rejects_wrong_values() {
        let dir = temp_dir();
        let token = issue_desktop_upgrade_origin_token(&dir).expect("issue desktop token");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(desktop_upgrade_origin_token_path(&dir, &token))
                .expect("origin token metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        assert!(!consume_desktop_upgrade_origin_token(
            &dir,
            "not-a-valid-token"
        ));
        assert!(consume_desktop_upgrade_origin_token(&dir, &token));
        assert!(!consume_desktop_upgrade_origin_token(&dir, &token));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn each_desktop_upgrade_origin_token_can_be_consumed_once() {
        let dir = temp_dir();
        let old = issue_desktop_upgrade_origin_token(&dir).expect("issue old token");
        let current = issue_desktop_upgrade_origin_token(&dir).expect("issue current token");

        assert!(consume_desktop_upgrade_origin_token(&dir, &old));
        assert!(consume_desktop_upgrade_origin_token(&dir, &current));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expired_desktop_upgrade_origin_is_rejected_and_consumed() {
        let dir = temp_dir();
        let token = uuid::Uuid::new_v4().to_string();
        let path = desktop_upgrade_origin_token_path(&dir, &token);
        std::fs::write(
            path,
            (Utc::now() - chrono::Duration::seconds(60)).to_rfc3339(),
        )
        .expect("write expired token");

        assert!(!consume_desktop_upgrade_origin_token(&dir, &token));
        assert!(!consume_desktop_upgrade_origin_token(&dir, &token));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_desktop_upgrade_origin_artifacts_are_rejected_and_cleaned() {
        let dir = temp_dir();

        let directory_token = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir(desktop_upgrade_origin_token_path(&dir, &directory_token))
            .expect("create directory-shaped token");
        assert!(!consume_desktop_upgrade_origin_token(
            &dir,
            &directory_token
        ));

        let malformed_token = uuid::Uuid::new_v4().to_string();
        std::fs::write(
            desktop_upgrade_origin_token_path(&dir, &malformed_token),
            "not-an-rfc3339-timestamp",
        )
        .expect("write malformed token");
        assert!(!consume_desktop_upgrade_origin_token(
            &dir,
            &malformed_token
        ));

        clear_expired_desktop_upgrade_origin_tokens(&dir.join("missing"));

        let stale_token = uuid::Uuid::new_v4().to_string();
        let stale_path = desktop_upgrade_origin_token_path(&dir, &stale_token);
        std::fs::write(&stale_path, "invalid timestamp").expect("write stale token");
        clear_expired_desktop_upgrade_origin_tokens(&dir);
        assert!(!stale_path.exists(), "invalid token must be removed");

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
        old.updated_at =
            (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 10)).to_rfc3339();
        assert!(is_stale(&old, DEFAULT_STALE_SECS));

        // Old but terminal => never stale.
        let mut done = UpgradeProgress::new(UpgradePhase::Completed, "x");
        done.updated_at =
            (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 10)).to_rfc3339();
        assert!(!is_stale(&done, DEFAULT_STALE_SECS));

        // Unparseable timestamp on active => stale.
        let mut bad = UpgradeProgress::new(UpgradePhase::Installing, "x");
        bad.updated_at = "not-a-date".to_string();
        assert!(is_stale(&bad, DEFAULT_STALE_SECS));
    }
}
