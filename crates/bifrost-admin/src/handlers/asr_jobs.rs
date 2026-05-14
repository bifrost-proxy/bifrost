use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, Local, LocalResult, NaiveDate, TimeZone,
    Timelike,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::warn;

use crate::asr_runtime::{now_ms, text_output_dir};
use crate::handlers::asr::{
    probe_asr_health, resolve_managed_target, start_managed_service, stop_any_managed_service,
    target_from_query,
};
use crate::handlers::asr_jobs_timeline::{
    inspect_source_audio, render_timeline_text, source_modified_ms, source_size, TimelineSegment,
    TranscriptTimeline,
};
use crate::handlers::asr_streaming::{
    append_transcript_delta, build_stream_windows, call_asr_text_endpoint, dedupe_increment,
    extract_wav_segment, normalize_asr_text, parse_wav_pcm_i16, stream_options_from_query,
};
use crate::handlers::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};

static ASR_JOB_RUN_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static ASR_SCHEDULER_STARTED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

const TASK_STORE_VERSION: u32 = 1;
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "aif", "flac", "m4a", "mp3", "mp4", "ogg", "opus", "wav", "webm",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDirectoryTask {
    pub id: String,
    pub name: String,
    pub audio_dir: PathBuf,
    pub recursive: bool,
    pub enabled: bool,
    #[serde(default = "default_task_schedule")]
    pub schedule: AsrTaskSchedule,
    pub language: String,
    pub model: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_run_at_ms: Option<u64>,
    pub next_run_at_ms: Option<u64>,
    pub last_error: Option<String>,
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
    media_duration_ms: Option<u64>,
    status: FileStatus,
    output_text_path: Option<PathBuf>,
    output_metadata_path: Option<PathBuf>,
    output_timeline_path: Option<PathBuf>,
    text_chars: usize,
    error: Option<String>,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileStore {
    version: u32,
    files: BTreeMap<String, FileRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskSummary {
    discovered: usize,
    processed: usize,
    pending: usize,
    failed: usize,
    deleted_after_processing: usize,
    running: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TaskWithSummary {
    #[serde(flatten)]
    task: AsrDirectoryTask,
    summary: TaskSummary,
}

#[derive(Debug, Clone, Serialize)]
struct FileRecordWithKey {
    key: String,
    #[serde(flatten)]
    record: FileRecord,
}

#[derive(Debug, Clone, Serialize)]
struct TaskDetail {
    #[serde(flatten)]
    task: AsrDirectoryTask,
    summary: TaskSummary,
    files: Vec<FileRecordWithKey>,
}

struct TranscriptionOutput {
    text: String,
    text_path: PathBuf,
    metadata_path: PathBuf,
    timeline_path: PathBuf,
    timeline: TranscriptTimeline,
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
}

#[derive(Debug, Serialize)]
struct RunTaskResponse {
    task: TaskWithSummary,
    processed_now: usize,
    failed_now: usize,
    message: String,
}

pub async fn handle_asr_tasks(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    ensure_scheduler_started().await;

    match (req.method(), path) {
        (&Method::GET, "/api/asr/tasks") => list_tasks_response(),
        (&Method::POST, "/api/asr/tasks") => create_task_response(req).await,
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/timeline") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/timeline")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "files" {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            }
            get_task_file_timeline_response(parts[0], parts[2])
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") => {
            let Some(id) = path
                .strip_prefix("/api/asr/tasks/")
                .filter(|id| !id.contains('/'))
            else {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            };
            get_task_response(id)
        }
        (&Method::DELETE, _) if path.starts_with("/api/asr/tasks/") => {
            let Some(id) = path
                .strip_prefix("/api/asr/tasks/")
                .filter(|id| !id.contains('/'))
            else {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            };
            delete_task_response(id)
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/run") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/run")
                .trim_end_matches('/');
            run_task_response(id).await
        }
        (&Method::GET, _) | (&Method::POST, _) | (&Method::DELETE, _) => {
            error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

async fn create_task_response(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read ASR task body: {error}"),
            );
        }
    };
    let create: CreateTaskRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid ASR task JSON: {error}"),
            )
        }
    };
    if !create.audio_dir.is_dir() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "ASR task audio_dir must be an existing directory",
        );
    }

    let now = now_ms();
    let schedule = create.schedule.unwrap_or_else(default_task_schedule);
    if let Err(error) = schedule.validate() {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let enabled = create.enabled.unwrap_or(true);
    let next_run_at_ms = enabled
        .then(|| schedule.initial_next_run_at_ms(now))
        .flatten();
    let task = AsrDirectoryTask {
        id: uuid::Uuid::new_v4().as_simple().to_string(),
        name: create
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "ASR directory task".to_string()),
        audio_dir: create.audio_dir,
        recursive: create.recursive.unwrap_or(true),
        enabled,
        schedule,
        language: create.language.unwrap_or_else(|| "chinese".to_string()),
        model: create.model.unwrap_or_else(|| "Qwen3-ASR-1.7B".to_string()),
        created_at_ms: now,
        updated_at_ms: now,
        last_run_at_ms: None,
        next_run_at_ms,
        last_error: None,
    };

    match add_task(task.clone()) {
        Ok(()) => json_response_with_status(StatusCode::CREATED, &task_with_summary(task)),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn list_tasks_response() -> Response<BoxBody> {
    let tasks = load_tasks()
        .tasks
        .into_iter()
        .map(task_with_summary)
        .collect::<Vec<_>>();
    json_response(&serde_json::json!({ "tasks": tasks }))
}

fn get_task_response(id: &str) -> Response<BoxBody> {
    match load_tasks().tasks.into_iter().find(|task| task.id == id) {
        Some(task) => json_response(&task_detail(task)),
        None => error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    }
}

fn get_task_file_timeline_response(task_id: &str, file_key: &str) -> Response<BoxBody> {
    let files = load_file_store(task_id);
    let Some(record) = files.files.get(file_key) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file not found");
    };
    let Some(path) = &record.output_timeline_path else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file timeline not found");
    };
    match std::fs::read_to_string(path)
        .map_err(|error| format!("read timeline {}: {error}", path.display()))
        .and_then(|content| {
            serde_json::from_str::<serde_json::Value>(&content)
                .map_err(|error| format!("parse timeline {}: {error}", path.display()))
        }) {
        Ok(value) => json_response(&value),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn delete_task_response(id: &str) -> Response<BoxBody> {
    let mut store = load_tasks();
    let before = store.tasks.len();
    store.tasks.retain(|task| task.id != id);
    if before == store.tasks.len() {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    }
    match save_tasks(&store) {
        Ok(()) => json_response(&serde_json::json!({ "ok": true })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn run_task_response(id: &str) -> Response<BoxBody> {
    let task = match load_tasks().tasks.into_iter().find(|task| task.id == id) {
        Some(task) => task,
        None => return error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    };

    match run_directory_task(task.clone()).await {
        Ok((task, processed_now, failed_now)) => json_response(&RunTaskResponse {
            task: task_with_summary(task),
            processed_now,
            failed_now,
            message: "ASR directory task run completed.".to_string(),
        }),
        Err(error) => {
            let _ = update_task_after_run(&task.id, Some(error.clone()));
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
        }
    }
}

async fn ensure_scheduler_started() {
    let mut started = ASR_SCHEDULER_STARTED.lock().await;
    if *started {
        return;
    }
    *started = true;
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now = now_ms();
            let due = load_tasks()
                .tasks
                .into_iter()
                .filter(|task| task.enabled && task.next_run_at_ms.is_some_and(|next| next <= now))
                .collect::<Vec<_>>();
            for task in due {
                tokio::spawn(async move {
                    if let Err(error) = run_directory_task(task.clone()).await {
                        warn!(task_id = %task.id, error = %error, "ASR scheduled directory task failed");
                        update_task_after_run(&task.id, Some(error)).ok();
                    }
                });
            }
        }
    });
}

async fn run_directory_task(
    task: AsrDirectoryTask,
) -> Result<(AsrDirectoryTask, usize, usize), String> {
    let _guard = ASR_JOB_RUN_LOCK.lock().await;
    let _task_lock = TaskRunFileLock::acquire(&task.id)?;
    let discovered = discover_audio_files(&task.audio_dir, task.recursive)?;
    let mut files = load_file_store(&task.id);
    for path in &discovered {
        let key = source_key(path);
        files
            .files
            .entry(key)
            .or_insert_with(|| pending_record(&task.id, path));
    }
    save_file_store(&task.id, &files)?;

    let pending = discovered
        .iter()
        .filter(|path| {
            files
                .files
                .get(&source_key(path))
                .map(|record| record.status != FileStatus::Success)
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();

    let target = target_from_query(Some(&format!(
        "language={}&model={}",
        urlencoding::encode(&task.language),
        urlencoding::encode(&task.model)
    )))?;
    let before = resolve_managed_target(target.clone()).await;
    let started_by_task = probe_asr_health(&before).await.is_err();
    let active_target = if started_by_task {
        let result = start_managed_service(target)
            .await
            .map_err(|response| response.detail.unwrap_or(response.message))?;
        let port = result
            .server_url
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| format!("failed to parse ASR service URL {}", result.server_url))?;
        target_from_query(Some(&format!(
            "port={port}&language={}&model={}",
            urlencoding::encode(&task.language),
            urlencoding::encode(&task.model)
        )))?
    } else {
        before
    };

    let mut processed_now = 0usize;
    let mut failed_now = 0usize;
    for path in pending {
        let key = source_key(&path);
        let mut record = pending_record(&task.id, &path);
        record.status = FileStatus::Processing;
        record.started_at_ms = Some(now_ms());
        files.files.insert(key.clone(), record);
        save_file_store(&task.id, &files)?;

        match transcribe_file_for_task(&task, &active_target, &path).await {
            Ok(output) => {
                let mut record = pending_record(&task.id, &path);
                record.status = FileStatus::Success;
                record.media_duration_ms = output.timeline.media_duration_ms;
                record.output_text_path = Some(output.text_path);
                record.output_metadata_path = Some(output.metadata_path);
                record.output_timeline_path = Some(output.timeline_path);
                record.text_chars = output.text.chars().count();
                record.finished_at_ms = Some(now_ms());
                files.files.insert(key, record);
                processed_now += 1;
            }
            Err(error) => {
                let mut record = pending_record(&task.id, &path);
                record.status = FileStatus::Failed;
                record.error = Some(error);
                record.finished_at_ms = Some(now_ms());
                files.files.insert(key, record);
                failed_now += 1;
            }
        }
        save_file_store(&task.id, &files)?;
    }

    if started_by_task {
        stop_any_managed_service().await;
    }

    let updated = update_task_after_run(
        &task.id,
        (failed_now > 0).then(|| format!("{failed_now} file(s) failed")),
    )?;
    Ok((updated, processed_now, failed_now))
}

async fn transcribe_file_for_task(
    task: &AsrDirectoryTask,
    target: &crate::handlers::asr::AsrTarget,
    path: &Path,
) -> Result<TranscriptionOutput, String> {
    let Some(server_url) = target.server_url() else {
        return Err("ASR service is not running".to_string());
    };
    let temp = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;
    let wav = temp.path().join("input.wav");
    normalize_audio_file(path, &wav).await?;
    let wav_bytes = std::fs::read(&wav).map_err(|error| format!("read normalized WAV: {error}"))?;
    let audio = parse_wav_pcm_i16(&wav_bytes)?;
    let source_info = inspect_source_audio(path);
    let options = stream_options_from_query(None)?;
    let windows = build_stream_windows(&audio, options);

    let mut committed = String::new();
    let mut segments = Vec::new();
    for window in &windows {
        let segment = temp.path().join(format!("segment-{:04}.wav", window.index));
        extract_wav_segment(&wav, &segment, window).await?;
        let text = call_asr_text_endpoint(&server_url, &task.language, &segment).await?;
        let text = normalize_asr_text(&text);
        let delta = dedupe_increment(&committed, &text);
        if !delta.is_empty() {
            append_transcript_delta(&mut committed, &delta);
            segments.push(TimelineSegment {
                index: segments.len(),
                audio_start_ms: window.stable_start_ms,
                audio_end_ms: window.stable_end_ms,
                absolute_start_ms: source_info
                    .source_created_at_ms
                    .map(|start| start.saturating_add(window.stable_start_ms)),
                absolute_end_ms: source_info
                    .source_created_at_ms
                    .map(|start| start.saturating_add(window.stable_end_ms)),
                text: delta,
            });
        }
        let _ = std::fs::remove_file(segment);
    }

    let text = committed.trim().to_string();
    let timeline = TranscriptTimeline {
        task_id: task.id.clone(),
        task_name: task.name.clone(),
        source_path: path.to_path_buf(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source.clone(),
        media_duration_ms: source_info
            .media_duration_ms
            .or_else(|| windows.last().map(|window| window.stable_end_ms)),
        model: task.model.clone(),
        language: task.language.clone(),
        processed_at_ms: now_ms(),
        segments,
    };
    let (text_path, metadata_path, timeline_path) = output_paths(&task.id, path);
    if let Some(parent) = text_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create text dir: {error}"))?;
    }
    let timeline_text = render_timeline_text(&timeline, &text);
    std::fs::write(&text_path, &timeline_text)
        .map_err(|error| format!("write transcript text: {error}"))?;
    std::fs::write(
        &timeline_path,
        serde_json::to_string_pretty(&timeline).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript timeline: {error}"))?;
    let metadata = serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "source_path": path,
        "source_size": timeline.source_size,
        "source_modified_ms": timeline.source_modified_ms,
        "source_created_at_ms": timeline.source_created_at_ms,
        "source_created_at_source": timeline.source_created_at_source,
        "media_duration_ms": timeline.media_duration_ms,
        "text_path": text_path,
        "timeline_path": timeline_path,
        "model": task.model,
        "language": task.language,
        "processed_at_ms": timeline.processed_at_ms,
    });
    std::fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript metadata: {error}"))?;
    Ok(TranscriptionOutput {
        text,
        text_path,
        metadata_path,
        timeline_path,
        timeline,
    })
}

async fn normalize_audio_file(input: &Path, output: &Path) -> Result<(), String> {
    let result = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("failed to run ffmpeg: {error}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
    }
}

fn add_task(task: AsrDirectoryTask) -> Result<(), String> {
    let mut store = load_tasks();
    store.tasks.push(task);
    save_tasks(&store)
}

fn load_tasks() -> TaskStore {
    let path = task_store_path();
    let content = std::fs::read_to_string(path).ok();
    content
        .and_then(|content| serde_json::from_str::<TaskStore>(&content).ok())
        .filter(|store| store.version == TASK_STORE_VERSION)
        .unwrap_or(TaskStore {
            version: TASK_STORE_VERSION,
            tasks: Vec::new(),
        })
}

fn save_tasks(store: &TaskStore) -> Result<(), String> {
    let path = task_store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create ASR task dir: {error}"))?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(store).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write ASR task store {}: {error}", path.display()))
}

fn update_task_after_run(id: &str, error: Option<String>) -> Result<AsrDirectoryTask, String> {
    let mut store = load_tasks();
    let now = now_ms();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == id) else {
        return Err(format!("ASR task '{id}' not found"));
    };
    task.last_run_at_ms = Some(now);
    task.updated_at_ms = now;
    task.last_error = error;
    task.next_run_at_ms = task
        .enabled
        .then(|| {
            task.schedule
                .next_run_at_ms(now.saturating_add(60_000), false)
        })
        .flatten();
    let updated = task.clone();
    save_tasks(&store)?;
    Ok(updated)
}

fn task_store_path() -> PathBuf {
    bifrost_storage::data_dir().join("asr/tasks.json")
}

fn file_store_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("files.json")
}

fn load_file_store(task_id: &str) -> FileStore {
    let path = file_store_path(task_id);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<FileStore>(&content).ok())
        .filter(|store| store.version == TASK_STORE_VERSION)
        .unwrap_or(FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        })
}

fn save_file_store(task_id: &str, store: &FileStore) -> Result<(), String> {
    let path = file_store_path(task_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create ASR file store dir: {error}"))?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(store).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write ASR file store {}: {error}", path.display()))
}

fn task_with_summary(task: AsrDirectoryTask) -> TaskWithSummary {
    let summary = summarize_task(&task);
    TaskWithSummary { task, summary }
}

fn task_detail(task: AsrDirectoryTask) -> TaskDetail {
    let summary = summarize_task(&task);
    let mut files = load_file_store(&task.id)
        .files
        .into_iter()
        .map(|(key, record)| FileRecordWithKey { key, record })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.record.source_path.cmp(&right.record.source_path));
    TaskDetail {
        task,
        summary,
        files,
    }
}

fn summarize_task(task: &AsrDirectoryTask) -> TaskSummary {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive).unwrap_or_default();
    let discovered_keys = discovered
        .iter()
        .map(|path| source_key(path))
        .collect::<HashSet<_>>();
    let file_store = load_file_store(&task.id);
    let processed = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::Success)
        .count();
    let failed = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::Failed)
        .count();
    let pending = discovered
        .iter()
        .filter(|path| {
            file_store
                .files
                .get(&source_key(path))
                .map(|record| matches!(record.status, FileStatus::Pending | FileStatus::Processing))
                .unwrap_or(true)
        })
        .count();
    let deleted_after_processing = file_store
        .files
        .keys()
        .filter(|key| !discovered_keys.contains(*key))
        .count();
    TaskSummary {
        discovered: discovered.len(),
        processed,
        pending,
        failed,
        deleted_after_processing,
        running: false,
    }
}

fn discover_audio_files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Err(format!(
            "audio directory does not exist: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    discover_audio_files_inner(root, recursive, &mut files)?;
    files.sort();
    Ok(files)
}

fn discover_audio_files_inner(
    root: &Path,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for entry in
        std::fs::read_dir(root).map_err(|error| format!("read dir {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("read dir entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read file type: {error}"))?;
        if file_type.is_dir() && recursive {
            discover_audio_files_inner(&path, recursive, out)?;
        } else if file_type.is_file() && is_audio_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn pending_record(task_id: &str, path: &Path) -> FileRecord {
    let source_info = inspect_source_audio(path);
    FileRecord {
        task_id: task_id.to_string(),
        source_path: path.to_path_buf(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source,
        media_duration_ms: source_info.media_duration_ms,
        status: FileStatus::Pending,
        output_text_path: None,
        output_metadata_path: None,
        output_timeline_path: None,
        text_chars: 0,
        error: None,
        started_at_ms: None,
        finished_at_ms: None,
    }
}

fn source_key(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha1::new();
    hasher.update(absolute.to_string_lossy().as_bytes());
    if let Some(size) = source_size(path) {
        hasher.update(size.to_le_bytes());
    }
    if let Some(modified_ms) = source_modified_ms(path) {
        hasher.update(modified_ms.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn output_paths(task_id: &str, source: &Path) -> (PathBuf, PathBuf, PathBuf) {
    output_paths_in(&bifrost_storage::data_dir(), task_id, source)
}

fn output_paths_in(data_dir: &Path, task_id: &str, source: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let hash = source_key(source);
    let dir = text_output_dir(data_dir).join(task_id);
    (
        dir.join(format!("{hash}.txt")),
        dir.join(format!("{hash}.json")),
        dir.join(format!("{hash}.timeline.json")),
    )
}

struct TaskRunFileLock {
    path: PathBuf,
}

impl TaskRunFileLock {
    fn acquire(task_id: &str) -> Result<Self, String> {
        let path = bifrost_storage::data_dir()
            .join("asr/tasks")
            .join(task_id)
            .join("run.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create task lock dir: {error}"))?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("ASR task is already running or lock is stale: {error}"))?;
        Ok(Self { path })
    }
}

impl Drop for TaskRunFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;

    static TEST_DATA_DIR_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        previous: PathBuf,
    }

    impl EnvGuard {
        fn set_data_dir(path: &Path) -> Self {
            let previous = bifrost_storage::data_dir();
            bifrost_storage::set_data_dir(path.to_path_buf());
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            bifrost_storage::set_data_dir(self.previous.clone());
        }
    }

    #[test]
    fn discovers_audio_files_recursively_and_filters_extensions() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("nested")).unwrap();
        std::fs::write(temp.path().join("a.wav"), b"not real audio").unwrap();
        std::fs::write(temp.path().join("nested/b.m4a"), b"not real audio").unwrap();
        std::fs::write(temp.path().join("note.txt"), b"ignore").unwrap();

        let flat = discover_audio_files(temp.path(), false).unwrap();
        assert_eq!(flat.len(), 1);
        assert!(flat[0].ends_with("a.wav"));

        let recursive = discover_audio_files(temp.path(), true).unwrap();
        assert_eq!(recursive.len(), 2);
    }

    #[test]
    fn daily_schedule_can_start_on_current_minute_then_advances_to_next_day() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 14, 10, 30, 20)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Daily {
            hour: 10,
            minute: 30,
        };

        assert_eq!(schedule.initial_next_run_at_ms(now), Some(now));

        let next = schedule.next_run_at_ms(now.saturating_add(60_000), false);
        let next_dt = Local
            .timestamp_millis_opt(next.unwrap() as i64)
            .earliest()
            .unwrap();
        assert_eq!(next_dt.day(), 15);
        assert_eq!(next_dt.hour(), 10);
        assert_eq!(next_dt.minute(), 30);
    }

    #[test]
    fn weekly_schedule_uses_iso_weekday_and_wall_clock_time() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 14, 10, 30, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Weekly {
            weekday: 5,
            hour: 9,
            minute: 15,
        };

        let next = schedule.next_run_at_ms(now, false).unwrap();
        let next_dt = Local.timestamp_millis_opt(next as i64).earliest().unwrap();
        assert_eq!(next_dt.weekday().number_from_monday(), 5);
        assert_eq!(next_dt.hour(), 9);
        assert_eq!(next_dt.minute(), 15);
    }

    #[test]
    fn monthly_schedule_clamps_oversized_day_to_month_end() {
        let now = Local
            .with_ymd_and_hms(2026, 4, 1, 9, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Monthly {
            day: 31,
            hour: 10,
            minute: 5,
        };

        let next = schedule.next_run_at_ms(now, false).unwrap();
        let next_dt = Local.timestamp_millis_opt(next as i64).earliest().unwrap();
        assert_eq!(next_dt.month(), 4);
        assert_eq!(next_dt.day(), 30);
        assert_eq!(next_dt.hour(), 10);
        assert_eq!(next_dt.minute(), 5);
    }

    #[test]
    fn schedule_validation_rejects_out_of_range_values() {
        assert!(AsrTaskSchedule::Hourly { minute: 60 }.validate().is_err());
        assert!(AsrTaskSchedule::Weekly {
            weekday: 0,
            hour: 9,
            minute: 0
        }
        .validate()
        .is_err());
        assert!(AsrTaskSchedule::Monthly {
            day: 32,
            hour: 9,
            minute: 0
        }
        .validate()
        .is_err());
    }

    #[test]
    fn summary_keeps_processed_files_after_source_deletion() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("done.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();
        std::fs::remove_file(&audio).unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 0);
        assert_eq!(summary.processed, 1);
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.deleted_after_processing, 1);
    }

    #[test]
    fn summary_counts_failed_files_separately_from_pending() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("bad.wav");
        std::fs::write(&audio, b"bad-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Failed;
        record.error = Some("decode failed".to_string());
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.processed, 0);
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn summary_treats_recreated_same_path_audio_as_new_pending_file() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("same.wav");
        std::fs::write(&audio, b"old-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();

        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(&audio, b"new-audio-with-different-size").unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.processed, 1);
        assert_eq!(summary.pending, 1);
    }

    #[test]
    fn task_detail_includes_file_progress_records() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("done.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task-detail".to_string(),
            name: "Task detail".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        record.text_chars = 12;
        record.output_text_path = Some(temp.path().join("asr/data/text/task-detail/done.txt"));
        record.output_timeline_path = Some(
            temp.path()
                .join("asr/data/text/task-detail/done.timeline.json"),
        );
        store.files.insert(key.clone(), record);
        save_file_store(&task.id, &store).unwrap();

        let detail = task_detail(task);
        assert_eq!(detail.summary.processed, 1);
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].key, key);
        assert_eq!(detail.files[0].record.status, FileStatus::Success);
        assert_eq!(detail.files[0].record.text_chars, 12);
    }

    #[test]
    fn output_paths_are_under_bifrost_asr_text_dir() {
        let temp = TempDir::new().unwrap();
        let (text, metadata, timeline) =
            output_paths_in(temp.path(), "task1", Path::new("/tmp/audio.wav"));
        assert!(text.starts_with(temp.path().join("asr/data/text/task1")));
        assert_eq!(text.extension().and_then(|ext| ext.to_str()), Some("txt"));
        assert_eq!(
            metadata.extension().and_then(|ext| ext.to_str()),
            Some("json")
        );
        assert!(timeline
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .ends_with(".timeline.json"));
    }

    #[test]
    fn task_run_lock_rejects_concurrent_runs_and_releases_after_drop() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());

        let first = TaskRunFileLock::acquire("task1").unwrap();
        let second = TaskRunFileLock::acquire("task1");
        assert!(second.is_err());

        drop(first);
        let third = TaskRunFileLock::acquire("task1");
        assert!(third.is_ok());
    }
}
