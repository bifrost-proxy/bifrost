use super::*;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug)]
pub(super) struct DesktopInstallCommandOutput {
    pub(super) status: std::process::ExitStatus,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn run_desktop_install_command(
    command: Command,
    target_version: &str,
    progress_source: &str,
) -> Result<std::process::ExitStatus, BifrostError> {
    Ok(run_desktop_install_command_output(command, target_version, progress_source)?.status)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn run_desktop_install_command_output(
    command: Command,
    target_version: &str,
    progress_source: &str,
) -> Result<DesktopInstallCommandOutput, BifrostError> {
    run_desktop_install_command_output_with_timeout(
        command,
        target_version,
        progress_source,
        DESKTOP_INSTALL_COMMAND_TIMEOUT,
        DESKTOP_INSTALL_COMMAND_HEARTBEAT,
    )
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn run_desktop_install_command_output_with_timeout(
    mut command: Command,
    target_version: &str,
    progress_source: &str,
    timeout: Duration,
    heartbeat: Duration,
) -> Result<DesktopInstallCommandOutput, BifrostError> {
    let mut stdout =
        tempfile::tempfile().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
    let mut stderr =
        tempfile::tempfile().map_err(|error| BifrostError::Io(std::io::Error::other(error)))?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()
        .map_err(BifrostError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut next_heartbeat = Instant::now() + heartbeat;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BifrostError::Config(format!(
                    "desktop package installer timed out after {} seconds",
                    timeout.as_secs_f64()
                )));
            }
            Ok(None) => {
                if Instant::now() >= next_heartbeat {
                    write_app_progress(
                        UpgradePhase::Installing,
                        "Installing desktop app…",
                        Some(target_version.to_string()),
                        progress_source,
                        None,
                        None,
                    );
                    next_heartbeat = Instant::now() + heartbeat;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    };
    let _ = stdout.seek(SeekFrom::Start(0));
    let _ = stderr.seek(SeekFrom::Start(0));
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let _ = stdout.read_to_string(&mut stdout_text);
    let _ = stderr.read_to_string(&mut stderr_text);
    Ok(DesktopInstallCommandOutput {
        status,
        stdout: stdout_text,
        stderr: stderr_text,
    })
}
