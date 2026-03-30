use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_storage::{MAX_TRAFFIC_MAX_RECORDS, MIN_TRAFFIC_MAX_RECORDS};
use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::broadcast;

use super::query::{Direction, QueryParams, QueryResult};
use super::schema::{
    get_insert_detail_sql, get_insert_sql, get_update_detail_sql, get_update_sql, init_database,
    InitError,
};
use super::types::{
    build_socket_status_summary, encode_flags, summarize_matched_rules, TrafficDbStats,
    TrafficSummaryCompact,
};
use crate::body_store::BodyRef;
use crate::traffic::TrafficRecord;

const DEFAULT_CACHE_SIZE: usize = 500;
const CLEANUP_CHECK_INTERVAL: u64 = 100;
const CLEANUP_LOW_WATERMARK_PERCENT: usize = 95;
const METRICS_CACHE_TTL: Duration = Duration::from_secs(5);

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
    orig_req_headers_blob: SerializedBlob,
    actual_res_headers_blob: SerializedBlob,
    req_script_results_blob: SerializedBlob,
    res_script_results_blob: SerializedBlob,
    decode_req_results_blob: SerializedBlob,
    decode_res_results_blob: SerializedBlob,
}

#[derive(Clone)]
struct CachedValue<T> {
    value: T,
    expires_at: Instant,
}

#[derive(Clone, Copy)]
enum QueryTotalMode {
    None,
    Estimated,
    Exact,
}

pub struct TrafficDbStore {
    db_path: PathBuf,
    write_conn: Mutex<Connection>,
    read_conn: Mutex<Connection>,
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
    host_metrics_cache: Mutex<Option<CachedValue<Vec<HostMetricsAggregate>>>>,
    app_metrics_cache: Mutex<Option<CachedValue<Vec<AppMetricsAggregate>>>>,
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
}

#[derive(Debug, Clone)]
pub struct HostMetricsAggregate {
    pub host: String,
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

#[derive(Debug, Clone)]
pub struct AppMetricsAggregate {
    pub app_name: String,
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

impl TrafficDbStore {
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

        let write_conn = match Self::open_or_reset_database(&db_path) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to open database");
                return Err(e);
            }
        };

        let read_conn = Connection::open(&db_path)?;
        read_conn.execute_batch(
            "PRAGMA query_only = true; PRAGMA cache_size = 5000; PRAGMA mmap_size = 134217728; PRAGMA foreign_keys = ON;",
        )?;

        let current_seq = Self::get_max_sequence(&write_conn).unwrap_or(0);
        let record_count = Self::get_record_count(&write_conn).unwrap_or(0);

        let (tx, _) = broadcast::channel(1024);

        let cache_size = std::num::NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap();

        tracing::info!(
            current_sequence = current_seq,
            "[TRAFFIC_DB] SQLite traffic store initialized"
        );

        Ok(Self {
            db_path,
            write_conn: Mutex::new(write_conn),
            read_conn: Mutex::new(read_conn),
            record_count: AtomicUsize::new(record_count),
            max_records: AtomicUsize::new(max_records),
            max_db_size_bytes: AtomicU64::new(max_db_size_bytes),
            retention_hours: AtomicU64::new(retention_hours.unwrap_or(168)),
            tx,
            current_sequence: AtomicU64::new(current_seq + 1),
            recent_cache: RwLock::new(LruCache::new(cache_size)),
            write_count: AtomicU64::new(0),
            cleanup_notifier: RwLock::new(None),
            host_metrics_cache: Mutex::new(None),
            app_metrics_cache: Mutex::new(None),
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

    fn invalidate_metrics_cache(&self) {
        *self.host_metrics_cache.lock() = None;
        *self.app_metrics_cache.lock() = None;
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

                let wal_path = db_path.with_extension("db-wal");
                let shm_path = db_path.with_extension("db-shm");
                if let Err(e) = fs::remove_file(db_path) {
                    tracing::warn!(error = %e, "[TRAFFIC_DB] Failed to remove old database file");
                }
                if wal_path.exists() {
                    fs::remove_file(&wal_path).ok();
                }
                if shm_path.exists() {
                    fs::remove_file(&shm_path).ok();
                }

                let mut new_conn = Connection::open(db_path)?;
                init_database(&mut new_conn).map_err(|e| match e {
                    InitError::Sqlite(e) => e,
                    InitError::VersionMismatch { .. } => rusqlite::Error::QueryReturnedNoRows,
                })?;
                tracing::info!("[TRAFFIC_DB] Database reset successfully");
                Ok(new_conn)
            }
            Err(InitError::Sqlite(e)) => Err(e),
        }
    }

    fn get_max_sequence(conn: &Connection) -> Option<u64> {
        conn.query_row("SELECT MAX(sequence) FROM traffic_records", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .ok()
        .flatten()
        .map(|v| v as u64)
    }

    fn get_record_count(conn: &Connection) -> Option<usize> {
        conn.query_row("SELECT COUNT(*) FROM traffic_records", [], |row| {
            row.get::<_, i64>(0)
        })
        .ok()
        .map(|v| v as usize)
    }

    fn serialize_detail_fields(record: &TrafficRecord) -> SerializedDetailFields {
        let timing_blob = record
            .timing
            .as_ref()
            .and_then(|t| bincode::serialize(t).ok());
        let req_headers_blob = record
            .request_headers
            .as_ref()
            .and_then(|h| bincode::serialize(h).ok());
        let res_headers_blob = record
            .response_headers
            .as_ref()
            .and_then(|h| bincode::serialize(h).ok());
        let rules_blob = record
            .matched_rules
            .as_ref()
            .and_then(|r| bincode::serialize(r).ok());
        let socket_status_blob = record
            .socket_status
            .as_ref()
            .and_then(|s| bincode::serialize(s).ok());
        let req_body_blob = record
            .request_body_ref
            .as_ref()
            .and_then(|b| bincode::serialize(b).ok());
        let res_body_blob = record
            .response_body_ref
            .as_ref()
            .and_then(|b| bincode::serialize(b).ok());
        let derived_res_body_blob = record
            .derived_response_body_ref
            .as_ref()
            .and_then(|b| bincode::serialize(b).ok());
        let raw_req_body_blob = record
            .raw_request_body_ref
            .as_ref()
            .and_then(|b| bincode::serialize(b).ok());
        let raw_res_body_blob = record
            .raw_response_body_ref
            .as_ref()
            .and_then(|b| bincode::serialize(b).ok());
        let orig_req_headers_blob = record
            .original_request_headers
            .as_ref()
            .and_then(|h| bincode::serialize(h).ok());
        let actual_res_headers_blob = record
            .actual_response_headers
            .as_ref()
            .and_then(|h| bincode::serialize(h).ok());
        let req_script_results_blob = record
            .req_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let res_script_results_blob = record
            .res_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let decode_req_results_blob = record
            .decode_req_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let decode_res_results_blob = record
            .decode_res_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());

        SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            orig_req_headers_blob,
            actual_res_headers_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        }
    }

    fn insert_record_tx(
        tx: &rusqlite::Transaction<'_>,
        seq: u64,
        record: &TrafficRecord,
    ) -> rusqlite::Result<()> {
        let flags = encode_flags(record);
        let socket_is_open = record.socket_status.as_ref().is_some_and(|s| s.is_open);
        let socket_send_count = record.socket_status.as_ref().map_or(0, |s| s.send_count);
        let socket_receive_count = record.socket_status.as_ref().map_or(0, |s| s.receive_count);
        let socket_send_bytes = record.socket_status.as_ref().map_or(0, |s| s.send_bytes);
        let socket_receive_bytes = record.socket_status.as_ref().map_or(0, |s| s.receive_bytes);
        let socket_frame_count = record
            .socket_status
            .as_ref()
            .map_or(record.frame_count, |s| s.frame_count);
        let (rule_count, rule_protocols) = summarize_matched_rules(record.matched_rules.as_deref());
        let rule_protocols_json = serde_json::to_string(&rule_protocols).ok();
        let SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            orig_req_headers_blob,
            actual_res_headers_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        } = Self::serialize_detail_fields(record);

        tx.execute(
            get_insert_sql(),
            params![
                seq as i64,
                &record.id,
                record.timestamp as i64,
                &record.host,
                &record.method,
                record.status as i32,
                &record.protocol,
                &record.url,
                &record.path,
                &record.content_type,
                &record.request_content_type,
                record.request_size as i64,
                record.response_size as i64,
                record.duration_ms as i64,
                &record.client_ip,
                &record.client_app,
                record.client_pid.map(|p| p as i32),
                &record.client_path,
                flags as i32,
                record.frame_count as i64,
                record.last_frame_id as i64,
                socket_is_open,
                socket_send_count as i64,
                socket_receive_count as i64,
                socket_send_bytes as i64,
                socket_receive_bytes as i64,
                socket_frame_count as i64,
                rule_count as i64,
                rule_protocols_json,
            ],
        )?;
        tx.execute(
            get_insert_detail_sql(),
            params![
                &record.id,
                timing_blob,
                req_headers_blob,
                res_headers_blob,
                rules_blob,
                req_body_blob,
                res_body_blob,
                derived_res_body_blob,
                raw_req_body_blob,
                raw_res_body_blob,
                &record.actual_url,
                &record.actual_host,
                orig_req_headers_blob,
                actual_res_headers_blob,
                socket_status_blob,
                req_script_results_blob,
                res_script_results_blob,
                decode_req_results_blob,
                decode_res_results_blob,
                &record.error_message,
            ],
        )?;
        Ok(())
    }

    pub fn record(&self, mut record: TrafficRecord) {
        let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);
        record.sequence = seq;

        if self.tx.receiver_count() > 0 {
            let _ = self.tx.send(TrafficStoreEvent::Inserted(record.clone()));
        }

        let mut conn = self.write_conn.lock();
        let result = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            Self::insert_record_tx(&tx, seq, &record)?;
            tx.commit()
        })();

        if let Err(e) = result {
            tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to insert record");
        } else if Self::should_keep_in_cache(&record) {
            self.increase_record_count(1);
            self.invalidate_metrics_cache();
            let mut cache = self.recent_cache.write();
            cache.put(
                record.id.clone(),
                TrafficSummaryCompact::from_record(&record),
            );
        } else {
            self.increase_record_count(1);
            self.invalidate_metrics_cache();
        }

        let count = self.write_count.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(CLEANUP_CHECK_INTERVAL) {
            self.maybe_cleanup(&conn);
        }
    }

    pub fn record_batch(&self, records: Vec<TrafficRecord>) {
        if records.is_empty() {
            return;
        }

        let mut records_with_seq = Vec::with_capacity(records.len());
        let should_broadcast = self.tx.receiver_count() > 0;
        for mut record in records {
            let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);
            record.sequence = seq;
            if should_broadcast {
                let _ = self.tx.send(TrafficStoreEvent::Inserted(record.clone()));
            }
            records_with_seq.push((seq, record));
        }

        let mut conn = self.write_conn.lock();
        let result = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            for (seq, record) in &records_with_seq {
                Self::insert_record_tx(&tx, *seq, record)?;
            }
            tx.commit()
        })();

        if let Err(e) = result {
            tracing::error!(error = %e, batch_size = records_with_seq.len(), "[TRAFFIC_DB] Failed to insert record batch");
            return;
        }

        let mut cache = self.recent_cache.write();
        for (_, record) in &records_with_seq {
            if Self::should_keep_in_cache(record) {
                cache.put(
                    record.id.clone(),
                    TrafficSummaryCompact::from_record(record),
                );
            }
        }
        drop(cache);

        self.increase_record_count(records_with_seq.len());
        self.invalidate_metrics_cache();

        let count = self
            .write_count
            .fetch_add(records_with_seq.len() as u64, Ordering::Relaxed);
        if count.is_multiple_of(CLEANUP_CHECK_INTERVAL) {
            self.maybe_cleanup(&conn);
        }
    }

    pub fn update_by_id<F>(&self, id: &str, updater: F) -> bool
    where
        F: FnOnce(&mut TrafficRecord),
    {
        // 注意：recent_cache 只保存 summary，不保存完整 TrafficRecord。
        // update 必须以 DB 为准，否则会出现“用精简结构覆盖写回导致字段丢失”的风险。
        if let Some(mut record) = self.get_by_id_from_db(id) {
            updater(&mut record);
            self.persist_update(&record);
            {
                let mut cache = self.recent_cache.write();
                if Self::should_keep_in_cache(&record) {
                    cache.put(
                        record.id.clone(),
                        TrafficSummaryCompact::from_record(&record),
                    );
                } else {
                    cache.pop(&record.id);
                }
            }
            if self.tx.receiver_count() > 0 {
                let _ = self.tx.send(TrafficStoreEvent::Updated(record));
            }
            return true;
        }

        false
    }

    fn should_keep_in_cache(record: &TrafficRecord) -> bool {
        if record.status == 0 {
            return true;
        }
        if record.is_websocket || record.is_sse || record.is_tunnel {
            return true;
        }
        record.socket_status.as_ref().is_some_and(|s| s.is_open)
    }

    fn persist_update(&self, record: &TrafficRecord) {
        let mut conn = self.write_conn.lock();
        let flags = encode_flags(record);
        let socket_is_open = record.socket_status.as_ref().is_some_and(|s| s.is_open);
        let socket_send_count = record.socket_status.as_ref().map_or(0, |s| s.send_count);
        let socket_receive_count = record.socket_status.as_ref().map_or(0, |s| s.receive_count);
        let socket_send_bytes = record.socket_status.as_ref().map_or(0, |s| s.send_bytes);
        let socket_receive_bytes = record.socket_status.as_ref().map_or(0, |s| s.receive_bytes);
        let socket_frame_count = record
            .socket_status
            .as_ref()
            .map_or(record.frame_count, |s| s.frame_count);
        let (rule_count, rule_protocols) = summarize_matched_rules(record.matched_rules.as_deref());
        let rule_protocols_json = serde_json::to_string(&rule_protocols).ok();
        let SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            orig_req_headers_blob,
            actual_res_headers_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        } = Self::serialize_detail_fields(record);

        let result = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute(
                get_update_sql(),
                params![
                    record.status as i32,
                    &record.content_type,
                    &record.request_content_type,
                    record.request_size as i64,
                    record.response_size as i64,
                    record.duration_ms as i64,
                    &record.client_app,
                    record.client_pid.map(|p| p as i32),
                    &record.client_path,
                    flags as i32,
                    record.frame_count as i64,
                    record.last_frame_id as i64,
                    socket_is_open,
                    socket_send_count as i64,
                    socket_receive_count as i64,
                    socket_send_bytes as i64,
                    socket_receive_bytes as i64,
                    socket_frame_count as i64,
                    rule_count as i64,
                    rule_protocols_json,
                    &record.id,
                ],
            )?;
            tx.execute(
                get_update_detail_sql(),
                params![
                    &record.id,
                    timing_blob,
                    req_headers_blob,
                    res_headers_blob,
                    rules_blob,
                    req_body_blob,
                    res_body_blob,
                    derived_res_body_blob,
                    raw_req_body_blob,
                    raw_res_body_blob,
                    &record.actual_url,
                    &record.actual_host,
                    orig_req_headers_blob,
                    actual_res_headers_blob,
                    socket_status_blob,
                    req_script_results_blob,
                    res_script_results_blob,
                    decode_req_results_blob,
                    decode_res_results_blob,
                    &record.error_message,
                ],
            )?;
            tx.commit()
        })();

        if let Err(e) = result {
            tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to update record");
        } else {
            self.invalidate_metrics_cache();
        }
    }

    pub fn query(&self, params: &QueryParams) -> QueryResult {
        #[cfg(test)]
        self.query_calls.fetch_add(1, Ordering::Relaxed);
        self.query_internal(params, QueryTotalMode::Estimated)
    }

    pub fn query_with_exact_total(&self, params: &QueryParams) -> QueryResult {
        #[cfg(test)]
        self.query_calls.fetch_add(1, Ordering::Relaxed);
        self.query_internal(params, QueryTotalMode::Exact)
    }

    pub fn query_latest_window(&self, limit: usize) -> QueryResult {
        let params = QueryParams {
            limit: Some(limit),
            direction: Direction::Backward,
            ..Default::default()
        };
        let mut result = self.query(&params);
        Self::normalize_latest_window_result(&mut result);
        result
    }

    /// 用于搜索等高频迭代场景的查询：不会计算 total（COUNT(*)），避免重复全表扫描。
    pub fn query_for_search(&self, params: &QueryParams) -> QueryResult {
        self.query_internal(params, QueryTotalMode::None)
    }

    fn query_internal(&self, params: &QueryParams, total_mode: QueryTotalMode) -> QueryResult {
        let conn = self.read_conn.lock();
        let (sql, values) = params.build_select_sql();
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to prepare query");
                return QueryResult {
                    records: vec![],
                    next_cursor: None,
                    prev_cursor: None,
                    has_more: false,
                    total: self.record_count.load(Ordering::Relaxed),
                    server_sequence: self.current_sequence.load(Ordering::Relaxed),
                };
            }
        };

        let records: Vec<TrafficSummaryCompact> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let socket_status = build_socket_status_summary(
                    row.get::<_, bool>(18)?,
                    row.get::<_, i64>(19)? as u64,
                    row.get::<_, i64>(20)? as u64,
                    row.get::<_, i64>(21)? as u64,
                    row.get::<_, i64>(22)? as u64,
                    row.get::<_, i64>(23)? as usize,
                );
                let rc = row.get::<_, i64>(24)? as usize;
                let rp = row
                    .get::<_, Option<String>>(25)?
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();

                Ok(TrafficSummaryCompact {
                    seq: row.get::<_, i64>(0)? as u64,
                    id: row.get(1)?,
                    ts: row.get::<_, i64>(2)? as u64,
                    h: row.get(3)?,
                    m: row.get(4)?,
                    s: row.get::<_, i32>(5)? as u16,
                    proto: row.get(6)?,
                    p: row.get(8)?,
                    ct: row.get(9)?,
                    req_ct: row.get(26)?,
                    req_sz: row.get::<_, i64>(10)? as usize,
                    res_sz: row.get::<_, i64>(11)? as usize,
                    dur: row.get::<_, i64>(12)? as u64,
                    cip: row.get(13)?,
                    capp: row.get(14)?,
                    cpid: row.get::<_, Option<i32>>(15)?.map(|v| v as u32),
                    flags: row.get::<_, i32>(16)? as u32,
                    fc: row.get::<_, i64>(17)? as usize,
                    ss: socket_status,
                    st: format_timestamp_ms(row.get::<_, i64>(2)? as u64),
                    et: {
                        let ts = row.get::<_, i64>(2)? as u64;
                        let dur = row.get::<_, i64>(12)? as u64;
                        if dur > 0 {
                            Some(format_timestamp_ms(ts + dur))
                        } else {
                            None
                        }
                    },
                    rc,
                    rp,
                })
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        let has_more = records.len() >= params.limit.unwrap_or(100);

        let (next_cursor, prev_cursor) = if records.is_empty() {
            (None, None)
        } else {
            match params.direction {
                Direction::Forward => (
                    records.last().map(|r| r.seq),
                    records.first().map(|r| r.seq),
                ),
                Direction::Backward => (
                    records.last().map(|r| r.seq),
                    records.first().map(|r| r.seq),
                ),
            }
        };

        let total = match total_mode {
            QueryTotalMode::None => 0,
            QueryTotalMode::Estimated => self.record_count.load(Ordering::Relaxed),
            QueryTotalMode::Exact => {
                if params.has_filters() {
                    self.count_with_conn(&conn, params)
                } else {
                    self.record_count.load(Ordering::Relaxed)
                }
            }
        };

        QueryResult {
            records,
            next_cursor,
            prev_cursor,
            has_more,
            total,
            server_sequence: self.current_sequence.load(Ordering::Relaxed),
        }
    }

    fn normalize_latest_window_result(result: &mut QueryResult) {
        if result.records.is_empty() {
            result.next_cursor = None;
            result.prev_cursor = None;
            return;
        }

        result.records.reverse();
        result.prev_cursor = result.records.first().map(|r| r.seq);
        result.next_cursor = result.records.last().map(|r| r.seq);
    }

    /// 批量拉取搜索所需的轻量字段，避免 search 中的 N+1 `get_by_id`。
    pub fn get_search_fields_by_ids(
        &self,
        ids: &[&str],
        need_url: bool,
        need_request_headers: bool,
        need_response_headers: bool,
        need_request_body_ref: bool,
        need_response_body_ref: bool,
    ) -> std::collections::HashMap<String, TrafficSearchFields> {
        use std::collections::HashMap;

        if ids.is_empty() {
            return HashMap::new();
        }

        // 至少要取 id。
        let mut columns: Vec<&str> = vec!["tr.id"];
        if need_url {
            columns.push("tr.url");
        }
        if need_request_headers {
            columns.push("td.request_headers_blob");
        }
        if need_response_headers {
            columns.push("td.response_headers_blob");
        }
        if need_request_body_ref {
            columns.push("td.request_body_ref_blob");
        }
        if need_response_body_ref {
            columns.push("td.response_body_ref_blob");
            columns.push("td.derived_response_body_ref_blob");
        }

        // 全部不需要也就不查。
        if columns.len() == 1 {
            return ids
                .iter()
                .map(|id| {
                    (
                        (*id).to_string(),
                        TrafficSearchFields {
                            id: (*id).to_string(),
                            url: None,
                            request_headers: None,
                            response_headers: None,
                            request_body_ref: None,
                            response_body_ref: None,
                            derived_response_body_ref: None,
                        },
                    )
                })
                .collect();
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT {} FROM traffic_records tr \
             LEFT JOIN traffic_record_details td ON td.id = tr.id \
             WHERE tr.id IN ({})",
            columns.join(","),
            placeholders.join(",")
        );

        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "[TRAFFIC_DB] Failed to prepare get_search_fields_by_ids");
                return HashMap::new();
            }
        };

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let mut out: HashMap<String, TrafficSearchFields> = HashMap::new();
        let iter = match stmt.query_map(params.as_slice(), |row| {
            let mut idx = 0usize;
            let id: String = row.get(idx)?;
            idx += 1;

            let url: Option<String> = if need_url {
                let v: String = row.get(idx)?;
                idx += 1;
                Some(v)
            } else {
                None
            };

            let request_headers: Option<Vec<(String, String)>> = if need_request_headers {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| bincode::deserialize(&b).ok())
            } else {
                None
            };

            let response_headers: Option<Vec<(String, String)>> = if need_response_headers {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| bincode::deserialize(&b).ok())
            } else {
                None
            };

            let request_body_ref: Option<BodyRef> = if need_request_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| bincode::deserialize(&b).ok())
            } else {
                None
            };

            let response_body_ref: Option<BodyRef> = if need_response_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| bincode::deserialize(&b).ok())
            } else {
                None
            };

            let derived_response_body_ref: Option<BodyRef> = if need_response_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                blob.and_then(|b| bincode::deserialize(&b).ok())
            } else {
                None
            };

            Ok(TrafficSearchFields {
                id: id.clone(),
                url,
                request_headers,
                response_headers,
                request_body_ref,
                response_body_ref,
                derived_response_body_ref,
            })
        }) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "[TRAFFIC_DB] get_search_fields_by_ids query failed");
                return HashMap::new();
            }
        };

        for row in iter.flatten() {
            out.insert(row.id.clone(), row);
        }

        out
    }

    fn count_with_conn(
        &self,
        conn: &parking_lot::MutexGuard<'_, Connection>,
        params: &QueryParams,
    ) -> usize {
        let (sql, values) = params.build_count_sql();
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        conn.query_row(&sql, param_refs.as_slice(), |row| row.get::<_, i64>(0))
            .map(|v| v as usize)
            .unwrap_or(0)
    }

    pub fn get_by_id(&self, id: &str) -> Option<TrafficRecord> {
        // 详情强制走 DB（避免把完整 record 常驻内存）
        self.get_by_id_from_db(id)
    }

    fn get_by_id_from_db(&self, id: &str) -> Option<TrafficRecord> {
        let conn = self.read_conn.lock();
        let mut record = conn
            .query_row(
                "SELECT sequence, id, timestamp, host, method, status, protocol, url, path, \
                 content_type, request_content_type, request_size, response_size, duration_ms, \
                 client_ip, client_app, client_pid, client_path, flags, frame_count, \
                 last_frame_id, socket_is_open, socket_send_count, socket_receive_count, \
                 socket_send_bytes, socket_receive_bytes, socket_frame_count \
                 FROM traffic_records WHERE id = ?",
                [id],
                Self::row_to_record_core,
            )
            .optional()
            .ok()
            .flatten()?;

        Self::load_record_details(&conn, id, &mut record);
        Some(record)
    }

    pub fn get_by_ids(&self, ids: &[&str]) -> Vec<TrafficSummaryCompact> {
        #[cfg(test)]
        self.get_by_ids_calls.fetch_add(1, Ordering::Relaxed);
        if ids.is_empty() {
            return vec![];
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT sequence, id, timestamp, host, method, status, protocol, \
             url, path, content_type, request_size, response_size, duration_ms, \
             client_ip, client_app, client_pid, flags, frame_count, \
             socket_is_open, socket_send_count, socket_receive_count, \
             socket_send_bytes, socket_receive_bytes, socket_frame_count, \
             rule_count, rule_protocols, request_content_type \
             FROM traffic_records WHERE id IN ({}) ORDER BY sequence DESC",
            placeholders.join(",")
        );

        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        stmt.query_map(params.as_slice(), |row| {
            let socket_status = build_socket_status_summary(
                row.get::<_, bool>(18)?,
                row.get::<_, i64>(19)? as u64,
                row.get::<_, i64>(20)? as u64,
                row.get::<_, i64>(21)? as u64,
                row.get::<_, i64>(22)? as u64,
                row.get::<_, i64>(23)? as usize,
            );
            let rc = row.get::<_, i64>(24)? as usize;
            let rp = row
                .get::<_, Option<String>>(25)?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();

            Ok(TrafficSummaryCompact {
                seq: row.get::<_, i64>(0)? as u64,
                id: row.get(1)?,
                ts: row.get::<_, i64>(2)? as u64,
                h: row.get(3)?,
                m: row.get(4)?,
                s: row.get::<_, i32>(5)? as u16,
                proto: row.get(6)?,
                p: row.get(8)?,
                ct: row.get(9)?,
                req_ct: row.get(26)?,
                req_sz: row.get::<_, i64>(10)? as usize,
                res_sz: row.get::<_, i64>(11)? as usize,
                dur: row.get::<_, i64>(12)? as u64,
                cip: row.get(13)?,
                capp: row.get(14)?,
                cpid: row.get::<_, Option<i32>>(15)?.map(|v| v as u32),
                flags: row.get::<_, i32>(16)? as u32,
                fc: row.get::<_, i64>(17)? as usize,
                ss: socket_status,
                st: format_timestamp_ms(row.get::<_, i64>(2)? as u64),
                et: {
                    let ts = row.get::<_, i64>(2)? as u64;
                    let dur = row.get::<_, i64>(12)? as u64;
                    if dur > 0 {
                        Some(format_timestamp_ms(ts + dur))
                    } else {
                        None
                    }
                },
                rc,
                rp,
            })
        })
        .map(|r| r.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn row_to_record_core(row: &rusqlite::Row) -> rusqlite::Result<TrafficRecord> {
        let flags: i32 = row.get("flags")?;

        Ok(TrafficRecord {
            sequence: row.get::<_, i64>("sequence")? as u64,
            id: row.get("id")?,
            timestamp: row.get::<_, i64>("timestamp")? as u64,
            host: row.get("host")?,
            method: row.get("method")?,
            status: row.get::<_, i32>("status")? as u16,
            protocol: row.get("protocol")?,
            url: row.get("url")?,
            path: row.get("path")?,
            content_type: row.get("content_type")?,
            request_content_type: row.get("request_content_type")?,
            request_size: row.get::<_, i64>("request_size")? as usize,
            response_size: row.get::<_, i64>("response_size")? as usize,
            duration_ms: row.get::<_, i64>("duration_ms")? as u64,
            client_ip: row.get("client_ip")?,
            client_app: row.get("client_app")?,
            client_pid: row.get::<_, Option<i32>>("client_pid")?.map(|v| v as u32),
            client_path: row.get("client_path")?,
            is_tunnel: flags & 1 != 0,
            is_websocket: flags & 2 != 0,
            is_sse: flags & 4 != 0,
            is_h3: flags & 8 != 0,
            has_rule_hit: flags & 16 != 0,
            is_replay: flags & 32 != 0,
            frame_count: row.get::<_, i64>("frame_count")? as usize,
            last_frame_id: row.get::<_, i64>("last_frame_id")? as u64,
            timing: None,
            request_headers: None,
            response_headers: None,
            matched_rules: None,
            socket_status: build_socket_status_summary(
                row.get::<_, bool>("socket_is_open")?,
                row.get::<_, i64>("socket_send_count")? as u64,
                row.get::<_, i64>("socket_receive_count")? as u64,
                row.get::<_, i64>("socket_send_bytes")? as u64,
                row.get::<_, i64>("socket_receive_bytes")? as u64,
                row.get::<_, i64>("socket_frame_count")? as usize,
            ),
            request_body_ref: None,
            response_body_ref: None,
            derived_response_body_ref: None,
            raw_request_body_ref: None,
            raw_response_body_ref: None,
            actual_url: None,
            actual_host: None,
            original_request_headers: None,
            actual_response_headers: None,
            error_message: None,
            req_script_results: None,
            res_script_results: None,
            decode_req_script_results: None,
            decode_res_script_results: None,
        })
    }

    fn load_record_details(conn: &Connection, id: &str, record: &mut TrafficRecord) {
        let _ = conn
            .query_row(
                "SELECT timing_blob, request_headers_blob, response_headers_blob, \
                 matched_rules_blob, request_body_ref_blob, response_body_ref_blob, \
                 derived_response_body_ref_blob, raw_request_body_ref_blob, raw_response_body_ref_blob, actual_url, actual_host, \
                 original_request_headers_blob, actual_response_headers_blob, \
                 socket_status_blob, req_script_results_blob, res_script_results_blob, \
                 decode_req_script_results_blob, decode_res_script_results_blob, error_message \
                 FROM traffic_record_details WHERE id = ?",
                [id],
                |row| {
                    let timing_blob: Option<Vec<u8>> = row.get(0)?;
                    let req_headers_blob: Option<Vec<u8>> = row.get(1)?;
                    let res_headers_blob: Option<Vec<u8>> = row.get(2)?;
                    let rules_blob: Option<Vec<u8>> = row.get(3)?;
                    let req_body_blob: Option<Vec<u8>> = row.get(4)?;
                    let res_body_blob: Option<Vec<u8>> = row.get(5)?;
                    let derived_res_body_blob: Option<Vec<u8>> = row.get(6)?;
                    let raw_req_body_blob: Option<Vec<u8>> = row.get(7)?;
                    let raw_res_body_blob: Option<Vec<u8>> = row.get(8)?;
                    let orig_req_headers_blob: Option<Vec<u8>> = row.get(11)?;
                    let actual_res_headers_blob: Option<Vec<u8>> = row.get(12)?;
                    let socket_status_blob: Option<Vec<u8>> = row.get(13)?;
                    let req_script_results_blob: Option<Vec<u8>> = row.get(14)?;
                    let res_script_results_blob: Option<Vec<u8>> = row.get(15)?;
                    let decode_req_results_blob: Option<Vec<u8>> = row.get(16)?;
                    let decode_res_results_blob: Option<Vec<u8>> = row.get(17)?;

                    record.timing = timing_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.request_headers =
                        req_headers_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.response_headers =
                        res_headers_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.matched_rules = rules_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.request_body_ref =
                        req_body_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.response_body_ref =
                        res_body_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.derived_response_body_ref =
                        derived_res_body_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.raw_request_body_ref =
                        raw_req_body_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.raw_response_body_ref =
                        raw_res_body_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.actual_url = row.get(9)?;
                    record.actual_host = row.get(10)?;
                    record.original_request_headers =
                        orig_req_headers_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.actual_response_headers =
                        actual_res_headers_blob.and_then(|b| bincode::deserialize(&b).ok());
                    record.socket_status = socket_status_blob
                        .and_then(|b| bincode::deserialize(&b).ok())
                        .or_else(|| record.socket_status.clone());
                    record.req_script_results =
                        req_script_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.res_script_results =
                        res_script_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.decode_req_script_results =
                        decode_req_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.decode_res_script_results =
                        decode_res_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.error_message = row.get(18)?;
                    Ok(())
                },
            )
            .optional();
    }

    pub fn clear(&self) {
        self.clear_with_active_ids(&[]);
    }

    pub fn clear_with_active_ids(&self, active_connection_ids: &[String]) {
        let conn = self.write_conn.lock();

        let active_ids_set: std::collections::HashSet<&str> =
            active_connection_ids.iter().map(|s| s.as_str()).collect();

        let pending_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM traffic_records WHERE status = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        tracing::info!(
            pending = pending_count,
            active_connections = active_connection_ids.len(),
            "[TRAFFIC_DB] Clearing traffic records, preserving active"
        );

        if active_connection_ids.is_empty() {
            if let Err(e) = conn.execute("DELETE FROM traffic_records", []) {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to clear traffic records");
            } else {
                self.record_count.store(0, Ordering::Relaxed);
                self.invalidate_metrics_cache();
            }
        } else {
            let placeholders: String = active_connection_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");

            let sql = format!(
                "DELETE FROM traffic_records WHERE id NOT IN ({})",
                placeholders
            );

            if let Err(e) = conn.execute(
                &sql,
                rusqlite::params_from_iter(active_connection_ids.iter()),
            ) {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to clear traffic records");
            } else {
                self.record_count
                    .store(active_connection_ids.len(), Ordering::Relaxed);
                self.invalidate_metrics_cache();
            }
        }

        let mut cache = self.recent_cache.write();
        let preserved_ids: Vec<String> = cache
            .iter()
            .filter(|(id, summary)| {
                active_ids_set.contains(id.as_str())
                    || (summary.is_websocket() && summary.ss.as_ref().is_some_and(|s| s.is_open))
            })
            .map(|(k, _)| k.clone())
            .collect();

        let mut new_cache = LruCache::new(
            std::num::NonZeroUsize::new(cache.cap().get())
                .unwrap_or(std::num::NonZeroUsize::new(1000).unwrap()),
        );
        for id in preserved_ids {
            if let Some(record) = cache.pop(&id) {
                new_cache.put(id, record);
            }
        }
        *cache = new_cache;

        if active_connection_ids.is_empty() {
            drop(conn);
            self.compact_db(true);
        } else {
            drop(conn);
            self.compact_db(false);
        }

        tracing::info!("[TRAFFIC_DB] Traffic records cleared (active preserved)");
    }

    fn compact_with_conn(conn: &Connection, full_vacuum: bool) {
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
            tracing::warn!(error = %e, "[TRAFFIC_DB] WAL checkpoint failed during compact");
        }
        if full_vacuum {
            if let Err(e) = conn.execute_batch("VACUUM") {
                tracing::warn!(error = %e, "[TRAFFIC_DB] VACUUM failed");
            } else {
                tracing::info!("[TRAFFIC_DB] VACUUM completed");
            }
        }
    }

    pub fn compact_db(&self, full_vacuum: bool) {
        let conn = self.write_conn.lock();
        Self::compact_with_conn(&conn, full_vacuum);
    }

    pub fn delete_by_ids(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }

        let conn = self.write_conn.lock();

        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM traffic_records WHERE id IN ({})", placeholders);

        match conn.execute(&sql, rusqlite::params_from_iter(ids.iter())) {
            Ok(count) => {
                self.decrease_record_count(count);
                self.invalidate_metrics_cache();
                tracing::info!(count = count, "[TRAFFIC_DB] Deleted traffic records by ids");
            }
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to delete records by ids");
            }
        }

        self.remove_from_cache(ids);
    }

    fn cleanup_low_watermark(max: usize) -> usize {
        let low = max.saturating_mul(CLEANUP_LOW_WATERMARK_PERCENT) / 100;
        low.max(1).min(max)
    }

    fn maybe_cleanup(&self, conn: &Connection) {
        let max = self.max_records.load(Ordering::Relaxed);
        let count = self.record_count.load(Ordering::Relaxed);

        if count > max {
            let target = Self::cleanup_low_watermark(max);
            let delete_count = count.saturating_sub(target);
            let deleted = self.delete_oldest_by_limit(conn, delete_count);
            if deleted > 0 {
                self.decrease_record_count(deleted);
                self.invalidate_metrics_cache();
                tracing::debug!(
                    deleted = deleted,
                    max = max,
                    target = target,
                    "[TRAFFIC_DB] Cleaned up old records"
                );
                Self::compact_with_conn(conn, false);
            }
        }

        let max_db_size_bytes = self.max_db_size_bytes.load(Ordering::Relaxed);
        if max_db_size_bytes > 0 {
            let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
            if db_size > max_db_size_bytes {
                let target_size = max_db_size_bytes.saturating_sub(max_db_size_bytes / 4);
                let avg_bytes_per_record = if count > 0 {
                    (db_size / count as u64).max(1)
                } else {
                    1
                };
                let bytes_to_remove = db_size.saturating_sub(target_size);
                let mut to_remove = bytes_to_remove.div_ceil(avg_bytes_per_record) as i64;
                if to_remove < 1 {
                    to_remove = 1;
                }
                let deleted = self.delete_oldest_by_limit(conn, to_remove as usize);
                if deleted > 0 {
                    self.decrease_record_count(deleted);
                    self.invalidate_metrics_cache();
                    tracing::info!(
                        deleted = deleted,
                        db_size = db_size,
                        max_db_size_bytes = max_db_size_bytes,
                        target_size = target_size,
                        "[TRAFFIC_DB] Cleaned up records due to DB size limit"
                    );
                    Self::compact_with_conn(conn, false);
                }
            }
        }
    }

    pub fn cleanup_expired_records(&self) -> usize {
        let retention_hours = self.retention_hours.load(Ordering::Relaxed);
        let retention_ms = retention_hours * 60 * 60 * 1000;
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let cutoff = now.saturating_sub(retention_ms);

        let conn = self.write_conn.lock();
        let deleted = self.delete_expired_by_cutoff(&conn, cutoff);
        if deleted > 0 {
            self.decrease_record_count(deleted);
            self.invalidate_metrics_cache();
            tracing::info!(
                deleted = deleted,
                retention_hours = retention_hours,
                "[TRAFFIC_DB] Cleaned up expired records"
            );
        }
        deleted
    }

    pub fn count(&self) -> usize {
        self.record_count.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> TrafficDbStats {
        let count = self.count();
        let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);

        let conn = self.read_conn.lock();
        let oldest: Option<u64> = conn
            .query_row("SELECT MIN(timestamp) FROM traffic_records", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .map(|v| v as u64);

        let newest: Option<u64> = conn
            .query_row("SELECT MAX(timestamp) FROM traffic_records", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .map(|v| v as u64);

        TrafficDbStats {
            record_count: count,
            db_size,
            db_path: self.db_path.display().to_string(),
            max_records: self.max_records.load(Ordering::Relaxed),
            retention_hours: self.retention_hours.load(Ordering::Relaxed),
            current_sequence: self.current_sequence.load(Ordering::Relaxed),
            oldest_timestamp: oldest,
            newest_timestamp: newest,
        }
    }

    pub fn aggregate_host_metrics(&self) -> Vec<HostMetricsAggregate> {
        if let Some(cached) = self.host_metrics_cache.lock().clone() {
            if cached.expires_at > Instant::now() {
                return cached.value;
            }
        }

        let conn = self.read_conn.lock();
        let sql = "SELECT COALESCE(NULLIF(host, ''), 'Unknown') AS host, \
                   COUNT(*) AS requests, \
                   COALESCE(SUM(request_size), 0) AS bytes_sent, \
                   COALESCE(SUM(response_size), 0) AS bytes_received, \
                   SUM(CASE WHEN protocol = 'http' THEN 1 ELSE 0 END) AS http_requests, \
                   SUM(CASE WHEN protocol = 'https' THEN 1 ELSE 0 END) AS https_requests, \
                   SUM(CASE WHEN protocol = 'tunnel' THEN 1 ELSE 0 END) AS tunnel_requests, \
                   SUM(CASE WHEN protocol = 'ws' THEN 1 ELSE 0 END) AS ws_requests, \
                   SUM(CASE WHEN protocol = 'wss' THEN 1 ELSE 0 END) AS wss_requests, \
                   SUM(CASE WHEN protocol = 'h3' THEN 1 ELSE 0 END) AS h3_requests, \
                   SUM(CASE WHEN protocol = 'socks5' THEN 1 ELSE 0 END) AS socks5_requests \
                   FROM traffic_records \
                   GROUP BY host \
                   ORDER BY requests DESC";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to prepare host metrics aggregate query");
                return vec![];
            }
        };

        let out: Vec<HostMetricsAggregate> = stmt
            .query_map([], |row| {
                Ok(HostMetricsAggregate {
                    host: row.get(0)?,
                    requests: row.get::<_, i64>(1)? as u64,
                    bytes_sent: row.get::<_, i64>(2)? as u64,
                    bytes_received: row.get::<_, i64>(3)? as u64,
                    http_requests: row.get::<_, i64>(4)? as u64,
                    https_requests: row.get::<_, i64>(5)? as u64,
                    tunnel_requests: row.get::<_, i64>(6)? as u64,
                    ws_requests: row.get::<_, i64>(7)? as u64,
                    wss_requests: row.get::<_, i64>(8)? as u64,
                    h3_requests: row.get::<_, i64>(9)? as u64,
                    socks5_requests: row.get::<_, i64>(10)? as u64,
                })
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        *self.host_metrics_cache.lock() = Some(CachedValue {
            value: out.clone(),
            expires_at: Instant::now() + METRICS_CACHE_TTL,
        });
        out
    }

    pub fn aggregate_app_metrics(&self) -> Vec<AppMetricsAggregate> {
        if let Some(cached) = self.app_metrics_cache.lock().clone() {
            if cached.expires_at > Instant::now() {
                return cached.value;
            }
        }

        let conn = self.read_conn.lock();
        let sql = "SELECT COALESCE(NULLIF(client_app, ''), 'Unknown') AS app_name, \
                   COUNT(*) AS requests, \
                   COALESCE(SUM(request_size), 0) AS bytes_sent, \
                   COALESCE(SUM(response_size), 0) AS bytes_received, \
                   SUM(CASE WHEN protocol = 'http' THEN 1 ELSE 0 END) AS http_requests, \
                   SUM(CASE WHEN protocol = 'https' THEN 1 ELSE 0 END) AS https_requests, \
                   SUM(CASE WHEN protocol = 'tunnel' THEN 1 ELSE 0 END) AS tunnel_requests, \
                   SUM(CASE WHEN protocol = 'ws' THEN 1 ELSE 0 END) AS ws_requests, \
                   SUM(CASE WHEN protocol = 'wss' THEN 1 ELSE 0 END) AS wss_requests, \
                   SUM(CASE WHEN protocol = 'h3' THEN 1 ELSE 0 END) AS h3_requests, \
                   SUM(CASE WHEN protocol = 'socks5' THEN 1 ELSE 0 END) AS socks5_requests \
                   FROM traffic_records \
                   GROUP BY app_name \
                   ORDER BY requests DESC";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to prepare app metrics aggregate query");
                return vec![];
            }
        };

        let out: Vec<AppMetricsAggregate> = stmt
            .query_map([], |row| {
                Ok(AppMetricsAggregate {
                    app_name: row.get(0)?,
                    requests: row.get::<_, i64>(1)? as u64,
                    bytes_sent: row.get::<_, i64>(2)? as u64,
                    bytes_received: row.get::<_, i64>(3)? as u64,
                    http_requests: row.get::<_, i64>(4)? as u64,
                    https_requests: row.get::<_, i64>(5)? as u64,
                    tunnel_requests: row.get::<_, i64>(6)? as u64,
                    ws_requests: row.get::<_, i64>(7)? as u64,
                    wss_requests: row.get::<_, i64>(8)? as u64,
                    h3_requests: row.get::<_, i64>(9)? as u64,
                    socks5_requests: row.get::<_, i64>(10)? as u64,
                })
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        *self.app_metrics_cache.lock() = Some(CachedValue {
            value: out.clone(),
            expires_at: Instant::now() + METRICS_CACHE_TTL,
        });
        out
    }

    pub fn current_sequence(&self) -> u64 {
        self.current_sequence.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TrafficStoreEvent> {
        self.tx.subscribe()
    }

    pub fn find_latest_client_path_by_app(&self, app_name: &str) -> Option<String> {
        let conn = self.read_conn.lock();
        conn.query_row(
            "SELECT client_path FROM traffic_records WHERE client_app = ?1 AND client_path IS NOT NULL ORDER BY sequence DESC LIMIT 1",
            [app_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
    }

    pub fn set_max_records(&self, max: usize) {
        let normalized = max.clamp(MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS);
        let old = self.max_records.swap(normalized, Ordering::SeqCst);
        if old != normalized {
            tracing::info!(
                old = old,
                new = normalized,
                "[TRAFFIC_DB] Max records updated"
            );
            let conn = self.write_conn.lock();
            self.maybe_cleanup(&conn);
        }
    }

    pub fn set_max_db_size_bytes(&self, max: u64) {
        let old = self.max_db_size_bytes.swap(max, Ordering::SeqCst);
        if old != max {
            tracing::info!(
                old = old,
                new = max,
                "[TRAFFIC_DB] Max db size bytes updated"
            );
            let conn = self.write_conn.lock();
            self.maybe_cleanup(&conn);
        }
    }

    pub fn max_db_size_bytes(&self) -> u64 {
        self.max_db_size_bytes.load(Ordering::Relaxed)
    }

    pub fn set_retention_hours(&self, hours: u64) {
        let old = self.retention_hours.swap(hours, Ordering::SeqCst);
        if old != hours {
            tracing::info!(
                old = old,
                new = hours,
                "[TRAFFIC_DB] Retention hours updated"
            );
        }
    }

    fn notify_cleanup(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Some(notifier) = self.cleanup_notifier.read().as_ref() {
            notifier(ids);
        }
    }

    fn remove_from_cache(&self, ids: &[String]) {
        let ids_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut cache = self.recent_cache.write();
        for id in &ids_set {
            cache.pop(&id.to_string());
        }
    }

    fn delete_by_ids_with_conn(&self, conn: &Connection, ids: &[String]) -> usize {
        if ids.is_empty() {
            return 0;
        }
        let mut deleted = 0usize;
        for chunk in ids.chunks(500) {
            let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM traffic_records WHERE id IN ({})", placeholders);
            if let Ok(count) = conn.execute(&sql, rusqlite::params_from_iter(chunk.iter())) {
                deleted += count;
            }
        }
        if deleted > 0 {
            self.invalidate_metrics_cache();
        }
        self.remove_from_cache(ids);
        deleted
    }

    fn delete_oldest_by_limit(&self, conn: &Connection, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        let mut remaining = limit;
        let mut deleted = 0usize;
        while remaining > 0 {
            let batch = remaining.min(500);
            let mut ids = Vec::new();
            let mut stmt = match conn
                .prepare("SELECT id FROM traffic_records ORDER BY sequence ASC LIMIT ?")
            {
                Ok(s) => s,
                Err(_) => break,
            };
            if let Ok(iter) = stmt.query_map([batch as i64], |row| row.get(0)) {
                for id in iter.flatten() {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                break;
            }
            deleted += self.delete_by_ids_with_conn(conn, &ids);
            self.notify_cleanup(&ids);
            if ids.len() >= remaining {
                break;
            }
            remaining = remaining.saturating_sub(ids.len());
        }
        deleted
    }

    fn delete_expired_by_cutoff(&self, conn: &Connection, cutoff: u64) -> usize {
        let mut deleted = 0usize;
        loop {
            let mut ids = Vec::new();
            let mut stmt = match conn.prepare(
                "SELECT id FROM traffic_records WHERE timestamp < ? ORDER BY sequence ASC LIMIT ?",
            ) {
                Ok(s) => s,
                Err(_) => break,
            };
            if let Ok(iter) = stmt.query_map([cutoff as i64, 500i64], |row| row.get(0)) {
                for id in iter.flatten() {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                break;
            }
            deleted += self.delete_by_ids_with_conn(conn, &ids);
            self.notify_cleanup(&ids);
        }
        deleted
    }

    pub fn all_ids_set(&self) -> std::collections::HashSet<String> {
        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare("SELECT id FROM traffic_records") {
            Ok(s) => s,
            Err(_) => return std::collections::HashSet::new(),
        };
        let iter = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(i) => i,
            Err(_) => return std::collections::HashSet::new(),
        };
        iter.flatten().collect()
    }

    pub fn oldest_ids(&self, limit: usize, offset: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let conn = self.read_conn.lock();
        let mut stmt = match conn
            .prepare("SELECT id FROM traffic_records ORDER BY sequence ASC LIMIT ? OFFSET ?")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let iter = match stmt.query_map([limit as i64, offset as i64], |row| row.get(0)) {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        iter.flatten().collect()
    }

    pub fn checkpoint(&self) {
        let conn = self.write_conn.lock();
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
            tracing::warn!(error = %e, "[TRAFFIC_DB] WAL checkpoint failed");
        }
    }

    #[cfg(test)]
    pub fn reset_debug_query_counters(&self) {
        self.query_calls.store(0, Ordering::Relaxed);
        self.get_by_ids_calls.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn debug_query_counters(&self) -> (usize, usize) {
        (
            self.query_calls.load(Ordering::Relaxed),
            self.get_by_ids_calls.load(Ordering::Relaxed),
        )
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
mod tests {
    use super::*;
    use crate::body_store::BodyRef;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!(
            "bifrost_traffic_db_test_{}_{}_{}",
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
    fn test_query_for_search_skips_total_count() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        store.record(TrafficRecord::new(
            "req-1".to_string(),
            "GET".to_string(),
            "https://a.com/p1".to_string(),
        ));
        store.record(TrafficRecord::new(
            "req-2".to_string(),
            "GET".to_string(),
            "https://a.com/p2".to_string(),
        ));
        store.record(TrafficRecord::new(
            "req-3".to_string(),
            "GET".to_string(),
            "https://a.com/p3".to_string(),
        ));

        let params = QueryParams {
            limit: Some(2),
            direction: Direction::Backward,
            ..Default::default()
        };

        let normal = store.query(&params);
        let fast = store.query_for_search(&params);

        assert_eq!(normal.records.len(), fast.records.len());
        assert_eq!(normal.has_more, fast.has_more);
        assert!(normal.total >= 3);
        assert_eq!(fast.total, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_query_latest_window_returns_latest_records_in_ascending_order() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        for i in 0..5 {
            store.record(TrafficRecord::new(
                format!("req-{}", i),
                "GET".to_string(),
                format!("https://a.com/{}", i),
            ));
        }

        let result = store.query_latest_window(2);
        let sequences: Vec<u64> = result.records.iter().map(|r| r.seq).collect();
        let ids: Vec<&str> = result.records.iter().map(|r| r.id.as_str()).collect();

        assert_eq!(sequences.len(), 2);
        assert!(sequences[0] < sequences[1]);
        assert_eq!(ids, vec!["req-3", "req-4"]);
        assert_eq!(result.prev_cursor, Some(sequences[0]));
        assert_eq!(result.next_cursor, Some(sequences[1]));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_get_search_fields_by_ids_respects_field_flags() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        let mut record = TrafficRecord::new(
            "req-1".to_string(),
            "POST".to_string(),
            "https://a.com/p1".to_string(),
        );
        record.request_headers = Some(vec![("X-Test".to_string(), "1".to_string())]);
        record.response_headers = Some(vec![("Y-Test".to_string(), "2".to_string())]);
        record.request_body_ref = Some(BodyRef::Inline {
            data: "hello".to_string(),
        });
        record.response_body_ref = Some(BodyRef::Inline {
            data: "world".to_string(),
        });
        store.record(record);

        let ids = ["req-1" as &str];

        let m = store.get_search_fields_by_ids(&ids, true, true, true, true, true);
        let f = m.get("req-1").expect("missing fields");
        assert!(f.url.as_deref().unwrap_or("").contains("https://a.com/p1"));
        assert!(f
            .request_headers
            .as_ref()
            .is_some_and(|h| h.iter().any(|(k, v)| k == "X-Test" && v == "1")));
        assert!(f
            .response_headers
            .as_ref()
            .is_some_and(|h| h.iter().any(|(k, v)| k == "Y-Test" && v == "2")));
        assert!(matches!(f.request_body_ref, Some(BodyRef::Inline { .. })));
        assert!(matches!(f.response_body_ref, Some(BodyRef::Inline { .. })));

        let m2 = store.get_search_fields_by_ids(&ids, false, false, false, false, false);
        let f2 = m2.get("req-1").expect("missing fields");
        assert!(f2.url.is_none());
        assert!(f2.request_headers.is_none());
        assert!(f2.response_headers.is_none());
        assert!(f2.request_body_ref.is_none());
        assert!(f2.response_body_ref.is_none());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_get_by_id_loads_detail_fields_from_split_table() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        let mut record = TrafficRecord::new(
            "detail-1".to_string(),
            "POST".to_string(),
            "https://a.com/p1".to_string(),
        );
        record.actual_url = Some("https://upstream.example.com/p1".to_string());
        record.actual_host = Some("upstream.example.com".to_string());
        record.request_headers = Some(vec![(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )]);
        record.response_headers = Some(vec![("X-Trace".to_string(), "trace-1".to_string())]);
        record.request_body_ref = Some(BodyRef::Inline {
            data: "{\"hello\":1}".to_string(),
        });
        record.response_body_ref = Some(BodyRef::Inline {
            data: "{\"ok\":true}".to_string(),
        });
        record.socket_status = Some(crate::traffic::SocketStatus {
            is_open: false,
            send_count: 3,
            receive_count: 5,
            send_bytes: 120,
            receive_bytes: 240,
            frame_count: 8,
            close_code: Some(1000),
            close_reason: Some("Normal closure".to_string()),
        });
        record.error_message = Some("upstream timeout".to_string());
        store.record(record);

        let loaded = store.get_by_id("detail-1").expect("record should exist");
        assert_eq!(loaded.actual_host.as_deref(), Some("upstream.example.com"));
        assert!(loaded.request_headers.as_ref().is_some_and(|h| h
            .iter()
            .any(|(k, v)| k == "Content-Type" && v == "application/json")));
        assert!(matches!(
            loaded.request_body_ref,
            Some(BodyRef::Inline { .. })
        ));
        assert_eq!(loaded.error_message.as_deref(), Some("upstream timeout"));
        assert_eq!(
            loaded.socket_status.as_ref().and_then(|s| s.close_code),
            Some(1000)
        );
        assert_eq!(
            loaded
                .socket_status
                .as_ref()
                .and_then(|s| s.close_reason.as_deref()),
            Some("Normal closure")
        );

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_get_by_ids_keeps_request_content_type_and_rule_summary() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        let mut record = TrafficRecord::new(
            "summary-1".to_string(),
            "POST".to_string(),
            "https://a.com/p1".to_string(),
        );
        record.request_content_type = Some("application/json".to_string());
        record.has_rule_hit = true;
        record.matched_rules = Some(vec![
            crate::traffic::MatchedRule {
                pattern: "/p1".to_string(),
                protocol: "reqHeaders".to_string(),
                value: "X-Test: 1".to_string(),
                rule_name: Some("rule-1".to_string()),
                raw: None,
                line: Some(1),
            },
            crate::traffic::MatchedRule {
                pattern: "/p1".to_string(),
                protocol: "resBody".to_string(),
                value: "ok".to_string(),
                rule_name: Some("rule-2".to_string()),
                raw: None,
                line: Some(2),
            },
        ]);
        store.record(record);

        let rows = store.get_by_ids(&["summary-1"]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].req_ct.as_deref(), Some("application/json"));
        assert_eq!(rows[0].rc, 2);
        assert!(rows[0].rp.iter().any(|p| p == "reqHeaders"));
        assert!(rows[0].rp.iter().any(|p| p == "resBody"));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_clear_removes_pending_records_when_no_active_connections() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        let record = TrafficRecord::new(
            "req-1".to_string(),
            "GET".to_string(),
            "https://a.com".to_string(),
        );
        store.record(record);
        assert_eq!(store.count(), 1);

        store.clear_with_active_ids(&[]);
        assert_eq!(store.count(), 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_cleanup_drops_to_low_watermark_instead_of_exact_limit() {
        let dir = create_test_dir();
        let max_records = MIN_TRAFFIC_MAX_RECORDS;
        let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

        for i in 0..=max_records {
            store.record(TrafficRecord::new(
                format!("req-{}", i),
                "GET".to_string(),
                format!("https://a.com/{}", i),
            ));
        }

        let expected_count = max_records * CLEANUP_LOW_WATERMARK_PERCENT / 100;
        assert_eq!(store.count(), expected_count);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_set_max_records_clamps_to_supported_range() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 10, 0, None).unwrap();

        store.set_max_records(10);
        assert_eq!(store.stats().max_records, MIN_TRAFFIC_MAX_RECORDS);

        store.set_max_records(MAX_TRAFFIC_MAX_RECORDS + 1);
        assert_eq!(store.stats().max_records, MAX_TRAFFIC_MAX_RECORDS);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_schema_does_not_keep_flags_index() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 5_000, 0, None).unwrap();

        let conn = store.read_conn.lock();
        let has_idx_flags: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_flags')",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(!has_idx_flags);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_clear_preserves_active_connection_records() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        let active = TrafficRecord::new(
            "active-1".to_string(),
            "GET".to_string(),
            "https://a.com".to_string(),
        );
        let inactive = TrafficRecord::new(
            "inactive-1".to_string(),
            "GET".to_string(),
            "https://b.com".to_string(),
        );
        store.record(active);
        store.record(inactive);
        assert_eq!(store.count(), 2);

        let active_ids = vec!["active-1".to_string()];
        store.clear_with_active_ids(&active_ids);
        assert_eq!(store.count(), 1);
        assert!(store.get_by_id("active-1").is_some());

        cleanup_test_dir(&dir);
    }
}
