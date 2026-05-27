use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::{error_response, json_response, json_response_with_status, BoxBody};
use crate::asr_runtime::now_ms;
use crate::handlers::asr_jobs::{
    identify_speaker_voice_from_wav_file, registered_speaker_profile_exists,
    SpeakerVoiceIdentifyResponse, VOICEPRINT_SPEAKER_MATCH_THRESHOLD,
};

const STORE_VERSION: u32 = 1;
const DEFAULT_COOLDOWN_MS: u64 = 1500;
const DEFAULT_KWS_SCORE: f32 = 1.5;
const DEFAULT_KWS_THRESHOLD: f32 = 0.35;
const DEFAULT_SPEAKER_THRESHOLD: f32 = VOICEPRINT_SPEAKER_MATCH_THRESHOLD;
const MAX_EVENTS: usize = 50;
const DEFAULT_LISTENER_CHUNK_MS: u64 = 2500;
const MIN_LISTENER_CHUNK_MS: u64 = 1000;
const MAX_LISTENER_CHUNK_MS: u64 = 10_000;

static VOICE_WAKE_LISTENER: Lazy<Mutex<VoiceWakeListenerRuntime>> =
    Lazy::new(|| Mutex::new(VoiceWakeListenerRuntime::default()));

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceWakeStore {
    version: u32,
    enabled: bool,
    profiles: Vec<VoiceWakeProfile>,
    bindings: Vec<VoiceWakeBinding>,
    events: Vec<VoiceWakeEvent>,
}

impl Default for VoiceWakeStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            enabled: true,
            profiles: Vec::new(),
            bindings: Vec::new(),
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceWakeProfile {
    id: String,
    display_name: String,
    voiceprint_profile_id: Option<String>,
    speaker_threshold: f32,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceWakeBinding {
    id: String,
    enabled: bool,
    phrase: String,
    normalized_phrase: String,
    profile_id: String,
    kws_score: f32,
    kws_threshold: f32,
    speaker_threshold: f32,
    cooldown_ms: u64,
    action: VoiceWakeAction,
    created_at_ms: u64,
    updated_at_ms: u64,
    last_triggered_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceWakeAction {
    KeyPress {
        key: Option<String>,
        keycode: Option<u16>,
        #[serde(default)]
        modifiers: Vec<String>,
        #[serde(default = "default_press_count")]
        press_count: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceWakeEvent {
    id: String,
    binding_id: String,
    phrase: String,
    profile_id: String,
    speaker_confidence: Option<f32>,
    dry_run: bool,
    matched_at_ms: u64,
    action_result: VoiceWakeActionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoiceWakeActionResult {
    action_type: String,
    dry_run: bool,
    executed: bool,
    platform: String,
    message: String,
    command_preview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VoiceWakeProfileCreateRequest {
    id: Option<String>,
    display_name: String,
    #[serde(default)]
    voiceprint_profile_id: Option<String>,
    #[serde(default)]
    speaker_threshold: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct VoiceWakeBindingCreateRequest {
    id: Option<String>,
    phrase: String,
    profile_id: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    kws_score: Option<f32>,
    #[serde(default)]
    kws_threshold: Option<f32>,
    #[serde(default)]
    speaker_threshold: Option<f32>,
    #[serde(default)]
    cooldown_ms: Option<u64>,
    action: VoiceWakeAction,
}

#[derive(Debug, Deserialize)]
struct VoiceWakeTriggerRequest {
    phrase: String,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    speaker_confidence: Option<f32>,
    #[serde(default = "default_dry_run")]
    dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
struct VoiceWakeListenerState {
    running: bool,
    source: String,
    device: Option<String>,
    worker_pid: Option<u32>,
    chunk_ms: u64,
    started_at_ms: Option<u64>,
    stopped_at_ms: Option<u64>,
    last_transcript: Option<String>,
    last_transcript_at_ms: Option<u64>,
    last_error: Option<String>,
    last_error_at_ms: Option<u64>,
    last_speaker_profile_id: Option<String>,
    last_speaker_confidence: Option<f32>,
    last_speaker_status: Option<String>,
    trigger_count: u64,
}

impl Default for VoiceWakeListenerState {
    fn default() -> Self {
        Self {
            running: false,
            source: "mic".to_string(),
            device: None,
            worker_pid: None,
            chunk_ms: DEFAULT_LISTENER_CHUNK_MS,
            started_at_ms: None,
            stopped_at_ms: None,
            last_transcript: None,
            last_transcript_at_ms: None,
            last_error: None,
            last_error_at_ms: None,
            last_speaker_profile_id: None,
            last_speaker_confidence: None,
            last_speaker_status: None,
            trigger_count: 0,
        }
    }
}

#[derive(Default)]
struct VoiceWakeListenerRuntime {
    cancel: Option<Arc<AtomicBool>>,
    worker_pid: Option<u32>,
    task: Option<JoinHandle<()>>,
    state: VoiceWakeListenerState,
}

#[derive(Debug, Clone, Deserialize)]
struct VoiceWakeListenerStartRequest {
    #[serde(default = "default_listener_source")]
    source: String,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    chunk_ms: Option<u64>,
    #[serde(default = "default_execute")]
    execute: bool,
    #[serde(default)]
    mock_transcripts: Vec<String>,
    #[serde(default)]
    mock_interval_ms: Option<u64>,
    #[serde(default)]
    mock_speaker_profile_id: Option<String>,
    #[serde(default)]
    mock_speaker_confidence: Option<f32>,
}

impl Default for VoiceWakeListenerStartRequest {
    fn default() -> Self {
        Self {
            source: default_listener_source(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        }
    }
}

pub(super) async fn handle_voice_wake(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::GET, "/api/voice/wake/status") => get_status_response().await,
        (&Method::GET, "/api/voice/wake/profiles") => get_profiles_response(),
        (&Method::POST, "/api/voice/wake/profiles") => post_profile_response(req).await,
        (&Method::GET, "/api/voice/wake/bindings") => get_bindings_response(),
        (&Method::POST, "/api/voice/wake/bindings") => post_binding_response(req).await,
        (&Method::POST, "/api/voice/wake/trigger") => post_trigger_response(req).await,
        (&Method::POST, "/api/voice/wake/listener/start") => {
            post_listener_start_response(req).await
        }
        (&Method::POST, "/api/voice/wake/listener/stop") => post_listener_stop_response().await,
        (&Method::GET, "/api/voice/wake/events") => get_events_response(),
        (&Method::GET, _) | (&Method::POST, _) => {
            error_response(StatusCode::NOT_FOUND, "Voice wake endpoint not found")
        }
        _ => error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
    }
}

async fn get_status_response() -> Response<BoxBody> {
    let listener = listener_state_snapshot().await;
    match load_store() {
        Ok(store) => json_response(&serde_json::json!({
            "enabled": store.enabled,
            "profile_count": store.profiles.len(),
            "binding_count": store.bindings.len(),
            "event_count": store.events.len(),
            "mode": "backend_asr_listener",
            "store_path": store_path(),
            "default_dry_run": true,
            "listener": listener
        })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn get_profiles_response() -> Response<BoxBody> {
    match load_store() {
        Ok(store) => json_response(&serde_json::json!({ "profiles": store.profiles })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn post_profile_response(req: Request<Incoming>) -> Response<BoxBody> {
    let create = match read_json_body::<VoiceWakeProfileCreateRequest>(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let display_name = create.display_name.trim();
    if display_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "display_name is required");
    }
    let id = match create.id {
        Some(id) => match validate_id(&id, "profile id") {
            Ok(()) => id,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        },
        None => format!("wake_profile_{}", uuid::Uuid::new_v4().as_simple()),
    };
    let threshold = create
        .speaker_threshold
        .unwrap_or(DEFAULT_SPEAKER_THRESHOLD)
        .clamp(0.0, 1.0);
    let Some(voiceprint_profile_id) = create
        .voiceprint_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "voiceprint_profile_id is required; enroll a speaker voiceprint first",
        );
    };
    if !registered_speaker_profile_exists(voiceprint_profile_id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "speaker voiceprint profile not found",
        );
    }
    match update_store(|store| {
        if store.profiles.iter().any(|profile| profile.id == id) {
            return Err("voice wake profile already exists".to_string());
        }
        let now = now_ms();
        let profile = VoiceWakeProfile {
            id,
            display_name: display_name.to_string(),
            voiceprint_profile_id: Some(voiceprint_profile_id.to_string()),
            speaker_threshold: threshold,
            created_at_ms: now,
            updated_at_ms: now,
        };
        store.profiles.push(profile.clone());
        Ok(serde_json::json!(profile))
    }) {
        Ok(value) => json_response_with_status(StatusCode::CREATED, &value),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn get_bindings_response() -> Response<BoxBody> {
    match load_store() {
        Ok(store) => json_response(&serde_json::json!({ "bindings": store.bindings })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn post_binding_response(req: Request<Incoming>) -> Response<BoxBody> {
    let create = match read_json_body::<VoiceWakeBindingCreateRequest>(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let phrase = create.phrase.trim();
    if phrase.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "phrase is required");
    }
    if let Err(error) = validate_key_action(&create.action) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let id = match create.id {
        Some(id) => match validate_id(&id, "binding id") {
            Ok(()) => id,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        },
        None => format!("wake_binding_{}", uuid::Uuid::new_v4().as_simple()),
    };
    match update_store(|store| {
        if store.bindings.iter().any(|binding| binding.id == id) {
            return Err("voice wake binding already exists".to_string());
        }
        let Some(profile) = store
            .profiles
            .iter()
            .find(|profile| profile.id == create.profile_id)
        else {
            return Err("voice wake profile not found".to_string());
        };
        let now = now_ms();
        let binding = VoiceWakeBinding {
            id,
            enabled: create.enabled,
            phrase: phrase.to_string(),
            normalized_phrase: normalize_phrase(phrase),
            profile_id: profile.id.clone(),
            kws_score: create.kws_score.unwrap_or(DEFAULT_KWS_SCORE),
            kws_threshold: create
                .kws_threshold
                .unwrap_or(DEFAULT_KWS_THRESHOLD)
                .clamp(0.0, 1.0),
            speaker_threshold: create
                .speaker_threshold
                .unwrap_or(profile.speaker_threshold)
                .clamp(0.0, 1.0),
            cooldown_ms: create.cooldown_ms.unwrap_or(DEFAULT_COOLDOWN_MS),
            action: create.action,
            created_at_ms: now,
            updated_at_ms: now,
            last_triggered_at_ms: None,
        };
        store.bindings.push(binding.clone());
        Ok(serde_json::json!(binding))
    }) {
        Ok(value) => json_response_with_status(StatusCode::CREATED, &value),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn post_trigger_response(req: Request<Incoming>) -> Response<BoxBody> {
    let trigger = match read_json_body::<VoiceWakeTriggerRequest>(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match update_store(|store| trigger_binding(store, trigger)) {
        Ok(value) => json_response(&value),
        Err(error) if error == "cooldown" => error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "voice wake binding is cooling down",
        ),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn post_listener_start_response(req: Request<Incoming>) -> Response<BoxBody> {
    let (admin_host, admin_port) = listener_admin_endpoint(&req);
    let create = match read_json_or_default_body::<VoiceWakeListenerStartRequest>(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let source = create.source.trim().to_ascii_lowercase();
    if source != "mic" && source != "mock" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "voice wake listener source must be mic or mock",
        );
    }
    if source == "mock" && create.mock_transcripts.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "mock_transcripts is required when source is mock",
        );
    }
    if source == "mic" && std::env::consts::OS != "macos" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "backend microphone listener is currently implemented for macOS",
        );
    }
    let store = match load_store() {
        Ok(store) => store,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if let Some(reason) = listener_start_block_reason(&store) {
        return error_response(StatusCode::BAD_REQUEST, &reason);
    }
    let chunk_ms = create
        .chunk_ms
        .unwrap_or(DEFAULT_LISTENER_CHUNK_MS)
        .clamp(MIN_LISTENER_CHUNK_MS, MAX_LISTENER_CHUNK_MS);
    let mut runtime = VOICE_WAKE_LISTENER.lock().await;
    if runtime
        .task
        .as_ref()
        .map(|task| !task.is_finished())
        .unwrap_or(false)
    {
        return json_response(&serde_json::json!({ "listener": runtime.state }));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let request = VoiceWakeListenerStartRequest {
        source: source.clone(),
        device: create.device.clone(),
        chunk_ms: Some(chunk_ms),
        ..create
    };
    runtime.cancel = Some(cancel.clone());
    runtime.state = VoiceWakeListenerState {
        running: true,
        source,
        device: request.device.clone(),
        worker_pid: None,
        chunk_ms,
        started_at_ms: Some(now_ms()),
        stopped_at_ms: None,
        last_transcript: None,
        last_transcript_at_ms: None,
        last_error: None,
        last_error_at_ms: None,
        last_speaker_profile_id: None,
        last_speaker_confidence: None,
        last_speaker_status: None,
        trigger_count: 0,
    };
    if request.source == "mic" {
        match spawn_voice_wake_worker(&request, &admin_host, admin_port).await {
            Ok((pid, task)) => {
                runtime.worker_pid = Some(pid);
                runtime.state.worker_pid = Some(pid);
                runtime.task = Some(task);
            }
            Err(error) => {
                runtime.cancel = None;
                runtime.state.running = false;
                runtime.state.stopped_at_ms = Some(now_ms());
                runtime.state.last_error = Some(error.clone());
                runtime.state.last_error_at_ms = Some(now_ms());
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
            }
        }
    } else {
        runtime.task = Some(tokio::spawn(async move {
            run_voice_wake_listener(cancel, request).await;
        }));
    }
    json_response(&serde_json::json!({ "listener": runtime.state }))
}

async fn post_listener_stop_response() -> Response<BoxBody> {
    let mut runtime = VOICE_WAKE_LISTENER.lock().await;
    if let Some(cancel) = runtime.cancel.take() {
        cancel.store(true, Ordering::SeqCst);
    }
    if let Some(pid) = runtime.worker_pid.take() {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    if let Some(task) = runtime.task.take() {
        task.abort();
    }
    runtime.state.running = false;
    runtime.state.worker_pid = None;
    runtime.state.stopped_at_ms = Some(now_ms());
    json_response(&serde_json::json!({ "listener": runtime.state }))
}

fn listener_admin_endpoint(req: &Request<Incoming>) -> (String, u16) {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1:9900");
    let port = host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .unwrap_or(9900);
    ("127.0.0.1".to_string(), port)
}

async fn spawn_voice_wake_worker(
    request: &VoiceWakeListenerStartRequest,
    admin_host: &str,
    admin_port: u16,
) -> Result<(u32, JoinHandle<()>), String> {
    let log_path = bifrost_storage::data_dir()
        .join("voice")
        .join("wake")
        .join("listener")
        .join("worker.log");
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("create voice wake worker log dir: {error}"))?;
    }
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("open voice wake worker log {}: {error}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| format!("clone voice wake worker log handle: {error}"))?;
    let current_exe =
        std::env::current_exe().map_err(|error| format!("resolve current executable: {error}"))?;
    let chunk_ms = request
        .chunk_ms
        .unwrap_or(DEFAULT_LISTENER_CHUNK_MS)
        .clamp(MIN_LISTENER_CHUNK_MS, MAX_LISTENER_CHUNK_MS);
    let mut command = tokio::process::Command::new(current_exe);
    command
        .arg("-p")
        .arg(admin_port.to_string())
        .args(["ai", "voice", "wake", "worker"])
        .arg("--admin-host")
        .arg(admin_host)
        .arg("--admin-port")
        .arg(admin_port.to_string())
        .arg("--chunk-ms")
        .arg(chunk_ms.to_string())
        .env(
            "BIFROST_VOICE_WAKE_PARENT_PID",
            std::process::id().to_string(),
        )
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    if let Some(device) = request
        .device
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        command.arg("--device").arg(device);
    }
    if !request.execute {
        command.arg("--dry-run");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn voice wake worker: {error}"))?;
    let pid = child
        .id()
        .ok_or_else(|| "voice wake worker pid is unavailable".to_string())?;
    let task = tokio::spawn(async move {
        let status = child.wait().await;
        update_listener_state(|state| {
            state.running = false;
            state.worker_pid = None;
            state.stopped_at_ms = Some(now_ms());
            if let Err(error) = status {
                state.last_error = Some(format!("voice wake worker wait failed: {error}"));
                state.last_error_at_ms = Some(now_ms());
            }
        })
        .await;
    });
    Ok((pid, task))
}

fn get_events_response() -> Response<BoxBody> {
    match load_store() {
        Ok(store) => json_response(&serde_json::json!({ "events": store.events })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn listener_state_snapshot() -> VoiceWakeListenerState {
    let mut runtime = VOICE_WAKE_LISTENER.lock().await;
    if runtime
        .task
        .as_ref()
        .map(|task| task.is_finished())
        .unwrap_or(false)
    {
        runtime.task = None;
        runtime.cancel = None;
        runtime.worker_pid = None;
        runtime.state.running = false;
        runtime.state.worker_pid = None;
        runtime.state.stopped_at_ms.get_or_insert_with(now_ms);
    }
    runtime.state.clone()
}

async fn update_listener_state<F>(update: F)
where
    F: FnOnce(&mut VoiceWakeListenerState),
{
    let mut runtime = VOICE_WAKE_LISTENER.lock().await;
    update(&mut runtime.state);
}

async fn run_voice_wake_listener(cancel: Arc<AtomicBool>, request: VoiceWakeListenerStartRequest) {
    let result = run_mock_voice_wake_listener(cancel.clone(), &request).await;
    update_listener_state(|state| {
        state.running = false;
        state.stopped_at_ms = Some(now_ms());
        if let Err(error) = result {
            state.last_error = Some(error);
            state.last_error_at_ms = Some(now_ms());
        }
    })
    .await;
}

async fn run_mock_voice_wake_listener(
    cancel: Arc<AtomicBool>,
    request: &VoiceWakeListenerStartRequest,
) -> Result<(), String> {
    let interval = request.mock_interval_ms.unwrap_or(250).clamp(1, 10_000);
    for transcript in &request.mock_transcripts {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        process_listener_transcript(transcript, None, request).await?;
        tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
    }
    Ok(())
}

async fn process_listener_transcript(
    transcript: &str,
    chunk_path: Option<&Path>,
    request: &VoiceWakeListenerStartRequest,
) -> Result<(), String> {
    update_listener_state(|state| {
        state.last_transcript = Some(transcript.to_string());
        state.last_transcript_at_ms = Some(now_ms());
    })
    .await;
    let candidate =
        match load_store().and_then(|store| listener_match_candidate(&store, transcript)) {
            Ok(candidate) => candidate,
            Err(error) if error == "no_match" || error == "cooldown" => return Ok(()),
            Err(error) => {
                record_listener_error(&error).await;
                return Ok(());
            }
        };
    let speaker = match identify_listener_speaker(chunk_path, request) {
        Ok(speaker) => speaker,
        Err(error) => {
            record_listener_error(&error).await;
            return Ok(());
        }
    };
    update_listener_state(|state| {
        state.last_speaker_profile_id = speaker.profile_id.clone();
        state.last_speaker_confidence = Some(speaker.confidence);
        state.last_speaker_status = Some(speaker.status.clone());
    })
    .await;
    let speaker_profile_id = speaker.profile_id.as_deref().unwrap_or("");
    if !speaker.matched
        || speaker_profile_id != candidate.voiceprint_profile_id
        || speaker.confidence < candidate.speaker_threshold
    {
        record_listener_error(&format!(
            "speaker verification failed: expected {}, got {} at {:.3} ({})",
            candidate.voiceprint_profile_id, speaker_profile_id, speaker.confidence, speaker.status
        ))
        .await;
        return Ok(());
    }
    match update_store(|store| {
        trigger_binding_by_id(
            store,
            &candidate.binding_id,
            !request.execute,
            Some(speaker.confidence),
        )
    }) {
        Ok(value) => {
            if value["matched"].as_bool().unwrap_or(false) {
                update_listener_state(|state| {
                    state.trigger_count = state.trigger_count.saturating_add(1);
                    state.last_error = None;
                    state.last_error_at_ms = None;
                })
                .await;
            }
            Ok(())
        }
        Err(error) if error == "no_match" || error == "cooldown" => Ok(()),
        Err(error) => {
            record_listener_error(&error).await;
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
struct ListenerMatchCandidate {
    binding_id: String,
    voiceprint_profile_id: String,
    speaker_threshold: f32,
}

fn listener_match_candidate(
    store: &VoiceWakeStore,
    transcript: &str,
) -> Result<ListenerMatchCandidate, String> {
    if !store.enabled {
        return Err("voice wake actions are disabled".to_string());
    }
    let normalized_transcript = normalize_phrase(transcript);
    let now = now_ms();
    let Some(binding) = store.bindings.iter().find(|binding| {
        binding.enabled
            && !binding.normalized_phrase.is_empty()
            && normalized_transcript.contains(&binding.normalized_phrase)
    }) else {
        return Err("no_match".to_string());
    };
    if let Some(last_triggered_at_ms) = binding.last_triggered_at_ms {
        if now < last_triggered_at_ms.saturating_add(binding.cooldown_ms) {
            return Err("cooldown".to_string());
        }
    }
    let profile = store
        .profiles
        .iter()
        .find(|profile| profile.id == binding.profile_id)
        .ok_or_else(|| "voice wake profile not found".to_string())?;
    let voiceprint_profile_id = profile
        .voiceprint_profile_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "voice wake requires an enrolled speaker voiceprint".to_string())?;
    Ok(ListenerMatchCandidate {
        binding_id: binding.id.clone(),
        voiceprint_profile_id: voiceprint_profile_id.to_string(),
        speaker_threshold: binding.speaker_threshold,
    })
}

fn listener_start_block_reason(store: &VoiceWakeStore) -> Option<String> {
    if store.profiles.iter().all(|profile| {
        profile
            .voiceprint_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    }) {
        return Some(
            "voice wake listener requires an enrolled speaker voiceprint before starting"
                .to_string(),
        );
    }
    let has_enabled_binding_with_voiceprint = store.bindings.iter().any(|binding| {
        binding.enabled
            && store.profiles.iter().any(|profile| {
                profile.id == binding.profile_id
                    && profile
                        .voiceprint_profile_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_some()
            })
    });
    if !has_enabled_binding_with_voiceprint {
        return Some(
            "voice wake listener requires a saved voice command binding before starting"
                .to_string(),
        );
    }
    None
}

fn identify_listener_speaker(
    chunk_path: Option<&Path>,
    request: &VoiceWakeListenerStartRequest,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    if request.source == "mock" {
        let profile_id = request.mock_speaker_profile_id.clone();
        let confidence = request.mock_speaker_confidence.unwrap_or(0.0);
        let matched = profile_id.is_some();
        return Ok(SpeakerVoiceIdentifyResponse {
            matched,
            profile_id,
            display_name: "Mock Speaker".to_string(),
            speaker: "speaker_00".to_string(),
            confidence,
            status: if matched {
                "matched".to_string()
            } else {
                "unmatched".to_string()
            },
            reason: None,
            audio_duration_ms: 0,
            speech_duration_ms: 0,
        });
    }
    let path = chunk_path.ok_or_else(|| "voice wake audio chunk is required".to_string())?;
    identify_speaker_voice_from_wav_file(path)
}

async fn record_listener_error(error: &str) {
    update_listener_state(|state| {
        state.last_error = Some(error.to_string());
        state.last_error_at_ms = Some(now_ms());
    })
    .await;
}

fn trigger_binding(
    store: &mut VoiceWakeStore,
    trigger: VoiceWakeTriggerRequest,
) -> Result<serde_json::Value, String> {
    if !store.enabled {
        return Err("voice wake actions are disabled".to_string());
    }
    let normalized = normalize_phrase(&trigger.phrase);
    let profile_filter = trigger.profile_id.as_deref();
    let now = now_ms();
    let Some(binding_index) = store.bindings.iter().position(|binding| {
        binding.enabled
            && binding.normalized_phrase == normalized
            && profile_filter
                .map(|profile_id| profile_id == binding.profile_id)
                .unwrap_or(true)
    }) else {
        return Err("no enabled voice wake binding matched phrase/profile".to_string());
    };
    let binding = &mut store.bindings[binding_index];
    if let Some(last_triggered_at_ms) = binding.last_triggered_at_ms {
        if now < last_triggered_at_ms.saturating_add(binding.cooldown_ms) {
            return Err("cooldown".to_string());
        }
    }
    if let Some(confidence) = trigger.speaker_confidence {
        if confidence < binding.speaker_threshold {
            return Err(format!(
                "speaker confidence {:.3} below threshold {:.3}",
                confidence, binding.speaker_threshold
            ));
        }
    } else if profile_filter.is_none() {
        return Err(
            "profile_id or speaker_confidence is required for voice wake trigger".to_string(),
        );
    }
    let result = execute_action(&binding.action, trigger.dry_run)?;
    binding.last_triggered_at_ms = Some(now);
    binding.updated_at_ms = now;
    let event = VoiceWakeEvent {
        id: format!("wake_event_{}", uuid::Uuid::new_v4().as_simple()),
        binding_id: binding.id.clone(),
        phrase: binding.phrase.clone(),
        profile_id: binding.profile_id.clone(),
        speaker_confidence: trigger.speaker_confidence,
        dry_run: trigger.dry_run,
        matched_at_ms: now,
        action_result: result.clone(),
    };
    store.events.push(event.clone());
    if store.events.len() > MAX_EVENTS {
        let drain_count = store.events.len() - MAX_EVENTS;
        store.events.drain(0..drain_count);
    }
    Ok(serde_json::json!({
        "matched": true,
        "binding": binding,
        "event": event,
        "action_result": result,
    }))
}

fn trigger_binding_by_id(
    store: &mut VoiceWakeStore,
    binding_id: &str,
    dry_run: bool,
    speaker_confidence: Option<f32>,
) -> Result<serde_json::Value, String> {
    let now = now_ms();
    let Some(binding_index) = store
        .bindings
        .iter()
        .position(|binding| binding.enabled && binding.id == binding_id)
    else {
        return Err("no enabled voice wake binding matched id".to_string());
    };
    trigger_binding_by_index(store, binding_index, now, dry_run, speaker_confidence)
}

fn trigger_binding_by_index(
    store: &mut VoiceWakeStore,
    binding_index: usize,
    now: u64,
    dry_run: bool,
    speaker_confidence: Option<f32>,
) -> Result<serde_json::Value, String> {
    let binding = &mut store.bindings[binding_index];
    if let Some(last_triggered_at_ms) = binding.last_triggered_at_ms {
        if now < last_triggered_at_ms.saturating_add(binding.cooldown_ms) {
            return Err("cooldown".to_string());
        }
    }
    let result = execute_action(&binding.action, dry_run)?;
    binding.last_triggered_at_ms = Some(now);
    binding.updated_at_ms = now;
    let event = VoiceWakeEvent {
        id: format!("wake_event_{}", uuid::Uuid::new_v4().as_simple()),
        binding_id: binding.id.clone(),
        phrase: binding.phrase.clone(),
        profile_id: binding.profile_id.clone(),
        speaker_confidence,
        dry_run,
        matched_at_ms: now,
        action_result: result.clone(),
    };
    store.events.push(event.clone());
    if store.events.len() > MAX_EVENTS {
        let drain_count = store.events.len() - MAX_EVENTS;
        store.events.drain(0..drain_count);
    }
    Ok(serde_json::json!({
        "matched": true,
        "binding": binding,
        "event": event,
        "action_result": result,
    }))
}

fn execute_action(
    action: &VoiceWakeAction,
    dry_run: bool,
) -> Result<VoiceWakeActionResult, String> {
    match action {
        VoiceWakeAction::KeyPress {
            key,
            keycode,
            modifiers,
            press_count,
        } => execute_key_press(key.as_deref(), *keycode, modifiers, *press_count, dry_run),
    }
}

fn execute_key_press(
    key: Option<&str>,
    keycode: Option<u16>,
    modifiers: &[String],
    press_count: u8,
    dry_run: bool,
) -> Result<VoiceWakeActionResult, String> {
    let press_count = press_count.clamp(1, 8);
    let script = build_macos_keypress_script(key, keycode, modifiers, press_count)?;
    if dry_run {
        return Ok(VoiceWakeActionResult {
            action_type: "key_press".to_string(),
            dry_run: true,
            executed: false,
            platform: std::env::consts::OS.to_string(),
            message: "dry-run: key press was matched but not executed".to_string(),
            command_preview: Some(script),
        });
    }
    if std::env::consts::OS != "macos" {
        return Err(format!(
            "key_press execution is currently implemented only on macOS; current platform is {}",
            std::env::consts::OS
        ));
    }
    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .map_err(|error| format!("run osascript key_press: {error}"))?;
    if !status.success() {
        return Err(format!(
            "osascript key_press failed with status {status}; grant Accessibility permission to the terminal/Bifrost process"
        ));
    }
    Ok(VoiceWakeActionResult {
        action_type: "key_press".to_string(),
        dry_run: false,
        executed: true,
        platform: std::env::consts::OS.to_string(),
        message: "key press executed".to_string(),
        command_preview: Some(script),
    })
}

fn build_macos_keypress_script(
    key: Option<&str>,
    keycode: Option<u16>,
    modifiers: &[String],
    press_count: u8,
) -> Result<String, String> {
    let normalized_modifiers = normalize_modifiers(modifiers)?;
    let using_clause = if normalized_modifiers.is_empty() {
        String::new()
    } else {
        format!(" using {{{}}}", normalized_modifiers.join(", "))
    };
    let action = if let Some(keycode) = keycode {
        format!("key code {keycode}{using_clause}")
    } else {
        let key = key
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| "key or keycode is required for key_press".to_string())?;
        if let Some(code) = named_keycode(key) {
            format!("key code {code}{using_clause}")
        } else if key.chars().count() == 1 {
            format!("keystroke \"{}\"{using_clause}", escape_applescript(key))
        } else {
            return Err(format!(
                "unsupported key '{key}'; use a single character, one of space/return/tab/escape, or keycode"
            ));
        }
    };
    if press_count <= 1 {
        return Ok(format!("tell application \"System Events\" to {action}"));
    }
    Ok(format!(
        "tell application \"System Events\" to repeat {press_count} times\n  {action}\nend repeat"
    ))
}

fn named_keycode(key: &str) -> Option<u16> {
    match key.to_ascii_lowercase().as_str() {
        "space" => Some(49),
        "return" | "enter" => Some(36),
        "tab" => Some(48),
        "escape" | "esc" => Some(53),
        "left" => Some(123),
        "right" => Some(124),
        "down" => Some(125),
        "up" => Some(126),
        _ => None,
    }
}

fn normalize_modifiers(modifiers: &[String]) -> Result<Vec<&'static str>, String> {
    modifiers
        .iter()
        .map(
            |modifier| match modifier.trim().to_ascii_lowercase().as_str() {
                "" => Err("modifier cannot be empty".to_string()),
                "cmd" | "command" | "meta" => Ok("command down"),
                "shift" => Ok("shift down"),
                "ctrl" | "control" => Ok("control down"),
                "option" | "alt" => Ok("option down"),
                other => Err(format!("unsupported key modifier '{other}'")),
            },
        )
        .collect()
}

fn validate_key_action(action: &VoiceWakeAction) -> Result<(), String> {
    match action {
        VoiceWakeAction::KeyPress {
            key,
            keycode,
            modifiers,
            press_count,
        } => {
            if key.as_deref().unwrap_or("").trim().is_empty() && keycode.is_none() {
                return Err("key_press action requires key or keycode".to_string());
            }
            if *press_count == 0 || *press_count > 8 {
                return Err("key_press press_count must be between 1 and 8".to_string());
            }
            build_macos_keypress_script(key.as_deref(), *keycode, modifiers, *press_count)
                .map(|_| ())
        }
    }
}

async fn read_json_body<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {error}"),
            )
        })?
        .to_bytes();
    serde_json::from_slice::<T>(&body)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}")))
}

async fn read_json_or_default_body<T>(req: Request<Incoming>) -> Result<T, Response<BoxBody>>
where
    T: serde::de::DeserializeOwned + Default,
{
    let body = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {error}"),
            )
        })?
        .to_bytes();
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(&body)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}")))
}

fn load_store() -> Result<VoiceWakeStore, String> {
    let path = store_path();
    if !path.exists() {
        return Ok(VoiceWakeStore::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|error| format!("read voice wake store {}: {error}", path.display()))?;
    serde_json::from_str::<VoiceWakeStore>(&data)
        .map_err(|error| format!("parse voice wake store {}: {error}", path.display()))
}

fn update_store<F>(update: F) -> Result<serde_json::Value, String>
where
    F: FnOnce(&mut VoiceWakeStore) -> Result<serde_json::Value, String>,
{
    let mut store = load_store()?;
    let value = update(&mut store)?;
    write_store(&store)?;
    Ok(value)
}

fn write_store(store: &VoiceWakeStore) -> Result<(), String> {
    let path = store_path();
    atomic_json_write(&path, store)
}

fn store_path() -> PathBuf {
    bifrost_storage::data_dir()
        .join("voice")
        .join("wake")
        .join("actions.json")
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|error| format!("write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|error| format!("rename {} -> {}: {error}", tmp.display(), path.display()))
}

fn normalize_phrase(phrase: &str) -> String {
    phrase.to_lowercase().split_whitespace().collect()
}

fn validate_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 96
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn default_enabled() -> bool {
    true
}

fn default_dry_run() -> bool {
    true
}

fn default_execute() -> bool {
    true
}

fn default_listener_source() -> String {
    "mic".to_string()
}

fn default_press_count() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_phrase_whitespace_without_changing_text() {
        assert_eq!(normalize_phrase("  打开   录音 "), "打开录音");
    }

    #[test]
    fn builds_safe_macos_keypress_script_for_named_key() {
        let script = build_macos_keypress_script(
            Some("space"),
            None,
            &["cmd".to_string(), "shift".to_string()],
            1,
        )
        .expect("script");
        assert_eq!(
            script,
            "tell application \"System Events\" to key code 49 using {command down, shift down}"
        );
    }

    #[test]
    fn validates_key_press_key_at_binding_create_time() {
        let error = validate_key_action(&VoiceWakeAction::KeyPress {
            key: Some("unsupported_multi_key".to_string()),
            keycode: None,
            modifiers: Vec::new(),
            press_count: 1,
        })
        .unwrap_err();
        assert!(error.contains("unsupported key"));
    }

    #[test]
    fn trigger_requires_matching_profile_or_confidence() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![VoiceWakeProfile {
                id: "p1".to_string(),
                display_name: "Eden".to_string(),
                voiceprint_profile_id: None,
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                created_at_ms: now,
                updated_at_ms: now,
            }],
            bindings: vec![VoiceWakeBinding {
                id: "b1".to_string(),
                enabled: true,
                phrase: "打开录音".to_string(),
                normalized_phrase: "打开录音".to_string(),
                profile_id: "p1".to_string(),
                kws_score: DEFAULT_KWS_SCORE,
                kws_threshold: DEFAULT_KWS_THRESHOLD,
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                cooldown_ms: DEFAULT_COOLDOWN_MS,
                action: VoiceWakeAction::KeyPress {
                    key: Some("space".to_string()),
                    keycode: None,
                    modifiers: Vec::new(),
                    press_count: 1,
                },
                created_at_ms: now,
                updated_at_ms: now,
                last_triggered_at_ms: None,
            }],
            ..VoiceWakeStore::default()
        };
        let value = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: Some("p1".to_string()),
                speaker_confidence: None,
                dry_run: true,
            },
        )
        .expect("trigger");
        assert_eq!(value["matched"], true);
        assert_eq!(store.events.len(), 1);
    }

    #[test]
    fn backend_listener_requires_enrolled_voiceprint_profile() {
        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![VoiceWakeProfile {
                id: "p1".to_string(),
                display_name: "Eden".to_string(),
                voiceprint_profile_id: None,
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                created_at_ms: now,
                updated_at_ms: now,
            }],
            bindings: vec![VoiceWakeBinding {
                id: "b1".to_string(),
                enabled: true,
                phrase: "打开录音".to_string(),
                normalized_phrase: normalize_phrase("打开录音"),
                profile_id: "p1".to_string(),
                kws_score: DEFAULT_KWS_SCORE,
                kws_threshold: DEFAULT_KWS_THRESHOLD,
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                cooldown_ms: DEFAULT_COOLDOWN_MS,
                action: VoiceWakeAction::KeyPress {
                    key: Some("escape".to_string()),
                    keycode: None,
                    modifiers: Vec::new(),
                    press_count: 1,
                },
                created_at_ms: now,
                updated_at_ms: now,
                last_triggered_at_ms: None,
            }],
            ..VoiceWakeStore::default()
        };
        let error = listener_match_candidate(&store, "现在 打开 录音").unwrap_err();
        assert!(error.contains("enrolled speaker voiceprint"));
    }

    #[test]
    fn listener_start_requires_voiceprint_and_binding() {
        let now = now_ms();
        let empty = VoiceWakeStore::default();
        assert!(listener_start_block_reason(&empty)
            .expect("blocked")
            .contains("speaker voiceprint"));

        let no_binding = VoiceWakeStore {
            profiles: vec![VoiceWakeProfile {
                id: "p1".to_string(),
                display_name: "Eden".to_string(),
                voiceprint_profile_id: Some("speaker_eden".to_string()),
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                created_at_ms: now,
                updated_at_ms: now,
            }],
            ..VoiceWakeStore::default()
        };
        assert!(listener_start_block_reason(&no_binding)
            .expect("blocked")
            .contains("saved voice command"));
    }

    #[test]
    fn backend_listener_event_records_speaker_confidence() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![VoiceWakeProfile {
                id: "p1".to_string(),
                display_name: "Eden".to_string(),
                voiceprint_profile_id: Some("speaker_eden".to_string()),
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                created_at_ms: now,
                updated_at_ms: now,
            }],
            bindings: vec![VoiceWakeBinding {
                id: "b1".to_string(),
                enabled: true,
                phrase: "打开录音".to_string(),
                normalized_phrase: normalize_phrase("打开录音"),
                profile_id: "p1".to_string(),
                kws_score: DEFAULT_KWS_SCORE,
                kws_threshold: DEFAULT_KWS_THRESHOLD,
                speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
                cooldown_ms: DEFAULT_COOLDOWN_MS,
                action: VoiceWakeAction::KeyPress {
                    key: Some("escape".to_string()),
                    keycode: None,
                    modifiers: Vec::new(),
                    press_count: 1,
                },
                created_at_ms: now,
                updated_at_ms: now,
                last_triggered_at_ms: None,
            }],
            ..VoiceWakeStore::default()
        };
        let value = trigger_binding_by_id(&mut store, "b1", true, Some(0.93)).expect("trigger");
        assert_eq!(value["matched"], true);
        assert_eq!(store.events.len(), 1);
        assert_eq!(store.events[0].speaker_confidence, Some(0.93));
    }
}
