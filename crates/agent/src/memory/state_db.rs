//! SQLite state database for memory consolidation state.

use crate::memory::types::Phase2State;
use rusqlite::Connection;
use std::path::Path;

const DB_FILENAME: &str = ".memory_state.db";

fn open_db(root: &Path) -> Result<Connection, String> {
    let db_path = root.join(DB_FILENAME);
    let conn =
        Connection::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?;
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
    .map_err(|e| format!("create table: {e}"))?;
    conn.execute("INSERT OR IGNORE INTO phase2_state (id) VALUES (1)", [])
        .map_err(|e| format!("init row: {e}"))?;
    Ok(conn)
}

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
        rusqlite::params![
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
