//! macOS implementation: creates IOKit `IOPMAssertion` handles via the
//! public `IOPMAssertionCreateWithName` API. Requires **no** elevated
//! privileges.
//!
//! We acquire two assertions in parallel and keep them for the lifetime of
//! [`AssertionHandle`]:
//!
//! * `PreventUserIdleSystemSleep` — prevents idle sleep (keyboard/mouse idle
//!   timer). Equivalent to `caffeinate -i`.
//! * `PreventSystemSleep` — on AC power, also prevents lid-close sleep.
//!   Equivalent to `caffeinate -s`. Apple's policy does *not* allow
//!   user-space processes to prevent sleep while running on battery with
//!   the lid closed — we surface this as a warning in the manager.
//!
//! References:
//! * Apple headers: `IOKit/pwr_mgt/IOPMLib.h`
//! * `man caffeinate(8)` — the system-provided CLI is literally a thin
//!   wrapper around the same IOKit APIs.

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef as SysCFStringRef;
use std::os::raw::{c_int, c_uint};
use tracing::{debug, warn};

use crate::{PowerError, Result};

// --------------------------------------------------------------------------
// FFI — kept minimal and hand-written to avoid pulling in a heavier crate.
// --------------------------------------------------------------------------

type IOPMAssertionID = u32;
type IOPMAssertionLevel = u32;
type IOReturn = c_int;

const K_IO_RETURN_SUCCESS: IOReturn = 0;
const K_IOPM_ASSERTION_LEVEL_ON: IOPMAssertionLevel = 255;

// Assertion type names (from IOKit/pwr_mgt/IOPMLib.h).
const PREVENT_USER_IDLE_SYSTEM_SLEEP: &str = "PreventUserIdleSystemSleep";
const PREVENT_SYSTEM_SLEEP: &str = "PreventSystemSleep";
const ASSERTION_REASON: &str = "Bifrost keep-awake (remote invoke)";

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: SysCFStringRef,
        assertion_level: IOPMAssertionLevel,
        assertion_name: SysCFStringRef,
        assertion_id: *mut IOPMAssertionID,
    ) -> IOReturn;

    fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;

    // Power source inspection — used for battery detection.
    fn IOPSCopyPowerSourcesInfo() -> *const core_foundation_sys::base::CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(
        ps_blob: *const core_foundation_sys::base::CFTypeRef,
    ) -> SysCFStringRef;
}

// Opaque wrapper holding the raw assertion id. Only used internally; the
// public surface is [`AssertionHandle`].
struct RawAssertion {
    id: IOPMAssertionID,
    kind: &'static str,
}

impl RawAssertion {
    fn acquire(kind: &'static str) -> Result<Self> {
        let type_cf = CFString::new(kind);
        let name_cf = CFString::new(ASSERTION_REASON);
        let mut id: IOPMAssertionID = 0;
        // SAFETY: `type_cf`/`name_cf` outlive the call; out-param is valid.
        let rc = unsafe {
            IOPMAssertionCreateWithName(
                type_cf.as_concrete_TypeRef(),
                K_IOPM_ASSERTION_LEVEL_ON,
                name_cf.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if rc != K_IO_RETURN_SUCCESS {
            return Err(PowerError::Platform(format!(
                "IOPMAssertionCreateWithName({kind}) failed: IOReturn=0x{:x}",
                rc as c_uint
            )));
        }
        debug!(kind, id, "acquired IOPM assertion");
        Ok(Self { id, kind })
    }
}

impl Drop for RawAssertion {
    fn drop(&mut self) {
        // SAFETY: `id` is a valid assertion ID returned by the create API.
        let rc = unsafe { IOPMAssertionRelease(self.id) };
        if rc != K_IO_RETURN_SUCCESS {
            warn!(
                kind = self.kind,
                id = self.id,
                rc = format!("0x{:x}", rc as c_uint),
                "IOPMAssertionRelease failed (leaking assertion)"
            );
        } else {
            debug!(kind = self.kind, id = self.id, "released IOPM assertion");
        }
    }
}

/// RAII handle holding both assertions for the lifetime of the struct.
/// Private fields — construct via [`acquire_assertion`].
pub struct AssertionHandle {
    // The only purpose of these fields is to keep the Drop chain alive.
    _idle: RawAssertion,
    _system: RawAssertion,
}

pub(crate) fn acquire_assertion() -> Result<AssertionHandle> {
    // Acquire in order; if the second fails the first is dropped
    // automatically by RAII.
    let idle = RawAssertion::acquire(PREVENT_USER_IDLE_SYSTEM_SLEEP)?;
    let system = RawAssertion::acquire(PREVENT_SYSTEM_SLEEP)?;
    Ok(AssertionHandle {
        _idle: idle,
        _system: system,
    })
}

// --------------------------------------------------------------------------
// Battery detection
// --------------------------------------------------------------------------

// Mirrors the constants from IOKit/ps/IOPSKeys.h
const K_IOPS_AC_POWER_VALUE: &str = "AC Power";

pub(crate) fn is_on_battery_impl() -> bool {
    // SAFETY: IOPSCopyPowerSourcesInfo returns a +1 retained CFTypeRef (or
    // NULL if the info is unavailable). We release via CFRelease.
    unsafe {
        let ps_blob = IOPSCopyPowerSourcesInfo();
        if ps_blob.is_null() {
            return false;
        }
        let kind_ref: CFStringRef = IOPSGetProvidingPowerSourceType(ps_blob);
        let on_battery = if kind_ref.is_null() {
            false
        } else {
            let s = CFString::wrap_under_get_rule(kind_ref).to_string();
            s != K_IOPS_AC_POWER_VALUE
        };
        core_foundation_sys::base::CFRelease(ps_blob as core_foundation_sys::base::CFTypeRef);
        on_battery
    }
}

// Silence unused-import warning when CFDictionaryRef path isn't used (kept
// for future expansion to per-source inspection).
#[allow(dead_code)]
fn _unused_dictref(_d: CFDictionaryRef) {}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests actually call into IOKit, which always works on any Mac.
    // They're cheap but require the target OS, so gate by cfg(target_os).
    #[test]
    fn acquire_and_release_roundtrip() {
        let h = acquire_assertion().expect("acquire");
        drop(h);
    }

    #[test]
    fn battery_query_does_not_panic() {
        // Either true or false is acceptable — we just want the call to
        // succeed without crashing. This is the property we rely on at
        // runtime.
        let _ = is_on_battery_impl();
    }
}
