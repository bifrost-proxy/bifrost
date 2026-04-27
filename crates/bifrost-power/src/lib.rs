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

mod platform;

pub use manager::{KeepAwakeManager, SharedKeepAwakeManager};
pub use mode::{Mode, ParseModeError};
pub use platform::{is_on_battery, PlatformSupport};

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
