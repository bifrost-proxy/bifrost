use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

const DEFAULT_STREAM_WINDOW_MS: u64 = 1_000;
const DEFAULT_STREAM_OVERLAP_MS: u64 = 300;
const MIN_STREAM_WINDOW_MS: u64 = 300;
const STREAM_BOUNDARY_SEARCH_BEFORE_MS: u64 = 250;
const STREAM_BOUNDARY_SEARCH_AFTER_MS: u64 = 125;
const STREAM_ENERGY_FRAME_MS: u64 = 50;

#[derive(Debug, Clone, Deserialize)]
struct AsrStreamQuery {
    window_ms: Option<u64>,
    overlap_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingOptions {
    pub(crate) window_ms: u64,
    pub(crate) overlap_ms: u64,
}

#[derive(Debug)]
pub(crate) struct WavAudio {
    sample_rate: u32,
    samples: Vec<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamWindow {
    pub(crate) index: usize,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) stable_start_ms: u64,
    pub(crate) stable_end_ms: u64,
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

pub(crate) async fn extract_wav_segment(
    source_wav: &Path,
    segment_path: &Path,
    window: &StreamWindow,
) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format_seconds(window.start_ms))
        .arg("-to")
        .arg(format_seconds(window.end_ms))
        .arg("-i")
        .arg(source_wav)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(segment_path)
        .output()
        .await
        .map_err(|error| format!("failed to run ffmpeg for ASR window: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub(crate) fn build_stream_windows(
    audio: &WavAudio,
    options: StreamingOptions,
) -> Vec<StreamWindow> {
    let duration_ms = audio.duration_ms();
    if duration_ms < MIN_STREAM_WINDOW_MS || audio.samples.is_empty() {
        return Vec::new();
    }

    let mut windows = Vec::new();
    let mut stable_start_ms = 0u64;
    while stable_start_ms < duration_ms {
        let target_end_ms = (stable_start_ms + options.window_ms).min(duration_ms);
        let is_tail = target_end_ms >= duration_ms;
        let mut stable_end_ms = if is_tail {
            duration_ms
        } else {
            choose_energy_boundary_ms(audio, stable_start_ms, target_end_ms, options.window_ms)
        };
        let min_end_ms = (stable_start_ms + MIN_STREAM_WINDOW_MS).min(duration_ms);
        if stable_end_ms < min_end_ms {
            stable_end_ms = min_end_ms;
        }
        if stable_end_ms <= stable_start_ms {
            stable_end_ms = target_end_ms;
        }

        let start_ms = stable_start_ms.saturating_sub(options.overlap_ms);
        windows.push(StreamWindow {
            index: windows.len(),
            start_ms,
            end_ms: stable_end_ms,
            stable_start_ms,
            stable_end_ms,
        });

        if stable_end_ms >= duration_ms {
            break;
        }
        stable_start_ms = stable_end_ms;
    }
    windows
}

fn choose_energy_boundary_ms(
    audio: &WavAudio,
    stable_start_ms: u64,
    target_end_ms: u64,
    window_ms: u64,
) -> u64 {
    let lower = (target_end_ms.saturating_sub(STREAM_BOUNDARY_SEARCH_BEFORE_MS))
        .max(stable_start_ms + MIN_STREAM_WINDOW_MS);
    let upper = (target_end_ms + STREAM_BOUNDARY_SEARCH_AFTER_MS).min(audio.duration_ms());
    if lower >= upper {
        return target_end_ms;
    }

    let frame_ms = STREAM_ENERGY_FRAME_MS.max(1);
    let mut best = target_end_ms;
    let mut best_energy = f64::MAX;
    let mut cursor = lower;
    while cursor + frame_ms <= upper {
        let energy = frame_energy(audio, cursor, cursor + frame_ms);
        if energy < best_energy {
            best_energy = energy;
            best = cursor + frame_ms / 2;
        }
        cursor += frame_ms;
    }

    let max_drift = window_ms / 3;
    if best.abs_diff(target_end_ms) <= max_drift {
        best
    } else {
        target_end_ms
    }
}

fn frame_energy(audio: &WavAudio, start_ms: u64, end_ms: u64) -> f64 {
    let start = audio.sample_index(start_ms).min(audio.samples.len());
    let end = audio.sample_index(end_ms).min(audio.samples.len());
    if start >= end {
        return f64::MAX;
    }
    let sum: f64 = audio.samples[start..end]
        .iter()
        .map(|sample| (*sample as f64).abs())
        .sum();
    sum / (end - start) as f64
}

pub(crate) fn parse_wav_pcm_i16(bytes: &[u8]) -> Result<WavAudio, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("expected RIFF/WAVE audio after FFmpeg normalization".to_string());
    }

    let mut offset = 12usize;
    let mut sample_rate = None::<u32>;
    let mut channels = None::<u16>;
    let mut bits_per_sample = None::<u16>;
    let mut data = None::<&[u8]>;

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.saturating_add(chunk_len).min(bytes.len());
        if chunk_end < chunk_start {
            break;
        }

        match chunk_id {
            b"fmt " if chunk_len >= 16 && chunk_start + 16 <= chunk_end => {
                let audio_format = u16::from_le_bytes([bytes[chunk_start], bytes[chunk_start + 1]]);
                if audio_format != 1 {
                    return Err(format!("expected PCM WAV, got format {audio_format}"));
                }
                channels = Some(u16::from_le_bytes([
                    bytes[chunk_start + 2],
                    bytes[chunk_start + 3],
                ]));
                sample_rate = Some(u32::from_le_bytes([
                    bytes[chunk_start + 4],
                    bytes[chunk_start + 5],
                    bytes[chunk_start + 6],
                    bytes[chunk_start + 7],
                ]));
                bits_per_sample = Some(u16::from_le_bytes([
                    bytes[chunk_start + 14],
                    bytes[chunk_start + 15],
                ]));
            }
            b"data" => data = Some(&bytes[chunk_start..chunk_end]),
            _ => {}
        }

        offset = chunk_end + (chunk_len % 2);
    }

    let sample_rate = sample_rate.ok_or_else(|| "WAV fmt chunk missing sample rate".to_string())?;
    if channels != Some(1) {
        return Err(format!("expected mono WAV, got channels {channels:?}"));
    }
    if bits_per_sample != Some(16) {
        return Err(format!(
            "expected 16-bit PCM WAV, got bits {bits_per_sample:?}"
        ));
    }
    let data = data.ok_or_else(|| "WAV data chunk missing".to_string())?;
    let samples = data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    Ok(WavAudio {
        sample_rate,
        samples,
    })
}

pub(crate) fn normalize_asr_text(text: &str) -> String {
    text.trim()
        .trim_start_matches("<asr_text>")
        .trim_end_matches("</asr_text>")
        .trim()
        .to_string()
}

pub(crate) fn dedupe_increment(committed: &str, candidate: &str) -> String {
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

pub(crate) fn append_transcript_delta(committed: &mut String, delta: &str) {
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

fn format_seconds(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

impl WavAudio {
    fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        ((self.samples.len() as u64) * 1000) / self.sample_rate as u64
    }

    fn sample_index(&self, ms: u64) -> usize {
        ((ms * self.sample_rate as u64) / 1000) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_stream_windows, dedupe_increment, normalize_asr_text, parse_wav_pcm_i16,
        stream_options_from_query, StreamingOptions, WavAudio,
    };

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
    fn streaming_windows_use_overlap_and_flush_tail() {
        let audio = WavAudio {
            sample_rate: 1_000,
            samples: vec![100; 5_250],
        };
        let windows = build_stream_windows(
            &audio,
            StreamingOptions {
                window_ms: 2_000,
                overlap_ms: 600,
            },
        );

        assert!(windows.len() >= 3);
        assert_eq!(windows[0].start_ms, 0);
        assert_eq!(windows[0].stable_start_ms, 0);
        assert_eq!(windows[1].start_ms, windows[1].stable_start_ms - 600);
        assert_eq!(windows.last().unwrap().stable_end_ms, 5_250);
    }

    #[test]
    fn streaming_windows_pick_low_energy_boundary_near_target() {
        let mut samples = vec![3_000; 4_500];
        samples[1_820..1_900].fill(0);
        let audio = WavAudio {
            sample_rate: 1_000,
            samples,
        };
        let windows = build_stream_windows(
            &audio,
            StreamingOptions {
                window_ms: 2_000,
                overlap_ms: 500,
            },
        );

        assert!((1_800..=1_950).contains(&windows[0].stable_end_ms));
    }

    #[test]
    fn streaming_windows_ignore_too_short_audio() {
        let audio = WavAudio {
            sample_rate: 16_000,
            samples: vec![0; 2_000],
        };
        let windows = build_stream_windows(
            &audio,
            StreamingOptions {
                window_ms: 2_000,
                overlap_ms: 600,
            },
        );
        assert!(windows.is_empty());
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

    #[test]
    fn normalized_wav_parser_reads_pcm_duration() {
        let wav = test_wav_pcm_mono(1_000, &[0; 1_000]);
        let parsed = parse_wav_pcm_i16(&wav).unwrap();
        assert_eq!(parsed.sample_rate, 1_000);
        assert_eq!(parsed.samples.len(), 1_000);
        assert_eq!(parsed.duration_ms(), 1_000);
    }

    fn test_wav_pcm_mono(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = samples.len() as u32 * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }
}
