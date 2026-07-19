use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};

use bifrost_core::BifrostError;

pub(crate) fn installed_desktop_app_is_target_version(
    install_path: &Path,
    target_version: &str,
) -> bool {
    installed_desktop_app_version(install_path)
        .map(|installed| versions_equal(&installed, target_version))
        .unwrap_or(false)
}

pub(super) fn verify_installed_desktop_target_version(
    install_path: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
    let Some(installed) = installed_desktop_app_version(install_path) else {
        return Err(BifrostError::Config(format!(
            "desktop app install completed but {} does not report an installed version",
            install_path.display()
        )));
    };
    if versions_equal(&installed, target_version) {
        return Ok(());
    }
    Err(BifrostError::Config(format!(
        "desktop app install completed but {} reports version v{} instead of target v{}",
        install_path.display(),
        normalize_version(&installed),
        normalize_version(target_version)
    )))
}

pub(super) fn versions_equal(installed: &str, target: &str) -> bool {
    normalize_version(installed) == normalize_version(target)
}

pub(super) fn normalize_version(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

pub(super) fn installed_desktop_app_version(install_path: &Path) -> Option<String> {
    // App bundles are ordinary directories containing an Info.plist. Keep this
    // parser available on every host so the atomic staging/swap contract can be
    // exercised in Linux CI as well as on macOS.
    #[cfg(any(target_os = "macos", test))]
    if install_path.is_dir() {
        return read_macos_app_version(install_path);
    }
    #[cfg(target_os = "macos")]
    {
        None
    }
    #[cfg(target_os = "windows")]
    {
        read_windows_app_version(install_path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = install_path;
        None
    }
}

#[cfg(any(target_os = "macos", test))]
fn read_macos_app_version(install_path: &Path) -> Option<String> {
    let plist_path = install_path.join("Contents").join("Info.plist");
    let plist = plist::Value::from_file(plist_path).ok()?;
    let dict = plist.as_dictionary()?;
    ["CFBundleShortVersionString", "CFBundleVersion"]
        .into_iter()
        .find_map(|key| dict.get(key).and_then(|value| value.as_string()))
        .map(str::to_string)
}

#[cfg(target_os = "windows")]
fn read_windows_app_version(install_path: &Path) -> Option<String> {
    if !install_path.is_file() {
        return None;
    }
    let script = r#"
param([string]$Path)
$info = (Get-Item -LiteralPath $Path).VersionInfo
if ($info.ProductVersion) { $info.ProductVersion } elseif ($info.FileVersion) { $info.FileVersion }
"#;
    let powershell = if Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
    {
        "powershell.exe"
    } else {
        "pwsh"
    };
    let output = Command::new(powershell)
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .arg(install_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}
