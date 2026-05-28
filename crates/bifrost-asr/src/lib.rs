pub mod artifacts;
pub mod offline;
pub mod planner;
pub mod platform;
pub mod profiles;
pub mod runtime;
pub mod subtitle;
pub mod timeline;

#[cfg(any(feature = "qwen3-offline", feature = "diarization-sherpa"))]
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
