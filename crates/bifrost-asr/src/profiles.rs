use serde::{Deserialize, Serialize};

pub const DEFAULT_DIARIZATION_PROFILE: &str = "sherpa-onnx-balanced";

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

pub fn default_diarization_profile() -> String {
    DEFAULT_DIARIZATION_PROFILE.to_string()
}
