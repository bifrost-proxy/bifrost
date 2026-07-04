use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use bifrost_core::macos_native_app::{
    app_path_for_install_dir, default_install_dir, release_download_url, status_for_install_dir,
    BIFROST_NATIVE_APP_NAME,
};
use bifrost_core::{BifrostError, Result};
use serde_json::json;

use crate::cli::NativeAppCommands;

pub fn handle_native_app_command(action: NativeAppCommands) -> Result<()> {
    match action {
        NativeAppCommands::Status {
            install_dir,
            latest_version,
            format,
        } => {
            let install_dir = install_dir.unwrap_or_else(default_install_dir);
            let status = status_for_install_dir(&install_dir, latest_version.as_deref());
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("{}", status.message);
                println!("Path: {}", status.install_path);
                if let Some(version) = status.installed_version {
                    println!("Installed version: {version}");
                }
                if let Some(version) = status.latest_version {
                    println!("Latest version: {version}");
                }
            }
            Ok(())
        }
        NativeAppCommands::Install {
            source,
            url,
            latest_version,
            install_dir,
            dry_run,
            open,
            yes,
        } => {
            let options = NativeAppInstallOptions {
                source: source
                    .or_else(|| std::env::var_os("BIFROST_NATIVE_APP_SOURCE").map(PathBuf::from)),
                url: url.or_else(|| std::env::var("BIFROST_NATIVE_APP_URL").ok()),
                latest_version,
                install_dir: install_dir.unwrap_or_else(default_install_dir),
                dry_run,
                open_after_install: open,
            };
            confirm_install_if_needed(&options, yes)?;
            install_native_app(options)
        }
        NativeAppCommands::Uninstall {
            install_dir,
            dry_run,
            yes,
        } => {
            let install_dir = install_dir.unwrap_or_else(default_install_dir);
            confirm_uninstall_if_needed(&install_dir, dry_run, yes)?;
            uninstall_native_app(&install_dir, dry_run)
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeAppInstallOptions {
    pub source: Option<PathBuf>,
    pub url: Option<String>,
    pub latest_version: Option<String>,
    pub install_dir: PathBuf,
    pub dry_run: bool,
    pub open_after_install: bool,
}

pub fn install_native_app(options: NativeAppInstallOptions) -> Result<()> {
    let target_app = app_path_for_install_dir(&options.install_dir);
    let source = resolve_source(&options)?;

    if options.dry_run {
        println!(
            "{}",
            json!({
                "dry_run": true,
                "source": source.display().to_string(),
                "target": target_app.display().to_string(),
                "open_after_install": options.open_after_install,
            })
        );
        return Ok(());
    }

    fs::create_dir_all(&options.install_dir)?;
    let staged_app = if source.is_dir()
        && source.file_name().and_then(|s| s.to_str()) == Some(BIFROST_NATIVE_APP_NAME)
    {
        source
    } else if source.extension().and_then(|s| s.to_str()) == Some("dmg") {
        extract_app_from_dmg(&source)?
    } else {
        return Err(BifrostError::Config(format!(
            "Native app source must be a {} bundle or .dmg: {}",
            BIFROST_NATIVE_APP_NAME,
            source.display()
        )));
    };

    quit_native_app_before_replacement(&target_app);

    let temp_target = target_app.with_extension(format!("app.tmp.{}", std::process::id()));
    let backup_target = target_app.with_extension("app.backup");
    let _ = fs::remove_dir_all(&temp_target);
    copy_dir_recursive(&staged_app, &temp_target)?;
    if target_app.exists() {
        let _ = fs::remove_dir_all(&backup_target);
        fs::rename(&target_app, &backup_target)?;
    }
    if let Err(error) = fs::rename(&temp_target, &target_app) {
        if backup_target.exists() && !target_app.exists() {
            let _ = fs::rename(&backup_target, &target_app);
        }
        return Err(BifrostError::Io(error));
    }
    let _ = fs::remove_dir_all(&backup_target);

    #[cfg(target_os = "macos")]
    clear_quarantine_attr(&target_app);

    if options.open_after_install {
        open_app(&target_app)?;
    }

    let status = status_for_install_dir(&options.install_dir, options.latest_version.as_deref());
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

pub fn uninstall_native_app(install_dir: &Path, dry_run: bool) -> Result<()> {
    let target_app = app_path_for_install_dir(install_dir);
    if dry_run {
        println!(
            "{}",
            json!({
                "dry_run": true,
                "target": target_app.display().to_string(),
                "installed": target_app.exists(),
            })
        );
        return Ok(());
    }

    quit_native_app_before_replacement(&target_app);
    if target_app.exists() {
        fs::remove_dir_all(&target_app)?;
        println!("Removed {}", target_app.display());
    } else {
        println!(
            "Bifrost Native App is not installed at {}",
            target_app.display()
        );
    }
    Ok(())
}

fn confirm_install_if_needed(options: &NativeAppInstallOptions, yes: bool) -> Result<()> {
    if yes || options.dry_run {
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(BifrostError::Config(
            "Native app install requires -y in non-interactive mode".to_string(),
        ));
    }

    let target_app = app_path_for_install_dir(&options.install_dir);
    print!(
        "Install or update Bifrost Native App at {}? (y/n) ",
        target_app.display()
    );
    let _ = io::stdout().flush();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_ascii_lowercase();
    if matches!(answer.as_str(), "y" | "yes") {
        return Ok(());
    }
    Err(BifrostError::Config(
        "Native app installation was cancelled".to_string(),
    ))
}

fn confirm_uninstall_if_needed(install_dir: &Path, dry_run: bool, yes: bool) -> Result<()> {
    if yes || dry_run {
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(BifrostError::Config(
            "Native app uninstall requires -y in non-interactive mode".to_string(),
        ));
    }

    let target_app = app_path_for_install_dir(install_dir);
    print!(
        "Uninstall Bifrost Native App at {}? (y/n) ",
        target_app.display()
    );
    let _ = io::stdout().flush();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_ascii_lowercase();
    if matches!(answer.as_str(), "y" | "yes") {
        return Ok(());
    }
    Err(BifrostError::Config(
        "Native app uninstall was cancelled".to_string(),
    ))
}

fn resolve_source(options: &NativeAppInstallOptions) -> Result<PathBuf> {
    if let Some(source) = &options.source {
        return Ok(source.clone());
    }
    let url = options
        .url
        .clone()
        .or_else(|| {
            options
                .latest_version
                .as_deref()
                .and_then(release_download_url)
        })
        .ok_or_else(|| {
            BifrostError::Config(
                "Native app install needs --source, --url, BIFROST_NATIVE_APP_SOURCE, BIFROST_NATIVE_APP_URL, or --latest-version".to_string(),
            )
        })?;
    download_to_temp(&url)
}

fn download_to_temp(url: &str) -> Result<PathBuf> {
    let temp_dir = tempfile::Builder::new()
        .prefix("bifrost-native-app-download-")
        .tempdir()?
        .keep();
    let file_name = url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("BifrostNativeApp.dmg");
    let target = temp_dir.join(file_name);
    let response = bifrost_core::direct_ureq_agent()
        .get(url)
        .call()
        .map_err(|error| {
            BifrostError::Network(format!("Failed to download native app: {error}"))
        })?;
    let mut reader = response.into_reader();
    let mut file = fs::File::create(&target)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(target)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
            if let Ok(metadata) = fs::metadata(&source_path) {
                let _ = fs::set_permissions(&target_path, metadata.permissions());
            }
        }
    }
    Ok(())
}

fn quit_native_app_before_replacement(target_app: &Path) {
    if !target_app.exists() {
        return;
    }
    if cfg!(test) {
        return;
    }
    quit_native_app();
}

#[cfg(target_os = "macos")]
fn quit_native_app() {
    use bifrost_core::macos_native_app::BIFROST_NATIVE_BUNDLE_ID;
    let script = format!(
        "tell application id \"{}\" to quit",
        BIFROST_NATIVE_BUNDLE_ID
    );
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    for _ in 0..20 {
        if !is_native_app_process_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let _ = Command::new("pkill")
        .args(["-x", "Bifrost"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn is_native_app_process_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "Bifrost"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn quit_native_app() {}

#[cfg(target_os = "macos")]
fn extract_app_from_dmg(dmg_path: &Path) -> Result<PathBuf> {
    let mount_root = tempfile::Builder::new()
        .prefix("bifrost-native-app-mount-")
        .tempdir()?;
    let status = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(mount_root.path())
        .arg(dmg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()?;
    if !status.success() {
        return Err(BifrostError::Config(format!(
            "Failed to mount native app dmg: {}",
            dmg_path.display()
        )));
    }
    let app_path = mount_root.path().join(BIFROST_NATIVE_APP_NAME);
    if !app_path.exists() {
        detach_dmg(mount_root.path());
        return Err(BifrostError::Config(format!(
            "Mounted dmg does not contain {}",
            BIFROST_NATIVE_APP_NAME
        )));
    }
    let extracted_root = tempfile::Builder::new()
        .prefix("bifrost-native-app-extract-")
        .tempdir()?
        .keep();
    let extracted_app = extracted_root.join(BIFROST_NATIVE_APP_NAME);
    let copy_result = copy_dir_recursive(&app_path, &extracted_app);
    detach_dmg(mount_root.path());
    copy_result?;
    Ok(extracted_app)
}

#[cfg(not(target_os = "macos"))]
fn extract_app_from_dmg(dmg_path: &Path) -> Result<PathBuf> {
    Err(BifrostError::Config(format!(
        "Installing a dmg is supported only on macOS: {}",
        dmg_path.display()
    )))
}

#[cfg(target_os = "macos")]
fn detach_dmg(mount_point: &Path) {
    let _ = Command::new("hdiutil")
        .arg("detach")
        .arg(mount_point)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn open_app(app_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-n").arg(app_path).status()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_path;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn clear_quarantine_attr(path: &Path) {
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_app(root: &Path, version: &str) -> PathBuf {
        let app = root.join(BIFROST_NATIVE_APP_NAME);
        let contents = app.join("Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!(
                "<plist><dict><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"
            ),
        )
        .unwrap();
        app
    }

    #[test]
    fn installs_local_app_bundle_into_target_dir() {
        let source_root = tempfile::tempdir().unwrap();
        let install_root = tempfile::tempdir().unwrap();
        let source = fixture_app(source_root.path(), "0.0.138");

        install_native_app(NativeAppInstallOptions {
            source: Some(source),
            url: None,
            latest_version: Some("0.0.138".to_string()),
            install_dir: install_root.path().to_path_buf(),
            dry_run: false,
            open_after_install: false,
        })
        .unwrap();

        let status = status_for_install_dir(install_root.path(), Some("0.0.138"));
        assert!(status.installed);
        assert_eq!(status.installed_version, Some("0.0.138".to_string()));
    }

    #[test]
    fn dry_run_does_not_create_target_app() {
        let source_root = tempfile::tempdir().unwrap();
        let install_root = tempfile::tempdir().unwrap();
        let source = fixture_app(source_root.path(), "0.0.138");

        install_native_app(NativeAppInstallOptions {
            source: Some(source),
            url: None,
            latest_version: Some("0.0.138".to_string()),
            install_dir: install_root.path().to_path_buf(),
            dry_run: true,
            open_after_install: false,
        })
        .unwrap();

        assert!(!install_root.path().join(BIFROST_NATIVE_APP_NAME).exists());
    }

    #[test]
    fn uninstall_removes_target_app() {
        let install_root = tempfile::tempdir().unwrap();
        fixture_app(install_root.path(), "0.0.138");

        uninstall_native_app(install_root.path(), false).unwrap();

        assert!(!install_root.path().join(BIFROST_NATIVE_APP_NAME).exists());
    }

    #[test]
    fn dry_run_uninstall_keeps_target_app() {
        let install_root = tempfile::tempdir().unwrap();
        fixture_app(install_root.path(), "0.0.138");

        uninstall_native_app(install_root.path(), true).unwrap();

        assert!(install_root.path().join(BIFROST_NATIVE_APP_NAME).exists());
    }

    #[test]
    fn non_interactive_install_requires_yes_flag() {
        let install_root = tempfile::tempdir().unwrap();
        let error = confirm_install_if_needed(
            &NativeAppInstallOptions {
                source: None,
                url: None,
                latest_version: Some("0.0.138".to_string()),
                install_dir: install_root.path().to_path_buf(),
                dry_run: false,
                open_after_install: false,
            },
            false,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires -y in non-interactive mode"));
    }

    #[test]
    fn non_interactive_uninstall_requires_yes_flag() {
        let install_root = tempfile::tempdir().unwrap();
        let error = confirm_uninstall_if_needed(install_root.path(), false, false).unwrap_err();

        assert!(error
            .to_string()
            .contains("requires -y in non-interactive mode"));
    }
}
