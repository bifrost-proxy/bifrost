use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Command;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, warn};
use uuid::Uuid;

use super::{error_response, BoxBody};
use crate::handlers::asr::{
    resolve_asr_target, AsrErrorPayload, AsrSegmentPayload, AsrStreamPayload,
};
use crate::handlers::asr_streaming::{
    append_transcript_delta, call_asr_text_endpoint, dedupe_increment, normalize_asr_text,
    stream_options_from_query, StreamingOptions,
};
use crate::handlers::websocket::generate_accept_key;

const MAX_WS_TEXT_BYTES: usize = 32 * 1024;
const MAX_WS_AUDIO_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MAX_WS_BUFFERED_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 800;
const MIN_FLUSH_INTERVAL_MS: u64 = 500;
const MAX_FLUSH_INTERVAL_MS: u64 = 10_000;
const MIN_REALTIME_SEGMENT_MS: u64 = 300;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AsrWsClientMessage {
    Start {
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        file_name: Option<String>,
    },
    Audio {
        data: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        sequence: Option<u64>,
    },
    Flush,
    Finish,
    Cancel,
}

#[derive(Debug, Clone)]
pub(crate) struct AsrWsOptions {
    pub(crate) stream: StreamingOptions,
    pub(crate) flush_interval: Duration,
}

#[derive(Debug)]
pub(crate) struct AsrRealtimeBuffer {
    session_bytes: Vec<u8>,
    processed_duration_ms: u64,
    max_bytes: usize,
}

impl AsrRealtimeBuffer {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            session_bytes: Vec::new(),
            processed_duration_ms: 0,
            max_bytes,
        }
    }

    pub(crate) fn push(&mut self, chunk: Vec<u8>) -> Result<(), String> {
        if chunk.is_empty() {
            return Ok(());
        }
        if self.session_bytes.len().saturating_add(chunk.len()) > self.max_bytes {
            return Err("ASR WebSocket audio buffer exceeded its bounded capacity".to_string());
        }
        self.session_bytes.extend_from_slice(&chunk);
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.session_bytes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.session_bytes.len()
    }

    fn processed_duration_ms(&self) -> u64 {
        self.processed_duration_ms
    }

    fn session_bytes(&self) -> &[u8] {
        &self.session_bytes
    }

    fn mark_processed(&mut self, duration_ms: u64) {
        self.processed_duration_ms = self.processed_duration_ms.max(duration_ms);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RealtimeAudioInfo {
    duration_ms: u64,
    has_audio: bool,
}

impl RealtimeAudioInfo {
    fn enough_new_audio_since(self, processed_duration_ms: u64, final_flush: bool) -> bool {
        if !self.has_audio || self.duration_ms <= processed_duration_ms {
            return false;
        }
        final_flush
            || self.duration_ms.saturating_sub(processed_duration_ms) >= MIN_REALTIME_SEGMENT_MS
    }
}

#[derive(Debug)]
pub(crate) struct AsrRealtimeCommitter {
    committed: String,
}

impl AsrRealtimeCommitter {
    pub(crate) fn new() -> Self {
        Self {
            committed: String::new(),
        }
    }

    pub(crate) fn committed(&self) -> &str {
        &self.committed
    }

    pub(crate) fn commit_candidate(&mut self, candidate: &str) -> String {
        let delta = dedupe_increment(&self.committed, candidate);
        if !delta.is_empty() {
            append_transcript_delta(&mut self.committed, &delta);
        }
        delta
    }
}

pub(crate) fn parse_ws_client_message(text: &str) -> Result<AsrWsClientMessage, String> {
    if text.len() > MAX_WS_TEXT_BYTES {
        return Err("ASR WebSocket control frame is too large".to_string());
    }
    serde_json::from_str(text).map_err(|error| format!("invalid ASR WebSocket message: {error}"))
}

pub(crate) fn decode_audio_payload(data: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("invalid base64 audio payload: {error}"))
}

pub(crate) fn asr_ws_options_from_query(query: Option<&str>) -> Result<AsrWsOptions, String> {
    let stream = stream_options_from_query(query)?;
    let mut flush_interval_ms = DEFAULT_FLUSH_INTERVAL_MS;
    if let Some(query) = query {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "flush_interval_ms" {
                    let value = urlencoding::decode(value).unwrap_or_default();
                    if !value.is_empty() {
                        flush_interval_ms = value
                            .parse::<u64>()
                            .map_err(|error| format!("invalid flush_interval_ms: {error}"))?;
                    }
                }
            }
        }
    }
    Ok(AsrWsOptions {
        stream,
        flush_interval: Duration::from_millis(
            flush_interval_ms.clamp(MIN_FLUSH_INTERVAL_MS, MAX_FLUSH_INTERVAL_MS),
        ),
    })
}

pub async fn handle_asr_ws_upgrade(req: Request<Incoming>) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid ASR WebSocket upgrade header",
        );
    }

    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header"),
    };

    let target = match resolve_asr_target(req.uri().query()).await {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let options = match asr_ws_options_from_query(req.uri().query()) {
        Ok(options) => options,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let accept_key = generate_accept_key(&ws_key);

    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                error!("ASR WebSocket upgrade failed: {}", error);
                return;
            }
        };
        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        handle_asr_ws_connection(ws_stream, target, options).await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

async fn handle_asr_ws_connection<S>(
    ws_stream: WebSocketStream<S>,
    target: crate::handlers::asr::AsrTarget,
    options: AsrWsOptions,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, mut receiver) = ws_stream.split();
    if let Err(error) = send_progress_ws(
        &mut sender,
        AsrStreamPayload {
            phase: "connected",
            status: "running",
            progress: 1,
            message: "ASR WebSocket connection established.",
            detail: None,
            file: None,
            server_url: Some(target.server_url_display()),
        },
    )
    .await
    {
        warn!("failed to send ASR WebSocket connected event: {}", error);
        return;
    }

    if let Err(error) = crate::handlers::asr::probe_asr_health(&target).await {
        let _ = send_error_ws(
            &mut sender,
            "Qwen3-ASR model service is not running. Start it from AI > Tools > ASR first.",
            Some(&format!("server: {}; {error}", target.server_url_display())),
        )
        .await;
        let _ = sender.close().await;
        return;
    }
    let Some(server_url) = target.server_url() else {
        let _ = send_error_ws(
            &mut sender,
            "Qwen3-ASR model service is not running. Start it from AI > Tools > ASR first.",
            Some("no managed ASR server port is available"),
        )
        .await;
        let _ = sender.close().await;
        return;
    };

    let tmp_dir = match tempfile::tempdir() {
        Ok(tmp_dir) => tmp_dir,
        Err(error) => {
            let _ = send_error_ws(
                &mut sender,
                "Failed to create ASR WebSocket temp directory.",
                Some(&error.to_string()),
            )
            .await;
            return;
        }
    };
    let tmp_root = tmp_dir.path().to_path_buf();
    let mut buffer = AsrRealtimeBuffer::new(MAX_WS_BUFFERED_BYTES);
    let mut committer = AsrRealtimeCommitter::new();
    let mut started = false;
    let mut mime_type = "audio/webm".to_string();
    let mut last_flush = Instant::now();
    let mut segment_index = 0usize;

    while let Some(message) = receiver.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                warn!("ASR WebSocket receive failed: {}", error);
                break;
            }
        };

        match message {
            Message::Text(text) => match parse_ws_client_message(&text) {
                Ok(AsrWsClientMessage::Start {
                    mime_type: next_mime_type,
                    file_name,
                }) => {
                    started = true;
                    if let Some(next_mime_type) = next_mime_type.filter(|value| !value.is_empty()) {
                        mime_type = next_mime_type;
                    }
                    let _ = send_progress_ws(
                        &mut sender,
                        AsrStreamPayload {
                            phase: "stream",
                            status: "running",
                            progress: 5,
                            message: "ASR WebSocket audio stream started.",
                            detail: Some(&format!(
                                "mime_type: {}; file: {}",
                                mime_type,
                                file_name.unwrap_or_else(|| "microphone".to_string())
                            )),
                            file: None,
                            server_url: Some(target.server_url_display()),
                        },
                    )
                    .await;
                }
                Ok(AsrWsClientMessage::Audio {
                    data,
                    mime_type: next_mime_type,
                    sequence,
                }) => {
                    started = true;
                    if let Some(next_mime_type) = next_mime_type.filter(|value| !value.is_empty()) {
                        mime_type = next_mime_type;
                    }
                    let chunk = match decode_audio_payload(&data) {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            let _ = send_error_ws(
                                &mut sender,
                                "Invalid ASR WebSocket audio chunk.",
                                Some(&error),
                            )
                            .await;
                            continue;
                        }
                    };
                    if chunk.len() > MAX_WS_AUDIO_CHUNK_BYTES {
                        let _ = send_error_ws(
                            &mut sender,
                            "ASR WebSocket audio chunk is too large.",
                            Some(&format!("sequence: {sequence:?}; bytes: {}", chunk.len())),
                        )
                        .await;
                        continue;
                    }
                    if let Err(error) = buffer.push(chunk) {
                        let _ = send_error_ws(
                            &mut sender,
                            "ASR WebSocket audio buffer failed.",
                            Some(&error),
                        )
                        .await;
                        break;
                    }
                    if last_flush.elapsed() >= options.flush_interval {
                        let flush_result = flush_ws_audio(
                            &mut sender,
                            &tmp_root,
                            &server_url,
                            &target.language,
                            &mime_type,
                            &mut buffer,
                            &mut committer,
                            &mut segment_index,
                            options.stream,
                            false,
                        )
                        .await;
                        if let Err(error) = flush_result {
                            let _ = send_error_ws(
                                &mut sender,
                                "ASR WebSocket transcription failed.",
                                Some(&error),
                            )
                            .await;
                        }
                        last_flush = Instant::now();
                    }
                }
                Ok(AsrWsClientMessage::Flush) => {
                    if let Err(error) = flush_ws_audio(
                        &mut sender,
                        &tmp_root,
                        &server_url,
                        &target.language,
                        &mime_type,
                        &mut buffer,
                        &mut committer,
                        &mut segment_index,
                        options.stream,
                        false,
                    )
                    .await
                    {
                        let _ =
                            send_error_ws(&mut sender, "ASR WebSocket flush failed.", Some(&error))
                                .await;
                    }
                    last_flush = Instant::now();
                }
                Ok(AsrWsClientMessage::Finish) => {
                    if started {
                        if let Err(error) = flush_ws_audio(
                            &mut sender,
                            &tmp_root,
                            &server_url,
                            &target.language,
                            &mime_type,
                            &mut buffer,
                            &mut committer,
                            &mut segment_index,
                            options.stream,
                            true,
                        )
                        .await
                        {
                            let _ = send_error_ws(
                                &mut sender,
                                "ASR WebSocket final flush failed.",
                                Some(&error),
                            )
                            .await;
                        }
                    }
                    let _ = send_text_ws(&mut sender, committer.committed().trim()).await;
                    let _ = send_done_ws(&mut sender).await;
                    let _ = sender.close().await;
                    break;
                }
                Ok(AsrWsClientMessage::Cancel) => {
                    let _ = send_done_ws(&mut sender).await;
                    let _ = sender.close().await;
                    break;
                }
                Err(error) => {
                    let _ = send_error_ws(
                        &mut sender,
                        "Invalid ASR WebSocket control message.",
                        Some(&error),
                    )
                    .await;
                }
            },
            Message::Binary(chunk) => {
                started = true;
                if chunk.len() > MAX_WS_AUDIO_CHUNK_BYTES {
                    let _ = send_error_ws(
                        &mut sender,
                        "ASR WebSocket binary audio chunk is too large.",
                        Some(&format!("bytes: {}", chunk.len())),
                    )
                    .await;
                    continue;
                }
                if let Err(error) = buffer.push(chunk.to_vec()) {
                    let _ = send_error_ws(
                        &mut sender,
                        "ASR WebSocket audio buffer failed.",
                        Some(&error),
                    )
                    .await;
                    break;
                }
                if last_flush.elapsed() >= options.flush_interval {
                    if let Err(error) = flush_ws_audio(
                        &mut sender,
                        &tmp_root,
                        &server_url,
                        &target.language,
                        &mime_type,
                        &mut buffer,
                        &mut committer,
                        &mut segment_index,
                        options.stream,
                        false,
                    )
                    .await
                    {
                        let _ = send_error_ws(
                            &mut sender,
                            "ASR WebSocket transcription failed.",
                            Some(&error),
                        )
                        .await;
                    }
                    last_flush = Instant::now();
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

    debug!(segments = segment_index, "ASR WebSocket connection closed");
}

#[allow(clippy::too_many_arguments)]
async fn flush_ws_audio<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    tmp_root: &std::path::Path,
    server_url: &str,
    language: &str,
    mime_type: &str,
    buffer: &mut AsrRealtimeBuffer,
    committer: &mut AsrRealtimeCommitter,
    segment_index: &mut usize,
    options: StreamingOptions,
    final_flush: bool,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if buffer.is_empty() {
        return Ok(());
    }
    let session_path =
        transcode_realtime_session(tmp_root, buffer.session_bytes(), mime_type).await?;
    let info = probe_realtime_audio(&session_path).await?;
    if !info.enough_new_audio_since(buffer.processed_duration_ms(), final_flush) {
        let _ = std::fs::remove_file(session_path);
        return Ok(());
    }
    let start_ms = buffer
        .processed_duration_ms()
        .saturating_sub(options.overlap_ms);
    let end_ms = info.duration_ms;
    let segment_path = extract_realtime_segment(tmp_root, &session_path, start_ms, end_ms).await?;
    let text =
        normalize_asr_text(&call_asr_text_endpoint(server_url, language, &segment_path).await?);
    let delta = committer.commit_candidate(&text);
    let committed = committer.committed().to_string();
    let index = *segment_index;
    *segment_index += 1;
    let stable_start_ms = buffer.processed_duration_ms();
    let stable_end_ms = info.duration_ms;
    buffer.mark_processed(stable_end_ms);
    let payload = AsrSegmentPayload {
        index,
        start_ms,
        end_ms,
        stable_start_ms,
        stable_end_ms,
        text: &text,
        delta: &delta,
        committed: &committed,
        speaker: None,
        speaker_display_name: None,
        speaker_profile_id: None,
        speaker_confidence: None,
    };
    send_asr_segment_ws(sender, "partial", &payload)
        .await
        .map_err(|error| error.to_string())?;
    send_asr_segment_ws(sender, "final", &payload)
        .await
        .map_err(|error| error.to_string())?;
    send_progress_ws(
        sender,
        AsrStreamPayload {
            phase: if final_flush { "finish" } else { "stream" },
            status: "running",
            progress: if final_flush { 98 } else { 50 },
            message: "Processed ASR WebSocket audio chunk.",
            detail: Some(&format!(
                "segment: {index}; buffered_bytes: {}; processed_ms: {}; duration_ms: {}",
                buffer.len(),
                buffer.processed_duration_ms(),
                info.duration_ms
            )),
            file: segment_path.file_name().and_then(|name| name.to_str()),
            server_url: Some(server_url.to_string()),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(session_path);
    let _ = std::fs::remove_file(segment_path);
    Ok(())
}

async fn transcode_realtime_session(
    tmp_root: &std::path::Path,
    audio_bytes: &[u8],
    mime_type: &str,
) -> Result<PathBuf, String> {
    let id = Uuid::new_v4();
    let extension = audio_extension_from_mime(mime_type);
    let input_path = tmp_root.join(format!("ws-{id}.{extension}"));
    let output_path = tmp_root.join(format!("ws-{id}.wav"));
    std::fs::write(&input_path, audio_bytes).map_err(|error| format!("write WS audio: {error}"))?;
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
    let _ = std::fs::remove_file(&input_path);
    if !output.status.success() {
        return Err(format!(
            "ffmpeg failed to convert WebSocket audio to WAV: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output_path)
}

async fn probe_realtime_audio(wav_path: &std::path::Path) -> Result<RealtimeAudioInfo, String> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(wav_path)
        .output()
        .await
        .map_err(|error| format!("failed to run ffprobe for ASR WebSocket audio: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe failed for WebSocket audio: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let duration_secs = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|error| format!("parse WebSocket audio duration: {error}"))?;
    let duration_ms = (duration_secs * 1000.0).round().max(0.0) as u64;
    Ok(RealtimeAudioInfo {
        duration_ms,
        has_audio: duration_ms >= MIN_REALTIME_SEGMENT_MS,
    })
}

async fn extract_realtime_segment(
    tmp_root: &std::path::Path,
    source_wav: &std::path::Path,
    start_ms: u64,
    end_ms: u64,
) -> Result<PathBuf, String> {
    let id = Uuid::new_v4();
    let output_path = tmp_root.join(format!("ws-segment-{id}.wav"));
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format_seconds(start_ms))
        .arg("-to")
        .arg(format_seconds(end_ms))
        .arg("-i")
        .arg(source_wav)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(&output_path)
        .output()
        .await
        .map_err(|error| format!("failed to slice ASR WebSocket audio: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg failed to slice WebSocket audio: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output_path)
}

fn format_seconds(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

fn audio_extension_from_mime(mime_type: &str) -> &'static str {
    let mime_type = mime_type.to_ascii_lowercase();
    if mime_type.contains("wav") {
        "wav"
    } else if mime_type.contains("ogg") {
        "ogg"
    } else if mime_type.contains("mp4") || mime_type.contains("m4a") {
        "m4a"
    } else {
        "webm"
    }
}

async fn send_progress_ws<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    payload: AsrStreamPayload<'_>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let event_type = asr_ws_progress_event_type(payload.phase);
    send_json_ws(sender, event_type, payload).await
}

fn asr_ws_progress_event_type(phase: &str) -> &str {
    match phase {
        "connected" => "connected",
        "stream" => "stream",
        "finish" => "finish",
        _ => "progress",
    }
}

async fn send_text_ws<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    text: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_json_ws(
        sender,
        "text",
        crate::handlers::asr::AsrTextPayload { text },
    )
    .await
}

async fn send_asr_segment_ws<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    event_type: &str,
    payload: &AsrSegmentPayload<'_>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_json_ws(sender, event_type, payload).await
}

async fn send_error_ws<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    message: &str,
    detail: Option<&str>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_json_ws(sender, "error", AsrErrorPayload { message, detail }).await
}

async fn send_done_ws<S>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_json_ws(sender, "done", serde_json::json!({ "ok": true })).await
}

async fn send_json_ws<S, T>(
    sender: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    event_type: &str,
    payload: T,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: Serialize,
{
    let mut value = serde_json::to_value(payload).unwrap_or_else(
        |error| serde_json::json!({ "message": format!("failed to serialize event: {error}") }),
    );
    if let serde_json::Value::Object(object) = &mut value {
        object.insert(
            "type".to_string(),
            serde_json::Value::String(event_type.to_string()),
        );
    } else {
        value = serde_json::json!({ "type": event_type, "data": value });
    }
    sender.send(Message::Text(value.to_string().into())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ws_client_audio_message() {
        let msg = parse_ws_client_message(
            r#"{"type":"audio","data":"AQID","mime_type":"audio/webm","sequence":7}"#,
        )
        .expect("message");
        match msg {
            AsrWsClientMessage::Audio {
                data,
                mime_type,
                sequence,
            } => {
                assert_eq!(data, "AQID");
                assert_eq!(mime_type.as_deref(), Some("audio/webm"));
                assert_eq!(sequence, Some(7));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn test_decode_audio_payload_rejects_invalid_base64() {
        let err = decode_audio_payload("***").unwrap_err();
        assert!(err.contains("invalid base64 audio payload"));
    }

    #[test]
    fn test_realtime_buffer_rejects_overflow_to_bound_memory() {
        let mut buffer = AsrRealtimeBuffer::new(5);
        buffer.push(vec![1, 2, 3]).unwrap();
        let err = buffer.push(vec![4, 5, 6]).unwrap_err();
        assert!(err.contains("bounded capacity"));
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.session_bytes(), &[1, 2, 3]);
    }

    #[test]
    fn test_realtime_buffer_keeps_complete_mediarecorder_session() {
        let mut buffer = AsrRealtimeBuffer::new(16);
        buffer.push(vec![1, 2]).unwrap();
        buffer.push(vec![3, 4]).unwrap();
        assert_eq!(buffer.session_bytes(), &[1, 2, 3, 4]);
        assert_eq!(buffer.processed_duration_ms(), 0);
        buffer.mark_processed(2_000);
        buffer.mark_processed(1_000);
        assert_eq!(buffer.processed_duration_ms(), 2_000);
    }

    #[test]
    fn test_realtime_audio_info_requires_new_audio_before_flush() {
        let info = RealtimeAudioInfo {
            duration_ms: 2_200,
            has_audio: true,
        };
        assert!(info.enough_new_audio_since(1_800, false));
        assert!(!info.enough_new_audio_since(2_000, false));
        assert!(info.enough_new_audio_since(2_000, true));
        assert!(!RealtimeAudioInfo {
            duration_ms: 200,
            has_audio: false,
        }
        .enough_new_audio_since(0, true));
    }

    #[test]
    fn test_realtime_committer_dedupes_incremental_text() {
        let mut committer = AsrRealtimeCommitter::new();
        assert_eq!(committer.commit_candidate("hello world"), "hello world");
        assert_eq!(committer.committed(), "hello world");
        assert_eq!(committer.commit_candidate("world again"), " again");
        assert_eq!(committer.committed(), "hello world again");
        assert_eq!(committer.commit_candidate("hello world"), "");
    }

    #[test]
    fn test_ws_options_clamps_flush_interval() {
        let options = asr_ws_options_from_query(Some("flush_interval_ms=1&window_ms=400")).unwrap();
        assert_eq!(
            options.flush_interval,
            Duration::from_millis(MIN_FLUSH_INTERVAL_MS)
        );
        assert_eq!(options.stream.window_ms, 400);

        let defaults = asr_ws_options_from_query(None).unwrap();
        assert_eq!(defaults.flush_interval, Duration::from_millis(800));
        assert_eq!(defaults.stream.window_ms, 1_000);
    }

    #[test]
    fn test_ws_progress_event_type_uses_visible_realtime_phases() {
        assert_eq!(asr_ws_progress_event_type("connected"), "connected");
        assert_eq!(asr_ws_progress_event_type("stream"), "stream");
        assert_eq!(asr_ws_progress_event_type("finish"), "finish");
        assert_eq!(asr_ws_progress_event_type("preprocess"), "progress");
    }
}

#[cfg(test)]
mod ws_extra_tests {
    use super::*;

    #[test]
    fn decode_audio_payload_decodes_valid_base64() {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let decoded = decode_audio_payload(&encoded).unwrap();
        assert_eq!(decoded, vec![1, 2, 3]);
    }

    #[test]
    fn parse_ws_client_message_rejects_oversized_payloads() {
        // MAX_WS_TEXT_BYTES is an upper bound for accepted control frames
        let oversized = "x".repeat(MAX_WS_TEXT_BYTES + 1);
        let err = parse_ws_client_message(&oversized).unwrap_err();
        assert!(err.contains("too large"));
    }

    #[test]
    fn ws_options_flush_interval_is_clamped_to_maximum() {
        let query = format!(
            "flush_interval_ms=200000&window_ms={}",
            crate::handlers::asr_streaming::MIN_STREAM_WINDOW_MS
        );
        let options = asr_ws_options_from_query(Some(&query)).unwrap();
        assert_eq!(
            options.flush_interval,
            Duration::from_millis(MAX_FLUSH_INTERVAL_MS)
        );
    }

    #[test]
    fn audio_extension_from_mime_inferrs_common_extensions() {
        assert_eq!(audio_extension_from_mime("audio/webm"), "webm");
        assert_eq!(audio_extension_from_mime("audio/ogg"), "ogg");
        assert_eq!(audio_extension_from_mime("audio/mp4"), "m4a");
        assert_eq!(audio_extension_from_mime("audio/wav"), "wav");
        assert_eq!(audio_extension_from_mime("application/octet-stream"), "webm");
    }

    #[test]
    fn format_seconds_formats_milliseconds_with_three_fractional_digits() {
        assert_eq!(format_seconds(1_500), "1.500");
        assert_eq!(format_seconds(0), "0.000");
    }
}
