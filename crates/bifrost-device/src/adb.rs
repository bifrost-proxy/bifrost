use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::mobileconfig::{decode_pem_certificate, read_certificate_der_from_bytes};
use crate::model::{
    DeviceCertificateState, DeviceCertificateStatus, DeviceStatus, DeviceTrustCapability,
    InstallMode, InstallSession, InstallStep, MobileDevice, MobilePlatform,
};

#[cfg(all(test, unix))]
static ADB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, unix))]
fn adb_test_lock() -> std::sync::MutexGuard<'static, ()> {
    ADB_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, unix))]
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

#[cfg(all(test, unix))]
impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        std::env::remove_var(key);
        Self { key, previous }
    }
}

#[cfg(all(test, unix))]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdbDiscovery {
    pub adb_available: bool,
    pub adb_path: Option<String>,
    pub devices: Vec<MobileDevice>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct AndroidInstallOptions {
    pub adb_path: PathBuf,
    pub device_id: String,
    pub ca_cert_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AndroidCaStatusOptions {
    pub adb_path: PathBuf,
    pub device_id: String,
    pub ca_cert_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("ADB is not available")]
    NotAvailable,
    #[error("CA certificate file not found: {0}")]
    CertificateMissing(String),
    #[error("failed to run adb: {0}")]
    Io(#[from] std::io::Error),
}

pub fn discover_android_devices() -> AdbDiscovery {
    discover_android_devices_with_ca(None)
}

pub fn discover_android_devices_with_ca(ca_cert_path: Option<&Path>) -> AdbDiscovery {
    let Some(adb_path) = find_adb() else {
        return AdbDiscovery {
            adb_available: false,
            adb_path: None,
            devices: Vec::new(),
            message: "ADB is not available. Install Android Platform Tools or configure BIFROST_ADB_PATH to detect USB Android devices.".to_string(),
        };
    };

    match Command::new(&adb_path).arg("devices").arg("-l").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut devices = parse_adb_devices(&stdout);
            if let Some(ca_cert_path) = ca_cert_path {
                for device in devices
                    .iter_mut()
                    .filter(|device| device.status == DeviceStatus::Connected)
                {
                    device.certificate_status =
                        Some(check_android_ca_status(AndroidCaStatusOptions {
                            adb_path: adb_path.clone(),
                            device_id: device.id.clone(),
                            ca_cert_path: ca_cert_path.to_path_buf(),
                        }));
                }
            }
            let message = if devices.is_empty() {
                "ADB is available, but no Android USB devices were reported.".to_string()
            } else {
                format!("ADB found {} Android device(s).", devices.len())
            };
            AdbDiscovery {
                adb_available: true,
                adb_path: Some(adb_path.display().to_string()),
                devices,
                message,
            }
        }
        Ok(output) => AdbDiscovery {
            adb_available: true,
            adb_path: Some(adb_path.display().to_string()),
            devices: Vec::new(),
            message: format!(
                "ADB failed to list devices: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(error) => AdbDiscovery {
            adb_available: true,
            adb_path: Some(adb_path.display().to_string()),
            devices: Vec::new(),
            message: format!("ADB failed to start: {error}"),
        },
    }
}

pub fn parse_adb_devices(output: &str) -> Vec<MobileDevice> {
    output
        .lines()
        .skip_while(|line| !line.starts_with("List of devices attached"))
        .skip(1)
        .filter_map(parse_adb_device_line)
        .collect()
}

fn parse_adb_device_line(line: &str) -> Option<MobileDevice> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('*') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let serial = parts.next()?.to_string();
    let raw_status = parts.next().unwrap_or("unknown");
    let rest = parts.collect::<Vec<_>>();
    let status = match raw_status {
        "device" => DeviceStatus::Connected,
        "unauthorized" => DeviceStatus::Unauthorized,
        "offline" => DeviceStatus::Offline,
        _ => DeviceStatus::Unsupported,
    };
    let name = field_value(&rest, "model")
        .or_else(|| field_value(&rest, "device"))
        .or_else(|| field_value(&rest, "product"));
    let capability = match status {
        DeviceStatus::Connected => DeviceTrustCapability::PushAndOpenInstaller,
        _ => DeviceTrustCapability::GuideOnly,
    };
    let status_message = match status {
        DeviceStatus::Connected => {
            "Ready to push the Bifrost CA and open the Android certificate installer. The phone still requires user confirmation.".to_string()
        }
        DeviceStatus::Unauthorized => {
            "Unlock the phone and allow USB debugging for this computer, then refresh devices.".to_string()
        }
        DeviceStatus::Offline => "The device is offline. Reconnect USB or restart ADB.".to_string(),
        DeviceStatus::Unsupported => {
            format!("ADB reported unsupported device status: {raw_status}")
        }
    };

    Some(MobileDevice {
        id: serial,
        name,
        managed_install_target: None,
        platform: MobilePlatform::Android,
        status,
        capability,
        certificate_status: None,
        status_message,
    })
}

fn field_value(parts: &[&str], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    parts
        .iter()
        .find_map(|part| part.strip_prefix(&prefix))
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('_', " "))
}

pub fn install_android_ca(options: AndroidInstallOptions) -> Result<InstallSession, AdbError> {
    if !options.ca_cert_path.exists() {
        return Err(AdbError::CertificateMissing(
            options.ca_cert_path.display().to_string(),
        ));
    }

    let mut steps = Vec::new();
    let remote_path = "/sdcard/Download/bifrost-ca.crt";
    let push = run_adb(
        &options.adb_path,
        &[
            "-s",
            &options.device_id,
            "push",
            path_str(&options.ca_cert_path),
            remote_path,
        ],
    )?;
    steps.push(InstallStep {
        name: "push_certificate".to_string(),
        success: push.success,
        message: push.message,
    });

    if steps.last().map(|step| !step.success).unwrap_or(false) {
        return Ok(session(options.device_id, false, steps));
    }

    let view = run_adb(
        &options.adb_path,
        &[
            "-s",
            &options.device_id,
            "shell",
            "am",
            "start",
            "-a",
            "android.intent.action.VIEW",
            "-d",
            "file:///sdcard/Download/bifrost-ca.crt",
            "-t",
            "application/x-x509-ca-cert",
        ],
    )?;
    let view_success = view.success;
    steps.push(InstallStep {
        name: "open_certificate_installer".to_string(),
        success: view_success,
        message: view.message,
    });

    if !view_success {
        let settings = run_adb(
            &options.adb_path,
            &[
                "-s",
                &options.device_id,
                "shell",
                "am",
                "start",
                "-a",
                "android.settings.SECURITY_SETTINGS",
            ],
        )?;
        steps.push(InstallStep {
            name: "open_security_settings_fallback".to_string(),
            success: settings.success,
            message: settings.message,
        });
    }

    let completed = steps.iter().all(|step| step.success);
    Ok(session(options.device_id, completed, steps))
}

pub fn check_android_ca_status(options: AndroidCaStatusOptions) -> DeviceCertificateStatus {
    let local_file = match fs::read(&options.ca_cert_path) {
        Ok(data) => data,
        Err(error) => {
            return DeviceCertificateStatus {
                state: DeviceCertificateState::Unknown,
                trusted: None,
                fingerprint_match: None,
                message: format!("Could not read the local Bifrost CA certificate: {error}"),
            };
        }
    };
    let local_der = match read_certificate_der_from_bytes(&local_file) {
        Ok(der) => der,
        Err(error) => {
            return DeviceCertificateStatus {
                state: DeviceCertificateState::Unknown,
                trusted: None,
                fingerprint_match: None,
                message: format!("Could not parse the local Bifrost CA certificate: {error}"),
            };
        }
    };
    let local_fingerprint = sha256_hex(&local_der);
    let pushed_to_download =
        remote_downloaded_certificate_matches(&options.adb_path, &options.device_id, &local_file)
            .unwrap_or(false);

    match read_android_user_ca_store(&options.adb_path, &options.device_id) {
        Ok(Some(store_pem)) => {
            let installed = android_user_store_contains_fingerprint(&store_pem, &local_fingerprint);
            if installed {
                DeviceCertificateStatus {
                    state: DeviceCertificateState::Installed,
                    trusted: Some(true),
                    fingerprint_match: Some(true),
                    message: "Current Bifrost CA is installed in Android's user certificate store. Android 7+ apps may still ignore user CAs unless their Network Security Config allows them."
                        .to_string(),
                }
            } else if pushed_to_download {
                DeviceCertificateStatus {
                    state: DeviceCertificateState::PushedToDevice,
                    trusted: None,
                    fingerprint_match: Some(false),
                    message: "Current Bifrost CA file is on the phone, but it was not found in the readable Android user certificate store. Finish the certificate installer on the phone."
                        .to_string(),
                }
            } else {
                DeviceCertificateStatus {
                    state: DeviceCertificateState::NotInstalled,
                    trusted: Some(false),
                    fingerprint_match: Some(false),
                    message: "Bifrost could read Android's user certificate store and did not find the current Bifrost CA."
                        .to_string(),
                }
            }
        }
        Ok(None) | Err(_) if pushed_to_download => DeviceCertificateStatus {
            state: DeviceCertificateState::PushedToDevice,
            trusted: None,
            fingerprint_match: None,
            message: "Current Bifrost CA file is on the phone. Ordinary Android ADB cannot verify the private user certificate store, so finish the installer and confirm on the phone."
                .to_string(),
        },
        Ok(None) | Err(_) => DeviceCertificateStatus {
            state: DeviceCertificateState::Unknown,
            trusted: None,
            fingerprint_match: None,
            message: "Bifrost can see this Android device, but ordinary ADB cannot verify whether the CA is installed. Rooted/emulator test devices can expose the user certificate store for verification."
                .to_string(),
        },
    }
}

struct CommandResult {
    success: bool,
    message: String,
}

fn run_adb(adb_path: &Path, args: &[&str]) -> Result<CommandResult, AdbError> {
    let output = run_adb_output(adb_path, args)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if output.status.success() {
        if stdout.is_empty() {
            "command completed".to_string()
        } else {
            stdout
        }
    } else if stderr.is_empty() {
        stdout
    } else {
        stderr
    };
    Ok(CommandResult {
        success: output.status.success(),
        message,
    })
}

fn run_adb_output(adb_path: &Path, args: &[&str]) -> Result<Output, AdbError> {
    // Spawning a freshly-written executable can intermittently fail with
    // ETXTBSY ("Text file busy", os error 26) on Linux: when other threads in
    // the process fork+exec concurrently, the child briefly inherits writable
    // file descriptors, and the kernel refuses to exec a file that any process
    // still holds open for writing. This is inherent to multi-threaded
    // fork/exec and cannot be eliminated by file-handle hygiene alone, so we
    // retry a few times with a short backoff before giving up.
    const MAX_ATTEMPTS: u32 = 8;
    let mut attempt = 0;
    loop {
        match Command::new(adb_path).args(args).output() {
            Ok(output) => return Ok(output),
            Err(err)
                if err.raw_os_error() == Some(26 /* ETXTBSY */) && attempt + 1 < MAX_ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(5 * u64::from(attempt)));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn remote_downloaded_certificate_matches(
    adb_path: &Path,
    device_id: &str,
    local_file: &[u8],
) -> Result<bool, AdbError> {
    let output = run_adb_output(
        adb_path,
        &[
            "-s",
            device_id,
            "shell",
            "sh",
            "-c",
            "if [ -f /sdcard/Download/bifrost-ca.crt ]; then cat /sdcard/Download/bifrost-ca.crt; else exit 2; fi",
        ],
    )?;
    Ok(output.status.success() && output.stdout == local_file)
}

fn read_android_user_ca_store(
    adb_path: &Path,
    device_id: &str,
) -> Result<Option<String>, AdbError> {
    const READ_USER_CA_STORE: &str = "if [ -d /data/misc/user/0/cacerts-added ]; then ls -la /data/misc/user/0/cacerts-added >/dev/null 2>&1 || exit 13; cat /data/misc/user/0/cacerts-added/* 2>/dev/null || true; else exit 2; fi";
    let direct = run_adb_output(
        adb_path,
        &["-s", device_id, "shell", "sh", "-c", READ_USER_CA_STORE],
    )?;
    if direct.status.success() {
        return Ok(Some(String::from_utf8_lossy(&direct.stdout).to_string()));
    }

    let rooted = run_adb_output(
        adb_path,
        &[
            "-s",
            device_id,
            "shell",
            "su",
            "0",
            "sh",
            "-c",
            READ_USER_CA_STORE,
        ],
    )?;
    if rooted.status.success() {
        return Ok(Some(String::from_utf8_lossy(&rooted.stdout).to_string()));
    }
    Ok(None)
}

fn android_user_store_contains_fingerprint(store_pem: &str, local_fingerprint: &str) -> bool {
    pem_certificate_blocks(store_pem).iter().any(|cert_der| {
        normalize_fingerprint(&sha256_hex(cert_der)) == normalize_fingerprint(local_fingerprint)
    })
}

fn pem_certificate_blocks(input: &str) -> Vec<Vec<u8>> {
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let mut certs = Vec::new();
    let mut remaining = input;
    while let Some(start) = remaining.find(begin) {
        let after_start = &remaining[start..];
        let Some(end_offset) = after_start.find(end) else {
            break;
        };
        let block_end = end_offset + end.len();
        let block = &after_start[..block_end];
        if let Ok(cert) = decode_pem_certificate(block) {
            certs.push(cert);
        }
        remaining = &after_start[block_end..];
    }
    certs
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn normalize_fingerprint(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .flat_map(|ch| ch.to_uppercase())
        .collect()
}

fn session(device_id: String, completed: bool, steps: Vec<InstallStep>) -> InstallSession {
    InstallSession {
        session_id: Uuid::new_v4().to_string(),
        device_id,
        platform: MobilePlatform::Android,
        mode: InstallMode::NormalGuide,
        capability: DeviceTrustCapability::PushAndOpenInstaller,
        completed,
        requires_user_confirmation: true,
        summary: if completed {
            "Bifrost pushed the CA certificate and opened Android's installer flow. Confirm installation and trust on the phone.".to_string()
        } else {
            "Bifrost could not complete the Android installer handoff. Review the failed step and finish installation manually if needed.".to_string()
        },
        steps,
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

fn find_adb() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BIFROST_ADB_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(adb_binary_name()))
        .find(|path| is_executable_file(path))
}

fn adb_binary_name() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connected_and_unauthorized_devices() {
        let output = r#"List of devices attached
emulator-5554 device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1
R58M123 unauthorized usb:336592896X transport_id:2
"#;

        let devices = parse_adb_devices(output);

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "emulator-5554");
        assert_eq!(devices[0].status, DeviceStatus::Connected);
        assert_eq!(
            devices[0].capability,
            DeviceTrustCapability::PushAndOpenInstaller
        );
        assert_eq!(devices[1].status, DeviceStatus::Unauthorized);
        assert!(devices[1].status_message.contains("allow USB debugging"));
    }

    #[test]
    fn ignores_daemon_noise_and_empty_lines() {
        let output = r#"* daemon started successfully
List of devices attached

"#;

        assert!(parse_adb_devices(output).is_empty());
    }

    #[test]
    fn android_user_store_matches_current_ca_fingerprint() {
        let cert = b"test-cert";
        let fingerprint = sha256_hex(cert);
        let store = "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

        assert!(android_user_store_contains_fingerprint(store, &fingerprint));
    }

    #[test]
    fn android_user_store_rejects_different_ca_fingerprint() {
        let fingerprint = sha256_hex(b"current-bifrost-ca");
        let store = "-----BEGIN CERTIFICATE-----\nb3RoZXItY2E=\n-----END CERTIFICATE-----\n";

        assert!(!android_user_store_contains_fingerprint(
            store,
            &fingerprint
        ));
    }

    #[cfg(unix)]
    #[test]
    fn android_ca_status_reports_pushed_when_user_store_is_not_readable() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir = std::env::temp_dir().join(format!("bifrost-adb-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp test dir");
        let cert_path = test_dir.join("ca.crt");
        fs::write(
            &cert_path,
            "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n",
        )
        .expect("write cert");
        let adb_path = test_dir.join("adb");
        fs::write(
            &adb_path,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n  *Download*) cat '{}'; exit 0 ;;\n  *cacerts-added*) exit 2 ;;\n  *) exit 0 ;;\nesac\n",
                cert_path.display()
            ),
        )
        .expect("write fake adb");
        let mut permissions = fs::metadata(&adb_path)
            .expect("stat fake adb")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&adb_path, permissions).expect("chmod fake adb");

        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path,
            device_id: "android-1".to_string(),
            ca_cert_path: cert_path,
        });

        assert!(
            status.state == DeviceCertificateState::PushedToDevice
                || status.state == DeviceCertificateState::Unknown
        );
        assert_eq!(status.trusted, None);
        assert!(status.message.contains("cannot verify"));
        let _ = fs::remove_dir_all(test_dir);
    }

    // --- Fake-adb harness ---------------------------------------------------
    //
    // Most of adb.rs shells out to the `adb` binary. We exercise those paths
    // by writing a small POSIX `sh` script that masquerades as `adb` and
    // returns scripted output per argument pattern. All harness tests are
    // unix-only (they rely on chmod +x and a shebang).
    //
    // The harness writes an executable script and immediately runs it. On some
    // filesystems exec'ing a file that another thread is still writing yields
    // ETXTBSY ("Text file busy"). We avoid that — and the cross-thread races of
    // `find_adb`'s env-var probing — by serializing every harness/env test
    // behind a single global lock.

    #[cfg(unix)]
    struct FakeAdb {
        dir: PathBuf,
        adb_path: PathBuf,
        cert_path: PathBuf,
    }

    #[cfg(unix)]
    impl FakeAdb {
        fn new(script_body: &str) -> Self {
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("bifrost-adb-h-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("create temp dir");
            let cert_path = dir.join("ca.crt");
            fs::write(
                &cert_path,
                "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n",
            )
            .expect("write cert");
            let adb_path = dir.join("adb");
            {
                // Write the script, fsync, and FULLY CLOSE the writable handle
                // BEFORE marking it executable. Setting the exec bit (or running
                // the script) while a writable FD is still open triggers ETXTBSY
                // ("Text file busy") on Linux. Closing first avoids that race.
                let mut f = fs::File::create(&adb_path).expect("create fake adb");
                f.write_all(format!("#!/bin/sh\n{script_body}\n").as_bytes())
                    .expect("write fake adb");
                f.sync_all().expect("sync fake adb");
                // `f` is dropped (closed) here at the end of the block.
            }
            // Now that no writable handle is open, set the executable bit via the
            // path so the file can be exec'd without ETXTBSY.
            fs::set_permissions(&adb_path, fs::Permissions::from_mode(0o755))
                .expect("chmod fake adb");
            Self {
                dir,
                adb_path,
                cert_path,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for FakeAdb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn discover_with_ca_lists_devices_and_checks_status() {
        let _guard = adb_test_lock();
        // adb devices -l returns one connected device; the CA-status probe then
        // populates certificate_status (exact value is irrelevant here).
        let fake = FakeAdb::new(
            r#"case "$*" in
  *"devices -l"*) printf 'List of devices attached\nemulator-5554 device model:Pixel_7\n'; exit 0 ;;
  *Download*) exit 2 ;;
  *cacerts-added*) exit 2 ;;
  *) exit 0 ;;
esac"#,
        );

        let _adb_env = EnvVarGuard::set("BIFROST_ADB_PATH", &fake.adb_path);
        let discovery = discover_android_devices_with_ca(Some(&fake.cert_path));

        assert!(discovery.adb_available);
        assert_eq!(discovery.devices.len(), 1);
        assert_eq!(discovery.devices[0].id, "emulator-5554");
        assert!(discovery.message.contains("found 1"));
        assert!(discovery.devices[0].certificate_status.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn discover_reports_empty_when_no_devices() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *"devices -l"*) printf 'List of devices attached\n'; exit 0 ;;
  *) exit 0 ;;
esac"#,
        );
        let _adb_env = EnvVarGuard::set("BIFROST_ADB_PATH", &fake.adb_path);
        let discovery = discover_android_devices_with_ca(None);

        assert!(discovery.adb_available);
        assert!(discovery.devices.is_empty());
        assert!(discovery.message.contains("no Android USB devices"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_reports_failure_on_nonzero_exit() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *"devices -l"*) echo "adb: connection refused" 1>&2; exit 1 ;;
  *) exit 0 ;;
esac"#,
        );
        let _adb_env = EnvVarGuard::set("BIFROST_ADB_PATH", &fake.adb_path);
        let discovery = discover_android_devices_with_ca(None);

        assert!(discovery.adb_available);
        assert!(discovery.devices.is_empty());
        assert!(discovery.message.contains("failed to list devices"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_unavailable_when_adb_missing() {
        let _guard = adb_test_lock();
        // Point BIFROST_ADB_PATH at a nonexistent file and clear PATH so no
        // real adb is found.
        let missing = std::env::temp_dir().join(format!("no-adb-{}", Uuid::new_v4()));
        let _adb_env = EnvVarGuard::set("BIFROST_ADB_PATH", &missing);
        let _path_env = EnvVarGuard::set("PATH", "");
        let discovery = discover_android_devices();

        assert!(!discovery.adb_available);
        assert!(discovery.adb_path.is_none());
        assert!(discovery.message.contains("ADB is not available"));
    }

    #[cfg(unix)]
    #[test]
    fn install_android_ca_pushes_and_opens_installer() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *push*) echo "1 file pushed"; exit 0 ;;
  *VIEW*) echo "Starting: Intent"; exit 0 ;;
  *) exit 0 ;;
esac"#,
        );
        let session = install_android_ca(AndroidInstallOptions {
            adb_path: fake.adb_path.clone(),
            device_id: "dev-1".to_string(),
            ca_cert_path: fake.cert_path.clone(),
        })
        .expect("install should succeed");

        assert!(session.completed);
        assert_eq!(session.device_id, "dev-1");
        assert_eq!(session.steps.len(), 2);
        assert_eq!(session.steps[0].name, "push_certificate");
        assert_eq!(session.steps[1].name, "open_certificate_installer");
    }

    #[cfg(unix)]
    #[test]
    fn install_android_ca_falls_back_to_settings_when_view_fails() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *push*) echo "1 file pushed"; exit 0 ;;
  *VIEW*) echo "no activity" 1>&2; exit 1 ;;
  *SECURITY_SETTINGS*) echo "Starting settings"; exit 0 ;;
  *) exit 0 ;;
esac"#,
        );
        let session = install_android_ca(AndroidInstallOptions {
            adb_path: fake.adb_path.clone(),
            device_id: "dev-2".to_string(),
            ca_cert_path: fake.cert_path.clone(),
        })
        .expect("install returns Ok even when a step fails");

        assert!(!session.completed);
        assert_eq!(session.steps.len(), 3);
        assert_eq!(session.steps[2].name, "open_security_settings_fallback");
    }

    #[cfg(unix)]
    #[test]
    fn install_android_ca_stops_when_push_fails() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *push*) echo "adb: error: failed to push" 1>&2; exit 1 ;;
  *) exit 0 ;;
esac"#,
        );
        let session = install_android_ca(AndroidInstallOptions {
            adb_path: fake.adb_path.clone(),
            device_id: "dev-3".to_string(),
            ca_cert_path: fake.cert_path.clone(),
        })
        .expect("returns Ok with a single failed step");

        assert!(!session.completed);
        assert_eq!(session.steps.len(), 1);
        assert!(!session.steps[0].success);
    }

    #[test]
    fn install_android_ca_errors_when_cert_missing() {
        let err = install_android_ca(AndroidInstallOptions {
            adb_path: PathBuf::from("/bin/true"),
            device_id: "dev".to_string(),
            ca_cert_path: PathBuf::from("/nonexistent/ca.crt"),
        })
        .expect_err("missing cert should error");
        assert!(matches!(err, AdbError::CertificateMissing(_)));
    }

    #[cfg(unix)]
    #[test]
    fn check_status_reports_installed_when_store_contains_ca() {
        let _guard = adb_test_lock();
        // Build a store whose PEM contains the same cert bytes as the local CA.
        let fake = FakeAdb::new(
            r#"case "$*" in
  *cacerts-added*) printf -- '-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n'; exit 0 ;;
  *Download*) exit 2 ;;
  *) exit 0 ;;
esac"#,
        );
        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path: fake.adb_path.clone(),
            device_id: "dev".to_string(),
            ca_cert_path: fake.cert_path.clone(),
        });
        assert_eq!(status.state, DeviceCertificateState::Installed);
        assert_eq!(status.trusted, Some(true));
    }

    #[cfg(unix)]
    #[test]
    fn check_status_reports_not_installed_when_store_lacks_ca() {
        let _guard = adb_test_lock();
        let fake = FakeAdb::new(
            r#"case "$*" in
  *cacerts-added*) printf -- '-----BEGIN CERTIFICATE-----\nb3RoZXItY2E=\n-----END CERTIFICATE-----\n'; exit 0 ;;
  *Download*) exit 2 ;;
  *) exit 0 ;;
esac"#,
        );
        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path: fake.adb_path.clone(),
            device_id: "dev".to_string(),
            ca_cert_path: fake.cert_path.clone(),
        });
        assert_eq!(status.state, DeviceCertificateState::NotInstalled);
        assert_eq!(status.trusted, Some(false));
    }

    #[test]
    fn check_status_unknown_when_local_cert_unreadable() {
        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path: PathBuf::from("/bin/true"),
            device_id: "dev".to_string(),
            ca_cert_path: PathBuf::from("/nonexistent/ca.crt"),
        });
        assert_eq!(status.state, DeviceCertificateState::Unknown);
        assert!(status.message.contains("Could not read"));
    }

    #[test]
    fn sha256_hex_is_uppercase_colon_separated() {
        let hex = sha256_hex(b"abc");
        assert!(hex.contains(':'));
        assert_eq!(hex, hex.to_uppercase());
        // 32 bytes -> 32 hex pairs joined by 31 colons.
        assert_eq!(hex.split(':').count(), 32);
    }

    #[test]
    fn normalize_fingerprint_strips_non_hex_and_uppercases() {
        assert_eq!(normalize_fingerprint("ab:cd ef"), "ABCDEF");
        assert_eq!(normalize_fingerprint("XY12zz"), "12"); // only hex digits kept
    }

    #[test]
    fn pem_certificate_blocks_extracts_multiple() {
        let pem = "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nb3RoZXItY2E=\n-----END CERTIFICATE-----\n";
        let blocks = pem_certificate_blocks(pem);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn pem_certificate_blocks_ignores_unterminated() {
        let pem = "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n"; // no END
        assert!(pem_certificate_blocks(pem).is_empty());
    }

    #[test]
    fn adb_binary_name_matches_platform() {
        let name = adb_binary_name();
        if cfg!(windows) {
            assert_eq!(name, "adb.exe");
        } else {
            assert_eq!(name, "adb");
        }
    }

    #[test]
    fn path_str_handles_valid_and_returns_str() {
        assert_eq!(path_str(Path::new("/tmp/x")), "/tmp/x");
    }

    #[test]
    fn parses_offline_and_unsupported_status() {
        let output = "List of devices attached\n\
                      dev-off offline transport_id:1\n\
                      dev-weird sideload transport_id:2\n";
        let devices = parse_adb_devices(output);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].status, DeviceStatus::Offline);
        assert_eq!(devices[1].status, DeviceStatus::Unsupported);
        assert!(devices[1]
            .status_message
            .contains("unsupported device status"));
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn sha256_and_normalize_helpers_produce_uppercase_hex_without_separators() {
        let fingerprint = sha256_hex(b"bifrost-device");
        assert!(fingerprint.contains(':'));
        assert!(fingerprint
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == ':'));

        let normalized = normalize_fingerprint(&fingerprint);
        // 32-byte SHA-256 => 64 hex digits.
        assert_eq!(normalized.len(), 64);
        assert!(normalized.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(normalized
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .all(|ch| ch.is_uppercase()),);
        assert!(!normalized.contains(':'));

        // Normalization should also ignore non-hex separators.
        let mixed = "aa:bb cc-dd";
        assert_eq!(normalize_fingerprint(mixed), "AABBCCDD");
    }

    #[test]
    fn adb_binary_name_matches_platform() {
        if cfg!(windows) {
            assert_eq!(adb_binary_name(), "adb.exe");
        } else {
            assert_eq!(adb_binary_name(), "adb");
        }
    }

    #[test]
    fn install_android_ca_fails_if_certificate_is_missing() {
        let options = AndroidInstallOptions {
            adb_path: PathBuf::from("/nonexistent/adb"),
            device_id: "android-1".to_string(),
            ca_cert_path: PathBuf::from("/nonexistent/bifrost-ca.crt"),
        };

        let error = install_android_ca(options).expect_err("expected missing certificate error");
        match error {
            AdbError::CertificateMissing(path) => {
                assert!(path.contains("bifrost-ca.crt") || path.contains("ca.crt"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_android_ca_records_steps_and_fallback_when_view_fails() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = adb_test_lock();
        let test_dir = std::env::temp_dir().join(format!("bifrost-adb-install-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cert_path = test_dir.join("ca.crt");
        fs::write(&cert_path, "test-cert").expect("write test cert");

        let adb_path = test_dir.join("adb-install");
        fs::write(
            &adb_path,
            r#"#!/bin/sh
case "$*" in
  *" push "*)
    echo "pushed"
    exit 0
    ;;
  *"android.intent.action.VIEW"*)
    if [ "$BIFROST_ADB_VIEW_FAIL" = "1" ]; then
      echo "cannot open installer" >&2
      exit 1
    fi
    echo "opened installer"
    exit 0
    ;;
  *"android.settings.SECURITY_SETTINGS"*)
    echo "opened settings"
    exit 0
    ;;
  *)
    echo "ok"
    exit 0
    ;;
esac
"#,
        )
        .expect("write fake adb installer");
        let mut perms = fs::metadata(&adb_path)
            .expect("stat fake adb installer")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&adb_path, perms).expect("chmod fake adb installer");

        {
            // Successful installer flow: push + VIEW succeed, no fallback.
            let _view_fail_env = EnvVarGuard::remove("BIFROST_ADB_VIEW_FAIL");
            let session = install_android_ca(AndroidInstallOptions {
                adb_path: adb_path.clone(),
                device_id: "android-1".to_string(),
                ca_cert_path: cert_path.clone(),
            })
            .expect("install_android_ca should succeed");

            assert_eq!(session.platform, MobilePlatform::Android);
            assert!(session.completed);
            assert!(session.requires_user_confirmation);
            assert_eq!(session.steps.len(), 2);
            assert_eq!(session.steps[0].name, "push_certificate");
            assert!(session.steps[0].success);
            assert_eq!(session.steps[1].name, "open_certificate_installer");
            assert!(session.steps[1].success);
            assert!(session.summary.contains("pushed the CA certificate"));
        }

        // Fallback flow: VIEW fails so SECURITY_SETTINGS is opened.
        let _view_fail_env = EnvVarGuard::set("BIFROST_ADB_VIEW_FAIL", "1");
        let fallback_session = install_android_ca(AndroidInstallOptions {
            adb_path: adb_path.clone(),
            device_id: "android-2".to_string(),
            ca_cert_path: cert_path.clone(),
        })
        .expect("install_android_ca should still return a session");

        assert_eq!(fallback_session.platform, MobilePlatform::Android);
        assert!(!fallback_session.completed);
        assert!(fallback_session.requires_user_confirmation);
        assert_eq!(fallback_session.steps.len(), 3);
        assert_eq!(fallback_session.steps[0].name, "push_certificate");
        assert!(fallback_session.steps[0].success);
        assert_eq!(fallback_session.steps[1].name, "open_certificate_installer");
        assert!(!fallback_session.steps[1].success);
        assert_eq!(
            fallback_session.steps[2].name,
            "open_security_settings_fallback"
        );
        assert!(fallback_session.steps[2].success);
        assert!(fallback_session
            .summary
            .contains("could not complete the Android installer handoff"));

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[cfg(unix)]
    #[test]
    fn find_adb_and_discover_android_devices_use_bifrost_env_and_fake_adb() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = adb_test_lock();
        let test_dir =
            std::env::temp_dir().join(format!("bifrost-adb-discover-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let adb_path = test_dir.join("adb-discover");
        fs::write(
            &adb_path,
            r#"#!/bin/sh
if [ "$1" = "devices" ]; then
  cat <<'EOF'
List of devices attached
emulator-5554 device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1
R58M123 unauthorized usb:336592896X transport_id:2
EOF
  exit 0
fi

echo "unexpected adb invocation" >&2
exit 1
"#,
        )
        .expect("write fake adb discover");
        let mut perms = fs::metadata(&adb_path)
            .expect("stat fake adb discover")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&adb_path, perms).expect("chmod fake adb discover");

        let _adb_env = EnvVarGuard::set("BIFROST_ADB_PATH", &adb_path);

        // find_adb should return the same path
        let found = find_adb().expect("expected find_adb to use BIFROST_ADB_PATH");
        assert_eq!(found, adb_path);

        // discover_android_devices should use the fake adb and parse devices.
        let discovery = discover_android_devices();

        assert!(discovery.adb_available);
        let expected_path = adb_path.display().to_string();
        assert_eq!(discovery.adb_path.as_deref(), Some(expected_path.as_str()));
        assert_eq!(discovery.devices.len(), 2);
        assert_eq!(discovery.message, "ADB found 2 Android device(s).");
        assert_eq!(discovery.devices[0].id, "emulator-5554");
        assert_eq!(discovery.devices[0].status, DeviceStatus::Connected);
        assert_eq!(discovery.devices[1].status, DeviceStatus::Unauthorized);

        let _ = fs::remove_dir_all(&test_dir);
    }
}

#[cfg(all(test, unix))]
mod ca_status_tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn android_ca_status_reports_installed_when_fingerprint_matches_user_store() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir =
            std::env::temp_dir().join(format!("bifrost-adb-ca-installed-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cert_path = test_dir.join("ca.crt");
        let pem = "-----BEGIN CERTIFICATE-----\ndGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";
        fs::write(&cert_path, pem).expect("write cert");

        let store_path = test_dir.join("store.pem");
        fs::write(&store_path, pem).expect("write store");

        let adb_path = test_dir.join("adb");
        fs::write(
            &adb_path,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n  *Download*) cat '{}' ; exit 0 ;;\n  *cacerts-added*) cat '{}' ; exit 0 ;;\n  *) exit 0 ;;\nesac\n",
                cert_path.display(),
                store_path.display(),
            ),
        )
        .expect("write fake adb");
        let mut perms = fs::metadata(&adb_path)
            .expect("stat fake adb")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&adb_path, perms).expect("chmod fake adb");

        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path,
            device_id: "android-installed".to_string(),
            ca_cert_path: cert_path,
        });

        assert_eq!(status.state, DeviceCertificateState::Installed);
        assert_eq!(status.trusted, Some(true));
        assert_eq!(status.fingerprint_match, Some(true));
        assert!(status.message.contains("Android's user certificate store"));

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[cfg(unix)]
    #[test]
    fn android_ca_status_reports_not_installed_when_store_does_not_match_and_no_downloaded_cert() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir =
            std::env::temp_dir().join(format!("bifrost-adb-ca-notinstalled-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cert_path = test_dir.join("ca.crt");
        let pem_current =
            "-----BEGIN CERTIFICATE-----\nY3VycmVudC1jZXJ0\n-----END CERTIFICATE-----\n";
        fs::write(&cert_path, pem_current).expect("write current cert");

        let store_path = test_dir.join("store.pem");
        let pem_other = "-----BEGIN CERTIFICATE-----\nb3RoZXItY2E=\n-----END CERTIFICATE-----\n";
        fs::write(&store_path, pem_other).expect("write store");

        let adb_path = test_dir.join("adb");
        fs::write(
            &adb_path,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n  *Download*) exit 2 ;;\n  *cacerts-added*) cat '{}' ; exit 0 ;;\n  *) exit 0 ;;\nesac\n",
                store_path.display(),
            ),
        )
        .expect("write fake adb");
        let mut perms = fs::metadata(&adb_path)
            .expect("stat fake adb")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&adb_path, perms).expect("chmod fake adb");

        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path,
            device_id: "android-notinstalled".to_string(),
            ca_cert_path: cert_path,
        });

        assert_eq!(status.state, DeviceCertificateState::NotInstalled);
        assert_eq!(status.trusted, Some(false));
        assert_eq!(status.fingerprint_match, Some(false));
        assert!(status
            .message
            .contains("did not find the current Bifrost CA"));

        let _ = fs::remove_dir_all(&test_dir);
    }
}
#[cfg(test)]
mod ca_status_extra_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn android_ca_status_reports_unknown_when_certificate_cannot_be_read() {
        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path: PathBuf::from("/nonexistent/adb"),
            device_id: "android-read-error".to_string(),
            ca_cert_path: PathBuf::from("/nonexistent/bifrost-ca.crt"),
        });

        assert_eq!(status.state, DeviceCertificateState::Unknown);
        assert_eq!(status.trusted, None);
        assert_eq!(status.fingerprint_match, None);
        assert!(status
            .message
            .starts_with("Could not read the local Bifrost CA certificate:"));
    }

    #[test]
    fn android_ca_status_reports_unknown_when_certificate_pem_is_invalid() {
        let test_dir =
            std::env::temp_dir().join(format!("bifrost-adb-ca-parse-error-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cert_path = test_dir.join("ca.crt");
        fs::write(
            &cert_path,
            "-----BEGIN CERTIFICATE-----\n***invalid***\n-----END CERTIFICATE-----\n",
        )
        .expect("write invalid pem");

        let status = check_android_ca_status(AndroidCaStatusOptions {
            adb_path: test_dir.join("adb-not-used"),
            device_id: "android-parse-error".to_string(),
            ca_cert_path: cert_path.clone(),
        });

        assert_eq!(status.state, DeviceCertificateState::Unknown);
        assert_eq!(status.trusted, None);
        assert_eq!(status.fingerprint_match, None);
        assert!(status
            .message
            .starts_with("Could not parse the local Bifrost CA certificate:"));

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[cfg(unix)]
    #[test]
    fn run_adb_propagates_failure_stderr_message() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir =
            std::env::temp_dir().join(format!("bifrost-adb-run-error-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let adb_path = test_dir.join("adb-run-error");
        fs::write(
            &adb_path,
            "#!/bin/sh\necho 'something failed' >&2\nexit 3\n",
        )
        .expect("write fake adb run error");
        let mut perms = fs::metadata(&adb_path)
            .expect("stat fake adb run error")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&adb_path, perms).expect("chmod fake adb run error");

        let result = run_adb(&adb_path, &["devices"]).expect("run_adb should still return");
        assert!(!result.success);
        assert_eq!(result.message, "something failed");

        let _ = fs::remove_dir_all(&test_dir);
    }
}
