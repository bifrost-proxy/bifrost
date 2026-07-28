use super::*;

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
    ConfirmRecovery {
        failures: u32,
        degraded_for: Duration,
    },
}

#[derive(Debug, Default)]
pub(crate) struct BackendWatchdogHealth {
    first_failure_at: Option<Instant>,
    consecutive_failures: u32,
}

impl BackendWatchdogHealth {
    pub(crate) fn observe_success(&mut self, now: Instant) -> WatchdogProbeDisposition {
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
    }
}

pub(crate) fn monitor_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");

    let mut watchdog_health = BackendWatchdogHealth::default();
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
            watchdog_health.reset();
            continue;
        }

        if let Some(reason) = poll_managed_backend_exit(&state) {
            watchdog_health.reset();
            attempt_backend_recovery(app, &reason);
            continue;
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

                watchdog_health.reset();
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
                if managed_backend {
                    attempt_backend_recovery(app, &reason);
                } else {
                    mark_backend_unavailable_for_manual_start(&state, &reason);
                }
            }
        }
    }
}
