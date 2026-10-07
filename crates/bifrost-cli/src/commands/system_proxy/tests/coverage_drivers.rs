use super::*;
use std::time::Duration;

fn runtime_fixture(data: &std::path::Path, live: bool) -> RuntimeInfo {
    let port = unused_loopback_port();
    write_restart_fixture(
        data,
        port,
        std::env::current_exe().unwrap(),
        true,
        port,
        true,
    );
    let mut runtime = read_runtime_info_from(data).unwrap();
    if live {
        runtime.pid = std::process::id();
        runtime.started_at_ms = bifrost_core::current_process_start_time_ms();
        save_runtime(data, &runtime);
    }
    runtime
}
fn save_runtime(data: &std::path::Path, runtime: &RuntimeInfo) {
    std::fs::write(
        data.join("runtime.json"),
        serde_json::to_vec(runtime).unwrap(),
    )
    .unwrap();
}
fn disabled(data: &std::path::Path) {
    persist_system_proxy_config(
        &ConfigManager::new(data.to_path_buf()).unwrap(),
        false,
        None,
    )
    .unwrap();
}

#[test]
fn recovery_preflight_preserves_unknown_identity_missing_intent_and_changed_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = runtime_fixture(dir.path(), true);
    let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
    assert_eq!(
        restart_managed_runtime_with_generation(
            dir.path(),
            Duration::ZERO,
            &mut proxy,
            Some("new-generation")
        ),
        ManagedRuntimeRestartOutcome::OwnershipChanged
    );
    runtime.pid = 0;
    save_runtime(dir.path(), &runtime);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::ZERO,
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::RecoveryFailed
    );
    runtime.pid = std::process::id();
    save_runtime(dir.path(), &runtime);
    std::fs::remove_file(dir.path().join("config.toml")).unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::ZERO,
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::RecoveryFailed
    );
    assert!(proxy.resume_calls.is_empty());
    assert!(proxy.suspend_calls.is_empty());
    std::fs::remove_file(dir.path().join("runtime.json")).unwrap();
    std::fs::create_dir(dir.path().join("runtime.json")).unwrap();
    assert!(read_runtime_info_from_checked(dir.path()).is_err());
}

#[test]
fn recovery_rereads_intent_and_stop_after_loading_ownership() {
    for case in ["disabled", "corrupt", "stopped", "unwritable_handoff"] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_fixture(dir.path(), false);
        let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
        let data = dir.path().to_path_buf();
        proxy.after_ensure = Some(Box::new(move || match case {
            "disabled" => disabled(&data),
            "corrupt" => std::fs::write(data.join("config.toml"), "bad [toml").unwrap(),
            "stopped" => bifrost_core::write_system_proxy_shutdown_mode(
                &data,
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
            )
            .unwrap(),
            _ => std::fs::create_dir(data.join(".system_proxy_shutdown_mode.lock")).unwrap(),
        }));
        let result = restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_millis(10),
            &mut proxy,
        );
        assert_eq!(
            result,
            match case {
                "corrupt" => ManagedRuntimeRestartOutcome::RecoveryFailed,
                "stopped" => ManagedRuntimeRestartOutcome::Cancelled,
                _ => ManagedRuntimeRestartOutcome::NotAttempted,
            }
        );
        assert!(proxy.resume_calls.is_empty());
        assert!(proxy.suspend_calls.is_empty());
    }
}

#[test]
fn live_recovery_disable_and_missing_intent_never_resume_and_report_truthful_results() {
    for case in [
        "disabled",
        "ownership_changed",
        "suspend_failed",
        "corrupt",
        "stop",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_fixture(dir.path(), true);
        let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
        if case == "ownership_changed" {
            proxy.suspend_outcome =
                Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
        }
        if case == "suspend_failed" {
            proxy.suspend_failures = 1;
        }
        let data = dir.path().to_path_buf();
        proxy.after_ensure = Some(Box::new(move || match case {
            "corrupt" => std::fs::write(data.join("config.toml"), "bad [toml").unwrap(),
            "stop" => bifrost_core::write_system_proxy_shutdown_mode(
                &data,
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
            )
            .unwrap(),
            _ => disabled(&data),
        }));
        let result = restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_millis(20),
            &mut proxy,
        );
        assert_eq!(
            result,
            match case {
                "ownership_changed" => ManagedRuntimeRestartOutcome::OwnershipChanged,
                "suspend_failed" | "corrupt" => ManagedRuntimeRestartOutcome::RecoveryFailed,
                _ => ManagedRuntimeRestartOutcome::Cancelled,
            }
        );
        assert!(proxy.resume_calls.is_empty());
    }
}

// ZERO deterministically exhausts the readiness budget before the first probe,
// including on fast hosts. Every adapter is a mock, even on Windows/macOS.
#[test]
fn recovery_deadline_checks_disabled_intent_before_either_policy_fallback() {
    use bifrost_core::GuardedSystemProxyTransition as Transition;

    for mode in ["fail-open", "fail-closed"] {
        for (transition, expected) in [
            (
                Some(Transition::Applied),
                ManagedRuntimeRestartOutcome::Cancelled,
            ),
            (
                Some(Transition::AlreadyInState),
                ManagedRuntimeRestartOutcome::Cancelled,
            ),
            (
                Some(Transition::OwnershipChanged),
                ManagedRuntimeRestartOutcome::OwnershipChanged,
            ),
            (
                Some(Transition::NotManaged),
                ManagedRuntimeRestartOutcome::OwnershipChanged,
            ),
            (None, ManagedRuntimeRestartOutcome::RecoveryFailed),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let runtime = runtime_fixture(dir.path(), true);
            persist_system_proxy_recovery_policy(
                &ConfigManager::new(dir.path().into()).unwrap(),
                mode,
                3,
            )
            .unwrap();
            let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
            // Applied uses the mock's actual state transition.
            if transition == Some(Transition::AlreadyInState) {
                proxy = TestManagedProxyRecovery::new(runtime.port, false);
                proxy.suspend_outcome = transition;
            } else if transition.is_none() {
                proxy.suspend_failures = 1;
            } else if transition != Some(Transition::Applied) {
                proxy.suspend_outcome = transition;
            }
            let data = dir.path().to_path_buf();
            proxy.after_ensure = Some(Box::new(move || disabled(&data)));

            assert_eq!(
                restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                    dir.path(),
                    Duration::ZERO,
                    &mut proxy,
                ),
                expected,
                "{mode}: {transition:?}",
            );
            assert_eq!(proxy.suspend_calls, ["generation-fixture"]);
            assert!(proxy.resume_calls.is_empty());
            assert_eq!(
                proxy.ownership.as_ref().unwrap().applied,
                expected != ManagedRuntimeRestartOutcome::Cancelled,
            );
            let config = read_system_proxy_config(dir.path()).unwrap();
            assert!(!config.enabled);
            assert_eq!(config.intent_revision, 1);
            let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 10).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, "helper_recovery_cancelled");
            assert_eq!(
                events[0].ownership_generation.as_deref(),
                Some("generation-fixture")
            );
            assert_eq!(events[0].error.is_some(), transition.is_none());
            let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(owner.phase.as_deref(), Some("desired_disabled"));
            assert_eq!(owner.last_error.is_some(), transition.is_none());
        }
    }
}

#[test]
fn recovery_deadline_stop_and_unknown_intent_never_apply_either_policy() {
    for mode in ["fail-open", "fail-closed"] {
        for case in [
            "foreground_stop",
            "background_stop",
            "stop_and_corrupt",
            "missing",
            "corrupt",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let runtime = runtime_fixture(dir.path(), true);
            persist_system_proxy_recovery_policy(
                &ConfigManager::new(dir.path().into()).unwrap(),
                mode,
                3,
            )
            .unwrap();
            let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
            let data = dir.path().to_path_buf();
            let stopping = case.contains("stop");
            let shutdown_mode = if case == "background_stop" {
                bifrost_core::SystemProxyShutdownMode::BackgroundCleanup
            } else {
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
            };
            proxy.after_ensure = Some(Box::new(move || {
                if case == "missing" {
                    std::fs::remove_file(data.join("config.toml")).unwrap();
                } else if case.contains("corrupt") {
                    std::fs::write(data.join("config.toml"), "bad [toml").unwrap();
                }
                if stopping {
                    bifrost_core::write_system_proxy_shutdown_mode(&data, shutdown_mode).unwrap();
                }
            }));

            assert_eq!(
                restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                    dir.path(),
                    Duration::ZERO,
                    &mut proxy,
                ),
                if stopping {
                    ManagedRuntimeRestartOutcome::Cancelled
                } else {
                    ManagedRuntimeRestartOutcome::RecoveryFailed
                },
                "{mode}: {case}",
            );
            assert!(proxy.suspend_calls.is_empty());
            assert!(proxy.resume_calls.is_empty());
            assert!(proxy.ownership.as_ref().unwrap().applied);
            assert!(
                bifrost_core::read_recent_system_proxy_events(dir.path(), 10)
                    .unwrap()
                    .is_empty()
            );
            if stopping {
                assert_eq!(
                    bifrost_core::read_system_proxy_shutdown_mode(dir.path()),
                    Some(shutdown_mode)
                );
            }
            if case == "missing" {
                assert!(!dir.path().join("config.toml").exists());
            } else if case.contains("corrupt") {
                assert_eq!(
                    std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
                    "bad [toml"
                );
            }
        }
    }
}

#[test]
fn recovery_deadline_resolves_current_runtime_intent_before_fallback() {
    for mode in ["fail-open", "fail-closed"] {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = runtime_fixture(dir.path(), true);
        persist_system_proxy_recovery_policy(
            &ConfigManager::new(dir.path().into()).unwrap(),
            mode,
            3,
        )
        .unwrap();
        let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
        runtime.system_proxy_enabled = Some(false);
        proxy.replace_runtime_on_ensure = Some((dir.path().into(), runtime));

        assert_eq!(
            restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                dir.path(),
                Duration::ZERO,
                &mut proxy,
            ),
            ManagedRuntimeRestartOutcome::Cancelled,
        );
        assert_eq!(proxy.suspend_calls, ["generation-fixture"]);
        assert!(proxy.resume_calls.is_empty());
        assert!(!proxy.ownership.as_ref().unwrap().applied);
        assert!(read_system_proxy_config(dir.path()).unwrap().enabled);
    }
}

#[test]
fn recovery_deadline_keeps_enabled_policy_fallback_without_resuming() {
    for mode in ["fail-open", "fail-closed"] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_fixture(dir.path(), true);
        persist_system_proxy_recovery_policy(
            &ConfigManager::new(dir.path().into()).unwrap(),
            mode,
            3,
        )
        .unwrap();
        let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
        let fail_open = mode == "fail-open";

        assert_eq!(
            restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                dir.path(),
                Duration::ZERO,
                &mut proxy,
            ),
            if fail_open {
                ManagedRuntimeRestartOutcome::FailOpenSuspended
            } else {
                ManagedRuntimeRestartOutcome::FailClosedPreserved
            },
        );
        assert_eq!(proxy.suspend_calls.len(), usize::from(fail_open));
        assert!(proxy.resume_calls.is_empty());
        assert_eq!(proxy.ownership.as_ref().unwrap().applied, !fail_open);
        assert!(read_system_proxy_config(dir.path()).unwrap().enabled);
        let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "helper_runtime_restart_not_ready");
    }
}

#[test]
fn desktop_exit_preserves_fail_closed_stop_and_failed_suspend_states() {
    for case in ["closed", "stop", "lost", "failed"] {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = runtime_fixture(dir.path(), false);
        runtime.start_mode = RuntimeStartMode::Desktop;
        runtime.restartable_runtime = false;
        save_runtime(dir.path(), &runtime);
        let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
        match case {
            "closed" => {
                persist_system_proxy_recovery_policy(
                    &ConfigManager::new(dir.path().into()).unwrap(),
                    "fail-closed",
                    3,
                )
                .unwrap();
            }
            "stop" => {
                let path = dir.path().to_path_buf();
                proxy.after_ensure = Some(Box::new(move || {
                    bifrost_core::write_system_proxy_shutdown_mode(
                        &path,
                        bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
                    )
                    .unwrap()
                }));
            }
            "lost" => {
                proxy.suspend_outcome =
                    Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged)
            }
            _ => proxy.suspend_failures = 1,
        }
        assert_eq!(
            restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                dir.path(),
                Duration::ZERO,
                &mut proxy
            ),
            match case {
                "closed" => ManagedRuntimeRestartOutcome::FailClosedPreserved,
                "stop" => ManagedRuntimeRestartOutcome::Cancelled,
                "lost" => ManagedRuntimeRestartOutcome::OwnershipChanged,
                _ => ManagedRuntimeRestartOutcome::RecoveryFailed,
            }
        );
        assert!(proxy.resume_calls.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn wake_driver_covers_observational_guards_and_truthful_recovery_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let never = |_: &std::path::Path| -> ManagedRuntimeRestartOutcome {
        panic!("wake guard must not attempt recovery")
    };
    reconcile_system_proxy_after_power_wake_with(dir.path(), None, None, never).unwrap();
    let mut runtime = runtime_fixture(dir.path(), true);
    reconcile_system_proxy_after_power_wake_with(
        dir.path(),
        Some(runtime.pid + 1),
        runtime.started_at_ms,
        never,
    )
    .unwrap();
    reconcile_system_proxy_after_power_wake_with(
        dir.path(),
        Some(runtime.pid),
        Some(runtime.started_at_ms.unwrap_or(0) + 10_000),
        never,
    )
    .unwrap();
    reconcile_system_proxy_after_power_wake_with(dir.path(), None, None, never).unwrap();
    disabled(dir.path());
    reconcile_system_proxy_after_power_wake_with(
        dir.path(),
        Some(runtime.pid),
        runtime.started_at_ms,
        never,
    )
    .unwrap();
    persist_system_proxy_config(&ConfigManager::new(dir.path().into()).unwrap(), true, None)
        .unwrap();
    for outcome in [
        ManagedRuntimeRestartOutcome::Ready,
        ManagedRuntimeRestartOutcome::Cancelled,
        ManagedRuntimeRestartOutcome::OwnershipChanged,
        ManagedRuntimeRestartOutcome::NotAttempted,
        ManagedRuntimeRestartOutcome::RecoveryFailed,
        ManagedRuntimeRestartOutcome::FailOpenSuspended,
        ManagedRuntimeRestartOutcome::FailClosedPreserved,
    ] {
        let mut attempts = 0;
        let result = reconcile_system_proxy_after_power_wake_with(
            dir.path(),
            Some(runtime.pid),
            runtime.started_at_ms,
            |_| {
                attempts += 1;
                outcome
            },
        );
        assert_eq!(attempts, 1);
        assert_eq!(
            result.is_err(),
            matches!(
                outcome,
                ManagedRuntimeRestartOutcome::RecoveryFailed
                    | ManagedRuntimeRestartOutcome::FailOpenSuspended
                    | ManagedRuntimeRestartOutcome::FailClosedPreserved
            )
        );
    }
    // Reused old-parent identity with a changed marker is observational too.
    runtime.started_at_ms = Some(runtime.started_at_ms.unwrap_or(0).saturating_add(1));
    save_runtime(dir.path(), &runtime);
    bifrost_core::write_system_proxy_shutdown_mode(
        dir.path(),
        bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
    )
    .unwrap();
    reconcile_system_proxy_after_power_wake(dir.path(), Some(runtime.pid), runtime.started_at_ms)
        .unwrap();
}

fn in_isolated_home(name: &str, run: impl FnOnce()) {
    const CHILD: &str = "BIFROST_PROXY_COVERAGE_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(home.join(".coverage-test-owned").is_file());
        run();
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".coverage-test-owned"), "isolated").unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("HOME", home.path())
        .env("BIFROST_DATA_DIR", home.path().join("data"))
        .env("BIFROST_DISABLE_TRAY", "1")
        .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
        .output()
        .unwrap();
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.status.success(), "{output}");
    assert!(output.contains("running 1 test"), "{output}");
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn isolated_cleanup_and_parent_exit_execute_production_without_os_mutation() {
    in_isolated_home("commands::system_proxy::tests::coverage_drivers::isolated_cleanup_and_parent_exit_execute_production_without_os_mutation", || {
        assert!(!bifrost_core::SystemProxyManager::is_supported());
        let data = PathBuf::from(std::env::var_os("BIFROST_DATA_DIR").unwrap());
        let runtime = runtime_fixture(&data, false);
        let fence = RuntimeCleanupFence { runtime: Some(runtime.clone()), parent_pid: None, parent_started_at_ms: None, generation: Some("generation-fixture".into()) };
        assert!(fence.permits_cleanup(&data).unwrap());
        cleanup_owned_proxy_after_exit(&data, &fence).unwrap();
        let profiles = bifrost_core::CliProxyEnvironmentManager::new(bifrost_core::CliProxyShell::Bash).unwrap().disable().unwrap();
        assert!(profiles.changed_paths.is_empty());
        cleanup_or_restart_managed_runtime(&data, &fence).unwrap();
        bifrost_core::write_system_proxy_shutdown_mode(&data, bifrost_core::SystemProxyShutdownMode::BackgroundCleanup).unwrap();
        cleanup_after_parent_exit(&data, Some(runtime.pid), runtime.started_at_ms, LifecycleRecoveryTrigger::PidMissing).unwrap();
        assert!(bifrost_core::read_system_proxy_shutdown_mode(&data).is_none());
        let events = bifrost_core::read_recent_system_proxy_events(&data, 20).unwrap();
        assert!(events.iter().any(|event| event.decision.as_deref() == Some("background_cleanup")));
        let mut other = runtime.clone(); other.pid += 1; save_runtime(&data, &other);
        assert!(!fence.permits_cleanup(&data).unwrap());
        save_runtime(&data, &runtime);
        let mut manager = bifrost_core::SystemProxyManager::new(data.clone());
        assert_eq!(ManagedSystemProxyRecovery::suspend_managed_if_generation_guarded(&mut manager, "missing", || Ok(true)).unwrap(), bifrost_core::GuardedSystemProxyTransition::NotManaged);
        // Wake's real adapter is safe and observational on an unsupported OS.
        reconcile_system_proxy_after_power_wake(&data, Some(runtime.pid), runtime.started_at_ms).unwrap();
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn isolated_cleanup_failure_respects_fail_closed_and_desktop_policy() {
    in_isolated_home("commands::system_proxy::tests::coverage_drivers::isolated_cleanup_failure_respects_fail_closed_and_desktop_policy", || {
        assert!(!bifrost_core::SystemProxyManager::is_supported());
        let data = PathBuf::from(std::env::var_os("BIFROST_DATA_DIR").unwrap());
        let mut runtime = runtime_fixture(&data, false);
        for (mode, desktop) in [("fail_open", false), ("fail_closed", false), ("fail_open", true)] {
            runtime.start_mode = if desktop { RuntimeStartMode::Desktop } else { RuntimeStartMode::Daemon };
            save_runtime(&data, &runtime);
            std::fs::write(data.join("config.toml"), "bad [toml").unwrap();
            bifrost_core::update_system_proxy_owner_state(&data, |state| state.recovery_mode = Some(mode.into())).unwrap();
            let fence = RuntimeCleanupFence { runtime: Some(runtime.clone()), parent_pid: Some(runtime.pid), parent_started_at_ms: runtime.started_at_ms, generation: Some("generation-fixture".into()) };
            assert!(cleanup_or_restart_managed_runtime(&data, &fence).is_err());
            assert!(data.join("proxy_state.json").exists());
        }
    });
}

#[test]
fn isolated_target_routing_uses_one_live_runtime_and_rejects_other_targets() {
    in_isolated_home("commands::system_proxy::tests::coverage_drivers::isolated_target_routing_uses_one_live_runtime_and_rejects_other_targets", || {
        let data = PathBuf::from(std::env::var_os("BIFROST_DATA_DIR").unwrap());
        let mut runtime = runtime_fixture(&data, true);
        assert_eq!(running_runtime_admin_port_for_target("127.0.0.1", runtime.port), Some(runtime.port));
        assert_eq!(running_runtime_admin_port_for_target("127.0.0.1", runtime.port.saturating_sub(1)), None);
        runtime.started_at_ms = Some(runtime.started_at_ms.unwrap_or(0).saturating_add(10_000));
        save_runtime(&data, &runtime);
        cleanup_after_parent_exit(&data, Some(runtime.pid), Some(runtime.started_at_ms.unwrap() - 10_000), LifecycleRecoveryTrigger::PidReused).unwrap();
        assert_eq!(running_runtime_admin_port_for_target("127.0.0.1", runtime.port), None);
    });
}

#[test]
fn data_canary_connection_refusal_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_fixture(dir.path(), true);
    assert!(!runtime_data_plane_is_ready(&runtime));
}

struct ScriptedCanary {
    port: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl ScriptedCanary {
    fn new(mut after_request: impl FnMut(usize) + Send + 'static) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut requests = 0;
            while !flag.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = [0; 1024];
                    let _ = stream.read(&mut request);
                    requests += 1;
                    after_request(requests);
                    let _ =
                        stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for ScriptedCanary {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn late_resume_case(case: &'static str, expected: ManagedRuntimeRestartOutcome) {
    let server = ScriptedCanary::new(|_| {});
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = runtime_fixture(dir.path(), true);
    runtime.port = server.port;
    save_runtime(dir.path(), &runtime);
    let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
    let path = dir.path().to_path_buf();
    match case {
        "disable_lost" => {
            proxy.disable_on_resume = Some(path);
            proxy.suspend_outcome =
                Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
        }
        "disable_failed" => {
            proxy.disable_on_resume = Some(path);
            proxy.suspend_failures = 1;
        }
        "corrupt" => {
            proxy.after_resume = Some(Box::new(move || {
                std::fs::write(path.join("config.toml"), "bad [toml").unwrap()
            }))
        }
        "stop" => {
            proxy.after_resume = Some(Box::new(move || {
                bifrost_core::write_system_proxy_shutdown_mode(
                    &path,
                    bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
                )
                .unwrap()
            }))
        }
        "replacement" => {
            proxy.after_resume = Some(Box::new(move || {
                runtime.pid = 0;
                save_runtime(&path, &runtime);
            }))
        }
        "reused_start" => {
            proxy.after_resume = Some(Box::new(move || {
                runtime.started_at_ms =
                    Some(runtime.started_at_ms.unwrap_or(0).saturating_add(10_000));
                save_runtime(&path, &runtime);
            }))
        }
        _ => unreachable!(),
    }
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(6),
            &mut proxy
        ),
        expected
    );
    assert_eq!(proxy.resume_calls.len(), 1);
    let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 30).unwrap();
    assert!(!events
        .iter()
        .any(|event| event.event == "helper_runtime_restart_ready"));
}

#[test]
fn resume_compensation_ownership_loss_is_never_ready() {
    late_resume_case(
        "disable_lost",
        ManagedRuntimeRestartOutcome::OwnershipChanged,
    );
}
#[test]
fn resume_compensation_failure_is_never_ready() {
    late_resume_case(
        "disable_failed",
        ManagedRuntimeRestartOutcome::RecoveryFailed,
    );
}
#[test]
fn unreadable_intent_after_resume_is_unconfirmed() {
    late_resume_case("corrupt", ManagedRuntimeRestartOutcome::RecoveryFailed);
}
#[test]
fn explicit_stop_after_resume_is_not_consumed_or_reported_ready() {
    late_resume_case("stop", ManagedRuntimeRestartOutcome::Cancelled);
}
#[test]
fn replacement_during_resume_resets_canary_history() {
    late_resume_case(
        "replacement",
        ManagedRuntimeRestartOutcome::FailOpenSuspended,
    );
}

fn change_during_canary(case: &'static str, expected: ManagedRuntimeRestartOutcome) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let mut first_request = None;
    let mut changed = false;
    let server = ScriptedCanary::new(move |requests| {
        let first = *first_request.get_or_insert_with(std::time::Instant::now);
        // The server observes request one before the driver's first success.
        // Mutate before the first response that can satisfy the 2s/3-sample
        // gate, independent of per-probe scheduling or instrumentation cost.
        if !changed && requests >= 3 && first.elapsed() >= Duration::from_secs(2) {
            changed = true;
            if case == "disable" {
                disabled(&path);
            } else {
                let mut runtime = read_runtime_info_from(&path).unwrap();
                runtime.pid = 0;
                save_runtime(&path, &runtime);
            }
        }
    });
    let mut runtime = runtime_fixture(dir.path(), true);
    runtime.port = server.port;
    save_runtime(dir.path(), &runtime);
    let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(3),
            &mut proxy
        ),
        expected
    );
    assert!(proxy.resume_calls.is_empty());
    assert_eq!(proxy.suspend_calls.len(), 1);
}
#[test]
fn latest_disable_during_final_canary_prevents_resume() {
    change_during_canary("disable", ManagedRuntimeRestartOutcome::Cancelled);
}
#[test]
fn marker_replacement_during_final_canary_prevents_resume() {
    change_during_canary("replace", ManagedRuntimeRestartOutcome::FailOpenSuspended);
}

#[test]
fn grace_suspend_ownership_change_ends_recovery_without_reenable() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_fixture(dir.path(), true);
    persist_system_proxy_recovery_policy(
        &ConfigManager::new(dir.path().into()).unwrap(),
        "fail-open",
        3,
    )
    .unwrap();
    let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
    proxy.suspend_outcome = Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(5),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::OwnershipChanged
    );
    assert_eq!(proxy.suspend_calls.len(), 1);
    assert!(proxy.resume_calls.is_empty());
}

#[cfg(unix)]
#[test]
fn power_notification_driver_coalesces_wakes_and_retries_until_success() {
    let (sender, events) = std::sync::mpsc::channel();
    let mut pending = false;
    for event in [
        PowerEvent::CanSystemSleep,
        PowerEvent::SystemWillSleep,
        PowerEvent::Unknown(7),
    ] {
        sender.send(event).unwrap();
    }
    process_power_notifications(&events, &mut pending, || {
        panic!("non-wake events must not acquire proxy")
    });
    assert!(!pending);
    sender.send(PowerEvent::SystemHasPoweredOn).unwrap();
    sender.send(PowerEvent::SystemHasPoweredOn).unwrap();
    let mut attempts = 0;
    process_power_notifications(&events, &mut pending, || {
        attempts += 1;
        Err(cleanup_error("not ready yet"))
    });
    assert!(pending);
    assert_eq!(attempts, 1);
    process_power_notifications(&events, &mut pending, || {
        attempts += 1;
        Ok(())
    });
    assert!(!pending);
    assert_eq!(attempts, 2);
    drop(sender);
    process_power_notifications(&events, &mut pending, || {
        panic!("completed wake must not be repeated")
    });
}

#[test]
fn process_group_sentinels_cannot_become_runtime_readiness_identities() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = runtime_fixture(dir.path(), true);
    assert!(runtime_identity_is_current(&runtime));
    runtime.started_at_ms = None;
    assert!(
        runtime_identity_is_current(&runtime),
        "legacy valid process identity remains supported"
    );
    for invalid in [0, u32::MAX] {
        runtime.pid = invalid;
        assert!(!runtime_identity_is_current(&runtime));
    }
}

#[test]
fn reused_start_identity_during_resume_cannot_reuse_ready_history() {
    late_resume_case(
        "reused_start",
        ManagedRuntimeRestartOutcome::FailOpenSuspended,
    );
}
