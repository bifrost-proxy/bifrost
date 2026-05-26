//! Usage frequency tracking for memory rollouts.
//!
//! Implements citation-based usage recording aligned with Codex's memory read
//! usage model. Tracks which rollouts are actually referenced during sessions
//! and uses this signal for retention decisions.

use crate::memory::state_db;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Tracks citation-based usage of rollout summaries and memory files.
pub(crate) struct UsageTracker {
    root: PathBuf,
}

impl UsageTracker {
    /// Create a new usage tracker backed by the state DB at `root`.
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Record that the given rollout_ids were cited/used in the current session.
    /// Increments usage_count and updates last_usage timestamp.
    pub(crate) fn record_citation_usage(&self, rollout_ids: &[String]) -> Result<(), String> {
        if rollout_ids.is_empty() {
            return Ok(());
        }
        state_db::record_usage(&self.root, rollout_ids)?;
        debug!(
            count = rollout_ids.len(),
            "recorded citation usage for rollouts"
        );
        Ok(())
    }
}
