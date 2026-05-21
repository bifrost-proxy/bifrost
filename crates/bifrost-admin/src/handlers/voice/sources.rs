use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::handlers::voice_stateful::STATEFUL_PROVIDER_ID;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSourceStatus {
    Ready,
    NeedsPermission,
    Unsupported,
    SourceUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceSource {
    pub id: String,
    pub kind: String,
    pub status: VoiceSourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct VoiceProviderStatus {
    pub id: &'static str,
    pub local_only: bool,
    pub available: bool,
    pub supports_stateful_streaming: bool,
    pub notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct VoiceStatusResponse {
    pub local_only: bool,
    pub audio_leaves_device: bool,
    pub raw_audio_persisted_by_default: bool,
    pub providers: Vec<VoiceProviderStatus>,
}

pub(super) fn voice_status_response() -> VoiceStatusResponse {
    VoiceStatusResponse {
        local_only: true,
        audio_leaves_device: false,
        raw_audio_persisted_by_default: false,
        providers: vec![VoiceProviderStatus {
            id: STATEFUL_PROVIDER_ID,
            local_only: true,
            available: cfg!(target_os = "macos"),
            supports_stateful_streaming: true,
            notes: "Uses qwen3-asr Rust StreamingState in a per-session worker process; macOS Metal builds only.",
        }],
    }
}

pub fn discover_voice_sources() -> Vec<VoiceSource> {
    let mut sources = vec![
        VoiceSource {
            id: "web_mic".to_string(),
            kind: "web_mic".to_string(),
            status: VoiceSourceStatus::Ready,
            reason: None,
        },
        VoiceSource {
            id: "file:realtime".to_string(),
            kind: "file".to_string(),
            status: VoiceSourceStatus::Ready,
            reason: Some("local realtime file stream for automated validation".to_string()),
        },
    ];

    sources.push(VoiceSource {
        id: "mic:default".to_string(),
        kind: "mic".to_string(),
        status: default_mic_status(),
        reason: default_mic_reason(),
    });

    if cfg!(target_os = "macos") {
        sources.push(VoiceSource {
            id: "system:default".to_string(),
            kind: "system".to_string(),
            status: VoiceSourceStatus::NeedsPermission,
            reason: Some("requires macOS audio capture permission".to_string()),
        });
        sources.push(VoiceSource {
            id: "app:process".to_string(),
            kind: "app".to_string(),
            status: VoiceSourceStatus::Unsupported,
            reason: Some("requires macOS 14.2+ Core Audio process tap integration".to_string()),
        });
    } else {
        sources.push(VoiceSource {
            id: "system:default".to_string(),
            kind: "system".to_string(),
            status: VoiceSourceStatus::Unsupported,
            reason: Some("native system audio capture is only planned for macOS V1/V2".to_string()),
        });
        sources.push(VoiceSource {
            id: "app:process".to_string(),
            kind: "app".to_string(),
            status: VoiceSourceStatus::Unsupported,
            reason: Some("per-application audio capture is only planned for macOS".to_string()),
        });
    }

    sources
}

fn default_mic_status() -> VoiceSourceStatus {
    if !cfg!(target_os = "macos") {
        return VoiceSourceStatus::Unsupported;
    }
    if command_available("ffmpeg") {
        VoiceSourceStatus::Ready
    } else {
        VoiceSourceStatus::SourceUnavailable
    }
}

fn default_mic_reason() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some(
            "native microphone capture is currently implemented only for macOS".to_string(),
        );
    }
    if command_available("ffmpeg") {
        None
    } else {
        Some(
            "ffmpeg is required for CLI microphone capture; install it with `brew install ffmpeg`"
                .to_string(),
        )
    }
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
