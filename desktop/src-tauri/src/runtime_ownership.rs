use super::*;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackendHealthProbeResult {
    pub(super) healthy: bool,
    pub(super) elapsed: Duration,
    pub(super) failure: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeHealthLaneProbeResult {
    pub(super) healthy: bool,
    pub(super) elapsed: Duration,
    pub(super) failure: Option<String>,
    pub(super) snapshot: Option<bifrost_core::RuntimeHealthSnapshot>,
}

pub(super) fn is_backend_ready(port: u16) -> bool {
    probe_backend_health(port)
}

pub(super) fn runtime_marker_matches_child(data_dir: &Path, child_pid: u32, port: u16) -> bool {
    let Some(marker) = read_desktop_runtime_marker(data_dir) else {
        return false;
    };

    marker.pid == child_pid && marker.port == port
}

pub(super) fn read_desktop_runtime_marker(data_dir: &Path) -> Option<DesktopRuntimeMarker> {
    let content = fs::read_to_string(data_dir.join("runtime.json")).ok()?;
    serde_json::from_str(&content).ok()
}

pub(super) fn desktop_shutdown_backend_action(
    has_managed_child: bool,
    runtime_start_mode: Option<&str>,
    runtime_matches_active_backend: bool,
) -> DesktopShutdownBackendAction {
    if has_managed_child
        || (runtime_start_mode == Some("desktop") && runtime_matches_active_backend)
    {
        DesktopShutdownBackendAction::StopOwnedRuntime
    } else {
        DesktopShutdownBackendAction::PreserveExternalRuntime
    }
}

pub(super) fn runtime_marker_matches_active_backend(
    runtime: &DesktopRuntimeMarker,
    active_port: u16,
    identity: &BackendSystemIdentity,
) -> bool {
    active_port != 0 && runtime.port == active_port && runtime.pid == identity.pid
}

pub(super) fn existing_backend_candidate_matches_runtime(
    runtime: Option<&DesktopRuntimeMarker>,
    candidate_port: u16,
    identity: Option<&BackendSystemIdentity>,
    healthy: bool,
) -> bool {
    healthy
        && runtime.zip(identity).is_some_and(|(marker, identity)| {
            runtime_marker_matches_active_backend(marker, candidate_port, identity)
        })
}

pub(super) fn desktop_shutdown_backend_action_for_state(
    state: &BackendState,
) -> DesktopShutdownBackendAction {
    let has_managed_child = state
        .child
        .lock()
        .map(|child| child.is_some())
        .unwrap_or(false);
    let runtime = read_desktop_runtime_marker(&state.data_dir);
    let active_port = state.port.lock().map(|port| *port).unwrap_or_default();
    let runtime_matches_active_backend = runtime.as_ref().is_some_and(|marker| {
        probe_backend_identity(marker.port).is_some_and(|identity| {
            runtime_marker_matches_active_backend(marker, active_port, &identity)
        })
    });
    desktop_shutdown_backend_action(
        has_managed_child,
        runtime
            .as_ref()
            .and_then(|marker| marker.start_mode.as_deref()),
        runtime_matches_active_backend,
    )
}

pub(super) fn find_existing_backend_port(data_dir: &Path, preferred_port: u16) -> Option<u16> {
    let runtime = read_desktop_runtime_marker(data_dir);
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }

        let Some(marker) = runtime.as_ref().filter(|marker| marker.port == port) else {
            continue;
        };
        let identity = probe_backend_identity(port);
        if existing_backend_candidate_matches_runtime(
            Some(marker),
            port,
            identity.as_ref(),
            probe_backend_health(port),
        ) {
            append_desktop_bootstrap_log(
                data_dir,
                format!(
                    "detected healthy backend candidate owned by the current data directory on port {port} before spawning"
                ),
            );
            return Some(port);
        }
    }

    None
}

pub(super) fn probe_backend_health(port: u16) -> bool {
    probe_backend_health_with_timeout(port, BACKEND_HEALTH_PROBE_TIMEOUT).healthy
}

pub(super) fn probe_backend_health_with_timeout(
    port: u16,
    timeout: Duration,
) -> BackendHealthProbeResult {
    let started = Instant::now();
    let client = match direct_blocking_reqwest_client_builder()
        .timeout(timeout)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return BackendHealthProbeResult {
                healthy: false,
                elapsed: started.elapsed(),
                failure: Some(format!("HTTP client build failed: {error}")),
            };
        }
    };

    let url = format!("http://{BACKEND_ADMIN_HOST}:{port}/_bifrost/api/proxy/system/support");
    let response = match client.get(url).send() {
        Ok(response) => response,
        Err(error) => {
            return BackendHealthProbeResult {
                healthy: false,
                elapsed: started.elapsed(),
                failure: Some(format!("request failed: {error}")),
            };
        }
    };

    let status = response.status();
    BackendHealthProbeResult {
        healthy: status.is_success(),
        elapsed: started.elapsed(),
        failure: (!status.is_success()).then(|| format!("HTTP status {}", status.as_u16())),
    }
}

pub(super) fn probe_data_plane_canary_with_timeout(
    port: u16,
    timeout: Duration,
) -> BackendHealthProbeResult {
    probe_raw_loopback_http(
        port,
        "GET http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary HTTP/1.1\r\nHost: bifrost-runtime-canary.invalid\r\nConnection: close\r\n\r\n",
        timeout,
        "204",
    )
}

pub(super) fn probe_runtime_health_lane_with_timeout(
    health_port: Option<u16>,
    timeout: Duration,
) -> RuntimeHealthLaneProbeResult {
    let started = Instant::now();
    let Some(port) = health_port else {
        return RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some("runtime marker has no dedicated health port".into()),
            snapshot: None,
        };
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, timeout) {
        Ok(stream) => stream,
        Err(error) => {
            return RuntimeHealthLaneProbeResult {
                healthy: false,
                elapsed: started.elapsed(),
                failure: Some(format!("health lane connect failed: {error}")),
                snapshot: None,
            };
        }
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if let Err(error) = stream.write_all(
        b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    ) {
        return RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some(format!("health lane write failed: {error}")),
            snapshot: None,
        };
    }
    let mut response = Vec::new();
    if let Err(error) = stream.read_to_end(&mut response) {
        return RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some(format!("health lane read failed: {error}")),
            snapshot: None,
        };
    }
    let response = String::from_utf8_lossy(&response);
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some("health lane returned an invalid HTTP response".into()),
            snapshot: None,
        };
    };
    if !head.lines().next().is_some_and(|line| line.contains(" 200 ")) {
        return RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some(format!(
                "health lane returned {}",
                head.lines().next().unwrap_or("unknown status")
            )),
            snapshot: None,
        };
    }
    match serde_json::from_str::<bifrost_core::RuntimeHealthSnapshot>(body) {
        Ok(snapshot) => RuntimeHealthLaneProbeResult {
            healthy: true,
            elapsed: started.elapsed(),
            failure: None,
            snapshot: Some(snapshot),
        },
        Err(error) => RuntimeHealthLaneProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some(format!("health lane JSON was invalid: {error}")),
            snapshot: None,
        },
    }
}

fn probe_raw_loopback_http(
    port: u16,
    request: &str,
    timeout: Duration,
    expected_status: &str,
) -> BackendHealthProbeResult {
    let started = Instant::now();
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, timeout) {
        Ok(stream) => stream,
        Err(error) => {
            return BackendHealthProbeResult {
                healthy: false,
                elapsed: started.elapsed(),
                failure: Some(format!("connect failed: {error}")),
            };
        }
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if let Err(error) = stream.write_all(request.as_bytes()) {
        return BackendHealthProbeResult {
            healthy: false,
            elapsed: started.elapsed(),
            failure: Some(format!("write failed: {error}")),
        };
    }
    let mut response = [0_u8; 256];
    let read = match stream.read(&mut response) {
        Ok(read) => read,
        Err(error) => {
            return BackendHealthProbeResult {
                healthy: false,
                elapsed: started.elapsed(),
                failure: Some(format!("read failed: {error}")),
            };
        }
    };
    let status_line = String::from_utf8_lossy(&response[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let healthy = status_line.contains(&format!(" {expected_status} "));
    BackendHealthProbeResult {
        healthy,
        elapsed: started.elapsed(),
        failure: (!healthy).then(|| format!("unexpected response: {status_line}")),
    }
}

pub(super) fn wait_for_backend_shutdown(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !probe_backend_health(port) {
            return true;
        }

        std::thread::sleep(Duration::from_millis(150));
    }

    !probe_backend_health(port)
}

pub(super) fn is_port_available(port: u16) -> bool {
    TcpListener::bind((BACKEND_BIND_HOST, port)).is_ok()
}

pub(super) fn has_runtime_marker(data_dir: &Path) -> bool {
    data_dir.join("bifrost.pid").exists() || data_dir.join("runtime.json").exists()
}

pub(super) fn runtime_markers_belong_to_exited_pid(
    data_dir: &Path,
    exited_pid: u32,
) -> tauri::Result<bool> {
    let runtime_path = data_dir.join("runtime.json");
    let pid_path = data_dir.join("bifrost.pid");
    let mut found_marker = false;

    if runtime_path.exists() {
        found_marker = true;
        let content = fs::read_to_string(&runtime_path).map_err(|error| {
            anyhow(format!(
                "failed to read runtime marker {}: {error}",
                runtime_path.display()
            ))
        })?;
        let marker: DesktopRuntimeMarker = serde_json::from_str(&content).map_err(|error| {
            anyhow(format!(
                "failed to parse runtime marker {}: {error}",
                runtime_path.display()
            ))
        })?;
        if marker.pid != exited_pid {
            return Err(anyhow(format!(
                "runtime marker belongs to pid={} instead of confirmed exited pid={exited_pid}",
                marker.pid
            )));
        }
    }

    if pid_path.exists() {
        found_marker = true;
        let content = fs::read_to_string(&pid_path).map_err(|error| {
            anyhow(format!(
                "failed to read pid marker {}: {error}",
                pid_path.display()
            ))
        })?;
        let marker_pid = content.trim().parse::<u32>().map_err(|error| {
            anyhow(format!(
                "failed to parse pid marker {}: {error}",
                pid_path.display()
            ))
        })?;
        if marker_pid != exited_pid {
            return Err(anyhow(format!(
                "pid marker belongs to pid={marker_pid} instead of confirmed exited pid={exited_pid}"
            )));
        }
    }

    Ok(found_marker)
}

pub(super) fn cleanup_existing_backend(binary_path: &Path, data_dir: &Path) -> tauri::Result<()> {
    if has_runtime_marker(data_dir) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "found existing backend runtime markers under {}; stopping stale backend",
                data_dir.display()
            ),
        );
        if let Err(error) = stop_backend_with_binary(binary_path, data_dir) {
            append_desktop_bootstrap_log(
                data_dir,
                format!(
                    "stale backend stop failed; refusing to start a second backend for the same data directory: {error}"
                ),
            );
            return Err(anyhow(format!(
                "failed to stop the stale Bifrost service safely: {error}. Refusing to start another service with the same data directory"
            )));
        }
    }
    Ok(())
}
