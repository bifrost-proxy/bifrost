pub mod artifacts;
pub mod decision;
pub mod offline;
pub mod planner;
pub mod platform;
pub mod profiles;
pub mod resources;
pub mod runtime;
pub mod speaker;
pub mod subtitle;
pub mod timeline;
pub mod transcription;
pub mod wake;

#[cfg(all(
    target_os = "macos",
    target_arch = "aarch64",
    any(
        feature = "qwen3-offline",
        feature = "diarization-sherpa",
        feature = "wake-sherpa"
    )
))]
pub mod native {
    #[cfg(feature = "qwen3-offline")]
    pub use qwen3_asr;

    #[cfg(feature = "diarization-sherpa")]
    pub use sherpa_onnx;
}

pub use platform::{
    asr_platform_supported, asr_platform_supported_for, current_platform, AsrPlatform,
    SUPPORTED_ASR_TARGET,
};
