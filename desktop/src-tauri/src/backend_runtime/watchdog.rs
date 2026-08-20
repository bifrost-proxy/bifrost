use super::*;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogProbeDisposition {
    Healthy,
    Recovered {
        failures: u32,
        degraded_for: Duration,
    },
    Degraded {
        failures: u32,
        degraded_for: Duration,
    },
    Preserved,
    ConfirmRecovery {
        failures: u32,
        degraded_for: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SustainedReadinessAction {
    RecoverManagedChild,
    MarkExternalUnavailable,
}

const SCHEDULER_HEARTBEAT_STALE_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackendSignalSnapshot {
    pub(crate) admin_healthy: bool,
    pub(crate) data_plane_healthy: bool,
    pub(crate) health_lane_present: bool,
    pub(crate) health_lane_healthy: bool,
    pub(crate) scheduler_heartbeat_age_ms: Option<u64>,
}

pub(crate) fn confirms_managed_runtime_unresponsive(signals: BackendSignalSnapshot) -> bool {
    let scheduler_or_lane_failed = signals.health_lane_present
        && (!signals.health_lane_healthy
            || signals
                .scheduler_heartbeat_age_ms
                .is_some_and(|age| age >= SCHEDULER_HEARTBEAT_STALE_MS));
    !signals.admin_healthy && !signals.data_plane_healthy && scheduler_or_lane_failed
}

pub(crate) fn sustained_readiness_failure_action(
    has_managed_child: bool,
) -> SustainedReadinessAction {
    if has_managed_child {
        SustainedReadinessAction::RecoverManagedChild
    } else {
        SustainedReadinessAction::MarkExternalUnavailable
    }
}

#[derive(Debug, Default)]
pub(crate) struct BackendRecoveryBudget {
    attempts: VecDeque<Instant>,
}

impl BackendRecoveryBudget {
    pub(crate) fn try_acquire(&mut self, now: Instant) -> bool {
        while self.attempts.front().is_some_and(|started_at| {
            now.checked_duration_since(*started_at)
                .is_some_and(|elapsed| elapsed >= BACKEND_WATCHDOG_RECOVERY_WINDOW)
        }) {
            self.attempts.pop_front();
        }

        if self.attempts.len() >= BACKEND_WATCHDOG_MAX_RECOVERIES {
            return false;
        }

        self.attempts.push_back(now);
        true
    }
}

pub(crate) fn open_backend_recovery_circuit(state: &BackendState, message: String) {
    state.startup_ready.store(false, Ordering::SeqCst);
    record_startup_error(state, message);
}

#[derive(Debug, Default)]
pub(crate) struct BackendWatchdogHealth {
    first_failure_at: Option<Instant>,
    consecutive_failures: u32,
    recovery_requested: bool,
}

impl BackendWatchdogHealth {
    pub(crate) fn observe_success(&mut self, now: Instant) -> WatchdogProbeDisposition {
        self.recovery_requested = false;
        let Some(first_failure_at) = self.first_failure_at.take() else {
            self.consecutive_failures = 0;
            return WatchdogProbeDisposition::Healthy;
        };
        let failures = std::mem::take(&mut self.consecutive_failures);
        WatchdogProbeDisposition::Recovered {
            failures,
            degraded_for: now
                .checked_duration_since(first_failure_at)
                .unwrap_or_default(),
        }
    }

    pub(crate) fn observe_failure(&mut self, now: Instant) -> WatchdogProbeDisposition {
        let first_failure_at = *self.first_failure_at.get_or_insert(now);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let degraded_for = now
            .checked_duration_since(first_failure_at)
            .unwrap_or_default();
        let failures = self.consecutive_failures;

        if self.recovery_requested {
            return WatchdogProbeDisposition::Preserved;
        }

        if failures >= BACKEND_WATCHDOG_MIN_FAILURES
            && degraded_for >= BACKEND_WATCHDOG_UNHEALTHY_GRACE
        {
            WatchdogProbeDisposition::ConfirmRecovery {
                failures,
                degraded_for,
            }
        } else {
            WatchdogProbeDisposition::Degraded {
                failures,
                degraded_for,
            }
        }
    }

    pub(super) fn reset(&mut self) {
        self.first_failure_at = None;
        self.consecutive_failures = 0;
        self.recovery_requested = false;
    }

    pub(crate) fn mark_recovery_requested(&mut self) {
        self.recovery_requested = true;
    }
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
        health_lane_present: health_port.is_some(),
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
    let mut event =
        bifrost_core::SystemProxyLifecycleEvent::new(event_name, "desktop_watchdog");
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

pub(crate) fn monitor_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");

    let mut watchdog_health = BackendWatchdogHealth::default();
    let mut recovery_budget = BackendRecoveryBudget::default();
    let mut shutdown_paused = false;
    let mut isolated_admin_failure_active = false;
    loop {
        std::thread::sleep(BACKEND_WATCHDOG_POLL_INTERVAL);

        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };

        if state.force_exit.load(Ordering::SeqCst) {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop backend watchdog stopped because final Desktop exit is in progress",
            );
            return;
        }
        if state.shutdown_started.load(Ordering::SeqCst) {
            if !shutdown_paused {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "desktop backend watchdog paused while lifecycle group shutdown is in progress",
                );
                shutdown_paused = true;
            }
            continue;
        }
        if shutdown_paused {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop backend watchdog resumed after lifecycle group shutdown was cancelled",
            );
            shutdown_paused = false;
        }

        if state.backend_recovery_in_progress.load(Ordering::SeqCst) {
            watchdog_health.reset();
            continue;
        }

        match poll_managed_backend_exit(&state) {
            Ok(Some(exited)) => {
                watchdog_health.reset();
                if recovery_budget.try_acquire(Instant::now()) {
                    attempt_backend_recovery(app, &exited);
                } else {
                    let message = format!(
                        "desktop backend exited repeatedly; automatic recovery circuit opened after {} attempts in {}s; last_exit={}",
                        BACKEND_WATCHDOG_MAX_RECOVERIES,
                        BACKEND_WATCHDOG_RECOVERY_WINDOW.as_secs(),
                        exited.detail
                    );
                    open_backend_recovery_circuit(&state, message);
                    try_start_native_handoff(app, "backend recovery circuit open");
                }
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                watchdog_health.reset();
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!(
                        "desktop backend child inspection failed; preserving managed child and refusing replacement: {error}"
                    ),
                );
                continue;
            }
        }

        let current_port = match state.port.lock() {
            Ok(port) => *port,
            Err(_) => continue,
        };

        if current_port == 0 {
            watchdog_health.reset();
            continue;
        }

        let probe =
            probe_backend_signals(&state.data_dir, current_port, BACKEND_HEALTH_PROBE_TIMEOUT);
        let isolated_admin_failure = !probe.admin.healthy && !probe.confirmed_unresponsive();
        if !isolated_admin_failure {
            isolated_admin_failure_active = false;
        }
        let observed_at = Instant::now();
        let disposition = if probe.confirmed_unresponsive() {
            watchdog_health.observe_failure(observed_at)
        } else {
            watchdog_health.observe_success(observed_at)
        };

        match disposition {
            WatchdogProbeDisposition::Healthy => {
                if probe.admin.healthy {
                    clear_backend_unavailable_after_healthy_probe(
                        &state,
                        current_port,
                        "desktop backend watchdog observed healthy backend",
                    );
                } else if !isolated_admin_failure_active {
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!(
                            "desktop backend Admin probe failed in isolation; managed process preserved; port={current_port} {}",
                            probe.summary()
                        ),
                    );
                    append_watchdog_signal_event(
                        &state.data_dir,
                        "watchdog_admin_probe_isolated_failure",
                        "preserve_process",
                        &probe,
                    );
                    isolated_admin_failure_active = true;
                }
            }
            WatchdogProbeDisposition::Recovered {
                failures,
                degraded_for,
            } => {
                if probe.admin.healthy {
                    clear_backend_unavailable_after_healthy_probe(
                        &state,
                        current_port,
                        "desktop backend watchdog observed recovered backend",
                    );
                }
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!(
                        "desktop backend multi-signal health recovered without restart; port={current_port} consecutive_failures={failures} degraded_ms={} {}",
                        degraded_for.as_millis(),
                        probe.summary()
                    ),
                );
            }
            WatchdogProbeDisposition::Degraded {
                failures,
                degraded_for,
            } => {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!(
                        "desktop backend multi-signal health degraded; port={current_port} consecutive_failures={failures} degraded_ms={} {}",
                        degraded_for.as_millis(),
                        probe.summary()
                    ),
                );
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
                    current_port,
                    BACKEND_HEALTH_CONFIRMATION_TIMEOUT,
                );
                if !confirmation.confirmed_unresponsive() {
                    watchdog_health.reset();
                    if confirmation.admin.healthy {
                        clear_backend_unavailable_after_healthy_probe(
                            &state,
                            current_port,
                            "desktop backend watchdog confirmation observed recovered backend",
                        );
                    }
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!(
                            "desktop backend recovery cancelled because final multi-signal confirmation was not unanimous; port={current_port} consecutive_failures={failures} degraded_ms={} {}",
                            degraded_for.as_millis(),
                            confirmation.summary()
                        ),
                    );
                    continue;
                }

                let reason = format!(
                    "backend Admin, data-plane canary, and scheduler/health lane all remained unavailable after grace window on port {current_port}; consecutive_failures={failures} degraded_ms={} last={} confirmation={}",
                    degraded_for.as_millis(),
                    probe.summary(),
                    confirmation.summary()
                );
                append_watchdog_signal_event(
                    &state.data_dir,
                    "watchdog_multi_signal_recovery_confirmed",
                    "recover_managed_child",
                    &confirmation,
                );
                let managed_backend = state
                    .child
                    .lock()
                    .map(|child| child.is_some())
                    .unwrap_or(false);
                match sustained_readiness_failure_action(managed_backend) {
                    SustainedReadinessAction::RecoverManagedChild => {
                        append_desktop_bootstrap_log(
                            &state.data_dir,
                            format!(
                                "desktop backend readiness remained degraded; terminating unresponsive managed child for bounded recovery; {reason}"
                            ),
                        );
                        watchdog_health.mark_recovery_requested();
                        if !recovery_budget.try_acquire(Instant::now()) {
                            open_backend_recovery_circuit(
                                &state,
                                format!(
                                    "desktop backend remained unresponsive repeatedly; automatic recovery circuit opened after {} attempts in {}s",
                                    BACKEND_WATCHDOG_MAX_RECOVERIES,
                                    BACKEND_WATCHDOG_RECOVERY_WINDOW.as_secs()
                                ),
                            );
                            try_start_native_handoff(app, "backend recovery circuit open");
                            continue;
                        }
                        let pid = state
                            .child
                            .lock()
                            .ok()
                            .and_then(|guard| guard.as_ref().map(std::process::Child::id))
                            .unwrap_or_default();
                        if let Err(error) =
                            terminate_managed_backend(&state, "after sustained readiness failure")
                        {
                            open_backend_recovery_circuit(
                                &state,
                                format!(
                                    "failed to terminate unresponsive managed backend: {error}"
                                ),
                            );
                            continue;
                        }
                        attempt_backend_recovery(
                            app,
                            &ManagedBackendExit {
                                pid,
                                exit_code: None,
                                exit_signal: None,
                                detail: format!(
                                    "managed backend child pid={pid} was terminated after sustained readiness failure"
                                ),
                            },
                        );
                        watchdog_health.reset();
                    }
                    SustainedReadinessAction::MarkExternalUnavailable => {
                        mark_backend_unavailable_for_manual_start(&state, &reason);
                        watchdog_health.reset();
                    }
                }
            }
        }
    }
}
