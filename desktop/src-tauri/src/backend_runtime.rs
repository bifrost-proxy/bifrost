use super::*;

pub(super) fn ensure_backend_running(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
    upgrade_relaunch: Option<&DesktopUpgradeRelaunchMarker>,
) -> tauri::Result<(Option<Child>, u16)> {
    ensure_backend_running_with_cli_wait(
        binary_path,
        data_dir,
        startup_session_id,
        preferred_port,
        upgrade_relaunch,
        DESKTOP_UPGRADE_RELAUNCH_PORT_WAIT,
    )
}

pub(super) fn ensure_backend_running_with_cli_wait(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
    upgrade_relaunch: Option<&DesktopUpgradeRelaunchMarker>,
    external_cli_wait: Duration,
) -> tauri::Result<(Option<Child>, u16)> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "ensuring backend is running; preferred_port={} data_dir={}",
            preferred_port,
            data_dir.display()
        ),
    );

    let mut fallback_from_external_cli_handoff = false;
    if let Some(marker) =
        upgrade_relaunch.filter(|marker| upgrade_relaunch_uses_external_cli_backend(marker))
    {
        let effective_wait = external_cli_handoff_wait(data_dir, marker, external_cli_wait);
        if effective_wait.is_zero() && !external_cli_wait.is_zero() {
            append_desktop_bootstrap_log(
                data_dir,
                "previous CLI-owned handoff already failed; retrying recovery without another wait",
            );
        }
        match resolve_external_cli_backend_handoff(data_dir, marker, effective_wait)? {
            ExternalCliBackendHandoff::Reuse(port) => return Ok((None, port)),
            ExternalCliBackendHandoff::StartManagedFallback => {
                fallback_from_external_cli_handoff = true;
            }
        }
    }

    if upgrade_relaunch.is_none() {
        if let Some(port) = find_existing_backend_port(data_dir, preferred_port) {
            append_desktop_bootstrap_log(
                data_dir,
                format!("reusing existing backend instance already serving on port {port}"),
            );
            return Ok((None, port));
        }
    } else if let Some(marker) = upgrade_relaunch {
        if fallback_from_external_cli_handoff {
            append_desktop_bootstrap_log(
                data_dir,
                format!(
                    "desktop upgrade handoff fallback will start a managed backend on port {}",
                    marker.proxy_port
                ),
            );
        }
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade handoff is active; skipping existing backend reuse on port {}",
                marker.proxy_port
            ),
        );
        if !fallback_from_external_cli_handoff {
            wait_for_upgrade_handoff_release(data_dir, marker);
        }
    }

    cleanup_existing_backend(binary_path, data_dir)?;

    let (child, port) = launch_backend_on_available_port(
        binary_path,
        data_dir,
        startup_session_id,
        preferred_port,
    )?;
    Ok((Some(child), port))
}

pub(super) fn launch_backend_on_available_port(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
) -> tauri::Result<(Child, u16)> {
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }
        if !is_port_available(port) {
            continue;
        }

        let mut child = start_backend(binary_path, data_dir, startup_session_id, port)?;
        match wait_for_backend(&mut child, data_dir, port, Duration::from_secs(20)) {
            Ok(()) => {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend became ready at http://{BACKEND_ADMIN_HOST}:{port}"),
                );
                return Ok((child, port));
            }
            Err(error) => {
                let should_retry_port = should_retry_backend_candidate(
                    error.kind,
                    is_port_available(port),
                    offset < MAX_PORT_INCREMENT_ATTEMPTS,
                );
                let error_message = format!(
                    "{error}; inspect {}",
                    log_dir(data_dir).join("desktop-sidecar.err.log").display()
                );
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend failed to become ready on port {port}: {error_message}"),
                );
                if let Err(stop_error) = stop_backend_with_binary(binary_path, data_dir) {
                    append_desktop_bootstrap_log(
                        data_dir,
                        format!("backend stop after failed start returned an error: {stop_error}"),
                    );
                }
                let _ = terminate_child(child);
                if should_retry_port {
                    append_desktop_bootstrap_log(
                        data_dir,
                        format!(
                            "backend child exited while port {port} became unavailable; retrying the next candidate port"
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

pub(super) fn should_retry_backend_candidate(
    failure_kind: BackendWaitFailureKind,
    port_is_available_after_exit: bool,
    has_more_candidates: bool,
) -> bool {
    failure_kind == BackendWaitFailureKind::ChildExited
        && !port_is_available_after_exit
        && has_more_candidates
}

pub(super) fn bootstrap_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop backend bootstrap started asynchronously",
    );

    let _ = start_desktop_backend_now(app, "startup");
}

pub(super) fn desktop_startup_deadline() -> Duration {
    std::env::var(DESKTOP_STARTUP_DEADLINE_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_DESKTOP_STARTUP_DEADLINE)
}

pub(super) fn startup_deadline_disposition(
    main_webview_loaded: bool,
) -> StartupDeadlineDisposition {
    if main_webview_loaded {
        StartupDeadlineDisposition::HandoffToWebview
    } else {
        StartupDeadlineDisposition::ShowNativeError
    }
}

pub(super) fn publish_startup_ready(state: &BackendState) {
    if let Ok(mut startup_error) = state.startup_error.lock() {
        state.startup_ready.store(true, Ordering::SeqCst);
        *startup_error = None;
        return;
    }

    state.startup_ready.store(true, Ordering::SeqCst);
}

pub(super) fn record_startup_deadline_error(state: &BackendState, deadline: Duration) -> bool {
    if state.startup_ready.load(Ordering::SeqCst) {
        return false;
    }
    let Ok(mut startup_error) = state.startup_error.lock() else {
        return false;
    };
    if state.startup_ready.load(Ordering::SeqCst) || startup_error.is_some() {
        return false;
    }
    *startup_error = Some(format!(
        "Bifrost core did not finish starting within {} seconds. Check {} and retry.",
        deadline.as_secs_f32(),
        log_dir(&state.data_dir)
            .join("desktop-bootstrap.log")
            .display()
    ));
    true
}

pub(super) fn schedule_desktop_startup_deadline(app: &AppHandle) {
    if !supports_native_launcher() {
        return;
    }

    let app = app.clone();
    let deadline = desktop_startup_deadline();
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };
        if state.handoff_started.load(Ordering::SeqCst)
            || state.handoff_completed.load(Ordering::SeqCst)
        {
            return;
        }

        let startup_ready = state.startup_ready.load(Ordering::SeqCst);
        let webview_loaded = state.main_webview_loaded.load(Ordering::SeqCst);
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!(
                "desktop startup deadline exceeded after {}ms; startup_ready={startup_ready} main_webview_loaded={webview_loaded}",
                deadline.as_millis()
            ),
        );

        if record_startup_deadline_error(&state, deadline) {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop startup deadline recorded a recoverable startup error",
            );
        }

        match startup_deadline_disposition(state.main_webview_loaded.load(Ordering::SeqCst)) {
            StartupDeadlineDisposition::ShowNativeError => {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "desktop startup deadline retained native launcher as an error surface because the embedded webview is not loaded",
                );
                show_native_launcher_startup_error(&app);
            }
            StartupDeadlineDisposition::HandoffToWebview => {
                if let Err(error) = start_main_window_handoff(&app, "desktop startup deadline") {
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!("desktop startup deadline handoff failed: {error}"),
                    );
                }
            }
        }
    });
}

pub(super) fn show_native_launcher_startup_error(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };
    let Some(host_window) = app.get_window(HOST_WINDOW_LABEL) else {
        append_desktop_bootstrap_log(
            &state.data_dir,
            "failed to show native launcher startup error: missing host window",
        );
        return;
    };
    let overlay_ptr = state
        .launcher_overlay
        .lock()
        .ok()
        .and_then(|overlay| *overlay);
    if let Some(overlay_ptr) = overlay_ptr {
        if let Err(error) = native_launcher::set_overlay_error(&host_window, overlay_ptr) {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("failed to show native launcher startup error: {error}"),
            );
        }
    }
}

pub(super) struct BackendRecoveryGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for BackendRecoveryGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

pub(super) fn begin_backend_recovery(state: &BackendState) -> Option<BackendRecoveryGuard<'_>> {
    if state
        .backend_recovery_in_progress
        .swap(true, Ordering::SeqCst)
    {
        return None;
    }

    Some(BackendRecoveryGuard {
        flag: &state.backend_recovery_in_progress,
    })
}

pub(super) fn monitor_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");

    let mut consecutive_health_failures = 0u8;
    loop {
        std::thread::sleep(BACKEND_WATCHDOG_POLL_INTERVAL);

        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };

        if state.shutdown_started.load(Ordering::SeqCst) || state.force_exit.load(Ordering::SeqCst)
        {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop backend watchdog stopped because desktop shutdown is in progress",
            );
            return;
        }

        if state.backend_recovery_in_progress.load(Ordering::SeqCst) {
            consecutive_health_failures = 0;
            continue;
        }

        if let Some(reason) = poll_managed_backend_exit(&state) {
            consecutive_health_failures = 0;
            attempt_backend_recovery(app, &reason);
            continue;
        }

        let current_port = match state.port.lock() {
            Ok(port) => *port,
            Err(_) => continue,
        };

        if current_port == 0 {
            consecutive_health_failures = 0;
            continue;
        }

        if probe_backend_health(current_port) {
            clear_backend_unavailable_after_healthy_probe(
                &state,
                current_port,
                "desktop backend watchdog observed healthy backend",
            );
            consecutive_health_failures = 0;
            continue;
        }

        consecutive_health_failures = consecutive_health_failures.saturating_add(1);
        if consecutive_health_failures < BACKEND_WATCHDOG_FAILURE_THRESHOLD {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!(
                    "desktop backend health probe failed on port {current_port}; waiting for confirmation ({consecutive_health_failures}/{BACKEND_WATCHDOG_FAILURE_THRESHOLD})"
                ),
            );
            continue;
        }
        consecutive_health_failures = 0;
        let managed_backend = state
            .child
            .lock()
            .map(|child| child.is_some())
            .unwrap_or(false);
        let reason = format!("backend health probe failed on port {current_port}");
        if managed_backend {
            attempt_backend_recovery(app, &reason);
        } else {
            mark_backend_unavailable_for_manual_start(&state, &reason);
        }
    }
}

pub(super) fn poll_managed_backend_exit(state: &BackendState) -> Option<String> {
    let mut child_guard = state.child.lock().ok()?;
    let child = child_guard.as_mut()?;

    match child.try_wait() {
        Ok(Some(status)) => {
            let pid = child.id();
            let _ = child_guard.take();
            Some(format!(
                "managed backend child pid={pid} exited with status {status}"
            ))
        }
        Ok(None) => None,
        Err(error) => {
            let pid = child.id();
            let _ = child_guard.take();
            Some(format!(
                "failed to poll managed backend child pid={pid}: {error}"
            ))
        }
    }
}

pub(super) fn attempt_backend_recovery(app: &AppHandle, reason: &str) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    if state.shutdown_started.load(Ordering::SeqCst) || state.force_exit.load(Ordering::SeqCst) {
        return;
    }

    let Some(_recovery_guard) = begin_backend_recovery(&state) else {
        return;
    };

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend watchdog triggering recovery; reason={reason}"),
    );
    state.startup_ready.store(false, Ordering::SeqCst);

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(&state, "during watchdog recovery") {
        let message = format!(
            "failed to terminate managed backend child during recovery; refusing to start a replacement: {error}"
        );
        record_startup_error(&state, message);
        try_start_native_handoff(app, "backend recovery failed");
        return;
    }

    let preferred_port = match state.expected_port.lock() {
        Ok(port) => *port,
        Err(_) => {
            record_startup_error(
                &state,
                "failed to read desktop expected proxy port during watchdog recovery".to_string(),
            );
            return;
        }
    };

    match ensure_backend_running(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        preferred_port,
        None,
    ) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }

            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }

            publish_startup_ready(&state);
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("desktop backend watchdog recovery succeeded; active_port={port}"),
            );
            try_start_native_handoff(app, "backend watchdog recovery");
        }
        Err(error) => {
            record_startup_error(&state, format!("desktop watchdog recovery failed: {error}"));
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!(
                    "desktop backend watchdog recovery failed; will retry after {:?}",
                    BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY
                ),
            );
            std::thread::sleep(BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY);
        }
    }
}

pub(super) fn schedule_desktop_cert_ready(data_dir: &Path) {
    let data_dir = data_dir.to_path_buf();
    std::thread::spawn(move || {
        // Wait briefly so the window and embedded core can settle before any
        // macOS trust prompt interrupts the startup flow.
        std::thread::sleep(Duration::from_secs(2));
        append_desktop_bootstrap_log(
            &data_dir,
            "starting deferred desktop certificate preflight after startup",
        );
        ensure_desktop_cert_ready(&data_dir);
    });
}

pub(super) fn record_startup_error(state: &BackendState, error: String) {
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend bootstrap failed: {error}"),
    );

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = Some(error);
    }
}

pub(super) fn mark_backend_unavailable_for_manual_start(state: &BackendState, reason: &str) {
    let was_ready = state.startup_ready.swap(false, Ordering::SeqCst);
    let mut should_log = was_ready;
    if let Ok(mut startup_error) = state.startup_error.lock() {
        if startup_error.is_none() {
            should_log = true;
        }
        *startup_error = Some(
            "Bifrost service is not running. Start the service from Bifrost Desktop to continue."
                .to_string(),
        );
    }

    if should_log {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("desktop backend requires manual start; reason={reason}"),
        );
    }
}

pub(super) fn clear_backend_unavailable_if_healthy(state: &BackendState, reason: &str) -> bool {
    let Ok(current_port) = state.port.lock().map(|port| *port) else {
        return false;
    };

    if current_port == 0 {
        return false;
    }

    let upgrade_relaunch = state
        .upgrade_relaunch
        .lock()
        .ok()
        .and_then(|marker| marker.clone());
    if let Some(marker) = upgrade_relaunch
        .as_ref()
        .filter(|marker| upgrade_relaunch_uses_external_cli_backend(marker))
    {
        if current_port != marker.proxy_port {
            return false;
        }
        let Some(identity) = probe_backend_identity(current_port) else {
            return false;
        };
        if !external_cli_backend_matches_handoff(marker, &identity) {
            return false;
        }
        write_desktop_upgrade_terminal_progress(
            &state.data_dir,
            UpgradePhase::Completed,
            "Desktop app and core update complete",
            None,
        );
        clear_upgrade_relaunch_marker(&state.data_dir);
        if let Ok(mut marker_guard) = state.upgrade_relaunch.lock() {
            *marker_guard = None;
        }
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!(
                "recovered CLI-owned upgrade handoff from healthy target backend pid={} port={current_port}",
                identity.pid
            ),
        );
    } else if !probe_backend_health(current_port) {
        return false;
    }

    clear_backend_unavailable_after_healthy_probe(state, current_port, reason)
}

pub(super) fn clear_backend_unavailable_after_healthy_probe(
    state: &BackendState,
    current_port: u16,
    reason: &str,
) -> bool {
    let was_ready = state.startup_ready.swap(true, Ordering::SeqCst);
    let mut should_log = !was_ready;
    if let Ok(mut startup_error) = state.startup_error.lock() {
        if startup_error.is_some() {
            should_log = true;
        }
        *startup_error = None;
    }

    if should_log {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("desktop backend recovered from manual-start state; reason={reason}; active_port={current_port}"),
        );
    }

    true
}

pub(super) fn desktop_runtime_snapshot(state: &BackendState) -> Result<DesktopRuntimeInfo, String> {
    if !state.startup_ready.load(Ordering::SeqCst)
        || state
            .startup_error
            .lock()
            .map_err(|_| "failed to read desktop startup error".to_string())?
            .is_some()
    {
        clear_backend_unavailable_if_healthy(
            state,
            "desktop runtime snapshot observed healthy backend",
        );
    }

    let expected_port = *state
        .expected_port
        .lock()
        .map_err(|_| "failed to read desktop expected proxy port".to_string())?;
    let port = *state
        .port
        .lock()
        .map_err(|_| "failed to read desktop proxy port".to_string())?;
    let startup_error = state
        .startup_error
        .lock()
        .map_err(|_| "failed to read desktop startup error".to_string())?
        .clone();

    Ok(DesktopRuntimeInfo {
        expected_proxy_port: expected_port,
        proxy_port: port,
        platform: std::env::consts::OS,
        startup_ready: state.startup_ready.load(Ordering::SeqCst),
        startup_error,
        handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
    })
}

pub(super) fn start_desktop_backend_now(
    app: &AppHandle,
    reason: &str,
) -> Result<DesktopRuntimeInfo, String> {
    let Some(state) = app.try_state::<BackendState>() else {
        return Err("desktop backend state is not available".to_string());
    };

    let _recovery_guard = begin_backend_recovery(&state)
        .ok_or_else(|| "desktop backend start is already in progress".to_string())?;

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend start requested; reason={reason}"),
    );

    state.startup_ready.store(false, Ordering::SeqCst);
    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(&state, "before manual start") {
        let message = format!(
            "failed to terminate managed backend before manual start; refusing to start a replacement: {error}"
        );
        record_startup_error(&state, message.clone());
        try_start_native_handoff(app, "backend manual start failed");
        return Err(message);
    }

    let preferred_port = *state
        .expected_port
        .lock()
        .map_err(|_| "failed to read desktop expected proxy port".to_string())?;

    let upgrade_relaunch = state
        .upgrade_relaunch
        .lock()
        .ok()
        .and_then(|guard| guard.clone());

    match ensure_backend_running(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        preferred_port,
        upgrade_relaunch.as_ref(),
    ) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }
            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }
            publish_startup_ready(&state);
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("desktop backend start succeeded; active_port={port} reason={reason}"),
            );
            if let Some(marker) = upgrade_relaunch.as_ref() {
                if let Some(error) =
                    deferred_desktop_install_version_error(marker, env!("CARGO_PKG_VERSION"))
                {
                    write_desktop_upgrade_terminal_progress(
                        &state.data_dir,
                        UpgradePhase::Failed,
                        "Desktop app restarted but version verification failed",
                        Some(error.clone()),
                    );
                    append_desktop_bootstrap_log(&state.data_dir, error);
                    if marker.rollback.is_some() {
                        append_desktop_bootstrap_log(
                            &state.data_dir,
                            "requesting desktop shutdown so the deferred installer can roll back",
                        );
                        request_desktop_shutdown(app);
                        return Err(
                            "deferred desktop install verification failed; rollback requested"
                                .to_string(),
                        );
                    }
                } else {
                    write_desktop_upgrade_terminal_progress(
                        &state.data_dir,
                        UpgradePhase::Completed,
                        "Desktop app and core update complete",
                        None,
                    );
                }
                clear_upgrade_relaunch_marker(&state.data_dir);
                if let Ok(mut marker_guard) = state.upgrade_relaunch.lock() {
                    *marker_guard = None;
                }
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "desktop upgrade relaunch marker cleared after managed backend start",
                );
            }
            try_start_native_handoff(app, "backend ready");
            schedule_desktop_cert_ready(&state.data_dir);
            desktop_runtime_snapshot(&state)
        }
        Err(error) => {
            let message = error.to_string();
            if let Some(marker) = upgrade_relaunch.as_ref() {
                write_desktop_upgrade_terminal_progress(
                    &state.data_dir,
                    UpgradePhase::Failed,
                    "Desktop app updated but the new core failed to start",
                    Some(message.clone()),
                );
                if marker.rollback.is_some() {
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        "requesting desktop shutdown so the failed deferred install can roll back",
                    );
                    request_desktop_shutdown(app);
                }
            }
            record_startup_error(&state, message.clone());
            try_start_native_handoff(app, "backend startup failed");
            Err(message)
        }
    }
}

pub(super) fn wait_for_backend(
    child: &mut Child,
    data_dir: &Path,
    port: u16,
    timeout: Duration,
) -> Result<(), BackendWaitFailure> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let pid = child.id();
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(BackendWaitFailure {
                    kind: BackendWaitFailureKind::ChildExited,
                    message: format!(
                        "backend process pid={pid} exited before becoming ready at http://{BACKEND_ADMIN_HOST}:{port}; status={status}"
                    ),
                });
            }
            Ok(None) => {}
            Err(error) => {
                return Err(BackendWaitFailure {
                    kind: BackendWaitFailureKind::ChildInspection,
                    message: format!(
                        "failed to inspect backend process pid={pid} while waiting for http://{BACKEND_ADMIN_HOST}:{port}: {error}"
                    ),
                });
            }
        }

        if is_backend_ready(port) && runtime_marker_matches_child(data_dir, pid, port) {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(250));
    }

    Err(BackendWaitFailure {
        kind: BackendWaitFailureKind::TimedOut,
        message: format!("backend did not become ready at http://{BACKEND_ADMIN_HOST}:{port}"),
    })
}

pub(super) fn terminate_managed_backend(state: &BackendState, context: &str) -> tauri::Result<()> {
    let child = state
        .child
        .lock()
        .map_err(|_| anyhow(format!("failed to access managed backend child {context}")))?
        .take();
    if let Some(child) = child {
        terminate_child(child).map_err(|error| {
            anyhow(format!(
                "managed backend termination failed {context}: {error}"
            ))
        })?;
    }
    Ok(())
}

pub(super) fn stop_backend_before_restart(
    binary_path: &Path,
    data_dir: &Path,
    current_port: u16,
    shutdown_timeout: Duration,
) -> tauri::Result<()> {
    if let Err(error) = stop_backend_with_binary(binary_path, data_dir) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "backend stop failed before restart; refusing to start a replacement for the same data directory: {error}"
            ),
        );
        return Err(anyhow(format!(
            "failed to stop the existing Bifrost service safely before restart: {error}. Refusing to start a replacement with the same data directory"
        )));
    }

    if !wait_for_backend_shutdown(current_port, shutdown_timeout) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "backend remained healthy on port {current_port} after stop; refusing to start a replacement for the same data directory"
            ),
        );
        return Err(anyhow(format!(
            "the existing Bifrost service remained healthy on port {current_port} after stop. Refusing to start a replacement with the same data directory"
        )));
    }

    Ok(())
}

pub(super) fn stop_backend_with_binary(binary_path: &Path, data_dir: &Path) -> tauri::Result<()> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "running synchronous backend stop; binary_path={} data_dir={}",
            binary_path.display(),
            data_dir.display()
        ),
    );
    let mut command = Command::new(binary_path);
    command
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_windows_child_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| anyhow(format!("failed to stop backend: {error}")))?;
    let status = wait_for_child_exit(&mut child, BACKEND_STOP_TIMEOUT).map_err(|error| {
        anyhow(format!(
            "backend stop command did not complete within {}ms: {error}",
            BACKEND_STOP_TIMEOUT.as_millis()
        ))
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow(format!(
            "backend stop command exited with status {status}"
        )))
    }
}

pub(super) fn wait_for_child_exit(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    match kill_child_and_wait(child, BACKEND_KILL_WAIT_TIMEOUT) {
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("process pid={} timed out and was killed", child.id()),
        )),
        Err(error) => Err(error),
    }
}

pub(super) fn kill_child_and_wait(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<ExitStatus> {
    child.kill().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("process pid={} could not be killed: {error}", child.id()),
        )
    })?;

    let kill_deadline = Instant::now() + timeout;
    while Instant::now() < kill_deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "process pid={} did not exit within {}ms after kill",
            child.id(),
            timeout.as_millis()
        ),
    ))
}

pub(super) fn spawn_backend_stop(binary_path: &Path, data_dir: &Path) -> tauri::Result<Child> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "spawning asynchronous backend stop; binary_path={} data_dir={}",
            binary_path.display(),
            data_dir.display()
        ),
    );
    let stdout_log = open_sidecar_log_file(data_dir, "desktop-sidecar.out.log")?;
    let stderr_log = open_sidecar_log_file(data_dir, "desktop-sidecar.err.log")?;

    let mut command = Command::new(binary_path);
    command
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));
    hide_windows_child_console(&mut command);
    command
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn backend stop: {error}")))
}

pub(super) fn hide_windows_child_console(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = command;
    }
}

pub(super) fn terminate_child(mut child: Child) -> tauri::Result<()> {
    kill_child_and_wait(&mut child, BACKEND_KILL_WAIT_TIMEOUT).map_err(|error| {
        anyhow(format!(
            "failed to terminate backend child within {}ms: {error}",
            BACKEND_KILL_WAIT_TIMEOUT.as_millis()
        ))
    })?;
    Ok(())
}

#[tauri::command]
pub(super) fn get_desktop_runtime(
    state: State<'_, BackendState>,
) -> Result<DesktopRuntimeInfo, String> {
    desktop_runtime_snapshot(&state)
}

#[tauri::command]
pub(super) fn start_desktop_core(app: AppHandle) -> Result<DesktopRuntimeInfo, String> {
    start_desktop_backend_now(&app, "frontend request")
}

#[tauri::command]
pub(super) fn update_desktop_proxy_port(
    state: State<'_, BackendState>,
    port: u16,
) -> Result<DesktopRuntimeInfo, String> {
    if port == 0 {
        return Err("proxy port must be greater than 0".to_string());
    }

    {
        let current_expected_port = state
            .expected_port
            .lock()
            .map_err(|_| "failed to access current desktop expected port".to_string())?;
        if *current_expected_port == port {
            let current_port = *state
                .port
                .lock()
                .map_err(|_| "failed to access current desktop port".to_string())?;
            return Ok(DesktopRuntimeInfo {
                expected_proxy_port: port,
                proxy_port: current_port,
                platform: std::env::consts::OS,
                startup_ready: state.startup_ready.load(Ordering::SeqCst),
                startup_error: state
                    .startup_error
                    .lock()
                    .map_err(|_| "failed to read desktop startup error".to_string())?
                    .clone(),
                handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
            });
        }
    }

    let current_port = *state
        .port
        .lock()
        .map_err(|_| "failed to access current desktop port".to_string())?;
    let updated_runtime = match request_backend_port_transition(current_port, port)
        .map_err(|error| error.to_string())?
    {
        BackendPortTransition::Rebound(runtime) => runtime,
        BackendPortTransition::RestartRequired => {
            restart_backend_on_port(&state, current_port, port)
                .map_err(|error| error.to_string())?
        }
    };
    save_desktop_config(&state.config_path, &DesktopConfig { proxy_port: port })
        .map_err(|error| error.to_string())?;

    {
        let mut expected_port = state
            .expected_port
            .lock()
            .map_err(|_| "failed to update desktop expected proxy port".to_string())?;
        *expected_port = port;
    }
    {
        let mut current_port = state
            .port
            .lock()
            .map_err(|_| "failed to update desktop proxy port".to_string())?;
        *current_port = updated_runtime.actual_port;
    }

    Ok(DesktopRuntimeInfo {
        expected_proxy_port: port,
        proxy_port: updated_runtime.actual_port,
        platform: std::env::consts::OS,
        startup_ready: state.startup_ready.load(Ordering::SeqCst),
        startup_error: state
            .startup_error
            .lock()
            .map_err(|_| "failed to read desktop startup error".to_string())?
            .clone(),
        handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
    })
}

#[tauri::command]
pub(super) fn notify_main_window_ready(app: AppHandle) -> Result<(), String> {
    if !supports_native_launcher() {
        return Ok(());
    }

    let Some(state) = app.try_state::<BackendState>() else {
        return Ok(());
    };

    state.main_window_ready.store(true, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        "received embedded webview ready handshake from frontend shell",
    );

    start_main_window_handoff(&app, "frontend ready handshake").map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) fn get_pending_desktop_open_requests(
    state: State<'_, BackendState>,
) -> Result<Vec<DesktopOpenRequest>, String> {
    let mut pending = state
        .pending_open_requests
        .lock()
        .map_err(|_| "failed to read pending desktop open requests".to_string())?;
    Ok(pending.drain(..).collect())
}

#[tauri::command]
pub(super) fn set_document_edited(app: AppHandle, edited: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let window = app
            .get_window(HOST_WINDOW_LABEL)
            .ok_or_else(|| "host window not found".to_string())?;
        let window_for_main_thread = window.clone();
        let run_result = window.run_on_main_thread(move || unsafe {
            let ns_window: &NSWindow = &*window_for_main_thread
                .ns_window()
                .expect("failed to get ns_window for host window")
                .cast();
            ns_window.setDocumentEdited(edited);
        });
        run_result.map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let _ = edited;
        Ok(())
    }
}

#[tauri::command]
pub(super) fn open_external_url(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|error| format!("invalid URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" | "mailto" | "bifrost" | "macappstore" => {}
        scheme => return Err(format!("unsupported URL scheme: {scheme}")),
    }

    open::that(parsed.as_str()).map_err(|error| format!("failed to open URL: {error}"))
}

#[tauri::command]
pub(super) fn write_clipboard(text: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::NSString;

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_string = NSString::from_str(&text);
        let ok = unsafe { pb.setString_forType(&ns_string, objc2_app_kit::NSPasteboardTypeString) };
        if !ok {
            return Err("NSPasteboard setString failed".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Write as _;
        #[cfg(target_os = "windows")]
        let mut child = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn clip: {e}"))?;
        #[cfg(target_os = "linux")]
        let mut child = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn xclip: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("failed to write to clipboard process: {e}"))?;
        }
        child
            .wait()
            .map_err(|e| format!("clipboard process failed: {e}"))?;
        Ok(())
    }
}

pub(super) fn restart_backend_on_port(
    state: &BackendState,
    current_port: u16,
    expected_port: u16,
) -> tauri::Result<DesktopPortUpdateResponse> {
    let _recovery_guard = begin_backend_recovery(state)
        .ok_or_else(|| anyhow("desktop backend recovery is already in progress".to_string()))?;

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "backend did not confirm dynamic port rebind; restarting embedded core on preferred port {expected_port}"
        ),
    );

    state.startup_ready.store(false, Ordering::SeqCst);

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(state, "before port-change restart") {
        let message = format!(
            "failed to terminate managed backend child before restart; refusing to start a replacement: {error}"
        );
        record_startup_error(state, message.clone());
        return Err(anyhow(message));
    }

    if let Err(error) = stop_backend_before_restart(
        &state.binary_path,
        &state.data_dir,
        current_port,
        Duration::from_secs(3),
    ) {
        let message = error.to_string();
        record_startup_error(state, message.clone());
        return Err(anyhow(message));
    }

    let (child, actual_port) = launch_backend_on_available_port(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        expected_port,
    )?;

    if let Ok(mut child_guard) = state.child.lock() {
        *child_guard = Some(child);
    }

    publish_startup_ready(state);

    Ok(DesktopPortUpdateResponse {
        expected_port,
        actual_port,
    })
}

pub(super) fn anyhow(message: String) -> tauri::Error {
    let error: Box<dyn std::error::Error> = Box::new(std::io::Error::other(message));
    tauri::Error::Setup(error.into())
}

pub(super) fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

pub(super) fn cleanup_desktop_logs_once(data_dir: &Path) {
    static CLEANED_DIRS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    let log_dir = log_dir(data_dir);
    let should_cleanup = CLEANED_DIRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut dirs| {
            if dirs.iter().any(|dir| dir == &log_dir) {
                false
            } else {
                dirs.push(log_dir.clone());
                true
            }
        })
        .unwrap_or(true);
    if should_cleanup {
        let _ = cleanup_bifrost_log_dir(&log_dir, DESKTOP_LOG_RETENTION_DAYS);
    }
}

pub(super) fn append_desktop_bootstrap_log(data_dir: &Path, message: impl AsRef<str>) {
    let log_dir = log_dir(data_dir);
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    cleanup_desktop_logs_once(data_dir);

    let _write_guard = DESKTOP_BOOTSTRAP_LOG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let log_path = log_dir.join("desktop-bootstrap.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };

    let _ = writeln!(file, "[{:?}] {}", SystemTime::now(), message.as_ref());
}

pub(super) fn open_sidecar_log_file(data_dir: &Path, file_name: &str) -> tauri::Result<fs::File> {
    let log_dir = log_dir(data_dir);
    fs::create_dir_all(&log_dir)
        .map_err(|error| anyhow(format!("failed to create log dir: {error}")))?;
    cleanup_desktop_logs_once(data_dir);

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join(file_name))
        .map_err(|error| anyhow(format!("failed to open {file_name}: {error}")))
}

pub(super) fn request_backend_port_transition(
    current_port: u16,
    expected_port: u16,
) -> tauri::Result<BackendPortTransition> {
    let client = direct_blocking_reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| anyhow(format!("failed to build backend rebind client: {error}")))?;
    let url = format!("http://{BACKEND_ADMIN_HOST}:{current_port}/_bifrost/api/config/server");
    let response = client
        .put(url)
        .json(&serde_json::json!({ "port": expected_port }))
        .send()
        .map_err(|error| anyhow(format!("failed to call backend port rebind API: {error}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow(format!(
            "backend port rebind API failed with status {}: {}",
            status, body
        )));
    }

    let response_body = response.text().map_err(|error| {
        anyhow(format!(
            "failed to read backend port rebind response: {error}"
        ))
    })?;

    if let Some(runtime) = parse_port_update_response(&response_body) {
        return Ok(BackendPortTransition::Rebound(runtime));
    }

    if is_server_config_response(&response_body) {
        return Ok(BackendPortTransition::RestartRequired);
    }

    let actual_port = wait_for_rebound_backend_port(expected_port, Duration::from_secs(2))
        .map_err(|probe_error| {
            anyhow(format!(
                "failed to decode backend port rebind response; fallback probe failed: {probe_error}; body={response_body}"
            ))
        })?;

    Ok(BackendPortTransition::Rebound(DesktopPortUpdateResponse {
        expected_port,
        actual_port,
    }))
}

pub(super) fn wait_for_rebound_backend_port(
    expected_port: u16,
    timeout: Duration,
) -> tauri::Result<u16> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
            let port = expected_port.saturating_add(offset);
            if port == 0 {
                continue;
            }

            if probe_backend_health(port) {
                return Ok(port);
            }
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    Err(anyhow(format!(
        "backend did not become healthy on any port starting from {expected_port}"
    )))
}

pub(super) fn parse_port_update_response(response_body: &str) -> Option<DesktopPortUpdateResponse> {
    serde_json::from_str::<DesktopPortUpdateResponse>(response_body).ok()
}

pub(super) fn is_server_config_response(response_body: &str) -> bool {
    serde_json::from_str::<DesktopServerConfigResponse>(response_body)
        .map(|response| {
            response.timeout_secs > 0
                && response.http1_max_header_size > 0
                && response.http2_max_header_list_size > 0
                && response.websocket_handshake_max_header_size > 0
        })
        .unwrap_or(false)
}
