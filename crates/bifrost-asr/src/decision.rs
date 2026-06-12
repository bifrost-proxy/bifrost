use serde::{Deserialize, Serialize};

use crate::profiles::{
    builtin_profile, OFFLINE_ASR_MODEL, OFFLINE_PLAIN_ASR_PROFILE,
    OFFLINE_SPEAKER_SUBTITLE_PROFILE, REALTIME_DICTATION_PROFILE, REALTIME_WAKE_PROFILE,
    SCHEDULED_SPEAKER_SUBTITLE_PROFILE,
};
use crate::profiles::{SpeechMode, SpeechPipelineProfile};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeechRequest {
    #[serde(default)]
    pub pipeline_profile: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub speaker_aware: bool,
    #[serde(default)]
    pub scheduled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechRuntimeEnv {
    pub platform_supported: bool,
    pub asr_ready: bool,
    pub diarization_ready: bool,
    pub realtime_voice_active: bool,
    pub offline_asr_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineDecision {
    pub mode: SpeechMode,
    pub pipeline_profile: String,
    pub profile: SpeechPipelineProfile,
    pub asr: AsrDecision,
    pub wake: Option<WakeDecision>,
    pub diarization: Option<DiarizationDecision>,
    pub subtitle: Option<SubtitleDecision>,
    pub runtime: RuntimeDecision,
    pub resource: ResourceDecision,
    pub reasons: Vec<DecisionReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrDecision {
    pub provider: String,
    pub model: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeDecision {
    pub engine: String,
    pub fallback_engine: Option<String>,
    pub requires_qwen_by_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizationDecision {
    pub enabled: bool,
    pub provider: String,
    pub profile: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleDecision {
    pub formats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDecision {
    pub strategy: String,
    pub input_contract: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDecision {
    pub available: bool,
    pub priority: u8,
    pub preemptible: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionReason {
    pub code: String,
    pub message: String,
}

pub fn resolve_engine_decision(
    mode: SpeechMode,
    request: SpeechRequest,
    env: SpeechRuntimeEnv,
) -> EngineDecision {
    let profile_id = request
        .pipeline_profile
        .as_deref()
        .filter(|id| builtin_profile(id).is_some())
        .map(str::to_string)
        .unwrap_or_else(|| default_profile_for(mode, &request).to_string());
    let profile = builtin_profile(&profile_id)
        .or_else(|| builtin_profile(default_profile_for(mode, &request)))
        .expect("builtin speech profile must exist");
    let mut reasons = Vec::new();
    if request
        .pipeline_profile
        .as_deref()
        .is_some_and(|id| id != profile_id)
    {
        reasons.push(DecisionReason {
            code: "unknown_profile_fallback".to_string(),
            message: format!("unknown profile requested; resolved to {profile_id}"),
        });
    }
    if !env.platform_supported {
        reasons.push(DecisionReason {
            code: "unsupported_platform".to_string(),
            message: "local ASR is not supported by this build target".to_string(),
        });
    }
    let asr = resolve_asr(mode, &request, &profile, &env);
    let diarization = profile.offline.as_ref().and_then(|offline| {
        offline
            .diarization
            .as_ref()
            .map(|config| DiarizationDecision {
                enabled: config.enabled,
                provider: "sherpa-onnx".to_string(),
                profile: config.profile.clone(),
                ready: !config.enabled || env.diarization_ready,
            })
    });
    if diarization
        .as_ref()
        .is_some_and(|decision| decision.enabled && !decision.ready)
    {
        reasons.push(DecisionReason {
            code: "diarization_missing_assets".to_string(),
            message: "speaker diarization profile is not initialized".to_string(),
        });
    }
    let resource_available = env.platform_supported
        && match mode {
            SpeechMode::RealtimeDictation | SpeechMode::RealtimeWake => !env.offline_asr_active,
            SpeechMode::OfflineFile | SpeechMode::DirectoryTask => !env.realtime_voice_active,
        };
    let resource_reason = (!resource_available).then(|| match mode {
        SpeechMode::RealtimeDictation | SpeechMode::RealtimeWake if env.offline_asr_active => {
            "offline ASR is currently using the local model resource".to_string()
        }
        SpeechMode::OfflineFile | SpeechMode::DirectoryTask if env.realtime_voice_active => {
            "realtime voice input is active; background ASR should yield".to_string()
        }
        _ => "local ASR resource is unavailable".to_string(),
    });
    if let Some(reason) = &resource_reason {
        reasons.push(DecisionReason {
            code: "resource_unavailable".to_string(),
            message: reason.clone(),
        });
    }
    EngineDecision {
        mode,
        pipeline_profile: profile.id.clone(),
        subtitle: profile.offline.as_ref().map(|offline| SubtitleDecision {
            formats: offline.subtitle_formats.clone(),
        }),
        wake: (mode == SpeechMode::RealtimeWake).then(|| WakeDecision {
            engine: "lightweight_kws_listener".to_string(),
            fallback_engine: Some("bounded_asr_phrase_match".to_string()),
            requires_qwen_by_default: false,
        }),
        runtime: RuntimeDecision {
            strategy: profile
                .offline
                .as_ref()
                .map(|offline| offline.reuse_strategy.clone())
                .unwrap_or_else(|| "stateful_streaming".to_string()),
            input_contract: match mode {
                SpeechMode::RealtimeDictation => "pcm16_16k_mono".to_string(),
                SpeechMode::RealtimeWake => "mic_vad_kws".to_string(),
                SpeechMode::OfflineFile | SpeechMode::DirectoryTask => {
                    "ffmpeg_16k_mono_wav".to_string()
                }
            },
        },
        resource: ResourceDecision {
            available: resource_available,
            priority: profile.resources.priority,
            preemptible: profile.resources.preemptible,
            reason: resource_reason,
        },
        asr,
        diarization,
        profile,
        reasons,
    }
}

pub fn default_profile_for(mode: SpeechMode, request: &SpeechRequest) -> &'static str {
    match mode {
        SpeechMode::RealtimeDictation => REALTIME_DICTATION_PROFILE,
        SpeechMode::RealtimeWake => REALTIME_WAKE_PROFILE,
        SpeechMode::OfflineFile if request.speaker_aware => OFFLINE_SPEAKER_SUBTITLE_PROFILE,
        SpeechMode::OfflineFile => OFFLINE_PLAIN_ASR_PROFILE,
        SpeechMode::DirectoryTask if request.scheduled || request.speaker_aware => {
            SCHEDULED_SPEAKER_SUBTITLE_PROFILE
        }
        SpeechMode::DirectoryTask => OFFLINE_PLAIN_ASR_PROFILE,
    }
}

fn resolve_asr(
    mode: SpeechMode,
    request: &SpeechRequest,
    profile: &SpeechPipelineProfile,
    env: &SpeechRuntimeEnv,
) -> AsrDecision {
    let (provider, model) = match mode {
        SpeechMode::RealtimeDictation => (
            "qwen3_stateful_streaming".to_string(),
            profile
                .realtime
                .as_ref()
                .map(|realtime| realtime.model.clone())
                .unwrap_or_else(|| "Qwen3-ASR-0.6B".to_string()),
        ),
        SpeechMode::RealtimeWake => (
            "lightweight_kws_listener".to_string(),
            profile
                .realtime
                .as_ref()
                .map(|realtime| realtime.model.clone())
                .unwrap_or_else(|| "Qwen3-ASR-0.6B".to_string()),
        ),
        SpeechMode::OfflineFile | SpeechMode::DirectoryTask => (
            profile
                .offline
                .as_ref()
                .map(|offline| offline.asr_provider.clone())
                .unwrap_or_else(|| "qwen3-offline".to_string()),
            request.model.clone().unwrap_or_else(|| {
                profile
                    .offline
                    .as_ref()
                    .map(|offline| offline.model.clone())
                    .unwrap_or_else(|| OFFLINE_ASR_MODEL.to_string())
            }),
        ),
    };
    AsrDecision {
        provider,
        model,
        ready: env.platform_supported
            && match mode {
                SpeechMode::RealtimeWake => true,
                _ => env.asr_ready,
            },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> SpeechRuntimeEnv {
        SpeechRuntimeEnv {
            platform_supported: true,
            asr_ready: true,
            diarization_ready: true,
            realtime_voice_active: false,
            offline_asr_active: false,
        }
    }

    #[test]
    fn resolve_realtime_dictation_uses_stateful_0_6b() {
        let decision = resolve_engine_decision(
            SpeechMode::RealtimeDictation,
            SpeechRequest {
                pipeline_profile: None,
                model: None,
                language: None,
                speaker_aware: false,
                scheduled: false,
            },
            env(),
        );
        assert_eq!(decision.asr.provider, "qwen3_stateful_streaming");
        assert_eq!(decision.asr.model, "Qwen3-ASR-0.6B");
    }

    #[test]
    fn resolve_offline_subtitle_enables_sherpa_diarization() {
        let decision = resolve_engine_decision(
            SpeechMode::OfflineFile,
            SpeechRequest {
                speaker_aware: true,
                ..SpeechRequest::default()
            },
            env(),
        );
        assert_eq!(decision.pipeline_profile, OFFLINE_SPEAKER_SUBTITLE_PROFILE);
        assert!(decision
            .diarization
            .as_ref()
            .is_some_and(|diarization| diarization.enabled && diarization.ready));
    }

    #[test]
    fn resolve_directory_plain_task_keeps_diarization_off() {
        let decision =
            resolve_engine_decision(SpeechMode::DirectoryTask, SpeechRequest::default(), env());
        assert_eq!(decision.pipeline_profile, OFFLINE_PLAIN_ASR_PROFILE);
        assert!(decision.diarization.is_none());
    }

    #[test]
    fn default_profile_for_directory_task_uses_scheduled_speaker_profile_when_scheduled_or_speaker_aware(
    ) {
        let scheduled = SpeechRequest {
            scheduled: true,
            ..SpeechRequest::default()
        };
        assert_eq!(
            default_profile_for(SpeechMode::DirectoryTask, &scheduled),
            SCHEDULED_SPEAKER_SUBTITLE_PROFILE
        );

        let speaker_aware = SpeechRequest {
            speaker_aware: true,
            ..SpeechRequest::default()
        };
        assert_eq!(
            default_profile_for(SpeechMode::DirectoryTask, &speaker_aware),
            SCHEDULED_SPEAKER_SUBTITLE_PROFILE
        );
    }

    #[test]
    fn resolve_engine_decision_falls_back_to_default_profile_for_unknown_id() {
        let decision = resolve_engine_decision(
            SpeechMode::OfflineFile,
            SpeechRequest {
                pipeline_profile: Some("nonexistent-profile".to_string()),
                ..SpeechRequest::default()
            },
            env(),
        );

        assert_ne!(decision.pipeline_profile, "nonexistent-profile");
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "unknown_profile_fallback"));
    }

    #[test]
    fn resolve_engine_decision_marks_unsupported_platform_and_resource_unavailable() {
        let env = SpeechRuntimeEnv {
            platform_supported: false,
            asr_ready: false,
            diarization_ready: false,
            realtime_voice_active: false,
            offline_asr_active: false,
        };
        let decision =
            resolve_engine_decision(SpeechMode::OfflineFile, SpeechRequest::default(), env);

        assert!(!decision.asr.ready);
        assert!(!decision.resource.available);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "unsupported_platform"));
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "resource_unavailable"));
        assert!(decision.resource.reason.is_some());
    }

    #[test]
    fn resolve_engine_decision_conflicts_with_offline_asr_for_realtime() {
        let mut env = env();
        env.offline_asr_active = true;
        let decision =
            resolve_engine_decision(SpeechMode::RealtimeDictation, SpeechRequest::default(), env);

        assert!(!decision.resource.available);
        let reason = decision.resource.reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("offline ASR is currently using the local model resource"),
            "unexpected resource reason: {reason}"
        );
    }

    #[test]
    fn resolve_engine_decision_conflicts_with_realtime_voice_for_offline_file() {
        let mut env = env();
        env.realtime_voice_active = true;
        let decision =
            resolve_engine_decision(SpeechMode::OfflineFile, SpeechRequest::default(), env);

        assert!(!decision.resource.available);
        let reason = decision.resource.reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("realtime voice input is active"),
            "unexpected resource reason: {reason}"
        );
    }

    #[test]
    fn request_model_overrides_offline_profile_default() {
        let decision = resolve_engine_decision(
            SpeechMode::OfflineFile,
            SpeechRequest {
                model: Some("Custom-Model".to_string()),
                ..SpeechRequest::default()
            },
            env(),
        );

        assert_eq!(decision.asr.model, "Custom-Model");
    }

    #[test]
    fn resolve_realtime_wake_uses_wake_provider_and_is_ready_without_asr_assets() {
        let mut env = env();
        env.asr_ready = false;
        let decision =
            resolve_engine_decision(SpeechMode::RealtimeWake, SpeechRequest::default(), env);

        assert_eq!(decision.asr.provider, "lightweight_kws_listener");
        assert!(decision.asr.ready);
    }
}
