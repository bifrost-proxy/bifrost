use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::get_bifrost_dir;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStartMode {
    Foreground,
    Daemon,
    Desktop,
    #[default]
    Unknown,
}

impl RuntimeStartMode {
    pub fn is_restartable(self) -> bool {
        matches!(self, Self::Daemon)
    }
}

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
    #[serde(rename = "runtime_start_mode", alias = "start_mode", default)]
    pub start_mode: RuntimeStartMode,
    #[serde(default)]
    pub restartable_runtime: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binary_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub system_proxy_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub system_proxy_bypass: Option<String>,
}

impl RuntimeInfo {
    pub fn new(
        pid: u32,
        port: u16,
        socks5_port: Option<u16>,
        host: Option<String>,
        start_mode: RuntimeStartMode,
    ) -> Self {
        Self {
            pid,
            port,
            socks5_port,
            host,
            started_at_ms: bifrost_core::current_process_start_time_ms(),
            start_mode,
            restartable_runtime: start_mode.is_restartable(),
            binary_path: std::env::current_exe().ok(),
            system_proxy_enabled: None,
            system_proxy_bypass: None,
        }
    }

    pub fn restartable_daemon(&self) -> bool {
        self.restartable_runtime && self.start_mode.is_restartable()
    }

    pub fn with_system_proxy(mut self, enabled: bool, bypass: impl Into<String>) -> Self {
        self.system_proxy_enabled = Some(enabled);
        self.system_proxy_bypass = Some(bypass.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSystemProxySnapshot {
    pub bypass: String,
}

pub fn runtime_system_proxy_host(runtime_host: Option<&str>) -> &str {
    match runtime_host {
        Some("0.0.0.0") | Some("[::]") | Some("::") | None | Some("") => "127.0.0.1",
        Some(host) => host,
    }
}

pub fn capture_runtime_system_proxy_snapshot(
    runtime_info: Option<&RuntimeInfo>,
) -> Option<RuntimeSystemProxySnapshot> {
    let runtime_info = runtime_info?;
    if !bifrost_core::SystemProxyManager::is_supported() {
        return None;
    }

    let current = bifrost_core::SystemProxyManager::get_current().ok()?;
    let runtime_host = runtime_system_proxy_host(runtime_info.host.as_deref());
    let runtime_port = runtime_info.port;
    let current_matches_runtime = current.target_matches(runtime_host, runtime_port);
    let any_service_matches_runtime = if current_matches_runtime {
        true
    } else {
        bifrost_core::SystemProxyManager::any_service_proxy_matches(runtime_host, runtime_port)
            .unwrap_or(false)
    };

    if !any_service_matches_runtime {
        return None;
    }

    Some(RuntimeSystemProxySnapshot {
        bypass: current.bypass,
    })
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

/// Block until `port` is bindable on both `0.0.0.0` and `127.0.0.1` for TCP and
/// UDP, or `budget` elapses. Returns `true` if released within budget.
///
/// Used on both Unix and Windows: after a daemon is stopped, the OS does not
/// release the listening socket instantaneously, so a fresh daemon that binds
/// immediately can fail (EADDRINUSE on Unix / WSAEADDRINUSE on Windows). The
/// bind-probe logic is platform-agnostic, so the same implementation serves
/// both targets.
#[cfg(any(unix, windows))]
pub fn wait_for_port_released(port: u16, budget: std::time::Duration) -> bool {
    use std::net::{SocketAddr, TcpListener, UdpSocket};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + budget;
    let any: SocketAddr = ([0, 0, 0, 0], port).into();
    let lo: SocketAddr = ([127, 0, 0, 1], port).into();

    while Instant::now() < deadline {
        let tcp_any_ok = TcpListener::bind(any).is_ok();
        let tcp_lo_ok = TcpListener::bind(lo).is_ok();
        let udp_any_ok = UdpSocket::bind(any).is_ok();
        let udp_lo_ok = UdpSocket::bind(lo).is_ok();
        if tcp_any_ok && tcp_lo_ok && udp_any_ok && udp_lo_ok {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
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
        let is_tcp_listener = trimmed.starts_with("TCP") && trimmed.contains("LISTENING");
        let is_udp_socket = trimmed.starts_with("UDP");
        if !is_tcp_listener && !is_udp_socket {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if is_tcp_listener && parts.len() >= 5 {
            let local_addr = parts[1];
            if local_addr.ends_with(&port_str) {
                if let Ok(pid) = parts[4].parse::<u32>() {
                    let name = get_process_name_windows(pid).unwrap_or_default();
                    return Some(PortProcessInfo { pid, name });
                }
            }
        } else if is_udp_socket && parts.len() >= 4 {
            let local_addr = parts[1];
            if local_addr.ends_with(&port_str) {
                if let Ok(pid) = parts[3].parse::<u32>() {
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
    fn runtime_system_proxy_host_maps_wildcard_listeners_to_loopback() {
        assert_eq!(runtime_system_proxy_host(Some("0.0.0.0")), "127.0.0.1");
        assert_eq!(runtime_system_proxy_host(Some("[::]")), "127.0.0.1");
        assert_eq!(runtime_system_proxy_host(Some("::")), "127.0.0.1");
        assert_eq!(runtime_system_proxy_host(Some("")), "127.0.0.1");
        assert_eq!(runtime_system_proxy_host(None), "127.0.0.1");
        assert_eq!(
            runtime_system_proxy_host(Some("192.168.1.20")),
            "192.168.1.20"
        );
    }

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
        // Port zero asks the OS to allocate a real ephemeral port when binding;
        // it is never itself a listening port. Avoid releasing an ephemeral
        // port and racing another parallel test or process that can immediately
        // claim the same number before lsof/netstat runs.
        let result = find_process_on_port(0);
        assert!(
            result.is_none(),
            "should not find any process listening on reserved port zero"
        );
    }

    // Windows can keep the UDP side of a freshly released TCP ephemeral port
    // unavailable long enough to make this positive timing assertion flaky.
    #[cfg(unix)]
    #[test]
    fn wait_for_port_released_returns_quickly_when_port_is_free() {
        let mut attempts = Vec::new();
        for _ in 0..10 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let start = std::time::Instant::now();
            let freed = wait_for_port_released(port, std::time::Duration::from_secs(2));
            let elapsed = start.elapsed();
            attempts.push((port, freed, elapsed));
            if freed {
                assert!(
                    elapsed < std::time::Duration::from_millis(1500),
                    "free port should return well before the 2s timeout; took {:?}",
                    elapsed
                );
                return;
            }
        }
        panic!("expected one free ephemeral port to be reported free; attempts={attempts:?}");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn wait_for_port_released_times_out_when_port_is_held() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let start = std::time::Instant::now();
        let freed = wait_for_port_released(port, std::time::Duration::from_millis(400));
        let elapsed = start.elapsed();
        drop(listener);

        assert!(!freed, "held port {} should NOT be reported free", port);
        assert!(
            elapsed >= std::time::Duration::from_millis(350),
            "probe should use most of the budget; took {:?}",
            elapsed
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn wait_for_port_released_times_out_when_udp_port_is_held() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();

        let start = std::time::Instant::now();
        let freed = wait_for_port_released(port, std::time::Duration::from_millis(400));
        let elapsed = start.elapsed();
        drop(socket);

        assert!(!freed, "held UDP port {} should NOT be reported free", port);
        assert!(
            elapsed >= std::time::Duration::from_millis(350),
            "probe should use most of the budget; took {:?}",
            elapsed
        );
    }

    #[test]
    fn runtime_info_defaults_old_json_to_not_restartable() {
        let json = r#"{
          "pid": 42,
          "port": 9900,
          "host": "127.0.0.1"
        }"#;

        let info: RuntimeInfo = serde_json::from_str(json).expect("old runtime json parses");

        assert_eq!(info.start_mode, RuntimeStartMode::Unknown);
        assert!(!info.restartable_runtime);
        assert!(!info.restartable_daemon());
    }

    #[test]
    fn runtime_info_accepts_legacy_start_mode_alias() {
        let json = r#"{
          "pid": 42,
          "port": 9900,
          "start_mode": "daemon",
          "restartable_runtime": true,
          "binary_path": "/tmp/bifrost"
        }"#;

        let info: RuntimeInfo = serde_json::from_str(json).expect("legacy runtime json parses");

        assert_eq!(info.start_mode, RuntimeStartMode::Daemon);
        assert!(info.restartable_daemon());
        assert_eq!(info.binary_path, Some(PathBuf::from("/tmp/bifrost")));
    }

    #[test]
    fn runtime_info_new_daemon_is_restartable() {
        let info = RuntimeInfo::new(
            42,
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );

        assert!(info.restartable_runtime);
        assert!(info.restartable_daemon());
        assert_eq!(info.start_mode, RuntimeStartMode::Daemon);

        let json = serde_json::to_value(&info).expect("runtime info serializes");
        assert_eq!(json["runtime_start_mode"], "daemon");
        assert!(json.get("start_mode").is_none());
    }

    #[test]
    fn runtime_info_new_desktop_is_app_bound_not_cli_restartable() {
        let info = RuntimeInfo::new(
            42,
            9900,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Desktop,
        );

        assert!(!info.restartable_runtime);
        assert!(!info.restartable_daemon());
        assert_eq!(info.start_mode, RuntimeStartMode::Desktop);

        let json = serde_json::to_value(&info).expect("runtime info serializes");
        assert_eq!(json["runtime_start_mode"], "desktop");
    }
}
