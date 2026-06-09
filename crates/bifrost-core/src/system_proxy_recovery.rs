//! Shared retry policy for system-proxy crash recovery operations.
//!
//! Both the macOS LaunchDaemon (Layer 2) and the per-process lifecycle helper
//! (Layer 1) wrap [`crate::SystemProxyManager::recover_from_crash`] with the
//! same retry policy: most transient failures are bounded by a short window,
//! while macOS network service enumeration returning empty means the system
//! network stack is not ready yet and must be polled until it becomes readable.
//! This module owns the policy so both layers stay in lock step.

use std::time::{Duration, Instant};

use crate::error::BifrostError;

/// Default retry window mirroring the LaunchDaemon Layer-2 behaviour.
pub const RECOVERY_RETRY_WINDOW: Duration = Duration::from_secs(60);
/// Default retry interval mirroring the LaunchDaemon Layer-2 behaviour.
pub const RECOVERY_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Returns `true` for the small set of error messages we know are transient
/// during system boot and thus worth retrying.
pub fn is_retryable_recovery_error(error: &BifrostError) -> bool {
    let message = error.to_string();
    message.contains("networksetup")
        || message.contains("No enabled macOS network services")
        || message.contains("Failed to execute launchctl")
}

/// Returns `true` when macOS network services are still unavailable during
/// boot/shutdown. The cleanup process must keep polling this case instead of
/// exiting and waiting for launchd to wake it again.
pub fn is_network_services_not_ready_error(error: &BifrostError) -> bool {
    error
        .to_string()
        .contains("No enabled macOS network services")
}

/// Run `op` with retries while it returns retryable errors and we are still
/// inside the retry window. When macOS network services are not ready yet,
/// continue polling past the retry window until the service list is readable.
/// Non-retryable errors are returned immediately.
///
/// `attempt` starts at 1 and is incremented before every retry; callers can
/// use it to log the retry counter.
pub fn retry_with_policy<T, F>(
    retry_window: Duration,
    retry_interval: Duration,
    mut op: F,
) -> Result<T, BifrostError>
where
    F: FnMut(u32) -> Result<T, BifrostError>,
{
    let started_at = Instant::now();
    let mut attempt: u32 = 1;
    loop {
        match op(attempt) {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !is_retryable_recovery_error(&error) {
                    return Err(error);
                }
                let keep_waiting_for_network_services = is_network_services_not_ready_error(&error);
                if !keep_waiting_for_network_services && started_at.elapsed() >= retry_window {
                    return Err(error);
                }
                let delay = if keep_waiting_for_network_services {
                    retry_interval
                } else {
                    recovery_retry_delay(started_at, retry_window, retry_interval)
                };
                tracing::warn!(
                    target: "bifrost_core::system_proxy_recovery",
                    attempt,
                    retry_delay_ms = delay.as_millis() as u64,
                    retry_window_ms = retry_window.as_millis() as u64,
                    error = %error,
                    keep_waiting_for_network_services,
                    "system proxy recovery hit a retryable error; retrying"
                );
                std::thread::sleep(delay);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// The remaining sleep before the next retry, capped by the configured
/// `retry_interval` and floored at 1 ms so the loop always makes progress.
pub fn recovery_retry_delay(
    started_at: Instant,
    retry_window: Duration,
    retry_interval: Duration,
) -> Duration {
    let remaining = retry_window.saturating_sub(started_at.elapsed());
    retry_interval.min(remaining).max(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_messages_match_legacy_set() {
        let cases = [
            "networksetup -listallnetworkservices failed",
            "No enabled macOS network services were returned by networksetup",
            "Failed to execute launchctl print",
        ];
        for message in cases {
            let err = BifrostError::Config(message.to_string());
            assert!(is_retryable_recovery_error(&err), "{}", message);
        }

        let non_retryable = BifrostError::Config("permission denied: chmod".to_string());
        assert!(!is_retryable_recovery_error(&non_retryable));
    }

    #[test]
    fn network_services_not_ready_is_unbounded_readiness_error() {
        let err = BifrostError::Config(
            "No enabled macOS network services were returned by networksetup".to_string(),
        );
        assert!(is_network_services_not_ready_error(&err));

        let other_retryable = BifrostError::Config("networksetup transient".to_string());
        assert!(!is_network_services_not_ready_error(&other_retryable));
    }

    #[test]
    fn non_retryable_error_returns_immediately() {
        let started = Instant::now();
        let result: Result<(), _> = retry_with_policy(
            Duration::from_secs(5),
            Duration::from_millis(10),
            |attempt| {
                assert_eq!(attempt, 1);
                Err(BifrostError::Config("fatal: nothing to do".to_string()))
            },
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn retries_until_window_expires() {
        let started = Instant::now();
        let result: Result<(), _> = retry_with_policy(
            Duration::from_millis(120),
            Duration::from_millis(20),
            |_attempt| Err(BifrostError::Config("networksetup transient".to_string())),
        );
        assert!(result.is_err());
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(80),
            "expected to spend most of the window retrying, got {:?}",
            elapsed
        );
    }

    #[test]
    fn network_services_not_ready_retries_past_window_until_success() {
        let mut counter = 0u32;
        let result: Result<u32, _> = retry_with_policy(
            Duration::from_millis(1),
            Duration::from_millis(1),
            |attempt| {
                counter = attempt;
                if attempt < 3 {
                    Err(BifrostError::Config(
                        "No enabled macOS network services were returned by networksetup"
                            .to_string(),
                    ))
                } else {
                    Ok(attempt)
                }
            },
        );

        assert_eq!(result.unwrap(), 3);
        assert_eq!(counter, 3);
    }

    #[test]
    fn succeeds_after_transient_failures() {
        let mut counter = 0u32;
        let result: Result<u32, _> = retry_with_policy(
            Duration::from_secs(2),
            Duration::from_millis(10),
            |attempt| {
                counter = attempt;
                if attempt < 3 {
                    Err(BifrostError::Config("networksetup transient".to_string()))
                } else {
                    Ok(attempt)
                }
            },
        );
        assert_eq!(result.unwrap(), 3);
        assert_eq!(counter, 3);
    }
}
