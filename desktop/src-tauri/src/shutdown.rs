use super::*;

pub(super) fn request_desktop_shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        app.exit(0);
        return;
    };

    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }

    state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);

    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop shutdown requested; hiding window and waiting for owned backend and tray to stop",
    );
    if let Some(window) = app.get_window(HOST_WINDOW_LABEL) {
        let _ = window.hide();
    }

    let app_handle = app.clone();
    if state.launcher_only {
        state.force_exit.store(true, Ordering::SeqCst);
        app.exit(0);
    } else {
        std::thread::spawn(move || {
            complete_desktop_shutdown(&app_handle);
        });
    }
}

fn complete_desktop_shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        app.exit(0);
        return;
    };

    // Invalidate slow watchdog observations before waiting for the active lifecycle
    // operation. A replacement that finishes meanwhile is discarded by its owner.
    let deadline = Instant::now() + DESKTOP_QUIT_STOP_TIMEOUT;
    let _recovery_guard = loop {
        if let Some(guard) = begin_backend_recovery(&state) {
            break guard;
        }
        if Instant::now() >= deadline {
            cancel_desktop_shutdown(app, &state, "backend lifecycle operation did not finish");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    match desktop_shutdown_backend_action_for_state(&state) {
        DesktopShutdownBackendAction::StopOwnedRuntime => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop shutdown owns the active backend; requesting backend stop",
            );
            let stop_result = match spawn_backend_stop(&state.binary_path, &state.data_dir) {
                Ok(mut child) => {
                    let helper_pid = child.id();
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!(
                            "spawned backend stop helper pid={helper_pid}; waiting for owned backend and tray shutdown"
                        ),
                    );
                    wait_for_backend_stop_helper(&mut child, DESKTOP_QUIT_STOP_TIMEOUT).map_err(
                        |error| {
                            format!(
                                "backend stop helper pid={helper_pid} did not complete successfully: {error}"
                            )
                        },
                    )
                }
                Err(error) => Err(format!("failed to spawn backend stop helper: {error}")),
            };
            if let Err(error) = stop_result {
                cancel_desktop_shutdown(app, &state, &error);
                return;
            }
            append_desktop_bootstrap_log(
                &state.data_dir,
                "backend stop helper completed successfully; owned backend and tray are stopped",
            );
        }
        DesktopShutdownBackendAction::PreserveExternalRuntime => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop shutdown is preserving the external CLI-owned backend",
            );
        }
    }

    if let Ok(mut child_guard) = state.child.lock() {
        if let Some(mut child) = child_guard.take() {
            let child_pid = child.id();
            match wait_for_child_exit(&mut child, BACKEND_KILL_WAIT_TIMEOUT) {
                Ok(status) => append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("reaped stopped backend child pid={child_pid}; status={status}"),
                ),
                Err(error) => append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("failed to reap stopped backend child pid={child_pid}: {error}"),
                ),
            }
        }
    } else {
        append_desktop_bootstrap_log(
            &state.data_dir,
            "failed to lock managed backend child after successful stop; continuing final Desktop exit",
        );
    }

    state.force_exit.store(true, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop lifecycle group shutdown complete; requesting final app exit",
    );
    app.exit(0);
}

fn cancel_desktop_shutdown(app: &AppHandle, state: &BackendState, error: &str) {
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop shutdown cancelled because owned backend/tray stop failed; keeping Desktop alive: {error}"
        ),
    );
    state.shutdown_started.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_window(HOST_WINDOW_LABEL) {
        reveal_host_window(&window);
    }
}
