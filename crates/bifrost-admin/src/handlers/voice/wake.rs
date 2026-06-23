use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use bifrost_asr::wake::{
    normalize_wake_phrase, wake_phrase_matches, SherpaKwsModelPack, DEFAULT_WAKE_KWS_ARCHIVE_URL,
    DEFAULT_WAKE_KWS_PROFILE,
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
const MAX_LISTENER_CHUNK_MS: u64 = 4_000;

static VOICE_WAKE_LISTENER: Lazy<Mutex<VoiceWakeListenerRuntime>> =
    Lazy::new(|| Mutex::new(VoiceWakeListenerRuntime::default()));

#[cfg(test)]
static VOICE_WAKE_LISTENER_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

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
        #[serde(default = "default_repeat_delay_ms")]
        repeat_delay_ms: u64,
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

#[derive(Debug, Deserialize)]
struct VoiceWakeListenerProgressRequest {
    status: String,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    device_label: Option<String>,
    #[serde(default)]
    transcript: Option<String>,
    #[serde(default)]
    phrase: Option<String>,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    binding_id: Option<String>,
    #[serde(default)]
    speaker_profile_id: Option<String>,
    #[serde(default)]
    speaker_confidence: Option<f32>,
    #[serde(default)]
    speaker_status: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct VoiceWakeListenerState {
    running: bool,
    source: String,
    engine: String,
    device: Option<String>,
    device_label: Option<String>,
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
    last_match_status: Option<String>,
    last_match_binding_id: Option<String>,
    last_match_phrase: Option<String>,
    last_action_result: Option<VoiceWakeActionResult>,
    last_action_at_ms: Option<u64>,
    trigger_count: u64,
    /// Model download progress: "downloading", "extracting", "downloaded", "failed"
    model_download_status: Option<String>,
    /// Bytes downloaded so far
    model_download_progress: Option<u64>,
    /// Total bytes (from Content-Length)
    model_download_total: Option<u64>,
}

impl Default for VoiceWakeListenerState {
    fn default() -> Self {
        Self {
            running: false,
            source: "mic".to_string(),
            engine: default_listener_engine(),
            device: None,
            device_label: None,
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
            last_match_status: None,
            last_match_binding_id: None,
            last_match_phrase: None,
            last_action_result: None,
            last_action_at_ms: None,
            trigger_count: 0,
            model_download_status: None,
            model_download_progress: None,
            model_download_total: None,
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
    #[serde(default = "default_listener_engine")]
    engine: String,
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
            engine: default_listener_engine(),
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
        (&Method::GET, "/api/voice/wake/kws/status") => get_kws_status_response(),
        (&Method::POST, "/api/voice/wake/kws/init") => post_kws_init_response(),
        (&Method::GET, "/api/voice/wake/profiles") => get_profiles_response(),
        (&Method::POST, "/api/voice/wake/profiles") => post_profile_response(req).await,
        (&Method::GET, "/api/voice/wake/bindings") => get_bindings_response(),
        (&Method::POST, "/api/voice/wake/bindings") => post_binding_response(req).await,
        (&Method::POST, "/api/voice/wake/trigger") => post_trigger_response(req).await,
        (&Method::POST, "/api/voice/wake/listener/start") => {
            post_listener_start_response(req).await
        }
        (&Method::POST, "/api/voice/wake/listener/progress") => {
            post_listener_progress_response(req).await
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
    let default_engine = default_listener_engine();
    let kws_pack = voice_wake_kws_model_pack();
    match load_store() {
        Ok(store) => json_response(&serde_json::json!({
            "enabled": store.enabled,
            "profile_count": store.profiles.len(),
            "binding_count": store.bindings.len(),
            "event_count": store.events.len(),
            "mode": default_engine,
            "fallback": serde_json::Value::Null,
            "requires_qwen_by_default": false,
            "kws": {
                "engine": "sherpa-onnx",
                "profile": DEFAULT_WAKE_KWS_PROFILE,
                "ready": kws_pack.is_ready(),
                "install_dir": kws_pack.root_dir,
                "missing_files": kws_pack.missing_files(),
            },
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

fn get_kws_status_response() -> Response<BoxBody> {
    json_response(&voice_wake_kws_status_json())
}

fn post_kws_init_response() -> Response<BoxBody> {
    match prepare_voice_wake_kws_profile() {
        Ok(()) => json_response(&serde_json::json!({
            "profile": DEFAULT_WAKE_KWS_PROFILE,
            "status": voice_wake_kws_status_json(),
            "message": "voice wake KWS profile initialized"
        })),
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
    let voiceprint_profile_id = create
        .voiceprint_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(voiceprint_profile_id) = voiceprint_profile_id {
        if !registered_speaker_profile_exists(voiceprint_profile_id) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "speaker voiceprint profile not found",
            );
        }
    }
    let stored_voiceprint_profile_id = voiceprint_profile_id.map(ToString::to_string);
    match update_store(|store| {
        if store.profiles.iter().any(|profile| profile.id == id) {
            return Err("voice wake profile already exists".to_string());
        }
        let now = now_ms();
        let profile = VoiceWakeProfile {
            id,
            display_name: display_name.to_string(),
            voiceprint_profile_id: stored_voiceprint_profile_id,
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
    let engine = create.engine.trim().to_ascii_lowercase();
    if engine != "lightweight_kws_listener" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "voice wake listener engine must be lightweight_kws_listener",
        );
    }
    if source == "mic" && std::env::consts::OS != "macos" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "voice wake microphone listener is currently implemented for macOS",
        );
    }
    let needs_model_download = if source == "mic" && engine == "lightweight_kws_listener" {
        let pack = voice_wake_kws_model_pack();
        if !pack.is_ready() {
            tracing::info!(
                profile = DEFAULT_WAKE_KWS_PROFILE,
                install_dir = %pack.root_dir.display(),
                "voice wake KWS assets are missing; will download asynchronously"
            );
            true
        } else {
            false
        }
    } else {
        false
    };
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
        engine: engine.clone(),
        device: create.device.clone(),
        chunk_ms: Some(chunk_ms),
        ..create
    };
    runtime.cancel = Some(cancel.clone());
    runtime.state = VoiceWakeListenerState {
        running: true,
        source,
        engine,
        device: request.device.clone(),
        device_label: None,
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
        last_match_status: None,
        last_match_binding_id: None,
        last_match_phrase: None,
        last_action_result: None,
        last_action_at_ms: None,
        trigger_count: 0,
        model_download_status: None,
        model_download_progress: None,
        model_download_total: None,
    };
    if request.source == "mic" && request.engine == "lightweight_kws_listener" {
        if needs_model_download {
            // Set initial download status and spawn async download + start task
            runtime.state.model_download_status = Some("downloading".to_string());
            runtime.state.model_download_progress = Some(0);
            let admin_host_clone = admin_host.clone();
            let request_clone = request.clone();
            let cancel_clone = cancel.clone();
            runtime.task = Some(tokio::spawn(async move {
                match download_and_start_listener(
                    cancel_clone,
                    request_clone,
                    admin_host_clone,
                    admin_port,
                )
                .await
                {
                    Ok(()) => {}
                    Err(error) => {
                        tracing::error!(%error, "voice wake async model download/start failed");
                        update_listener_state(|state| {
                            state.model_download_status = Some("failed".to_string());
                            state.last_error = Some(error);
                            state.last_error_at_ms = Some(now_ms());
                            state.running = false;
                            state.stopped_at_ms = Some(now_ms());
                        })
                        .await;
                    }
                }
            }));
        } else {
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
    runtime.state.last_match_status = None;
    json_response(&serde_json::json!({ "listener": runtime.state }))
}

async fn post_listener_progress_response(req: Request<Incoming>) -> Response<BoxBody> {
    let progress = match read_json_body::<VoiceWakeListenerProgressRequest>(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let status = progress.status.trim();
    if status.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "listener progress status is required",
        );
    }
    let now = now_ms();
    let mut runtime = VOICE_WAKE_LISTENER.lock().await;
    if !runtime.state.running {
        return json_response(&serde_json::json!({ "listener": runtime.state }));
    }
    if let Some(device) = progress
        .device
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        runtime.state.device = Some(device.to_string());
    }
    if let Some(device_label) = progress
        .device_label
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        runtime.state.device_label = Some(device_label.to_string());
    }
    runtime.state.last_match_status = Some(status.to_string());
    if let Some(transcript) = progress.transcript {
        runtime.state.last_transcript = Some(transcript);
        runtime.state.last_transcript_at_ms = Some(now);
    }
    if let Some(phrase) = progress.phrase {
        runtime.state.last_match_phrase = Some(phrase);
    }
    if let Some(match_id) = progress.binding_id.or(progress.profile_id) {
        runtime.state.last_match_binding_id = Some(match_id);
    }
    if let Some(speaker_profile_id) = progress.speaker_profile_id {
        runtime.state.last_speaker_profile_id = Some(speaker_profile_id);
    }
    if let Some(speaker_confidence) = progress.speaker_confidence {
        runtime.state.last_speaker_confidence = Some(speaker_confidence);
    }
    if let Some(speaker_status) = progress.speaker_status {
        runtime.state.last_speaker_status = Some(speaker_status);
    }
    if let Some(error) = progress.error {
        runtime.state.last_error = Some(error);
        runtime.state.last_error_at_ms = Some(now);
    } else if matches!(
        status,
        "capturing" | "captured" | "transcribing" | "recognized" | "no_match"
    ) {
        runtime.state.last_error = None;
        runtime.state.last_error_at_ms = None;
    }
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
    let current_exe = labeled_process_executable(&current_exe, "bifrost-voice");
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
        .arg("--engine")
        .arg(request.engine.clone())
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

fn labeled_process_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    match bifrost_core::process_alias_executable(executable, &alias_dir, alias_name) {
        Ok(alias) => alias,
        Err(error) => {
            tracing::warn!(
                executable = %executable.display(),
                alias_name = %alias_name,
                error = %error,
                "falling back to unlabeled bifrost voice wake executable"
            );
            executable.to_path_buf()
        }
    }
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
    let result = if request.source == "mock" {
        run_mock_voice_wake_listener(cancel.clone(), &request).await
    } else {
        run_lightweight_mic_wake_listener(cancel.clone(), &request).await
    };
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

async fn run_lightweight_mic_wake_listener(
    cancel: Arc<AtomicBool>,
    request: &VoiceWakeListenerStartRequest,
) -> Result<(), String> {
    let interval = request
        .chunk_ms
        .unwrap_or(DEFAULT_LISTENER_CHUNK_MS)
        .clamp(MIN_LISTENER_CHUNK_MS, MAX_LISTENER_CHUNK_MS);
    while !cancel.load(Ordering::SeqCst) {
        update_listener_state(|state| {
            state.last_speaker_status = Some("lightweight_kws_listener_idle".to_string());
        })
        .await;
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
        state.last_match_status = Some("recognized".to_string());
        state.last_match_binding_id = None;
        state.last_match_phrase = None;
        state.last_action_result = None;
        state.last_action_at_ms = None;
    })
    .await;
    let candidate =
        match load_store().and_then(|store| listener_match_candidate(&store, transcript)) {
            Ok(candidate) => candidate,
            Err(error) if error == "no_match" || error == "cooldown" => {
                update_listener_state(|state| {
                    state.last_match_status = Some(error);
                })
                .await;
                return Ok(());
            }
            Err(error) => {
                record_listener_error(&error).await;
                return Ok(());
            }
        };
    update_listener_state(|state| {
        state.last_match_status = Some("phrase_matched".to_string());
        state.last_match_binding_id = Some(candidate.binding_id.clone());
        state.last_match_phrase = Some(candidate.phrase.clone());
    })
    .await;
    let speaker_confidence =
        if let Some(expected_voiceprint_profile_id) = candidate.voiceprint_profile_id.as_deref() {
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
                || speaker_profile_id != expected_voiceprint_profile_id
                || speaker.confidence < candidate.speaker_threshold
            {
                update_listener_state(|state| {
                    state.last_match_status = Some("speaker_rejected".to_string());
                })
                .await;
                record_listener_error(&format!(
                    "speaker verification failed: expected {}, got {} at {:.3} ({})",
                    expected_voiceprint_profile_id,
                    speaker_profile_id,
                    speaker.confidence,
                    speaker.status
                ))
                .await;
                return Ok(());
            }
            Some(speaker.confidence)
        } else {
            update_listener_state(|state| {
                state.last_speaker_profile_id = None;
                state.last_speaker_confidence = None;
                state.last_speaker_status = Some("phrase_only".to_string());
                state.last_match_status = Some("speaker_allowed".to_string());
            })
            .await;
            None
        };
    let dry_run = !request.execute;
    match update_store(|store| {
        trigger_binding_by_id(store, &candidate.binding_id, dry_run, speaker_confidence)
    }) {
        Ok(value) => {
            if value["matched"].as_bool().unwrap_or(false) {
                let action_result =
                    serde_json::from_value::<VoiceWakeActionResult>(value["action_result"].clone())
                        .ok();
                update_listener_state(|state| {
                    state.trigger_count = state.trigger_count.saturating_add(1);
                    state.last_error = None;
                    state.last_error_at_ms = None;
                    state.last_match_status = action_result
                        .as_ref()
                        .map(|result| {
                            if result.executed {
                                "executed".to_string()
                            } else if result.dry_run {
                                "dry_run_matched".to_string()
                            } else {
                                "matched".to_string()
                            }
                        })
                        .or_else(|| Some("matched".to_string()));
                    state.last_action_result = action_result;
                    state.last_action_at_ms = Some(now_ms());
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
    phrase: String,
    voiceprint_profile_id: Option<String>,
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
            && phrase_matches(&normalized_transcript, &binding.normalized_phrase)
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
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Ok(ListenerMatchCandidate {
        binding_id: binding.id.clone(),
        phrase: binding.phrase.clone(),
        voiceprint_profile_id,
        speaker_threshold: binding.speaker_threshold,
    })
}

fn listener_start_block_reason(store: &VoiceWakeStore) -> Option<String> {
    let has_enabled_binding = store.bindings.iter().any(|binding| {
        binding.enabled
            && store
                .profiles
                .iter()
                .any(|profile| profile.id == binding.profile_id)
    });
    if !has_enabled_binding {
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
            && phrase_matches(&normalized, &binding.normalized_phrase)
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
    if !trigger.dry_run && trigger.speaker_confidence.is_none() {
        // Only require speaker_confidence when the binding's profile has voiceprint configured
        let profile_requires_voiceprint = store
            .profiles
            .iter()
            .find(|p| p.id == binding.profile_id)
            .and_then(|p| p.voiceprint_profile_id.as_deref())
            .map(|id| !id.trim().is_empty())
            .unwrap_or(false);
        if profile_requires_voiceprint {
            return Err(
                "speaker_confidence is required to execute voice wake action with voiceprint"
                    .to_string(),
            );
        }
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
            repeat_delay_ms,
        } => execute_key_press(
            key.as_deref(),
            *keycode,
            modifiers,
            *press_count,
            *repeat_delay_ms,
            dry_run,
        ),
    }
}

fn execute_key_press(
    key: Option<&str>,
    keycode: Option<u16>,
    modifiers: &[String],
    press_count: u8,
    repeat_delay_ms: u64,
    dry_run: bool,
) -> Result<VoiceWakeActionResult, String> {
    let press_count = press_count.clamp(1, 8);
    let repeat_delay_ms = repeat_delay_ms.min(5_000);
    let script =
        build_macos_keypress_script(key, keycode, modifiers, press_count, repeat_delay_ms)?;
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
    repeat_delay_ms: u64,
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
    let delay_seconds = format_repeat_delay_seconds(repeat_delay_ms);
    Ok(format!(
        "tell application \"System Events\" to repeat with pressIndex from 1 to {press_count}\n  {action}\n  if pressIndex is less than {press_count} then delay {delay_seconds}\nend repeat"
    ))
}

fn format_repeat_delay_seconds(delay_ms: u64) -> String {
    let seconds = delay_ms as f64 / 1000.0;
    let formatted = format!("{seconds:.3}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
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
        "cmd" | "command" | "meta" => Some(55),
        "shift" => Some(56),
        "option" | "alt" => Some(58),
        "ctrl" | "control" => Some(59),
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
            repeat_delay_ms,
        } => {
            if key.as_deref().unwrap_or("").trim().is_empty() && keycode.is_none() {
                return Err("key_press action requires key or keycode".to_string());
            }
            if *press_count == 0 || *press_count > 8 {
                return Err("key_press press_count must be between 1 and 8".to_string());
            }
            if *repeat_delay_ms > 5_000 {
                return Err("key_press repeat_delay_ms must be between 0 and 5000".to_string());
            }
            build_macos_keypress_script(
                key.as_deref(),
                *keycode,
                modifiers,
                *press_count,
                *repeat_delay_ms,
            )
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

fn voice_wake_kws_model_pack() -> SherpaKwsModelPack {
    SherpaKwsModelPack::for_root(
        bifrost_storage::data_dir()
            .join("asr")
            .join("wake")
            .join("kws")
            .join(DEFAULT_WAKE_KWS_PROFILE),
    )
}

fn voice_wake_kws_status_json() -> serde_json::Value {
    let pack = voice_wake_kws_model_pack();
    serde_json::json!({
        "engine": "sherpa-onnx",
        "profile": DEFAULT_WAKE_KWS_PROFILE,
        "ready": pack.is_ready(),
        "install_dir": pack.root_dir,
        "model_dir": pack.model_dir,
        "archive_url": DEFAULT_WAKE_KWS_ARCHIVE_URL,
        "missing_files": pack.missing_files(),
    })
}

fn prepare_voice_wake_kws_profile() -> Result<(), String> {
    let pack = voice_wake_kws_model_pack();
    if pack.is_ready() {
        return Ok(());
    }
    std::fs::create_dir_all(&pack.root_dir)
        .map_err(|error| format!("create voice wake KWS profile dir: {error}"))?;
    let archive_path = pack.archive_path();
    download_voice_wake_kws_archive(&archive_path)?;
    let status = Command::new("tar")
        .arg("-xjf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&pack.root_dir)
        .status()
        .map_err(|error| format!("extract voice wake KWS archive with tar: {error}"))?;
    if !status.success() {
        return Err(format!(
            "extract voice wake KWS archive failed with status {status}"
        ));
    }
    if !pack.is_ready() {
        return Err(format!(
            "voice wake KWS profile extracted but required files are missing: {}",
            pack.missing_files()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

fn download_voice_wake_kws_archive(destination: &Path) -> Result<(), String> {
    if destination
        .metadata()
        .map(|metadata| metadata.is_file() && metadata.len() > 1_000_000)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let temp_path = destination.with_extension("download");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30 * 60))
        .build();
    let response = agent
        .get(DEFAULT_WAKE_KWS_ARCHIVE_URL)
        .call()
        .map_err(|error| format!("download voice wake KWS model: {error}"))?;
    let mut reader = response.into_reader();
    let mut output = std::fs::File::create(&temp_path)
        .map_err(|error| format!("create {}: {error}", temp_path.display()))?;
    std::io::copy(&mut reader, &mut output)
        .map_err(|error| format!("write {}: {error}", temp_path.display()))?;
    drop(output);
    let len = temp_path
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if len <= 1_000_000 {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "downloaded voice wake KWS archive is incomplete: {} has {len} bytes",
            temp_path.display()
        ));
    }
    std::fs::rename(&temp_path, destination).map_err(|error| {
        format!(
            "install voice wake KWS archive {} -> {}: {error}",
            temp_path.display(),
            destination.display()
        )
    })
}

/// Async model download + extract + start worker. Runs in a spawned task.
async fn download_and_start_listener(
    cancel: Arc<AtomicBool>,
    request: VoiceWakeListenerStartRequest,
    admin_host: String,
    admin_port: u16,
) -> Result<(), String> {
    let pack = voice_wake_kws_model_pack();
    tokio::fs::create_dir_all(&pack.root_dir)
        .await
        .map_err(|e| format!("create KWS dir: {e}"))?;
    let archive_path = pack.archive_path();

    // Phase 1: async streaming download with progress
    download_voice_wake_kws_archive_async(&archive_path, &cancel).await?;

    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".to_string());
    }

    // Phase 2: extract
    update_listener_state(|state| {
        state.model_download_status = Some("extracting".to_string());
    })
    .await;

    let root_dir = pack.root_dir.clone();
    let archive_for_extract = archive_path.clone();
    let extract_result = tokio::task::spawn_blocking(move || {
        let status = Command::new("tar")
            .arg("-xjf")
            .arg(&archive_for_extract)
            .arg("-C")
            .arg(&root_dir)
            .status()
            .map_err(|e| format!("extract tar: {e}"))?;
        if !status.success() {
            return Err(format!("tar extract failed: {status}"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))?;
    extract_result?;

    if !pack.is_ready() {
        let missing = pack
            .missing_files()
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("extracted but missing files: {missing}"));
    }

    update_listener_state(|state| {
        state.model_download_status = Some("downloaded".to_string());
    })
    .await;

    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".to_string());
    }

    // Phase 3: start the worker
    match spawn_voice_wake_worker(&request, &admin_host, admin_port).await {
        Ok((pid, task)) => {
            let mut runtime = VOICE_WAKE_LISTENER.lock().await;
            runtime.worker_pid = Some(pid);
            runtime.state.worker_pid = Some(pid);
            runtime.state.model_download_status = None;
            runtime.state.model_download_progress = None;
            runtime.state.model_download_total = None;
            // Replace our own task handle with the worker monitoring task
            runtime.task = Some(task);
            Ok(())
        }
        Err(error) => {
            update_listener_state(|state| {
                state.running = false;
                state.stopped_at_ms = Some(now_ms());
                state.model_download_status = Some("failed".to_string());
                state.last_error = Some(error.clone());
                state.last_error_at_ms = Some(now_ms());
            })
            .await;
            Err(error)
        }
    }
}

/// Async streaming download of the KWS model archive with progress updates.
async fn download_voice_wake_kws_archive_async(
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use futures_util::StreamExt;

    // Skip if already downloaded
    if destination
        .metadata()
        .map(|m| m.is_file() && m.len() > 1_000_000)
        .unwrap_or(false)
    {
        update_listener_state(|state| {
            state.model_download_status = Some("downloaded".to_string());
        })
        .await;
        return Ok(());
    }

    let temp_path = destination.with_extension("download");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| format!("create HTTP client: {e}"))?;

    let response = client
        .get(DEFAULT_WAKE_KWS_ARCHIVE_URL)
        .send()
        .await
        .map_err(|e| format!("download KWS model: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("download KWS model: HTTP {}", response.status()));
    }

    let total_size = response.content_length();
    update_listener_state(|state| {
        state.model_download_total = total_size;
    })
    .await;

    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| format!("create {}: {e}", temp_path.display()))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_progress_update = tokio::time::Instant::now();

    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err("cancelled".to_string());
        }
        let chunk = chunk.map_err(|e| format!("download stream: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write {}: {e}", temp_path.display()))?;
        downloaded += chunk.len() as u64;

        // Throttle progress updates to every 200ms
        if last_progress_update.elapsed() >= Duration::from_millis(200) {
            last_progress_update = tokio::time::Instant::now();
            update_listener_state(|state| {
                state.model_download_progress = Some(downloaded);
            })
            .await;
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("flush {}: {e}", temp_path.display()))?;
    drop(file);

    // Final progress update
    update_listener_state(|state| {
        state.model_download_progress = Some(downloaded);
    })
    .await;

    if downloaded <= 1_000_000 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(format!(
            "downloaded KWS archive incomplete: {downloaded} bytes"
        ));
    }

    tokio::fs::rename(&temp_path, destination)
        .await
        .map_err(|e| format!("install KWS archive: {e}"))?;

    Ok(())
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
    normalize_wake_phrase(phrase)
}

fn phrase_matches(normalized_transcript: &str, normalized_binding: &str) -> bool {
    wake_phrase_matches(normalized_transcript, normalized_binding)
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

fn default_listener_engine() -> String {
    "lightweight_kws_listener".to_string()
}

fn default_press_count() -> u8 {
    1
}

fn default_repeat_delay_ms() -> u64 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_phrase_whitespace_without_changing_text() {
        assert_eq!(normalize_phrase("  打开   录音 "), "打开录音");
    }

    #[test]
    fn normalizes_phrase_punctuation() {
        assert_eq!(normalize_phrase("哈喽哈喽。"), "哈喽哈喽");
        assert_eq!(normalize_phrase("Hello, Bifrost!"), "hellobifrost");
    }

    #[test]
    fn phrase_match_collapses_repeated_wake_word() {
        let binding = normalize_phrase("哈喽哈喽。");
        let transcript = normalize_phrase("哈喽");
        assert!(phrase_matches(&transcript, &binding));
        assert!(phrase_matches(
            &normalize_phrase("现在 哈喽 一下"),
            &binding
        ));
        assert!(!phrase_matches(
            &normalize_phrase("打开"),
            &normalize_phrase("打开录音")
        ));
    }

    #[test]
    fn builds_safe_macos_keypress_script_for_named_key() {
        let script = build_macos_keypress_script(
            Some("space"),
            None,
            &["cmd".to_string(), "shift".to_string()],
            1,
            100,
        )
        .expect("script");
        assert_eq!(
            script,
            "tell application \"System Events\" to key code 49 using {command down, shift down}"
        );
    }

    #[test]
    fn builds_double_keypress_with_100ms_delay() {
        let script = build_macos_keypress_script(Some("space"), None, &[], 2, 100).expect("script");
        assert_eq!(
            script,
            "tell application \"System Events\" to repeat with pressIndex from 1 to 2\n  key code 49\n  if pressIndex is less than 2 then delay 0.1\nend repeat"
        );
    }

    #[test]
    fn builds_double_option_keypress_with_100ms_delay() {
        let script =
            build_macos_keypress_script(Some("option"), None, &[], 2, 100).expect("script");
        assert_eq!(
            script,
            "tell application \"System Events\" to repeat with pressIndex from 1 to 2\n  key code 58\n  if pressIndex is less than 2 then delay 0.1\nend repeat"
        );
    }

    #[test]
    fn validates_key_press_key_at_binding_create_time() {
        let error = validate_key_action(&VoiceWakeAction::KeyPress {
            key: Some("unsupported_multi_key".to_string()),
            keycode: None,
            modifiers: Vec::new(),
            press_count: 1,
            repeat_delay_ms: 100,
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
                    repeat_delay_ms: 100,
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
    fn listener_match_allows_phrase_only_candidate() {
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
                    repeat_delay_ms: 100,
                },
                created_at_ms: now,
                updated_at_ms: now,
                last_triggered_at_ms: None,
            }],
            ..VoiceWakeStore::default()
        };
        let candidate = listener_match_candidate(&store, "现在 打开 录音").unwrap();
        assert_eq!(candidate.binding_id, "b1");
        assert_eq!(candidate.voiceprint_profile_id, None);
    }

    #[test]
    fn listener_start_requires_binding_only() {
        let now = now_ms();
        let empty = VoiceWakeStore::default();
        assert!(listener_start_block_reason(&empty)
            .expect("blocked")
            .contains("saved voice command"));

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

        let phrase_only = VoiceWakeStore {
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
                    repeat_delay_ms: 100,
                },
                created_at_ms: now,
                updated_at_ms: now,
                last_triggered_at_ms: None,
            }],
            ..VoiceWakeStore::default()
        };
        assert!(listener_start_block_reason(&phrase_only).is_none());
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
                    repeat_delay_ms: 100,
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

#[cfg(test)]
mod wake_helper_tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::io::Read as _;
    use tempfile::tempdir;

    #[test]
    fn atomic_json_write_creates_parent_dirs_and_writes_pretty_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested/config.json");
        let value = json!({"k": 1, "s": "v"});

        atomic_json_write(&path, &value).expect("atomic write");

        let mut file = fs::File::open(&path).expect("written file exists");
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed, value);

        // Temporary file should not remain.
        let tmp_path = path.with_extension("tmp");
        assert!(!tmp_path.exists());
    }

    #[test]
    fn validate_id_accepts_safe_ids_and_rejects_invalid() {
        validate_id("id-1_ok.2", "label").expect("valid id");
        assert!(validate_id("", "id").is_err());
        assert!(validate_id(&"x".repeat(97), "id").is_err());
        assert!(validate_id("bad id", "id").is_err());
        assert!(validate_id("bad#id", "id").is_err());
    }

    #[test]
    fn escape_applescript_escapes_backslashes_and_quotes() {
        let input = "a\\b\"c";
        let escaped = escape_applescript(input);
        // Backslashes doubled, quotes escaped.
        assert_eq!(escaped, "a\\\\b\\\"c");
    }

    #[test]
    fn default_flags_and_values_match_expected_defaults() {
        assert!(default_enabled());
        assert!(default_dry_run());
        assert!(default_execute());
        assert_eq!(default_listener_source(), "mic".to_string());
        assert_eq!(
            default_listener_engine(),
            "lightweight_kws_listener".to_string()
        );
        assert_eq!(default_press_count(), 1);
        assert_eq!(default_repeat_delay_ms(), 100);
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::test_env::BifrostDataDirGuard;

    use std::fs;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::tempdir;

    // ---- Helper macros for small pure functions ----

    macro_rules! id_case {
        ($name:ident, $value:expr, $ok:expr) => {
            #[test]
            fn $name() {
                assert_eq!(validate_id($value, "id").is_ok(), $ok);
            }
        };
    }

    id_case!(validate_id_accepts_simple, "abc-123_45.ok", true);
    id_case!(validate_id_rejects_empty, "", false);
    id_case!(validate_id_rejects_too_long, &"x".repeat(97), false);
    id_case!(validate_id_rejects_space, "bad id", false);
    id_case!(validate_id_rejects_pound, "bad#id", false);

    macro_rules! repeat_delay_case {
        ($name:ident, $ms:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(format_repeat_delay_seconds($ms), $expected.to_string());
            }
        };
    }

    repeat_delay_case!(repeat_delay_zero, 0, "0");
    repeat_delay_case!(repeat_delay_one_ms, 1, "0.001");
    repeat_delay_case!(repeat_delay_hundred_ms, 100, "0.1");
    repeat_delay_case!(repeat_delay_two_and_half_sec, 2500, "2.5");

    macro_rules! named_keycode_case {
        ($name:ident, $key:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(named_keycode($key), $expected);
            }
        };
    }

    named_keycode_case!(named_keycode_space, "space", Some(49));
    named_keycode_case!(named_keycode_return, "return", Some(36));
    named_keycode_case!(named_keycode_tab, "tab", Some(48));
    named_keycode_case!(named_keycode_right, "right", Some(124));
    named_keycode_case!(named_keycode_control, "control", Some(59));
    named_keycode_case!(named_keycode_unknown_none, "unknown", None);

    // ---- Key modifier and action helpers ----

    #[test]
    fn normalize_modifiers_accepts_common_modifiers() {
        let mods = normalize_modifiers(&[
            "cmd".to_string(),
            "shift".to_string(),
            "ctrl".to_string(),
            "option".to_string(),
        ])
        .expect("mods");
        assert_eq!(
            mods,
            vec!["command down", "shift down", "control down", "option down"]
        );
    }

    #[test]
    fn normalize_modifiers_rejects_unknown() {
        let err = normalize_modifiers(&["super".to_string()]).unwrap_err();
        assert!(err.contains("unsupported key modifier"));
    }

    #[test]
    fn execute_key_press_dry_run_builds_preview() {
        let result = execute_key_press(Some("space"), None, &["cmd".to_string()], 10, 10_000, true)
            .expect("result");
        assert!(result.dry_run);
        assert!(!result.executed);
        assert_eq!(result.action_type, "key_press");
        assert!(result.command_preview.unwrap().contains("key code"));
    }

    #[test]
    fn validate_key_action_rejects_zero_press_count() {
        let err = validate_key_action(&VoiceWakeAction::KeyPress {
            key: Some("space".to_string()),
            keycode: None,
            modifiers: Vec::new(),
            press_count: 0,
            repeat_delay_ms: 100,
        })
        .unwrap_err();
        assert!(err.contains("press_count"));
    }

    #[test]
    fn validate_key_action_rejects_press_count_over_eight() {
        let err = validate_key_action(&VoiceWakeAction::KeyPress {
            key: Some("space".to_string()),
            keycode: None,
            modifiers: Vec::new(),
            press_count: 9,
            repeat_delay_ms: 100,
        })
        .unwrap_err();
        assert!(err.contains("press_count"));
    }

    #[test]
    fn validate_key_action_rejects_repeat_delay_too_large() {
        let err = validate_key_action(&VoiceWakeAction::KeyPress {
            key: Some("space".to_string()),
            keycode: None,
            modifiers: Vec::new(),
            press_count: 1,
            repeat_delay_ms: 6000,
        })
        .unwrap_err();
        assert!(err.contains("repeat_delay_ms"));
    }

    #[test]
    fn execute_action_forwards_to_key_press_dry_run() {
        let action = VoiceWakeAction::KeyPress {
            key: Some("space".to_string()),
            keycode: None,
            modifiers: vec!["shift".to_string()],
            press_count: 1,
            repeat_delay_ms: 50,
        };
        let result = execute_action(&action, true).expect("execute");
        assert!(result.dry_run);
        assert!(!result.executed);
    }

    // ---- Listener candidate matching & triggers ----

    fn sample_profile(id: &str, voiceprint: Option<&str>, now: u64) -> VoiceWakeProfile {
        VoiceWakeProfile {
            id: id.to_string(),
            display_name: format!("Profile {id}"),
            voiceprint_profile_id: voiceprint.map(|v| v.to_string()),
            speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    fn sample_binding(id: &str, profile_id: &str, now: u64) -> VoiceWakeBinding {
        VoiceWakeBinding {
            id: id.to_string(),
            enabled: true,
            phrase: "打开录音".to_string(),
            normalized_phrase: normalize_phrase("打开录音"),
            profile_id: profile_id.to_string(),
            kws_score: DEFAULT_KWS_SCORE,
            kws_threshold: DEFAULT_KWS_THRESHOLD,
            speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
            cooldown_ms: DEFAULT_COOLDOWN_MS,
            action: VoiceWakeAction::KeyPress {
                key: Some("space".to_string()),
                keycode: None,
                modifiers: Vec::new(),
                press_count: 1,
                repeat_delay_ms: 100,
            },
            created_at_ms: now,
            updated_at_ms: now,
            last_triggered_at_ms: None,
        }
    }

    #[test]
    fn listener_match_candidate_errors_when_store_disabled() {
        let now = now_ms();
        let store = VoiceWakeStore {
            enabled: false,
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = listener_match_candidate(&store, "打开录音").unwrap_err();
        assert!(err.contains("disabled"));
    }

    #[test]
    fn listener_match_candidate_errors_when_no_binding_matches() {
        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = listener_match_candidate(&store, "其他命令").unwrap_err();
        assert!(err.contains("no_match"));
    }

    #[test]
    fn listener_match_candidate_errors_for_missing_profile() {
        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: Vec::new(),
            bindings: vec![sample_binding("b1", "missing", now)],
            ..VoiceWakeStore::default()
        };
        let err = listener_match_candidate(&store, "打开录音").unwrap_err();
        assert!(err.contains("voice wake profile not found"));
    }

    #[test]
    fn listener_match_candidate_respects_cooldown() {
        let now = now_ms();
        let mut binding = sample_binding("b1", "p1", now);
        binding.last_triggered_at_ms = Some(now);
        let store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![binding],
            ..VoiceWakeStore::default()
        };
        let err = listener_match_candidate(&store, "打开录音").unwrap_err();
        assert_eq!(err, "cooldown".to_string());
    }

    #[test]
    fn trigger_binding_errors_when_actions_disabled() {
        let store = VoiceWakeStore {
            enabled: false,
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding(
            &mut store.clone(),
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: Some("p1".to_string()),
                speaker_confidence: None,
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("disabled"));
    }

    #[test]
    fn trigger_binding_errors_when_no_binding_matches() {
        let mut store = VoiceWakeStore::default();
        let err = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: None,
                speaker_confidence: None,
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("no enabled voice wake binding"));
    }

    #[test]
    fn trigger_binding_errors_for_cooldown() {
        let now = now_ms();
        let mut binding = sample_binding("b1", "p1", now);
        binding.last_triggered_at_ms = Some(now);
        let mut store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![binding],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: Some("p1".to_string()),
                speaker_confidence: Some(DEFAULT_SPEAKER_THRESHOLD + 0.1),
                dry_run: true,
            },
        )
        .unwrap_err();
        assert_eq!(err, "cooldown".to_string());
    }

    #[test]
    fn trigger_binding_errors_for_low_speaker_confidence() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: Some("p1".to_string()),
                speaker_confidence: Some(0.0),
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("below threshold"));
    }

    #[test]
    fn trigger_binding_requires_profile_or_confidence() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: None,
                speaker_confidence: None,
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("profile_id or speaker_confidence is required"));
    }

    #[test]
    fn trigger_binding_requires_speaker_confidence_when_voiceprint_configured() {
        let now = now_ms();
        let profile = VoiceWakeProfile {
            voiceprint_profile_id: Some("speaker_eden".to_string()),
            ..sample_profile("p1", None, now)
        };
        let mut store = VoiceWakeStore {
            profiles: vec![profile],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding(
            &mut store,
            VoiceWakeTriggerRequest {
                phrase: "打开录音".to_string(),
                profile_id: Some("p1".to_string()),
                speaker_confidence: None,
                dry_run: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("speaker_confidence is required"));
    }

    #[test]
    fn trigger_binding_by_index_errors_for_cooldown() {
        let now = now_ms();
        let mut binding = sample_binding("b1", "p1", now);
        binding.last_triggered_at_ms = Some(now);
        let mut store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![binding],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding_by_index(&mut store, 0, now, true, None).unwrap_err();
        assert_eq!(err, "cooldown".to_string());
    }

    #[test]
    fn trigger_binding_by_id_missing_returns_error() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let err = trigger_binding_by_id(&mut store, "missing", true, None).unwrap_err();
        assert!(err.contains("matched id"));
    }

    // ---- Store and KWS helpers touching the filesystem ----

    #[test]
    fn load_store_and_write_store_roundtrip() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            enabled: true,
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write");
        let loaded = load_store().expect("load");
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.bindings.len(), 1);
        assert!(loaded.enabled);
    }

    #[test]
    fn download_voice_wake_kws_archive_returns_ok_when_large_file_present() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("kws.tar.bz2");
        let data = vec![0u8; 1_500_000];
        fs::write(&path, &data).expect("write");
        download_voice_wake_kws_archive(&path).expect("skip download");
    }

    #[test]
    fn voice_wake_kws_status_json_has_expected_keys() {
        let value = voice_wake_kws_status_json();
        assert_eq!(value["engine"], "sherpa-onnx");
        assert_eq!(value["profile"], DEFAULT_WAKE_KWS_PROFILE);
        assert!(value.get("install_dir").is_some());
        assert!(value.get("missing_files").is_some());
    }

    #[tokio::test]
    async fn download_voice_wake_kws_archive_async_marks_downloaded_status() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_listener_runtime().await;
        let dest = dir.path().join("kws_async.tar.bz2");
        let data = vec![0u8; 1_500_000];
        fs::write(&dest, &data).expect("write");

        let cancel = AtomicBool::new(false);
        download_voice_wake_kws_archive_async(&dest, &cancel)
            .await
            .expect("async skip");

        let state = listener_state_snapshot().await;
        assert_eq!(state.model_download_status.as_deref(), Some("downloaded"));
    }

    // ---- Listener runtime helpers ----

    #[tokio::test]
    async fn listener_state_snapshot_clears_finished_task() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        reset_listener_runtime().await;
        // Ensure a finished task is cleared from the runtime.
        {
            let mut runtime = VOICE_WAKE_LISTENER.lock().await;
            runtime.task = Some(tokio::spawn(async {}));
        }
        tokio::task::yield_now().await;
        let state = listener_state_snapshot().await;
        assert!(!state.running);
    }

    #[tokio::test]
    async fn run_lightweight_mic_wake_listener_respects_cancel_flag() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        reset_listener_runtime().await;
        let cancel = Arc::new(AtomicBool::new(true));
        let request = VoiceWakeListenerStartRequest {
            source: "mic".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: Some(DEFAULT_LISTENER_CHUNK_MS),
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };
        run_lightweight_mic_wake_listener(cancel, &request)
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn record_listener_error_updates_state() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        reset_listener_runtime().await;
        record_listener_error("something went wrong").await;
        let state = listener_state_snapshot().await;
        assert_eq!(state.last_error.as_deref(), Some("something went wrong"));
        assert!(state.last_error_at_ms.is_some());
    }

    // ---- Speaker identification ----

    #[test]
    fn identify_listener_speaker_mock_matched_and_unmatched() {
        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: Some("speaker_eden".to_string()),
            mock_speaker_confidence: Some(0.9),
        };
        let matched = identify_listener_speaker(None, &request).expect("mock");
        assert!(matched.matched);
        assert_eq!(matched.profile_id.as_deref(), Some("speaker_eden"));

        let request_unmatched = VoiceWakeListenerStartRequest {
            mock_speaker_profile_id: None,
            mock_speaker_confidence: Some(0.0),
            ..request
        };
        let unmatched = identify_listener_speaker(None, &request_unmatched).expect("mock");
        assert!(!unmatched.matched);
        assert_eq!(unmatched.status, "unmatched");
    }

    #[test]
    fn identify_listener_speaker_requires_chunk_for_real_source() {
        let request = VoiceWakeListenerStartRequest {
            source: "mic".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };
        let err = identify_listener_speaker(None, &request).unwrap_err();
        assert!(err.contains("audio chunk is required"));
    }

    // ---- process_listener_transcript high-level behaviour ----

    async fn reset_listener_runtime() {
        let mut runtime = VOICE_WAKE_LISTENER.lock().await;
        if let Some(task) = runtime.task.take() {
            task.abort();
        }
        runtime.cancel = None;
        runtime.worker_pid = None;
        runtime.state = VoiceWakeListenerState::default();
    }

    #[tokio::test]
    async fn process_listener_transcript_no_match_sets_last_match_status() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_listener_runtime().await;

        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", None, now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write store");

        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };

        process_listener_transcript("不匹配", None, &request)
            .await
            .expect("ok");

        let state = listener_state_snapshot().await;
        assert_eq!(state.last_match_status.as_deref(), Some("no_match"));
    }

    #[tokio::test]
    async fn process_listener_transcript_dry_run_success_updates_trigger_count() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_listener_runtime().await;

        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![sample_profile("p1", Some("speaker_eden"), now)],
            bindings: vec![sample_binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write store");

        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: false, // dry run
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: Some("speaker_eden".to_string()),
            mock_speaker_confidence: Some(DEFAULT_SPEAKER_THRESHOLD + 0.1),
        };

        process_listener_transcript("打开录音", None, &request)
            .await
            .expect("ok");

        let state = listener_state_snapshot().await;
        assert_eq!(state.trigger_count, 1);
        assert_eq!(state.last_match_status.as_deref(), Some("dry_run_matched"));
        assert_eq!(
            state.last_speaker_profile_id.as_deref(),
            Some("speaker_eden")
        );
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::test_env::BifrostDataDirGuard;

    use serde_json::Value;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn profile(id: &str, voiceprint: Option<&str>, now: u64) -> VoiceWakeProfile {
        VoiceWakeProfile {
            id: id.to_string(),
            display_name: format!("Profile {id}"),
            voiceprint_profile_id: voiceprint.map(|v| v.to_string()),
            speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    fn binding(id: &str, profile_id: &str, now: u64) -> VoiceWakeBinding {
        VoiceWakeBinding {
            id: id.to_string(),
            enabled: true,
            phrase: "打开录音".to_string(),
            normalized_phrase: normalize_phrase("打开录音"),
            profile_id: profile_id.to_string(),
            kws_score: DEFAULT_KWS_SCORE,
            kws_threshold: DEFAULT_KWS_THRESHOLD,
            speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
            cooldown_ms: DEFAULT_COOLDOWN_MS,
            action: VoiceWakeAction::KeyPress {
                key: Some("space".to_string()),
                keycode: None,
                modifiers: Vec::new(),
                press_count: 1,
                repeat_delay_ms: 100,
            },
            created_at_ms: now,
            updated_at_ms: now,
            last_triggered_at_ms: None,
        }
    }

    async fn reset_runtime() {
        let mut runtime = VOICE_WAKE_LISTENER.lock().await;
        if let Some(task) = runtime.task.take() {
            task.abort();
        }
        runtime.cancel = None;
        runtime.worker_pid = None;
        runtime.state = VoiceWakeListenerState::default();
    }

    #[test]
    fn voice_wake_store_default_is_enabled_and_empty() {
        let store = VoiceWakeStore::default();
        assert!(store.enabled);
        assert!(store.profiles.is_empty());
        assert!(store.bindings.is_empty());
        assert!(store.events.is_empty());
    }

    #[test]
    fn voice_wake_listener_state_default_uses_expected_defaults() {
        let state = VoiceWakeListenerState::default();
        assert!(!state.running);
        assert_eq!(state.source, default_listener_source());
        assert_eq!(state.engine, default_listener_engine());
        assert_eq!(state.chunk_ms, DEFAULT_LISTENER_CHUNK_MS);
        assert_eq!(state.trigger_count, 0);
    }

    #[test]
    fn voice_wake_listener_start_request_default_execute_true() {
        let req = VoiceWakeListenerStartRequest::default();
        assert_eq!(req.source, default_listener_source());
        assert!(req.execute);
        assert!(req.mock_transcripts.is_empty());
    }

    #[test]
    fn normalize_modifiers_rejects_empty_modifier() {
        let err = normalize_modifiers(&["".to_string()]).unwrap_err();
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn named_keycode_resolves_arrow_and_escape_keys() {
        assert_eq!(named_keycode("left"), Some(123));
        assert_eq!(named_keycode("up"), Some(126));
        assert_eq!(named_keycode("esc"), Some(53));
    }

    #[test]
    fn format_repeat_delay_seconds_trims_trailing_zeros() {
        assert_eq!(format_repeat_delay_seconds(1000), "1");
        assert_eq!(format_repeat_delay_seconds(2500), "2.5");
    }

    #[test]
    fn update_store_roundtrip_adds_profile() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let now = now_ms();
        let value = update_store(|store| {
            store.profiles.push(profile("p1", None, now));
            Ok(serde_json::json!({"ok": true}))
        })
        .expect("update");
        assert_eq!(value["ok"], true);

        let loaded = load_store().expect("load");
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].id, "p1");
    }

    #[tokio::test]
    async fn get_events_response_returns_empty_events_when_store_missing() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let resp = get_events_response();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .expect("collect");
        let bytes = body.to_bytes();
        let v: Value = serde_json::from_slice(&bytes[..]).expect("json");
        assert!(v["events"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_events_response_includes_written_events() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let now = now_ms();
        let event = VoiceWakeEvent {
            id: "e1".to_string(),
            binding_id: "b1".to_string(),
            phrase: "打开录音".to_string(),
            profile_id: "p1".to_string(),
            speaker_confidence: Some(0.9),
            dry_run: true,
            matched_at_ms: now,
            action_result: VoiceWakeActionResult {
                action_type: "key_press".to_string(),
                dry_run: true,
                executed: false,
                platform: "test".to_string(),
                message: "ok".to_string(),
                command_preview: None,
            },
        };
        let store = VoiceWakeStore {
            events: vec![event],
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write");

        let resp = get_events_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .expect("collect");
        let bytes = body.to_bytes();
        let v: Value = serde_json::from_slice(&bytes[..]).expect("json");
        assert_eq!(v["events"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn trigger_binding_by_index_updates_event_and_binding() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let value =
            trigger_binding_by_index(&mut store, 0, now, true, Some(DEFAULT_SPEAKER_THRESHOLD))
                .expect("trigger");
        assert!(value["matched"].as_bool().unwrap_or(false));
        assert_eq!(store.events.len(), 1);
        assert!(store.bindings[0].last_triggered_at_ms.is_some());
    }

    #[test]
    fn trigger_binding_by_index_trims_events_above_max() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            events: Vec::new(),
            ..VoiceWakeStore::default()
        };
        for i in 0..(MAX_EVENTS + 5) {
            let t = now + i as u64 * (DEFAULT_COOLDOWN_MS + 1);
            let _ = trigger_binding_by_index(&mut store, 0, t, true, None).expect("trigger");
        }
        assert_eq!(store.events.len(), MAX_EVENTS);
    }

    #[tokio::test]
    async fn run_mock_voice_wake_listener_processes_all_transcripts() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_runtime().await;

        let cancel = Arc::new(AtomicBool::new(false));
        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: vec!["一号".to_string(), "二号".to_string()],
            mock_interval_ms: Some(1),
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };

        run_mock_voice_wake_listener(cancel, &request)
            .await
            .expect("mock listener");

        let state = listener_state_snapshot().await;
        assert_eq!(state.last_transcript.as_deref(), Some("二号"));
        assert!(matches!(
            state.last_match_status.as_deref(),
            Some("recognized") | Some("no_match")
        ));
    }

    #[tokio::test]
    async fn run_voice_wake_listener_mock_source_updates_state_to_stopped() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_runtime().await;

        let cancel = Arc::new(AtomicBool::new(false));
        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: vec!["test".to_string()],
            mock_interval_ms: Some(1),
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };

        run_voice_wake_listener(cancel, request).await;
        let state = listener_state_snapshot().await;
        assert!(!state.running);
        assert!(state.stopped_at_ms.is_some());
    }

    #[tokio::test]
    async fn process_listener_transcript_records_error_for_mic_without_chunk() {
        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_runtime().await;

        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![profile("p1", Some("speaker_eden"), now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write");

        let request = VoiceWakeListenerStartRequest {
            source: "mic".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };

        process_listener_transcript("打开录音", None, &request)
            .await
            .expect("ok");

        let state = listener_state_snapshot().await;
        assert!(state
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("audio chunk is required"));
    }

    #[tokio::test]
    async fn process_listener_transcript_records_action_execution_error_on_non_macos() {
        if std::env::consts::OS == "macos" {
            return;
        }

        let _listener_guard = VOICE_WAKE_LISTENER_TEST_LOCK.lock().await;
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());
        reset_runtime().await;

        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        write_store(&store).expect("write");

        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: None,
        };

        process_listener_transcript("打开录音", None, &request)
            .await
            .expect("ok");

        let state = listener_state_snapshot().await;
        assert!(state
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("key_press execution is currently implemented only on macOS"));
    }

    #[test]
    fn profile_helper_sets_voiceprint_and_timestamps() {
        let now = now_ms();
        let profile = profile("p1", Some("voiceprint"), now);
        assert_eq!(profile.id, "p1");
        assert_eq!(profile.voiceprint_profile_id.as_deref(), Some("voiceprint"));
        assert_eq!(profile.created_at_ms, now);
        assert_eq!(profile.updated_at_ms, now);
    }

    #[test]
    fn binding_helper_uses_normalized_phrase_and_defaults() {
        let now = now_ms();
        let binding = binding("b1", "p1", now);
        assert_eq!(binding.id, "b1");
        assert_eq!(binding.profile_id, "p1");
        assert_eq!(binding.normalized_phrase, normalize_phrase("打开录音"));
        assert_eq!(binding.cooldown_ms, DEFAULT_COOLDOWN_MS);
        matches!(binding.action, VoiceWakeAction::KeyPress { .. });
    }

    #[test]
    fn listener_start_block_reason_returns_message_when_no_enabled_binding() {
        let store = VoiceWakeStore::default();
        let reason = listener_start_block_reason(&store).expect("reason");
        assert!(reason.contains("requires a saved voice command binding"));
    }

    #[test]
    fn listener_start_block_reason_returns_none_when_binding_and_profile_present() {
        let now = now_ms();
        let store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        assert!(listener_start_block_reason(&store).is_none());
    }

    #[test]
    fn identify_listener_speaker_mock_unmatched_status_is_unmatched() {
        let request = VoiceWakeListenerStartRequest {
            source: "mock".to_string(),
            engine: default_listener_engine(),
            device: None,
            chunk_ms: None,
            execute: true,
            mock_transcripts: Vec::new(),
            mock_interval_ms: None,
            mock_speaker_profile_id: None,
            mock_speaker_confidence: Some(0.1),
        };
        let speaker = identify_listener_speaker(None, &request).expect("mock");
        assert!(!speaker.matched);
        assert!(speaker.profile_id.is_none());
        assert_eq!(speaker.status, "unmatched");
    }

    #[test]
    fn update_store_can_disable_store_flag() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let value = update_store(|store| {
            store.enabled = false;
            Ok(serde_json::json!({"enabled": store.enabled}))
        })
        .expect("update");

        assert_eq!(value["enabled"], false);

        let loaded = load_store().expect("load");
        assert!(!loaded.enabled);
    }

    #[test]
    fn get_events_response_sets_json_content_type_header() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let resp = get_events_response();
        let content_type = resp
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
    }

    #[test]
    fn get_events_response_returns_internal_error_when_store_parse_fails() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let path = store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("dirs");
        }
        std::fs::write(&path, b"not json").expect("write");

        let resp = get_events_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn trigger_binding_by_index_sets_event_speaker_confidence() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let _ = trigger_binding_by_index(
            &mut store,
            0,
            now,
            true,
            Some(DEFAULT_SPEAKER_THRESHOLD + 0.1),
        )
        .expect("trigger");
        assert_eq!(
            store.events[0].speaker_confidence,
            Some(DEFAULT_SPEAKER_THRESHOLD + 0.1)
        );
    }

    #[test]
    fn trigger_binding_by_index_without_speaker_confidence_leaves_field_none() {
        let now = now_ms();
        let mut store = VoiceWakeStore {
            profiles: vec![profile("p1", None, now)],
            bindings: vec![binding("b1", "p1", now)],
            ..VoiceWakeStore::default()
        };
        let _ = trigger_binding_by_index(&mut store, 0, now, true, None).expect("trigger");
        assert!(store.events[0].speaker_confidence.is_none());
    }
}
