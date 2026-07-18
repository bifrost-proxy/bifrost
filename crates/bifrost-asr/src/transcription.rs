use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default)]
    pub overlap: bool,
}

impl TranscriptionSegment {
    pub fn normalized_speaker(&self) -> Option<&str> {
        self.speaker
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionFinishReason {
    Completed,
    Length,
    Cancelled,
    Failed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredTranscription {
    pub text: String,
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,
    #[serde(default)]
    pub finish_reason: TranscriptionFinishReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TranscriptionUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionCompleteness {
    Complete,
    Incomplete,
    Unknown,
}

/// Assess whether generation covered all expected speech.
///
/// `expected_speech_end_ms` must come from speech-aware metadata such as VAD or
/// an existing timeline. Media duration alone is deliberately not accepted:
/// trailing silence is common in meeting recordings and is not truncation.
pub fn assess_completeness(
    transcription: &StructuredTranscription,
    expected_speech_end_ms: Option<u64>,
    tolerance_ms: u64,
) -> TranscriptionCompleteness {
    match transcription.finish_reason {
        TranscriptionFinishReason::Length
        | TranscriptionFinishReason::Cancelled
        | TranscriptionFinishReason::Failed => return TranscriptionCompleteness::Incomplete,
        TranscriptionFinishReason::Completed | TranscriptionFinishReason::Unknown => {}
    }

    let Some(expected_end_ms) = expected_speech_end_ms else {
        return match transcription.finish_reason {
            TranscriptionFinishReason::Completed => TranscriptionCompleteness::Complete,
            _ => TranscriptionCompleteness::Unknown,
        };
    };
    let Some(actual_end_ms) = transcription
        .segments
        .iter()
        .map(|segment| segment.end_ms)
        .max()
    else {
        return if expected_end_ms <= tolerance_ms && transcription.text.trim().is_empty() {
            TranscriptionCompleteness::Complete
        } else {
            TranscriptionCompleteness::Incomplete
        };
    };

    if actual_end_ms.saturating_add(tolerance_ms) >= expected_end_ms {
        TranscriptionCompleteness::Complete
    } else {
        TranscriptionCompleteness::Incomplete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptionCapabilities {
    pub provider: &'static str,
    pub realtime: bool,
    pub native_speakers: bool,
    pub prompt: bool,
    /// The default prompt contains machine-readable output rules and must be
    /// extended instead of replaced by user context.
    pub protocol_prompt_required: bool,
    pub structured_timestamps: bool,
}

pub const TRANSCRIPTION_PROVIDERS: &[TranscriptionCapabilities] = &[
    TranscriptionCapabilities {
        provider: "qwen-openai",
        realtime: true,
        native_speakers: false,
        prompt: false,
        protocol_prompt_required: false,
        structured_timestamps: true,
    },
    TranscriptionCapabilities {
        provider: "moss-mlx",
        realtime: false,
        native_speakers: true,
        prompt: true,
        protocol_prompt_required: true,
        structured_timestamps: true,
    },
    TranscriptionCapabilities {
        provider: "moss-cpp",
        realtime: false,
        native_speakers: true,
        prompt: true,
        protocol_prompt_required: true,
        structured_timestamps: true,
    },
];

pub fn transcription_provider(provider: &str) -> Option<&'static TranscriptionCapabilities> {
    TRANSCRIPTION_PROVIDERS
        .iter()
        .find(|capabilities| capabilities.provider == provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcription(
        finish_reason: TranscriptionFinishReason,
        end_ms: u64,
    ) -> StructuredTranscription {
        StructuredTranscription {
            text: "hello".to_string(),
            segments: vec![TranscriptionSegment {
                start_ms: 0,
                end_ms,
                text: "hello".to_string(),
                speaker: Some(" speaker_00 ".to_string()),
                overlap: false,
            }],
            finish_reason,
            usage: None,
        }
    }

    #[test]
    fn provider_registry_distinguishes_realtime_and_joint_transcription() {
        let qwen = transcription_provider("qwen-openai").unwrap();
        assert!(qwen.realtime);
        assert!(!qwen.native_speakers);

        let moss = transcription_provider("moss-mlx").unwrap();
        assert!(!moss.realtime);
        assert!(moss.native_speakers);
        assert!(moss.prompt);
        assert!(moss.protocol_prompt_required);
        let moss_cpp = transcription_provider("moss-cpp").unwrap();
        assert!(moss_cpp.native_speakers);
        assert!(moss_cpp.prompt);
        assert!(moss_cpp.protocol_prompt_required);
        assert!(transcription_provider("missing").is_none());
    }

    #[test]
    fn trailing_silence_is_not_mistaken_for_truncation() {
        let result = transcription(TranscriptionFinishReason::Unknown, 1_706_785);
        assert_eq!(
            assess_completeness(&result, Some(1_706_785), 1_000),
            TranscriptionCompleteness::Complete
        );
        assert_eq!(
            assess_completeness(&result, None, 1_000),
            TranscriptionCompleteness::Unknown
        );
    }

    #[test]
    fn explicit_length_cancel_and_failure_are_incomplete() {
        for reason in [
            TranscriptionFinishReason::Length,
            TranscriptionFinishReason::Cancelled,
            TranscriptionFinishReason::Failed,
        ] {
            let result = transcription(reason, 10_000);
            assert_eq!(
                assess_completeness(&result, Some(10_000), 1_000),
                TranscriptionCompleteness::Incomplete
            );
        }
    }

    #[test]
    fn completeness_covers_completed_empty_and_truncated_boundaries() {
        let completed = transcription(TranscriptionFinishReason::Completed, 2_000);
        assert_eq!(
            assess_completeness(&completed, None, 500),
            TranscriptionCompleteness::Complete
        );

        let empty = StructuredTranscription::default();
        assert_eq!(
            assess_completeness(&empty, Some(500), 500),
            TranscriptionCompleteness::Complete
        );

        let nonempty_without_segments = StructuredTranscription {
            text: "unstructured output".to_string(),
            ..Default::default()
        };
        assert_eq!(
            assess_completeness(&nonempty_without_segments, Some(500), 500),
            TranscriptionCompleteness::Incomplete
        );

        let truncated = transcription(TranscriptionFinishReason::Unknown, 1_000);
        assert_eq!(
            assess_completeness(&truncated, Some(2_000), 500),
            TranscriptionCompleteness::Incomplete
        );
    }

    #[test]
    fn structured_result_round_trips_speaker_and_usage() {
        let mut result = transcription(TranscriptionFinishReason::Completed, 2_000);
        result.usage = Some(TranscriptionUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        });
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredTranscription = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
        assert_eq!(decoded.segments[0].normalized_speaker(), Some("speaker_00"));
    }
}
