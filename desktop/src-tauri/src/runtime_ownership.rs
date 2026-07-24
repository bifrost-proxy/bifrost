use super::*;

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
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }

        if probe_backend_health(port) {
            append_desktop_bootstrap_log(
                data_dir,
                format!("detected healthy backend candidate on port {port} before spawning"),
            );
            return Some(port);
        }
    }

    None
}

pub(super) fn probe_backend_health(port: u16) -> bool {
    let Ok(client) = direct_blocking_reqwest_client_builder()
        .timeout(Duration::from_millis(450))
        .build()
    else {
        return false;
    };

    let url = format!("http://{BACKEND_ADMIN_HOST}:{port}/_bifrost/api/proxy/system/support");
    let Ok(response) = client.get(url).send() else {
        return false;
    };

    response.status().is_success()
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
