//! Platform dispatch. `acquire_assertion()` returns an [`AssertionHandle`]
//! whose `Drop` impl releases the underlying OS resources.

use crate::{PowerError, Result};

/// Which platform is currently active. Used by the manager to surface
/// "unsupported" status to callers without having to `cfg!` everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSupport {
    MacOs,
    Unsupported(&'static str),
}

impl PlatformSupport {
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            PlatformSupport::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            PlatformSupport::Unsupported("linux")
        }
        #[cfg(target_os = "windows")]
        {
            PlatformSupport::Unsupported("windows")
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            PlatformSupport::Unsupported("other")
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            PlatformSupport::MacOs => "macos",
            PlatformSupport::Unsupported(label) => label,
        }
    }

    pub const fn is_supported(self) -> bool {
        matches!(self, PlatformSupport::MacOs)
    }
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::{acquire_assertion, is_on_battery_impl, AssertionHandle};

// Stub implementations for non-macOS platforms.
#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(not(target_os = "macos"))]
pub(crate) use stub::{acquire_assertion, is_on_battery_impl, AssertionHandle};

/// Whether the system is currently running on battery power.
///
/// Returns `false` on unsupported platforms (conservative default, prevents
/// spurious UI warnings).
pub fn is_on_battery() -> bool {
    is_on_battery_impl()
}

/// Convenience: platform dispatch wrapper that returns a typed error on
/// unsupported platforms instead of panicking.
#[allow(dead_code)]
pub(crate) fn check_supported() -> Result<()> {
    match PlatformSupport::current() {
        PlatformSupport::MacOs => Ok(()),
        PlatformSupport::Unsupported(label) => Err(PowerError::Unsupported(label)),
    }
}
