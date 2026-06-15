use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::connection_monitor::WebSocketFrameRecord;

const DEFAULT_RETENTION_HOURS: u64 = 24;
const FRAMES_SUBDIR: &str = "frames";
const TRAFFIC_DB_SUBDIR: &str = "traffic";
const FRAME_METADATA_TABLE: &str = "frame_connection_metadata";
const BATCH_FLUSH_INTERVAL_MS: u64 = 500;
const BATCH_SIZE_THRESHOLD: usize = 50;
const METADATA_CACHE_CAP: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameStoreMetadata {
    pub connection_id: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub frame_count: u64,
    pub last_frame_id: u64,
    pub is_closed: bool,
}

impl FrameStoreMetadata {
    pub fn new(connection_id: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            connection_id: connection_id.to_string(),
            created_at: now,
            updated_at: now,
            frame_count: 0,
            last_frame_id: 0,
            is_closed: false,
        }
    }
}

struct PendingFrames {
    frames: HashMap<String, Vec<WebSocketFrameRecord>>,
    last_flush: Instant,
}

impl Default for PendingFrames {
    fn default() -> Self {
        Self {
            frames: HashMap::new(),
            last_flush: Instant::now(),
        }
    }
}

pub struct FrameStore {
    base_dir: PathBuf,
    retention_hours: u64,
    write_conn: Mutex<Connection>,
    read_conn: Mutex<Connection>,
    metadata_cache: RwLock<LruCache<String, FrameStoreMetadata>>,
    pending_frames: Mutex<PendingFrames>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrameStoreMemoryStats {
    pub metadata_cache_len: usize,
    pub pending_connection_count: usize,
    pub pending_frames_total: usize,
    pub pending_preview_bytes: usize,
    pub pending_inline_ref_bytes: usize,
}

impl FrameStore {
    pub fn new(base_dir: PathBuf, retention_hours: Option<u64>) -> Self {
        let frames_dir = base_dir.join(FRAMES_SUBDIR);
        if !frames_dir.exists() {
            let _ = fs::create_dir_all(&frames_dir);
        }
        // Captured frames are sensitive; restrict directories to the owner on
        // Unix. Done once at construction to avoid per-write cost.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&frames_dir, fs::Permissions::from_mode(0o700)) {
                tracing::warn!(
                    "failed to set 0700 permissions on {}: {e}",
                    frames_dir.display()
                );
            }
        }

        let traffic_dir = base_dir.join(TRAFFIC_DB_SUBDIR);
        if !traffic_dir.exists() {
            let _ = fs::create_dir_all(&traffic_dir);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&traffic_dir, fs::Permissions::from_mode(0o700)) {
                tracing::warn!(
                    "failed to set 0700 permissions on {}: {e}",
                    traffic_dir.display()
                );
            }
        }

        let db_path = traffic_dir.join("traffic.db");
        let write_conn = Self::open_metadata_connection(&db_path, false);
        let read_conn = Self::open_metadata_connection(&db_path, true);

        Self {
            base_dir,
            retention_hours: retention_hours.unwrap_or(DEFAULT_RETENTION_HOURS),
            write_conn: Mutex::new(write_conn),
            read_conn: Mutex::new(read_conn),
            metadata_cache: RwLock::new(LruCache::new(
                NonZeroUsize::new(METADATA_CACHE_CAP).unwrap(),
            )),
            pending_frames: Mutex::new(PendingFrames::default()),
        }
    }

    fn frames_dir(&self) -> PathBuf {
        self.base_dir.join(FRAMES_SUBDIR)
    }

    fn connection_file_path(&self, connection_id: &str) -> PathBuf {
        let safe_id = connection_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.frames_dir().join(format!("{}.jsonl", safe_id))
    }

    fn open_metadata_connection(db_path: &PathBuf, query_only: bool) -> Connection {
        let conn = Connection::open(db_path).unwrap_or_else(|error| {
            tracing::warn!(
                error = %error,
                db_path = %db_path.display(),
                "[FRAME_STORE] Failed to open frame metadata DB, falling back to in-memory SQLite"
            );
            Connection::open_in_memory()
                .expect("in-memory SQLite should be available for frame metadata")
        });

        if let Err(error) = Self::init_metadata_database(&conn, query_only) {
            tracing::warn!(
                error = %error,
                db_path = %db_path.display(),
                "[FRAME_STORE] Failed to initialize frame metadata table"
            );
        }

        conn
    }

    fn init_metadata_database(conn: &Connection, query_only: bool) -> rusqlite::Result<()> {
        if !query_only {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = 1000; PRAGMA mmap_size = 134217728;",
            )?;

            conn.execute_batch(&format!(
                "
                CREATE TABLE IF NOT EXISTS {FRAME_METADATA_TABLE} (
                    connection_id TEXT PRIMARY KEY NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    frame_count INTEGER NOT NULL DEFAULT 0,
                    last_frame_id INTEGER NOT NULL DEFAULT 0,
                    is_closed INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_frame_metadata_updated
                    ON {FRAME_METADATA_TABLE}(updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_frame_metadata_closed_updated
                    ON {FRAME_METADATA_TABLE}(is_closed, updated_at DESC);
                "
            ))?;
        } else {
            conn.execute_batch(
                "PRAGMA query_only = true; PRAGMA cache_size = 500; PRAGMA mmap_size = 67108864;",
            )?;
        }

        Ok(())
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn load_metadata_from_db(&self, connection_id: &str) -> Option<FrameStoreMetadata> {
        let conn = self.read_conn.lock();
        conn.query_row(
            &format!(
                "SELECT connection_id, created_at, updated_at, frame_count, last_frame_id, is_closed
                 FROM {FRAME_METADATA_TABLE}
                 WHERE connection_id = ?1"
            ),
            params![connection_id],
            |row| {
                Ok(FrameStoreMetadata {
                    connection_id: row.get(0)?,
                    created_at: row.get::<_, i64>(1)? as u64,
                    updated_at: row.get::<_, i64>(2)? as u64,
                    frame_count: row.get::<_, i64>(3)? as u64,
                    last_frame_id: row.get::<_, i64>(4)? as u64,
                    is_closed: row.get::<_, i64>(5)? != 0,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    fn upsert_metadata(&self, metadata: &FrameStoreMetadata) {
        {
            let conn = self.write_conn.lock();
            if let Err(error) = conn.execute(
                &format!(
                    "INSERT INTO {FRAME_METADATA_TABLE} (
                        connection_id, created_at, updated_at, frame_count, last_frame_id, is_closed
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    ON CONFLICT(connection_id) DO UPDATE SET
                        created_at = excluded.created_at,
                        updated_at = excluded.updated_at,
                        frame_count = excluded.frame_count,
                        last_frame_id = excluded.last_frame_id,
                        is_closed = excluded.is_closed"
                ),
                params![
                    metadata.connection_id,
                    metadata.created_at as i64,
                    metadata.updated_at as i64,
                    metadata.frame_count as i64,
                    metadata.last_frame_id as i64,
                    if metadata.is_closed { 1 } else { 0 },
                ],
            ) {
                tracing::warn!(
                    error = %error,
                    connection_id = %metadata.connection_id,
                    "[FRAME_STORE] Failed to persist frame metadata"
                );
            }
        }

        self.metadata_cache
            .write()
            .put(metadata.connection_id.clone(), metadata.clone());
    }

    fn delete_metadata_rows(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }

        let conn = self.write_conn.lock();
        let sql = format!("DELETE FROM {FRAME_METADATA_TABLE} WHERE connection_id = ?1");
        for id in ids {
            if let Err(error) = conn.execute(&sql, params![id]) {
                tracing::warn!(
                    error = %error,
                    connection_id = %id,
                    "[FRAME_STORE] Failed to delete frame metadata row"
                );
            }
        }
    }

    fn load_all_connection_ids_from_db(&self) -> Vec<String> {
        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare(&format!(
            "SELECT connection_id FROM {FRAME_METADATA_TABLE} ORDER BY updated_at DESC"
        )) {
            Ok(stmt) => stmt,
            Err(error) => {
                tracing::warn!(error = %error, "[FRAME_STORE] Failed to query frame metadata IDs");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(error = %error, "[FRAME_STORE] Failed to iterate frame metadata IDs");
                return Vec::new();
            }
        };
        rows.filter_map(Result::ok).collect()
    }

    fn load_expired_connection_ids_from_db(&self, cutoff_timestamp: u64) -> Vec<String> {
        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare(&format!(
            "SELECT connection_id FROM {FRAME_METADATA_TABLE}
             WHERE is_closed = 1 AND updated_at < ?1"
        )) {
            Ok(stmt) => stmt,
            Err(error) => {
                tracing::warn!(error = %error, "[FRAME_STORE] Failed to query expired frame metadata");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(params![cutoff_timestamp as i64], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(error = %error, "[FRAME_STORE] Failed to iterate expired frame metadata");
                return Vec::new();
            }
        };
        rows.filter_map(Result::ok).collect()
    }

    fn save_metadata(&self, metadata: &FrameStoreMetadata) {
        self.upsert_metadata(metadata);
    }

    pub fn append_frame(
        &self,
        connection_id: &str,
        frame: &WebSocketFrameRecord,
    ) -> std::io::Result<()> {
        let should_flush = {
            let mut pending = self.pending_frames.lock();
            pending
                .frames
                .entry(connection_id.to_string())
                .or_default()
                .push(frame.clone());

            let total_frames: usize = pending.frames.values().map(|v| v.len()).sum();
            let time_elapsed = pending.last_flush.elapsed().as_millis() as u64;

            total_frames >= BATCH_SIZE_THRESHOLD || time_elapsed >= BATCH_FLUSH_INTERVAL_MS
        };

        if should_flush {
            self.flush_pending_frames();
        }

        Ok(())
    }

    pub fn load_pending_frames(
        &self,
        connection_id: &str,
        after_frame_id: Option<u64>,
        limit: usize,
    ) -> Vec<WebSocketFrameRecord> {
        let pending = self.pending_frames.lock();
        let frames = match pending.frames.get(connection_id) {
            Some(frames) => frames,
            None => return Vec::new(),
        };

        let iter = frames.iter().filter(|f| {
            after_frame_id
                .map(|after| f.frame_id > after)
                .unwrap_or(true)
        });

        iter.take(limit).cloned().collect()
    }

    fn flush_pending_frames(&self) {
        let frames_to_write: HashMap<String, Vec<WebSocketFrameRecord>> = {
            let mut pending = self.pending_frames.lock();
            pending.last_flush = Instant::now();
            std::mem::take(&mut pending.frames)
        };

        if frames_to_write.is_empty() {
            return;
        }

        for (connection_id, frames) in frames_to_write {
            if frames.is_empty() {
                continue;
            }

            let path = self.connection_file_path(&connection_id);
            let file = match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(
                        "[FRAME_STORE] Failed to open file for {}: {}",
                        connection_id,
                        e
                    );
                    continue;
                }
            };

            let mut writer = BufWriter::new(file);
            let mut last_frame_id = 0u64;
            let frame_count = frames.len() as u64;

            for frame in &frames {
                if serde_json::to_writer(&mut writer, frame).is_ok() {
                    let _ = writer.write_all(b"\n");
                    last_frame_id = frame.frame_id;
                }
            }
            let _ = writer.flush();

            let mut cache = self.metadata_cache.write();
            if cache.get(&connection_id).is_none() {
                cache.put(
                    connection_id.clone(),
                    FrameStoreMetadata::new(&connection_id),
                );
            }
            let metadata = cache.get_mut(&connection_id).unwrap();

            metadata.frame_count += frame_count;
            metadata.last_frame_id = last_frame_id;
            metadata.updated_at = Self::now_ms();

            let m = metadata.clone();
            drop(cache);
            self.save_metadata(&m);
        }
    }

    pub fn flush(&self) {
        self.flush_pending_frames();
    }

    pub fn load_frames(
        &self,
        connection_id: &str,
        after_frame_id: Option<u64>,
        limit: usize,
    ) -> std::io::Result<(Vec<WebSocketFrameRecord>, bool)> {
        let path = self.connection_file_path(connection_id);

        if !path.exists() {
            return Ok((Vec::new(), false));
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);

        let mut frames = Vec::new();
        let mut has_more = false;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<WebSocketFrameRecord>(&line) {
                Ok(frame) => {
                    let should_include = match after_frame_id {
                        Some(after_id) => frame.frame_id > after_id,
                        None => true,
                    };

                    if should_include {
                        if frames.len() < limit {
                            frames.push(frame);
                        } else {
                            has_more = true;
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[FRAME_STORE] Failed to parse frame line for {}: {}",
                        connection_id,
                        e
                    );
                }
            }
        }

        Ok((frames, has_more))
    }

    pub fn load_frame_by_id(
        &self,
        connection_id: &str,
        frame_id: u64,
    ) -> std::io::Result<Option<WebSocketFrameRecord>> {
        let path = self.connection_file_path(connection_id);
        if !path.exists() {
            return Ok(None);
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<WebSocketFrameRecord>(&line) {
                Ok(frame) => {
                    if frame.frame_id == frame_id {
                        return Ok(Some(frame));
                    }
                    if frame.frame_id > frame_id {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[FRAME_STORE] Failed to parse frame line for {}: {}",
                        connection_id,
                        e
                    );
                }
            }
        }

        Ok(None)
    }

    pub fn load_all_frames(
        &self,
        connection_id: &str,
    ) -> std::io::Result<Vec<WebSocketFrameRecord>> {
        let (frames, _) = self.load_frames(connection_id, None, usize::MAX)?;
        Ok(frames)
    }

    pub fn get_metadata(&self, connection_id: &str) -> Option<FrameStoreMetadata> {
        if let Some(metadata) = self.metadata_cache.write().get(connection_id).cloned() {
            return Some(metadata);
        }

        let metadata = self.load_metadata_from_db(connection_id)?;
        self.metadata_cache
            .write()
            .put(connection_id.to_string(), metadata.clone());
        Some(metadata)
    }

    pub fn mark_connection_closed(&self, connection_id: &str) {
        let existing_metadata = self.get_metadata(connection_id);
        {
            let mut pending = self.pending_frames.lock();
            if let Some(frames) = pending.frames.remove(connection_id) {
                if !frames.is_empty() {
                    let path = self.connection_file_path(connection_id);
                    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
                        let mut writer = BufWriter::new(file);
                        for frame in &frames {
                            if serde_json::to_writer(&mut writer, frame).is_ok() {
                                let _ = writer.write_all(b"\n");
                            }
                        }
                        let _ = writer.flush();

                        let mut cache = self.metadata_cache.write();
                        if cache.get(connection_id).is_none() {
                            cache.put(
                                connection_id.to_string(),
                                existing_metadata
                                    .clone()
                                    .unwrap_or_else(|| FrameStoreMetadata::new(connection_id)),
                            );
                        }
                        let metadata = cache.get_mut(connection_id).unwrap();
                        metadata.frame_count += frames.len() as u64;
                        if let Some(last_frame) = frames.last() {
                            metadata.last_frame_id = last_frame.frame_id;
                        }
                        metadata.updated_at = Self::now_ms();
                    }
                }
            }
        }

        let mut cache = self.metadata_cache.write();
        if cache.get(connection_id).is_none() {
            cache.put(
                connection_id.to_string(),
                existing_metadata.unwrap_or_else(|| FrameStoreMetadata::new(connection_id)),
            );
        }
        let metadata = cache.get_mut(connection_id).unwrap();
        metadata.is_closed = true;
        metadata.updated_at = Self::now_ms();
        let m = metadata.clone();
        drop(cache);
        self.save_metadata(&m);
    }

    pub fn connection_exists(&self, connection_id: &str) -> bool {
        self.connection_file_path(connection_id).exists()
    }

    pub fn get_last_frame_id(&self, connection_id: &str) -> Option<u64> {
        self.get_metadata(connection_id).map(|m| m.last_frame_id)
    }

    pub fn cleanup_expired(&self) -> std::io::Result<usize> {
        let frames_dir = self.frames_dir();
        if !frames_dir.exists() {
            return Ok(0);
        }

        let retention_duration = Duration::from_secs(self.retention_hours * 60 * 60);
        let mut removed_count = 0;
        let cutoff_timestamp = Self::now_ms().saturating_sub(retention_duration.as_millis() as u64);
        let to_remove = self.load_expired_connection_ids_from_db(cutoff_timestamp);

        for connection_id in to_remove {
            if self.remove_connection(&connection_id).is_ok() {
                removed_count += 1;
            }
        }

        tracing::info!(
            "[FRAME_STORE] Cleaned up {} expired frame files",
            removed_count
        );
        Ok(removed_count)
    }

    pub fn remove_connection(&self, connection_id: &str) -> std::io::Result<()> {
        let frame_path = self.connection_file_path(connection_id);

        if frame_path.exists() {
            fs::remove_file(&frame_path)?;
        }

        self.metadata_cache.write().pop(connection_id);
        self.delete_metadata_rows(&[connection_id.to_string()]);
        Ok(())
    }

    pub fn list_connections(&self) -> Vec<String> {
        self.load_all_connection_ids_from_db()
    }

    pub fn clear(&self) -> std::io::Result<usize> {
        let frames_dir = self.frames_dir();
        let mut removed_count = 0;

        if frames_dir.exists() {
            if let Ok(entries) = fs::read_dir(&frames_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && fs::remove_file(&path).is_ok() {
                        removed_count += 1;
                    }
                }
            }
        }

        self.metadata_cache.write().clear();
        {
            let conn = self.write_conn.lock();
            if let Err(error) = conn.execute(&format!("DELETE FROM {FRAME_METADATA_TABLE}"), []) {
                tracing::warn!(error = %error, "[FRAME_STORE] Failed to clear frame metadata table");
            }
        }

        tracing::info!("[FRAME_STORE] Cleared {} frame files", removed_count);

        Ok(removed_count)
    }

    pub fn delete_by_ids(&self, ids: &[String]) -> std::io::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let frames_dir = self.frames_dir();
        let ids_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut removed_count = 0;

        if frames_dir.exists() {
            if let Ok(entries) = fs::read_dir(&frames_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                            if ids_set.contains(file_stem) && fs::remove_file(&path).is_ok() {
                                removed_count += 1;
                            }
                        }
                    }
                }
            }
        }

        {
            let mut metadata_cache = self.metadata_cache.write();
            for id in ids {
                metadata_cache.pop(id);
            }
        }
        self.delete_metadata_rows(ids);

        tracing::debug!(
            count = removed_count,
            "[FRAME_STORE] Deleted frame files by ids"
        );

        Ok(removed_count)
    }

    pub fn stats(&self) -> FrameStoreStats {
        let frames_dir = self.frames_dir();
        let mut file_count = 0;
        let mut total_size = 0u64;

        if frames_dir.exists() {
            if let Ok(entries) = fs::read_dir(&frames_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                        file_count += 1;
                        if let Ok(metadata) = entry.metadata() {
                            total_size += metadata.len();
                        }
                    }
                }
            }
        }

        FrameStoreStats {
            connection_count: file_count,
            total_size,
            frames_dir: frames_dir.to_string_lossy().to_string(),
            retention_hours: self.retention_hours,
        }
    }

    pub fn memory_stats(&self) -> FrameStoreMemoryStats {
        let metadata_cache_len = self.metadata_cache.read().len();
        let pending = self.pending_frames.lock();

        let mut out = FrameStoreMemoryStats {
            metadata_cache_len,
            pending_connection_count: pending.frames.len(),
            ..Default::default()
        };

        for frames in pending.frames.values() {
            out.pending_frames_total = out.pending_frames_total.saturating_add(frames.len());
            for f in frames.iter() {
                if let Some(p) = &f.payload_preview {
                    out.pending_preview_bytes = out.pending_preview_bytes.saturating_add(p.len());
                }
                if let Some(p) = &f.raw_payload_preview {
                    out.pending_preview_bytes = out.pending_preview_bytes.saturating_add(p.len());
                }
                if let Some(crate::body_store::BodyRef::Inline { data }) = &f.payload_ref {
                    out.pending_inline_ref_bytes =
                        out.pending_inline_ref_bytes.saturating_add(data.len());
                }
                if let Some(crate::body_store::BodyRef::Inline { data }) = &f.raw_payload_ref {
                    out.pending_inline_ref_bytes =
                        out.pending_inline_ref_bytes.saturating_add(data.len());
                }
            }
        }

        out
    }

    pub fn sizes_by_id(&self) -> std::io::Result<std::collections::HashMap<String, u64>> {
        let mut sizes = std::collections::HashMap::new();
        let frames_dir = self.frames_dir();
        if !frames_dir.exists() {
            return Ok(sizes);
        }
        for entry in fs::read_dir(&frames_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let id = file_name.strip_suffix(".jsonl");
                    if let Some(base_id) = id {
                        *sizes.entry(base_id.to_string()).or_insert(0) += size;
                    }
                }
            }
        }
        Ok(sizes)
    }

    pub fn recently_modified_ids(&self, max_age: Duration) -> HashSet<String> {
        let mut ids = HashSet::new();
        let frames_dir = self.frames_dir();
        if !frames_dir.exists() {
            return ids;
        }
        let now = SystemTime::now();
        let entries = match fs::read_dir(&frames_dir) {
            Ok(e) => e,
            Err(_) => return ids,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let within_age = (|| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                let age = now.duration_since(modified).ok()?;
                Some(age <= max_age)
            })()
            .unwrap_or(false);
            if within_age {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    if let Some(base_id) = file_name.strip_suffix(".jsonl") {
                        ids.insert(base_id.to_string());
                    }
                }
            }
        }
        ids
    }
}

impl Drop for FrameStore {
    fn drop(&mut self) {
        self.flush_pending_frames();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameStoreStats {
    pub connection_count: usize,
    pub total_size: u64,
    pub frames_dir: String,
    pub retention_hours: u64,
}

pub type SharedFrameStore = Arc<FrameStore>;

pub fn start_frame_cleanup_task(store: SharedFrameStore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Ok(removed) = store.cleanup_expired() {
                if removed > 0 {
                    tracing::info!(
                        "[FRAME_STORE] Periodic cleanup removed {} expired frame files",
                        removed
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic::{FrameDirection, FrameType};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "bifrost_frame_store_test_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            counter
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn cleanup_test_dir(dir: &PathBuf) {
        let _ = fs::remove_dir_all(dir);
    }

    fn create_test_frame(frame_id: u64, payload: &str) -> WebSocketFrameRecord {
        WebSocketFrameRecord {
            frame_id,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            direction: FrameDirection::Send,
            frame_type: FrameType::Text,
            payload_size: payload.len(),
            payload_is_text: true,
            payload_preview: Some(payload.to_string()),
            payload_ref: None,
            raw_payload_size: None,
            raw_payload_is_text: None,
            raw_payload_preview: None,
            raw_payload_ref: None,
            is_masked: false,
            is_fin: true,
        }
    }

    #[test]
    fn test_append_and_load_frames() {
        let dir = create_test_dir();
        let store = FrameStore::new(dir.clone(), Some(24));

        let frame1 = create_test_frame(0, "Hello");
        let frame2 = create_test_frame(1, "World");

        store.append_frame("conn-1", &frame1).unwrap();
        store.append_frame("conn-1", &frame2).unwrap();
        store.flush();

        let (frames, has_more) = store.load_frames("conn-1", None, 10).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(!has_more);
        assert_eq!(frames[0].frame_id, 0);
        assert_eq!(frames[1].frame_id, 1);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_load_frames_with_after() {
        let dir = create_test_dir();
        let store = FrameStore::new(dir.clone(), Some(24));

        for i in 0..5 {
            let frame = create_test_frame(i, &format!("Message {}", i));
            store.append_frame("conn-1", &frame).unwrap();
        }
        store.flush();

        let (frames, _) = store.load_frames("conn-1", Some(2), 10).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame_id, 3);
        assert_eq!(frames[1].frame_id, 4);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_metadata() {
        let dir = create_test_dir();
        let store = FrameStore::new(dir.clone(), Some(24));

        let frame = create_test_frame(0, "Test");
        store.append_frame("conn-1", &frame).unwrap();
        store.flush();

        let metadata = store.get_metadata("conn-1").unwrap();
        assert_eq!(metadata.connection_id, "conn-1");
        assert_eq!(metadata.frame_count, 1);
        assert_eq!(metadata.last_frame_id, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_connection_closed() {
        let dir = create_test_dir();
        let store = FrameStore::new(dir.clone(), Some(24));

        let frame = create_test_frame(0, "Test");
        store.append_frame("conn-1", &frame).unwrap();
        store.mark_connection_closed("conn-1");

        let metadata = store.get_metadata("conn-1").unwrap();
        assert!(metadata.is_closed);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_metadata_persists_in_sqlite() {
        let dir = create_test_dir();
        {
            let store = FrameStore::new(dir.clone(), Some(24));
            let frame = create_test_frame(7, "Persist");
            store.append_frame("conn-persist", &frame).unwrap();
            store.mark_connection_closed("conn-persist");
        }

        let reopened = FrameStore::new(dir.clone(), Some(24));
        let metadata = reopened.get_metadata("conn-persist").unwrap();
        assert_eq!(metadata.connection_id, "conn-persist");
        assert_eq!(metadata.frame_count, 1);
        assert_eq!(metadata.last_frame_id, 7);
        assert!(metadata.is_closed);
        assert_eq!(
            reopened.list_connections(),
            vec!["conn-persist".to_string()]
        );

        cleanup_test_dir(&dir);
    }
}
