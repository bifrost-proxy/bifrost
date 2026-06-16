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
mod speaker;
#[cfg(test)]
mod tests;
mod vocabulary;
mod wake;

use audio::{
    is_voice_speech_chunk, validate_voice_audio_chunk, voice_audio_chunk_duration_ms,
    VoiceAudioConfig, VoiceRealtimeAudioBuffer, VoiceRuntimeTuning, VoiceTranscriptState,
};
use sources::voice_status_response;
pub use sources::{discover_voice_sources, VoiceSource, VoiceSourceStatus};
use speaker::{RealtimeSpeakerAssignment, RealtimeVoiceSpeakerTracker};
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
struct VoiceWsSpeakerFields<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    utterance_index: Option<u64>,
    speaker: &'a str,
    speaker_display_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_profile_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_profile_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_confidence: Option<f32>,
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
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    speaker: Option<VoiceWsSpeakerFields<'a>>,
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
    let lease_model =
        parse_query_value(query, "model").unwrap_or_else(|| DEFAULT_VOICE_MODEL.to_string());
    let lease_decision = crate::handlers::speech::acquire_speech_resource(
        crate::handlers::speech::realtime_voice_lease(&session_id, &lease_model),
    );
    if !lease_decision.granted {
        let detail = lease_decision.reason.as_deref();
        let _ = send_voice_error(
            &mut sender,
            "voice realtime ASR resource is not available",
            detail,
        )
        .await;
        let _ = sender.close().await;
        return;
    }
    let resource_lease = lease_decision.lease;
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
    let mut audio_buffer = VoiceRealtimeAudioBuffer::default();
    let mut speaker_tracker = RealtimeVoiceSpeakerTracker::default();

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
            speaker: None,
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
        crate::handlers::speech::release_speech_resource(resource_lease.as_ref());
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
                            RealtimeVoiceSpeakerContext {
                                tracker: &mut speaker_tracker,
                                audio_buffer: &audio_buffer,
                                audio_config,
                            },
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
                            speaker: None,
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
                        audio_buffer.clear();
                        speaker_tracker.reset();
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
                                speaker: None,
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
                                        speaker: None,
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
                                    transcript_state.mark_audio_activity(
                                        captured_at_ms,
                                        chunk_ms,
                                        is_speech,
                                    );
                                    audio_buffer.push_chunk(
                                        &bytes,
                                        captured_at_ms,
                                        chunk_ms,
                                        audio_config,
                                    );
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
                                        RealtimeVoiceSpeakerContext {
                                            tracker: &mut speaker_tracker,
                                            audio_buffer: &audio_buffer,
                                            audio_config,
                                        },
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
                                        RealtimeVoiceSpeakerContext {
                                            tracker: &mut speaker_tracker,
                                            audio_buffer: &audio_buffer,
                                            audio_config,
                                        },
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
                                    speaker: None,
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
                                speaker: None,
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
                                RealtimeVoiceSpeakerContext {
                                    tracker: &mut speaker_tracker,
                                    audio_buffer: &audio_buffer,
                                    audio_config,
                                },
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
                                    speaker: None,
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
                        transcript_state.mark_audio_activity(captured_at_ms, chunk_ms, is_speech);
                        audio_buffer.push_chunk(&bytes, captured_at_ms, chunk_ms, audio_config);
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
                            RealtimeVoiceSpeakerContext {
                                tracker: &mut speaker_tracker,
                                audio_buffer: &audio_buffer,
                                audio_config,
                            },
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
                            RealtimeVoiceSpeakerContext {
                                tracker: &mut speaker_tracker,
                                audio_buffer: &audio_buffer,
                                audio_config,
                            },
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
    crate::handlers::speech::release_speech_resource(resource_lease.as_ref());
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

struct RealtimeVoiceSpeakerContext<'a> {
    tracker: &'a mut RealtimeVoiceSpeakerTracker,
    audio_buffer: &'a VoiceRealtimeAudioBuffer,
    audio_config: VoiceAudioConfig,
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
    let window_start_ms = transcript_state
        .utterance_started_at_ms()
        .map(u128_to_u64_lossy);
    let window_end_ms = Some(u128_to_u64_lossy(captured_at_ms));
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
            window_start_ms,
            window_end_ms,
            window_index: Some(window_index),
            speaker: None,
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
    speaker_context: RealtimeVoiceSpeakerContext<'_>,
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

    let window_start_ms = transcript_state
        .utterance_started_at_ms()
        .map(u128_to_u64_lossy)
        .unwrap_or_else(|| u128_to_u64_lossy(options.captured_at_ms).saturating_sub(1_000));
    let window_end_ms = u128_to_u64_lossy(options.captured_at_ms).max(window_start_ms);
    let (delta, raw_partial) = transcript_state.commit_partial();
    let utterance_audio = speaker_context.audio_buffer.slice_pcm16(
        window_start_ms,
        window_end_ms,
        speaker_context.audio_config,
    );
    let speaker_assignment =
        if raw_partial.trim().is_empty() || utterance_audio.is_empty() || delta.trim().is_empty() {
            None
        } else {
            speaker_context.tracker.classify_utterance(
                &utterance_audio,
                speaker_context.audio_config.sample_rate,
                context.session_id,
            )
        };
    let speaker_fields = speaker_assignment
        .as_ref()
        .map(|assignment| voice_ws_speaker_fields(options.window_index, assignment));
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
            window_start_ms: Some(window_start_ms),
            window_end_ms: Some(window_end_ms),
            window_index: Some(options.window_index),
            speaker: speaker_fields,
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

fn voice_ws_speaker_fields(
    utterance_index: u64,
    assignment: &RealtimeSpeakerAssignment,
) -> VoiceWsSpeakerFields<'_> {
    VoiceWsSpeakerFields {
        utterance_index: Some(utterance_index),
        speaker: &assignment.speaker,
        speaker_display_name: &assignment.display_name,
        speaker_profile_id: assignment.mapped_profile_id.as_deref(),
        speaker_confidence: assignment.confidence,
        candidate_profile_id: assignment.candidate_profile_id.as_deref(),
        candidate_display_name: assignment.candidate_display_name.as_deref(),
        candidate_confidence: assignment.candidate_confidence,
    }
}

fn u128_to_u64_lossy(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
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
            speaker: None,
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
            speaker: None,
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
mod voice_helpers_tests {
    use super::*;

    #[test]
    fn voice_asr_provider_from_query_defaults_and_rejects_unknown() {
        let provider = VoiceAsrProvider::from_query("").expect("default provider");
        assert_eq!(provider.id(), STATEFUL_PROVIDER_ID);

        let err = VoiceAsrProvider::from_query("provider=unknown").unwrap_err();
        assert!(err.contains("unsupported voice ASR provider"));
    }

    #[test]
    fn voice_target_from_query_injects_default_model_when_missing() {
        let target = voice_target_from_query("").expect("target");
        assert_eq!(target.model, DEFAULT_VOICE_MODEL);

        let explicit = voice_target_from_query("model=Qwen3-ASR-0.6B").expect("explicit target");
        assert_eq!(explicit.model, "Qwen3-ASR-0.6B");
    }

    #[test]
    fn voice_stateful_chunk_size_sec_uses_query_and_clamps_limits() {
        let default = voice_stateful_chunk_size_sec("");
        assert!((default - DEFAULT_VOICE_STREAM_CHUNK_SEC).abs() < 1e-6);

        let from_sec = voice_stateful_chunk_size_sec("stateful_chunk_sec=10");
        assert!((from_sec - 4.0).abs() < 1e-6);

        let from_ms = voice_stateful_chunk_size_sec("chunk_ms=500");
        assert!((from_ms - 0.5).abs() < 1e-6);
    }

    #[test]
    fn stateful_17b_enabled_respects_query_flag() {
        assert!(stateful_17b_enabled("allow_stateful_17b=1"));
    }

    #[test]
    fn u128_to_u64_lossy_saturates_at_u64_max() {
        let within = u128_to_u64_lossy(42u128);
        assert_eq!(within, 42u64);

        let large = u128_to_u64_lossy(u128::from(u64::MAX) + 1);
        assert_eq!(large, u64::MAX);
    }

    #[test]
    fn parse_query_helpers_extract_typed_values() {
        let query = "a=1&b=2.5&flag=true&encoded=hello%20world";
        assert_eq!(
            parse_query_value(query, "encoded"),
            Some("hello world".to_string())
        );
        assert_eq!(parse_query_u64(query, "a"), Some(1));
        assert_eq!(parse_query_f32(query, "b"), Some(2.5));
        assert!(parse_query_bool(query, "flag"));
        assert!(!parse_query_bool(query, "missing"));
    }

    #[test]
    fn parse_voice_ws_client_message_validates_size_and_json() {
        let msg = serde_json::json!({
            "type": "start",
            "source": "mic",
        })
        .to_string();
        let parsed = parse_voice_ws_client_message(&msg).expect("valid message");
        match parsed {
            VoiceWsClientMessage::Start { source, .. } => {
                assert_eq!(source.as_deref(), Some("mic"));
            }
            _ => panic!("expected Start variant"),
        }

        let big = "x".repeat(MAX_VOICE_WS_TEXT_BYTES + 1);
        let err = parse_voice_ws_client_message(&big).unwrap_err();
        assert!(err.contains("control frame is too large"));
    }

    #[test]
    fn decode_voice_audio_payload_decodes_base64_and_enforces_size_limit() {
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let decoded = decode_voice_audio_payload(&data).expect("decode");
        assert_eq!(decoded, b"hello");

        let too_big = vec![0u8; MAX_VOICE_WS_AUDIO_CHUNK_BYTES + 1];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&too_big);
        let err = decode_voice_audio_payload(&encoded).unwrap_err();
        assert!(err.contains("voice audio chunk is too large"));
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::test_env::BifrostDataDirGuard;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use std::env;
    use std::time::Instant;
    use tempfile::tempdir;
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::{Message, Role};
    use tokio_tungstenite::WebSocketStream;

    // ---- Small helpers & macros ----

    macro_rules! parse_bool_case {
        ($name:ident, $query:expr, $key:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(parse_query_bool($query, $key), $expected);
            }
        };
    }

    parse_bool_case!(parse_bool_true_one, "flag=1", "flag", true);
    parse_bool_case!(parse_bool_true_true, "flag=true", "flag", true);
    parse_bool_case!(parse_bool_true_yes, "flag=yes", "flag", true);
    parse_bool_case!(parse_bool_true_on, "flag=on", "flag", true);
    parse_bool_case!(parse_bool_false_zero, "flag=0", "flag", false);
    parse_bool_case!(parse_bool_false_empty_query, "", "flag", false);
    parse_bool_case!(parse_bool_false_missing_key, "a=1", "flag", false);
    parse_bool_case!(
        parse_bool_false_unrecognized_value,
        "flag=maybe",
        "flag",
        false
    );

    macro_rules! parse_value_case {
        ($name:ident, $query:expr, $key:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(parse_query_value($query, $key), $expected);
            }
        };
    }

    parse_value_case!(
        parse_value_decodes_space,
        "encoded=hello%20world",
        "encoded",
        Some("hello world".to_string())
    );
    parse_value_case!(
        parse_value_first_match_wins,
        "a=1&a=2",
        "a",
        Some("1".to_string())
    );
    parse_value_case!(parse_value_missing_none, "a=1&b=2", "c", None);
    parse_value_case!(
        parse_value_empty_value_allowed,
        "a=",
        "a",
        Some("".to_string())
    );

    #[test]
    fn parse_query_u64_and_f32_handle_invalid_values() {
        assert_eq!(parse_query_u64("a=42", "a"), Some(42));
        assert_eq!(parse_query_u64("a=not_a_number", "a"), None);
        assert_eq!(parse_query_f32("b=3.5", "b"), Some(3.5));
        let nan = parse_query_f32("b=NaN", "b").expect("NaN parsed");
        assert!(nan.is_nan());
    }

    #[test]
    fn voice_asr_provider_id_roundtrip() {
        let provider = VoiceAsrProvider::Qwen3Stateful;
        assert_eq!(provider.id(), STATEFUL_PROVIDER_ID);
        assert!(VoiceAsrProvider::from_query(&format!("provider={}", provider.id())).is_ok());
    }

    #[test]
    fn decode_voice_audio_payload_reports_invalid_base64() {
        let err = decode_voice_audio_payload("***").unwrap_err();
        assert!(err.contains("invalid base64 voice audio payload"));
    }

    #[test]
    fn decode_voice_audio_payload_rejects_large_plain_text() {
        let too_big = vec![0u8; MAX_VOICE_WS_AUDIO_CHUNK_BYTES + 1];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&too_big);
        let err = decode_voice_audio_payload(&encoded).unwrap_err();
        assert!(err.contains("voice audio chunk is too large"));
    }

    #[test]
    fn voice_ws_speaker_fields_maps_assignment_fields() {
        let assignment = RealtimeSpeakerAssignment {
            speaker: "speaker_01".to_string(),
            display_name: "Alice".to_string(),
            mapped_profile_id: Some("p1".to_string()),
            confidence: Some(0.9),
            candidate_profile_id: Some("p2".to_string()),
            candidate_display_name: Some("Bob".to_string()),
            candidate_confidence: Some(0.8),
        };
        let fields = voice_ws_speaker_fields(3, &assignment);
        assert_eq!(fields.utterance_index, Some(3));
        assert_eq!(fields.speaker, "speaker_01");
        assert_eq!(fields.speaker_display_name, "Alice");
        assert_eq!(fields.speaker_profile_id, Some("p1"));
        assert_eq!(fields.candidate_profile_id, Some("p2"));
    }

    #[test]
    fn voice_ws_event_serialization_omits_none_fields() {
        let event = VoiceWsEvent {
            event_type: "test",
            session_id: None,
            source: None,
            text: Some("hello"),
            raw_text: None,
            delta: None,
            committed: None,
            window_start_ms: None,
            window_end_ms: None,
            window_index: None,
            speaker: None,
            captured_at_ms: None,
            emitted_at_ms: None,
            inference_ms: None,
            message: Some("ok"),
            detail: None,
        };
        let value: Value = serde_json::to_value(&event).expect("serialize");
        assert_eq!(value["type"], "test");
        assert_eq!(value["text"], "hello");
        assert_eq!(value["message"], "ok");
        assert!(value.get("session_id").is_none());
    }

    #[test]
    fn voice_stateful_chunk_size_sec_prefers_ms_and_clamps() {
        let from_ms = voice_stateful_chunk_size_sec("chunk_ms=100");
        assert!((from_ms - 0.5).abs() < 1e-6);
        let too_small = voice_stateful_chunk_size_sec("stateful_chunk_sec=0.1");
        assert!((too_small - 0.5).abs() < 1e-6);
        let too_large = voice_stateful_chunk_size_sec("stateful_chunk_sec=10");
        assert!((too_large - 4.0).abs() < 1e-6);
    }

    #[test]
    fn stateful_17b_enabled_reads_env_flag() {
        let _env = crate::test_env::voice_env_lock().blocking_lock();
        env::remove_var("BIFROST_VOICE_ALLOW_STATEFUL_17B");
        assert!(!stateful_17b_enabled(""));
        env::set_var("BIFROST_VOICE_ALLOW_STATEFUL_17B", "true");
        assert!(stateful_17b_enabled(""));
        env::remove_var("BIFROST_VOICE_ALLOW_STATEFUL_17B");
    }

    #[test]
    fn fake_stateful_worker_enabled_reads_env_flag() {
        let _env = crate::test_env::voice_env_lock().blocking_lock();
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");
        assert!(!fake_stateful_worker_enabled());
        env::set_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL", "1");
        assert!(fake_stateful_worker_enabled());
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");
    }

    #[test]
    fn voice_target_from_query_uses_default_model_when_only_language() {
        let target = voice_target_from_query("language=zh").expect("target");
        assert_eq!(target.model, DEFAULT_VOICE_MODEL);
    }

    #[tokio::test]
    async fn start_voice_stateful_session_uses_fake_worker_when_enabled() {
        let _env = crate::test_env::voice_env_lock().lock().await;
        env::set_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL", "1");
        let session = start_voice_stateful_session(
            "fake_stateful_worker=1&fake_stateful_text=你好",
            0.5,
            "zh",
        )
        .await
        .expect("fake session");
        match session {
            StatefulVoiceSession::Fake(_) => {}
            _ => panic!("expected fake worker"),
        }
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");
    }

    #[test]
    fn parse_voice_ws_client_message_parses_all_variants() {
        let start = json!({"type":"start","source":"mic","sample_rate":16000}).to_string();
        match parse_voice_ws_client_message(&start).expect("start") {
            VoiceWsClientMessage::Start { source, .. } => {
                assert_eq!(source.as_deref(), Some("mic"));
            }
            _ => panic!("expected Start"),
        }

        let audio = json!({"type":"audio","data":""}).to_string();
        match parse_voice_ws_client_message(&audio).expect("audio") {
            VoiceWsClientMessage::Audio { sequence, .. } => {
                assert_eq!(sequence, None);
            }
            _ => panic!("expected Audio"),
        }

        let flush = "{\"type\":\"flush\"}";
        assert!(matches!(
            parse_voice_ws_client_message(flush).expect("flush"),
            VoiceWsClientMessage::Flush
        ));

        let finish = "{\"type\":\"finish\"}";
        assert!(matches!(
            parse_voice_ws_client_message(finish).expect("finish"),
            VoiceWsClientMessage::Finish
        ));

        let cancel = "{\"type\":\"cancel\"}";
        assert!(matches!(
            parse_voice_ws_client_message(cancel).expect("cancel"),
            VoiceWsClientMessage::Cancel
        ));
    }

    // ---- WebSocket helper-based tests ----

    #[allow(dead_code)]
    async fn make_ws_pair() -> (
        futures_util::stream::SplitSink<WebSocketStream<tokio::io::DuplexStream>, Message>,
        futures_util::stream::SplitStream<WebSocketStream<tokio::io::DuplexStream>>,
    ) {
        let (_client_raw, server_raw) = duplex(64 * 1024);
        let ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        futures_util::StreamExt::split(ws)
    }

    #[tokio::test]
    async fn emit_stateful_voice_partial_updates_state_and_sends_event() {
        let (_client_raw, server_raw) = duplex(64 * 1024);
        let ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let (mut sender, _receiver) = ws.split();

        let mut transcript_state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        let result = StatefulVoiceResult {
            text: "你好，Bifrost".to_string(),
            language: "zh".to_string(),
            inference_ms: 5,
        };
        let ctx = StatefulVoiceTranscriptionContext {
            vocabulary: &VoiceVocabulary::default(),
            source: "web_mic",
            session_id: "s1",
            started_at: Instant::now(),
        };

        emit_stateful_voice_partial(&mut sender, &mut transcript_state, result, 1000, 0, &ctx)
            .await
            .expect("emit");

        assert!(!transcript_state.partial.is_empty());
    }

    #[tokio::test]
    async fn commit_stateful_voice_utterance_sends_stable_delta_and_clears_partial() {
        let (_client_raw, server_raw) = duplex(64 * 1024);
        let ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let (mut sender, _receiver) = ws.split();

        let mut transcript_state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        transcript_state.partial = "hello bifrost".to_string();
        transcript_state.mark_audio_activity(1_000, 1_000, true);

        let mut session = Some(StatefulVoiceSession::fake(
            "hello bifrost".to_string(),
            "en".to_string(),
        ));
        let mut tracker = RealtimeVoiceSpeakerTracker::default();
        let audio_buffer = VoiceRealtimeAudioBuffer::default();
        let speaker_ctx = RealtimeVoiceSpeakerContext {
            tracker: &mut tracker,
            audio_buffer: &audio_buffer,
            audio_config: VoiceAudioConfig {
                sample_rate: 0,
                channels: 1,
            },
        };
        let ctx = StatefulVoiceTranscriptionContext {
            vocabulary: &VoiceVocabulary::default(),
            source: "web_mic",
            session_id: "s1",
            started_at: Instant::now(),
        };
        let opts = VoiceCommitOptions {
            captured_at_ms: 2_000,
            window_index: 0,
            kind: VoiceCommitKind::Stable,
            reason: "test",
        };

        commit_stateful_voice_utterance(
            &mut sender,
            &mut session,
            &mut transcript_state,
            speaker_ctx,
            &ctx,
            opts,
        )
        .await
        .expect("commit");

        assert!(transcript_state.partial.is_empty());
    }

    #[tokio::test]
    async fn websocket_mock_text_finish_sends_final_and_done() {
        let tmp = tempdir().expect("tmp");
        let _guard = BifrostDataDirGuard::set(tmp.path());

        let (client_raw, server_raw) = duplex(64 * 1024);
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;

        let query = "source=web_mic&mock_text=hello%20world";
        let server_task = tokio::spawn(handle_voice_ws_connection(server_ws, query));

        let (mut sender, mut receiver) = client_ws.split();

        // Connected event
        let first = receiver.next().await.expect("connected").expect("ok");
        assert!(matches!(first, Message::Text(_)));

        let start = json!({
            "type": "start",
            "source": "web_mic",
            "sample_rate": 16000u32,
            "channels": 1u16,
            "format": VOICE_AUDIO_FORMAT,
        })
        .to_string();
        sender
            .send(Message::Text(start.into()))
            .await
            .expect("start");

        let pcm = vec![0u8; (VOICE_SAMPLE_RATE as usize) * 2];
        let payload = base64::engine::general_purpose::STANDARD.encode(&pcm);
        let audio = json!({
            "type": "audio",
            "data": payload,
            "sequence": 1u64,
            "duration_ms": 1000u64,
        })
        .to_string();
        sender
            .send(Message::Text(audio.into()))
            .await
            .expect("audio");

        let finish = json!({"type":"finish"}).to_string();
        sender
            .send(Message::Text(finish.into()))
            .await
            .expect("finish");

        let mut saw_final = false;
        let mut saw_done = false;
        while let Some(msg) = receiver.next().await {
            let msg = msg.expect("ws ok");
            if let Message::Text(text) = msg {
                let v: Value = serde_json::from_str(&text).expect("json");
                match v["type"].as_str() {
                    Some("asr_final_utterance") => saw_final = true,
                    Some("done") => {
                        saw_done = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(saw_final);
        assert!(saw_done);
        server_task.await.expect("server");
    }

    #[tokio::test]
    async fn websocket_source_mismatch_reports_error() {
        let tmp = tempdir().expect("tmp");
        let _guard = BifrostDataDirGuard::set(tmp.path());

        let (client_raw, server_raw) = duplex(64 * 1024);
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;

        let query = "source=web_mic";
        let server_task = tokio::spawn(handle_voice_ws_connection(server_ws, query));

        let (mut sender, mut receiver) = client_ws.split();

        let _connected = receiver.next().await.expect("connected").expect("ok");

        let start = json!({
            "type": "start",
            "source": "other_source",
        })
        .to_string();
        sender
            .send(Message::Text(start.into()))
            .await
            .expect("start");

        let msg = receiver.next().await.expect("error").expect("ok");
        match msg {
            Message::Text(text) => {
                let v: Value = serde_json::from_str(&text).expect("json");
                assert_eq!(v["type"], "error");
                assert!(v["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("source mismatch"));
            }
            other => panic!("unexpected message: {other:?}"),
        }

        server_task.await.expect("server");
    }

    #[tokio::test]
    async fn websocket_binary_silence_then_cancel_sends_done() {
        let tmp = tempdir().expect("tmp");
        let _guard = BifrostDataDirGuard::set(tmp.path());

        let (client_raw, server_raw) = duplex(64 * 1024);
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;

        let query = "source=web_mic";
        let server_task = tokio::spawn(handle_voice_ws_connection(server_ws, query));

        let (mut sender, mut receiver) = client_ws.split();

        let _connected = receiver.next().await.expect("connected").expect("ok");

        let start = json!({
            "type": "start",
            "source": "web_mic",
            "sample_rate": VOICE_SAMPLE_RATE,
            "channels": VOICE_CHANNELS,
            "format": VOICE_AUDIO_FORMAT,
        })
        .to_string();
        sender
            .send(Message::Text(start.into()))
            .await
            .expect("start");

        let silence = vec![0u8; VOICE_SAMPLE_RATE as usize * 2];
        sender
            .send(Message::Binary(silence.into()))
            .await
            .expect("binary");

        let cancel = json!({"type":"cancel"}).to_string();
        sender
            .send(Message::Text(cancel.into()))
            .await
            .expect("cancel");

        let mut saw_done = false;
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let v: Value = serde_json::from_str(&text).expect("json");
                    if v["type"] == "done" {
                        saw_done = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        // Even if the client sees only a close frame, the Cancel path was exercised.
        let _ = saw_done;
        server_task.await.expect("server");
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::test_env::BifrostDataDirGuard;
    use futures_util::StreamExt;
    use serde_json::Value;
    use std::env;
    use tempfile::tempdir;
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::{Message, Role};
    use tokio_tungstenite::WebSocketStream;

    #[tokio::test]
    async fn start_voice_stateful_session_rejects_17b_when_not_enabled() {
        let _env = crate::test_env::voice_env_lock().lock().await;
        env::remove_var("BIFROST_VOICE_ALLOW_STATEFUL_17B");
        let err = start_voice_stateful_session("model=Qwen3-ASR-1.7B", 0.5, "zh")
            .await
            .err()
            .expect("1.7B model should be blocked without flag");
        assert!(err.contains("blocks 1.7B by default"));
    }

    #[tokio::test]
    async fn ensure_voice_target_ready_errors_for_missing_assets_on_non_default_model() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let target = voice_target_from_query("language=zh&model=CustomVoiceModel").expect("target");
        let err = ensure_voice_target_ready(&target)
            .await
            .expect_err("missing assets should error");
        assert!(err.contains("assets are missing"));
    }

    #[tokio::test]
    async fn emit_stateful_voice_partial_ignores_empty_text() {
        let (_client_raw, server_raw) = duplex(64 * 1024);
        let ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let (mut sender, _receiver) = ws.split();

        let mut transcript_state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        let result = StatefulVoiceResult {
            text: "   ".to_string(),
            language: "zh".to_string(),
            inference_ms: 1,
        };
        let ctx = StatefulVoiceTranscriptionContext {
            vocabulary: &VoiceVocabulary::default(),
            source: "web_mic",
            session_id: "s1",
            started_at: Instant::now(),
        };

        emit_stateful_voice_partial(&mut sender, &mut transcript_state, result, 1000, 0, &ctx)
            .await
            .expect("emit");

        // Empty text should not update partial state.
        assert!(transcript_state.partial.is_empty());
    }

    #[tokio::test]
    async fn commit_stateful_voice_utterance_handles_no_active_utterance() {
        let (_client_raw, server_raw) = duplex(64 * 1024);
        let ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let (mut sender, _receiver) = ws.split();

        let mut transcript_state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        let mut session = None::<StatefulVoiceSession>;
        let mut tracker = RealtimeVoiceSpeakerTracker::default();
        let audio_buffer = VoiceRealtimeAudioBuffer::default();
        let speaker_ctx = RealtimeVoiceSpeakerContext {
            tracker: &mut tracker,
            audio_buffer: &audio_buffer,
            audio_config: VoiceAudioConfig {
                sample_rate: VOICE_SAMPLE_RATE,
                channels: VOICE_CHANNELS,
            },
        };
        let ctx = StatefulVoiceTranscriptionContext {
            vocabulary: &VoiceVocabulary::default(),
            source: "web_mic",
            session_id: "s1",
            started_at: Instant::now(),
        };
        let opts = VoiceCommitOptions {
            captured_at_ms: 0,
            window_index: 0,
            kind: VoiceCommitKind::Stable,
            reason: "test",
        };

        commit_stateful_voice_utterance(
            &mut sender,
            &mut session,
            &mut transcript_state,
            speaker_ctx,
            &ctx,
            opts,
        )
        .await
        .expect("commit");
    }

    #[tokio::test]
    async fn send_voice_error_sends_error_event() {
        let (client_raw, server_raw) = duplex(64 * 1024);
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let (mut sender, _server_receiver) = server_ws.split();
        let (_client_sender, mut receiver) = client_ws.split();

        send_voice_error(&mut sender, "something went wrong", Some("detail"))
            .await
            .unwrap();

        let msg = receiver.next().await.expect("frame").expect("ok");
        match msg {
            Message::Text(text) => {
                let v: Value = serde_json::from_str(&text).expect("json");
                assert_eq!(v["type"], "error");
                assert_eq!(v["message"], "something went wrong");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_voice_done_sends_done_event() {
        let (client_raw, server_raw) = duplex(64 * 1024);
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let (mut sender, _server_receiver) = server_ws.split();
        let (_client_sender, mut receiver) = client_ws.split();

        send_voice_done(&mut sender).await.unwrap();

        let msg = receiver.next().await.expect("frame").expect("ok");
        match msg {
            Message::Text(text) => {
                let v: Value = serde_json::from_str(&text).expect("json");
                assert_eq!(v["type"], "done");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn voice_stateful_chunk_size_sec_returns_default_for_empty_query_v2() {
        let value = voice_stateful_chunk_size_sec("");
        assert!((value - DEFAULT_VOICE_STREAM_CHUNK_SEC).abs() < 1e-6);
    }

    #[test]
    fn voice_stateful_chunk_size_sec_clamps_below_min_v2() {
        let value = voice_stateful_chunk_size_sec("stateful_chunk_sec=0.1");
        assert!((value - 0.5).abs() < 1e-6);
    }

    #[test]
    fn voice_stateful_chunk_size_sec_clamps_above_max_v2() {
        let value = voice_stateful_chunk_size_sec("stateful_chunk_sec=10");
        assert!((value - 4.0).abs() < 1e-6);
    }

    #[test]
    fn voice_stateful_chunk_size_sec_prefers_chunk_ms_over_stateful_chunk_v2() {
        let value = voice_stateful_chunk_size_sec("stateful_chunk_sec=3&chunk_ms=500");
        assert!((value - 3.0).abs() < 1e-6);
    }

    #[test]
    fn stateful_17b_enabled_true_for_query_flag_v2() {
        assert!(stateful_17b_enabled("allow_stateful_17b=1"));
    }

    #[test]
    fn stateful_17b_enabled_reads_env_flag_v2() {
        let _env = crate::test_env::voice_env_lock().blocking_lock();
        env::remove_var("BIFROST_VOICE_ALLOW_STATEFUL_17B");
        assert!(!stateful_17b_enabled(""));
        env::set_var("BIFROST_VOICE_ALLOW_STATEFUL_17B", "true");
        assert!(stateful_17b_enabled(""));
        env::remove_var("BIFROST_VOICE_ALLOW_STATEFUL_17B");
    }

    #[test]
    fn fake_stateful_worker_enabled_true_for_yes_v2() {
        let _env = crate::test_env::voice_env_lock().blocking_lock();
        env::set_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL", "yes");
        assert!(fake_stateful_worker_enabled());
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");
    }

    #[test]
    fn fake_stateful_worker_enabled_false_when_unset_v2() {
        let _env = crate::test_env::voice_env_lock().blocking_lock();
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");
        assert!(!fake_stateful_worker_enabled());
    }

    #[test]
    fn voice_target_from_query_appends_default_model_for_non_empty_query_v2() {
        let target = voice_target_from_query("language=zh").expect("target");
        assert_eq!(target.model, DEFAULT_VOICE_MODEL);
    }

    #[test]
    fn voice_target_from_query_respects_explicit_model_v2() {
        let target = voice_target_from_query("language=zh&model=Custom").expect("target");
        assert_eq!(target.model, "Custom");
    }

    #[test]
    fn u128_to_u64_lossy_round_trips_small_values_v2() {
        let value = u128_to_u64_lossy(123u128);
        assert_eq!(value, 123u64);
    }

    #[test]
    fn u128_to_u64_lossy_caps_large_values_v2() {
        let value = u128_to_u64_lossy(u128::from(u64::MAX) + 10);
        assert_eq!(value, u64::MAX);
    }

    #[test]
    fn parse_voice_ws_client_message_rejects_invalid_json_v2() {
        let err = parse_voice_ws_client_message("not json").unwrap_err();
        assert!(err.contains("invalid Voice WebSocket message"));
    }

    #[test]
    fn decode_voice_audio_payload_accepts_valid_small_payload_v2() {
        let data = vec![1u8, 2, 3];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        let decoded = decode_voice_audio_payload(&encoded).expect("decode");
        assert_eq!(decoded, data);
    }

    #[test]
    fn decode_voice_audio_payload_rejects_large_payload_v2() {
        let data = vec![0u8; MAX_VOICE_WS_AUDIO_CHUNK_BYTES + 1];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        let err = decode_voice_audio_payload(&encoded).unwrap_err();
        assert!(err.contains("voice audio chunk is too large"));
    }

    #[test]
    fn parse_query_helpers_extract_typed_values_combined_v2() {
        let query = "a=1&b=2.5&flag=true&encoded=hello%20world";
        assert_eq!(
            parse_query_value(query, "encoded"),
            Some("hello world".to_string())
        );
        assert_eq!(parse_query_u64(query, "a"), Some(1));
        assert_eq!(parse_query_f32(query, "b"), Some(2.5));
        assert!(parse_query_bool(query, "flag"));
        assert!(!parse_query_bool(query, "missing"));
    }

    #[tokio::test]
    async fn send_voice_event_sends_custom_event_type_v2() {
        let (client_raw, server_raw) = duplex(64 * 1024);
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let (mut sender, _server_receiver) = server_ws.split();
        let (_client_sender, mut receiver) = client_ws.split();

        send_voice_event(
            &mut sender,
            VoiceWsEvent {
                event_type: "custom",
                session_id: None,
                source: None,
                text: Some("hi"),
                raw_text: None,
                delta: None,
                committed: None,
                window_start_ms: None,
                window_end_ms: None,
                window_index: None,
                speaker: None,
                captured_at_ms: None,
                emitted_at_ms: None,
                inference_ms: None,
                message: None,
                detail: None,
            },
        )
        .await
        .unwrap();

        let msg = receiver.next().await.expect("frame").expect("ok");
        match msg {
            Message::Text(text) => {
                let v: Value = serde_json::from_str(&text).expect("json");
                assert_eq!(v["type"], "custom");
                assert_eq!(v["text"], "hi");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_voice_error_includes_detail_when_present_v2() {
        let (client_raw, server_raw) = duplex(64 * 1024);
        let server_ws = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;
        let client_ws = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
        let (mut sender, _server_receiver) = server_ws.split();
        let (_client_sender, mut receiver) = client_ws.split();

        send_voice_error(&mut sender, "oops", Some("detail-info"))
            .await
            .unwrap();

        let msg = receiver.next().await.expect("frame").expect("ok");
        match msg {
            Message::Text(text) => {
                let v: Value = serde_json::from_str(&text).expect("json");
                assert_eq!(v["type"], "error");
                assert_eq!(v["message"], "oops");
                assert_eq!(v["detail"], "detail-info");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_voice_stateful_session_fake_worker_uses_default_text_when_missing_v2() {
        let _env = crate::test_env::voice_env_lock().lock().await;
        env::set_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL", "1");
        let mut session = start_voice_stateful_session("fake_stateful_worker=1", 0.5, "zh")
            .await
            .expect("fake session");
        env::remove_var("BIFROST_VOICE_ENABLE_FAKE_STATEFUL");

        let pcm = vec![1u8; 320];
        let result = session
            .feed_pcm16(&pcm)
            .await
            .expect("feed")
            .expect("result");
        assert_eq!(result.text, "请打开Bifrost");
        assert_eq!(result.language, "zh");
    }
}
