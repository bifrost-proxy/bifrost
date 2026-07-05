use std::env;
use std::fs;
#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};
use bifrost_core::version_check::make_release_tag;
use bifrost_core::BifrostError;
use bifrost_storage::data_dir;
use colored::Colorize;

use crate::cli::AppCommands;

use super::update_check::get_latest_version_fresh_with_diagnostics;
use super::upgrade::handle_upgrade;

#[cfg(not(target_os = "macos"))]
const APP_NAME: &str = "Bifrost";
const MACOS_APP_BUNDLE: &str = "Bifrost.app";
#[cfg(target_os = "windows")]
const WINDOWS_APP_EXE: &str = "Bifrost.exe";
const GITHUB_RELEASE_DOWNLOAD_URL: &str =
    "https://github.com/bifrost-proxy/bifrost/releases/download";

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
    let install_dir = resolve_app_dir(app_dir)?;
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
        upgrade_cli_if_present(&progress_source)?;
    }

    let package_path = match package {
        Some(path) => path,
        None => download_desktop_package(&target_version, &progress_source)?,
    };

    write_app_progress(
        UpgradePhase::Installing,
        "Installing desktop app…",
        Some(target_version.clone()),
        &progress_source,
        None,
        None,
    );
    install_desktop_package(&package_path, &install_dir, &install_path)?;

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
        restart_desktop_app(&install_path)?;
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

fn upgrade_cli_if_present(progress_source: &str) -> Result<(), BifrostError> {
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
        let status = Command::new(&cli_path)
            .arg("upgrade")
            .arg("-y")
            .stdin(Stdio::null())
            .status()
            .map_err(BifrostError::Io)?;
        if !status.success() {
            return Err(BifrostError::Config(format!(
                "installed CLI upgrade exited with status {status}"
            )));
        }
    } else {
        println!(
            "{}",
            "No standalone CLI install found; desktop app package will update the bundled backend."
                .dimmed()
        );
    }

    Ok(())
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
        Ok(local_app_data.join("Programs").join(APP_NAME))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(BifrostError::Config(
            "desktop app install is supported on macOS and Windows only".to_string(),
        ))
    }
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
        app_dir.join(APP_NAME)
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
    let response = bifrost_core::github_blocking_reqwest_client_builder()
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
    let bytes = response
        .bytes()
        .map_err(|error| BifrostError::Network(format!("failed to read download body: {error}")))?;
    fs::write(&package_path, &bytes)?;
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

fn install_desktop_package(
    package: &Path,
    install_dir: &Path,
    install_path: &Path,
) -> Result<(), BifrostError> {
    fs::create_dir_all(install_dir)?;

    if package.is_dir()
        && package.file_name().and_then(|name| name.to_str()) == Some(MACOS_APP_BUNDLE)
    {
        copy_dir_replace(package, install_path)?;
        clear_macos_xattrs(install_path);
        return Ok(());
    }

    let extension = package
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "dmg" => install_macos_dmg(package, install_path),
        "exe" => run_windows_installer(package, &["/S"]),
        "msi" => run_windows_msi(package),
        "zip" => install_windows_zip(package, install_path),
        _ => Err(BifrostError::Config(format!(
            "unsupported desktop package type: {}",
            package.display()
        ))),
    }
}

fn copy_dir_replace(source: &Path, target: &Path) -> Result<(), BifrostError> {
    let _ = fs::remove_dir_all(target);
    #[cfg(target_os = "macos")]
    if Command::new("ditto")
        .arg(source)
        .arg(target)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
    {
        return Ok(());
    }

    copy_dir_recursive(source, target)
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
fn install_macos_dmg(package: &Path, install_path: &Path) -> Result<(), BifrostError> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(package)
        .output()
        .map_err(BifrostError::Io)?;
    if !output.status.success() {
        return Err(BifrostError::Config(format!(
            "failed to attach dmg: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount = stdout
        .lines()
        .filter_map(|line| line.split('\t').next_back())
        .map(str::trim)
        .find(|part| part.starts_with("/Volumes/"))
        .map(PathBuf::from)
        .ok_or_else(|| BifrostError::Parse("failed to find mounted dmg volume".to_string()))?;
    let source_app = mount.join(MACOS_APP_BUNDLE);
    let result = copy_dir_replace(&source_app, install_path);
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).status();
    result?;
    clear_macos_xattrs(install_path);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_macos_dmg(_package: &Path, _install_path: &Path) -> Result<(), BifrostError> {
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
fn run_windows_installer(package: &Path, args: &[&str]) -> Result<(), BifrostError> {
    let status = Command::new(package)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .map_err(BifrostError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "desktop installer exited with status {status}"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn run_windows_installer(_package: &Path, _args: &[&str]) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "Windows desktop packages can only be installed on Windows".to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn run_windows_msi(package: &Path) -> Result<(), BifrostError> {
    let status = Command::new("msiexec")
        .arg("/i")
        .arg(package)
        .args(["/qn", "/norestart"])
        .stdin(Stdio::null())
        .status()
        .map_err(BifrostError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "msiexec exited with status {status}"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn run_windows_msi(_package: &Path) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "MSI desktop packages can only be installed on Windows".to_string(),
    ))
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
    let source = find_file_named(temp_dir.path(), WINDOWS_APP_EXE).ok_or_else(|| {
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

fn restart_desktop_app(install_path: &Path) -> Result<(), BifrostError> {
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
mod tests {
    use super::*;

    #[test]
    fn release_asset_name_uses_desktop_prefix_and_target() {
        assert_eq!(
            release_asset_name("0.0.138", DesktopTarget::MacosAarch64),
            "bifrost-desktop-v0.0.138-aarch64-apple-darwin.dmg"
        );
        assert_eq!(
            release_asset_name("0.0.138", DesktopTarget::WindowsX64),
            "bifrost-desktop-v0.0.138-x86_64-pc-windows-msvc.msi"
        );
    }

    #[test]
    fn macos_app_path_is_bundle_under_install_dir() {
        let dir = PathBuf::from("/Applications");
        let path = resolve_app_path(&dir);
        if cfg!(target_os = "macos") {
            assert_eq!(path, PathBuf::from("/Applications/Bifrost.app"));
        }
    }
}
