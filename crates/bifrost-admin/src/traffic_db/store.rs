use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use bifrost_storage::{MAX_TRAFFIC_MAX_RECORDS, MIN_TRAFFIC_MAX_RECORDS};
use lru::LruCache;
use parking_lot::{Mutex, MutexGuard, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::broadcast;

use super::query::{Direction, QueryParams, QueryResult};
use super::schema::{
    get_insert_detail_sql, get_insert_sql, get_replace_sql, get_update_detail_sql, get_update_sql,
    init_database, InitError,
};
use super::statistics::{
    AppMetricsAggregate, HostMetricsAggregate, TrafficStatistics, TrafficStatisticsDimensions,
    TrafficStatisticsSnapshot,
};
use super::types::{
    build_socket_status_summary, encode_flags, summarize_matched_rules, TrafficDbStats,
    TrafficSummaryCompact,
};
use crate::body_store::BodyRef;
use crate::traffic::TrafficRecord;

const DEFAULT_CACHE_SIZE: usize = 500;
const CLEANUP_CHECK_INTERVAL: u64 = 100;
const CLEANUP_TRIGGER_OVERFLOW_PERCENT: usize = 15;
const CLEANUP_TRIGGER_OVERFLOW_MAX: usize = 2000;
const CLEANUP_TARGET_PERCENT: usize = 80;
const READ_POOL_SIZE: usize = 2;

fn encode_detail_blob<T: serde::Serialize>(value: &T) -> postcard::Result<Vec<u8>> {
    postcard::to_allocvec(value)
}

fn decode_detail_blob<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> postcard::Result<T> {
    postcard::from_bytes(bytes)
}

fn trusted_upload_bytes(record: &TrafficRecord) -> usize {
    if record.upload_bytes > 0 {
        return record.upload_bytes;
    }
    if let Some(status) = record.socket_status.as_ref() {
        if status.send_bytes > 0 {
            return status.send_bytes as usize;
        }
    }
    record.request_size
}

fn trusted_download_bytes(record: &TrafficRecord) -> usize {
    if record.download_bytes > 0 {
        return record.download_bytes;
    }
    if let Some(status) = record.socket_status.as_ref() {
        if status.receive_bytes > 0 {
            return status.receive_bytes as usize;
        }
    }
    record.response_size
}

struct ReadPool {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl ReadPool {
    fn new(db_path: &std::path::Path, size: usize) -> Result<Self, rusqlite::Error> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open(db_path)?;
            conn.execute_batch(
                "PRAGMA query_only = true; \
                 PRAGMA cache_size = 1000; \
                 PRAGMA mmap_size = 134217728; \
                 PRAGMA foreign_keys = ON;",
            )?;
            conns.push(Mutex::new(conn));
        }
        Ok(Self {
            conns,
            next: AtomicUsize::new(0),
        })
    }

    fn acquire(&self) -> MutexGuard<'_, Connection> {
        let start = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        for i in 0..self.conns.len() {
            let idx = (start + i) % self.conns.len();
            if let Some(guard) = self.conns[idx].try_lock() {
                return guard;
            }
        }
        self.conns[start].lock()
    }
}

pub type SharedTrafficDbStore = Arc<TrafficDbStore>;
type CleanupNotifier = Arc<dyn Fn(&[String]) + Send + Sync>;

#[derive(Debug, Clone)]
pub enum TrafficStoreEvent {
    Inserted(TrafficRecord),
    Updated(TrafficRecord),
}

type SerializedBlob = Option<Vec<u8>>;

struct SerializedDetailFields {
    timing_blob: SerializedBlob,
    req_headers_blob: SerializedBlob,
    res_headers_blob: SerializedBlob,
    rules_blob: SerializedBlob,
    socket_status_blob: SerializedBlob,
    req_body_blob: SerializedBlob,
    res_body_blob: SerializedBlob,
    derived_res_body_blob: SerializedBlob,
    raw_req_body_blob: SerializedBlob,
    raw_res_body_blob: SerializedBlob,
    body_metadata_blob: SerializedBlob,
    orig_req_headers_blob: SerializedBlob,
    res_headers_modified_blob: SerializedBlob,
    req_script_results_blob: SerializedBlob,
    res_script_results_blob: SerializedBlob,
    decode_req_results_blob: SerializedBlob,
    decode_res_results_blob: SerializedBlob,
}

#[derive(Clone, Copy)]
enum QueryTotalMode {
    None,
    Estimated,
    Exact,
}

pub struct TrafficDbStore {
    db_path: PathBuf,
    database_epoch: String,
    write_conn: Mutex<Connection>,
    read_pool: ReadPool,
    record_count: AtomicUsize,
    max_records: AtomicUsize,
    max_db_size_bytes: AtomicU64,
    retention_hours: AtomicU64,
    tx: broadcast::Sender<TrafficStoreEvent>,
    current_sequence: AtomicU64,
    // 仅用于“活跃/未完成连接”的轻量缓存：支持 traffic 列表/推送所需字段。
    // 详细信息（headers/body/script results 等）一律从 DB 按需读取，避免常驻内存膨胀。
    recent_cache: RwLock<LruCache<String, TrafficSummaryCompact>>,
    write_count: AtomicU64,
    cleanup_notifier: RwLock<Option<CleanupNotifier>>,
    traffic_statistics: RwLock<TrafficStatistics>,
    #[cfg(test)]
    clear_before_cache: RwLock<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    clear_before_compact: RwLock<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    query_calls: AtomicUsize,
    #[cfg(test)]
    get_by_ids_calls: AtomicUsize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecentCacheStats {
    pub len: usize,
    pub cap: usize,
}

#[derive(Debug, Clone)]
pub struct TrafficSearchFields {
    pub id: String,
    pub url: Option<String>,
    pub request_headers: Option<Vec<(String, String)>>,
    pub response_headers: Option<Vec<(String, String)>>,
    pub request_body_ref: Option<BodyRef>,
    pub response_body_ref: Option<BodyRef>,
    pub derived_response_body_ref: Option<BodyRef>,
    pub request_body_content_encoding: Option<String>,
    pub response_body_content_encoding: Option<String>,
}

mod maintenance;
mod reads;
mod writes;

impl TrafficDbStore {
    #[cfg(test)]
    pub(crate) fn set_write_query_only(&self, enabled: bool) -> rusqlite::Result<()> {
        self.write_conn
            .lock()
            .pragma_update(None, "query_only", if enabled { "ON" } else { "OFF" })
    }

    #[inline]
    pub fn has_traffic_event_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }

    pub fn new(
        db_dir: PathBuf,
        max_records: usize,
        max_db_size_bytes: u64,
        retention_hours: Option<u64>,
    ) -> Result<Self, rusqlite::Error> {
        let max_records = max_records.clamp(MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS);
        if !db_dir.exists() {
            fs::create_dir_all(&db_dir).ok();
        }

        let db_path = db_dir.join("traffic.db");

        tracing::info!(
            db_path = %db_path.display(),
            max_records = max_records,
            max_db_size_bytes = max_db_size_bytes,
            retention_hours = retention_hours.unwrap_or(168),
            "[TRAFFIC_DB] Initializing SQLite traffic store"
        );

        let mut write_conn = match Self::open_or_reset_database(&db_path) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to open database");
                return Err(e);
            }
        };

        // Keep additive metadata initialization outside the destructive schema
        // recovery path: a metadata write failure must preserve existing traffic.
        let (database_epoch, current_seq) = Self::initialize_sequence_metadata(&mut write_conn)?;

        let read_pool = ReadPool::new(&db_path, READ_POOL_SIZE)?;
        let traffic_statistics = TrafficStatistics::load(&write_conn);
        let record_count = traffic_statistics.total_requests();

        let (tx, _) = broadcast::channel(1024);

        let cache_size = std::num::NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap();

        tracing::info!(
            current_sequence = current_seq,
            "[TRAFFIC_DB] SQLite traffic store initialized"
        );

        Ok(Self {
            db_path,
            database_epoch,
            write_conn: Mutex::new(write_conn),
            read_pool,
            record_count: AtomicUsize::new(record_count),
            max_records: AtomicUsize::new(max_records),
            max_db_size_bytes: AtomicU64::new(max_db_size_bytes),
            retention_hours: AtomicU64::new(retention_hours.unwrap_or(168)),
            tx,
            current_sequence: AtomicU64::new(current_seq + 1),
            recent_cache: RwLock::new(LruCache::new(cache_size)),
            write_count: AtomicU64::new(0),
            cleanup_notifier: RwLock::new(None),
            traffic_statistics: RwLock::new(traffic_statistics),
            #[cfg(test)]
            clear_before_cache: RwLock::new(None),
            #[cfg(test)]
            clear_before_compact: RwLock::new(None),
            #[cfg(test)]
            query_calls: AtomicUsize::new(0),
            #[cfg(test)]
            get_by_ids_calls: AtomicUsize::new(0),
        })
    }

    pub fn set_cleanup_notifier(&self, notifier: CleanupNotifier) {
        *self.cleanup_notifier.write() = Some(notifier);
    }

    pub fn recent_cache_stats(&self) -> RecentCacheStats {
        let cache = self.recent_cache.read();
        RecentCacheStats {
            len: cache.len(),
            cap: cache.cap().get(),
        }
    }

    fn increase_record_count(&self, count: usize) {
        if count > 0 {
            self.record_count.fetch_add(count, Ordering::Relaxed);
        }
    }

    fn decrease_record_count(&self, count: usize) {
        if count == 0 {
            return;
        }
        let _ = self
            .record_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(count))
            });
    }

    pub fn traffic_statistics(&self) -> TrafficStatisticsSnapshot {
        let mut snapshot = self
            .traffic_statistics
            .read()
            .snapshot(self.current_sequence.load(Ordering::Relaxed));
        snapshot.database_epoch = Some(self.database_epoch.clone());
        snapshot
    }

    pub fn database_epoch(&self) -> &str {
        &self.database_epoch
    }

    pub fn aggregate_host_metrics(&self) -> Vec<HostMetricsAggregate> {
        self.traffic_statistics.read().host_metrics()
    }

    pub fn aggregate_app_metrics(&self) -> Vec<AppMetricsAggregate> {
        self.traffic_statistics.read().app_metrics()
    }

    fn open_or_reset_database(db_path: &PathBuf) -> Result<Connection, rusqlite::Error> {
        let mut conn = Connection::open(db_path)?;

        match init_database(&mut conn) {
            Ok(()) => Ok(conn),
            Err(InitError::VersionMismatch { current, expected }) => {
                tracing::warn!(
                    current_version = current,
                    expected_version = expected,
                    "[TRAFFIC_DB] Schema version mismatch, resetting database"
                );
                drop(conn);
                Self::reset_database(db_path)
            }
            Err(InitError::Sqlite(e)) => {
                tracing::warn!(
                    error = %e,
                    "[TRAFFIC_DB] Schema initialization failed, resetting database"
                );
                drop(conn);
                Self::reset_database(db_path)
            }
        }
    }

    fn reset_database(db_path: &PathBuf) -> Result<Connection, rusqlite::Error> {
        let wal_path = db_path.with_extension("db-wal");
        let shm_path = db_path.with_extension("db-shm");
        for path in [db_path, &wal_path, &shm_path] {
            if let Err(e) = fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        error = %e,
                        "[TRAFFIC_DB] Failed to remove old database file"
                    );
                }
            }
        }

        let mut new_conn = Connection::open(db_path)?;
        init_database(&mut new_conn).map_err(|_| rusqlite::Error::QueryReturnedNoRows)?;
        tracing::info!("[TRAFFIC_DB] Database reset successfully");
        Ok(new_conn)
    }

    fn initialize_sequence_metadata(conn: &mut Connection) -> rusqlite::Result<(String, u64)> {
        // Concurrent openers must observe the identity that won the insert.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO metadata (key, value) VALUES ('database_epoch', ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        let epoch: String = tx.query_row(
            "SELECT value FROM metadata WHERE key = 'database_epoch'",
            [],
            |row| row.get(0),
        )?;
        if epoch.is_empty() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let high_water: Option<String> = tx
            .query_row(
                "SELECT value FROM metadata WHERE key = 'sequence_high_water'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let high_water = high_water
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?
            .unwrap_or(0);
        let max_sequence: u64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM traffic_records",
            [],
            |row| row.get(0),
        )?;
        let current_sequence = high_water.max(max_sequence);
        if current_sequence >= i64::MAX as u64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        tx.commit()?;
        Ok((epoch, current_sequence))
    }

    fn preserve_sequence_high_water(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Retained rows already persist their sequences. Only removal needs an
        // extra write, committed first so even a crash cannot reuse deleted IDs.
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('sequence_high_water', ?1)
             ON CONFLICT(key) DO UPDATE SET value =
             MAX(CAST(metadata.value AS INTEGER), CAST(excluded.value AS INTEGER))",
            [self
                .current_sequence
                .load(Ordering::SeqCst)
                .saturating_sub(1)
                .to_string()],
        )?;
        Ok(())
    }
}

fn format_timestamp_ms(timestamp_ms: u64) -> String {
    use chrono::{Local, TimeZone};
    let secs = (timestamp_ms / 1000) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    Local
        .timestamp_opt(secs, nanos)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn start_db_cleanup_task(store: SharedTrafficDbStore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let deleted = store.cleanup_expired_records();
            if deleted > 0 {
                store.compact_db(false);
            }
        }
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "clear_concurrency_tests.rs"]
mod clear_concurrency_tests;
