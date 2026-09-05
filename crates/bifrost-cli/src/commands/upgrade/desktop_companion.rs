use super::*;

#[derive(Debug)]
pub(super) struct ChildProgressWatch {
    data_dir: PathBuf,
    target_version: String,
    source: String,
    last_updated_at: Option<String>,
}

impl ChildProgressWatch {
    pub(super) fn new(data_dir: &Path, target_version: &str, source: &str) -> Self {
        let mut watch = Self {
            data_dir: data_dir.to_path_buf(),
            target_version: target_version.to_string(),
            source: source.to_string(),
            last_updated_at: None,
        };
        // The parent transaction may already own a matching Checking record.
        // Seed it without treating that pre-child state as fresh activity.
        let _ = watch.observe_activity();
        watch
    }

    pub(super) fn observe_activity(&mut self) -> bool {
        let path = bifrost_core::upgrade_progress::progress_file_path(&self.data_dir);
        let Ok(content) = fs::read_to_string(path) else {
            return false;
        };
        let Ok(progress) =
            serde_json::from_str::<bifrost_core::upgrade_progress::UpgradeProgress>(&content)
        else {
            return false;
        };
        if !progress.is_active()
            || progress.target_version.as_deref() != Some(self.target_version.as_str())
            || progress.source.as_deref() != Some(self.source.as_str())
            || progress.updated_at.is_empty()
        {
            return false;
        }
        if self.last_updated_at.as_deref() == Some(progress.updated_at.as_str()) {
            return false;
        }
        self.last_updated_at = Some(progress.updated_at);
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DesktopCompanionMode {
    CallerManaged,
    DesktopHandoff,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) const DESKTOP_UPGRADE_SHUTDOWN_ARG: &str = "--bifrost-upgrade-shutdown";
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
        let matches = path_is_within(executable, app_path);
        matches.then(|| (pid.as_u32(), executable.to_path_buf()))
    })
}

#[cfg(any(test, target_os = "macos"))]
fn paths_match_after_canonicalization(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn path_is_within(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root)
        || candidate
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .is_some_and(|(candidate, root)| candidate.starts_with(root))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn running_desktop_shell_process(app_path: &Path) -> Option<(u32, PathBuf)> {
    use sysinfo::{ProcessesToUpdate, System};

    let expected_executable = desktop_shell_executable(app_path);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All);
    select_running_desktop_shell_process(
        &expected_executable,
        system.processes().iter().filter_map(|(pid, process)| {
            process
                .exe()
                .map(|executable| (pid.as_u32(), executable.to_path_buf()))
        }),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn desktop_shell_executable(app_path: &Path) -> PathBuf {
    app_path.join("Contents/MacOS/bifrost-desktop")
}

#[cfg(target_os = "windows")]
pub(super) fn desktop_shell_executable(app_path: &Path) -> PathBuf {
    app_path.to_path_buf()
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn select_running_desktop_shell_process(
    expected_executable: &Path,
    processes: impl IntoIterator<Item = (u32, PathBuf)>,
) -> Option<(u32, PathBuf)> {
    processes.into_iter().find(|(_, executable)| {
        #[cfg(target_os = "windows")]
        {
            windows_paths_match(executable, expected_executable)
        }
        #[cfg(not(target_os = "windows"))]
        {
            paths_match_after_canonicalization(executable, expected_executable)
        }
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn installed_desktop_app_is_running(app_path: &Path) -> bool {
    running_desktop_process(app_path).is_some()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn installed_desktop_app_is_running(_app_path: &Path) -> bool {
    false
}

pub(crate) fn desktop_app_is_running(app_path: &Path) -> bool {
    installed_desktop_app_is_running(app_path)
}

pub(crate) fn shutdown_running_desktop_for_app_upgrade(
    app_path: &Path,
) -> Result<(), BifrostError> {
    request_running_desktop_shutdown(app_path)
}

pub(crate) fn restore_desktop_after_failed_app_upgrade(
    app_path: &Path,
    desktop_was_shut_down: bool,
    original_error: BifrostError,
    relaunch: impl FnOnce(&Path) -> Result<(), BifrostError>,
) -> BifrostError {
    restore_desktop_after_failed_companion(
        app_path,
        desktop_was_shut_down,
        original_error,
        relaunch,
    )
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn request_running_desktop_shutdown(app_path: &Path) -> Result<(), BifrostError> {
    println!(
        "{}",
        "Requesting the running desktop shell to release its installed files...".bright_cyan()
    );
    let Some((pid, _)) = running_desktop_process(app_path) else {
        // The shell exited between discovery and the shutdown request, which
        // already gives the installer the required released-file state.
        return Ok(());
    };

    if let Some((_, executable)) = running_desktop_shell_process(app_path) {
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
    } else {
        eprintln!(
            "{}",
            "⚠ Could not locate the running desktop shell; trying the platform fallback."
                .bright_yellow()
        );
    }

    if wait_for_desktop_app_exit(app_path, DESKTOP_UPGRADE_INTERNAL_SHUTDOWN_TIMEOUT) {
        return Ok(());
    }

    let legacy_result = request_legacy_desktop_shutdown(pid, app_path);
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn request_running_desktop_shutdown(_app_path: &Path) -> Result<(), BifrostError> {
    Ok(())
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
fn request_legacy_desktop_shutdown(pid: u32, app_path: &Path) -> Result<(), BifrostError> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("osascript");
        command.args(macos_desktop_quit_args(app_path));
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let _ = app_path;
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

#[cfg(any(test, target_os = "macos"))]
pub(super) fn macos_desktop_quit_args(app_path: &Path) -> Vec<std::ffi::OsString> {
    [
        "-e",
        "on run argv",
        "-e",
        "set appPath to item 1 of argv",
        "-e",
        "using terms from application \"Finder\"",
        "-e",
        "tell application appPath to quit",
        "-e",
        "end using terms from",
        "-e",
        "end run",
        "--",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .chain(std::iter::once(app_path.as_os_str().to_owned()))
    .collect()
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
        windows_desktop_app_install_candidates_from_roots(
            [
                "LOCALAPPDATA",
                "ProgramFiles",
                "ProgramW6432",
                "ProgramFiles(x86)",
            ]
            .into_iter()
            .filter_map(env::var_os)
            .map(PathBuf::from),
        )
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_desktop_app_install_candidates_from_roots(
    roots: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots {
        let candidate = root.join("Bifrost").join("bifrost-desktop.exe");
        if !candidates
            .iter()
            .any(|existing| windows_paths_match(existing, &candidate))
        {
            candidates.push(candidate);
        }
    }
    candidates
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

// Same schema consumed by Desktop's upgrade_handoff module. Terminal-driven
// upgrades need this marker too: a normal App launch otherwise loses both the
// original owner and a non-default port during the shutdown gap.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn caller_managed_relaunch_marker(
    runtime: &RuntimeInfo,
    app_pid: u32,
    app_path: &Path,
    target_version: &str,
) -> serde_json::Value {
    let desktop_owned = runtime.start_mode == RuntimeStartMode::Desktop;
    serde_json::json!({
        "schema_version": 1,
        "created_at_ms": chrono::Utc::now().timestamp_millis(),
        "old_app_pid": app_pid,
        "old_core_pid": desktop_owned.then_some(runtime.pid),
        "observed_external_core_pid": (!desktop_owned).then_some(runtime.pid),
        "proxy_port": runtime.port,
        "app_target": app_path,
        "target_version": target_version,
    })
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
    if crate::commands::app::installed_desktop_app_is_target_version(&app_path, target_version) {
        println!(
            "{}",
            format!(
                "✓ Bifrost desktop app is already on target version (v{}); leaving its installation and process state unchanged.",
                target_version
            )
            .bright_green()
        );
        return Ok(());
    }
    println!("{}", "Updating Bifrost desktop app...".bright_cyan());

    let desktop_process_running = installed_desktop_app_is_running(&app_path);
    let webview_origin = env_flag(WEBVIEW_UPGRADE_ORIGIN_ENV);
    let mode = desktop_companion_mode(
        cfg!(any(target_os = "macos", target_os = "windows")),
        desktop_process_running,
        webview_origin,
    );
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let desktop_was_shut_down = should_request_desktop_shutdown_before_update(
        true,
        desktop_process_running,
        webview_origin,
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let desktop_was_shut_down = false;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let relaunch_snapshot = if mode == DesktopCompanionMode::CallerManaged {
        read_runtime_info().map(|runtime| {
            let app_pid = running_desktop_shell_process(&app_path).map_or(0, |(pid, _)| pid);
            caller_managed_relaunch_marker(&runtime, app_pid, &app_path, target_version)
        })
    } else {
        None
    };
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if desktop_was_shut_down {
        request_running_desktop_shutdown(&app_path)?;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Some(snapshot) = &relaunch_snapshot {
        let marker_result = get_bifrost_dir().and_then(|data_dir| {
            fs::write(
                data_dir.join("desktop-upgrade-relaunch.json"),
                snapshot.to_string(),
            )
            .map_err(BifrostError::Io)
        });
        marker_result.map_err(|error| {
            restore_desktop_after_failed_companion(
                &app_path,
                desktop_was_shut_down,
                error,
                crate::commands::app::restart_desktop_app,
            )
        })?;
    }
    let args = post_upgrade_desktop_app_args(target_version, app_path.parent(), mode);
    let environment = desktop_companion_environment(mode);
    let parent_lock_data_dir = get_bifrost_dir().map_err(|error| {
        restore_desktop_after_failed_companion(
            &app_path,
            desktop_was_shut_down,
            error,
            crate::commands::app::restart_desktop_app,
        )
    })?;
    let progress_watch = (mode == DesktopCompanionMode::DesktopHandoff).then(|| {
        ChildProgressWatch::new(
            &parent_lock_data_dir,
            target_version,
            mode.progress_source(),
        )
    });
    let result = match command_output_with_timeout_and_env_streaming(
        executable,
        &args,
        Duration::from_secs(POST_UPGRADE_APP_UPDATE_STALL_TIMEOUT_SECS),
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        &environment,
        Some(&parent_lock_data_dir),
        progress_watch,
    ) {
        Ok(output) if output.status == TimedCommandStatus::Success => {
            if mode == DesktopCompanionMode::DesktopHandoff
                && child_scheduled_desktop_handoff(&parent_lock_data_dir, target_version)
            {
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
    };
    result.map_err(|error| {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(snapshot) = &relaunch_snapshot {
            let path = parent_lock_data_dir.join("desktop-upgrade-relaunch.json");
            if fs::read_to_string(&path).ok().as_deref() == Some(snapshot.to_string().as_str()) {
                let _ = fs::remove_file(path);
            }
        }
        restore_desktop_after_failed_companion(
            &app_path,
            desktop_was_shut_down,
            error,
            crate::commands::app::restart_desktop_app,
        )
    })
}

pub(super) fn finish_already_latest_upgrade(
    latest_version: &str,
    behavior: UpgradeBehavior,
) -> Result<(), BifrostError> {
    if !behavior.restart_if_already_latest && !behavior.update_desktop_app {
        return Ok(());
    }
    let install_method = detect_install_method();
    finish_already_latest_upgrade_for_method(latest_version, behavior, &install_method)
}

pub(super) fn finish_already_latest_upgrade_for_method(
    latest_version: &str,
    behavior: UpgradeBehavior,
    install_method: &InstallMethod,
) -> Result<(), BifrostError> {
    let restart_executable = match restart_executable_for_install_method(install_method) {
        Ok(executable) => executable,
        Err(error) if behavior.require_desktop_app_update => return Err(error),
        Err(_) => return Ok(()),
    };
    let should_restart_proxy = behavior.restart_if_already_latest && behavior.restart_proxy;
    if should_restart_proxy {
        println!(
            "{}",
            "  On-disk binary is current; restarting any running proxy so it adopts this version."
                .bright_cyan()
        );
    }
    finish_already_latest_upgrade_steps(
        behavior,
        || update_desktop_companion(&restart_executable, latest_version, behavior),
        || maybe_restart_running_proxy(&restart_executable),
    )
}

pub(super) fn finish_already_latest_upgrade_steps(
    behavior: UpgradeBehavior,
    update_desktop: impl FnOnce() -> Result<(), BifrostError>,
    restart_proxy: impl FnOnce() -> Result<(), BifrostError>,
) -> Result<(), BifrostError> {
    finish_upgrade_steps(
        behavior.restart_if_already_latest && behavior.restart_proxy,
        update_desktop,
        restart_proxy,
    )
}

pub(super) fn finish_installed_upgrade(
    restart_executable: &Path,
    latest_version: &str,
    behavior: UpgradeBehavior,
) -> Result<(), BifrostError> {
    finish_installed_upgrade_steps(
        behavior,
        || update_desktop_companion(restart_executable, latest_version, behavior),
        || maybe_restart_running_proxy(restart_executable),
    )
}

pub(super) fn finish_installed_upgrade_steps(
    behavior: UpgradeBehavior,
    update_desktop: impl FnOnce() -> Result<(), BifrostError>,
    restart_proxy: impl FnOnce() -> Result<(), BifrostError>,
) -> Result<(), BifrostError> {
    finish_upgrade_steps(behavior.restart_proxy, update_desktop, restart_proxy)
}

fn finish_upgrade_steps(
    should_restart_proxy: bool,
    update_desktop: impl FnOnce() -> Result<(), BifrostError>,
    restart_proxy: impl FnOnce() -> Result<(), BifrostError>,
) -> Result<(), BifrostError> {
    // `start -d` returns only after daemon readiness. Desktop can then reuse
    // that server instead of racing it during the old/new process gap. A
    // failed CLI restart must not launch Desktop and silently change ownership.
    if should_restart_proxy {
        restart_proxy()?;
    }
    update_desktop()
}

pub(super) fn restore_desktop_after_failed_companion(
    app_path: &Path,
    desktop_was_shut_down: bool,
    original_error: BifrostError,
    relaunch: impl FnOnce(&Path) -> Result<(), BifrostError>,
) -> BifrostError {
    if !desktop_was_shut_down {
        return original_error;
    }
    eprintln!(
        "{}",
        "⚠ Desktop app update failed after shutdown; relaunching the previous shell."
            .bright_yellow()
    );
    match relaunch(app_path) {
        Ok(()) => {
            eprintln!(
                "{}",
                "✓ Previous Bifrost desktop shell relaunched.".bright_green()
            );
            original_error
        }
        Err(relaunch_error) => BifrostError::Config(format!(
            "{original_error}; previous desktop shell relaunch also failed: {relaunch_error}"
        )),
    }
}

pub(super) fn child_scheduled_desktop_handoff(data_dir: &Path, target_version: &str) -> bool {
    let progress = bifrost_core::upgrade_progress::read_progress(data_dir);
    progress.phase == bifrost_core::upgrade_progress::UpgradePhase::Restarting
        && progress.source.as_deref() == Some("desktop")
        && progress.target_version.as_deref() == Some(target_version)
}
