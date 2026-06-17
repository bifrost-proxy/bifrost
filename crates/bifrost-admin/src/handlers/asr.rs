use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub(crate) use bifrost_asr::asr_platform_supported;
#[cfg(test)]
pub(crate) use bifrost_asr::asr_platform_supported_for;
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::asr_runtime::{
    clear_service_state, fixed_asr_home, install_dir, model_dir, now_ms, read_service_state,
    stop_pid, write_service_state, AsrServiceState, DEFAULT_ASR_HOST, DEFAULT_ASR_LANGUAGE,
    DEFAULT_ASR_MODEL,
};
use crate::handlers::asr_cli_invoke::{
    default_footprint_limit_bytes, physical_footprint_sample_interval, read_process_footprint_bytes,
};
use crate::handlers::asr_jobs::{
    handle_asr_tasks, transcribe_uploaded_wav_with_voiceprint_speakers,
};
use crate::handlers::asr_streaming::{
    append_transcript_delta, call_asr_whole_file_endpoint, dedupe_increment,
};
use crate::handlers::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};
use crate::resource_download::{download_with_resume, DownloadProgress, DownloadRequest};

const SERVICE_START_TIMEOUT_SECS: u64 = 180;
const MAX_ASR_UPLOAD_BYTES: usize = 512 * 1024 * 1024;
const ASR_UPLOAD_CHUNK_DURATION_SECS: u64 = 30;
const ASR_UPLOAD_CHUNK_OVERLAP_SECS: u64 = 2;
const ASR_RELEASE_REPO: &str = "second-state/qwen3_asr_rs";
const ASR_SAMPLE_BASE_URL: &str =
    "https://raw.githubusercontent.com/second-state/qwen3_asr_rs/main/test_audio";

static MANAGED_SERVICE: Lazy<Mutex<Option<ManagedAsrService>>> = Lazy::new(|| Mutex::new(None));
static ASR_INIT_TASK: Lazy<Mutex<Option<AsrInitTask>>> = Lazy::new(|| Mutex::new(None));

mod offline_jobs;

#[derive(Debug, Clone, Deserialize)]
struct AsrQuery {
    host: Option<String>,
    port: Option<u16>,
    language: Option<String>,
    model: Option<String>,
    owner_module: Option<String>,
    owner_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AsrTarget {
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) language: String,
    pub(crate) model: String,
    pub(crate) home: PathBuf,
    pub(crate) owner_module: String,
    pub(crate) owner_id: Option<String>,
}

struct ManagedAsrService {
    target: AsrTarget,
    child: Child,
}

const SERVICE_FOOTPRINT_MAX_SAMPLE_FAILURES: u32 = 3;
const SERVICE_FOOTPRINT_WARNING_LOG_INTERVAL_SECS: u64 = 60;

fn service_watchdog_should_kill_for_sample(
    footprint: u64,
    reliable_physical_footprint: bool,
    limit: u64,
) -> bool {
    reliable_physical_footprint && footprint > limit
}

fn service_watchdog_should_log_warning(last_logged: &mut Option<Instant>, now: Instant) -> bool {
    let should_log = last_logged
        .map(|last| {
            now.duration_since(last)
                >= Duration::from_secs(SERVICE_FOOTPRINT_WARNING_LOG_INTERVAL_SECS)
        })
        .unwrap_or(true);
    if should_log {
        *last_logged = Some(now);
    }
    should_log
}

#[derive(Clone)]
struct AsrInitTask {
    target: AsrTarget,
    sender: broadcast::Sender<String>,
    history: Arc<Mutex<Vec<String>>>,
    finished: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
struct AsrStatusResponse {
    status: &'static str,
    ready: bool,
    installed: bool,
    platform_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unsupported_reason: Option<String>,
    ffmpeg_available: bool,
    server_url: String,
    install_dir: String,
    model_dir: String,
    model: String,
    language: String,
    managed: bool,
    owner_module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_id: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct AsrCapabilityFlag {
    enabled: bool,
    hidden: bool,
    platform_supported: bool,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct AsrCapabilitiesResponse {
    platform: String,
    arch: String,
    supported_target: &'static str,
    qwen3_asr: AsrCapabilityFlag,
    local_transcription: AsrCapabilityFlag,
    speech_workbench: AsrCapabilityFlag,
    directory_tasks: AsrCapabilityFlag,
    speaker_diarization: AsrCapabilityFlag,
    voiceprint: AsrCapabilityFlag,
    voice_wake_asr: AsrCapabilityFlag,
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrServiceResponse {
    pub(crate) ready: bool,
    pub(crate) managed: bool,
    pub(crate) server_url: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

struct UploadedAudio {
    bytes: Vec<u8>,
    filename: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrStreamPayload<'a> {
    pub(crate) phase: &'a str,
    pub(crate) status: &'a str,
    pub(crate) progress: u8,
    pub(crate) message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) server_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct AsrDownloadProgressPayload<'a> {
    phase: &'static str,
    status: &'static str,
    progress: u8,
    message: &'static str,
    detail: Option<&'a str>,
    file: Option<&'a str>,
    server_url: Option<String>,
    downloaded_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    download_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_per_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eta_seconds: Option<u64>,
    elapsed_ms: u64,
    resumed: bool,
    complete: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrTextPayload<'a> {
    pub(crate) text: &'a str,
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrSegmentPayload<'a> {
    pub(crate) index: usize,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) stable_start_ms: u64,
    pub(crate) stable_end_ms: u64,
    pub(crate) text: &'a str,
    pub(crate) delta: &'a str,
    pub(crate) committed: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speaker: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speaker_display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speaker_profile_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speaker_confidence: Option<f32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrErrorPayload<'a> {
    pub(crate) message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<&'a str>,
}

pub async fn handle_asr(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    if path.starts_with("/api/asr/tasks")
        || path.starts_with("/api/asr/diarization")
        || path.starts_with("/api/asr/speaker-profiles")
        || path == "/api/asr/external-volumes"
    {
        return handle_asr_tasks(req, path).await;
    }

    match (req.method(), path) {
        (&Method::GET, "/api/asr/capabilities") => handle_capabilities(),
        (&Method::GET, "/api/asr/status") => handle_status(req).await,
        (&Method::GET, "/api/asr/init-stream") => handle_init_stream(req).await,
        (&Method::POST, "/api/asr/service/start") => handle_service_start(req).await,
        (&Method::POST, "/api/asr/service/stop") => handle_service_stop(req).await,
        (&Method::POST, "/api/asr/offline-jobs") => offline_jobs::handle_create(req).await,
        (&Method::GET, _) if path.starts_with("/api/asr/offline-jobs/") => {
            offline_jobs::handle_get(path)
        }
        (&Method::POST, "/api/asr/transcribe-stream") => handle_transcribe_stream(req).await,
        (&Method::GET, "/api/asr/transcribe-ws") => error_response(
            StatusCode::GONE,
            "legacy ASR WebSocket is no longer a realtime entrypoint; use /api/voice/listen-ws with 16kHz mono PCM16",
        ),
        (&Method::GET, _) | (&Method::POST, _) => {
            error_response(StatusCode::NOT_FOUND, "ASR endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

fn handle_capabilities() -> Response<BoxBody> {
    json_response(&asr_capabilities_response())
}

fn asr_capabilities_response() -> AsrCapabilitiesResponse {
    let platform = bifrost_asr::current_platform();
    let reason = platform.unsupported_reason.clone();
    let flag = || AsrCapabilityFlag {
        enabled: platform.supported,
        hidden: !platform.supported,
        platform_supported: platform.supported,
        reason: reason.clone(),
    };
    AsrCapabilitiesResponse {
        platform: platform.os,
        arch: platform.arch,
        supported_target: platform.supported_target,
        qwen3_asr: flag(),
        local_transcription: flag(),
        speech_workbench: flag(),
        directory_tasks: flag(),
        speaker_diarization: flag(),
        voiceprint: flag(),
        voice_wake_asr: flag(),
    }
}

async fn handle_status(req: Request<Incoming>) -> Response<BoxBody> {
    let target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    if let Err(reason) = detect_asr_release_asset() {
        return json_response(&AsrStatusResponse {
            status: "unsupported",
            ready: false,
            installed: false,
            platform_supported: false,
            unsupported_reason: Some(reason.clone()),
            ffmpeg_available: false,
            server_url: target.server_url_display(),
            install_dir: target.install_dir().display().to_string(),
            model_dir: target.model_dir().display().to_string(),
            model: target.model,
            language: target.language,
            managed: false,
            owner_module: target.owner_module,
            owner_id: target.owner_id,
            message: reason,
        });
    }

    let resolved_target = resolve_managed_target(target.clone()).await;
    let installed = target.assets_installed();
    let ready = resolved_target.port.is_some() && probe_asr_health(&resolved_target).await.is_ok();
    let managed = if ready {
        managed_service_matches(&resolved_target).await
    } else {
        managed_service_matches(&target).await
    };
    let ffmpeg_available = command_succeeds("ffmpeg", &["-version"]).await;
    let status_target = if ready { &resolved_target } else { &target };
    let status = if ready {
        "ready"
    } else if installed {
        "installed"
    } else {
        "missing"
    };

    json_response(&AsrStatusResponse {
        status,
        ready,
        installed,
        platform_supported: true,
        unsupported_reason: None,
        ffmpeg_available,
        server_url: status_target.server_url_display(),
        install_dir: target.install_dir().display().to_string(),
        model_dir: target.model_dir().display().to_string(),
        model: target.model,
        language: target.language,
        managed,
        owner_module: target.owner_module,
        owner_id: target.owner_id,
        message: if ready {
            if managed {
                "Qwen3-ASR service is running under Bifrost management.".to_string()
            } else {
                "Qwen3-ASR local server is reachable, but it is not managed by this Bifrost process.".to_string()
            }
        } else if installed {
            "Qwen3-ASR files are installed, but the model service is stopped or not healthy."
                .to_string()
        } else {
            "Qwen3-ASR files are not installed yet.".to_string()
        },
    })
}

async fn handle_init_stream(req: Request<Incoming>) -> Response<BoxBody> {
    let target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if let Err(reason) = detect_asr_release_asset() {
        return sse_response(move |tx| async move {
            send_error(
                &tx,
                "Qwen3-ASR is not supported on this operating system.",
                Some(&reason),
            )
            .await;
        });
    }
    if target.assets_installed() {
        return sse_response(move |tx| async move {
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "preflight",
                    status: "running",
                    progress: 92,
                    message: "Checking ASR runtime dependencies.",
                    detail: Some("ffmpeg"),
                    file: None,
                    server_url: Some(target.server_url_display()),
                },
            )
            .await;
            if let Err(error) = ensure_ffmpeg_available(&target, Some(&tx)).await {
                send_error(
                    &tx,
                    "Qwen3-ASR initialization self-check failed.",
                    Some(&error),
                )
                .await;
                return;
            }
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "installed",
                    status: "ready",
                    progress: 100,
                    message: "Qwen3-ASR assets are already installed. Start the model service when you need transcription.",
                    detail: None,
                    file: None,
                    server_url: Some(target.server_url_display()),
                },
            )
            .await;
            send_done(&tx).await;
        });
    }

    let task = ensure_asr_init_task(target).await;
    sse_response(move |tx| stream_asr_init_task(task, tx))
}

async fn handle_service_start(req: Request<Incoming>) -> Response<BoxBody> {
    let target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    match start_managed_service(target).await {
        Ok(response) => json_response(&response),
        Err(response) if is_service_busy(&response) => {
            json_response_with_status(StatusCode::CONFLICT, &response)
        }
        Err(response) => json_response(&response),
    }
}

async fn handle_service_stop(req: Request<Incoming>) -> Response<BoxBody> {
    let target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    let existing = MANAGED_SERVICE.lock().await.take();
    match existing {
        Some(mut managed) if target_matches_request(&managed.target, &target) => {
            let _ = managed.child.kill().await;
            let _ = clear_service_state(&bifrost_storage::data_dir());
            json_response(&AsrServiceResponse {
                ready: false,
                managed: false,
                server_url: managed.target.server_url_display(),
                message: "Qwen3-ASR managed model service stopped.".to_string(),
                detail: None,
            })
        }
        Some(managed) => {
            let managed_url = managed.target.server_url_display();
            *MANAGED_SERVICE.lock().await = Some(managed);
            json_response(&AsrServiceResponse {
                ready: probe_asr_health(&target).await.is_ok(),
                managed: false,
                server_url: target.server_url_display(),
                message: "No matching Bifrost-managed ASR service is running for this target."
                    .to_string(),
                detail: Some(format!("managed target is {managed_url}")),
            })
        }
        None => {
            if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
                if state.host == target.host
                    && state.model == target.model
                    && state.home == target.home
                    && target.port.is_none_or(|port| port == state.port)
                {
                    if let Some(pid) = state.pid {
                        let _ = stop_pid(pid);
                    }
                    let _ = clear_service_state(&bifrost_storage::data_dir());
                    return json_response(&AsrServiceResponse {
                        ready: false,
                        managed: false,
                        server_url: format!("http://{}:{}", state.host, state.port),
                        message: "Qwen3-ASR persisted model service stopped.".to_string(),
                        detail: Some(format!("managed by {}", state.managed_by)),
                    });
                }
            }
            json_response(&AsrServiceResponse {
                ready: probe_asr_health(&target).await.is_ok(),
                managed: false,
                server_url: target.server_url_display(),
                message: "No Bifrost-managed ASR service is running.".to_string(),
                detail: None,
            })
        }
    }
}

fn query_flag_enabled(query: &str, key: &str) -> bool {
    query_param_value(query, key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

fn query_param_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (pair_key, pair_value) = pair.split_once('=')?;
        (pair_key == key).then(|| {
            urlencoding::decode(pair_value)
                .map(|value| value.into_owned())
                .unwrap_or_else(|_| pair_value.to_string())
        })
    })
}

async fn handle_transcribe_stream(req: Request<Incoming>) -> Response<BoxBody> {
    let requested_target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if let Some(conflict) = find_conflicting_healthy_service(&requested_target).await {
        let response = service_busy_response(&requested_target, &conflict);
        return sse_response(move |tx| async move {
            send_error(&tx, &response.message, response.detail.as_deref()).await;
        });
    }
    let target = resolve_managed_target(requested_target).await;
    let content_type = match req
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
    {
        Some(value) if value.starts_with("multipart/form-data") => value,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "ASR transcription requires multipart/form-data",
            );
        }
    };

    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read audio upload: {error}"),
            );
        }
    };
    if body.len() > MAX_ASR_UPLOAD_BYTES {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ASR upload is too large for bounded local streaming",
        );
    }

    sse_response(move |tx| async move {
        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "preflight",
                status: "running",
                progress: 5,
                message: "Checking Qwen3-ASR local server before transcription.",
                detail: None,
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;

        if let Err(error) = probe_asr_health(&target).await {
            send_error(
                &tx,
                "Qwen3-ASR model service is not running. Start it from AI > Tools > ASR first.",
                Some(&format!("server: {}; {error}", target.server_url_display())),
            )
            .await;
            return;
        }

        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "upload",
                status: "running",
                progress: 30,
                message: "Reading uploaded audio.",
                detail: None,
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;

        let wav_bytes = match prepare_audio_for_asr(&content_type, &body).await {
            Ok(bytes) => bytes,
            Err(error) => {
                send_error(&tx, "Failed to prepare audio for Qwen3-ASR.", Some(&error)).await;
                return;
            }
        };

        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "preprocess",
                status: "running",
                progress: 55,
                message: "Audio normalized to 16 kHz mono WAV.",
                detail: None,
                file: Some("upload.wav"),
                server_url: Some(target.server_url_display()),
            },
        )
        .await;

        let Some(server_url) = target.server_url() else {
            send_error(
                &tx,
                "Qwen3-ASR model service is not running. Start it from AI > Tools > ASR first.",
                Some("no managed ASR server port is available"),
            )
            .await;
            return;
        };

        let tmp_dir = match tempfile::tempdir() {
            Ok(tmp_dir) => tmp_dir,
            Err(error) => {
                send_error(
                    &tx,
                    "Failed to create ASR temp directory.",
                    Some(&error.to_string()),
                )
                .await;
                return;
            }
        };
        let source_wav = tmp_dir.path().join("upload.wav");
        if let Err(error) = std::fs::write(&source_wav, &wav_bytes) {
            send_error(
                &tx,
                "Failed to write normalized ASR audio.",
                Some(&error.to_string()),
            )
            .await;
            return;
        }

        let approx_duration_ms = wav_pcm_duration_ms(&wav_bytes);

        match transcribe_uploaded_wav_with_voiceprint_speakers(
            &server_url,
            &target.language,
            &source_wav,
            Some(&tx),
        )
        .await
        {
            Ok(Some(diarized)) => {
                send_progress(
                    &tx,
                    AsrStreamPayload {
                        phase: "diarization",
                        status: "done",
                        progress: 95,
                        message: "Speaker-aware transcription completed.",
                        detail: Some("voiceprint matching enabled"),
                        file: None,
                        server_url: Some(target.server_url_display()),
                    },
                )
                .await;
                send_text(&tx, diarized.text.trim()).await;
                send_progress(
                    &tx,
                    AsrStreamPayload {
                        phase: "done",
                        status: "done",
                        progress: 100,
                        message: "Transcription completed.",
                        detail: Some("speaker-aware upload"),
                        file: None,
                        server_url: Some(target.server_url_display()),
                    },
                )
                .await;
                send_done(&tx).await;
                return;
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    error,
                    "speaker-aware upload transcription failed; falling back to plain ASR"
                );
            }
        }

        match transcribe_uploaded_wav_in_chunks(
            &tx,
            &server_url,
            &target.language,
            &source_wav,
            approx_duration_ms,
        )
        .await
        {
            Ok(text) => {
                send_progress(
                    &tx,
                    AsrStreamPayload {
                        phase: "transcribe",
                        status: "done",
                        progress: 98,
                        message: "Chunked transcription completed.",
                        detail: Some("model window: 30s"),
                        file: None,
                        server_url: Some(target.server_url_display()),
                    },
                )
                .await;
                send_text(&tx, text.trim()).await;
            }
            Err(error) => {
                send_error(&tx, "ASR transcription failed.", Some(&error)).await;
                return;
            }
        }

        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "done",
                status: "done",
                progress: 100,
                message: "Transcription completed.",
                detail: None,
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;
        send_done(&tx).await;
    })
}

async fn transcribe_uploaded_wav_in_chunks(
    tx: &tokio::sync::mpsc::Sender<Bytes>,
    server_url: &str,
    language: &str,
    source_wav: &Path,
    media_duration_ms: Option<u64>,
) -> Result<String, String> {
    let Some(media_duration_ms) = media_duration_ms else {
        send_progress(
            tx,
            AsrStreamPayload {
                phase: "transcribe",
                status: "running",
                progress: 60,
                message: "Sending audio to Qwen3-ASR model.",
                detail: Some("duration unavailable; using one bounded request"),
                file: Some("upload.wav"),
                server_url: Some(server_url.to_string()),
            },
        )
        .await;
        let result = call_asr_whole_file_endpoint(server_url, language, source_wav, None).await?;
        return Ok(result.text);
    };

    let boundaries = plan_upload_chunk_boundaries(media_duration_ms);
    send_progress(
        tx,
        AsrStreamPayload {
            phase: "transcribe",
            status: "running",
            progress: 60,
            message: "Sending audio to Qwen3-ASR model in 30 second chunks.",
            detail: Some(&format!(
                "chunks: {}; chunk_duration_secs: {}; overlap_secs: {}",
                boundaries.len(),
                ASR_UPLOAD_CHUNK_DURATION_SECS,
                ASR_UPLOAD_CHUNK_OVERLAP_SECS
            )),
            file: Some("upload.wav"),
            server_url: Some(server_url.to_string()),
        },
    )
    .await;

    let temp =
        tempfile::tempdir().map_err(|error| format!("create ASR chunk temp dir: {error}"))?;
    let mut committed = String::new();
    let mut segment_index = 0usize;

    for (chunk_index, (offset_secs, duration_secs)) in boundaries.iter().copied().enumerate() {
        let chunk_path = if boundaries.len() == 1 {
            source_wav.to_path_buf()
        } else {
            let path = temp
                .path()
                .join(format!("upload-chunk-{chunk_index:04}.wav"));
            ffmpeg_split_upload_chunk(source_wav, &path, offset_secs, duration_secs).await?;
            path
        };

        let chunk_duration_ms = duration_secs * 1000;
        let result = call_asr_whole_file_endpoint(
            server_url,
            language,
            &chunk_path,
            Some(chunk_duration_ms),
        )
        .await
        .map_err(|error| format!("chunk {} at {}s: {error}", chunk_index + 1, offset_secs))?;

        let chunk_offset_ms = offset_secs * 1000;
        for (local_start_ms, local_end_ms, seg_text) in result.segments.iter() {
            let start_ms = chunk_offset_ms.saturating_add(*local_start_ms);
            let end_ms = chunk_offset_ms.saturating_add(*local_end_ms);
            let delta = if committed.is_empty() {
                seg_text.clone()
            } else {
                dedupe_increment(&committed, seg_text)
            };
            if !delta.is_empty() {
                append_transcript_delta(&mut committed, &delta);
            }
            send_asr_segment(
                tx,
                "final",
                AsrSegmentPayload {
                    index: segment_index,
                    start_ms,
                    end_ms,
                    stable_start_ms: start_ms,
                    stable_end_ms: end_ms,
                    text: seg_text,
                    delta: &delta,
                    committed: &committed,
                    speaker: None,
                    speaker_display_name: None,
                    speaker_profile_id: None,
                    speaker_confidence: None,
                },
            )
            .await;
            segment_index += 1;
        }

        if result.segments.is_empty() && !result.text.trim().is_empty() {
            let delta = if committed.is_empty() {
                result.text.clone()
            } else {
                dedupe_increment(&committed, &result.text)
            };
            if !delta.is_empty() {
                append_transcript_delta(&mut committed, &delta);
            }
            let start_ms = chunk_offset_ms;
            let end_ms = chunk_offset_ms.saturating_add(chunk_duration_ms);
            send_asr_segment(
                tx,
                "final",
                AsrSegmentPayload {
                    index: segment_index,
                    start_ms,
                    end_ms,
                    stable_start_ms: start_ms,
                    stable_end_ms: end_ms,
                    text: &result.text,
                    delta: &delta,
                    committed: &committed,
                    speaker: None,
                    speaker_display_name: None,
                    speaker_profile_id: None,
                    speaker_confidence: None,
                },
            )
            .await;
            segment_index += 1;
        }

        let _ = std::fs::remove_file(&chunk_path);
        let progress = 60 + (((chunk_index + 1) as u64 * 35) / boundaries.len() as u64) as u8;
        send_progress(
            tx,
            AsrStreamPayload {
                phase: "transcribe",
                status: "running",
                progress: progress.min(95),
                message: "Processed ASR upload chunk.",
                detail: Some(&format!(
                    "chunk: {}/{}; offset_secs: {}; duration_secs: {}",
                    chunk_index + 1,
                    boundaries.len(),
                    offset_secs,
                    duration_secs
                )),
                file: chunk_path.file_name().and_then(|name| name.to_str()),
                server_url: Some(server_url.to_string()),
            },
        )
        .await;
    }

    Ok(committed)
}

fn wav_pcm_duration_ms(wav_bytes: &[u8]) -> Option<u64> {
    // prepare_audio_for_asr always emits 16 kHz mono 16-bit PCM WAV:
    // 16000 samples/s * 2 bytes = 32000 bytes/s, plus a small RIFF header.
    (wav_bytes.len() > 44).then(|| ((wav_bytes.len() - 44) as u64 * 1000) / 32000)
}

fn plan_upload_chunk_boundaries(media_duration_ms: u64) -> Vec<(u64, u64)> {
    let duration_secs = media_duration_ms.div_ceil(1000).max(1);
    if duration_secs <= ASR_UPLOAD_CHUNK_DURATION_SECS {
        return vec![(0, duration_secs)];
    }
    let step_secs = ASR_UPLOAD_CHUNK_DURATION_SECS
        .saturating_sub(ASR_UPLOAD_CHUNK_OVERLAP_SECS)
        .max(1);
    let mut boundaries = Vec::new();
    let mut offset = 0u64;
    while offset < duration_secs {
        let remaining = duration_secs - offset;
        let this_chunk = remaining.min(ASR_UPLOAD_CHUNK_DURATION_SECS);
        boundaries.push((offset, this_chunk));
        offset += step_secs;
        if offset < duration_secs && duration_secs - offset <= ASR_UPLOAD_CHUNK_OVERLAP_SECS {
            if let Some(last) = boundaries.last_mut() {
                last.1 = duration_secs - last.0;
            }
            break;
        }
    }
    boundaries
}

async fn ffmpeg_split_upload_chunk(
    source: &Path,
    output: &Path,
    offset_secs: u64,
    duration_secs: u64,
) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(offset_secs.to_string())
        .arg("-t")
        .arg(duration_secs.to_string())
        .arg("-i")
        .arg(source)
        .arg("-c")
        .arg("copy")
        .arg(output)
        .output()
        .await
        .map_err(|error| format!("run ffmpeg upload chunk split: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg upload chunk split failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn sse_response<F, Fut>(run: F) -> Response<BoxBody>
where
    F: FnOnce(tokio::sync::mpsc::Sender<Bytes>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(32);
    tokio::spawn(run(tx));

    let stream =
        ReceiverStream::new(rx).map(|b| Ok::<_, hyper::Error>(hyper::body::Frame::data(b)));
    let body_stream = http_body_util::StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

pub(crate) async fn start_managed_service(
    requested_target: AsrTarget,
) -> Result<AsrServiceResponse, AsrServiceResponse> {
    if let Err(reason) = detect_asr_release_asset() {
        return Err(AsrServiceResponse {
            ready: false,
            managed: false,
            server_url: requested_target.server_url_display(),
            message: "Qwen3-ASR is not supported on this operating system.".to_string(),
            detail: Some(reason),
        });
    }

    if let Some(conflict) = find_conflicting_healthy_service(&requested_target).await {
        return Err(service_busy_response(&requested_target, &conflict));
    }

    if let Some(existing) = find_managed_target(&requested_target).await {
        if probe_asr_health(&existing).await.is_ok() {
            return Ok(AsrServiceResponse {
                ready: true,
                managed: true,
                server_url: existing.server_url_display(),
                message: "Qwen3-ASR managed model service is already running.".to_string(),
                detail: None,
            });
        }
        stop_managed_service_for_target(&existing).await;
    }

    if let Some(existing) = find_reusable_persisted_target(&requested_target) {
        if probe_asr_health(&existing).await.is_ok() {
            return Ok(AsrServiceResponse {
                ready: true,
                managed: true,
                server_url: existing.server_url_display(),
                message: "Qwen3-ASR persisted model service is already running.".to_string(),
                detail: None,
            });
        }
        stop_managed_service_for_target(&existing).await;
    }

    let target = match requested_target.port {
        Some(_) => requested_target,
        None => requested_target.with_port(allocate_loopback_port().map_err(|error| {
            AsrServiceResponse {
                ready: false,
                managed: false,
                server_url: requested_target.server_url_display(),
                message: "Failed to allocate a dynamic ASR service port.".to_string(),
                detail: Some(error),
            }
        })?),
    };

    if let Some(conflict) = find_conflicting_healthy_service(&target).await {
        return Err(service_busy_response(&target, &conflict));
    }

    if !target.assets_installed() {
        run_initializer_silent(target.clone())
            .await
            .map_err(|error| AsrServiceResponse {
                ready: false,
                managed: false,
                server_url: target.server_url_display(),
                message: "Qwen3-ASR self-check could not repair missing assets.".to_string(),
                detail: Some(error),
            })?;
    }

    if !target.assets_installed() {
        return Err(AsrServiceResponse {
            ready: false,
            managed: false,
            server_url: target.server_url_display(),
            message: "Qwen3-ASR assets are missing. Initialize the converter first.".to_string(),
            detail: Some(format!(
                "expected assets under {}",
                target.install_dir().display()
            )),
        });
    }

    ensure_ffmpeg_available(&target, None)
        .await
        .map_err(|error| AsrServiceResponse {
            ready: false,
            managed: false,
            server_url: target.server_url_display(),
            message: "Qwen3-ASR self-check could not prepare ffmpeg.".to_string(),
            detail: Some(error),
        })?;

    if probe_asr_health(&target).await.is_ok() {
        return Ok(AsrServiceResponse {
            ready: true,
            managed: managed_service_matches(&target).await,
            server_url: target.server_url_display(),
            message: "Qwen3-ASR service is already reachable.".to_string(),
            detail: None,
        });
    }

    if let Some(conflict) = find_conflicting_healthy_service(&target).await {
        return Err(service_busy_response(&target, &conflict));
    }

    let log_path = target.install_dir().join("bifrost-managed-asr-server.log");
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| AsrServiceResponse {
            ready: false,
            managed: false,
            server_url: target.server_url_display(),
            message: "Failed to open ASR service log file.".to_string(),
            detail: Some(error.to_string()),
        })?;
    let stderr = stdout.try_clone().map_err(|error| AsrServiceResponse {
        ready: false,
        managed: false,
        server_url: target.server_url_display(),
        message: "Failed to clone ASR service log file.".to_string(),
        detail: Some(error.to_string()),
    })?;

    let asr_server = labeled_process_executable(
        &target.install_dir().join("asr-server"),
        "bifrost-asr-server",
    );
    let mut command = Command::new(asr_server);
    command
        .arg("--model-dir")
        .arg(target.model_dir())
        .arg("--host")
        .arg(&target.host)
        .arg("--port")
        .arg(
            target
                .port
                .expect("managed ASR target must have a port")
                .to_string(),
        )
        .arg("--language")
        .arg(&target.language)
        .kill_on_drop(true)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    configure_service_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| AsrServiceResponse {
        ready: false,
        managed: false,
        server_url: target.server_url_display(),
        message: "Failed to start Qwen3-ASR model service.".to_string(),
        detail: Some(error.to_string()),
    })?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(SERVICE_START_TIMEOUT_SECS);
    loop {
        if probe_asr_health(&target).await.is_ok() {
            let service_pid = child.id();
            let _ = write_service_state(
                &bifrost_storage::data_dir(),
                &AsrServiceState {
                    host: target.host.clone(),
                    port: target.port.expect("healthy ASR target must have a port"),
                    model: target.model.clone(),
                    language: target.language.clone(),
                    home: target.home.clone(),
                    pid: child.id(),
                    managed_by: target.owner_module.clone(),
                    owner_module: Some(target.owner_module.clone()),
                    owner_id: target.owner_id.clone(),
                    started_at_ms: now_ms(),
                },
            );
            *MANAGED_SERVICE.lock().await = Some(ManagedAsrService {
                target: target.clone(),
                child,
            });
            if let Some(pid) = service_pid {
                spawn_service_footprint_watchdog(target.clone(), pid, log_path.clone());
            }
            return Ok(AsrServiceResponse {
                ready: true,
                managed: true,
                server_url: target.server_url_display(),
                message: "Qwen3-ASR managed model service started.".to_string(),
                detail: Some(format!("log: {}", log_path.display())),
            });
        }

        if let Ok(Some(status)) = child.try_wait() {
            return Err(AsrServiceResponse {
                ready: false,
                managed: false,
                server_url: target.server_url_display(),
                message: "Qwen3-ASR model service exited before becoming healthy.".to_string(),
                detail: Some(format!("status: {status}; log: {}", log_path.display())),
            });
        }

        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(AsrServiceResponse {
                ready: false,
                managed: false,
                server_url: target.server_url_display(),
                message: "Timed out waiting for Qwen3-ASR model service to become healthy."
                    .to_string(),
                detail: Some(format!("log: {}", log_path.display())),
            });
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn managed_service_matches(target: &AsrTarget) -> bool {
    if MANAGED_SERVICE
        .lock()
        .await
        .as_ref()
        .map(|service| same_target(&service.target, target))
        .unwrap_or(false)
    {
        return true;
    }
    read_service_state(&bifrost_storage::data_dir())
        .map(|state| {
            state.host == target.host
                && target.port == Some(state.port)
                && state.model == target.model
                && state.home == target.home
        })
        .unwrap_or(false)
}

async fn prepare_audio_for_asr(content_type: &str, body: &Bytes) -> Result<Vec<u8>, String> {
    let boundary =
        multipart_boundary(content_type).ok_or_else(|| "multipart boundary missing".to_string())?;
    let upload = extract_multipart_audio(body, &boundary)?;
    transcode_audio_to_wav(upload).await
}

async fn transcode_audio_to_wav(upload: UploadedAudio) -> Result<Vec<u8>, String> {
    let tmp_dir = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;
    let input_path = tmp_dir.path().join(format!(
        "input-{}{}",
        Uuid::new_v4(),
        file_extension(&upload.filename)
    ));
    let output_path = tmp_dir.path().join("upload.wav");
    std::fs::write(&input_path, upload.bytes)
        .map_err(|error| format!("write uploaded audio: {error}"))?;

    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(&input_path)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(&output_path)
        .output()
        .await
        .map_err(|error| {
            format!("failed to run ffmpeg. Install it with `brew install ffmpeg`. error: {error}")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg failed to convert {} to WAV: {}",
            upload.filename,
            stderr.trim()
        ));
    }

    std::fs::read(&output_path).map_err(|error| format!("read normalized WAV: {error}"))
}

fn file_extension(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_else(|| ".audio".to_string())
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("boundary="))
        .map(|value| value.trim_matches('"').to_string())
}

fn extract_multipart_audio(bytes: &Bytes, boundary: &str) -> Result<UploadedAudio, String> {
    let marker = format!("--{boundary}");
    let body = bytes.as_ref();
    let mut offset = 0usize;
    while let Some(start) = find_bytes(&body[offset..], marker.as_bytes()) {
        let part_start = offset + start + marker.len();
        let Some(headers_end_rel) = find_bytes(&body[part_start..], b"\r\n\r\n") else {
            break;
        };
        let headers_start = part_start + 2;
        let headers_end = part_start + headers_end_rel;
        let headers = String::from_utf8_lossy(&body[headers_start..headers_end]);
        let data_start = headers_end + 4;
        let next_marker = format!("\r\n--{boundary}");
        let Some(data_end_rel) = find_bytes(&body[data_start..], next_marker.as_bytes()) else {
            break;
        };
        let data_end = data_start + data_end_rel;
        if headers.contains("name=\"file\"") {
            let filename =
                extract_multipart_filename(&headers).unwrap_or_else(|| "upload.audio".to_string());
            return Ok(UploadedAudio {
                bytes: body[data_start..data_end].to_vec(),
                filename,
            });
        }
        offset = data_end;
    }
    Err("multipart field 'file' missing".to_string())
}

fn extract_multipart_filename(headers: &str) -> Option<String> {
    headers
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("filename="))
        .map(|value| value.trim_matches('"').to_string())
        .filter(|value| !value.trim().is_empty())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn find_managed_target(target: &AsrTarget) -> Option<AsrTarget> {
    MANAGED_SERVICE
        .lock()
        .await
        .as_ref()
        .filter(|service| target_matches_request(&service.target, target))
        .map(|service| service.target.clone())
}

fn find_reusable_persisted_target(target: &AsrTarget) -> Option<AsrTarget> {
    let state = read_service_state(&bifrost_storage::data_dir())?;
    let existing = target.with_state(&state);
    persisted_target_matches_request(target, &existing).then_some(existing)
}

async fn find_conflicting_healthy_service(target: &AsrTarget) -> Option<AsrTarget> {
    let existing_managed = MANAGED_SERVICE
        .lock()
        .await
        .as_ref()
        .map(|service| service.target.clone());
    if let Some(existing) = existing_managed {
        if !target_matches_request(&existing, target) && probe_asr_health(&existing).await.is_ok() {
            return Some(existing);
        }
    }

    let state = read_service_state(&bifrost_storage::data_dir())?;
    let existing = target.with_state(&state);
    if target_matches_request(&existing, target) {
        return None;
    }
    probe_asr_health(&existing).await.ok().map(|_| existing)
}

fn service_busy_response(target: &AsrTarget, existing: &AsrTarget) -> AsrServiceResponse {
    AsrServiceResponse {
        ready: false,
        managed: true,
        server_url: existing.server_url_display(),
        message: "Qwen3-ASR service is busy.".to_string(),
        detail: Some(format!(
            "requested owner={} model={}; active owner={} model={} server={}",
            target.owner_label(),
            target.model,
            existing.owner_label(),
            existing.model,
            existing.server_url_display()
        )),
    }
}

fn is_service_busy(response: &AsrServiceResponse) -> bool {
    response.message == "Qwen3-ASR service is busy."
}

fn labeled_process_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    match bifrost_core::process_alias_executable(executable, &alias_dir, alias_name) {
        Ok(alias) => alias,
        Err(error) => {
            warn!(
                executable = %executable.display(),
                alias_name = %alias_name,
                error = %error,
                "falling back to unlabeled ASR executable"
            );
            executable.to_path_buf()
        }
    }
}

pub(crate) async fn resolve_managed_target(target: AsrTarget) -> AsrTarget {
    if target.port.is_some() {
        return target;
    }
    if let Some(managed) = find_managed_target(&target).await {
        return managed;
    }
    let Some(state) = read_service_state(&bifrost_storage::data_dir()) else {
        return target;
    };
    if state.host == target.host && state.model == target.model && state.home == target.home {
        let persisted = target.with_state(&state);
        if probe_asr_health(&persisted).await.is_ok() {
            return persisted;
        }
    }
    target
}

/// Shutdown the managed ASR service (if any). Called during Bifrost process exit
/// to ensure the child ASR server doesn't become an orphan.
pub async fn shutdown_managed_asr_service() {
    let managed = MANAGED_SERVICE.lock().await.take();
    if let Some(mut managed) = managed {
        let pid = managed.child.id();
        if let Some(pid) = pid {
            tracing::info!(pid, "Shutting down managed ASR service on Bifrost exit");
            terminate_service_process_group(Some(pid));
            let _ = managed.child.kill().await;
        } else {
            tracing::info!("Managed ASR service already exited, cleaning up state");
        }
        let _ = crate::asr_runtime::clear_service_state(&bifrost_storage::data_dir());
    }
}

pub(crate) async fn stop_managed_service_for_target(target: &AsrTarget) {
    let managed_to_stop = {
        let mut guard = MANAGED_SERVICE.lock().await;
        let should_stop = guard
            .as_ref()
            .map(|managed| target_matches_request(&managed.target, target))
            .unwrap_or(false);
        if should_stop {
            guard.take()
        } else {
            None
        }
    };
    if let Some(mut managed) = managed_to_stop {
        terminate_service_process_group(managed.child.id());
        let _ = managed.child.kill().await;
    }
    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if state.host == target.host
            && state.model == target.model
            && state.home == target.home
            && target.port.is_none_or(|port| port == state.port)
        {
            if let Some(pid) = state.pid {
                terminate_service_process_group(Some(pid));
                let _ = stop_pid(pid);
            }
            let _ = clear_service_state(&bifrost_storage::data_dir());
        }
    }
}

fn spawn_service_footprint_watchdog(target: AsrTarget, pid: u32, log_path: PathBuf) {
    let Some(limit) = default_footprint_limit_bytes(&target.model_dir()) else {
        return;
    };
    tokio::spawn(async move {
        let mut peak = 0u64;
        let mut sample_failures = 0u32;
        let mut advisory_warning_logged_at: Option<Instant> = None;
        let mut sampler_warning_logged_at: Option<Instant> = None;
        let sample_interval = physical_footprint_sample_interval();
        loop {
            let sample = tokio::task::spawn_blocking(move || read_process_footprint_bytes(pid))
                .await
                .unwrap_or_else(|error| Err(format!("join footprint sampler: {error}")));
            match sample {
                Ok((footprint, reliable)) => {
                    if reliable {
                        sample_failures = 0;
                    } else {
                        sample_failures += 1;
                        warn!(
                            pid,
                            sample_failures,
                            rss_mb = footprint / 1024 / 1024,
                            "ASR service physical footprint sampling fell back to RSS"
                        );
                    }
                    peak = peak.max(footprint);
                    if service_watchdog_should_kill_for_sample(footprint, reliable, limit) {
                        warn!(
                            pid,
                            model = %target.model,
                            footprint_mb = footprint / 1024 / 1024,
                            limit_mb = limit / 1024 / 1024,
                            peak_mb = peak / 1024 / 1024,
                            "ASR managed service exceeded footprint limit; terminating"
                        );
                        append_service_watchdog_log(
                            &log_path,
                            format!(
                                "Bifrost ASR service watchdog killed pid={pid}: footprint_mb={} limit_mb={} peak_mb={}\n",
                                footprint / 1024 / 1024,
                                limit / 1024 / 1024,
                                peak / 1024 / 1024
                            ),
                        );
                        terminate_service_process_group(Some(pid));
                        clear_managed_service_for_pid(pid).await;
                        break;
                    }
                    if !reliable && sample_failures >= SERVICE_FOOTPRINT_MAX_SAMPLE_FAILURES {
                        if service_watchdog_should_log_warning(
                            &mut advisory_warning_logged_at,
                            Instant::now(),
                        ) {
                            warn!(
                                pid,
                                model = %target.model,
                                sample_failures,
                                rss_mb = footprint / 1024 / 1024,
                                limit_mb = limit / 1024 / 1024,
                                peak_mb = peak / 1024 / 1024,
                                "ASR service physical footprint unavailable repeatedly; continuing with RSS-only advisory samples"
                            );
                            append_service_watchdog_log(
                                &log_path,
                                format!(
                                    "Bifrost ASR service watchdog advisory pid={pid}: physical footprint unavailable {sample_failures} times; rss_mb={} limit_mb={} peak_mb={}; process_alive=true continuing\n",
                                    footprint / 1024 / 1024,
                                    limit / 1024 / 1024,
                                    peak / 1024 / 1024
                                ),
                            );
                        }
                        sample_failures = 0;
                    }
                }
                Err(error) => {
                    if !process_is_alive(pid) {
                        append_service_watchdog_log(
                            &log_path,
                            format!(
                                "Bifrost ASR service watchdog noticed pid={pid} exited during footprint sampling: {error}\n"
                            ),
                        );
                        clear_managed_service_for_pid(pid).await;
                        break;
                    }
                    sample_failures += 1;
                    if sample_failures >= SERVICE_FOOTPRINT_MAX_SAMPLE_FAILURES {
                        if service_watchdog_should_log_warning(
                            &mut sampler_warning_logged_at,
                            Instant::now(),
                        ) {
                            warn!(
                                pid,
                                model = %target.model,
                                sample_failures,
                                limit_mb = limit / 1024 / 1024,
                                peak_mb = peak / 1024 / 1024,
                                %error,
                                "ASR service footprint sampler failed repeatedly while process is alive; continuing without killing service"
                            );
                            append_service_watchdog_log(
                                &log_path,
                                format!(
                                    "Bifrost ASR service watchdog sampler_failure pid={pid}: footprint sampling failed {sample_failures} times; process_alive=true limit_mb={} peak_mb={}; continuing; last_error={error}\n",
                                    limit / 1024 / 1024,
                                    peak / 1024 / 1024
                                ),
                            );
                        }
                        sample_failures = 0;
                    }
                }
            }
            tokio::time::sleep(sample_interval).await;
        }
    });
}

async fn clear_managed_service_for_pid(pid: u32) {
    let mut managed = MANAGED_SERVICE.lock().await;
    if managed
        .as_ref()
        .and_then(|service| service.child.id())
        .is_some_and(|managed_pid| managed_pid == pid)
    {
        *managed = None;
    }
    if read_service_state(&bifrost_storage::data_dir())
        .and_then(|state| state.pid)
        .is_some_and(|stored_pid| stored_pid == pid)
    {
        let _ = clear_service_state(&bifrost_storage::data_dir());
    }
}

fn append_service_watchdog_log(log_path: &Path, line: String) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        use std::io::Write;
        let _ = file.write_all(line.as_bytes());
    }
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn configure_service_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_service_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_service_process_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
}

#[cfg(not(unix))]
fn terminate_service_process_group(_pid: Option<u32>) {}

fn same_target(left: &AsrTarget, right: &AsrTarget) -> bool {
    left.host == right.host
        && left.port == right.port
        && left.model == right.model
        && left.home == right.home
}

fn same_logical_target(left: &AsrTarget, right: &AsrTarget) -> bool {
    left.host == right.host && left.model == right.model && left.home == right.home
}

fn target_matches_request(managed: &AsrTarget, requested: &AsrTarget) -> bool {
    same_logical_target(managed, requested)
        && requested
            .port
            .map(|requested_port| managed.port == Some(requested_port))
            .unwrap_or(true)
}

#[cfg(test)]
fn same_service_owner(left: &AsrTarget, right: &AsrTarget) -> bool {
    left.owner_module == right.owner_module && left.owner_id == right.owner_id
}

/// 检查持久化的 ASR target 是否匹配当前请求。
/// 注意参数顺序：第一个是请求方的 target，第二个是从持久化 state 恢复的 target。
/// 内部委托 target_matches_request(existing=persisted, requested=requested)。
fn persisted_target_matches_request(requested: &AsrTarget, persisted: &AsrTarget) -> bool {
    target_matches_request(persisted, requested)
}

async fn run_initializer(
    target: AsrTarget,
    tx: tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    debug!("starting qwen3-asr rust initializer");

    run_preflight(&target, &tx).await?;
    download_asr_assets(&target, tx.clone()).await?;
    ensure_ffmpeg_available(&target, Some(&tx)).await?;
    install_release(&target, &tx).await?;
    prepare_model(&target, &tx).await?;
    verify_cli_sample(&target, &tx).await?;
    Ok(())
}

async fn run_initializer_silent(target: AsrTarget) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel::<Bytes>(32);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = run_initializer(target, tx).await;
    let _ = drain.await;
    result
}

/// Public wrapper around `run_initializer_silent` for use by `asr_jobs`.
pub(crate) async fn run_initializer_silent_pub(target: AsrTarget) -> Result<(), String> {
    run_initializer_silent(target).await
}

async fn ensure_asr_init_task(target: AsrTarget) -> AsrInitTask {
    let mut current = ASR_INIT_TASK.lock().await;
    if let Some(task) = current.as_ref() {
        if !task.finished.load(Ordering::SeqCst) && same_logical_target(&task.target, &target) {
            return task.clone();
        }
    }

    let (sender, _) = broadcast::channel::<String>(256);
    let history = Arc::new(Mutex::new(Vec::new()));
    let finished = Arc::new(AtomicBool::new(false));
    let task = AsrInitTask {
        target: target.clone(),
        sender: sender.clone(),
        history: Arc::clone(&history),
        finished: Arc::clone(&finished),
    };
    *current = Some(task.clone());

    let (event_tx, mut event_rx) = mpsc::channel::<Bytes>(64);
    let collector_sender = sender.clone();
    let collector_history = Arc::clone(&history);
    tokio::spawn(async move {
        while let Some(frame) = event_rx.recv().await {
            let frame = String::from_utf8_lossy(&frame).to_string();
            {
                let mut history = collector_history.lock().await;
                history.push(frame.clone());
                if history.len() > 200 {
                    let overflow = history.len() - 200;
                    history.drain(0..overflow);
                }
            }
            let _ = collector_sender.send(frame);
        }
    });

    tokio::spawn(async move {
        send_progress(
            &event_tx,
            AsrStreamPayload {
                phase: "install",
                status: "running",
                progress: 8,
                message: "Preparing ASR assets asynchronously.",
                detail: None,
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;
        match run_initializer(target.clone(), event_tx.clone()).await {
            Ok(()) => {
                send_progress(
                    &event_tx,
                    AsrStreamPayload {
                        phase: "installed",
                        status: "ready",
                        progress: 100,
                        message: "Qwen3-ASR assets are ready. Start the model service when you need transcription.",
                        detail: None,
                        file: None,
                        server_url: Some(target.server_url_display()),
                    },
                )
                .await;
                send_done(&event_tx).await;
            }
            Err(error) => {
                send_error(&event_tx, "Qwen3-ASR initialization failed.", Some(&error)).await;
            }
        }
        finished.store(true, Ordering::SeqCst);
    });

    task
}

async fn stream_asr_init_task(task: AsrInitTask, tx: tokio::sync::mpsc::Sender<Bytes>) {
    let history = task.history.lock().await.clone();
    let history_has_terminal = history
        .iter()
        .any(|frame| frame.starts_with("event: done") || frame.starts_with("event: error"));
    for frame in history {
        if tx.send(Bytes::from(frame)).await.is_err() {
            return;
        }
    }
    if history_has_terminal {
        return;
    }

    let mut rx = task.sender.subscribe();
    loop {
        match rx.recv().await {
            Ok(frame) => {
                let is_terminal =
                    frame.starts_with("event: done") || frame.starts_with("event: error");
                if tx.send(Bytes::from(frame)).await.is_err() || is_terminal {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn run_preflight(
    target: &AsrTarget,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    let asset = detect_asr_release_asset()?;
    send_progress(
        tx,
        AsrStreamPayload {
            phase: "preflight",
            status: "running",
            progress: 10,
            message: "Qwen3-ASR platform and dependency checks passed.",
            detail: Some(asset),
            file: None,
            server_url: Some(target.server_url_display()),
        },
    )
    .await;
    Ok(())
}

async fn ensure_ffmpeg_available(
    target: &AsrTarget,
    tx: Option<&tokio::sync::mpsc::Sender<Bytes>>,
) -> Result<(), String> {
    if command_succeeds("ffmpeg", &["-version"]).await {
        return Ok(());
    }
    if !command_succeeds("brew", &["--version"]).await {
        return Err(
            "ffmpeg is required for ASR audio preprocessing, and Homebrew was not found to install it automatically. Install Homebrew and run `brew install ffmpeg`, then retry the same ASR action."
                .to_string(),
        );
    }
    if let Some(tx) = tx {
        send_progress(
            tx,
            AsrStreamPayload {
                phase: "preflight",
                status: "running",
                progress: 68,
                message: "Installing ffmpeg with Homebrew for ASR audio preprocessing.",
                detail: Some("brew install ffmpeg"),
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;
    }
    let output = Command::new("brew")
        .arg("install")
        .arg("ffmpeg")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("install ffmpeg with Homebrew: {error}"))?;
    if output.status.success() && command_succeeds("ffmpeg", &["-version"]).await {
        Ok(())
    } else {
        Err(format!(
            "Homebrew ffmpeg installation failed with {}. Install it manually with `brew install ffmpeg`, then retry the same ASR action. {}",
            output.status,
            summarize_command_output(&output.stdout, &output.stderr)
        ))
    }
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

async fn command_succeeds(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn download_asr_assets(
    target: &AsrTarget,
    tx: tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    let requests = asr_download_requests(target)?;
    if requests.is_empty() {
        return Ok(());
    }

    let client = build_asr_download_client()?;
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let progress_event_tx = tx.clone();
    let server_url = target.server_url_display();
    let progress_task = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            send_download_progress(&progress_event_tx, &server_url, progress).await;
        }
    });

    for request in requests {
        download_with_resume(&client, request, Some(progress_tx.clone())).await?;
    }
    drop(progress_tx);
    let _ = progress_task.await;
    Ok(())
}

fn build_asr_download_client() -> Result<reqwest::Client, String> {
    bifrost_core::direct_reqwest_client_builder()
        .build()
        .map_err(|error| format!("build downloader client: {error}"))
}

async fn send_download_progress(
    tx: &tokio::sync::mpsc::Sender<Bytes>,
    server_url: &str,
    progress: DownloadProgress,
) {
    let detail = download_detail(&progress);
    send_event(
        tx,
        "progress",
        &AsrDownloadProgressPayload {
            phase: "download",
            status: if progress.complete { "done" } else { "running" },
            progress: progress.percent.unwrap_or(0),
            message: if progress.complete {
                "Downloaded ASR resource."
            } else if progress.resumed {
                "Resuming ASR resource download."
            } else {
                "Downloading ASR resource."
            },
            detail: detail.as_deref(),
            file: Some(progress.label.as_str()),
            server_url: Some(server_url.to_string()),
            downloaded_bytes: progress.downloaded_bytes,
            total_bytes: progress.total_bytes,
            download_percent: progress.percent,
            bytes_per_second: progress.bytes_per_second,
            eta_seconds: progress.eta_seconds,
            elapsed_ms: progress.elapsed_ms,
            resumed: progress.resumed,
            complete: progress.complete,
        },
    )
    .await;
}

fn download_detail(progress: &DownloadProgress) -> Option<String> {
    let total = progress.total_bytes?;
    let percent = progress.percent.unwrap_or(0);
    Some(format!(
        "{} / {} bytes ({}%)",
        progress.downloaded_bytes, total, percent
    ))
}

fn asr_download_requests(target: &AsrTarget) -> Result<Vec<DownloadRequest>, String> {
    let mut requests = Vec::new();
    let install_dir = target.install_dir();
    if !install_dir.join("asr").is_file() || !install_dir.join("asr-server").is_file() {
        let asset = detect_asr_release_asset()?;
        requests.push(DownloadRequest {
            url: format!(
                "https://github.com/{ASR_RELEASE_REPO}/releases/latest/download/{asset}.zip"
            ),
            dest: target.home.join(format!("{asset}.zip")),
            label: format!("{asset}.zip"),
        });
    }

    for file in required_model_files(&target.model) {
        let dest = target.model_dir().join(file);
        if !dest.is_file() {
            requests.push(DownloadRequest {
                url: format!(
                    "https://huggingface.co/Qwen/{}/resolve/main/{file}",
                    target.model
                ),
                dest,
                label: format!("{}/{}", target.model, file),
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
        let dest = install_dir.join(sample);
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

fn detect_asr_release_asset() -> Result<&'static str, String> {
    if asr_platform_supported() {
        Ok("asr-macos-aarch64")
    } else {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        Err(format!(
            "Qwen3-ASR local runtime is only supported on Apple Silicon macOS; current platform is {os}-{arch}"
        ))
    }
}

async fn install_release(
    target: &AsrTarget,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    if target.install_dir().join("asr").is_file()
        && target.install_dir().join("asr-server").is_file()
    {
        return Ok(());
    }
    let asset = detect_asr_release_asset()?;
    let zip_path = target.home.join(format!("{asset}.zip"));
    let install_dir = target.install_dir();
    extract_zip_to_dir(&zip_path, &target.home).await?;
    let extracted_dir = target.home.join(asset);
    tokio::fs::create_dir_all(&install_dir)
        .await
        .map_err(|error| format!("create ASR install dir {}: {error}", install_dir.display()))?;
    copy_dir_contents(&extracted_dir, &install_dir).await?;
    let _ = tokio::fs::remove_dir_all(&extracted_dir).await;
    let _ = tokio::fs::remove_file(&zip_path).await;
    mark_asr_binaries_executable(&install_dir).await?;
    send_progress(
        tx,
        AsrStreamPayload {
            phase: "install",
            status: "running",
            progress: 72,
            message: "Qwen3-ASR native runtime installed.",
            detail: Some(install_dir.to_str().unwrap_or("")),
            file: None,
            server_url: Some(target.server_url_display()),
        },
    )
    .await;
    Ok(())
}

async fn prepare_model(
    target: &AsrTarget,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(target.model_dir())
        .await
        .map_err(|error| {
            format!(
                "create ASR model dir {}: {error}",
                target.model_dir().display()
            )
        })?;
    for file in required_model_files(&target.model) {
        let path = target.model_dir().join(file);
        if !path.is_file() {
            return Err(format!("missing ASR model file {}", path.display()));
        }
    }
    let tokenizer_src = target
        .install_dir()
        .join("tokenizers")
        .join(format!("tokenizer-{}.json", tokenizer_size(&target.model)?));
    let tokenizer_dest = target.model_dir().join("tokenizer.json");
    tokio::fs::copy(&tokenizer_src, &tokenizer_dest)
        .await
        .map_err(|error| {
            format!(
                "copy tokenizer {} -> {}: {error}",
                tokenizer_src.display(),
                tokenizer_dest.display()
            )
        })?;
    send_progress(
        tx,
        AsrStreamPayload {
            phase: "install",
            status: "running",
            progress: 82,
            message: "Qwen3-ASR model files are ready.",
            detail: Some(target.model_dir().to_str().unwrap_or("")),
            file: None,
            server_url: Some(target.server_url_display()),
        },
    )
    .await;
    Ok(())
}

async fn verify_cli_sample(
    target: &AsrTarget,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    let sample = target.install_dir().join("sample3.wav");
    if !sample.is_file() {
        return Err(format!("missing ASR sample audio {}", sample.display()));
    }
    send_progress(
        tx,
        AsrStreamPayload {
            phase: "verify",
            status: "running",
            progress: 90,
            message: "Running Qwen3-ASR bundled Chinese sample verification.",
            detail: None,
            file: Some("sample3.wav"),
            server_url: Some(target.server_url_display()),
        },
    )
    .await;
    let asr_cli = labeled_process_executable(&target.install_dir().join("asr"), "bifrost-asr-cli");
    let output = Command::new(asr_cli)
        .arg(target.model_dir())
        .arg(sample)
        .arg(&target.language)
        .output()
        .await
        .map_err(|error| format!("run ASR CLI sample: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "ASR CLI sample exited with {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("Qwen3") && !stdout.contains("语音") && !stdout.contains("测试") {
        return Err(
            "ASR CLI sample output did not contain expected Chinese sample text".to_string(),
        );
    }
    Ok(())
}

async fn extract_zip_to_dir(zip_path: &Path, out_dir: &Path) -> Result<(), String> {
    let zip_path = zip_path.to_path_buf();
    let out_dir = out_dir.to_path_buf();
    tokio::task::spawn_blocking(move || extract_zip_to_dir_blocking(&zip_path, &out_dir))
        .await
        .map_err(|error| format!("join zip extraction task: {error}"))?
}

fn extract_zip_to_dir_blocking(zip_path: &Path, out_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path)
        .map_err(|error| format!("open ASR release zip {}: {error}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("read ASR release zip {}: {error}", zip_path.display()))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("read zip entry {index}: {error}"))?;
        let Some(enclosed_name) = file.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let out_path = out_dir.join(enclosed_name);
        if file.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|error| format!("create zip dir {}: {error}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create zip parent {}: {error}", parent.display()))?;
        }
        let mut output = fs::File::create(&out_path)
            .map_err(|error| format!("create zip file {}: {error}", out_path.display()))?;
        io::copy(&mut file, &mut output)
            .map_err(|error| format!("extract zip file {}: {error}", out_path.display()))?;
    }
    Ok(())
}

async fn copy_dir_contents(from: &Path, to: &Path) -> Result<(), String> {
    let from = from.to_path_buf();
    let to = to.to_path_buf();
    tokio::task::spawn_blocking(move || copy_dir_contents_blocking(&from, &to))
        .await
        .map_err(|error| format!("join copy dir task: {error}"))?
}

fn copy_dir_contents_blocking(from: &Path, to: &Path) -> Result<(), String> {
    for entry in
        fs::read_dir(from).map_err(|error| format!("read dir {}: {error}", from.display()))?
    {
        let entry = entry.map_err(|error| format!("read dir entry {}: {error}", from.display()))?;
        let source = entry.path();
        let dest = to.join(entry.file_name());
        if source.is_dir() {
            fs::create_dir_all(&dest)
                .map_err(|error| format!("create dir {}: {error}", dest.display()))?;
            copy_dir_contents_blocking(&source, &dest)?;
        } else {
            fs::copy(&source, &dest).map_err(|error| {
                format!("copy {} -> {}: {error}", source.display(), dest.display())
            })?;
        }
    }
    Ok(())
}

async fn mark_asr_binaries_executable(install_dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["asr", "asr-server"] {
            let path = install_dir.join(name);
            let mut permissions = tokio::fs::metadata(&path)
                .await
                .map_err(|error| format!("stat ASR binary {}: {error}", path.display()))?
                .permissions();
            permissions.set_mode(0o755);
            tokio::fs::set_permissions(&path, permissions)
                .await
                .map_err(|error| format!("chmod ASR binary {}: {error}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = install_dir;
    }
    Ok(())
}

fn tokenizer_size(model: &str) -> Result<&'static str, String> {
    match model {
        "Qwen3-ASR-0.6B" => Ok("0.6B"),
        "Qwen3-ASR-1.7B" => Ok("1.7B"),
        _ => Err(format!("unsupported model: {model}")),
    }
}

pub(crate) async fn probe_asr_health(target: &AsrTarget) -> Result<(), String> {
    let Some(server_url) = target.server_url() else {
        return Err("no managed ASR service is running".to_string());
    };
    let client = reqwest::Client::new();
    let url = format!("{server_url}/health");
    let response = client
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("health check returned {}", response.status()))
    }
}

async fn send_progress(tx: &tokio::sync::mpsc::Sender<Bytes>, payload: AsrStreamPayload<'_>) {
    send_event(tx, "progress", &payload).await;
}

async fn send_text(tx: &tokio::sync::mpsc::Sender<Bytes>, text: &str) {
    send_event(tx, "text", &AsrTextPayload { text }).await;
}

pub(crate) async fn send_asr_segment(
    tx: &tokio::sync::mpsc::Sender<Bytes>,
    event: &str,
    payload: AsrSegmentPayload<'_>,
) {
    send_event(tx, event, &payload).await;
}

async fn send_error(tx: &tokio::sync::mpsc::Sender<Bytes>, message: &str, detail: Option<&str>) {
    send_event(tx, "error", &AsrErrorPayload { message, detail }).await;
}

async fn send_done(tx: &tokio::sync::mpsc::Sender<Bytes>) {
    send_event(tx, "done", &serde_json::json!({ "ok": true })).await;
}

async fn send_event<T: Serialize>(tx: &tokio::sync::mpsc::Sender<Bytes>, event: &str, payload: &T) {
    let json = serde_json::to_string(payload).unwrap_or_else(|error| {
        serde_json::json!({ "message": format!("failed to serialize event: {error}") }).to_string()
    });
    let frame = format!("event: {event}\ndata: {}\n\n", json.replace('\n', "\\n"));
    let _ = tx.send(Bytes::from(frame)).await;
}

pub(crate) fn target_from_query(query: Option<&str>) -> Result<AsrTarget, String> {
    let params: AsrQuery = query
        .map(serde_urlencoded::from_str)
        .transpose()
        .map_err(|error| format!("invalid ASR query: {error}"))?
        .unwrap_or(AsrQuery {
            host: None,
            port: None,
            language: None,
            model: None,
            owner_module: None,
            owner_id: None,
        });
    let host = params.host.unwrap_or_else(|| DEFAULT_ASR_HOST.to_string());
    validate_loopback_host(&host)?;
    if params.port == Some(0) {
        return Err("ASR port must be between 1 and 65535".to_string());
    }

    Ok(AsrTarget {
        host,
        port: params.port,
        language: params
            .language
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ASR_LANGUAGE.to_string()),
        model: params
            .model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ASR_MODEL.to_string()),
        home: default_home(),
        owner_module: params
            .owner_module
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "model_management".to_string()),
        owner_id: params.owner_id.filter(|value| !value.trim().is_empty()),
    })
}

fn validate_loopback_host(host: &str) -> Result<(), String> {
    match host {
        "127.0.0.1" | "localhost" | "::1" => Ok(()),
        _ => Err("ASR server host must be localhost, 127.0.0.1, or ::1".to_string()),
    }
}

pub(crate) fn default_home() -> PathBuf {
    fixed_asr_home()
}

impl AsrTarget {
    pub(crate) fn server_url(&self) -> Option<String> {
        let port = self.port?;
        Some(if self.host == "::1" {
            format!("http://[::1]:{port}")
        } else {
            format!("http://{}:{port}", self.host)
        })
    }

    pub(crate) fn server_url_display(&self) -> String {
        self.server_url()
            .unwrap_or_else(|| "dynamic port (managed by Bifrost)".to_string())
    }

    fn with_port(&self, port: u16) -> Self {
        let mut target = self.clone();
        target.port = Some(port);
        target
    }

    fn with_state(&self, state: &AsrServiceState) -> Self {
        let mut target = self.clone();
        target.host = state.host.clone();
        target.port = Some(state.port);
        target.model = state.model.clone();
        target.language = state.language.clone();
        target.home = state.home.clone();
        target.owner_module = state.lease_owner_module().to_string();
        target.owner_id = state.owner_id.clone();
        target
    }

    fn owner_label(&self) -> String {
        match self.owner_id.as_deref() {
            Some(owner_id) => format!("{}:{owner_id}", self.owner_module),
            None => self.owner_module.clone(),
        }
    }

    pub(crate) fn install_dir(&self) -> PathBuf {
        install_dir(&self.home)
    }

    pub(crate) fn model_dir(&self) -> PathBuf {
        model_dir(&self.home, &self.model)
    }

    pub(crate) fn assets_installed(&self) -> bool {
        self.install_dir().join("asr").is_file()
            && self.install_dir().join("asr-server").is_file()
            && self.model_dir().join("tokenizer.json").is_file()
            && required_model_files(&self.model)
                .iter()
                .all(|file| self.model_dir().join(file).is_file())
    }
}

fn allocate_loopback_port() -> Result<u16, String> {
    TcpListener::bind((DEFAULT_ASR_HOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex as StdMutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        asr_capabilities_response, asr_download_requests, asr_platform_supported_for,
        build_asr_download_client, default_home, persisted_target_matches_request,
        plan_upload_chunk_boundaries, same_service_owner, service_watchdog_should_kill_for_sample,
        target_from_query, target_matches_request, validate_loopback_host, wav_pcm_duration_ms,
        ASR_UPLOAD_CHUNK_DURATION_SECS, SERVICE_FOOTPRINT_WARNING_LOG_INTERVAL_SECS,
    };

    fn proxy_env_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    struct ProxyEnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl Drop for ProxyEnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn install_invalid_proxy_env() -> ProxyEnvGuard {
        let vars = [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ];
        let saved: Vec<(&'static str, Option<String>)> = vars
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();

        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::set_var(key, "http://127.0.0.1:1");
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");

        ProxyEnvGuard { saved }
    }

    #[test]
    fn asr_platform_support_matrix_is_apple_silicon_macos_only() {
        assert!(asr_platform_supported_for("macos", "aarch64"));
        assert!(!asr_platform_supported_for("macos", "x86_64"));
        assert!(!asr_platform_supported_for("linux", "x86_64"));
        assert!(!asr_platform_supported_for("windows", "x86_64"));
    }

    #[test]
    fn asr_capabilities_are_hidden_on_unsupported_current_platform() {
        let capabilities = asr_capabilities_response();
        let expected_supported =
            asr_platform_supported_for(std::env::consts::OS, std::env::consts::ARCH);
        assert_eq!(capabilities.supported_target, "macos-aarch64");
        assert_eq!(capabilities.qwen3_asr.enabled, expected_supported);
        assert_eq!(capabilities.qwen3_asr.hidden, !expected_supported);
        assert_eq!(capabilities.speaker_diarization.enabled, expected_supported);
        assert_eq!(capabilities.voiceprint.enabled, expected_supported);
        assert_eq!(capabilities.voice_wake_asr.enabled, expected_supported);
    }

    fn spawn_local_http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = stream.flush();
        });
        format!("http://{addr}")
    }

    #[test]
    fn asr_target_rejects_remote_hosts() {
        let err = target_from_query(Some("host=192.168.1.5&port=8080")).unwrap_err();
        assert!(err.contains("loopback") || err.contains("localhost"));
        assert!(validate_loopback_host("example.com").is_err());
    }

    #[test]
    fn asr_target_accepts_loopback_defaults() {
        let target = target_from_query(Some("port=18080&home=/tmp/ignored-asr")).unwrap();
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, Some(18080));
        assert_eq!(
            target.server_url().as_deref(),
            Some("http://127.0.0.1:18080")
        );
        assert_eq!(target.home, default_home());
        assert!(target.home.ends_with(".bifrost/asr"));

        let dynamic_target = target_from_query(None).unwrap();
        assert_eq!(dynamic_target.port, None);
        assert_eq!(
            dynamic_target.server_url_display(),
            "dynamic port (managed by Bifrost)"
        );
    }

    #[test]
    fn asr_download_requests_include_missing_runtime_model_and_samples() {
        let temp = tempfile::tempdir().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-1.7B")).unwrap();
        target.home = temp.path().to_path_buf();
        let Ok(requests) = asr_download_requests(&target) else {
            return;
        };
        let labels = requests
            .iter()
            .map(|request| request.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label.ends_with(".zip")));
        assert!(labels.contains(&"Qwen3-ASR-1.7B/config.json"));
        assert!(labels.contains(&"Qwen3-ASR-1.7B/model-00001-of-00002.safetensors"));
        assert!(labels.contains(&"sample3.wav"));
    }

    #[test]
    fn asr_download_requests_include_qwen3_asr_0_6b_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B")).unwrap();
        target.home = temp.path().to_path_buf();
        std::fs::create_dir_all(target.install_dir()).unwrap();
        std::fs::write(target.install_dir().join("asr"), "").unwrap();
        std::fs::write(target.install_dir().join("asr-server"), "").unwrap();
        let requests = asr_download_requests(&target).unwrap();
        let labels = requests
            .iter()
            .map(|request| request.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Qwen3-ASR-0.6B/config.json"));
        assert!(labels.contains(&"Qwen3-ASR-0.6B/model.safetensors"));
        assert!(requests.iter().any(|request| {
            request
                .url
                .contains("https://huggingface.co/Qwen/Qwen3-ASR-0.6B/resolve/main/config.json")
        }));
    }

    #[test]
    fn asr_download_client_bypasses_proxy_env() {
        let _lock = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _env_guard = install_invalid_proxy_env();
        let url = spawn_local_http_server();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let body = runtime.block_on(async {
            build_asr_download_client()
                .unwrap()
                .get(url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        assert_eq!(body, "ok");
    }

    #[test]
    fn qwen3_service_reuse_ignores_request_language() {
        let managed =
            target_from_query(Some("model=Qwen3-ASR-0.6B&language=chinese&port=18080")).unwrap();
        let requested = target_from_query(Some("model=Qwen3-ASR-0.6B&language=english")).unwrap();

        assert!(target_matches_request(&managed, &requested));
    }

    #[test]
    fn qwen3_service_runtime_is_shared_across_modules_for_same_model() {
        let workbench = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=speech_workbench&port=18080",
        ))
        .unwrap();
        let directory_task = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();
        let same_directory_task = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();

        assert!(!same_service_owner(&workbench, &directory_task));
        assert!(same_service_owner(&directory_task, &same_directory_task));
        assert!(target_matches_request(&workbench, &directory_task));
    }

    #[test]
    fn qwen3_service_runtime_is_shared_across_owner_ids_for_same_model() {
        let task_a = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();
        let task_b = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-b",
        ))
        .unwrap();
        let task_a_without_port = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();

        assert!(!same_service_owner(&task_a, &task_b));
        assert!(same_service_owner(&task_a, &task_a_without_port));
        assert!(target_matches_request(&task_a, &task_b));
    }

    #[test]
    fn qwen3_default_owner_is_model_management() {
        let target = target_from_query(Some("model=Qwen3-ASR-0.6B")).unwrap();

        assert_eq!(target.owner_module, "model_management");
        assert_eq!(target.owner_id, None);
    }

    #[test]
    fn service_watchdog_kills_only_on_reliable_physical_footprint_over_limit() {
        let limit = 10 * 1024 * 1024;

        assert!(service_watchdog_should_kill_for_sample(
            limit + 1,
            true,
            limit
        ));
        assert!(!service_watchdog_should_kill_for_sample(
            limit + 1,
            false,
            limit
        ));
        assert!(!service_watchdog_should_kill_for_sample(limit, true, limit));
    }

    #[test]
    fn service_watchdog_warning_log_is_rate_limited() {
        let mut last_logged = None;
        let start = Instant::now();

        assert!(super::service_watchdog_should_log_warning(
            &mut last_logged,
            start
        ));
        assert!(!super::service_watchdog_should_log_warning(
            &mut last_logged,
            start + Duration::from_secs(SERVICE_FOOTPRINT_WARNING_LOG_INTERVAL_SECS - 1)
        ));
        assert!(super::service_watchdog_should_log_warning(
            &mut last_logged,
            start + Duration::from_secs(SERVICE_FOOTPRINT_WARNING_LOG_INTERVAL_SECS)
        ));
    }

    #[test]
    fn qwen3_owner_agnostic_status_reuses_matching_port() {
        let target = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();
        let workbench =
            target
                .clone()
                .with_port(18080)
                .with_state(&crate::asr_runtime::AsrServiceState {
                    host: "127.0.0.1".to_string(),
                    port: 18080,
                    model: "Qwen3-ASR-0.6B".to_string(),
                    language: "chinese".to_string(),
                    home: default_home(),
                    pid: None,
                    managed_by: "speech_workbench".to_string(),
                    owner_module: Some("speech_workbench".to_string()),
                    owner_id: None,
                    started_at_ms: 1,
                });

        assert_eq!(target.port, None);
        assert_eq!(workbench.port, Some(18080));
        assert!(!same_service_owner(&target, &workbench));
        assert!(target_matches_request(&workbench, &target));
    }

    #[test]
    fn qwen3_resolved_state_preserves_owner_and_port() {
        let requested =
            target_from_query(Some("model=Qwen3-ASR-0.6B&owner_module=speech_workbench")).unwrap();
        let resolved = requested.with_state(&crate::asr_runtime::AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 18081,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            home: default_home(),
            pid: None,
            managed_by: "speech_workbench".to_string(),
            owner_module: Some("speech_workbench".to_string()),
            owner_id: None,
            started_at_ms: 1,
        });

        assert_eq!(requested.port, None);
        assert_eq!(resolved.port, Some(18081));
        assert_eq!(resolved.owner_module, "speech_workbench");
        assert!(same_service_owner(&requested, &resolved));
    }

    #[test]
    fn qwen3_dynamic_request_reuses_persisted_service_across_owners() {
        let requested = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-a",
        ))
        .unwrap();
        let persisted = requested.with_state(&crate::asr_runtime::AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 60241,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            home: default_home(),
            pid: Some(12345),
            managed_by: "directory_task".to_string(),
            owner_module: Some("directory_task".to_string()),
            owner_id: Some("task-a".to_string()),
            started_at_ms: 1,
        });
        let conflicting_task = target_from_query(Some(
            "model=Qwen3-ASR-0.6B&owner_module=directory_task&owner_id=task-b",
        ))
        .unwrap();

        assert_eq!(requested.port, None);
        assert_eq!(persisted.port, Some(60241));
        assert!(persisted_target_matches_request(&requested, &persisted));
        assert!(persisted_target_matches_request(
            &conflicting_task,
            &persisted
        ));
    }

    #[test]
    fn upload_transcribe_boundaries_are_thirty_second_windows() {
        let boundaries = plan_upload_chunk_boundaries(180_015);
        assert_eq!(
            boundaries.first(),
            Some(&(0, ASR_UPLOAD_CHUNK_DURATION_SECS))
        );
        assert!(boundaries
            .iter()
            .all(|&(offset, duration)| duration <= 30 && offset + duration <= 181));
        assert_eq!(boundaries[1].0, 28);
        assert_eq!(boundaries.last(), Some(&(168, 13)));

        assert_eq!(plan_upload_chunk_boundaries(30_000), vec![(0, 30)]);
        assert_eq!(plan_upload_chunk_boundaries(30_001), vec![(0, 30), (28, 3)]);
    }

    #[test]
    fn normalized_wav_duration_uses_sixteen_k_mono_pcm_size() {
        let wav = vec![0u8; 44 + 32_000 * 30 + 16_000];
        assert_eq!(wav_pcm_duration_ms(&wav), Some(30_500));
        assert_eq!(wav_pcm_duration_ms(&[0u8; 44]), None);
    }
}

#[cfg(test)]
mod asr_additional_tests {
    use super::*;

    fn sample_download_progress(
        total_bytes: Option<u64>,
        percent: Option<u8>,
    ) -> crate::resource_download::DownloadProgress {
        crate::resource_download::DownloadProgress {
            label: "asr".to_string(),
            url: "http://example.test/asr".to_string(),
            dest: "/tmp/asr.zip".to_string(),
            downloaded_bytes: 42,
            total_bytes,
            percent,
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 100,
            resumed: false,
            complete: false,
        }
    }

    #[test]
    fn query_param_value_decodes_percent_encoded_pairs() {
        let query = "flag=1&name=hello%20world&empty=";
        assert_eq!(
            super::query_param_value(query, "name"),
            Some("hello world".to_string())
        );
        assert_eq!(
            super::query_param_value(query, "flag"),
            Some("1".to_string())
        );
        assert_eq!(super::query_param_value(query, "missing"), None);
    }

    #[test]
    fn query_flag_enabled_recognizes_truthy_values() {
        assert!(super::query_flag_enabled("flag=1", "flag"));
        assert!(super::query_flag_enabled("flag=true", "flag"));
        assert!(super::query_flag_enabled("flag=TRUE", "flag"));
        assert!(super::query_flag_enabled("flag=yes", "flag"));
        assert!(super::query_flag_enabled("flag=on", "flag"));
        assert!(!super::query_flag_enabled("flag=0", "flag"));
        assert!(!super::query_flag_enabled("other=1", "flag"));
    }

    #[test]
    fn download_detail_formats_bytes_when_total_known() {
        let progress = sample_download_progress(Some(100), Some(25));
        let detail = super::download_detail(&progress).unwrap();
        assert_eq!(detail, "42 / 100 bytes (25%)");

        let progress = sample_download_progress(None, None);
        assert!(super::download_detail(&progress).is_none());
    }

    #[test]
    fn tokenizer_size_rejects_unknown_models() {
        let err = super::tokenizer_size("Unknown-ASR").unwrap_err();
        assert!(err.contains("unsupported model: Unknown-ASR"));
    }

    #[tokio::test]
    async fn sse_event_helpers_emit_expected_frames() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        super::send_progress(
            &tx,
            AsrStreamPayload {
                phase: "test",
                status: "running",
                progress: 10,
                message: "testing progress",
                detail: Some("detail"),
                file: Some("file.wav"),
                server_url: Some("http://127.0.0.1:18080".to_string()),
            },
        )
        .await;
        let frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(frame.starts_with("event: progress\n"));
        assert!(frame.contains("\"phase\":\"test\""));

        super::send_text(&tx, "line1\nline2").await;
        let text_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(text_frame.starts_with("event: text\n"));
        assert!(text_frame.contains("line1\\nline2"));

        super::send_asr_segment(
            &tx,
            "segment",
            AsrSegmentPayload {
                index: 1,
                start_ms: 0,
                end_ms: 1000,
                stable_start_ms: 0,
                stable_end_ms: 1000,
                text: "hello",
                delta: "hello",
                committed: "hello",
                speaker: None,
                speaker_display_name: None,
                speaker_profile_id: None,
                speaker_confidence: None,
            },
        )
        .await;
        let segment_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(segment_frame.starts_with("event: segment\n"));
        assert!(segment_frame.contains("\"index\":1"));

        super::send_error(&tx, "oops", Some("detail")).await;
        let error_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(error_frame.starts_with("event: error\n"));
        assert!(error_frame.contains("\"message\":\"oops\""));
        assert!(error_frame.contains("\"detail\":\"detail\""));

        super::send_done(&tx).await;
        let done_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(done_frame.starts_with("event: done\n"));
        assert!(done_frame.contains("\"ok\":true"));
    }
}

#[cfg(test)]
mod asr_more_tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn file_extension_extracts_suffix_or_defaults() {
        assert_eq!(file_extension("test.wav"), ".wav");
        assert_eq!(file_extension("no_extension"), ".audio");
        assert_eq!(file_extension("archive.tar.gz"), ".gz");
    }

    #[test]
    fn multipart_boundary_extracts_and_strips_quotes() {
        let ct = "multipart/form-data; boundary=abc123";
        assert_eq!(multipart_boundary(ct), Some("abc123".to_string()));

        let ct = "multipart/form-data; charset=utf-8; boundary=\"xyz\"";
        assert_eq!(multipart_boundary(ct), Some("xyz".to_string()));

        assert!(multipart_boundary("text/plain").is_none());
    }

    #[test]
    fn extract_multipart_filename_parses_filename_and_ignores_empty() {
        let headers = "form-data; name=\"file\"; filename=\"clip.wav\"";
        assert_eq!(
            extract_multipart_filename(headers),
            Some("clip.wav".to_string())
        );
        assert!(extract_multipart_filename("form-data; name=\"file\"").is_none());
    }

    #[test]
    fn find_bytes_finds_subsequence_or_none() {
        let haystack = b"abcde";
        assert_eq!(find_bytes(haystack, b"bc"), Some(1));
        assert_eq!(find_bytes(haystack, b"de"), Some(3));
        assert_eq!(find_bytes(haystack, b"xx"), None);
    }

    #[test]
    fn extract_multipart_audio_returns_error_when_file_field_missing() {
        let body = Bytes::from_static(b"--boundary\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nvalue\r\n--boundary--\r\n");
        let err = extract_multipart_audio(&body, "boundary")
            .err()
            .expect("should be error when file field missing");
        assert!(err.contains("multipart field 'file' missing"));
    }

    #[test]
    fn extract_multipart_audio_extracts_file_bytes_and_filename() {
        let body = Bytes::from_static(b"--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\nDATA\r\n--b--\r\n");
        let uploaded = extract_multipart_audio(&body, "b").expect("multipart file");
        assert_eq!(uploaded.bytes, b"DATA");
        assert_eq!(uploaded.filename, "a.wav");
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use std::fs;
    use std::io::Write as _;

    use bytes::Bytes;
    use tempfile::tempdir;

    /// Helper to build an ASR target rooted at a temp home directory.
    fn make_temp_target(model: &str) -> (AsrTarget, tempfile::TempDir) {
        let temp = tempdir().expect("tempdir");
        let target = AsrTarget {
            host: "127.0.0.1".to_string(),
            port: Some(18080),
            language: "chinese".to_string(),
            model: model.to_string(),
            home: temp.path().to_path_buf(),
            owner_module: "test_module".to_string(),
            owner_id: Some("owner-1".to_string()),
        };
        (target, temp)
    }

    #[test]
    fn target_from_query_rejects_zero_port() {
        let err = target_from_query(Some("port=0")).expect_err("port=0 must be rejected");
        assert!(err.contains("between 1 and 65535"));
    }

    #[test]
    fn target_from_query_ignores_whitespace_only_language_model_and_owner() {
        let default = target_from_query(None).unwrap();
        let target = target_from_query(Some(
            "host=localhost&port=18080&language=%20%20%20&model=%20%20%20&owner_module=%20%20%20&owner_id=%20%20%20",
        ))
        .unwrap();
        assert_eq!(target.language, default.language);
        assert_eq!(target.model, default.model);
        assert_eq!(target.owner_module, "model_management".to_string());
        assert!(target.owner_id.is_none());
    }

    #[test]
    fn validate_loopback_host_accepts_ipv6_loopback() {
        validate_loopback_host("::1").expect("::1 should be accepted");
    }

    #[test]
    fn asr_target_server_url_formats_ipv6() {
        let target = AsrTarget {
            host: "::1".to_string(),
            port: Some(12345),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            home: default_home(),
            owner_module: "test".to_string(),
            owner_id: None,
        };
        assert_eq!(target.server_url().as_deref(), Some("http://[::1]:12345"));
        assert_eq!(
            target.server_url_display(),
            "http://[::1]:12345".to_string()
        );
    }

    #[test]
    fn owner_label_includes_owner_id_when_present() {
        let mut target = AsrTarget {
            host: "127.0.0.1".to_string(),
            port: None,
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            home: default_home(),
            owner_module: "module".to_string(),
            owner_id: None,
        };
        assert_eq!(target.owner_label(), "module");
        target.owner_id = Some("id-123".to_string());
        assert_eq!(target.owner_label(), "module:id-123");
    }

    #[test]
    fn assets_installed_is_false_when_runtime_missing() {
        let (target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        assert!(!target.assets_installed());
    }

    #[test]
    fn allocate_loopback_port_returns_valid_port() {
        let port = allocate_loopback_port().expect("allocate port");
        assert!(port > 0);
    }

    #[test]
    fn tokenizer_size_known_models() {
        assert_eq!(tokenizer_size("Qwen3-ASR-0.6B").unwrap(), "0.6B");
        assert_eq!(tokenizer_size("Qwen3-ASR-1.7B").unwrap(), "1.7B");
    }

    #[test]
    fn detect_asr_release_asset_matches_platform_support() {
        let supported = asr_platform_supported();
        let result = detect_asr_release_asset();
        if supported {
            assert_eq!(result.unwrap(), "asr-macos-aarch64");
        } else {
            let msg = result.unwrap_err();
            assert!(msg.contains("only supported on Apple Silicon macOS"));
        }
    }

    #[tokio::test]
    async fn download_asr_assets_noop_when_all_assets_present() {
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        // Ensure we operate entirely inside the temp home.
        target.home = _temp.path().to_path_buf();

        fs::create_dir_all(target.install_dir()).unwrap();
        fs::write(target.install_dir().join("asr"), b"").unwrap();
        fs::write(target.install_dir().join("asr-server"), b"").unwrap();

        fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        for sample in [
            "sample1.wav",
            "sample1.txt",
            "sample2.wav",
            "sample2.txt",
            "sample3.wav",
            "sample3.txt",
        ] {
            fs::write(target.install_dir().join(sample), b"sample").unwrap();
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        download_asr_assets(&target, tx)
            .await
            .expect("no downloads");
        // No progress events are required, but channel should be closed.
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn install_release_noop_when_runtime_already_installed() {
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        target.home = _temp.path().to_path_buf();

        fs::create_dir_all(target.install_dir()).unwrap();
        fs::write(target.install_dir().join("asr"), b"binary").unwrap();
        fs::write(target.install_dir().join("asr-server"), b"binary").unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        install_release(&target, &tx)
            .await
            .expect("should return early when binaries exist");
    }

    #[tokio::test]
    async fn prepare_model_fails_when_required_file_missing() {
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        target.home = _temp.path().to_path_buf();
        fs::create_dir_all(target.model_dir()).unwrap();
        // Do not create required model files => expect error.
        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        let err = prepare_model(&target, &tx).await.expect_err("missing file");
        assert!(err.contains("missing ASR model file"));
    }

    #[tokio::test]
    async fn prepare_model_succeeds_with_model_and_tokenizer() {
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        target.home = _temp.path().to_path_buf();

        fs::create_dir_all(target.install_dir().join("tokenizers")).unwrap();
        fs::write(
            target
                .install_dir()
                .join("tokenizers")
                .join("tokenizer-0.6B.json"),
            b"{}",
        )
        .unwrap();

        fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        prepare_model(&target, &tx)
            .await
            .expect("prepare_model should succeed");
        // Drain at least one progress event.
        let _ = rx.recv().await;
        assert!(target.model_dir().join("tokenizer.json").is_file());
    }

    #[tokio::test]
    async fn verify_cli_sample_reports_missing_sample() {
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        target.home = _temp.path().to_path_buf();
        fs::create_dir_all(target.install_dir()).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        let err = verify_cli_sample(&target, &tx)
            .await
            .expect_err("sample3.wav missing should be reported");
        assert!(err.contains("missing ASR sample audio"));
    }

    #[test]
    fn extract_zip_to_dir_blocking_extracts_nested_files() {
        let temp = tempdir().unwrap();
        let zip_path = temp.path().join("asr.zip");
        let out_dir = temp.path().join("out");
        fs::create_dir_all(&out_dir).unwrap();

        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("bin/asr", options).unwrap();
            zip.write_all(b"asr-binary").unwrap();
            zip.start_file("bin/asr-server", options).unwrap();
            zip.write_all(b"asr-server-binary").unwrap();
            zip.finish().unwrap();
        }

        extract_zip_to_dir_blocking(&zip_path, &out_dir).expect("extract zip");
        assert!(out_dir.join("bin/asr").is_file());
        assert!(out_dir.join("bin/asr-server").is_file());
    }

    #[test]
    fn copy_dir_contents_blocking_copies_all_files() {
        let temp = tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::create_dir_all(&to).unwrap();
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::write(from.join("root.txt"), b"root").unwrap();
        fs::write(from.join("sub/sub.txt"), b"sub").unwrap();

        copy_dir_contents_blocking(&from, &to).expect("copy tree");
        assert_eq!(fs::read_to_string(to.join("root.txt")).unwrap(), "root");
        assert_eq!(fs::read_to_string(to.join("sub/sub.txt")).unwrap(), "sub");
    }

    #[tokio::test]
    async fn mark_asr_binaries_executable_updates_permissions() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path();
        for name in ["asr", "asr-server"] {
            fs::write(install_dir.join(name), b"#!/bin/sh\nexit 0\n").unwrap();
        }
        mark_asr_binaries_executable(install_dir)
            .await
            .expect("chmod should succeed");
    }

    #[test]
    fn download_detail_handles_missing_total() {
        let progress = DownloadProgress {
            label: "test".to_string(),
            url: "http://example.test".to_string(),
            dest: "/tmp/test".to_string(),
            downloaded_bytes: 42,
            total_bytes: None,
            percent: None,
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 100,
            resumed: false,
            complete: false,
        };
        assert!(download_detail(&progress).is_none());
    }

    #[test]
    fn summarize_command_output_formats_and_truncates() {
        let summary = summarize_command_output(b"", b"");
        assert!(summary.contains("No command output"));

        let long = "x".repeat(1500);
        let summary = summarize_command_output(long.as_bytes(), b"");
        assert!(summary.starts_with("..."));
        assert!(summary.len() >= 10);
    }

    #[tokio::test]
    async fn sse_response_wraps_frames_in_http_response() {
        let response = sse_response(|tx| async move {
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "test",
                    status: "running",
                    progress: 1,
                    message: "hello",
                    detail: None,
                    file: None,
                    server_url: Some("http://127.0.0.1".to_string()),
                },
            )
            .await;
            send_done(&tx).await;
        });

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("Content-Type")
                .and_then(|h| h.to_str().ok()),
            Some("text/event-stream")
        );

        let collected = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("collect body");
        let text = String::from_utf8(collected.to_bytes().to_vec()).unwrap();
        assert!(text.contains("event: progress"));
        assert!(text.contains("event: done"));
    }

    #[tokio::test]
    async fn start_managed_service_errors_on_unsupported_platform() {
        if asr_platform_supported() {
            // On supported platforms this path would start a real service; skip.
            return;
        }
        let (mut target, _temp) = make_temp_target("Qwen3-ASR-0.6B");
        target.home = _temp.path().to_path_buf();
        let err = start_managed_service(target)
            .await
            .expect_err("unsupported platform should error");
        assert!(err.message.contains("not supported"));
    }
}

#[cfg(test)]
mod asr_target_extra_tests {
    use super::*;

    #[test]
    fn same_logical_target_ignores_port_difference() {
        let base = AsrTarget {
            host: "127.0.0.1".to_string(),
            port: None,
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            home: default_home(),
            owner_module: "module".to_string(),
            owner_id: None,
        };
        let mut with_port = base.clone();
        with_port.port = Some(18080);

        assert!(same_logical_target(&base, &with_port));
        assert!(!same_target(&base, &with_port));
    }

    #[test]
    fn service_busy_response_marks_busy_and_includes_owner_labels() {
        let requested = AsrTarget {
            host: "127.0.0.1".to_string(),
            port: Some(18080),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            home: default_home(),
            owner_module: "module_a".to_string(),
            owner_id: Some("a".to_string()),
        };
        let existing = AsrTarget {
            host: "127.0.0.1".to_string(),
            port: Some(18080),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            home: default_home(),
            owner_module: "module_b".to_string(),
            owner_id: Some("b".to_string()),
        };

        let response = service_busy_response(&requested, &existing);
        assert!(is_service_busy(&response));
        let detail = response.detail.expect("detail should be present");
        assert!(detail.contains("requested owner="));
        assert!(detail.contains("module_a:a"));
        assert!(detail.contains("module_b:b"));
    }

    #[test]
    fn process_is_alive_reports_current_pid() {
        let pid = std::process::id();
        assert!(process_is_alive(pid));
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;

    use bytes::Bytes;

    #[test]
    fn query_param_value_returns_none_when_key_absent() {
        let query = "flag=1&name=alice";
        assert!(query_param_value(query, "missing").is_none());
    }

    #[test]
    fn query_param_value_falls_back_to_raw_on_invalid_percent_encoding() {
        // "%GG" is invalid percent-encoding and should be returned as-is.
        let query = "name=%GG";
        assert_eq!(query_param_value(query, "name"), Some("%GG".to_string()));
    }

    #[test]
    fn query_flag_enabled_is_false_when_value_unrecognized() {
        assert!(!query_flag_enabled("flag=yeah", "flag"));
        assert!(!query_flag_enabled("other=1", "flag"));
    }

    #[test]
    fn query_flag_enabled_is_case_sensitive_for_key() {
        // "Flag" should not match key "flag".
        assert!(!query_flag_enabled("Flag=1", "flag"));
    }

    #[test]
    fn validate_loopback_host_accepts_localhost_and_ipv4() {
        validate_loopback_host("localhost").unwrap();
        validate_loopback_host("127.0.0.1").unwrap();
    }

    #[test]
    fn validate_loopback_host_rejects_non_loopback_variants() {
        assert!(validate_loopback_host("192.168.0.1").is_err());
        assert!(validate_loopback_host("example.com").is_err());
    }

    #[test]
    fn asr_target_server_url_is_none_when_port_missing() {
        let target = target_from_query(None).unwrap();
        assert!(target.port.is_none());
        assert!(target.server_url().is_none());
        assert_eq!(
            target.server_url_display(),
            "dynamic port (managed by Bifrost)".to_string()
        );
    }

    #[test]
    fn asr_target_with_port_sets_port_without_changing_host() {
        let target = target_from_query(Some("host=127.0.0.1")).unwrap();
        let with_port = target.with_port(18080);
        assert_eq!(with_port.host, "127.0.0.1");
        assert_eq!(with_port.port, Some(18080));
        assert_eq!(
            with_port.server_url().as_deref(),
            Some("http://127.0.0.1:18080")
        );
    }

    #[test]
    fn required_model_files_unknown_model_defaults_to_config_only() {
        let files = required_model_files("Unknown-Model");
        assert_eq!(files, &["config.json"]);
    }

    #[test]
    fn wav_pcm_duration_ms_returns_none_for_header_only_or_short() {
        assert_eq!(wav_pcm_duration_ms(&[0u8; 10]), None);
        assert_eq!(wav_pcm_duration_ms(&[0u8; 44]), None);
    }

    #[test]
    fn plan_upload_chunk_boundaries_clamps_zero_duration_to_single_second() {
        let boundaries = plan_upload_chunk_boundaries(0);
        assert_eq!(boundaries, vec![(0, 1)]);
    }

    #[test]
    fn plan_upload_chunk_boundaries_handles_short_clip_less_than_window() {
        let boundaries = plan_upload_chunk_boundaries(1_500);
        // 1500ms rounds up to 2 seconds.
        assert_eq!(boundaries, vec![(0, 2)]);
    }

    #[test]
    fn file_extension_handles_hidden_files_and_no_extension() {
        assert_eq!(file_extension(".hidden"), ".audio");
        assert_eq!(file_extension("no_extension"), ".audio");
    }

    #[test]
    fn multipart_boundary_returns_none_when_missing() {
        assert!(multipart_boundary("multipart/form-data; charset=utf-8").is_none());
    }

    #[test]
    fn extract_multipart_filename_ignores_whitespace_only_value() {
        let headers = "form-data; name=\"file\"; filename=\"   \"";
        assert!(extract_multipart_filename(headers).is_none());
    }

    #[test]
    fn find_bytes_detects_prefix_match_and_returns_zero_index() {
        let haystack = b"abcdef";
        assert_eq!(find_bytes(haystack, b"ab"), Some(0));
        assert_eq!(find_bytes(haystack, b"cd"), Some(2));
    }

    #[test]
    fn detect_asr_release_asset_error_mentions_platform_on_unsupported() {
        if asr_platform_supported() {
            // On supported platforms this returns Ok; skip the error-path assertion.
            assert_eq!(detect_asr_release_asset().unwrap(), "asr-macos-aarch64");
            return;
        }
        let err = detect_asr_release_asset().unwrap_err();
        assert!(err.contains("Qwen3-ASR local runtime is only supported"));
    }

    #[test]
    fn tokenizer_size_known_models_still_supported() {
        assert_eq!(tokenizer_size("Qwen3-ASR-0.6B").unwrap(), "0.6B");
        assert_eq!(tokenizer_size("Qwen3-ASR-1.7B").unwrap(), "1.7B");
    }

    #[test]
    fn command_succeeds_returns_false_for_missing_binary() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ok = rt.block_on(async { command_succeeds("this-binary-should-not-exist", &[]).await });
        assert!(!ok);
    }

    #[tokio::test]
    async fn summarize_command_output_trims_and_uses_tail_for_long_text() {
        let long_stdout = "x".repeat(2000);
        let summary = summarize_command_output(long_stdout.as_bytes(), b"stderr");
        assert!(summary.len() <= 1203); // 1200 chars plus leading "..." at most.
        assert!(summary.starts_with("..."));
        assert!(summary.contains("stderr"));
    }

    #[test]
    fn download_detail_handles_zero_total_bytes() {
        let progress = DownloadProgress {
            label: "test".to_string(),
            url: "http://example.test".to_string(),
            dest: "/tmp/test".to_string(),
            downloaded_bytes: 0,
            total_bytes: Some(0),
            percent: Some(0),
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 0,
            resumed: false,
            complete: false,
        };
        let detail = download_detail(&progress).unwrap();
        assert_eq!(detail, "0 / 0 bytes (0%)");
    }

    #[tokio::test]
    async fn sse_helpers_handle_empty_text_and_detail_none() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        send_error(&tx, "", None).await;
        let error_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(error_frame.starts_with("event: error\n"));
        assert!(error_frame.contains("\"message\":\"\""));
        assert!(!error_frame.contains("detail"));

        send_done(&tx).await;
        let done_frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(done_frame.starts_with("event: done\n"));
    }
}

#[cfg(test)]
mod coverage_boost_v3 {
    use super::*;

    use bytes::Bytes;
    use tempfile::TempDir;

    use crate::resource_download::DownloadProgress;
    use crate::test_support::TestAdminState;

    #[tokio::test]
    async fn prepare_audio_for_asr_errors_when_boundary_missing() {
        let body = Bytes::from_static(b"dummy");
        let err = prepare_audio_for_asr("multipart/form-data", &body)
            .await
            .expect_err("missing boundary must error");
        assert!(err.contains("multipart boundary missing"));
    }

    #[tokio::test]
    async fn prepare_audio_for_asr_errors_when_file_field_missing() {
        let body = Bytes::from_static(
            b"--b\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nDATA\r\n--b--\r\n",
        );
        let err = prepare_audio_for_asr("multipart/form-data; boundary=b", &body)
            .await
            .expect_err("missing file field must error");
        assert!(err.contains("multipart field 'file' missing"));
    }

    #[test]
    fn append_service_watchdog_log_creates_and_appends_lines() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("watchdog.log");

        append_service_watchdog_log(&log_path, "first\n".to_string());
        append_service_watchdog_log(&log_path, "second\n".to_string());

        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("first"));
        assert!(contents.contains("second"));
    }

    #[test]
    fn asr_download_requests_returns_empty_when_all_assets_present() {
        let temp = TempDir::new().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B")).unwrap();
        target.home = temp.path().to_path_buf();

        std::fs::create_dir_all(target.install_dir()).unwrap();
        std::fs::write(target.install_dir().join("asr"), b"bin").unwrap();
        std::fs::write(target.install_dir().join("asr-server"), b"bin").unwrap();

        std::fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            std::fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        for sample in [
            "sample1.wav",
            "sample1.txt",
            "sample2.wav",
            "sample2.txt",
            "sample3.wav",
            "sample3.txt",
        ] {
            std::fs::write(target.install_dir().join(sample), b"s").unwrap();
        }

        let requests = asr_download_requests(&target).unwrap();
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn prepare_model_errors_when_tokenizer_source_missing() {
        let temp = TempDir::new().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B&language=chinese")).unwrap();
        target.home = temp.path().to_path_buf();

        std::fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            std::fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        std::fs::create_dir_all(target.install_dir().join("tokenizers")).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        let err = prepare_model(&target, &tx)
            .await
            .expect_err("missing tokenizer source should error");
        assert!(err.contains("copy tokenizer"));
    }

    #[tokio::test]
    async fn test_admin_state_builder_can_be_used_in_asr_tests() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        // Basic sanity: values_storage and traffic_db_store are wired for handlers that
        // rely on bifrost_storage::data_dir() and related components.
        assert!(state.values_storage.is_some());
        assert!(state.traffic_db_store.is_some());
    }

    // --- Preflight and initializer helpers -----------------------------------

    #[tokio::test]
    async fn run_preflight_behaves_according_to_platform_support() {
        let target = target_from_query(None).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        let result = run_preflight(&target, &tx).await;
        drop(tx);

        if asr_platform_supported() {
            result.expect("preflight should succeed on supported platforms");
            let frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
            assert!(frame.contains("event: progress"));
            assert!(frame.contains("\"phase\":\"preflight\""));
        } else {
            let err = result.expect_err("preflight should fail on unsupported platforms");
            assert!(err.contains("Qwen3-ASR local runtime is only supported"));
            assert!(rx.recv().await.is_none());
        }
    }

    #[tokio::test]
    async fn run_initializer_silent_pub_errors_on_unsupported_platform() {
        if asr_platform_supported() {
            // On supported platforms this would try to perform real initialization; skip.
            return;
        }
        let target = target_from_query(None).unwrap();
        let err = run_initializer_silent_pub(target)
            .await
            .expect_err("unsupported platform should error");
        assert!(err.contains("Qwen3-ASR local runtime is only supported"));
    }

    // --- Download request planning -------------------------------------------

    #[test]
    fn asr_download_requests_only_include_sample_files_when_only_samples_missing() {
        let temp = TempDir::new().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B&language=chinese")).unwrap();
        target.home = temp.path().to_path_buf();

        std::fs::create_dir_all(target.install_dir()).unwrap();
        std::fs::write(target.install_dir().join("asr"), b"bin").unwrap();
        std::fs::write(target.install_dir().join("asr-server"), b"bin").unwrap();

        std::fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            std::fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        let requests = asr_download_requests(&target).unwrap();
        assert_eq!(requests.len(), 6);
        assert!(requests
            .iter()
            .all(|r| r.label.starts_with("sample") && r.url.contains(ASR_SAMPLE_BASE_URL)));
    }

    #[test]
    fn asr_download_requests_skips_samples_that_already_exist() {
        let temp = TempDir::new().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B")).unwrap();
        target.home = temp.path().to_path_buf();

        std::fs::create_dir_all(target.install_dir()).unwrap();
        std::fs::write(target.install_dir().join("asr"), b"bin").unwrap();
        std::fs::write(target.install_dir().join("asr-server"), b"bin").unwrap();

        std::fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            std::fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        for sample in ["sample1.wav", "sample2.wav", "sample3.wav"] {
            std::fs::write(target.install_dir().join(sample), b"s").unwrap();
        }

        let requests = asr_download_requests(&target).unwrap();
        let labels: Vec<_> = requests.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels.len(), 3);
        assert!(labels.contains(&"sample1.txt"));
        assert!(labels.contains(&"sample2.txt"));
        assert!(labels.contains(&"sample3.txt"));
    }

    #[test]
    fn asr_download_requests_use_sample_base_url_for_samples() {
        let temp = TempDir::new().unwrap();
        let mut target = target_from_query(Some("model=Qwen3-ASR-0.6B")).unwrap();
        target.home = temp.path().to_path_buf();

        std::fs::create_dir_all(target.install_dir()).unwrap();
        std::fs::write(target.install_dir().join("asr"), b"bin").unwrap();
        std::fs::write(target.install_dir().join("asr-server"), b"bin").unwrap();

        std::fs::create_dir_all(target.model_dir()).unwrap();
        for file in required_model_files(&target.model) {
            std::fs::write(target.model_dir().join(file), b"model").unwrap();
        }

        let requests = asr_download_requests(&target).unwrap();
        assert!(requests
            .iter()
            .filter(|r| r.label.starts_with("sample"))
            .all(|r| r.url.contains(ASR_SAMPLE_BASE_URL)));
    }

    // --- Download progress SSE helpers ---------------------------------------

    #[test]
    fn download_detail_formats_without_percent_uses_zero_percent() {
        let progress = DownloadProgress {
            label: "test".to_string(),
            url: "http://example.test".to_string(),
            dest: "/tmp/test".to_string(),
            downloaded_bytes: 50,
            total_bytes: Some(100),
            percent: None,
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 0,
            resumed: false,
            complete: false,
        };
        let detail = download_detail(&progress).unwrap();
        assert_eq!(detail, "50 / 100 bytes (0%)");
    }

    #[tokio::test]
    async fn send_download_progress_emits_running_event_for_fresh_download() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        let progress = DownloadProgress {
            label: "model.zip".to_string(),
            url: "http://example.test/model.zip".to_string(),
            dest: "/tmp/model.zip".to_string(),
            downloaded_bytes: 10,
            total_bytes: Some(100),
            percent: Some(10),
            bytes_per_second: Some(5),
            eta_seconds: Some(18),
            elapsed_ms: 1000,
            resumed: false,
            complete: false,
        };

        send_download_progress(&tx, "http://127.0.0.1:18080", progress).await;
        let frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(frame.starts_with("event: progress\n"));
        assert!(frame.contains("\"status\":\"running\""));
        assert!(frame.contains("\"file\":\"model.zip\""));
        assert!(frame.contains("\"downloaded_bytes\":10"));
    }

    #[tokio::test]
    async fn send_download_progress_emits_running_event_for_resumed_download() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        let progress = DownloadProgress {
            label: "model.zip".to_string(),
            url: "http://example.test/model.zip".to_string(),
            dest: "/tmp/model.zip".to_string(),
            downloaded_bytes: 60,
            total_bytes: Some(100),
            percent: Some(60),
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 2000,
            resumed: true,
            complete: false,
        };

        send_download_progress(&tx, "http://127.0.0.1:18080", progress).await;
        let frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(frame.contains("Resuming ASR resource download."));
        assert!(frame.contains("\"resumed\":true"));
    }

    #[tokio::test]
    async fn send_download_progress_emits_done_event_for_completed_download() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        let progress = DownloadProgress {
            label: "model.zip".to_string(),
            url: "http://example.test/model.zip".to_string(),
            dest: "/tmp/model.zip".to_string(),
            downloaded_bytes: 100,
            total_bytes: Some(100),
            percent: Some(100),
            bytes_per_second: None,
            eta_seconds: None,
            elapsed_ms: 3000,
            resumed: false,
            complete: true,
        };

        send_download_progress(&tx, "http://127.0.0.1:18080", progress).await;
        let frame = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(frame.contains("\"status\":\"done\""));
        assert!(frame.contains("Downloaded ASR resource."));
        assert!(frame.contains("\"complete\":true"));
    }

    // --- Audio helpers -------------------------------------------------------

    #[test]
    fn wav_pcm_duration_ms_handles_large_wav_buffer() {
        let seconds = 120u64;
        let pcm_bytes = (seconds * 32_000) as usize;
        let wav = vec![0u8; 44 + pcm_bytes];
        assert_eq!(wav_pcm_duration_ms(&wav), Some(seconds * 1000));
    }

    #[test]
    fn plan_upload_chunk_boundaries_outputs_single_chunk_for_short_audio() {
        let boundaries = plan_upload_chunk_boundaries(500);
        assert_eq!(boundaries, vec![(0, 1)]);
    }

    #[test]
    fn plan_upload_chunk_boundaries_generates_overlapping_chunks_for_long_audio() {
        let boundaries = plan_upload_chunk_boundaries(90_000);
        assert!(boundaries.len() >= 3);
        for window in boundaries.windows(2) {
            assert!(window[1].0 > window[0].0);
        }
        assert!(boundaries
            .iter()
            .all(|(offset, duration)| offset + duration <= 91));
    }

    // --- Query helpers -------------------------------------------------------

    #[test]
    fn target_from_query_parses_full_query_into_asr_target() {
        let target = target_from_query(Some(
            "host=localhost&port=18080&language=english&model=Qwen3-ASR-1.7B&owner_module=module&owner_id=owner",
        ))
        .unwrap();
        assert_eq!(target.host, "localhost");
        assert_eq!(target.port, Some(18080));
        assert_eq!(target.language, "english");
        assert_eq!(target.model, "Qwen3-ASR-1.7B");
        assert_eq!(target.owner_module, "module");
        assert_eq!(target.owner_id.as_deref(), Some("owner"));
    }

    #[test]
    fn target_from_query_errors_on_invalid_query_syntax() {
        let err = target_from_query(Some("host=127.0.0.1&port=not-a-number")).unwrap_err();
        assert!(err.starts_with("invalid ASR query:"));
    }

    #[test]
    fn query_param_value_returns_none_for_empty_query() {
        assert!(query_param_value("", "key").is_none());
    }

    #[test]
    fn query_flag_enabled_is_false_for_empty_value() {
        assert!(!query_flag_enabled("flag=", "flag"));
    }

    #[test]
    fn validate_loopback_host_rejects_empty_hostname() {
        assert!(validate_loopback_host("").is_err());
    }

    // --- Misc byte and filename helpers -------------------------------------

    #[test]
    fn file_extension_preserves_case_for_uppercase_extensions() {
        assert_eq!(file_extension("TEST.WAV"), ".WAV");
    }

    #[test]
    fn find_bytes_returns_none_when_needle_longer_than_haystack() {
        assert_eq!(find_bytes(b"abc", b"abcd"), None);
    }

    // --- Tokenizer helpers ---------------------------------------------------

    #[test]
    fn required_model_files_for_qwen3_0_6b_match_expected_set() {
        let files = required_model_files("Qwen3-ASR-0.6B");
        assert_eq!(files, &["config.json", "model.safetensors"]);
    }

    #[test]
    fn required_model_files_for_qwen3_1_7b_include_sharded_files() {
        let files = required_model_files("Qwen3-ASR-1.7B");
        assert!(files.contains(&"model-00001-of-00002.safetensors"));
        assert!(files.contains(&"model-00002-of-00002.safetensors"));
    }

    #[test]
    fn tokenizer_size_error_message_includes_model_name() {
        let err = tokenizer_size("Unknown-Model").unwrap_err();
        assert!(err.contains("unsupported model: Unknown-Model"));
    }

    #[test]
    fn tokenizer_size_ok_for_known_models_in_v3() {
        assert_eq!(tokenizer_size("Qwen3-ASR-0.6B").unwrap(), "0.6B");
        assert_eq!(tokenizer_size("Qwen3-ASR-1.7B").unwrap(), "1.7B");
    }
}
