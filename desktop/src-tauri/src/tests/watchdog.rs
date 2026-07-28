use super::*;

#[test]
fn desktop_watchdog_short_health_failures_stay_degraded_and_recover() {
    let started = Instant::now();
    let mut health = BackendWatchdogHealth::default();

    assert_eq!(
        health.observe_failure(started),
        WatchdogProbeDisposition::Degraded {
            failures: 1,
            degraded_for: Duration::ZERO,
        }
    );
    assert_eq!(
        health.observe_failure(started + Duration::from_secs(2)),
        WatchdogProbeDisposition::Degraded {
            failures: 2,
            degraded_for: Duration::from_secs(2),
        }
    );
    assert_eq!(
        health.observe_success(started + Duration::from_secs(3)),
        WatchdogProbeDisposition::Recovered {
            failures: 2,
            degraded_for: Duration::from_secs(3),
        }
    );
    assert_eq!(
        health.observe_success(started + Duration::from_secs(4)),
        WatchdogProbeDisposition::Healthy
    );
}

#[test]
fn desktop_watchdog_requires_failure_count_and_grace_window() {
    let started = Instant::now();

    let mut too_few_failures = BackendWatchdogHealth::default();
    assert!(matches!(
        too_few_failures.observe_failure(started),
        WatchdogProbeDisposition::Degraded { .. }
    ));
    assert_eq!(
        too_few_failures.observe_failure(started + Duration::from_secs(30)),
        WatchdogProbeDisposition::Degraded {
            failures: 2,
            degraded_for: Duration::from_secs(30),
        }
    );

    let mut too_little_time = BackendWatchdogHealth::default();
    for offset in 0..3 {
        assert!(matches!(
            too_little_time.observe_failure(started + Duration::from_secs(offset)),
            WatchdogProbeDisposition::Degraded { .. }
        ));
    }
    assert_eq!(
        too_little_time.observe_failure(started + Duration::from_secs(3)),
        WatchdogProbeDisposition::Degraded {
            failures: 4,
            degraded_for: Duration::from_secs(3),
        }
    );

    let mut sustained = BackendWatchdogHealth::default();
    for offset in [0, 5, 10] {
        assert!(matches!(
            sustained.observe_failure(started + Duration::from_secs(offset)),
            WatchdogProbeDisposition::Degraded { .. }
        ));
    }
    assert_eq!(
        sustained.observe_failure(started + Duration::from_secs(15)),
        WatchdogProbeDisposition::ConfirmRecovery {
            failures: 4,
            degraded_for: Duration::from_secs(15),
        }
    );
}

#[test]
fn desktop_watchdog_health_probe_reports_timeout_details() {
    let (port, server) = spawn_delayed_health_server(Duration::from_millis(150), 200);
    let result = probe_backend_health_with_timeout(port, Duration::from_millis(30));

    assert!(!result.healthy);
    assert!(result.elapsed >= Duration::from_millis(20));
    assert!(result
        .failure
        .as_deref()
        .expect("timeout failure")
        .contains("request failed"));
    server.join().expect("delayed health server join");
}

#[test]
fn desktop_watchdog_health_probe_rejects_non_success_status() {
    let (port, server) = spawn_delayed_health_server(Duration::ZERO, 503);
    let result = probe_backend_health_with_timeout(port, Duration::from_secs(1));

    assert!(!result.healthy);
    assert_eq!(result.failure.as_deref(), Some("HTTP status 503"));
    server.join().expect("health server join");
}
