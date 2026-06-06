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
    let configurator = discover_configurator();
    if !cfg!(target_os = "macos") {
        return IosDiscovery {
            supported: false,
            devices: Vec::new(),
            configurator,
            message: "iOS USB detection is currently supported on macOS only.".to_string(),
        };
    }

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

pub fn discover_configurator() -> ConfiguratorDiscovery {
    if !cfg!(target_os = "macos") {
        return ConfiguratorDiscovery {
            supported: false,
            cfgutil_available: false,
            cfgutil_path: None,
            message: "Apple Configurator installation is supported on macOS only.".to_string(),
        };
    }

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

    let output = Command::new(&options.cfgutil_path)
        .args(cfgutil_install_profile_args(
            &options.profile_path,
            options.cfgutil_target.as_deref(),
        ))
        .output()?;
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

fn merge_cfgutil_devices(devices: &mut Vec<MobileDevice>, configurator: &ConfiguratorDiscovery) {
    let Some(cfgutil_path) = configurator
        .cfgutil_path
        .as_ref()
        .filter(|_| configurator.cfgutil_available)
    else {
        return;
    };

    let Ok(output) = Command::new(cfgutil_path)
        .args(["--timeout", "1", "--format", "JSON", "list"])
        .output()
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
    fn detects_cfgutil_user_interaction_required_error() {
        let message = "cfgutil: error: 安装此描述文件需要用户在设备上交互。 (Domain: ConfigurationUtilityKit.error Code: 625)";

        assert!(is_cfgutil_user_interaction_required(message));
    }
}
