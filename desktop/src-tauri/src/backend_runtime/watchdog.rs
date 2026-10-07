use super::*;

pub(crate) fn open_backend_recovery_circuit(state: &BackendState, message: String) {
    state.startup_ready.store(false, Ordering::SeqCst);
    record_startup_error(state, message);
}

struct BackendSignalObservation {
    admin: BackendHealthProbeResult,
    data_plane: BackendHealthProbeResult,
    health_lane: RuntimeHealthLaneProbeResult,
    signals: BackendSignalSnapshot,
}

impl BackendSignalObservation {
    fn confirmed_unresponsive(&self) -> bool {
        confirms_managed_runtime_unresponsive(self.signals)
    }

    fn summary(&self) -> String {
        format!(
            "admin_ok={} admin_ms={} admin_error={} data_ok={} data_ms={} data_error={} health_port_present={} health_ok={} health_ms={} health_error={} heartbeat_age_ms={}",
            self.admin.healthy,
            self.admin.elapsed.as_millis(),
            self.admin.failure.as_deref().unwrap_or("none"),
            self.data_plane.healthy,
            self.data_plane.elapsed.as_millis(),
            self.data_plane.failure.as_deref().unwrap_or("none"),
            self.signals.health_lane_present,
            self.health_lane.healthy,
            self.health_lane.elapsed.as_millis(),
            self.health_lane.failure.as_deref().unwrap_or("none"),
            self.signals
                .scheduler_heartbeat_age_ms
                .map(|age| age.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )
    }
}

fn probe_backend_signals(
    data_dir: &std::path::Path,
    port: u16,
    timeout: Duration,
) -> BackendSignalObservation {
    let marker = read_desktop_runtime_marker(data_dir).filter(|marker| marker.port == port);
    let health_port = marker.as_ref().and_then(|marker| marker.health_port);
    let admin = probe_backend_health_with_timeout(port, timeout);
    let data_plane = probe_data_plane_canary_with_timeout(port, timeout);
    let health_lane = probe_runtime_health_lane_with_timeout(health_port, timeout);
    let signals = BackendSignalSnapshot {
        admin_healthy: admin.healthy,
        data_plane_healthy: data_plane.healthy,
        health_lane_present: health_port.is_some()
            && health_lane_identity_matches(
                marker.as_ref().map(|marker| marker.pid),
                health_lane.snapshot.as_ref().map(|snapshot| snapshot.pid),
            ),
        health_lane_healthy: health_lane.healthy,
        scheduler_heartbeat_age_ms: health_lane
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.scheduler_heartbeat_age_ms),
    };
    BackendSignalObservation {
        admin,
        data_plane,
        health_lane,
        signals,
    }
}

fn append_watchdog_signal_event(
    data_dir: &std::path::Path,
    event_name: &str,
    decision: &str,
    observation: &BackendSignalObservation,
) {
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(event_name, "desktop_watchdog");
    event.decision = Some(decision.into());
    event.admin_probe_ms = Some(observation.admin.elapsed.as_millis() as u64);
    event.data_plane_probe_ms = Some(observation.data_plane.elapsed.as_millis() as u64);
    event.health_lane_probe_ms = Some(observation.health_lane.elapsed.as_millis() as u64);
    if let Some(snapshot) = observation.health_lane.snapshot.as_ref() {
        event.new_pid = Some(snapshot.pid);
        event.scheduler_heartbeat_age_ms = Some(snapshot.scheduler_heartbeat_age_ms);
        event.rss_bytes = Some(snapshot.rss_bytes);
        event.cpu_percent = Some(snapshot.cpu_percent);
        event.fd_count = Some(snapshot.fd_count);
        event.fd_limit = Some(snapshot.fd_limit);
        event.active_connections = Some(snapshot.active_connections);
        event.queue_depth = Some(snapshot.queue_depth);
        event.queue_capacity = Some(snapshot.queue_capacity);
    }
    if let Err(error) = bifrost_core::append_system_proxy_event(data_dir, &event) {
        append_desktop_bootstrap_log(
            data_dir,
            format!("failed to persist watchdog lifecycle event: {error}"),
        );
    }
}

fn capture_watchdog_snapshot(state: &BackendState) -> Option<BackendRecoverySnapshot> {
    match BackendRecoverySnapshot::capture(state) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            // Missing proof must preserve the process, never preserve stale
            // readiness. Surface corrupt/inaccessible ownership metadata.
            open_backend_recovery_circuit(
                state,
                format!("cannot safely inspect desktop recovery ownership: {error}"),
            );
            None
        }
    }
}

fn apply_recovery_result(
    app: &AppHandle,
    result: BackendRecoveryResult,
) -> Option<PendingBackendRecovery> {
    match result {
        BackendRecoveryResult::Recovered => {
            try_start_native_handoff(app, "backend watchdog recovery");
            None
        }
        BackendRecoveryResult::Retry(pending) => {
            try_start_native_handoff(app, "backend recovery waiting to retry");
            Some(pending)
        }
        BackendRecoveryResult::Cancelled => None,
    }
}

pub(crate) fn monitor_desktop_backend(app: &AppHandle) {
    let mut watchdog_health = BackendWatchdogHealth::default();
    let mut recovery_budget = BackendRecoveryBudget::default();
    let mut pending: Option<PendingBackendRecovery> = None;
    let mut observed_runtime: Option<BackendRecoverySnapshot> = None;
    loop {
        std::thread::sleep(BACKEND_WATCHDOG_POLL_INTERVAL);
        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };
        if state.force_exit.load(Ordering::SeqCst) {
            return;
        }
        if state.shutdown_started.load(Ordering::SeqCst) {
            pending = None;
            observed_runtime = None;
            watchdog_health.reset();
            continue;
        }
        // Even exit polling mutates the owned-child slot. Serialize it with
        // manual start, rebind, restart and shutdown, rather than checking a flag
        // and then racing those operations.
        let Some(guard) = begin_backend_recovery(&state) else {
            continue;
        };
        if backend_shutdown_requested(&state) {
            continue;
        }

        if let Some(mut retry) = pending.take() {
            if let Some(current) = retry.expected.refresh_retry_snapshot(&state) {
                retry.expected = current;
                let now = Instant::now();
                if !retry.is_due(now) {
                    pending = Some(retry);
                    continue;
                }
                if recovery_budget.try_acquire(now) {
                    pending = apply_recovery_result(
                        app,
                        attempt_backend_recovery(&state, &guard, &retry.expected, &retry.exited),
                    );
                } else {
                    retry.retry_at = recovery_budget.next_available_at(now);
                    open_backend_recovery_circuit(
                        &state,
                        format!(
                            "desktop recovery circuit open; next guarded half-open attempt in {}s",
                            retry.retry_at.saturating_duration_since(now).as_secs()
                        ),
                    );
                    suspend_failed_backend_proxy(&state, &guard, &retry.expected);
                    pending = Some(retry);
                    try_start_native_handoff(app, "backend recovery circuit open");
                }
                continue;
            }
            append_desktop_bootstrap_log(
                &state.data_dir,
                "cancelled pending backend retry because lifecycle ownership changed",
            );
        }

        let Some(before_exit) = capture_watchdog_snapshot(&state) else {
            continue;
        };
        match poll_managed_backend_exit(&state) {
            Ok(Some(exited)) => {
                state.startup_ready.store(false, Ordering::SeqCst);
                watchdog_health.reset();
                observed_runtime = None;
                let Some(expected) = before_exit.after_owned_exit(&state, exited.pid) else {
                    continue;
                };
                let now = Instant::now();
                if recovery_budget.try_acquire(now) {
                    pending = apply_recovery_result(
                        app,
                        attempt_backend_recovery(&state, &guard, &expected, &exited),
                    );
                } else {
                    open_backend_recovery_circuit(&state,
                        "desktop backend repeatedly exited; bounded recovery is waiting for a half-open retry".into());
                    suspend_failed_backend_proxy(&state, &guard, &expected);
                    pending = Some(PendingBackendRecovery {
                        expected,
                        exited,
                        retry_at: recovery_budget.next_available_at(now),
                    });
                    try_start_native_handoff(app, "backend recovery circuit open");
                }
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                watchdog_health.reset();
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("desktop child inspection failed; preserving owned child: {error}"),
                );
                continue;
            }
        }
        let Some(expected) = capture_watchdog_snapshot(&state) else {
            continue;
        };
        if expected.port == 0 {
            continue;
        }
        if observed_runtime.as_ref() != Some(&expected) {
            watchdog_health.reset();
            observed_runtime = Some(expected.clone());
        }
        // Slow probes deliberately run outside the guard. Their snapshot is
        // revalidated after reacquiring it before any state change or child kill.
        drop(guard);
        let probe =
            probe_backend_signals(&state.data_dir, expected.port, BACKEND_HEALTH_PROBE_TIMEOUT);
        let Some(guard) = begin_observed_backend_recovery(&state, &expected) else {
            watchdog_health.reset();
            continue;
        };
        let now = Instant::now();
        let disposition = watchdog_health.observe_signals(probe.signals, now);
        match disposition {
            WatchdogProbeDisposition::Healthy | WatchdogProbeDisposition::Recovered { .. } => {
                if probe.admin.healthy && backend_unavailable_gate_active(&state) {
                    clear_backend_unavailable_if_healthy_guarded(
                        &state,
                        &guard,
                        "desktop watchdog observed a healthy current runtime",
                    );
                }
            }
            WatchdogProbeDisposition::Degraded { .. } => {
                append_watchdog_signal_event(
                    &state.data_dir,
                    "watchdog_multi_signal_degraded",
                    "observe_grace_window",
                    &probe,
                );
            }
            WatchdogProbeDisposition::Preserved => {}
            WatchdogProbeDisposition::ConfirmRecovery {
                failures,
                degraded_for,
            } => {
                let confirmation = probe_backend_signals(
                    &state.data_dir,
                    expected.port,
                    BACKEND_HEALTH_CONFIRMATION_TIMEOUT,
                );
                if !expected.is_current(&state) {
                    continue;
                }
                if !backend_signals_unavailable(confirmation.signals) {
                    watchdog_health.reset();
                    continue;
                }
                let reason = format!(
                    "Admin and data-plane unavailable on port {}; failures={failures} degraded_ms={} {}",
                    expected.port, degraded_for.as_millis(), confirmation.summary()
                );
                // Missing/healthy scheduler metadata prevents a destructive kill,
                // but it is not evidence that Admin and data-plane recovered.
                if !confirmation.confirmed_unresponsive() || !expected.owned_runtime_matches() {
                    mark_backend_unavailable_for_manual_start(&state, &reason);
                    append_watchdog_signal_event(
                        &state.data_dir,
                        "watchdog_unavailable_preserved",
                        "unavailable_without_safe_kill_evidence",
                        &confirmation,
                    );
                    continue;
                }
                let now = Instant::now();
                if !recovery_budget.try_acquire(now) {
                    open_backend_recovery_circuit(
                        &state,
                        "desktop runtime remains unavailable; recovery circuit is cooling down"
                            .into(),
                    );
                    suspend_failed_backend_proxy(&state, &guard, &expected);
                    continue;
                }
                // Revalidation is inside the same guard as the termination and
                // replacement. A newer manual child can never be its victim.
                match terminate_observed_backend(&state, &guard, &expected) {
                    Ok(true) => {
                        let pid = expected.child_pid.expect("validated owned child");
                        let exited = ManagedBackendExit {
                            pid, exit_code: None, exit_signal: None,
                            detail: format!("managed child pid={pid} terminated after confirmed unresponsiveness"),
                        };
                        // An exiting runtime may have removed its own marker.
                        let Some(after_exit) = expected.after_owned_exit(&state, pid) else {
                            continue;
                        };
                        pending = apply_recovery_result(
                            app,
                            attempt_backend_recovery(&state, &guard, &after_exit, &exited),
                        );
                        watchdog_health.reset();
                        observed_runtime = None;
                    }
                    Ok(false) => watchdog_health.reset(),
                    Err(error) => {
                        open_backend_recovery_circuit(
                            &state,
                            format!("failed to terminate owned unresponsive backend: {error}"),
                        );
                        try_start_native_handoff(app, "backend termination failed");
                    }
                }
            }
        }
    }
}
