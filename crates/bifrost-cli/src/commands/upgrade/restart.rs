use super::*;

pub(super) fn prepare_running_proxy_marker(
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

pub(super) fn prepare_running_proxy_marker_with(
    restart_hint: Option<RunningProxyHint>,
    runtime_info: Option<RuntimeInfo>,
    is_running: impl Fn(u32) -> bool,
    listener_pid: impl Fn(u16) -> Option<u32>,
    write_runtime: impl Fn(&RuntimeInfo) -> Result<(), BifrostError>,
) -> Result<(), BifrostError> {
    let Some(hint) = restart_hint else {
        return Ok(());
    };
    if !is_running(hint.pid) {
        return Err(BifrostError::Config(format!(
            "Admin restart hint process {} is no longer running",
            hint.pid
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
    if listener_pid(hint.port) != Some(hint.pid) {
        return Err(BifrostError::Config(format!(
            "Admin restart hint no longer owns the live listener on port {}",
            hint.port
        )));
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
            .or_else(|| Some("127.0.0.1".to_string())),
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

pub(super) fn maybe_restart_running_proxy(restart_executable: &Path) -> Result<(), BifrostError> {
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
    crate::commands::upgrade_background::report_restarting();
    crate::commands::stop::run_stop_for_restart()
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
        .env_remove(crate::commands::start::DETACHED_DAEMON_CHILD_ENV)
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

pub(super) fn cli_owns_runtime_restart(runtime_info: Option<&RuntimeInfo>) -> bool {
    runtime_info
        .map(|info| info.start_mode != RuntimeStartMode::Desktop)
        .unwrap_or(true)
}

#[cfg(windows)]
pub(super) fn maybe_restart_running_proxy_after_windows_deferred_install(
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
    crate::commands::upgrade_background::report_restarting();
    crate::commands::stop::run_stop_for_restart()
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
pub(super) fn stop_tray_helper_before_windows_deferred_install(data_dir: &Path) {
    println!(
        "{}",
        "  Stopping tray helper so Windows can replace bifrost.exe...".bright_cyan()
    );
    crate::commands::tray_launcher::stop_tray_helper(data_dir);
}

#[cfg(windows)]
pub(super) fn schedule_windows_deferred_install(
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
    let progress_source = progress_snapshot.source.unwrap_or_default();
    let publish_progress = !env_flag(PARENT_UPGRADE_LOCK_HELD_ENV);

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
  [string]$Source,
  [int]$PublishProgress
)

$ErrorActionPreference = "Stop"
function Write-UpgradeLog([string]$Message) {
  $timestamp = (Get-Date).ToString("o")
  Add-Content -LiteralPath $LogPath -Value "$timestamp $Message"
}

function Write-UpgradeProgress([string]$Phase, [string]$Message, [string]$ErrorMessage) {
  if ($PublishProgress -eq 0) {
    return
  }
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
        .arg(&deferred_install.target_version)
        .arg("-Source")
        .arg(progress_source)
        .arg("-PublishProgress")
        .arg(if publish_progress { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The helper relaunches bifrost with `start -d`; strip the detached-daemon
        // marker so the relaunched proxy detaches properly instead of running in
        // the foreground (see the unix restart path for the full rationale).
        .env_remove(crate::commands::start::DETACHED_DAEMON_CHILD_ENV);

    command.spawn().map_err(BifrostError::Io)?;
    mark_deferred_install_scheduled();
    println!(
        "{} {}",
        "  Windows upgrade helper log:".dimmed(),
        log_path.display().to_string().dimmed()
    );
    Ok(())
}

pub(super) fn default_restart_system_proxy_config() -> Result<RestartSystemProxyConfig, BifrostError>
{
    let bifrost_dir = get_bifrost_dir()?;
    let config_manager = ConfigManager::new(bifrost_dir)?;
    let config = futures::executor::block_on(config_manager.config());
    Ok(RestartSystemProxyConfig {
        enabled: config.system_proxy.enabled,
        bypass: config.system_proxy.bypass,
    })
}

pub(super) fn restart_ports_from_runtime(
    runtime_info: Option<&crate::process::RuntimeInfo>,
) -> Vec<u16> {
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

pub(super) fn wait_for_restart_ports_release(ports: &[u16]) -> Result<(), BifrostError> {
    for port in ports {
        wait_for_restart_port_release(*port)?;
    }
    Ok(())
}

#[cfg(any(unix, windows))]
pub(super) fn wait_for_restart_port_release(port: u16) -> Result<(), BifrostError> {
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
pub(super) fn wait_for_restart_port_release(_port: u16) -> Result<(), BifrostError> {
    Ok(())
}

pub(super) fn build_restart_args(
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

pub(super) fn update_desktop_companion(
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
