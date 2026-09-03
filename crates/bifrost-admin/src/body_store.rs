use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::resource_alerts::{resource_alert_level, ResourceAlertLevel};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BodyRef {
    Inline {
        data: String,
    },
    File {
        path: String,
        size: usize,
    },
    FileRange {
        path: String,
        offset: u64,
        size: usize,
    },
}

impl BodyRef {
    /// Persist HTTP content-coding metadata beside the body file.
    ///
    /// `BodyRef` is a public API and a postcard-persisted database value, so
    /// changing its enum shape would break both existing clients and stored
    /// traffic. The sidecar keeps that contract stable while surviving a
    /// process restart.
    pub fn with_content_encoding(self, content_encoding: Option<&str>) -> std::io::Result<Self> {
        match content_encoding
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(content_encoding) => {
                if content_encoding.len() > 256 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "content-encoding metadata exceeds 256 bytes",
                    ));
                }
                let path = self.file_path().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "content-encoding metadata requires a file-backed body",
                    )
                })?;
                fs::write(content_encoding_marker_path(path), content_encoding)?;
                Ok(self)
            }
            None => Ok(self),
        }
    }

    pub fn content_encoding(&self) -> Option<String> {
        let path = self.file_path()?;
        fs::read_to_string(content_encoding_marker_path(path))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn storage_ref(&self) -> &BodyRef {
        self
    }

    pub fn size(&self) -> usize {
        match self {
            BodyRef::Inline { data } => data.len(),
            BodyRef::File { size, .. } => *size,
            BodyRef::FileRange { size, .. } => *size,
        }
    }

    pub fn is_file(&self) -> bool {
        match self {
            BodyRef::File { .. } | BodyRef::FileRange { .. } => true,
            BodyRef::Inline { .. } => false,
        }
    }

    fn file_path(&self) -> Option<&str> {
        match self {
            BodyRef::File { path, .. } | BodyRef::FileRange { path, .. } => Some(path),
            BodyRef::Inline { .. } => None,
        }
    }
}

fn content_encoding_marker_path(path: &str) -> PathBuf {
    PathBuf::from(format!("{path}.content-encoding"))
}

fn retention_modified_time(path: &std::path::Path, own_modified: SystemTime) -> SystemTime {
    let Some(path_text) = path.to_str() else {
        return own_modified;
    };
    let Some(body_path) = path_text.strip_suffix(".content-encoding") else {
        return own_modified;
    };
    fs::metadata(body_path)
        .and_then(|metadata| metadata.modified())
        .map(|body_modified| body_modified.max(own_modified))
        .unwrap_or(own_modified)
}

pub struct BodyStore {
    temp_dir: PathBuf,
    max_memory_size: usize,
    retention_days: u64,
    stream_flush_bytes: usize,
    stream_flush_interval: Duration,
    active_stream_writers: Arc<AtomicUsize>,
    active_stream_ids: Arc<RwLock<HashSet<String>>>,
    max_open_stream_writers: usize,
}

// 兜底：当文件写入失败时，最多只保留这么多字节的 inline 预览，避免把完整 body 复制到内存里。
const INLINE_FALLBACK_PREVIEW_BYTES: usize = 8 * 1024;
const DEFAULT_MAX_OPEN_STREAM_WRITERS: usize = 128;

pub struct BodyStreamWriter {
    path: PathBuf,
    file: fs::File,
    size: usize,
    buffer: Vec<u8>,
    flush_bytes: usize,
    flush_interval: Duration,
    last_flush: Instant,
    active_stream_writers: Arc<AtomicUsize>,
    active_stream_ids: Arc<RwLock<HashSet<String>>>,
    record_id: String,
    released_stream_slot: bool,
}

impl BodyStreamWriter {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn body_ref(&self) -> BodyRef {
        BodyRef::File {
            path: self.path.to_string_lossy().to_string(),
            size: self.size,
        }
    }

    pub fn write_chunk(&mut self, data: &[u8]) -> std::io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if !bifrost_core::payload_persistence_allowed() {
            self.size = self.size.saturating_sub(self.buffer.len());
            self.buffer.clear();
            return Ok(());
        }
        self.buffer.extend_from_slice(data);
        self.size += data.len();
        if self.buffer.len() >= self.flush_bytes || self.last_flush.elapsed() >= self.flush_interval
        {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush_interval(&self) -> Duration {
        self.flush_interval
    }

    pub fn flush_buffered(&mut self) -> std::io::Result<()> {
        self.flush()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if !bifrost_core::payload_persistence_allowed() {
            self.size = self.size.saturating_sub(self.buffer.len());
            self.buffer.clear();
            return Ok(());
        }
        self.file.write_all(&self.buffer)?;
        self.buffer.clear();
        self.last_flush = Instant::now();
        Ok(())
    }

    pub fn finish(mut self) -> BodyRef {
        let _ = self.flush();
        self.release_stream_slot();
        BodyRef::File {
            path: self.path.to_string_lossy().to_string(),
            size: self.size,
        }
    }

    fn release_stream_slot(&mut self) {
        if self.released_stream_slot {
            return;
        }
        self.released_stream_slot = true;
        self.active_stream_writers.fetch_sub(1, Ordering::SeqCst);
        self.active_stream_ids.write().remove(&self.record_id);
    }
}

impl Drop for BodyStreamWriter {
    fn drop(&mut self) {
        self.release_stream_slot();
    }
}

#[derive(Debug, Clone, Default)]
pub struct BodyStoreConfigUpdate {
    pub max_memory_size: Option<usize>,
    pub retention_days: Option<u64>,
    pub stream_flush_bytes: Option<usize>,
    pub stream_flush_interval_ms: Option<u64>,
}

impl BodyStore {
    pub fn new(
        temp_dir: PathBuf,
        max_memory_size: usize,
        retention_days: u64,
        stream_flush_bytes: usize,
        stream_flush_interval: Duration,
    ) -> Self {
        Self::new_with_limits(
            temp_dir,
            max_memory_size,
            retention_days,
            stream_flush_bytes,
            stream_flush_interval,
            DEFAULT_MAX_OPEN_STREAM_WRITERS,
        )
    }

    fn new_with_limits(
        temp_dir: PathBuf,
        max_memory_size: usize,
        retention_days: u64,
        stream_flush_bytes: usize,
        stream_flush_interval: Duration,
        max_open_stream_writers: usize,
    ) -> Self {
        if !temp_dir.exists() {
            let _ = fs::create_dir_all(&temp_dir);
        }
        // Captured response bodies are sensitive; restrict the directory to the
        // owner on Unix. Done once at construction to avoid per-write cost.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&temp_dir, fs::Permissions::from_mode(0o700)) {
                tracing::warn!(
                    "failed to set 0700 permissions on {}: {e}",
                    temp_dir.display()
                );
            }
        }
        Self {
            temp_dir,
            max_memory_size,
            retention_days,
            stream_flush_bytes,
            stream_flush_interval,
            active_stream_writers: Arc::new(AtomicUsize::new(0)),
            active_stream_ids: Arc::new(RwLock::new(HashSet::new())),
            max_open_stream_writers: max_open_stream_writers.max(1),
        }
    }

    pub fn update_config(&mut self, update: BodyStoreConfigUpdate) {
        if let Some(max_memory_size) = update.max_memory_size {
            tracing::info!(
                "BodyStore config updated: max_memory_size {} -> {}",
                self.max_memory_size,
                max_memory_size
            );
            self.max_memory_size = max_memory_size;
        }
        if let Some(retention_days) = update.retention_days {
            tracing::info!(
                "BodyStore config updated: retention_days {} -> {}",
                self.retention_days,
                retention_days
            );
            self.retention_days = retention_days;
        }
        if let Some(stream_flush_bytes) = update.stream_flush_bytes {
            tracing::info!(
                "BodyStore config updated: stream_flush_bytes {} -> {}",
                self.stream_flush_bytes,
                stream_flush_bytes
            );
            self.stream_flush_bytes = stream_flush_bytes;
        }
        if let Some(stream_flush_interval_ms) = update.stream_flush_interval_ms {
            tracing::info!(
                "BodyStore config updated: stream_flush_interval_ms {:?} -> {}",
                self.stream_flush_interval.as_millis(),
                stream_flush_interval_ms
            );
            self.stream_flush_interval = Duration::from_millis(stream_flush_interval_ms);
        }
    }

    pub fn store(&self, id: &str, kind: &str, data: &[u8]) -> Option<BodyRef> {
        if data.is_empty() || !bifrost_core::payload_persistence_allowed() {
            return None;
        }

        // 关键策略：默认不把 body 以 Inline 形式常驻在 TrafficRecord 里。
        // 即使 body 很小，也优先落盘，避免在内存中形成一份 UTF-8/losy 的拷贝导致内存膨胀。
        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);
        let _ = fs::remove_file(content_encoding_marker_path(&path.to_string_lossy()));

        match fs::File::create(&path) {
            Ok(mut file) => {
                if file.write_all(data).is_ok() {
                    Some(BodyRef::File {
                        path: path.to_string_lossy().to_string(),
                        size: data.len(),
                    })
                } else {
                    let _ = fs::remove_file(&path);
                    let preview = &data[..data.len().min(INLINE_FALLBACK_PREVIEW_BYTES)];
                    let text = String::from_utf8_lossy(preview).to_string();
                    Some(BodyRef::Inline { data: text })
                }
            }
            Err(_) => {
                let preview = &data[..data.len().min(INLINE_FALLBACK_PREVIEW_BYTES)];
                let text = String::from_utf8_lossy(preview).to_string();
                Some(BodyRef::Inline { data: text })
            }
        }
    }

    pub fn store_force_file(&self, id: &str, kind: &str, data: &[u8]) -> Option<BodyRef> {
        if data.is_empty() || !bifrost_core::payload_persistence_allowed() {
            return None;
        }

        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);
        let _ = fs::remove_file(content_encoding_marker_path(&path.to_string_lossy()));

        match fs::File::create(&path) {
            Ok(mut file) => {
                if file.write_all(data).is_ok() {
                    Some(BodyRef::File {
                        path: path.to_string_lossy().to_string(),
                        size: data.len(),
                    })
                } else {
                    let _ = fs::remove_file(&path);
                    let preview = &data[..data.len().min(INLINE_FALLBACK_PREVIEW_BYTES)];
                    let text = String::from_utf8_lossy(preview).to_string();
                    Some(BodyRef::Inline { data: text })
                }
            }
            Err(_) => {
                let preview = &data[..data.len().min(INLINE_FALLBACK_PREVIEW_BYTES)];
                let text = String::from_utf8_lossy(preview).to_string();
                Some(BodyRef::Inline { data: text })
            }
        }
    }

    pub fn start_stream(&self, id: &str, kind: &str) -> std::io::Result<BodyStreamWriter> {
        if !bifrost_core::payload_persistence_allowed() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "body persistence paused by resource pressure",
            ));
        }
        self.acquire_stream_slot()?;
        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);
        let _ = fs::remove_file(content_encoding_marker_path(&path.to_string_lossy()));
        let file = match fs::File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                self.active_stream_writers.fetch_sub(1, Ordering::SeqCst);
                return Err(error);
            }
        };
        let record_id = id.to_string();
        self.active_stream_ids.write().insert(record_id.clone());
        Ok(BodyStreamWriter {
            path,
            file,
            size: 0,
            buffer: Vec::with_capacity(self.stream_flush_bytes),
            flush_bytes: self.stream_flush_bytes,
            flush_interval: self.stream_flush_interval,
            last_flush: Instant::now(),
            active_stream_writers: Arc::clone(&self.active_stream_writers),
            active_stream_ids: Arc::clone(&self.active_stream_ids),
            record_id,
            released_stream_slot: false,
        })
    }

    fn acquire_stream_slot(&self) -> std::io::Result<()> {
        loop {
            let current = self.active_stream_writers.load(Ordering::SeqCst);
            if current >= self.max_open_stream_writers {
                tracing::warn!(
                    active_stream_writers = current,
                    max_open_stream_writers = self.max_open_stream_writers,
                    "[BODY_STORE] refusing to open new stream writer because active writer limit was reached"
                );
                return Err(std::io::Error::other(format!(
                    "body stream writer limit reached ({}/{})",
                    current, self.max_open_stream_writers
                )));
            }

            if self
                .active_stream_writers
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let next = current + 1;
                let level = resource_alert_level(next, self.max_open_stream_writers);
                if matches!(
                    level,
                    ResourceAlertLevel::Warn | ResourceAlertLevel::Critical
                ) {
                    tracing::warn!(
                        active_stream_writers = next,
                        max_open_stream_writers = self.max_open_stream_writers,
                        level = ?level,
                        "[BODY_STORE] stream writer usage is approaching the open-file limit"
                    );
                }
                return Ok(());
            }
        }
    }

    pub fn load(&self, body_ref: &BodyRef) -> Option<String> {
        match body_ref {
            BodyRef::Inline { data } => Some(data.clone()),
            BodyRef::File { path, size, .. } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                let mut contents = Vec::with_capacity(*size);
                file.read_to_end(&mut contents).ok()?;
                Some(String::from_utf8_lossy(&contents).to_string())
            }
            BodyRef::FileRange {
                path, offset, size, ..
            } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                file.seek(SeekFrom::Start(*offset)).ok()?;
                let mut contents = vec![0u8; *size];
                let mut read_size = 0usize;
                while read_size < *size {
                    let n = file.read(&mut contents[read_size..]).ok()?;
                    if n == 0 {
                        break;
                    }
                    read_size += n;
                }
                contents.truncate(read_size);
                Some(String::from_utf8_lossy(&contents).to_string())
            }
        }
    }

    pub fn load_bytes(&self, body_ref: &BodyRef) -> Option<Vec<u8>> {
        match body_ref {
            BodyRef::Inline { data } => Some(data.as_bytes().to_vec()),
            BodyRef::File { path, size, .. } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                let mut contents = Vec::with_capacity(*size);
                file.read_to_end(&mut contents).ok()?;
                Some(contents)
            }
            BodyRef::FileRange {
                path, offset, size, ..
            } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                file.seek(SeekFrom::Start(*offset)).ok()?;
                let mut contents = vec![0u8; *size];
                let mut read_size = 0usize;
                while read_size < *size {
                    let n = file.read(&mut contents[read_size..]).ok()?;
                    if n == 0 {
                        break;
                    }
                    read_size += n;
                }
                contents.truncate(read_size);
                Some(contents)
            }
        }
    }

    pub fn cleanup_expired(&self) -> std::io::Result<usize> {
        if !self.temp_dir.exists() {
            return Ok(0);
        }

        let retention_duration = Duration::from_secs(self.retention_days * 24 * 60 * 60);
        let now = SystemTime::now();
        let mut removed_count = 0;

        for entry in fs::read_dir(&self.temp_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let modified = retention_modified_time(&path, modified);
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
        if !self.temp_dir.exists() {
            return Ok(0);
        }

        let mut removed_count = 0;
        for entry in fs::read_dir(&self.temp_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && fs::remove_file(&path).is_ok() {
                removed_count += 1;
            }
        }
        Ok(removed_count)
    }

    pub fn delete_by_ids(&self, ids: &[String]) -> std::io::Result<usize> {
        if ids.is_empty() || !self.temp_dir.exists() {
            return Ok(0);
        }

        let ids_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut removed_count = 0;

        for entry in fs::read_dir(&self.temp_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let base_id = extract_base_id(file_name);
                    if ids_set.contains(base_id) && fs::remove_file(&path).is_ok() {
                        removed_count += 1;
                    }
                }
            }
        }

        tracing::debug!(count = removed_count, "[BODY_STORE] Deleted bodies by ids");
        Ok(removed_count)
    }

    pub fn remove(&self, body_ref: &BodyRef) {
        match body_ref {
            BodyRef::File { path, .. } | BodyRef::FileRange { path, .. } => {
                let _ = fs::remove_file(path);
                let _ = fs::remove_file(content_encoding_marker_path(path));
            }
            BodyRef::Inline { .. } => {}
        }
    }

    pub fn stats(&self) -> BodyStoreStats {
        let mut file_count = 0;
        let mut total_size = 0u64;

        if self.temp_dir.exists() {
            if let Ok(entries) = fs::read_dir(&self.temp_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if path.to_string_lossy().ends_with(".content-encoding") {
                            continue;
                        }
                        file_count += 1;
                        if let Ok(metadata) = entry.metadata() {
                            total_size += metadata.len();
                        }
                    }
                }
            }
        }

        BodyStoreStats {
            file_count,
            total_size,
            temp_dir: self.temp_dir.to_string_lossy().to_string(),
            max_memory_size: self.max_memory_size,
            retention_days: self.retention_days,
            active_stream_writers: self.active_stream_writers.load(Ordering::SeqCst),
            max_open_stream_writers: self.max_open_stream_writers,
        }
    }

    pub fn sizes_by_id(&self) -> std::io::Result<std::collections::HashMap<String, u64>> {
        let mut sizes = std::collections::HashMap::new();
        if !self.temp_dir.exists() {
            return Ok(sizes);
        }
        for entry in fs::read_dir(&self.temp_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let base_id = extract_base_id(file_name);
                    *sizes.entry(base_id.to_string()).or_insert(0) += size;
                }
            }
        }
        Ok(sizes)
    }

    pub fn active_stream_id_set(&self) -> HashSet<String> {
        self.active_stream_ids.read().clone()
    }

    pub fn recently_modified_ids(&self, max_age: Duration) -> HashSet<String> {
        let mut ids = HashSet::new();
        if !self.temp_dir.exists() {
            return ids;
        }
        let now = SystemTime::now();
        let entries = match fs::read_dir(&self.temp_dir) {
            Ok(e) => e,
            Err(_) => return ids,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let dominated_by_age = (|| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                let age = now.duration_since(modified).ok()?;
                Some(age <= max_age)
            })()
            .unwrap_or(false);
            if dominated_by_age {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    ids.insert(extract_base_id(file_name).to_string());
                }
            }
        }
        ids
    }
}

fn extract_base_id(file_name: &str) -> &str {
    let file_name = file_name
        .strip_suffix(".content-encoding")
        .unwrap_or(file_name);
    for suffix in [
        "_res_openai_like",
        "_sse_raw",
        "_req_raw",
        "_res_raw",
        "_req",
        "_res",
    ] {
        if let Some(id) = file_name.strip_suffix(suffix) {
            return id;
        }
    }
    file_name
        .rsplit_once('_')
        .map(|(id, _)| id)
        .unwrap_or(file_name)
}

pub type SharedBodyStore = Arc<RwLock<BodyStore>>;

pub fn start_body_cleanup_task(store: SharedBodyStore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Ok(removed) = store.read().cleanup_expired() {
                if removed > 0 {
                    tracing::info!(
                        "[BODY_STORE] Periodic cleanup removed {} expired files",
                        removed
                    );
                }
            }
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyStoreStats {
    pub file_count: usize,
    pub total_size: u64,
    pub temp_dir: String,
    pub max_memory_size: usize,
    pub retention_days: u64,
    pub active_stream_writers: usize,
    pub max_open_stream_writers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!(
            "bifrost_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
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

    #[test]
    fn test_store_inline_small_body() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1024, 7, 64 * 1024, Duration::from_millis(200));

        let data = b"Hello, World!";
        let body_ref = store.store("test1", "req", data).unwrap();

        // 新策略：即使 body 很小也优先落盘，避免 Inline 导致 TrafficRecord 常驻内存变大。
        assert!(matches!(body_ref, BodyRef::File { .. }));
        assert_eq!(store.load(&body_ref).unwrap(), "Hello, World!");

        cleanup_test_dir(&dir);
    }

    #[test]
    fn content_encoding_metadata_keeps_the_existing_public_variant() {
        let dir = create_test_dir();
        let path = dir.join("wire-body").to_string_lossy().to_string();
        let stored = BodyRef::File {
            path: path.clone(),
            size: 10,
        };
        let encoded = stored.clone().with_content_encoding(Some("gzip")).unwrap();

        assert!(matches!(
            encoded.storage_ref(),
            BodyRef::File { path: stored_path, .. } if stored_path == &path
        ));
        assert_eq!(encoded.content_encoding().as_deref(), Some("gzip"));
        let json = serde_json::to_value(&encoded).unwrap();
        assert!(json.get("File").is_some());
        assert!(json.get("ContentEncoded").is_none());

        let legacy: BodyRef = serde_json::from_value(serde_json::json!({
            "File": { "path": "legacy", "size": 3 }
        }))
        .unwrap();
        assert_eq!(legacy.content_encoding(), None);
        assert!(std::ptr::eq(stored.storage_ref(), &stored));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn content_encoding_metadata_rejects_unpersistable_sidecars() {
        let dir = create_test_dir();
        let path = dir.join("wire-body").to_string_lossy().to_string();
        let stored = BodyRef::File {
            path: path.clone(),
            size: 10,
        };
        let overlong = "gzip,".repeat(60);
        let error = stored
            .clone()
            .with_content_encoding(Some(&overlong))
            .expect_err("overlong metadata must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        fs::create_dir_all(content_encoding_marker_path(&path)).unwrap();
        let error = stored
            .with_content_encoding(Some("gzip"))
            .expect_err("sidecar write failure must be visible");
        assert_ne!(error.kind(), std::io::ErrorKind::InvalidInput);

        let inline = BodyRef::Inline {
            data: "wire".to_string(),
        };
        assert!(inline.with_content_encoding(Some("gzip")).is_err());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn content_encoding_sidecar_inherits_live_body_retention() {
        let dir = create_test_dir();
        let body_path = dir.join("active_sse_raw");
        fs::write(&body_path, b"live wire bytes").unwrap();
        let sidecar = content_encoding_marker_path(&body_path.to_string_lossy());
        fs::write(&sidecar, "gzip").unwrap();
        let stale_sidecar_time = SystemTime::UNIX_EPOCH;

        let effective = retention_modified_time(&sidecar, stale_sidecar_time);

        assert!(effective > stale_sidecar_time);
        cleanup_test_dir(&dir);
    }

    #[test]
    fn overwriting_or_removing_a_body_clears_its_encoding_sidecar() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1024, 7, 64 * 1024, Duration::from_millis(200));
        let encoded = store
            .store("sidecar", "res", b"wire")
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        assert_eq!(encoded.content_encoding().as_deref(), Some("gzip"));

        let replaced = store.store("sidecar", "res", b"plaintext").unwrap();
        assert_eq!(replaced.content_encoding(), None);
        let encoded = replaced.with_content_encoding(Some("br")).unwrap();
        store.remove(&encoded);
        assert_eq!(encoded.content_encoding(), None);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_store_file_large_body() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 10, 7, 64 * 1024, Duration::from_millis(200));

        let data = b"This is a large body that exceeds the memory limit";
        let body_ref = store.store("test2", "res", data).unwrap();

        assert!(matches!(body_ref, BodyRef::File { .. }));
        assert!(body_ref.is_file());
        assert_eq!(body_ref.size(), data.len());
        assert_eq!(
            store.load(&body_ref).unwrap(),
            "This is a large body that exceeds the memory limit"
        );

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_load_file_range() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 10, 7, 64 * 1024, Duration::from_millis(200));

        let data = b"Hello range body";
        let body_ref = store.store("test_range", "res", data).unwrap();
        let path = match body_ref {
            BodyRef::File { path, .. } => path,
            _ => {
                cleanup_test_dir(&dir);
                return;
            }
        };
        let range_ref = BodyRef::FileRange {
            path,
            offset: 6,
            size: 5,
        };
        assert_eq!(store.load(&range_ref).unwrap(), "range");

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_empty_body() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1024, 7, 64 * 1024, Duration::from_millis(200));

        let body_ref = store.store("test3", "req", b"");
        assert!(body_ref.is_none());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn critical_pressure_disables_all_body_persistence_paths() {
        const CHILD: &str = "BIFROST_BODY_PRESSURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "body_store::tests::critical_pressure_disables_all_body_persistence_paths",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 8, 7, 64 * 1024, Duration::from_secs(60));
        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Critical);
        assert!(store.store("critical", "req", b"body").is_none());
        assert!(store.store_force_file("critical", "res", b"body").is_none());
        match store.start_stream("critical", "stream") {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock),
            Ok(_) => panic!("critical pressure unexpectedly opened a body stream"),
        }

        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Normal);
        let mut writer = store.start_stream("active", "stream").unwrap();
        writer.write_chunk(b"buffered").unwrap();
        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Critical);
        writer.write_chunk(b"discarded").unwrap();
        assert_eq!(writer.body_ref().size(), 0);

        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Normal);
        writer.write_chunk(b"buffered").unwrap();
        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Critical);
        writer.flush_buffered().unwrap();
        assert_eq!(writer.body_ref().size(), 0);
        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Normal);
        drop(writer);
        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_delete_by_ids_with_hyphenated_id() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1, 7, 64 * 1024, Duration::from_millis(200));

        let id = "req-123-abc";
        let data = b"large body for file storage";
        let body_ref = store.store(id, "req", data).unwrap();
        assert!(body_ref.is_file());

        let before_stats = store.stats();
        assert_eq!(before_stats.file_count, 1);

        let removed = store.delete_by_ids(&[id.to_string()]).unwrap();
        assert_eq!(removed, 1);

        let after_stats = store.stats();
        assert_eq!(after_stats.file_count, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_cleanup() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 10, 0, 64 * 1024, Duration::from_millis(200));

        let data = b"Test data for cleanup";
        store.store("test4", "req", data);

        std::thread::sleep(std::time::Duration::from_millis(100));

        let removed = store.cleanup_expired().unwrap();
        assert!(removed >= 1);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_stream_writer_limit_released_after_finish() {
        let dir = create_test_dir();
        let store = BodyStore::new_with_limits(
            dir.clone(),
            10,
            7,
            64 * 1024,
            Duration::from_millis(200),
            1,
        );

        let writer = store.start_stream("test-stream", "res").unwrap();
        let stats = store.stats();
        assert_eq!(stats.active_stream_writers, 1);
        assert!(store.start_stream("test-stream-2", "res").is_err());

        let _ = writer.finish();

        let stats = store.stats();
        assert_eq!(stats.active_stream_writers, 0);
        assert!(store.start_stream("test-stream-3", "res").is_ok());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_extract_base_id() {
        assert_eq!(
            extract_base_id("REQ-69c50db8-165713_req"),
            "REQ-69c50db8-165713"
        );
        assert_eq!(
            extract_base_id("REQ-69c50db8-165713_res"),
            "REQ-69c50db8-165713"
        );
        assert_eq!(
            extract_base_id("REQ-69c50db8-165720_sse_raw"),
            "REQ-69c50db8-165720"
        );
        assert_eq!(
            extract_base_id("REQ-69c50db8-165720_req_raw"),
            "REQ-69c50db8-165720"
        );
        assert_eq!(
            extract_base_id("REQ-69c50db8-165720_res_raw"),
            "REQ-69c50db8-165720"
        );
        assert_eq!(
            extract_base_id("OUT-REQ-69c50db8-165720_req_raw"),
            "OUT-REQ-69c50db8-165720"
        );
        assert_eq!(
            extract_base_id("OUT-REQ-69c50db8-165720_res_raw"),
            "OUT-REQ-69c50db8-165720"
        );
        assert_eq!(
            extract_base_id("REQ-69c62cd8-072562_res_openai_like"),
            "REQ-69c62cd8-072562"
        );
        assert_eq!(
            extract_base_id("REQ-abcdef01-000001_req"),
            "REQ-abcdef01-000001"
        );
        assert_eq!(extract_base_id("some_unknown_file"), "some_unknown");
        assert_eq!(
            extract_base_id("REQ-69c50db8-165720_sse_raw.content-encoding"),
            "REQ-69c50db8-165720"
        );
    }

    #[test]
    fn test_delete_by_ids_removes_content_encoding_sidecar() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1, 7, 64 * 1024, Duration::from_millis(200));
        let id = "REQ-encoded-sse";
        let body_ref = store
            .store(id, "sse_raw", b"compressed SSE payload")
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let body_path = body_ref.file_path().unwrap().to_string();
        let sidecar = content_encoding_marker_path(&body_path);
        assert!(sidecar.exists());

        let removed = store.delete_by_ids(&[id.to_string()]).unwrap();

        assert_eq!(removed, 2);
        assert!(!sidecar.exists());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_delete_by_ids_with_multi_segment_suffixes() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1, 7, 64 * 1024, Duration::from_millis(200));

        let id = "OUT-REQ-69c50db8-165720";
        store.store(id, "req", b"request body").unwrap();
        store.store(id, "res", b"response body").unwrap();
        store.store(id, "sse_raw", b"sse raw data").unwrap();
        store.store(id, "req_raw", b"raw request").unwrap();
        store.store(id, "res_raw", b"raw response").unwrap();
        store
            .store(id, "res_openai_like", b"openai like data")
            .unwrap();

        let stats = store.stats();
        assert_eq!(stats.file_count, 6);

        let removed = store.delete_by_ids(&[id.to_string()]).unwrap();
        assert_eq!(removed, 6);

        let stats = store.stats();
        assert_eq!(stats.file_count, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_sizes_by_id_with_multi_segment_suffixes() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1, 7, 64 * 1024, Duration::from_millis(200));

        let id = "REQ-69c50db8-165720";
        store.store(id, "req", b"12345").unwrap();
        store.store(id, "sse_raw", b"1234567890").unwrap();

        let sizes = store.sizes_by_id().unwrap();
        assert_eq!(sizes.len(), 1);
        assert_eq!(*sizes.get(id).unwrap(), 15);

        cleanup_test_dir(&dir);
    }
}
