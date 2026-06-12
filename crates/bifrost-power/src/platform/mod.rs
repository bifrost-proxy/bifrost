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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_matches_support_flag() {
        let p = PlatformSupport::current();
        // is_supported() is true iff we are on macOS.
        assert_eq!(p.is_supported(), matches!(p, PlatformSupport::MacOs));
        // label() is always non-empty and stable.
        assert!(!p.label().is_empty());
    }

    #[test]
    fn unsupported_variant_round_trips_label() {
        let p = PlatformSupport::Unsupported("linux");
        assert_eq!(p.label(), "linux");
        assert!(!p.is_supported());

        let mac = PlatformSupport::MacOs;
        assert_eq!(mac.label(), "macos");
        assert!(mac.is_supported());
    }

    #[test]
    fn check_supported_matches_current_platform() {
        let result = check_supported();
        if PlatformSupport::current().is_supported() {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(PowerError::Unsupported(_))));
        }
    }

    #[test]
    fn is_on_battery_is_callable() {
        // On the stub platform this is always false; on macOS it reflects the
        // real state. Either way the call must not panic.
        let _ = is_on_battery();
        #[cfg(not(target_os = "macos"))]
        assert!(!is_on_battery());
    }
}
