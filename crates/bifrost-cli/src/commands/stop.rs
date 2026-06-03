use bifrost_storage::set_data_dir;

use crate::config::get_bifrost_dir;
use crate::process::{is_process_running, read_pid, read_runtime_info, remove_pid};

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

fn cleanup_proxy_state(bifrost_dir: &std::path::Path) {
    if let Err(e) = bifrost_core::SystemProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover system proxy: {}", e);
    }

    ensure_system_proxy_disabled(bifrost_dir);

    if let Err(e) = bifrost_core::ShellProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover CLI proxy: {}", e);
    }
    let mut shell_manager = bifrost_core::ShellProxyManager::new(bifrost_dir.to_path_buf());
    let _ = shell_manager.disable_persistent();
}

fn ensure_system_proxy_disabled(bifrost_dir: &std::path::Path) {
    if !bifrost_core::SystemProxyManager::is_supported() {
        return;
    }

    let runtime_info = read_runtime_info();

    let current = match bifrost_core::SystemProxyManager::get_current() {
        Ok(c) => c,
        Err(_) => return,
    };

    if !current.enable {
        return;
    }

    let is_bifrost_proxy = runtime_info.as_ref().is_some_and(|info| {
        current.port == info.port
            && host_matches_system_proxy(
                &current.host,
                runtime_system_proxy_host(info.host.as_deref()),
            )
    });

    if !is_bifrost_proxy {
        return;
    }

    let mut manager = bifrost_core::SystemProxyManager::new(bifrost_dir.to_path_buf());
    match manager.disable_if_matches(&current.host, current.port) {
        Ok(bifrost_core::SystemProxyDisableOutcome::Disabled) => {
            println!("System proxy disabled.");
        }
        Ok(bifrost_core::SystemProxyDisableOutcome::NotEnabled) => {}
        Ok(bifrost_core::SystemProxyDisableOutcome::OwnedByOther) => {}
        Err(e) => eprintln!("Failed to disable system proxy: {}", e),
    }
}

pub fn run_stop() -> bifrost_core::Result<()> {
    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());

    let pid = read_pid().ok_or_else(|| {
        bifrost_core::BifrostError::NotFound("No PID file found. Is the proxy running?".to_string())
    })?;

    if !is_process_running(pid) {
        cleanup_proxy_state(&bifrost_dir);
        remove_pid()?;
        println!("Bifrost proxy is not running (stale PID file removed).");
        return Ok(());
    }

    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        println!("Stopping Bifrost proxy (PID: {})...", pid);
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM).map_err(|e| {
            bifrost_core::BifrostError::Config(format!("Failed to send SIGTERM: {}", e))
        })?;

        for i in 0..300 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !is_process_running(pid) {
                cleanup_proxy_state(&bifrost_dir);
                remove_pid()?;
                println!("Bifrost proxy stopped.");
                return Ok(());
            }
            if i == 250 {
                println!("Sending SIGKILL...");
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
        }

        cleanup_proxy_state(&bifrost_dir);
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
            cleanup_proxy_state(&bifrost_dir);
            remove_pid()?;
        } else {
            let terminated = unsafe { TerminateProcess(handle, 1) };
            if terminated != 0 {
                unsafe {
                    WaitForSingleObject(handle, 5000);
                }
                cleanup_proxy_state(&bifrost_dir);
                remove_pid()?;
                println!("Bifrost proxy stopped.");
            } else {
                eprintln!("Failed to terminate process (PID: {}).", pid);
                cleanup_proxy_state(&bifrost_dir);
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
