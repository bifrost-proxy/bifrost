//! Cross-process exclusive lock for Phase 2 consolidation.
//!
//! Provides both file-based advisory locking (Phase2LockGuard) and
//! DB-backed lease/heartbeat management (LeaseManager) aligned with
//! Codex's distributed locking model.

use crate::memory::state_db;
use crate::memory::utils::now_secs;
use fs2::FileExt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Cross-process exclusive lock for Phase 2 consolidation. Uses real `fs2`
/// advisory locking so that if a process dies abruptly the OS releases the
/// lock — no staleness heuristic required.
pub(crate) struct Phase2LockGuard {
    file: Option<fs::File>,
    path: PathBuf,
}

impl Drop for Phase2LockGuard {
    fn drop(&mut self) {
        if let Some(ref file) = self.file {
            let _ = FileExt::unlock(file);
        }
        // Keep the lock file on disk so that `ls -la` still shows it — the
        // advisory lock is what matters, not the file existence. Cleaning up
        // unconditionally can race with another process that is right now
        // trying to open+lock.
        let _ = &self.path;
    }
}

impl Phase2LockGuard {
    pub(crate) fn try_acquire(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(".phase2.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                // best-effort: record pid + timestamp for human debugging
                let note = format!("pid={} acquired_at={}\n", std::process::id(), now_secs());
                let _ = (&file).write_all(note.as_bytes());
                let _ = file.sync_data();
                Ok(Some(Self {
                    file: Some(file),
                    path,
                }))
            }
            Err(error) => {
                // contention: another process holds the lock.
                // On Unix this surfaces as WouldBlock; on Windows the OS
                // reports ERROR_LOCK_VIOLATION (raw os error 33).
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(33)
                {
                    return Ok(None);
                }
                Err(format!("lock {}: {error}", path.display()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DB-backed lease/heartbeat manager (aligned with Codex state DB leases)
// ---------------------------------------------------------------------------

/// Default lease key for Phase 2 consolidation.
#[allow(dead_code)]
const PHASE2_LEASE_KEY: &str = "phase2_consolidation";

/// DB-backed lease manager that supports heartbeats and distributed claim/release.
/// Unlike the file-based lock which relies on OS advisory locking, this approach
/// survives process restarts and works across machines sharing a DB.
#[allow(dead_code)]
pub(crate) struct LeaseManager {
    root: PathBuf,
    lease_duration: Duration,
    heartbeat_interval: Duration,
}

#[allow(dead_code)]
impl LeaseManager {
    /// Create a new lease manager.
    pub(crate) fn new(root: &Path, lease_duration: Duration, heartbeat_interval: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            lease_duration,
            heartbeat_interval,
        }
    }

    /// Default lease manager for Phase 2 with sensible defaults
    /// (30s lease, 10s heartbeat).
    pub(crate) fn phase2_default(root: &Path) -> Self {
        Self::new(root, Duration::from_secs(30), Duration::from_secs(10))
    }

    /// Get the configured heartbeat interval.
    pub(crate) fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }

    /// Try to claim the Phase 2 lease.
    /// Returns true if the lease was successfully acquired.
    pub(crate) fn try_claim_phase2(&self, token: &str) -> Result<bool, String> {
        state_db::try_claim_lease(&self.root, PHASE2_LEASE_KEY, token, self.lease_duration)
    }

    /// Send a heartbeat to keep the Phase 2 lease alive.
    /// Returns true if the heartbeat was accepted (we still hold the lease).
    pub(crate) fn heartbeat(&self, token: &str) -> Result<bool, String> {
        state_db::heartbeat_lease(&self.root, PHASE2_LEASE_KEY, token)
    }

    /// Release the Phase 2 lease.
    /// Returns true if the lease was successfully released.
    pub(crate) fn release(&self, token: &str) -> Result<bool, String> {
        state_db::release_lease(&self.root, PHASE2_LEASE_KEY, token)
    }

    /// Check whether a lease is expired.
    pub(crate) fn is_expired(last_heartbeat: i64) -> bool {
        // Use a generous default lease duration for expiry checks.
        state_db::is_lease_expired(last_heartbeat, Duration::from_secs(30))
    }

    /// Try to claim a custom-keyed lease.
    pub(crate) fn try_claim_custom(&self, lease_key: &str, token: &str) -> Result<bool, String> {
        state_db::try_claim_lease(&self.root, lease_key, token, self.lease_duration)
    }

    /// Send heartbeat on a custom-keyed lease.
    pub(crate) fn heartbeat_custom(&self, lease_key: &str, token: &str) -> Result<bool, String> {
        state_db::heartbeat_lease(&self.root, lease_key, token)
    }

    /// Release a custom-keyed lease.
    pub(crate) fn release_custom(&self, lease_key: &str, token: &str) -> Result<bool, String> {
        state_db::release_lease(&self.root, lease_key, token)
    }
}
