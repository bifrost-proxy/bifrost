static ASR_JOB_RUN_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static ASR_SCHEDULER_STARTED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
/// Tracks which task IDs are currently being executed (manual or scheduled).
/// Uses std::sync::Mutex for access from both sync and async contexts.
static RUNNING_TASKS: Lazy<StdMutex<HashSet<String>>> = Lazy::new(|| StdMutex::new(HashSet::new()));
static FORCE_PAUSED_TASKS: Lazy<StdMutex<HashSet<String>>> =
    Lazy::new(|| StdMutex::new(HashSet::new()));
static BULK_CHUNK_RETRY_JOBS: Lazy<StdMutex<BTreeMap<String, BulkChunkRetryState>>> =
    Lazy::new(|| StdMutex::new(BTreeMap::new()));
static EXTERNAL_IMPORT_LOCK: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));
static RUNNING_EXTERNAL_IMPORT_TASKS: Lazy<StdMutex<HashSet<String>>> =
    Lazy::new(|| StdMutex::new(HashSet::new()));
static CONTENT_HASH_QUEUE_LOCK: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));

const TASK_STORE_VERSION: u32 = 1;
const ASR_TASK_PAUSED_MESSAGE: &str = "ASR task paused by request";
const ASR_AUTO_FALLBACK_RTF_MULTIPLIER: f64 = 1.5;
const MIN_BISECT_SECS: u64 = 2;
const FFMPEG_NORMALIZE_MIN_TIMEOUT_SECS: u64 = 120;
const FFMPEG_NORMALIZE_MAX_TIMEOUT_SECS: u64 = 30 * 60;
const FFMPEG_CHUNK_SPLIT_TIMEOUT_SECS: u64 = 60;
#[cfg(not(test))]
const ASR_PREFLIGHT_HASH_MAX_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(test)]
const ASR_PREFLIGHT_HASH_MAX_BYTES: u64 = 1024;
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "aif", "flac", "m4a", "mp3", "mp4", "ogg", "opus", "wav", "webm",
];

struct RunningTaskGuard {
    task_id: String,
}

impl RunningTaskGuard {
    fn acquire(task_id: &str) -> Result<Self, ()> {
        let mut running = RUNNING_TASKS.lock().unwrap();
        if running.contains(task_id) {
            return Err(());
        }
        running.insert(task_id.to_string());
        Ok(Self {
            task_id: task_id.to_string(),
        })
    }
}

impl Drop for RunningTaskGuard {
    fn drop(&mut self) {
        RUNNING_TASKS.lock().unwrap().remove(&self.task_id);
    }
}

fn task_is_running(task_id: &str) -> bool {
    RUNNING_TASKS.lock().unwrap().contains(task_id)
}

struct RunningExternalImportGuard {
    task_id: String,
}

impl RunningExternalImportGuard {
    fn acquire(task_id: &str) -> Result<Self, ()> {
        let mut running = RUNNING_EXTERNAL_IMPORT_TASKS.lock().unwrap();
        if running.contains(task_id) {
            return Err(());
        }
        running.insert(task_id.to_string());
        Ok(Self {
            task_id: task_id.to_string(),
        })
    }
}

impl Drop for RunningExternalImportGuard {
    fn drop(&mut self) {
        RUNNING_EXTERNAL_IMPORT_TASKS
            .lock()
            .unwrap()
            .remove(&self.task_id);
    }
}

fn external_import_is_running(task_id: &str) -> bool {
    RUNNING_EXTERNAL_IMPORT_TASKS
        .lock()
        .unwrap()
        .contains(task_id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDirectoryTask {
    pub id: String,
    pub name: String,
    pub audio_dir: PathBuf,
    pub recursive: bool,
    pub enabled: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at_ms: Option<u64>,
    #[serde(default = "default_task_schedule")]
    pub schedule: AsrTaskSchedule,
    pub language: String,
    pub model: String,
    #[serde(default)]
    pub runtime_strategy: AsrRuntimeStrategy,
    #[serde(default)]
    pub diarization: AsrDiarizationConfig,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_run_at_ms: Option<u64>,
    pub next_run_at_ms: Option<u64>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub daily_agent: AsrDailyAgentConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_devices: Vec<AsrExternalDeviceBinding>,
    #[serde(default)]
    pub import_policy: AsrExternalImportPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AsrTaskPauseMode {
    Temporary,
    LongTerm,
}

impl AsrTaskPauseMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Temporary => "temporary",
            Self::LongTerm => "long_term",
        }
    }

    fn from_query(value: &str) -> Option<Self> {
        match value {
            "temporary" | "temp" | "until_next_schedule" => Some(Self::Temporary),
            "long_term" | "long-term" | "indefinite" | "manual" => Some(Self::LongTerm),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AsrExternalDeviceBinding {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_identifier: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_import_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrExternalImportPolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_external_file_stable_secs")]
    pub file_stable_secs: u64,
    #[serde(default = "default_external_min_free_bytes")]
    pub min_free_bytes: u64,
    #[serde(default = "default_external_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_true")]
    pub auto_run_after_import: bool,
    #[serde(default = "default_true")]
    pub content_hash_dedupe_enabled: bool,
    #[serde(default = "default_content_hash_algorithm")]
    pub content_hash_algorithm: String,
    #[serde(default)]
    pub delete_source_after_import: bool,
}

impl Default for AsrExternalImportPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            file_stable_secs: default_external_file_stable_secs(),
            min_free_bytes: default_external_min_free_bytes(),
            max_file_bytes: default_external_max_file_bytes(),
            auto_run_after_import: true,
            content_hash_dedupe_enabled: true,
            content_hash_algorithm: default_content_hash_algorithm(),
            delete_source_after_import: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_external_file_stable_secs() -> u64 {
    10
}

fn default_external_min_free_bytes() -> u64 {
    10 * 1024 * 1024 * 1024
}

fn default_external_max_file_bytes() -> u64 {
    50 * 1024 * 1024 * 1024
}

fn default_content_hash_algorithm() -> String {
    "blake3".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrRuntimeStrategy {
    /// Conservative isolation: every chunk gets a fresh native `asr` process.
    ForkPerChunk,
    /// Experimental: reuse one managed `asr-server` across chunks.
    ReuseServer,
    /// Production default: reuse one managed server inside a file, then restart at file boundaries.
    #[default]
    ReusePerFile,
    /// Try server reuse first; fall back to fork-per-chunk on errors or RTF drift.
    Auto,
    /// Run both server reuse and fork-per-chunk for the same chunk, keep fork output.
    Compare,
}

impl AsrRuntimeStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::ForkPerChunk => "fork_per_chunk",
            Self::ReuseServer => "reuse_server",
            Self::ReusePerFile => "reuse_per_file",
            Self::Auto => "auto",
            Self::Compare => "compare",
        }
    }

    fn uses_task_lifetime_server(self) -> bool {
        matches!(self, Self::ReuseServer | Self::Auto | Self::Compare)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AsrTaskSchedule {
    Hourly {
        minute: u32,
    },
    Daily {
        hour: u32,
        minute: u32,
    },
    Weekly {
        weekday: u32,
        hour: u32,
        minute: u32,
    },
    Monthly {
        day: u32,
        hour: u32,
        minute: u32,
    },
}

impl AsrTaskSchedule {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Hourly { minute } if *minute <= 59 => Ok(()),
            Self::Daily { hour, minute } if *hour <= 23 && *minute <= 59 => Ok(()),
            Self::Weekly {
                weekday,
                hour,
                minute,
            } if (1..=7).contains(weekday) && *hour <= 23 && *minute <= 59 => Ok(()),
            Self::Monthly { day, hour, minute }
                if (1..=31).contains(day) && *hour <= 23 && *minute <= 59 =>
            {
                Ok(())
            }
            _ => Err(
                "ASR task schedule is invalid; use hourly minute 0-59, daily/weekly/monthly wall-clock values"
                    .to_string(),
            ),
        }
    }

    fn initial_next_run_at_ms(&self, now_ms: u64) -> Option<u64> {
        self.next_run_at_ms(now_ms, true)
    }

    fn next_run_at_ms(&self, now_ms: u64, include_current_minute: bool) -> Option<u64> {
        let now = Local.timestamp_millis_opt(now_ms as i64).earliest()?;
        match self {
            Self::Hourly { minute } => {
                for hour_offset in 0..=24 {
                    let candidate_base = now + ChronoDuration::hours(hour_offset);
                    let candidate = local_datetime(
                        candidate_base.date_naive(),
                        candidate_base.hour(),
                        *minute,
                    )?;
                    if is_due_candidate(candidate, now, include_current_minute) {
                        return Some(candidate.timestamp_millis().max(now_ms as i64) as u64);
                    }
                }
                None
            }
            Self::Daily { hour, minute } => {
                for day_offset in 0..=7 {
                    let date = now.date_naive() + ChronoDuration::days(day_offset);
                    let candidate = local_datetime(date, *hour, *minute)?;
                    if is_due_candidate(candidate, now, include_current_minute) {
                        return Some(candidate.timestamp_millis().max(now_ms as i64) as u64);
                    }
                }
                None
            }
            Self::Weekly {
                weekday,
                hour,
                minute,
            } => {
                for day_offset in 0..=14 {
                    let date = now.date_naive() + ChronoDuration::days(day_offset);
                    if date.weekday().number_from_monday() != *weekday {
                        continue;
                    }
                    let candidate = local_datetime(date, *hour, *minute)?;
                    if is_due_candidate(candidate, now, include_current_minute) {
                        return Some(candidate.timestamp_millis().max(now_ms as i64) as u64);
                    }
                }
                None
            }
            Self::Monthly { day, hour, minute } => {
                for month_offset in 0..=13 {
                    let (year, month) = add_months(now.year(), now.month(), month_offset);
                    let clamped_day = (*day).min(last_day_of_month(year, month)?);
                    let date = NaiveDate::from_ymd_opt(year, month, clamped_day)?;
                    let candidate = local_datetime(date, *hour, *minute)?;
                    if is_due_candidate(candidate, now, include_current_minute) {
                        return Some(candidate.timestamp_millis().max(now_ms as i64) as u64);
                    }
                }
                None
            }
        }
    }
}

fn default_task_schedule() -> AsrTaskSchedule {
    AsrTaskSchedule::Daily { hour: 2, minute: 0 }
}

fn is_due_candidate(
    candidate: DateTime<Local>,
    now: DateTime<Local>,
    include_current_minute: bool,
) -> bool {
    candidate >= now
        || (include_current_minute
            && candidate.year() == now.year()
            && candidate.month() == now.month()
            && candidate.day() == now.day()
            && candidate.hour() == now.hour()
            && candidate.minute() == now.minute())
}

fn local_datetime(date: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Local>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(first, second) => Some(first.min(second)),
        LocalResult::None => {
            // DST gaps are rare but real. Move forward up to three hours and pick
            // the first representable local instant instead of dropping the task.
            for offset in 1..=180 {
                let shifted = naive + ChronoDuration::minutes(offset);
                match Local.from_local_datetime(&shifted) {
                    LocalResult::Single(value) => return Some(value),
                    LocalResult::Ambiguous(first, second) => return Some(first.min(second)),
                    LocalResult::None => {}
                }
            }
            None
        }
    }
}

fn add_months(year: i32, month: u32, offset: u32) -> (i32, u32) {
    let month_zero = month - 1 + offset;
    (year + (month_zero / 12) as i32, month_zero % 12 + 1)
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = add_months(year, month, 1);
    let first_next_month = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some(first_next_month.pred_opt()?.day())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskStore {
    version: u32,
    tasks: Vec<AsrDirectoryTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileStatus {
    Pending,
    Processing,
    Success,
    /// Some chunks failed but the rest succeeded. The file has partial text.
    PartialSuccess,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRecord {
    task_id: String,
    source_path: PathBuf,
    source_size: Option<u64>,
    source_modified_ms: Option<u64>,
    source_created_at_ms: Option<u64>,
    source_created_at_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash_algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duplicate_of_source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_alias: Option<String>,
    media_duration_ms: Option<u64>,
    status: FileStatus,
    output_text_path: Option<PathBuf>,
    output_metadata_path: Option<PathBuf>,
    output_timeline_path: Option<PathBuf>,
    text_chars: usize,
    error: Option<String>,
    #[serde(default)]
    runtime_strategy: AsrRuntimeStrategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chunk_metrics: Vec<AsrChunkMetric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_reason: Option<String>,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    /// Number of audio windows processed so far (for in-progress files).
    #[serde(default)]
    progress_current: Option<usize>,
    /// Total number of audio windows for this file.
    #[serde(default)]
    progress_total: Option<usize>,
    /// Chunks that failed all retries + bisect. Each entry records the chunk
    /// offset/duration so it can be retried individually later.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failed_chunks: Vec<FailedChunkRecord>,
    /// Memory-limit fallback hints learned from previous runs. A matching
    /// chunk starts directly at the remembered safe window instead of first
    /// retrying the full 30-second chunk that previously blew up Metal memory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    memory_limit_hints: Vec<AsrChunkMemoryHint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrChunkMetric {
    chunk_index: usize,
    offset_secs: u64,
    duration_secs: u64,
    runner: String,
    status: String,
    elapsed_ms: u64,
    rtf: f64,
    text_chars: usize,
    text_sha1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    recorded_at_ms: u64,
}

/// Record of a chunk that failed all retry and bisect attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FailedChunkRecord {
    /// Chunk index within the original file split.
    chunk_index: usize,
    /// Offset in seconds from the start of the normalized WAV.
    offset_secs: u64,
    /// Duration in seconds of this chunk.
    duration_secs: u64,
    /// Last error message.
    error: String,
    /// Number of retry attempts (including bisect sub-attempts).
    attempts: u32,
    /// RMS energy of the chunk WAV (16-bit PCM scale). None if not computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    energy_rms: Option<f64>,
    /// Whether this chunk was detected as silence (below threshold).
    #[serde(default)]
    is_silent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AsrChunkMemoryHint {
    model: String,
    offset_secs: u64,
    duration_secs: u64,
    preferred_chunk_secs: u64,
    trigger_count: u32,
    last_triggered_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct AsrMemoryLimitEvent {
    offset_secs: u64,
    duration_secs: u64,
    suggested_chunk_secs: u64,
    error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileStore {
    version: u32,
    files: BTreeMap<String, FileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrExternalImportStore {
    version: u32,
    #[serde(default)]
    devices: BTreeMap<String, AsrExternalDeviceState>,
    #[serde(default)]
    runs: Vec<AsrExternalImportRunSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrExternalDeviceState {
    binding_name: String,
    last_seen_mount_path: Option<PathBuf>,
    last_scan_at_ms: Option<u64>,
    last_import_at_ms: Option<u64>,
    last_status: Option<String>,
    last_error: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, AsrImportedFileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrImportedFileRecord {
    relative_path: PathBuf,
    source_size: u64,
    source_modified_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    source_hashes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_fingerprint: Option<String>,
    target_path: PathBuf,
    target_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_seen_at_ms: Option<u64>,
    imported_at_ms: u64,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrExternalImportRunSummary {
    run_id: String,
    device_name: Option<String>,
    started_at_ms: u64,
    finished_at_ms: u64,
    imported: usize,
    skipped: usize,
    #[serde(default)]
    processed_record_skipped: usize,
    failed: usize,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrExternalImportRunProgress {
    run_id: String,
    trigger: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<u64>,
    imported: usize,
    skipped: usize,
    #[serde(default)]
    processed_record_skipped: usize,
    failed: usize,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_file_size: Option<u64>,
    #[serde(default)]
    current_file_copied_bytes: u64,
    #[serde(default)]
    total_files_discovered: usize,
    #[serde(default)]
    processed_files: usize,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrContentHashIndex {
    version: u32,
    #[serde(default)]
    hashes: BTreeMap<String, AsrContentHashRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrContentHashRecord {
    algorithm: String,
    hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_audio_hash: Option<String>,
    size: u64,
    canonical_source_key: String,
    canonical_source_path: PathBuf,
    transcript_artifacts: AsrTranscriptArtifacts,
    model: String,
    language: String,
    runtime_strategy: AsrRuntimeStrategy,
    completed_at_ms: u64,
    duplicate_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrTranscriptArtifacts {
    text_path: PathBuf,
    metadata_path: PathBuf,
    timeline_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskSummary {
    discovered: usize,
    processed: usize,
    pending: usize,
    failed: usize,
    /// Files that completed but have some failed chunks.
    partial_success: usize,
    /// Total number of failed chunks across all files.
    failed_chunk_count: usize,
    deleted_after_processing: usize,
    /// Current disk usage of audio files still present under this task's audio directory.
    audio_source_bytes: u64,
    audio_source_file_count: usize,
    /// Subset of source audio files that are safe to delete because ASR outputs exist.
    cleanable_source_bytes: u64,
    cleanable_source_file_count: usize,
    running: bool,
    diarization_enabled: bool,
    diarization_ready: bool,
    diarization_running: bool,
    diarized_files: usize,
    speaker_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWithSummary {
    #[serde(flatten)]
    task: AsrDirectoryTask,
    summary: TaskSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    bulk_retry: Option<BulkChunkRetryState>,
}

#[derive(Debug, Clone, Serialize)]
struct FileRecordWithKey {
    key: String,
    #[serde(flatten)]
    record: FileRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarization_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarization_manifest_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct ExternalImportStatusResponse {
    policy: AsrExternalImportPolicy,
    devices: Vec<AsrExternalDeviceStatus>,
    runs: Vec<AsrExternalImportRunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_run: Option<AsrExternalImportRunProgress>,
}

#[derive(Debug, Clone, Serialize)]
struct AsrExternalDeviceStatus {
    binding: AsrExternalDeviceBinding,
    connected: bool,
    mount_path: Option<PathBuf>,
    status: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ExternalVolumeInfo {
    name: String,
    mount_path: PathBuf,
    volume_uuid: Option<String>,
    device_identifier: Option<String>,
    kind: String,
    read_only: bool,
    available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskDetail {
    #[serde(flatten)]
    task: AsrDirectoryTask,
    summary: TaskSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    bulk_retry: Option<BulkChunkRetryState>,
    files: Vec<FileRecordWithKey>,
    daily_documents: Vec<AsrDailyDocumentSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrRunProgress {
    run_id: String,
    trigger: String,
    status: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_source_path: Option<PathBuf>,
    #[serde(default)]
    current_file_index: usize,
    #[serde(default)]
    current_file_total: usize,
    #[serde(default)]
    current_chunk_done: usize,
    #[serde(default)]
    current_chunk_total: usize,
    #[serde(default)]
    processed_now: usize,
    #[serde(default)]
    failed_now: usize,
    #[serde(default = "default_asr_progress_stage")]
    stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn default_asr_progress_stage() -> String {
    "idle".to_string()
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchTask {
    id: String,
    name: String,
    audio_dir: PathBuf,
    enabled: bool,
    paused: bool,
    running: bool,
    schedule: AsrTaskSchedule,
    language: String,
    model: String,
    runtime_strategy: AsrRuntimeStrategy,
    diarization: AsrDiarizationConfig,
    last_run_at_ms: Option<u64>,
    next_run_at_ms: Option<u64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchProgress {
    discovered: usize,
    processed: usize,
    pending: usize,
    failed: usize,
    partial_success: usize,
    failed_chunk_count: usize,
    deleted_after_processing: usize,
    file_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_file_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_source_path: Option<PathBuf>,
    current_file_index: usize,
    current_file_total: usize,
    current_chunk_done: usize,
    current_chunk_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    eta_ms: Option<u64>,
    eta_confidence: &'static str,
    stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchConsumption {
    source_bytes_total: u64,
    source_bytes_processed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_bytes_processed_estimated: Option<u64>,
    audio_duration_ms_total: u64,
    audio_duration_ms_processed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_duration_ms_processed_estimated: Option<u64>,
    inference_elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_rtf: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_chunk_rtf: Option<f64>,
    text_chars: usize,
    chunks_completed: usize,
    chunks_failed: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchService {
    managed: bool,
    server_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    owner_module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchFile {
    key: String,
    source_path: PathBuf,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress_current: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress_total: Option<usize>,
    text_chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_chunk_rtf: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarization_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchDailyAgentDocument {
    date: String,
    status: String,
    change_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    report_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_len_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    processed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchDailyAgent {
    enabled: bool,
    runner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run_at_ms: Option<u64>,
    daily_files: usize,
    processed_documents: usize,
    pending_documents: usize,
    report_files: usize,
    indexed_reports: usize,
    unindexed_reports: usize,
    processed_missing_report: usize,
    recent_documents: Vec<TaskWatchDailyAgentDocument>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchSnapshot {
    task: TaskWatchTask,
    progress: TaskWatchProgress,
    consumption: TaskWatchConsumption,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<TaskWatchService>,
    recent_files: Vec<TaskWatchFile>,
    daily_agent: TaskWatchDailyAgent,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_progress: Option<AsrRunProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    snapshot_source: String,
    warnings: Vec<String>,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWatchListResponse {
    tasks: Vec<TaskWatchSnapshot>,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BulkChunkRetryStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
struct BulkChunkRetryFileResult {
    file_key: String,
    source_path: String,
    failed_before: usize,
    recovered: usize,
    still_failed: usize,
    status: String,
    elapsed_ms: u64,
    message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    daily_documents_refreshed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    persist_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BulkChunkRetryState {
    task_id: String,
    status: BulkChunkRetryStatus,
    queued_files: usize,
    processed_files: usize,
    total_failed_chunks: usize,
    recovered_chunks: usize,
    still_failed_chunks: usize,
    started_at_ms: Option<u64>,
    updated_at_ms: u64,
    finished_at_ms: Option<u64>,
    current_file_key: Option<String>,
    current_source_path: Option<String>,
    message: String,
    #[serde(default)]
    results: Vec<BulkChunkRetryFileResult>,
}

#[derive(Debug, Clone)]
struct BulkChunkRetryFileTarget {
    file_key: String,
    source_path: String,
    failed_chunks: usize,
}

#[derive(Debug)]
struct RetryFileChunksOutcome {
    status: FileStatus,
    total: usize,
    recovered: usize,
    still_failed_chunks: Vec<FailedChunkRecord>,
    daily_documents_refreshed: Vec<String>,
    persist_warnings: Vec<String>,
}

#[derive(Debug)]
enum RetryFileChunksError {
    TaskNotFound,
    FileNotFound,
    NoFailedChunks(FileStatus),
    Internal(String),
}

struct TranscriptionOutput {
    text: String,
    text_path: PathBuf,
    metadata_path: PathBuf,
    timeline_path: PathBuf,
    timeline: TranscriptTimeline,
    /// Chunks that failed all retries (empty for short files or full success).
    failed_chunks: Vec<FailedChunkRecord>,
    memory_limit_hints: Vec<AsrChunkMemoryHint>,
    chunk_metrics: Vec<AsrChunkMetric>,
    fallback_reason: Option<String>,
}

struct DiarizedTranscriptionOutput {
    text: String,
    timeline_segments: Vec<TimelineSegment>,
    speakers: Vec<TimelineSpeaker>,
    diarization_segments: Vec<DiarizationSegment>,
    failed_chunks: Vec<FailedChunkRecord>,
    memory_limit_hints: Vec<AsrChunkMemoryHint>,
    chunk_metrics: Vec<AsrChunkMetric>,
    fallback_reason: Option<String>,
}

type ChunkProgressCallback<'a> = dyn Fn(usize, usize) + Send + Sync + 'a;
type ChunkMetricCallback<'a> = dyn Fn(AsrChunkMetric) + Send + Sync + 'a;
type PauseCheckCallback<'a> = dyn Fn() -> bool + Send + Sync + 'a;

struct TaskTranscribeHooks<'a> {
    on_chunk_progress: Option<&'a ChunkProgressCallback<'a>>,
    on_chunk_metric: Option<&'a ChunkMetricCallback<'a>>,
    pause_check: Option<&'a PauseCheckCallback<'a>>,
    force_pause_task_id: Option<&'a str>,
    memory_limit_hints: &'a [AsrChunkMemoryHint],
    server_url: Option<&'a str>,
    startup_fallback_reason: Option<&'a str>,
    server_state: Option<&'a mut Option<ServerRunnerState>>,
    managed_server_restart: Option<ManagedServerRestartContext<'a>>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskRequest {
    name: Option<String>,
    audio_dir: PathBuf,
    recursive: Option<bool>,
    enabled: Option<bool>,
    schedule: Option<AsrTaskSchedule>,
    language: Option<String>,
    model: Option<String>,
    runtime_strategy: Option<AsrRuntimeStrategy>,
    diarization: Option<AsrDiarizationConfig>,
    external_devices: Option<Vec<AsrExternalDeviceBinding>>,
    import_policy: Option<AsrExternalImportPolicy>,
}

#[derive(Debug, Deserialize)]
struct UpdateTaskRequest {
    name: Option<String>,
    audio_dir: Option<PathBuf>,
    recursive: Option<bool>,
    enabled: Option<bool>,
    paused: Option<bool>,
    schedule: Option<AsrTaskSchedule>,
    language: Option<String>,
    model: Option<String>,
    runtime_strategy: Option<AsrRuntimeStrategy>,
    diarization: Option<AsrDiarizationConfig>,
    daily_agent: Option<AsrDailyAgentConfig>,
    external_devices: Option<Vec<AsrExternalDeviceBinding>>,
    import_policy: Option<AsrExternalImportPolicy>,
}

#[derive(Debug, Deserialize)]
struct ExternalImportUpdateRequest {
    external_devices: Option<Vec<AsrExternalDeviceBinding>>,
    import_policy: Option<AsrExternalImportPolicy>,
}

#[derive(Debug, Serialize)]
struct RunTaskResponse {
    task: TaskWithSummary,
    processed_now: usize,
    failed_now: usize,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct CleanupSourceAudioFailure {
    source_path: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct CleanupSourceAudioResponse {
    ok: bool,
    deleted_files: usize,
    deleted_bytes: u64,
    skipped_files: usize,
    skipped_bytes: u64,
    failed_files: Vec<CleanupSourceAudioFailure>,
    summary: TaskSummary,
    message: String,
}
