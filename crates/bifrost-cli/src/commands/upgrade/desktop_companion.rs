use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DesktopCompanionMode {
    CallerManaged,
    DesktopHandoff,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) const DESKTOP_UPGRADE_SHUTDOWN_ARG: &str = "--bifrost-upgrade-shutdown";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DESKTOP_UPGRADE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DESKTOP_UPGRADE_INTERNAL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

impl DesktopCompanionMode {
    fn progress_source(self) -> &'static str {
        match self {
            Self::CallerManaged => "cli-upgrade",
            Self::DesktopHandoff => "desktop",
        }
    }
}

pub(super) fn desktop_companion_mode(
    desktop_handoff_supported: bool,
    desktop_process_running: bool,
    webview_origin: bool,
) -> DesktopCompanionMode {
    if desktop_handoff_supported && desktop_process_running && webview_origin {
        DesktopCompanionMode::DesktopHandoff
    } else {
        DesktopCompanionMode::CallerManaged
    }
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn should_request_desktop_shutdown_before_update(
    desktop_handoff_supported: bool,
    desktop_process_running: bool,
    webview_origin: bool,
) -> bool {
    desktop_handoff_supported && desktop_process_running && !webview_origin
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn running_desktop_process(app_path: &Path) -> Option<(u32, PathBuf)> {
    use sysinfo::{ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All);
    system.processes().iter().find_map(|(pid, process)| {
        let executable = process.exe()?;
        #[cfg(target_os = "windows")]
        let matches = windows_paths_match(executable, app_path);
        #[cfg(target_os = "macos")]
        let matches = executable.starts_with(app_path);
        matches.then(|| (pid.as_u32(), executable.to_path_buf()))
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn installed_desktop_app_is_running(app_path: &Path) -> bool {
    running_desktop_process(app_path).is_some()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn installed_desktop_app_is_running(_app_path: &Path) -> bool {
    false
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn request_running_desktop_shutdown(app_path: &Path) -> Result<(), BifrostError> {
    println!(
        "{}",
        "Requesting the running desktop shell to release its installed files...".bright_cyan()
    );
    let Some((pid, executable)) = running_desktop_process(app_path) else {
        // The shell exited between discovery and the shutdown request, which
        // already gives the installer the required released-file state.
        return Ok(());
    };
    let internal_result = command_output_with_timeout(
        &executable,
        &[DESKTOP_UPGRADE_SHUTDOWN_ARG.to_string()],
        DESKTOP_UPGRADE_INTERNAL_SHUTDOWN_TIMEOUT,
    );
    if let Ok(output) = &internal_result {
        if output.status != TimedCommandStatus::Success {
            eprintln!(
                "{} {}",
                "⚠ Desktop shell did not accept the internal shutdown request; trying the platform fallback."
                    .bright_yellow(),
                summarize_command_output(output).dimmed()
            );
        }
    } else if let Err(error) = &internal_result {
        eprintln!(
            "{} {}",
            "⚠ Could not send the internal desktop shutdown request; trying the platform fallback."
                .bright_yellow(),
            error.to_string().dimmed()
        );
    }

    if wait_for_desktop_app_exit(app_path, DESKTOP_UPGRADE_INTERNAL_SHUTDOWN_TIMEOUT) {
        return Ok(());
    }

    let legacy_result = request_legacy_desktop_shutdown(pid);
    if wait_for_desktop_app_exit(app_path, DESKTOP_UPGRADE_SHUTDOWN_TIMEOUT) {
        return Ok(());
    }
    let fallback_detail = legacy_result
        .err()
        .map(|error| format!("; platform fallback also reported: {error}"))
        .unwrap_or_default();
    Err(BifrostError::Config(format!(
        "desktop shell did not exit within {} seconds; refusing to replace files that may still be locked{}",
        DESKTOP_UPGRADE_SHUTDOWN_TIMEOUT.as_secs(),
        fallback_detail
    )))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn wait_for_desktop_app_exit(app_path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !installed_desktop_app_is_running(app_path) {
            return true;
        }
        thread::sleep(Duration::from_millis(150));
    }
    !installed_desktop_app_is_running(app_path)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn request_legacy_desktop_shutdown(pid: u32) -> Result<(), BifrostError> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("osascript");
        command.args(["-e", "tell application \"Bifrost\" to quit"]);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T"]);
        command
    };
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(BifrostError::Io)?;
    if !status.success() {
        return Err(BifrostError::Config(format!(
            "platform desktop shutdown request for PID {pid} failed with status {status}"
        )));
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_paths_match(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .to_lowercase()
    }
    normalized(left) == normalized(right)
}

pub(super) fn installed_desktop_app_path() -> Option<PathBuf> {
    select_installed_desktop_app_path(desktop_app_install_candidates(), |path| {
        installed_desktop_app_is_running(path)
    })
}

pub(super) fn select_installed_desktop_app_path(
    candidates: impl IntoIterator<Item = PathBuf>,
    is_running: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let candidates: Vec<_> = candidates.into_iter().collect();
    candidates
        .iter()
        .find(|path| is_running(path))
        .cloned()
        .or_else(|| candidates.into_iter().find(|path| path.exists()))
}

pub(super) fn desktop_app_install_candidates() -> Vec<PathBuf> {
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

pub(super) fn resolve_desktop_app_path(app_dir: &Path) -> PathBuf {
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

pub(super) fn post_upgrade_desktop_app_args(
    target_version: &str,
    app_dir: Option<&Path>,
    mode: DesktopCompanionMode,
) -> Vec<String> {
    let mut args = vec![
        "app".to_string(),
        "upgrade".to_string(),
        "--no-cli".to_string(),
        "--source".to_string(),
        mode.progress_source().to_string(),
        "--version".to_string(),
        target_version.to_string(),
    ];
    if let Some(app_dir) = app_dir {
        args.push("--app-dir".to_string());
        args.push(app_dir.to_string_lossy().into_owned());
    }
    args.push("-y".to_string());
    args
}

pub(super) fn desktop_companion_environment(
    mode: DesktopCompanionMode,
) -> Vec<(&'static str, &'static str)> {
    let mut environment = Vec::new();
    if mode == DesktopCompanionMode::DesktopHandoff {
        environment.push((DESKTOP_UPGRADE_HANDOFF_ENV, "1"));
    }
    environment
}

pub(super) fn update_desktop_app_after_upgrade_best_effort(
    executable: &Path,
    target_version: &str,
) {
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

pub(super) fn update_desktop_app_after_upgrade(
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

    let desktop_process_running = installed_desktop_app_is_running(&app_path);
    let webview_origin = env_flag(WEBVIEW_UPGRADE_ORIGIN_ENV);
    let mode = desktop_companion_mode(
        cfg!(any(target_os = "macos", target_os = "windows")),
        desktop_process_running,
        webview_origin,
    );
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if should_request_desktop_shutdown_before_update(true, desktop_process_running, webview_origin)
    {
        request_running_desktop_shutdown(&app_path)?;
    }
    let args = post_upgrade_desktop_app_args(target_version, app_path.parent(), mode);
    let environment = desktop_companion_environment(mode);
    let parent_lock_data_dir = get_bifrost_dir()?;
    match command_output_with_timeout_and_env(
        executable,
        &args,
        Duration::from_secs(POST_UPGRADE_APP_UPDATE_TIMEOUT_SECS),
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        &environment,
        Some(&parent_lock_data_dir),
    ) {
        Ok(output) if output.status == TimedCommandStatus::Success => {
            if mode == DesktopCompanionMode::DesktopHandoff {
                mark_desktop_handoff_scheduled();
            }
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
