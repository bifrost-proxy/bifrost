use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use bifrost_asr::wake::{
    detect_sherpa_kws_in_wav, keywords_buf_from_phrases, normalize_wake_phrase,
    wake_phrase_matches, SherpaKwsModelPack, DEFAULT_WAKE_KWS_PROFILE,
};
use bifrost_core::{BifrostError, Result};
use serde_json::Value;

use super::asr::AsrTaskClient;
use super::voice::recognize_wake_phrase_from_audio;

pub(super) fn run_wake_worker(
    client: &AsrTaskClient,
    device: Option<&str>,
    chunk_ms: u64,
    execute: bool,
    engine: &str,
) -> Result<()> {
    let chunk_ms = chunk_ms.clamp(1_000, 10_000);
    let engine = engine.trim().to_ascii_lowercase();
    let parent_pid = std::env::var("BIFROST_VOICE_WAKE_PARENT_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0);
    loop {
        if !voice_wake_parent_is_running(parent_pid) {
            return Ok(());
        }
        let temp = tempfile::tempdir().map_err(BifrostError::Io)?;
        let chunk_path = temp.path().join("voice-wake-chunk.wav");
        if let Err(error) = capture_wake_worker_mic_chunk(&chunk_path, chunk_ms, device) {
            eprintln!("{error}");
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        if !voice_wake_parent_is_running(parent_pid) {
            return Ok(());
        }
        let candidate = if engine == "lightweight_kws_listener" {
            match wake_worker_kws_candidate(client, &chunk_path) {
                Ok(candidate) => candidate,
                Err(error) => {
                    eprintln!("{error}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
            }
        } else {
            let transcript = match recognize_wake_phrase_from_audio(
                client,
                &chunk_path,
                "Qwen3-ASR-0.6B",
                "chinese",
            ) {
                Ok(transcript) => transcript,
                Err(error) => {
                    eprintln!("{error}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
            };
            if transcript.trim().is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            wake_worker_text_candidate(client, &transcript)?
        };
        let Some(candidate) = candidate else {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        let speaker = if candidate.voiceprint_profile_id.is_some() {
            match identify_wake_worker_speaker(client, &chunk_path) {
                Ok(speaker) => Some(speaker),
                Err(error) => {
                    eprintln!("{error}");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
            }
        } else {
            None
        };
        if wake_worker_candidate_speaker_allowed(&candidate, speaker.as_ref()) {
            let body = serde_json::json!({
                "phrase": candidate.phrase,
                "profile_id": candidate.profile_id,
                "speaker_confidence": speaker.as_ref().and_then(|value| value["confidence"].as_f64()),
                "dry_run": !execute || candidate.voiceprint_profile_id.is_none(),
            });
            if let Err(error) = client.post_json_body("/voice/wake/trigger", &body) {
                eprintln!("{error}");
            }
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

fn capture_wake_worker_mic_chunk(path: &Path, chunk_ms: u64, device: Option<&str>) -> Result<()> {
    let duration = format!("{:.3}", chunk_ms as f64 / 1000.0);
    let device = device
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(":0");
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "avfoundation"])
        .arg("-i")
        .arg(device)
        .arg("-t")
        .arg(duration)
        .args(["-ar", "16000", "-ac", "1", "-acodec", "pcm_s16le"])
        .arg(path)
        .output()
        .map_err(BifrostError::Io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "voice wake worker microphone capture failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
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

fn wake_worker_kws_candidate(
    client: &AsrTaskClient,
    chunk_path: &Path,
) -> Result<Option<WakeWorkerCandidate>> {
    let profiles = client.get_json("/voice/wake/profiles")?["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();
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
    let Some(detection) =
        detect_sherpa_kws_in_wav(&pack, chunk_path, &keywords_buf, score, threshold)
            .map_err(BifrostError::Config)?
    else {
        return Ok(None);
    };
    wake_worker_match_binding(&profiles, &bindings, &detection.keyword)
}

fn wake_worker_text_candidate(
    client: &AsrTaskClient,
    transcript: &str,
) -> Result<Option<WakeWorkerCandidate>> {
    let profiles = client.get_json("/voice/wake/profiles")?["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let bindings = client.get_json("/voice/wake/bindings")?["bindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    wake_worker_match_binding(&profiles, &bindings, &normalize_wake_phrase(transcript))
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
}
