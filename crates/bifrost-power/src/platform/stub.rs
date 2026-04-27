//! Stub platform implementation for non-macOS targets.
//!
//! All mutating operations return [`PowerError::Unsupported`]; battery check
//! returns `false` (conservative — no UI warning shown).

use crate::{PlatformSupport, PowerError, Result};

/// Inert handle that never holds any OS resource. Its Drop is a no-op.
pub struct AssertionHandle {
    _private: (),
}

pub(crate) fn acquire_assertion() -> Result<AssertionHandle> {
    let label = match PlatformSupport::current() {
        PlatformSupport::Unsupported(l) => l,
        PlatformSupport::MacOs => unreachable!("stub compiled on macOS"),
    };
    Err(PowerError::Unsupported(label))
}

pub(crate) fn is_on_battery_impl() -> bool {
    false
}
