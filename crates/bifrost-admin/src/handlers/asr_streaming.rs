use std::path::Path;
use std::time::Duration;

use bifrost_asr::transcription::{
    StructuredTranscription, TranscriptionFinishReason, TranscriptionSegment, TranscriptionUsage,
};
use serde::Deserialize;

/// Response from the ASR server when using `response_format=verbose_json`.
#[derive(Debug, Deserialize)]
struct VerboseTranscriptionResponse {
    text: String,
    #[serde(default)]
    segments: Vec<VerboseSegment>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    usage: Option<TranscriptionUsage>,
}

/// A single segment from the verbose_json response, with start/end in seconds.
#[derive(Debug, Deserialize)]
struct VerboseSegment {
    start: f64,
    end: f64,
    text: String,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    speaker_id: Option<String>,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    overlap: bool,
}

/// Result of a whole-file transcription request.
#[derive(Debug, Default)]
pub struct WholeFileTranscription {
    pub text: String,
    /// Segments with (audio_start_ms, audio_end_ms, text).
    pub segments: Vec<(u64, u64, String)>,
    /// Extended segment view for providers that return speakers or overlap.
    pub structured: StructuredTranscription,
}

#[allow(dead_code)]
const DEFAULT_STREAM_WINDOW_MS: u64 = 1_000;
#[allow(dead_code)]
const DEFAULT_STREAM_OVERLAP_MS: u64 = 300;
#[allow(dead_code)]
const MIN_STREAM_WINDOW_MS: u64 = 300;
const ASR_SERVER_REQUEST_TIMEOUT_SECS_ENV: &str = "BIFROST_ASR_SERVER_REQUEST_TIMEOUT_SECS";
const ASR_TEXT_REQUEST_TIMEOUT_SECS_ENV: &str = "BIFROST_ASR_TEXT_REQUEST_TIMEOUT_SECS";

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct AsrStreamQuery {
    window_ms: Option<u64>,
    overlap_ms: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingOptions {
    pub(crate) window_ms: u64,
    pub(crate) overlap_ms: u64,
}

#[allow(dead_code)]
pub(crate) fn stream_options_from_query(query: Option<&str>) -> Result<StreamingOptions, String> {
    let params: AsrStreamQuery = query
        .map(serde_urlencoded::from_str)
        .transpose()
        .map_err(|error| format!("invalid ASR stream query: {error}"))?
        .unwrap_or(AsrStreamQuery {
            window_ms: None,
            overlap_ms: None,
        });
    let window_ms = params
        .window_ms
        .unwrap_or(DEFAULT_STREAM_WINDOW_MS)
        .clamp(MIN_STREAM_WINDOW_MS, 30_000);
    let overlap_ms = params
        .overlap_ms
        .unwrap_or(DEFAULT_STREAM_OVERLAP_MS)
        .min(window_ms / 2);
    Ok(StreamingOptions {
        window_ms,
        overlap_ms,
    })
}

pub(crate) async fn call_asr_text_endpoint(
    server_url: &str,
    language: &str,
    wav_path: &Path,
) -> Result<String, String> {
    let wav_bytes = std::fs::read(wav_path)
        .map_err(|error| format!("read streaming window {}: {error}", wav_path.display()))?;
    let url = format!("{server_url}/v1/audio/transcriptions");
    let client = reqwest::Client::new();
    let mut last_error = None::<String>;
    let request_timeout = asr_text_request_timeout();

    for attempt in 0..2 {
        let part = reqwest::multipart::Part::bytes(wav_bytes.clone())
            .file_name("window.wav")
            .mime_str("audio/wav")
            .map_err(|error| format!("build ASR upload: {error}"))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("language", language.to_string())
            .text("response_format", "text");

        match client
            .post(&url)
            .multipart(form)
            .timeout(request_timeout)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let text = response
                    .text()
                    .await
                    .map_err(|error| format!("read ASR response: {error}"))?;
                if status.is_success() {
                    return Ok(text);
                }
                let error = format!("status: {status}; body: {text}");
                if status.is_server_error() && attempt == 0 {
                    last_error = Some(error);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                return Err(error);
            }
            Err(error) if attempt == 0 => {
                last_error = Some(error.to_string());
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => return Err(error.to_string()),
        }
    }

    Err(last_error.unwrap_or_else(|| "ASR server request failed".to_string()))
}

/// Send the entire WAV file in a single request to the ASR server.
/// Tries `verbose_json` format first to obtain per-segment timestamps;
/// falls back to plain text with a single segment covering the full duration.
pub async fn call_asr_whole_file_endpoint(
    server_url: &str,
    language: &str,
    wav_path: &Path,
    media_duration_ms: Option<u64>,
) -> Result<WholeFileTranscription, String> {
    let wav_bytes = std::fs::read(wav_path)
        .map_err(|error| format!("read WAV file {}: {error}", wav_path.display()))?;
    let url = format!("{server_url}/v1/audio/transcriptions");
    let client = reqwest::Client::new();
    let request_timeout = asr_server_request_timeout(media_duration_ms);

    // --- attempt 1: verbose_json (segments with timestamps) ---
    let mut last_error = None::<String>;
    for attempt in 0..2 {
        let part = reqwest::multipart::Part::bytes(wav_bytes.clone())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|error| format!("build ASR upload: {error}"))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("language", language.to_string())
            .text("response_format", "verbose_json");

        match client
            .post(&url)
            .multipart(form)
            .timeout(request_timeout)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .map_err(|error| format!("read ASR response: {error}"))?;
                if status.is_success() {
                    // Try parsing as verbose_json.
                    if let Ok(verbose) = serde_json::from_str::<VerboseTranscriptionResponse>(&body)
                    {
                        return Ok(whole_file_from_verbose(verbose));
                    }
                    // Server returned success but not JSON — treat body as plain text.
                    return Ok(whole_file_text_fallback(&body, media_duration_ms));
                }
                let error = format!("status: {status}; body: {body}");
                if status.is_server_error() && attempt == 0 {
                    last_error = Some(error);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                // 4xx (e.g. "unsupported response_format") — fall through to text mode.
                if status.is_client_error() {
                    break;
                }
                return Err(error);
            }
            Err(error) => return Err(error.to_string()),
        }
    }

    // --- attempt 2: plain text fallback ---
    tracing::debug!("verbose_json not supported, falling back to text format");
    let part = reqwest::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|error| format!("build ASR upload: {error}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("language", language.to_string())
        .text("response_format", "text");

    let response = client
        .post(&url)
        .multipart(form)
        .timeout(request_timeout)
        .send()
        .await
        .map_err(|error| {
            format!(
                "ASR text fallback: {error} (verbose_json error: {})",
                last_error.as_deref().unwrap_or("none")
            )
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("read ASR text response: {error}"))?;
    if !status.is_success() {
        return Err(format!("ASR text status: {status}; body: {body}"));
    }
    Ok(whole_file_text_fallback(&body, media_duration_ms))
}

pub(crate) fn asr_server_request_timeout(media_duration_ms: Option<u64>) -> Duration {
    if let Ok(value) = std::env::var(ASR_SERVER_REQUEST_TIMEOUT_SECS_ENV) {
        if let Ok(secs) = value.parse::<u64>() {
            if secs > 0 {
                return Duration::from_secs(secs);
            }
        }
    }
    let Some(ms) = media_duration_ms else {
        return Duration::from_secs(600);
    };
    let duration_secs = ms.div_ceil(1000).max(1);
    Duration::from_secs((duration_secs * 4).clamp(60, 180))
}

pub(crate) fn asr_text_request_timeout() -> Duration {
    if let Ok(value) = std::env::var(ASR_TEXT_REQUEST_TIMEOUT_SECS_ENV) {
        if let Ok(secs) = value.parse::<u64>() {
            if secs > 0 {
                return Duration::from_secs(secs);
            }
        }
    }
    Duration::from_secs(45)
}

/// Build a single-segment transcription from plain text.
fn whole_file_text_fallback(body: &str, media_duration_ms: Option<u64>) -> WholeFileTranscription {
    let text = normalize_asr_text(body);
    let structured_segments = if text.is_empty() {
        vec![]
    } else {
        vec![TranscriptionSegment {
            start_ms: 0,
            end_ms: media_duration_ms.unwrap_or(0),
            text: text.clone(),
            speaker: None,
            overlap: false,
        }]
    };
    let segments = structured_segments
        .iter()
        .map(|segment| (segment.start_ms, segment.end_ms, segment.text.clone()))
        .collect();
    WholeFileTranscription {
        text: text.clone(),
        segments,
        structured: StructuredTranscription {
            text,
            segments: structured_segments,
            finish_reason: TranscriptionFinishReason::Unknown,
            usage: None,
        },
    }
}

fn whole_file_from_verbose(verbose: VerboseTranscriptionResponse) -> WholeFileTranscription {
    let text = normalize_asr_text(&verbose.text);
    let structured_segments = verbose
        .segments
        .into_iter()
        .filter_map(|segment| {
            let text = normalize_asr_text(&segment.text);
            if text.is_empty() {
                return None;
            }
            let start_ms = seconds_to_millis(segment.start);
            let end_ms = seconds_to_millis(segment.end).max(start_ms);
            let speaker = segment
                .speaker
                .or(segment.speaker_id)
                .or(segment.speaker_label)
                .map(|speaker| speaker.trim().to_string())
                .filter(|speaker| !speaker.is_empty());
            Some(TranscriptionSegment {
                start_ms,
                end_ms,
                text,
                speaker,
                overlap: segment.overlap,
            })
        })
        .collect::<Vec<_>>();
    let segments = structured_segments
        .iter()
        .map(|segment| (segment.start_ms, segment.end_ms, segment.text.clone()))
        .collect();
    let finish_reason = match verbose.finish_reason.as_deref() {
        Some("stop" | "completed" | "complete") => TranscriptionFinishReason::Completed,
        Some("length" | "max_tokens") => TranscriptionFinishReason::Length,
        Some("cancelled" | "canceled") => TranscriptionFinishReason::Cancelled,
        Some("failed" | "error") => TranscriptionFinishReason::Failed,
        _ => TranscriptionFinishReason::Unknown,
    };
    WholeFileTranscription {
        text: text.clone(),
        segments,
        structured: StructuredTranscription {
            text,
            segments: structured_segments,
            finish_reason,
            usage: verbose.usage,
        },
    }
}

fn seconds_to_millis(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1000.0).round() as u64
    } else {
        0
    }
}

pub(crate) fn normalize_asr_text(text: &str) -> String {
    text.trim()
        .trim_start_matches("<asr_text>")
        .trim_end_matches("</asr_text>")
        .trim()
        .to_string()
}

pub fn dedupe_increment(committed: &str, candidate: &str) -> String {
    let committed = committed.trim();
    let candidate = candidate.trim();
    if candidate.is_empty() || committed.contains(candidate) {
        return String::new();
    }
    if committed.is_empty() {
        return candidate.to_string();
    }

    let committed_chars: Vec<char> = committed.chars().collect();
    let candidate_chars: Vec<char> = candidate.chars().collect();
    let max = committed_chars.len().min(candidate_chars.len());
    for len in (1..=max).rev() {
        if committed_chars[committed_chars.len() - len..] == candidate_chars[..len] {
            return candidate_chars[len..].iter().collect::<String>();
        }
    }
    candidate.to_string()
}

pub fn append_transcript_delta(committed: &mut String, delta: &str) {
    let delta = delta.trim();
    if delta.is_empty() {
        return;
    }
    let starts_with_punctuation = delta
        .chars()
        .next()
        .map(|ch| ch.is_ascii_punctuation() || is_cjk_punctuation(ch))
        .unwrap_or(false);
    let joins_cjk = committed
        .chars()
        .next_back()
        .zip(delta.chars().next())
        .map(|(left, right)| is_cjk_text(left) || is_cjk_text(right))
        .unwrap_or(false);
    if committed.is_empty() || starts_with_punctuation || joins_cjk {
        committed.push_str(delta);
    } else {
        committed.push(' ');
        committed.push_str(delta);
    }
}

fn is_cjk_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '。' | '，' | '、' | '；' | '：' | '？' | '！' | '）' | '】' | '》'
    )
}

fn is_cjk_text(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x9FFF | 0xF900..=0xFAFF)
}

#[cfg(test)]
mod tests {
    use super::{dedupe_increment, normalize_asr_text, stream_options_from_query};

    #[test]
    fn stream_options_default_to_one_second_windows() {
        let options = stream_options_from_query(None).unwrap();
        assert_eq!(options.window_ms, 1_000);
        assert_eq!(options.overlap_ms, 300);

        let clamped = stream_options_from_query(Some("window_ms=100&overlap_ms=5000")).unwrap();
        assert_eq!(clamped.window_ms, 300);
        assert_eq!(clamped.overlap_ms, 150);
    }

    #[test]
    fn transcript_dedupe_keeps_only_new_suffix() {
        assert_eq!(
            dedupe_increment("你好，这是宽增", "这是宽增语音测试"),
            "语音测试"
        );
        assert_eq!(dedupe_increment("hello world", "hello world"), "");
        assert_eq!(dedupe_increment("", "first"), "first");
    }

    #[test]
    fn asr_text_markers_are_stripped() {
        assert_eq!(
            normalize_asr_text("  <asr_text>你好，测试。</asr_text>\n"),
            "你好，测试。"
        );
    }
}

#[cfg(test)]
mod streaming_extra_tests {
    use super::*;

    #[test]
    fn whole_file_text_fallback_builds_single_segment_when_text_present() {
        let transcription = whole_file_text_fallback("  hello  ", Some(42_000));
        assert_eq!(transcription.text, "hello");
        assert_eq!(transcription.segments.len(), 1);
        assert_eq!(transcription.segments[0], (0, 42_000, "hello".to_string()));
    }

    #[test]
    fn whole_file_text_fallback_produces_no_segments_for_empty_text() {
        let transcription = whole_file_text_fallback("   \n", Some(10_000));
        assert!(transcription.text.is_empty());
        assert!(transcription.segments.is_empty());
    }

    #[test]
    fn verbose_response_preserves_moss_speaker_and_usage() {
        let verbose: VerboseTranscriptionResponse = serde_json::from_value(serde_json::json!({
            "text": "你好，开始开会。",
            "segments": [{
                "start": 0.125,
                "end": 2.4,
                "text": "你好，开始开会。",
                "speaker_id": " speaker_00 ",
                "overlap": true
            }],
            "finish_reason": "length",
            "usage": {
                "prompt_tokens": 128,
                "completion_tokens": 42,
                "total_tokens": 170
            }
        }))
        .unwrap();
        let transcription = whole_file_from_verbose(verbose);
        assert_eq!(
            transcription.segments[0],
            (125, 2_400, "你好，开始开会。".to_string())
        );
        assert_eq!(
            transcription.structured.segments[0].speaker.as_deref(),
            Some("speaker_00")
        );
        assert!(transcription.structured.segments[0].overlap);
        assert_eq!(
            transcription.structured.finish_reason,
            TranscriptionFinishReason::Length
        );
        assert_eq!(transcription.structured.usage.unwrap().total_tokens, 170);
    }

    #[test]
    fn verbose_response_remains_compatible_without_speaker_fields() {
        let verbose: VerboseTranscriptionResponse = serde_json::from_value(serde_json::json!({
            "text": "hello",
            "segments": [{"start": -1.0, "end": 1.25, "text": "hello"}]
        }))
        .unwrap();
        let transcription = whole_file_from_verbose(verbose);
        assert_eq!(transcription.segments[0], (0, 1_250, "hello".to_string()));
        assert!(transcription.structured.segments[0].speaker.is_none());
        assert_eq!(
            transcription.structured.finish_reason,
            TranscriptionFinishReason::Unknown
        );
    }

    #[test]
    fn verbose_response_filters_blank_segments_and_maps_terminal_failures() {
        for (finish_reason, expected) in [
            ("cancelled", TranscriptionFinishReason::Cancelled),
            ("failed", TranscriptionFinishReason::Failed),
        ] {
            let verbose: VerboseTranscriptionResponse = serde_json::from_value(serde_json::json!({
                "text": "usable",
                "segments": [
                    {"start": 0.0, "end": 0.5, "text": "   "},
                    {"start": 0.5, "end": 1.0, "text": "usable"}
                ],
                "finish_reason": finish_reason
            }))
            .unwrap();

            let transcription = whole_file_from_verbose(verbose);
            assert_eq!(transcription.segments.len(), 1);
            assert_eq!(transcription.structured.finish_reason, expected);
        }
    }

    #[test]
    fn append_transcript_delta_inserts_spaces_between_ascii_words() {
        let mut committed = "hello".to_string();
        append_transcript_delta(&mut committed, "world");
        assert_eq!(committed, "hello world");

        // empty delta is ignored
        append_transcript_delta(&mut committed, "   ");
        assert_eq!(committed, "hello world");
    }

    #[test]
    fn append_transcript_delta_joins_cjk_text_without_extra_space() {
        let mut committed = "你好".to_string();
        append_transcript_delta(&mut committed, "，世界");
        // CJK punctuation at the start of delta should attach without an extra space
        assert_eq!(committed, "你好，世界");

        let mut committed = String::new();
        append_transcript_delta(&mut committed, "测试");
        assert_eq!(committed, "测试");
    }
}
