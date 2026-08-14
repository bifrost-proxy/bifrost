use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};

use bifrost_core::BifrostError;

#[cfg(any(target_os = "windows", test))]
pub(super) const WINDOWS_DESKTOP_VERSION_PATH_ENV: &str = "BIFROST_DESKTOP_VERSION_PATH";

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_desktop_version_probe_script() -> &'static str {
    r#"
$path = $env:BIFROST_DESKTOP_VERSION_PATH
if (-not $path) { exit 2 }
$info = (Get-Item -LiteralPath $path).VersionInfo
if ($info.ProductVersion) { $info.ProductVersion } elseif ($info.FileVersion) { $info.FileVersion }
"#
}

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
    let installed = normalize_version(installed);
    let target = normalize_version(target);
    installed == target || windows_msi_safe_version(target).as_deref() == Some(installed)
}

fn windows_msi_safe_version(version: &str) -> Option<String> {
    let version = normalize_version(version);
    let without_build = version.split('+').next()?;
    let Some((core, prerelease)) = without_build.split_once('-') else {
        return Some(without_build.to_string());
    };
    let identifiers: Vec<_> = prerelease
        .split(['.', '-'])
        .filter(|part| !part.is_empty())
        .collect();
    let first = identifiers.first()?.to_ascii_lowercase();

    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let value = first.parse::<u32>().ok()?;
        return (value <= 65_535).then(|| format!("{core}-{value}"));
    }

    let alphabetic_end = first
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .unwrap_or(first.len());
    let has_valid_label_shape = alphabetic_end > 0
        && first[alphabetic_end..]
            .chars()
            .all(|ch| ch.is_ascii_digit());
    let label = if has_valid_label_shape {
        &first[..alphabetic_end]
    } else {
        first.as_str()
    };
    let inline_sequence = (has_valid_label_shape && alphabetic_end < first.len())
        .then(|| first[alphabetic_end..].parse::<u32>().ok())
        .flatten();
    let explicit_sequence = identifiers
        .iter()
        .skip(1)
        .find_map(|part| part.parse::<u32>().ok());
    let channel_base = match label {
        "alpha" => Some(10_000),
        "beta" => Some(20_000),
        "rc" => Some(30_000),
        _ => None,
    };
    let fallback_hash = prerelease.bytes().map(u32::from).sum::<u32>() % 10_000;
    let base = channel_base.unwrap_or(40_000);
    let sequence = inline_sequence
        .or(explicit_sequence)
        .unwrap_or_else(|| channel_base.map(|_| 0).unwrap_or(fallback_hash))
        .min(9_999);
    Some(format!("{core}-{}", base + sequence))
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
        .env(WINDOWS_DESKTOP_VERSION_PATH_ENV, install_path)
        .arg("-NoProfile")
        .arg("-Command")
        .arg(windows_desktop_version_probe_script())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}
