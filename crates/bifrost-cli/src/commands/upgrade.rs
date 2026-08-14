mod desktop_companion;
mod external_worker;
mod install_method;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) use desktop_companion::DESKTOP_UPGRADE_SHUTDOWN_ARG;
use desktop_companion::*;
pub(crate) use desktop_companion::{
    desktop_app_is_running, restore_desktop_after_failed_app_upgrade,
    shutdown_running_desktop_for_app_upgrade,
};
use install_method::*;

use bifrost_core::BifrostError;
use colored::Colorize;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use super::streamed_output::StreamedOutputCapture;
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
const UPGRADE_COMMAND_SPAWN_MAX_ATTEMPTS: u32 = 8;
const UPGRADE_COMMAND_SPAWN_RETRY_BASE_DELAY_MS: u64 = 5;
// Antivirus and indexers can retain handles to a freshly copied executable for
// substantially longer than an ordinary sharing-conflict retry. Keep cleanup
// bounded, but allow roughly 34 seconds so a failed upgrade does not leave a
// staged executable or helper scratch files behind.
const WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS: usize = 480;
#[cfg(windows)]
const WINDOWS_UPGRADE_HANDOFF_READY_MAX_ATTEMPTS: usize = 400;
#[cfg(any(windows, test))]
const WINDOWS_UPGRADE_HANDOFF_READY_POLL_MS: u64 = 25;
#[cfg(unix)]
const TEXT_FILE_BUSY_RAW_OS_ERROR: i32 = 26;
const POST_UPGRADE_SKILL_INSTALL_TIMEOUT_SECS: u64 = 120;
const POST_UPGRADE_SKILL_INSTALL_ARGS: &[&str] = &["install-skill", "--tool", "all", "-y"];
/// Maximum time without a fresh child-owned progress record before the
/// desktop companion is considered stalled. The full download is allowed to
/// take longer as long as it continues publishing progress.
const POST_UPGRADE_APP_UPDATE_STALL_TIMEOUT_SECS: u64 = 600;
const UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS: u64 = 30;
const HOMEBREW_COMMAND_TIMEOUT_SECS: u64 = 600;
const HOMEBREW_METADATA_TIMEOUT_SECS: u64 = 60;
const UPGRADE_TEST_INSTALL_TARGET_ENV: &str = "BIFROST_UPGRADE_TEST_INSTALL_TARGET";
pub(crate) const DESKTOP_MANAGED_SKIP_APP_ENV: &str = "BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP";
pub(crate) const DESKTOP_MANAGED_SKIP_RESTART_ENV: &str =
    "BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART";
pub(crate) const DESKTOP_MANAGED_TARGET_ENV: &str =
    "BIFROST_DESKTOP_MANAGED_UPGRADE_TARGET_VERSION";
pub(crate) const DESKTOP_MANAGED_DEFERRED_STATUS_ENV: &str =
    "BIFROST_DESKTOP_MANAGED_DEFERRED_STATUS";
pub(crate) const DESKTOP_UPGRADE_HANDOFF_ENV: &str = "BIFROST_DESKTOP_UPGRADE_HANDOFF";
pub(crate) const WEBVIEW_UPGRADE_ORIGIN_ENV: &str = "BIFROST_WEBVIEW_UPGRADE_ORIGIN_INTERNAL";
static DEFERRED_INSTALL_SCHEDULED: AtomicBool = AtomicBool::new(false);
static DESKTOP_HANDOFF_SCHEDULED: AtomicBool = AtomicBool::new(false);

pub(super) fn remove_windows_upgrade_file_with_retry(path: &Path) -> Result<(), BifrostError> {
    remove_windows_upgrade_file_with(
        path,
        |candidate| fs::remove_file(candidate),
        thread::sleep,
        cfg!(windows),
    )
}

fn remove_windows_upgrade_file_with<Remove, Sleep>(
    path: &Path,
    mut remove: Remove,
    mut sleep: Sleep,
    retry_sharing_errors: bool,
) -> Result<(), BifrostError>
where
    Remove: FnMut(&Path) -> io::Result<()>,
    Sleep: FnMut(Duration),
{
    for attempt in 0..WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS {
        match remove(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if retry_sharing_errors
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && attempt + 1 < WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS =>
            {
                sleep(Duration::from_millis(25 + (attempt as u64 % 10) * 10));
            }
            Err(error) => {
                return Err(BifrostError::Io(io::Error::new(
                    error.kind(),
                    format!(
                        "failed to clean Windows upgrade artifact {}: {error}",
                        path.display()
                    ),
                )))
            }
        }
    }
    unreachable!("bounded cleanup loop always returns")
}

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

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsDeferredInstall {
    staged_binary: PathBuf,
    target_path: PathBuf,
    target_version: String,
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

pub(crate) fn take_desktop_handoff_scheduled() -> bool {
    DESKTOP_HANDOFF_SCHEDULED.swap(false, Ordering::SeqCst)
}

pub(crate) fn desktop_handoff_scheduled() -> bool {
    DESKTOP_HANDOFF_SCHEDULED.load(Ordering::SeqCst)
}

fn mark_desktop_handoff_scheduled() {
    DESKTOP_HANDOFF_SCHEDULED.store(true, Ordering::SeqCst);
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
        false,
    )
}

fn command_status_with_timeout_streaming(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<TimedCommandStatus, BifrostError> {
    command_status_with_timeout_and_heartbeat(
        program,
        args,
        timeout,
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        true,
    )
}

fn command_status_with_timeout_and_heartbeat(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    heartbeat: Duration,
    stream_output: bool,
) -> Result<TimedCommandStatus, BifrostError> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if stream_output {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn().map_err(BifrostError::Io)?;
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
    command_output_with_timeout_and_env(
        program,
        args,
        timeout,
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        &[],
        None,
    )
}

#[cfg(test)]
fn command_output_with_timeout_and_heartbeat(
    program: &Path,
    args: &[String],
    timeout: Duration,
    heartbeat: Duration,
) -> Result<TimedCommandOutput, BifrostError> {
    command_output_with_timeout_and_env(program, args, timeout, heartbeat, &[], None)
}

fn command_output_with_timeout_and_env_streaming(
    program: &Path,
    args: &[String],
    timeout: Duration,
    heartbeat: Duration,
    environment: &[(&str, &str)],
    parent_upgrade_lock_data_dir: Option<&Path>,
    progress_watch: Option<ChildProgressWatch>,
) -> Result<TimedCommandOutput, BifrostError> {
    let activity_watch = match progress_watch {
        Some(watch) => ChildActivityWatch::StructuredProgress(watch),
        None => ChildActivityWatch::StreamedOutput,
    };
    command_output_with_timeout_and_env_inner(
        program,
        args,
        timeout,
        heartbeat,
        environment,
        parent_upgrade_lock_data_dir,
        activity_watch,
    )
}

fn command_output_with_timeout_and_env(
    program: &Path,
    args: &[String],
    timeout: Duration,
    heartbeat: Duration,
    environment: &[(&str, &str)],
    parent_upgrade_lock_data_dir: Option<&Path>,
) -> Result<TimedCommandOutput, BifrostError> {
    command_output_with_timeout_and_env_inner(
        program,
        args,
        timeout,
        heartbeat,
        environment,
        parent_upgrade_lock_data_dir,
        ChildActivityWatch::None,
    )
}

enum ChildActivityWatch {
    None,
    StreamedOutput,
    StructuredProgress(ChildProgressWatch),
}

fn command_output_with_timeout_and_env_inner(
    program: &Path,
    args: &[String],
    timeout: Duration,
    heartbeat: Duration,
    environment: &[(&str, &str)],
    parent_upgrade_lock_data_dir: Option<&Path>,
    mut activity_watch: ChildActivityWatch,
) -> Result<TimedCommandOutput, BifrostError> {
    let parent_lock_environment = parent_upgrade_lock_data_dir
        .map(super::upgrade_background::parent_upgrade_lock_child_environment)
        .unwrap_or_default();
    let mut output_capture = StreamedOutputCapture::new().map_err(BifrostError::Io)?;
    let mut command = Command::new(program);
    command.env_remove(super::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV);
    command.env_remove(super::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV);
    command
        .args(args)
        .envs(parent_lock_environment)
        .envs(environment.iter().copied())
        .stdin(Stdio::null())
        .stdout(output_capture.stdout_stdio().map_err(BifrostError::Io)?)
        .stderr(output_capture.stderr_stdio().map_err(BifrostError::Io)?);
    let mut child =
        spawn_upgrade_command_with_retry(|| command.spawn()).map_err(BifrostError::Io)?;
    let mut deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;
    let mut next_progress_check = Instant::now();
    let progress_check_interval = heartbeat
        .min(Duration::from_millis(250))
        .max(Duration::from_millis(1));
    let status;
    let stream_output = !matches!(&activity_watch, ChildActivityWatch::None);

    loop {
        let fresh_child_output = stream_output && output_capture.forward_available();
        // Desktop handoff owns a structured progress stream. Its stdout is
        // diagnostic only and must not keep a stalled installer alive.
        let child_output_activity =
            matches!(&activity_watch, ChildActivityWatch::StreamedOutput) && fresh_child_output;
        match child.try_wait() {
            Ok(Some(exit_status)) => {
                status = if exit_status.success() {
                    TimedCommandStatus::Success
                } else {
                    TimedCommandStatus::Failure
                };
                break;
            }
            Ok(None) => {
                let now = Instant::now();
                let mut child_progress_activity = false;
                if now >= next_progress_check {
                    if let ChildActivityWatch::StructuredProgress(watch) = &mut activity_watch {
                        child_progress_activity = watch.observe_activity();
                    }
                    next_progress_check = now + progress_check_interval;
                }
                if child_output_activity || child_progress_activity {
                    deadline = now + timeout;
                }
                if now >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    status = TimedCommandStatus::TimedOut;
                    break;
                }
                // A child-owned progress stream is authoritative. The generic
                // parent heartbeat would overwrite its phase/source and can
                // hide real download activity from the WebView.
                if !matches!(&activity_watch, ChildActivityWatch::StructuredProgress(_))
                    && now >= next_heartbeat
                {
                    super::upgrade_background::report_installing();
                    next_heartbeat = now + heartbeat;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    }

    if stream_output {
        output_capture.forward_available();
    }
    let (stdout, stderr) = output_capture.read_all();

    Ok(TimedCommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn is_retryable_upgrade_command_spawn_error(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(TEXT_FILE_BUSY_RAW_OS_ERROR)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn spawn_upgrade_command_with_retry<T>(mut spawn: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if is_retryable_upgrade_command_spawn_error(&error)
                    && attempt < UPGRADE_COMMAND_SPAWN_MAX_ATTEMPTS =>
            {
                thread::sleep(Duration::from_millis(
                    UPGRADE_COMMAND_SPAWN_RETRY_BASE_DELAY_MS.saturating_mul(u64::from(attempt)),
                ));
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn verify_installed_cli_target_version(
    executable: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
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
    match command_status_with_timeout_streaming(
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

fn verify_installed_cli_target_version_or_restore(
    target_path: &Path,
    target_version: &str,
) -> Result<(), BifrostError> {
    if let Err(error) = verify_installed_cli_target_version(target_path, target_version) {
        restore_binary_backup_best_effort(target_path);
        return Err(error);
    }
    cleanup_binary_backup(target_path);
    Ok(())
}

fn install_binary_atomically(
    new_binary: &Path,
    target_path: &Path,
    _target_version: &str,
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
                target_version: _target_version.to_string(),
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

mod download;
pub(crate) use download::download_progress_line;
use download::*;
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
                crate::commands::upgrade_background::report_download_status(
                    download_source_progress_message(&base, attempt),
                );
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

    install_binary_atomically(&new_binary, target_path, version)
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
    if external_worker::is_external_cli_worker() {
        return external_worker::delegate_upgrade();
    }

    let skip_app = env_flag(DESKTOP_MANAGED_SKIP_APP_ENV);
    let skip_restart = env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV);
    let pinned_target = env::var(DESKTOP_MANAGED_TARGET_ENV).ok();
    let data_dir = get_bifrost_dir()?;
    let managed_child = super::upgrade_background::parent_upgrade_lock_is_valid(&data_dir)
        && skip_app
        && skip_restart
        && pinned_target.is_some();
    let _upgrade_lock = if managed_child {
        None
    } else {
        Some(
            super::upgrade_background::try_acquire_upgrade_lock(&data_dir)?.ok_or_else(|| {
                BifrostError::Config("Upgrade is already running in another process".to_string())
            })?,
        )
    };
    handle_upgrade_inner(
        UpgradeBehavior::interactive(skip_app, skip_restart),
        pinned_target,
    )
}

pub(crate) fn handle_app_managed_upgrade(target_version: String) -> Result<(), BifrostError> {
    handle_upgrade_inner(app_managed_upgrade_behavior(), Some(target_version))
}

fn app_managed_upgrade_behavior() -> UpgradeBehavior {
    // `bifrost app upgrade` suppresses the recursive App companion only. It is
    // still the top-level CLI updater, so a CLI-owned running core must restart
    // onto the newly installed executable.
    let mut behavior = UpgradeBehavior::interactive(true, false);
    // The on-disk CLI can already be at the pinned target while a daemon still
    // serves the previous in-memory version. The App entrypoint must converge
    // that stale runtime just like the Admin background updater does.
    behavior.restart_if_already_latest = true;
    behavior
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn companion_target_without_cli_upgrade<'a>(current: &'a str, discovered: &'a str) -> &'a str {
    if is_newer_version(current, discovered) {
        discovered
    } else {
        current
    }
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
        // A fallback source can legitimately know only an older release. The
        // CLI is already newer in that case, so companion convergence must use
        // the running version instead of downgrading the desktop app.
        return finish_already_latest_upgrade(
            companion_target_without_cli_upgrade(current_version, &cache.latest_version),
            behavior,
        );
    }

    print_update_info(current_version, &cache);

    let install_method = detect_install_method();
    println!(
        "     {} {}",
        "Install method:".dimmed(),
        format!("{}", install_method).bright_white()
    );
    println!();

    let mut restart_executable = restart_executable_for_install_method(&install_method)?;

    let upgrade_result = match &install_method {
        InstallMethod::Homebrew => {
            upgrade_via_homebrew(&cache.latest_version).map(|()| UpgradeInstallOutcome::Installed)
        }
        InstallMethod::Npm | InstallMethod::Pnpm => {
            upgrade_via_node_package_manager(&install_method, &cache.latest_version)
                .map(|()| UpgradeInstallOutcome::Installed)
        }
        // Keep script installs on the selected target instead of re-running an
        // online installer that can drift to a newer release mid-transaction.
        InstallMethod::Script => upgrade_manual(&restart_executable, &cache.latest_version),
        InstallMethod::Manual(path) => upgrade_manual(path, &cache.latest_version),
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

    if install_method.is_node_package_manager() {
        // pnpm stores packages in versioned content-addressed paths, and npm
        // may replace the platform package directory. Resolve again after the
        // package-manager transaction so all post-upgrade work uses the new
        // binary rather than the still-running old executable.
        restart_executable = restart_executable_for_install_method(&install_method)?;
    }

    match upgrade_outcome {
        UpgradeInstallOutcome::Installed => {
            if install_method.is_node_package_manager() {
                // Package-manager installs own their files and rollback. Do
                // not inspect or clean the manual-upgrade backup path inside
                // node_modules; only validate the resolved new executable.
                verify_installed_cli_target_version(&restart_executable, &cache.latest_version)?;
            } else {
                verify_installed_cli_target_version_or_restore(
                    &restart_executable,
                    &cache.latest_version,
                )?;
            }
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

mod restart;
#[cfg_attr(not(windows), allow(unused_imports))]
pub(crate) use restart::handle_windows_upgrade_handoff;
use restart::*;

#[cfg(test)]
mod tests;
