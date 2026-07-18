use bifrost_core::BifrostError;
use colored::Colorize;
use std::env;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use super::update_check::{get_latest_version, get_latest_version_fresh_with_diagnostics};
use crate::config::get_bifrost_dir;
use crate::process::{
    capture_runtime_system_proxy_snapshot, find_process_on_port, is_process_running, read_pid,
    read_runtime_info, write_runtime_info, RuntimeInfo, RuntimeStartMode,
    RuntimeSystemProxySnapshot,
};
use bifrost_core::version_check::{
    is_newer_version, make_release_tag, VersionCache, GITHUB_RELEASE_URL,
};
use bifrost_storage::ConfigManager;
const GITHUB_BASE_URL: &str = "https://github.com";
const DEFAULT_GITHUB_MIRROR_URLS: &[&str] = &[
    "https://github.com",
    "https://ghfast.top/https://github.com",
    "https://github.moeyy.xyz/https://github.com",
];
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 10;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
const MIRROR_PROBE_TIMEOUT_SECS: u64 = 5;
const DOWNLOAD_TRIES: usize = 2;
const UPGRADE_RESTART_PORT_RELEASE_TIMEOUT_SECS: u64 = 30;
const BINARY_VERIFY_TIMEOUT_SECS: u64 = 15;
const POST_UPGRADE_SKILL_INSTALL_TIMEOUT_SECS: u64 = 120;
const POST_UPGRADE_SKILL_INSTALL_ARGS: &[&str] = &["install-skill", "--tool", "all", "-y"];
const POST_UPGRADE_APP_UPDATE_TIMEOUT_SECS: u64 = 600;
const UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS: u64 = 30;
const HOMEBREW_COMMAND_TIMEOUT_SECS: u64 = 600;
const HOMEBREW_METADATA_TIMEOUT_SECS: u64 = 60;
pub(crate) const DESKTOP_MANAGED_SKIP_APP_ENV: &str = "BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP";
pub(crate) const DESKTOP_MANAGED_SKIP_RESTART_ENV: &str =
    "BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART";
pub(crate) const DESKTOP_MANAGED_TARGET_ENV: &str =
    "BIFROST_DESKTOP_MANAGED_UPGRADE_TARGET_VERSION";
static DEFERRED_INSTALL_SCHEDULED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimedCommandStatus {
    Success,
    Failure,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartSystemProxyConfig {
    enabled: bool,
    bypass: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunningProxyHint {
    pid: u32,
    port: u16,
}

impl RunningProxyHint {
    pub(crate) fn from_parts(pid: Option<u32>, port: Option<u16>) -> Option<Self> {
        pid.zip(port).map(|(pid, port)| Self { pid, port })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UpgradeBehavior {
    restart_if_already_latest: bool,
    update_desktop_app: bool,
    require_desktop_app_update: bool,
    restart_proxy: bool,
}

impl UpgradeBehavior {
    fn background() -> Self {
        Self {
            restart_if_already_latest: true,
            update_desktop_app: true,
            require_desktop_app_update: true,
            restart_proxy: true,
        }
    }

    fn interactive(skip_app: bool, skip_restart: bool) -> Self {
        Self {
            restart_if_already_latest: false,
            update_desktop_app: !skip_app,
            require_desktop_app_update: false,
            restart_proxy: !skip_restart,
        }
    }
}

#[derive(Debug)]
struct TimedCommandOutput {
    status: TimedCommandStatus,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, Copy)]
enum RestartArgsSource<'a> {
    Runtime(&'a RuntimeInfo),
    DefaultConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpgradeInstallOutcome {
    Installed,
    #[cfg(windows)]
    DeferredWindows(WindowsDeferredInstall),
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsDeferredInstall {
    staged_binary: PathBuf,
    target_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadTuning {
    connect_timeout_secs: u64,
    download_timeout_secs: u64,
    mirror_probe_timeout_secs: u64,
    download_tries: usize,
}

pub(crate) fn take_deferred_install_scheduled() -> bool {
    DEFERRED_INSTALL_SCHEDULED.swap(false, Ordering::SeqCst)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn mark_deferred_install_scheduled() {
    DEFERRED_INSTALL_SCHEDULED.store(true, Ordering::SeqCst);
}

impl Default for DownloadTuning {
    fn default() -> Self {
        Self {
            connect_timeout_secs: DOWNLOAD_CONNECT_TIMEOUT_SECS,
            download_timeout_secs: DOWNLOAD_TIMEOUT_SECS,
            mirror_probe_timeout_secs: MIRROR_PROBE_TIMEOUT_SECS,
            download_tries: DOWNLOAD_TRIES,
        }
    }
}

impl DownloadTuning {
    fn from_env() -> Self {
        Self {
            connect_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_CONNECT_TIMEOUT",
                DOWNLOAD_CONNECT_TIMEOUT_SECS,
            ),
            download_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_TIMEOUT",
                DOWNLOAD_TIMEOUT_SECS,
            ),
            mirror_probe_timeout_secs: positive_env_u64(
                "BIFROST_MIRROR_PROBE_TIMEOUT",
                MIRROR_PROBE_TIMEOUT_SECS,
            ),
            download_tries: positive_env_usize("BIFROST_DOWNLOAD_TRIES", DOWNLOAD_TRIES),
        }
    }
}

fn parse_positive_u64(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_positive_usize(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_u64(name: &str, default: u64) -> u64 {
    parse_positive_u64(env::var(name).ok().as_deref(), default)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    parse_positive_usize(env::var(name).ok().as_deref(), default)
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstallMethod {
    Homebrew,
    Script,
    Manual(PathBuf),
    Unknown,
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallMethod::Homebrew => write!(f, "Homebrew"),
            InstallMethod::Script => write!(f, "Install script"),
            InstallMethod::Manual(path) => write!(f, "Manual ({})", path.display()),
            InstallMethod::Unknown => write!(f, "Unknown"),
        }
    }
}

fn detect_install_method() -> InstallMethod {
    let exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(_) => return InstallMethod::Unknown,
    };

    let exe_path_str = exe_path.to_string_lossy();

    if exe_path_str.contains("/opt/homebrew/")
        || exe_path_str.contains("/usr/local/Cellar/")
        || exe_path_str.contains("/home/linuxbrew/")
    {
        return InstallMethod::Homebrew;
    }

    if exe_path_str.contains("/.bifrost/bin/") {
        return InstallMethod::Script;
    }

    InstallMethod::Manual(exe_path)
}

fn restart_executable_for_install_method(method: &InstallMethod) -> Result<PathBuf, BifrostError> {
    match method {
        InstallMethod::Homebrew => env::current_exe()
            .map(|current| homebrew_launcher_for_executable(&current))
            .map_err(BifrostError::Io),
        InstallMethod::Script => env::current_exe().map_err(BifrostError::Io),
        InstallMethod::Manual(path) => Ok(path.clone()),
        InstallMethod::Unknown => Err(BifrostError::Config(
            "Cannot determine restart executable for unknown install method".to_string(),
        )),
    }
}

fn homebrew_launcher_for_executable(current: &Path) -> PathBuf {
    let current_text = current.to_string_lossy();
    if current_text.starts_with("/opt/homebrew/") {
        PathBuf::from("/opt/homebrew/bin/bifrost")
    } else if current_text.starts_with("/usr/local/") {
        PathBuf::from("/usr/local/bin/bifrost")
    } else if current_text.starts_with("/home/linuxbrew/") {
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/bifrost")
    } else {
        // InstallMethod::Homebrew means the newly installed binary must be
        // resolved through Homebrew's stable launcher. Reusing an arbitrary
        // current executable here can restart the old, versioned binary (and
        // is also wrong for wrappers/tests that explicitly select Homebrew).
        PathBuf::from("bifrost")
    }
}

fn get_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("aarch64-apple-darwin")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("x86_64-apple-darwin")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
    {
        Some("x86_64-unknown-linux-musl")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(target_env = "musl")))]
    {
        if should_use_musl_fallback() {
            Some("x86_64-unknown-linux-musl")
        } else {
            Some("x86_64-unknown-linux-gnu")
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
    {
        Some("aarch64-unknown-linux-musl")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", not(target_env = "musl")))]
    {
        if should_use_musl_fallback() {
            Some("aarch64-unknown-linux-musl")
        } else {
            Some("aarch64-unknown-linux-gnu")
        }
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("x86_64-pc-windows-msvc")
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        Some("aarch64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    {
        None
    }
}

#[cfg(any(
    test,
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        not(target_env = "musl")
    )
))]
const MIN_GLIBC_VERSION: (u32, u32) = (2, 39);

#[cfg(any(
    test,
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        not(target_env = "musl")
    )
))]
fn glibc_requires_musl_fallback(version: Option<(u32, u32)>) -> bool {
    match version {
        Some(version) => version < MIN_GLIBC_VERSION,
        None => true,
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    not(target_env = "musl")
))]
fn should_use_musl_fallback() -> bool {
    glibc_requires_musl_fallback(detect_glibc_version())
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    not(target_env = "musl")
))]
fn detect_glibc_version() -> Option<(u32, u32)> {
    let output = Command::new("ldd").arg("--version").output().ok()?;

    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);

    if !text.to_lowercase().contains("glibc") && !text.to_lowercase().contains("gnu libc") {
        return None;
    }

    let first_line = text.lines().next()?;
    let version_str = first_line.split_whitespace().rfind(|word| {
        let parts: Vec<&str> = word.split('.').collect();
        parts.len() == 2 && parts[0].parse::<u32>().is_ok() && parts[1].parse::<u32>().is_ok()
    })?;

    let parts: Vec<&str> = version_str.split('.').collect();
    let major = parts[0].parse::<u32>().ok()?;
    let minor = parts[1].parse::<u32>().ok()?;
    Some((major, minor))
}

#[cfg(target_os = "macos")]
fn clear_quarantine_attr(path: &Path) {
    use tracing::debug;
    for flag in ["-c", "-d com.apple.quarantine", "-d com.apple.provenance"] {
        let args: Vec<&str> = flag.split_whitespace().collect();
        let result = Command::new("xattr").args(&args).arg(path).output();
        match result {
            Ok(output) if !output.status.success() => {
                debug!(
                    flag,
                    path = %path.display(),
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "xattr removal returned non-zero (may be absent, safe to ignore)"
                );
            }
            Err(e) => {
                debug!(
                    flag,
                    path = %path.display(),
                    error = %e,
                    "failed to run xattr command"
                );
            }
            _ => {}
        }
    }
}

fn get_musl_fallback_triple(target: &str) -> Option<String> {
    match target {
        "x86_64-unknown-linux-gnu" => Some("x86_64-unknown-linux-musl".to_string()),
        "aarch64-unknown-linux-gnu" => Some("aarch64-unknown-linux-musl".to_string()),
        _ => None,
    }
}

fn command_status_with_timeout(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<TimedCommandStatus, BifrostError> {
    command_status_with_timeout_and_heartbeat(
        program,
        args,
        timeout,
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
    )
}

fn command_status_with_timeout_and_heartbeat(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    heartbeat: Duration,
) -> Result<TimedCommandStatus, BifrostError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(if status.success() {
                    TimedCommandStatus::Success
                } else {
                    TimedCommandStatus::Failure
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                let pid = child.id();
                let _ = child.kill();
                thread::spawn(move || {
                    let _ = child.wait();
                });
                eprintln!(
                    "Warning: command timed out after {}s and was asked to terminate: {} (pid {}) {}",
                    timeout.as_secs(),
                    program.display(),
                    pid,
                    args.join(" ")
                );
                return Ok(TimedCommandStatus::TimedOut);
            }
            Ok(None) => {
                if Instant::now() >= next_heartbeat {
                    super::upgrade_background::report_installing();
                    next_heartbeat = Instant::now() + heartbeat;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    }
}

fn command_output_with_timeout(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<TimedCommandOutput, BifrostError> {
    command_output_with_timeout_and_heartbeat(
        program,
        args,
        timeout,
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
    )
}

fn command_output_with_timeout_and_heartbeat(
    program: &Path,
    args: &[String],
    timeout: Duration,
    heartbeat: Duration,
) -> Result<TimedCommandOutput, BifrostError> {
    let mut stdout_file =
        tempfile::tempfile().map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;
    let mut stderr_file =
        tempfile::tempfile().map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout_file
                .try_clone()
                .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?,
        ))
        .stderr(Stdio::from(
            stderr_file
                .try_clone()
                .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?,
        ))
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;
    let status;

    loop {
        match child.try_wait() {
            Ok(Some(exit_status)) => {
                status = if exit_status.success() {
                    TimedCommandStatus::Success
                } else {
                    TimedCommandStatus::Failure
                };
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                status = TimedCommandStatus::TimedOut;
                break;
            }
            Ok(None) => {
                if Instant::now() >= next_heartbeat {
                    super::upgrade_background::report_installing();
                    next_heartbeat = Instant::now() + heartbeat;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    }

    Ok(TimedCommandOutput {
        status,
        stdout: read_temp_output(&mut stdout_file),
        stderr: read_temp_output(&mut stderr_file),
    })
}

fn read_temp_output(file: &mut fs::File) -> String {
    let mut output = String::new();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.read_to_string(&mut output);
    output
}

fn verify_installed_cli_target_version(
    executable: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
    if upgrade_test_overrides_enabled() && env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE").is_some() {
        return Ok(());
    }
    let output = command_output_with_timeout(
        executable,
        &["--version".to_string()],
        Duration::from_secs(BINARY_VERIFY_TIMEOUT_SECS),
    )?;
    if output.status != TimedCommandStatus::Success {
        return Err(BifrostError::Config(format!(
            "installed CLI version verification failed: {}",
            summarize_command_output(&output)
        )));
    }
    let normalized_target = target_version.trim().trim_start_matches('v');
    if output
        .stdout
        .split_whitespace()
        .any(|part| part.trim().trim_start_matches('v') == normalized_target)
    {
        return Ok(());
    }
    Err(BifrostError::Config(format!(
        "CLI install command reported success but {} reports `{}` instead of target v{}",
        executable.display(),
        output.stdout.trim(),
        normalized_target
    )))
}

fn verify_binary(path: &Path) -> bool {
    matches!(
        command_status_with_timeout(
            path,
            &["--version"],
            Duration::from_secs(BINARY_VERIFY_TIMEOUT_SECS)
        ),
        Ok(TimedCommandStatus::Success)
    )
}

fn post_upgrade_skill_install_message(status: TimedCommandStatus) -> &'static str {
    match status {
        TimedCommandStatus::Success => "✓ Bifrost skills installed successfully.",
        TimedCommandStatus::Failure => {
            "⚠ Bifrost skill installation failed; retry manually with: bifrost install-skill --tool all -y"
        }
        TimedCommandStatus::TimedOut => {
            "⚠ Bifrost skill installation timed out; retry manually with: bifrost install-skill --tool all -y"
        }
    }
}

fn install_skills_after_upgrade_best_effort(executable: &Path) {
    println!("{}", "Installing latest Bifrost skills...".bright_cyan());
    match command_status_with_timeout(
        executable,
        POST_UPGRADE_SKILL_INSTALL_ARGS,
        Duration::from_secs(POST_UPGRADE_SKILL_INSTALL_TIMEOUT_SECS),
    ) {
        Ok(status) => {
            let message = post_upgrade_skill_install_message(status);
            if status == TimedCommandStatus::Success {
                println!("{}", message.bright_green());
            } else {
                eprintln!("{}", message.bright_yellow());
            }
        }
        Err(error) => {
            eprintln!(
                "{} {}",
                "⚠ Could not run Bifrost skill installation after upgrade:".bright_yellow(),
                error.to_string().dimmed()
            );
            eprintln!(
                "{}",
                "  Retry manually with: bifrost install-skill --tool all -y".dimmed()
            );
        }
    }
}

fn installed_desktop_app_path() -> Option<PathBuf> {
    desktop_app_install_candidates()
        .into_iter()
        .find(|path| path.exists())
}

fn desktop_app_install_candidates() -> Vec<PathBuf> {
    if let Some(dir) = env::var_os("BIFROST_APP_INSTALL_DIR") {
        return vec![resolve_desktop_app_path(&PathBuf::from(dir))];
    }

    #[cfg(target_os = "macos")]
    {
        let mut candidates = Vec::new();
        candidates.push(PathBuf::from("/Applications/Bifrost.app"));
        if let Some(home) = env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join("Applications").join("Bifrost.app"));
        }
        candidates
    }
    #[cfg(target_os = "windows")]
    {
        let mut candidates = Vec::new();
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Bifrost")
                    .join("bifrost-desktop.exe"),
            );
        }
        candidates
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

fn resolve_desktop_app_path(app_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        app_dir.join("Bifrost.app")
    }
    #[cfg(target_os = "windows")]
    {
        app_dir.join("bifrost-desktop.exe")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        app_dir.join("Bifrost")
    }
}

fn post_upgrade_desktop_app_args(target_version: &str) -> Vec<String> {
    vec![
        "app".to_string(),
        "upgrade".to_string(),
        "--no-cli".to_string(),
        "--source".to_string(),
        "cli-upgrade".to_string(),
        "--version".to_string(),
        target_version.to_string(),
        "-y".to_string(),
    ]
}

fn update_desktop_app_after_upgrade_best_effort(executable: &Path, target_version: &str) {
    if let Err(error) = update_desktop_app_after_upgrade(executable, target_version) {
        eprintln!(
            "{} {}",
            "⚠ Bifrost desktop app update failed; continuing CLI upgrade.".bright_yellow(),
            error.to_string().dimmed()
        );
        eprintln!(
            "{}",
            "  Retry manually with: bifrost app upgrade --no-cli -y".dimmed()
        );
    }
}

fn update_desktop_app_after_upgrade(
    executable: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
    let Some(app_path) = installed_desktop_app_path() else {
        return Ok(());
    };

    println!();
    println!(
        "{} {}",
        "Detected installed Bifrost desktop app:".bright_cyan(),
        app_path.display()
    );
    println!("{}", "Updating Bifrost desktop app...".bright_cyan());

    let args = post_upgrade_desktop_app_args(target_version);
    match command_output_with_timeout(
        executable,
        &args,
        Duration::from_secs(POST_UPGRADE_APP_UPDATE_TIMEOUT_SECS),
    ) {
        Ok(output) if output.status == TimedCommandStatus::Success => {
            println!(
                "{}",
                "✓ Bifrost desktop app updated successfully.".bright_green()
            );
            Ok(())
        }
        Ok(output) => {
            let reason = summarize_command_output(&output);
            Err(BifrostError::Config(format!(
                "desktop app update command failed: {reason}"
            )))
        }
        Err(error) => Err(BifrostError::Config(format!(
            "could not run desktop app update: {error}"
        ))),
    }
}

fn summarize_command_output(output: &TimedCommandOutput) -> String {
    let reason = if output.status == TimedCommandStatus::TimedOut {
        "command timed out".to_string()
    } else {
        let stderr = output.stderr.trim();
        let stdout = output.stdout.trim();
        if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            "command exited with a non-zero status".to_string()
        }
    };
    truncate_reason(&reason, 600)
}

fn truncate_reason(reason: &str, max_chars: usize) -> String {
    let mut chars = reason.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

fn unique_temp_binary_path(target_path: &Path) -> PathBuf {
    let file_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bifrost");
    target_path.with_file_name(format!(".{}.tmp.{}", file_name, std::process::id()))
}

fn binary_backup_path(target_path: &Path) -> PathBuf {
    target_path.with_extension("backup")
}

fn cleanup_binary_backup(target_path: &Path) {
    let backup_path = binary_backup_path(target_path);
    if backup_path.exists() {
        let _ = fs::remove_file(backup_path);
    }
}

fn restore_binary_backup(target_path: &Path) -> Result<bool, BifrostError> {
    let backup_path = binary_backup_path(target_path);
    if !backup_path.exists() {
        return Ok(false);
    }

    #[cfg(windows)]
    {
        if target_path.exists() {
            fs::remove_file(target_path)?;
        }
    }

    #[cfg(unix)]
    {
        if target_path.exists() {
            fs::remove_file(target_path)?;
        }
    }

    fs::rename(&backup_path, target_path)?;
    Ok(true)
}

fn restore_binary_backup_best_effort(target_path: &Path) {
    match restore_binary_backup(target_path) {
        Ok(true) => eprintln!(
            "Restored previous Bifrost binary from backup: {}",
            target_path.display()
        ),
        Ok(false) => {}
        Err(error) => eprintln!(
            "Failed to restore previous Bifrost binary from backup: {}",
            error
        ),
    }
}

fn install_binary_atomically(
    new_binary: &Path,
    target_path: &Path,
) -> Result<UpgradeInstallOutcome, BifrostError> {
    #[cfg(windows)]
    if is_current_exe_path(target_path)? {
        let staged_binary = unique_pending_binary_path(target_path);
        let _ = fs::remove_file(&staged_binary);
        fs::copy(new_binary, &staged_binary)?;
        return Ok(UpgradeInstallOutcome::DeferredWindows(
            WindowsDeferredInstall {
                staged_binary,
                target_path: target_path.to_path_buf(),
            },
        ));
    }

    let temp_target = unique_temp_binary_path(target_path);
    let backup_path = binary_backup_path(target_path);

    let _ = fs::remove_file(&temp_target);
    fs::copy(new_binary, &temp_target)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_target)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_target, perms)?;
    }

    #[cfg(target_os = "macos")]
    {
        clear_quarantine_attr(&temp_target);
    }

    if target_path.exists() && !backup_path.exists() {
        fs::copy(target_path, &backup_path).map_err(|e| {
            BifrostError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to backup current binary {}: {}",
                    target_path.display(),
                    e
                ),
            ))
        })?;
    }

    #[cfg(windows)]
    {
        if target_path.exists() {
            fs::remove_file(target_path)?;
        }
    }

    match fs::rename(&temp_target, target_path) {
        Ok(()) => Ok(UpgradeInstallOutcome::Installed),
        Err(error) => {
            let _ = fs::remove_file(&temp_target);
            if backup_path.exists() && !target_path.exists() {
                let _ = fs::copy(&backup_path, target_path);
            }
            Err(BifrostError::Io(error))
        }
    }
}

#[cfg(windows)]
fn is_current_exe_path(target_path: &Path) -> Result<bool, BifrostError> {
    let current_exe = env::current_exe().map_err(BifrostError::Io)?;
    let current_exe = fs::canonicalize(&current_exe).unwrap_or(current_exe);
    let target_path = fs::canonicalize(target_path).unwrap_or_else(|_| target_path.to_path_buf());
    Ok(current_exe == target_path)
}

#[cfg(windows)]
fn unique_pending_binary_path(target_path: &Path) -> PathBuf {
    let file_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bifrost.exe");
    target_path.with_file_name(format!(".{}.pending.{}", file_name, std::process::id()))
}

fn github_mirror_bases() -> Vec<String> {
    let preferred = env::var("BIFROST_GITHUB_MIRROR")
        .ok()
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty());
    let mut bases = Vec::new();

    if let Some(preferred) = preferred.as_ref() {
        bases.push(preferred.clone());
    }

    for base in DEFAULT_GITHUB_MIRROR_URLS {
        let normalized = base.trim_end_matches('/').to_string();
        if preferred.as_deref() != Some(normalized.as_str()) {
            bases.push(normalized);
        }
    }

    bases
}

fn mirror_display_name(base_url: &str) -> String {
    base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

fn github_path_url(base_url: &str, github_path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        github_path.trim_start_matches('/')
    )
}

fn probe_github_url(url: &str, tuning: DownloadTuning) -> bool {
    let client = match bifrost_core::github_blocking_reqwest_client_builder()
        .connect_timeout(Duration::from_secs(tuning.connect_timeout_secs))
        .timeout(Duration::from_secs(tuning.mirror_probe_timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    let head_ok = client
        .head(url)
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false);
    if head_ok {
        return true;
    }

    client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .map(|response| {
            response.status().is_success()
                || response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        })
        .unwrap_or(false)
}

fn select_fastest_github_base(github_path: &str, tuning: DownloadTuning) -> Option<String> {
    let bases = github_mirror_bases();
    if bases.is_empty() {
        return None;
    }
    if bases.len() == 1 {
        return bases.into_iter().next();
    }

    let (tx, rx) = mpsc::channel();
    for (index, base) in bases.iter().cloned().enumerate() {
        let tx = tx.clone();
        let url = github_path_url(&base, github_path);
        thread::spawn(move || {
            let started = Instant::now();
            if probe_github_url(&url, tuning) {
                let _ = tx.send((index, base, started.elapsed()));
            }
        });
    }
    drop(tx);

    rx.recv_timeout(Duration::from_secs(tuning.mirror_probe_timeout_secs + 1))
        .ok()
        .map(|(_, base, _)| base)
}

pub(crate) fn download_progress_line(
    downloaded: u64,
    total: Option<u64>,
    started: Instant,
) -> String {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let speed = downloaded as f64 / elapsed;
    match total {
        Some(total) if total > 0 => {
            let percent = ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
            format!(
                "Downloading… {:>5.1}% ({}/{}, {}/s)",
                percent,
                human_bytes(downloaded),
                human_bytes(total),
                human_bytes(speed as u64)
            )
        }
        _ => format!(
            "Downloading… {} ({}/s)",
            human_bytes(downloaded),
            human_bytes(speed as u64)
        ),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn download_file_with_progress(
    url: &str,
    output_path: &Path,
    tuning: DownloadTuning,
) -> Result<(), BifrostError> {
    let client = bifrost_core::github_blocking_reqwest_client_builder()
        .connect_timeout(Duration::from_secs(tuning.connect_timeout_secs))
        .timeout(Duration::from_secs(tuning.download_timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| BifrostError::Network(format!("Failed to build HTTP client: {error}")))?;

    let mut last_error = None;
    for attempt in 1..=tuning.download_tries {
        if attempt > 1 {
            println!(
                "{}",
                format!(
                    "Retrying download ({}/{})...",
                    attempt, tuning.download_tries
                )
                .bright_yellow()
            );
        }

        match download_file_once_with_progress(&client, url, output_path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let _ = fs::remove_file(output_path);
                last_error = Some(error);
                if attempt < tuning.download_tries {
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| BifrostError::Network(format!("Failed to download {}", url))))
}

fn download_file_once_with_progress(
    client: &reqwest::blocking::Client,
    url: &str,
    output_path: &Path,
) -> Result<(), BifrostError> {
    let mut response = client.get(url).send().map_err(|error| {
        BifrostError::Network(format!(
            "Failed to download {url} — {}",
            bifrost_core::format_reqwest_error(&error)
        ))
    })?;

    if !response.status().is_success() {
        return Err(BifrostError::Network(format!(
            "Failed to download {} — HTTP {}",
            url,
            response.status()
        )));
    }

    let total = response.content_length();
    let mut file = fs::File::create(output_path).map_err(BifrostError::Io)?;
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_render = Instant::now() - Duration::from_secs(1);
    let started = Instant::now();

    loop {
        let read = response.read(&mut buffer).map_err(BifrostError::Io)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(BifrostError::Io)?;
        downloaded += read as u64;

        if last_render.elapsed() >= Duration::from_millis(250) {
            print!("\r{}", download_progress_line(downloaded, total, started));
            io::stdout().flush().ok();
            last_render = Instant::now();
            super::upgrade_background::report_download(downloaded, total, started);
        }
    }

    file.flush().map_err(BifrostError::Io)?;
    println!("\r{}", download_progress_line(downloaded, total, started));
    super::upgrade_background::report_download(downloaded, total, started);

    if downloaded == 0 {
        return Err(BifrostError::Network(format!(
            "Failed to download {} — empty response",
            url
        )));
    }

    Ok(())
}

fn ordered_download_bases(github_path: &str, tuning: DownloadTuning) -> Vec<String> {
    let bases = github_mirror_bases();
    let selected = select_fastest_github_base(github_path, tuning);
    let mut ordered = Vec::new();

    if let Some(selected) = selected {
        ordered.push(selected);
    }

    for base in bases {
        if !ordered.iter().any(|existing| existing == &base) {
            ordered.push(base);
        }
    }

    if ordered.is_empty() {
        ordered.push(GITHUB_BASE_URL.to_string());
    }

    ordered
}

fn archive_ext_candidates_for_os(
    os: &str,
    tar_xz_supported: bool,
    xz_disabled: bool,
) -> Vec<&'static str> {
    match os {
        "windows" => vec!["zip"],
        _ => {
            let mut candidates = Vec::new();
            if tar_xz_supported && !xz_disabled {
                candidates.push("tar.xz");
            }
            candidates.push("tar.gz");
            candidates
        }
    }
}

fn tar_supports_xz() -> bool {
    if env::var("BIFROST_DISABLE_XZ_ARCHIVE").ok().as_deref() == Some("1") {
        return false;
    }

    let output = Command::new("tar").arg("--help").output();
    let Ok(output) = output else {
        return false;
    };
    let help = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    help.contains("-J") || help.to_ascii_lowercase().contains("xz")
}

fn release_archive_ext_candidates() -> Vec<&'static str> {
    let os = if cfg!(windows) { "windows" } else { "unix" };
    archive_ext_candidates_for_os(
        os,
        tar_supports_xz(),
        env::var("BIFROST_DISABLE_XZ_ARCHIVE").ok().as_deref() == Some("1"),
    )
}

fn archive_ext_from_path(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.ends_with(".tar.xz") {
        Some("tar.xz")
    } else if file_name.ends_with(".tar.gz") {
        Some("tar.gz")
    } else if file_name.ends_with(".zip") {
        Some("zip")
    } else {
        None
    }
}

fn upgrade_test_overrides_enabled() -> bool {
    cfg!(debug_assertions)
        || env::var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES")
            .ok()
            .as_deref()
            == Some("1")
}

fn test_upgrade_archive_override() -> Result<Option<(PathBuf, &'static str)>, BifrostError> {
    if !upgrade_test_overrides_enabled() {
        return Ok(None);
    }

    let Some(path) = env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE").map(PathBuf::from) else {
        return Ok(None);
    };
    let archive_ext = archive_ext_from_path(&path).ok_or_else(|| {
        BifrostError::Config(format!(
            "BIFROST_UPGRADE_TEST_ARCHIVE must point to .tar.xz, .tar.gz, or .zip: {}",
            path.display()
        ))
    })?;
    Ok(Some((path, archive_ext)))
}

fn test_upgrade_latest_version_override() -> Option<VersionCache> {
    if !upgrade_test_overrides_enabled() {
        return None;
    }

    env::var("BIFROST_UPGRADE_TEST_LATEST_VERSION")
        .ok()
        .map(|latest_version| VersionCache {
            latest_version,
            release_highlights: vec!["local upgrade restart e2e".to_string()],
            checked_at: chrono::Utc::now(),
        })
}

fn validate_downloaded_archive(path: &Path, archive_ext: &str) -> Result<(), BifrostError> {
    if archive_ext == "zip" {
        return Ok(());
    }

    let tar_flag = if archive_ext == "tar.xz" {
        "-tJf"
    } else {
        "-tzf"
    };
    let output = Command::new("tar")
        .arg(tar_flag)
        .arg(path)
        .output()
        .map_err(BifrostError::Io)?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(BifrostError::Parse(format!(
            "Downloaded archive is invalid — {}",
            stderr.trim()
        )))
    }
}

fn print_update_info(current: &str, cache: &VersionCache) {
    let separator = "─".repeat(64);
    let release_url = format!("{}/v{}", GITHUB_RELEASE_URL, cache.latest_version);

    println!();
    println!("{}", separator.bright_cyan());
    println!("{}", "  📦 New version available!".bright_cyan().bold());
    println!();
    println!("     Current version: {}", current.bright_yellow().bold());
    println!(
        "     Latest version:  {}",
        cache.latest_version.bright_green().bold()
    );

    if !cache.release_highlights.is_empty() {
        println!();
        println!("     {}", "What's new:".bright_white().bold());
        for highlight in &cache.release_highlights {
            println!("       {} {}", "•".bright_cyan(), highlight.bright_white());
        }
    }

    println!();
    println!(
        "     {} {}",
        "Release notes:".dimmed(),
        release_url.dimmed()
    );
    println!("{}", separator.bright_cyan());
    println!();
}

const HOMEBREW_FORMULA_NAME: &str = "bifrost-proxy/bifrost/bifrost";

fn upgrade_via_homebrew(target_version: &str) -> Result<(), BifrostError> {
    println!("{}", "Refreshing Homebrew tap...".bright_cyan());

    let output = command_output_with_timeout(
        Path::new("brew"),
        &[
            "--repository".to_string(),
            "bifrost-proxy/bifrost".to_string(),
        ],
        Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
    );

    if let Ok(output) = output {
        if output.status == TimedCommandStatus::Success {
            let tap_path = output.stdout.trim();
            if !tap_path.is_empty() {
                let _ = command_status_with_timeout(
                    Path::new("git"),
                    &["-C", tap_path, "fetch", "--all", "-q"],
                    Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
                );
                let _ = command_status_with_timeout(
                    Path::new("git"),
                    &["-C", tap_path, "reset", "--hard", "origin/main", "-q"],
                    Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
                );
            }
        }
    }

    println!("{}", "Upgrading via Homebrew...".bright_cyan());

    let status = command_status_with_timeout(
        Path::new("brew"),
        &["reinstall", HOMEBREW_FORMULA_NAME],
        Duration::from_secs(HOMEBREW_COMMAND_TIMEOUT_SECS),
    );

    let success = match status {
        Ok(TimedCommandStatus::Success) => true,
        _ => {
            println!(
                "{}",
                "Standard install failed, trying --build-from-source...".bright_yellow()
            );
            command_status_with_timeout(
                Path::new("brew"),
                &["reinstall", "--build-from-source", HOMEBREW_FORMULA_NAME],
                Duration::from_secs(HOMEBREW_COMMAND_TIMEOUT_SECS),
            )? == TimedCommandStatus::Success
        }
    };

    if !success {
        return Err(BifrostError::Network(format!(
            "Homebrew upgrade failed. Try: brew reinstall {}",
            HOMEBREW_FORMULA_NAME
        )));
    }

    let output = command_output_with_timeout(
        Path::new("brew"),
        &[
            "info".to_string(),
            "--json=v2".to_string(),
            HOMEBREW_FORMULA_NAME.to_string(),
        ],
        Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
    )?;

    if output.status == TimedCommandStatus::Success {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&output.stdout) {
            if let Some(installed) = json["formulae"]
                .get(0)
                .and_then(|f| f["installed"].as_array())
                .and_then(|arr| arr.first())
                .and_then(|i| i["version"].as_str())
            {
                if installed == target_version {
                    println!(
                        "{}",
                        "✓ Upgrade completed successfully!".bright_green().bold()
                    );
                    return Ok(());
                } else {
                    println!(
                        "{}",
                        format!(
                            "⚠ Installed version ({}) doesn't match target version ({}).",
                            installed, target_version
                        )
                        .bright_yellow()
                    );
                    println!(
                            "{}",
                            "  The Homebrew tap may not be updated yet. Please try again later or install manually:"
                                .bright_yellow()
                        );
                    println!(
                            "  {}",
                            "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                                .bright_cyan()
                        );
                    return Ok(());
                }
            }
        }
    }

    println!(
        "{}",
        "⚠ Upgrade completed but could not verify installed version.".bright_yellow()
    );
    println!(
        "{}",
        "  Run `bifrost --version` to confirm the upgrade succeeded.".dimmed()
    );
    Ok(())
}

fn download_and_install(
    target: &str,
    version: &str,
    target_path: &Path,
    temp_dir: &tempfile::TempDir,
) -> Result<UpgradeInstallOutcome, BifrostError> {
    let tuning = DownloadTuning::from_env();
    let release_tag = make_release_tag(version);
    let mut last_error = None;
    let mut selected_archive_path = None;
    let mut selected_archive_ext = None;

    if let Some((archive_path, archive_ext)) = test_upgrade_archive_override()? {
        println!(
            "{} {}",
            "Using local test archive:".bright_cyan(),
            archive_path.display()
        );
        validate_downloaded_archive(&archive_path, archive_ext)?;
        selected_archive_path = Some(archive_path);
        selected_archive_ext = Some(archive_ext);
    }

    if selected_archive_path.is_none() {
        for archive_ext in release_archive_ext_candidates() {
            let archive_name = format!("bifrost-v{}-{}.{}", version, target, archive_ext);
            let archive_github_path = format!(
                "bifrost-proxy/bifrost/releases/download/{}/{}",
                release_tag, archive_name
            );
            let archive_path = temp_dir.path().join(&archive_name);

            for (attempt, base) in ordered_download_bases(&archive_github_path, tuning)
                .into_iter()
                .enumerate()
            {
                let download_url = github_path_url(&base, &archive_github_path);
                if attempt == 0 {
                    println!(
                        "{} {}",
                        "Selected fastest available source:".bright_cyan(),
                        mirror_display_name(&base).bright_white()
                    );
                } else {
                    println!(
                        "{} {}",
                        "Retrying with source:".bright_yellow(),
                        mirror_display_name(&base).bright_white()
                    );
                }
                println!("{} {}", "Downloading:".bright_cyan(), download_url.dimmed());

                match download_file_with_progress(&download_url, &archive_path, tuning) {
                    Ok(()) => {
                        if let Err(error) = validate_downloaded_archive(&archive_path, archive_ext)
                        {
                            let _ = fs::remove_file(&archive_path);
                            println!(
                                "{} {}",
                                "Downloaded archive failed validation:".bright_yellow(),
                                error.to_string().dimmed()
                            );
                            last_error = Some(error);
                            continue;
                        }
                        if attempt > 0 {
                            println!(
                                "{} {}",
                                "Downloaded via fallback source:".bright_green(),
                                mirror_display_name(&base).bright_white()
                            );
                        }
                        selected_archive_path = Some(archive_path);
                        selected_archive_ext = Some(archive_ext);
                        last_error = None;
                        break;
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&archive_path);
                        println!(
                            "{} {}",
                            "Download source failed:".bright_yellow(),
                            error.to_string().dimmed()
                        );
                        last_error = Some(error);
                    }
                }
            }

            if selected_archive_path.is_some() {
                break;
            }
            if archive_ext != "tar.gz" && archive_ext != "zip" {
                println!(
                    "{} {}",
                    "Archive download failed, falling back to:".bright_yellow(),
                    "tar.gz".bright_white()
                );
            }
        }
    }

    let archive_path = selected_archive_path.ok_or_else(|| {
        last_error.unwrap_or_else(|| {
            BifrostError::Network("Failed to download release archive".to_string())
        })
    })?;
    let archive_ext = selected_archive_ext
        .ok_or_else(|| BifrostError::Network("Failed to download release archive".to_string()))?;

    println!("{}", "Extracting archive...".bright_cyan());
    super::upgrade_background::report_installing();

    let extract_dir = temp_dir.path().join(format!("extract_{}", target));
    fs::create_dir_all(&extract_dir)?;

    if cfg!(windows) {
        let output = Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}'",
                    archive_path.display(),
                    extract_dir.display()
                ),
            ])
            .output()
            .map_err(BifrostError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BifrostError::Parse(format!(
                "Failed to extract archive — {}",
                stderr.trim()
            )));
        }
    } else {
        let tar_flag = if archive_ext == "tar.xz" {
            "-xJf"
        } else {
            "-xzf"
        };
        let output = Command::new("tar")
            .args([
                tar_flag,
                archive_path.to_str().unwrap(),
                "-C",
                extract_dir.to_str().unwrap(),
            ])
            .output()
            .map_err(BifrostError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BifrostError::Parse(format!(
                "Failed to extract archive — {}",
                stderr.trim()
            )));
        }
    }

    let binary_name = if cfg!(windows) {
        "bifrost.exe"
    } else {
        "bifrost"
    };
    let extracted_dir = extract_dir.join(format!("bifrost-v{}-{}", version, target));
    let new_binary = extracted_dir.join(binary_name);

    if !new_binary.exists() {
        return Err(BifrostError::NotFound(format!(
            "Binary not found in archive: {}",
            new_binary.display()
        )));
    }

    println!(
        "{} {}",
        "Replacing binary at:".bright_cyan(),
        target_path.display()
    );

    install_binary_atomically(&new_binary, target_path)
}

fn upgrade_manual(
    target_path: &Path,
    version: &str,
) -> Result<UpgradeInstallOutcome, BifrostError> {
    let target = get_target_triple().ok_or_else(|| {
        BifrostError::Config("Unsupported platform for automatic upgrade".to_string())
    })?;

    let temp_dir = tempfile::tempdir().map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

    let mut effective_target = target.to_string();

    let install_result = download_and_install(target, version, target_path, &temp_dir);

    let needs_musl_fallback = match &install_result {
        Ok(UpgradeInstallOutcome::Installed) => !verify_binary(target_path),
        #[cfg(windows)]
        Ok(UpgradeInstallOutcome::DeferredWindows(_)) => false,
        Err(_) => true,
    };

    let install_outcome = if needs_musl_fallback {
        if let Some(musl_target) = get_musl_fallback_triple(target) {
            let reason = if install_result.is_err() {
                "download/install failed"
            } else {
                "binary failed to run — likely a glibc version mismatch"
            };
            println!(
                "{}",
                format!("⚠ {} binary {}", target, reason).bright_yellow()
            );
            println!(
                "{}",
                format!("  Retrying with musl build: {}", musl_target).bright_cyan()
            );

            let fallback_result =
                match download_and_install(&musl_target, version, target_path, &temp_dir) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        restore_binary_backup_best_effort(target_path);
                        return Err(error);
                    }
                };

            if matches!(fallback_result, UpgradeInstallOutcome::Installed)
                && !verify_binary(target_path)
            {
                restore_binary_backup_best_effort(target_path);
                return Err(BifrostError::Config(
                    "Fallback musl binary also failed to run. Try installing manually with:\n  curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash".to_string(),
                ));
            }

            effective_target = musl_target;
            println!("{}", "✓ musl fallback succeeded".bright_green());
            cleanup_binary_backup(target_path);
            fallback_result
        } else if let Err(e) = install_result {
            restore_binary_backup_best_effort(target_path);
            return Err(e);
        } else {
            restore_binary_backup_best_effort(target_path);
            return Err(BifrostError::Config(
                "Installed binary failed verification (`bifrost --version` returned non-zero). Try installing manually with:\n  curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash".to_string(),
            ));
        }
    } else {
        cleanup_binary_backup(target_path);
        install_result?
    };

    println!(
        "{}",
        format!("✓ Upgrade completed successfully! ({})", effective_target)
            .bright_green()
            .bold()
    );
    Ok(install_outcome)
}

pub(crate) fn handle_background_upgrade(
    restart_hint: Option<RunningProxyHint>,
    target_version: Option<String>,
) -> Result<(), BifrostError> {
    prepare_running_proxy_marker(restart_hint)?;
    handle_upgrade_inner(UpgradeBehavior::background(), target_version)
}

pub fn handle_upgrade(_yes: bool) -> Result<(), BifrostError> {
    handle_upgrade_inner(
        UpgradeBehavior::interactive(
            env_flag(DESKTOP_MANAGED_SKIP_APP_ENV),
            env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV),
        ),
        env::var(DESKTOP_MANAGED_TARGET_ENV).ok(),
    )
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn handle_upgrade_inner(
    behavior: UpgradeBehavior,
    pinned_target: Option<String>,
) -> Result<(), BifrostError> {
    let current_version = env!("CARGO_PKG_VERSION");

    println!(
        "{} {}",
        "Checking for updates...".bright_cyan(),
        format!("(current: v{})", current_version).dimmed()
    );

    let test_latest = test_upgrade_latest_version_override();

    let cache = if let Some(target) = pinned_target {
        VersionCache {
            latest_version: target,
            release_highlights: Vec::new(),
            checked_at: chrono::Utc::now(),
        }
    } else if let Some(cache) = test_latest {
        cache
    } else {
        match get_latest_version_fresh_with_diagnostics() {
            Ok(c) => c,
            Err(diagnostic) => {
                if let Some(cached) = get_latest_version() {
                    println!(
                        "{}",
                        format!(
                            "⚠ Could not fetch latest version ({}), using cached data.",
                            diagnostic
                        )
                        .bright_yellow()
                    );
                    cached
                } else {
                    if behavior.require_desktop_app_update {
                        return Err(BifrostError::Config(format!(
                            "could not resolve the target release for background upgrade: {diagnostic}"
                        )));
                    }
                    println!(
                        "{}",
                        format!("⚠ Could not check for updates: {}", diagnostic).bright_yellow()
                    );
                    println!();
                    println!("{}", "  You can upgrade manually by running:".dimmed());
                    println!(
                        "  {}",
                        "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                            .bright_cyan()
                    );
                    println!();
                    println!("{}", "  Troubleshooting tips:".dimmed());
                    println!("{}", "    • Check your internet connection".dimmed());
                    println!(
                        "{}",
                        "    • If behind a proxy/firewall, ensure github.com is accessible"
                            .dimmed()
                    );
                    println!(
                        "{}",
                        "    • Try: curl -sI -o /dev/null -w '%{url_effective}' -L https://github.com/bifrost-proxy/bifrost/releases/latest"
                            .dimmed()
                    );
                    println!(
                        "{}",
                        "    • Set RUST_LOG=debug for detailed diagnostics".dimmed()
                    );
                    return Ok(());
                }
            }
        }
    };

    if !is_newer_version(current_version, &cache.latest_version) {
        println!(
            "{}",
            format!(
                "✓ You're already on the latest version (v{})",
                current_version
            )
            .bright_green()
            .bold()
        );
        return finish_already_latest_upgrade(&cache.latest_version, behavior);
    }

    print_update_info(current_version, &cache);

    let install_method = detect_install_method();
    println!(
        "     {} {}",
        "Install method:".dimmed(),
        format!("{}", install_method).bright_white()
    );
    println!();

    let restart_executable = restart_executable_for_install_method(&install_method)?;

    let upgrade_result = match install_method {
        InstallMethod::Homebrew => {
            upgrade_via_homebrew(&cache.latest_version).map(|()| UpgradeInstallOutcome::Installed)
        }
        // Script installs live in a stable user-owned path. Use the built-in
        // target-aware atomic installer here instead of re-running the online
        // install script, which always follows a fresh `latest` and could drift
        // away from the App target selected by this upgrade transaction.
        InstallMethod::Script => upgrade_manual(&restart_executable, &cache.latest_version),
        InstallMethod::Manual(path) => upgrade_manual(&path, &cache.latest_version),
        InstallMethod::Unknown => {
            if behavior.require_desktop_app_update {
                return Err(BifrostError::Config(
                    "could not detect the CLI installation method for background upgrade"
                        .to_string(),
                ));
            }
            println!(
                "{}",
                "⚠ Could not detect installation method.".bright_yellow()
            );
            println!("Please upgrade manually:");
            println!(
                "  {}",
                "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                    .bright_cyan()
            );
            println!(
                "  Or download from: {}",
                format!("{}/v{}", GITHUB_RELEASE_URL, cache.latest_version).bright_cyan()
            );
            return Ok(());
        }
    };

    let upgrade_outcome = upgrade_result?;

    match upgrade_outcome {
        UpgradeInstallOutcome::Installed => {
            verify_installed_cli_target_version(&restart_executable, &cache.latest_version)?;
            install_skills_after_upgrade_best_effort(&restart_executable);
            finish_installed_upgrade(&restart_executable, &cache.latest_version, behavior)?;
        }
        #[cfg(windows)]
        UpgradeInstallOutcome::DeferredWindows(deferred_install) => {
            // The helper cannot replace the running CLI until this process
            // exits, but the installed App can and must be brought to the same
            // pinned target before we schedule that handoff. Otherwise Windows
            // would report a completed CLI update while silently leaving the
            // App on the old version.
            update_desktop_companion(&restart_executable, &cache.latest_version, behavior)?;
            maybe_restart_running_proxy_after_windows_deferred_install(
                deferred_install,
                behavior.restart_proxy,
            )?
        }
    }

    Ok(())
}

fn finish_already_latest_upgrade(
    latest_version: &str,
    behavior: UpgradeBehavior,
) -> Result<(), BifrostError> {
    if !behavior.restart_if_already_latest && !behavior.update_desktop_app {
        return Ok(());
    }
    let install_method = detect_install_method();
    finish_already_latest_upgrade_for_method(latest_version, behavior, &install_method)
}

fn finish_already_latest_upgrade_for_method(
    latest_version: &str,
    behavior: UpgradeBehavior,
    install_method: &InstallMethod,
) -> Result<(), BifrostError> {
    let restart_executable = match restart_executable_for_install_method(install_method) {
        Ok(executable) => executable,
        Err(error) if behavior.require_desktop_app_update => return Err(error),
        Err(_) => return Ok(()),
    };
    update_desktop_companion(&restart_executable, latest_version, behavior)?;
    if behavior.restart_if_already_latest && behavior.restart_proxy {
        println!(
            "{}",
            "  On-disk binary is current; restarting any running proxy so it adopts this version."
                .bright_cyan()
        );
        maybe_restart_running_proxy(&restart_executable)?;
    }
    Ok(())
}

fn finish_installed_upgrade(
    restart_executable: &Path,
    latest_version: &str,
    behavior: UpgradeBehavior,
) -> Result<(), BifrostError> {
    update_desktop_companion(restart_executable, latest_version, behavior)?;
    if behavior.restart_proxy {
        maybe_restart_running_proxy(restart_executable)?;
    }
    Ok(())
}

fn update_desktop_companion(
    executable: &Path,
    target_version: &str,
    behavior: UpgradeBehavior,
) -> Result<(), BifrostError> {
    if !behavior.update_desktop_app {
        return Ok(());
    }
    if behavior.require_desktop_app_update {
        update_desktop_app_after_upgrade(executable, target_version)
    } else {
        update_desktop_app_after_upgrade_best_effort(executable, target_version);
        Ok(())
    }
}

fn prepare_running_proxy_marker(
    restart_hint: Option<RunningProxyHint>,
) -> Result<(), BifrostError> {
    prepare_running_proxy_marker_with(
        restart_hint,
        read_runtime_info(),
        is_process_running,
        |port| find_process_on_port(port).map(|listener| listener.pid),
        write_runtime_info,
    )
}

fn prepare_running_proxy_marker_with(
    restart_hint: Option<RunningProxyHint>,
    runtime_info: Option<RuntimeInfo>,
    is_running: impl Fn(u32) -> bool,
    listener_pid: impl Fn(u16) -> Option<u32>,
    write_runtime: impl Fn(&RuntimeInfo) -> Result<(), BifrostError>,
) -> Result<(), BifrostError> {
    let Some(hint) = restart_hint else {
        return Ok(());
    };
    if !is_running(hint.pid) || listener_pid(hint.port) != Some(hint.pid) {
        return Err(BifrostError::Config(format!(
            "Admin restart hint no longer owns the live listener on port {}",
            hint.port
        )));
    }
    let matching_runtime = runtime_info
        .as_ref()
        .filter(|info| info.pid == hint.pid && info.port == hint.port);
    if let Some(info) = matching_runtime {
        if info.restartable_daemon() {
            return Ok(());
        }
    }

    // A Web UI request issued by a CLI-owned foreground core must survive the
    // current terminal process exiting. Convert that exact, validated listener
    // to the detached-daemon restart contract before the updater stops it. The
    // desktop path never supplies a restart hint, so an App-owned core cannot be
    // reclassified or taken over here.
    let mut recovered = RuntimeInfo::new(
        hint.pid,
        hint.port,
        matching_runtime.and_then(|info| info.socks5_port),
        matching_runtime
            .and_then(|info| info.host.clone())
            .or_else(|| Some("0.0.0.0".to_string())),
        RuntimeStartMode::Daemon,
    );
    recovered.started_at_ms = matching_runtime.and_then(|info| info.started_at_ms);
    recovered.binary_path = matching_runtime.and_then(|info| info.binary_path.clone());
    recovered.system_proxy_enabled = matching_runtime.and_then(|info| info.system_proxy_enabled);
    recovered.system_proxy_bypass =
        matching_runtime.and_then(|info| info.system_proxy_bypass.clone());
    write_runtime(&recovered)?;
    println!(
        "{}",
        format!(
            "  Recovered missing runtime markers from the live Admin listener on port {}.",
            recovered.port
        )
        .bright_cyan()
    );
    Ok(())
}

fn maybe_restart_running_proxy(restart_executable: &Path) -> Result<(), BifrostError> {
    let runtime_info = read_runtime_info();
    if !cli_owns_runtime_restart(runtime_info.as_ref()) {
        println!(
            "{}",
            "  Running proxy is owned by the desktop app; leaving restart to the app handoff."
                .bright_cyan()
        );
        return Ok(());
    }
    let pid = match read_pid() {
        Some(pid) if is_process_running(pid) => pid,
        _ => return Ok(()),
    };

    println!();
    println!(
        "{}",
        format!("  Detected running Bifrost proxy (PID: {})", pid)
            .bright_yellow()
            .bold()
    );

    println!("{}", "  Auto-restarting the running proxy...".bright_cyan());

    let system_proxy_snapshot = capture_runtime_system_proxy_snapshot(runtime_info.as_ref());
    let default_system_proxy = if runtime_info.is_none() {
        Some(default_restart_system_proxy_config()?)
    } else {
        None
    };

    println!("{}", "  Stopping current proxy...".bright_cyan());
    super::upgrade_background::report_restarting();
    super::stop::run_stop_for_restart()
        .map_err(|e| BifrostError::Config(format!("Failed to stop running proxy: {}", e)))?;

    let restart_source = runtime_info
        .as_ref()
        .map(RestartArgsSource::Runtime)
        .unwrap_or(RestartArgsSource::DefaultConfig);
    let args = build_restart_args(
        restart_source,
        system_proxy_snapshot.as_ref(),
        default_system_proxy.as_ref(),
    );
    let restart_ports = restart_ports_from_runtime(runtime_info.as_ref());

    wait_for_restart_ports_release(&restart_ports)?;

    println!(
        "{} {} {}",
        "  Starting proxy with:".bright_cyan(),
        restart_executable.display(),
        args.join(" ")
    );

    let status = Command::new(restart_executable)
        .args(&args)
        // The proxy we are restarting from may itself be a detached daemon child,
        // in which case it carries BIFROST_DETACHED_DAEMON_CHILD=1 in its env. That
        // var is inherited by this upgrade process (and by an admin-spawned
        // self-update). If we let the restart command inherit it, `start -d` would
        // think it is *already* the detached child and run in the FOREGROUND,
        // blocking this `.status()` call forever (the upgrade hangs at
        // "restarting"). Strip it so the restart spawns a fresh, properly detached
        // daemon and returns control here.
        .env_remove(super::start::DETACHED_DAEMON_CHILD_ENV)
        .status()
        .map_err(BifrostError::Io)?;

    if status.success() {
        println!(
            "{}",
            "✓ Proxy restarted successfully with the new version!"
                .bright_green()
                .bold()
        );
    } else {
        return Err(BifrostError::Config(
            "Failed to restart proxy after upgrade. Please start manually with: bifrost start -d"
                .to_string(),
        ));
    }

    Ok(())
}

fn cli_owns_runtime_restart(runtime_info: Option<&RuntimeInfo>) -> bool {
    runtime_info
        .map(RuntimeInfo::restartable_daemon)
        .unwrap_or(true)
}

#[cfg(windows)]
fn maybe_restart_running_proxy_after_windows_deferred_install(
    deferred_install: WindowsDeferredInstall,
    restart_proxy: bool,
) -> Result<(), BifrostError> {
    let data_dir = get_bifrost_dir()?;
    let runtime_info = read_runtime_info();
    if !restart_proxy || !cli_owns_runtime_restart(runtime_info.as_ref()) {
        stop_tray_helper_before_windows_deferred_install(&data_dir);
        schedule_windows_deferred_install(deferred_install, None)?;
        println!(
            "{}",
            "✓ Upgrade replacement scheduled; runtime restart remains owned by the desktop app."
                .bright_green()
        );
        return Ok(());
    }
    let pid = match read_pid() {
        Some(pid) if is_process_running(pid) => Some(pid),
        _ => None,
    };

    let Some(pid) = pid else {
        stop_tray_helper_before_windows_deferred_install(&data_dir);
        schedule_windows_deferred_install(deferred_install, None)?;
        println!(
            "{}",
            "✓ Upgrade replacement scheduled and will finish after this process exits."
                .bright_green()
                .bold()
        );
        return Ok(());
    };
    println!();
    println!(
        "{}",
        format!("  Detected running Bifrost proxy (PID: {})", pid)
            .bright_yellow()
            .bold()
    );

    println!(
        "{}",
        "  Auto-restarting the running proxy so Windows can replace bifrost.exe...".bright_cyan()
    );

    let system_proxy_snapshot = capture_runtime_system_proxy_snapshot(runtime_info.as_ref());
    let default_system_proxy = if runtime_info.is_none() {
        Some(default_restart_system_proxy_config()?)
    } else {
        None
    };

    println!("{}", "  Stopping current proxy...".bright_cyan());
    super::upgrade_background::report_restarting();
    super::stop::run_stop_for_restart()
        .map_err(|e| BifrostError::Config(format!("Failed to stop running proxy: {}", e)))?;

    let restart_source = runtime_info
        .as_ref()
        .map(RestartArgsSource::Runtime)
        .unwrap_or(RestartArgsSource::DefaultConfig);
    let args = build_restart_args(
        restart_source,
        system_proxy_snapshot.as_ref(),
        default_system_proxy.as_ref(),
    );
    let restart_ports = restart_ports_from_runtime(runtime_info.as_ref());

    wait_for_restart_ports_release(&restart_ports)?;
    stop_tray_helper_before_windows_deferred_install(&data_dir);

    println!(
        "{} {} {}",
        "  Scheduling proxy restart with:".bright_cyan(),
        deferred_install.target_path.display(),
        args.join(" ")
    );

    schedule_windows_deferred_install(deferred_install, Some(&args))?;
    println!(
        "{}",
        "✓ Proxy restart scheduled with the new version after this process exits."
            .bright_green()
            .bold()
    );

    Ok(())
}

#[cfg(windows)]
fn stop_tray_helper_before_windows_deferred_install(data_dir: &Path) {
    println!(
        "{}",
        "  Stopping tray helper so Windows can replace bifrost.exe...".bright_cyan()
    );
    super::tray_launcher::stop_tray_helper(data_dir);
}

#[cfg(windows)]
fn schedule_windows_deferred_install(
    deferred_install: WindowsDeferredInstall,
    restart_args: Option<&[String]>,
) -> Result<(), BifrostError> {
    let parent_pid = std::process::id();
    let target_dir = deferred_install
        .target_path
        .parent()
        .ok_or_else(|| {
            BifrostError::Config(format!(
                "Cannot determine install directory for {}",
                deferred_install.target_path.display()
            ))
        })?
        .to_path_buf();
    let suffix = parent_pid.to_string();
    let script_path = target_dir.join(format!(".bifrost-upgrade-{}.ps1", suffix));
    let args_path = target_dir.join(format!(".bifrost-upgrade-{}.args", suffix));
    let log_path = target_dir.join(format!(".bifrost-upgrade-{}.log", suffix));
    let ready_path = target_dir.join(format!(".bifrost-upgrade-{}.ok", suffix));
    let progress_dir = get_bifrost_dir()?;
    let progress_path = progress_dir.join(bifrost_core::upgrade_progress::PROGRESS_FILE_NAME);
    let progress_snapshot = bifrost_core::upgrade_progress::read_progress(&progress_dir);
    let progress_target_version = progress_snapshot.target_version.unwrap_or_default();
    let progress_source = progress_snapshot.source.unwrap_or_default();

    if let Some(args) = restart_args {
        fs::write(&args_path, args.join("\n")).map_err(BifrostError::Io)?;
    } else {
        let _ = fs::remove_file(&args_path);
    }

    fs::write(
        &script_path,
        r#"
param(
  [int]$ParentPid,
  [string]$PendingPath,
  [string]$TargetPath,
  [string]$RestartArgsPath,
  [string]$ReadyPath,
  [string]$LogPath,
  [string]$ProgressPath,
  [string]$TargetVersion,
  [string]$Source
)

$ErrorActionPreference = "Stop"
function Write-UpgradeLog([string]$Message) {
  $timestamp = (Get-Date).ToString("o")
  Add-Content -LiteralPath $LogPath -Value "$timestamp $Message"
}

function Write-UpgradeProgress([string]$Phase, [string]$Message, [string]$ErrorMessage) {
  try {
    $previous = $null
    if ($ProgressPath -and (Test-Path -LiteralPath $ProgressPath)) {
      try {
        $previous = Get-Content -LiteralPath $ProgressPath -Raw -Encoding UTF8 | ConvertFrom-Json
      } catch {
        Write-UpgradeLog "WARNING: ignoring unreadable previous progress: $($_.Exception.Message)"
      }
    }
    $progress = [ordered]@{
      phase = $Phase
      percent = $null
      message = $Message
      target_version = if ($TargetVersion) { $TargetVersion } elseif ($previous -and $previous.target_version) { $previous.target_version } else { $null }
      source = if ($Source) { $Source } elseif ($previous -and $previous.source) { $previous.source } else { $null }
      error = if ($ErrorMessage) { $ErrorMessage } else { $null }
      updated_at = (Get-Date).ToUniversalTime().ToString("o")
    }
    $tmpPath = "$ProgressPath.tmp"
    $json = $progress | ConvertTo-Json -Depth 4
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)
    Move-Item -LiteralPath $tmpPath -Destination $ProgressPath -Force
  } catch {
    Write-UpgradeLog "WARNING: failed to write progress: $($_.Exception.Message)"
  }
}

function Wait-TargetPathWritable([string]$Path, [int]$TimeoutSeconds) {
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    try {
      if (Test-Path -LiteralPath $Path) {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $stream.Close()
      }
      return
    } catch {
      Write-UpgradeProgress "restarting" "Waiting for the old CLI to exit..." $null
      Start-Sleep -Milliseconds 500
    }
  }
  throw "target binary is still locked: $Path"
}

$backupPath = "$TargetPath.upgrade-backup"
$replacementVerified = $false

try {
  Write-UpgradeLog "waiting for parent pid $ParentPid"
  $parent = Get-Process -Id $ParentPid -ErrorAction SilentlyContinue
  if ($parent) {
    $parent | Wait-Process -Timeout 120
  }
  if (Get-Process -Id $ParentPid -ErrorAction SilentlyContinue) {
    throw "parent process $ParentPid did not exit before timeout"
  }

  Write-UpgradeProgress "restarting" "Finalizing upgrade..." $null
  Write-UpgradeLog "waiting for target binary to become writable"
  Wait-TargetPathWritable $TargetPath 120

  if (Test-Path -LiteralPath $backupPath) {
    Write-UpgradeLog "recovering interrupted replacement from $backupPath"
    if (Test-Path -LiteralPath $TargetPath) {
      Remove-Item -LiteralPath $TargetPath -Force
    }
    Move-Item -LiteralPath $backupPath -Destination $TargetPath -Force
  }

  Write-UpgradeLog "replacing $TargetPath"
  if (Test-Path -LiteralPath $TargetPath) {
    Copy-Item -LiteralPath $TargetPath -Destination $backupPath -Force
    Remove-Item -LiteralPath $TargetPath -Force
  }
  Move-Item -LiteralPath $PendingPath -Destination $TargetPath -Force

  $versionOutput = ((& $TargetPath --version 2>&1) | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) {
    throw "installed CLI version check exited with code $LASTEXITCODE"
  }
  if ($TargetVersion) {
    $normalizedTarget = $TargetVersion.TrimStart("v")
    $targetPattern = "(^|\s)v?$([Regex]::Escape($normalizedTarget))(\s|$)"
    if ($versionOutput -notmatch $targetPattern) {
      throw "installed CLI reports '$versionOutput' instead of target v$normalizedTarget"
    }
  }
  $replacementVerified = $true

  Write-UpgradeLog "installing Bifrost skills"
  $skillChild = Start-Process -FilePath $TargetPath -ArgumentList @("install-skill", "--tool", "all", "-y") -NoNewWindow -PassThru -Wait
  if ($skillChild.ExitCode -ne 0) {
    Write-UpgradeLog "WARNING: skill installation exited with code $($skillChild.ExitCode)"
  }

  if ($RestartArgsPath -and (Test-Path -LiteralPath $RestartArgsPath)) {
    $restartArgs = [System.IO.File]::ReadAllLines($RestartArgsPath)
    if ($restartArgs.Count -gt 0) {
      Write-UpgradeLog "starting $TargetPath $($restartArgs -join ' ')"
      $child = Start-Process -FilePath $TargetPath -ArgumentList $restartArgs -NoNewWindow -PassThru -Wait
      if ($child.ExitCode -ne 0) {
        throw "restart command exited with code $($child.ExitCode)"
      }
    }
  }

  Set-Content -LiteralPath $ReadyPath -Value "ok"
  Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue
  Write-UpgradeProgress "completed" "Upgrade complete" $null
  Write-UpgradeLog "done"
  Remove-Item -LiteralPath $RestartArgsPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
  exit 0
} catch {
  $errorMessage = $_.Exception.Message
  if (-not $replacementVerified -and (Test-Path -LiteralPath $backupPath)) {
    try {
      if (Test-Path -LiteralPath $TargetPath) {
        Remove-Item -LiteralPath $TargetPath -Force
      }
      Move-Item -LiteralPath $backupPath -Destination $TargetPath -Force
      Write-UpgradeLog "restored previous CLI after replacement failure"
    } catch {
      Write-UpgradeLog "ERROR: failed to restore previous CLI: $($_.Exception.Message)"
    }
  }
  Write-UpgradeProgress "failed" "Upgrade failed" $errorMessage
  Write-UpgradeLog "ERROR: $errorMessage"
  exit 1
}
"#,
    )
    .map_err(BifrostError::Io)?;

    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script_path)
        .arg("-ParentPid")
        .arg(parent_pid.to_string())
        .arg("-PendingPath")
        .arg(&deferred_install.staged_binary)
        .arg("-TargetPath")
        .arg(&deferred_install.target_path)
        .arg("-RestartArgsPath")
        .arg(&args_path)
        .arg("-ReadyPath")
        .arg(&ready_path)
        .arg("-LogPath")
        .arg(&log_path)
        .arg("-ProgressPath")
        .arg(&progress_path)
        .arg("-TargetVersion")
        .arg(progress_target_version)
        .arg("-Source")
        .arg(progress_source)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The helper relaunches bifrost with `start -d`; strip the detached-daemon
        // marker so the relaunched proxy detaches properly instead of running in
        // the foreground (see the unix restart path for the full rationale).
        .env_remove(super::start::DETACHED_DAEMON_CHILD_ENV);

    command.spawn().map_err(BifrostError::Io)?;
    mark_deferred_install_scheduled();
    println!(
        "{} {}",
        "  Windows upgrade helper log:".dimmed(),
        log_path.display().to_string().dimmed()
    );
    Ok(())
}

fn default_restart_system_proxy_config() -> Result<RestartSystemProxyConfig, BifrostError> {
    let bifrost_dir = get_bifrost_dir()?;
    let config_manager = ConfigManager::new(bifrost_dir)?;
    let config = futures::executor::block_on(config_manager.config());
    Ok(RestartSystemProxyConfig {
        enabled: config.system_proxy.enabled,
        bypass: config.system_proxy.bypass,
    })
}

fn restart_ports_from_runtime(runtime_info: Option<&crate::process::RuntimeInfo>) -> Vec<u16> {
    let Some(info) = runtime_info else {
        return vec![9900];
    };

    let mut ports = vec![info.port];
    if let Some(socks5_port) = info.socks5_port {
        if socks5_port != info.port {
            ports.push(socks5_port);
        }
    }
    ports
}

fn wait_for_restart_ports_release(ports: &[u16]) -> Result<(), BifrostError> {
    for port in ports {
        wait_for_restart_port_release(*port)?;
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn wait_for_restart_port_release(port: u16) -> Result<(), BifrostError> {
    let budget = Duration::from_secs(UPGRADE_RESTART_PORT_RELEASE_TIMEOUT_SECS);
    println!(
        "{}",
        format!(
            "  Waiting for proxy port {} to be released before restart...",
            port
        )
        .bright_cyan()
    );

    if crate::process::wait_for_port_released(port, budget) {
        return Ok(());
    }

    if let Ok(data_dir) = crate::config::get_bifrost_dir() {
        if let Err(error) = bifrost_core::SystemProxyManager::recover_from_crash(&data_dir) {
            eprintln!("Failed to recover system proxy after aborted upgrade restart: {error}");
        }
        let _ = bifrost_core::consume_system_proxy_shutdown_mode(&data_dir);
    }

    let holder = find_process_on_port(port)
        .map(|info| format!(" Current listener: {} (PID {}).", info.name, info.pid))
        .unwrap_or_else(|| " No listener was visible when reporting.".to_string());

    #[cfg(windows)]
    let hint = format!(
        "Run `netstat -ano | findstr :{}` to find the owning PID, then run `bifrost start -d` after the port is free.",
        port
    );
    #[cfg(unix)]
    let hint = format!(
        "Try `lsof -nP -iTCP:{} -sTCP:LISTEN` and then run `bifrost start -d` after the port is free.",
        port
    );

    Err(BifrostError::Network(format!(
        "Proxy port {} was still occupied after {}s, so upgrade did not start a replacement daemon to avoid a bind failure.{} {}",
        port, UPGRADE_RESTART_PORT_RELEASE_TIMEOUT_SECS, holder, hint
    )))
}

#[cfg(not(any(unix, windows)))]
fn wait_for_restart_port_release(_port: u16) -> Result<(), BifrostError> {
    Ok(())
}

fn build_restart_args(
    source: RestartArgsSource<'_>,
    system_proxy_snapshot: Option<&RuntimeSystemProxySnapshot>,
    default_system_proxy: Option<&RestartSystemProxyConfig>,
) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "-d".to_string(),
        "-y".to_string(),
        "--skip-cert-check".to_string(),
    ];

    if let RestartArgsSource::Runtime(info) = source {
        args.push("-p".to_string());
        args.push(info.port.to_string());

        if let Some(ref host) = info.host {
            if host != "127.0.0.1" {
                args.push("--host".to_string());
                args.push(host.clone());
            }
        }

        if let Some(socks5_port) = info.socks5_port {
            args.push("--socks5-port".to_string());
            args.push(socks5_port.to_string());
        }
    }

    if let Some(snapshot) = system_proxy_snapshot {
        args.push("--system-proxy".to_string());
        args.push("--proxy-bypass".to_string());
        args.push(snapshot.bypass.clone());
    } else if let RestartArgsSource::Runtime(info) = source {
        if info.system_proxy_enabled.unwrap_or(false) {
            args.push("--system-proxy".to_string());
            if let Some(bypass) = info.system_proxy_bypass.as_ref() {
                args.push("--proxy-bypass".to_string());
                args.push(bypass.clone());
            }
        } else {
            args.push("--no-system-proxy".to_string());
        }
    } else if let Some(config) = default_system_proxy {
        if config.enabled {
            args.push("--system-proxy".to_string());
            args.push("--proxy-bypass".to_string());
            args.push(config.bypass.clone());
        } else {
            args.push("--no-system-proxy".to_string());
        }
    } else {
        args.push("--no-system-proxy".to_string());
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_install_method_returns_valid_variant() {
        let method = detect_install_method();
        match method {
            InstallMethod::Homebrew
            | InstallMethod::Script
            | InstallMethod::Manual(_)
            | InstallMethod::Unknown => {}
        }
    }

    #[test]
    fn homebrew_restart_uses_stable_launcher_outside_versioned_cellar() {
        assert_eq!(
            homebrew_launcher_for_executable(Path::new(
                "/opt/homebrew/Cellar/bifrost/0.0.155/bin/bifrost"
            )),
            PathBuf::from("/opt/homebrew/bin/bifrost")
        );
        assert_eq!(
            homebrew_launcher_for_executable(Path::new(
                "/usr/local/Cellar/bifrost/0.0.155/bin/bifrost"
            )),
            PathBuf::from("/usr/local/bin/bifrost")
        );
        assert_eq!(
            homebrew_launcher_for_executable(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/bifrost/0.0.155/bin/bifrost"
            )),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin/bifrost")
        );
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_upgrade_commands_are_bounded_and_verify_formula_target() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "BIFROST_TEST_HOMEBREW_UPGRADE_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::tests::homebrew_upgrade_commands_are_bounded_and_verify_formula_target",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated Homebrew upgrade test");
            assert!(status.success());
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let brew = dir.path().join("brew");
        let git = dir.path().join("git");
        fs::write(
            &brew,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  --repository) echo '{}' ;;\n  reinstall) exit 0 ;;\n  info) echo '{{\"formulae\":[{{\"installed\":[{{\"version\":\"0.0.156\"}}]}}]}}' ;;\nesac\n",
                dir.path().display()
            ),
        )
        .unwrap();
        fs::write(&git, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", dir.path());

        upgrade_via_homebrew("0.0.156").expect("fake Homebrew target verified");
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_upgrade_fallback_and_verification_failures_are_bounded() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "BIFROST_TEST_HOMEBREW_FAILURE_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::tests::homebrew_upgrade_fallback_and_verification_failures_are_bounded",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated Homebrew failure test");
            assert!(status.success());
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let brew = dir.path().join("brew");
        let git = dir.path().join("git");
        fs::write(
            &brew,
            "#!/bin/sh\n\
             case \"$1\" in\n\
               --repository) echo /missing/tap ;;\n\
               reinstall)\n\
                 if [ \"$BIFROST_TEST_BREW_MODE\" = fail-all ]; then exit 9; fi\n\
                 if [ \"$2\" = --build-from-source ]; then exit 0; fi\n\
                 exit 7 ;;\n\
               info)\n\
                 if [ \"$BIFROST_TEST_BREW_MODE\" = bad-info ]; then exit 8; fi\n\
                 echo '{\"formulae\":[{\"installed\":[{\"version\":\"0.0.155\"}]}]}' ;;\n\
             esac\n",
        )
        .expect("write fake brew");
        fs::write(&git, "#!/bin/sh\nexit 1\n").expect("write fake git");
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).expect("chmod brew");
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).expect("chmod git");
        std::env::set_var("PATH", dir.path());

        upgrade_via_homebrew("0.0.156")
            .expect("source-build fallback completes before final CLI verification");
        std::env::set_var("BIFROST_TEST_BREW_MODE", "bad-info");
        upgrade_via_homebrew("0.0.156")
            .expect("unreadable metadata is deferred to final CLI verification");
        std::env::set_var("BIFROST_TEST_BREW_MODE", "fail-all");
        assert!(upgrade_via_homebrew("0.0.156").is_err());
    }

    #[test]
    fn test_install_method_display() {
        assert_eq!(InstallMethod::Homebrew.to_string(), "Homebrew");
        assert_eq!(InstallMethod::Script.to_string(), "Install script");
        assert_eq!(
            InstallMethod::Manual(PathBuf::from("/usr/local/bin/bifrost")).to_string(),
            "Manual (/usr/local/bin/bifrost)"
        );
        assert_eq!(InstallMethod::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_cli_upgrade_rejects_restart_flag() {
        use crate::cli::Cli;
        use clap::Parser;

        let result = Cli::try_parse_from(["bifrost", "upgrade", "--restart"]);
        assert!(result.is_err(), "--restart should be removed from upgrade");
    }

    #[test]
    fn test_cli_upgrade_hidden_yes_flag_is_accepted() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from(["bifrost", "upgrade", "-y"]);
        match cli.command {
            Some(Commands::Upgrade { yes }) => {
                assert!(yes);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_no_flags() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from(["bifrost", "upgrade"]);
        match cli.command {
            Some(Commands::Upgrade { yes }) => {
                assert!(!yes);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn background_upgrade_restarts_when_disk_binary_is_already_latest() {
        let background = UpgradeBehavior::background();
        assert!(background.restart_if_already_latest);
        assert!(background.update_desktop_app);
        assert!(background.require_desktop_app_update);
        assert!(background.restart_proxy);

        let desktop_managed = UpgradeBehavior::interactive(true, true);
        assert!(!desktop_managed.restart_if_already_latest);
        assert!(!desktop_managed.update_desktop_app);
        assert!(!desktop_managed.require_desktop_app_update);
        assert!(!desktop_managed.restart_proxy);

        let manual = UpgradeBehavior::interactive(false, false);
        assert!(!manual.restart_if_already_latest);
        assert!(manual.update_desktop_app);
        assert!(!manual.require_desktop_app_update);
        assert!(manual.restart_proxy);
    }

    #[cfg(unix)]
    #[test]
    fn upgrade_behavior_executes_companion_and_runtime_ownership_branches() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "BIFROST_TEST_UPGRADE_BEHAVIOR_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::tests::upgrade_behavior_executes_companion_and_runtime_ownership_branches",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated upgrade behavior test");
            assert!(status.success(), "isolated upgrade behavior test failed");
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let app_dir = temp.path().join("apps");
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&app_dir).expect("create app dir");
        std::fs::create_dir_all(&data_dir).expect("create data dir");

        let env_keys = [
            "BIFROST_APP_INSTALL_DIR",
            "BIFROST_DATA_DIR",
            DESKTOP_MANAGED_SKIP_APP_ENV,
            DESKTOP_MANAGED_SKIP_RESTART_ENV,
            "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        ];
        let previous: Vec<_> = env_keys
            .iter()
            .map(|key| ((*key).to_string(), std::env::var_os(key)))
            .collect();
        std::env::set_var("BIFROST_APP_INSTALL_DIR", &app_dir);
        std::env::set_var("BIFROST_DATA_DIR", &data_dir);
        std::env::set_var(
            "BIFROST_UPGRADE_TEST_LATEST_VERSION",
            env!("CARGO_PKG_VERSION"),
        );

        let app_path = desktop_app_install_candidates()
            .into_iter()
            .next()
            .expect("app install candidate");
        if app_path
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            std::fs::create_dir_all(&app_path).expect("create app bundle");
        } else {
            std::fs::write(&app_path, b"fixture").expect("write app fixture");
        }

        let success = temp.path().join("success-cli");
        std::fs::write(&success, "#!/bin/sh\nexit 0\n").expect("write success helper");
        std::fs::set_permissions(&success, std::fs::Permissions::from_mode(0o755))
            .expect("chmod success helper");
        let failure = temp.path().join("failure-cli");
        std::fs::write(&failure, "#!/bin/sh\necho app-failed >&2\nexit 7\n")
            .expect("write failure helper");
        std::fs::set_permissions(&failure, std::fs::Permissions::from_mode(0o755))
            .expect("chmod failure helper");

        update_desktop_app_after_upgrade(&success, "99.0.1").expect("strict app success");
        let error =
            update_desktop_app_after_upgrade(&failure, "99.0.1").expect_err("strict app failure");
        assert!(error.to_string().contains("app-failed"));
        assert!(
            update_desktop_app_after_upgrade(&temp.path().join("missing"), "99.0.1")
                .expect_err("spawn error")
                .to_string()
                .contains("could not run")
        );
        update_desktop_app_after_upgrade_best_effort(&failure, "99.0.1");

        update_desktop_companion(&failure, "99.0.1", UpgradeBehavior::interactive(true, true))
            .expect("desktop-managed child skips app");
        update_desktop_companion(&success, "99.0.1", UpgradeBehavior::background())
            .expect("background requires successful app");
        update_desktop_companion(
            &failure,
            "99.0.1",
            UpgradeBehavior::interactive(false, true),
        )
        .expect("manual app update remains best effort");

        std::fs::remove_file(&app_path)
            .or_else(|_| std::fs::remove_dir_all(&app_path))
            .ok();
        finish_already_latest_upgrade(
            env!("CARGO_PKG_VERSION"),
            UpgradeBehavior::interactive(true, true),
        )
        .expect("desktop-managed already-latest is a no-op");
        finish_already_latest_upgrade(env!("CARGO_PKG_VERSION"), UpgradeBehavior::background())
            .expect("background already-latest restarts when no app is installed");
        assert!(finish_already_latest_upgrade_for_method(
            env!("CARGO_PKG_VERSION"),
            UpgradeBehavior::background(),
            &InstallMethod::Unknown,
        )
        .is_err());
        finish_already_latest_upgrade_for_method(
            env!("CARGO_PKG_VERSION"),
            UpgradeBehavior::interactive(false, true),
            &InstallMethod::Unknown,
        )
        .expect("manual unknown install remains best effort");
        finish_installed_upgrade(
            &success,
            env!("CARGO_PKG_VERSION"),
            UpgradeBehavior::interactive(true, true),
        )
        .expect("desktop-managed installed finish skips app and restart");

        let desktop_runtime = RuntimeInfo::new(
            std::process::id(),
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Desktop,
        );
        write_runtime_info(&desktop_runtime).expect("write desktop runtime");
        maybe_restart_running_proxy(&success).expect("desktop owns restart handoff");
        finish_installed_upgrade(
            &success,
            env!("CARGO_PKG_VERSION"),
            UpgradeBehavior {
                restart_if_already_latest: false,
                update_desktop_app: false,
                require_desktop_app_update: false,
                restart_proxy: true,
            },
        )
        .expect("installed desktop runtime leaves restart to app");

        let free_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
        let free_port = free_listener.local_addr().expect("local addr").port();
        drop(free_listener);
        assert!(prepare_running_proxy_marker(Some(RunningProxyHint {
            pid: std::process::id(),
            port: free_port,
        }))
        .is_err());

        std::env::set_var(DESKTOP_MANAGED_SKIP_APP_ENV, "yes");
        std::env::set_var(DESKTOP_MANAGED_SKIP_RESTART_ENV, "true");
        assert!(env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));
        assert!(env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV));
        handle_upgrade(true).expect("desktop-managed CLI wrapper");
        std::env::remove_var(DESKTOP_MANAGED_SKIP_APP_ENV);
        assert!(!env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn only_restartable_daemon_runtime_is_owned_by_cli_upgrade() {
        assert!(cli_owns_runtime_restart(None));
        let daemon = RuntimeInfo::new(
            12345,
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );
        let desktop = RuntimeInfo::new(
            12346,
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Desktop,
        );
        let foreground = RuntimeInfo::new(
            12347,
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Foreground,
        );
        assert!(cli_owns_runtime_restart(Some(&daemon)));
        assert!(!cli_owns_runtime_restart(Some(&desktop)));
        assert!(!cli_owns_runtime_restart(Some(&foreground)));
    }

    #[test]
    fn self_update_accepts_admin_running_proxy_hint_as_a_complete_pair() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from([
            "bifrost",
            "self-update",
            "--source",
            "admin",
            "--running-proxy-pid",
            "12345",
            "--running-proxy-port",
            "18890",
        ]);
        match cli.command {
            Some(Commands::SelfUpdate {
                running_proxy_pid,
                running_proxy_port,
                ..
            }) => {
                assert_eq!(running_proxy_pid, Some(12345));
                assert_eq!(running_proxy_port, Some(18890));
            }
            _ => panic!("Expected SelfUpdate command"),
        }

        assert!(
            Cli::try_parse_from(["bifrost", "self-update", "--running-proxy-pid", "12345",])
                .is_err()
        );
        assert_eq!(
            RunningProxyHint::from_parts(Some(12345), Some(18890)),
            Some(RunningProxyHint {
                pid: 12345,
                port: 18890,
            })
        );
        assert_eq!(RunningProxyHint::from_parts(Some(12345), None), None);
    }

    #[test]
    fn running_proxy_hint_requires_exact_live_listener_before_recovering_marker() {
        let hint = RunningProxyHint {
            pid: 12345,
            port: 18890,
        };

        prepare_running_proxy_marker(None).unwrap();
        assert!(prepare_running_proxy_marker_with(
            Some(hint),
            None,
            |_| false,
            |_| panic!("listener lookup must not run for a dead pid"),
            |_| panic!("dead pid must not write runtime markers"),
        )
        .is_err());
        assert!(prepare_running_proxy_marker_with(
            Some(hint),
            None,
            |_| true,
            |_| Some(54321),
            |_| panic!("mismatched listener must not write runtime markers"),
        )
        .is_err());

        let existing = RuntimeInfo::new(
            hint.pid,
            hint.port,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );
        prepare_running_proxy_marker_with(
            Some(hint),
            Some(existing),
            |_| true,
            |_| Some(hint.pid),
            |_| panic!("matching runtime marker must not be rewritten"),
        )
        .unwrap();

        let foreground = RuntimeInfo::new(
            hint.pid,
            hint.port,
            Some(18891),
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Foreground,
        )
        .with_system_proxy(true, "localhost");
        let normalized = std::cell::RefCell::new(None);
        prepare_running_proxy_marker_with(
            Some(hint),
            Some(foreground),
            |_| true,
            |_| Some(hint.pid),
            |runtime| {
                normalized.replace(Some(runtime.clone()));
                Ok(())
            },
        )
        .unwrap();
        let normalized = normalized
            .into_inner()
            .expect("foreground marker normalized for detached restart");
        assert_eq!(normalized.start_mode, RuntimeStartMode::Daemon);
        assert_eq!(normalized.socks5_port, Some(18891));
        assert_eq!(normalized.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(normalized.system_proxy_enabled, Some(true));
        assert_eq!(normalized.system_proxy_bypass.as_deref(), Some("localhost"));

        let recovered = std::cell::RefCell::new(None);
        prepare_running_proxy_marker_with(
            Some(hint),
            None,
            |_| true,
            |_| Some(hint.pid),
            |runtime| {
                recovered.replace(Some(runtime.clone()));
                Ok(())
            },
        )
        .unwrap();
        let recovered = recovered.into_inner().expect("recovered runtime marker");
        assert_eq!(recovered.pid, hint.pid);
        assert_eq!(recovered.port, hint.port);
        assert_eq!(recovered.start_mode, RuntimeStartMode::Daemon);
        assert!(recovered.started_at_ms.is_none());
        assert!(recovered.binary_path.is_none());

        assert!(handle_background_upgrade(
            Some(RunningProxyHint {
                pid: i32::MAX as u32,
                port: 1,
            }),
            None,
        )
        .is_err());
    }

    #[test]
    fn windows_deferred_install_waits_for_tray_unlock_and_reports_terminal_progress() {
        let source = include_str!("upgrade.rs");
        assert!(source.contains(
            "update_desktop_companion(&restart_executable, &cache.latest_version, behavior)?;"
        ));
        assert!(source.contains("stop_tray_helper_before_windows_deferred_install(&data_dir);"));
        assert!(source.contains("Wait-TargetPathWritable $TargetPath 120"));
        assert!(
            source.contains("Copy-Item -LiteralPath $TargetPath -Destination $backupPath -Force")
        );
        assert!(source.contains("installed CLI reports '$versionOutput' instead of target"));
        assert!(source.contains("restored previous CLI after replacement failure"));
        assert!(source.contains("[System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)"));
        assert!(source.contains("Get-Content -LiteralPath $ProgressPath -Raw -Encoding UTF8"));
        assert!(source.contains("target_version = if ($TargetVersion)"));
        assert!(source.contains(".arg(\"-TargetVersion\")"));
        assert!(source.contains(".arg(\"-Source\")"));
        assert!(source.contains("mark_deferred_install_scheduled();"));
        assert!(source.contains("Write-UpgradeProgress \"completed\" \"Upgrade complete\" $null"));
        assert!(
            source.contains("Write-UpgradeProgress \"failed\" \"Upgrade failed\" $errorMessage")
        );
    }

    #[test]
    fn script_installs_use_the_target_aware_atomic_upgrade_path() {
        let source = include_str!("upgrade.rs");

        assert!(source.contains(
            "InstallMethod::Script => upgrade_manual(&restart_executable, &cache.latest_version)"
        ));
    }

    #[test]
    fn test_glibc_2_38_requires_musl_for_upgrade() {
        assert!(glibc_requires_musl_fallback(Some((2, 38))));
    }

    #[test]
    fn test_glibc_2_39_keeps_gnu_for_upgrade() {
        assert!(!glibc_requires_musl_fallback(Some((2, 39))));
    }

    #[test]
    fn test_unknown_glibc_requires_musl_for_upgrade() {
        assert!(glibc_requires_musl_fallback(None));
    }

    #[test]
    fn test_build_restart_args_with_runtime_info() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 8080,
            socks5_port: Some(1080),
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: Some(false),
            system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "8080",
                "--host",
                "0.0.0.0",
                "--socks5-port",
                "1080",
                "--no-system-proxy"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_default_host_skipped() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: Some(false),
            system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "9900",
                "--no-system-proxy"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_no_runtime_info_uses_default_config_system_proxy() {
        let default_system_proxy = RestartSystemProxyConfig {
            enabled: true,
            bypass: "localhost,127.0.0.1,::1,*.local".to_string(),
        };
        let args = build_restart_args(
            RestartArgsSource::DefaultConfig,
            None,
            Some(&default_system_proxy),
        );
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "--system-proxy",
                "--proxy-bypass",
                "localhost,127.0.0.1,::1,*.local"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_no_runtime_info_preserves_disabled_default_config_system_proxy() {
        let default_system_proxy = RestartSystemProxyConfig {
            enabled: false,
            bypass: "localhost,127.0.0.1,::1,*.local".to_string(),
        };
        let args = build_restart_args(
            RestartArgsSource::DefaultConfig,
            None,
            Some(&default_system_proxy),
        );
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "--no-system-proxy"
            ]
        );
    }

    #[test]
    fn upgrade_restart_port_from_runtime_defaults_to_9900() {
        assert_eq!(restart_ports_from_runtime(None), vec![9900]);
    }

    #[test]
    fn upgrade_restart_ports_from_runtime_uses_runtime_ports() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 18891,
            socks5_port: Some(18892),
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        };

        assert_eq!(restart_ports_from_runtime(Some(&info)), vec![18891, 18892]);
    }

    #[test]
    fn upgrade_restart_executable_uses_install_target_for_manual_install() {
        let target_path = PathBuf::from("/tmp/bifrost-upgrade-target/bin/bifrost");

        assert_eq!(
            restart_executable_for_install_method(&InstallMethod::Manual(target_path.clone()))
                .expect("restart executable"),
            target_path
        );
    }

    #[test]
    fn upgrade_restart_executable_uses_path_for_homebrew_install() {
        assert_eq!(
            restart_executable_for_install_method(&InstallMethod::Homebrew)
                .expect("restart executable"),
            PathBuf::from("bifrost")
        );
    }

    #[test]
    fn test_build_restart_args_no_host() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 8800,
            socks5_port: None,
            host: None,
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: Some(false),
            system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "8800",
                "--no-system-proxy"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_preserves_system_proxy_snapshot() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: Some(false),
            system_proxy_bypass: Some("runtime-bypass-ignored-by-snapshot".to_string()),
        };
        let snapshot = RuntimeSystemProxySnapshot {
            bypass: "localhost,127.0.0.1,*.local".to_string(),
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), Some(&snapshot), None);

        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "9900",
                "--system-proxy",
                "--proxy-bypass",
                "localhost,127.0.0.1,*.local"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_preserves_runtime_system_proxy_request() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: Some(true),
            system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);

        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "9900",
                "--system-proxy",
                "--proxy-bypass",
                "localhost,127.0.0.1,*.local"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_defaults_to_no_system_proxy_for_legacy_runtime() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        };

        let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);

        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "9900",
                "--no-system-proxy"
            ]
        );
    }

    #[test]
    fn upgrade_download_progress_formats_percent_and_size() {
        let started = Instant::now() - Duration::from_secs(2);
        let line = download_progress_line(512, Some(1024), started);

        assert!(line.contains("50.0%"));
        assert!(line.contains("512 B/1.0 KiB"));
        assert!(line.contains("/s"));
    }

    #[test]
    fn upgrade_github_path_url_joins_mirror_and_release_path() {
        assert_eq!(
            github_path_url(
                "https://ghfast.top/https://github.com/",
                "bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
            ),
            "https://ghfast.top/https://github.com/bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
        );
    }

    #[test]
    fn upgrade_mirror_display_name_hides_full_path() {
        assert_eq!(
            mirror_display_name("https://ghfast.top/https://github.com"),
            "ghfast.top"
        );
    }

    #[test]
    fn upgrade_archive_candidates_prefer_xz_then_keep_gz_compatibility() {
        assert_eq!(
            archive_ext_candidates_for_os("macos", true, false),
            vec!["tar.xz", "tar.gz"]
        );
        assert_eq!(
            archive_ext_candidates_for_os("linux", true, true),
            vec!["tar.gz"]
        );
        assert_eq!(
            archive_ext_candidates_for_os("windows", true, false),
            vec!["zip"]
        );
    }

    #[test]
    fn archive_ext_from_path_accepts_supported_upgrade_archives() {
        assert_eq!(
            archive_ext_from_path(Path::new("bifrost.tar.xz")),
            Some("tar.xz")
        );
        assert_eq!(
            archive_ext_from_path(Path::new("bifrost.tar.gz")),
            Some("tar.gz")
        );
        assert_eq!(archive_ext_from_path(Path::new("bifrost.zip")), Some("zip"));
        assert_eq!(archive_ext_from_path(Path::new("bifrost.gz")), None);
    }

    #[test]
    fn upgrade_archive_validation_rejects_invalid_tar_xz_before_extract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("broken.tar.xz");
        std::fs::write(&archive, b"not an xz archive").expect("write archive");

        assert!(validate_downloaded_archive(&archive, "tar.xz").is_err());
    }

    #[test]
    fn upgrade_install_binary_atomically_replaces_existing_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("new-bifrost");
        let target = dir.path().join("bifrost");
        std::fs::write(&source, b"new binary").expect("write source");
        std::fs::write(&target, b"old binary").expect("write target");

        install_binary_atomically(&source, &target).expect("install atomically");

        assert_eq!(std::fs::read(&target).expect("read target"), b"new binary");
        assert!(!unique_temp_binary_path(&target).exists());
        assert_eq!(
            std::fs::read(binary_backup_path(&target)).expect("read backup"),
            b"old binary"
        );

        cleanup_binary_backup(&target);
        assert!(!binary_backup_path(&target).exists());
    }

    #[test]
    fn upgrade_restore_binary_backup_restores_previous_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("new-bifrost");
        let target = dir.path().join("bifrost");
        std::fs::write(&source, b"new binary").expect("write source");
        std::fs::write(&target, b"old binary").expect("write target");

        install_binary_atomically(&source, &target).expect("install atomically");
        assert_eq!(std::fs::read(&target).expect("read target"), b"new binary");

        assert!(restore_binary_backup(&target).expect("restore backup"));
        assert_eq!(std::fs::read(&target).expect("read target"), b"old binary");
        assert!(!binary_backup_path(&target).exists());
    }

    #[test]
    fn upgrade_post_install_skill_messages_cover_all_statuses() {
        assert!(
            post_upgrade_skill_install_message(TimedCommandStatus::Success)
                .contains("installed successfully")
        );
        assert!(
            post_upgrade_skill_install_message(TimedCommandStatus::Failure)
                .contains("retry manually")
        );
        assert!(
            post_upgrade_skill_install_message(TimedCommandStatus::TimedOut).contains("timed out")
        );
    }

    #[test]
    fn upgrade_post_install_skill_args_cover_all_supported_tools() {
        assert_eq!(
            POST_UPGRADE_SKILL_INSTALL_ARGS,
            &["install-skill", "--tool", "all", "-y"]
        );
    }

    #[test]
    fn upgrade_post_install_desktop_app_args_disable_cli_recursion() {
        assert_eq!(
            post_upgrade_desktop_app_args("0.0.145"),
            vec![
                "app",
                "upgrade",
                "--no-cli",
                "--source",
                "cli-upgrade",
                "--version",
                "0.0.145",
                "-y"
            ]
        );
    }

    #[test]
    fn upgrade_desktop_app_install_path_uses_override_dir() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("tempdir");
        let previous = std::env::var_os("BIFROST_APP_INSTALL_DIR");
        std::env::set_var("BIFROST_APP_INSTALL_DIR", temp.path());
        let path = desktop_app_install_candidates()
            .into_iter()
            .next()
            .expect("app path");
        match previous {
            Some(value) => std::env::set_var("BIFROST_APP_INSTALL_DIR", value),
            None => std::env::remove_var("BIFROST_APP_INSTALL_DIR"),
        }

        #[cfg(target_os = "macos")]
        assert_eq!(path, temp.path().join("Bifrost.app"));
        #[cfg(target_os = "windows")]
        assert_eq!(path, temp.path().join("bifrost-desktop.exe"));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(path, temp.path().join("Bifrost"));
    }

    #[test]
    fn upgrade_desktop_app_failure_reason_prefers_stderr() {
        let output = TimedCommandOutput {
            status: TimedCommandStatus::Failure,
            stdout: "stdout detail".to_string(),
            stderr: "stderr detail".to_string(),
        };

        assert_eq!(summarize_command_output(&output), "stderr detail");
    }

    #[test]
    #[cfg(unix)]
    fn upgrade_command_status_with_timeout_reports_success_and_failure() {
        assert_eq!(
            command_status_with_timeout(
                Path::new("/bin/sh"),
                &["-c", "exit 0"],
                Duration::from_secs(1)
            )
            .unwrap(),
            TimedCommandStatus::Success
        );

        assert_eq!(
            command_status_with_timeout(
                Path::new("/bin/sh"),
                &["-c", "exit 7"],
                Duration::from_secs(1)
            )
            .unwrap(),
            TimedCommandStatus::Failure
        );

        assert_eq!(
            command_status_with_timeout_and_heartbeat(
                Path::new("/bin/sh"),
                &["-c", "sleep 0.08"],
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .unwrap(),
            TimedCommandStatus::Success
        );

        let output = command_output_with_timeout_and_heartbeat(
            Path::new("/bin/sh"),
            &["-c".to_string(), "sleep 0.08; echo ready".to_string()],
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .unwrap();
        assert_eq!(output.status, TimedCommandStatus::Success);
        assert_eq!(output.stdout.trim(), "ready");
    }

    #[test]
    #[cfg(unix)]
    fn upgrade_command_status_with_timeout_does_not_block_on_hung_child() {
        let started = Instant::now();
        let status = command_status_with_timeout(
            Path::new("/bin/sh"),
            &["-c", "sleep 5"],
            Duration::from_millis(50),
        )
        .unwrap();

        assert_eq!(status, TimedCommandStatus::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout helper should return promptly"
        );
    }

    #[test]
    #[cfg(unix)]
    fn installed_cli_version_must_match_the_pinned_target() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
        let previous_archive = std::env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE");
        std::env::remove_var("BIFROST_UPGRADE_TEST_ARCHIVE");
        let temp = tempfile::tempdir().expect("tempdir");
        let matching = temp.path().join("matching");
        let stale = temp.path().join("stale");
        let failing = temp.path().join("failing");
        fs::write(&matching, "#!/bin/sh\necho 'bifrost 0.0.156'\n").unwrap();
        fs::write(&stale, "#!/bin/sh\necho 'bifrost 0.0.155'\n").unwrap();
        fs::write(&failing, "#!/bin/sh\necho broken >&2\nexit 7\n").unwrap();
        fs::set_permissions(&matching, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();

        verify_installed_cli_target_version(&matching, "0.0.156")
            .expect("matching installed CLI version");
        assert!(verify_installed_cli_target_version(&stale, "0.0.156").is_err());
        assert!(verify_installed_cli_target_version(&failing, "0.0.156").is_err());

        if let Some(value) = previous_archive {
            std::env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", value);
        }
    }

    #[test]
    fn upgrade_download_tuning_parses_positive_values() {
        let tuning = DownloadTuning {
            connect_timeout_secs: parse_positive_u64(Some("7"), DOWNLOAD_CONNECT_TIMEOUT_SECS),
            download_timeout_secs: parse_positive_u64(Some("90"), DOWNLOAD_TIMEOUT_SECS),
            mirror_probe_timeout_secs: parse_positive_u64(Some("3"), MIRROR_PROBE_TIMEOUT_SECS),
            download_tries: parse_positive_usize(Some("4"), DOWNLOAD_TRIES),
        };

        assert_eq!(
            tuning,
            DownloadTuning {
                connect_timeout_secs: 7,
                download_timeout_secs: 90,
                mirror_probe_timeout_secs: 3,
                download_tries: 4,
            }
        );
    }

    #[test]
    fn upgrade_download_tuning_rejects_invalid_values() {
        assert_eq!(parse_positive_u64(Some("0"), 5), 5);
        assert_eq!(parse_positive_u64(Some("abc"), 5), 5);
        assert_eq!(parse_positive_usize(Some("0"), 2), 2);
        assert_eq!(parse_positive_usize(Some("abc"), 2), 2);
    }

    #[test]
    fn parse_positive_u64_parses_and_trims() {
        assert_eq!(parse_positive_u64(Some("42"), 7), 42);
        assert_eq!(parse_positive_u64(Some(" 5 "), 7), 5);
    }

    #[test]
    fn parse_positive_u64_uses_default_for_zero_invalid_and_none() {
        assert_eq!(parse_positive_u64(Some("0"), 7), 7);
        assert_eq!(parse_positive_u64(Some("abc"), 7), 7);
        assert_eq!(parse_positive_u64(None, 7), 7);
    }

    #[test]
    fn parse_positive_usize_parses_and_trims() {
        assert_eq!(parse_positive_usize(Some("3"), 1), 3);
        assert_eq!(parse_positive_usize(Some(" 8 "), 1), 8);
    }

    #[test]
    fn parse_positive_usize_uses_default_for_zero_invalid_and_none() {
        assert_eq!(parse_positive_usize(Some("0"), 2), 2);
        assert_eq!(parse_positive_usize(Some("zzz"), 2), 2);
        assert_eq!(parse_positive_usize(None, 2), 2);
    }

    #[test]
    fn musl_fallback_triple_is_defined_for_gnu_targets() {
        assert_eq!(
            get_musl_fallback_triple("x86_64-unknown-linux-gnu").as_deref(),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            get_musl_fallback_triple("aarch64-unknown-linux-gnu").as_deref(),
            Some("aarch64-unknown-linux-musl")
        );
    }

    #[test]
    fn musl_fallback_triple_is_none_for_other_targets() {
        assert!(get_musl_fallback_triple("x86_64-apple-darwin").is_none());
    }

    #[test]
    fn human_bytes_formats_small_values() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1), "1 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn human_bytes_formats_larger_units() {
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn download_progress_line_without_total_omits_percentage() {
        let started = Instant::now() - Duration::from_secs(1);
        let line = download_progress_line(2048, None, started);
        assert!(line.contains("Downloading…"));
        assert!(line.contains("2.0 KiB"));
        assert!(!line.contains("%"));
    }

    fn with_mirror_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
        let key = "BIFROST_GITHUB_MIRROR";
        let prev = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let result = f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        result
    }

    #[test]
    fn github_mirror_bases_respects_preferred_env() {
        with_mirror_env(Some("https://example.com/github"), || {
            let bases = github_mirror_bases();
            assert!(!bases.is_empty());
            assert_eq!(bases[0], "https://example.com/github");
            assert!(bases.iter().any(|b| b.contains("github.com")));
        });
    }

    #[test]
    fn mirror_display_name_strips_scheme_and_path() {
        assert_eq!(
            mirror_display_name("https://ghfast.top/https://github.com"),
            "ghfast.top"
        );
        assert_eq!(mirror_display_name("http://foo.bar/"), "foo.bar");
        assert_eq!(mirror_display_name("plain-host"), "plain-host");
    }

    #[test]
    fn github_path_url_normalizes_slashes() {
        assert_eq!(
            github_path_url("https://github.com/", "/owner/repo/releases"),
            "https://github.com/owner/repo/releases"
        );
        assert_eq!(
            github_path_url("https://github.com", "owner/repo"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn version_comparison_is_newer_version_behaviour() {
        assert!(is_newer_version("0.0.1", "0.0.2"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
    }
}
