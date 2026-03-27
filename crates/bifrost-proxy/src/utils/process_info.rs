use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tracing::{debug, info, trace, warn};

#[derive(Debug, Clone)]
pub struct ClientProcess {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

impl ClientProcess {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ConnKey {
    client_addr: SocketAddr,
    proxy_addr: Option<SocketAddr>,
}

impl ConnKey {
    fn from_peer_addr(peer_addr: &SocketAddr) -> Self {
        Self {
            client_addr: *peer_addr,
            proxy_addr: None,
        }
    }

    fn from_connection(peer_addr: &SocketAddr, local_addr: &SocketAddr) -> Self {
        Self {
            client_addr: *peer_addr,
            proxy_addr: Some(*local_addr),
        }
    }
}

struct CachedProcess {
    process: Option<ClientProcess>,
    expires_at: Instant,
}

#[cfg(not(target_os = "macos"))]
struct SocketSnapshot {
    connections_to_pids: HashMap<ConnKey, u32>,
    expires_at: Instant,
}

pub struct ProcessResolver {
    cache: RwLock<HashMap<ConnKey, CachedProcess>>,
    pid_cache: RwLock<HashMap<u32, CachedProcess>>,
    #[cfg(not(target_os = "macos"))]
    socket_snapshot: RwLock<Option<SocketSnapshot>>,
    cache_ttl: Duration,
    negative_cache_ttl: Duration,
    #[cfg(not(target_os = "macos"))]
    socket_snapshot_ttl: Duration,
}

impl Default for ProcessResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessResolver {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            pid_cache: RwLock::new(HashMap::new()),
            #[cfg(not(target_os = "macos"))]
            socket_snapshot: RwLock::new(None),
            cache_ttl: Duration::from_secs(30),
            negative_cache_ttl: Duration::from_secs(5),
            #[cfg(not(target_os = "macos"))]
            socket_snapshot_ttl: Duration::from_millis(250),
        }
    }

    pub fn resolve(&self, peer_addr: &SocketAddr) -> Option<ClientProcess> {
        self.resolve_by_key(ConnKey::from_peer_addr(peer_addr))
    }

    pub fn resolve_for_connection(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
    ) -> Option<ClientProcess> {
        self.resolve_by_key(ConnKey::from_connection(peer_addr, local_addr))
    }

    pub fn resolve_cached(&self, peer_addr: &SocketAddr) -> Option<ClientProcess> {
        self.get_from_cache(&ConnKey::from_peer_addr(peer_addr))
            .flatten()
    }

    pub fn resolve_cached_for_connection(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
    ) -> Option<ClientProcess> {
        self.get_from_cache(&ConnKey::from_connection(peer_addr, local_addr))
            .flatten()
    }

    pub fn resolve_with_retry(
        &self,
        peer_addr: &SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        self.resolve_with_retry_by_key(ConnKey::from_peer_addr(peer_addr), max_retries, delay_ms)
    }

    pub fn resolve_for_connection_with_retry(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        self.resolve_with_retry_by_key(
            ConnKey::from_connection(peer_addr, local_addr),
            max_retries,
            delay_ms,
        )
    }

    pub async fn resolve_async(
        &self,
        peer_addr: SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        if let Some(cached) = self.get_from_cache(&ConnKey::from_peer_addr(&peer_addr)) {
            return cached;
        }

        if !peer_addr.ip().is_loopback() {
            return None;
        }

        self.resolve_with_retry(&peer_addr, max_retries, delay_ms)
    }

    pub async fn resolve_async_for_connection(
        &self,
        peer_addr: SocketAddr,
        local_addr: SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        let key = ConnKey::from_connection(&peer_addr, &local_addr);
        if let Some(cached) = self.get_from_cache(&key) {
            return cached;
        }

        if !peer_addr.ip().is_loopback() {
            return None;
        }

        self.resolve_for_connection_with_retry(&peer_addr, &local_addr, max_retries, delay_ms)
    }

    fn resolve_by_key(&self, key: ConnKey) -> Option<ClientProcess> {
        if let Some(cached) = self.get_from_cache(&key) {
            return cached;
        }

        let process = self.lookup_process(&key);
        self.update_cache(key, process.clone());
        process
    }

    fn resolve_with_retry_by_key(
        &self,
        key: ConnKey,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        for attempt in 0..=max_retries {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }

            let process = self.lookup_process(&key);
            if process.is_some() {
                debug!(?key, attempt, "Resolved client process");
                self.update_cache(key, process.clone());
                return process;
            }

            debug!(
                ?key,
                attempt, max_retries, "Client process lookup attempt missed"
            );
        }

        warn!(
            ?key,
            max_retries, delay_ms, "Failed to resolve client process after retries"
        );
        self.update_cache(key, None);
        None
    }

    fn get_from_cache(&self, key: &ConnKey) -> Option<Option<ClientProcess>> {
        let cache = self.cache.read().ok()?;
        if let Some(cached) = cache.get(key) {
            if cached.expires_at > Instant::now() {
                trace!(?key, "Process info cache hit");
                return Some(cached.process.clone());
            }
        }
        None
    }

    fn update_cache(&self, key: ConnKey, process: Option<ClientProcess>) {
        if let Ok(mut cache) = self.cache.write() {
            let ttl = if process.is_some() {
                self.cache_ttl
            } else {
                self.negative_cache_ttl
            };
            cache.insert(
                key,
                CachedProcess {
                    process,
                    expires_at: Instant::now() + ttl,
                },
            );

            if cache.len() > 10000 {
                let now = Instant::now();
                cache.retain(|_, value| value.expires_at > now);
            }
        }
    }

    fn lookup_process(&self, key: &ConnKey) -> Option<ClientProcess> {
        let pid = self.lookup_pid(key)?;
        self.lookup_cached_process_by_pid(pid)
    }

    #[cfg(target_os = "macos")]
    fn lookup_pid(&self, key: &ConnKey) -> Option<u32> {
        lookup_socket_pid_macos(key)
    }

    #[cfg(not(target_os = "macos"))]
    fn lookup_pid(&self, key: &ConnKey) -> Option<u32> {
        let now = Instant::now();

        if let Ok(snapshot_guard) = self.socket_snapshot.read() {
            if let Some(snapshot) = snapshot_guard.as_ref() {
                if snapshot.expires_at > now {
                    return snapshot.connections_to_pids.get(key).copied();
                }
            }
        }

        let connections_to_pids = lookup_socket_pid_map();
        let pid = connections_to_pids.get(key).copied();

        if let Ok(mut snapshot_guard) = self.socket_snapshot.write() {
            *snapshot_guard = Some(SocketSnapshot {
                connections_to_pids,
                expires_at: now + self.socket_snapshot_ttl,
            });
        }

        pid
    }

    fn lookup_cached_process_by_pid(&self, pid: u32) -> Option<ClientProcess> {
        let now = Instant::now();

        if let Ok(cache) = self.pid_cache.read() {
            if let Some(cached) = cache.get(&pid) {
                if cached.expires_at > now {
                    trace!(pid, "Process info pid cache hit");
                    return cached.process.clone();
                }
            }
        }

        let (name, path) = get_process_info(pid);
        let process = Some(ClientProcess { pid, name, path });

        if let Ok(mut cache) = self.pid_cache.write() {
            cache.insert(
                pid,
                CachedProcess {
                    process: process.clone(),
                    expires_at: now + self.cache_ttl,
                },
            );
            if cache.len() > 10000 {
                cache.retain(|_, value| value.expires_at > now);
            }
        }

        process
    }

    pub fn cleanup_expired(&self) {
        if let Ok(mut cache) = self.cache.write() {
            let now = Instant::now();
            let before = cache.len();
            cache.retain(|_, value| value.expires_at > now);
            let after = cache.len();
            if before != after {
                debug!(
                    removed = before - after,
                    remaining = after,
                    "Cleaned up expired process cache entries"
                );
            }
        }

        if let Ok(mut cache) = self.pid_cache.write() {
            cache.retain(|_, value| value.expires_at > Instant::now());
        }

        #[cfg(not(target_os = "macos"))]
        if let Ok(mut snapshot) = self.socket_snapshot.write() {
            if snapshot
                .as_ref()
                .is_some_and(|cached| cached.expires_at <= Instant::now())
            {
                *snapshot = None;
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let name = get_process_name_macos(pid).unwrap_or_else(|| format!("PID:{}", pid));
    let path = get_process_path_macos(pid);
    (name, path)
}

#[cfg(target_os = "macos")]
fn get_process_name_macos(pid: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0u8; 1024];
    let len = unsafe {
        libc::proc_name(
            pid as i32,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };

    if len > 0 {
        CStr::from_bytes_until_nul(&buffer[..])
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn get_process_path_macos(pid: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0u8; 4096];
    let len = unsafe {
        libc::proc_pidpath(
            pid as i32,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };

    if len > 0 {
        CStr::from_bytes_until_nul(&buffer[..])
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::ConnKey;
    use std::mem::size_of;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use tracing::{debug, trace};

    const PROC_ALL_PIDS: u32 = 1;
    const PROC_PIDLISTFDS: i32 = 1;
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    const PROX_FDTYPE_SOCKET: u32 = 2;
    const SOCKINFO_TCP: i32 = 2;
    const INI_IPV4: u8 = 0x1;
    const INI_IPV6: u8 = 0x2;
    const TCP_STATES_OF_INTEREST: [i32; 7] = [2, 3, 4, 5, 6, 8, 9];
    const SOCKET_FDINFO_SIZE: usize = 792;
    const SOCKET_FDINFO_PSI_OFFSET: usize = 24;
    #[cfg(test)]
    const SOCKET_INFO_PROTOCOL_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 156;
    #[cfg(test)]
    const SOCKET_INFO_FAMILY_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 160;
    const SOCKET_INFO_KIND_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 232;
    const SOCKET_INFO_PROTO_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 240;
    const TCP_SOCKINFO_STATE_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 80;
    const IN_SOCKINFO_FPORT_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET;
    const IN_SOCKINFO_LPORT_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 4;
    const IN_SOCKINFO_VFLAG_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 24;
    const IN_SOCKINFO_FADDR_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 32;
    const IN_SOCKINFO_LADDR_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 48;

    unsafe extern "C" {
        fn proc_listpids(
            kind: u32,
            typeinfo: u32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidfdinfo(
            pid: i32,
            fd: i32,
            flavor: i32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcFdInfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    struct ParsedTcpSocket {
        kind: i32,
        state: i32,
        vflag: u8,
        local_port_raw: i32,
        remote_port_raw: i32,
        local_addr_raw: [u8; 16],
        remote_addr_raw: [u8; 16],
    }

    pub(super) fn lookup_socket_pid_macos(key: &ConnKey) -> Option<u32> {
        let pids = list_all_pids();
        let pid = pids
            .iter()
            .copied()
            .find(|&pid| process_has_matching_socket(pid, key));

        match pid {
            Some(pid) => {
                debug!(
                    ?key,
                    pid,
                    pid_count = pids.len(),
                    "Matched macOS client socket to process"
                );
                Some(pid)
            }
            None => {
                debug!(
                    ?key,
                    pid_count = pids.len(),
                    "No macOS process matched accepted connection"
                );
                None
            }
        }
    }

    fn list_all_pids() -> Vec<u32> {
        let estimated_bytes = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        if estimated_bytes <= 0 {
            return Vec::new();
        }

        let mut buffer =
            vec![0i32; (estimated_bytes as usize / size_of::<i32>()).saturating_add(32)];
        let bytes_filled = unsafe {
            proc_listpids(
                PROC_ALL_PIDS,
                0,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<i32>()) as i32,
            )
        };
        if bytes_filled <= 0 {
            return Vec::new();
        }

        buffer.truncate(bytes_filled as usize / size_of::<i32>());
        buffer
            .into_iter()
            .filter(|pid| *pid > 0)
            .map(|pid| pid as u32)
            .collect()
    }

    fn process_has_matching_socket(pid: u32, key: &ConnKey) -> bool {
        let mut capacity = 64usize;
        loop {
            let mut fds = vec![
                ProcFdInfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                capacity
            ];
            let buffer_size = (capacity * size_of::<ProcFdInfo>()) as i32;
            let bytes_filled = unsafe {
                proc_pidinfo(
                    pid as i32,
                    PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    buffer_size,
                )
            };

            if bytes_filled <= 0 {
                return false;
            }

            if bytes_filled as usize == buffer_size as usize && capacity < 4096 {
                capacity *= 2;
                continue;
            }

            fds.truncate(bytes_filled as usize / size_of::<ProcFdInfo>());
            for fd in fds {
                if fd.proc_fdtype != PROX_FDTYPE_SOCKET {
                    continue;
                }

                if socket_fd_matches(pid, fd.proc_fd, key) {
                    return true;
                }
            }

            return false;
        }
    }

    fn socket_fd_matches(pid: u32, fd: i32, key: &ConnKey) -> bool {
        let mut socket_fdinfo = [0u8; SOCKET_FDINFO_SIZE];
        let bytes_filled = unsafe {
            proc_pidfdinfo(
                pid as i32,
                fd,
                PROC_PIDFDSOCKETINFO,
                socket_fdinfo.as_mut_ptr().cast(),
                SOCKET_FDINFO_SIZE as i32,
            )
        };
        if bytes_filled != SOCKET_FDINFO_SIZE as i32 {
            trace!(
                pid,
                fd,
                bytes_filled,
                expected = SOCKET_FDINFO_SIZE as i32,
                error = %std::io::Error::last_os_error(),
                "proc_pidfdinfo(PROC_PIDFDSOCKETINFO) did not return socket info"
            );
            return false;
        }

        let Some(tcp) = parse_tcp_socket(&socket_fdinfo) else {
            return false;
        };
        if tcp.kind != SOCKINFO_TCP || !TCP_STATES_OF_INTEREST.contains(&tcp.state) {
            return false;
        }

        match_connection_raw(key, &tcp)
    }

    #[cfg(test)]
    pub(super) fn describe_process_tcp_sockets(pid: u32) -> Vec<String> {
        let mut capacity = 64usize;
        loop {
            let mut fds = vec![
                ProcFdInfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                capacity
            ];
            let buffer_size = (capacity * size_of::<ProcFdInfo>()) as i32;
            let bytes_filled = unsafe {
                proc_pidinfo(
                    pid as i32,
                    PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    buffer_size,
                )
            };

            if bytes_filled <= 0 {
                return vec![format!(
                    "pid={pid}: proc_pidinfo(PROC_PIDLISTFDS) returned {bytes_filled}"
                )];
            }

            if bytes_filled as usize == buffer_size as usize && capacity < 4096 {
                capacity *= 2;
                continue;
            }

            fds.truncate(bytes_filled as usize / size_of::<ProcFdInfo>());
            let mut out = Vec::new();
            for fd in fds {
                if fd.proc_fdtype != PROX_FDTYPE_SOCKET {
                    continue;
                }

                let mut socket_fdinfo = [0u8; SOCKET_FDINFO_SIZE];
                let bytes_filled = unsafe {
                    proc_pidfdinfo(
                        pid as i32,
                        fd.proc_fd,
                        PROC_PIDFDSOCKETINFO,
                        socket_fdinfo.as_mut_ptr().cast(),
                        SOCKET_FDINFO_SIZE as i32,
                    )
                };
                if bytes_filled != SOCKET_FDINFO_SIZE as i32 {
                    out.push(format!(
                        "fd={} kind=? proc_pidfdinfo_bytes={} errno={}",
                        fd.proc_fd,
                        bytes_filled,
                        std::io::Error::last_os_error()
                    ));
                    continue;
                }

                let Some(info) = parse_tcp_socket(&socket_fdinfo) else {
                    out.push(format!("fd={} parse_failed", fd.proc_fd));
                    continue;
                };
                if info.kind != SOCKINFO_TCP {
                    out.push(format!("fd={} kind={} non_tcp", fd.proc_fd, info.kind));
                    continue;
                }

                let local_ip = extract_ip_bytes(info.vflag, &info.local_addr_raw);
                let local_port = decode_port(info.local_port_raw);
                let remote_ip = extract_ip_bytes(info.vflag, &info.remote_addr_raw);
                let remote_port = decode_port(info.remote_port_raw);
                let family =
                    read_i32(&socket_fdinfo, SOCKET_INFO_FAMILY_OFFSET).unwrap_or_default();
                let protocol =
                    read_i32(&socket_fdinfo, SOCKET_INFO_PROTOCOL_OFFSET).unwrap_or_default();
                out.push(format!(
                    "fd={} state={} local={:?}:{:?} remote={:?}:{:?} vflag={} family={} protocol={}",
                    fd.proc_fd,
                    info.state,
                    local_ip,
                    local_port,
                    remote_ip,
                    remote_port,
                    info.vflag,
                    family,
                    protocol
                ));
            }

            return out;
        }
    }

    fn parse_tcp_socket(buffer: &[u8; SOCKET_FDINFO_SIZE]) -> Option<ParsedTcpSocket> {
        Some(ParsedTcpSocket {
            kind: read_i32(buffer, SOCKET_INFO_KIND_OFFSET)?,
            state: read_i32(buffer, TCP_SOCKINFO_STATE_OFFSET)?,
            vflag: *buffer.get(IN_SOCKINFO_VFLAG_OFFSET)?,
            local_port_raw: read_i32(buffer, IN_SOCKINFO_LPORT_OFFSET)?,
            remote_port_raw: read_i32(buffer, IN_SOCKINFO_FPORT_OFFSET)?,
            local_addr_raw: read_fixed_16(buffer, IN_SOCKINFO_LADDR_OFFSET)?,
            remote_addr_raw: read_fixed_16(buffer, IN_SOCKINFO_FADDR_OFFSET)?,
        })
    }

    fn read_i32(buffer: &[u8], offset: usize) -> Option<i32> {
        let bytes: [u8; 4] = buffer.get(offset..offset + 4)?.try_into().ok()?;
        Some(i32::from_ne_bytes(bytes))
    }

    fn read_fixed_16(buffer: &[u8], offset: usize) -> Option<[u8; 16]> {
        buffer.get(offset..offset + 16)?.try_into().ok()
    }

    fn match_connection_raw(key: &ConnKey, socket: &ParsedTcpSocket) -> bool {
        let client_ip = match extract_ip_bytes(socket.vflag, &socket.local_addr_raw) {
            Some(ip) => ip,
            None => return false,
        };
        let client_port = match decode_port(socket.local_port_raw) {
            Some(port) => port,
            None => return false,
        };

        if client_ip != key.client_addr.ip() || client_port != key.client_addr.port() {
            return false;
        }

        if let Some(proxy_addr) = key.proxy_addr {
            let proxy_ip = match extract_ip_bytes(socket.vflag, &socket.remote_addr_raw) {
                Some(ip) => ip,
                None => return false,
            };
            let proxy_port = match decode_port(socket.remote_port_raw) {
                Some(port) => port,
                None => return false,
            };

            return proxy_ip == proxy_addr.ip() && proxy_port == proxy_addr.port();
        }

        true
    }

    fn decode_port(raw_port: i32) -> Option<u16> {
        let raw_port = u16::try_from(raw_port).ok()?;
        Some(u16::from_be(raw_port))
    }

    fn extract_ip_bytes(vflag: u8, raw_addr: &[u8; 16]) -> Option<IpAddr> {
        match vflag {
            INI_IPV4 => Some(IpAddr::V4(Ipv4Addr::new(
                raw_addr[12],
                raw_addr[13],
                raw_addr[14],
                raw_addr[15],
            ))),
            INI_IPV6 => Some(IpAddr::V6(Ipv6Addr::from(*raw_addr))),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
use macos::lookup_socket_pid_macos;

#[cfg(target_os = "windows")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let path = get_process_path_windows(pid);
    let name = path
        .as_ref()
        .and_then(|path| {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())
        })
        .unwrap_or_else(|| format!("PID:{}", pid));
    (name, path)
}

#[cfg(target_os = "windows")]
fn get_process_path_windows(pid: u32) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;

        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );

        let _ = CloseHandle(handle);

        if result.is_ok() {
            let path = OsString::from_wide(&buffer[..size as usize]);
            path.into_string().ok()
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let path = get_process_path_linux(pid);
    let name = get_process_name_linux(pid).unwrap_or_else(|| {
        path.as_ref()
            .and_then(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| format!("PID:{}", pid))
    });
    (name, path)
}

#[cfg(target_os = "linux")]
fn get_process_name_linux(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|content| content.trim().to_string())
}

#[cfg(target_os = "linux")]
fn get_process_path_linux(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| path.to_str().map(|path| path.to_string()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn get_process_info(_pid: u32) -> (String, Option<String>) {
    ("Unknown".to_string(), None)
}

lazy_static::lazy_static! {
    pub static ref PROCESS_RESOLVER: ProcessResolver = ProcessResolver::new();
}

static BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(8)
    });

static BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE: std::sync::LazyLock<Semaphore> =
    std::sync::LazyLock::new(|| Semaphore::new(*BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY));

static APP_POLICY_PROCESS_RESOLUTION_RETRIES: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_APP_POLICY_PROCESS_RESOLUTION_RETRIES")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(20)
    });

static APP_POLICY_PROCESS_RESOLUTION_DELAY_MS: std::sync::LazyLock<u64> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_APP_POLICY_PROCESS_RESOLUTION_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(50)
    });

#[cfg(not(target_os = "macos"))]
fn lookup_socket_pid_map() -> HashMap<ConnKey, u32> {
    use netstat2::{
        get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
    };

    let af_flags = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let proto_flags = ProtocolFlags::TCP;

    let sockets = match get_sockets_info(af_flags, proto_flags) {
        Ok(sockets) => sockets,
        Err(error) => {
            warn!(error = %error, "Failed to get socket info");
            return HashMap::new();
        }
    };

    let mut connections_to_pids = HashMap::new();
    for socket in sockets {
        if let ProtocolSocketInfo::Tcp(tcp) = socket.protocol_socket_info {
            if matches!(
                tcp.state,
                TcpState::Established
                    | TcpState::SynSent
                    | TcpState::SynReceived
                    | TcpState::FinWait1
                    | TcpState::FinWait2
                    | TcpState::CloseWait
                    | TcpState::LastAck
            ) {
                if let Some(&pid) = socket.associated_pids.first() {
                    let key = ConnKey {
                        client_addr: SocketAddr::new(tcp.local_addr, tcp.local_port),
                        proxy_addr: Some(SocketAddr::new(tcp.remote_addr, tcp.remote_port)),
                    };
                    connections_to_pids.entry(key).or_insert(pid);
                }
            }
        }
    }

    debug!(
        socket_count = connections_to_pids.len(),
        "Refreshed client socket pid snapshot"
    );
    connections_to_pids
}

pub fn resolve_client_process(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve(peer_addr)
}

pub fn resolve_client_process_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_for_connection(peer_addr, local_addr)
}

pub fn resolve_client_process_cached(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_cached(peer_addr)
}

pub fn resolve_client_process_cached_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_cached_for_connection(peer_addr, local_addr)
}

pub fn resolve_client_process_with_retry(
    peer_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_with_retry(peer_addr, max_retries, delay_ms)
}

pub fn resolve_client_process_for_connection_with_retry(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_for_connection_with_retry(peer_addr, local_addr, max_retries, delay_ms)
}

pub async fn resolve_client_process_async(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    resolve_client_process_async_with_retry(peer_addr, 3, 10).await
}

pub async fn resolve_client_process_async_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    resolve_client_process_async_for_connection_with_retry(peer_addr, local_addr, 3, 10).await
}

pub async fn resolve_client_process_async_with_retry(
    peer_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    if let Some(cached) = PROCESS_RESOLVER.resolve_cached(peer_addr) {
        return Some(cached);
    }

    if !peer_addr.ip().is_loopback() {
        return None;
    }

    let peer_addr = *peer_addr;
    match tokio::task::spawn_blocking(move || {
        PROCESS_RESOLVER.resolve_with_retry(&peer_addr, max_retries, delay_ms)
    })
    .await
    {
        Ok(process) => process,
        Err(err) => {
            warn!(peer_addr = %peer_addr, error = %err, "Async process resolution task failed");
            None
        }
    }
}

pub async fn resolve_client_process_async_for_connection_with_retry(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    if let Some(cached) = PROCESS_RESOLVER.resolve_cached_for_connection(peer_addr, local_addr) {
        debug!(
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            client_app = %cached.name,
            client_pid = cached.pid,
            "Client process resolution cache hit for connection"
        );
        return Some(cached);
    }

    if !peer_addr.ip().is_loopback() {
        return None;
    }

    debug!(
        peer_addr = %peer_addr,
        local_addr = %local_addr,
        max_retries,
        delay_ms,
        "Starting async client process resolution for connection"
    );

    let peer_addr = *peer_addr;
    let local_addr = *local_addr;
    match tokio::task::spawn_blocking(move || {
        PROCESS_RESOLVER.resolve_for_connection_with_retry(
            &peer_addr,
            &local_addr,
            max_retries,
            delay_ms,
        )
    })
    .await
    {
        Ok(Some(process)) => {
            debug!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                client_app = %process.name,
                client_pid = process.pid,
                "Async client process resolution completed"
            );
            Some(process)
        }
        Ok(None) => {
            warn!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                max_retries,
                delay_ms,
                "Async client process resolution completed without a match"
            );
            None
        }
        Err(err) => {
            warn!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                error = %err,
                "Async process resolution task failed"
            );
            None
        }
    }
}

pub fn spawn_async_process_resolver<F>(
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    record_id: String,
    callback: F,
) where
    F: FnOnce(String, ClientProcess) + Send + 'static,
{
    tokio::spawn(async move {
        debug!(
            record_id,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "Scheduling background client process backfill"
        );
        let permit = match BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                warn!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    "Background client process backfill skipped because semaphore is closed"
                );
                return;
            }
        };

        #[cfg(not(target_os = "macos"))]
        tokio::time::sleep(Duration::from_millis(25)).await;

        let result = tokio::task::spawn_blocking(move || {
            PROCESS_RESOLVER.resolve_for_connection_with_retry(&peer_addr, &local_addr, 20, 50)
        })
        .await;
        drop(permit);

        match result {
            Ok(Some(process)) => {
                info!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    client_app = %process.name,
                    client_pid = process.pid,
                    "Background client process backfill resolved client"
                );
                callback(record_id, process);
            }
            Ok(None) => {
                warn!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    "Background client process backfill finished without a match"
                );
            }
            Err(err) => {
                warn!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    error = %err,
                    "Background client process backfill task failed"
                );
            }
        }
    });
}

pub fn app_policy_process_resolution_retry_config() -> (u32, u64) {
    (
        *APP_POLICY_PROCESS_RESOLUTION_RETRIES,
        *APP_POLICY_PROCESS_RESOLUTION_DELAY_MS,
    )
}

pub fn format_client_info(peer_addr: &SocketAddr, process: Option<&ClientProcess>) -> String {
    match process {
        Some(process) => process.display_name(),
        None => peer_addr.ip().to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::macos::describe_process_tcp_sockets;
    use super::*;
    #[cfg(target_os = "macos")]
    use std::env;
    use std::net::{IpAddr, Ipv4Addr};
    #[cfg(target_os = "macos")]
    use std::path::PathBuf;

    #[cfg(target_os = "macos")]
    use std::process::Stdio;

    #[cfg(target_os = "macos")]
    use tokio::io::AsyncWriteExt;

    #[cfg(target_os = "macos")]
    use tokio::process::Command;

    #[test]
    fn test_format_client_info_with_process() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let process = ClientProcess {
            pid: 1234,
            name: "Chrome".to_string(),
            path: Some("/Applications/Chrome.app".to_string()),
        };
        let result = format_client_info(&addr, Some(&process));
        assert_eq!(result, "Chrome");
    }

    #[test]
    fn test_format_client_info_without_process() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 12345);
        let result = format_client_info(&addr, None);
        assert_eq!(result, "192.168.1.100");
    }

    #[test]
    fn test_process_resolver_cache() {
        let resolver = ProcessResolver::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);

        let _ = resolver.resolve(&addr);

        let cached = resolver.get_from_cache(&ConnKey::from_peer_addr(&addr));
        assert!(cached.is_some());
    }

    #[test]
    fn test_process_resolver_cached_lookup_miss() {
        let resolver = ProcessResolver::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);

        let cached = resolver.resolve_cached(&addr);
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_process_resolver_async_returns_cached_hit() {
        let resolver = ProcessResolver::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 54321);
        let process = ClientProcess {
            pid: 1234,
            name: "Chrome".to_string(),
            path: Some("/Applications/Chrome.app".to_string()),
        };

        resolver.update_cache(ConnKey::from_peer_addr(&addr), Some(process.clone()));

        let resolved = resolver.resolve_async(addr, 3, 10).await;
        assert_eq!(
            resolved.as_ref().map(|process| process.name.as_str()),
            Some("Chrome")
        );
        assert_eq!(resolved.as_ref().map(|process| process.pid), Some(1234));
    }

    #[test]
    fn test_process_resolver_retry_caches_miss() {
        let resolver = ProcessResolver::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1);

        let resolved = resolver.resolve_with_retry(&addr, 0, 0);
        assert!(resolved.is_none());
        assert!(matches!(
            resolver.get_from_cache(&ConnKey::from_peer_addr(&addr)),
            Some(None)
        ));
    }

    #[test]
    fn test_conn_key_uses_proxy_addr() {
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51000);
        let proxy_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let proxy_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);

        assert_ne!(
            ConnKey::from_connection(&peer_addr, &proxy_a),
            ConnKey::from_connection(&peer_addr, &proxy_b)
        );
    }

    #[cfg(target_os = "macos")]
    fn find_test_program(program: &str) -> Option<PathBuf> {
        let mut candidates = Vec::new();

        if program.contains(std::path::MAIN_SEPARATOR) {
            candidates.push(PathBuf::from(program));
        } else {
            if let Some(path) = env::var_os("PATH") {
                candidates.extend(env::split_paths(&path).map(|entry| entry.join(program)));
            }

            if let Some(home) = env::var_os("HOME") {
                let home = PathBuf::from(home);
                candidates.push(home.join(".local/share/mise/shims").join(program));
                candidates.push(home.join(".mise/shims").join(program));
                candidates.push(home.join(".asdf/shims").join(program));
            }

            candidates.push(PathBuf::from("/opt/homebrew/bin").join(program));
            candidates.push(PathBuf::from("/usr/local/bin").join(program));
            candidates.push(PathBuf::from("/usr/bin").join(program));
        }

        candidates.into_iter().find(|candidate| candidate.is_file())
    }

    #[cfg(target_os = "macos")]
    async fn resolve_process_from_external_client(
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<ClientProcess, String> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| format!("bind listener: {error}"))?;
        let local_addr = listener
            .local_addr()
            .map_err(|error| format!("listener local_addr: {error}"))?;
        let url = format!("http://{local_addr}/resolver-test");
        let resolved_program = find_test_program(program)
            .ok_or_else(|| format!("unable to locate executable for test program {program}"))?;

        let mut command = Command::new(&resolved_program);
        command.args(args.iter().map(|arg| arg.replace("{url}", &url)));
        command.stdout(Stdio::null());
        command.stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value.replace("{url}", &url));
        }

        let child = command
            .spawn()
            .map_err(|error| format!("spawn {program}: {error}"))?;
        let child_pid = child.id();

        let (mut stream, peer_addr) = listener
            .accept()
            .await
            .map_err(|error| format!("accept connection from {program}: {error}"))?;

        let resolved =
            resolve_client_process_async_for_connection_with_retry(&peer_addr, &local_addr, 20, 50)
                .await
                .ok_or_else(|| {
                    let socket_dump = child_pid
                        .map(|pid| describe_process_tcp_sockets(pid).join(" | "))
                        .unwrap_or_else(|| "child pid unavailable".to_string());
                    format!(
                        "resolver returned None for {program} peer={peer_addr} local={local_addr}; sockets={socket_dump}"
                    )
                })?;

        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .map_err(|error| format!("write response to {program}: {error}"))?;

        let output = child
            .wait_with_output()
            .await
            .map_err(|error| format!("wait for {program}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "{program} exited with {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(resolved)
    }

    #[cfg(target_os = "macos")]
    fn assert_process_name_matches(process: &ClientProcess, expected_tokens: &[&str]) {
        let process_name = process.name.to_lowercase();
        let process_path = process.path.as_deref().unwrap_or_default().to_lowercase();
        assert!(
            expected_tokens
                .iter()
                .any(|token| process_name.contains(token) || process_path.contains(token)),
            "expected process {:?} / {:?} to match one of {:?}",
            process.name,
            process.path,
            expected_tokens
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_process_resolver_detects_curl_client() {
        let process = resolve_process_from_external_client("curl", &["-sS", "{url}"], &[])
            .await
            .expect("resolve curl client process");

        assert_process_name_matches(&process, &["curl"]);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_process_resolver_detects_node_client() {
        let process = resolve_process_from_external_client(
            "node",
            &[
                "-e",
                "const http = require('http'); const url = process.env.TEST_URL; http.get(url, (res) => { res.resume(); res.on('end', () => process.exit(0)); }).on('error', (err) => { console.error(err); process.exit(1); });",
            ],
            &[("TEST_URL", "{url}")],
        )
        .await
        .expect("resolve node client process");

        assert_process_name_matches(&process, &["node"]);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_process_resolver_detects_python_client() {
        let process = resolve_process_from_external_client(
            "python3",
            &[
                "-c",
                "import os, sys, urllib.request; urllib.request.urlopen(os.environ['TEST_URL']).read(); sys.exit(0)",
            ],
            &[("TEST_URL", "{url}")],
        )
        .await
        .expect("resolve python client process");

        assert_process_name_matches(&process, &["python"]);
    }
}
