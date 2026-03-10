use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use super::query::{Direction, QueryParams, QueryResult};
use super::schema::{get_insert_sql, get_update_sql, init_database, InitError};
use super::types::{encode_flags, TrafficDbStats, TrafficSummaryCompact};
use crate::body_store::BodyRef;
use crate::traffic::{SocketStatus, TrafficRecord};

const DEFAULT_CACHE_SIZE: usize = 500;
const CLEANUP_CHECK_INTERVAL: u64 = 100;

// Body index v1 parameters (no compression)
const BODY_INDEX_ALGO_VERSION: i32 = 1;
const BODY_INDEX_BLOCK_SIZE: usize = 64 * 1024;
const BODY_INDEX_BITSET_BITS: usize = 32 * 1024;
const BODY_INDEX_BITSET_BYTES: usize = BODY_INDEX_BITSET_BITS / 8;
const BODY_INDEX_MIN_BODY_SIZE: usize = 64 * 1024;
const BODY_INDEX_QUEUE_CAPACITY: usize = 128;
const BODY_INDEX_DEDUPE_CACHE_SIZE: usize = 2048;

// Body index scheduler (debounce + budget)
// 目的：不影响主链路代理性能。
// - debounce: body 仍在增长时不重复构建；只在稳定一段时间后构建一次
// - budget: 后台每秒最多处理一定字节量，避免吃满 CPU
const BODY_INDEX_IDLE_DEBOUNCE_MS: u64 = 3_000;
const BODY_INDEX_SCHED_TICK_MS: u64 = 200;
const BODY_INDEX_RETRY_BACKOFF_MS: u64 = 500;
const BODY_INDEX_BUDGET_BYTES_PER_SEC: usize = 2 * 1024 * 1024;
const BODY_INDEX_MAX_JOBS_PER_TICK: usize = 2;

pub type SharedTrafficDbStore = Arc<TrafficDbStore>;
type CleanupNotifier = Arc<dyn Fn(&[String]) + Send + Sync>;

pub struct TrafficDbStore {
    db_path: PathBuf,
    write_conn: Mutex<Connection>,
    read_conn: Mutex<Connection>,
    max_records: AtomicUsize,
    max_db_size_bytes: AtomicU64,
    retention_hours: AtomicU64,
    tx: broadcast::Sender<TrafficRecord>,
    current_sequence: AtomicU64,
    recent_cache: RwLock<LruCache<String, TrafficRecord>>,
    write_count: AtomicU64,
    cleanup_notifier: RwLock<Option<CleanupNotifier>>,

    // Best-effort background body indexer (must not block main write path)
    body_index_enabled: AtomicBool,
    body_index_tx: Option<mpsc::Sender<BodyIndexJob>>,
    body_index_rx: Mutex<Option<mpsc::Receiver<BodyIndexJob>>>,
    body_index_dedupe: RwLock<LruCache<String, BodyIndexDedupeKey>>,

    // Debounced candidates; keyed by "{id}:{kind}".
    // IMPORTANT: must be cheap to update (main write path).
    body_index_pending: Mutex<HashMap<String, PendingBodyIndexJob>>,
    body_index_scheduler_started: AtomicBool,

    // Scheduler/worker lifetime management (important for tests / drop safety)
    body_index_scheduler_cancel: Arc<AtomicBool>,
    body_index_scheduler_handle: Mutex<Option<JoinHandle<()>>>,
    body_index_worker_handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone)]
struct BodyIndexDedupeKey {
    fingerprint: u64,
    size: usize,
}

#[derive(Debug, Clone)]
struct BodyIndexJob {
    id: String,
    kind: i32, // 0=req, 1=res
    approx_size: usize,
}

#[derive(Debug, Clone)]
struct PendingBodyIndexJob {
    id: String,
    kind: i32,
    last_change_ms: u64,
    next_attempt_ms: u64,
}

#[derive(Debug, Clone)]
pub struct BodyIndexRow {
    pub id: String,
    pub kind: i32,
    pub path: String,
    pub offset: u64,
    pub size: usize,
    pub block_count: usize,
    pub bitsets: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TrafficSearchFields {
    pub id: String,
    pub url: Option<String>,
    pub request_headers: Option<Vec<(String, String)>>,
    pub response_headers: Option<Vec<(String, String)>>,
    pub request_body_ref: Option<BodyRef>,
    pub response_body_ref: Option<BodyRef>,
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
    pub fn new(
        db_dir: PathBuf,
        max_records: usize,
        max_db_size_bytes: u64,
        retention_hours: Option<u64>,
    ) -> Result<Self, rusqlite::Error> {
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
            "PRAGMA query_only = true; PRAGMA cache_size = 5000; PRAGMA mmap_size = 134217728;",
        )?;

        let current_seq = Self::get_max_sequence(&write_conn).unwrap_or(0);

        let (tx, _) = broadcast::channel(1024);

        let cache_size = std::num::NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap();

        let (body_index_tx, body_index_rx) =
            mpsc::channel::<BodyIndexJob>(BODY_INDEX_QUEUE_CAPACITY);
        // Lazy-start index worker on first enqueue to avoid any startup impact.

        let dedupe_cap = std::num::NonZeroUsize::new(BODY_INDEX_DEDUPE_CACHE_SIZE).unwrap();

        tracing::info!(
            current_sequence = current_seq,
            "[TRAFFIC_DB] SQLite traffic store initialized"
        );

        Ok(Self {
            db_path,
            write_conn: Mutex::new(write_conn),
            read_conn: Mutex::new(read_conn),
            max_records: AtomicUsize::new(max_records),
            max_db_size_bytes: AtomicU64::new(max_db_size_bytes),
            retention_hours: AtomicU64::new(retention_hours.unwrap_or(168)),
            tx,
            current_sequence: AtomicU64::new(current_seq + 1),
            recent_cache: RwLock::new(LruCache::new(cache_size)),
            write_count: AtomicU64::new(0),
            cleanup_notifier: RwLock::new(None),

            body_index_tx: Some(body_index_tx),
            body_index_rx: Mutex::new(Some(body_index_rx)),
            body_index_dedupe: RwLock::new(LruCache::new(dedupe_cap)),
            // 默认关闭：body_index 计算对长连接/流式 body 场景会显著增加 CPU。
            // 如需开启，请通过 performance config 显式打开。
            body_index_enabled: AtomicBool::new(false),

            body_index_pending: Mutex::new(HashMap::new()),
            body_index_scheduler_started: AtomicBool::new(false),

            body_index_scheduler_cancel: Arc::new(AtomicBool::new(false)),
            body_index_scheduler_handle: Mutex::new(None),
            body_index_worker_handle: Mutex::new(None),
        })
    }

    pub fn set_body_index_enabled(&self, enabled: bool) {
        let old = self.body_index_enabled.swap(enabled, Ordering::SeqCst);
        if old != enabled {
            tracing::info!(enabled = enabled, "[BODY_INDEX] Body index feature toggled");
        }

        if !enabled {
            // 关闭时清理候选任务，避免之后重新开启时立刻集中跑一波。
            self.body_index_pending.lock().clear();
        }
    }

    pub fn is_body_index_enabled(&self) -> bool {
        self.body_index_enabled.load(Ordering::Relaxed)
    }

    fn pending_body_index_len(&self) -> usize {
        self.body_index_pending.lock().len()
    }

    fn hash_body_ref_fingerprint(path: &str, offset: u64, size: usize) -> u64 {
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        offset.hash(&mut h);
        size.hash(&mut h);
        h.finish()
    }

    fn get_body_index_seen(&self, key: &str) -> Option<BodyIndexDedupeKey> {
        self.body_index_dedupe.read().peek(key).cloned()
    }

    fn ensure_body_index_scheduler_started(&self) {
        if self
            .body_index_scheduler_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        let tx = self.body_index_tx.clone();
        if tx.is_none() {
            return;
        }

        // NOTE: 这里使用指针地址（usize）绕过 `Send` 约束；scheduler 线程只做后台调度。
        // `TrafficDbStore` 在进程生命周期内通常由 `Arc` 持有直到退出，因此该引用是稳定的。
        let this = self as *const TrafficDbStore as usize;
        let cancel = self.body_index_scheduler_cancel.clone();
        let handle = std::thread::spawn(move || {
            // Token bucket for budget
            let mut tokens: usize = BODY_INDEX_BUDGET_BYTES_PER_SEC;
            let mut last_refill = Instant::now();

            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                // refill
                let elapsed_ms = last_refill.elapsed().as_millis() as u64;
                if elapsed_ms > 0 {
                    let add = (BODY_INDEX_BUDGET_BYTES_PER_SEC as u64)
                        .saturating_mul(elapsed_ms)
                        .saturating_div(1000) as usize;
                    tokens = tokens
                        .saturating_add(add)
                        .min(BODY_INDEX_BUDGET_BYTES_PER_SEC);
                    last_refill = Instant::now();
                }

                let (enabled, jobs) = unsafe {
                    let store = &*(this as *const TrafficDbStore);
                    if !store.is_body_index_enabled() {
                        (false, Vec::new())
                    } else {
                        (
                            true,
                            store.drain_ready_body_index_jobs(TrafficDbStore::now_ms()),
                        )
                    }
                };

                if enabled && !jobs.is_empty() {
                    let pending_left =
                        unsafe { (&*(this as *const TrafficDbStore)).pending_body_index_len() };
                    tracing::debug!(
                        ready = jobs.len(),
                        pending_left = pending_left,
                        tokens = tokens,
                        "[BODY_INDEX] Scheduler tick"
                    );
                }

                if enabled {
                    if let Some(ref tx) = tx {
                        let mut sent = 0usize;
                        for job in jobs {
                            if sent >= BODY_INDEX_MAX_JOBS_PER_TICK {
                                unsafe {
                                    (&*(this as *const TrafficDbStore))
                                        .requeue_body_index_job(job, BODY_INDEX_RETRY_BACKOFF_MS);
                                }
                                continue;
                            }

                            // Budget: approximate cost by body size
                            if tokens < job.approx_size {
                                unsafe {
                                    (&*(this as *const TrafficDbStore))
                                        .requeue_body_index_job(job, BODY_INDEX_RETRY_BACKOFF_MS);
                                }
                                continue;
                            }

                            // Ensure worker started (lazy)
                            unsafe {
                                (&*(this as *const TrafficDbStore))
                                    .ensure_body_index_worker_started()
                            };

                            match tx.try_send(job.clone()) {
                                Ok(()) => {
                                    tokens = tokens.saturating_sub(job.approx_size);
                                    sent += 1;
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => unsafe {
                                    (&*(this as *const TrafficDbStore))
                                        .requeue_body_index_job(job, BODY_INDEX_RETRY_BACKOFF_MS)
                                },
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // worker closed; drop tasks
                                    break;
                                }
                            }
                        }
                    }
                }

                // idle sleep
                if !enabled {
                    std::thread::sleep(Duration::from_millis(500));
                } else {
                    std::thread::sleep(Duration::from_millis(BODY_INDEX_SCHED_TICK_MS));
                }
            }

            tracing::info!("[BODY_INDEX] Scheduler stopped");
        });

        *self.body_index_scheduler_handle.lock() = Some(handle);

        tracing::info!("[BODY_INDEX] Scheduler started");
    }

    fn drain_ready_body_index_jobs(&self, now: u64) -> Vec<BodyIndexJob> {
        let mut out = Vec::new();
        let mut pending = self.body_index_pending.lock();

        // Collect keys first to avoid holding borrow across removals.
        let mut ready_keys: Vec<String> = Vec::new();
        for (k, v) in pending.iter() {
            if v.next_attempt_ms > now {
                continue;
            }
            if now.saturating_sub(v.last_change_ms) < BODY_INDEX_IDLE_DEBOUNCE_MS {
                continue;
            }
            ready_keys.push(k.clone());
        }

        for k in ready_keys {
            if let Some(v) = pending.remove(&k) {
                let approx_size = self
                    .get_body_index_seen(&k)
                    .map(|s| s.size)
                    .unwrap_or(BODY_INDEX_MIN_BODY_SIZE);
                out.push(BodyIndexJob {
                    id: v.id,
                    kind: v.kind,
                    approx_size,
                });
            }
        }
        out
    }

    fn requeue_body_index_job(&self, job: BodyIndexJob, backoff_ms: u64) {
        let now = Self::now_ms();
        let key = format!("{}:{}", job.id, job.kind);
        let mut pending = self.body_index_pending.lock();
        pending.insert(
            key.clone(),
            PendingBodyIndexJob {
                id: job.id,
                kind: job.kind,
                // 已经稳定过一次了，重试不需要重新 debounce
                last_change_ms: now.saturating_sub(BODY_INDEX_IDLE_DEBOUNCE_MS + 1),
                next_attempt_ms: now.saturating_add(backoff_ms),
            },
        );

        tracing::trace!(
            id = %key,
            approx_size = job.approx_size,
            backoff_ms = backoff_ms,
            "[BODY_INDEX] Requeued job"
        );
    }

    fn ensure_body_index_worker_started(&self) {
        let rx = self.body_index_rx.lock().take();
        let Some(rx) = rx else {
            return;
        };
        let handle = Self::start_body_index_worker(self.db_path.clone(), rx);
        *self.body_index_worker_handle.lock() = Some(handle);
    }

    fn start_body_index_worker(
        db_path: PathBuf,
        mut rx: mpsc::Receiver<BodyIndexJob>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let conn = match Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, db_path = %db_path.display(), "[BODY_INDEX] Failed to open index connection");
                    return;
                }
            };

            if let Err(e) = conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=2000;",
            ) {
                tracing::warn!(error = %e, "[BODY_INDEX] Failed to set PRAGMA");
            }

            tracing::info!("[BODY_INDEX] Worker started");

            fn fetch_body_ref_info(
                conn: &Connection,
                id: &str,
                kind: i32,
            ) -> Option<(String, u64, usize)> {
                let (req_blob, res_blob): (Option<Vec<u8>>, Option<Vec<u8>>) = conn
                    .query_row(
                        "SELECT request_body_ref_blob, response_body_ref_blob FROM traffic_records WHERE id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .ok()?;

                let blob = if kind == 0 { req_blob } else { res_blob }?;
                let body_ref: BodyRef = bincode::deserialize(&blob).ok()?;
                match body_ref {
                    BodyRef::File { path, size } => Some((path, 0u64, size)),
                    BodyRef::FileRange { path, offset, size } => Some((path, offset, size)),
                    BodyRef::Inline { .. } => None,
                }
            }

            fn is_index_row_up_to_date(
                conn: &Connection,
                id: &str,
                kind: i32,
                path: &str,
                offset: u64,
                size: usize,
            ) -> bool {
                let row: Option<(String, u64, usize, i32, i64, i64)> = conn
                    .query_row(
                        "SELECT body_path, range_offset, body_size, algo_version, block_size, bitset_bits \
                         FROM traffic_body_index_v1 WHERE id = ?1 AND kind = ?2",
                        params![id, kind],
                        |r| {
                            Ok((
                                r.get(0)?,
                                r.get::<_, i64>(1)? as u64,
                                r.get::<_, i64>(2)? as usize,
                                r.get(3)?,
                                r.get::<_, i64>(4)?,
                                r.get::<_, i64>(5)?,
                            ))
                        },
                    )
                    .optional()
                    .ok()
                    .flatten();

                let Some((p, off, sz, algo, block, bits)) = row else {
                    return false;
                };
                algo == BODY_INDEX_ALGO_VERSION
                    && block == BODY_INDEX_BLOCK_SIZE as i64
                    && bits == BODY_INDEX_BITSET_BITS as i64
                    && p == path
                    && off == offset
                    && sz == size
            }

            while let Some(job) = rx.blocking_recv() {
                let (path, offset, size) = match fetch_body_ref_info(&conn, &job.id, job.kind) {
                    Some(v) => v,
                    None => continue,
                };

                if size < BODY_INDEX_MIN_BODY_SIZE {
                    continue;
                }

                if is_index_row_up_to_date(&conn, &job.id, job.kind, &path, offset, size) {
                    continue;
                }

                match build_body_index_v1(&path, offset, size) {
                    Ok((block_count, bitsets)) => {
                        let result = conn.execute(
                            "INSERT OR REPLACE INTO traffic_body_index_v1 \
                             (id, kind, algo_version, block_size, bitset_bits, body_path, range_offset, body_size, block_count, bitsets) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                            params![
                                job.id,
                                job.kind,
                                BODY_INDEX_ALGO_VERSION,
                                BODY_INDEX_BLOCK_SIZE as i64,
                                BODY_INDEX_BITSET_BITS as i64,
                                path,
                                offset as i64,
                                size as i64,
                                block_count as i64,
                                bitsets,
                            ],
                        );
                        if let Err(e) = result {
                            tracing::warn!(error = %e, "[BODY_INDEX] Failed to upsert index row");
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, path = %path, "[BODY_INDEX] Skip index build");
                    }
                }
            }

            tracing::info!("[BODY_INDEX] Worker stopped");
        })
    }

    pub fn set_cleanup_notifier(&self, notifier: CleanupNotifier) {
        *self.cleanup_notifier.write() = Some(notifier);
    }

    fn open_or_reset_database(db_path: &PathBuf) -> Result<Connection, rusqlite::Error> {
        let conn = Connection::open(db_path)?;

        match init_database(&conn) {
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

                let new_conn = Connection::open(db_path)?;
                init_database(&new_conn).map_err(|e| match e {
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

    pub fn record(&self, mut record: TrafficRecord) {
        let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);
        record.sequence = seq;

        let _ = self.tx.send(record.clone());

        let conn = self.write_conn.lock();
        let flags = encode_flags(&record);

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
        let socket_blob = record
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
            .and_then(|r| bincode::serialize(r).ok());
        let res_script_results_blob = record
            .res_script_results
            .as_ref()
            .and_then(|r| bincode::serialize(r).ok());

        let result = conn.execute(
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
                timing_blob,
                req_headers_blob,
                res_headers_blob,
                rules_blob,
                socket_blob,
                req_body_blob,
                res_body_blob,
                &record.actual_url,
                &record.actual_host,
                orig_req_headers_blob,
                actual_res_headers_blob,
                req_script_results_blob,
                res_script_results_blob,
                &record.error_message,
            ],
        );

        if let Err(e) = result {
            tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to insert record");
        } else if Self::should_keep_in_cache(&record) {
            let mut cache = self.recent_cache.write();
            cache.put(record.id.clone(), record.clone());

            // Enqueue body index jobs (best-effort, must not block)
            self.enqueue_body_index_jobs(&record);
        } else {
            // Enqueue body index jobs even if not cached
            self.enqueue_body_index_jobs(&record);
        }

        let count = self.write_count.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(CLEANUP_CHECK_INTERVAL) {
            self.maybe_cleanup(&conn);
        }
    }

    pub fn update_by_id<F>(&self, id: &str, updater: F) -> bool
    where
        F: FnOnce(&mut TrafficRecord),
    {
        let mut updater = Some(updater);
        {
            let mut cache = self.recent_cache.write();
            let updated = if let Some(record) = cache.get_mut(id) {
                if let Some(updater) = updater.take() {
                    updater(record);
                }
                Some(record.clone())
            } else {
                None
            };
            if let Some(updated) = updated {
                if !Self::should_keep_in_cache(&updated) {
                    cache.pop(id);
                }
                drop(cache);
                self.persist_update(&updated);
                let _ = self.tx.send(updated);
                return true;
            }
        }

        if let Some(mut record) = self.get_by_id_from_db(id) {
            if let Some(updater) = updater.take() {
                updater(&mut record);
            }
            self.persist_update(&record);
            {
                let mut cache = self.recent_cache.write();
                if Self::should_keep_in_cache(&record) {
                    cache.put(record.id.clone(), record.clone());
                } else {
                    cache.pop(&record.id);
                }
            }
            let _ = self.tx.send(record);
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
        let conn = self.write_conn.lock();
        let flags = encode_flags(record);

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
        let socket_blob = record
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
            .and_then(|r| bincode::serialize(r).ok());
        let res_script_results_blob = record
            .res_script_results
            .as_ref()
            .and_then(|r| bincode::serialize(r).ok());

        let result = conn.execute(
            get_update_sql(),
            params![
                record.status as i32,
                &record.content_type,
                record.request_size as i64,
                record.response_size as i64,
                record.duration_ms as i64,
                &record.client_app,
                record.client_pid.map(|p| p as i32),
                &record.client_path,
                flags as i32,
                record.frame_count as i64,
                record.last_frame_id as i64,
                timing_blob,
                req_headers_blob,
                res_headers_blob,
                rules_blob,
                socket_blob,
                req_body_blob,
                res_body_blob,
                &record.actual_url,
                &record.actual_host,
                orig_req_headers_blob,
                actual_res_headers_blob,
                req_script_results_blob,
                res_script_results_blob,
                &record.error_message,
                &record.id,
            ],
        );

        if let Err(e) = result {
            tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to update record");
        } else {
            // Body refs can be updated after initial insert (e.g. streaming)
            self.enqueue_body_index_jobs(record);
        }
    }

    fn enqueue_body_index_jobs(&self, record: &TrafficRecord) {
        if !self.is_body_index_enabled() {
            return;
        }
        let Some(_tx) = &self.body_index_tx else {
            return;
        };

        // Ensure scheduler started (lazy). Must not block.
        self.ensure_body_index_scheduler_started();

        if let Some(ref r) = record.request_body_ref {
            self.register_body_index_candidate(&record.id, 0, r);
        }
        if let Some(ref r) = record.response_body_ref {
            self.register_body_index_candidate(&record.id, 1, r);
        }
    }

    fn register_body_index_candidate(&self, id: &str, kind: i32, body_ref: &BodyRef) {
        let (path, offset, size) = match body_ref {
            BodyRef::File { path, size } => (path.clone(), 0u64, *size),
            BodyRef::FileRange { path, offset, size } => (path.clone(), *offset, *size),
            BodyRef::Inline { .. } => return,
        };

        if size < BODY_INDEX_MIN_BODY_SIZE {
            return;
        }

        // 只缓存轻量 fingerprint/size，避免在 pending 队列里保存 path 等大对象。
        let dedupe_key = format!("{}:{}", id, kind);
        let fingerprint = Self::hash_body_ref_fingerprint(&path, offset, size);
        let now = Self::now_ms();

        let mut changed = true;
        {
            let mut cache = self.body_index_dedupe.write();
            if let Some(prev) = cache.get(&dedupe_key) {
                if prev.fingerprint == fingerprint && prev.size == size {
                    changed = false;
                }
            }
            // refresh LRU even if unchanged
            cache.put(dedupe_key.clone(), BodyIndexDedupeKey { fingerprint, size });
        }

        // Update pending candidate (debounced). Must not block.
        let mut pending = self.body_index_pending.lock();
        match pending.get_mut(&dedupe_key) {
            Some(p) => {
                // 只有观察到 body 变化时才刷新稳定窗口；否则会导致持续更新的请求永远无法进入 idle。
                if changed {
                    p.last_change_ms = now;
                    p.next_attempt_ms = 0;
                }
            }
            None => {
                pending.insert(
                    dedupe_key,
                    PendingBodyIndexJob {
                        id: id.to_string(),
                        kind,
                        last_change_ms: now,
                        next_attempt_ms: 0,
                    },
                );
            }
        }

        // NOTE: 不在这里直接 try_send，避免把索引构建压力注入写路径。
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    pub fn get_body_indexes_by_ids(
        &self,
        ids: &[&str],
        kind: i32,
    ) -> std::collections::HashMap<String, BodyIndexRow> {
        use std::collections::HashMap;

        if ids.is_empty() {
            return HashMap::new();
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT id, kind, body_path, range_offset, body_size, block_count, bitsets \
             FROM traffic_body_index_v1 WHERE kind = ? AND id IN ({})",
            placeholders.join(",")
        );

        let conn = self.read_conn.lock();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len() + 1);
        params.push(&kind);
        for id in ids {
            params.push(id);
        }

        let mut out = HashMap::new();
        let iter = match stmt.query_map(params.as_slice(), |row| {
            Ok(BodyIndexRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                path: row.get(2)?,
                offset: row.get::<_, i64>(3)? as u64,
                size: row.get::<_, i64>(4)? as usize,
                block_count: row.get::<_, i64>(5)? as usize,
                bitsets: row.get(6)?,
            })
        }) {
            Ok(i) => i,
            Err(_) => return HashMap::new(),
        };

        for r in iter.flatten() {
            out.insert(r.id.clone(), r);
        }
        out
    }

    pub fn query(&self, params: &QueryParams) -> QueryResult {
        self.query_internal(params, true)
    }

    /// 用于搜索等高频迭代场景的查询：不会计算 total（COUNT(*)），避免重复全表扫描。
    pub fn query_for_search(&self, params: &QueryParams) -> QueryResult {
        self.query_internal(params, false)
    }

    fn query_internal(&self, params: &QueryParams, include_total: bool) -> QueryResult {
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
                    total: 0,
                    server_sequence: self.current_sequence.load(Ordering::Relaxed),
                };
            }
        };

        let records: Vec<TrafficSummaryCompact> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let socket_blob: Option<Vec<u8>> = row.get(18)?;
                let socket_status: Option<SocketStatus> =
                    socket_blob.and_then(|b| bincode::deserialize(&b).ok());

                let rules_blob: Option<Vec<u8>> = row.get(19)?;
                let matched_rules: Vec<crate::traffic::MatchedRule> = rules_blob
                    .and_then(|b| bincode::deserialize(&b).ok())
                    .unwrap_or_default();
                let rc = matched_rules.len();
                let rp: Vec<String> = matched_rules
                    .iter()
                    .map(|r| r.protocol.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();

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
                    req_ct: row.get(20)?,
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

        let total = if include_total {
            self.count_with_conn(&conn, params)
        } else {
            0
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
        let mut columns: Vec<&str> = vec!["id"];
        if need_url {
            columns.push("url");
        }
        if need_request_headers {
            columns.push("request_headers_blob");
        }
        if need_response_headers {
            columns.push("response_headers_blob");
        }
        if need_request_body_ref {
            columns.push("request_body_ref_blob");
        }
        if need_response_body_ref {
            columns.push("response_body_ref_blob");
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
                        },
                    )
                })
                .collect();
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT {} FROM traffic_records WHERE id IN ({})",
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
                // idx += 1;
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
        {
            let mut cache = self.recent_cache.write();
            if let Some(record) = cache.get(id) {
                return Some(record.clone());
            }
        }
        self.get_by_id_from_db(id)
    }

    fn get_by_id_from_db(&self, id: &str) -> Option<TrafficRecord> {
        let conn = self.read_conn.lock();

        conn.query_row("SELECT * FROM traffic_records WHERE id = ?", [id], |row| {
            Self::row_to_record(row)
        })
        .optional()
        .ok()
        .flatten()
    }

    pub fn get_by_ids(&self, ids: &[&str]) -> Vec<TrafficSummaryCompact> {
        if ids.is_empty() {
            return vec![];
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT sequence, id, timestamp, host, method, status, protocol, \
             url, path, content_type, request_size, response_size, duration_ms, \
             client_ip, client_app, client_pid, flags, frame_count, socket_status_blob, \
             matched_rules_blob \
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
            let socket_blob: Option<Vec<u8>> = row.get(18)?;
            let socket_status: Option<SocketStatus> =
                socket_blob.and_then(|b| bincode::deserialize(&b).ok());

            let rules_blob: Option<Vec<u8>> = row.get(19)?;
            let matched_rules: Vec<crate::traffic::MatchedRule> = rules_blob
                .and_then(|b| bincode::deserialize(&b).ok())
                .unwrap_or_default();
            let rc = matched_rules.len();
            let rp: Vec<String> = matched_rules
                .iter()
                .map(|r| r.protocol.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

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
                req_ct: row.get(20)?,
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

    fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<TrafficRecord> {
        let timing_blob: Option<Vec<u8>> = row.get("timing_blob")?;
        let req_headers_blob: Option<Vec<u8>> = row.get("request_headers_blob")?;
        let res_headers_blob: Option<Vec<u8>> = row.get("response_headers_blob")?;
        let rules_blob: Option<Vec<u8>> = row.get("matched_rules_blob")?;
        let socket_blob: Option<Vec<u8>> = row.get("socket_status_blob")?;
        let req_body_blob: Option<Vec<u8>> = row.get("request_body_ref_blob")?;
        let res_body_blob: Option<Vec<u8>> = row.get("response_body_ref_blob")?;
        let orig_req_headers_blob: Option<Vec<u8>> = row.get("original_request_headers_blob")?;
        let actual_res_headers_blob: Option<Vec<u8>> = row.get("actual_response_headers_blob")?;
        let req_script_results_blob: Option<Vec<u8>> = row.get("req_script_results_blob")?;
        let res_script_results_blob: Option<Vec<u8>> = row.get("res_script_results_blob")?;

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
            timing: timing_blob.and_then(|b| bincode::deserialize(&b).ok()),
            request_headers: req_headers_blob.and_then(|b| bincode::deserialize(&b).ok()),
            response_headers: res_headers_blob.and_then(|b| bincode::deserialize(&b).ok()),
            matched_rules: rules_blob.and_then(|b| bincode::deserialize(&b).ok()),
            socket_status: socket_blob.and_then(|b| bincode::deserialize(&b).ok()),
            request_body_ref: req_body_blob.and_then(|b| bincode::deserialize(&b).ok()),
            response_body_ref: res_body_blob.and_then(|b| bincode::deserialize(&b).ok()),
            actual_url: row.get("actual_url")?,
            actual_host: row.get("actual_host")?,
            original_request_headers: orig_req_headers_blob
                .and_then(|b| bincode::deserialize(&b).ok()),
            actual_response_headers: actual_res_headers_blob
                .and_then(|b| bincode::deserialize(&b).ok()),
            error_message: row.get("error_message")?,
            req_script_results: req_script_results_blob.and_then(|b| bincode::deserialize(&b).ok()),
            res_script_results: res_script_results_blob.and_then(|b| bincode::deserialize(&b).ok()),
        })
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
            }
        }

        let mut cache = self.recent_cache.write();
        let preserved_ids: Vec<String> = cache
            .iter()
            .filter(|(id, record)| {
                active_ids_set.contains(id.as_str())
                    || (record.is_websocket
                        && record.socket_status.as_ref().is_some_and(|s| s.is_open))
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
                tracing::info!(count = count, "[TRAFFIC_DB] Deleted traffic records by ids");
            }
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to delete records by ids");
            }
        }

        self.remove_from_cache(ids);
    }

    fn maybe_cleanup(&self, conn: &Connection) {
        let max = self.max_records.load(Ordering::Relaxed);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM traffic_records", [], |row| row.get(0))
            .unwrap_or(0);

        if count as usize > max {
            let excess = count as usize - max;
            let deleted = self.delete_oldest_by_limit(conn, excess);
            if deleted > 0 {
                tracing::debug!(
                    deleted = deleted,
                    max = max,
                    "[TRAFFIC_DB] Cleaned up old records"
                );
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
                    tracing::info!(
                        deleted = deleted,
                        db_size = db_size,
                        max_db_size_bytes = max_db_size_bytes,
                        target_size = target_size,
                        "[TRAFFIC_DB] Cleaned up records due to DB size limit"
                    );
                    Self::compact_with_conn(conn, true);
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
            tracing::info!(
                deleted = deleted,
                retention_hours = retention_hours,
                "[TRAFFIC_DB] Cleaned up expired records"
            );
        }
        deleted
    }

    pub fn count(&self) -> usize {
        let conn = self.read_conn.lock();
        conn.query_row("SELECT COUNT(*) FROM traffic_records", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|v| v as usize)
        .unwrap_or(0)
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

        stmt.query_map([], |row| {
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
        .unwrap_or_default()
    }

    pub fn aggregate_app_metrics(&self) -> Vec<AppMetricsAggregate> {
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

        stmt.query_map([], |row| {
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
        .unwrap_or_default()
    }

    pub fn current_sequence(&self) -> u64 {
        self.current_sequence.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TrafficRecord> {
        self.tx.subscribe()
    }

    pub fn set_max_records(&self, max: usize) {
        let old = self.max_records.swap(max, Ordering::SeqCst);
        if old != max {
            tracing::info!(old = old, new = max, "[TRAFFIC_DB] Max records updated");
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

            let index_sql = format!(
                "DELETE FROM traffic_body_index_v1 WHERE id IN ({})",
                placeholders
            );
            let _ = conn.execute(&index_sql, rusqlite::params_from_iter(chunk.iter()));
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
}

impl Drop for TrafficDbStore {
    fn drop(&mut self) {
        // Stop scheduler first (it owns a clone of body_index_tx).
        self.body_index_scheduler_cancel
            .store(true, Ordering::SeqCst);
        if let Some(handle) = self.body_index_scheduler_handle.lock().take() {
            let _ = handle.join();
        }

        // Close channel so worker can exit.
        self.body_index_tx.take();

        if let Some(handle) = self.body_index_worker_handle.lock().take() {
            let _ = handle.join();
        }

        // Best-effort: release pending jobs to keep drop lightweight in tests.
        self.body_index_pending.lock().clear();
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

pub fn start_db_cleanup_task(store: SharedTrafficDbStore) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let deleted = store.cleanup_expired_records();
            if deleted > 0 {
                store.compact_db(false);
            }
        }
    });
}

fn build_body_index_v1(
    path: &str,
    offset: u64,
    size: usize,
) -> Result<(usize, Vec<u8>), std::io::Error> {
    if size == 0 {
        return Ok((0, Vec::new()));
    }

    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;

    let block_count = size.div_ceil(BODY_INDEX_BLOCK_SIZE);
    let mut bitsets = vec![0u8; block_count.saturating_mul(BODY_INDEX_BITSET_BYTES)];

    let mut remaining = size;
    let mut buf = vec![0u8; BODY_INDEX_BLOCK_SIZE];

    for block_idx in 0..block_count {
        let to_read = remaining.min(BODY_INDEX_BLOCK_SIZE);
        if to_read == 0 {
            break;
        }

        file.read_exact(&mut buf[..to_read])?;
        remaining = remaining.saturating_sub(to_read);

        let base = block_idx * BODY_INDEX_BITSET_BYTES;
        let bits = &mut bitsets[base..base + BODY_INDEX_BITSET_BYTES];
        index_block_bytes_v1(&buf[..to_read], bits);
    }

    Ok((block_count, bitsets))
}

#[inline]
fn index_block_bytes_v1(block: &[u8], bitset: &mut [u8]) {
    if block.len() < 3 {
        return;
    }

    let mask = (BODY_INDEX_BITSET_BITS - 1) as u32;
    for i in 0..(block.len() - 2) {
        let b0 = fold_ascii_lower(block[i]);
        let b1 = fold_ascii_lower(block[i + 1]);
        let b2 = fold_ascii_lower(block[i + 2]);
        let idx = (hash_trigram_u32(b0, b1, b2) & mask) as usize;
        let byte = idx >> 3;
        let bit = 1u8 << (idx & 7);
        bitset[byte] |= bit;
    }
}

#[inline]
fn fold_ascii_lower(b: u8) -> u8 {
    if b.is_ascii_uppercase() {
        b + 32
    } else {
        b
    }
}

#[inline]
fn hash_trigram_u32(b0: u8, b1: u8, b2: u8) -> u32 {
    // Cheap mixing for 3-byte key; bitset_bits is power-of-two so we use mask.
    let mut x = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_store::BodyRef;
    use std::env;
    use std::io::Write;
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
    fn test_body_index_threads_stop_on_drop() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();
        store.set_body_index_enabled(true);

        let body_path = dir.join("body.bin");
        let size = BODY_INDEX_MIN_BODY_SIZE + 1024;
        let mut f = fs::File::create(&body_path).unwrap();
        f.write_all(&vec![b'A'; size]).unwrap();

        let mut record = TrafficRecord::new(
            "req-1".to_string(),
            "GET".to_string(),
            "https://a.com/p1".to_string(),
        );
        record.response_body_ref = Some(BodyRef::File {
            path: body_path.to_string_lossy().to_string(),
            size,
        });

        // Start scheduler (lazy) and make job ready immediately (avoid 3s debounce in tests).
        store.enqueue_body_index_jobs(&record);
        let key = format!("{}:{}", record.id, 1);
        let now = TrafficDbStore::now_ms();
        {
            let mut pending = store.body_index_pending.lock();
            if let Some(p) = pending.get_mut(&key) {
                p.last_change_ms = now.saturating_sub(BODY_INDEX_IDLE_DEBOUNCE_MS + 1000);
            }
        }

        // Give scheduler a chance to tick and start worker.
        std::thread::sleep(std::time::Duration::from_millis(
            BODY_INDEX_SCHED_TICK_MS * 2,
        ));

        drop(store);
        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_body_index_requeue_does_not_wait_full_debounce() {
        let dir = create_test_dir();
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

        // Requeued jobs should become eligible after backoff without waiting another debounce window.
        store.requeue_body_index_job(
            BodyIndexJob {
                id: "req-1".to_string(),
                kind: 1,
                approx_size: BODY_INDEX_MIN_BODY_SIZE,
            },
            10,
        );

        // Not ready before backoff.
        assert!(store
            .drain_ready_body_index_jobs(TrafficDbStore::now_ms())
            .is_empty());

        std::thread::sleep(std::time::Duration::from_millis(20));
        let ready = store.drain_ready_body_index_jobs(TrafficDbStore::now_ms());
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "req-1");
        assert_eq!(ready[0].kind, 1);

        cleanup_test_dir(&dir);
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
