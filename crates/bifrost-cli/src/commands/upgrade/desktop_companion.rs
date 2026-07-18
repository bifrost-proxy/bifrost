use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DesktopCompanionMode {
    CallerManaged,
    DesktopHandoff,
}

impl DesktopCompanionMode {
    fn progress_source(self) -> &'static str {
        match self {
            Self::CallerManaged => "cli-upgrade",
            Self::DesktopHandoff => "desktop",
        }
    }
}

pub(super) fn desktop_companion_mode(
    windows: bool,
    desktop_process_running: bool,
    desktop_owns_runtime: bool,
) -> DesktopCompanionMode {
    if windows && desktop_process_running && desktop_owns_runtime {
        DesktopCompanionMode::DesktopHandoff
    } else {
        DesktopCompanionMode::CallerManaged
    }
}

#[cfg(target_os = "windows")]
fn installed_desktop_app_is_running(app_path: &Path) -> bool {
    use sysinfo::{ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All);
    system.processes().values().any(|process| {
        process
            .exe()
            .is_some_and(|executable| windows_paths_match(executable, app_path))
    })
}

#[cfg(not(target_os = "windows"))]
fn installed_desktop_app_is_running(_app_path: &Path) -> bool {
    false
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
    desktop_app_install_candidates()
        .into_iter()
        .find(|path| path.exists())
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

    let mode = desktop_companion_mode(
        cfg!(target_os = "windows"),
        installed_desktop_app_is_running(&app_path),
        read_runtime_info().as_ref().is_some_and(|runtime| {
            runtime.start_mode == RuntimeStartMode::Desktop && is_process_running(runtime.pid)
        }),
    );
    let args = post_upgrade_desktop_app_args(target_version, app_path.parent(), mode);
    let environment = match mode {
        DesktopCompanionMode::CallerManaged => Vec::new(),
        DesktopCompanionMode::DesktopHandoff => vec![(DESKTOP_UPGRADE_HANDOFF_ENV, "1")],
    };
    match command_output_with_timeout_and_env(
        executable,
        &args,
        Duration::from_secs(POST_UPGRADE_APP_UPDATE_TIMEOUT_SECS),
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        &environment,
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
