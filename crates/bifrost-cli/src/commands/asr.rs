use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bifrost_admin::asr_runtime::{
    clear_service_state, fixed_asr_home, install_dir, model_dir, now_ms, probe_health_blocking,
    read_service_state, stop_pid, write_service_state, AsrServiceState, DEFAULT_ASR_HOST,
};
use bifrost_admin::asr_streaming::{
    append_transcript_delta, call_asr_whole_file_endpoint, dedupe_increment, WholeFileTranscription,
};
use bifrost_admin::resource_download::{download_with_resume, DownloadProgress, DownloadRequest};
use bifrost_core::{BifrostError, Result};
use chrono::{Local, TimeZone};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::cli::{AiAsrCommands, AiAsrTaskCommands, AiAsrTaskDailyCommands, AiCommands};

const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(180);
const ASR_RELEASE_REPO: &str = "second-state/qwen3_asr_rs";
const ASR_SAMPLE_BASE_URL: &str =
    "https://raw.githubusercontent.com/second-state/qwen3_asr_rs/main/test_audio";

pub fn handle_ai_command(action: AiCommands, admin_host: &str, admin_port: u16) -> Result<()> {
    match action {
        AiCommands::Asr { action } => handle_asr_command(action, admin_host, admin_port),
    }
}

fn handle_asr_command(action: AiAsrCommands, admin_host: &str, admin_port: u16) -> Result<()> {
    match action {
        AiAsrCommands::Start { model, language } => {
            ensure_supported_platform()?;
            let state = start_service(&model, &language)?;
            println!(
                "Qwen3-ASR service started: http://{}:{}",
                state.host, state.port
            );
            Ok(())
        }
        AiAsrCommands::Stop => {
            ensure_supported_platform()?;
            stop_service()?;
            println!("Qwen3-ASR service stopped.");
            Ok(())
        }
        AiAsrCommands::Status { json } => {
            ensure_supported_platform()?;
            print_status(json)?;
            Ok(())
        }
        AiAsrCommands::StreamFile {
            audio,
            model,
            language,
            format: _,
        } => {
            ensure_supported_platform()?;
            stream_file(&audio, &model, &language)
        }
        AiAsrCommands::Task { action } => {
            let client = AsrTaskClient::new(admin_host, admin_port);
            handle_asr_task_command(&client, action)
        }
    }
}

#[derive(Debug)]
struct AsrTaskClient {
    base_url: String,
    agent: ureq::Agent,
}

impl AsrTaskClient {
    fn new(host: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}/_bifrost/api", host, port),
            agent: bifrost_core::direct_ureq_agent_builder()
                .timeout(Duration::from_secs(30))
                .build(),
        }
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|error| asr_task_api_error("GET", &url, error))?;
        read_json_response("GET", &url, response)
    }

    fn post_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .post(&url)
            .call()
            .map_err(|error| asr_task_api_error("POST", &url, error))?;
        read_json_response("POST", &url, response)
    }
}

fn read_json_response(method: &str, url: &str, response: ureq::Response) -> Result<Value> {
    let body = response.into_string().map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read ASR task API response from {url}: {error}"
        )))
    })?;
    serde_json::from_str(&body).map_err(|error| {
        BifrostError::Config(format!(
            "{method} {url} returned invalid JSON: {error}; body: {}",
            truncate(&body, 300)
        ))
    })
}

fn asr_task_api_error(method: &str, url: &str, error: ureq::Error) -> BifrostError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            BifrostError::Config(format!(
                "{method} {url} failed with HTTP {status}: {}",
                truncate(&body, 500)
            ))
        }
        other => BifrostError::Config(format!(
            "Failed to connect to Bifrost admin API at {url}\n\
             Is the proxy server running?\n\n\
             Hint: Start the proxy with: bifrost start\n\n\
             Error: {other}"
        )),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AsrTaskSummary {
    #[serde(default)]
    discovered: usize,
    #[serde(default)]
    processed: usize,
    #[serde(default)]
    pending: usize,
    #[serde(default)]
    failed: usize,
    #[serde(default)]
    partial_success: usize,
    #[serde(default)]
    failed_chunk_count: usize,
    #[serde(default)]
    deleted_after_processing: usize,
    #[serde(default)]
    running: bool,
}

#[derive(Debug, Deserialize, Default)]
struct AsrTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    audio_dir: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    schedule: Value,
    #[serde(default)]
    language: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    runtime_strategy: Value,
    #[serde(default)]
    last_run_at_ms: Option<i64>,
    #[serde(default)]
    next_run_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    summary: AsrTaskSummary,
    #[serde(default)]
    files: Vec<AsrTaskFile>,
    #[serde(default)]
    daily_documents: Vec<AsrDailyDocument>,
}

#[derive(Debug, Deserialize, Default)]
struct AsrTaskFile {
    #[serde(default)]
    key: String,
    #[serde(default)]
    source_path: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    source_size: Option<u64>,
    #[serde(default)]
    media_duration_ms: Option<u64>,
    #[serde(default)]
    output_text_path: Option<String>,
    #[serde(default)]
    output_timeline_path: Option<String>,
    #[serde(default)]
    text_chars: Option<usize>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    finished_at_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
struct AsrDailyDocument {
    #[serde(default)]
    date: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_ms: i64,
    #[serde(default)]
    text_chars: usize,
}

#[derive(Debug, Deserialize, Default)]
struct AsrDailyDocumentDetail {
    #[serde(default)]
    content: String,
}

fn handle_asr_task_command(client: &AsrTaskClient, action: AiAsrTaskCommands) -> Result<()> {
    match action {
        AiAsrTaskCommands::List { json } => {
            let value = client.get_json("/asr/tasks")?;
            if json {
                print_json(&value)?;
                return Ok(());
            }
            let tasks = value
                .get("tasks")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let tasks: Vec<AsrTask> = serde_json::from_value(tasks)
                .map_err(|error| BifrostError::Config(format!("parse ASR task list: {error}")))?;
            print_task_list(&tasks);
            Ok(())
        }
        AiAsrTaskCommands::Show { task_id, json } => {
            let value = client.get_json(&format!("/asr/tasks/{}", url_encode(&task_id)))?;
            if json {
                print_json(&value)?;
                return Ok(());
            }
            let task = parse_task(value)?;
            print_task_detail(&task);
            Ok(())
        }
        AiAsrTaskCommands::Files {
            task_id,
            status,
            limit,
            json,
        } => {
            let value = client.get_json(&format!("/asr/tasks/{}", url_encode(&task_id)))?;
            if json {
                print_json(&value)?;
                return Ok(());
            }
            let task = parse_task(value)?;
            print_task_files(&task, status.as_deref(), limit);
            Ok(())
        }
        AiAsrTaskCommands::Run {
            task_id,
            wait,
            json,
        } => run_task(client, &task_id, wait, json),
        AiAsrTaskCommands::Daily { action } => handle_asr_task_daily_command(client, action),
    }
}

fn handle_asr_task_daily_command(
    client: &AsrTaskClient,
    action: AiAsrTaskDailyCommands,
) -> Result<()> {
    match action {
        AiAsrTaskDailyCommands::List { task_id, json } => {
            let value = client.get_json(&format!("/asr/tasks/{}/daily", url_encode(&task_id)))?;
            if json {
                print_json(&value)?;
                return Ok(());
            }
            let documents = value
                .get("documents")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let documents: Vec<AsrDailyDocument> =
                serde_json::from_value(documents).map_err(|error| {
                    BifrostError::Config(format!("parse ASR daily documents: {error}"))
                })?;
            print_daily_documents(&task_id, &documents);
            Ok(())
        }
        AiAsrTaskDailyCommands::Show {
            task_id,
            date,
            output,
            json,
        } => {
            let value = client.get_json(&format!(
                "/asr/tasks/{}/daily/{}",
                url_encode(&task_id),
                url_encode(&date)
            ))?;
            if json {
                print_json(&value)?;
                return Ok(());
            }
            let document: AsrDailyDocumentDetail =
                serde_json::from_value(value).map_err(|error| {
                    BifrostError::Config(format!("parse ASR daily document: {error}"))
                })?;
            if let Some(output) = output {
                fs::write(&output, &document.content).map_err(|error| {
                    BifrostError::Io(io::Error::other(format!(
                        "write ASR daily document {}: {error}",
                        output.display()
                    )))
                })?;
                println!("Wrote {}", output.display());
            } else {
                print!("{}", document.content);
                io::stdout().flush().map_err(BifrostError::Io)?;
            }
            Ok(())
        }
    }
}

fn run_task(client: &AsrTaskClient, task_id: &str, wait: bool, json: bool) -> Result<()> {
    let response = client.post_json(&format!("/asr/tasks/{}/run", url_encode(task_id)))?;
    if json && !wait {
        print_json(&response)?;
        return Ok(());
    }
    if !json {
        if let Some(message) = response.get("message").and_then(Value::as_str) {
            println!("{message}");
        } else {
            println!("ASR directory task started.");
        }
    }
    if !wait {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        thread::sleep(Duration::from_secs(1));
        let value = client.get_json(&format!("/asr/tasks/{}", url_encode(task_id)))?;
        let task = parse_task(value.clone())?;
        if !task.summary.running {
            if json {
                print_json(&value)?;
            } else {
                println!(
                    "ASR task completed: processed={}, failed={}, partial_success={}, daily_documents={}",
                    task.summary.processed,
                    task.summary.failed,
                    task.summary.partial_success,
                    task.daily_documents.len()
                );
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BifrostError::Config(format!(
                "Timed out waiting for ASR task {task_id} to finish"
            )));
        }
    }
}

fn parse_task(value: Value) -> Result<AsrTask> {
    serde_json::from_value(value)
        .map_err(|error| BifrostError::Config(format!("parse ASR task detail: {error}")))
}

fn print_json(value: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|error| BifrostError::Config(error.to_string()))?
    );
    Ok(())
}

fn print_task_list(tasks: &[AsrTask]) {
    if tasks.is_empty() {
        println!("No ASR directory tasks.");
        return;
    }
    println!(
        "{:<34}  {:<24}  {:<10}  {:>5}  {:>5}  {:>5}  NEXT_RUN",
        "ID", "NAME", "STATE", "FILES", "DONE", "PEND"
    );
    for task in tasks {
        println!(
            "{:<34}  {:<24}  {:<10}  {:>5}  {:>5}  {:>5}  {}",
            task.id,
            truncate(&task.name, 24),
            task_state(task),
            task.summary.discovered,
            task.summary.processed,
            task.summary.pending,
            format_optional_ms(task.next_run_at_ms)
        );
    }
}

fn print_task_detail(task: &AsrTask) {
    println!("ID:              {}", task.id);
    println!("Name:            {}", task.name);
    println!("Audio dir:       {}", task.audio_dir);
    println!("State:           {}", task_state(task));
    println!("Enabled:         {}", task.enabled);
    println!("Paused:          {}", task.paused);
    println!("Model:           {}", task.model);
    println!("Language:        {}", task.language);
    println!("Runtime:         {}", format_value(&task.runtime_strategy));
    println!("Schedule:        {}", format_value(&task.schedule));
    println!(
        "Last run:        {}",
        format_optional_ms(task.last_run_at_ms)
    );
    println!(
        "Next run:        {}",
        format_optional_ms(task.next_run_at_ms)
    );
    if let Some(error) = &task.last_error {
        println!("Last error:      {error}");
    }
    println!(
        "Summary:         discovered={}, processed={}, pending={}, failed={}, partial_success={}, failed_chunks={}, deleted_after_processing={}",
        task.summary.discovered,
        task.summary.processed,
        task.summary.pending,
        task.summary.failed,
        task.summary.partial_success,
        task.summary.failed_chunk_count,
        task.summary.deleted_after_processing
    );
    println!("Files:           {}", task.files.len());
    println!("Daily documents: {}", task.daily_documents.len());
    if !task.daily_documents.is_empty() {
        println!();
        print_daily_documents(&task.id, &task.daily_documents);
    }
}

fn print_task_files(task: &AsrTask, status: Option<&str>, limit: usize) {
    let files = task
        .files
        .iter()
        .filter(|file| status.is_none_or(|status| file.status == status))
        .take(limit)
        .collect::<Vec<_>>();
    if files.is_empty() {
        println!("No ASR task files matched.");
        return;
    }
    println!(
        "{:<12}  {:>8}  {:>10}  {:<19}  SOURCE",
        "STATUS", "CHARS", "DURATION", "FINISHED"
    );
    for file in files {
        println!(
            "{:<12}  {:>8}  {:>10}  {:<19}  {}",
            file.status,
            file.text_chars
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            file.media_duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string()),
            format_optional_ms(file.finished_at_ms),
            file.source_path
        );
        if let Some(error) = &file.error {
            println!("  error: {}", truncate(error, 180));
        }
        if let Some(path) = &file.output_text_path {
            println!("  text: {path}");
        }
        if let Some(path) = &file.output_timeline_path {
            println!("  timeline: {path}");
        }
        if !file.key.is_empty() {
            println!("  key: {}", file.key);
        }
        if let Some(size) = file.source_size {
            println!("  source_size: {size}");
        }
    }
}

fn print_daily_documents(task_id: &str, documents: &[AsrDailyDocument]) {
    if documents.is_empty() {
        println!("No daily documents for ASR task {task_id}.");
        return;
    }
    println!(
        "{:<10}  {:>10}  {:>10}  {:<19}  PATH",
        "DATE", "SIZE", "CHARS", "MODIFIED"
    );
    for document in documents {
        println!(
            "{:<10}  {:>10}  {:>10}  {:<19}  {}",
            document.date,
            document.size,
            document.text_chars,
            format_ms(document.modified_ms),
            document.path
        );
    }
}

fn task_state(task: &AsrTask) -> &'static str {
    if task.summary.running {
        "running"
    } else if task.paused {
        "paused"
    } else if !task.enabled {
        "disabled"
    } else {
        "enabled"
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "-".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "-".to_string()),
    }
}

fn format_optional_ms(value: Option<i64>) -> String {
    value.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_ms(value: i64) -> String {
    Local
        .timestamp_millis_opt(value)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| value.to_string())
}

fn format_duration_ms(value: u64) -> String {
    let seconds = value / 1000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn url_encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn ensure_supported_platform() -> Result<()> {
    if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "Qwen3-ASR local runtime is only supported on Apple Silicon macOS; current platform is {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )))
    }
}

fn print_status(json: bool) -> Result<()> {
    let state = read_service_state(&bifrost_storage::data_dir());
    let ready = state
        .as_ref()
        .map(|state| probe_health_blocking(&state.host, state.port, Duration::from_secs(2)).is_ok())
        .unwrap_or(false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ready": ready,
                "service": state,
            }))
            .map_err(|error| BifrostError::Config(error.to_string()))?
        );
    } else if let Some(state) = state {
        println!("ready: {ready}");
        println!("server: http://{}:{}", state.host, state.port);
        println!("model: {}", state.model);
        println!("language: {}", state.language);
        println!("managed_by: {}", state.managed_by);
    } else {
        println!("ready: false");
        println!("server: not running");
    }
    Ok(())
}

/// Max seconds for a single native ASR inference.
///
/// The native qwen3_asr_rs CLI can spike Metal/MLX physical footprint on larger
/// windows. Keep this aligned with the WebUI scheduled-task default, which has
/// been the best-performing and most stable window in real 1.7B runs.
const CHUNK_DURATION_SECS: u64 = 30;
/// Overlap between consecutive chunks to avoid cutting words at boundaries.
const CHUNK_OVERLAP_SECS: u64 = 2;
/// Number of concurrent ASR inference workers. The current `asr-server`
/// supports only one in-flight inference (model + GPU are a single shared
/// resource), so concurrency=1 is the optimum — extra HTTP-level concurrency
/// only adds queueing overhead. Kept as a tunable for future multi-instance
/// deployments via `BIFROST_ASR_CONCURRENCY`.
const DEFAULT_CONCURRENCY: usize = 1;
/// Minimum silence duration (seconds) before a region qualifies for skipping.
/// 2.5s is longer than any natural conversational pause, hesitation, or
/// breath, so we never mistake those for "silence".
const VAD_MIN_SILENCE_SECS: f64 = 2.5;
/// Lower clamp for adaptive silence threshold (dBFS). Even in very quiet
/// recordings, we never call anything louder than -55dB "silence" — that's
/// noise-floor territory and could still contain the faintest speech.
const VAD_THRESHOLD_FLOOR_DB: f64 = -55.0;
/// Upper clamp for adaptive silence threshold (dBFS). Even in noisy
/// recordings, we never claim anything louder than -30dB is "silence" —
/// that's already conversational speech volume.
const VAD_THRESHOLD_CEIL_DB: f64 = -30.0;
/// Margin below `mean_volume` used to derive the silence threshold.
/// `threshold = clamp(mean_volume - MARGIN, FLOOR, CEIL)`.
/// 8dB is conservative: real speech sits ~10–20dB above the recording's
/// mean RMS, so this margin lands the threshold safely below typical speech
/// while still capturing genuine dead air and persistent low-level hiss.
const VAD_THRESHOLD_MARGIN_DB: f64 = 8.0;
/// Safety margin (seconds) shrunk off either end of every detected silent
/// region before deciding a chunk is "fully inside" silence. This guarantees
/// we never accidentally skip a chunk whose edges contain speech onsets/offsets.
const VAD_CHUNK_SAFETY_MARGIN_SECS: f64 = 1.0;

fn stream_file(audio: &Path, model: &str, language: &str) -> Result<()> {
    if !audio.is_file() {
        return Err(BifrostError::Config(format!(
            "audio file does not exist: {}",
            audio.display()
        )));
    }

    let home = fixed_asr_home();
    let install = install_dir(&home);
    let model_path = model_dir(&home, model);
    prepare_cli_assets(&home, model)?;
    ensure_ffmpeg_for_cli()?;
    if !install.join("asr-server").is_file() || !model_path.join("tokenizer.json").is_file() {
        return Err(BifrostError::Config(format!(
            "Qwen3-ASR assets are still missing under {} after self-check.",
            install.display()
        )));
    }

    let duration_secs = probe_duration_secs(audio);
    let origin_epoch = probe_recording_origin(audio);
    if let Some(epoch) = origin_epoch {
        let dt = format_datetime(epoch);
        eprintln!("Recording origin: {dt}");
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| BifrostError::Io(io::Error::other(format!("create tokio runtime: {e}"))))?;

    let existing_state = healthy_state(model, language);
    let service_state = start_service(model, language)?;
    let stop_after_use = existing_state.is_none();
    let _service_guard = StreamFileServiceGuard { stop_after_use };
    let server_url = service_state_url(&service_state);
    eprintln!(
        "Qwen3-ASR service: {server_url} (runtime=reuse_per_file, stop_after_use={stop_after_use})"
    );

    if duration_secs > CHUNK_DURATION_SECS {
        eprintln!("Audio is {duration_secs}s (>{CHUNK_DURATION_SECS}s), splitting into chunks...");
        runtime.block_on(transcribe_chunked(
            audio,
            language,
            duration_secs,
            origin_epoch,
            &server_url,
        ))
    } else {
        runtime.block_on(transcribe_single(
            audio,
            language,
            duration_secs,
            origin_epoch,
            &server_url,
        ))
    }
}

struct StreamFileServiceGuard {
    stop_after_use: bool,
}

impl Drop for StreamFileServiceGuard {
    fn drop(&mut self) {
        if self.stop_after_use {
            let _ = stop_service();
        }
    }
}

fn service_state_url(state: &AsrServiceState) -> String {
    if state.host == "::1" {
        format!("http://[::1]:{}", state.port)
    } else {
        format!("http://{}:{}", state.host, state.port)
    }
}

/// Format seconds as HH:MM:SS.mmm for subtitle display.
fn format_timestamp(secs: f64) -> String {
    let total_ms = (secs * 1000.0) as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Format a Unix epoch (seconds) as "YYYY-MM-DD HH:MM:SS".
fn format_datetime(epoch_secs: i64) -> String {
    let secs = epoch_secs % 86400;
    let days = epoch_secs / 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    // Simple days-since-epoch to date (good enough for 2000-2099)
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Convert days since Unix epoch to (year, month, day).
fn epoch_days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// Format an absolute timestamp: origin_epoch + offset_secs.
fn format_absolute_time(origin_epoch: i64, offset_secs: f64) -> String {
    format_datetime(origin_epoch + offset_secs as i64)
}

/// Run ASR on the whole file via the file-scoped `asr-server` (short audio, no chunking).
async fn transcribe_single(
    audio: &Path,
    language: &str,
    duration_secs: u64,
    origin_epoch: Option<i64>,
    server_url: &str,
) -> Result<()> {
    // Normalize first so the CLI gets a small, predictable WAV body.
    let temp_dir = tempfile::tempdir()
        .map_err(|e| BifrostError::Io(io::Error::other(format!("create temp dir: {e}"))))?;
    let normalized = temp_dir.path().join("normalized.wav");
    normalize_audio(audio, &normalized)?;

    let result = call_asr_whole_file_endpoint(
        server_url,
        language,
        &normalized,
        Some(duration_secs * 1000),
    )
    .await
    .map_err(BifrostError::Config)?;

    emit_segments(&result, 0, origin_epoch);

    let mut fin = serde_json::json!({
        "type": "final",
        "text": result.text,
        "duration_ms": duration_secs * 1000,
        "segments": result.segments.len().max(1),
    });
    if let Some(epoch) = origin_epoch {
        fin["recording_origin"] = serde_json::json!(format_datetime(epoch));
    }
    println!("{fin}");
    Ok(())
}

/// Emit verbose_json segments to stdout, optionally shifted by `chunk_offset_ms`
/// (for chunked mode) and annotated with absolute time when origin is known.
fn emit_segments(result: &WholeFileTranscription, chunk_offset_ms: u64, origin_epoch: Option<i64>) {
    if result.segments.is_empty() && !result.text.is_empty() {
        // Server returned plain text only; emit a single segment.
        let mut seg = serde_json::json!({
            "type": "segment",
            "start_ms": chunk_offset_ms,
            "end_ms": chunk_offset_ms,
            "start": format_timestamp(chunk_offset_ms as f64 / 1000.0),
            "end": format_timestamp(chunk_offset_ms as f64 / 1000.0),
            "text": result.text,
        });
        if let Some(epoch) = origin_epoch {
            let off = chunk_offset_ms as f64 / 1000.0;
            seg["time"] = serde_json::json!(format_absolute_time(epoch, off));
            seg["time_end"] = serde_json::json!(format_absolute_time(epoch, off));
        }
        println!("{seg}");
        return;
    }
    for (start_ms, end_ms, text) in &result.segments {
        let abs_start_ms = start_ms + chunk_offset_ms;
        let abs_end_ms = end_ms + chunk_offset_ms;
        let mut seg = serde_json::json!({
            "type": "segment",
            "start_ms": abs_start_ms,
            "end_ms": abs_end_ms,
            "start": format_timestamp(abs_start_ms as f64 / 1000.0),
            "end": format_timestamp(abs_end_ms as f64 / 1000.0),
            "text": text,
        });
        if let Some(epoch) = origin_epoch {
            seg["time"] =
                serde_json::json!(format_absolute_time(epoch, abs_start_ms as f64 / 1000.0));
            seg["time_end"] =
                serde_json::json!(format_absolute_time(epoch, abs_end_ms as f64 / 1000.0));
        }
        println!("{seg}");
    }
}

/// Split long audio into chunks, transcribe in parallel, and stream results
/// in chunk-index order with absolute timeline info.
///
/// Timeline contract: the *original* timeline is preserved end-to-end.
/// We never collapse silent regions in the audio itself; we only *detect*
/// them via `silencedetect` and skip chunks that fall entirely inside a
/// long silent region. Speech chunks keep their exact original offsets.
async fn transcribe_chunked(
    audio: &Path,
    language: &str,
    total_secs: u64,
    origin_epoch: Option<i64>,
    server_url: &str,
) -> Result<()> {
    let temp_dir = tempfile::tempdir()
        .map_err(|e| BifrostError::Io(io::Error::other(format!("create temp dir: {e}"))))?;

    // Step 1: Normalize ONLY (no timeline-altering filters). This guarantees
    // every chunk's offset matches the original wall-clock recording time.
    let normalized = temp_dir.path().join("normalized.wav");
    eprintln!("Normalizing audio (16kHz mono 16-bit PCM, timeline preserved)...");
    let norm_start = Instant::now();
    normalize_audio(audio, &normalized)?;
    let norm_elapsed = norm_start.elapsed().as_secs_f64();
    eprintln!("Normalized in {:.2}s", norm_elapsed);

    // Step 2: Detect silent regions using an *adaptive* threshold derived
    // from the recording's own loudness statistics, then run silencedetect.
    // The audio file is NOT modified — we only learn *where* dead air is.
    //
    // Why adaptive: a fixed dB threshold is too rigid — a noisy office
    // recording (mean ≈ -25dB) needs a much higher threshold than a clean
    // studio recording (mean ≈ -45dB), or you'll either fail to skip dead
    // air or wrongly skip quiet speech. We compute mean_volume once via
    // ffmpeg `volumedetect`, then set:
    //     threshold = clamp(mean_volume - MARGIN_DB, FLOOR_DB, CEIL_DB)
    // This matches what mature speech pipelines (whisper.cpp, faster-whisper)
    // do as their non-ML VAD baseline.
    let (mean_db, max_db) = probe_volume_stats(&normalized).unwrap_or((None, None));
    let threshold_db = compute_silence_threshold(mean_db);
    if let (Some(m), Some(mx)) = (mean_db, max_db) {
        eprintln!(
            "VAD calibration: mean_volume={:.1}dB, max_volume={:.1}dB → adaptive threshold={:.1}dB",
            m, mx, threshold_db
        );
    } else {
        eprintln!(
            "VAD calibration: volumedetect failed; using floor threshold={:.1}dB",
            threshold_db
        );
    }
    let silent_ranges = detect_silent_ranges(&normalized, threshold_db).unwrap_or_else(|err| {
        eprintln!("VAD silencedetect skipped ({err}); proceeding without skip-list");
        Vec::new()
    });
    let total_silent_secs: f64 = silent_ranges.iter().map(|(s, e)| (e - s).max(0.0)).sum();
    if !silent_ranges.is_empty() {
        eprintln!(
            "VAD: {} silent region(s), total {:.1}s of dead air detected (threshold={:.1}dB, \
             min_silence={}s) — chunks fully inside these regions will be skipped",
            silent_ranges.len(),
            total_silent_secs,
            threshold_db,
            VAD_MIN_SILENCE_SECS
        );
    }

    // Step 3: Compute chunk boundaries over the *original* timeline.
    let chunk_secs = CHUNK_DURATION_SECS;
    let overlap_secs = CHUNK_OVERLAP_SECS;
    let step_secs = chunk_secs.saturating_sub(overlap_secs).max(1);
    let mut boundaries: Vec<(u64, u64)> = Vec::new();
    let mut offset = 0u64;
    while offset < total_secs {
        let remaining = total_secs - offset;
        let this_chunk = remaining.min(chunk_secs);
        boundaries.push((offset, this_chunk));
        offset += step_secs;
        if offset < total_secs && (total_secs - offset) <= overlap_secs {
            let last = boundaries.last_mut().unwrap();
            last.1 = total_secs - last.0;
            break;
        }
    }
    let total_chunks = boundaries.len();
    // Concurrency reserved for future use. The current `asr-server` serializes
    // inference against one loaded model, so parallel HTTP requests mostly add
    // queueing overhead. We default to 1 (sequential). The env override lets
    // power users experiment without recompiling.
    let concurrency = std::env::var("BIFROST_ASR_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONCURRENCY)
        .max(1);

    // Determine which chunks fall entirely inside a detected silent region
    // (with a small safety margin so we never cut off speech onsets/offsets).
    let chunk_skipped: Vec<bool> = boundaries
        .iter()
        .map(|&(start, dur)| {
            let cs = start as f64;
            let ce = (start + dur) as f64;
            silent_ranges.iter().any(|&(ss, se)| {
                ss + VAD_CHUNK_SAFETY_MARGIN_SECS <= cs && ce <= se - VAD_CHUNK_SAFETY_MARGIN_SECS
            })
        })
        .collect();
    let skipped_count = chunk_skipped.iter().filter(|&&b| b).count();
    eprintln!(
        "Split into {total_chunks} chunks ({chunk_secs}s each, {overlap_secs}s overlap), \
         concurrency={concurrency}, skipping {skipped_count} silent chunk(s)"
    );

    // Step 4: Extract only the *non-skipped* chunks via ffmpeg -c copy in parallel.
    let chunk_paths: Vec<PathBuf> = (0..total_chunks)
        .map(|i| temp_dir.path().join(format!("chunk_{i:04}.wav")))
        .collect();
    let extract_handles: Vec<_> = boundaries
        .iter()
        .zip(chunk_paths.iter())
        .zip(chunk_skipped.iter())
        .filter_map(|((&(start, duration), out), &skip)| {
            if skip {
                return None;
            }
            let src = normalized.clone();
            let out = out.clone();
            Some(tokio::task::spawn_blocking(move || {
                ffmpeg_extract_chunk(&src, &out, start, duration)
            }))
        })
        .collect();
    for h in extract_handles {
        h.await.map_err(|e| {
            BifrostError::Io(io::Error::other(format!("chunk extract join: {e}")))
        })??;
    }

    // Step 5: Sequential ASR inference through one file-scoped `asr-server`.
    // Current local benchmarks show this beats fork-per-chunk on the same
    // long file while still releasing the server when `stream-file` exits.
    let overall_start = Instant::now();
    let mut all_text = String::new();

    for (idx, ((&(chunk_offset_secs, this_chunk_secs), chunk_path), &was_skipped)) in boundaries
        .iter()
        .zip(chunk_paths.iter())
        .zip(chunk_skipped.iter())
        .enumerate()
    {
        let chunk_offset_ms = chunk_offset_secs * 1000;
        if was_skipped {
            eprintln!(
                "Chunk {}/{}: {:.1}s audio → skipped (silent)",
                idx + 1,
                total_chunks,
                this_chunk_secs
            );
            continue;
        }

        let started = Instant::now();
        let result = call_asr_whole_file_endpoint(
            server_url,
            language,
            chunk_path,
            Some(this_chunk_secs * 1000),
        )
        .await
        .map_err(BifrostError::Config)?;
        let elapsed = started.elapsed().as_secs_f64();
        let chars: usize = result
            .segments
            .iter()
            .map(|(_, _, t)| t.chars().count())
            .sum::<usize>()
            .max(result.text.chars().count());
        eprintln!(
            "Chunk {}/{}: {:.1}s audio → {:.1}s inference, {} chars",
            idx + 1,
            total_chunks,
            this_chunk_secs,
            elapsed,
            chars
        );

        // Deduplicate transcript prefix for the rolling text.
        let chunk_text = result.text.clone();
        if !chunk_text.is_empty() {
            if overlap_secs > 0 && !all_text.is_empty() {
                let d = dedupe_increment(&all_text, &chunk_text);
                if !d.is_empty() {
                    append_transcript_delta(&mut all_text, &d);
                }
            } else if !all_text.is_empty() {
                all_text.push('\n');
                all_text.push_str(&chunk_text);
            } else {
                all_text.push_str(&chunk_text);
            }
        }

        emit_segments(&result, chunk_offset_ms, origin_epoch);
    }

    let total_elapsed = overall_start.elapsed().as_secs_f64();
    let rtf = total_elapsed / total_secs as f64;
    eprintln!(
        "Done: {total_secs}s audio → {total_chunks} chunks ({} skipped) → {:.1}s total, \
         RTF={:.3} ({:.1}x realtime, {} concurrent workers)",
        skipped_count,
        total_elapsed,
        rtf,
        if rtf > 0.0 { 1.0 / rtf } else { 0.0 },
        concurrency
    );

    let mut fin = serde_json::json!({
        "type": "final",
        "text": all_text,
        "duration_ms": total_secs * 1000,
        "segments": total_chunks,
        "skipped_silent_chunks": skipped_count,
        "elapsed_secs": format!("{:.1}", total_elapsed),
        "rtf": format!("{:.3}", rtf),
        "concurrency": concurrency,
    });
    if let Some(epoch) = origin_epoch {
        fin["recording_origin"] = serde_json::json!(format_datetime(epoch));
    }
    println!("{fin}");

    Ok(())
}

/// Probe the recording's mean and max volume in dBFS via ffmpeg `volumedetect`.
/// Returns `(mean_dB, max_dB)` when available. ffmpeg's volumedetect computes
/// these over the whole stream in a single pass, which is fast (constant
/// memory, ~real-time-or-better on Apple Silicon).
fn probe_volume_stats(audio: &Path) -> std::result::Result<(Option<f64>, Option<f64>), String> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "info"])
        .arg("-i")
        .arg(audio)
        .args(["-af", "volumedetect", "-f", "null", "-"])
        .output()
        .map_err(|e| format!("run ffmpeg volumedetect: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut mean_db: Option<f64> = None;
    let mut max_db: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(rest) = line.split("mean_volume:").nth(1) {
            mean_db = parse_db_value(rest);
        } else if let Some(rest) = line.split("max_volume:").nth(1) {
            max_db = parse_db_value(rest);
        }
    }
    Ok((mean_db, max_db))
}

fn parse_db_value(s: &str) -> Option<f64> {
    // ffmpeg prints "mean_volume: -27.4 dB"
    let mut parts = s.split_whitespace();
    parts.next().and_then(|v| v.parse::<f64>().ok())
}

/// Derive an adaptive silence threshold from the recording's mean RMS.
///
/// Falls back to `VAD_THRESHOLD_FLOOR_DB` when mean is unknown — this is the
/// conservative choice (low threshold ⇒ less aggressive skipping ⇒ no risk
/// of dropping speech).
fn compute_silence_threshold(mean_db: Option<f64>) -> f64 {
    let raw = match mean_db {
        Some(m) => m - VAD_THRESHOLD_MARGIN_DB,
        None => VAD_THRESHOLD_FLOOR_DB,
    };
    raw.clamp(VAD_THRESHOLD_FLOOR_DB, VAD_THRESHOLD_CEIL_DB)
}

/// Run ffmpeg silencedetect over the audio and return a list of
/// `(start_secs, end_secs)` ranges that are entirely "silent" by
/// `threshold_db` for at least `VAD_MIN_SILENCE_SECS` seconds.
///
/// Critically, this does NOT modify the audio file. The audio's timeline is
/// preserved; we only return *where* the silence is so callers can decide
/// whether to skip ASR work for those regions.
fn detect_silent_ranges(
    audio: &Path,
    threshold_db: f64,
) -> std::result::Result<Vec<(f64, f64)>, String> {
    // ffmpeg silencedetect writes to stderr lines like:
    //   [silencedetect @ 0x...] silence_start: 12.345
    //   [silencedetect @ 0x...] silence_end: 18.901 | silence_duration: 6.556
    let filter = format!(
        "silencedetect=noise={:.1}dB:d={}",
        threshold_db, VAD_MIN_SILENCE_SECS
    );
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "info"])
        .arg("-i")
        .arg(audio)
        .args(["-af", &filter, "-f", "null", "-"])
        .output()
        .map_err(|e| format!("run ffmpeg silencedetect: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut ranges = Vec::new();
    let mut current_start: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(idx) = line.find("silence_start:") {
            let rest = &line[idx + "silence_start:".len()..];
            if let Some(val) = rest.split_whitespace().next() {
                if let Ok(s) = val.parse::<f64>() {
                    current_start = Some(s);
                }
            }
        } else if let Some(idx) = line.find("silence_end:") {
            let rest = &line[idx + "silence_end:".len()..];
            if let Some(val) = rest.split_whitespace().next() {
                if let Ok(e) = val.parse::<f64>() {
                    if let Some(s) = current_start.take() {
                        if e > s {
                            ranges.push((s, e));
                        }
                    }
                }
            }
        }
    }
    Ok(ranges)
}

/// Extract a chunk from an already-normalized WAV with `-c copy` (zero-transcode).
fn ffmpeg_extract_chunk(src: &Path, out: &Path, start: u64, duration: u64) -> Result<()> {
    let r = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-ss")
        .arg(start.to_string())
        .arg("-i")
        .arg(src)
        .arg("-t")
        .arg(duration.to_string())
        .args(["-c", "copy"])
        .arg(out)
        .output()
        .map_err(|e| BifrostError::Io(io::Error::other(format!("ffmpeg chunk: {e}"))))?;
    if !r.status.success() {
        return Err(BifrostError::Config(format!(
            "ffmpeg chunk extraction failed: {}",
            String::from_utf8_lossy(&r.stderr)
        )));
    }
    Ok(())
}

/// Try to extract the recording origin timestamp from audio metadata.
/// Looks for WAV bext `date` + `creation_time` tags via ffprobe, then falls
/// back to filename timestamp patterns (e.g. `TX01_MIC004_20260514_170241_orig.wav`).
fn probe_recording_origin(audio: &Path) -> Option<i64> {
    // 1. Try ffprobe bext metadata
    if let Some(epoch) = probe_bext_origin(audio) {
        return Some(epoch);
    }
    // 2. Fallback: parse timestamp from filename (consistent with WebUI)
    parse_filename_origin(audio)
}

fn probe_bext_origin(audio: &Path) -> Option<i64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format_tags=date,creation_time",
            "-of",
            "json",
        ])
        .arg(audio)
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let tags = json.get("format")?.get("tags")?;

    // Try bext-style: date = "2026-05-14", creation_time = "17:02:41"
    let date_str = tags.get("date").and_then(|v| v.as_str())?;
    let time_str = tags
        .get("creation_time")
        .and_then(|v| v.as_str())
        .unwrap_or("00:00:00");

    parse_datetime_to_epoch(date_str, time_str)
}

/// Parse timestamp from filename like `TX01_MIC004_20260514_170241_orig.wav`.
/// Looks for consecutive `_YYYYMMDD_HHMMSS_` pattern.
fn parse_filename_origin(audio: &Path) -> Option<i64> {
    let filename = audio.file_name()?.to_str()?;
    let parts: Vec<&str> = filename.split('_').collect();
    for window in parts.windows(2) {
        let date_part = window[0];
        let time_part = window[1];
        if date_part.len() == 8
            && time_part.len() >= 6
            && date_part.chars().all(|c| c.is_ascii_digit())
            && time_part[..6].chars().all(|c| c.is_ascii_digit())
        {
            let y: i32 = date_part[..4].parse().ok()?;
            let m: u32 = date_part[4..6].parse().ok()?;
            let d: u32 = date_part[6..8].parse().ok()?;
            let hh: i64 = time_part[..2].parse().ok()?;
            let mm: i64 = time_part[2..4].parse().ok()?;
            let ss: i64 = time_part[4..6].parse().ok()?;
            let days = ymd_to_epoch_days(y, m, d);
            return Some(days * 86400 + hh * 3600 + mm * 60 + ss);
        }
    }
    None
}

/// Parse "YYYY-MM-DD" + "HH:MM:SS" into Unix epoch seconds.
fn parse_datetime_to_epoch(date: &str, time: &str) -> Option<i64> {
    let mut parts = date.splitn(3, '-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;

    let mut tparts = time.splitn(3, ':');
    let hh: i64 = tparts.next()?.parse().ok()?;
    let mm: i64 = tparts.next()?.parse().ok()?;
    // Handle fractional seconds like "17:02:41.123"
    let ss_str = tparts.next().unwrap_or("0");
    let ss: i64 = ss_str.split('.').next()?.parse().ok()?;

    // Convert date to days since epoch using inverse of epoch_days_to_ymd
    let days = ymd_to_epoch_days(y, m, d);
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// Convert (year, month, day) to days since Unix epoch.
fn ymd_to_epoch_days(y: i32, m: u32, d: u32) -> i64 {
    // Inverse of Howard Hinnant's algorithm
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Get audio duration in seconds via ffprobe. Returns 0 on failure.
fn probe_duration_secs(audio: &Path) -> u64 {
    Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(audio)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .map(|d| d.ceil() as u64)
        .unwrap_or(0)
}

/// Normalize audio to 16kHz mono 16-bit PCM WAV using ffmpeg.
///
/// IMPORTANT: this does NOT modify the timeline (no silenceremove, no atrim,
/// no atempo). Every sample stays at its original wall-clock offset, which is
/// what `transcribe_chunked` relies on when emitting absolute timestamps.
fn normalize_audio(input: &Path, output: &Path) -> Result<()> {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-i")
        .arg(input)
        .args(["-ac", "1", "-ar", "16000", "-sample_fmt", "s16"])
        .arg(output)
        .output()
        .map_err(|e| BifrostError::Io(io::Error::other(format!("ffmpeg normalize: {e}"))))?;
    if !result.status.success() {
        return Err(BifrostError::Config(format!(
            "ffmpeg normalize failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    Ok(())
}

fn start_service(model: &str, language: &str) -> Result<AsrServiceState> {
    if let Some(state) = healthy_state(model, language) {
        return Ok(state);
    }

    let home = fixed_asr_home();
    let install = install_dir(&home);
    let model_path = model_dir(&home, model);
    prepare_cli_assets(&home, model)?;
    ensure_ffmpeg_for_cli()?;

    if !install.join("asr-server").is_file() || !model_path.join("tokenizer.json").is_file() {
        return Err(BifrostError::Config(format!(
            "Qwen3-ASR assets are still missing under {} after self-check.",
            install.display()
        )));
    }

    let port = allocate_port()?;
    let log_path = bifrost_storage::data_dir().join("asr/asr-server.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create ASR log dir: {error}"
            )))
        })?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("open ASR log: {error}")))
        })?;
    let stderr = stdout.try_clone().map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!("clone ASR log: {error}")))
    })?;

    let child = Command::new(install.join("asr-server"))
        .arg("--model-dir")
        .arg(model_path)
        .arg("--host")
        .arg(DEFAULT_ASR_HOST)
        .arg("--port")
        .arg(port.to_string())
        .arg("--language")
        .arg(language)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("start ASR server: {error}")))
        })?;

    let state = AsrServiceState {
        host: DEFAULT_ASR_HOST.to_string(),
        port,
        model: model.to_string(),
        language: language.to_string(),
        home,
        pid: Some(child.id()),
        managed_by: "cli".to_string(),
        started_at_ms: now_ms(),
    };
    write_service_state(&bifrost_storage::data_dir(), &state).map_err(BifrostError::Config)?;

    let deadline = Instant::now() + SERVICE_START_TIMEOUT;
    while Instant::now() < deadline {
        if probe_health_blocking(DEFAULT_ASR_HOST, port, Duration::from_secs(2)).is_ok() {
            return Ok(state);
        }
        thread::sleep(Duration::from_secs(1));
    }

    let _ = stop_service();
    Err(BifrostError::Config(format!(
        "Timed out waiting for Qwen3-ASR service to become healthy. Log: {}",
        log_path.display()
    )))
}

fn stop_service() -> Result<()> {
    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if let Some(pid) = state.pid {
            let _ = stop_pid(pid);
        }
    }
    clear_service_state(&bifrost_storage::data_dir()).map_err(BifrostError::Config)
}

fn healthy_state(model: &str, language: &str) -> Option<AsrServiceState> {
    let state = read_service_state(&bifrost_storage::data_dir())?;
    if state.model != model || state.language != language {
        return None;
    }
    probe_health_blocking(&state.host, state.port, Duration::from_secs(2))
        .is_ok()
        .then_some(state)
}

fn allocate_port() -> Result<u16> {
    TcpListener::bind((DEFAULT_ASR_HOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("allocate ASR port: {error}")))
        })
}

fn prepare_cli_assets(home: &Path, model: &str) -> Result<()> {
    if cli_assets_installed(home, model) {
        return Ok(());
    }
    eprintln!("Qwen3-ASR self-check is repairing missing runtime or model assets.");
    let runtime = tokio::runtime::Runtime::new().map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "create ASR download runtime: {error}"
        )))
    })?;
    runtime.block_on(download_cli_assets(home.to_path_buf(), model.to_string()))?;
    install_cli_release(home)?;
    prepare_cli_model(home, model)?;
    Ok(())
}

async fn download_cli_assets(home: PathBuf, model: String) -> Result<()> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| BifrostError::Config(format!("build ASR downloader client: {error}")))?;
    let requests = cli_download_requests(&home, &model)?;
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let progress_task = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            if progress.complete {
                eprintln!("downloaded {}", progress.label);
            } else if let Some(percent) = progress.percent {
                eprintln!("downloading {}: {}%", progress.label, percent);
            } else {
                eprintln!(
                    "downloading {}: {} bytes",
                    progress.label, progress.downloaded_bytes
                );
            }
        }
    });
    for request in requests {
        download_with_resume(&client, request, Some(progress_tx.clone()))
            .await
            .map_err(BifrostError::Config)?;
    }
    drop(progress_tx);
    let _ = progress_task.await;
    Ok(())
}

fn cli_download_requests(home: &Path, model: &str) -> Result<Vec<DownloadRequest>> {
    let mut requests = Vec::new();
    let install = install_dir(home);
    if !install.join("asr").is_file() || !install.join("asr-server").is_file() {
        let asset = detect_asr_release_asset()?;
        requests.push(DownloadRequest {
            url: format!(
                "https://github.com/{ASR_RELEASE_REPO}/releases/latest/download/{asset}.zip"
            ),
            dest: home.join(format!("{asset}.zip")),
            label: format!("{asset}.zip"),
        });
    }
    for file in required_model_files(model) {
        let dest = model_dir(home, model).join(file);
        if !dest.is_file() {
            requests.push(DownloadRequest {
                url: format!("https://huggingface.co/Qwen/{model}/resolve/main/{file}"),
                dest,
                label: format!("{model}/{file}"),
            });
        }
    }
    for sample in [
        "sample1.wav",
        "sample1.txt",
        "sample2.wav",
        "sample2.txt",
        "sample3.wav",
        "sample3.txt",
    ] {
        let dest = install.join(sample);
        if !dest.is_file() {
            requests.push(DownloadRequest {
                url: format!("{ASR_SAMPLE_BASE_URL}/{sample}"),
                dest,
                label: sample.to_string(),
            });
        }
    }
    Ok(requests)
}

fn install_cli_release(home: &Path) -> Result<()> {
    let install = install_dir(home);
    if install.join("asr").is_file() && install.join("asr-server").is_file() {
        return Ok(());
    }
    let asset = detect_asr_release_asset()?;
    let zip_path = home.join(format!("{asset}.zip"));
    let extracted = home.join(asset);
    extract_zip_to_dir(&zip_path, home)?;
    fs::create_dir_all(&install).map_err(|error| {
        BifrostError::Io(io::Error::other(format!("create ASR install dir: {error}")))
    })?;
    copy_dir_contents(&extracted, &install)?;
    let _ = fs::remove_dir_all(&extracted);
    let _ = fs::remove_file(&zip_path);
    mark_cli_binaries_executable(&install)?;
    Ok(())
}

fn prepare_cli_model(home: &Path, model: &str) -> Result<()> {
    let model_path = model_dir(home, model);
    fs::create_dir_all(&model_path).map_err(|error| {
        BifrostError::Io(io::Error::other(format!("create ASR model dir: {error}")))
    })?;
    for file in required_model_files(model) {
        let path = model_path.join(file);
        if !path.is_file() {
            return Err(BifrostError::Config(format!(
                "missing ASR model file after download: {}",
                path.display()
            )));
        }
    }
    let tokenizer_src = install_dir(home)
        .join("tokenizers")
        .join(format!("tokenizer-{}.json", tokenizer_size(model)?));
    fs::copy(&tokenizer_src, model_path.join("tokenizer.json")).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "copy ASR tokenizer {}: {error}",
            tokenizer_src.display()
        )))
    })?;
    Ok(())
}

fn ensure_ffmpeg_for_cli() -> Result<()> {
    if command_succeeds("ffmpeg", &["-version"]) {
        return Ok(());
    }
    if !command_succeeds("brew", &["--version"]) {
        return Err(BifrostError::Config(
            "ffmpeg is required for ASR audio preprocessing, and Homebrew was not found to install it automatically. Install Homebrew and run `brew install ffmpeg`, then retry the same ASR command."
                .to_string(),
        ));
    }
    eprintln!("Qwen3-ASR self-check is installing ffmpeg with Homebrew.");
    let output = Command::new("brew")
        .arg("install")
        .arg("ffmpeg")
        .output()
        .map_err(|error| BifrostError::Io(io::Error::other(format!("run brew: {error}"))))?;
    if output.status.success() && command_succeeds("ffmpeg", &["-version"]) {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "Homebrew ffmpeg installation failed with {}. Install it manually with `brew install ffmpeg`, then retry the same ASR command. {}",
            output.status,
            summarize_command_output(&output.stdout, &output.stderr)
        )))
    }
}

fn cli_assets_installed(home: &Path, model: &str) -> bool {
    install_dir(home).join("asr").is_file()
        && install_dir(home).join("asr-server").is_file()
        && model_dir(home, model).join("tokenizer.json").is_file()
        && required_model_files(model)
            .iter()
            .all(|file| model_dir(home, model).join(file).is_file())
}

fn detect_asr_release_asset() -> Result<&'static str> {
    ensure_supported_platform()?;
    Ok("asr-macos-aarch64")
}

fn required_model_files(model: &str) -> &'static [&'static str] {
    match model {
        "Qwen3-ASR-0.6B" => &["config.json", "model.safetensors"],
        "Qwen3-ASR-1.7B" => &[
            "config.json",
            "model.safetensors.index.json",
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
        _ => &["config.json"],
    }
}

fn tokenizer_size(model: &str) -> Result<&'static str> {
    match model {
        "Qwen3-ASR-0.6B" => Ok("0.6B"),
        "Qwen3-ASR-1.7B" => Ok("1.7B"),
        _ => Err(BifrostError::Config(format!(
            "unsupported ASR model: {model}"
        ))),
    }
}

fn extract_zip_to_dir(zip_path: &Path, dest: &Path) -> Result<()> {
    let zip_path = zip_path.to_path_buf();
    let dest = dest.to_path_buf();
    let file = fs::File::open(&zip_path).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "open ASR release zip {}: {error}",
            zip_path.display()
        )))
    })?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| BifrostError::Config(format!("read ASR release zip: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| BifrostError::Config(format!("read zip entry: {error}")))?;
        let Some(enclosed) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let output = dest.join(enclosed);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR unzip dir {}: {error}",
                    output.display()
                )))
            })?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR unzip parent {}: {error}",
                    parent.display()
                )))
            })?;
        }
        let mut out = fs::File::create(&output).map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "create ASR unzip file {}: {error}",
                output.display()
            )))
        })?;
        io::copy(&mut entry, &mut out).map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "extract ASR unzip file {}: {error}",
                output.display()
            )))
        })?;
    }
    Ok(())
}

fn copy_dir_contents(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read ASR release dir {}: {error}",
            from.display()
        )))
    })? {
        let entry = entry.map_err(|error| {
            BifrostError::Io(io::Error::other(format!("read ASR release entry: {error}")))
        })?;
        let source = entry.path();
        let dest = to.join(entry.file_name());
        if source.is_dir() {
            fs::create_dir_all(&dest).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR install dir {}: {error}",
                    dest.display()
                )))
            })?;
            copy_dir_contents(&source, &dest)?;
        } else {
            fs::copy(&source, &dest).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "copy ASR release {} -> {}: {error}",
                    source.display(),
                    dest.display()
                )))
            })?;
        }
    }
    Ok(())
}

fn mark_cli_binaries_executable(install: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["asr", "asr-server"] {
            let path = install.join(name);
            let mut permissions = fs::metadata(&path)
                .map_err(|error| {
                    BifrostError::Io(io::Error::other(format!(
                        "stat ASR binary {}: {error}",
                        path.display()
                    )))
                })?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "chmod ASR binary {}: {error}",
                    path.display()
                )))
            })?;
        }
    }
    Ok(())
}

fn command_succeeds(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn summarize_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined = format!("{}{}", stdout.trim(), stderr.trim());
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        return "No command output was captured.".to_string();
    }
    let max_chars = 1200;
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let tail = trimmed
        .chars()
        .rev()
        .take(max_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

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
    fn status_reads_persisted_asr_service_state() {
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let state = AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 18080,
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            home: fixed_asr_home(),
            pid: Some(42),
            managed_by: "test".to_string(),
            started_at_ms: 1,
        };
        write_service_state(temp.path(), &state).unwrap();
        let loaded = read_service_state(temp.path()).unwrap();
        assert_eq!(loaded.port, 18080);
        assert_eq!(loaded.managed_by, "test");
    }

    #[test]
    fn adaptive_threshold_quiet_studio_recording() {
        // Clean studio: mean ≈ -45 dB → threshold should sit at -53 dB
        // (mean - 8 dB margin), still safely above the -55 dB floor.
        let t = compute_silence_threshold(Some(-45.0));
        assert!((t - -53.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn adaptive_threshold_noisy_office_recording() {
        // Noisy office: mean ≈ -25 dB → raw threshold -33 dB → still below
        // the -30 dB ceiling. We want the threshold to track the room noise.
        let t = compute_silence_threshold(Some(-25.0));
        assert!((t - -33.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn adaptive_threshold_clamped_to_floor() {
        // Pathological dead-quiet recording (mean -65 dB) must NOT be allowed
        // to push the threshold below -55 dB — anything quieter than that is
        // genuine silence territory.
        let t = compute_silence_threshold(Some(-65.0));
        assert!((t - -55.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn adaptive_threshold_clamped_to_ceiling() {
        // Loud broadcast-style recording (mean -10 dB) must NOT push the
        // threshold above -30 dB; we never want to call -25 dB "silence".
        let t = compute_silence_threshold(Some(-10.0));
        assert!((t - -30.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn adaptive_threshold_falls_back_to_floor_when_unknown() {
        // No volumedetect info → conservative: use the floor so we skip
        // only the most obviously silent regions.
        let t = compute_silence_threshold(None);
        assert!((t - -55.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn parse_db_value_handles_ffmpeg_output() {
        assert_eq!(parse_db_value(" -27.4 dB").unwrap(), -27.4);
        assert_eq!(parse_db_value(" -0.1 dB").unwrap(), -0.1);
        assert!(parse_db_value(" garbage").is_none());
        assert!(parse_db_value("").is_none());
    }
}
