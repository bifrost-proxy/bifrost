use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const BIFROST_NATIVE_APP_NAME: &str = "Bifrost.app";
pub const BIFROST_NATIVE_BUNDLE_ID: &str = "com.bifrost.native.mac";
pub const BIFROST_NATIVE_RELEASE_ASSET_PREFIX: &str = "bifrost-native";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacosNativeAppStatus {
    pub supported: bool,
    pub installed: bool,
    pub install_path: String,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub needs_install: bool,
    pub download_url: Option<String>,
    pub message: String,
}

pub fn default_install_dir() -> PathBuf {
    std::env::var_os("BIFROST_NATIVE_APP_INSTALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Applications"))
}

pub fn app_path_for_install_dir(install_dir: &Path) -> PathBuf {
    install_dir.join(BIFROST_NATIVE_APP_NAME)
}

pub fn release_asset_name(version: &str) -> Option<String> {
    let target = native_app_target_triple()?;
    Some(format!(
        "{BIFROST_NATIVE_RELEASE_ASSET_PREFIX}-v{}-{}.dmg",
        version.trim_start_matches('v'),
        target
    ))
}

pub fn release_download_url(version: &str) -> Option<String> {
    let normalized = version.trim_start_matches('v');
    let asset = release_asset_name(normalized)?;
    Some(format!(
        "https://github.com/bifrost-proxy/bifrost/releases/download/v{normalized}/{asset}"
    ))
}

pub fn native_app_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Some("aarch64-apple-darwin");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Some("x86_64-apple-darwin");
    }
    #[allow(unreachable_code)]
    None
}

pub fn read_installed_version(app_path: &Path) -> Option<String> {
    let plist_path = app_path.join("Contents").join("Info.plist");
    let content = std::fs::read_to_string(plist_path).ok()?;
    string_value_after_key(&content, "CFBundleShortVersionString")
        .or_else(|| string_value_after_key(&content, "CFBundleVersion"))
}

fn string_value_after_key(content: &str, key: &str) -> Option<String> {
    let key_marker = format!("<key>{key}</key>");
    let after_key = content.split_once(&key_marker)?.1;
    let after_string = after_key.split_once("<string>")?.1;
    let value = after_string.split_once("</string>")?.0.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn status_for_install_dir(
    install_dir: &Path,
    latest_version: Option<&str>,
) -> MacosNativeAppStatus {
    let supported = native_app_target_triple().is_some();
    let app_path = app_path_for_install_dir(install_dir);
    let installed_version = read_installed_version(&app_path);
    let installed = installed_version.is_some() || app_path.exists();
    let latest = latest_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches('v').to_string());
    let needs_install = supported
        && match (installed_version.as_deref(), latest.as_deref()) {
            (Some(installed), Some(latest)) => {
                crate::version_check::is_newer_version(installed, latest)
            }
            (None, _) => true,
            (Some(_), None) => false,
        };
    let download_url = latest.as_deref().and_then(release_download_url);
    let message = if !supported {
        "Bifrost Native App is available only on macOS.".to_string()
    } else if !installed {
        "Bifrost Native App is not installed.".to_string()
    } else if needs_install {
        "A newer Bifrost Native App is available.".to_string()
    } else {
        "Bifrost Native App is installed.".to_string()
    };

    MacosNativeAppStatus {
        supported,
        installed,
        install_path: app_path.to_string_lossy().to_string(),
        installed_version,
        latest_version: latest,
        needs_install,
        download_url,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-native-app-core-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_installed_version_from_info_plist() {
        let dir = temp_dir("version");
        let plist = dir
            .join(BIFROST_NATIVE_APP_NAME)
            .join("Contents")
            .join("Info.plist");
        std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
        std::fs::write(
            &plist,
            r#"<plist><dict><key>CFBundleShortVersionString</key><string>0.0.137</string></dict></plist>"#,
        )
        .unwrap();

        assert_eq!(
            read_installed_version(&dir.join(BIFROST_NATIVE_APP_NAME)),
            Some("0.0.137".to_string())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_marks_missing_app_as_needing_install_on_supported_platforms() {
        let dir = temp_dir("missing");
        let status = status_for_install_dir(&dir, Some("0.0.138"));
        assert!(!status.installed);
        assert_eq!(status.needs_install, native_app_target_triple().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn release_asset_name_matches_native_app_contract_when_supported() {
        if let Some(target) = native_app_target_triple() {
            assert_eq!(
                release_asset_name("0.0.138"),
                Some(format!("bifrost-native-v0.0.138-{target}.dmg"))
            );
        } else {
            assert_eq!(release_asset_name("0.0.138"), None);
        }
    }
}
