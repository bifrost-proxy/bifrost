use super::*;
use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

struct CanaryServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl CanaryServer {
    fn start(response: &'static [u8]) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut request = [0; 1024];
                        let _ = stream.read(&mut request);
                        let _ = stream.write_all(response);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for CanaryServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}

fn live_fixture(data_dir: &std::path::Path, port: u16) -> RuntimeInfo {
    write_restart_fixture(
        data_dir,
        port,
        std::env::current_exe().unwrap(),
        true,
        port,
        false,
    );
    let mut runtime = read_runtime_info_from(data_dir).unwrap();
    runtime.pid = std::process::id();
    runtime.started_at_ms = bifrost_core::current_process_start_time_ms();
    std::fs::write(
        data_dir.join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    runtime
}

#[test]
fn readiness_requires_stability_and_resets_on_failure_or_new_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = live_fixture(dir.path(), unused_loopback_port());
    let mut gate = StableReadiness::default();
    for ms in [0, 200, 400] {
        assert!(!gate.observe(Some(&runtime), true, Duration::from_millis(ms)));
    }
    assert!(gate.observe(Some(&runtime), true, Duration::from_secs(2)));
    assert!(!gate.observe(Some(&runtime), false, Duration::from_secs(3)));
    assert!(!gate.observe(Some(&runtime), true, Duration::from_secs(4)));
    assert!(!gate.observe(Some(&runtime), true, Duration::from_secs(5)));
    runtime.pid += 1;
    assert!(!gate.observe(Some(&runtime), true, Duration::from_secs(6)));
    assert!(!gate.observe(Some(&runtime), true, Duration::from_secs(7)));
    assert!(gate.observe(Some(&runtime), true, Duration::from_secs(8)));
    assert!(!gate.observe(None, true, Duration::from_secs(9)));
}

#[test]
fn current_disabled_intent_beats_legacy_runtime_and_restart_args() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = live_fixture(dir.path(), unused_loopback_port());
    runtime.system_proxy_config_revision = None;
    let configured = bifrost_storage::NewSystemProxyConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(!should_try_managed_runtime_restart(&runtime, &configured));
    let args = build_managed_runtime_restart_args(&runtime, &(false, "new-bypass".into()));
    assert!(args.iter().any(|arg| arg == "--no-system-proxy"));
    assert!(!args.iter().any(|arg| arg == "--system-proxy"));
    runtime.system_proxy_enabled = Some(false);
    assert!(should_try_managed_runtime_restart(
        &runtime,
        &Default::default()
    ));
}

#[test]
fn canary_rejects_http_failure_and_tcp_only_acceptance() {
    for response in [
        b"HTTP/1.1 503 Unavailable\r\n\r\n".as_slice(),
        b"HTTP/1.1 2040 Invalid\r\n\r\n",
        b"",
    ] {
        let server = CanaryServer::start(response);
        let dir = tempfile::tempdir().unwrap();
        let runtime = live_fixture(dir.path(), server.port);
        assert!(!runtime_data_plane_is_ready(&runtime));
    }
}

#[test]
fn failed_suspend_and_lost_ownership_are_never_reported_as_fail_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = TestManagedProxyRecovery::new(unused_loopback_port(), true);
    proxy.suspend_failures = 1;
    assert_eq!(
        apply_recovery_policy_after_failed_restart(
            dir.path(),
            &mut proxy,
            "generation-fixture",
            SystemProxyRecoveryMode::FailOpen,
            std::time::Instant::now(),
            false,
            None
        ),
        ManagedRuntimeRestartOutcome::RecoveryFailed
    );
    let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(owner.phase.as_deref(), Some("recovery_failed"));
    assert!(owner.last_error.unwrap().contains("temporary suspend"));
    proxy.suspend_outcome = Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
    assert_eq!(
        apply_recovery_policy_after_failed_restart(
            dir.path(),
            &mut proxy,
            "generation-fixture",
            SystemProxyRecoveryMode::FailOpen,
            std::time::Instant::now(),
            false,
            None
        ),
        ManagedRuntimeRestartOutcome::OwnershipChanged
    );
}

#[test]
fn stable_runtime_retries_failed_resume_without_false_ready_event() {
    let server = CanaryServer::start(b"HTTP/1.1 204 No Content\r\n\r\n");
    let dir = tempfile::tempdir().unwrap();
    live_fixture(dir.path(), server.port);
    let mut proxy = TestManagedProxyRecovery::new(server.port, false);
    proxy.resume_failures = 1;
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(6),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::Ready
    );
    assert_eq!(proxy.resume_calls.len(), 2);
    let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 20).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event == "helper_runtime_restart_ready")
            .count(),
        1
    );
    assert!(events
        .iter()
        .any(|event| event.event == "helper_proxy_resume_failed"));
}

#[test]
fn stable_runtime_reports_ownership_change_instead_of_ready() {
    let server = CanaryServer::start(b"HTTP/1.1 204 No Content\r\n\r\n");
    let dir = tempfile::tempdir().unwrap();
    live_fixture(dir.path(), server.port);
    let mut proxy = TestManagedProxyRecovery::new(server.port, false);
    proxy.resume_outcome = Some(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(4),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::OwnershipChanged
    );
    assert!(
        !bifrost_core::read_recent_system_proxy_events(dir.path(), 20)
            .unwrap()
            .iter()
            .any(|event| event.event == "helper_runtime_restart_ready")
    );
}

#[test]
fn disable_during_resume_is_compensated_without_overwriting_latest_intent() {
    let server = CanaryServer::start(b"HTTP/1.1 204 No Content\r\n\r\n");
    let dir = tempfile::tempdir().unwrap();
    live_fixture(dir.path(), server.port);
    let mut proxy = TestManagedProxyRecovery::new(server.port, false);
    proxy.disable_on_resume = Some(dir.path().to_path_buf());
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(4),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::Cancelled
    );
    assert_eq!(proxy.suspend_calls.len(), 1);
    assert!(!read_system_proxy_config(dir.path()).unwrap().enabled);
}

#[cfg(unix)]
#[test]
fn fail_open_retries_transient_suspend_after_grace() {
    let dir = tempfile::tempdir().unwrap();
    let port = unused_loopback_port();
    write_restart_fixture(dir.path(), port, "/usr/bin/true".into(), true, port, true);
    let config = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    persist_system_proxy_recovery_policy(
        &config,
        "fail-open",
        MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
    )
    .unwrap();
    let mut proxy = TestManagedProxyRecovery::new(port, true);
    proxy.suspend_failures = 1;
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_millis(3600),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::FailOpenSuspended
    );
    assert_eq!(proxy.suspend_calls.len(), 3);
    let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 20).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event == "helper_fail_open_failed"));
    assert!(events
        .iter()
        .any(|event| event.event == "helper_fail_open_applied"));
}

#[test]
fn stop_and_superseding_runtime_prevent_old_helper_actions() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = live_fixture(dir.path(), unused_loopback_port());
    let mut proxy = TestManagedProxyRecovery::new(runtime.port, true);
    bifrost_core::write_system_proxy_shutdown_mode(
        dir.path(),
        bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
    )
    .unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::ZERO,
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::Cancelled
    );
    assert!(proxy.suspend_calls.is_empty());
    assert!(proxy.resume_calls.is_empty());
    cleanup_after_parent_exit(
        dir.path(),
        Some(runtime.pid + 1),
        runtime.started_at_ms,
        LifecycleRecoveryTrigger::PidMissing,
    )
    .unwrap();
    assert!(!dir.path().join("system_proxy_owner_state.json").exists());
    assert_eq!(read_runtime_info_from(dir.path()).unwrap().pid, runtime.pid);
}

#[test]
fn replacement_published_during_preflight_is_never_force_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let port = unused_loopback_port();
    write_restart_fixture(
        dir.path(),
        port,
        std::env::current_exe().unwrap(),
        true,
        port,
        true,
    );
    let mut replacement = read_runtime_info_from(dir.path()).unwrap();
    replacement.pid = std::process::id();
    replacement.started_at_ms = bifrost_core::current_process_start_time_ms();
    let mut proxy = TestManagedProxyRecovery::new(port, true);
    proxy.replace_runtime_on_ensure = Some((dir.path().to_path_buf(), replacement.clone()));
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_millis(20),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::Cancelled
    );
    assert!(proxy.resume_calls.is_empty());
    assert!(proxy.suspend_calls.is_empty());
    assert_eq!(
        read_runtime_info_from(dir.path()).unwrap().pid,
        replacement.pid
    );
    assert!(
        !build_managed_runtime_restart_args(&replacement, &(true, "localhost".into()))
            .iter()
            .any(|arg| arg == "--yes")
    );
    assert_eq!(
        bifrost_core::read_system_proxy_shutdown_mode(dir.path()),
        None
    );
}

#[test]
fn missing_or_corrupt_intent_never_creates_default_enabled_config() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_system_proxy_config(dir.path()).is_err());
    assert!(!dir.path().join("config.toml").exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    std::fs::write(dir.path().join("config.toml"), "this is not valid = [").unwrap();
    assert!(read_system_proxy_config(dir.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
        "this is not valid = ["
    );
}

#[test]
fn direct_command_failure_keeps_accepted_enable_and_disable_intent() {
    for enabled in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigManager::new(dir.path().to_path_buf()).unwrap();
        let result: bifrost_core::Result<()> = with_accepted_system_proxy_intent(
            &config,
            enabled,
            Some("accepted-bypass".into()),
            |_| {
                let current = read_system_proxy_config(dir.path()).unwrap();
                assert_eq!(current.enabled, enabled);
                assert_eq!(current.bypass, "accepted-bypass");
                Err(cleanup_error("simulated OS permission failure"))
            },
        );
        assert!(result.is_err());
        assert_eq!(
            read_system_proxy_config(dir.path()).unwrap().enabled,
            enabled
        );
    }
}

#[test]
fn exit_cleanup_fence_rejects_same_generation_replacement_and_live_runtime() {
    let dir = tempfile::tempdir().unwrap();
    write_restart_fixture(
        dir.path(),
        unused_loopback_port(),
        std::env::current_exe().unwrap(),
        true,
        unused_loopback_port(),
        true,
    );
    let runtime = read_runtime_info_from(dir.path()).unwrap();
    let fence = RuntimeCleanupFence {
        runtime: Some(runtime.clone()),
        parent_pid: Some(runtime.pid),
        parent_started_at_ms: runtime.started_at_ms,
        generation: Some("same-generation".into()),
    };
    assert!(fence.permits_cleanup(dir.path()).unwrap());
    let mut replacement = runtime.clone();
    replacement.pid = std::process::id();
    replacement.started_at_ms = bifrost_core::current_process_start_time_ms();
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&replacement).unwrap(),
    )
    .unwrap();
    assert!(!fence.permits_cleanup(dir.path()).unwrap());
    std::fs::write(dir.path().join("runtime.json"), "corrupt").unwrap();
    assert!(fence.permits_cleanup(dir.path()).is_err());
    std::fs::remove_file(dir.path().join("runtime.json")).unwrap();
    assert!(fence.permits_cleanup(dir.path()).unwrap());
    bifrost_core::write_system_proxy_shutdown_mode(
        dir.path(),
        bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
    )
    .unwrap();
    assert!(!fence.permits_cleanup(dir.path()).unwrap());
}

#[test]
fn desktop_owned_exit_suspends_existing_generation_without_cli_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let port = unused_loopback_port();
    write_restart_fixture(
        dir.path(),
        port,
        dir.path().join("must-not-launch"),
        false,
        port,
        true,
    );
    let mut runtime = read_runtime_info_from(dir.path()).unwrap();
    runtime.start_mode = RuntimeStartMode::Desktop;
    runtime.restartable_runtime = false;
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    let mut proxy = TestManagedProxyRecovery::new(port, true);
    proxy.suspend_failures = 1;
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            Duration::from_secs(1),
            &mut proxy
        ),
        ManagedRuntimeRestartOutcome::FailOpenSuspended
    );
    assert_eq!(proxy.suspend_calls.len(), 2);
    assert_eq!(
        proxy.ownership.as_ref().unwrap().generation,
        "generation-fixture"
    );
    assert!(proxy.ownership.as_ref().unwrap().is_suspended());
    assert!(proxy.resume_calls.is_empty());
    assert_eq!(
        bifrost_core::read_system_proxy_shutdown_mode(dir.path()),
        None
    );
}

#[test]
fn delayed_api_failure_never_replays_over_newer_intent() {
    for initial in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigManager::new(dir.path().to_path_buf()).unwrap();
        persist_system_proxy_config(&config, initial, None).unwrap();
        let mut fallback_calls = 0;
        let result = route_explicit_proxy_command(
            Some(18891),
            |_| {
                // The API accepted our request, then a newer opposite request won
                // before the delayed HTTP failure reached the CLI.
                persist_system_proxy_config(&config, !initial, None).unwrap();
                Err(cleanup_error(
                    "API accepted but delayed OS verification failed",
                ))
            },
            || {
                fallback_calls += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(fallback_calls, 0);
        assert_eq!(
            read_system_proxy_config(dir.path()).unwrap().enabled,
            !initial
        );
    }
}

#[test]
fn direct_accepted_revision_never_adopts_newer_intent() {
    for enabled in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigManager::new(dir.path().to_path_buf()).unwrap();
        with_accepted_system_proxy_intent(&config, enabled, None, |accepted| {
            assert!(accepted_intent_is_current(dir.path(), accepted)?);
            persist_system_proxy_config(&config, !enabled, None)?;
            assert!(!accepted_intent_is_current(dir.path(), accepted)?);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn implicit_cli_bypass_preserves_newer_durable_value() {
    let dir = tempfile::tempdir().unwrap();
    let stale_manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    let newer_writer = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    persist_system_proxy_config(&newer_writer, false, Some("newer-bypass".into())).unwrap();
    with_accepted_system_proxy_intent(&stale_manager, true, None, |accepted| {
        assert_eq!(accepted.bypass, "newer-bypass");
        Ok(())
    })
    .unwrap();
}
