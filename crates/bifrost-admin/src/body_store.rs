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
    pub fn size(&self) -> usize {
        match self {
            BodyRef::Inline { data } => data.len(),
            BodyRef::File { size, .. } => *size,
            BodyRef::FileRange { size, .. } => *size,
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, BodyRef::File { .. } | BodyRef::FileRange { .. })
    }
}

pub struct BodyStore {
    temp_dir: PathBuf,
    max_memory_size: usize,
    retention_days: u64,
    stream_flush_bytes: usize,
    stream_flush_interval: Duration,
    active_stream_writers: Arc<AtomicUsize>,
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
        Self {
            temp_dir,
            max_memory_size,
            retention_days,
            stream_flush_bytes,
            stream_flush_interval,
            active_stream_writers: Arc::new(AtomicUsize::new(0)),
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
        if data.is_empty() {
            return None;
        }

        // 关键策略：默认不把 body 以 Inline 形式常驻在 TrafficRecord 里。
        // 即使 body 很小，也优先落盘，避免在内存中形成一份 UTF-8/losy 的拷贝导致内存膨胀。
        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);

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
        if data.is_empty() {
            return None;
        }

        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);

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
        self.acquire_stream_slot()?;
        let filename = format!("{}_{}", id, kind);
        let path = self.temp_dir.join(&filename);
        let file = match fs::File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                self.active_stream_writers.fetch_sub(1, Ordering::SeqCst);
                return Err(error);
            }
        };
        Ok(BodyStreamWriter {
            path,
            file,
            size: 0,
            buffer: Vec::with_capacity(self.stream_flush_bytes),
            flush_bytes: self.stream_flush_bytes,
            flush_interval: self.stream_flush_interval,
            last_flush: Instant::now(),
            active_stream_writers: Arc::clone(&self.active_stream_writers),
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
            BodyRef::File { path, .. } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                let mut contents = Vec::new();
                file.read_to_end(&mut contents).ok()?;
                Some(String::from_utf8_lossy(&contents).to_string())
            }
            BodyRef::FileRange { path, offset, size } => {
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
            BodyRef::File { path, .. } => {
                let path = PathBuf::from(path);
                if !path.exists() {
                    return None;
                }
                let mut file = fs::File::open(&path).ok()?;
                let mut contents = Vec::new();
                file.read_to_end(&mut contents).ok()?;
                Some(contents)
            }
            BodyRef::FileRange { path, offset, size } => {
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
}

fn extract_base_id(file_name: &str) -> &str {
    if let Some(prefix_end) = file_name.find('-') {
        if let Some(second_dash) = file_name[prefix_end + 1..].find('-') {
            let digits_start = prefix_end + 1 + second_dash + 1;
            let digits_end = file_name[digits_start..]
                .find(|c: char| !c.is_ascii_digit())
                .map(|pos| digits_start + pos)
                .unwrap_or(file_name.len());
            if digits_end < file_name.len() && file_name.as_bytes()[digits_end] == b'_' {
                return &file_name[..digits_end];
            }
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
            extract_base_id("REQ-69c62cd8-072562_res_openai_like"),
            "REQ-69c62cd8-072562"
        );
        assert_eq!(
            extract_base_id("REQ-abcdef01-000001_req"),
            "REQ-abcdef01-000001"
        );
        assert_eq!(extract_base_id("some_unknown_file"), "some_unknown");
    }

    #[test]
    fn test_delete_by_ids_with_multi_segment_suffixes() {
        let dir = create_test_dir();
        let store = BodyStore::new(dir.clone(), 1, 7, 64 * 1024, Duration::from_millis(200));

        let id = "REQ-69c50db8-165720";
        store.store(id, "req", b"request body").unwrap();
        store.store(id, "res", b"response body").unwrap();
        store.store(id, "sse_raw", b"sse raw data").unwrap();
        store
            .store(id, "res_openai_like", b"openai like data")
            .unwrap();

        let stats = store.stats();
        assert_eq!(stats.file_count, 4);

        let removed = store.delete_by_ids(&[id.to_string()]).unwrap();
        assert_eq!(removed, 4);

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
