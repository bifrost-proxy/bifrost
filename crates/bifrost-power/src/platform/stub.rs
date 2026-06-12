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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_assertion_is_unsupported() {
        // On the stub platform acquiring an assertion must always fail with a
        // typed Unsupported error carrying the current platform label.
        match acquire_assertion() {
            Ok(_) => panic!("stub acquire_assertion must not succeed"),
            Err(PowerError::Unsupported(label)) => {
                assert_eq!(label, PlatformSupport::current().label());
                assert!(!label.is_empty());
            }
            Err(other) => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn is_on_battery_is_false_on_stub() {
        // Conservative default: never report battery to avoid spurious warnings.
        assert!(!is_on_battery_impl());
    }
}
