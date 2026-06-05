use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    DeviceStatus, DeviceTrustCapability, InstallMode, InstallSession, InstallStep, MobileDevice,
    MobilePlatform,
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
            let devices = parse_adb_devices(&stdout);
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

struct CommandResult {
    success: bool,
    message: String,
}

fn run_adb(adb_path: &Path, args: &[&str]) -> Result<CommandResult, AdbError> {
    let output = Command::new(adb_path).args(args).output()?;
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
}
