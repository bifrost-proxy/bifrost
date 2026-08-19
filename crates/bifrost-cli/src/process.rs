use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::get_bifrost_dir;

/// A conservative observation of a recorded runtime process identity.
///
/// Lifecycle recovery must distinguish a process that is definitely gone from
/// one that merely cannot be inspected. The latter must never trigger a proxy
/// cleanup or replacement runtime on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessIdentityStatus {
    /// The recorded PID is still present and does not appear to have been reused.
    Alive,
    /// The operating system definitively reported that the PID no longer exists.
    Exited,
    /// The PID exists but belongs to a different process instance.
    Reused,
    /// The process could not be inspected conclusively (for example, permissions).
    Unknown,
}

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

#[derive(Debug, Deserialize)]
struct AdminRuntimeOverview {
    server: AdminRuntimeServer,
    system: AdminRuntimeSystem,
}

#[derive(Debug, Deserialize)]
struct AdminRuntimeServer {
    port: u16,
}

#[derive(Debug, Deserialize)]
struct AdminRuntimeSystem {
    pid: u32,
    uptime_secs: u64,
    version: String,
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

pub fn runtime_is_live_desktop_owned(
    runtime: &RuntimeInfo,
    process_running: bool,
    observed_started_at_ms: Option<u64>,
) -> bool {
    runtime.start_mode == RuntimeStartMode::Desktop
        && process_running
        && !matches!(
            bifrost_core::start_times_match(runtime.started_at_ms, observed_started_at_ms),
            bifrost_core::StartTimeMatch::Mismatch { .. }
        )
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

fn runtime_info_from_admin_overview(
    requested_port: u16,
    overview: AdminRuntimeOverview,
    listener_pid: Option<u32>,
) -> Option<RuntimeInfo> {
    if overview.server.port != requested_port
        || overview.system.pid == 0
        || overview.system.version.trim().is_empty()
        || listener_pid.is_some_and(|pid| pid != overview.system.pid)
    {
        return None;
    }

    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| {
            (duration.as_millis() as u64)
                .saturating_sub(overview.system.uptime_secs.saturating_mul(1000))
        });

    Some(RuntimeInfo {
        pid: overview.system.pid,
        port: requested_port,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms,
        start_mode: RuntimeStartMode::Unknown,
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
    })
}

/// Discover a live Bifrost listener when local runtime metadata is missing or
/// stale.
///
/// The probe is deliberately fail-closed: the Admin overview must identify the
/// requested port and a non-zero PID, and when the platform can resolve the
/// listening process its PID must match the Admin response. A successful
/// loopback response is itself the liveness proof when the caller cannot query
/// the service process because of OS permissions or PID namespaces. This prevents
/// `start --yes` from treating an already-running Bifrost as an arbitrary port
/// owner and terminating it.
pub fn discover_bifrost_runtime(port: u16) -> Option<RuntimeInfo> {
    let url = format!("http://127.0.0.1:{port}/_bifrost/api/system/overview");
    let response = bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .get(&url)
        .call()
        .ok()?;
    let overview = response.into_json::<AdminRuntimeOverview>().ok()?;
    let listener_pid = find_process_on_port(port).map(|process| process.pid);
    runtime_info_from_admin_overview(port, overview, listener_pid)
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

/// Inspect a process instance without treating an inconclusive OS lookup as an
/// exit. This is intentionally separate from [`is_process_running`]: callers
/// that modify system proxy state need a stronger signal than a boolean probe.
#[cfg(unix)]
pub fn inspect_process_identity(
    pid: u32,
    recorded_started_at_ms: Option<u64>,
) -> ProcessIdentityStatus {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    if pid == 0 || pid > libc::pid_t::MAX as u32 {
        return ProcessIdentityStatus::Unknown;
    }

    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => {}
        Err(Errno::ESRCH) => return ProcessIdentityStatus::Exited,
        Err(Errno::EPERM) => return ProcessIdentityStatus::Unknown,
        Err(_) => return ProcessIdentityStatus::Unknown,
    }

    match bifrost_core::start_times_match(
        recorded_started_at_ms,
        bifrost_core::get_process_start_time_ms(pid),
    ) {
        bifrost_core::StartTimeMatch::Mismatch { .. } => ProcessIdentityStatus::Reused,
        bifrost_core::StartTimeMatch::Match | bifrost_core::StartTimeMatch::Unknown => {
            ProcessIdentityStatus::Alive
        }
    }
}

/// Windows' existing process-handle check is used as a conservative fallback.
/// A failed handle lookup can be caused by access restrictions, so only the
/// explicit "invalid PID" error is treated as an exit.
#[cfg(windows)]
pub fn inspect_process_identity(
    pid: u32,
    recorded_started_at_ms: Option<u64>,
) -> ProcessIdentityStatus {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return ProcessIdentityStatus::Unknown;
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            ProcessIdentityStatus::Exited
        } else {
            ProcessIdentityStatus::Unknown
        };
    }

    let mut exit_code = 0_u32;
    let query_succeeded = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
    unsafe {
        CloseHandle(handle);
    }
    if !query_succeeded {
        return ProcessIdentityStatus::Unknown;
    }
    if exit_code != STILL_ACTIVE as u32 {
        return ProcessIdentityStatus::Exited;
    }

    match bifrost_core::start_times_match(
        recorded_started_at_ms,
        bifrost_core::get_process_start_time_ms(pid),
    ) {
        bifrost_core::StartTimeMatch::Mismatch { .. } => ProcessIdentityStatus::Reused,
        bifrost_core::StartTimeMatch::Match | bifrost_core::StartTimeMatch::Unknown => {
            ProcessIdentityStatus::Alive
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
pub fn inspect_process_identity(
    _pid: u32,
    _recorded_started_at_ms: Option<u64>,
) -> ProcessIdentityStatus {
    ProcessIdentityStatus::Unknown
}

#[cfg(test)]
fn classify_process_identity_from_probe(
    pid_exists: Result<(), i32>,
    process_not_found_error: i32,
    start_time_match: bifrost_core::StartTimeMatch,
) -> ProcessIdentityStatus {
    match pid_exists {
        Ok(()) => match start_time_match {
            bifrost_core::StartTimeMatch::Mismatch { .. } => ProcessIdentityStatus::Reused,
            bifrost_core::StartTimeMatch::Match | bifrost_core::StartTimeMatch::Unknown => {
                ProcessIdentityStatus::Alive
            }
        },
        Err(code) if code == process_not_found_error => ProcessIdentityStatus::Exited,
        Err(_) => ProcessIdentityStatus::Unknown,
    }
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
/// Windows can retain an exclusive bind reservation after the owning process
/// has disappeared. In that case two consecutive process-table snapshots with
/// no TCP listener or UDP owner are accepted as proof that a restart is safe.
#[cfg(any(unix, windows))]
pub fn wait_for_port_released(port: u16, budget: std::time::Duration) -> bool {
    use std::net::{SocketAddr, TcpListener, UdpSocket};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + budget;
    let any: SocketAddr = ([0, 0, 0, 0], port).into();
    let lo: SocketAddr = ([127, 0, 0, 1], port).into();
    #[cfg(windows)]
    let mut consecutive_no_owner = 0_u8;

    while Instant::now() < deadline {
        let tcp_any_ok = TcpListener::bind(any).is_ok();
        let tcp_lo_ok = TcpListener::bind(lo).is_ok();
        let udp_any_ok = UdpSocket::bind(any).is_ok();
        let udp_lo_ok = UdpSocket::bind(lo).is_ok();
        if tcp_any_ok && tcp_lo_ok && udp_any_ok && udp_lo_ok {
            return true;
        }

        #[cfg(windows)]
        {
            if matches!(query_process_on_port_windows(port), Ok(None)) {
                consecutive_no_owner += 1;
                if consecutive_no_owner >= 2 {
                    return true;
                }
            } else {
                consecutive_no_owner = 0;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(windows)]
fn query_process_on_port_windows(port: u16) -> std::io::Result<Option<PortProcessInfo>> {
    let output = std::process::Command::new("netstat")
        .args(["-ano"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "netstat exited with {}",
            output.status
        )));
    }

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
                    return Ok(Some(PortProcessInfo { pid, name }));
                }
            }
        } else if is_udp_socket && parts.len() >= 4 {
            let local_addr = parts[1];
            if local_addr.ends_with(&port_str) {
                if let Ok(pid) = parts[3].parse::<u32>() {
                    let name = get_process_name_windows(pid).unwrap_or_default();
                    return Ok(Some(PortProcessInfo { pid, name }));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(windows)]
pub fn find_process_on_port(port: u16) -> Option<PortProcessInfo> {
    query_process_on_port_windows(port).ok().flatten()
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
    fn process_identity_classification_only_treats_esrch_as_definite_exit() {
        assert_eq!(
            classify_process_identity_from_probe(
                Err(404),
                404,
                bifrost_core::StartTimeMatch::Unknown
            ),
            ProcessIdentityStatus::Exited
        );
        assert_eq!(
            classify_process_identity_from_probe(
                Err(403),
                404,
                bifrost_core::StartTimeMatch::Unknown
            ),
            ProcessIdentityStatus::Unknown
        );
        assert_eq!(
            classify_process_identity_from_probe(
                Ok(()),
                404,
                bifrost_core::StartTimeMatch::Mismatch {
                    recorded: 10,
                    observed: 20
                }
            ),
            ProcessIdentityStatus::Reused
        );
        assert_eq!(
            classify_process_identity_from_probe(
                Ok(()),
                404,
                bifrost_core::StartTimeMatch::Unknown
            ),
            ProcessIdentityStatus::Alive
        );
    }

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
    fn admin_runtime_overview_requires_matching_port_pid_and_version() {
        let current_pid = std::process::id();
        let overview = AdminRuntimeOverview {
            server: AdminRuntimeServer { port: 18888 },
            system: AdminRuntimeSystem {
                pid: current_pid,
                uptime_secs: 12,
                version: "0.0.test".to_string(),
            },
        };

        let runtime = runtime_info_from_admin_overview(18888, overview, Some(current_pid))
            .expect("matching Bifrost overview should be accepted");

        assert_eq!(runtime.pid, current_pid);
        assert_eq!(runtime.port, 18888);
        assert_eq!(runtime.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(runtime.start_mode, RuntimeStartMode::Unknown);
        assert!(!runtime.restartable_runtime);
    }

    #[test]
    fn admin_runtime_overview_rejects_listener_pid_mismatch() {
        let current_pid = std::process::id();
        let overview = AdminRuntimeOverview {
            server: AdminRuntimeServer { port: 18888 },
            system: AdminRuntimeSystem {
                pid: current_pid,
                uptime_secs: 12,
                version: "0.0.test".to_string(),
            },
        };

        assert!(runtime_info_from_admin_overview(
            18888,
            overview,
            Some(current_pid.saturating_add(1))
        )
        .is_none());
    }

    #[test]
    fn admin_runtime_overview_rejects_wrong_port_or_empty_version() {
        let current_pid = std::process::id();
        let wrong_port = AdminRuntimeOverview {
            server: AdminRuntimeServer { port: 18889 },
            system: AdminRuntimeSystem {
                pid: current_pid,
                uptime_secs: 12,
                version: "0.0.test".to_string(),
            },
        };
        assert!(runtime_info_from_admin_overview(18888, wrong_port, None).is_none());

        let empty_version = AdminRuntimeOverview {
            server: AdminRuntimeServer { port: 18888 },
            system: AdminRuntimeSystem {
                pid: current_pid,
                uptime_secs: 12,
                version: " ".to_string(),
            },
        };
        assert!(runtime_info_from_admin_overview(18888, empty_version, None).is_none());

        let zero_pid = AdminRuntimeOverview {
            server: AdminRuntimeServer { port: 18888 },
            system: AdminRuntimeSystem {
                pid: 0,
                uptime_secs: 12,
                version: "0.0.test".to_string(),
            },
        };
        assert!(runtime_info_from_admin_overview(18888, zero_pid, None).is_none());
    }

    #[test]
    fn discover_bifrost_runtime_reads_live_admin_overview() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let pid = std::process::id();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept overview request");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("read overview request");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /_bifrost/api/system/overview "));

            let body = format!(
                r#"{{"server":{{"port":{port}}},"system":{{"pid":{pid},"uptime_secs":12,"version":"0.0.test"}}}}"#
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write overview response");
        });

        let runtime = discover_bifrost_runtime(port).expect("discover live overview");
        server.join().expect("overview server");

        assert_eq!(runtime.pid, pid);
        assert_eq!(runtime.port, port);
        assert_eq!(runtime.host.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn test_find_process_on_port_returns_some_for_listening_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // macOS can take a short moment to expose a newly bound listener to
        // lsof, especially while the full workspace suite is spawning many
        // child processes. Poll within a small bound instead of assuming the
        // first process-table snapshot is immediately consistent.
        let result = (0..20).find_map(|_| {
            let result = find_process_on_port(port);
            if result.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result
        });
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

    #[cfg(any(unix, windows))]
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
