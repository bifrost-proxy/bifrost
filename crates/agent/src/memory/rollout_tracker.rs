//! Rollout tracking system for persistent session metadata.
//!
//! Aligned with Codex's state DB rollout model: tracks thread lifecycle,
//! Phase 1 extraction scheduling, and provides a high-level API for the
//! memory consolidation pipeline.

use crate::memory::state_db;
use crate::memory::types::{Phase1Status, RolloutRecord};
use crate::memory::utils::now_secs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, warn};

/// High-level rollout tracker that wraps state_db operations.
#[allow(dead_code)]
pub(crate) struct RolloutTracker {
    root: PathBuf,
}

#[allow(dead_code)]
impl RolloutTracker {
    /// Create a new tracker pointing at the memory root directory.
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Register a new rollout (session) in the state DB.
    /// If the rollout already exists, updates its metadata.
    pub(crate) fn register_rollout(
        &self,
        thread_id: &str,
        cwd: Option<&str>,
        rollout_path: Option<&Path>,
        ownership_token: Option<&str>,
    ) -> Result<(), String> {
        let now = now_secs() as i64;
        let existing = state_db::get_rollout(&self.root, thread_id)?;
        let record = if let Some(mut existing) = existing {
            existing.updated_at = now;
            if let Some(cwd) = cwd {
                existing.cwd = Some(cwd.to_string());
            }
            if let Some(path) = rollout_path {
                existing.rollout_path = Some(path.to_path_buf());
            }
            if let Some(token) = ownership_token {
                existing.ownership_token = Some(token.to_string());
            }
            existing
        } else {
            RolloutRecord {
                thread_id: thread_id.to_string(),
                cwd: cwd.map(|s| s.to_string()),
                rollout_path: rollout_path.map(|p| p.to_path_buf()),
                created_at: now,
                updated_at: now,
                ownership_token: ownership_token.map(|s| s.to_string()),
                phase1_status: Phase1Status::Pending,
                usage_count: 0,
                last_usage: None,
            }
        };
        state_db::upsert_rollout(&self.root, &record)?;
        debug!(thread_id, "rollout registered/updated in state DB");
        Ok(())
    }

    /// Get a rollout record by thread_id.
    pub(crate) fn get(&self, thread_id: &str) -> Result<Option<RolloutRecord>, String> {
        state_db::get_rollout(&self.root, thread_id)
    }

    /// List recent rollouts.
    pub(crate) fn list_recent(&self, limit: usize) -> Result<Vec<RolloutRecord>, String> {
        state_db::list_rollouts(&self.root, limit)
    }

    /// Delete a rollout from the state DB.
    pub(crate) fn remove(&self, thread_id: &str) -> Result<bool, String> {
        state_db::delete_rollout(&self.root, thread_id)
    }

    /// Claim a batch of Phase 1 extraction jobs.
    /// Returns thread_ids that were successfully claimed.
    pub(crate) fn claim_phase1_batch(
        &self,
        batch_size: usize,
        lease_duration: Option<Duration>,
    ) -> Result<Vec<String>, String> {
        let claimed = state_db::claim_phase1_jobs(&self.root, batch_size, lease_duration)?;
        if !claimed.is_empty() {
            debug!(count = claimed.len(), "claimed Phase 1 jobs");
        }
        Ok(claimed)
    }

    /// Mark a Phase 1 job as successfully completed.
    pub(crate) fn complete_phase1(&self, thread_id: &str) -> Result<(), String> {
        state_db::complete_phase1_job(&self.root, thread_id)?;
        debug!(thread_id, "Phase 1 job completed");
        Ok(())
    }

    /// Mark a Phase 1 job as failed with retry backoff.
    pub(crate) fn fail_phase1(&self, thread_id: &str, retry_after_secs: u64) -> Result<(), String> {
        state_db::fail_phase1_job(&self.root, thread_id, retry_after_secs)?;
        warn!(
            thread_id,
            retry_after_secs, "Phase 1 job failed, will retry"
        );
        Ok(())
    }

    /// Get counts of rollouts grouped by Phase 1 status.
    pub(crate) fn status_counts(&self) -> Result<Vec<(String, usize)>, String> {
        state_db::count_rollouts_by_status(&self.root)
    }

    /// Touch updated_at for a rollout.
    pub(crate) fn touch_updated_at(&self, thread_id: &str) -> Result<(), String> {
        if let Some(mut record) = state_db::get_rollout(&self.root, thread_id)? {
            record.updated_at = now_secs() as i64;
            state_db::upsert_rollout(&self.root, &record)?;
        }
        Ok(())
    }

    /// Get rollouts that are ready for Phase 2 consolidation input,
    /// sorted by usage_count DESC (most-cited first).
    pub(crate) fn get_phase2_candidates(&self, limit: usize) -> Result<Vec<RolloutRecord>, String> {
        state_db::get_usage_sorted_rollouts(&self.root, limit)
    }
}
