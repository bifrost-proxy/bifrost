use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;
use uuid::Uuid;

use crate::asr_runtime::{
    clear_service_state, fixed_asr_home, install_dir, model_dir, now_ms, read_service_state,
    stop_pid, write_service_state, AsrServiceState, DEFAULT_ASR_HOST, DEFAULT_ASR_LANGUAGE,
    DEFAULT_ASR_MODEL,
};
use crate::handlers::asr_jobs::handle_asr_tasks;
use crate::handlers::asr_streaming::{
    append_transcript_delta, build_stream_windows, call_asr_text_endpoint, dedupe_increment,
    extract_wav_segment, normalize_asr_text, parse_wav_pcm_i16, stream_options_from_query,
};
use crate::handlers::asr_ws::handle_asr_ws_upgrade;
use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};
use crate::resource_download::{download_with_resume, DownloadProgress, DownloadRequest};

const SERVICE_START_TIMEOUT_SECS: u64 = 180;
const MAX_ASR_UPLOAD_BYTES: usize = 512 * 1024 * 1024;
const ASR_RELEASE_REPO: &str = "second-state/qwen3_asr_rs";
const ASR_SAMPLE_BASE_URL: &str =
    "https://raw.githubusercontent.com/second-state/qwen3_asr_rs/main/test_audio";

static MANAGED_SERVICE: Lazy<Mutex<Option<ManagedAsrService>>> = Lazy::new(|| Mutex::new(None));
static ASR_INIT_TASK: Lazy<Mutex<Option<AsrInitTask>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone, Deserialize)]
struct AsrQuery {
    host: Option<String>,
    port: Option<u16>,
    language: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AsrTarget {
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) language: String,
    pub(crate) model: String,
    pub(crate) home: PathBuf,
}

struct ManagedAsrService {
    target: AsrTarget,
    child: Child,
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
    message: String,
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
}

#[derive(Debug, Serialize)]
pub(crate) struct AsrErrorPayload<'a> {
    pub(crate) message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<&'a str>,
}

pub async fn handle_asr(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    if path.starts_with("/api/asr/tasks") {
        return handle_asr_tasks(req, path).await;
    }

    match (req.method(), path) {
        (&Method::GET, "/api/asr/status") => handle_status(req).await,
        (&Method::GET, "/api/asr/init-stream") => handle_init_stream(req).await,
        (&Method::POST, "/api/asr/service/start") => handle_service_start(req).await,
        (&Method::POST, "/api/asr/service/stop") => handle_service_stop(req).await,
        (&Method::POST, "/api/asr/transcribe-stream") => handle_transcribe_stream(req).await,
        (&Method::GET, "/api/asr/transcribe-ws") => handle_asr_ws_upgrade(req).await,
        (&Method::GET, _) | (&Method::POST, _) => {
            error_response(StatusCode::NOT_FOUND, "ASR endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

async fn handle_status(req: Request<Incoming>) -> Response<BoxBody> {
    let requested_target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let target = resolve_managed_target(requested_target).await;

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
            message: reason,
        });
    }

    let installed = target.assets_installed();
    let ready = probe_asr_health(&target).await.is_ok();
    let managed = managed_service_matches(&target).await;
    let ffmpeg_available = command_succeeds("ffmpeg", &["-version"]).await;
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
        server_url: target.server_url_display(),
        install_dir: target.install_dir().display().to_string(),
        model_dir: target.model_dir().display().to_string(),
        model: target.model,
        language: target.language,
        managed,
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

    let response = match start_managed_service(target).await {
        Ok(response) | Err(response) => response,
    };
    json_response(&response)
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
                    && state.language == target.language
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

async fn handle_transcribe_stream(req: Request<Incoming>) -> Response<BoxBody> {
    let requested_target = match target_from_query(req.uri().query()) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let stream_options = match stream_options_from_query(req.uri().query()) {
        Ok(options) => options,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
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
                    "Failed to create ASR streaming temp directory.",
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

        let audio = match parse_wav_pcm_i16(&wav_bytes) {
            Ok(audio) => audio,
            Err(error) => {
                send_error(&tx, "Failed to inspect normalized WAV audio.", Some(&error)).await;
                return;
            }
        };
        let windows = build_stream_windows(&audio, stream_options);
        if windows.is_empty() {
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "done",
                    status: "done",
                    progress: 100,
                    message: "Uploaded audio did not contain enough decodable samples.",
                    detail: Some("empty audio"),
                    file: None,
                    server_url: Some(target.server_url_display()),
                },
            )
            .await;
            send_text(&tx, "").await;
            send_done(&tx).await;
            return;
        }

        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "stream",
                status: "running",
                progress: 60,
                message: "Streaming transcription in 2 second windows with overlap context.",
                detail: Some(&format!(
                    "windows: {}; window_ms: {}; overlap_ms: {}",
                    windows.len(),
                    stream_options.window_ms,
                    stream_options.overlap_ms
                )),
                file: Some("upload.wav"),
                server_url: Some(target.server_url_display()),
            },
        )
        .await;

        let mut committed = String::new();
        let mut any_model_error = None::<String>;
        for window in windows.iter() {
            let segment_path = tmp_dir
                .path()
                .join(format!("segment-{:04}.wav", window.index));
            if let Err(error) = extract_wav_segment(&source_wav, &segment_path, window).await {
                send_error(&tx, "Failed to slice ASR streaming window.", Some(&error)).await;
                return;
            }

            match call_asr_text_endpoint(&server_url, &target.language, &segment_path).await {
                Ok(text) => {
                    let text = normalize_asr_text(&text);
                    let delta = dedupe_increment(&committed, &text);
                    send_asr_segment(
                        &tx,
                        "partial",
                        AsrSegmentPayload {
                            index: window.index,
                            start_ms: window.start_ms,
                            end_ms: window.end_ms,
                            stable_start_ms: window.stable_start_ms,
                            stable_end_ms: window.stable_end_ms,
                            text: &text,
                            delta: &delta,
                            committed: &committed,
                        },
                    )
                    .await;

                    if !delta.is_empty() {
                        append_transcript_delta(&mut committed, &delta);
                    }
                    send_asr_segment(
                        &tx,
                        "final",
                        AsrSegmentPayload {
                            index: window.index,
                            start_ms: window.start_ms,
                            end_ms: window.end_ms,
                            stable_start_ms: window.stable_start_ms,
                            stable_end_ms: window.stable_end_ms,
                            text: &text,
                            delta: &delta,
                            committed: &committed,
                        },
                    )
                    .await;
                }
                Err(error) => {
                    any_model_error = Some(error.clone());
                    send_error(
                        &tx,
                        "ASR model temporarily failed for a streaming window.",
                        Some(&format!("window {}: {error}", window.index)),
                    )
                    .await;
                }
            }

            let progress =
                60u8 + (((window.index + 1) as f32 / windows.len() as f32) * 35.0).round() as u8;
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "stream",
                    status: "running",
                    progress: progress.min(95),
                    message: "Processed ASR streaming window.",
                    detail: Some(&format!(
                        "window {}: {}-{} ms, stable {}-{} ms",
                        window.index,
                        window.start_ms,
                        window.end_ms,
                        window.stable_start_ms,
                        window.stable_end_ms
                    )),
                    file: segment_path.file_name().and_then(|name| name.to_str()),
                    server_url: Some(target.server_url_display()),
                },
            )
            .await;
            let _ = std::fs::remove_file(segment_path);
        }

        if committed.is_empty() && any_model_error.is_some() {
            send_error(
                &tx,
                "ASR streaming transcription did not produce text.",
                any_model_error.as_deref(),
            )
            .await;
            return;
        }

        send_progress(
            &tx,
            AsrStreamPayload {
                phase: "transcribe",
                status: "done",
                progress: 98,
                message: "ASR streaming transcription produced stable text.",
                detail: None,
                file: None,
                server_url: Some(target.server_url_display()),
            },
        )
        .await;
        send_text(&tx, committed.trim()).await;
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
        stop_any_managed_service().await;
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

    stop_any_managed_service().await;

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

    let mut child = Command::new(target.install_dir().join("asr-server"))
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
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| AsrServiceResponse {
            ready: false,
            managed: false,
            server_url: target.server_url_display(),
            message: "Failed to start Qwen3-ASR model service.".to_string(),
            detail: Some(error.to_string()),
        })?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(SERVICE_START_TIMEOUT_SECS);
    loop {
        if probe_asr_health(&target).await.is_ok() {
            let _ = write_service_state(
                &bifrost_storage::data_dir(),
                &AsrServiceState {
                    host: target.host.clone(),
                    port: target.port.expect("healthy ASR target must have a port"),
                    model: target.model.clone(),
                    language: target.language.clone(),
                    home: target.home.clone(),
                    pid: child.id(),
                    managed_by: "webui".to_string(),
                    started_at_ms: now_ms(),
                },
            );
            *MANAGED_SERVICE.lock().await = Some(ManagedAsrService {
                target: target.clone(),
                child,
            });
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
                && state.language == target.language
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
    if state.host == target.host
        && state.model == target.model
        && state.language == target.language
        && state.home == target.home
    {
        let persisted = target.with_port(state.port);
        if probe_asr_health(&persisted).await.is_ok() {
            return persisted;
        }
    }
    target
}

pub(crate) async fn stop_any_managed_service() {
    if let Some(mut managed) = MANAGED_SERVICE.lock().await.take() {
        let _ = managed.child.kill().await;
    }
    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if let Some(pid) = state.pid {
            let _ = stop_pid(pid);
        }
        let _ = clear_service_state(&bifrost_storage::data_dir());
    }
}

fn same_target(left: &AsrTarget, right: &AsrTarget) -> bool {
    left.host == right.host
        && left.port == right.port
        && left.model == right.model
        && left.language == right.language
        && left.home == right.home
}

fn same_logical_target(left: &AsrTarget, right: &AsrTarget) -> bool {
    left.host == right.host
        && left.model == right.model
        && left.language == right.language
        && left.home == right.home
}

fn target_matches_request(managed: &AsrTarget, requested: &AsrTarget) -> bool {
    same_logical_target(managed, requested)
        && requested
            .port
            .map(|requested_port| managed.port == Some(requested_port))
            .unwrap_or(true)
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

    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("build downloader client: {error}"))?;
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
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("asr-macos-aarch64"),
        (os, arch) => Err(format!(
            "Qwen3-ASR local runtime is only supported on Apple Silicon macOS; current platform is {os}-{arch}"
        )),
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
    let output = Command::new(target.install_dir().join("asr"))
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

async fn send_asr_segment(
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
    })
}

pub(crate) async fn resolve_asr_target(query: Option<&str>) -> Result<AsrTarget, String> {
    Ok(resolve_managed_target(target_from_query(query)?).await)
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

    pub(crate) fn install_dir(&self) -> PathBuf {
        install_dir(&self.home)
    }

    pub(crate) fn model_dir(&self) -> PathBuf {
        model_dir(&self.home, &self.model)
    }

    fn assets_installed(&self) -> bool {
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
    use super::{asr_download_requests, default_home, target_from_query, validate_loopback_host};

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
}
