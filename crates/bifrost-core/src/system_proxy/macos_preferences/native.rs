//! Minimal public Apple C API bindings. No authorization object or installed
//! privileged helper: the parent supplies its existing bounded privilege mode.
use super::*;
use std::ffi::{c_char, c_void};
use std::ptr;

type Ref = *const c_void;
type Boolean = u8;
const UTF8: u32 = 0x0800_0100;
const SINT64: isize = 4;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ref);
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        a: Ref,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        ext: Boolean,
    ) -> Ref;
    fn CFStringGetLength(value: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
    fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> Boolean;
    fn CFNumberCreate(a: Ref, kind: isize, value: *const c_void) -> Ref;
    fn CFNumberGetValue(value: Ref, kind: isize, result: *mut c_void) -> Boolean;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFDictionaryCreateMutableCopy(a: Ref, capacity: isize, dictionary: Ref) -> Ref;
    fn CFDictionarySetValue(dictionary: Ref, key: Ref, value: Ref);
    fn CFDictionaryRemoveValue(dictionary: Ref, key: Ref);
}

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCError() -> i32;
    fn SCErrorString(status: i32) -> *const c_char;
    fn SCPreferencesCreate(a: Ref, name: Ref, prefs_id: Ref) -> Ref;
    fn SCPreferencesLock(prefs: Ref, wait: Boolean) -> Boolean;
    fn SCPreferencesUnlock(prefs: Ref) -> Boolean;
    fn SCPreferencesCommitChanges(prefs: Ref) -> Boolean;
    fn SCPreferencesApplyChanges(prefs: Ref) -> Boolean;
    fn SCPreferencesSynchronize(prefs: Ref);
    fn SCNetworkSetCopyCurrent(prefs: Ref) -> Ref;
    fn SCNetworkSetCopyServices(set: Ref) -> Ref;
    fn SCNetworkServiceGetName(service: Ref) -> Ref;
    fn SCNetworkServiceCopyProtocol(service: Ref, kind: Ref) -> Ref;
    fn SCNetworkServiceGetServiceID(service: Ref) -> Ref;
    fn SCPreferencesGetValue(prefs: Ref, key: Ref) -> Ref;
    fn SCPreferencesPathSetValue(prefs: Ref, path: Ref, dictionary: Ref) -> Boolean;
}

struct Owned(Ref);
impl Owned {
    fn new(value: Ref, operation: &str) -> Result<Self> {
        if value.is_null() {
            Err(error(operation))
        } else {
            Ok(Self(value))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: Owned contains one non-null Create/Copy result, released once.
        unsafe { CFRelease(self.0) }
    }
}
fn error(operation: &str) -> BifrostError {
    // SAFETY: SCErrorString returns a process-lifetime, nullable C string.
    let (code, description) = unsafe {
        let code = SCError();
        let description = SCErrorString(code);
        let description = if description.is_null() {
            "unknown error".into()
        } else {
            std::ffi::CStr::from_ptr(description)
                .to_string_lossy()
                .into_owned()
        };
        (code, description)
    };
    // kSCStatusAccessError is the public write-authorization failure code.
    let prefix = if code == 1003 { "RequiresAdmin: " } else { "" };
    BifrostError::Config(format!(
        "{prefix}SystemConfiguration {operation} failed ({code}): {description}"
    ))
}
fn checked(value: Boolean, operation: &str) -> Result<()> {
    if value != 0 {
        Ok(())
    } else {
        Err(error(operation))
    }
}
fn invalid(message: &str) -> BifrostError {
    BifrostError::Config(format!(
        "Unexpected SystemConfiguration {message}; refusing restoration"
    ))
}
fn string(value: &str) -> Result<Owned> {
    // SAFETY: UTF-8 bytes remain valid for this copying call; null allocator is default.
    Owned::new(
        unsafe {
            CFStringCreateWithBytes(ptr::null(), value.as_ptr(), value.len() as isize, UTF8, 0)
        },
        "create string",
    )
}
fn read_string(value: Ref) -> Result<String> {
    // SAFETY: call sites supply live CF references; validate dynamic type first.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return Err(invalid("string"));
        }
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8) + 1;
        if !(1..=1024 * 1024).contains(&size) {
            return Err(invalid("string size"));
        }
        let mut bytes = vec![0; size as usize];
        if CFStringGetCString(value, bytes.as_mut_ptr().cast(), size, UTF8) == 0 {
            return Err(invalid("UTF-8 string"));
        }
        let end = bytes
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| invalid("string terminator"))?;
        let decoded =
            String::from_utf8(bytes[..end].to_vec()).map_err(|_| invalid("UTF-8 string"))?;
        if decoded.encode_utf16().count() != CFStringGetLength(value) as usize {
            return Err(invalid("embedded-NUL string"));
        }
        Ok(decoded)
    }
}
fn get(dictionary: Ref, key: &str) -> Result<Ref> {
    let key = string(key)?;
    // SAFETY: dictionary is a checked live dictionary; key remains live.
    Ok(unsafe { CFDictionaryGetValue(dictionary, key.0) })
}
fn plain_dictionary(value: Ref) -> Result<Ref> {
    // SAFETY: callers supply a live CF property-list value or null.
    if value.is_null() || unsafe { CFGetTypeID(value) != CFDictionaryGetTypeID() } {
        return Err(invalid("preference-path dictionary"));
    }
    if !get(value, "__LINK__")?.is_null() {
        return Err(invalid("linked proxy preference path"));
    }
    Ok(value)
}
fn property(dictionary: Ref, key: &str) -> Result<Property> {
    let value = get(dictionary, key)?;
    if value.is_null() {
        return Ok(Property::Missing);
    }
    // SAFETY: value is borrowed from a live checked dictionary; validate type.
    unsafe {
        if CFGetTypeID(value) == CFStringGetTypeID() {
            return read_string(value).map(Property::Text);
        }
        if CFGetTypeID(value) == CFNumberGetTypeID() {
            let mut result = 0i64;
            if CFNumberGetValue(value, SINT64, (&mut result as *mut i64).cast()) != 0 {
                return Ok(Property::Number(result));
            }
        }
    }
    Ok(Property::Other)
}
fn prefix(field: Field) -> Result<&'static str> {
    match field {
        Field::Http => Ok("HTTP"),
        Field::Https => Ok("HTTPS"),
        Field::Bypass => Err(invalid("non-HTTP(S) field")),
    }
}

pub(super) fn inherited_lock(expected: &LockIdentity) -> Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    // FromRawFd requires an open owned descriptor, including for malformed
    // direct invocations of this private mode with stdin explicitly closed.
    if unsafe { libc::fcntl(0, libc::F_GETFD) } < 0 {
        return Err(invalid("missing inherited ownership lock"));
    }
    // SAFETY: this dedicated helper owns stdin. Never reopen its pathname or
    // LOCK_UN: fd0 is the parent's duplicated flock open-file-description.
    let file = unsafe { std::fs::File::from_raw_fd(0) };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
    {
        return Err(invalid("inherited ownership lock"));
    }
    // This must already be the same owned description; confirming it is
    // nonblocking protects against a competing owner without releasing it.
    if unsafe { libc::flock(0, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(invalid("unavailable inherited ownership lock"));
    }
    Ok(file)
}

pub(super) fn process_identity(pid: u32) -> Result<ProcessIdentity> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| invalid("originating PID"))?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: public libproc flavor matches this full output structure.
    let actual = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if actual != size {
        return Err(invalid("unavailable originating process"));
    }
    // SAFETY: proc_pidinfo initialized the complete structure on success.
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid as u32 || info.pbi_status == libc::SZOMB {
        return Err(invalid("terminated originating process"));
    }
    Ok(ProcessIdentity {
        pid: info.pbi_pid,
        started_sec: info.pbi_start_tvsec,
        started_usec: info.pbi_start_tvusec,
    })
}
pub(super) fn check_owner(owner: &ProcessIdentity) -> Result<()> {
    verify_process_identity(owner, &process_identity(owner.pid)?)
}

pub(super) struct Session {
    preferences: Owned,
    locked: bool,
    deadline_millis: u64,
    owner: ProcessIdentity,
}
pub(super) struct Dictionary {
    path: Owned,
    values: Owned,
}
impl Session {
    pub(super) fn new(deadline_millis: u64, owner: ProcessIdentity) -> Result<Self> {
        let name = string("Bifrost dormant proxy restoration")?;
        // SAFETY: inputs are live CFString/default allocator/default preferences.
        let preferences = Owned::new(
            unsafe { SCPreferencesCreate(ptr::null(), name.0, ptr::null()) },
            "create preferences",
        )?;
        Ok(Self {
            preferences,
            locked: false,
            deadline_millis,
            owner,
        })
    }
}
impl Session {
    fn check_deadline(&self) -> Result<()> {
        check_owner(&self.owner)?;
        if monotonic_millis()? >= self.deadline_millis {
            return Err(BifrostError::Config(
                "Proxy restoration request expired".into(),
            ));
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.locked {
            // SAFETY: this session successfully acquired this live preferences lock.
            unsafe {
                SCPreferencesUnlock(self.preferences.0);
            }
        }
    }
}
impl Preferences for Session {
    type Dictionary = Dictionary;
    fn lock(&mut self) -> Result<()> {
        self.check_deadline()?;
        // Nonblocking lock, additionally bounded by the parent process timeout.
        checked(unsafe { SCPreferencesLock(self.preferences.0, 0) }, "lock")?;
        self.locked = true;
        Ok(())
    }
    fn unlock(&mut self) -> Result<()> {
        checked(unsafe { SCPreferencesUnlock(self.preferences.0) }, "unlock")?;
        self.locked = false;
        Ok(())
    }
    fn read(&mut self, name: &str) -> Result<Dictionary> {
        self.check_deadline()?;
        // SAFETY: all references returned by Copy functions are owned locally;
        // array members remain borrowed only while the owning array is live.
        unsafe {
            let set = Owned::new(SCNetworkSetCopyCurrent(self.preferences.0), "current set")?;
            let services = Owned::new(SCNetworkSetCopyServices(set.0), "current services")?;
            let mut matched = None;
            for index in 0..CFArrayGetCount(services.0) {
                let service = CFArrayGetValueAtIndex(services.0, index);
                let service_name = SCNetworkServiceGetName(service);
                if service_name.is_null() {
                    continue;
                }
                if read_string(service_name)? == name {
                    if matched.is_some() {
                        return Err(invalid("duplicate service name"));
                    }
                    matched = Some(service);
                }
            }
            let service = matched.ok_or_else(|| invalid("missing/renamed current-set service"))?;
            let kind = string("Proxies")?;
            let _protocol = Owned::new(
                SCNetworkServiceCopyProtocol(service, kind.0),
                "copy proxy protocol",
            )?;
            let service_id = read_string(SCNetworkServiceGetServiceID(service))?;
            if service_id.is_empty() || service_id.contains('/') {
                return Err(invalid("service identifier"));
            }
            // Public preference-path API avoids the protocol setter's
            // normalization of the unrelated __INACTIVE__ dictionary key.
            let path = string(&format!("/NetworkServices/{service_id}/Proxies"))?;
            // PathGetValue follows __LINK__, but PathSetValue may replace a
            // terminal link. Inspect raw ancestry and reject any redirection so
            // no linked configuration is flattened or changed at another path.
            let root_key = string("NetworkServices")?;
            let root = plain_dictionary(SCPreferencesGetValue(self.preferences.0, root_key.0))?;
            let service_values = plain_dictionary(get(root, &service_id)?)?;
            let values = plain_dictionary(get(service_values, "Proxies")?)?;
            let values = Owned::new(
                CFDictionaryCreateMutableCopy(ptr::null(), 0, values),
                "copy proxy dictionary",
            )?;
            Ok(Dictionary { path, values })
        }
    }
    fn protocol(&self, dictionary: &Dictionary, field: Field) -> Result<Protocol> {
        protocol_from_properties(field, |key| property(dictionary.values.0, key))
    }

    fn replace_endpoint(
        &mut self,
        dictionary: Dictionary,
        field: Field,
        desired: &Protocol,
    ) -> Result<()> {
        self.check_deadline()?;
        let prefix = prefix(field)?;
        let host_key = string(&format!("{prefix}Proxy"))?;
        let port_key = string(&format!("{prefix}Port"))?;
        // Only these two HTTP(S) endpoint keys are owned here. Removing an
        // empty/zero key restores networksetup's empty/0 representation without
        // changing its disabled flag, authentication, PAC, SOCKS or bypass.
        unsafe {
            if desired.host.is_empty() {
                CFDictionaryRemoveValue(dictionary.values.0, host_key.0);
            } else {
                let host = string(&desired.host)?;
                CFDictionarySetValue(dictionary.values.0, host_key.0, host.0);
            }
            if desired.port == 0 {
                CFDictionaryRemoveValue(dictionary.values.0, port_key.0);
            } else {
                let port = i64::from(desired.port);
                let value = Owned::new(
                    CFNumberCreate(ptr::null(), SINT64, (&port as *const i64).cast()),
                    "create port",
                )?;
                CFDictionarySetValue(dictionary.values.0, port_key.0, value.0);
            }
            checked(
                SCPreferencesPathSetValue(
                    self.preferences.0,
                    dictionary.path.0,
                    dictionary.values.0,
                ),
                "set proxy endpoint",
            )
        }
    }
    fn commit(&mut self) -> Result<()> {
        self.check_deadline()?;
        checked(
            unsafe { SCPreferencesCommitChanges(self.preferences.0) },
            "commit",
        )
    }
    fn apply(&mut self) -> Result<()> {
        self.check_deadline()?;
        checked(
            unsafe { SCPreferencesApplyChanges(self.preferences.0) },
            "apply",
        )
    }
    fn synchronize(&mut self) {
        unsafe { SCPreferencesSynchronize(self.preferences.0) }
    }
}
