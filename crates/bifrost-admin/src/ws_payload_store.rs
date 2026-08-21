use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use lru::LruCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::body_store::BodyRef;
use crate::resource_alerts::{resource_alert_level, ResourceAlertLevel};

const WS_PAYLOAD_SUBDIR: &str = "ws_payload";

struct WsPayloadWriter {
    path: PathBuf,
    file: fs::File,
    buffer: Vec<u8>,
    size: u64,
    last_flush: Instant,
    flush_bytes: usize,
    flush_interval: Duration,
}

impl WsPayloadWriter {
    fn new(path: PathBuf, flush_bytes: usize, flush_interval: Duration) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            buffer: Vec::with_capacity(flush_bytes),
            size,
            last_flush: Instant::now(),
            flush_bytes,
            flush_interval,
        })
    }

    fn append(&mut self, bytes: &[u8]) -> std::io::Result<BodyRef> {
        let offset = self.size;
        self.size += bytes.len() as u64;
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() >= self.flush_bytes || self.last_flush.elapsed() >= self.flush_interval
        {
            self.flush()?;
        }
        Ok(BodyRef::FileRange {
            path: self.path.to_string_lossy().to_string(),
            offset,
            size: bytes.len(),
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.buffer)?;
        self.file.flush()?;
        self.buffer.clear();
        self.last_flush = Instant::now();
        Ok(())
    }

    fn update_config(&mut self, flush_bytes: usize, flush_interval: Duration) {
        self.flush_bytes = flush_bytes;
        self.flush_interval = flush_interval;
        if self.buffer.capacity() < flush_bytes {
            self.buffer.reserve(flush_bytes - self.buffer.capacity());
        }
    }
}

struct WsPayloadStoreState {
    flush_bytes: usize,
    flush_interval: Duration,
    max_open_files: usize,
    retention_days: u64,
    writers: LruCache<String, WsPayloadWriter>,
}

pub struct WsPayloadStore {
    base_dir: PathBuf,
    state: Mutex<WsPayloadStoreState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WsPayloadStoreMemoryStats {
    pub writer_count: usize,
    pub max_open_files: usize,
    pub total_buffer_len: usize,
    pub total_buffer_capacity: usize,
    pub flush_bytes: usize,
    pub flush_interval_ms: u64,
    pub retention_days: u64,
}

#[derive(Debug, Clone, Default)]
pub struct WsPayloadStoreConfigUpdate {
    pub flush_bytes: Option<usize>,
    pub flush_interval_ms: Option<u64>,
    pub max_open_files: Option<usize>,
    pub retention_days: Option<u64>,
}

impl WsPayloadStore {
    pub fn new(
        base_dir: PathBuf,
        flush_bytes: usize,
        flush_interval: Duration,
        max_open_files: usize,
        retention_days: u64,
    ) -> Self {
        let payload_dir = base_dir.join(WS_PAYLOAD_SUBDIR);
        if !payload_dir.exists() {
            let _ = fs::create_dir_all(&payload_dir);
        }
        // Captured WebSocket payloads are sensitive; restrict the directory to
        // the owner on Unix. Done once at construction to avoid per-write cost.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&payload_dir, fs::Permissions::from_mode(0o700)) {
                tracing::warn!(
                    "failed to set 0700 permissions on {}: {e}",
                    payload_dir.display()
                );
            }
        }
        let cache_size = std::num::NonZeroUsize::new(max_open_files.max(1))
            .unwrap_or_else(|| std::num::NonZeroUsize::new(1).expect("non-zero"));
        Self {
            base_dir,
            state: Mutex::new(WsPayloadStoreState {
                flush_bytes,
                flush_interval,
                max_open_files,
                retention_days,
                writers: LruCache::new(cache_size),
            }),
        }
    }

    fn payload_dir(&self) -> PathBuf {
        self.base_dir.join(WS_PAYLOAD_SUBDIR)
    }

    pub fn safe_connection_id(connection_id: &str) -> String {
        connection_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
    }

    fn connection_path(&self, safe_id: &str) -> PathBuf {
        self.payload_dir().join(format!("{}.bin", safe_id))
    }

    pub fn append_bytes(&self, connection_id: &str, bytes: &[u8]) -> Option<BodyRef> {
        if bytes.is_empty() || !bifrost_core::payload_persistence_allowed() {
            return None;
        }
        let safe_id = Self::safe_connection_id(connection_id);
        let path = self.connection_path(&safe_id);
        let mut state = self.state.lock();
        if state.writers.get(&safe_id).is_none() {
            if let Ok(writer) = WsPayloadWriter::new(path, state.flush_bytes, state.flush_interval)
            {
                state.writers.put(safe_id.clone(), writer);
                while state.writers.len() > state.max_open_files {
                    if let Some((_id, mut evicted)) = state.writers.pop_lru() {
                        let _ = evicted.flush();
                    } else {
                        break;
                    }
                }
                let active_writers = state.writers.len();
                let level = resource_alert_level(active_writers, state.max_open_files);
                if matches!(
                    level,
                    ResourceAlertLevel::Warn | ResourceAlertLevel::Critical
                ) {
                    tracing::warn!(
                        active_writers,
                        max_open_files = state.max_open_files,
                        level = ?level,
                        "[WS_PAYLOAD_STORE] writer usage is approaching the open-file limit"
                    );
                }
            } else {
                return None;
            }
        }
        let writer = state.writers.get_mut(&safe_id)?;
        writer.append(bytes).ok()
    }

    pub fn is_ws_payload_ref(&self, body_ref: &BodyRef) -> bool {
        match body_ref {
            BodyRef::FileRange { path, .. } => PathBuf::from(path).starts_with(self.payload_dir()),
            _ => false,
        }
    }

    pub fn read_range(&self, body_ref: &BodyRef) -> Option<Vec<u8>> {
        let (path, offset, size) = match body_ref {
            BodyRef::FileRange { path, offset, size } => (path, *offset, *size),
            _ => return None,
        };

        let safe_id = PathBuf::from(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        if let Some(safe_id) = safe_id {
            let mut state = self.state.lock();
            if let Some(writer) = state.writers.get_mut(&safe_id) {
                let _ = writer.flush();
            }
        }

        let path = PathBuf::from(path);
        if !path.exists() {
            return None;
        }
        let mut file = fs::File::open(&path).ok()?;
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut contents = vec![0u8; size];
        let mut read_size = 0usize;
        while read_size < size {
            let n = file.read(&mut contents[read_size..]).ok()?;
            if n == 0 {
                break;
            }
            read_size += n;
        }
        contents.truncate(read_size);
        Some(contents)
    }

    pub fn close(&self, connection_id: &str) {
        let safe_id = Self::safe_connection_id(connection_id);
        let mut state = self.state.lock();
        if let Some(mut writer) = state.writers.pop(&safe_id) {
            let _ = writer.flush();
        }
    }

    pub fn update_config(&self, update: WsPayloadStoreConfigUpdate) {
        let mut state = self.state.lock();
        if let Some(flush_bytes) = update.flush_bytes {
            state.flush_bytes = flush_bytes;
        }
        if let Some(interval_ms) = update.flush_interval_ms {
            state.flush_interval = Duration::from_millis(interval_ms);
        }
        if let Some(max_open_files) = update.max_open_files {
            state.max_open_files = max_open_files.max(1);
        }
        if let Some(retention_days) = update.retention_days {
            state.retention_days = retention_days;
        }
        let flush_bytes = state.flush_bytes;
        let flush_interval = state.flush_interval;
        for (_, writer) in state.writers.iter_mut() {
            writer.update_config(flush_bytes, flush_interval);
        }
        while state.writers.len() > state.max_open_files {
            if let Some((_id, mut evicted)) = state.writers.pop_lru() {
                let _ = evicted.flush();
            } else {
                break;
            }
        }
    }

    pub fn memory_stats(&self) -> WsPayloadStoreMemoryStats {
        let state = self.state.lock();
        let mut out = WsPayloadStoreMemoryStats {
            writer_count: state.writers.len(),
            max_open_files: state.max_open_files,
            total_buffer_len: 0,
            total_buffer_capacity: 0,
            flush_bytes: state.flush_bytes,
            flush_interval_ms: state.flush_interval.as_millis() as u64,
            retention_days: state.retention_days,
        };

        for (_id, writer) in state.writers.iter() {
            out.total_buffer_len = out.total_buffer_len.saturating_add(writer.buffer.len());
            out.total_buffer_capacity = out
                .total_buffer_capacity
                .saturating_add(writer.buffer.capacity());
        }
        out
    }

    pub fn cleanup_expired(&self) -> std::io::Result<usize> {
        let payload_dir = self.payload_dir();
        if !payload_dir.exists() {
            return Ok(0);
        }
        let retention_duration = Duration::from_secs(self.get_retention_days() * 24 * 60 * 60);
        let now = SystemTime::now();
        let mut removed_count = 0;
        for entry in fs::read_dir(payload_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age > retention_duration && fs::remove_file(&path).is_ok() {
                                removed_count += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(removed_count)
    }

    pub fn clear(&self) -> std::io::Result<usize> {
        let payload_dir = self.payload_dir();
        if !payload_dir.exists() {
            return Ok(0);
        }
        let mut removed_count = 0;
        for entry in fs::read_dir(payload_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && fs::remove_file(&path).is_ok() {
                removed_count += 1;
            }
        }
        let mut state = self.state.lock();
        state.writers.clear();
        Ok(removed_count)
    }

    pub fn delete_by_ids(&self, ids: &[String]) -> std::io::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let payload_dir = self.payload_dir();
        if !payload_dir.exists() {
            return Ok(0);
        }

        let mut removed_count = 0;
        let mut state = self.state.lock();
        for id in ids {
            let safe_id = Self::safe_connection_id(id);
            if let Some(mut writer) = state.writers.pop(&safe_id) {
                let _ = writer.flush();
            }
            let path = self.connection_path(&safe_id);
            if path.is_file() && fs::remove_file(&path).is_ok() {
                removed_count += 1;
            }
        }
        tracing::debug!(
            count = removed_count,
            "[WS_PAYLOAD_STORE] Deleted payloads by ids"
        );
        Ok(removed_count)
    }

    pub fn stats(&self) -> WsPayloadStoreStats {
        let payload_dir = self.payload_dir();
        let mut file_count = 0;
        let mut total_size = 0u64;
        let state = self.state.lock();
        if payload_dir.exists() {
            if let Ok(entries) = fs::read_dir(&payload_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|e| e == "bin") {
                        file_count += 1;
                        if let Ok(metadata) = entry.metadata() {
                            total_size += metadata.len();
                        }
                    }
                }
            }
        }
        WsPayloadStoreStats {
            file_count,
            total_size,
            payload_dir: payload_dir.to_string_lossy().to_string(),
            retention_days: state.retention_days,
            active_writers: state.writers.len(),
            max_open_files: state.max_open_files,
        }
    }

    pub fn sizes_by_safe_id(&self) -> std::io::Result<std::collections::HashMap<String, u64>> {
        let mut sizes = std::collections::HashMap::new();
        let payload_dir = self.payload_dir();
        if !payload_dir.exists() {
            return Ok(sizes);
        }
        for entry in fs::read_dir(&payload_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                    *sizes.entry(file_stem.to_string()).or_insert(0) += size;
                }
            }
        }
        Ok(sizes)
    }

    pub fn active_connection_ids(&self) -> HashSet<String> {
        let state = self.state.lock();
        state.writers.iter().map(|(id, _)| id.clone()).collect()
    }

    pub fn recently_modified_ids(&self, max_age: Duration) -> HashSet<String> {
        let mut ids = HashSet::new();
        let payload_dir = self.payload_dir();
        if !payload_dir.exists() {
            return ids;
        }
        let now = SystemTime::now();
        let entries = match fs::read_dir(&payload_dir) {
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
                if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.insert(file_stem.to_string());
                }
            }
        }
        ids
    }

    fn get_retention_days(&self) -> u64 {
        self.state.lock().retention_days
    }
}

pub type SharedWsPayloadStore = std::sync::Arc<WsPayloadStore>;

pub fn start_ws_payload_cleanup_task(store: SharedWsPayloadStore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Ok(removed) = store.cleanup_expired() {
                if removed > 0 {
                    tracing::info!(
                        "[WS_PAYLOAD_STORE] Periodic cleanup removed {} expired files",
                        removed
                    );
                }
            }
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsPayloadStoreStats {
    pub file_count: usize,
    pub total_size: u64,
    pub payload_dir: String,
    pub retention_days: u64,
    pub active_writers: usize,
    pub max_open_files: usize,
}
