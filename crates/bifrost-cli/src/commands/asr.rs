use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write};
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
use bifrost_core::{process_alias_executable, BifrostError, Result};
use chrono::{Local, TimeZone};
use dialoguer::Select;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use super::asr_tui::{handle_asr_task_tui, AsrTaskTuiOptions};
use crate::cli::{
    AiAsrCommands, AiAsrDiarizationCommands, AiAsrDiarizationSpeakerCommands, AiAsrTaskCommands,
    AiAsrTaskDailyCommands, AiCommands,
};

const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(180);
const ASR_RELEASE_REPO: &str = "second-state/qwen3_asr_rs";
const ASR_SAMPLE_BASE_URL: &str =
    "https://raw.githubusercontent.com/second-state/qwen3_asr_rs/main/test_audio";

pub fn handle_ai_command(action: AiCommands, admin_host: &str, admin_port: u16) -> Result<()> {
    match action {
        AiCommands::Asr { action } => handle_asr_command(action, admin_host, admin_port),
        AiCommands::Voice { action } => {
            super::voice::handle_voice_command(action, admin_host, admin_port)
        }
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
            speaker_aware,
            format: _,
        } => {
            ensure_supported_platform()?;
            if speaker_aware {
                let client = AsrTaskClient::new(admin_host, admin_port);
                stream_file_with_admin_speakers(&client, &audio, &model, &language)
            } else {
                stream_file(&audio, &model, &language)
            }
        }
        AiAsrCommands::Subtitle {
            audio,
            model,
            language,
            profile,
            speaker_aware,
            format,
            out,
            json,
        } => {
            ensure_supported_platform()?;
            let client = AsrTaskClient::new(admin_host, admin_port);
            subtitle_file_with_admin_pipeline(
                &client,
                &audio,
                &model,
                &language,
                &profile,
                speaker_aware,
                &format,
                &out,
                json,
            )
        }
        AiAsrCommands::Task { action } => {
            let client = AsrTaskClient::new(admin_host, admin_port);
            handle_asr_task_command(&client, action)
        }
        AiAsrCommands::Diarization { action } => {
            let client = AsrTaskClient::new(admin_host, admin_port);
            handle_asr_diarization_command(&client, action)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn subtitle_file_with_admin_pipeline(
    client: &AsrTaskClient,
    audio: &Path,
    model: &str,
    language: &str,
    profile: &str,
    speaker_aware: bool,
    formats: &[String],
    out: &Path,
    json: bool,
) -> Result<()> {
    if !audio.is_file() {
        return Err(BifrostError::Config(format!(
            "audio file does not exist: {}",
            audio.display()
        )));
    }
    fs::create_dir_all(out).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "create subtitle output dir {}: {error}",
            out.display()
        )))
    })?;
    let file_name = audio
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("upload.audio");
    let audio_bytes = fs::read(audio).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read audio file {}: {error}",
            audio.display()
        )))
    })?;
    let (content_type, body) = build_asr_upload_multipart(file_name, &audio_bytes);
    let url = format!(
        "{}/asr/offline-jobs?model={}&language={}&pipeline_profile={}&speaker_aware={}",
        client.base_url,
        url_encode(model),
        url_encode(language),
        url_encode(profile),
        if speaker_aware { "1" } else { "0" },
    );
    let response = client
        .agent
        .post(&url)
        .set("content-type", &content_type)
        .set("accept", "application/json")
        .send_bytes(&body)
        .map_err(|error| asr_task_api_error("POST", &url, error))?;
    let created = read_json_response("POST", &url, response)?;
    let job_id = created["job_id"]
        .as_str()
        .ok_or_else(|| BifrostError::Config("offline job response missing job_id".to_string()))?;
    let job = wait_for_offline_job(client, job_id)?;
    if job["status"].as_str() != Some("succeeded") {
        return Err(BifrostError::Config(format!(
            "offline subtitle job {} failed: {}",
            job_id,
            job["error"].as_str().unwrap_or("unknown error")
        )));
    }
    let outputs = download_offline_job_artifacts(client, job_id, audio, out, formats)?;
    let summary = serde_json::json!({
        "job_id": job_id,
        "status": job["status"],
        "pipeline_profile": job["pipeline_profile"],
        "outputs": outputs,
    });
    if json {
        print_json(&summary)
    } else {
        println!("Offline subtitle job {job_id} succeeded.");
        for output in summary["outputs"].as_array().into_iter().flatten() {
            println!(
                "{}\t{}",
                output["format"].as_str().unwrap_or("-"),
                output["path"].as_str().unwrap_or("-")
            );
        }
        Ok(())
    }
}

fn wait_for_offline_job(client: &AsrTaskClient, job_id: &str) -> Result<Value> {
    let started = Instant::now();
    loop {
        let job = client.get_json(&format!("/asr/offline-jobs/{}", url_encode(job_id)))?;
        match job["status"].as_str() {
            Some("succeeded") | Some("failed") => return Ok(job),
            _ => {
                if started.elapsed() > Duration::from_secs(60 * 60) {
                    return Err(BifrostError::Config(format!(
                        "offline subtitle job {job_id} did not finish within 1 hour"
                    )));
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn download_offline_job_artifacts(
    client: &AsrTaskClient,
    job_id: &str,
    audio: &Path,
    out: &Path,
    formats: &[String],
) -> Result<Vec<Value>> {
    let stem = audio
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("subtitle");
    let mut outputs = Vec::new();
    for format in normalized_subtitle_formats(formats) {
        let text = client.get_text(&format!(
            "/asr/offline-jobs/{}/artifacts/{}",
            url_encode(job_id),
            url_encode(&format)
        ))?;
        let extension = match format.as_str() {
            "timeline_json" => "timeline.json",
            "metadata" => "metadata.json",
            "txt" => "txt",
            "srt" => "srt",
            "vtt" => "vtt",
            other => other,
        };
        let path = out.join(format!("{stem}.{extension}"));
        fs::write(&path, text).map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "write subtitle artifact {}: {error}",
                path.display()
            )))
        })?;
        outputs.push(serde_json::json!({
            "format": format,
            "path": path,
        }));
    }
    Ok(outputs)
}

fn normalized_subtitle_formats(formats: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for format in formats {
        let value = format.trim().to_ascii_lowercase();
        let value = match value.as_str() {
            "json" | "timeline" => "timeline_json",
            "metadata_json" => "metadata",
            "text" => "txt",
            other => other,
        };
        if matches!(value, "srt" | "vtt" | "txt" | "timeline_json" | "metadata")
            && !normalized.iter().any(|existing| existing == value)
        {
            normalized.push(value.to_string());
        }
    }
    if normalized.is_empty() {
        normalized.extend(
            ["srt", "vtt", "txt", "timeline_json", "metadata"]
                .into_iter()
                .map(ToString::to_string),
        );
    }
    normalized
}

fn stream_file_with_admin_speakers(
    client: &AsrTaskClient,
    audio: &Path,
    model: &str,
    language: &str,
) -> Result<()> {
    if !audio.is_file() {
        return Err(BifrostError::Config(format!(
            "audio file does not exist: {}",
            audio.display()
        )));
    }
    let file_name = audio
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("upload.audio");
    let audio_bytes = fs::read(audio).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read audio file {}: {error}",
            audio.display()
        )))
    })?;
    let (content_type, body) = build_asr_upload_multipart(file_name, &audio_bytes);
    let url = format!(
        "{}/asr/transcribe-stream?model={}&language={}&owner_module=speech_workbench",
        client.base_url,
        url_encode(model),
        url_encode(language)
    );
    let response = client
        .agent
        .post(&url)
        .set("content-type", &content_type)
        .set("accept", "text/event-stream")
        .send_bytes(&body)
        .map_err(|error| asr_task_api_error("POST", &url, error))?;
    let stream = response.into_reader();
    consume_asr_sse_jsonl(stream)
}

fn build_asr_upload_multipart(file_name: &str, audio_bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("bifrost-asr-{}", uuid::Uuid::new_v4().as_simple());
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            sanitize_multipart_filename(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(audio_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn sanitize_multipart_filename(file_name: &str) -> String {
    file_name
        .chars()
        .map(|ch| match ch {
            '"' | '\r' | '\n' => '_',
            other => other,
        })
        .collect()
}

fn consume_asr_sse_jsonl(reader: impl io::Read) -> Result<()> {
    let reader = io::BufReader::new(reader);
    for line in reader.lines() {
        let line = line.map_err(|error| {
            BifrostError::Io(io::Error::other(format!("read ASR stream: {error}")))
        })?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(data).map_err(|error| {
            BifrostError::Config(format!(
                "ASR stream returned invalid JSON: {error}; data: {}",
                truncate(data, 300)
            ))
        })?;
        println!("{value}");
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct AsrTaskClient {
    pub(crate) base_url: String,
    pub(crate) agent: ureq::Agent,
}

impl AsrTaskClient {
    pub(crate) fn new(host: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}/_bifrost/api", host, port),
            agent: bifrost_core::direct_ureq_agent_builder()
                .timeout(Duration::from_secs(30 * 60))
                .build(),
        }
    }

    pub(crate) fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|error| asr_task_api_error("GET", &url, error))?;
        read_json_response("GET", &url, response)
    }

    pub(crate) fn get_text(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|error| asr_task_api_error("GET", &url, error))?;
        response.into_string().map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "read ASR API response from {url}: {error}"
            )))
        })
    }

    pub(crate) fn post_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .post(&url)
            .call()
            .map_err(|error| asr_task_api_error("POST", &url, error))?;
        read_json_response("POST", &url, response)
    }

    pub(crate) fn post_json_body(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .post(&url)
            .set("content-type", "application/json")
            .send_string(&body.to_string())
            .map_err(|error| asr_task_api_error("POST", &url, error))?;
        read_json_response("POST", &url, response)
    }

    pub(crate) fn put_json_body(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .agent
            .put(&url)
            .set("content-type", "application/json")
            .send_string(&body.to_string())
            .map_err(|error| asr_task_api_error("PUT", &url, error))?;
        read_json_response("PUT", &url, response)
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
    #[serde(default)]
    diarization_enabled: bool,
    #[serde(default)]
    diarization_ready: bool,
    #[serde(default)]
    diarization_running: bool,
    #[serde(default)]
    diarized_files: usize,
    #[serde(default)]
    speaker_count: usize,
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
    diarization: Value,
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
struct AsrTaskWatchList {
    #[serde(default)]
    tasks: Vec<AsrTaskWatchChoice>,
}

#[derive(Debug, Deserialize, Default)]
struct AsrTaskWatchChoice {
    #[serde(default)]
    task: AsrTaskWatchChoiceTask,
    #[serde(default)]
    progress: AsrTaskWatchChoiceProgress,
}

#[derive(Debug, Deserialize, Default)]
struct AsrTaskWatchChoiceTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    next_run_at_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
struct AsrTaskWatchChoiceProgress {
    #[serde(default)]
    discovered: usize,
    #[serde(default)]
    processed: usize,
    #[serde(default)]
    pending: usize,
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
    diarization_status: Option<String>,
    #[serde(default)]
    speaker_count: Option<usize>,
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
        AiAsrTaskCommands::Create {
            name,
            dir,
            model,
            language,
            runtime_strategy,
            time,
            disabled,
            non_recursive,
            no_speaker_diarization,
            diarization_profile,
            known_speaker_count,
            no_voiceprint_matching,
            json,
        } => create_task(
            client,
            CreateTaskCliInput {
                name,
                dir,
                model,
                language,
                runtime_strategy,
                time,
                disabled,
                non_recursive,
                no_speaker_diarization,
                diarization_profile,
                known_speaker_count,
                no_voiceprint_matching,
                json,
            },
        ),
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
        AiAsrTaskCommands::Watch {
            task,
            refresh_ms,
            no_interactive_select,
            all,
            json_snapshot,
            read_only,
        }
        | AiAsrTaskCommands::Tui {
            task,
            refresh_ms,
            no_interactive_select,
            all,
            json_snapshot,
            read_only,
        } => handle_asr_task_tui(
            client,
            AsrTaskTuiOptions {
                task,
                refresh_ms,
                no_interactive_select,
                all,
                json_snapshot,
                read_only,
            },
        ),
        AiAsrTaskCommands::Daily { action } => handle_asr_task_daily_command(client, action),
    }
}

#[derive(Debug)]
struct CreateTaskCliInput {
    name: Option<String>,
    dir: PathBuf,
    model: String,
    language: String,
    runtime_strategy: String,
    time: String,
    disabled: bool,
    non_recursive: bool,
    no_speaker_diarization: bool,
    diarization_profile: String,
    known_speaker_count: Option<u8>,
    no_voiceprint_matching: bool,
    json: bool,
}

fn create_task(client: &AsrTaskClient, input: CreateTaskCliInput) -> Result<()> {
    let (hour, minute) = parse_daily_time(&input.time)?;
    let body = build_create_task_body(&input, hour, minute);
    let value = client.post_json_body("/asr/tasks", &body)?;
    if input.json {
        print_json(&value)?;
        return Ok(());
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .or(input.name.as_deref())
        .unwrap_or("ASR directory task");
    println!("ASR directory task created: {name} ({id})");
    println!(
        "Defaults: model={}, speaker_diarization={}, voiceprint_matching={}",
        input.model, !input.no_speaker_diarization, !input.no_voiceprint_matching
    );
    Ok(())
}

fn build_create_task_body(input: &CreateTaskCliInput, hour: u8, minute: u8) -> Value {
    serde_json::json!({
        "name": input.name,
        "audio_dir": input.dir,
        "recursive": !input.non_recursive,
        "enabled": !input.disabled,
        "schedule": {
            "kind": "daily",
            "hour": hour,
            "minute": minute
        },
        "language": input.language,
        "model": input.model,
        "runtime_strategy": input.runtime_strategy,
        "diarization": {
            "enabled": !input.no_speaker_diarization,
            "profile": input.diarization_profile,
            "known_speaker_count": input.known_speaker_count,
            "voiceprint_matching": !input.no_voiceprint_matching
        }
    })
}

fn parse_daily_time(value: &str) -> Result<(u8, u8)> {
    let (hour, minute) = value.split_once(':').ok_or_else(|| {
        BifrostError::Config("Invalid --time; expected HH:MM, for example 02:00".to_string())
    })?;
    let hour = hour.parse::<u8>().map_err(|_| {
        BifrostError::Config("Invalid --time hour; expected 00 through 23".to_string())
    })?;
    let minute = minute.parse::<u8>().map_err(|_| {
        BifrostError::Config("Invalid --time minute; expected 00 through 59".to_string())
    })?;
    if hour > 23 || minute > 59 {
        return Err(BifrostError::Config(
            "Invalid --time; expected HH:MM with hour 00-23 and minute 00-59".to_string(),
        ));
    }
    Ok((hour, minute))
}

fn handle_asr_diarization_command(
    client: &AsrTaskClient,
    action: AiAsrDiarizationCommands,
) -> Result<()> {
    match action {
        AiAsrDiarizationCommands::Profiles { json } => {
            let value = client.get_json("/asr/diarization/profiles")?;
            if json {
                print_json(&value)?;
            } else {
                print_diarization_profiles(&value);
            }
            Ok(())
        }
        AiAsrDiarizationCommands::Status { profile, json } => {
            let value = client.get_json(&format!(
                "/asr/diarization/status?profile={}",
                url_encode(&profile)
            ))?;
            if json {
                print_json(&value)?;
            } else {
                print_diarization_status(&value);
            }
            Ok(())
        }
        AiAsrDiarizationCommands::Init { profile, json } => {
            let stream = client.get_text(&format!(
                "/asr/diarization/init-stream?profile={}",
                url_encode(&profile)
            ))?;
            let value = sse_last_json(&stream).unwrap_or_else(|| Value::String(stream.clone()));
            if json {
                print_json(&value)?;
            } else {
                println!("Diarization profile initialized: {profile}");
                print_diarization_status(value.get("status").unwrap_or(&value));
            }
            Ok(())
        }
        AiAsrDiarizationCommands::Speakers { action } => {
            handle_asr_diarization_speaker_command(client, action)
        }
    }
}

fn handle_asr_diarization_speaker_command(
    client: &AsrTaskClient,
    action: AiAsrDiarizationSpeakerCommands,
) -> Result<()> {
    match action {
        AiAsrDiarizationSpeakerCommands::List { json } => {
            let value = client.get_json("/asr/speaker-profiles")?;
            if json {
                print_json(&value)?;
            } else {
                print_speaker_profiles(&value);
            }
            Ok(())
        }
        AiAsrDiarizationSpeakerCommands::Show { profile_id, json } => {
            let value = client.get_json(&format!(
                "/asr/speaker-profiles/{}",
                url_encode(&profile_id)
            ))?;
            if json {
                print_json(&value)?;
            } else {
                print_speaker_profile(&value);
            }
            Ok(())
        }
        AiAsrDiarizationSpeakerCommands::EnrollLive {
            name,
            profile,
            phrase_seconds,
            device,
            test_pcm16,
            json,
        } => enroll_speaker_live(
            client,
            &name,
            &profile,
            phrase_seconds,
            &device,
            test_pcm16.as_deref(),
            json,
        ),
    }
}

fn enroll_speaker_live(
    client: &AsrTaskClient,
    name: &str,
    profile: &str,
    phrase_seconds: u64,
    device: &str,
    test_pcm16: Option<&Path>,
    json: bool,
) -> Result<()> {
    let session_value = client.post_json_body(
        "/asr/speaker-profiles/enrollment-sessions",
        &serde_json::json!({
            "name": name,
            "diarization_profile": profile,
        }),
    )?;
    let session = session_value
        .get("session")
        .ok_or_else(|| BifrostError::Config("speaker enrollment session missing".to_string()))?;
    let session_id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| BifrostError::Config("speaker enrollment session id missing".to_string()))?;
    let prompts = session
        .get("prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if prompts.is_empty() {
        return Err(BifrostError::Config(
            "speaker enrollment session did not return prompts".to_string(),
        ));
    }

    for (index, prompt) in prompts.iter().enumerate() {
        let prompt_id = prompt.get("id").and_then(Value::as_str).ok_or_else(|| {
            BifrostError::Config("speaker enrollment prompt id missing".to_string())
        })?;
        let text = prompt.get("text").and_then(Value::as_str).unwrap_or("");
        if test_pcm16.is_none() {
            println!();
            println!("Prompt {}/{}:", index + 1, prompts.len());
            println!("{text}");
            println!("Recording {phrase_seconds}s from local microphone device '{device}'...");
        }
        let audio = match test_pcm16 {
            Some(path) => fs::read(path).map_err(BifrostError::Io)?,
            None => record_pcm16_with_ffmpeg(phrase_seconds, device)?,
        };
        send_enrollment_audio_chunk(client, session_id, prompt_id, &audio, true)?;
    }
    let result = client.post_json_body(
        &format!(
            "/asr/speaker-profiles/enrollment-sessions/{}/finish",
            url_encode(session_id)
        ),
        &serde_json::json!({}),
    )?;
    if json {
        print_json(&result)?;
    } else {
        let profile = result.get("profile").unwrap_or(&result);
        println!(
            "Speaker voiceprint enrolled: {} ({})",
            profile
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(name),
            profile.get("id").and_then(Value::as_str).unwrap_or("-")
        );
    }
    Ok(())
}

fn send_enrollment_audio_chunk(
    client: &AsrTaskClient,
    session_id: &str,
    prompt_id: &str,
    audio: &[u8],
    final_chunk: bool,
) -> Result<()> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(audio);
    client.post_json_body(
        &format!(
            "/asr/speaker-profiles/enrollment-sessions/{}/audio",
            url_encode(session_id)
        ),
        &serde_json::json!({
            "prompt_id": prompt_id,
            "pcm16le_base64": encoded,
            "sample_rate": 16000,
            "channels": 1,
            "final_chunk": final_chunk,
        }),
    )?;
    Ok(())
}

fn record_pcm16_with_ffmpeg(seconds: u64, device: &str) -> Result<Vec<u8>> {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-t",
        &seconds.max(1).to_string(),
        "-ac",
        "1",
        "-ar",
        "16000",
        "-f",
    ]);
    if cfg!(target_os = "macos") {
        command.args(["avfoundation", "-i", device]);
    } else {
        command.args(["pulse", "-i", device]);
    }
    command.args(["-f", "s16le", "-"]);
    let output = command.output().map_err(BifrostError::Io)?;
    if !output.status.success() {
        return Err(BifrostError::Config(format!(
            "ffmpeg microphone capture failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if output.stdout.is_empty() {
        return Err(BifrostError::Config(
            "ffmpeg microphone capture produced no audio".to_string(),
        ));
    }
    Ok(output.stdout)
}

fn sse_last_json(stream: &str) -> Option<Value> {
    stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .next_back()
}

fn handle_asr_task_daily_command(
    client: &AsrTaskClient,
    action: AiAsrTaskDailyCommands,
) -> Result<()> {
    match action {
        AiAsrTaskDailyCommands::List { task, json } => {
            let task_id = select_asr_task_id(client, task.as_deref())?;
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
            first,
            second,
            task,
            output,
            json,
        } => {
            let (task_query, date) = resolve_daily_show_args(first, second, task)?;
            let task_id = select_asr_task_id(client, task_query.as_deref())?;
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
        AiAsrTaskDailyCommands::SetSyncDir {
            task,
            dir,
            clear,
            json,
        } => {
            if clear && dir.is_some() {
                return Err(BifrostError::Config(
                    "Use either --dir or --clear, not both".to_string(),
                ));
            }
            if !clear && dir.is_none() {
                return Err(BifrostError::Config(
                    "daily set-sync-dir requires --dir <PATH> or --clear".to_string(),
                ));
            }
            let task_id = select_asr_task_id(client, task.as_deref())?;
            let sync_dir = if clear {
                String::new()
            } else {
                dir.unwrap().to_string_lossy().to_string()
            };
            let value = client.put_json_body(
                &format!("/asr/tasks/{}/daily-agent", url_encode(&task_id)),
                &serde_json::json!({ "report_sync_dir": sync_dir }),
            )?;
            if json {
                print_json(&value)?;
            } else if clear {
                println!("Cleared Daily Agent report sync directory for task {task_id}");
            } else {
                let configured = value
                    .pointer("/config/report_sync_dir")
                    .and_then(Value::as_str)
                    .unwrap_or(sync_dir.as_str());
                println!("Daily Agent report sync directory for task {task_id}: {configured}");
            }
            Ok(())
        }
        AiAsrTaskDailyCommands::Sync { task, dir, json } => {
            let task_id = select_asr_task_id(client, task.as_deref())?;
            if let Some(dir) = dir {
                let sync_dir = dir.to_string_lossy().to_string();
                let _ = client.put_json_body(
                    &format!("/asr/tasks/{}/daily-agent", url_encode(&task_id)),
                    &serde_json::json!({ "report_sync_dir": sync_dir }),
                )?;
            }
            let value = client.post_json(&format!(
                "/asr/tasks/{}/daily-agent/sync",
                url_encode(&task_id)
            ))?;
            if json {
                print_json(&value)?;
            } else {
                print_daily_agent_sync_result(&task_id, &value);
            }
            Ok(())
        }
    }
}

fn print_daily_agent_sync_result(task_id: &str, value: &Value) {
    let sync = value.get("sync").unwrap_or(value);
    let target_dir = sync
        .get("target_dir")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let total = sync.get("total_files").and_then(Value::as_u64).unwrap_or(0);
    let copied = sync
        .get("copied_files")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let skipped = sync
        .get("skipped_files")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let failed = sync
        .get("failed_files")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("Daily Agent report sync for task {task_id}");
    println!("  Target:  {target_dir}");
    println!("  Total:   {total}");
    println!("  Copied:  {copied}");
    println!("  Skipped: {skipped}");
    println!("  Failed:  {failed}");
}

fn select_asr_task_id(client: &AsrTaskClient, query: Option<&str>) -> Result<String> {
    let value = client.get_json("/asr/tasks/-/watch")?;
    let list: AsrTaskWatchList = serde_json::from_value(value)
        .map_err(|error| BifrostError::Config(format!("parse ASR task list: {error}")))?;
    if let Some(query) = query {
        return resolve_task_choice_query(&list.tasks, query).map(|task| task.task.id.clone());
    }
    match list.tasks.len() {
        0 => Err(BifrostError::Config("No ASR directory tasks.".to_string())),
        1 => Ok(list.tasks[0].task.id.clone()),
        _ if !io::stdin().is_terminal() => Err(BifrostError::Config(
            "Multiple ASR directory tasks exist; pass a task id, unique id prefix, or unique name"
                .to_string(),
        )),
        _ => {
            let labels = list.tasks.iter().map(task_choice_label).collect::<Vec<_>>();
            let index = Select::new()
                .with_prompt("Select ASR task")
                .items(&labels)
                .default(0)
                .interact()
                .map_err(|error| BifrostError::Io(io::Error::other(error)))?;
            Ok(list.tasks[index].task.id.clone())
        }
    }
}

fn resolve_task_choice_query<'a>(
    tasks: &'a [AsrTaskWatchChoice],
    query: &str,
) -> Result<&'a AsrTaskWatchChoice> {
    if let Some(task) = tasks.iter().find(|task| task.task.id == query) {
        return Ok(task);
    }
    let prefix_matches = tasks
        .iter()
        .filter(|task| task.task.id.starts_with(query))
        .collect::<Vec<_>>();
    if prefix_matches.len() == 1 {
        return Ok(prefix_matches[0]);
    }
    if prefix_matches.len() > 1 {
        return Err(BifrostError::Config(format!(
            "ambiguous ASR task id prefix '{query}'; use the full task id"
        )));
    }
    let name_matches = tasks
        .iter()
        .filter(|task| task.task.name == query)
        .collect::<Vec<_>>();
    if name_matches.len() == 1 {
        return Ok(name_matches[0]);
    }
    if name_matches.len() > 1 {
        return Err(BifrostError::Config(format!(
            "ambiguous ASR task name '{query}'; use a task id"
        )));
    }
    Err(BifrostError::Config(format!(
        "ASR task '{query}' not found"
    )))
}

fn task_choice_label(choice: &AsrTaskWatchChoice) -> String {
    format!(
        "{:<24} {:<10} {:>3}/{:<3} pending {:<3} next {}",
        truncate(&choice.task.name, 24),
        task_choice_state(&choice.task),
        choice.progress.processed,
        choice.progress.discovered,
        choice.progress.pending,
        format_optional_ms(choice.task.next_run_at_ms)
    )
}

fn task_choice_state(task: &AsrTaskWatchChoiceTask) -> &'static str {
    if task.running {
        "running"
    } else if task.paused {
        "paused"
    } else if !task.enabled {
        "disabled"
    } else {
        "enabled"
    }
}

fn resolve_daily_show_args(
    first: String,
    second: Option<String>,
    task: Option<String>,
) -> Result<(Option<String>, String)> {
    match (second, task) {
        (Some(_date), Some(_)) => Err(BifrostError::Config(
            "Use either `daily show <task> <date>` or `daily show <date> --task <task>`, not both"
                .to_string(),
        )),
        (Some(date), None) => Ok((Some(first), date)),
        (None, task) => Ok((task, first)),
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
    write_stdout_text(&format!(
        "{}\n",
        serde_json::to_string_pretty(value)
            .map_err(|error| BifrostError::Config(error.to_string()))?
    ))
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

fn print_diarization_profiles(value: &Value) {
    let profiles = value
        .get("profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if profiles.is_empty() {
        println!("No ASR diarization profiles.");
        return;
    }
    println!("{:<28}  {:<18}  {:<10}  READY", "PROFILE", "ENGINE", "TIER");
    for profile in profiles {
        println!(
            "{:<28}  {:<18}  {:<10}  {}",
            profile.get("id").and_then(Value::as_str).unwrap_or("-"),
            profile.get("engine").and_then(Value::as_str).unwrap_or("-"),
            profile
                .get("quality_tier")
                .and_then(Value::as_str)
                .unwrap_or("-"),
            profile
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
    }
}

fn print_diarization_status(value: &Value) {
    let profile = value.get("profile").unwrap_or(value);
    println!(
        "Profile:          {}",
        profile.get("id").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "Engine:           {}",
        profile.get("engine").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "Ready:            {}",
        profile
            .get("ready")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    );
    println!(
        "Install dir:      {}",
        profile
            .get("install_dir")
            .and_then(Value::as_str)
            .unwrap_or("-")
    );
    if let Some(dir) = value.get("voiceprint_dir").and_then(Value::as_str) {
        println!("Voiceprint dir:   {dir}");
    }
    if let Some(count) = value.get("speaker_profile_count").and_then(Value::as_u64) {
        println!("Speaker profiles: {count}");
    }
    if let Some(message) = profile.get("message").and_then(Value::as_str) {
        println!("Message:          {message}");
    }
}

fn print_speaker_profiles(value: &Value) {
    let profiles = value
        .get("profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if profiles.is_empty() {
        println!("No enrolled speaker voiceprints.");
        return;
    }
    println!("{:<40}  {:<24}  EMBEDDING", "PROFILE", "NAME");
    for profile in profiles {
        println!(
            "{:<40}  {:<24}  {}",
            profile.get("id").and_then(Value::as_str).unwrap_or("-"),
            profile
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or("-"),
            profile
                .get("embedding_dim")
                .and_then(Value::as_u64)
                .map(|dim| format!("{dim}d"))
                .unwrap_or_else(|| "-".to_string())
        );
    }
}

fn print_speaker_profile(value: &Value) {
    println!(
        "Speaker:          {}",
        value
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or("-")
    );
    println!(
        "Profile ID:       {}",
        value.get("id").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "Source:           {}",
        value.get("source").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "Diarization:      {}",
        value
            .get("diarization_profile")
            .and_then(Value::as_str)
            .unwrap_or("-")
    );
    println!(
        "Duration:         {} ms",
        value
            .get("total_duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    println!(
        "Embedding:        {}d",
        value
            .get("embedding_dim")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
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
    println!("Diarization:     {}", format_value(&task.diarization));
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
    if task.summary.diarization_enabled {
        println!(
            "Speaker state:   ready={}, running={}, diarized_files={}, speakers={}",
            task.summary.diarization_ready,
            task.summary.diarization_running,
            task.summary.diarized_files,
            task.summary.speaker_count
        );
    }
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
        "{:<12}  {:<12}  {:>8}  {:>10}  {:<19}  SOURCE",
        "STATUS", "SPEAKERS", "CHARS", "DURATION", "FINISHED"
    );
    for file in files {
        println!(
            "{:<12}  {:<12}  {:>8}  {:>10}  {:<19}  {}",
            file.status,
            file.diarization_status
                .as_deref()
                .map(|status| match file.speaker_count {
                    Some(count) => format!("{status}:{count}"),
                    None => status.to_string(),
                })
                .unwrap_or_else(|| "-".to_string()),
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

#[cfg(test)]
mod daily_command_tests {
    use super::*;

    fn choice(id: &str, name: &str) -> AsrTaskWatchChoice {
        AsrTaskWatchChoice {
            task: AsrTaskWatchChoiceTask {
                id: id.to_string(),
                name: name.to_string(),
                enabled: true,
                ..AsrTaskWatchChoiceTask::default()
            },
            ..AsrTaskWatchChoice::default()
        }
    }

    #[test]
    fn resolve_task_choice_query_accepts_unique_prefix_and_name() {
        let tasks = vec![choice("abcdef123", "Alpha"), choice("fedcba987", "Beta")];

        assert_eq!(
            resolve_task_choice_query(&tasks, "abcdef").unwrap().task.id,
            "abcdef123"
        );
        assert_eq!(
            resolve_task_choice_query(&tasks, "Beta").unwrap().task.id,
            "fedcba987"
        );
    }

    #[test]
    fn resolve_task_choice_query_rejects_ambiguous_prefix_or_name() {
        let tasks = vec![choice("abcdef123", "Same"), choice("abc999999", "Same")];

        assert!(resolve_task_choice_query(&tasks, "abc")
            .unwrap_err()
            .to_string()
            .contains("ambiguous ASR task id prefix"));
        assert!(resolve_task_choice_query(&tasks, "Same")
            .unwrap_err()
            .to_string()
            .contains("ambiguous ASR task name"));
    }

    #[test]
    fn resolve_daily_show_args_supports_legacy_and_task_option_forms() {
        assert_eq!(
            resolve_daily_show_args("2026-05-24".to_string(), None, None).unwrap(),
            (None, "2026-05-24".to_string())
        );
        assert_eq!(
            resolve_daily_show_args("2026-05-24".to_string(), None, Some("Alpha".to_string()))
                .unwrap(),
            (Some("Alpha".to_string()), "2026-05-24".to_string())
        );
        assert_eq!(
            resolve_daily_show_args("Alpha".to_string(), Some("2026-05-24".to_string()), None)
                .unwrap(),
            (Some("Alpha".to_string()), "2026-05-24".to_string())
        );
    }

    #[test]
    fn resolve_daily_show_args_rejects_mixed_legacy_and_task_option() {
        assert!(resolve_daily_show_args(
            "Alpha".to_string(),
            Some("2026-05-24".to_string()),
            Some("Beta".to_string())
        )
        .unwrap_err()
        .to_string()
        .contains("Use either"));
    }
}

fn print_status(json: bool) -> Result<()> {
    let state = read_service_state(&bifrost_storage::data_dir());
    let ready = state
        .as_ref()
        .map(|state| probe_health_blocking(&state.host, state.port, Duration::from_secs(2)).is_ok())
        .unwrap_or(false);
    if json {
        write_stdout_text(&format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "ready": ready,
                "service": state,
            }))
            .map_err(|error| BifrostError::Config(error.to_string()))?
        ))?;
    } else if let Some(state) = state {
        let owner_id = state.owner_id.as_deref().unwrap_or("-");
        write_stdout_text(&format!(
            "ready: {ready}\nserver: http://{}:{}\nmodel: {}\nlanguage: {}\nmanaged_by: {}\nowner_module: {}\nowner_id: {}\n",
            state.host,
            state.port,
            state.model,
            state.language,
            state.managed_by,
            state.lease_owner_module(),
            owner_id
        ))?;
    } else {
        write_stdout_text("ready: false\nserver: not running\n")?;
    }
    Ok(())
}

fn write_stdout_text(text: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    write_text_ignore_broken_pipe(&mut stdout, text)
}

fn write_text_ignore_broken_pipe(writer: &mut impl Write, text: &str) -> Result<()> {
    match writer
        .write_all(text.as_bytes())
        .and_then(|_| writer.flush())
    {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(BifrostError::Io(error)),
    }
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

pub(super) fn stream_file(audio: &Path, model: &str, language: &str) -> Result<()> {
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

    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if probe_health_blocking(&state.host, state.port, Duration::from_secs(2)).is_ok() {
            return Err(BifrostError::Config(format!(
                "Qwen3-ASR service is busy: active owner={} model={} server=http://{}:{}; requested owner=cli model={}",
                state.lease_owner_module(),
                state.model,
                state.host,
                state.port,
                model
            )));
        }
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

    let asr_server = labeled_process_executable(&install.join("asr-server"), "bifrost-asr-server");
    let child = Command::new(asr_server)
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
        owner_module: Some("cli".to_string()),
        owner_id: None,
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

fn labeled_process_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    match process_alias_executable(executable, &alias_dir, alias_name) {
        Ok(alias) => alias,
        Err(error) => {
            eprintln!(
                "warning: falling back to unlabeled ASR executable {}: {}",
                executable.display(),
                error
            );
            executable.to_path_buf()
        }
    }
}

fn stop_service() -> Result<()> {
    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if state.lease_owner_module() != "cli" {
            return Err(BifrostError::Config(format!(
                "Refusing to stop ASR service owned by {}. Stop it from the owning module or use its API.",
                state.lease_owner_module()
            )));
        }
        if let Some(pid) = state.pid {
            let _ = stop_pid(pid);
        }
    }
    clear_service_state(&bifrost_storage::data_dir()).map_err(BifrostError::Config)
}

fn healthy_state(model: &str, language: &str) -> Option<AsrServiceState> {
    let state = read_service_state(&bifrost_storage::data_dir())?;
    if state.model != model || state.language != language || state.lease_owner_module() != "cli" {
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
    let client = bifrost_core::outbound_reqwest_client_builder()
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
    #[cfg(not(unix))]
    {
        let _ = install;
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

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk output unavailable"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
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
            owner_module: Some("test".to_string()),
            owner_id: Some("status-fixture".to_string()),
            started_at_ms: 1,
        };
        write_service_state(temp.path(), &state).unwrap();
        let loaded = read_service_state(temp.path()).unwrap();
        assert_eq!(loaded.port, 18080);
        assert_eq!(loaded.managed_by, "test");
        assert_eq!(loaded.lease_owner_module(), "test");
        assert_eq!(loaded.owner_id.as_deref(), Some("status-fixture"));
    }

    #[test]
    fn asr_upload_multipart_contains_file_field_and_sanitized_filename() {
        let (content_type, body) = build_asr_upload_multipart("bad\"name\n.wav", b"abc");
        let body = String::from_utf8(body).unwrap();

        assert!(content_type.starts_with("multipart/form-data; boundary=bifrost-asr-"));
        assert!(body.contains("name=\"file\""));
        assert!(body.contains("filename=\"bad_name_.wav\""));
        assert!(body.contains("\r\n\r\nabc\r\n--"));
    }

    #[test]
    fn status_reads_legacy_webui_service_state_as_workbench_owner() {
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let state_dir = temp.path().join("asr");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("service.json"),
            serde_json::json!({
                "host": "127.0.0.1",
                "port": 18080,
                "model": "Qwen3-ASR-1.7B",
                "language": "chinese",
                "home": fixed_asr_home(),
                "pid": null,
                "managed_by": "webui",
                "started_at_ms": 1
            })
            .to_string(),
        )
        .unwrap();

        let loaded = read_service_state(temp.path()).unwrap();

        assert_eq!(loaded.managed_by, "webui");
        assert_eq!(loaded.lease_owner_module(), "speech_workbench");
        assert_eq!(loaded.owner_id, None);
    }

    #[test]
    fn asr_status_output_ignores_broken_pipe() {
        let mut writer = BrokenPipeWriter;
        write_text_ignore_broken_pipe(&mut writer, "{\n  \"ready\": false\n}\n")
            .expect("broken pipe should be treated as a closed downstream pipe");
    }

    #[test]
    fn asr_status_output_keeps_real_io_errors() {
        let mut writer = FailingWriter;
        let error = write_text_ignore_broken_pipe(&mut writer, "ready: false\n")
            .expect_err("non-broken-pipe errors should still be returned");
        assert!(error.to_string().contains("disk output unavailable"));
    }

    #[test]
    fn create_task_cli_body_defaults_to_speaker_aware_0_6b() {
        let input = CreateTaskCliInput {
            name: Some("meetings".to_string()),
            dir: PathBuf::from("/tmp/meetings"),
            model: bifrost_asr::runtime::DEFAULT_ASR_MODEL.to_string(),
            language: "chinese".to_string(),
            runtime_strategy: "reuse_per_file".to_string(),
            time: "02:00".to_string(),
            disabled: false,
            non_recursive: false,
            no_speaker_diarization: false,
            diarization_profile: bifrost_asr::profiles::DEFAULT_DIARIZATION_PROFILE.to_string(),
            known_speaker_count: None,
            no_voiceprint_matching: false,
            json: false,
        };

        let body = build_create_task_body(&input, 2, 0);

        assert_eq!(body["model"], "Qwen3-ASR-0.6B");
        assert_eq!(body["diarization"]["enabled"], true);
        assert_eq!(body["diarization"]["voiceprint_matching"], true);
        assert_eq!(body["diarization"]["profile"], "sherpa-onnx-balanced");
        assert_eq!(body["runtime_strategy"], "reuse_per_file");
    }

    #[test]
    fn parse_daily_time_rejects_invalid_clock_values() {
        assert_eq!(parse_daily_time("23:59").unwrap(), (23, 59));
        assert!(parse_daily_time("24:00").is_err());
        assert!(parse_daily_time("02").is_err());
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

    #[test]
    fn service_state_url_formats_ipv4_and_ipv6_hosts() {
        let mut state = AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 18080,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            home: fixed_asr_home(),
            pid: None,
            managed_by: "cli".to_string(),
            owner_module: None,
            owner_id: None,
            started_at_ms: 0,
        };
        assert_eq!(service_state_url(&state), "http://127.0.0.1:18080");
        state.host = "::1".to_string();
        assert_eq!(service_state_url(&state), "http://[::1]:18080");
    }

    #[test]
    fn format_timestamp_formats_hh_mm_ss_millis() {
        assert_eq!(format_timestamp(0.0), "00:00:00.000".to_string());
        assert_eq!(format_timestamp(1.234), "00:00:01.234".to_string());
        assert_eq!(format_timestamp(65.001), "00:01:05.001".to_string());
    }

    #[test]
    fn format_datetime_epoch_zero_is_unix_epoch_start() {
        assert_eq!(format_datetime(0), "1970-01-01 00:00:00".to_string());
    }

    #[test]
    fn epoch_days_to_ymd_and_ymd_to_epoch_days_roundtrip() {
        let days = ymd_to_epoch_days(2026, 5, 14);
        let (y, m, d) = epoch_days_to_ymd(days);
        assert_eq!((y, m, d), (2026, 5, 14));

        let epoch0 = ymd_to_epoch_days(1970, 1, 1);
        assert_eq!(epoch0, 0);
    }

    #[test]
    fn parse_datetime_to_epoch_parses_fractional_seconds() {
        let days = ymd_to_epoch_days(2026, 5, 14);
        let expected = days * 86400 + 17 * 3600 + 2 * 60 + 41;
        assert_eq!(
            parse_datetime_to_epoch("2026-05-14", "17:02:41.123"),
            Some(expected)
        );
        assert_eq!(
            parse_datetime_to_epoch("2026-05-14", "17:02:41"),
            Some(expected)
        );
    }

    #[test]
    fn parse_datetime_to_epoch_rejects_invalid_input() {
        assert!(parse_datetime_to_epoch("not-a-date", "12:00:00").is_none());
        assert!(parse_datetime_to_epoch("2026-05-14", "not-a-time").is_none());
    }

    #[test]
    fn parse_filename_origin_extracts_timestamp_from_name() {
        let path = std::path::Path::new("TX01_MIC004_20260514_170241_orig.wav");
        let epoch = parse_filename_origin(path).expect("timestamp");
        let days = ymd_to_epoch_days(2026, 5, 14);
        let expected = days * 86400 + 17 * 3600 + 2 * 60 + 41;
        assert_eq!(epoch, expected);
    }

    #[test]
    fn parse_filename_origin_returns_none_for_unparsable_name() {
        let path = std::path::Path::new("recording.wav");
        assert!(parse_filename_origin(path).is_none());
    }

    #[test]
    fn format_absolute_time_adds_offset_seconds() {
        let origin_days = ymd_to_epoch_days(2026, 5, 14);
        let origin_epoch = origin_days * 86400;
        let formatted = format_absolute_time(origin_epoch, 1.5);
        assert_eq!(formatted, format_datetime(origin_epoch + 1));
    }

    #[test]
    fn format_duration_ms_formats_mm_ss() {
        assert_eq!(format_duration_ms(0), "00:00".to_string());
        assert_eq!(format_duration_ms(1_000), "00:01".to_string());
        assert_eq!(format_duration_ms(61_000), "01:01".to_string());
    }

    #[test]
    fn truncate_shorter_or_equal_keeps_original_string() {
        assert_eq!(truncate("abc", 3), "abc".to_string());
        assert_eq!(truncate("abc", 10), "abc".to_string());
    }

    #[test]
    fn truncate_longer_appends_ellipsis() {
        assert_eq!(truncate("abcdef", 3), "abc...".to_string());
    }

    #[test]
    fn url_encode_escapes_spaces_and_utf8() {
        let encoded = url_encode("Hello 世界");
        assert!(!encoded.contains(' '));
        assert!(encoded.contains("Hello"));
        assert!(encoded.contains('%'));
    }

    #[test]
    fn format_value_handles_string_null_and_object() {
        assert_eq!(
            format_value(&Value::String("x".to_string())),
            "x".to_string()
        );
        assert_eq!(format_value(&Value::Null), "-".to_string());
        let obj = serde_json::json!({"a": 1});
        assert_eq!(format_value(&obj), "{\"a\":1}".to_string());
    }

    #[test]
    fn format_optional_ms_formats_some_and_none() {
        assert_eq!(format_optional_ms(None), "-".to_string());
        let s = format_optional_ms(Some(i64::MAX));
        assert_eq!(s, format_ms(i64::MAX));
    }

    #[test]
    fn format_ms_out_of_range_falls_back_to_numeric_string() {
        let s = format_ms(i64::MAX);
        assert_eq!(s, i64::MAX.to_string());
    }

    #[test]
    fn task_state_prioritizes_running_over_paused_and_enabled() {
        let mut task = AsrTask::default();
        assert_eq!(task_state(&task), "disabled");
        task.enabled = true;
        assert_eq!(task_state(&task), "enabled");
        task.paused = true;
        assert_eq!(task_state(&task), "paused");
        task.summary.running = true;
        assert_eq!(task_state(&task), "running");
    }

    #[test]
    fn task_choice_state_prioritizes_running_over_paused_and_enabled() {
        let mut task = AsrTaskWatchChoiceTask::default();
        assert_eq!(task_choice_state(&task), "disabled");
        task.enabled = true;
        assert_eq!(task_choice_state(&task), "enabled");
        task.paused = true;
        assert_eq!(task_choice_state(&task), "paused");
        task.running = true;
        assert_eq!(task_choice_state(&task), "running");
    }

    #[test]
    fn summarize_command_output_handles_empty_and_trims() {
        assert_eq!(
            summarize_command_output(b"", b""),
            "No command output was captured.".to_string()
        );
        assert_eq!(
            summarize_command_output(b" out ", b" err "),
            "outerr".to_string()
        );
    }

    #[test]
    fn summarize_command_output_truncates_from_tail_when_long() {
        let long = "a".repeat(1300);
        let out = summarize_command_output(long.as_bytes(), b"");
        assert!(out.starts_with("..."));
        assert_eq!(out.chars().count(), 1203);
        assert!(out.chars().skip(3).all(|c| c == 'a'));
    }

    #[test]
    fn ensure_supported_platform_matches_current_target() {
        let result = ensure_supported_platform();
        if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- Helpers -----------------------------------------------------------------

    fn make_test_client_with_base_url(base_url: String) -> AsrTaskClient {
        AsrTaskClient {
            base_url,
            agent: bifrost_core::direct_ureq_agent_builder()
                .timeout(Duration::from_secs(5))
                .build(),
        }
    }

    async fn make_test_client_from_mock(mock: &MockServer) -> AsrTaskClient {
        let base_uri = mock.uri();
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().expect("mock server host");
        let port: u16 = parts
            .next()
            .expect("mock server port")
            .parse()
            .expect("valid port number");
        AsrTaskClient::new(host, port)
    }

    fn create_temp_audio_file(dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, b"test-audio").unwrap();
        path
    }

    // --- normalized_subtitle_formats --------------------------------------------

    #[test]
    fn normalized_subtitle_formats_deduplicates_and_normalizes_aliases() {
        let formats = vec![
            "  SRT  ".to_string(),
            "vtt".to_string(),
            "json".to_string(),
            "metadata_json".to_string(),
            "text".to_string(),
            "unknown".to_string(),
            "srt".to_string(),
        ];
        let normalized = normalized_subtitle_formats(&formats);
        assert_eq!(
            normalized,
            vec![
                "srt".to_string(),
                "vtt".to_string(),
                "timeline_json".to_string(),
                "metadata".to_string(),
                "txt".to_string(),
            ]
        );
    }

    #[test]
    fn normalized_subtitle_formats_defaults_when_input_empty() {
        let normalized = normalized_subtitle_formats(&[]);
        assert_eq!(
            normalized,
            vec![
                "srt".to_string(),
                "vtt".to_string(),
                "txt".to_string(),
                "timeline_json".to_string(),
                "metadata".to_string(),
            ]
        );
    }

    #[test]
    fn normalized_subtitle_formats_uses_defaults_when_only_invalid() {
        let normalized = normalized_subtitle_formats(&["??".to_string()]);
        assert_eq!(
            normalized,
            vec![
                "srt".to_string(),
                "vtt".to_string(),
                "txt".to_string(),
                "timeline_json".to_string(),
                "metadata".to_string(),
            ]
        );
    }

    // --- SSE helpers -------------------------------------------------------------

    #[test]
    fn consume_asr_sse_jsonl_ignores_non_data_lines_and_done_marker() {
        let input = b"event: keep-alive\n\
                      data:   \n\
                      data: [DONE]\n\
                      data: {\"value\": 1}\n";
        consume_asr_sse_jsonl(Cursor::new(&input[..]))
            .expect("stream should be consumed successfully");
    }

    #[test]
    fn consume_asr_sse_jsonl_reports_invalid_json() {
        let input = b"data: not-json\n";
        let err = consume_asr_sse_jsonl(Cursor::new(&input[..]))
            .expect_err("invalid JSON should be reported as config error");
        let message = err.to_string();
        assert!(message.contains("invalid JSON"));
        assert!(message.contains("data:"));
    }

    #[test]
    fn sse_last_json_returns_last_valid_event() {
        let stream = "data: {\"a\":1}\n\
                      data: not-json\n\
                      data: {\"b\":2}\n";
        let value = sse_last_json(stream).expect("last JSON event");
        assert_eq!(value["b"], 2);
    }

    // --- AsrTaskClient + JSON helpers -------------------------------------------

    #[tokio::test]
    async fn asr_task_client_get_json_parses_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{ "id": "t1" }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_test_client_with_base_url(mock_server.uri());
        let value = client.get_json("/asr/tasks").expect("JSON should parse");
        assert_eq!(value["tasks"][0]["id"], "t1");
    }

    #[tokio::test]
    async fn asr_task_client_get_json_reports_invalid_json() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/asr/bad"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&mock_server)
            .await;

        let client = make_test_client_with_base_url(mock_server.uri());
        let err = client
            .get_json("/asr/bad")
            .expect_err("invalid JSON should be reported");
        let message = err.to_string();
        assert!(message.contains("returned invalid JSON"));
        assert!(message.contains("body:"));
    }

    #[tokio::test]
    async fn asr_task_client_get_json_reports_http_error_status() {
        let mock_server = MockServer::start().await;

        let long_body = "x".repeat(600);
        Mock::given(method("GET"))
            .and(path("/asr/fail"))
            .respond_with(ResponseTemplate::new(500).set_body_string(long_body.clone()))
            .mount(&mock_server)
            .await;

        let client = make_test_client_with_base_url(mock_server.uri());
        let err = client
            .get_json("/asr/fail")
            .expect_err("HTTP 500 should be reported as config error");
        let message = err.to_string();
        assert!(message.contains("failed with HTTP 500"));
        // Body snippet is included but truncated by helper.
        assert!(message.contains("x"));
    }

    #[tokio::test]
    async fn asr_task_client_post_and_put_json_body_succeed() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-1"
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/asr/tasks/task-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&mock_server)
            .await;

        let client = make_test_client_with_base_url(mock_server.uri());
        let created = client
            .post_json_body("/asr/tasks", &serde_json::json!({"name": "alpha"}))
            .expect("create request should succeed");
        assert_eq!(created["id"], "task-1");

        let updated = client
            .put_json_body("/asr/tasks/task-1", &serde_json::json!({"enabled": true}))
            .expect("update request should succeed");
        assert_eq!(updated["ok"], true);
    }

    #[tokio::test]
    async fn asr_task_client_get_text_reads_plain_body() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/asr/plain"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello text"))
            .mount(&mock_server)
            .await;

        let client = make_test_client_with_base_url(mock_server.uri());
        let body = client.get_text("/asr/plain").expect("plain text response");
        assert_eq!(body, "hello text");
    }

    // --- Offline subtitle pipeline ----------------------------------------------

    #[test]
    fn subtitle_file_with_admin_pipeline_errors_when_audio_missing() {
        let temp = TempDir::new().unwrap();
        let audio = temp.path().join("missing.wav");
        let out = temp.path().join("out");
        let client = make_test_client_with_base_url("http://127.0.0.1:0".to_string());

        let err = subtitle_file_with_admin_pipeline(
            &client,
            &audio,
            "Qwen3-ASR-0.6B",
            "chinese",
            "profile",
            false,
            &["srt".to_string()],
            &out,
            false,
        )
        .expect_err("missing audio should be rejected");
        assert!(err.to_string().contains("audio file does not exist"));
    }

    #[tokio::test]
    async fn subtitle_file_with_admin_pipeline_happy_path_json() {
        let temp = TempDir::new().unwrap();
        let audio = create_temp_audio_file(&temp, "audio.wav");
        let out = temp.path().join("subtitles");

        let mock_server = MockServer::start().await;
        let client = make_test_client_with_base_url(mock_server.uri());

        // 1) Create offline job
        Mock::given(method("POST"))
            .and(path("/asr/offline-jobs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": "job-1"
            })))
            .mount(&mock_server)
            .await;

        // 2) Poll job status (immediately succeeded)
        Mock::given(method("GET"))
            .and(path("/asr/offline-jobs/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": "job-1",
                "status": "succeeded",
                "pipeline_profile": "offline-speaker-subtitle-local"
            })))
            .mount(&mock_server)
            .await;

        // 3) Download artifacts
        Mock::given(method("GET"))
            .and(path("/asr/offline-jobs/job-1/artifacts/srt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("SUBTITLE"))
            .mount(&mock_server)
            .await;

        subtitle_file_with_admin_pipeline(
            &client,
            &audio,
            "Qwen3-ASR-0.6B",
            "chinese",
            "profile",
            false,
            &["srt".to_string()],
            &out,
            true,
        )
        .expect("subtitle pipeline should succeed");

        let srt_path = out.join("audio.srt");
        assert!(srt_path.is_file());
        let contents = std::fs::read_to_string(srt_path).unwrap();
        assert!(contents.contains("SUBTITLE"));
    }

    #[tokio::test]
    async fn subtitle_file_with_admin_pipeline_reports_job_failure() {
        let temp = TempDir::new().unwrap();
        let audio = create_temp_audio_file(&temp, "audio.wav");
        let out = temp.path().join("subtitles");

        let mock_server = MockServer::start().await;
        let client = make_test_client_with_base_url(mock_server.uri());

        Mock::given(method("POST"))
            .and(path("/asr/offline-jobs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": "job-err"
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/asr/offline-jobs/job-err"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": "job-err",
                "status": "failed",
                "error": "boom",
                "pipeline_profile": "offline-speaker-subtitle-local"
            })))
            .mount(&mock_server)
            .await;

        let err = subtitle_file_with_admin_pipeline(
            &client,
            &audio,
            "Qwen3-ASR-0.6B",
            "chinese",
            "profile",
            false,
            &["srt".to_string()],
            &out,
            false,
        )
        .expect_err("failed job should be reported as config error");
        assert!(err
            .to_string()
            .contains("offline subtitle job job-err failed"));
    }

    // --- wait_for_offline_job & download_offline_job_artifacts ------------------

    #[tokio::test]
    async fn wait_for_offline_job_returns_succeeded_job() {
        let mock_server = MockServer::start().await;
        let client = make_test_client_with_base_url(mock_server.uri());

        Mock::given(method("GET"))
            .and(path("/asr/offline-jobs/job-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "job_id": "job-2",
                "status": "succeeded"
            })))
            .mount(&mock_server)
            .await;

        let job = wait_for_offline_job(&client, "job-2").expect("job should succeed");
        assert_eq!(job["status"], "succeeded");
    }

    #[tokio::test]
    async fn download_offline_job_artifacts_writes_expected_files() {
        let temp = TempDir::new().unwrap();
        let audio = create_temp_audio_file(&temp, "clip.wav");
        let out = temp.path().join("artifacts");
        std::fs::create_dir_all(&out).unwrap();

        let mock_server = MockServer::start().await;
        let client = make_test_client_with_base_url(mock_server.uri());

        Mock::given(method("GET"))
            .and(path("/asr/offline-jobs/job-3/artifacts/srt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("SRT"))
            .mount(&mock_server)
            .await;

        let outputs =
            download_offline_job_artifacts(&client, "job-3", &audio, &out, &["srt".to_string()])
                .expect("artifact download");

        assert_eq!(outputs.len(), 1);
        let path = PathBuf::from(outputs[0]["path"].as_str().unwrap());
        assert!(path.is_file());
        assert!(std::fs::read_to_string(path).unwrap().contains("SRT"));
    }

    // --- stream_file_with_admin_speakers ----------------------------------------

    #[test]
    fn stream_file_with_admin_speakers_errors_when_audio_missing() {
        let temp = TempDir::new().unwrap();
        let audio = temp.path().join("missing.wav");
        let client = make_test_client_with_base_url("http://127.0.0.1:0".to_string());

        let err = stream_file_with_admin_speakers(&client, &audio, "model", "lang")
            .expect_err("missing audio should be rejected");
        assert!(err.to_string().contains("audio file does not exist"));
    }

    #[tokio::test]
    async fn stream_file_with_admin_speakers_consumes_sse_stream() {
        let temp = TempDir::new().unwrap();
        let audio = create_temp_audio_file(&temp, "audio.wav");

        let mock_server = MockServer::start().await;
        let client = make_test_client_with_base_url(mock_server.uri());

        Mock::given(method("POST"))
            .and(path("/asr/transcribe-stream"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "data: {\"text\":\"hello\"}\n\
                 data: [DONE]\n",
            ))
            .mount(&mock_server)
            .await;

        stream_file_with_admin_speakers(&client, &audio, "model", "lang")
            .expect("streaming SSE pipeline should succeed");
    }

    // --- create_task & handle_asr_task_command ----------------------------------

    #[tokio::test]
    async fn create_task_builds_expected_request_body_and_handles_json_output() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("audio");
        std::fs::create_dir_all(&dir).unwrap();

        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        Mock::given(method("POST"))
            .and(path("/_bifrost/api/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-123",
                "name": "meetings",
            })))
            .mount(&mock_server)
            .await;

        let input = CreateTaskCliInput {
            name: Some("meetings".to_string()),
            dir,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            runtime_strategy: "reuse_per_file".to_string(),
            time: "02:00".to_string(),
            disabled: false,
            non_recursive: false,
            no_speaker_diarization: false,
            diarization_profile: "sherpa-onnx-balanced".to_string(),
            known_speaker_count: None,
            no_voiceprint_matching: false,
            json: true,
        };

        create_task(&client, input).expect("create_task should succeed");
    }

    #[tokio::test]
    async fn handle_asr_task_command_list_uses_json_or_pretty_output() {
        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        let list_body = serde_json::json!({
            "tasks": [{
                "id": "t1",
                "name": "Alpha",
                "audio_dir": "/tmp/audio",
                "enabled": true,
                "paused": false,
                "model": "Qwen3-ASR-0.6B",
                "language": "chinese",
                "runtime_strategy": {},
                "diarization": {},
                "schedule": {},
                "summary": {"discovered": 1, "processed": 1, "pending": 0},
                "files": [],
                "daily_documents": [],
            }]
        });

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&list_body))
            .mount(&mock_server)
            .await;

        handle_asr_task_command(&client, AiAsrTaskCommands::List { json: true })
            .expect("json list should succeed");

        handle_asr_task_command(&client, AiAsrTaskCommands::List { json: false })
            .expect("pretty list should succeed");
    }

    #[tokio::test]
    async fn handle_asr_task_command_show_and_files_use_parsed_task() {
        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        let detail_body = serde_json::json!({
            "id": "t1",
            "name": "Alpha",
            "audio_dir": "/tmp/audio",
            "enabled": true,
            "paused": false,
            "model": "Qwen3-ASR-0.6B",
            "language": "chinese",
            "runtime_strategy": {},
            "diarization": {},
            "schedule": {},
            "summary": {"discovered": 5, "processed": 3, "pending": 2},
            "files": [
                {
                    "key": "k1",
                    "source_path": "/tmp/audio/a.wav",
                    "status": "success",
                    "media_duration_ms": 1_000u64,
                    "text_chars": 10u64,
                    "output_text_path": "/tmp/text.txt",
                    "output_timeline_path": null,
                    "source_size": 1024u64,
                    "error": null,
                    "diarization_status": "ok",
                    "speaker_count": 1u64,
                    "finished_at_ms": 1_i64,
                }
            ],
            "daily_documents": [],
        });

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/t1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&detail_body))
            .mount(&mock_server)
            .await;

        handle_asr_task_command(
            &client,
            AiAsrTaskCommands::Show {
                task_id: "t1".to_string(),
                json: false,
            },
        )
        .expect("show should succeed");

        handle_asr_task_command(
            &client,
            AiAsrTaskCommands::Files {
                task_id: "t1".to_string(),
                status: None,
                limit: 10,
                json: false,
            },
        )
        .expect("files should succeed");
    }

    #[tokio::test]
    async fn run_task_respects_wait_and_json_flags() {
        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        // POST /run
        Mock::given(method("POST"))
            .and(path("/_bifrost/api/asr/tasks/task-9/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": "started"
            })))
            .mount(&mock_server)
            .await;

        // GET /tasks/task-9 used when wait=true
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/task-9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-9",
                "name": "run-test",
                "audio_dir": "/tmp/audio",
                "enabled": true,
                "paused": false,
                "model": "Qwen3-ASR-0.6B",
                "language": "chinese",
                "runtime_strategy": {},
                "diarization": {},
                "schedule": {},
                "summary": {"discovered": 1, "processed": 1, "pending": 0, "running": false},
                "files": [],
                "daily_documents": [],
            })))
            .mount(&mock_server)
            .await;

        run_task(&client, "task-9", false, true).expect("json fire-and-forget should work");
        run_task(&client, "task-9", true, false).expect("wait=true should poll once");
    }

    // --- diarization profile APIs -----------------------------------------------

    #[tokio::test]
    async fn handle_asr_diarization_command_profiles_status_and_init() {
        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profiles": [
                    {"id": "p1", "engine": "e", "quality_tier": "t", "ready": true}
                ]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profile": {"id": "p1", "engine": "e", "ready": true, "install_dir": "/asr"},
                "voiceprint_dir": "/vp",
                "speaker_profile_count": 1,
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/init-stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("data: {\"status\":{\"profile\":{\"id\":\"p1\"}}}\n"),
            )
            .mount(&mock_server)
            .await;

        handle_asr_diarization_command(&client, AiAsrDiarizationCommands::Profiles { json: false })
            .expect("profiles should succeed");

        handle_asr_diarization_command(
            &client,
            AiAsrDiarizationCommands::Status {
                profile: "p1".to_string(),
                json: false,
            },
        )
        .expect("status should succeed");

        handle_asr_diarization_command(
            &client,
            AiAsrDiarizationCommands::Init {
                profile: "p1".to_string(),
                json: true,
            },
        )
        .expect("init should succeed");
    }

    #[tokio::test]
    async fn handle_asr_diarization_speaker_enroll_live_uses_test_pcm() {
        let temp = TempDir::new().unwrap();
        let audio = create_temp_audio_file(&temp, "prompt.raw");

        let mock_server = MockServer::start().await;
        let client = make_test_client_from_mock(&mock_server).await;

        // Create session with one prompt
        Mock::given(method("POST"))
            .and(path(
                "/_bifrost/api/asr/speaker-profiles/enrollment-sessions",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "session": {
                    "id": "session-1",
                    "prompts": [{"id": "prompt-1", "text": "Say hello"}],
                }
            })))
            .mount(&mock_server)
            .await;

        // Audio chunks
        Mock::given(method("POST"))
            .and(path(
                "/_bifrost/api/asr/speaker-profiles/enrollment-sessions/session-1/audio",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        // Finish
        Mock::given(method("POST"))
            .and(path(
                "/_bifrost/api/asr/speaker-profiles/enrollment-sessions/session-1/finish",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profile": {"id": "sp-1", "display_name": "Tester"}
            })))
            .mount(&mock_server)
            .await;

        enroll_speaker_live(
            &client,
            "Tester",
            "sherpa-onnx-balanced",
            1,
            ":0",
            Some(&audio),
            false,
        )
        .expect("enrollment should succeed with test PCM16");
    }

    // --- CLI asset helpers (model files, tokenizer, etc.) -----------------------

    fn create_model_files(home: &Path, model: &str, include_all: bool) {
        let install = install_dir(home);
        std::fs::create_dir_all(&install).unwrap();
        let model_path = model_dir(home, model);
        std::fs::create_dir_all(&model_path).unwrap();

        std::fs::write(install.join("asr"), b"bin").unwrap();
        std::fs::write(install.join("asr-server"), b"bin").unwrap();

        for file in required_model_files(model) {
            if include_all || *file != "model.safetensors" {
                std::fs::write(model_path.join(file), b"x").unwrap();
            }
        }

        std::fs::create_dir_all(install.join("tokenizers")).unwrap();
        std::fs::write(
            install.join("tokenizers").join("tokenizer-0.6B.json"),
            b"{}",
        )
        .unwrap();
    }

    #[test]
    fn cli_assets_installed_checks_required_files() {
        let temp = TempDir::new().unwrap();
        let home = temp.path();
        let model = "Qwen3-ASR-0.6B";

        assert!(!cli_assets_installed(home, model));

        create_model_files(home, model, true);
        // cli_assets_installed also requires tokenizer.json in the model dir
        std::fs::write(model_dir(home, model).join("tokenizer.json"), b"{}").unwrap();
        assert!(cli_assets_installed(home, model));
    }

    #[test]
    fn prepare_cli_model_copies_tokenizer_into_model_dir() {
        let temp = TempDir::new().unwrap();
        let home = temp.path();
        let model = "Qwen3-ASR-0.6B";

        create_model_files(home, model, true);

        prepare_cli_model(home, model).expect("prepare_cli_model should succeed");

        let tokenizer_path = model_dir(home, model).join("tokenizer.json");
        assert!(tokenizer_path.is_file());
    }

    #[test]
    fn prepare_cli_model_errors_when_model_files_missing() {
        let temp = TempDir::new().unwrap();
        let home = temp.path();
        let model = "Qwen3-ASR-0.6B";

        create_model_files(home, model, false);

        let err = prepare_cli_model(home, model)
            .expect_err("missing model.safetensors should be reported as config error");
        assert!(err
            .to_string()
            .contains("missing ASR model file after download"));
    }

    #[test]
    fn tokenizer_size_supports_known_models_and_rejects_unknown() {
        assert_eq!(tokenizer_size("Qwen3-ASR-0.6B").unwrap(), "0.6B");
        assert_eq!(tokenizer_size("Qwen3-ASR-1.7B").unwrap(), "1.7B");
        let err = tokenizer_size("Other").expect_err("unknown model should error");
        assert!(err.to_string().contains("unsupported ASR model"));
    }

    #[test]
    fn required_model_files_returns_expected_sets() {
        let small = required_model_files("Qwen3-ASR-0.6B");
        assert_eq!(small, &["config.json", "model.safetensors"]);

        let big = required_model_files("Qwen3-ASR-1.7B");
        assert!(big.contains(&"model-00001-of-00002.safetensors"));
        assert!(big.contains(&"model-00002-of-00002.safetensors"));

        let default = required_model_files("unknown-model");
        assert_eq!(default, &["config.json"]);
    }

    #[test]
    fn detect_asr_release_asset_matches_supported_platform() {
        let result = detect_asr_release_asset();
        if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "asr-macos-aarch64");
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn command_succeeds_handles_existing_and_missing_commands() {
        assert!(command_succeeds("true", &[]));
        assert!(!command_succeeds("definitely-not-a-command", &[]));
    }

    // --- Zip extraction & directory copy helpers --------------------------------

    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn extract_zip_to_dir_unpacks_entries() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("test.zip");
        let dest = temp.path().join("out");

        let bytes = build_test_zip(&[("dir/file.txt", b"hello"), ("root.bin", b"x")]);
        std::fs::write(&zip_path, bytes).unwrap();

        extract_zip_to_dir(&zip_path, &dest).expect("zip extraction should succeed");

        assert!(dest.join("dir/file.txt").is_file());
        assert!(dest.join("root.bin").is_file());
    }

    #[test]
    fn copy_dir_contents_recursively_copies_tree() {
        let temp = TempDir::new().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("root.txt"), b"root").unwrap();
        std::fs::write(from.join("sub/nested.txt"), b"nested").unwrap();
        std::fs::create_dir_all(&to).unwrap();

        copy_dir_contents(&from, &to).expect("copy_dir_contents should succeed");

        assert!(to.join("root.txt").is_file());
        assert!(to.join("sub/nested.txt").is_file());
    }

    #[test]
    fn mark_cli_binaries_executable_sets_execute_bits_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let temp = TempDir::new().unwrap();
            let install = temp.path();
            std::fs::create_dir_all(install).unwrap();
            let asr = install.join("asr");
            let asr_server = install.join("asr-server");
            std::fs::write(&asr, b"bin").unwrap();
            std::fs::write(&asr_server, b"bin").unwrap();

            mark_cli_binaries_executable(install).expect("chmod should succeed");

            let mode_asr = std::fs::metadata(&asr).unwrap().permissions().mode();
            let mode_server = std::fs::metadata(&asr_server).unwrap().permissions().mode();
            assert!(mode_asr & 0o111 != 0);
            assert!(mode_server & 0o111 != 0);
        }
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_host_port(mock: &MockServer) -> (String, u16) {
        let base_uri = mock.uri();
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().expect("mock server host").to_string();
        let port: u16 = parts
            .next()
            .expect("mock server port")
            .parse()
            .expect("valid port number");
        (host, port)
    }

    #[tokio::test]
    async fn handle_asr_command_task_list_json_uses_admin_api_client() {
        let mock_server = MockServer::start().await;
        let list_body = serde_json::json!({
            "tasks": [{
                "id": "t1",
                "name": "Alpha",
                "audio_dir": "/tmp/audio",
                "enabled": true,
                "paused": false,
                "model": "Qwen3-ASR-0.6B",
                "language": "chinese",
                "runtime_strategy": {},
                "diarization": {},
                "schedule": {},
                "summary": {"discovered": 1, "processed": 1, "pending": 0},
                "files": [],
                "daily_documents": [],
            }]
        });

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&list_body))
            .mount(&mock_server)
            .await;

        let (host, port) = mock_host_port(&mock_server).await;

        handle_asr_command(
            AiAsrCommands::Task {
                action: AiAsrTaskCommands::List { json: true },
            },
            &host,
            port,
        )
        .expect("task list should succeed via handle_asr_command");
    }

    #[tokio::test]
    async fn handle_asr_command_diarization_profiles_json_uses_admin_api_client() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profiles": [
                    {"id": "p1", "engine": "e", "quality_tier": "t", "ready": true}
                ]
            })))
            .mount(&mock_server)
            .await;

        let (host, port) = mock_host_port(&mock_server).await;

        handle_asr_command(
            AiAsrCommands::Diarization {
                action: AiAsrDiarizationCommands::Profiles { json: true },
            },
            &host,
            port,
        )
        .expect("profiles should succeed via handle_asr_command");
    }

    #[tokio::test]
    async fn handle_ai_command_routes_asr_task_to_handle_asr_command() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": []
            })))
            .mount(&mock_server)
            .await;

        let (host, port) = mock_host_port(&mock_server).await;

        handle_ai_command(
            AiCommands::Asr {
                action: AiAsrCommands::Task {
                    action: AiAsrTaskCommands::List { json: true },
                },
            },
            &host,
            port,
        )
        .expect("handle_ai_command should delegate to handle_asr_command for ASR tasks");
    }
}

#[cfg(test)]
mod coverage_boost_v3 {
    use super::*;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- Formatting helpers: diarization and speaker profiles -----------------

    #[test]
    fn print_diarization_profiles_handles_empty_list() {
        let value = serde_json::json!({ "profiles": [] });
        print_diarization_profiles(&value);
    }

    #[test]
    fn print_diarization_profiles_renders_single_profile() {
        let value = serde_json::json!({
            "profiles": [{
                "id": "p1",
                "engine": "engine-x",
                "quality_tier": "balanced",
                "ready": true
            }]
        });
        print_diarization_profiles(&value);
    }

    #[test]
    fn print_diarization_status_prints_optional_fields() {
        let value = serde_json::json!({
            "profile": {
                "id": "p1",
                "engine": "engine-x",
                "ready": false,
                "install_dir": "/asr",
                "message": "warming-up",
            },
            "voiceprint_dir": "/voiceprints",
            "speaker_profile_count": 3,
        });
        print_diarization_status(&value);
    }

    #[test]
    fn print_speaker_profiles_handles_empty_and_nonempty_lists() {
        let empty = serde_json::json!({ "profiles": [] });
        print_speaker_profiles(&empty);

        let value = serde_json::json!({
            "profiles": [{
                "id": "sp-1",
                "display_name": "Alice",
                "embedding_dim": 256,
            }]
        });
        print_speaker_profiles(&value);
    }

    #[test]
    fn print_speaker_profile_formats_all_fields() {
        let value = serde_json::json!({
            "display_name": "Bob",
            "id": "sp-42",
            "source": "daily-task",
            "diarization_profile": "sherpa-onnx-balanced",
            "total_duration_ms": 12_345u64,
            "embedding_dim": 384u64,
        });
        print_speaker_profile(&value);
    }

    // --- Task list / detail / files / daily documents -------------------------

    fn sample_task_with_file() -> AsrTask {
        let file = AsrTaskFile {
            key: "k1".to_string(),
            source_path: "/audio/a.wav".to_string(),
            status: "failed".to_string(),
            source_size: Some(1234),
            media_duration_ms: Some(90_000),
            output_text_path: Some("/out/a.txt".to_string()),
            output_timeline_path: Some("/out/a.timeline.json".to_string()),
            text_chars: Some(42),
            error: Some("something went wrong".to_string()),
            diarization_status: Some("ok".to_string()),
            speaker_count: Some(2),
            finished_at_ms: Some(1_700_000_000_000),
        };
        AsrTask {
            id: "task-1".to_string(),
            name: "Daily meetings".to_string(),
            audio_dir: "/audio".to_string(),
            enabled: true,
            paused: false,
            schedule: serde_json::json!({"kind":"daily"}),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            summary: AsrTaskSummary {
                discovered: 10,
                processed: 8,
                pending: 2,
                failed: 1,
                partial_success: 1,
                failed_chunk_count: 0,
                deleted_after_processing: 0,
                running: false,
                diarization_enabled: true,
                diarization_ready: true,
                diarization_running: false,
                diarized_files: 5,
                speaker_count: 3,
            },
            files: vec![file],
            daily_documents: vec![AsrDailyDocument {
                date: "2026-05-24".to_string(),
                path: "/reports/2026-05-24.txt".to_string(),
                size: 2048,
                modified_ms: 1_700_000_000_000,
                text_chars: 1000,
            }],
            ..AsrTask::default()
        }
    }

    #[test]
    fn print_task_list_handles_empty_and_nonempty_sets() {
        print_task_list(&[]);

        let task = sample_task_with_file();
        print_task_list(&[task]);
    }

    #[test]
    fn print_task_detail_includes_daily_documents_section() {
        let task = sample_task_with_file();
        print_task_detail(&task);
    }

    #[test]
    fn print_task_files_handles_no_matching_files() {
        let task = AsrTask {
            files: Vec::new(),
            ..Default::default()
        };
        print_task_files(&task, Some("success"), 10);
    }

    #[test]
    fn print_task_files_formats_extended_fields() {
        let task = sample_task_with_file();
        // status=None and a generous limit hit all formatting branches.
        print_task_files(&task, None, 10);
    }

    #[test]
    fn print_daily_documents_handles_empty_and_nonempty_lists() {
        print_daily_documents("task-1", &[]);

        let docs = vec![AsrDailyDocument {
            date: "2026-05-24".to_string(),
            path: "/reports/2026-05-24.txt".to_string(),
            size: 10_240,
            modified_ms: 1_700_000_000_000,
            text_chars: 2048,
        }];
        print_daily_documents("task-1", &docs);
    }

    #[test]
    fn print_daily_agent_sync_result_formats_counts() {
        let value = serde_json::json!({
            "sync": {
                "target_dir": "/reports",
                "total_files": 10,
                "copied_files": 7,
                "skipped_files": 2,
                "failed_files": 1,
            }
        });
        print_daily_agent_sync_result("task-1", &value);
    }

    // --- Status printer helpers ------------------------------------------------

    #[test]
    fn print_status_handles_missing_service_state_for_json_and_text() {
        let temp = TempDir::new().unwrap();
        let previous = bifrost_storage::data_dir();
        bifrost_storage::set_data_dir(temp.path().to_path_buf());

        // With no persisted service state, both JSON and human-readable paths
        // should complete without errors.
        print_status(true).unwrap();
        print_status(false).unwrap();

        bifrost_storage::set_data_dir(previous);
    }

    #[test]
    fn sse_last_json_returns_none_when_no_json_payloads() {
        let stream = "event: keep-alive\n\
                      data: [DONE]\n";
        assert!(sse_last_json(stream).is_none());
    }

    // --- handle_asr_diarization_command pretty-print flows ---------------------

    async fn mock_client_from_server(mock: &MockServer) -> AsrTaskClient {
        let base_uri = mock.uri();
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().unwrap();
        let port: u16 = parts.next().unwrap().parse().unwrap();
        AsrTaskClient::new(host, port)
    }

    #[tokio::test]
    async fn handle_asr_diarization_command_profiles_pretty_output() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profiles": [
                    {"id": "p1", "engine": "e", "quality_tier": "t", "ready": true}
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_diarization_command(&client, AiAsrDiarizationCommands::Profiles { json: false })
            .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_diarization_command_status_pretty_output() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profile": {
                    "id": "p1",
                    "engine": "e",
                    "ready": true,
                    "install_dir": "/asr",
                },
                "voiceprint_dir": "/vp",
                "speaker_profile_count": 1,
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_diarization_command(
            &client,
            AiAsrDiarizationCommands::Status {
                profile: "p1".to_string(),
                json: false,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_diarization_command_init_pretty_output_uses_status_snapshot() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/diarization/init-stream"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "data: {\"status\":{\"profile\":{\"id\":\"p1\",\"engine\":\"e\",\"ready\":true}}}\n",
            ))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_diarization_command(
            &client,
            AiAsrDiarizationCommands::Init {
                profile: "p1".to_string(),
                json: false,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_diarization_speaker_list_and_show_pretty_output() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/speaker-profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "profiles": [
                    {"id": "sp-1", "display_name": "Alice", "embedding_dim": 256}
                ]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/speaker-profiles/sp-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "sp-1",
                "display_name": "Alice",
                "source": "daily-task",
                "diarization_profile": "sherpa-onnx-balanced",
                "total_duration_ms": 10_000u64,
                "embedding_dim": 256u64,
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;

        handle_asr_diarization_speaker_command(
            &client,
            AiAsrDiarizationSpeakerCommands::List { json: false },
        )
        .unwrap();

        handle_asr_diarization_speaker_command(
            &client,
            AiAsrDiarizationSpeakerCommands::Show {
                profile_id: "sp-1".to_string(),
                json: false,
            },
        )
        .unwrap();
    }

    // --- AsrTaskClient extra coverage -----------------------------------------

    #[tokio::test]
    async fn asr_task_client_post_json_parses_simple_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/_bifrost/api/asr/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let value = client.post_json("/asr/ping").unwrap();
        assert_eq!(value["ok"], true);
    }

    // --- parse_task and task selection helpers -------------------------------

    #[test]
    fn parse_task_defaults_missing_fields() {
        let value = serde_json::json!({});
        let task = parse_task(value).expect("empty object should deserialize with defaults");
        assert_eq!(task.id, "");
        assert_eq!(task.name, "");
        assert_eq!(task.summary.discovered, 0);
        assert!(task.files.is_empty());
        assert!(task.daily_documents.is_empty());
    }

    #[test]
    fn parse_task_reports_error_for_invalid_types() {
        let value = serde_json::json!({
            "id": 123,
            "summary": { "discovered": "not-a-number" }
        });
        let err = parse_task(value).expect_err("invalid field types should error");
        let message = err.to_string();
        assert!(message.contains("parse ASR task detail"));
    }

    #[test]
    fn task_choice_label_formats_basic_fields() {
        let choice = AsrTaskWatchChoice {
            task: AsrTaskWatchChoiceTask {
                id: "task-1".to_string(),
                name: "Daily meetings".to_string(),
                enabled: true,
                ..AsrTaskWatchChoiceTask::default()
            },
            progress: AsrTaskWatchChoiceProgress {
                discovered: 10,
                processed: 5,
                pending: 5,
            },
        };
        let label = task_choice_label(&choice);
        assert!(label.contains("Daily meetings"));
        assert!(label.contains("5/10"));
        assert!(label.contains("pending 5"));
    }

    #[tokio::test]
    async fn select_asr_task_id_returns_error_when_no_tasks() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": []
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let err = select_asr_task_id(&client, None).unwrap_err();
        assert!(err.to_string().contains("No ASR directory tasks."));
    }

    #[tokio::test]
    async fn select_asr_task_id_returns_id_for_single_task_without_query() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let id = select_asr_task_id(&client, None).unwrap();
        assert_eq!(id, "t1");
    }

    #[tokio::test]
    async fn select_asr_task_id_uses_query_to_resolve_task_by_name() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [
                    {
                        "task": {
                            "id": "t1",
                            "name": "Alpha",
                            "enabled": true,
                            "paused": false,
                            "running": false,
                            "next_run_at_ms": null
                        },
                        "progress": {"discovered": 1, "processed": 1, "pending": 0}
                    },
                    {
                        "task": {
                            "id": "t2",
                            "name": "Beta",
                            "enabled": true,
                            "paused": false,
                            "running": false,
                            "next_run_at_ms": null
                        },
                        "progress": {"discovered": 0, "processed": 0, "pending": 0}
                    }
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let id = select_asr_task_id(&client, Some("Beta")).unwrap();
        assert_eq!(id, "t2");
    }

    // --- daily command flows --------------------------------------------------

    #[tokio::test]
    async fn handle_asr_task_daily_list_pretty_output_uses_print_daily_documents() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "documents": [{
                    "date": "2026-05-24",
                    "path": "/reports/2026-05-24.txt",
                    "size": 1024u64,
                    "modified_ms": 1_700_000_000_000i64,
                    "text_chars": 100u64
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::List {
                task: None,
                json: false,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_task_daily_list_json_output_uses_print_json() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "documents": []
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::List {
                task: None,
                json: true,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_task_daily_show_pretty_output_without_output_file() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily/2026-05-24"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": "hello world"
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::Show {
                first: "2026-05-24".to_string(),
                second: None,
                task: None,
                output: None,
                json: false,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_task_daily_show_writes_daily_document_to_output_file() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily/2026-05-24"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": "output content"
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("daily.txt");

        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::Show {
                first: "2026-05-24".to_string(),
                second: None,
                task: None,
                output: Some(output_path.clone()),
                json: false,
            },
        )
        .unwrap();

        let content = std::fs::read_to_string(output_path).unwrap();
        assert_eq!(content, "output content");
    }

    #[tokio::test]
    async fn handle_asr_task_daily_set_sync_dir_configures_directory() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "config": { "report_sync_dir": "/tmp/reports" }
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let temp_dir = TempDir::new().unwrap();

        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::SetSyncDir {
                task: None,
                dir: Some(temp_dir.path().to_path_buf()),
                clear: false,
                json: false,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn handle_asr_task_daily_sync_triggers_agent_sync_and_pretty_prints() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/asr/tasks/-/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tasks": [{
                    "task": {
                        "id": "t1",
                        "name": "Alpha",
                        "enabled": true,
                        "paused": false,
                        "running": false,
                        "next_run_at_ms": null
                    },
                    "progress": {"discovered": 1, "processed": 1, "pending": 0}
                }]
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/_bifrost/api/asr/tasks/t1/daily-agent/sync"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sync": {
                    "target_dir": "/tmp/reports",
                    "total_files": 3u64,
                    "copied_files": 2u64,
                    "skipped_files": 1u64,
                    "failed_files": 0u64
                }
            })))
            .mount(&mock_server)
            .await;

        let client = mock_client_from_server(&mock_server).await;
        let temp_dir = TempDir::new().unwrap();

        handle_asr_task_daily_command(
            &client,
            AiAsrTaskDailyCommands::Sync {
                task: None,
                dir: Some(temp_dir.path().to_path_buf()),
                json: false,
            },
        )
        .unwrap();
    }
}
