//! Keep-awake for Bifrost.
//!
//! Platform support matrix (see [`PlatformSupport::current`]):
//! * **macOS** — fully supported via two public IOKit `IOPMAssertion` handles
//!   (`PreventUserIdleSystemSleep` + `PreventSystemSleep`). No root / sudo
//!   required. Battery-powered machines with the lid closed may still be put
//!   to sleep by Apple's policy — callers should surface the
//!   [`Status::battery_warning`] field to users.
//! * **Linux / Windows / others** — explicitly unsupported. All mutating APIs
//!   return [`PowerError::Unsupported`]; read-only APIs return an inert
//!   [`Status`] with `supported = false`.
//!
//! The full state machine (mode × assertion active/inactive) is owned by
//! [`KeepAwakeManager`]. Platform code is kept minimal and hidden behind the
//! [`AssertionHandle`] RAII wrapper.

pub mod manager;
pub mod mode;
pub mod power_notifications;

mod platform;

pub use manager::{KeepAwakeManager, ModePersister, NoopPersister, SharedKeepAwakeManager};
pub use mode::{Mode, ParseModeError};
pub use platform::{is_on_battery, PlatformSupport};
pub use power_notifications::{PowerEvent, PowerNotificationWatcher};

use serde::Serialize;
use thiserror::Error;

/// Errors reported by the keep-awake subsystem.
#[derive(Debug, Error)]
pub enum PowerError {
    /// Current platform does not support keep-awake (non-macOS today).
    #[error("keep-awake is not supported on this platform (current: {0})")]
    Unsupported(&'static str),

    /// macOS IOKit API returned a non-success status.
    #[error("platform call failed: {0}")]
    Platform(String),

    /// Internal state was poisoned / invariants violated.
    #[error("invalid keep-awake state: {0}")]
    InvalidState(String),
}

pub type Result<T> = std::result::Result<T, PowerError>;

/// Snapshot of the keep-awake subsystem returned to clients.
///
/// This is the wire format (serialized as JSON) used by both the local HTTP
/// API and the remote-invoke RPC. Keep it stable.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// Whether the running platform has a real backing implementation.
    /// `false` on Linux/Windows — in that case `active` is always `false`
    /// and mutating operations will fail with [`PowerError::Unsupported`].
    pub supported: bool,
    /// Human-readable platform label (e.g. `"macos"`, `"linux"`).
    pub platform: &'static str,
    /// Currently persisted mode.
    pub mode: Mode,
    /// Whether an assertion is currently held.
    pub active: bool,
    /// Seconds since the current assertion was acquired, `None` when inactive.
    pub active_since_secs: Option<u64>,
    /// Whether the machine is running on battery power.
    pub on_battery: bool,
    /// Warning message surfaced to the UI when battery-mode lid sleep cannot
    /// be fully prevented (always set on macOS while on battery).
    pub battery_warning: Option<String>,
}

impl Status {
    /// Construct a Status for an unsupported platform.
    pub fn unsupported(platform: &'static str, mode: Mode) -> Self {
        Self {
            supported: false,
            platform,
            mode,
            active: false,
            active_since_secs: None,
            on_battery: false,
            battery_warning: None,
        }
    }
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    #[test]
    fn status_unsupported_is_inert() {
        let s = Status::unsupported("linux", Mode::Auto);
        assert!(!s.supported);
        assert_eq!(s.platform, "linux");
        assert_eq!(s.mode, Mode::Auto);
        assert!(!s.active);
        assert!(s.active_since_secs.is_none());
        assert!(!s.on_battery);
        assert!(s.battery_warning.is_none());
    }

    #[test]
    fn status_is_serializable_to_json() {
        // Status is a wire format; make sure serde round-trips the fields.
        let s = Status::unsupported("windows", Mode::Off);
        let json = serde_json::to_string(&s).expect("serialize Status");
        assert!(json.contains("\"supported\":false"));
        assert!(json.contains("\"platform\":\"windows\""));
    }

    #[test]
    fn power_error_display_messages() {
        // Exercise every `#[error(...)]` Display arm.
        let unsupported = PowerError::Unsupported("linux");
        assert!(format!("{unsupported}").contains("not supported"));
        assert!(format!("{unsupported}").contains("linux"));

        let platform = PowerError::Platform("io failure".to_string());
        assert!(format!("{platform}").contains("platform call failed"));
        assert!(format!("{platform}").contains("io failure"));

        let invalid = PowerError::InvalidState("bad".to_string());
        assert!(format!("{invalid}").contains("invalid keep-awake state"));
        assert!(format!("{invalid}").contains("bad"));
    }

    #[test]
    fn power_error_is_debug() {
        // Debug derive should render the variant name.
        let dbg = format!("{:?}", PowerError::Unsupported("linux"));
        assert!(dbg.contains("Unsupported"));
    }
}
