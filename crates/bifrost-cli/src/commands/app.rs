use std::env;
#[cfg(any(target_os = "windows", test))]
use std::ffi::OsString;
use std::fs;
#[cfg(target_os = "windows")]
use std::io::Cursor;
use std::io::{self, Read, Seek, SeekFrom, Write};
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
    download_progress_line, handle_app_managed_upgrade, DESKTOP_MANAGED_SKIP_APP_ENV,
    DESKTOP_MANAGED_SKIP_RESTART_ENV, DESKTOP_MANAGED_TARGET_ENV, DESKTOP_UPGRADE_HANDOFF_ENV,
};
#[cfg(test)]
use super::upgrade_background::{PARENT_UPGRADE_LOCK_OWNER_PID_ENV, PARENT_UPGRADE_LOCK_TOKEN_ENV};
mod installer;
pub(crate) use installer::desktop_pending_install_guard_is_active;
use installer::*;
mod version;
pub(crate) use version::installed_desktop_app_is_target_version;
#[cfg(test)]
use version::installed_desktop_app_version;
use version::{normalize_version, verify_installed_desktop_target_version, versions_equal};

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
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const DESKTOP_INSTALL_TERMINAL_HEARTBEAT: Duration = Duration::from_secs(5);
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
    let desktop_handoff_managed = desktop_upgrade_handoff_managed(&progress_source);
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

    let _upgrade_lock = acquire_top_level_app_upgrade_lock(&progress_source, &target_version)?;

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
    #[cfg(target_os = "windows")]
    let package_owned_by_updater = package.is_none();
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
    println!("{}", "Installing desktop app...".bright_cyan());
    #[cfg(target_os = "windows")]
    if should_defer_current_desktop_install(&progress_source, &package_path) {
        defer_desktop_install_to_handoff(&package_path, &target_version, package_owned_by_updater)
            .inspect_err(|error| {
                write_app_failed_progress(&target_version, &progress_source, error);
            })?;
        write_app_progress(
            UpgradePhase::Restarting,
            "Waiting for desktop shell to stop before installing…",
            Some(target_version.clone()),
            &progress_source,
            None,
            None,
        );
        println!(
            "{}",
            "✓ Desktop installer is ready; the desktop restart handoff will apply it."
                .bright_green()
                .bold()
        );
        return Ok(());
    }
    install_desktop_package_verified(
        &package_path,
        &install_dir,
        &install_path,
        &target_version,
        &progress_source,
    )
    .inspect_err(|error| {
        write_app_failed_progress(&target_version, &progress_source, error);
    })?;
    println!("{}", "✓ Desktop app installed successfully.".bright_green());

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
    println!(
        "{}",
        if progress_source == "desktop" {
            "Waiting for desktop shell to restart..."
        } else {
            "Restarting desktop app..."
        }
        .bright_cyan()
    );
    if desktop_handoff_managed {
        println!(
            "{}",
            "✓ Desktop update installed; waiting for the desktop restart handoff."
                .bright_green()
                .bold()
        );
        return Ok(());
    }
    if !skip_desktop_restart() {
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

fn upgrade_cli_if_present(progress_source: &str, target_version: &str) -> Result<(), BifrostError> {
    println!(
        "{}",
        "Checking CLI install before desktop app install...".bright_cyan()
    );

    if progress_source != "desktop" {
        // The visible `bifrost app upgrade` command is itself the CLI install,
        // so the existing upgrade engine should update the current executable.
        handle_app_managed_upgrade(target_version.to_string())?;
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
        .envs(
            crate::commands::upgrade_background::parent_upgrade_lock_child_environment(&data_dir()),
        )
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

fn uninstall_app(app_dir: Option<PathBuf>, dry_run: bool, _yes: bool) -> Result<(), BifrostError> {
    let install_dir = resolve_app_dir(app_dir)?;
    let install_path = resolve_app_path(&install_dir);
    println!(
        "{} {}",
        "Desktop app path:".bright_cyan(),
        install_path.display()
    );

    if dry_run {
        println!(
            "{}",
            "Dry run: would remove the desktop app only.".bright_yellow()
        );
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let uninstaller = install_dir.join("uninstall.exe");
        if uninstaller.exists() {
            let status = Command::new(&uninstaller)
                .arg("/S")
                .stdin(Stdio::null())
                .status()
                .map_err(BifrostError::Io)?;
            if !status.success() {
                return Err(BifrostError::Config(format!(
                    "desktop uninstaller exited with status {status}"
                )));
            }
            println!("{}", "✓ Desktop app uninstalled.".bright_green());
            return Ok(());
        }

        if let Some(product_code) = find_windows_msi_product_code_for_install_dir(&install_dir) {
            run_windows_msi_uninstall(&product_code)?;
            println!("{}", "✓ Desktop app uninstalled.".bright_green());
            return Ok(());
        }
    }

    if install_path.exists() {
        if install_path.is_dir() {
            fs::remove_dir_all(&install_path)?;
        } else {
            fs::remove_file(&install_path)?;
        }
    }
    println!("{}", "✓ Desktop app uninstalled.".bright_green());
    Ok(())
}

fn resolve_target_version(version: Option<String>) -> Result<String, BifrostError> {
    if let Some(version) = version {
        return Ok(version.trim_start_matches('v').to_string());
    }
    get_latest_version_fresh_with_diagnostics()
        .map(|cache| cache.latest_version)
        .map_err(|diagnostic| {
            BifrostError::Network(format!(
                "failed to resolve latest desktop app version: {diagnostic}"
            ))
        })
}

fn resolve_app_dir(app_dir: Option<PathBuf>) -> Result<PathBuf, BifrostError> {
    if let Some(dir) = app_dir {
        return Ok(dir);
    }
    if let Some(dir) = env::var_os("BIFROST_APP_INSTALL_DIR") {
        return Ok(PathBuf::from(dir));
    }

    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from("/Applications"))
    }
    #[cfg(target_os = "windows")]
    {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| BifrostError::Config("LOCALAPPDATA is not set".to_string()))?;
        Ok(local_app_data.join(WINDOWS_APP_NAME))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(BifrostError::Config(
            "desktop app install is supported on macOS and Windows only".to_string(),
        ))
    }
}

fn resolve_app_dir_for_source(
    app_dir: Option<PathBuf>,
    progress_source: &str,
) -> Result<PathBuf, BifrostError> {
    if app_dir.is_some() || progress_source != "desktop" {
        return resolve_app_dir(app_dir);
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(app_dir) = macos_app_dir_from_exe_path(&current_exe) {
            return Ok(app_dir);
        }
    }

    resolve_app_dir(None)
}

fn macos_app_dir_from_exe_path(exe_path: &Path) -> Option<PathBuf> {
    let app_bundle = exe_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(MACOS_APP_BUNDLE))?;
    app_bundle.parent().map(Path::to_path_buf)
}

fn resolve_app_path(app_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        app_dir.join(MACOS_APP_BUNDLE)
    }
    #[cfg(target_os = "windows")]
    {
        app_dir.join(WINDOWS_APP_EXE)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        app_dir.join("Bifrost")
    }
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
            let progress_line = download_progress_line(downloaded, total, started);
            let _ = write_terminal_download_progress(&progress_line, false, &mut io::stdout());
            write_app_progress(
                UpgradePhase::Downloading,
                progress_line,
                Some(version.to_string()),
                progress_source,
                percent,
                None,
            );
            last_report = Instant::now();
        }
    }
    file.flush().map_err(BifrostError::Io)?;

    let progress_line = download_progress_line(downloaded, total, started);
    let _ = write_terminal_download_progress(&progress_line, true, &mut io::stdout());

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

fn write_terminal_download_progress(
    progress_line: &str,
    finish_line: bool,
    output: &mut impl Write,
) -> io::Result<()> {
    if finish_line {
        writeln!(output, "\r{progress_line}")?;
    } else {
        write!(output, "\r{progress_line}")?;
    }
    output.flush()
}

fn install_desktop_package(
    package: &Path,
    install_dir: &Path,
    install_path: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    fs::create_dir_all(install_dir)?;

    if package.is_dir()
        && package.file_name().and_then(|name| name.to_str()) == Some(MACOS_APP_BUNDLE)
    {
        copy_dir_replace(package, install_path, target_version, progress_source)?;
        clear_macos_xattrs(install_path);
        return Ok(());
    }

    let extension = package
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "dmg" => install_macos_dmg(package, install_path, target_version, progress_source),
        "exe" => run_windows_installer(package, &["/S"], target_version, progress_source),
        "msi" => run_windows_msi(package, target_version, progress_source),
        "zip" => install_windows_zip(package, install_path),
        _ => Err(BifrostError::Config(format!(
            "unsupported desktop package type: {}",
            package.display()
        ))),
    }
}

fn copy_dir_replace(
    source: &Path,
    target: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    if fs::canonicalize(source).ok() == fs::canonicalize(target).ok() && target.exists() {
        return verify_installed_desktop_target_version(target, target_version);
    }

    let parent = target.parent().ok_or_else(|| {
        BifrostError::Config(format!(
            "desktop app target has no parent: {}",
            target.display()
        ))
    })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Bifrost.app");
    let staging = parent.join(format!(".{name}.upgrade-{}", std::process::id()));
    // The backup name is deliberately stable across updater PIDs. A new
    // process must be able to recover a bundle left between target -> backup
    // and staging -> target by an interrupted predecessor.
    let backup = parent.join(format!(".{name}.backup"));

    // Recover the only known-good bundle if a previous process was interrupted
    // in the narrow rename window after target -> backup but before staging ->
    // target. Deleting this backup on the next attempt would turn a recoverable
    // interrupted update into a missing application.
    if !target.exists() && backup.exists() {
        fs::rename(&backup, target)?;
    }
    let _ = fs::remove_dir_all(&staging);
    if target.exists() {
        let _ = fs::remove_dir_all(&backup);
    }

    #[cfg(target_os = "macos")]
    let copied = {
        let mut command = Command::new("ditto");
        command.arg(source).arg(&staging);
        run_desktop_install_command(command, target_version, progress_source)?.success()
    };
    #[cfg(not(target_os = "macos"))]
    let copied = {
        let _ = progress_source;
        false
    };
    if !copied {
        let _ = fs::remove_dir_all(&staging);
        copy_dir_recursive(source, &staging)?;
    }
    verify_installed_desktop_target_version(&staging, target_version).inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;

    if target.exists() {
        fs::rename(target, &backup)?;
    }
    if let Err(error) = fs::rename(&staging, target) {
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(BifrostError::Io(error));
    }
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), BifrostError> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos_dmg(
    package: &Path,
    install_path: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let mut command = Command::new("hdiutil");
    command
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(package);
    let output = run_desktop_install_command_output(command, target_version, progress_source)?;
    if !output.status.success() {
        return Err(BifrostError::Config(format!(
            "failed to attach dmg: {}",
            output.stderr.trim()
        )));
    }
    let stdout = output.stdout;
    let mount = stdout
        .lines()
        .filter_map(|line| line.split('\t').next_back())
        .map(str::trim)
        .find(|part| part.starts_with("/Volumes/"))
        .map(PathBuf::from)
        .ok_or_else(|| BifrostError::Parse("failed to find mounted dmg volume".to_string()))?;
    let source_app = mount.join(MACOS_APP_BUNDLE);
    let result = copy_dir_replace(&source_app, install_path, target_version, progress_source);
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).status();
    result?;
    clear_macos_xattrs(install_path);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_macos_dmg(
    _package: &Path,
    _install_path: &Path,
    _target_version: &str,
    _progress_source: &str,
) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "dmg desktop packages can only be installed on macOS".to_string(),
    ))
}

#[cfg(target_os = "macos")]
fn clear_macos_xattrs(path: &Path) {
    let _ = Command::new("xattr")
        .args(["-cr"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("xattr")
        .args(["-d", "com.apple.provenance"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(target_os = "macos"))]
fn clear_macos_xattrs(_path: &Path) {}

#[cfg(target_os = "windows")]
fn run_windows_installer(
    package: &Path,
    args: &[&str],
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let mut command = Command::new(package);
    command.args(args);
    let status = run_desktop_install_command(command, target_version, progress_source)?;
    if status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "desktop installer exited with status {status}"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn run_windows_installer(
    _package: &Path,
    _args: &[&str],
    _target_version: &str,
    _progress_source: &str,
) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "Windows desktop packages can only be installed on Windows".to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn run_windows_msi(
    package: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let log_path = windows_msi_log_path(package);
    let args = windows_msi_install_args(package, &log_path);
    let mut command = Command::new("msiexec");
    command.args(&args);
    let status = run_desktop_install_command(command, target_version, progress_source)?;
    if status.success() {
        let _ = fs::remove_file(&log_path);
        Ok(())
    } else {
        let log_summary = read_windows_msi_log_summary(&log_path);
        Err(BifrostError::Config(format!(
            "msiexec exited with status {status}; log: {}{}",
            log_path.display(),
            log_summary
                .map(|summary| format!("; {summary}"))
                .unwrap_or_default()
        )))
    }
}

#[cfg(target_os = "windows")]
fn run_windows_msi_uninstall(product_code: &str) -> Result<(), BifrostError> {
    let log_path = env::temp_dir().join(format!(
        "bifrost-desktop-msi-uninstall-{}-{}.log",
        std::process::id(),
        product_code.trim_matches(|ch| ch == '{' || ch == '}')
    ));
    let args = windows_msi_uninstall_args(product_code, &log_path);
    let status = Command::new("msiexec")
        .args(&args)
        .stdin(Stdio::null())
        .status()
        .map_err(BifrostError::Io)?;
    if status.success() {
        let _ = fs::remove_file(&log_path);
        Ok(())
    } else {
        let log_summary = read_windows_msi_log_summary(&log_path);
        Err(BifrostError::Config(format!(
            "msiexec uninstall exited with status {status}; log: {}{}",
            log_path.display(),
            log_summary
                .map(|summary| format!("; {summary}"))
                .unwrap_or_default()
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn run_windows_msi(
    _package: &Path,
    _target_version: &str,
    _progress_source: &str,
) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "MSI desktop packages can only be installed on Windows".to_string(),
    ))
}

#[cfg(any(target_os = "windows", test))]
fn windows_msi_install_args(package: &Path, log_path: &Path) -> Vec<OsString> {
    [
        OsString::from("/i"),
        package.as_os_str().to_os_string(),
        OsString::from("/qn"),
        OsString::from("/norestart"),
        // Tauri's WiX bundle sets ALLUSERS=1 by default, which makes silent
        // installs require elevation. The CLI installs into the current user's
        // LocalAppData path, so force MSI into a per-user install context.
        OsString::from("ALLUSERS=2"),
        OsString::from("MSIINSTALLPERUSER=1"),
        OsString::from("/l*v"),
        log_path.as_os_str().to_os_string(),
    ]
    .into()
}

#[cfg(any(target_os = "windows", test))]
fn windows_msi_uninstall_args(product_code: &str, log_path: &Path) -> Vec<OsString> {
    [
        OsString::from("/x"),
        OsString::from(product_code),
        OsString::from("/qn"),
        OsString::from("/norestart"),
        OsString::from("ALLUSERS=2"),
        OsString::from("MSIINSTALLPERUSER=1"),
        OsString::from("/l*v"),
        log_path.as_os_str().to_os_string(),
    ]
    .into()
}

#[cfg(any(target_os = "windows", test))]
fn windows_msi_log_path(package: &Path) -> PathBuf {
    let package_name = package
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("desktop")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    env::temp_dir().join(format!(
        "bifrost-desktop-msi-{}-{package_name}.log",
        std::process::id()
    ))
}

#[cfg(any(target_os = "windows", test))]
fn read_windows_msi_log_summary(log_path: &Path) -> Option<String> {
    let contents = fs::read_to_string(log_path).ok()?;
    let interesting = contents
        .lines()
        .rev()
        .find(|line| {
            line.contains("Error ")
                || line.contains("Return value 3")
                || line.contains("Installation failed")
                || line.contains("Product: Bifrost")
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())?;
    Some(format!("MSI detail: {interesting}"))
}

#[cfg(target_os = "windows")]
fn find_windows_msi_product_code_for_install_dir(install_dir: &Path) -> Option<String> {
    const UNINSTALL_HIVES: [&str; 2] = [
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    let expected_install_dir = normalize_windows_path_for_compare(install_dir);
    for hive in UNINSTALL_HIVES {
        let output = Command::new("reg")
            .args(["query", hive, "/s"])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(product_code) =
            parse_windows_msi_product_code_for_install_dir(&stdout, &expected_install_dir)
        {
            return Some(product_code);
        }
    }
    None
}

#[cfg(any(target_os = "windows", test))]
fn parse_windows_msi_product_code_for_install_dir(
    reg_output: &str,
    expected_install_dir: &str,
) -> Option<String> {
    let mut display_name = None;
    let mut uninstall_string = None;
    let mut install_location = None;

    for line in reg_output.lines().chain(std::iter::once("")) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("HKEY_") {
            if display_name.as_deref() == Some(WINDOWS_APP_NAME)
                && install_location
                    .as_deref()
                    .map(normalize_windows_path_for_compare_str)
                    .as_deref()
                    == Some(expected_install_dir)
            {
                if let Some(product_code) = uninstall_string
                    .as_deref()
                    .and_then(extract_msi_product_code)
                {
                    return Some(product_code);
                }
            }
            display_name = None;
            uninstall_string = None;
            install_location = None;
            continue;
        }

        if let Some(value) = parse_reg_value(trimmed, "DisplayName") {
            display_name = Some(value.to_string());
        } else if let Some(value) = parse_reg_value(trimmed, "UninstallString") {
            uninstall_string = Some(value.to_string());
        } else if let Some(value) = parse_reg_value(trimmed, "InstallLocation") {
            install_location = Some(value.to_string());
        }
    }

    None
}

#[cfg(any(target_os = "windows", test))]
fn parse_reg_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix("REG_SZ")?.trim_start();
    Some(rest.trim())
}

#[cfg(any(target_os = "windows", test))]
fn extract_msi_product_code(uninstall_string: &str) -> Option<String> {
    let start = uninstall_string.find('{')?;
    let end = uninstall_string[start..].find('}')? + start;
    let product_code = &uninstall_string[start..=end];
    if product_code.len() == 38 {
        Some(product_code.to_string())
    } else {
        None
    }
}

#[cfg(any(target_os = "windows", test))]
fn normalize_windows_path_for_compare(path: &Path) -> String {
    normalize_windows_path_for_compare_str(&path.display().to_string())
}

#[cfg(any(target_os = "windows", test))]
fn normalize_windows_path_for_compare_str(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(target_os = "windows")]
fn install_windows_zip(package: &Path, install_path: &Path) -> Result<(), BifrostError> {
    let bytes = fs::read(package)?;
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| BifrostError::Parse(format!("failed to open zip: {error}")))?;
    let temp_dir =
        tempfile::tempdir().map_err(|error| BifrostError::Io(io::Error::other(error)))?;
    archive
        .extract(temp_dir.path())
        .map_err(|error| BifrostError::Parse(format!("failed to extract zip: {error}")))?;
    let source = find_file_named(temp_dir.path(), WINDOWS_APP_EXE)
        .or_else(|| find_file_named(temp_dir.path(), WINDOWS_LEGACY_APP_EXE))
        .ok_or_else(|| {
            BifrostError::NotFound(format!(
                "{WINDOWS_APP_EXE} not found in {}",
                package.display()
            ))
        })?;
    if let Some(parent) = install_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, install_path)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn install_windows_zip(_package: &Path, _install_path: &Path) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "zip desktop packages are only supported for Windows installs".to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn find_file_named(root: &Path, file_name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_named(&path, file_name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            return Some(path);
        }
    }
    None
}

pub(crate) fn restart_desktop_app(install_path: &Path) -> Result<(), BifrostError> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(install_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(BifrostError::Io)?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new(install_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(BifrostError::Io)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = install_path;
    }
    Ok(())
}

fn write_app_progress(
    phase: UpgradePhase,
    message: impl Into<String>,
    target_version: Option<String>,
    source: &str,
    percent: Option<f64>,
    error: Option<String>,
) {
    if !should_write_app_progress(source) {
        return;
    }
    let mut progress = UpgradeProgress::new(phase, message)
        .with_target(target_version)
        .with_source(Some(source.to_string()));
    if let Some(percent) = percent {
        progress = progress.with_percent(Some(percent));
    }
    if let Some(error) = error {
        progress = progress.with_error(Some(error));
    }
    write_progress(&data_dir(), &progress);
}

#[cfg(test)]
mod tests;
