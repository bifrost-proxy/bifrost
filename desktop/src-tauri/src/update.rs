use reqwest::blocking::Client;
use serde::Serialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const REMOTE_CARGO_TOML_URL: &str =
    "https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/desktop/src-tauri/Cargo.toml";
const UPDATE_EVENT: &str = "desktop://update-status";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusPayload {
    pub phase: UpdatePhase,
    pub message: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub download_url: Option<String>,
    pub downloaded_path: Option<String>,
    pub progress: Option<DownloadProgress>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    Checking,
    UpToDate,
    UpdateAvailable,
    Downloading,
    Downloaded,
    Installing,
    Done,
    Error,
}

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub dry_run_install: bool,
    pub platform_override: Option<String>,
    pub current_version_override: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSummary {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub download_url: Option<String>,
    pub downloaded_path: Option<String>,
    pub dry_run_install: bool,
}

pub fn check_and_install_update(
    app: AppHandle,
    options: UpdateOptions,
) -> Result<UpdateSummary, String> {
    let mut emitter = |payload: UpdateStatusPayload| {
        let _ = app.emit(UPDATE_EVENT, payload);
    };

    let summary = check_and_install_update_with_emitter(&mut emitter, &options)?;

    if summary.update_available && !summary.dry_run_install {
        app.exit(0);
    }

    Ok(summary)
}

pub fn check_and_install_update_with_emitter(
    emit: &mut dyn FnMut(UpdateStatusPayload),
    options: &UpdateOptions,
) -> Result<UpdateSummary, String> {
    let current_version = options
        .current_version_override
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    emit(UpdateStatusPayload {
        phase: UpdatePhase::Checking,
        message: "正在检查更新".to_string(),
        current_version: Some(current_version.clone()),
        latest_version: None,
        download_url: None,
        downloaded_path: None,
        progress: None,
    });

    let latest_version = fetch_latest_version()?;

    if !is_version_newer(&latest_version, &current_version) {
        emit(UpdateStatusPayload {
            phase: UpdatePhase::UpToDate,
            message: format!("已是最新版本 v{current_version}"),
            current_version: Some(current_version.clone()),
            latest_version: Some(latest_version.clone()),
            download_url: None,
            downloaded_path: None,
            progress: None,
        });

        return Ok(UpdateSummary {
            update_available: false,
            current_version,
            latest_version: Some(latest_version),
            download_url: None,
            downloaded_path: None,
            dry_run_install: options.dry_run_install,
        });
    }

    let (download_url, asset_name) = build_release_asset_url(&latest_version, options)?;

    emit(UpdateStatusPayload {
        phase: UpdatePhase::UpdateAvailable,
        message: format!("发现新版本 v{latest_version}"),
        current_version: Some(current_version.clone()),
        latest_version: Some(latest_version.clone()),
        download_url: Some(download_url.clone()),
        downloaded_path: None,
        progress: None,
    });

    let download_path = download_asset(
        emit,
        &download_url,
        &asset_name,
        &current_version,
        &latest_version,
    )?;

    emit(UpdateStatusPayload {
        phase: UpdatePhase::Downloaded,
        message: "下载完成".to_string(),
        current_version: Some(current_version.clone()),
        latest_version: Some(latest_version.clone()),
        download_url: Some(download_url.clone()),
        downloaded_path: Some(download_path.display().to_string()),
        progress: None,
    });

    emit(UpdateStatusPayload {
        phase: UpdatePhase::Installing,
        message: if options.dry_run_install {
            "(dry-run) 准备执行安装".to_string()
        } else {
            "正在启动安装".to_string()
        },
        current_version: Some(current_version.clone()),
        latest_version: Some(latest_version.clone()),
        download_url: Some(download_url.clone()),
        downloaded_path: Some(download_path.display().to_string()),
        progress: None,
    });

    install_downloaded_asset(&download_path, &latest_version, options)?;

    emit(UpdateStatusPayload {
        phase: UpdatePhase::Done,
        message: if options.dry_run_install {
            "(dry-run) 安装流程已模拟完成".to_string()
        } else {
            "安装已启动，应用即将退出".to_string()
        },
        current_version: Some(current_version.clone()),
        latest_version: Some(latest_version.clone()),
        download_url: Some(download_url.clone()),
        downloaded_path: Some(download_path.display().to_string()),
        progress: None,
    });

    Ok(UpdateSummary {
        update_available: true,
        current_version,
        latest_version: Some(latest_version),
        download_url: Some(download_url),
        downloaded_path: Some(download_path.display().to_string()),
        dry_run_install: options.dry_run_install,
    })
}

fn fetch_latest_version() -> Result<String, String> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("failed to build update http client: {error}"))?;

    let response = client
        .get(REMOTE_CARGO_TOML_URL)
        .header(reqwest::header::USER_AGENT, "bifrost-desktop")
        .send()
        .map_err(|error| format!("failed to fetch remote Cargo.toml: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!(
            "remote Cargo.toml request failed with status {status}: {body}"
        ));
    }

    let content = response
        .text()
        .map_err(|error| format!("failed to read remote Cargo.toml response: {error}"))?;

    parse_version_from_cargo_toml(&content)
}

fn parse_version_from_cargo_toml(content: &str) -> Result<String, String> {
    let manifest: toml::Value =
        toml::from_str(content).map_err(|error| format!("invalid Cargo.toml: {error}"))?;
    let version = manifest
        .get("package")
        .and_then(|pkg| pkg.get("version"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing package.version in remote Cargo.toml".to_string())?;

    Ok(version.to_string())
}

fn is_version_newer(latest: &str, current: &str) -> bool {
    let Ok(latest_version) = semver::Version::parse(latest) else {
        return latest != current;
    };
    let Ok(current_version) = semver::Version::parse(current) else {
        return latest != current;
    };

    latest_version > current_version
}

fn build_release_asset_url(
    version: &str,
    options: &UpdateOptions,
) -> Result<(String, String), String> {
    let platform = options
        .platform_override
        .clone()
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let arch = std::env::consts::ARCH;

    let (target, ext) = match platform.as_str() {
        "windows" => {
            let target = match arch {
                "x86_64" => "x86_64-pc-windows-msvc",
                "aarch64" => "aarch64-pc-windows-msvc",
                other => {
                    return Err(format!("unsupported windows architecture: {other}"));
                }
            };
            (target, "msi")
        }
        "macos" | "darwin" => {
            let target = match arch {
                "x86_64" => "x86_64-apple-darwin",
                "aarch64" => "aarch64-apple-darwin",
                other => {
                    return Err(format!("unsupported macos architecture: {other}"));
                }
            };
            (target, "dmg")
        }
        other => {
            return Err(format!("unsupported platform for desktop updater: {other}"));
        }
    };

    let asset_name = format!("bifrost-desktop-v{version}-{target}.{ext}");
    let url = format!(
        "https://github.com/bifrost-proxy/bifrost/releases/download/v{version}/{asset_name}"
    );

    Ok((url, asset_name))
}

fn download_asset(
    emit: &mut dyn FnMut(UpdateStatusPayload),
    download_url: &str,
    asset_name: &str,
    current_version: &str,
    latest_version: &str,
) -> Result<PathBuf, String> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("failed to build download client: {error}"))?;

    let mut response = client
        .get(download_url)
        .header(reqwest::header::USER_AGENT, "bifrost-desktop")
        .send()
        .map_err(|error| format!("request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("download failed with status {status}: {body}"));
    }

    let total_bytes = response.content_length();
    let download_path = std::env::temp_dir().join(asset_name);
    let mut file = fs::File::create(&download_path)
        .map_err(|error| format!("failed to create temp file: {error}"))?;

    emit(UpdateStatusPayload {
        phase: UpdatePhase::Downloading,
        message: "正在下载更新包".to_string(),
        current_version: Some(current_version.to_string()),
        latest_version: Some(latest_version.to_string()),
        download_url: Some(download_url.to_string()),
        downloaded_path: Some(download_path.display().to_string()),
        progress: Some(DownloadProgress {
            downloaded_bytes: 0,
            total_bytes,
            percent: total_bytes.map(|_| 0),
        }),
    });

    let mut downloaded_bytes: u64 = 0;
    let mut buffer = [0u8; 64 * 1024];
    let mut last_emit = Instant::now();
    let mut last_percent: Option<u8> = None;

    loop {
        let n = response
            .read(&mut buffer)
            .map_err(|error| format!("download stream read failed: {error}"))?;
        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n])
            .map_err(|error| format!("failed to write to temp file: {error}"))?;
        downloaded_bytes = downloaded_bytes.saturating_add(n as u64);

        let percent = total_bytes.and_then(|total| {
            if total == 0 {
                None
            } else {
                let value = (downloaded_bytes.saturating_mul(100) / total).min(100) as u8;
                Some(value)
            }
        });

        let should_emit = last_emit.elapsed() >= Duration::from_millis(220)
            || percent.is_some_and(|p| Some(p) != last_percent);

        if should_emit {
            last_emit = Instant::now();
            last_percent = percent;
            emit(UpdateStatusPayload {
                phase: UpdatePhase::Downloading,
                message: "正在下载更新包".to_string(),
                current_version: Some(current_version.to_string()),
                latest_version: Some(latest_version.to_string()),
                download_url: Some(download_url.to_string()),
                downloaded_path: Some(download_path.display().to_string()),
                progress: Some(DownloadProgress {
                    downloaded_bytes,
                    total_bytes,
                    percent,
                }),
            });
        }
    }

    file.flush()
        .map_err(|error| format!("failed to flush download file: {error}"))?;

    Ok(download_path)
}

fn install_downloaded_asset(
    download_path: &Path,
    version: &str,
    options: &UpdateOptions,
) -> Result<(), String> {
    if options.dry_run_install {
        return Ok(());
    }

    let platform = options
        .platform_override
        .clone()
        .unwrap_or_else(|| std::env::consts::OS.to_string());

    match platform.as_str() {
        "windows" => install_windows_msi(download_path),
        "macos" | "darwin" => install_macos_dmg(download_path, version),
        other => Err(format!("unsupported platform for install step: {other}")),
    }
}

fn install_windows_msi(msi_path: &Path) -> Result<(), String> {
    Command::new("msiexec.exe")
        .arg("/i")
        .arg(msi_path)
        .arg("/passive")
        .arg("/norestart")
        .spawn()
        .map_err(|error| format!("failed to spawn msiexec: {error}"))?;

    Ok(())
}

fn install_macos_dmg(dmg_path: &Path, version: &str) -> Result<(), String> {
    let pid = std::process::id();
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable: {error}"))?;

    let app_bundle_path = current_exe
        .ancestors()
        .nth(3)
        .ok_or_else(|| "failed to locate .app bundle from executable path".to_string())?;

    if app_bundle_path.extension().and_then(|s| s.to_str()) != Some("app") {
        return Err(format!(
            "unexpected macOS app bundle path: {}",
            app_bundle_path.display()
        ));
    }

    let mount_point = mount_dmg(dmg_path)?;
    let source_app = find_app_bundle(&mount_point).ok_or_else(|| {
        let _ = detach_dmg(&mount_point);
        format!(
            "failed to locate .app bundle under mounted dmg at {}",
            mount_point.display()
        )
    })?;

    let temp_dir = std::env::temp_dir().join(format!("bifrost-update-{version}"));
    fs::create_dir_all(&temp_dir)
        .map_err(|error| format!("failed to create update temp dir: {error}"))?;

    let script_path = temp_dir.join("apply_update.sh");
    write_update_script(
        &script_path,
        pid,
        &source_app,
        app_bundle_path,
        &mount_point,
    )?;

    Command::new("/bin/sh")
        .arg(&script_path)
        .spawn()
        .map_err(|error| format!("failed to spawn update helper script: {error}"))?;

    Ok(())
}

fn mount_dmg(dmg_path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("hdiutil")
        .arg("attach")
        .arg(dmg_path)
        .arg("-nobrowse")
        .arg("-readonly")
        .output()
        .map_err(|error| format!("failed to execute hdiutil attach: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().rev() {
        if let Some(mount) = line.split('\t').last() {
            let mount = mount.trim();
            if mount.starts_with('/') {
                return Ok(PathBuf::from(mount));
            }
        }

        if let Some(idx) = line.rfind("/Volumes/") {
            return Ok(PathBuf::from(line[idx..].trim()));
        }
    }

    Err("failed to parse mount point from hdiutil output".to_string())
}

fn detach_dmg(mount_point: &Path) -> Result<(), String> {
    let status = Command::new("hdiutil")
        .arg("detach")
        .arg(mount_point)
        .arg("-quiet")
        .status()
        .map_err(|error| format!("failed to detach dmg: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("hdiutil detach returned non-zero".to_string())
    }
}

fn find_app_bundle(mount_point: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(mount_point).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("app") {
            return Some(path);
        }
    }
    None
}

fn write_update_script(
    script_path: &Path,
    pid: u32,
    source_app: &Path,
    dest_app: &Path,
    mount_point: &Path,
) -> Result<(), String> {
    let content = format!(
        "#!/bin/sh\n\
set -e\n\
PID=\"{pid}\"\n\
SRC_APP=\"{}\"\n\
DEST_APP=\"{}\"\n\
MOUNT_POINT=\"{}\"\n\
\
while kill -0 \"${{PID}}\" 2>/dev/null; do\n\
  sleep 0.2\n\
done\n\
\
rm -rf \"${{DEST_APP}}\"\n\
cp -R \"${{SRC_APP}}\" \"${{DEST_APP}}\"\n\
\
hdiutil detach \"${{MOUNT_POINT}}\" -quiet || true\n\
open \"${{DEST_APP}}\" || true\n",
        source_app.display(),
        dest_app.display(),
        mount_point.display()
    );

    fs::write(script_path, content)
        .map_err(|error| format!("failed to write update helper script: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(script_path)
            .map_err(|error| format!("failed to read script metadata: {error}"))?
            .permissions();
        perm.set_mode(0o755);
        fs::set_permissions(script_path, perm)
            .map_err(|error| format!("failed to set script permissions: {error}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_from_manifest() {
        let manifest = r#"
[package]
name = "bifrost-desktop"
version = "1.2.3-beta.1"
"#;

        let version = parse_version_from_cargo_toml(manifest).unwrap();
        assert_eq!(version, "1.2.3-beta.1");
    }

    #[test]
    fn test_build_release_asset_url_windows() {
        let options = UpdateOptions {
            dry_run_install: true,
            platform_override: Some("windows".to_string()),
            current_version_override: None,
        };

        let (url, asset) = build_release_asset_url("0.0.1", &options).unwrap();
        assert!(asset.ends_with(".msi"));
        assert!(url.contains(&asset));
    }

    #[test]
    fn test_build_release_asset_url_macos() {
        let options = UpdateOptions {
            dry_run_install: true,
            platform_override: Some("macos".to_string()),
            current_version_override: None,
        };

        let (url, asset) = build_release_asset_url("0.0.1", &options).unwrap();
        assert!(asset.ends_with(".dmg"));
        assert!(url.contains(&asset));
    }

    #[test]
    fn e2e_download_latest_release_asset() {
        if std::env::var("RUN_DESKTOP_UPDATE_E2E").is_err() {
            return;
        }

        let latest = fetch_latest_version().expect("fetch latest version");
        let options = UpdateOptions {
            dry_run_install: true,
            platform_override: Some("windows".to_string()),
            current_version_override: None,
        };
        let (url, asset) = build_release_asset_url(&latest, &options).expect("build url");

        let mut emitter = |_payload: UpdateStatusPayload| {};
        let path = download_asset(&mut emitter, &url, &asset, "0.0.0", &latest).expect("download");

        assert!(path.exists());
        let metadata = fs::metadata(&path).expect("metadata");
        assert!(metadata.len() > 0);
    }

    #[test]
    fn e2e_check_and_install_update_flow() {
        if std::env::var("RUN_DESKTOP_UPDATE_E2E").is_err() {
            return;
        }

        let options = UpdateOptions {
            dry_run_install: true,
            platform_override: Some("windows".to_string()),
            current_version_override: Some("0.0.0".to_string()),
        };

        let mut emitter = |_payload: UpdateStatusPayload| {};
        let summary = check_and_install_update_with_emitter(&mut emitter, &options)
            .expect("check update flow");

        assert!(summary.update_available);
        assert!(summary.latest_version.is_some());
        assert!(summary.download_url.is_some());
        assert!(summary.downloaded_path.is_some());
    }
}
