use super::*;

pub(crate) fn launch_backend_on_available_port(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
    allow_port_fallback: bool,
) -> tauri::Result<(Option<Child>, u16)> {
    let max_offset = if allow_port_fallback {
        MAX_PORT_INCREMENT_ATTEMPTS
    } else {
        0
    };
    for offset in 0..=max_offset {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }
        if !is_port_available(port) {
            continue;
        }

        let stderr_offset = sidecar_stderr_offset(data_dir);
        let mut child = start_backend(binary_path, data_dir, startup_session_id, port)?;
        match wait_for_backend(&mut child, data_dir, port, Duration::from_secs(20)) {
            Ok(()) => {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend became ready at http://{BACKEND_ADMIN_HOST}:{port}"),
                );
                return Ok((Some(child), port));
            }
            Err(error) => {
                let confirmed_bind_conflict = error.kind == BackendWaitFailureKind::ChildExited
                    && sidecar_stderr_reports_port_conflict_since(data_dir, port, stderr_offset);
                let should_retry_port = should_retry_backend_candidate(
                    error.kind,
                    is_port_available(port),
                    confirmed_bind_conflict,
                    offset < max_offset,
                );
                let error_message = format!(
                    "{error}; inspect {}",
                    log_dir(data_dir).join("desktop-sidecar.err.log").display()
                );
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend failed to become ready on port {port}: {error_message}"),
                );
                // The shared runtime marker may now describe the CLI daemon
                // that won the bind race. Never run a data-directory-wide stop
                // to clean up this failed child.
                terminate_child(child)?;
                if let Some(existing_port) = find_existing_backend_port(data_dir, port) {
                    if allow_port_fallback {
                        return Ok((None, existing_port));
                    }
                }
                if read_desktop_runtime_marker(data_dir).is_some() {
                    return Err(anyhow(format!(
                        "{error_message}; another runtime is recorded for this data directory; refusing to start a second backend"
                    )));
                }
                if should_retry_port {
                    append_desktop_bootstrap_log(
                        data_dir,
                        format!(
                            "backend child exited after a confirmed bind race on port {port}; retrying the next candidate port"
                        ),
                    );
                    continue;
                }
                return Err(anyhow(error_message));
            }
        }
    }

    Err(anyhow(format!(
        "failed to find an available backend port starting from {preferred_port}"
    )))
}

pub(crate) fn should_retry_backend_candidate(
    failure_kind: BackendWaitFailureKind,
    port_is_available_after_exit: bool,
    stderr_reports_bind_conflict: bool,
    has_more_candidates: bool,
) -> bool {
    failure_kind == BackendWaitFailureKind::ChildExited
        && (stderr_reports_bind_conflict || !port_is_available_after_exit)
        && has_more_candidates
}

pub(crate) fn sidecar_stderr_offset(data_dir: &Path) -> u64 {
    fs::metadata(log_dir(data_dir).join("desktop-sidecar.err.log"))
        .map(|metadata| metadata.len())
        .unwrap_or_default()
}

pub(crate) fn sidecar_stderr_reports_port_conflict_since(
    data_dir: &Path,
    port: u16,
    offset: u64,
) -> bool {
    let Ok(contents) = fs::read(log_dir(data_dir).join("desktop-sidecar.err.log")) else {
        return false;
    };
    let start = usize::try_from(offset)
        .unwrap_or(contents.len())
        .min(contents.len());
    let appended = String::from_utf8_lossy(&contents[start..]);
    appended.contains(&format!(
        "Port {BACKEND_BIND_HOST}:{port} is already in use"
    ))
}
