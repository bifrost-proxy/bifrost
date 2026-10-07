//! A narrowly scoped SystemConfiguration compare-and-restore transaction.
//! Native calls run only in a bounded, one-shot child of the existing binary.
use super::macos_owned::{Field, Protocol};
use crate::{BifrostError, Result};
use serde::{Deserialize, Serialize};

pub(super) const HELPER_ARGUMENT: &str = "__bifrost_restore_dormant_proxy";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub service: String,
    pub field: Field,
    pub expected: Protocol,
    pub desired: Protocol,
    pub deadline_millis: u64,
    pub owner: ProcessIdentity,
    pub lock: LockIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LockIdentity {
    pub device: u64,
    pub inode: u64,
}

pub(super) fn current_lock_identity() -> Result<LockIdentity> {
    #[cfg(all(target_os = "macos", not(any(test, bifrost_proxy_test_io))))]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = super::macos_operation_lock::clone_current()?.metadata()?;
        Ok(LockIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(any(not(target_os = "macos"), test, bifrost_proxy_test_io))]
    {
        Ok(LockIdentity {
            device: 0,
            inode: 0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessIdentity {
    pub pid: u32,
    pub started_sec: u64,
    pub started_usec: u64,
}

pub(super) fn originating_process() -> Result<ProcessIdentity> {
    #[cfg(all(target_os = "macos", not(any(test, bifrost_proxy_test_io))))]
    {
        native::process_identity(std::process::id())
    }
    #[cfg(any(not(target_os = "macos"), test, bifrost_proxy_test_io))]
    {
        Ok(ProcessIdentity {
            pid: std::process::id(),
            started_sec: 0,
            started_usec: 0,
        })
    }
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        if self.owner.pid == 0
            || self.service.is_empty()
            || self.service.len() > 4096
            || self.service.contains('\0')
            || self.expected.host.len() > 4096
            || self.desired.host.len() > 4096
            || self.expected.host.contains('\0')
            || self.desired.host.contains('\0')
            || !matches!(self.field, Field::Http | Field::Https)
            || self.expected.enabled
            || self.desired.enabled
            || self.expected.authenticated
            || self.desired.authenticated
            || !(self.desired.host.is_empty() || self.desired.port == 0)
        {
            return Err(BifrostError::Config(
                "Invalid disabled HTTP(S) endpoint restoration request".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Applied,
    OwnershipChanged,
}

#[cfg(any(test, not(bifrost_proxy_test_io)))]
#[derive(Debug)]
pub(super) enum Property {
    Missing,
    Text(String),
    Number(i64),
    Other,
}

#[cfg(any(test, not(bifrost_proxy_test_io)))]
pub(super) fn protocol_from_properties(
    field: Field,
    mut get: impl FnMut(&str) -> Result<Property>,
) -> Result<Protocol> {
    let prefix = match field {
        Field::Http => "HTTP",
        Field::Https => "HTTPS",
        Field::Bypass => return Err(BifrostError::Config("Invalid proxy endpoint field".into())),
    };
    let invalid = || BifrostError::Config("Malformed SystemConfiguration proxy value".into());
    let number = |value| match value {
        Property::Missing => Ok(0),
        Property::Number(number) => Ok(number),
        _ => Err(invalid()),
    };
    let text = |value| match value {
        Property::Missing => Ok(String::new()),
        Property::Text(value) => Ok(value),
        _ => Err(invalid()),
    };
    let enabled = number(get(&format!("{prefix}Enable"))?)?;
    let auth = number(get(&format!("{prefix}ProxyAuthenticated"))?)?;
    if !matches!(enabled, 0 | 1) || !matches!(auth, 0 | 1) {
        return Err(invalid());
    }
    let host = text(get(&format!("{prefix}Proxy"))?)?;
    let port = u16::try_from(number(get(&format!("{prefix}Port"))?)?).map_err(|_| invalid())?;
    // Stronger metadata than networksetup's captured bool is unsupported,
    // not proof of a new owner. Retain the journal on any credential hint,
    // including an empty string; never log or journal its contents.
    for suffix in ["User", "ProxyUser", "ProxyUsername", "ProxyPassword"] {
        if !matches!(get(&format!("{prefix}{suffix}"))?, Property::Missing) {
            return Err(BifrostError::Config("Unsupported SystemConfiguration proxy authentication metadata; original snapshot retained".into()));
        }
    }
    if auth != 0 {
        return Err(BifrostError::Config(
            "Unsupported SystemConfiguration authenticated proxy; original snapshot retained"
                .into(),
        ));
    }
    Ok(Protocol {
        enabled: enabled == 1,
        host,
        port,
        authenticated: false,
    })
}

/// The associated dictionary remains opaque: native CF objects are copied
/// without converting or discarding unrelated, unknown preference keys.
#[cfg(any(test, not(bifrost_proxy_test_io)))]
pub(super) trait Preferences {
    type Dictionary;
    fn lock(&mut self) -> Result<()>;
    fn unlock(&mut self) -> Result<()>;
    fn read(&mut self, service: &str) -> Result<Self::Dictionary>;
    fn protocol(&self, dictionary: &Self::Dictionary, field: Field) -> Result<Protocol>;
    fn replace_endpoint(
        &mut self,
        dictionary: Self::Dictionary,
        field: Field,
        desired: &Protocol,
    ) -> Result<()>;
    fn commit(&mut self) -> Result<()>;
    fn apply(&mut self) -> Result<()>;
    fn synchronize(&mut self);
}

#[cfg(any(test, not(bifrost_proxy_test_io)))]
pub(super) fn restore(api: &mut impl Preferences, request: &Request) -> Result<Outcome> {
    request.validate()?;
    api.lock()?;
    let result = (|| {
        let dictionary = api.read(&request.service)?;
        let current = api.protocol(&dictionary, request.field)?;
        if current != request.expected {
            return Ok(Outcome::OwnershipChanged);
        }
        if current != request.desired {
            api.replace_endpoint(dictionary, request.field, &request.desired)?;
            api.commit()?;
        }
        // A retry after commit/crash must still apply, even if persisted values
        // already match. Parent keeps needs_apply until this and read-back pass.
        api.apply()?;
        api.synchronize();
        let actual = api.read(&request.service)?;
        if api.protocol(&actual, request.field)? != request.desired {
            return Err(BifrostError::Config(
                "SystemConfiguration dormant endpoint read-back mismatch".into(),
            ));
        }
        Ok(Outcome::Applied)
    })();
    let unlocked = api.unlock();
    match (result, unlocked) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

/// Dispatch only the exact private mode, before either CLI or desktop startup.
/// Fixture and unit binaries refuse the mode, including direct invocation.
pub(super) fn dispatch(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<Result<String>> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_ARGUMENT)) {
        return None;
    }
    Some((|| {
        let payload = args
            .next()
            .ok_or_else(|| BifrostError::Config("Missing proxy restoration request".into()))?
            .into_string()
            .map_err(|_| BifrostError::Config("Proxy restoration request is not UTF-8".into()))?;
        if payload.len() > 64 * 1024 {
            return Err(BifrostError::Config(
                "Oversized proxy restoration request".into(),
            ));
        }
        if args.next().is_some() {
            return Err(BifrostError::Config(
                "Unexpected proxy restoration arguments".into(),
            ));
        }
        let request: Request = serde_json::from_str(&payload).map_err(|error| {
            BifrostError::Config(format!("Invalid proxy restoration request: {error}"))
        })?;
        request.validate()?;
        execute_native(&request).and_then(|outcome| {
            serde_json::to_string(&outcome).map_err(|error| {
                BifrostError::Config(format!("Cannot encode restoration result: {error}"))
            })
        })
    })())
}

#[cfg(any(test, not(bifrost_proxy_test_io)))]
fn bounded_deadline(now: u64, requested: u64) -> Result<u64> {
    let remaining = requested
        .checked_sub(now)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| BifrostError::Config("Proxy restoration request expired".into()))?;
    Ok(now.saturating_add(remaining.min(8000)))
}

#[cfg(any(test, not(bifrost_proxy_test_io)))]
fn verify_process_identity(expected: &ProcessIdentity, actual: &ProcessIdentity) -> Result<()> {
    if expected.pid == 0 || expected != actual {
        return Err(BifrostError::Config(
            "Proxy restoration originating process changed".into(),
        ));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", not(any(test, bifrost_proxy_test_io))))]
fn execute_native(request: &Request) -> Result<Outcome> {
    let _ownership_lock = native::inherited_lock(&request.lock)?;
    let now = monotonic_millis()?;
    let deadline = bounded_deadline(now, request.deadline_millis)?;
    native::check_owner(&request.owner)?;
    // An unelevated parent cannot kill a root child reliably. Independently
    // enforce the deadline and detect a dead/replaced originating process.
    let owner = request.owner.clone();
    std::thread::Builder::new()
        .name("proxy-restore-deadline".into())
        .spawn(move || loop {
            if monotonic_millis().map_or(true, |now| now >= deadline)
                || native::check_owner(&owner).is_err()
            {
                // SAFETY: dedicated one-shot process; do not run blocking Drop.
                unsafe { libc::_exit(124) }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        })?;
    restore(
        &mut native::Session::new(deadline, request.owner.clone())?,
        request,
    )
}

pub(super) fn monotonic_millis() -> Result<u64> {
    #[cfg(unix)]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: valid initialized output pointer and supported monotonic clock.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok((time.tv_sec as u64).saturating_mul(1000) + time.tv_nsec as u64 / 1_000_000)
    }
    #[cfg(not(unix))]
    {
        // Only mock adapter tests run here; native dispatch is compiled out.
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| BifrostError::Config("Clock precedes epoch".into()))?
            .as_millis() as u64)
    }
}

#[cfg(any(not(target_os = "macos"), test, bifrost_proxy_test_io))]
fn execute_native(_: &Request) -> Result<Outcome> {
    Err(BifrostError::Config(
        "Native SystemConfiguration I/O is forbidden in unit/fixture or non-macOS builds".into(),
    ))
}

#[cfg(all(target_os = "macos", not(any(test, bifrost_proxy_test_io))))]
mod native;
#[cfg(test)]
mod tests;
