use serde::{Deserialize, Serialize};

pub const SUPPORTED_ASR_TARGET: &str = "macos-aarch64";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrPlatform {
    pub os: String,
    pub arch: String,
    pub supported: bool,
    pub supported_target: &'static str,
    pub unsupported_reason: Option<String>,
}

pub fn asr_platform_supported_for(os: &str, arch: &str) -> bool {
    os == "macos" && arch == "aarch64"
}

pub fn asr_platform_supported() -> bool {
    asr_platform_supported_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn current_platform() -> AsrPlatform {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let supported = asr_platform_supported_for(&os, &arch);
    let unsupported_reason = (!supported).then(|| {
        format!(
            "ASR capabilities are available only on Apple Silicon macOS; current platform is {os}-{arch}"
        )
    });
    AsrPlatform {
        os,
        arch,
        supported,
        supported_target: SUPPORTED_ASR_TARGET,
        unsupported_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_matrix_is_apple_silicon_macos_only() {
        assert!(asr_platform_supported_for("macos", "aarch64"));
        assert!(!asr_platform_supported_for("macos", "x86_64"));
        assert!(!asr_platform_supported_for("linux", "x86_64"));
        assert!(!asr_platform_supported_for("windows", "x86_64"));
    }

    #[test]
    fn asr_platform_supported_matches_current_target() {
        assert_eq!(
            asr_platform_supported(),
            asr_platform_supported_for(std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn current_platform_reports_consistent_fields() {
        let platform = current_platform();
        assert_eq!(
            platform.supported,
            asr_platform_supported_for(&platform.os, &platform.arch)
        );
        assert_eq!(platform.supported_target, SUPPORTED_ASR_TARGET);
        if platform.supported {
            assert!(platform.unsupported_reason.is_none());
        } else {
            let reason = platform.unsupported_reason.as_deref().unwrap_or_default();
            assert!(reason.contains("Apple Silicon macOS"));
        }
    }
}
