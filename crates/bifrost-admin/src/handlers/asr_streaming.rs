use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Response from the ASR server when using `response_format=verbose_json`.
#[derive(Debug, Deserialize)]
struct VerboseTranscriptionResponse {
    text: String,
    #[serde(default)]
    segments: Vec<VerboseSegment>,
}

/// A single segment from the verbose_json response, with start/end in seconds.
#[derive(Debug, Deserialize)]
struct VerboseSegment {
    start: f64,
    end: f64,
    text: String,
}

/// Result of a whole-file transcription request.
#[derive(Debug)]
pub struct WholeFileTranscription {
    pub text: String,
    /// Segments with (audio_start_ms, audio_end_ms, text).
    pub segments: Vec<(u64, u64, String)>,
}

const DEFAULT_STREAM_WINDOW_MS: u64 = 1_000;
const DEFAULT_STREAM_OVERLAP_MS: u64 = 300;
const MIN_STREAM_WINDOW_MS: u64 = 300;

#[derive(Debug, Clone, Deserialize)]
struct AsrStreamQuery {
    window_ms: Option<u64>,
    overlap_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingOptions {
    #[allow(dead_code)] // read by asr_ws tests
    pub(crate) window_ms: u64,
    pub(crate) overlap_ms: u64,
}

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
            .timeout(Duration::from_secs(300))
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
            .timeout(Duration::from_secs(600))
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
                        let text = normalize_asr_text(&verbose.text);
                        let segments = verbose
                            .segments
                            .into_iter()
                            .filter(|seg| !seg.text.trim().is_empty())
                            .map(|seg| {
                                let start_ms = (seg.start * 1000.0) as u64;
                                let end_ms = (seg.end * 1000.0) as u64;
                                let text = normalize_asr_text(&seg.text);
                                (start_ms, end_ms, text)
                            })
                            .collect();
                        return Ok(WholeFileTranscription { text, segments });
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
            Err(error) if attempt == 0 => {
                last_error = Some(error.to_string());
                tokio::time::sleep(Duration::from_millis(500)).await;
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
        .timeout(Duration::from_secs(600))
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

/// Build a single-segment transcription from plain text.
fn whole_file_text_fallback(body: &str, media_duration_ms: Option<u64>) -> WholeFileTranscription {
    let text = normalize_asr_text(body);
    let segments = if text.is_empty() {
        vec![]
    } else {
        vec![(0, media_duration_ms.unwrap_or(0), text.clone())]
    };
    WholeFileTranscription { text, segments }
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
