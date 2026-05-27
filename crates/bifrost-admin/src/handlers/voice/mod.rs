use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{error, warn};
use uuid::Uuid;

use super::{error_response, json_response, json_response_with_status, BoxBody};
use crate::handlers::asr::{run_initializer_silent_pub, target_from_query, AsrTarget};
use crate::handlers::asr_streaming::{dedupe_increment, normalize_asr_text};
use crate::handlers::voice_stateful::{
    start_stateful_voice_session, StatefulVoiceConfig, StatefulVoiceResult, StatefulVoiceSession,
    STATEFUL_PROVIDER_ID,
};
use crate::handlers::websocket::generate_accept_key;
use crate::state::SharedAdminState;

mod audio;
mod sources;
mod vocabulary;
mod wake;

use audio::{
    is_voice_speech_chunk, validate_voice_audio_chunk, voice_audio_chunk_duration_ms,
    VoiceAudioConfig, VoiceRuntimeTuning, VoiceTranscriptState,
};
use sources::voice_status_response;
pub use sources::{discover_voice_sources, VoiceSource, VoiceSourceStatus};
use vocabulary::VoiceVocabularyImportRequest;
pub use vocabulary::{
    apply_voice_vocabulary, load_voice_vocabulary, save_voice_vocabulary, VoiceVocabulary,
    VoiceVocabularyTerm,
};

const MAX_VOICE_WS_TEXT_BYTES: usize = 32 * 1024;
const MAX_VOICE_WS_AUDIO_CHUNK_BYTES: usize = 512 * 1024;
const DEFAULT_VOICE_STREAM_CHUNK_SEC: f32 = 0.5;
const VOICE_SAMPLE_RATE: u32 = 16_000;
const VOICE_CHANNELS: u16 = 1;
const VOICE_AUDIO_FORMAT: &str = "pcm_s16le";
const DEFAULT_VOICE_MODEL: &str = "Qwen3-ASR-0.6B";
const VOICE_SILENCE_COMMIT_MS: u64 = 500;
const VOICE_WORKER_IDLE_UNLOAD_MS: u64 = 30_000;
const VOICE_MAX_UTTERANCE_MS: u64 = 30_000;
const VOICE_WS_IDLE_TIMEOUT_MS: u64 = 30_000;
const VOICE_SILENCE_RMS_THRESHOLD: f32 = 0.008;

#[derive(Debug, Clone, Serialize)]
struct VoiceSourcesResponse {
    platform: String,
    sources: Vec<VoiceSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceAsrProvider {
    Qwen3Stateful,
}

impl VoiceAsrProvider {
    fn from_query(query: &str) -> Result<Self, String> {
        match parse_query_value(query, "provider")
            .unwrap_or_else(|| STATEFUL_PROVIDER_ID.to_string())
            .as_str()
        {
            STATEFUL_PROVIDER_ID => Ok(Self::Qwen3Stateful),
            other => Err(format!("unsupported voice ASR provider: {other}")),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Qwen3Stateful => STATEFUL_PROVIDER_ID,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceWsClientMessage {
    Start {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        sample_rate: Option<u32>,
        #[serde(default)]
        channels: Option<u16>,
        #[serde(default)]
        format: Option<String>,
    },
    Audio {
        data: String,
        #[serde(default)]
        sequence: Option<u64>,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    Flush,
    Finish,
    Cancel,
}

#[derive(Debug, Serialize)]
struct VoiceWsEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_start_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_end_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured_at_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emitted_at_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

pub async fn handle_voice(
    req: Request<Incoming>,
    _state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::GET, "/api/voice/sources") => json_response(&VoiceSourcesResponse {
            platform: std::env::consts::OS.to_string(),
            sources: discover_voice_sources(),
        }),
        (&Method::GET, "/api/voice/status") => json_response(&voice_status_response()),
        (&Method::GET, "/api/voice/vocabulary") => match load_voice_vocabulary() {
            Ok(vocabulary) => json_response(&vocabulary),
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
        },
        (&Method::PUT, "/api/voice/vocabulary") => put_vocabulary(req).await,
        (&Method::POST, "/api/voice/sessions") => create_session_response(),
        (&Method::GET, "/api/voice/listen-ws") => handle_voice_ws_upgrade(req).await,
        _ if path.starts_with("/api/voice/wake") => wake::handle_voice_wake(req, path).await,
        _ => error_response(StatusCode::NOT_FOUND, "Voice API endpoint not found"),
    }
}

async fn put_vocabulary(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("read voice vocabulary request body: {error}"),
            )
        }
    };
    let request = if body.is_empty() {
        VoiceVocabularyImportRequest { terms: Vec::new() }
    } else {
        match serde_json::from_slice::<VoiceVocabularyImportRequest>(&body) {
            Ok(request) => request,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("invalid voice vocabulary JSON: {error}"),
                )
            }
        }
    };
    let vocabulary = VoiceVocabulary {
        version: 1,
        terms: request.terms,
    };
    match save_voice_vocabulary(&vocabulary) {
        Ok(()) => json_response(&vocabulary),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn create_session_response() -> Response<BoxBody> {
    let body = serde_json::json!({
        "session_id": Uuid::new_v4().to_string(),
        "local_only": true,
        "provider": STATEFUL_PROVIDER_ID,
        "status": "created"
    });
    json_response_with_status(StatusCode::CREATED, &body)
}

async fn handle_voice_ws_upgrade(req: Request<Incoming>) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid Voice WebSocket upgrade header",
        );
    }

    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header"),
    };
    let accept_key = generate_accept_key(&ws_key);
    let query = req.uri().query().unwrap_or("").to_string();

    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                error!("Voice WebSocket upgrade failed: {}", error);
                return;
            }
        };
        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        handle_voice_ws_connection(ws_stream, &query).await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

async fn handle_voice_ws_connection<S>(ws_stream: WebSocketStream<S>, query: &str)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, mut receiver) = ws_stream.split();
    let session_id = Uuid::new_v4().to_string();
    let mock_text = parse_query_value(query, "mock_text").unwrap_or_default();
    let source = parse_query_value(query, "source").unwrap_or_else(|| "web_mic".to_string());
    let tuning = VoiceRuntimeTuning::from_query(query);
    let chunk_size_sec = voice_stateful_chunk_size_sec(query);
    let language = parse_query_value(query, "language")
        .unwrap_or_else(|| crate::asr_runtime::DEFAULT_ASR_LANGUAGE.to_string());
    let provider = match VoiceAsrProvider::from_query(query) {
        Ok(provider) => provider,
        Err(error) => {
            let _ = send_voice_error(&mut sender, &error, None).await;
            return;
        }
    };
    let vocabulary = load_voice_vocabulary().unwrap_or_default();
    let mut received_audio_bytes = 0usize;
    let mut transcript_state = VoiceTranscriptState::new(tuning);
    let mut stateful_event_index = 0u64;
    let started_at = Instant::now();
    let mut audio_started_at = started_at;
    let mut stateful_session = None::<StatefulVoiceSession>;
    let mut audio_config = VoiceAudioConfig::default();
    // After worker startup completes, audio frames captured before this threshold
    // are stale (buffered during the blocking startup) and should be discarded.
    let mut stale_audio_before_ms: Option<u128> = None;

    if send_voice_event(
        &mut sender,
        VoiceWsEvent {
            event_type: "connected",
            session_id: Some(&session_id),
            source: Some(&source),
            text: None,
            raw_text: None,
            delta: None,
            committed: None,
            window_start_ms: None,
            window_end_ms: None,
            window_index: None,
            captured_at_ms: None,
            emitted_at_ms: None,
            inference_ms: None,
            message: Some("Voice input session is local-only."),
            detail: Some(provider.id()),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        let next_message = if stateful_session.is_some() {
            match tokio::time::timeout(
                Duration::from_millis(tuning.ws_idle_timeout_ms),
                receiver.next(),
            )
            .await
            {
                Ok(message) => message,
                Err(_) => {
                    if !transcript_state.partial.trim().is_empty() {
                        match commit_stateful_voice_utterance(
                            &mut sender,
                            &mut stateful_session,
                            &mut transcript_state,
                            &StatefulVoiceTranscriptionContext {
                                vocabulary: &vocabulary,
                                source: &source,
                                session_id: &session_id,
                                started_at: audio_started_at,
                            },
                            VoiceCommitOptions {
                                captured_at_ms: audio_started_at.elapsed().as_millis(),
                                window_index: stateful_event_index,
                                kind: VoiceCommitKind::Stable,
                                reason: "idle_timeout",
                            },
                        )
                        .await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    session_id = %session_id,
                                    "voice stateful ASR idle timeout commit failed",
                                );
                            }
                        }
                    }
                    // Use shutdown() to wait for the process to fully exit and release memory,
                    // avoiding overlap with a new worker spawn.
                    if let Some(session) = stateful_session.take() {
                        session.shutdown().await;
                    }
                    let detail = format!("idle_timeout_ms={}", tuning.ws_idle_timeout_ms);
                    let _ = send_voice_event(
                        &mut sender,
                        VoiceWsEvent {
                            event_type: "worker_idle_unloaded",
                            session_id: Some(&session_id),
                            source: Some(&source),
                            text: None,
                            raw_text: None,
                            delta: None,
                            committed: Some(&transcript_state.committed),
                            window_start_ms: None,
                            window_end_ms: None,
                            window_index: None,
                            captured_at_ms: None,
                            emitted_at_ms: Some(audio_started_at.elapsed().as_millis()),
                            inference_ms: None,
                            message: Some("Voice worker unloaded after idle timeout."),
                            detail: Some(&detail),
                        },
                    )
                    .await;
                    let _ = send_voice_done(&mut sender).await;
                    let _ = sender.close().await;
                    break;
                }
            }
        } else {
            receiver.next().await
        };
        let Some(message) = next_message else {
            break;
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                warn!("Voice WebSocket read failed: {}", error);
                break;
            }
        };
        match message {
            Message::Text(text) => {
                let parsed = match parse_voice_ws_client_message(&text) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = send_voice_error(&mut sender, &error, None).await;
                        continue;
                    }
                };
                match parsed {
                    VoiceWsClientMessage::Start {
                        source: requested_source,
                        sample_rate,
                        channels,
                        format,
                    } => {
                        if let Some(requested_source) = requested_source {
                            if requested_source != source {
                                let detail = format!("requested_source={requested_source}");
                                let _ = send_voice_error(
                                    &mut sender,
                                    "source mismatch for voice session",
                                    Some(&detail),
                                )
                                .await;
                                break;
                            }
                        }
                        match VoiceAudioConfig::from_start(sample_rate, channels, format.as_deref())
                        {
                            Ok(config) => audio_config = config,
                            Err(error) => {
                                let _ = send_voice_error(&mut sender, &error, None).await;
                                break;
                            }
                        }
                        if mock_text.is_empty() && stateful_session.is_none() {
                            match start_voice_stateful_session(query, chunk_size_sec, &language)
                                .await
                            {
                                Ok(session) => stateful_session = Some(session),
                                Err(error) => {
                                    let _ = send_voice_error(
                                        &mut sender,
                                        "voice stateful ASR is not ready",
                                        Some(&error),
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                        audio_started_at = Instant::now();
                        let detail = format!(
                            "sample_rate={}; channels={}; format={}",
                            audio_config.sample_rate, audio_config.channels, VOICE_AUDIO_FORMAT
                        );
                        let _ = send_voice_event(
                            &mut sender,
                            VoiceWsEvent {
                                event_type: "source_ready",
                                session_id: Some(&session_id),
                                source: Some(&source),
                                text: None,
                                raw_text: None,
                                delta: None,
                                committed: None,
                                window_start_ms: None,
                                window_end_ms: None,
                                window_index: None,
                                captured_at_ms: None,
                                emitted_at_ms: None,
                                inference_ms: None,
                                message: Some("Voice input source started."),
                                detail: Some(&detail),
                            },
                        )
                        .await;
                    }
                    VoiceWsClientMessage::Audio {
                        data,
                        sequence,
                        duration_ms,
                    } => match decode_voice_audio_payload(&data) {
                        Ok(bytes) => {
                            if let Err(error) = validate_voice_audio_chunk(&bytes, audio_config) {
                                let _ = send_voice_error(&mut sender, &error, None).await;
                                break;
                            }
                            received_audio_bytes = received_audio_bytes.saturating_add(bytes.len());
                            if !mock_text.is_empty() {
                                let mock_committed =
                                    apply_voice_vocabulary(&mock_text, &vocabulary);
                                let detail = format!(
                                    "sequence={}; received_audio_bytes={}; duration_ms={}",
                                    sequence.unwrap_or(0),
                                    received_audio_bytes,
                                    duration_ms.unwrap_or(0)
                                );
                                let _ = send_voice_event(
                                    &mut sender,
                                    VoiceWsEvent {
                                        event_type: "asr_partial",
                                        session_id: Some(&session_id),
                                        source: Some(&source),
                                        text: Some(&mock_text),
                                        raw_text: Some(&mock_text),
                                        delta: Some(&mock_committed),
                                        committed: Some(&mock_committed),
                                        window_start_ms: Some(0),
                                        window_end_ms: duration_ms.or(Some(1000)),
                                        window_index: Some(0),
                                        captured_at_ms: Some(
                                            audio_started_at.elapsed().as_millis(),
                                        ),
                                        emitted_at_ms: Some(audio_started_at.elapsed().as_millis()),
                                        inference_ms: Some(0),
                                        message: None,
                                        detail: Some(&detail),
                                    },
                                )
                                .await;
                            } else {
                                let captured_at_ms = audio_started_at.elapsed().as_millis();
                                let chunk_ms = voice_audio_chunk_duration_ms(&bytes, audio_config);
                                let is_speech = match is_voice_speech_chunk(&bytes) {
                                    Ok(is_speech) => is_speech,
                                    Err(error) => {
                                        let _ = send_voice_error(&mut sender, &error, None).await;
                                        break;
                                    }
                                };
                                transcript_state.mark_audio_activity(
                                    captured_at_ms,
                                    chunk_ms,
                                    is_speech,
                                );
                                if !is_speech
                                    && stateful_session.is_none()
                                    && transcript_state.partial.trim().is_empty()
                                {
                                    continue;
                                }
                                if stateful_session.is_none() {
                                    match start_voice_stateful_session(
                                        query,
                                        chunk_size_sec,
                                        &language,
                                    )
                                    .await
                                    {
                                        Ok(session) => {
                                            // Mark all audio captured before this point as stale;
                                            // these frames were buffered during the blocking worker
                                            // startup and would cause a processing backlog.
                                            stale_audio_before_ms =
                                                Some(audio_started_at.elapsed().as_millis());
                                            tracing::debug!(
                                                session_id = %session_id,
                                                stale_threshold_ms = ?stale_audio_before_ms,
                                                "voice worker started, will discard stale audio"
                                            );
                                            stateful_session = Some(session);
                                        }
                                        Err(error) => {
                                            let _ = send_voice_error(
                                                &mut sender,
                                                "voice stateful ASR is not ready",
                                                Some(&error),
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                }
                                if let Some(session) = stateful_session.as_mut() {
                                    // Discard audio frames that were buffered during worker startup
                                    if let Some(threshold) = stale_audio_before_ms {
                                        if captured_at_ms < threshold {
                                            continue;
                                        }
                                        // First fresh frame: clear the threshold
                                        stale_audio_before_ms = None;
                                        tracing::debug!(
                                            session_id = %session_id,
                                            captured_at_ms = captured_at_ms,
                                            "voice: first fresh audio frame after worker startup"
                                        );
                                    }
                                    match session.feed_pcm16(&bytes).await {
                                        Ok(Some(result)) => {
                                            if let Err(error) = emit_stateful_voice_partial(
                                                &mut sender,
                                                &mut transcript_state,
                                                result,
                                                captured_at_ms,
                                                stateful_event_index,
                                                &StatefulVoiceTranscriptionContext {
                                                    vocabulary: &vocabulary,
                                                    source: &source,
                                                    session_id: &session_id,
                                                    started_at: audio_started_at,
                                                },
                                            )
                                            .await
                                            {
                                                let _ = send_voice_error(
                                                    &mut sender,
                                                    "voice stateful ASR feed result failed",
                                                    Some(&error),
                                                )
                                                .await;
                                            }
                                            stateful_event_index += 1;
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            let _ = send_voice_error(
                                                &mut sender,
                                                "voice stateful ASR feed failed",
                                                Some(&error),
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                }
                                if transcript_state.should_commit_for_silence() {
                                    if let Err(error) = commit_stateful_voice_utterance(
                                        &mut sender,
                                        &mut stateful_session,
                                        &mut transcript_state,
                                        &StatefulVoiceTranscriptionContext {
                                            vocabulary: &vocabulary,
                                            source: &source,
                                            session_id: &session_id,
                                            started_at: audio_started_at,
                                        },
                                        VoiceCommitOptions {
                                            captured_at_ms,
                                            window_index: stateful_event_index,
                                            kind: VoiceCommitKind::Stable,
                                            reason: "silence",
                                        },
                                    )
                                    .await
                                    {
                                        let _ = send_voice_error(
                                            &mut sender,
                                            "voice stateful ASR silence commit failed",
                                            Some(&error),
                                        )
                                        .await;
                                        break;
                                    }
                                } else if transcript_state
                                    .should_commit_for_max_duration(captured_at_ms)
                                {
                                    if let Err(error) = commit_stateful_voice_utterance(
                                        &mut sender,
                                        &mut stateful_session,
                                        &mut transcript_state,
                                        &StatefulVoiceTranscriptionContext {
                                            vocabulary: &vocabulary,
                                            source: &source,
                                            session_id: &session_id,
                                            started_at: audio_started_at,
                                        },
                                        VoiceCommitOptions {
                                            captured_at_ms,
                                            window_index: stateful_event_index,
                                            kind: VoiceCommitKind::Stable,
                                            reason: "max_utterance_duration",
                                        },
                                    )
                                    .await
                                    {
                                        let _ = send_voice_error(
                                            &mut sender,
                                            "voice stateful ASR duration commit failed",
                                            Some(&error),
                                        )
                                        .await;
                                        break;
                                    }
                                } else if transcript_state.should_unload_idle_worker() {
                                    // Keep worker alive for the lifetime of the WS connection
                                    // to avoid respawn latency. The 30s auto-reset in the worker
                                    // prevents performance degradation from KV cache growth.
                                    transcript_state.silence_ms = 0;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = send_voice_error(&mut sender, &error, None).await;
                        }
                    },
                    VoiceWsClientMessage::Flush => {
                        let detail = format!("received_audio_bytes={received_audio_bytes}");
                        if !transcript_state.partial.trim().is_empty() {
                            let (delta, raw_partial) = transcript_state.commit_partial();
                            let _ = send_voice_event(
                                &mut sender,
                                VoiceWsEvent {
                                    event_type: "asr_stable_delta",
                                    session_id: Some(&session_id),
                                    source: Some(&source),
                                    text: Some(&transcript_state.committed),
                                    raw_text: Some(&raw_partial),
                                    delta: Some(&delta),
                                    committed: Some(&transcript_state.committed),
                                    window_start_ms: Some(0),
                                    window_end_ms: Some(1000),
                                    window_index: None,
                                    captured_at_ms: None,
                                    emitted_at_ms: Some(audio_started_at.elapsed().as_millis()),
                                    inference_ms: None,
                                    message: None,
                                    detail: Some(&detail),
                                },
                            )
                            .await;
                            continue;
                        }
                        let _ = send_voice_event(
                            &mut sender,
                            VoiceWsEvent {
                                event_type: "asr_stable_delta",
                                session_id: Some(&session_id),
                                source: Some(&source),
                                text: Some(&transcript_state.committed),
                                raw_text: Some(&mock_text),
                                delta: Some(""),
                                committed: Some(&transcript_state.committed),
                                window_start_ms: Some(0),
                                window_end_ms: Some(1000),
                                window_index: None,
                                captured_at_ms: None,
                                emitted_at_ms: Some(audio_started_at.elapsed().as_millis()),
                                inference_ms: None,
                                message: None,
                                detail: Some(&detail),
                            },
                        )
                        .await;
                    }
                    VoiceWsClientMessage::Finish => {
                        if mock_text.is_empty() {
                            let captured_at_ms = audio_started_at.elapsed().as_millis();
                            if let Err(error) = commit_stateful_voice_utterance(
                                &mut sender,
                                &mut stateful_session,
                                &mut transcript_state,
                                &StatefulVoiceTranscriptionContext {
                                    vocabulary: &vocabulary,
                                    source: &source,
                                    session_id: &session_id,
                                    started_at: audio_started_at,
                                },
                                VoiceCommitOptions {
                                    captured_at_ms,
                                    window_index: stateful_event_index,
                                    kind: VoiceCommitKind::Final,
                                    reason: "finish",
                                },
                            )
                            .await
                            {
                                let _ = send_voice_error(
                                    &mut sender,
                                    "voice stateful ASR finish failed",
                                    Some(&error),
                                )
                                .await;
                                break;
                            }
                        } else {
                            let raw = &mock_text;
                            let refined = apply_voice_vocabulary(raw, &vocabulary);
                            let _ = send_voice_event(
                                &mut sender,
                                VoiceWsEvent {
                                    event_type: "asr_final_utterance",
                                    session_id: Some(&session_id),
                                    source: Some(&source),
                                    text: Some(&refined),
                                    raw_text: Some(raw),
                                    delta: Some(&refined),
                                    committed: Some(&refined),
                                    window_start_ms: Some(0),
                                    window_end_ms: Some(1000),
                                    window_index: None,
                                    captured_at_ms: None,
                                    emitted_at_ms: Some(audio_started_at.elapsed().as_millis()),
                                    inference_ms: None,
                                    message: None,
                                    detail: None,
                                },
                            )
                            .await;
                        }
                        let _ = send_voice_done(&mut sender).await;
                        let _ = sender.close().await;
                        break;
                    }
                    VoiceWsClientMessage::Cancel => {
                        let _ = send_voice_done(&mut sender).await;
                        let _ = sender.close().await;
                        break;
                    }
                }
            }
            Message::Binary(bytes) => {
                if bytes.len() > MAX_VOICE_WS_AUDIO_CHUNK_BYTES {
                    let _ = send_voice_error(
                        &mut sender,
                        "voice binary audio frame is too large",
                        None,
                    )
                    .await;
                    break;
                }
                if let Err(error) = validate_voice_audio_chunk(&bytes, audio_config) {
                    let _ = send_voice_error(&mut sender, &error, None).await;
                    break;
                }
                received_audio_bytes = received_audio_bytes.saturating_add(bytes.len());
                if mock_text.is_empty() {
                    let captured_at_ms = audio_started_at.elapsed().as_millis();
                    let chunk_ms = voice_audio_chunk_duration_ms(&bytes, audio_config);
                    let is_speech = match is_voice_speech_chunk(&bytes) {
                        Ok(is_speech) => is_speech,
                        Err(error) => {
                            let _ = send_voice_error(&mut sender, &error, None).await;
                            break;
                        }
                    };
                    transcript_state.mark_audio_activity(captured_at_ms, chunk_ms, is_speech);
                    if !is_speech
                        && stateful_session.is_none()
                        && transcript_state.partial.trim().is_empty()
                    {
                        continue;
                    }
                    if stateful_session.is_none() {
                        match start_voice_stateful_session(query, chunk_size_sec, &language).await {
                            Ok(session) => {
                                stale_audio_before_ms =
                                    Some(audio_started_at.elapsed().as_millis());
                                tracing::debug!(
                                    session_id = %session_id,
                                    stale_threshold_ms = ?stale_audio_before_ms,
                                    "voice worker started (binary path), will discard stale audio"
                                );
                                stateful_session = Some(session);
                            }
                            Err(error) => {
                                let _ = send_voice_error(
                                    &mut sender,
                                    "voice stateful ASR is not ready",
                                    Some(&error),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    if let Some(session) = stateful_session.as_mut() {
                        // Discard audio frames that were buffered during worker startup
                        if let Some(threshold) = stale_audio_before_ms {
                            if captured_at_ms < threshold {
                                continue;
                            }
                            stale_audio_before_ms = None;
                            tracing::debug!(
                                session_id = %session_id,
                                captured_at_ms = captured_at_ms,
                                "voice: first fresh audio frame after worker startup (binary path)"
                            );
                        }
                        match session.feed_pcm16(&bytes).await {
                            Ok(Some(result)) => {
                                if let Err(error) = emit_stateful_voice_partial(
                                    &mut sender,
                                    &mut transcript_state,
                                    result,
                                    captured_at_ms,
                                    stateful_event_index,
                                    &StatefulVoiceTranscriptionContext {
                                        vocabulary: &vocabulary,
                                        source: &source,
                                        session_id: &session_id,
                                        started_at: audio_started_at,
                                    },
                                )
                                .await
                                {
                                    let _ = send_voice_error(
                                        &mut sender,
                                        "voice stateful ASR feed result failed",
                                        Some(&error),
                                    )
                                    .await;
                                }
                                stateful_event_index += 1;
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let _ = send_voice_error(
                                    &mut sender,
                                    "voice stateful ASR feed failed",
                                    Some(&error),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    if transcript_state.should_commit_for_silence() {
                        if let Err(error) = commit_stateful_voice_utterance(
                            &mut sender,
                            &mut stateful_session,
                            &mut transcript_state,
                            &StatefulVoiceTranscriptionContext {
                                vocabulary: &vocabulary,
                                source: &source,
                                session_id: &session_id,
                                started_at: audio_started_at,
                            },
                            VoiceCommitOptions {
                                captured_at_ms,
                                window_index: stateful_event_index,
                                kind: VoiceCommitKind::Stable,
                                reason: "silence",
                            },
                        )
                        .await
                        {
                            let _ = send_voice_error(
                                &mut sender,
                                "voice stateful ASR silence commit failed",
                                Some(&error),
                            )
                            .await;
                            break;
                        }
                    } else if transcript_state.should_commit_for_max_duration(captured_at_ms) {
                        if let Err(error) = commit_stateful_voice_utterance(
                            &mut sender,
                            &mut stateful_session,
                            &mut transcript_state,
                            &StatefulVoiceTranscriptionContext {
                                vocabulary: &vocabulary,
                                source: &source,
                                session_id: &session_id,
                                started_at: audio_started_at,
                            },
                            VoiceCommitOptions {
                                captured_at_ms,
                                window_index: stateful_event_index,
                                kind: VoiceCommitKind::Stable,
                                reason: "max_utterance_duration",
                            },
                        )
                        .await
                        {
                            let _ = send_voice_error(
                                &mut sender,
                                "voice stateful ASR duration commit failed",
                                Some(&error),
                            )
                            .await;
                            break;
                        }
                    } else if transcript_state.should_unload_idle_worker() {
                        // Keep worker alive for the lifetime of the WS connection
                        // to avoid respawn latency. The 30s auto-reset in the worker
                        // prevents performance degradation from KV cache growth.
                        transcript_state.silence_ms = 0;
                    }
                }
            }
            Message::Ping(payload) => {
                if sender.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
            Message::Frame(_) => {}
        }
    }
}

async fn start_voice_stateful_session(
    query: &str,
    chunk_size_sec: f32,
    language: &str,
) -> Result<StatefulVoiceSession, String> {
    if parse_query_bool(query, "fake_stateful_worker") && fake_stateful_worker_enabled() {
        let text = parse_query_value(query, "fake_stateful_text")
            .unwrap_or_else(|| "请打开Bifrost".to_string());
        return Ok(StatefulVoiceSession::fake(text, language.to_string()));
    }
    let target = voice_target_from_query(query)?;
    if target.model == "Qwen3-ASR-1.7B" && !stateful_17b_enabled(query) {
        return Err(
            "Qwen3 stateful streaming blocks 1.7B by default after local memory pressure; use Qwen3-ASR-0.6B, pass allow_stateful_17b=1, or set BIFROST_VOICE_ALLOW_STATEFUL_17B=1 for a controlled experiment."
                .to_string(),
        );
    }
    ensure_voice_target_ready(&target).await?;

    // Stop the standalone asr-server if it's running with the same model to avoid
    // loading the ~2GB ASR model twice (once in asr-server + once in voice worker).
    crate::handlers::asr::stop_managed_service_for_target(&target).await;

    start_stateful_voice_session(StatefulVoiceConfig {
        model: target.model.clone(),
        model_dir: target.model_dir(),
        language: language.to_string(),
        chunk_size_sec,
        initial_text: parse_query_value(query, "initial_text"),
    })
    .await
}

async fn ensure_voice_target_ready(target: &AsrTarget) -> Result<(), String> {
    if target.assets_installed() {
        return Ok(());
    }
    if target.model == DEFAULT_VOICE_MODEL {
        let mut init_target = target.clone();
        init_target.language = crate::asr_runtime::DEFAULT_ASR_LANGUAGE.to_string();
        run_initializer_silent_pub(init_target)
            .await
            .map_err(|error| {
                format!("initialize realtime voice ASR {DEFAULT_VOICE_MODEL}: {error}")
            })?;
        if target.assets_installed() {
            return Ok(());
        }
    }
    Err(format!(
        "Voice realtime ASR assets are missing for {}; initialize the model first or use the default {DEFAULT_VOICE_MODEL} realtime model",
        target.model
    ))
}

fn voice_target_from_query(query: &str) -> Result<AsrTarget, String> {
    if parse_query_value(query, "model").is_some() {
        return target_from_query(Some(query));
    }
    let query_with_model = if query.trim().is_empty() {
        format!("model={DEFAULT_VOICE_MODEL}")
    } else {
        format!("{query}&model={DEFAULT_VOICE_MODEL}")
    };
    target_from_query(Some(&query_with_model))
}

fn voice_stateful_chunk_size_sec(query: &str) -> f32 {
    parse_query_f32(query, "stateful_chunk_sec")
        .or_else(|| parse_query_u64(query, "chunk_ms").map(|value| value as f32 / 1000.0))
        .unwrap_or(DEFAULT_VOICE_STREAM_CHUNK_SEC)
        .clamp(0.5, 4.0)
}

fn stateful_17b_enabled(query: &str) -> bool {
    parse_query_bool(query, "allow_stateful_17b")
        || std::env::var("BIFROST_VOICE_ALLOW_STATEFUL_17B")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false)
}

fn fake_stateful_worker_enabled() -> bool {
    std::env::var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

struct StatefulVoiceTranscriptionContext<'a> {
    vocabulary: &'a VoiceVocabulary,
    source: &'a str,
    session_id: &'a str,
    started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceCommitKind {
    Stable,
    Final,
}

#[derive(Debug, Clone, Copy)]
struct VoiceCommitOptions<'a> {
    captured_at_ms: u128,
    window_index: u64,
    kind: VoiceCommitKind,
    reason: &'a str,
}

async fn emit_stateful_voice_partial<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    transcript_state: &mut VoiceTranscriptState,
    result: StatefulVoiceResult,
    captured_at_ms: u128,
    window_index: u64,
    context: &StatefulVoiceTranscriptionContext<'_>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let raw_text = normalize_asr_text(&result.text);
    if raw_text.is_empty() {
        return Ok(());
    }
    let refined = apply_voice_vocabulary(&raw_text, context.vocabulary);
    let delta = dedupe_increment(&transcript_state.committed, &refined);
    transcript_state.partial = refined.clone();
    let emitted_at_ms = context.started_at.elapsed().as_millis();
    let detail = format!(
        "provider={}; language={}; stable=false",
        STATEFUL_PROVIDER_ID, result.language
    );
    send_voice_event(
        sender,
        VoiceWsEvent {
            event_type: "asr_partial",
            session_id: Some(context.session_id),
            source: Some(context.source),
            text: Some(&refined),
            raw_text: Some(&raw_text),
            delta: Some(&delta),
            committed: Some(&transcript_state.committed),
            window_start_ms: None,
            window_end_ms: None,
            window_index: Some(window_index),
            captured_at_ms: Some(captured_at_ms),
            emitted_at_ms: Some(emitted_at_ms),
            inference_ms: Some(result.inference_ms),
            message: None,
            detail: Some(&detail),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn commit_stateful_voice_utterance<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    stateful_session: &mut Option<StatefulVoiceSession>,
    transcript_state: &mut VoiceTranscriptState,
    context: &StatefulVoiceTranscriptionContext<'_>,
    options: VoiceCommitOptions<'_>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if transcript_state.has_active_utterance() {
        if let Some(session) = stateful_session.as_mut() {
            // Use reset() instead of finish() to keep the worker process alive.
            // reset() finalizes the current utterance and reinits streaming state.
            match session.reset().await {
                Ok(result) => {
                    emit_stateful_voice_partial(
                        sender,
                        transcript_state,
                        result,
                        options.captured_at_ms,
                        options.window_index,
                        context,
                    )
                    .await?;
                }
                Err(error) => {
                    // reset() timed out or failed — worker is likely dead or stuck.
                    // Kill it and clear session; next audio frame will respawn a fresh worker.
                    tracing::warn!(
                        session_id = %context.session_id,
                        error = %error,
                        "voice worker reset failed, killing and will respawn on next audio",
                    );
                    if let Some(dead_session) = stateful_session.take() {
                        dead_session.shutdown().await;
                    }
                }
            }
        }
    }

    let (delta, raw_partial) = transcript_state.commit_partial();
    let emitted_at_ms = context.started_at.elapsed().as_millis();
    let detail = format!(
        "provider={}; reason={}; stable=true",
        STATEFUL_PROVIDER_ID, options.reason
    );
    let event_type = match options.kind {
        VoiceCommitKind::Stable => "asr_stable_delta",
        VoiceCommitKind::Final => "asr_final_utterance",
    };
    send_voice_event(
        sender,
        VoiceWsEvent {
            event_type,
            session_id: Some(context.session_id),
            source: Some(context.source),
            text: Some(&transcript_state.committed),
            raw_text: Some(&raw_partial),
            delta: Some(&delta),
            committed: Some(&transcript_state.committed),
            window_start_ms: None,
            window_end_ms: None,
            window_index: Some(options.window_index),
            captured_at_ms: Some(options.captured_at_ms),
            emitted_at_ms: Some(emitted_at_ms),
            inference_ms: None,
            message: None,
            detail: Some(&detail),
        },
    )
    .await
    .map_err(|error| error.to_string())
}

fn parse_query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=') {
            if name == key {
                return Some(urlencoding::decode(value).unwrap_or_default().to_string());
            }
        }
    }
    None
}

fn parse_query_u64(query: &str, key: &str) -> Option<u64> {
    parse_query_value(query, key)?.parse().ok()
}

fn parse_query_f32(query: &str, key: &str) -> Option<f32> {
    parse_query_value(query, key)?.parse().ok()
}

fn parse_query_bool(query: &str, key: &str) -> bool {
    parse_query_value(query, key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

fn parse_voice_ws_client_message(text: &str) -> Result<VoiceWsClientMessage, String> {
    if text.len() > MAX_VOICE_WS_TEXT_BYTES {
        return Err("Voice WebSocket control frame is too large".to_string());
    }
    serde_json::from_str(text).map_err(|error| format!("invalid Voice WebSocket message: {error}"))
}

fn decode_voice_audio_payload(data: &str) -> Result<Vec<u8>, String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("invalid base64 voice audio payload: {error}"))?;
    if decoded.len() > MAX_VOICE_WS_AUDIO_CHUNK_BYTES {
        return Err("voice audio chunk is too large".to_string());
    }
    Ok(decoded)
}

async fn send_voice_event<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    event: VoiceWsEvent<'_>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let text = serde_json::to_string(&event).unwrap_or_else(|_| {
        r#"{"type":"error","message":"serialize voice event failed"}"#.to_string()
    });
    sender.send(Message::Text(text.into())).await
}

async fn send_voice_error<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    message: &str,
    detail: Option<&str>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_voice_event(
        sender,
        VoiceWsEvent {
            event_type: "error",
            session_id: None,
            source: None,
            text: None,
            raw_text: None,
            delta: None,
            committed: None,
            window_start_ms: None,
            window_end_ms: None,
            window_index: None,
            captured_at_ms: None,
            emitted_at_ms: None,
            inference_ms: None,
            message: Some(message),
            detail,
        },
    )
    .await
}

async fn send_voice_done<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_voice_event(
        sender,
        VoiceWsEvent {
            event_type: "done",
            session_id: None,
            source: None,
            text: None,
            raw_text: None,
            delta: None,
            committed: None,
            window_start_ms: None,
            window_end_ms: None,
            window_index: None,
            captured_at_ms: None,
            emitted_at_ms: None,
            inference_ms: None,
            message: None,
            detail: None,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_sources_include_local_statuses() {
        let sources = discover_voice_sources();
        assert!(sources.iter().any(|source| source.id == "web_mic"));
        assert!(sources.iter().any(|source| source.id == "file:realtime"));
        assert!(sources.iter().any(|source| source.kind == "mic"));
        assert!(sources.iter().any(|source| source.kind == "system"));
        assert!(sources.iter().any(|source| source.kind == "app"));
    }

    #[test]
    fn vocabulary_rewrites_aliases_without_touching_raw_input() {
        let vocabulary = VoiceVocabulary {
            version: 1,
            terms: vec![VoiceVocabularyTerm {
                canonical: "Bifrost".to_string(),
                aliases: vec!["宽增".to_string(), "白 Frost".to_string()],
                category: Some("project".to_string()),
            }],
        };
        let raw = "请打开宽增并搜索白 Frost 的日志";
        assert_eq!(
            apply_voice_vocabulary(raw, &vocabulary),
            "请打开Bifrost并搜索Bifrost 的日志"
        );
        assert_eq!(raw, "请打开宽增并搜索白 Frost 的日志");
    }

    #[test]
    fn invalid_base64_audio_is_rejected() {
        let err = decode_voice_audio_payload("***").unwrap_err();
        assert!(err.contains("invalid base64 voice audio payload"));
    }

    #[test]
    fn voice_provider_selection_defaults_and_validates() {
        assert_eq!(
            VoiceAsrProvider::from_query("source=file").unwrap(),
            VoiceAsrProvider::Qwen3Stateful
        );
        assert_eq!(
            VoiceAsrProvider::from_query("provider=qwen3_stateful_streaming").unwrap(),
            VoiceAsrProvider::Qwen3Stateful
        );
        assert!(VoiceAsrProvider::from_query("provider=qwen3_rs_http_chunked").is_err());
        assert!(VoiceAsrProvider::from_query("provider=remote_cloud").is_err());
    }

    #[test]
    fn stateful_large_model_can_be_enabled_from_query() {
        assert!(!stateful_17b_enabled("provider=qwen3_stateful_streaming"));
        assert!(stateful_17b_enabled(
            "provider=qwen3_stateful_streaming&allow_stateful_17b=1"
        ));
    }

    #[test]
    fn voice_target_defaults_to_realtime_06b() {
        let target = voice_target_from_query("source=web_mic&language=english").unwrap();
        assert_eq!(target.model, DEFAULT_VOICE_MODEL);
        assert_eq!(target.language, "english");
    }

    #[test]
    fn voice_start_rejects_non_16k_pcm() {
        let error =
            VoiceAudioConfig::from_start(Some(48_000), Some(1), Some("pcm_s16le")).unwrap_err();
        assert!(error.contains("16000Hz PCM"));
        let error =
            VoiceAudioConfig::from_start(Some(16_000), Some(2), Some("pcm_s16le")).unwrap_err();
        assert!(error.contains("mono PCM"));
        let error =
            VoiceAudioConfig::from_start(Some(16_000), Some(1), Some("pcm_f32")).unwrap_err();
        assert!(error.contains("pcm_s16le"));
    }

    #[test]
    fn partial_commit_keeps_stable_text_monotonic() {
        let mut state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        state.partial = "hello bifrost".to_string();
        assert_eq!(state.committed, "");

        state.partial = "hello".to_string();
        assert_eq!(state.committed, "");

        state.partial = "hello bifrost voice".to_string();
        let (delta, partial) = state.commit_partial();
        assert_eq!(partial, "hello bifrost voice");
        assert_eq!(delta, "hello bifrost voice");
        assert_eq!(state.committed, "hello bifrost voice");

        state.partial = "hello bifrost voice input".to_string();
        let (delta, _) = state.commit_partial();
        assert_eq!(delta, " input");
        assert_eq!(state.committed, "hello bifrost voice input");
    }

    #[test]
    fn voice_vad_detects_silence_and_speech_boundaries() {
        let silence = vec![0u8; 16_000 * 2];
        assert!(!is_voice_speech_chunk(&silence).unwrap());
        assert_eq!(
            voice_audio_chunk_duration_ms(&silence, VoiceAudioConfig::default()),
            1000
        );

        let mut speech = Vec::new();
        for _ in 0..16_000 {
            speech.extend_from_slice(&10_000i16.to_le_bytes());
        }
        assert!(is_voice_speech_chunk(&speech).unwrap());
    }

    #[test]
    fn transcript_boundaries_use_testable_runtime_tuning() {
        let mut state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(
            "silence_commit_ms=25&worker_idle_unload_ms=50&max_utterance_ms=75",
        ));
        state.partial = "hello bifrost".to_string();
        state.mark_audio_activity(10, 10, true);
        assert!(!state.should_commit_for_max_duration(80));
        assert!(state.should_commit_for_max_duration(85));

        state.mark_audio_activity(90, 25, false);
        assert!(state.should_commit_for_silence());
        let (delta, _) = state.commit_partial();
        assert_eq!(delta, "hello bifrost");

        state.mark_audio_activity(120, 50, false);
        assert!(state.should_unload_idle_worker());
    }
}
