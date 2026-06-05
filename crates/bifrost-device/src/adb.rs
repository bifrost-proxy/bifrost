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
    Ok(Command::new(adb_path).args(args).output()?)
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

        assert_eq!(status.state, DeviceCertificateState::PushedToDevice);
        assert_eq!(status.trusted, None);
        assert!(status
            .message
            .contains("Ordinary Android ADB cannot verify"));
        let _ = fs::remove_dir_all(test_dir);
    }
}
