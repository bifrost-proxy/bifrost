use std::env;
#[cfg(any(target_os = "windows", test))]
use std::ffi::OsString;
use std::fs;
#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};
use bifrost_core::version_check::make_release_tag;
use bifrost_core::BifrostError;
use bifrost_storage::data_dir;
use colored::Colorize;

use crate::cli::AppCommands;

use super::update_check::get_latest_version_fresh_with_diagnostics;
use super::upgrade::{
    download_progress_line, handle_upgrade, DESKTOP_MANAGED_SKIP_APP_ENV,
    DESKTOP_MANAGED_SKIP_RESTART_ENV, DESKTOP_MANAGED_TARGET_ENV,
};

mod installer;
use installer::*;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_APP_NAME: &str = "Bifrost";
const MACOS_APP_BUNDLE: &str = "Bifrost.app";
#[cfg(target_os = "windows")]
const WINDOWS_APP_EXE: &str = "bifrost-desktop.exe";
#[cfg(target_os = "windows")]
const WINDOWS_LEGACY_APP_EXE: &str = "Bifrost.exe";
const GITHUB_RELEASE_DOWNLOAD_URL: &str =
    "https://github.com/bifrost-proxy/bifrost/releases/download";
const CALLER_MANAGED_PROGRESS_SOURCE: &str = "cli-upgrade";
const DESKTOP_MANAGED_CLI_TIMEOUT: Duration = Duration::from_secs(600);
const DESKTOP_MANAGED_CLI_HEARTBEAT: Duration = Duration::from_secs(30);
const DESKTOP_MANAGED_CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DESKTOP_INSTALL_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DESKTOP_INSTALL_COMMAND_HEARTBEAT: Duration = Duration::from_secs(30);

pub fn handle_app_command(action: AppCommands) -> Result<(), BifrostError> {
    match action {
        AppCommands::Install {
            package,
            app_dir,
            version,
            dry_run,
            yes,
        } => install_or_upgrade_app(AppInstallRequest {
            operation: AppOperation::Install,
            package,
            app_dir,
            version,
            include_cli: false,
            source: None,
            dry_run,
            yes,
        }),
        AppCommands::Uninstall {
            app_dir,
            dry_run,
            yes,
        } => uninstall_app(app_dir, dry_run, yes),
        AppCommands::Upgrade {
            package,
            app_dir,
            version,
            no_cli,
            source,
            dry_run,
            yes,
        } => install_or_upgrade_app(AppInstallRequest {
            operation: AppOperation::Upgrade,
            package,
            app_dir,
            version,
            include_cli: !no_cli,
            source,
            dry_run,
            yes,
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppOperation {
    Install,
    Upgrade,
}

struct AppInstallRequest {
    operation: AppOperation,
    package: Option<PathBuf>,
    app_dir: Option<PathBuf>,
    version: Option<String>,
    include_cli: bool,
    source: Option<String>,
    dry_run: bool,
    yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopTarget {
    MacosAarch64,
    MacosX64,
    WindowsX64,
    WindowsArm64,
}

impl DesktopTarget {
    fn current() -> Option<Self> {
        match (env::consts::OS, env::consts::ARCH) {
            ("macos", "aarch64") => Some(Self::MacosAarch64),
            ("macos", "x86_64") => Some(Self::MacosX64),
            ("windows", "x86_64") => Some(Self::WindowsX64),
            ("windows", "aarch64") => Some(Self::WindowsArm64),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::MacosAarch64 => "aarch64-apple-darwin",
            Self::MacosX64 => "x86_64-apple-darwin",
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
            Self::WindowsArm64 => "aarch64-pc-windows-msvc",
        }
    }

    fn package_ext(self) -> &'static str {
        match self {
            Self::MacosAarch64 | Self::MacosX64 => "dmg",
            Self::WindowsX64 | Self::WindowsArm64 => "msi",
        }
    }
}

fn install_or_upgrade_app(request: AppInstallRequest) -> Result<(), BifrostError> {
    let AppInstallRequest {
        operation,
        package,
        app_dir,
        version,
        include_cli,
        source,
        dry_run,
        yes: _yes,
    } = request;
    let progress_source = source.unwrap_or_else(|| "cli".to_string());
    let target_version = resolve_target_version(version)?;
    let install_dir = resolve_app_dir_for_source(app_dir, &progress_source)?;
    let install_path = resolve_app_path(&install_dir);

    println!(
        "{} {}",
        match operation {
            AppOperation::Install => "Desktop app install target:".bright_cyan(),
            AppOperation::Upgrade => "Desktop app upgrade target:".bright_cyan(),
        },
        install_path.display()
    );
    println!("{} v{}", "Target version:".bright_cyan(), target_version);

    if dry_run {
        println!("{}", "Dry run: no files will be changed.".bright_yellow());
        if include_cli {
            println!(
                "{}",
                "Would upgrade CLI with `bifrost upgrade` if installed.".dimmed()
            );
        }
        println!(
            "{} {}",
            "Would install desktop package from:".dimmed(),
            package
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| release_asset_url(&target_version)
                    .unwrap_or_else(|_| "<unsupported platform>".to_string()))
        );
        println!(
            "{}",
            if progress_source == "desktop" {
                "Would let the current desktop shell restart after a successful install.".dimmed()
            } else {
                "Would restart the desktop app after a successful install.".dimmed()
            }
        );
        return Ok(());
    }

    write_app_progress(
        UpgradePhase::Checking,
        "Checking desktop app update…",
        Some(target_version.clone()),
        &progress_source,
        None,
        None,
    );

    if include_cli {
        upgrade_cli_if_present(&progress_source, &target_version).inspect_err(|error| {
            write_app_failed_progress(&target_version, &progress_source, error);
        })?;
    }

    if operation == AppOperation::Upgrade
        && package.is_none()
        && installed_desktop_app_is_target_version(&install_path, &target_version)
    {
        write_app_progress(
            UpgradePhase::Completed,
            "Desktop app is already up to date",
            Some(target_version.clone()),
            &progress_source,
            None,
            None,
        );
        println!(
            "{}",
            format!(
                "✓ Desktop app is already on target version (v{}); skipping install.",
                target_version
            )
            .bright_green()
            .bold()
        );
        return Ok(());
    }

    let package_path = match package {
        Some(path) => path,
        None => {
            download_desktop_package(&target_version, &progress_source).inspect_err(|error| {
                write_app_failed_progress(&target_version, &progress_source, error);
            })?
        }
    };

    write_app_progress(
        UpgradePhase::Installing,
        "Installing desktop app…",
        Some(target_version.clone()),
        &progress_source,
        None,
        None,
    );
    install_desktop_package(
        &package_path,
        &install_dir,
        &install_path,
        &target_version,
        &progress_source,
    )
    .inspect_err(|error| {
        write_app_failed_progress(&target_version, &progress_source, error);
    })?;
    verify_installed_desktop_target_version(&install_path, &target_version).inspect_err(
        |error| {
            write_app_failed_progress(&target_version, &progress_source, error);
        },
    )?;

    write_app_progress(
        UpgradePhase::Restarting,
        if progress_source == "desktop" {
            "Waiting for desktop shell to restart…"
        } else {
            "Restarting desktop app…"
        },
        Some(target_version.clone()),
        &progress_source,
        None,
        None,
    );
    if progress_source != "desktop" && !skip_desktop_restart() {
        restart_desktop_app(&install_path).inspect_err(|error| {
            write_app_failed_progress(&target_version, &progress_source, error);
        })?;
    }

    write_app_progress(
        UpgradePhase::Completed,
        "Desktop app update complete",
        Some(target_version),
        &progress_source,
        None,
        None,
    );
    println!("{}", "✓ Desktop app is up to date.".bright_green().bold());
    Ok(())
}

fn write_app_failed_progress(target_version: &str, progress_source: &str, error: &BifrostError) {
    write_app_progress(
        UpgradePhase::Failed,
        "Desktop app update failed",
        Some(target_version.to_string()),
        progress_source,
        None,
        Some(error.to_string()),
    );
}

fn upgrade_cli_if_present(progress_source: &str, target_version: &str) -> Result<(), BifrostError> {
    println!(
        "{}",
        "Checking CLI install before desktop app install...".bright_cyan()
    );

    if progress_source != "desktop" {
        // The visible `bifrost app upgrade` command is itself the CLI install,
        // so the existing upgrade engine should update the current executable.
        handle_upgrade(true)?;
        return Ok(());
    }

    if let Some(cli_path) = find_standalone_cli_install() {
        println!(
            "{} {}",
            "Upgrading installed CLI:".bright_cyan(),
            cli_path.display()
        );
        let status = run_desktop_managed_cli_upgrade(
            &cli_path,
            target_version,
            progress_source,
            DESKTOP_MANAGED_CLI_TIMEOUT,
        )?;
        if !status.success() {
            return Err(BifrostError::Config(format!(
                "installed CLI upgrade exited with status {status}"
            )));
        }
        verify_installed_cli_target_version(&cli_path, target_version)?;
    } else {
        println!(
            "{}",
            "No standalone CLI install found; desktop app package will update the bundled backend."
                .dimmed()
        );
    }

    Ok(())
}

fn desktop_managed_cli_upgrade_command(cli_path: &Path, target_version: &str) -> Command {
    let mut command = Command::new(cli_path);
    command
        .arg("upgrade")
        .arg("-y")
        .env(DESKTOP_MANAGED_SKIP_APP_ENV, "1")
        .env(DESKTOP_MANAGED_SKIP_RESTART_ENV, "1")
        .env(DESKTOP_MANAGED_TARGET_ENV, target_version)
        .stdin(Stdio::null());
    command
}

fn run_desktop_managed_cli_upgrade(
    cli_path: &Path,
    target_version: &str,
    progress_source: &str,
    timeout: Duration,
) -> Result<std::process::ExitStatus, BifrostError> {
    let mut child = desktop_managed_cli_upgrade_command(cli_path, target_version)
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BifrostError::Config(format!(
                    "installed CLI upgrade timed out after {} seconds",
                    timeout.as_secs()
                )));
            }
            Ok(None) => {
                if Instant::now() >= next_heartbeat {
                    write_app_progress(
                        UpgradePhase::Installing,
                        "Upgrading installed CLI…",
                        Some(target_version.to_string()),
                        progress_source,
                        None,
                        None,
                    );
                    next_heartbeat = Instant::now() + DESKTOP_MANAGED_CLI_HEARTBEAT;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    }
}

fn verify_installed_cli_target_version(
    cli_path: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
    verify_installed_cli_target_version_with_timeout(
        cli_path,
        target_version,
        DESKTOP_MANAGED_CLI_VERSION_TIMEOUT,
    )
}

fn verify_installed_cli_target_version_with_timeout(
    cli_path: &Path,
    target_version: &str,
    timeout: Duration,
) -> Result<(), BifrostError> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "version probe did not run".to_string();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match read_installed_cli_version_with_timeout(cli_path, remaining) {
            Ok(output) => {
                if output
                    .split_whitespace()
                    .any(|part| versions_equal(part, target_version))
                {
                    return Ok(());
                }
                last_observation = format!("reports `{}`", output.trim());
            }
            Err(error) => {
                last_observation = error.to_string();
            }
        }
        if Instant::now() < deadline {
            thread::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    Err(BifrostError::Config(format!(
        "installed CLI upgrade reported success but {} {} instead of target v{} after waiting {} seconds",
        cli_path.display(),
        last_observation,
        normalize_version(target_version),
        timeout.as_secs_f64()
    )))
}

fn read_installed_cli_version_with_timeout(
    cli_path: &Path,
    timeout: Duration,
) -> Result<String, BifrostError> {
    let mut stdout =
        tempfile::tempfile().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
    let mut child = Command::new(cli_path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().map_err(|error| {
            BifrostError::Io(std::io::Error::other(error))
        })?))
        .stderr(Stdio::null())
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BifrostError::Config(format!(
                    "installed CLI version verification timed out after {} seconds",
                    timeout.as_secs()
                )));
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => return Err(BifrostError::Io(error)),
        }
    };
    if !status.success() {
        return Err(BifrostError::Config(format!(
            "installed CLI version verification exited with status {status}"
        )));
    }
    let _ = stdout.seek(SeekFrom::Start(0));
    let mut output = String::new();
    let _ = stdout.read_to_string(&mut output);
    Ok(output)
}

fn find_standalone_cli_install() -> Option<PathBuf> {
    let current_exe = env::current_exe()
        .ok()
        .map(|path| fs::canonicalize(&path).unwrap_or(path));
    let mut candidates = Vec::new();

    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            candidates.push(dir.join(cli_binary_name()));
        }
    }

    if let Some(dir) = env::var_os("BIFROST_INSTALL_DIR") {
        candidates.push(PathBuf::from(dir).join(cli_binary_name()));
    }

    #[cfg(unix)]
    {
        if let Some(home) = env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join(".local/bin/bifrost"));
            candidates.push(home.join(".bifrost/bin/bifrost"));
            candidates.push(home.join(".cargo/bin/bifrost"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin/bifrost"));
        candidates.push(PathBuf::from("/usr/local/bin/bifrost"));
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local_app_data).join("bifrost/bin/bifrost.exe"));
        }
        if let Some(user_profile) = env::var_os("USERPROFILE") {
            candidates.push(PathBuf::from(user_profile).join(".local/bin/bifrost.exe"));
        }
    }

    candidates.into_iter().find(|candidate| {
        if !candidate.is_file() {
            return false;
        }
        let canonical = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone());
        current_exe
            .as_ref()
            .map(|current| &canonical != current)
            .unwrap_or(true)
    })
}

fn cli_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "bifrost.exe"
    }
    #[cfg(not(windows))]
    {
        "bifrost"
    }
}

fn skip_desktop_restart() -> bool {
    env::var("BIFROST_APP_SKIP_RESTART")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn release_asset_name(version: &str, target: DesktopTarget) -> String {
    format!(
        "bifrost-desktop-v{}-{}.{}",
        version,
        target.as_str(),
        target.package_ext()
    )
}

fn release_asset_url(version: &str) -> Result<String, BifrostError> {
    let target = DesktopTarget::current().ok_or_else(|| {
        BifrostError::Config(
            "desktop app update is supported on macOS and Windows only".to_string(),
        )
    })?;
    let tag = make_release_tag(version);
    Ok(format!(
        "{}/{}/{}",
        GITHUB_RELEASE_DOWNLOAD_URL,
        tag,
        release_asset_name(version, target)
    ))
}

fn download_desktop_package(version: &str, progress_source: &str) -> Result<PathBuf, BifrostError> {
    if let Some(path) = env::var_os("BIFROST_APP_UPGRADE_TEST_PACKAGE") {
        return Ok(PathBuf::from(path));
    }

    let url = release_asset_url(version)?;
    let target = DesktopTarget::current().ok_or_else(|| {
        BifrostError::Config(
            "desktop app update is supported on macOS and Windows only".to_string(),
        )
    })?;
    let package_name = release_asset_name(version, target);
    let package_path = env::temp_dir().join(format!(
        "bifrost-desktop-upgrade-{}-{}",
        std::process::id(),
        package_name
    ));

    println!(
        "{} {}",
        "Downloading desktop app:".bright_cyan(),
        url.dimmed()
    );
    write_app_progress(
        UpgradePhase::Downloading,
        "Downloading desktop app…",
        Some(version.to_string()),
        progress_source,
        Some(0.0),
        None,
    );
    let mut response = bifrost_core::github_blocking_reqwest_client_builder()
        .build()
        .map_err(|error| BifrostError::Network(format!("failed to build HTTP client: {error}")))?
        .get(&url)
        .send()
        .map_err(|error| BifrostError::Network(format!("failed to download {url}: {error}")))?;
    if !response.status().is_success() {
        return Err(BifrostError::Network(format!(
            "failed to download {url}: HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    let mut file = fs::File::create(&package_path)?;
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let started = Instant::now();

    loop {
        let read = response.read(&mut buffer).map_err(BifrostError::Io)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(BifrostError::Io)?;
        downloaded += read as u64;

        if last_report.elapsed() >= Duration::from_millis(250) {
            let percent = total
                .filter(|total| *total > 0)
                .map(|total| ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0));
            write_app_progress(
                UpgradePhase::Downloading,
                download_progress_line(downloaded, total, started),
                Some(version.to_string()),
                progress_source,
                percent,
                None,
            );
            last_report = Instant::now();
        }
    }
    file.flush().map_err(BifrostError::Io)?;

    if downloaded == 0 {
        return Err(BifrostError::Network(format!(
            "failed to download {url}: empty response"
        )));
    }

    write_app_progress(
        UpgradePhase::Downloading,
        "Desktop app downloaded",
        Some(version.to_string()),
        progress_source,
        Some(100.0),
        None,
    );
    Ok(package_path)
}

#[cfg(test)]
mod tests;
