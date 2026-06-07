use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::get_bifrost_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks5_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub started_at_ms: Option<u64>,
}

pub fn get_pid_file() -> bifrost_core::Result<PathBuf> {
    Ok(get_bifrost_dir()?.join("bifrost.pid"))
}

pub fn get_runtime_file() -> bifrost_core::Result<PathBuf> {
    Ok(get_bifrost_dir()?.join("runtime.json"))
}

pub fn read_pid() -> Option<u32> {
    if let Some(info) = read_runtime_info() {
        return Some(info.pid);
    }
    let pid_file = get_pid_file().ok()?;
    std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

pub fn read_runtime_info() -> Option<RuntimeInfo> {
    let runtime_file = get_runtime_file().ok()?;
    let content = std::fs::read_to_string(&runtime_file).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn read_runtime_port() -> Option<u16> {
    read_runtime_info().map(|info| info.port)
}

pub fn write_pid(pid: u32) -> bifrost_core::Result<()> {
    let pid_file = get_pid_file()?;
    if let Some(parent) = pid_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_file, pid.to_string())?;
    Ok(())
}

pub fn write_runtime_info(info: &RuntimeInfo) -> bifrost_core::Result<()> {
    let runtime_file = get_runtime_file()?;
    if let Some(parent) = runtime_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(info).map_err(std::io::Error::other)?;
    std::fs::write(&runtime_file, json)?;
    write_pid(info.pid)?;
    Ok(())
}

pub fn remove_pid() -> bifrost_core::Result<()> {
    let pid_file = get_pid_file()?;
    if pid_file.exists() {
        std::fs::remove_file(&pid_file)?;
    }
    let runtime_file = get_runtime_file()?;
    if runtime_file.exists() {
        std::fs::remove_file(&runtime_file)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let p = Pid::from_raw(pid as i32);
    if kill(p, None).is_err() {
        return false;
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            if let Some(state_start) = stat.rfind(')') {
                let after_comm = &stat[state_start + 1..];
                let state = after_comm.trim_start().chars().next().unwrap_or(' ');
                if state == 'Z' {
                    return false;
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
        {
            if output.status.success() {
                let stat = String::from_utf8_lossy(&output.stdout);
                if stat.trim_start().starts_with('Z') {
                    return false;
                }
            }
        }
    }

    true
}

#[cfg(windows)]
pub fn is_process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }

    let mut exit_code = 0u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };

    unsafe {
        CloseHandle(handle);
    }

    ok != 0 && exit_code == STILL_ACTIVE as u32
}

#[cfg(all(not(unix), not(windows)))]
pub fn is_process_running(_pid: u32) -> bool {
    false
}

#[derive(Debug)]
pub struct PortProcessInfo {
    pub pid: u32,
    pub name: String,
}

#[cfg(unix)]
pub fn find_process_on_port(port: u16) -> Option<PortProcessInfo> {
    let output = std::process::Command::new("lsof")
        .args(["-i", &format!("TCP:{}", port), "-sTCP:LISTEN", "-n", "-P"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0].to_string();
            if let Ok(pid) = parts[1].parse::<u32>() {
                return Some(PortProcessInfo { pid, name });
            }
        }
    }
    None
}

#[cfg(windows)]
pub fn find_process_on_port(port: u16) -> Option<PortProcessInfo> {
    let output = std::process::Command::new("netstat")
        .args(["-ano"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let port_str = format!(":{}", port);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("LISTENING") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 5 {
            let local_addr = parts[1];
            if local_addr.ends_with(&port_str) {
                if let Ok(pid) = parts[4].parse::<u32>() {
                    let name = get_process_name_windows(pid).unwrap_or_default();
                    return Some(PortProcessInfo { pid, name });
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn get_process_name_windows(pid: u32) -> Option<String> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?;
    let name = line.split(',').next()?.trim_matches('"').to_string();
    if name.is_empty() || name.contains("INFO:") {
        return None;
    }
    Some(name)
}

#[cfg(all(not(unix), not(windows)))]
pub fn find_process_on_port(_port: u16) -> Option<PortProcessInfo> {
    None
}

#[cfg(unix)]
pub fn kill_process_by_pid(pid: u32) -> bool {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let p = Pid::from_raw(pid as i32);
    if kill(p, Signal::SIGTERM).is_err() {
        return false;
    }

    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !is_process_running(pid) {
            return true;
        }
    }

    let _ = kill(p, Signal::SIGKILL);
    std::thread::sleep(std::time::Duration::from_millis(200));
    !is_process_running(pid)
}

#[cfg(windows)]
pub fn kill_process_by_pid(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }

    let terminated = unsafe { TerminateProcess(handle, 1) };
    if terminated != 0 {
        unsafe {
            WaitForSingleObject(handle, 5000);
        }
    }
    unsafe {
        CloseHandle(handle);
    }

    terminated != 0
}

#[cfg(all(not(unix), not(windows)))]
pub fn kill_process_by_pid(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_process_on_port_returns_some_for_listening_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let result = find_process_on_port(port);
        assert!(
            result.is_some(),
            "should find the current process listening on port {}",
            port
        );

        let info = result.unwrap();
        assert_eq!(info.pid, std::process::id());
        assert!(!info.name.is_empty());

        drop(listener);
    }

    #[test]
    fn test_find_process_on_port_returns_none_for_free_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let result = find_process_on_port(port);
        assert!(
            result.is_none(),
            "should not find any process on a freed port {}",
            port
        );
    }
}
