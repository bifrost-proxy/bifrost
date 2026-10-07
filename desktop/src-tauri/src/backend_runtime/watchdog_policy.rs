use super::{
    BACKEND_WATCHDOG_MAX_RECOVERIES, BACKEND_WATCHDOG_MIN_FAILURES,
    BACKEND_WATCHDOG_RECOVERY_WINDOW, BACKEND_WATCHDOG_UNHEALTHY_GRACE,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedBackendExit {
    pub(crate) pid: u32,
    pub(crate) exit_code: Option<i32>,
    pub(crate) exit_signal: Option<i32>,
    pub(crate) detail: String,
}

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
#[cfg(test)]
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

pub(crate) fn health_lane_identity_matches(
    expected_pid: Option<u32>,
    observed_pid: Option<u32>,
) -> bool {
    expected_pid.is_some() && observed_pid.is_none_or(|pid| Some(pid) == expected_pid)
}

pub(crate) fn backend_signals_unavailable(signals: BackendSignalSnapshot) -> bool {
    !signals.admin_healthy && !signals.data_plane_healthy
}

pub(crate) fn confirms_managed_runtime_unresponsive(signals: BackendSignalSnapshot) -> bool {
    let scheduler_or_lane_failed = signals.health_lane_present
        && (!signals.health_lane_healthy
            || signals
                .scheduler_heartbeat_age_ms
                .is_some_and(|age| age >= SCHEDULER_HEARTBEAT_STALE_MS));
    backend_signals_unavailable(signals) && scheduler_or_lane_failed
}

#[cfg(test)]
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
    pub(crate) fn next_available_at(&self, now: Instant) -> Instant {
        if self.attempts.len() < BACKEND_WATCHDOG_MAX_RECOVERIES {
            now
        } else {
            self.attempts.front().map_or(now, |first| {
                (*first + BACKEND_WATCHDOG_RECOVERY_WINDOW).max(now)
            })
        }
    }

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

#[derive(Debug, Default)]
pub(crate) struct BackendWatchdogHealth {
    first_failure_at: Option<Instant>,
    consecutive_failures: u32,
    recovery_requested: bool,
}

impl BackendWatchdogHealth {
    pub(crate) fn observe_signals(
        &mut self,
        signals: BackendSignalSnapshot,
        now: Instant,
    ) -> WatchdogProbeDisposition {
        if backend_signals_unavailable(signals) {
            self.observe_failure(now)
        } else {
            self.observe_success(now)
        }
    }

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

    #[cfg(test)]
    pub(crate) fn mark_recovery_requested(&mut self) {
        self.recovery_requested = true;
    }
}
