use bifrost_storage::set_data_dir;

use crate::config::get_bifrost_dir;
use crate::process::{is_process_running, read_pid, read_runtime_info, remove_pid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopSystemProxyMode {
    ForegroundCleanupBeforeStop,
    PreserveForRestart,
}

impl StopSystemProxyMode {
    fn shutdown_mode(self) -> bifrost_core::SystemProxyShutdownMode {
        match self {
            Self::ForegroundCleanupBeforeStop => {
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
            }
            Self::PreserveForRestart => bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        }
    }

    fn should_cleanup_in_stop_process(self) -> bool {
        matches!(self, Self::ForegroundCleanupBeforeStop)
    }

    fn marker_write_must_succeed(self) -> bool {
        matches!(self, Self::PreserveForRestart)
    }
}

fn host_matches_system_proxy(proxy_host: &str, runtime_host: &str) -> bool {
    let proxy_host = proxy_host
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let runtime_host = runtime_host
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();

    if proxy_host == runtime_host {
        return true;
    }

    matches!(
        (proxy_host.as_str(), runtime_host.as_str()),
        ("localhost", "127.0.0.1")
            | ("127.0.0.1", "localhost")
            | ("::1", "127.0.0.1")
            | ("127.0.0.1", "::1")
            | ("::1", "localhost")
            | ("localhost", "::1")
    )
}

fn runtime_system_proxy_host(runtime_host: Option<&str>) -> &str {
    match runtime_host {
        Some("0.0.0.0") | Some("[::]") | Some("::") | None | Some("") => "127.0.0.1",
        Some(host) => host,
    }
}

fn cleanup_proxy_state(bifrost_dir: &std::path::Path) -> bool {
    let mut success = true;
    if let Err(e) = bifrost_core::SystemProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover system proxy: {}", e);
        success = false;
    }

    success &= ensure_system_proxy_disabled(bifrost_dir);

    if let Err(e) = bifrost_core::ShellProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover CLI proxy: {}", e);
        success = false;
    }
    let mut shell_manager = bifrost_core::ShellProxyManager::new(bifrost_dir.to_path_buf());
    let _ = shell_manager.disable_persistent();
    success
}

fn write_shutdown_marker_for_stop(
    bifrost_dir: &std::path::Path,
    mode: StopSystemProxyMode,
) -> bifrost_core::Result<()> {
    if !bifrost_core::SystemProxyManager::is_supported() {
        return Ok(());
    }

    let marker_result =
        bifrost_core::write_system_proxy_shutdown_mode(bifrost_dir, mode.shutdown_mode());
    if marker_result.is_ok() || mode.marker_write_must_succeed() {
        return marker_result;
    }

    if let Err(error) = marker_result {
        eprintln!(
            "Warning: failed to write system proxy shutdown marker; continuing stop after foreground cleanup: {error}"
        );
        tracing::warn!(
            target: "bifrost_cli::stop",
            error = %error,
            "failed to write best-effort foreground cleanup marker"
        );
    }
    Ok(())
}

fn ensure_system_proxy_disabled(bifrost_dir: &std::path::Path) -> bool {
    if !bifrost_core::SystemProxyManager::is_supported() {
        return true;
    }

    let runtime_info = read_runtime_info();

    let current = match bifrost_core::SystemProxyManager::get_current() {
        Ok(c) => c,
        Err(error) => {
            eprintln!("Failed to inspect current system proxy: {error}");
            return false;
        }
    };

    if !current.enable {
        return true;
    }

    let is_bifrost_proxy = runtime_info.as_ref().is_some_and(|info| {
        current.port == info.port
            && host_matches_system_proxy(
                &current.host,
                runtime_system_proxy_host(info.host.as_deref()),
            )
    });

    if !is_bifrost_proxy {
        return true;
    }

    let mut manager = bifrost_core::SystemProxyManager::new(bifrost_dir.to_path_buf());
    match manager.disable_if_matches(&current.host, current.port) {
        Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
            println!("System proxy disabled.");
            true
        }
        Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => true,
        Ok(bifrost_core::SystemProxyDisableOutcome::OwnedByOther) => {
            eprintln!("System proxy points to another proxy; leaving it unchanged.");
            false
        }
        Err(e) => {
            eprintln!("Failed to disable system proxy: {}", e);
            false
        }
    }
}

pub fn run_stop() -> bifrost_core::Result<()> {
    run_stop_with_system_proxy_mode(StopSystemProxyMode::ForegroundCleanupBeforeStop)
}

pub fn run_stop_for_restart() -> bifrost_core::Result<()> {
    run_stop_with_system_proxy_mode(StopSystemProxyMode::PreserveForRestart)
}

fn run_stop_with_system_proxy_mode(
    system_proxy_mode: StopSystemProxyMode,
) -> bifrost_core::Result<()> {
    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());

    let pid = read_pid().ok_or_else(|| {
        bifrost_core::BifrostError::NotFound("No PID file found. Is the proxy running?".to_string())
    })?;

    if !is_process_running(pid) {
        if system_proxy_mode.should_cleanup_in_stop_process() {
            let _ = cleanup_proxy_state(&bifrost_dir);
        }
        remove_pid()?;
        println!("Bifrost proxy is not running (stale PID file removed).");
        return Ok(());
    }

    write_shutdown_marker_for_stop(&bifrost_dir, system_proxy_mode)?;

    if system_proxy_mode.should_cleanup_in_stop_process() {
        println!("Cleaning system proxy before stopping Bifrost proxy...");
        if !cleanup_proxy_state(&bifrost_dir) {
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(&bifrost_dir);
            return Err(bifrost_core::BifrostError::Config(
                "System proxy cleanup failed before stopping Bifrost; service is still running to avoid breaking network connectivity.".to_string(),
            ));
        }
    }

    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        println!("Stopping Bifrost proxy (PID: {})...", pid);
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM).map_err(|e| {
            bifrost_core::BifrostError::Config(format!("Failed to send SIGTERM: {}", e))
        })?;

        // Poll for the process to exit. The graceful shutdown usually completes
        // within tens of milliseconds, so probe with a fine 5ms granularity for
        // the first ~200ms to make the command "notice" the exit promptly, then
        // degrade to a coarse 100ms interval to keep the overall ~30s budget
        // (with SIGKILL escalation near the 25s mark) without busy-spinning.
        const FAST_PROBE_INTERVAL_MS: u64 = 5;
        const FAST_PROBE_COUNT: u64 = 40; // 40 * 5ms = first ~200ms covered finely
        const SLOW_PROBE_INTERVAL_MS: u64 = 100;
        const SIGKILL_DEADLINE_MS: u64 = 25_000;
        const TOTAL_BUDGET_MS: u64 = 30_000;

        let mut elapsed_ms: u64 = 0;
        let mut sigkill_sent = false;
        while elapsed_ms < TOTAL_BUDGET_MS {
            let interval_ms = if elapsed_ms < FAST_PROBE_COUNT * FAST_PROBE_INTERVAL_MS {
                FAST_PROBE_INTERVAL_MS
            } else {
                SLOW_PROBE_INTERVAL_MS
            };
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            elapsed_ms += interval_ms;

            if !is_process_running(pid) {
                remove_pid()?;
                println!("Bifrost proxy stopped.");
                return Ok(());
            }

            if !sigkill_sent && elapsed_ms >= SIGKILL_DEADLINE_MS {
                println!("Sending SIGKILL...");
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
                sigkill_sent = true;
            }
        }

        remove_pid()?;
        println!("Bifrost proxy stopped (forced).");
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        };

        println!("Stopping Bifrost proxy (PID: {})...", pid);

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid as u32) };

        if handle.is_null() {
            eprintln!(
                "Failed to open process (PID: {}). It may have already exited.",
                pid
            );
            remove_pid()?;
        } else {
            let terminated = unsafe { TerminateProcess(handle, 1) };
            if terminated != 0 {
                unsafe {
                    WaitForSingleObject(handle, 5000);
                }
                remove_pid()?;
                println!("Bifrost proxy stopped.");
            } else {
                eprintln!("Failed to terminate process (PID: {}).", pid);
            }
            unsafe {
                CloseHandle(handle);
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        println!("Stop command is not supported on this platform.");
        println!("Please terminate the process manually (PID: {}).", pid);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_matching_accepts_loopback_aliases() {
        assert!(host_matches_system_proxy("localhost", "127.0.0.1"));
        assert!(host_matches_system_proxy("[::1]", "localhost"));
    }

    #[test]
    fn runtime_system_proxy_host_maps_wildcard_to_loopback() {
        assert_eq!(runtime_system_proxy_host(Some("0.0.0.0")), "127.0.0.1");
        assert_eq!(runtime_system_proxy_host(Some("::")), "127.0.0.1");
        assert_eq!(
            runtime_system_proxy_host(Some("192.168.1.2")),
            "192.168.1.2"
        );
    }
}
