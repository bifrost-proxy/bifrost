use serde::{Deserialize, Serialize};

pub const DEFAULT_DIARIZATION_PROFILE: &str = "sherpa-onnx-balanced";
pub const REALTIME_DICTATION_PROFILE: &str = "realtime-dictation-local";
pub const REALTIME_WAKE_PROFILE: &str = "realtime-wake-local";
pub const OFFLINE_PLAIN_ASR_PROFILE: &str = "offline-plain-asr-local";
pub const OFFLINE_SPEAKER_SUBTITLE_PROFILE: &str = "offline-speaker-subtitle-local";
pub const SCHEDULED_SPEAKER_SUBTITLE_PROFILE: &str = "scheduled-speaker-subtitle-local";
pub const REALTIME_ASR_MODEL: &str = "Qwen3-ASR-0.6B";
pub const OFFLINE_ASR_MODEL: &str = REALTIME_ASR_MODEL;
pub const DEFAULT_AUTO_MAX_SPEAKERS: u8 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrDiarizationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_diarization_profile")]
    pub profile: String,
    #[serde(default)]
    pub min_speakers: Option<u8>,
    #[serde(default)]
    pub max_speakers: Option<u8>,
    #[serde(default)]
    pub known_speaker_count: Option<u8>,
    #[serde(default)]
    pub voiceprint_matching: bool,
}

impl Default for AsrDiarizationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: default_diarization_profile(),
            min_speakers: None,
            max_speakers: None,
            known_speaker_count: None,
            voiceprint_matching: false,
        }
    }
}

impl AsrDiarizationConfig {
    pub fn speaker_aware_default() -> Self {
        Self {
            enabled: true,
            profile: default_diarization_profile(),
            min_speakers: None,
            max_speakers: Some(DEFAULT_AUTO_MAX_SPEAKERS),
            known_speaker_count: None,
            voiceprint_matching: true,
        }
    }
}

pub fn default_diarization_profile() -> String {
    DEFAULT_DIARIZATION_PROFILE.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechMode {
    RealtimeDictation,
    RealtimeWake,
    OfflineFile,
    DirectoryTask,
}

impl SpeechMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealtimeDictation => "realtime_dictation",
            Self::RealtimeWake => "realtime_wake",
            Self::OfflineFile => "offline_file",
            Self::DirectoryTask => "directory_task",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "realtime" | "realtime_dictation" => Some(Self::RealtimeDictation),
            "realtime_wake" | "wake" => Some(Self::RealtimeWake),
            "offline" | "offline_file" => Some(Self::OfflineFile),
            "directory" | "directory_task" | "scheduled" => Some(Self::DirectoryTask),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechPipelineProfile {
    pub id: String,
    pub label: String,
    pub modes: Vec<SpeechMode>,
    pub realtime: Option<RealtimeProfile>,
    pub offline: Option<OfflineProfile>,
    pub resources: ResourcePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeProfile {
    pub input: String,
    pub provider: String,
    pub model: String,
    pub wake_engine: Option<String>,
    pub vad: Option<String>,
    pub endpoint_silence_ms: Option<u64>,
    pub max_utterance_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineProfile {
    pub asr_provider: String,
    pub model: String,
    pub fallback_model: Option<String>,
    pub diarization: Option<AsrDiarizationConfig>,
    pub subtitle_formats: Vec<String>,
    pub reuse_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePolicy {
    pub priority: u8,
    pub preemptible: bool,
    pub pause_on_realtime_voice: bool,
    pub max_concurrent_files: u8,
}

pub fn builtin_speech_pipeline_profiles() -> Vec<SpeechPipelineProfile> {
    vec![
        SpeechPipelineProfile {
            id: REALTIME_DICTATION_PROFILE.to_string(),
            label: "Realtime dictation".to_string(),
            modes: vec![SpeechMode::RealtimeDictation],
            realtime: Some(RealtimeProfile {
                input: "pcm16_16k_mono".to_string(),
                provider: "qwen3_stateful_streaming".to_string(),
                model: REALTIME_ASR_MODEL.to_string(),
                wake_engine: None,
                vad: Some("rms-vad-v1".to_string()),
                endpoint_silence_ms: Some(500),
                max_utterance_ms: Some(30_000),
            }),
            offline: None,
            resources: ResourcePolicy {
                priority: 100,
                preemptible: false,
                pause_on_realtime_voice: false,
                max_concurrent_files: 1,
            },
        },
        SpeechPipelineProfile {
            id: REALTIME_WAKE_PROFILE.to_string(),
            label: "Realtime wake listener".to_string(),
            modes: vec![SpeechMode::RealtimeWake],
            realtime: Some(RealtimeProfile {
                input: "mic".to_string(),
                provider: "lightweight_kws_listener".to_string(),
                model: REALTIME_ASR_MODEL.to_string(),
                wake_engine: Some("sherpa-onnx-kws".to_string()),
                vad: Some("sherpa-vad".to_string()),
                endpoint_silence_ms: None,
                max_utterance_ms: Some(2_500),
            }),
            offline: None,
            resources: ResourcePolicy {
                priority: 90,
                preemptible: false,
                pause_on_realtime_voice: false,
                max_concurrent_files: 1,
            },
        },
        SpeechPipelineProfile {
            id: OFFLINE_PLAIN_ASR_PROFILE.to_string(),
            label: "Offline plain ASR".to_string(),
            modes: vec![SpeechMode::OfflineFile, SpeechMode::DirectoryTask],
            realtime: None,
            offline: Some(OfflineProfile {
                asr_provider: "qwen3-offline".to_string(),
                model: OFFLINE_ASR_MODEL.to_string(),
                fallback_model: Some(REALTIME_ASR_MODEL.to_string()),
                diarization: None,
                subtitle_formats: vec![
                    "txt".to_string(),
                    "timeline_json".to_string(),
                    "metadata_json".to_string(),
                    "srt".to_string(),
                    "vtt".to_string(),
                ],
                reuse_strategy: "reuse_per_file".to_string(),
            }),
            resources: ResourcePolicy {
                priority: 70,
                preemptible: true,
                pause_on_realtime_voice: true,
                max_concurrent_files: 1,
            },
        },
        SpeechPipelineProfile {
            id: OFFLINE_SPEAKER_SUBTITLE_PROFILE.to_string(),
            label: "Offline speaker subtitle".to_string(),
            modes: vec![SpeechMode::OfflineFile, SpeechMode::DirectoryTask],
            realtime: None,
            offline: Some(OfflineProfile {
                asr_provider: "qwen3-offline".to_string(),
                model: OFFLINE_ASR_MODEL.to_string(),
                fallback_model: Some(REALTIME_ASR_MODEL.to_string()),
                diarization: Some(AsrDiarizationConfig::speaker_aware_default()),
                subtitle_formats: vec![
                    "txt".to_string(),
                    "timeline_json".to_string(),
                    "diarization_json".to_string(),
                    "metadata_json".to_string(),
                    "srt".to_string(),
                    "vtt".to_string(),
                ],
                reuse_strategy: "reuse_per_file".to_string(),
            }),
            resources: ResourcePolicy {
                priority: 70,
                preemptible: true,
                pause_on_realtime_voice: true,
                max_concurrent_files: 1,
            },
        },
        SpeechPipelineProfile {
            id: SCHEDULED_SPEAKER_SUBTITLE_PROFILE.to_string(),
            label: "Scheduled speaker subtitle".to_string(),
            modes: vec![SpeechMode::DirectoryTask],
            realtime: None,
            offline: Some(OfflineProfile {
                asr_provider: "qwen3-offline".to_string(),
                model: OFFLINE_ASR_MODEL.to_string(),
                fallback_model: Some(REALTIME_ASR_MODEL.to_string()),
                diarization: Some(AsrDiarizationConfig::speaker_aware_default()),
                subtitle_formats: vec![
                    "txt".to_string(),
                    "timeline_json".to_string(),
                    "diarization_json".to_string(),
                    "metadata_json".to_string(),
                    "srt".to_string(),
                    "vtt".to_string(),
                ],
                reuse_strategy: "reuse_per_file".to_string(),
            }),
            resources: ResourcePolicy {
                priority: 20,
                preemptible: true,
                pause_on_realtime_voice: true,
                max_concurrent_files: 1,
            },
        },
    ]
}

pub fn builtin_profile(id: &str) -> Option<SpeechPipelineProfile> {
    builtin_speech_pipeline_profiles()
        .into_iter()
        .find(|profile| profile.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_aware_default_enables_diarization_and_voiceprints() {
        let config = AsrDiarizationConfig::speaker_aware_default();

        assert!(config.enabled);
        assert!(config.voiceprint_matching);
        assert_eq!(config.profile, DEFAULT_DIARIZATION_PROFILE);
        assert_eq!(config.max_speakers, Some(DEFAULT_AUTO_MAX_SPEAKERS));
    }

    #[test]
    fn offline_speaker_profiles_use_0_6b_and_voiceprint_matching() {
        for profile_id in [
            OFFLINE_SPEAKER_SUBTITLE_PROFILE,
            SCHEDULED_SPEAKER_SUBTITLE_PROFILE,
        ] {
            let profile = builtin_profile(profile_id).expect("profile exists");
            let offline = profile.offline.expect("offline profile");
            let diarization = offline.diarization.expect("diarization profile");

            assert_eq!(offline.model, "Qwen3-ASR-0.6B");
            assert!(diarization.enabled);
            assert!(diarization.voiceprint_matching);
        }
    }
}
