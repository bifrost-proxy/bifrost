use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

const WAKE_WORKER_SAMPLE_RATE: u32 = 16_000;
const WAKE_WORKER_CHANNELS: u16 = 1;
const WAKE_WORKER_BYTES_PER_SAMPLE: usize = 2;
const WAKE_WORKER_PRE_ROLL_MS: u64 = 1_200;
const WAKE_WORKER_POLL_MS: u64 = 250;
const WAKE_WORKER_RING_BUFFER_MS: u64 = 20_000;
const WAKE_WORKER_MAX_WAKE_WINDOW_MS: u64 = 4_000;

use base64::Engine as _;
use bifrost_asr::wake::{
    keywords_buf_from_phrases, normalize_wake_phrase, wake_phrase_matches, SherpaKwsModelPack,
    StreamingWakeKwsDetector, WakeKwsDetection, DEFAULT_WAKE_KWS_PROFILE,
};
use bifrost_core::{BifrostError, Result};
use serde_json::Value;

use super::asr::AsrTaskClient;

pub(super) fn run_wake_worker(
    client: &AsrTaskClient,
    device: Option<&str>,
    chunk_ms: u64,
    execute: bool,
    engine: &str,
) -> Result<()> {
    let chunk_ms = chunk_ms.clamp(1_000, WAKE_WORKER_MAX_WAKE_WINDOW_MS);
    let capture_window_ms =
        (chunk_ms + WAKE_WORKER_PRE_ROLL_MS).min(WAKE_WORKER_MAX_WAKE_WINDOW_MS);
    let engine = engine.trim().to_ascii_lowercase();
    if engine != "lightweight_kws_listener" {
        return Err(BifrostError::Config(
            "voice wake worker only supports lightweight_kws_listener".to_string(),
        ));
    }
    let selected_device = resolve_wake_worker_audio_device(device);
    post_wake_worker_progress(
        client,
        WakeWorkerProgress {
            status: "worker_started",
            device: Some(&selected_device.input),
            device_label: selected_device.label.as_deref(),
            ..WakeWorkerProgress::default()
        },
    );
    let capture = match ContinuousMicCapture::start(&selected_device.input) {
        Ok(capture) => capture,
        Err(error) => {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "capture_error",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    error: Some(&error.to_string()),
                    ..WakeWorkerProgress::default()
                },
            );
            return Err(error);
        }
    };
    let temp = tempfile::tempdir().map_err(BifrostError::Io)?;
    let chunk_path = temp.path().join("voice-wake-ring-buffer.wav");
    let parent_pid = std::env::var("BIFROST_VOICE_WAKE_PARENT_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0);
    let mut streaming_kws = create_streaming_wake_worker_kws_detector(client)?;
    let mut read_offset = 0_u64;
    loop {
        if !voice_wake_parent_is_running(parent_pid) {
            return Ok(());
        }
        post_wake_worker_progress(
            client,
            WakeWorkerProgress {
                status: "capturing",
                device: Some(&selected_device.input),
                device_label: selected_device.label.as_deref(),
                ..WakeWorkerProgress::default()
            },
        );
        match capture.write_latest_wav(&chunk_path, capture_window_ms) {
            Ok(false) => {
                std::thread::sleep(std::time::Duration::from_millis(WAKE_WORKER_POLL_MS));
                continue;
            }
            Ok(true) => {}
            Err(error) => {
                post_wake_worker_progress(
                    client,
                    WakeWorkerProgress {
                        status: "capture_error",
                        device: Some(&selected_device.input),
                        device_label: selected_device.label.as_deref(),
                        error: Some(&error.to_string()),
                        ..WakeWorkerProgress::default()
                    },
                );
                eprintln!("{error}");
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
        }
        let (new_pcm, next_offset) = capture.read_since(read_offset)?;
        read_offset = next_offset;
        if let Some(error) = capture.last_error() {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "capture_error",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    error: Some(&error),
                    ..WakeWorkerProgress::default()
                },
            );
            eprintln!("{error}");
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        post_wake_worker_progress(
            client,
            WakeWorkerProgress {
                status: "captured",
                device: Some(&selected_device.input),
                device_label: selected_device.label.as_deref(),
                ..WakeWorkerProgress::default()
            },
        );
        if !voice_wake_parent_is_running(parent_pid) {
            return Ok(());
        }
        let candidate = {
            let detection = streaming_kws.accept_pcm16le(&new_pcm, WAKE_WORKER_SAMPLE_RATE as i32);
            match detection
                .as_ref()
                .map(|detection| wake_worker_kws_detection_candidate(client, detection))
                .transpose()
            {
                Ok(candidate) => candidate.flatten(),
                Err(error) => {
                    post_wake_worker_progress(
                        client,
                        WakeWorkerProgress {
                            status: "kws_error",
                            device: Some(&selected_device.input),
                            device_label: selected_device.label.as_deref(),
                            error: Some(&error.to_string()),
                            ..WakeWorkerProgress::default()
                        },
                    );
                    eprintln!("{error}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
            }
        };
        let Some(candidate) = candidate else {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "no_match",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    ..WakeWorkerProgress::default()
                },
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        post_wake_worker_progress(
            client,
            WakeWorkerProgress {
                status: "phrase_matched",
                device: Some(&selected_device.input),
                device_label: selected_device.label.as_deref(),
                phrase: Some(&candidate.phrase),
                profile_id: Some(&candidate.profile_id),
                binding_id: Some(&candidate.binding_id),
                ..WakeWorkerProgress::default()
            },
        );
        let speaker = if candidate.voiceprint_profile_id.is_some() {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "speaker_identifying",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    phrase: Some(&candidate.phrase),
                    profile_id: Some(&candidate.profile_id),
                    binding_id: Some(&candidate.binding_id),
                    ..WakeWorkerProgress::default()
                },
            );
            match identify_wake_worker_speaker(client, &chunk_path) {
                Ok(speaker) => Some(speaker),
                Err(error) => {
                    post_wake_worker_progress(
                        client,
                        WakeWorkerProgress {
                            status: "speaker_error",
                            device: Some(&selected_device.input),
                            device_label: selected_device.label.as_deref(),
                            phrase: Some(&candidate.phrase),
                            profile_id: Some(&candidate.profile_id),
                            binding_id: Some(&candidate.binding_id),
                            error: Some(&error.to_string()),
                            ..WakeWorkerProgress::default()
                        },
                    );
                    eprintln!("{error}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
            }
        } else {
            None
        };
        if wake_worker_candidate_speaker_allowed(&candidate, speaker.as_ref()) {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "speaker_allowed",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    phrase: Some(&candidate.phrase),
                    profile_id: Some(&candidate.profile_id),
                    binding_id: Some(&candidate.binding_id),
                    speaker_profile_id: speaker
                        .as_ref()
                        .and_then(|value| value["profile_id"].as_str()),
                    speaker_confidence: speaker
                        .as_ref()
                        .and_then(|value| value["confidence"].as_f64())
                        .map(|value| value as f32),
                    speaker_status: speaker.as_ref().and_then(|value| value["status"].as_str()),
                    ..WakeWorkerProgress::default()
                },
            );
            let body = serde_json::json!({
                "phrase": candidate.phrase,
                "profile_id": candidate.profile_id,
                "speaker_confidence": speaker.as_ref().and_then(|value| value["confidence"].as_f64()),
                "dry_run": !execute,
            });
            if let Err(error) = client.post_json_body("/voice/wake/trigger", &body) {
                post_wake_worker_progress(
                    client,
                    WakeWorkerProgress {
                        status: "trigger_error",
                        device: Some(&selected_device.input),
                        device_label: selected_device.label.as_deref(),
                        error: Some(&error.to_string()),
                        ..WakeWorkerProgress::default()
                    },
                );
                eprintln!("{error}");
            }
        } else {
            post_wake_worker_progress(
                client,
                WakeWorkerProgress {
                    status: "speaker_rejected",
                    device: Some(&selected_device.input),
                    device_label: selected_device.label.as_deref(),
                    phrase: Some(&candidate.phrase),
                    profile_id: Some(&candidate.profile_id),
                    binding_id: Some(&candidate.binding_id),
                    speaker_profile_id: speaker
                        .as_ref()
                        .and_then(|value| value["profile_id"].as_str()),
                    speaker_confidence: speaker
                        .as_ref()
                        .and_then(|value| value["confidence"].as_f64())
                        .map(|value| value as f32),
                    speaker_status: speaker.as_ref().and_then(|value| value["status"].as_str()),
                    ..WakeWorkerProgress::default()
                },
            );
        }
    }
}

fn voice_wake_parent_is_running(parent_pid: Option<u32>) -> bool {
    let Some(parent_pid) = parent_pid else {
        return true;
    };
    if parent_pid == std::process::id() {
        return true;
    }
    Command::new("kill")
        .arg("-0")
        .arg(parent_pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WakeWorkerAudioDevice {
    input: String,
    label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AvfoundationAudioDevice {
    index: String,
    name: String,
}

#[derive(Default)]
struct WakeWorkerProgress<'a> {
    status: &'a str,
    device: Option<&'a str>,
    device_label: Option<&'a str>,
    transcript: Option<&'a str>,
    phrase: Option<&'a str>,
    profile_id: Option<&'a str>,
    binding_id: Option<&'a str>,
    speaker_profile_id: Option<&'a str>,
    speaker_confidence: Option<f32>,
    speaker_status: Option<&'a str>,
    error: Option<&'a str>,
}

fn post_wake_worker_progress(client: &AsrTaskClient, progress: WakeWorkerProgress<'_>) {
    let body = serde_json::json!({
        "status": progress.status,
        "device": progress.device,
        "device_label": progress.device_label,
        "transcript": progress.transcript,
        "phrase": progress.phrase,
        "profile_id": progress.profile_id,
        "binding_id": progress.binding_id,
        "speaker_profile_id": progress.speaker_profile_id,
        "speaker_confidence": progress.speaker_confidence,
        "speaker_status": progress.speaker_status,
        "error": progress.error,
    });
    if let Err(error) = client.post_json_body("/voice/wake/listener/progress", &body) {
        eprintln!("voice wake worker progress update failed: {error}");
    }
}

fn resolve_wake_worker_audio_device(configured: Option<&str>) -> WakeWorkerAudioDevice {
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let label = list_avfoundation_audio_devices()
            .ok()
            .and_then(|devices| label_for_avfoundation_input(&devices, configured));
        return WakeWorkerAudioDevice {
            input: configured.to_string(),
            label,
        };
    }
    let label = list_avfoundation_audio_devices()
        .ok()
        .and_then(|devices| label_for_avfoundation_input(&devices, ":0"));
    WakeWorkerAudioDevice {
        input: ":0".to_string(),
        label,
    }
}

fn list_avfoundation_audio_devices() -> Result<Vec<AvfoundationAudioDevice>> {
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ])
        .output()
        .map_err(BifrostError::Io)?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let devices = parse_avfoundation_audio_devices(&text);
    if devices.is_empty() {
        Err(BifrostError::Config(
            "ffmpeg did not report any avfoundation audio devices".to_string(),
        ))
    } else {
        Ok(devices)
    }
}

fn label_for_avfoundation_input(
    devices: &[AvfoundationAudioDevice],
    input: &str,
) -> Option<String> {
    let audio_index = input
        .trim()
        .strip_prefix(':')
        .filter(|value| !value.is_empty())?;
    devices
        .iter()
        .find(|device| device.index == audio_index)
        .map(|device| device.name.clone())
}

fn parse_avfoundation_audio_devices(text: &str) -> Vec<AvfoundationAudioDevice> {
    let mut in_audio = false;
    let mut devices = Vec::new();
    for line in text.lines() {
        if line.contains("AVFoundation audio devices:") {
            in_audio = true;
            continue;
        }
        if line.contains("AVFoundation video devices:") {
            in_audio = false;
            continue;
        }
        if !in_audio {
            continue;
        }
        let Some((_, rest)) = line.split_once("] [") else {
            continue;
        };
        let Some((index, name)) = rest.split_once("] ") else {
            continue;
        };
        let index = index.trim();
        let name = name.trim();
        if !index.is_empty() && !name.is_empty() {
            devices.push(AvfoundationAudioDevice {
                index: index.to_string(),
                name: name.to_string(),
            });
        }
    }
    devices
}

struct ContinuousMicCapture {
    child: Child,
    state: Arc<Mutex<ContinuousMicCaptureState>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct ContinuousMicCaptureState {
    bytes: VecDeque<u8>,
    total_bytes: u64,
    last_error: Option<String>,
}

impl ContinuousMicCapture {
    fn start(device: &str) -> Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "avfoundation"])
            .arg("-i")
            .arg(device)
            .args(["-ar", "16000", "-ac", "1", "-acodec", "pcm_s16le"])
            .args(["-f", "s16le", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(BifrostError::Io)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BifrostError::Config("voice wake worker failed to open ffmpeg stdout".to_string())
        })?;
        let state = Arc::new(Mutex::new(ContinuousMicCaptureState::default()));
        let reader_state = Arc::clone(&state);
        let reader = std::thread::spawn(move || {
            read_continuous_mic_stdout(stdout, reader_state);
        });
        Ok(Self {
            child,
            state,
            reader: Some(reader),
        })
    }

    fn write_latest_wav(&self, path: &Path, window_ms: u64) -> Result<bool> {
        let max_bytes = pcm_bytes_for_ms(window_ms);
        let bytes = {
            let state = self.state.lock().map_err(|_| {
                BifrostError::Config("voice wake worker capture buffer lock poisoned".to_string())
            })?;
            if state.bytes.len() < pcm_bytes_for_ms(600) {
                return Ok(false);
            }
            let start = state.bytes.len().saturating_sub(max_bytes);
            state.bytes.iter().skip(start).copied().collect::<Vec<_>>()
        };
        write_pcm16_wav(path, &bytes)?;
        Ok(true)
    }

    fn read_since(&self, offset: u64) -> Result<(Vec<u8>, u64)> {
        let state = self.state.lock().map_err(|_| {
            BifrostError::Config("voice wake worker capture buffer lock poisoned".to_string())
        })?;
        let retained_start = state.total_bytes.saturating_sub(state.bytes.len() as u64);
        let start_offset = offset.max(retained_start);
        let skip = start_offset.saturating_sub(retained_start) as usize;
        let bytes = state.bytes.iter().skip(skip).copied().collect::<Vec<_>>();
        Ok((bytes, state.total_bytes))
    }

    fn last_error(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.last_error.clone())
    }
}

impl Drop for ContinuousMicCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn read_continuous_mic_stdout(
    mut stdout: std::process::ChildStdout,
    state: Arc<Mutex<ContinuousMicCaptureState>>,
) {
    let max_bytes = pcm_bytes_for_ms(WAKE_WORKER_RING_BUFFER_MS);
    let mut buffer = [0_u8; 8192];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => {
                if let Ok(mut state) = state.lock() {
                    state.last_error =
                        Some("voice wake worker microphone stream ended".to_string());
                }
                return;
            }
            Ok(n) => {
                if let Ok(mut state) = state.lock() {
                    state.bytes.extend(&buffer[..n]);
                    state.total_bytes = state.total_bytes.saturating_add(n as u64);
                    while state.bytes.len() > max_bytes {
                        state.bytes.pop_front();
                    }
                } else {
                    return;
                }
            }
            Err(error) => {
                if let Ok(mut state) = state.lock() {
                    state.last_error = Some(format!(
                        "voice wake worker microphone stream failed: {error}"
                    ));
                }
                return;
            }
        }
    }
}

fn pcm_bytes_for_ms(ms: u64) -> usize {
    ((WAKE_WORKER_SAMPLE_RATE as u64
        * WAKE_WORKER_CHANNELS as u64
        * WAKE_WORKER_BYTES_PER_SAMPLE as u64
        * ms)
        / 1000) as usize
}

fn write_pcm16_wav(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path).map_err(BifrostError::Io)?;
    let data_len = bytes.len() as u32;
    let byte_rate =
        WAKE_WORKER_SAMPLE_RATE * WAKE_WORKER_CHANNELS as u32 * WAKE_WORKER_BYTES_PER_SAMPLE as u32;
    let block_align = WAKE_WORKER_CHANNELS * WAKE_WORKER_BYTES_PER_SAMPLE as u16;
    file.write_all(b"RIFF").map_err(BifrostError::Io)?;
    file.write_all(&(36_u32 + data_len).to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(b"WAVEfmt ").map_err(BifrostError::Io)?;
    file.write_all(&16_u32.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&1_u16.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&WAKE_WORKER_CHANNELS.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&WAKE_WORKER_SAMPLE_RATE.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&byte_rate.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&block_align.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(&16_u16.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(b"data").map_err(BifrostError::Io)?;
    file.write_all(&data_len.to_le_bytes())
        .map_err(BifrostError::Io)?;
    file.write_all(bytes).map_err(BifrostError::Io)?;
    Ok(())
}

fn identify_wake_worker_speaker(client: &AsrTaskClient, wav_path: &Path) -> Result<Value> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(wav_path)
        .args(["-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"])
        .output()
        .map_err(BifrostError::Io)?;
    if !output.status.success() {
        return Err(BifrostError::Config(format!(
            "voice wake worker speaker audio normalization failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let body = serde_json::json!({
        "pcm16le_base64": base64::engine::general_purpose::STANDARD.encode(output.stdout),
        "sample_rate": 16000,
        "channels": 1,
    });
    client.post_json_body("/asr/speaker-profiles/identify", &body)
}

#[derive(Debug)]
struct WakeWorkerCandidate {
    phrase: String,
    binding_id: String,
    profile_id: String,
    voiceprint_profile_id: Option<String>,
    speaker_threshold: f32,
}

fn wake_worker_kws_root() -> std::path::PathBuf {
    bifrost_storage::data_dir()
        .join("asr")
        .join("wake")
        .join("kws")
        .join(DEFAULT_WAKE_KWS_PROFILE)
}

fn create_streaming_wake_worker_kws_detector(
    client: &AsrTaskClient,
) -> Result<StreamingWakeKwsDetector> {
    let bindings = client.get_json("/voice/wake/bindings")?["bindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let enabled_phrases = bindings
        .iter()
        .filter(|binding| binding["enabled"].as_bool().unwrap_or(true))
        .filter_map(|binding| binding["phrase"].as_str().map(ToString::to_string))
        .collect::<Vec<_>>();
    let keywords_buf = keywords_buf_from_phrases(&enabled_phrases).map_err(BifrostError::Config)?;
    let score = bindings
        .iter()
        .filter_map(|binding| binding["kws_score"].as_f64())
        .fold(1.5_f64, f64::max) as f32;
    let threshold = bindings
        .iter()
        .filter_map(|binding| binding["kws_threshold"].as_f64())
        .fold(0.35_f64, f64::min) as f32;
    let pack = SherpaKwsModelPack::for_root(wake_worker_kws_root());
    StreamingWakeKwsDetector::new(&pack, &keywords_buf, score, threshold)
        .map_err(BifrostError::Config)
}

fn wake_worker_kws_detection_candidate(
    client: &AsrTaskClient,
    detection: &WakeKwsDetection,
) -> Result<Option<WakeWorkerCandidate>> {
    let profiles = client.get_json("/voice/wake/profiles")?["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let bindings = client.get_json("/voice/wake/bindings")?["bindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    wake_worker_match_binding(&profiles, &bindings, &detection.keyword)
}

fn wake_worker_match_binding(
    profiles: &[Value],
    bindings: &[Value],
    normalized_transcript: &str,
) -> Result<Option<WakeWorkerCandidate>> {
    for binding in bindings {
        if !binding["enabled"].as_bool().unwrap_or(true) {
            continue;
        }
        let phrase = binding["phrase"].as_str().unwrap_or("");
        if phrase.is_empty()
            || !wake_phrase_matches(normalized_transcript, &normalize_wake_phrase(phrase))
        {
            continue;
        }
        let profile_id = binding["profile_id"].as_str().unwrap_or("");
        let Some(profile) = profiles
            .iter()
            .find(|profile| profile["id"].as_str() == Some(profile_id))
        else {
            continue;
        };
        let threshold = binding["speaker_threshold"]
            .as_f64()
            .or_else(|| profile["speaker_threshold"].as_f64())
            .unwrap_or(0.72) as f32;
        return Ok(Some(WakeWorkerCandidate {
            phrase: phrase.to_string(),
            binding_id: binding["id"].as_str().unwrap_or("").to_string(),
            profile_id: profile_id.to_string(),
            voiceprint_profile_id: profile["voiceprint_profile_id"]
                .as_str()
                .map(ToString::to_string),
            speaker_threshold: threshold,
        }));
    }
    Ok(None)
}

fn wake_worker_candidate_speaker_allowed(
    candidate: &WakeWorkerCandidate,
    speaker: Option<&Value>,
) -> bool {
    let Some(expected_profile_id) = candidate.voiceprint_profile_id.as_deref() else {
        return true;
    };
    let Some(speaker) = speaker else {
        return false;
    };
    speaker["matched"].as_bool().unwrap_or(false)
        && speaker["profile_id"].as_str() == Some(expected_profile_id)
        && speaker["confidence"].as_f64().unwrap_or(0.0) as f32 >= candidate.speaker_threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_wake_worker_parent_guard_allows_current_process() {
        assert!(voice_wake_parent_is_running(None));
        assert!(voice_wake_parent_is_running(Some(std::process::id())));
    }

    #[test]
    fn wake_worker_phrase_match_collapses_repeated_wake_word() {
        assert!(wake_phrase_matches(
            &normalize_wake_phrase("哈喽"),
            &normalize_wake_phrase("哈喽哈喽。")
        ));
        assert!(!wake_phrase_matches(
            &normalize_wake_phrase("打开"),
            &normalize_wake_phrase("打开录音")
        ));
    }

    #[test]
    fn parse_avfoundation_audio_devices_reads_audio_section() {
        let output = r#"
[AVFoundation indev @ 0x879414140] AVFoundation video devices:
[AVFoundation indev @ 0x879414140] [0] FaceTime高清相机
[AVFoundation indev @ 0x879414140] AVFoundation audio devices:
[AVFoundation indev @ 0x879414140] [0] Wireless Mic Rx
[AVFoundation indev @ 0x879414140] [1] BlackHole 2ch
[AVFoundation indev @ 0x879414140] [2] MacBook Pro麦克风
"#;
        let devices = parse_avfoundation_audio_devices(output);

        assert_eq!(
            devices,
            vec![
                AvfoundationAudioDevice {
                    index: "0".to_string(),
                    name: "Wireless Mic Rx".to_string()
                },
                AvfoundationAudioDevice {
                    index: "1".to_string(),
                    name: "BlackHole 2ch".to_string()
                },
                AvfoundationAudioDevice {
                    index: "2".to_string(),
                    name: "MacBook Pro麦克风".to_string()
                }
            ]
        );
    }

    #[test]
    fn default_wake_worker_audio_device_uses_first_avfoundation_audio_input() {
        let selected = resolve_wake_worker_audio_device(None);

        assert_eq!(selected.input, ":0");
    }

    #[test]
    fn label_for_avfoundation_input_maps_explicit_audio_index() {
        let devices = vec![
            AvfoundationAudioDevice {
                index: "0".to_string(),
                name: "Wireless Mic Rx".to_string(),
            },
            AvfoundationAudioDevice {
                index: "1".to_string(),
                name: "BlackHole 2ch".to_string(),
            },
            AvfoundationAudioDevice {
                index: "2".to_string(),
                name: "MacBook Pro麦克风".to_string(),
            },
        ];

        assert_eq!(
            label_for_avfoundation_input(&devices, ":0").as_deref(),
            Some("Wireless Mic Rx")
        );
        assert_eq!(
            label_for_avfoundation_input(&devices, ":2").as_deref(),
            Some("MacBook Pro麦克风")
        );
        assert!(label_for_avfoundation_input(&devices, "Wireless Mic Rx").is_none());
    }

    #[test]
    fn pcm_bytes_for_ms_uses_16khz_mono_pcm16() {
        assert_eq!(pcm_bytes_for_ms(1_000), 32_000);
        assert_eq!(pcm_bytes_for_ms(4_000), 128_000);
    }

    #[test]
    fn write_pcm16_wav_writes_standard_header() {
        let temp = tempfile::tempdir().expect("tempdir");
        let wav_path = temp.path().join("sample.wav");
        write_pcm16_wav(&wav_path, &[0, 1, 2, 3]).expect("write wav");
        let bytes = std::fs::read(&wav_path).expect("read wav");

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 4);
        assert_eq!(&bytes[44..], &[0, 1, 2, 3]);
    }

    #[test]
    fn wake_worker_match_binding_selects_first_matching_enabled_binding() {
        let profiles = serde_json::json!([
            {
                "id": "p1",
                "voiceprint_profile_id": "vp-1",
                "speaker_threshold": 0.5
            }
        ]);
        let bindings = serde_json::json!([
            {
                "id": "b1",
                "enabled": true,
                "phrase": "hey bifrost",
                "profile_id": "p1",
                "speaker_threshold": 0.7
            },
            {
                "id": "b2",
                "enabled": false,
                "phrase": "hey bifrost",
                "profile_id": "p1"
            }
        ]);

        let candidate = wake_worker_match_binding(
            profiles.as_array().unwrap(),
            bindings.as_array().unwrap(),
            &normalize_wake_phrase("hey bifrost"),
        )
        .expect("match ok")
        .expect("candidate expected");

        assert_eq!(candidate.phrase, "hey bifrost");
        assert_eq!(candidate.profile_id, "p1");
        assert_eq!(candidate.binding_id, "b1");
        // speaker_threshold comes from binding when present
        assert!((candidate.speaker_threshold - 0.7).abs() < 1e-6);
    }

    #[test]
    fn wake_worker_match_binding_returns_none_when_no_binding_matches() {
        let profiles = serde_json::json!([
            {"id": "p1", "voiceprint_profile_id": "vp-1"}
        ]);
        let bindings = serde_json::json!([
            {
                "id": "b1",
                "enabled": true,
                "phrase": "different phrase",
                "profile_id": "p1"
            }
        ]);

        let candidate = wake_worker_match_binding(
            profiles.as_array().unwrap(),
            bindings.as_array().unwrap(),
            &normalize_wake_phrase("hey bifrost"),
        )
        .expect("match ok");

        assert!(candidate.is_none());
    }

    #[test]
    fn wake_worker_candidate_speaker_allowed_applies_profile_and_threshold() {
        let candidate = WakeWorkerCandidate {
            phrase: "hey".to_string(),
            binding_id: "b1".to_string(),
            profile_id: "p1".to_string(),
            voiceprint_profile_id: Some("vp-1".to_string()),
            speaker_threshold: 0.75,
        };

        // Missing speaker → rejected
        assert!(!wake_worker_candidate_speaker_allowed(&candidate, None));

        // Mismatched profile id → rejected
        let speaker = serde_json::json!({
            "matched": true,
            "profile_id": "vp-2",
            "confidence": 0.9
        });
        assert!(!wake_worker_candidate_speaker_allowed(
            &candidate,
            Some(&speaker)
        ));

        // Low confidence → rejected
        let speaker = serde_json::json!({
            "matched": true,
            "profile_id": "vp-1",
            "confidence": 0.5
        });
        assert!(!wake_worker_candidate_speaker_allowed(
            &candidate,
            Some(&speaker)
        ));

        // All conditions satisfied → allowed
        let speaker = serde_json::json!({
            "matched": true,
            "profile_id": "vp-1",
            "confidence": 0.9
        });
        assert!(wake_worker_candidate_speaker_allowed(
            &candidate,
            Some(&speaker)
        ));

        // Candidate without voiceprint profile always allowed
        let candidate = WakeWorkerCandidate {
            voiceprint_profile_id: None,
            ..candidate
        };
        assert!(wake_worker_candidate_speaker_allowed(&candidate, None));
    }
}
