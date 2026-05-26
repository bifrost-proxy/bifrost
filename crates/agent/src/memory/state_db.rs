//! SQLite state database for memory consolidation state and rollout tracking.
//!
//! Aligned with Codex state DB design: persistent rollout metadata, Phase 1 job
//! scheduling, and usage-based retention tracking.

use crate::memory::types::{Phase1Status, Phase2State, RolloutRecord};
use crate::memory::utils::now_secs;
use rusqlite::{params, Connection};
use std::path::Path;
use std::time::Duration;

const DB_FILENAME: &str = ".memory_state.db";

/// Lease duration for Phase 1 claims (default: 5 minutes).
const DEFAULT_PHASE1_LEASE_SECS: i64 = 300;

/// Open (or create) the state database at `root/.memory_state.db` and run all
/// migrations. Each migration is idempotent (CREATE TABLE IF NOT EXISTS).
fn open_db(root: &Path) -> Result<Connection, String> {
    let db_path = root.join(DB_FILENAME);
    let conn =
        Connection::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?;

    // Enable WAL mode for concurrent readers.
    conn.execute_batch("PRAGMA journal_mode = WAL;")
        .map_err(|e| format!("set WAL: {e}"))?;

    // Migration 1: Phase 2 consolidation state (original table).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS phase2_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            last_input_hash TEXT NOT NULL DEFAULT '',
            processed_input_count INTEGER NOT NULL DEFAULT 0,
            total_input_count INTEGER NOT NULL DEFAULT 0,
            has_more_inputs INTEGER NOT NULL DEFAULT 0,
            updated_at_unix INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0,
            pinned_failure_hash TEXT,
            phase2_mode TEXT NOT NULL DEFAULT '',
            pollution_state TEXT
        );",
    )
    .map_err(|e| format!("create phase2_state: {e}"))?;
    conn.execute("INSERT OR IGNORE INTO phase2_state (id) VALUES (1)", [])
        .map_err(|e| format!("init phase2_state row: {e}"))?;

    // Migration 2: Rollout records (thread/session metadata + phase1 job state).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rollouts (
            thread_id TEXT PRIMARY KEY,
            cwd TEXT,
            rollout_path TEXT,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            ownership_token TEXT,
            phase1_status TEXT NOT NULL DEFAULT 'pending',
            phase1_status_ts INTEGER NOT NULL DEFAULT 0,
            usage_count INTEGER NOT NULL DEFAULT 0,
            last_usage INTEGER
        );",
    )
    .map_err(|e| format!("create rollouts: {e}"))?;

    // Migration 3: Indexes for efficient queries.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_rollouts_updated_at ON rollouts (updated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_rollouts_usage_count ON rollouts (usage_count DESC);
         CREATE INDEX IF NOT EXISTS idx_rollouts_phase1_status ON rollouts (phase1_status);",
    )
    .map_err(|e| format!("create rollout indexes: {e}"))?;

    // Migration 4: Lease/heartbeat table for Phase 2 distributed locking.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS leases (
            lease_key TEXT PRIMARY KEY,
            holder_token TEXT NOT NULL,
            claimed_at INTEGER NOT NULL,
            last_heartbeat INTEGER NOT NULL,
            lease_duration_secs INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("create leases: {e}"))?;

    Ok(conn)
}

// ---------------------------------------------------------------------------
// Phase 2 consolidation state (backward-compatible with original API)
// ---------------------------------------------------------------------------

pub(crate) fn load_phase2_state_from_db(root: &Path) -> Option<Phase2State> {
    let conn = open_db(root).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT last_input_hash, processed_input_count, total_input_count, \
             has_more_inputs, updated_at_unix, failure_count, pinned_failure_hash, \
             phase2_mode, pollution_state \
             FROM phase2_state WHERE id = 1",
        )
        .ok()?;
    let result = stmt
        .query_row([], |row| {
            Ok(Phase2State {
                last_input_hash: row.get(0).unwrap_or_default(),
                processed_input_count: row.get(1).unwrap_or(0),
                total_input_count: row.get(2).unwrap_or(0),
                has_more_inputs: row.get::<_, i32>(3).unwrap_or(0) != 0,
                updated_at_unix: row.get(4).unwrap_or(0),
                failure_count: row.get(5).unwrap_or(0),
                pinned_failure_hash: row.get(6).ok().flatten(),
                phase2_mode: row.get(7).unwrap_or_default(),
                pollution_state: row.get(8).ok().flatten(),
            })
        })
        .ok()?;
    Some(result)
}

pub(crate) fn save_phase2_state_to_db(root: &Path, state: &Phase2State) -> Result<(), String> {
    let conn = open_db(root)?;
    conn.execute(
        "UPDATE phase2_state SET \
         last_input_hash = ?1, processed_input_count = ?2, total_input_count = ?3, \
         has_more_inputs = ?4, updated_at_unix = ?5, failure_count = ?6, \
         pinned_failure_hash = ?7, phase2_mode = ?8, pollution_state = ?9 \
         WHERE id = 1",
        params![
            &state.last_input_hash,
            state.processed_input_count as i32,
            state.total_input_count as i32,
            if state.has_more_inputs { 1i32 } else { 0i32 },
            state.updated_at_unix as i64,
            state.failure_count as i32,
            &state.pinned_failure_hash,
            &state.phase2_mode,
            &state.pollution_state,
        ],
    )
    .map_err(|e| format!("update state: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rollout record CRUD
// ---------------------------------------------------------------------------

/// Insert or update a rollout record.
pub(crate) fn upsert_rollout(root: &Path, record: &RolloutRecord) -> Result<(), String> {
    let conn = open_db(root)?;
    conn.execute(
        "INSERT INTO rollouts (thread_id, cwd, rollout_path, created_at, updated_at, \
         ownership_token, phase1_status, phase1_status_ts, usage_count, last_usage) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(thread_id) DO UPDATE SET \
         cwd = excluded.cwd, \
         rollout_path = excluded.rollout_path, \
         updated_at = excluded.updated_at, \
         ownership_token = excluded.ownership_token, \
         phase1_status = excluded.phase1_status, \
         phase1_status_ts = excluded.phase1_status_ts, \
         usage_count = excluded.usage_count, \
         last_usage = excluded.last_usage",
        params![
            &record.thread_id,
            &record.cwd,
            record
                .rollout_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            record.created_at,
            record.updated_at,
            &record.ownership_token,
            record.phase1_status.as_db_tag(),
            record.phase1_status.associated_timestamp(),
            record.usage_count as i64,
            record.last_usage,
        ],
    )
    .map_err(|e| format!("upsert rollout: {e}"))?;
    Ok(())
}

/// Retrieve a rollout record by thread_id.
pub(crate) fn get_rollout(root: &Path, thread_id: &str) -> Result<Option<RolloutRecord>, String> {
    let conn = open_db(root)?;
    let mut stmt = conn
        .prepare(
            "SELECT thread_id, cwd, rollout_path, created_at, updated_at, \
             ownership_token, phase1_status, phase1_status_ts, usage_count, last_usage \
             FROM rollouts WHERE thread_id = ?1",
        )
        .map_err(|e| format!("prepare get_rollout: {e}"))?;
    let result = stmt
        .query_row(params![thread_id], |row| Ok(row_to_rollout_record(row)))
        .ok();
    Ok(result)
}

/// List rollouts ordered by `updated_at DESC`, with optional limit.
pub(crate) fn list_rollouts(root: &Path, limit: usize) -> Result<Vec<RolloutRecord>, String> {
    let conn = open_db(root)?;
    let mut stmt = conn
        .prepare(
            "SELECT thread_id, cwd, rollout_path, created_at, updated_at, \
             ownership_token, phase1_status, phase1_status_ts, usage_count, last_usage \
             FROM rollouts ORDER BY updated_at DESC LIMIT ?1",
        )
        .map_err(|e| format!("prepare list_rollouts: {e}"))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| Ok(row_to_rollout_record(row)))
        .map_err(|e| format!("query list_rollouts: {e}"))?;
    let mut records = Vec::new();
    for record in rows.flatten() {
        records.push(record);
    }
    Ok(records)
}

/// Delete a rollout record.
pub(crate) fn delete_rollout(root: &Path, thread_id: &str) -> Result<bool, String> {
    let conn = open_db(root)?;
    let affected = conn
        .execute(
            "DELETE FROM rollouts WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("delete rollout: {e}"))?;
    Ok(affected > 0)
}

// ---------------------------------------------------------------------------
// Phase 1 job scheduling
// ---------------------------------------------------------------------------

/// Claim pending Phase 1 jobs up to `batch_size`. Returns the thread_ids claimed.
/// Uses an exclusive lease so multiple workers don't double-process.
pub(crate) fn claim_phase1_jobs(
    root: &Path,
    batch_size: usize,
    lease_duration: Option<Duration>,
) -> Result<Vec<String>, String> {
    let conn = open_db(root)?;
    let now = now_secs() as i64;
    let lease_secs = lease_duration
        .map(|d| d.as_secs() as i64)
        .unwrap_or(DEFAULT_PHASE1_LEASE_SECS);
    let lease_expires = now + lease_secs;

    // Find pending jobs, or failed jobs past retry_after, or expired claimed jobs.
    let mut stmt = conn
        .prepare(
            "SELECT thread_id FROM rollouts \
             WHERE phase1_status = 'pending' \
                OR (phase1_status = 'failed' AND phase1_status_ts <= ?1) \
                OR (phase1_status = 'claimed' AND phase1_status_ts <= ?1) \
             ORDER BY created_at ASC \
             LIMIT ?2",
        )
        .map_err(|e| format!("prepare claim_phase1_jobs: {e}"))?;
    let thread_ids: Vec<String> = stmt
        .query_map(params![now, batch_size as i64], |row| row.get(0))
        .map_err(|e| format!("query claim_phase1_jobs: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    // Claim them atomically.
    if !thread_ids.is_empty() {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("begin tx: {e}"))?;
        for tid in &thread_ids {
            tx.execute(
                "UPDATE rollouts SET phase1_status = 'claimed', phase1_status_ts = ?1 \
                 WHERE thread_id = ?2",
                params![lease_expires, tid],
            )
            .map_err(|e| format!("claim update: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit claim: {e}"))?;
    }

    Ok(thread_ids)
}

/// Mark a Phase 1 job as completed.
pub(crate) fn complete_phase1_job(root: &Path, thread_id: &str) -> Result<(), String> {
    let conn = open_db(root)?;
    let now = now_secs() as i64;
    conn.execute(
        "UPDATE rollouts SET phase1_status = 'completed', phase1_status_ts = 0, updated_at = ?1 \
         WHERE thread_id = ?2",
        params![now, thread_id],
    )
    .map_err(|e| format!("complete_phase1_job: {e}"))?;
    Ok(())
}

/// Mark a Phase 1 job as failed with a retry backoff.
pub(crate) fn fail_phase1_job(
    root: &Path,
    thread_id: &str,
    retry_after_secs: u64,
) -> Result<(), String> {
    let conn = open_db(root)?;
    let retry_after = now_secs() as i64 + retry_after_secs as i64;
    conn.execute(
        "UPDATE rollouts SET phase1_status = 'failed', phase1_status_ts = ?1, updated_at = ?2 \
         WHERE thread_id = ?3",
        params![retry_after, now_secs() as i64, thread_id],
    )
    .map_err(|e| format!("fail_phase1_job: {e}"))?;
    Ok(())
}

/// Count rollouts by phase1_status.
pub(crate) fn count_rollouts_by_status(root: &Path) -> Result<Vec<(String, usize)>, String> {
    let conn = open_db(root)?;
    let mut stmt = conn
        .prepare("SELECT phase1_status, COUNT(*) FROM rollouts GROUP BY phase1_status")
        .map_err(|e| format!("prepare count_by_status: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((status, count as usize))
        })
        .map_err(|e| format!("query count_by_status: {e}"))?;
    let mut result = Vec::new();
    for pair in rows.flatten() {
        result.push(pair);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Usage tracking (citation-based)
// ---------------------------------------------------------------------------

/// Increment usage_count and update last_usage for the given thread_ids.
pub(crate) fn record_usage(root: &Path, thread_ids: &[String]) -> Result<(), String> {
    if thread_ids.is_empty() {
        return Ok(());
    }
    let conn = open_db(root)?;
    let now = now_secs() as i64;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("begin tx: {e}"))?;
    for tid in thread_ids {
        tx.execute(
            "UPDATE rollouts SET usage_count = usage_count + 1, last_usage = ?1 \
             WHERE thread_id = ?2",
            params![now, tid],
        )
        .map_err(|e| format!("record_usage: {e}"))?;
    }
    tx.commit().map_err(|e| format!("commit usage: {e}"))?;
    Ok(())
}

/// List rollouts sorted by usage_count DESC (most-used first).
pub(crate) fn get_usage_sorted_rollouts(
    root: &Path,
    limit: usize,
) -> Result<Vec<RolloutRecord>, String> {
    let conn = open_db(root)?;
    let mut stmt = conn
        .prepare(
            "SELECT thread_id, cwd, rollout_path, created_at, updated_at, \
             ownership_token, phase1_status, phase1_status_ts, usage_count, last_usage \
             FROM rollouts \
             WHERE phase1_status = 'completed' \
             ORDER BY usage_count DESC, updated_at DESC \
             LIMIT ?1",
        )
        .map_err(|e| format!("prepare usage_sorted: {e}"))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| Ok(row_to_rollout_record(row)))
        .map_err(|e| format!("query usage_sorted: {e}"))?;
    let mut records = Vec::new();
    for record in rows.flatten() {
        records.push(record);
    }
    Ok(records)
}

/// Delete rollouts that have not been used for `max_unused_days` days.
/// Returns the number of records pruned.
pub(crate) fn prune_unused_rollouts(root: &Path, max_unused_days: u64) -> Result<u64, String> {
    let conn = open_db(root)?;
    let cutoff = now_secs() as i64 - (max_unused_days as i64 * 86_400);
    let affected = conn
        .execute(
            "DELETE FROM rollouts \
             WHERE usage_count = 0 \
               AND (last_usage IS NULL OR last_usage < ?1) \
               AND updated_at < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("prune_unused: {e}"))?;
    Ok(affected as u64)
}

// ---------------------------------------------------------------------------
// Lease management (DB-backed heartbeat for Phase 2)
// ---------------------------------------------------------------------------

/// Try to claim a lease. Returns true if claimed successfully.
pub(crate) fn try_claim_lease(
    root: &Path,
    lease_key: &str,
    holder_token: &str,
    lease_duration: Duration,
) -> Result<bool, String> {
    let conn = open_db(root)?;
    let now = now_secs() as i64;
    let duration_secs = lease_duration.as_secs() as i64;

    // Check if an existing lease is still valid.
    let existing: Option<(String, i64, i64)> = conn
        .prepare("SELECT holder_token, last_heartbeat, lease_duration_secs FROM leases WHERE lease_key = ?1")
        .map_err(|e| format!("prepare check_lease: {e}"))?
        .query_row(params![lease_key], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .ok();

    if let Some((existing_holder, last_hb, existing_dur)) = existing {
        // Lease is still active if last_heartbeat + duration > now.
        if last_hb + existing_dur > now && existing_holder != holder_token {
            return Ok(false);
        }
    }

    // Claim or re-claim.
    conn.execute(
        "INSERT INTO leases (lease_key, holder_token, claimed_at, last_heartbeat, lease_duration_secs) \
         VALUES (?1, ?2, ?3, ?3, ?4) \
         ON CONFLICT(lease_key) DO UPDATE SET \
         holder_token = excluded.holder_token, \
         claimed_at = excluded.claimed_at, \
         last_heartbeat = excluded.last_heartbeat, \
         lease_duration_secs = excluded.lease_duration_secs",
        params![lease_key, holder_token, now, duration_secs],
    )
    .map_err(|e| format!("claim_lease: {e}"))?;
    Ok(true)
}

/// Send a heartbeat to keep a lease alive.
pub(crate) fn heartbeat_lease(
    root: &Path,
    lease_key: &str,
    holder_token: &str,
) -> Result<bool, String> {
    let conn = open_db(root)?;
    let now = now_secs() as i64;
    let affected = conn
        .execute(
            "UPDATE leases SET last_heartbeat = ?1 WHERE lease_key = ?2 AND holder_token = ?3",
            params![now, lease_key, holder_token],
        )
        .map_err(|e| format!("heartbeat_lease: {e}"))?;
    Ok(affected > 0)
}

/// Release a lease.
pub(crate) fn release_lease(
    root: &Path,
    lease_key: &str,
    holder_token: &str,
) -> Result<bool, String> {
    let conn = open_db(root)?;
    let affected = conn
        .execute(
            "DELETE FROM leases WHERE lease_key = ?1 AND holder_token = ?2",
            params![lease_key, holder_token],
        )
        .map_err(|e| format!("release_lease: {e}"))?;
    Ok(affected > 0)
}

/// Check whether a lease is expired given its last_heartbeat and duration.
pub fn is_lease_expired(last_heartbeat: i64, lease_duration: Duration) -> bool {
    let now = now_secs() as i64;
    last_heartbeat + lease_duration.as_secs() as i64 <= now
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_rollout_record(row: &rusqlite::Row) -> RolloutRecord {
    let thread_id: String = row.get(0).unwrap_or_default();
    let cwd: Option<String> = row.get(1).ok().flatten();
    let rollout_path_str: Option<String> = row.get(2).ok().flatten();
    let created_at: i64 = row.get(3).unwrap_or(0);
    let updated_at: i64 = row.get(4).unwrap_or(0);
    let ownership_token: Option<String> = row.get(5).ok().flatten();
    let phase1_tag: String = row.get(6).unwrap_or_default();
    let phase1_ts: i64 = row.get(7).unwrap_or(0);
    let usage_count: i64 = row.get(8).unwrap_or(0);
    let last_usage: Option<i64> = row.get(9).ok().flatten();

    RolloutRecord {
        thread_id,
        cwd,
        rollout_path: rollout_path_str.map(std::path::PathBuf::from),
        created_at,
        updated_at,
        ownership_token,
        phase1_status: Phase1Status::from_db(&phase1_tag, phase1_ts),
        usage_count: usage_count as u64,
        last_usage,
    }
}
