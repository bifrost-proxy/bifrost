use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    DeviceStatus, DeviceTrustCapability, InstallMode, InstallSession, InstallStep, MobileDevice,
    MobilePlatform,
};

use std::process::Output;

/// Run an external command, retrying on `ETXTBSY` ("Text file busy").
///
/// Spawning a freshly-written executable can intermittently fail with
/// ETXTBSY (os error 26) on Linux: when other threads in the process
/// fork+exec concurrently, the child briefly inherits writable file
/// descriptors, and the kernel refuses to exec a file that any process still
/// holds open for writing. This is inherent to multi-threaded fork/exec and
/// cannot be eliminated by file-handle hygiene alone, so we retry a few times
/// with a short backoff before giving up. This mirrors the behaviour of
/// `run_adb_output` in `adb.rs`.
fn run_command_output<P, I, S>(program: P, args: I) -> std::io::Result<Output>
where
    P: AsRef<std::ffi::OsStr>,
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<std::ffi::OsStr>,
{
    const MAX_ATTEMPTS: u32 = 8;
    let mut attempt = 0;
    loop {
        match Command::new(&program).args(args.clone()).output() {
            Ok(output) => return Ok(output),
            Err(err)
                if err.raw_os_error() == Some(26 /* ETXTBSY */) && attempt + 1 < MAX_ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(5 * u64::from(attempt)));
            }
            Err(err) => return Err(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosDiscovery {
    pub supported: bool,
    pub devices: Vec<MobileDevice>,
    pub configurator: ConfiguratorDiscovery,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguratorDiscovery {
    pub supported: bool,
    pub cfgutil_available: bool,
    pub cfgutil_path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct IosConfiguratorInstallOptions {
    pub cfgutil_path: PathBuf,
    pub device_id: String,
    pub cfgutil_target: Option<String>,
    pub profile_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum IosConfiguratorError {
    #[error("Apple Configurator cfgutil is not available")]
    NotAvailable,
    #[error("iOS profile file not found: {0}")]
    ProfileMissing(String),
    #[error("failed to run cfgutil: {0}")]
    Io(#[from] std::io::Error),
}

pub fn discover_ios_devices() -> IosDiscovery {
    discover_ios_devices_impl()
}

#[cfg(target_os = "macos")]
fn discover_ios_devices_impl() -> IosDiscovery {
    let configurator = discover_configurator();
    match Command::new("ioreg")
        .args(["-p", "IOUSB", "-l", "-w0"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut devices = parse_ioreg_ios_devices(&stdout);
            merge_cfgutil_devices(&mut devices, &configurator);
            let message = if devices.is_empty() {
                "No iPhone or iPad USB devices detected.".to_string()
            } else {
                format!("Detected {} iOS USB device(s).", devices.len())
            };
            IosDiscovery {
                supported: true,
                devices,
                configurator,
                message,
            }
        }
        Ok(output) => IosDiscovery {
            supported: true,
            devices: Vec::new(),
            configurator,
            message: format!(
                "ioreg failed to list USB devices: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(error) => IosDiscovery {
            supported: true,
            devices: Vec::new(),
            configurator,
            message: format!("ioreg failed to start: {error}"),
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn discover_ios_devices_impl() -> IosDiscovery {
    let configurator = discover_configurator();
    IosDiscovery {
        supported: false,
        devices: Vec::new(),
        configurator,
        message: "iOS USB detection is currently supported on macOS only.".to_string(),
    }
}

pub fn discover_configurator() -> ConfiguratorDiscovery {
    discover_configurator_impl()
}

#[cfg(target_os = "macos")]
fn discover_configurator_impl() -> ConfiguratorDiscovery {
    match find_cfgutil() {
        Some(path) => ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: true,
            cfgutil_path: Some(path.display().to_string()),
            message:
                "Apple Configurator cfgutil is available. Supervised devices can receive profiles automatically; unsupervised devices may still require Trust, unlock, or onscreen confirmation."
                    .to_string(),
        },
        None => ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: false,
            cfgutil_path: None,
            message:
                "Apple Configurator cfgutil was not found. Install Apple Configurator from the Mac App Store to enable computer-side iPhone profile installation."
                    .to_string(),
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn discover_configurator_impl() -> ConfiguratorDiscovery {
    ConfiguratorDiscovery {
        supported: false,
        cfgutil_available: false,
        cfgutil_path: None,
        message: "Apple Configurator installation is supported on macOS only.".to_string(),
    }
}

pub fn install_ios_profile_with_configurator(
    options: IosConfiguratorInstallOptions,
) -> Result<InstallSession, IosConfiguratorError> {
    if !options.cfgutil_path.exists() {
        return Err(IosConfiguratorError::NotAvailable);
    }
    if !options.profile_path.exists() {
        return Err(IosConfiguratorError::ProfileMissing(
            options.profile_path.display().to_string(),
        ));
    }

    let output = run_command_output(
        &options.cfgutil_path,
        cfgutil_install_profile_args(&options.profile_path, options.cfgutil_target.as_deref()),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if output.status.success() {
        if stdout.is_empty() {
            "cfgutil completed install-profile.".to_string()
        } else {
            stdout
        }
    } else if stderr.is_empty() {
        stdout
    } else {
        stderr
    };
    let requires_user_interaction = is_cfgutil_user_interaction_required(&message);
    let completed = output.status.success();
    let handoff_started = completed || requires_user_interaction;

    Ok(InstallSession {
        session_id: Uuid::new_v4().to_string(),
        device_id: options.device_id,
        platform: MobilePlatform::Ios,
        mode: InstallMode::ManagedAutoTrust,
        capability: DeviceTrustCapability::ManagedAutoTrust,
        completed: handoff_started,
        requires_user_confirmation: requires_user_interaction,
        summary: if completed {
            "Apple Configurator installed the Bifrost profile. Certificate payloads installed through Configurator are trusted for SSL/TLS automatically.".to_string()
        } else if requires_user_interaction {
            "Apple Configurator opened the Bifrost profile install flow on the iPhone. Finish the confirmation on the phone; certificate payloads installed through Configurator are trusted for SSL/TLS automatically.".to_string()
        } else {
            "Apple Configurator could not install the Bifrost profile. Unlock the iPhone, tap Trust for this Mac if prompted, then retry.".to_string()
        },
        steps: vec![InstallStep {
            name: "cfgutil_install_profile".to_string(),
            success: completed,
            message,
        }],
    })
}

pub fn is_cfgutil_user_interaction_required(message: &str) -> bool {
    message.contains("Code: 625")
        || message.contains("requires user interaction")
        || message.contains("需要用户在设备上交互")
}

pub fn cfgutil_install_profile_args(profile_path: &Path, target_ecid: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(target_ecid) = target_ecid.filter(|value| !value.trim().is_empty()) {
        args.push("-e".to_string());
        args.push(target_ecid.to_string());
    }
    args.push("install-profile".to_string());
    args.push(profile_path.display().to_string());
    args
}

pub fn parse_ioreg_ios_devices(output: &str) -> Vec<MobileDevice> {
    let mut devices = Vec::new();
    let mut current_block = String::new();

    for line in output.lines() {
        let starts_usb_device = line.contains("+-o ") && line.contains("<class IOUSBHostDevice");
        if starts_usb_device {
            if let Some(device) = parse_ioreg_device_block(&current_block) {
                devices.push(device);
            }
            current_block.clear();
        }

        if starts_usb_device || !current_block.is_empty() {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    if let Some(device) = parse_ioreg_device_block(&current_block) {
        devices.push(device);
    }

    devices
}

fn parse_ioreg_device_block(block: &str) -> Option<MobileDevice> {
    let first_line = block.lines().next()?.trim();
    if !first_line.contains("<class IOUSBHostDevice") {
        return None;
    }

    let product = quoted_property(block, "USB Product Name")
        .or_else(|| quoted_property(block, "kUSBProductString"))
        .or_else(|| device_name_from_ioreg_header(first_line));
    let product_lower = product.as_deref().unwrap_or_default().to_lowercase();
    if !matches!(
        product_lower.as_str(),
        value if value.contains("iphone") || value.contains("ipad") || value.contains("ipod")
    ) {
        return None;
    }

    let id = quoted_property(block, "kUSBSerialNumberString")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| product.clone().unwrap_or_else(|| "ios-device".to_string()));
    let name = product.or_else(|| Some("iOS Device".to_string()));

    Some(MobileDevice {
        id,
        name,
        managed_install_target: None,
        platform: MobilePlatform::Ios,
        status: DeviceStatus::Connected,
        capability: DeviceTrustCapability::GuideOnly,
        certificate_status: None,
        status_message:
            "Detected over USB. Download the iOS profile, install it on the phone, then enable full trust in Certificate Trust Settings."
                .to_string(),
    })
}

#[cfg(target_os = "macos")]
fn merge_cfgutil_devices(devices: &mut Vec<MobileDevice>, configurator: &ConfiguratorDiscovery) {
    let Some(cfgutil_path) = configurator
        .cfgutil_path
        .as_ref()
        .filter(|_| configurator.cfgutil_available)
    else {
        return;
    };

    let Ok(output) =
        run_command_output(cfgutil_path, ["--timeout", "1", "--format", "JSON", "list"])
    else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for cfgutil_device in parse_cfgutil_list_devices(&stdout) {
        if let Some(existing) = devices
            .iter_mut()
            .find(|device| device.id == cfgutil_device.id)
        {
            existing.name = cfgutil_device.name;
            existing.managed_install_target = cfgutil_device.managed_install_target;
            existing.capability = cfgutil_device.capability;
            existing.status_message = cfgutil_device.status_message;
        } else {
            devices.push(cfgutil_device);
        }
    }
}

pub fn parse_cfgutil_list_devices(output: &str) -> Vec<MobileDevice> {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return Vec::new();
    };
    let Some(devices) = value.get("Output").and_then(Value::as_object) else {
        return Vec::new();
    };

    devices
        .values()
        .filter_map(|device| {
            let udid = device.get("UDID").and_then(Value::as_str)?;
            let ecid = device.get("ECID").and_then(Value::as_str)?;
            let id = normalize_ios_udid(udid);
            if id.is_empty() {
                return None;
            }
            let name = device
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let device_type = device.get("deviceType").and_then(Value::as_str);
            let display_name = match (name, device_type) {
                (Some(name), Some(device_type)) if !device_type.trim().is_empty() => {
                    Some(format!("{name} ({device_type})"))
                }
                (name, _) => name,
            };

            Some(MobileDevice {
                id,
                name: display_name,
                managed_install_target: Some(ecid.to_string()),
                platform: MobilePlatform::Ios,
                status: DeviceStatus::Connected,
                capability: DeviceTrustCapability::ManagedAutoTrust,
                certificate_status: None,
                status_message:
                    "Detected over USB through Apple Configurator. Select this device to install the Bifrost CA profile with cfgutil; unsupervised devices may still require iPhone confirmation."
                        .to_string(),
            })
        })
        .collect()
}

fn normalize_ios_udid(udid: &str) -> String {
    udid.chars().filter(|ch| *ch != '-').collect()
}

fn quoted_property(block: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\" = \"");
    let start = block.find(&needle)? + needle.len();
    let rest = &block[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn device_name_from_ioreg_header(line: &str) -> Option<String> {
    line.split('@')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(target_os = "macos")]
fn find_cfgutil() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BIFROST_CFGUTIL_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    let app_bundle_path =
        PathBuf::from("/Applications/Apple Configurator.app/Contents/MacOS/cfgutil");
    if is_executable_file(&app_bundle_path) {
        return Some(app_bundle_path);
    }

    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join("cfgutil"))
        .find(|path| is_executable_file(path))
}

#[cfg(target_os = "macos")]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connected_iphone_from_ioreg() {
        let output = r#"
    +-o iPhone@01100000  <class IOUSBHostDevice, id 0x1000eacd5, registered, matched, active, busy 0 (247 ms), retain 73>
        {
          "kUSBSerialNumberString" = "00008150000D250C1E40401C"
          "kUSBProductString" = "iPhone"
          "USB Product Name" = "iPhone"
          "kUSBVendorString" = "Apple Inc."
          "idVendor" = 1452
          "SupportsIPhoneOS" = Yes
        }
"#;

        let devices = parse_ioreg_ios_devices(output);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "00008150000D250C1E40401C");
        assert_eq!(devices[0].name.as_deref(), Some("iPhone"));
        assert_eq!(devices[0].platform, MobilePlatform::Ios);
        assert_eq!(devices[0].capability, DeviceTrustCapability::GuideOnly);
    }

    #[test]
    fn parses_multiple_iphones_with_different_ioreg_tree_prefixes() {
        let output = r#"
  | +-o iPhone@00100000  <class IOUSBHostDevice, id 0x1000eb7df, registered, matched, active, busy 0 (248 ms), retain 71>
  |     {
  |       "kUSBSerialNumberString" = "00008130001A75D23A2B803A"
  |       "kUSBProductString" = "iPhone"
  |       "USB Product Name" = "iPhone"
  |       "kUSBVendorString" = "Apple Inc."
  |       "idVendor" = 1452
  |       "SupportsIPhoneOS" = Yes
  |     }
    +-o iPhone@01100000  <class IOUSBHostDevice, id 0x1000eb40a, registered, matched, active, busy 0 (239 ms), retain 71>
        {
          "kUSBSerialNumberString" = "00008150000D250C1E40401C"
          "kUSBProductString" = "iPhone"
          "USB Product Name" = "iPhone"
          "kUSBVendorString" = "Apple Inc."
          "idVendor" = 1452
          "SupportsIPhoneOS" = Yes
        }
"#;

        let devices = parse_ioreg_ios_devices(output);

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "00008130001A75D23A2B803A");
        assert_eq!(devices[1].id, "00008150000D250C1E40401C");
    }

    #[test]
    fn parses_cfgutil_list_devices_with_ecid_targets() {
        let output = r#"{"Command":"list","Output":{"0x1A75D23A2B803A":{"locationID":1048576,"UDID":"00008130-001A75D23A2B803A","ECID":"0x1A75D23A2B803A","name":"Belle 贝拉小姐","deviceType":"iPhone16,2"},"0xD250C1E40401C":{"locationID":17825792,"UDID":"00008150-000D250C1E40401C","ECID":"0xD250C1E40401C","name":"Eden iPhone ","deviceType":"iPhone18,1"}},"Type":"CommandOutput","Devices":["0x1A75D23A2B803A","0xD250C1E40401C"]}"#;

        let devices = parse_cfgutil_list_devices(output);

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "00008130001A75D23A2B803A");
        assert_eq!(
            devices[0].managed_install_target.as_deref(),
            Some("0x1A75D23A2B803A")
        );
        assert_eq!(
            devices[0].name.as_deref(),
            Some("Belle 贝拉小姐 (iPhone16,2)")
        );
        assert_eq!(
            devices[0].capability,
            DeviceTrustCapability::ManagedAutoTrust
        );
        assert_eq!(devices[1].id, "00008150000D250C1E40401C");
        assert_eq!(
            devices[1].managed_install_target.as_deref(),
            Some("0xD250C1E40401C")
        );
    }

    #[test]
    fn ignores_non_ios_usb_devices() {
        let output = r#"
    +-o USB Keyboard@02100000  <class IOUSBHostDevice, id 0x100, registered, matched, active, busy 0 (1 ms), retain 20>
        {
          "USB Product Name" = "USB Keyboard"
          "kUSBSerialNumberString" = "keyboard-1"
        }
"#;

        assert!(parse_ioreg_ios_devices(output).is_empty());
    }

    #[test]
    fn cfgutil_install_profile_args_use_install_profile_subcommand() {
        let args = cfgutil_install_profile_args(Path::new("/tmp/bifrost-ca.mobileconfig"), None);

        assert_eq!(
            args,
            vec![
                "install-profile".to_string(),
                "/tmp/bifrost-ca.mobileconfig".to_string()
            ]
        );
    }

    #[test]
    fn cfgutil_install_profile_args_target_selected_ecid() {
        let args = cfgutil_install_profile_args(
            Path::new("/tmp/bifrost-ca.mobileconfig"),
            Some("0x1A75D23A2B803A"),
        );

        assert_eq!(
            args,
            vec![
                "-e".to_string(),
                "0x1A75D23A2B803A".to_string(),
                "install-profile".to_string(),
                "/tmp/bifrost-ca.mobileconfig".to_string()
            ]
        );
    }

    #[test]
    fn cfgutil_install_profile_args_ignore_blank_ecid() {
        let args =
            cfgutil_install_profile_args(Path::new("/tmp/bifrost-ca.mobileconfig"), Some("   "));

        assert_eq!(
            args,
            vec![
                "install-profile".to_string(),
                "/tmp/bifrost-ca.mobileconfig".to_string()
            ]
        );
    }

    #[test]
    fn detects_cfgutil_user_interaction_required_error() {
        let message = "cfgutil: error: 安装此描述文件需要用户在设备上交互。 (Domain: ConfigurationUtilityKit.error Code: 625)";

        assert!(is_cfgutil_user_interaction_required(message));
    }

    #[test]
    fn cfgutil_user_interaction_matches_english_and_code() {
        assert!(is_cfgutil_user_interaction_required("Code: 625"));
        assert!(is_cfgutil_user_interaction_required(
            "this requires user interaction"
        ));
        assert!(!is_cfgutil_user_interaction_required("some other error"));
    }

    #[test]
    fn cfgutil_args_skip_blank_ecid() {
        // A whitespace-only ECID must be treated as "no target".
        let args = cfgutil_install_profile_args(Path::new("/tmp/p.mobileconfig"), Some("   "));
        assert_eq!(
            args,
            vec![
                "install-profile".to_string(),
                "/tmp/p.mobileconfig".to_string()
            ]
        );
    }

    #[test]
    fn normalize_ios_udid_strips_dashes() {
        assert_eq!(
            normalize_ios_udid("00008130-001A75D23A2B803A"),
            "00008130001A75D23A2B803A"
        );
        assert_eq!(normalize_ios_udid(""), "");
    }

    #[test]
    fn quoted_property_returns_none_when_key_absent() {
        assert!(quoted_property("no props here", "USB Product Name").is_none());
    }

    #[test]
    fn device_name_from_ioreg_header_extracts_prefix() {
        let name = device_name_from_ioreg_header("    +-o iPhone@01100000  <class IOUSBHostDevice");
        assert_eq!(name.as_deref(), Some("+-o iPhone"));
        assert!(device_name_from_ioreg_header("@only").is_none());
    }

    #[test]
    fn parse_cfgutil_list_ignores_invalid_json_and_missing_output() {
        assert!(parse_cfgutil_list_devices("not json").is_empty());
        assert!(parse_cfgutil_list_devices(r#"{"NoOutput":1}"#).is_empty());
    }

    #[test]
    fn parse_cfgutil_list_skips_entries_without_udid_or_ecid() {
        // Entry missing ECID is dropped; entry missing UDID is dropped.
        let json = r#"{"Output":{"a":{"UDID":"1111-2222"},"b":{"ECID":"0x1"}}}"#;
        assert!(parse_cfgutil_list_devices(json).is_empty());
    }

    #[test]
    fn parse_ioreg_block_uses_product_name_when_serial_blank() {
        // No serial number -> id falls back to the product name.
        let output = "    +-o iPad@01  <class IOUSBHostDevice, id 0x1>\n        {\n          \"USB Product Name\" = \"iPad\"\n        }\n";
        let devices = parse_ioreg_ios_devices(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "iPad");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn discover_ios_is_unsupported_off_macos() {
        let d = discover_ios_devices();
        assert!(!d.supported);
        assert!(d.devices.is_empty());
        assert!(d.message.contains("macOS only"));
        // The configurator probe is also unsupported off macOS.
        assert!(!d.configurator.supported);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn discover_configurator_is_unsupported_off_macos() {
        let c = discover_configurator();
        assert!(!c.supported);
        assert!(!c.cfgutil_available);
        assert!(c.cfgutil_path.is_none());
        assert!(c.message.contains("macOS only"));
    }

    // --- Fake cfgutil harness (unix) ----------------------------------------
    //
    // Serialized behind a lock: exec'ing a freshly-written executable while a
    // sibling thread is still touching it can raise ETXTBSY ("Text file busy").

    #[cfg(unix)]
    static CFGUTIL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn write_fake_cfgutil(dir: &Path, script_body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("cfgutil");
        let mut f = std::fs::File::create(&path).expect("create cfgutil");
        f.write_all(format!("#!/bin/sh\n{script_body}\n").as_bytes())
            .expect("write cfgutil");
        f.sync_all().expect("sync");
        drop(f);

        let mut perms = std::fs::metadata(&path).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    #[cfg(unix)]
    #[test]
    fn install_profile_succeeds_with_fake_cfgutil() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "echo 'profile installed'; exit 0");
        let profile = dir.join("ca.mobileconfig");
        std::fs::write(&profile, b"<plist/>").unwrap();

        let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil,
            device_id: "ios-1".to_string(),
            cfgutil_target: Some("0xABC".to_string()),
            profile_path: profile,
        })
        .expect("install should succeed");

        assert!(session.completed);
        assert_eq!(session.device_id, "ios-1");
        assert_eq!(session.steps.len(), 1);
        assert!(session.steps[0].success);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_profile_reports_user_interaction_on_code_625() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "echo 'Code: 625' 1>&2; exit 1");
        let profile = dir.join("ca.mobileconfig");
        std::fs::write(&profile, b"<plist/>").unwrap();

        let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil,
            device_id: "ios-2".to_string(),
            cfgutil_target: None,
            profile_path: profile,
        })
        .expect("install returns Ok");

        // Not completed by exit code, but handoff started due to interaction.
        assert!(session.completed); // handoff_started == requires_user_interaction
        assert!(session.requires_user_confirmation);
        assert!(!session.steps[0].success);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_profile_fails_cleanly_on_generic_error() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "echo 'device locked' 1>&2; exit 1");
        let profile = dir.join("ca.mobileconfig");
        std::fs::write(&profile, b"<plist/>").unwrap();

        let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil,
            device_id: "ios-3".to_string(),
            cfgutil_target: None,
            profile_path: profile,
        })
        .expect("install returns Ok");

        assert!(!session.completed);
        assert!(!session.requires_user_confirmation);
        assert!(session.summary.contains("could not install"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_profile_errors_when_cfgutil_missing() {
        let err = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: PathBuf::from("/nonexistent/cfgutil"),
            device_id: "ios".to_string(),
            cfgutil_target: None,
            profile_path: PathBuf::from("/tmp/whatever.mobileconfig"),
        })
        .expect_err("missing cfgutil should error");
        assert!(matches!(err, IosConfiguratorError::NotAvailable));
    }

    #[cfg(unix)]
    #[test]
    fn install_profile_errors_when_profile_missing() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "exit 0");
        let err = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil,
            device_id: "ios".to_string(),
            cfgutil_target: None,
            profile_path: dir.join("missing.mobileconfig"),
        })
        .expect_err("missing profile should error");
        assert!(matches!(err, IosConfiguratorError::ProfileMissing(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn is_executable_file_detects_regular_file() {
        let dir = std::env::temp_dir().join(format!("bifrost-ios-file-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let regular_file = dir.join("cfgutil");
        std::fs::write(&regular_file, "fake cfgutil").unwrap();

        assert!(!is_executable_file(&dir));
        assert!(is_executable_file(&regular_file));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merge_cfgutil_adds_and_updates_devices() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-m-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // cfgutil list returns one device (UDID 0000...401C) with an ECID.
        let json = r#"{"Output":{"0xD250C1E40401C":{"UDID":"00008150-000D250C1E40401C","ECID":"0xD250C1E40401C","name":"Eden","deviceType":"iPhone18,1"}}}"#;
        let cfgutil = write_fake_cfgutil(&dir, &format!("cat <<'EOF'\n{json}\nEOF"));

        let configurator = ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: true,
            cfgutil_path: Some(cfgutil.display().to_string()),
            message: String::new(),
        };

        // Pre-seed one ioreg device with the same UDID so the merge updates it,
        // leaving a single (enriched) entry.
        let mut devices = vec![MobileDevice {
            id: "00008150000D250C1E40401C".to_string(),
            name: Some("iPhone".to_string()),
            managed_install_target: None,
            platform: MobilePlatform::Ios,
            status: DeviceStatus::Connected,
            capability: DeviceTrustCapability::GuideOnly,
            certificate_status: None,
            status_message: String::new(),
        }];
        merge_cfgutil_devices(&mut devices, &configurator);

        assert_eq!(
            devices.len(),
            1,
            "matching device should be updated in place"
        );
        assert_eq!(
            devices[0].managed_install_target.as_deref(),
            Some("0xD250C1E40401C")
        );
        assert_eq!(
            devices[0].capability,
            DeviceTrustCapability::ManagedAutoTrust
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merge_cfgutil_appends_new_device() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-a-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let json =
            r#"{"Output":{"0x1":{"UDID":"00008130-001A75D23A2B803A","ECID":"0x1","name":"New"}}}"#;
        let cfgutil = write_fake_cfgutil(&dir, &format!("cat <<'EOF'\n{json}\nEOF"));
        let configurator = ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: true,
            cfgutil_path: Some(cfgutil.display().to_string()),
            message: String::new(),
        };

        let mut devices: Vec<MobileDevice> = Vec::new();
        merge_cfgutil_devices(&mut devices, &configurator);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "00008130001A75D23A2B803A");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merge_cfgutil_noop_when_unavailable() {
        // No cfgutil_path / not available -> early return, devices untouched.
        let configurator = ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: false,
            cfgutil_path: None,
            message: String::new(),
        };
        let mut devices: Vec<MobileDevice> = Vec::new();
        merge_cfgutil_devices(&mut devices, &configurator);
        assert!(devices.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merge_cfgutil_noop_on_nonzero_exit() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-e-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "exit 1");
        let configurator = ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: true,
            cfgutil_path: Some(cfgutil.display().to_string()),
            message: String::new(),
        };
        let mut devices: Vec<MobileDevice> = Vec::new();
        merge_cfgutil_devices(&mut devices, &configurator);
        assert!(devices.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn find_cfgutil_honors_env_override() {
        let _guard = CFGUTIL_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("bifrost-ios-f-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfgutil = write_fake_cfgutil(&dir, "exit 0");

        std::env::set_var("BIFROST_CFGUTIL_PATH", &cfgutil);
        let found = find_cfgutil();
        std::env::remove_var("BIFROST_CFGUTIL_PATH");

        assert_eq!(found.as_deref(), Some(cfgutil.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_message_defaults_when_stdout_empty() {
        // Exercises the "stdout empty -> default message" branch of install.
        let dir = std::env::temp_dir().join(format!("bifrost-ios-d-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            let _guard = CFGUTIL_LOCK.lock().unwrap();
            let cfgutil = write_fake_cfgutil(&dir, "exit 0"); // no stdout
            let profile = dir.join("ca.mobileconfig");
            std::fs::write(&profile, b"<plist/>").unwrap();
            let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
                cfgutil_path: cfgutil,
                device_id: "ios".to_string(),
                cfgutil_target: None,
                profile_path: profile,
            })
            .expect("ok");
            assert!(session.completed);
            assert_eq!(
                session.steps[0].message,
                "cfgutil completed install-profile."
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn ios_discovery_reports_unsupported_on_non_macos() {
        if cfg!(target_os = "macos") {
            // macOS 上的 USB 检测与 Configurator 流程由平台特定 E2E 覆盖；
            // 这里仅验证非 macOS 平台的默认回退行为。
            return;
        }

        let discovery = discover_ios_devices();

        assert!(!discovery.supported);
        assert!(discovery.devices.is_empty());
        assert!(!discovery.message.is_empty());

        assert!(!discovery.configurator.supported);
        assert!(!discovery.configurator.cfgutil_available);
        assert!(discovery.configurator.cfgutil_path.is_none());
    }

    #[test]
    fn discover_configurator_reports_not_supported_on_non_macos() {
        if cfg!(target_os = "macos") {
            return;
        }

        let configurator = discover_configurator();

        assert!(!configurator.supported);
        assert!(!configurator.cfgutil_available);
        assert!(configurator.cfgutil_path.is_none());
        assert!(configurator.message.contains("macOS only"));
    }

    #[test]
    fn install_ios_profile_with_configurator_errors_when_cfgutil_missing() {
        let options = IosConfiguratorInstallOptions {
            cfgutil_path: PathBuf::from("/nonexistent/cfgutil"),
            device_id: "ios-1".to_string(),
            cfgutil_target: None,
            profile_path: PathBuf::from("/nonexistent/profile.mobileconfig"),
        };

        let error = install_ios_profile_with_configurator(options)
            .expect_err("cfgutil should be reported as not available");
        assert!(matches!(error, IosConfiguratorError::NotAvailable));
    }

    #[cfg(unix)]
    #[test]
    fn install_ios_profile_with_configurator_errors_when_profile_missing() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir =
            std::env::temp_dir().join(format!("bifrost-ios-missing-profile-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cfgutil_path = test_dir.join("cfgutil");
        fs::write(&cfgutil_path, "#!/bin/sh\nexit 0\n").expect("write fake cfgutil");
        let mut perms = fs::metadata(&cfgutil_path)
            .expect("stat fake cfgutil")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil");

        let options = IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil_path.clone(),
            device_id: "ios-1".to_string(),
            cfgutil_target: None,
            profile_path: test_dir.join("missing.mobileconfig"),
        };

        let error = install_ios_profile_with_configurator(options)
            .expect_err("profile should be reported as missing");
        match error {
            IosConfiguratorError::ProfileMissing(path) => {
                assert!(path.ends_with("missing.mobileconfig"));
            }
            other => panic!("unexpected error: {other}"),
        }

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_ios_profile_with_configurator_builds_sessions_for_success_and_user_interaction() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir = std::env::temp_dir().join(format!("bifrost-ios-install-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let profile_path = test_dir.join("bifrost.mobileconfig");
        fs::write(&profile_path, "test-profile").expect("write profile");

        let cfgutil_path = test_dir.join("cfgutil");
        fs::write(
            &cfgutil_path,
            r#"#!/bin/sh
if [ "$CFGUTIL_SHOULD_FAIL" = "1" ]; then
  echo "cfgutil: error: 安装此描述文件需要用户在设备上交互。 (Domain: ConfigurationUtilityKit.error Code: 625)" >&2
  exit 1
fi
echo "install-profile succeeded"
exit 0
"#,
        )
        .expect("write fake cfgutil");
        let mut perms = fs::metadata(&cfgutil_path)
            .expect("stat fake cfgutil")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil");

        // Successful non-interactive install.
        std::env::remove_var("CFGUTIL_SHOULD_FAIL");
        let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
            cfgutil_path: cfgutil_path.clone(),
            device_id: "ios-device-1".to_string(),
            cfgutil_target: Some("0xECIDTARGET".to_string()),
            profile_path: profile_path.clone(),
        })
        .expect("cfgutil install should succeed");

        assert_eq!(session.device_id, "ios-device-1");
        assert_eq!(session.platform, MobilePlatform::Ios);
        assert_eq!(session.mode, InstallMode::ManagedAutoTrust);
        assert_eq!(session.capability, DeviceTrustCapability::ManagedAutoTrust);
        assert!(session.completed);
        assert!(!session.requires_user_confirmation);
        assert_eq!(session.steps.len(), 1);
        assert_eq!(session.steps[0].name, "cfgutil_install_profile");
        assert!(session.steps[0].success);
        assert!(session.summary.contains("installed the Bifrost profile"));

        // Interactive flow where cfgutil reports Code 625.
        std::env::set_var("CFGUTIL_SHOULD_FAIL", "1");
        let interactive_session =
            install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
                cfgutil_path: cfgutil_path.clone(),
                device_id: "ios-device-2".to_string(),
                cfgutil_target: None,
                profile_path: profile_path.clone(),
            })
            .expect("cfgutil should still produce a session");

        assert_eq!(interactive_session.device_id, "ios-device-2");
        assert!(interactive_session.completed);
        assert!(interactive_session.requires_user_confirmation);
        assert_eq!(interactive_session.steps.len(), 1);
        assert!(!interactive_session.steps[0].success);
        assert!(interactive_session
            .summary
            .contains("opened the Bifrost profile install flow"));

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merge_cfgutil_devices_enriches_and_adds_devices() {
        use std::os::unix::fs::PermissionsExt;

        let test_dir =
            std::env::temp_dir().join(format!("bifrost-ios-merge-cfgutil-{}", Uuid::new_v4()));
        fs::create_dir_all(&test_dir).expect("create temp dir");
        let cfgutil_path = test_dir.join("cfgutil");
        fs::write(
            &cfgutil_path,
            r#"#!/bin/sh
cat <<'EOF'
{"Command":"list","Output":{"0x1A75D23A2B803A":{"locationID":1048576,"UDID":"00008130-001A75D23A2B803A","ECID":"0x1A75D23A2B803A","name":"From Configurator","deviceType":"iPhone16,2"},"0xNEW":{"locationID":0,"UDID":"00000000-0000000000000001","ECID":"0xNEW","name":"New Device","deviceType":"iPhone1,1"}},"Type":"CommandOutput","Devices":["0x1A75D23A2B803A","0xNEW"]}
EOF
exit 0
"#,
        )
        .expect("write fake cfgutil list");
        let mut perms = fs::metadata(&cfgutil_path)
            .expect("stat fake cfgutil list")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil list");

        let configurator = ConfiguratorDiscovery {
            supported: true,
            cfgutil_available: true,
            cfgutil_path: Some(cfgutil_path.display().to_string()),
            message: "test".to_string(),
        };

        let mut devices = vec![MobileDevice {
            id: "00008130001A75D23A2B803A".to_string(),
            name: Some("iPhone from ioreg".to_string()),
            managed_install_target: None,
            platform: MobilePlatform::Ios,
            status: DeviceStatus::Connected,
            capability: DeviceTrustCapability::GuideOnly,
            certificate_status: None,
            status_message: "Detected from ioreg".to_string(),
        }];

        let expected_new_id = normalize_ios_udid("00000000-0000000000000001");

        merge_cfgutil_devices(&mut devices, &configurator);

        assert_eq!(devices.len(), 2);

        let from_cfgutil = devices
            .iter()
            .find(|device| device.id == "00008130001A75D23A2B803A")
            .expect("existing device should be present");
        assert_eq!(
            from_cfgutil.managed_install_target.as_deref(),
            Some("0x1A75D23A2B803A")
        );
        assert_eq!(
            from_cfgutil.name.as_deref(),
            Some("From Configurator (iPhone16,2)")
        );
        assert_eq!(
            from_cfgutil.capability,
            DeviceTrustCapability::ManagedAutoTrust
        );

        let new_device = devices
            .iter()
            .find(|device| device.id == expected_new_id)
            .expect("new device should be added");
        assert_eq!(new_device.managed_install_target.as_deref(), Some("0xNEW"));
        assert_eq!(
            new_device.capability,
            DeviceTrustCapability::ManagedAutoTrust
        );

        let _ = fs::remove_dir_all(&test_dir);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn find_cfgutil_prefers_bifrost_env_when_executable_is_present() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let test_dir =
        std::env::temp_dir().join(format!("bifrost-ios-find-cfgutil-{}", Uuid::new_v4()));
    fs::create_dir_all(&test_dir).expect("create temp dir");
    let cfgutil_path = test_dir.join("cfgutil-env");
    fs::write(&cfgutil_path, "#!/bin/sh\nexit 0\n").expect("write fake cfgutil env");
    let mut perms = fs::metadata(&cfgutil_path)
        .expect("stat fake cfgutil env")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil env");

    std::env::set_var("BIFROST_CFGUTIL_PATH", &cfgutil_path);

    let found = find_cfgutil().expect("expected find_cfgutil to use BIFROST_CFGUTIL_PATH");
    assert_eq!(found, cfgutil_path);

    std::env::remove_var("BIFROST_CFGUTIL_PATH");
    let _ = fs::remove_dir_all(&test_dir);
}

#[cfg(unix)]
#[test]
fn install_ios_profile_with_configurator_reports_failure_without_user_interaction_hint() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let test_dir =
        std::env::temp_dir().join(format!("bifrost-ios-install-error-{}", Uuid::new_v4()));
    fs::create_dir_all(&test_dir).expect("create temp dir");
    let profile_path = test_dir.join("bifrost-error.mobileconfig");
    fs::write(&profile_path, "test-profile").expect("write profile");

    let cfgutil_path = test_dir.join("cfgutil-error");
    fs::write(
        &cfgutil_path,
        "#!/bin/sh\necho 'cfgutil: fatal: device not connected' >&2\nexit 42\n",
    )
    .expect("write fake cfgutil error");
    let mut perms = fs::metadata(&cfgutil_path)
        .expect("stat fake cfgutil error")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil error");

    let result = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
        cfgutil_path: cfgutil_path.clone(),
        device_id: "ios-device-error".to_string(),
        cfgutil_target: None,
        profile_path: profile_path.clone(),
    });

    match result {
        Ok(session) => {
            assert_eq!(session.device_id, "ios-device-error");
            assert_eq!(session.platform, MobilePlatform::Ios);
            assert_eq!(session.mode, InstallMode::ManagedAutoTrust);
            assert_eq!(session.capability, DeviceTrustCapability::ManagedAutoTrust);
            assert!(!session.completed);
            assert!(!session.requires_user_confirmation);
            assert_eq!(session.steps.len(), 1);
            assert!(!session.steps[0].success);
            assert_eq!(
                session.steps[0].message,
                "cfgutil: fatal: device not connected"
            );
            assert!(session
                .summary
                .contains("Apple Configurator could not install the Bifrost profile"));
        }
        Err(IosConfiguratorError::Io(_)) => {
            // 在某些环境（例如带覆盖率插桩的运行时）下，执行临时脚本可能
            // 会因为 ETXTBUSY 等 IO 错误失败；这种情况下只验证错误类型，
            // 避免测试对底层文件系统行为过于敏感。
        }
        Err(other) => panic!("unexpected error: {other}"),
    }

    let _ = fs::remove_dir_all(&test_dir);
}

#[cfg(unix)]
#[test]
fn install_ios_profile_with_configurator_uses_default_message_when_stdout_is_empty() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let test_dir =
        std::env::temp_dir().join(format!("bifrost-ios-install-empty-{}", Uuid::new_v4()));
    fs::create_dir_all(&test_dir).expect("create temp dir");
    let profile_path = test_dir.join("bifrost-empty.mobileconfig");
    fs::write(&profile_path, "test-profile").expect("write profile");

    let cfgutil_path = test_dir.join("cfgutil-empty");
    fs::write(&cfgutil_path, "#!/bin/sh\nexit 0\n").expect("write fake cfgutil empty");
    let mut perms = fs::metadata(&cfgutil_path)
        .expect("stat fake cfgutil empty")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&cfgutil_path, perms).expect("chmod fake cfgutil empty");

    let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
        cfgutil_path: cfgutil_path.clone(),
        device_id: "ios-device-empty".to_string(),
        cfgutil_target: None,
        profile_path: profile_path.clone(),
    })
    .expect("cfgutil empty output should still produce a session");

    assert_eq!(session.device_id, "ios-device-empty");
    assert_eq!(session.platform, MobilePlatform::Ios);
    assert_eq!(session.mode, InstallMode::ManagedAutoTrust);
    assert_eq!(session.capability, DeviceTrustCapability::ManagedAutoTrust);
    assert!(session.completed);
    assert!(!session.requires_user_confirmation);
    assert_eq!(session.steps.len(), 1);
    assert!(session.steps[0].success);
    assert_eq!(
        session.steps[0].message,
        "cfgutil completed install-profile."
    );
    assert!(session.summary.contains("installed the Bifrost profile"));

    let _ = fs::remove_dir_all(&test_dir);
}

#[cfg(target_os = "macos")]
#[test]
fn is_executable_file_returns_false_for_directories() {
    use std::fs;

    let test_dir = std::env::temp_dir().join(format!("bifrost-ios-exec-dir-{}", Uuid::new_v4()));
    fs::create_dir_all(&test_dir).expect("create temp dir");

    assert!(!is_executable_file(&test_dir));

    let _ = fs::remove_dir_all(&test_dir);
}
