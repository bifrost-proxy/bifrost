use super::*;
use std::process::Child;

fn recovery_exit(pid: u32) -> ManagedBackendExit {
    ManagedBackendExit {
        pid,
        exit_code: Some(1),
        exit_signal: None,
        detail: "test child exited".into(),
    }
}

fn write_recovery_marker(data_dir: &Path, pid: u32, port: u16, started: u64) {
    fs::write(data_dir.join("runtime.json"), format!(
        r#"{{"pid":{pid},"port":{port},"runtime_start_mode":"desktop","process_start_time_ms":{started}}}"#
    )).unwrap();
    fs::write(data_dir.join("bifrost.pid"), pid.to_string()).unwrap();
}

#[cfg(unix)]
fn recovery_sleep_child() -> Child {
    Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("start temporary child")
}

#[cfg(unix)]
fn attach_recovery_child(state: &BackendState) -> u32 {
    let child = recovery_sleep_child();
    let pid = child.id();
    *state.child.lock().unwrap() = Some(child);
    write_recovery_marker(&state.data_dir, pid, *state.port.lock().unwrap(), 1);
    pid
}

#[cfg(unix)]
fn reap_recovery_child(state: &BackendState) {
    if let Some(mut child) = state.child.lock().unwrap().take() {
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        child.wait().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn stale_watchdog_probe_cannot_kill_manual_replacement_after_barrier() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(test_backend_state(dir.path().into(), 19941, true, None));
    attach_recovery_child(&state);
    let old = BackendRecoverySnapshot::capture(&state).unwrap();
    let probe_started = Arc::new(Barrier::new(2));
    let replacement_finished = Arc::new(Barrier::new(2));
    let worker_state = state.clone();
    let worker_started = probe_started.clone();
    let worker_finished = replacement_finished.clone();
    let replacement = thread::spawn(move || {
        worker_started.wait();
        let guard = begin_backend_recovery(&worker_state).unwrap();
        worker_state
            .backend_lifecycle_epoch
            .fetch_add(1, Ordering::SeqCst);
        reap_recovery_child(&worker_state);
        let new_pid = attach_recovery_child(&worker_state);
        drop(guard);
        worker_finished.wait();
        new_pid
    });
    probe_started.wait();
    replacement_finished.wait();
    let new_pid = replacement.join().unwrap();
    // This is the production entry used after the slow health probe finishes.
    assert!(begin_observed_backend_recovery(&state, &old).is_none());
    let guard = begin_backend_recovery(&state).unwrap();
    assert!(!terminate_observed_backend(&state, &guard, &old).unwrap());
    let mut child = state.child.lock().unwrap();
    assert_eq!(child.as_ref().unwrap().id(), new_pid);
    assert!(child.as_mut().unwrap().try_wait().unwrap().is_none());
    drop(child);
    drop(guard);
    reap_recovery_child(&state);
}

#[test]
fn lifecycle_epoch_fences_same_pid_port_aba() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);
    assert!(begin_observed_backend_recovery(&state, &expected).is_none());
    assert!(!state.backend_recovery_in_progress.load(Ordering::SeqCst));
}

#[cfg(unix)]
#[test]
fn start_identity_and_runtime_marker_changes_reject_stale_observation() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let pid = attach_recovery_child(&state);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let mut wrong_start = expected.clone();
    wrong_start.child_start_time_ms = Some(expected.child_start_time_ms.unwrap_or(0) + 1);
    assert!(begin_observed_backend_recovery(&state, &wrong_start).is_none());
    write_recovery_marker(&state.data_dir, pid, 19941, 2);
    assert!(begin_observed_backend_recovery(&state, &expected).is_none());
    reap_recovery_child(&state);
}

#[test]
fn active_and_expected_port_changes_cancel_pending_probe() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    *state.port.lock().unwrap() = 19942;
    assert!(!expected.is_current(&state));
    *state.port.lock().unwrap() = 19941;
    *state.expected_port.lock().unwrap() = 19943;
    assert!(!expected.is_current(&state));
}

#[cfg(unix)]
#[test]
fn shutdown_cancels_queued_destructive_action_and_preserves_child() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    attach_recovery_child(&state);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_observed_backend_recovery(&state, &expected).unwrap();
    state.shutdown_started.store(true, Ordering::SeqCst);
    state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);
    assert!(!terminate_observed_backend(&state, &guard, &expected).unwrap());
    assert!(state
        .child
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    drop(guard);
    reap_recovery_child(&state);
}

#[cfg(unix)]
#[test]
fn confirmed_exit_does_not_adopt_new_external_runtime_generation() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let pid = attach_recovery_child(&state);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    reap_recovery_child(&state);
    write_recovery_marker(&state.data_dir, std::process::id(), 19941, 2);
    assert!(expected.after_owned_exit(&state, pid).is_none());
    assert!(state.child.lock().unwrap().is_none());
}

#[test]
fn external_runtime_is_never_terminated_or_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    write_recovery_marker(&state.data_dir, std::process::id(), 19941, 1);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_observed_backend_recovery(&state, &expected).unwrap();
    assert!(!terminate_observed_backend(&state, &guard, &expected).unwrap());
    assert!(matches!(
        attempt_backend_recovery_with_launcher(
            &state,
            &guard,
            &expected,
            &recovery_exit(1),
            |_| panic!("must not launch for external PID")
        ),
        BackendRecoveryResult::Cancelled
    ));
}

#[test]
fn failed_automatic_launch_schedules_real_retry_and_circuit_half_open() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let mut expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let mut budget = BackendRecoveryBudget::default();
    let now = Instant::now();
    for _ in 0..BACKEND_WATCHDOG_MAX_RECOVERIES {
        assert!(budget.try_acquire(now));
        let result = attempt_backend_recovery_with_launcher(
            &state,
            &guard,
            &expected,
            &recovery_exit(1),
            |_| Err(anyhow("spawn failed".into())),
        );
        let BackendRecoveryResult::Retry(retry) = result else {
            panic!("failure must schedule retry");
        };
        assert!(!retry.is_due(now));
        assert!(retry.is_due(retry.retry_at));
        assert!(retry.expected.is_current(&state));
        expected = retry.expected;
    }
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().unwrap().is_some());
    assert!(!budget.try_acquire(now + Duration::from_secs(10)));
    assert_eq!(
        budget.next_available_at(now),
        now + BACKEND_WATCHDOG_RECOVERY_WINDOW
    );
    assert!(budget.try_acquire(now + BACKEND_WATCHDOG_RECOVERY_WINDOW));
}

#[test]
fn failed_child_marker_is_retried_only_for_a_pid_spawned_by_this_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |pids| {
            pids.push(7123);
            write_recovery_marker(&state.data_dir, 7123, 19941, 1);
            Err(anyhow("owned replacement failed".into()))
        },
    );
    let BackendRecoveryResult::Retry(retry) = result else {
        panic!("owned failed child can retry");
    };
    assert_eq!(retry.exited.pid, 7123);
    assert!(retry.expected.is_current(&state));
}

#[test]
fn failed_launch_does_not_bless_racing_foreign_marker() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |pids| {
            pids.push(7123);
            write_recovery_marker(&state.data_dir, 8123, 19941, 1);
            Err(anyhow("foreign runtime won bind race".into()))
        },
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
    assert!(state.child.lock().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn shutdown_during_slow_launch_retains_child_for_shutdown_without_publishing_ready() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| {
            state.shutdown_started.store(true, Ordering::SeqCst);
            state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);
            Ok((Some(recovery_sleep_child()), 19942))
        },
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert_eq!(*state.port.lock().unwrap(), 19941);
    assert!(state.child.lock().unwrap().is_some());
    drop(guard);
    reap_recovery_child(&state);
}

#[test]
fn shutdown_during_failed_launch_cancels_retry() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| {
            state.shutdown_started.store(true, Ordering::SeqCst);
            Err(anyhow("cancelled launch".into()))
        },
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
}

#[test]
fn missing_health_lane_is_unavailable_without_permission_to_kill() {
    let mut health = BackendWatchdogHealth::default();
    let signals = BackendSignalSnapshot {
        admin_healthy: false,
        data_plane_healthy: false,
        health_lane_present: false,
        health_lane_healthy: false,
        scheduler_heartbeat_age_ms: None,
    };
    assert!(!confirms_managed_runtime_unresponsive(signals));
    let now = Instant::now();
    for seconds in [0, 5, 10] {
        assert!(matches!(
            health.observe_signals(signals, now + Duration::from_secs(seconds)),
            WatchdogProbeDisposition::Degraded { .. }
        ));
    }
    assert!(matches!(
        health.observe_signals(signals, now + Duration::from_secs(15)),
        WatchdogProbeDisposition::ConfirmRecovery { .. }
    ));
    assert!(matches!(
        health.observe_signals(
            BackendSignalSnapshot {
                admin_healthy: true,
                ..signals
            },
            now + Duration::from_secs(20)
        ),
        WatchdogProbeDisposition::Recovered { .. }
    ));
}

#[test]
fn healthy_scheduler_does_not_make_failed_admin_and_data_plane_available() {
    let mut health = BackendWatchdogHealth::default();
    let signals = BackendSignalSnapshot {
        admin_healthy: false,
        data_plane_healthy: false,
        health_lane_present: true,
        health_lane_healthy: true,
        scheduler_heartbeat_age_ms: Some(10),
    };
    assert!(!confirms_managed_runtime_unresponsive(signals));
    assert!(matches!(
        health.observe_signals(signals, Instant::now()),
        WatchdogProbeDisposition::Degraded { .. }
    ));
}

#[test]
fn mismatched_health_lane_pid_cannot_authorize_managed_child_kill() {
    assert!(health_lane_identity_matches(Some(7), Some(7)));
    assert!(health_lane_identity_matches(Some(7), None));
    assert!(!health_lane_identity_matches(Some(7), Some(8)));
    assert!(!health_lane_identity_matches(None, None));
}

#[cfg(unix)]
#[test]
fn successful_retry_publishes_only_owned_child_and_advances_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(
        dir.path().into(),
        19941,
        false,
        Some("previous failure".into()),
    );
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| Ok((Some(recovery_sleep_child()), 19941)),
    );
    assert!(matches!(result, BackendRecoveryResult::Recovered));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().unwrap().is_none());
    assert_eq!(
        state.backend_lifecycle_epoch.load(Ordering::SeqCst),
        expected.epoch + 1
    );
    assert!(state.child.lock().unwrap().is_some());
    drop(guard);
    reap_recovery_child(&state);
}

#[test]
fn external_reuse_does_not_claim_child_or_schedule_retry() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| Ok((None, 19941)),
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
    assert!(state.child.lock().unwrap().is_none());
}

#[test]
fn missing_binary_uses_production_launcher_and_schedules_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = test_backend_state(dir.path().into(), 19941, false, None);
    state.binary_path = dir.path().join("missing-bifrost-test-binary");
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    assert!(matches!(
        attempt_backend_recovery(&state, &guard, &expected, &recovery_exit(1)),
        BackendRecoveryResult::Retry(_)
    ));
    assert!(state.child.lock().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn current_owned_child_is_terminated_only_with_matching_guarded_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let pid = attach_recovery_child(&state);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_observed_backend_recovery(&state, &expected).unwrap();
    assert!(terminate_observed_backend(&state, &guard, &expected).unwrap());
    assert!(state.child.lock().unwrap().is_none());
    assert!(expected.after_owned_exit(&state, pid).is_some());
}

#[test]
fn manual_start_invalidates_pending_retry_even_after_due_time() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let BackendRecoveryResult::Retry(retry) = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| Err(anyhow("spawn failed".into())),
    ) else {
        panic!("pending retry required");
    };
    state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);
    assert!(retry.is_due(retry.retry_at));
    assert!(!retry.expected.is_current(&state));
    assert!(matches!(
        attempt_backend_recovery_with_launcher(
            &state,
            &guard,
            &retry.expected,
            &retry.exited,
            |_| panic!("manual start superseded retry")
        ),
        BackendRecoveryResult::Cancelled
    ));
}

#[test]
fn automatic_launch_passes_only_the_expected_proxy_generation() {
    let mut command = Command::new("bifrost-test");
    command.env(SYSTEM_PROXY_RECOVERY_GENERATION_ENV, "stale-inherited");
    configure_backend_recovery_generation(&mut command, Some("captured-generation"));
    assert!(command
        .get_envs()
        .any(|(name, value)| name == SYSTEM_PROXY_RECOVERY_GENERATION_ENV
            && value == Some(std::ffi::OsStr::new("captured-generation"))));
    configure_backend_recovery_generation(&mut command, Some(""));
    assert!(command
        .get_envs()
        .any(|(name, value)| name == SYSTEM_PROXY_RECOVERY_GENERATION_ENV
            && value == Some(std::ffi::OsStr::new(""))));
    configure_backend_recovery_generation(&mut command, None);
    assert!(command
        .get_envs()
        .any(|(name, value)| name == SYSTEM_PROXY_RECOVERY_GENERATION_ENV && value.is_none()));
}

#[cfg(unix)]
#[test]
fn old_lease_cleanup_after_exit_preserves_retry_without_authorizing_fresh_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let pid = attach_recovery_child(&state);
    let mut before = BackendRecoverySnapshot::capture(&state).unwrap();
    // Inject the pre-cleanup ownership observation. Core ownership writes are
    // intentionally never enabled in these process/lifecycle tests.
    before.proxy_generation = Some("old-generation".into());
    reap_recovery_child(&state);
    let next = before
        .after_owned_exit(&state, pid)
        .expect("released lease must not lose owned exit");
    assert!(next.proxy_generation.is_none());
    assert!(next.is_current(&state));
    assert!(recovery_generation_is_compatible(
        Some("old-generation"),
        None
    ));
    assert!(!recovery_generation_is_compatible(
        Some("old-generation"),
        Some("new-owner")
    ));
    assert!(!recovery_generation_is_compatible(None, Some("new-owner")));
}

#[test]
fn pending_retry_survives_old_journal_and_marker_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    write_recovery_marker(&state.data_dir, 7123, 19941, 1);
    let mut expected = BackendRecoverySnapshot::capture(&state).unwrap();
    expected.proxy_generation = Some("old-generation".into());
    fs::remove_file(state.data_dir.join("runtime.json")).unwrap();
    fs::remove_file(state.data_dir.join("bifrost.pid")).unwrap();
    let next = expected
        .refresh_retry_snapshot(&state)
        .expect("cleanup may release old metadata");
    assert!(next.proxy_generation.is_none());
    assert!(next.is_current(&state));
    write_recovery_marker(&state.data_dir, 8123, 19941, 1);
    assert!(next.refresh_retry_snapshot(&state).is_none());
}

#[cfg(unix)]
#[test]
fn failed_termination_keeps_owned_child_handle_for_later_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, true, None);
    let pid = attach_recovery_child(&state);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_observed_backend_recovery(&state, &expected).unwrap();
    assert!(
        terminate_observed_backend_with(&state, &guard, &expected, |_| Err(anyhow(
            "kill denied".into()
        )))
        .is_err()
    );
    assert_eq!(state.child.lock().unwrap().as_ref().unwrap().id(), pid);
    assert!(state
        .child
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    drop(guard);
    reap_recovery_child(&state);
}

#[test]
fn racing_foreign_pid_marker_without_runtime_marker_cancels_retry() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| {
            fs::write(state.data_dir.join("bifrost.pid"), "8123").unwrap();
            Err(anyhow("foreign PID appeared".into()))
        },
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
}

#[test]
fn malformed_runtime_identity_never_authorizes_automatic_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_backend_state(dir.path().into(), 19941, false, None);
    fs::write(
        state.data_dir.join("runtime.json"),
        "invalid-runtime-marker",
    )
    .unwrap();
    let expected = BackendRecoverySnapshot::capture(&state).unwrap();
    let guard = begin_backend_recovery(&state).unwrap();
    let result = attempt_backend_recovery_with_launcher(
        &state,
        &guard,
        &expected,
        &recovery_exit(1),
        |_| panic!("unverified runtime must be preserved"),
    );
    assert!(matches!(result, BackendRecoveryResult::Cancelled));
}
