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

pub(crate) fn monitor_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");

    let mut watchdog_health = BackendWatchdogHealth::default();
    let mut recovery_budget = BackendRecoveryBudget::default();
    let mut shutdown_paused = false;
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

        let probe = probe_backend_health_with_timeout(current_port, BACKEND_HEALTH_PROBE_TIMEOUT);
        let observed_at = Instant::now();
        let disposition = if probe.healthy {
            watchdog_health.observe_success(observed_at)
        } else {
            watchdog_health.observe_failure(observed_at)
        };

        match disposition {
            WatchdogProbeDisposition::Healthy => {
                clear_backend_unavailable_after_healthy_probe(
                    &state,
                    current_port,
                    "desktop backend watchdog observed healthy backend",
                );
            }
            WatchdogProbeDisposition::Recovered {
                failures,
                degraded_for,
            } => {
                clear_backend_unavailable_after_healthy_probe(
                    &state,
                    current_port,
                    "desktop backend watchdog observed recovered backend",
                );
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!(
                        "desktop backend health recovered without restart; port={current_port} consecutive_failures={failures} degraded_ms={} probe_elapsed_ms={}",
                        degraded_for.as_millis(),
                        probe.elapsed.as_millis()
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
                        "desktop backend health degraded; port={current_port} consecutive_failures={failures} degraded_ms={} probe_elapsed_ms={} error={}",
                        degraded_for.as_millis(),
                        probe.elapsed.as_millis(),
                        probe.failure.as_deref().unwrap_or("unknown probe failure")
                    ),
                );
            }
            WatchdogProbeDisposition::Preserved => {}
            WatchdogProbeDisposition::ConfirmRecovery {
                failures,
                degraded_for,
            } => {
                let confirmation = probe_backend_health_with_timeout(
                    current_port,
                    BACKEND_HEALTH_CONFIRMATION_TIMEOUT,
                );
                if confirmation.healthy {
                    watchdog_health.reset();
                    clear_backend_unavailable_after_healthy_probe(
                        &state,
                        current_port,
                        "desktop backend watchdog confirmation observed recovered backend",
                    );
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!(
                            "desktop backend health recovered during final confirmation; port={current_port} consecutive_failures={failures} degraded_ms={} confirmation_elapsed_ms={}",
                            degraded_for.as_millis(),
                            confirmation.elapsed.as_millis()
                        ),
                    );
                    continue;
                }

                let reason = format!(
                    "backend health remained unavailable after grace window on port {current_port}; consecutive_failures={failures} degraded_ms={} last_error={} confirmation_error={}",
                    degraded_for.as_millis(),
                    probe.failure.as_deref().unwrap_or("unknown probe failure"),
                    confirmation
                        .failure
                        .as_deref()
                        .unwrap_or("unknown confirmation failure")
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
                        if let Err(error) = terminate_managed_backend(
                            &state,
                            "after sustained readiness failure",
                        ) {
                            open_backend_recovery_circuit(
                                &state,
                                format!("failed to terminate unresponsive managed backend: {error}"),
                            );
                            continue;
                        }
                        attempt_backend_recovery(
                            app,
                            &ManagedBackendExit {
                                pid,
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
