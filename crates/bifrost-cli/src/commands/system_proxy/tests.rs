use super::*;
use crate::process::RuntimeStartMode;
use std::path::PathBuf;

fn cleanup_error(message: &str) -> bifrost_core::BifrostError {
    bifrost_core::BifrostError::Config(message.to_string())
}

fn unused_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_restart_fixture(
    data_dir: &std::path::Path,
    port: u16,
    binary_path: PathBuf,
    restartable: bool,
    target_port: u16,
    applied: bool,
) {
    // Fixtures explicitly create their config; helper reads must remain observational.
    ConfigManager::new(data_dir.to_path_buf()).unwrap();
    let runtime = RuntimeInfo {
        pid: 424_242,
        port,
        socks5_port: None,
        host: Some("127.0.0.1".into()),
        started_at_ms: Some(1),
        start_mode: if restartable {
            RuntimeStartMode::Daemon
        } else {
            RuntimeStartMode::Foreground
        },
        restartable_runtime: restartable,
        binary_path: Some(binary_path),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost,127.0.0.1".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    std::fs::write(
        data_dir.join("runtime.json"),
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();
    std::fs::write(
        data_dir.join("proxy_state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "generation": "generation-fixture",
            "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
            "target": {
                "enable": true,
                "host": "127.0.0.1",
                "port": target_port,
                "bypass": "localhost,127.0.0.1"
            },
            "applied": applied
        }))
        .unwrap(),
    )
    .unwrap();
}

struct TestManagedProxyRecovery {
    ownership: Option<bifrost_core::ManagedSystemProxyOwnership>,
    fail_ensure: bool,
    suspend_calls: Vec<String>,
    resume_calls: Vec<String>,
    suspend_failures: usize,
    resume_failures: usize,
    resume_outcome: Option<bifrost_core::GuardedSystemProxyTransition>,
    suspend_outcome: Option<bifrost_core::GuardedSystemProxyTransition>,
    disable_on_resume: Option<PathBuf>,
    replace_runtime_on_ensure: Option<(PathBuf, RuntimeInfo)>,
    after_ensure: Option<Box<dyn FnOnce()>>,
    after_resume: Option<Box<dyn FnOnce()>>,
}

impl TestManagedProxyRecovery {
    fn new(target_port: u16, applied: bool) -> Self {
        Self {
            ownership: Some(bifrost_core::ManagedSystemProxyOwnership {
                schema_version: 2,
                generation: "generation-fixture".into(),
                authorization_suppressed: false,
                original: bifrost_core::ProxyBackup {
                    enable: false,
                    host: String::new(),
                    port: 0,
                    bypass: String::new(),
                },
                target: bifrost_core::ProxyBackup {
                    enable: true,
                    host: "127.0.0.1".into(),
                    port: target_port,
                    bypass: "localhost,127.0.0.1".into(),
                },
                applied,
                phase: Some(if applied {
                    bifrost_core::system_proxy::ManagedSystemProxyPhase::Applied
                } else {
                    bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended
                }),
            }),
            fail_ensure: false,
            suspend_calls: Vec::new(),
            resume_calls: Vec::new(),
            suspend_failures: 0,
            resume_failures: 0,
            resume_outcome: None,
            suspend_outcome: None,
            disable_on_resume: None,
            replace_runtime_on_ensure: None,
            after_ensure: None,
            after_resume: None,
        }
    }
}

impl ManagedSystemProxyRecovery for TestManagedProxyRecovery {
    fn suspend_managed_if_generation_guarded(
        &mut self,
        generation: &str,
        should_suspend: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        if !should_suspend()? {
            return Ok(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.suspend_managed_if_generation(generation)
    }

    fn ensure_managed_ownership(
        &mut self,
    ) -> bifrost_core::Result<Option<bifrost_core::ManagedSystemProxyOwnership>> {
        if self.fail_ensure {
            return Err(bifrost_core::BifrostError::Config(
                "test ownership read failure".into(),
            ));
        }
        if let Some((path, runtime)) = &self.replace_runtime_on_ensure {
            std::fs::write(
                path.join("runtime.json"),
                serde_json::to_vec(runtime).unwrap(),
            )
            .unwrap();
        }
        if let Some(action) = self.after_ensure.take() {
            action();
        }
        Ok(self.ownership.clone())
    }

    fn suspend_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        self.suspend_calls.push(expected_generation.into());
        if self.suspend_failures > 0 {
            self.suspend_failures -= 1;
            return Err(cleanup_error("temporary suspend failure"));
        }
        if let Some(outcome) = self.suspend_outcome {
            return Ok(outcome);
        }
        if let Some(ownership) = self.ownership.as_mut() {
            ownership.applied = false;
            ownership.phase = Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended);
        }
        Ok(bifrost_core::GuardedSystemProxyTransition::Applied)
    }

    fn resume_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        self.resume_calls.push(expected_generation.into());
        if let Some(action) = self.after_resume.take() {
            action();
        }
        if self.resume_failures > 0 {
            self.resume_failures -= 1;
            return Err(cleanup_error("temporary resume failure"));
        }
        if let Some(outcome) = self.resume_outcome {
            return Ok(outcome);
        }
        if let Some(path) = &self.disable_on_resume {
            let config = ConfigManager::new(path.clone()).unwrap();
            persist_system_proxy_config(&config, false, None).unwrap();
        }
        if let Some(ownership) = self.ownership.as_mut() {
            ownership.applied = true;
            ownership.phase = Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Applied);
        }
        Ok(bifrost_core::GuardedSystemProxyTransition::Applied)
    }
}

#[test]
fn combined_proxy_cleanup_reports_success_and_each_failure_shape() {
    assert_eq!(
        combine_proxy_cleanup_results(
            Ok(()),
            Ok(vec![PathBuf::from(".zshrc"), PathBuf::from(".bashrc")])
        )
        .unwrap(),
        2
    );

    let system_only =
        combine_proxy_cleanup_results(Err(cleanup_error("system failed")), Ok(Vec::new()))
            .unwrap_err()
            .to_string();
    assert!(system_only.contains("system failed"));

    let cli_only = combine_proxy_cleanup_results(Ok(()), Err(cleanup_error("profile failed")))
        .unwrap_err()
        .to_string();
    assert!(cli_only.contains("profile failed"));

    let both = combine_proxy_cleanup_results(
        Err(cleanup_error("system failed")),
        Err(cleanup_error("profile failed")),
    )
    .unwrap_err()
    .to_string();
    assert!(both.contains("System proxy cleanup failed"));
    assert!(both.contains("system failed"));
    assert!(both.contains("CLI proxy environment cleanup failed"));
    assert!(both.contains("profile failed"));
}

#[test]
fn parent_identity_status_detects_pid_reuse_from_start_time_mismatch() {
    let recorded = bifrost_core::current_process_start_time_ms()
        .map(|started_at_ms| started_at_ms.saturating_add(10_000));

    assert_eq!(
        parent_identity_status(Some(std::process::id()), recorded),
        ProcessIdentityStatus::Reused
    );
    assert_eq!(
        immediate_parent_exit_trigger(Some(std::process::id()), recorded),
        Some(LifecycleRecoveryTrigger::PidReused)
    );
}

#[test]
fn runtime_identity_rejects_start_time_mismatch() {
    let runtime = RuntimeInfo {
        pid: std::process::id(),
        port: 18889,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: bifrost_core::current_process_start_time_ms()
            .map(|started_at_ms| started_at_ms.saturating_add(10_000)),
        start_mode: RuntimeStartMode::Foreground,
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert!(!runtime_identity_is_current(&runtime));
}

#[test]
fn managed_runtime_restart_skips_foreground_runtime() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Foreground,
        restartable_runtime: false,
        binary_path: Some(PathBuf::from("/tmp/bifrost")),
        system_proxy_enabled: None,
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert!(!should_try_managed_runtime_restart(
        &runtime,
        &Default::default()
    ));
}

#[test]
fn managed_runtime_restart_requires_binary_path() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert!(!should_try_managed_runtime_restart(
        &runtime,
        &Default::default()
    ));
}

#[test]
fn managed_runtime_restart_skips_explicitly_disabled_system_proxy() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: Some(PathBuf::from("/tmp/bifrost")),
        system_proxy_enabled: Some(false),
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert!(!should_try_managed_runtime_restart(
        &runtime,
        &Default::default()
    ));
}

#[test]
fn managed_runtime_restart_args_preserve_runtime_and_system_proxy() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: 18889,
        socks5_port: Some(18890),
        host: Some("0.0.0.0".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: Some(PathBuf::from("/tmp/bifrost")),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    let snapshot = (true, "localhost,127.0.0.1,*.local".to_string());

    assert_eq!(
        build_managed_runtime_restart_args(&runtime, &snapshot),
        vec![
            "start",
            "--daemon",
            "--port",
            "18889",
            "--host",
            "0.0.0.0",
            "--socks5-port",
            "18890",
            "--system-proxy",
            "--proxy-bypass",
            "localhost,127.0.0.1,*.local"
        ]
    );
}

#[test]
fn runtime_info_system_proxy_target_maps_wildcard_to_loopback() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: 18889,
        socks5_port: None,
        host: Some("0.0.0.0".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Foreground,
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert_eq!(
        runtime_info_system_proxy_target(&runtime),
        RuntimeSystemProxyTarget {
            host: "127.0.0.1".to_string(),
            port: 18889
        }
    );
}

#[test]
fn cli_disable_retries_with_runtime_target_only_for_owned_by_other() {
    let target = RuntimeSystemProxyTarget {
        host: "127.0.0.1".to_string(),
        port: 18889,
    };

    assert!(should_retry_disable_with_runtime_target(
        bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
        Some(&target)
    ));
    assert!(!should_retry_disable_with_runtime_target(
        bifrost_core::SystemProxyDisableOutcome::Disabled,
        Some(&target)
    ));
    assert!(!should_retry_disable_with_runtime_target(
        bifrost_core::SystemProxyDisableOutcome::OwnedByOther,
        None
    ));
}

#[test]
fn cli_disable_does_not_retry_for_non_owned_by_other_outcomes() {
    let target = RuntimeSystemProxyTarget {
        host: "127.0.0.1".to_string(),
        port: 18889,
    };

    assert!(!should_retry_disable_with_runtime_target(
        bifrost_core::SystemProxyDisableOutcome::Disabled,
        Some(&target)
    ));
    assert!(!should_retry_disable_with_runtime_target(
        bifrost_core::SystemProxyDisableOutcome::NotEnabled,
        Some(&target)
    ));
}

#[test]
fn parent_identity_status_is_unknown_without_parent_pid() {
    assert_eq!(
        parent_identity_status(None, Some(123)),
        ProcessIdentityStatus::Unknown
    );
    assert_eq!(
        parent_identity_status(None, None),
        ProcessIdentityStatus::Unknown
    );
    assert_eq!(immediate_parent_exit_trigger(None, None), None);
}

#[test]
fn lifecycle_recovery_trigger_names_are_diagnostic_stable() {
    assert_eq!(LifecycleRecoveryTrigger::PidMissing.as_str(), "pid_missing");
    assert_eq!(LifecycleRecoveryTrigger::PidReused.as_str(), "pid_reused");
    assert_eq!(
        LifecycleRecoveryTrigger::PollConfirmedExit.as_str(),
        "poll_confirmed_exit"
    );
    assert_eq!(
        LifecycleRecoveryTrigger::Signal("sigterm").as_str(),
        "sigterm"
    );
}

#[test]
fn runtime_info_system_proxy_target_preserves_specific_host() {
    let runtime = RuntimeInfo {
        pid: 123,
        port: 18889,
        socks5_port: None,
        host: Some("example.com".to_string()),
        started_at_ms: None,
        start_mode: RuntimeStartMode::Foreground,
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
        system_proxy_config_revision: Some(0),
        health_port: None,
    };

    assert_eq!(
        runtime_info_system_proxy_target(&runtime),
        RuntimeSystemProxyTarget {
            host: "example.com".to_string(),
            port: 18889,
        }
    );
}

#[test]
fn doctor_report_is_read_only_and_explains_missing_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let report = doctor_report_with_fake_observations(dir.path());

    assert!(report.runtime.is_none());
    assert_eq!(report.runtime_identity, "Missing");
    assert!(report
        .findings
        .iter()
        .any(|finding| finding == "runtime marker is missing"));
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.contains("health_port")));
    let text = render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::Text).unwrap();
    assert!(text.contains("Runtime identity:    Missing"));
    assert!(
        render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::Json)
            .unwrap()
            .starts_with('{')
    );
    assert!(
        render_system_proxy_doctor_report(&report, crate::cli::StatusFormat::JsonPretty)
            .unwrap()
            .contains("\n  \"runtime\"")
    );
    assert!(!dir.path().join("system_proxy_owner.json").exists());
    assert!(!dir.path().join("system_proxy_events.jsonl").exists());
}

fn doctor_report_with_fake_observations(data_dir: &std::path::Path) -> SystemProxyDoctorReport {
    let observations = std::cell::RefCell::new(Vec::new());
    let report = build_system_proxy_doctor_report_with_observations(
        data_dir,
        || {
            observations.borrow_mut().push("current");
            Ok(bifrost_core::ProxyBackup {
                enable: false,
                host: "fixture.proxy.invalid".into(),
                port: 0,
                bypass: String::new(),
            })
        },
        || {
            observations.borrow_mut().push("ownership");
            let path = data_dir.join("proxy_state.json");
            Ok(path
                .exists()
                .then(|| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()))
        },
        |host, port| {
            observations.borrow_mut().push("services");
            assert_eq!(host, "unrelated-target.invalid");
            assert_eq!(port, 54322);
            Ok(false)
        },
    );
    let expected_observations = if report.managed_ownership.is_some() {
        vec!["current", "ownership", "services"]
    } else {
        vec!["current", "ownership"]
    };
    assert_eq!(*observations.borrow(), expected_observations);
    assert_eq!(
        report.current_proxy.as_ref().unwrap().host,
        "fixture.proxy.invalid"
    );
    report
}

#[test]
fn doctor_report_covers_healthy_stale_invalid_and_ownership_findings() {
    fn health_server(body: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        (port, thread)
    }

    let dir = tempfile::tempdir().unwrap();
    let mut runtime = RuntimeInfo {
        pid: std::process::id(),
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("127.0.0.1".into()),
        started_at_ms: bifrost_core::current_process_start_time_ms(),
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: std::env::current_exe().ok(),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    let healthy = bifrost_core::RuntimeHealthSnapshot {
        scheduler_heartbeat_age_ms: 1,
        ..Default::default()
    };
    let (health_port, health_thread) = health_server(serde_json::to_vec(&healthy).unwrap());
    runtime.health_port = Some(health_port);
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    let report = doctor_report_with_fake_observations(dir.path());
    health_thread.join().unwrap();
    assert_eq!(report.runtime_identity, "Alive");
    assert!(report.health.is_some());
    assert!(report
        .findings
        .iter()
        .any(|finding| finding == "no blocking ownership or runtime health issue detected"));

    let stale = bifrost_core::RuntimeHealthSnapshot {
        scheduler_heartbeat_age_ms: 5_001,
        ..Default::default()
    };
    let (health_port, health_thread) = health_server(serde_json::to_vec(&stale).unwrap());
    runtime.health_port = Some(health_port);
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("proxy_state.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "generation": "doctor-generation",
            "original": {"enable": true, "host": "unrelated-original.invalid", "port": 54321, "bypass": ""},
            "target": {"enable": true, "host": "unrelated-target.invalid", "port": 54322, "bypass": ""},
            "applied": true
        }))
        .unwrap(),
    )
    .unwrap();
    let report = doctor_report_with_fake_observations(dir.path());
    health_thread.join().unwrap();
    assert!(report
        .findings
        .iter()
        .any(|finding| finding == "scheduler heartbeat is stale"));
    assert!(report
        .findings
        .iter()
        .any(|finding| { finding == "managed state says applied but OS proxy ownership changed" }));

    let (health_port, health_thread) = health_server(b"not-json".to_vec());
    runtime.health_port = Some(health_port);
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    let invalid = doctor_report_with_fake_observations(dir.path());
    health_thread.join().unwrap();
    assert!(invalid
        .health_error
        .as_deref()
        .is_some_and(|error| error.contains("invalid health response")));

    runtime.health_port = Some(unused_loopback_port());
    std::fs::write(
        dir.path().join("proxy_state.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "generation": "doctor-generation",
            "original": {"enable": true, "host": "unrelated-original.invalid", "port": 54321, "bypass": ""},
            "target": {"enable": true, "host": "unrelated-target.invalid", "port": 54322, "bypass": ""},
            "applied": false,
            "phase": "suspended"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    let unavailable = doctor_report_with_fake_observations(dir.path());
    assert!(unavailable
        .health_error
        .as_deref()
        .is_some_and(|error| error.contains("health lane unavailable")));
    assert!(unavailable.findings.iter().any(|finding| {
        finding == "fail-open state no longer matches the recorded original proxy"
    }));
}

#[test]
fn failed_restart_fail_closed_persists_diagnostics_without_touching_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = bifrost_core::SystemProxyManager::new(dir.path().to_path_buf());

    let outcome = apply_recovery_policy_after_failed_restart(
        dir.path(),
        &mut manager,
        "generation-closed",
        SystemProxyRecoveryMode::FailClosed,
        std::time::Instant::now(),
        false,
        Some("spawn failed".into()),
    );

    assert_eq!(outcome, ManagedRuntimeRestartOutcome::FailClosedPreserved);
    let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(owner.phase.as_deref(), Some("recovering_fail_closed"));
    assert_eq!(owner.last_error.as_deref(), Some("spawn failed"));
    let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 5).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "helper_runtime_restart_not_ready");
    assert_eq!(
        events[0].ownership_generation.as_deref(),
        Some("generation-closed")
    );
}

#[test]
fn recovery_policy_helper_persists_both_modes() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        persist_system_proxy_recovery_policy(&manager, "fail-closed", 3).unwrap(),
        SystemProxyRecoveryMode::FailClosed
    );
    assert_eq!(
        futures::executor::block_on(manager.config())
            .system_proxy
            .recovery_mode,
        SystemProxyRecoveryMode::FailClosed
    );
    assert_eq!(
        persist_system_proxy_recovery_policy(&manager, "anything-else", 5).unwrap(),
        SystemProxyRecoveryMode::FailOpen
    );
    let config = futures::executor::block_on(manager.config());
    assert_eq!(
        config.system_proxy.recovery_mode,
        SystemProxyRecoveryMode::FailOpen
    );
    assert_eq!(config.system_proxy.recovery_grace_secs, 5);
}

#[test]
fn parent_exit_markers_complete_without_proxy_mutation() {
    for mode in [
        bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
        bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
    ] {
        let dir = tempfile::tempdir().unwrap();
        bifrost_core::write_system_proxy_shutdown_mode(dir.path(), mode).unwrap();
        cleanup_after_parent_exit(
            dir.path(),
            Some(424_242),
            Some(1),
            LifecycleRecoveryTrigger::Signal("test"),
        )
        .unwrap();

        let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(owner.phase.as_deref(), Some("parent_exit_recovery"));
        let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 5).unwrap();
        assert_eq!(
            events.last().unwrap().event,
            "lifecycle_helper_recovery_completed"
        );
        assert_eq!(events.last().unwrap().trigger.as_deref(), Some("test"));
    }
}

#[test]
fn lifecycle_helper_heartbeat_tolerates_an_unwritable_data_path() {
    let dir = tempfile::tempdir().unwrap();
    let blocked_path = dir.path().join("not-a-directory");
    std::fs::write(&blocked_path, b"blocked").unwrap();

    record_lifecycle_helper_heartbeat(&blocked_path, Some(424_242), "test_unwritable_path");
}

#[test]
fn restart_helpers_cover_invalid_host_ready_runtime_and_real_adapter_empty_state() {
    let invalid_runtime = RuntimeInfo {
        pid: 424_242,
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("invalid host name".into()),
        started_at_ms: Some(1),
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: Some(std::env::current_exe().unwrap()),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    assert!(!runtime_data_plane_is_ready(&invalid_runtime));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 512];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    write_restart_fixture(
        dir.path(),
        port,
        std::env::current_exe().unwrap(),
        true,
        port,
        true,
    );
    let runtime = read_runtime_info_from(dir.path()).unwrap();
    assert!(runtime_data_plane_is_ready(&runtime));
    server.join().unwrap();

    let mut proxy = TestManagedProxyRecovery::new(port, true);
    proxy.fail_ensure = true;
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_millis(10),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::NotAttempted
    );
    proxy.fail_ensure = false;
    proxy.ownership = TestManagedProxyRecovery::new(port + 1, true).ownership;
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_millis(10),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    assert_eq!(
        reconcile_proxy_after_runtime_ready(&mut proxy, "generation-fixture").unwrap(),
        bifrost_core::GuardedSystemProxyTransition::Applied
    );
    assert_eq!(proxy.resume_calls, ["generation-fixture"]);

    let empty_dir = tempfile::tempdir().unwrap();
    let mut manager = bifrost_core::SystemProxyManager::new(empty_dir.path().to_path_buf());
    assert_eq!(
        ManagedSystemProxyRecovery::suspend_managed_if_generation(&mut manager, "missing").unwrap(),
        bifrost_core::GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        ManagedSystemProxyRecovery::resume_managed_if_generation(&mut manager, "missing").unwrap(),
        bifrost_core::GuardedSystemProxyTransition::NotManaged
    );
}

#[test]
fn managed_restart_rejects_missing_binary_and_unwritable_handoff_marker() {
    let dir = tempfile::tempdir().unwrap();
    ConfigManager::new(dir.path().to_path_buf()).unwrap();
    let port = unused_loopback_port();
    let mut runtime = RuntimeInfo {
        pid: 424_242,
        port,
        socks5_port: None,
        host: Some("127.0.0.1".into()),
        started_at_ms: Some(1),
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: None,
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();
    let mut proxy = TestManagedProxyRecovery::new(port, true);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_millis(10),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    runtime.binary_path = Some(dir.path().join("missing-bifrost"));
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_millis(10),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::NotAttempted
    );
    let owner_state = bifrost_core::read_system_proxy_owner_state(dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(
        owner_state.phase.as_deref(),
        Some("restart_preflight_failed")
    );
    assert_eq!(
        owner_state.last_action.as_deref(),
        Some("validate_runtime_binary")
    );
    assert!(owner_state
        .last_error
        .as_deref()
        .unwrap()
        .contains("missing-bifrost"));

    write_restart_fixture(
        dir.path(),
        port,
        std::env::current_exe().unwrap(),
        true,
        port,
        true,
    );
    std::fs::create_dir(dir.path().join(".system_proxy_shutdown_mode")).unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_millis(10),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::Cancelled
    );
}

#[test]
fn doctor_reports_a_dead_runtime_identity() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = RuntimeInfo {
        pid: 424_242,
        port: unused_loopback_port(),
        socks5_port: None,
        host: Some("127.0.0.1".into()),
        started_at_ms: Some(1),
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: Some(std::env::current_exe().unwrap()),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();
    let report = doctor_report_with_fake_observations(dir.path());
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.contains("runtime process identity is Exited")));
}

#[cfg(unix)]
#[test]
fn managed_restart_launches_replacement_and_waits_for_data_canary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let fake_binary = dir.path().join("fake-bifrost.py");
    std::fs::write(
        &fake_binary,
        r#"#!/usr/bin/env python3
import json, os, socket, sys, time
port = int(sys.argv[sys.argv.index('--port') + 1])
data_dir = os.environ['BIFROST_DATA_DIR']
with open(os.path.join(data_dir, 'replacement.pid'), 'w') as output:
    output.write(str(os.getpid()))
with open(os.path.join(data_dir, 'replacement-launch.json'), 'w') as output:
    json.dump({
        'args': sys.argv,
        'revision': os.environ.get('BIFROST_SYSTEM_PROXY_INTENT_REVISION_INTERNAL'),
        'generation': os.environ.get('BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL'),
    }, output)
time.sleep(3.2)
with open(os.path.join(data_dir, 'runtime.json')) as source:
    runtime = json.load(source)
runtime['pid'] = os.getpid()
runtime['started_at_ms'] = None
with open(os.path.join(data_dir, 'runtime.json'), 'w') as output:
    json.dump(runtime, output)
server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(('127.0.0.1', port))
server.listen(8)
while True:
    connection, _ = server.accept()
    connection.recv(4096)
    connection.sendall(b'HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
    connection.close()
"#,
    )
    .unwrap();
    // Validate embedded Python before launching: accidental indentation edits
    // must fail with a syntax diagnostic, not masquerade as a canary timeout.
    let syntax = std::process::Command::new("python3")
        .args(["-c", "import pathlib,sys; p=pathlib.Path(sys.argv[1]); compile(p.read_text(), str(p), 'exec')"])
        .arg(&fake_binary).output().unwrap();
    assert!(
        syntax.status.success(),
        "invalid replacement fixture: {}",
        String::from_utf8_lossy(&syntax.stderr)
    );
    let mut permissions = std::fs::metadata(&fake_binary).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_binary, permissions).unwrap();

    let runtime = RuntimeInfo {
        pid: 424_242,
        port,
        socks5_port: None,
        host: Some("127.0.0.1".into()),
        started_at_ms: Some(1),
        start_mode: RuntimeStartMode::Daemon,
        restartable_runtime: true,
        binary_path: Some(fake_binary),
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost,127.0.0.1".into()),
        system_proxy_config_revision: Some(0),
        health_port: None,
    };
    std::fs::write(
        dir.path().join("runtime.json"),
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("proxy_state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "generation": "generation-ready",
            "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
            "target": {
                "enable": true,
                "host": "127.0.0.1",
                "port": port,
                "bypass": "localhost,127.0.0.1"
            },
            "applied": true
        }))
        .unwrap(),
    )
    .unwrap();

    let config_manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    futures::executor::block_on(config_manager.update_system_proxy_config(
        SystemProxyConfigUpdate {
            enabled: None,
            bypass: None,
            auto_enable: None,
            recovery_mode: Some(SystemProxyRecoveryMode::FailOpen),
            recovery_grace_secs: Some(MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS),
        },
    ))
    .unwrap();

    struct ReplacementCleanup(PathBuf);
    impl Drop for ReplacementCleanup {
        fn drop(&mut self) {
            if let Ok(contents) = std::fs::read_to_string(self.0.join("replacement.pid")) {
                if let Ok(pid) = contents.parse::<u32>() {
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                }
            }
        }
    }
    let _cleanup = ReplacementCleanup(dir.path().to_path_buf());
    let mut proxy = TestManagedProxyRecovery::new(port, true);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_secs(7),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::Ready
    );
    assert_eq!(proxy.suspend_calls, ["generation-fixture"]);
    assert_eq!(proxy.resume_calls, ["generation-fixture"]);
    let launch: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("replacement-launch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(launch["revision"], "0");
    assert_eq!(launch["generation"], "generation-fixture");
    assert!(!launch["args"]
        .as_array()
        .unwrap()
        .iter()
        .any(|arg| arg == "--yes"));
    let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 10).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event == "helper_runtime_restart_started"));
    assert!(events
        .iter()
        .any(|event| event.event == "helper_runtime_restart_ready"));
    assert!(bifrost_core::read_system_proxy_shutdown_mode(dir.path()).is_none());
}

#[test]
fn managed_restart_preflight_rejects_incomplete_or_foreign_state() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    let port = unused_loopback_port();
    write_restart_fixture(
        dir.path(),
        port,
        PathBuf::from("/usr/bin/true"),
        false,
        port,
        true,
    );
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    write_restart_fixture(
        dir.path(),
        port,
        dir.path().join("missing-binary"),
        true,
        port,
        true,
    );
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    write_restart_fixture(
        dir.path(),
        port,
        PathBuf::from("/usr/bin/true"),
        true,
        port + 1,
        true,
    );
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );

    std::fs::write(dir.path().join("proxy_state.json"), "invalid").unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );
    std::fs::remove_file(dir.path().join("proxy_state.json")).unwrap();
    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );
}

#[cfg(unix)]
#[test]
fn managed_restart_spawn_failure_applies_fail_open_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let port = unused_loopback_port();
    let not_executable = dir.path().join("not-executable");
    std::fs::create_dir(&not_executable).unwrap();
    write_restart_fixture(dir.path(), port, not_executable, true, port, false);

    let mut proxy = TestManagedProxyRecovery::new(port, true);
    assert_eq!(
        restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
            dir.path(),
            std::time::Duration::from_secs(30),
            &mut proxy,
        ),
        ManagedRuntimeRestartOutcome::FailOpenSuspended
    );
    assert_eq!(proxy.suspend_calls, ["generation-fixture"]);
    let owner = bifrost_core::read_system_proxy_owner_state(dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(owner.phase.as_deref(), Some("recovering_fail_open"));
    assert!(owner.last_error.is_some());
}

#[cfg(unix)]
#[test]
fn managed_restart_grace_applies_both_recovery_policies() {
    for (mode, applied, expected) in [
        (
            SystemProxyRecoveryMode::FailOpen,
            false,
            ManagedRuntimeRestartOutcome::FailOpenSuspended,
        ),
        (
            SystemProxyRecoveryMode::FailClosed,
            true,
            ManagedRuntimeRestartOutcome::FailClosedPreserved,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let port = unused_loopback_port();
        write_restart_fixture(
            dir.path(),
            port,
            PathBuf::from("/usr/bin/true"),
            true,
            port,
            applied,
        );
        let config_manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
        futures::executor::block_on(config_manager.update_system_proxy_config(
            SystemProxyConfigUpdate {
                enabled: None,
                bypass: None,
                auto_enable: None,
                recovery_mode: Some(mode),
                recovery_grace_secs: Some(MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS),
            },
        ))
        .unwrap();

        let mut proxy = TestManagedProxyRecovery::new(port, applied);
        assert_eq!(
            restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                dir.path(),
                std::time::Duration::from_millis(3_250),
                &mut proxy,
            ),
            expected
        );
        assert_eq!(
            proxy.suspend_calls.len(),
            if mode == SystemProxyRecoveryMode::FailOpen {
                2
            } else {
                0
            }
        );
        let events = bifrost_core::read_recent_system_proxy_events(dir.path(), 20).unwrap();
        assert!(events.iter().any(|event| {
            event.event
                == match mode {
                    SystemProxyRecoveryMode::FailOpen => "helper_fail_open_applied",
                    SystemProxyRecoveryMode::FailClosed => "helper_fail_closed_preserved",
                }
        }));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn managed_restart_is_not_attempted_when_system_proxy_is_unsupported() {
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

    assert_eq!(
        restart_managed_runtime_before_cleanup(dir.path()),
        ManagedRuntimeRestartOutcome::NotAttempted
    );
}

#[test]
fn status_renderer_reports_recovery_policy_and_external_owner_warning() {
    let status = bifrost_core::ProxyBackup {
        enable: true,
        host: "external.proxy".into(),
        port: 8080,
        bypass: "localhost".into(),
    };
    let configured = bifrost_storage::NewSystemProxyConfig {
        enabled: true,
        bypass: "localhost,127.0.0.1".into(),
        auto_enable: true,
        recovery_mode: SystemProxyRecoveryMode::FailClosed,
        recovery_grace_secs: 3,
        intent_revision: 0,
    };

    let external = render_system_proxy_status(&status, false, &configured);
    assert!(external.contains("Recovery policy:     fail_closed (3s)"));
    assert!(external.contains("enabled by another application"));

    let managed = render_system_proxy_status(&status, true, &configured);
    assert!(managed.contains("Managed by Bifrost:  true"));
    assert!(!managed.contains("another application"));
}

mod recovery_regressions;

mod coverage_drivers;
