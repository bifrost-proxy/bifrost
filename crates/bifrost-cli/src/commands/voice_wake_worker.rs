use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use bifrost_core::{BifrostError, Result};
use serde_json::Value;

use super::asr::AsrTaskClient;
use super::voice::recognize_wake_phrase_from_audio;

pub(super) fn run_wake_worker(
    client: &AsrTaskClient,
    device: Option<&str>,
    chunk_ms: u64,
    execute: bool,
) -> Result<()> {
    let chunk_ms = chunk_ms.clamp(1_000, 10_000);
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
        let transcript = match recognize_wake_phrase_from_audio(
            client,
            &chunk_path,
            "Qwen3-ASR-1.7B",
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
        let speaker = match identify_wake_worker_speaker(client, &chunk_path) {
            Ok(speaker) => speaker,
            Err(error) => {
                eprintln!("{error}");
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
        };
        if let Some(candidate) = wake_worker_match_candidate(client, &transcript, &speaker)? {
            let body = serde_json::json!({
                "phrase": candidate.phrase,
                "profile_id": candidate.profile_id,
                "speaker_confidence": speaker["confidence"].as_f64(),
                "dry_run": !execute,
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
}

fn wake_worker_match_candidate(
    client: &AsrTaskClient,
    transcript: &str,
    speaker: &Value,
) -> Result<Option<WakeWorkerCandidate>> {
    if !speaker["matched"].as_bool().unwrap_or(false) {
        return Ok(None);
    }
    let Some(speaker_profile_id) = speaker["profile_id"].as_str() else {
        return Ok(None);
    };
    let confidence = speaker["confidence"].as_f64().unwrap_or(0.0) as f32;
    let profiles = client.get_json("/voice/wake/profiles")?["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let bindings = client.get_json("/voice/wake/bindings")?["bindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let normalized_transcript = normalize_wake_worker_phrase(transcript);
    for binding in bindings {
        if !binding["enabled"].as_bool().unwrap_or(true) {
            continue;
        }
        let phrase = binding["phrase"].as_str().unwrap_or("");
        if phrase.is_empty()
            || !normalized_transcript.contains(&normalize_wake_worker_phrase(phrase))
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
        if profile["voiceprint_profile_id"].as_str() != Some(speaker_profile_id) {
            continue;
        }
        let threshold = binding["speaker_threshold"]
            .as_f64()
            .or_else(|| profile["speaker_threshold"].as_f64())
            .unwrap_or(0.72) as f32;
        if confidence >= threshold {
            return Ok(Some(WakeWorkerCandidate {
                phrase: phrase.to_string(),
                profile_id: profile_id.to_string(),
            }));
        }
    }
    Ok(None)
}

fn normalize_wake_worker_phrase(value: &str) -> String {
    value.to_lowercase().split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_wake_worker_parent_guard_allows_current_process() {
        assert!(voice_wake_parent_is_running(None));
        assert!(voice_wake_parent_is_running(Some(std::process::id())));
    }
}
