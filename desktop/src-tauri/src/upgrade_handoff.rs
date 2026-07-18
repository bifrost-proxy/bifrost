use super::*;

pub(super) fn desktop_upgrade_relaunch_marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DESKTOP_UPGRADE_RELAUNCH_MARKER_FILE)
}

pub(super) fn desktop_pending_install_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DESKTOP_PENDING_INSTALL_FILE)
}

pub(super) fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub(super) fn read_pending_desktop_install(
    data_dir: &Path,
) -> Result<Option<PendingDesktopInstall>, String> {
    let marker_path = desktop_pending_install_path(data_dir);
    let content = match fs::read_to_string(&marker_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read deferred desktop installer {}: {error}",
                marker_path.display()
            ))
        }
    };
    let pending: PendingDesktopInstall = serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse deferred desktop installer: {error}"))?;
    if pending.schema_version != DESKTOP_PENDING_INSTALL_SCHEMA_VERSION {
        return Err(format!(
            "unsupported deferred desktop installer schema {}",
            pending.schema_version
        ));
    }
    let fresh = current_time_millis()
        .checked_sub(pending.created_at_ms)
        .map(|age_ms| age_ms <= DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS)
        .unwrap_or(true);
    if !fresh {
        return Err("deferred desktop installer marker is stale".to_string());
    }
    let package = Path::new(&pending.package_path);
    if !package.is_file() {
        return Err(format!(
            "deferred desktop installer is missing: {}",
            package.display()
        ));
    }
    let extension = package
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("msi") && !extension.eq_ignore_ascii_case("exe") {
        return Err(format!(
            "unsupported deferred desktop installer type: {}",
            package.display()
        ));
    }
    if pending.target_version.trim().is_empty() {
        return Err("deferred desktop installer target version is empty".to_string());
    }
    Ok(Some(pending))
}

pub(super) fn is_upgrade_relaunch_marker_active(
    marker: &DesktopUpgradeRelaunchMarker,
    now_ms: u64,
) -> bool {
    if marker.schema_version != DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION || marker.proxy_port == 0 {
        return false;
    }

    now_ms
        .checked_sub(marker.created_at_ms)
        .map(|age_ms| age_ms <= DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS)
        .unwrap_or(true)
}

pub(super) fn deferred_desktop_install_version_error(
    marker: &DesktopUpgradeRelaunchMarker,
    current_version: &str,
) -> Option<String> {
    let pending = marker.pending_install.as_ref()?;
    let expected = pending.target_version.trim().trim_start_matches('v');
    let installed = current_version.trim().trim_start_matches('v');
    (installed != expected).then(|| {
        format!(
            "deferred desktop installer target mismatch: expected v{expected}, relaunched v{installed}"
        )
    })
}

pub(super) fn write_upgrade_relaunch_marker(
    data_dir: &Path,
    marker: &DesktopUpgradeRelaunchMarker,
) -> tauri::Result<PathBuf> {
    fs::create_dir_all(data_dir)
        .map_err(|error| anyhow(format!("failed to create desktop data dir: {error}")))?;
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    let content = serde_json::to_string_pretty(marker)
        .map_err(|error| anyhow(format!("failed to encode upgrade relaunch marker: {error}")))?;
    fs::write(&marker_path, format!("{content}\n"))
        .map_err(|error| anyhow(format!("failed to write upgrade relaunch marker: {error}")))?;
    Ok(marker_path)
}

pub(super) fn read_active_upgrade_relaunch_marker(
    data_dir: &Path,
) -> Option<DesktopUpgradeRelaunchMarker> {
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    let content = match fs::read_to_string(&marker_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            append_desktop_bootstrap_log(
                data_dir,
                format!("failed to read desktop upgrade relaunch marker: {error}"),
            );
            return None;
        }
    };

    let marker = match serde_json::from_str::<DesktopUpgradeRelaunchMarker>(&content) {
        Ok(marker) => marker,
        Err(error) => {
            append_desktop_bootstrap_log(
                data_dir,
                format!("discarding invalid desktop upgrade relaunch marker: {error}"),
            );
            let _ = fs::remove_file(&marker_path);
            return None;
        }
    };

    if is_upgrade_relaunch_marker_active(&marker, current_time_millis()) {
        Some(marker)
    } else {
        append_desktop_bootstrap_log(data_dir, "discarding stale desktop upgrade relaunch marker");
        let _ = fs::remove_file(&marker_path);
        None
    }
}

pub(super) fn clear_upgrade_relaunch_marker(data_dir: &Path) {
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    if let Err(error) = fs::remove_file(&marker_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            append_desktop_bootstrap_log(
                data_dir,
                format!("failed to clear desktop upgrade relaunch marker: {error}"),
            );
        }
    }
}

pub(super) fn write_desktop_upgrade_terminal_progress(
    data_dir: &Path,
    phase: UpgradePhase,
    message: &str,
    error: Option<String>,
) {
    let previous = read_progress(data_dir);
    let progress = UpgradeProgress::new(phase, message)
        .with_target(previous.target_version)
        .with_source(Some(
            previous.source.unwrap_or_else(|| "desktop".to_string()),
        ))
        .with_error(error);
    write_progress(data_dir, &progress);
}

pub(super) fn persist_desktop_upgrade_handoff_failure(data_dir: &Path, message: String) -> String {
    append_desktop_bootstrap_log(
        data_dir,
        format!("desktop upgrade restart handoff failed: {message}"),
    );
    write_desktop_upgrade_terminal_progress(
        data_dir,
        UpgradePhase::Failed,
        "Desktop restart handoff failed",
        Some(message.clone()),
    );
    message
}

pub(super) fn may_reuse_existing_backend(
    upgrade_relaunch: Option<&DesktopUpgradeRelaunchMarker>,
) -> bool {
    upgrade_relaunch.is_none()
}

pub(super) fn wait_for_upgrade_handoff_release(
    data_dir: &Path,
    marker: &DesktopUpgradeRelaunchMarker,
) {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "waiting for desktop upgrade handoff release; old_app_pid={} old_core_pid={:?} proxy_port={}",
            marker.old_app_pid, marker.old_core_pid, marker.proxy_port
        ),
    );
    let app_exited =
        wait_for_process_exit(marker.old_app_pid, DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT);
    if let Some(old_core_pid) = marker.old_core_pid {
        let core_exited =
            wait_for_process_exit(old_core_pid, DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT);
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade process wait complete; app_exited={} core_exited={}",
                app_exited, core_exited
            ),
        );
    } else {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade process wait complete; app_exited={} core_exited=unknown",
                app_exited
            ),
        );
    }
    let port_released =
        wait_for_backend_shutdown(marker.proxy_port, DESKTOP_UPGRADE_RELAUNCH_PORT_WAIT);
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "desktop upgrade port wait complete; proxy_port={} released={}",
            marker.proxy_port, port_released
        ),
    );
}

pub(super) fn run_desktop_upgrade_relaunch_helper_from_env() -> bool {
    if !env_flag_enabled(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV) {
        return false;
    }

    let marker_path = match std::env::var_os(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV) {
        Some(path) => PathBuf::from(path),
        None => return true,
    };
    let target = match std::env::var_os(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV) {
        Some(target) => PathBuf::from(target),
        None => return true,
    };
    let data_dir = marker_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let marker = match fs::read_to_string(&marker_path)
        .ok()
        .and_then(|content| serde_json::from_str::<DesktopUpgradeRelaunchMarker>(&content).ok())
    {
        Some(marker) => marker,
        None => return true,
    };

    append_desktop_bootstrap_log(
        &data_dir,
        format!(
            "desktop upgrade relaunch helper started; old_app_pid={} old_core_pid={:?} proxy_port={} target={}",
            marker.old_app_pid,
            marker.old_core_pid,
            marker.proxy_port,
            target.display()
        ),
    );
    wait_for_upgrade_handoff_release(&data_dir, &marker);

    let mut command = relaunch_command_for_target(&target);
    sanitize_desktop_upgrade_relaunch_command(&mut command);
    hide_windows_child_console(&mut command);
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => append_desktop_bootstrap_log(
            &data_dir,
            format!(
                "desktop upgrade relaunch helper opened target; pid={}",
                child.id()
            ),
        ),
        Err(error) => {
            let message = format!("desktop upgrade relaunch helper failed to open target: {error}");
            append_desktop_bootstrap_log(&data_dir, &message);
            write_desktop_upgrade_terminal_progress(
                &data_dir,
                UpgradePhase::Failed,
                "Desktop app restart failed",
                Some(message),
            );
        }
    }

    true
}

pub(super) fn relaunch_command_for_target(target: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        if target.extension().and_then(|extension| extension.to_str()) == Some("app") {
            let mut command = Command::new("open");
            command.arg("-n").arg(target);
            return command;
        }
    }

    Command::new(target)
}

pub(super) fn sanitize_desktop_upgrade_relaunch_command(command: &mut Command) {
    command
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV)
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV)
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV);
}

pub(super) fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    if pid == 0 {
        return true;
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_is_running(pid) {
            return true;
        }

        std::thread::sleep(Duration::from_millis(150));
    }

    !process_is_running(pid)
}

pub(super) fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .stdin(Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }

    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[tauri::command]
pub(super) fn restart_desktop_after_update(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<BackendState>()
        .ok_or_else(|| "desktop backend state is not available".to_string())?;
    let exe = std::env::current_exe().map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to resolve current desktop executable: {error}"),
        )
    })?;

    let old_core_pid = state
        .child
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|child| child.id()));
    let proxy_port = state
        .port
        .lock()
        .map(|guard| *guard)
        .unwrap_or(DEFAULT_BACKEND_PORT);
    let pending_install = read_pending_desktop_install(&state.data_dir).map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to prepare deferred desktop install: {error}"),
        )
    })?;
    let app_target = desktop_relaunch_target(&exe);
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION,
        created_at_ms: current_time_millis(),
        old_app_pid: std::process::id(),
        old_core_pid,
        proxy_port,
        app_target: app_target.to_string_lossy().into_owned(),
        pending_install,
    };
    let marker_path = write_upgrade_relaunch_marker(&state.data_dir, &marker).map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to prepare desktop upgrade relaunch: {error}"),
        )
    })?;
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop upgrade relaunch marker written; old_app_pid={} old_core_pid={:?} proxy_port={} target={} deferred_install={}",
            marker.old_app_pid,
            marker.old_core_pid,
            marker.proxy_port,
            marker.app_target,
            marker.pending_install.is_some()
        ),
    );
    let helper = spawn_desktop_upgrade_relaunch_helper(&exe, &marker_path, &app_target, &marker)
        .map_err(|error| {
            clear_upgrade_relaunch_marker(&state.data_dir);
            persist_desktop_upgrade_handoff_failure(
                &state.data_dir,
                format!("failed to spawn desktop upgrade relaunch helper: {error}"),
            )
        })?;
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop upgrade relaunch helper spawned; pid={} target={}",
            helper.id(),
            marker.app_target
        ),
    );
    app.exit(0);
    Ok(())
}

pub(super) fn desktop_relaunch_target(exe: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(app_bundle) = macos_app_bundle_from_exe_path(exe) {
            return app_bundle;
        }
    }

    exe.to_path_buf()
}

pub(super) fn spawn_desktop_upgrade_relaunch_helper(
    exe: &Path,
    marker_path: &Path,
    target: &Path,
    marker: &DesktopUpgradeRelaunchMarker,
) -> tauri::Result<Child> {
    #[cfg(target_os = "windows")]
    if marker.pending_install.is_some() {
        return spawn_windows_desktop_upgrade_handoff(marker_path, target);
    }

    #[cfg(not(target_os = "windows"))]
    let _ = marker;
    let mut command = Command::new(exe);
    command
        .env(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV, "1")
        .env(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV, marker_path)
        .env(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV, target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_windows_child_console(&mut command);
    command
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn relaunch helper: {error}")))
}

#[cfg(target_os = "windows")]
pub(super) fn spawn_windows_desktop_upgrade_handoff(
    marker_path: &Path,
    target: &Path,
) -> tauri::Result<Child> {
    let data_dir = marker_path
        .parent()
        .ok_or_else(|| anyhow("desktop upgrade marker has no parent directory".to_string()))?;
    let script_path = data_dir.join(format!(
        ".desktop-upgrade-handoff-{}.ps1",
        std::process::id()
    ));
    fs::write(&script_path, WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT)
        .map_err(|error| anyhow(format!("failed to write Windows upgrade handoff: {error}")))?;
    let mut command = windows_desktop_upgrade_handoff_command(&script_path, marker_path, target);
    hide_windows_child_console(&mut command);
    let result = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn Windows upgrade handoff: {error}")));
    if result.is_err() {
        let _ = fs::remove_file(script_path);
    }
    result
}

#[cfg(any(target_os = "windows", test))]
pub(super) const WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT: &str = r#"
param([string]$MarkerPath, [string]$TargetPath)
$ErrorActionPreference = "Stop"
$dataDir = Split-Path -Parent $MarkerPath
$progressPath = Join-Path $dataDir "upgrade-progress.json"
$pendingPath = Join-Path $dataDir "desktop-upgrade-pending-install.json"
$bootstrapLog = Join-Path $dataDir "logs\desktop-bootstrap.log"

function Write-BootstrapLog([string]$Message) {
  $logDir = Split-Path -Parent $bootstrapLog
  New-Item -ItemType Directory -Path $logDir -Force | Out-Null
  Add-Content -LiteralPath $bootstrapLog -Value "$(Get-Date -Format o) $Message" -Encoding UTF8
}

function Write-Progress([string]$Phase, [string]$Message, [string]$TargetVersion, [string]$ErrorMessage) {
  $payload = [ordered]@{
    phase = $Phase
    percent = $null
    message = $Message
    target_version = $TargetVersion
    source = "desktop"
    error = if ($ErrorMessage) { $ErrorMessage } else { $null }
    updated_at = (Get-Date).ToUniversalTime().ToString("o")
  }
  $tmpPath = "$progressPath.tmp.$PID"
  $json = $payload | ConvertTo-Json -Compress
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)
  Move-Item -LiteralPath $tmpPath -Destination $progressPath -Force
}

function Wait-ForProcessExit([uint32]$ProcessId, [string]$Label) {
  if ($ProcessId -eq 0) { return }
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  while ([DateTime]::UtcNow -lt $deadline) {
    if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { return }
    Start-Sleep -Milliseconds 200
  }
  throw "$Label process $ProcessId did not exit within 30 seconds"
}

$targetVersion = $null
try {
  $marker = Get-Content -LiteralPath $MarkerPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($marker.pending_install) { $targetVersion = [string]$marker.pending_install.target_version }
  Wait-ForProcessExit ([uint32]$marker.old_app_pid) "desktop app"
  if ($null -ne $marker.old_core_pid) {
    Wait-ForProcessExit ([uint32]$marker.old_core_pid) "desktop core"
  }

  if ($marker.pending_install) {
    $packagePath = [string]$marker.pending_install.package_path
    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
      throw "deferred desktop installer is missing: $packagePath"
    }
    Write-Progress "installing" "Installing desktop app after shutdown..." $targetVersion $null
    Write-BootstrapLog "starting deferred desktop installer; target_version=$targetVersion package=$packagePath"
    $extension = [System.IO.Path]::GetExtension($packagePath).ToLowerInvariant()
    if ($extension -eq ".msi") {
      $installerLog = Join-Path $dataDir "logs\desktop-upgrade-installer.log"
      $quotedPackagePath = '"' + $packagePath + '"'
      $quotedInstallerLog = '"' + $installerLog + '"'
      $installerArgs = @("/i", $quotedPackagePath, "/qn", "/norestart", "ALLUSERS=2", "MSIINSTALLPERUSER=1", "/l*v", $quotedInstallerLog)
      $installer = Start-Process -FilePath "msiexec.exe" -ArgumentList $installerArgs -PassThru
    } elseif ($extension -eq ".exe") {
      $installer = Start-Process -FilePath $packagePath -ArgumentList @("/S") -PassThru
    } else {
      throw "unsupported deferred desktop installer type: $packagePath"
    }

    $deadline = [DateTime]::UtcNow.AddMinutes(10)
    while (-not $installer.WaitForExit(30000)) {
      if ([DateTime]::UtcNow -ge $deadline) {
        try { $installer.Kill() } catch {}
        throw "desktop installer timed out after 600 seconds"
      }
      Write-Progress "installing" "Installing desktop app after shutdown..." $targetVersion $null
    }
    if ($installer.ExitCode -notin @(0, 1641, 3010)) {
      throw "desktop installer exited with code $($installer.ExitCode)"
    }
    Remove-Item -LiteralPath $pendingPath -Force -ErrorAction SilentlyContinue
    if ([bool]$marker.pending_install.package_owned_by_updater) {
      Remove-Item -LiteralPath $packagePath -Force -ErrorAction SilentlyContinue
    }
    Write-BootstrapLog "deferred desktop installer completed; target_version=$targetVersion"
  }

  Start-Process -FilePath $TargetPath | Out-Null
  Write-BootstrapLog "desktop upgrade handoff opened target: $TargetPath"
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
} catch {
  $message = "desktop upgrade handoff failed: $($_.Exception.Message)"
  Remove-Item -LiteralPath $MarkerPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $pendingPath -Force -ErrorAction SilentlyContinue
  Write-Progress "failed" "Desktop app install or restart failed" $targetVersion $message
  Write-BootstrapLog $message
  try { Start-Process -FilePath $TargetPath | Out-Null } catch {}
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
  exit 1
}
"#;

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_desktop_upgrade_handoff_command(
    script_path: &Path,
    marker_path: &Path,
    target: &Path,
) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(script_path)
        .args(["-MarkerPath"])
        .arg(marker_path)
        .arg("-TargetPath")
        .arg(target);
    command
}

#[cfg(target_os = "macos")]
pub(super) fn macos_app_bundle_from_exe_path(exe_path: &Path) -> Option<PathBuf> {
    exe_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("Bifrost.app"))
        .map(Path::to_path_buf)
}
