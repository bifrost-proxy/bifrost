use super::*;

#[test]
fn normalize_proxy_host_trims_brackets_and_lowercases() {
    assert_eq!(normalize_proxy_host("  [::1] "), "::1");
    assert_eq!(normalize_proxy_host("LOCALHOST"), "localhost");
    assert_eq!(normalize_proxy_host("127.0.0.1"), "127.0.0.1");
    assert_eq!(normalize_proxy_host(""), "");
}

#[test]
fn proxy_hosts_match_loopback_aliases() {
    assert!(proxy_hosts_match("localhost", "127.0.0.1"));
    assert!(proxy_hosts_match("127.0.0.1", "localhost"));
    assert!(proxy_hosts_match("::1", "127.0.0.1"));
    assert!(proxy_hosts_match("127.0.0.1", "::1"));
    assert!(proxy_hosts_match("::1", "localhost"));
    assert!(proxy_hosts_match("localhost", "::1"));
    // Exact normalized match.
    assert!(proxy_hosts_match("[::1]", "::1"));
    assert!(proxy_hosts_match("EXAMPLE.com", "example.com"));
    // Non-matching distinct hosts.
    assert!(!proxy_hosts_match("10.0.0.1", "10.0.0.2"));
}

#[test]
fn runtime_host_to_system_proxy_host_maps_wildcards() {
    assert_eq!(runtime_host_to_system_proxy_host("0.0.0.0"), "127.0.0.1");
    assert_eq!(runtime_host_to_system_proxy_host("::"), "127.0.0.1");
    assert_eq!(runtime_host_to_system_proxy_host(""), "127.0.0.1");
    assert_eq!(runtime_host_to_system_proxy_host("10.1.2.3"), "10.1.2.3");
    assert_eq!(runtime_host_to_system_proxy_host("[::1]"), "::1");
}

#[test]
fn current_proxy_matches_target_delegates_to_target_matches() {
    let current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    let target = ProxyBackup {
        enable: true,
        host: "localhost".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    assert!(current_proxy_matches_target(&current, &target));

    let other = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 1111,
        bypass: String::new(),
    };
    assert!(!current_proxy_matches_target(&current, &other));
}

fn make_state(applied: bool, target_port: u16) -> ManagedProxyState {
    let mut state = ManagedProxyState {
        schema_version: 3,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: target_port,
            bypass: String::new(),
        },
        applied,
        phase: Some(if applied {
            ManagedSystemProxyPhase::Applied
        } else {
            ManagedSystemProxyPhase::Suspended
        }),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    state.macos_services = macos_backend::mock_services_for_state(&state);
    state
}

#[test]
fn guarded_transitions_require_matching_generation_and_observed_owner() {
    let applied = make_state(true, 18880);
    assert!(guarded_suspend_allowed(&applied, "test-generation", true));
    assert!(!guarded_suspend_allowed(&applied, "stale-generation", true));
    assert!(!guarded_suspend_allowed(&applied, "test-generation", false));

    let suspended = make_state(false, 18880);
    assert!(guarded_resume_allowed(&suspended, "test-generation", true));
    assert!(!guarded_resume_allowed(
        &suspended,
        "stale-generation",
        true
    ));
    assert!(!guarded_resume_allowed(
        &suspended,
        "test-generation",
        false
    ));
}

#[test]
fn managed_ownership_migrates_legacy_state_and_read_is_observational() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    std::fs::write(
        manager.state_file_path(),
        r#"{
                "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
                "target": {"enable": true, "host": "127.0.0.1", "port": 18880, "bypass": "localhost"}
            }"#,
    )
    .unwrap();

    let migrated = manager.ensure_managed_ownership().unwrap().unwrap();
    assert_eq!(migrated.schema_version, 3);
    assert!(!migrated.generation.is_empty());
    assert!(migrated.applied);
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(manager.read_managed_ownership().unwrap(), Some(migrated));
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);

    std::fs::remove_file(manager.state_file_path()).unwrap();
    assert!(manager.ensure_managed_ownership().unwrap().is_none());
    assert!(manager.read_managed_ownership().unwrap().is_none());
    std::fs::write(manager.state_file_path(), "not-json").unwrap();
    assert!(manager.ensure_managed_ownership().is_err());
    assert!(manager.read_managed_ownership().is_err());
}

#[test]
fn generation_transitions_reject_stale_or_already_applied_state_before_os_access() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    assert_eq!(
        manager.suspend_managed_if_generation("missing").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager.resume_managed_if_generation("missing").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );

    manager
        .write_managed_state(&make_state(true, 18880))
        .unwrap();
    assert_eq!(
        manager
            .suspend_managed_if_generation("stale-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );

    manager
        .write_managed_state(&make_state(false, 18880))
        .unwrap();
    *manager.mock_macos_state.lock().unwrap() = None;
    assert_eq!(
        manager
            .suspend_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );
    assert_eq!(
        manager
            .resume_managed_if_generation("stale-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );

    std::fs::write(manager.state_file_path(), "not-json").unwrap();
    assert!(manager
        .suspend_managed_if_generation("test-generation")
        .is_err());
    assert!(manager
        .resume_managed_if_generation("test-generation")
        .is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn generation_transitions_reject_observed_proxy_mismatch_without_writing_os_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    let applied = make_state(true, 18881);
    manager.write_managed_state(&applied).unwrap();
    let mut os = macos_backend::MockMacosState::from_journal(Some(&applied));
    for field in [macos_owned::Field::Http, macos_owned::Field::Https] {
        os.values.insert(
            (macos_backend::MockMacosState::SERVICE.into(), field as u8),
            macos_owned::Value::Protocol(macos_owned::Protocol {
                enabled: true,
                host: "external-proxy".into(),
                port: 8443,
                authenticated: false,
            }),
        );
    }
    *manager.mock_macos_state.lock().unwrap() = Some(os);
    assert_eq!(
        manager
            .suspend_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    manager
        .write_managed_state(&make_state(false, 18881))
        .unwrap();
    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writes
        .is_empty());
}

#[test]
fn generation_transitions_persist_suspend_and_resume_with_test_backend() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    manager
        .write_managed_state(&make_state(true, 18880))
        .unwrap();

    assert_eq!(
        manager
            .suspend_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let suspended = manager.load_managed_state().unwrap();
    assert!(!suspended.applied);
    assert!(!manager.is_set);
    assert!(manager.original_proxy.is_none());

    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let resumed = manager.load_managed_state().unwrap();
    assert!(resumed.applied);
    assert!(manager.is_set);
    assert!(manager.original_proxy.is_some());
    manager.detach_in_place();

    let events = crate::read_recent_system_proxy_events(dir.path(), 10).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event == "system_proxy_fail_open_suspended"));
    assert!(events
        .iter()
        .any(|event| event.event == "system_proxy_generation_resumed"));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_enabled_proxy_audit_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    let mut os = manager
        .macos_backend(macos_command::Privilege::Direct)
        .unwrap();
    assert!(!macos_any_service_proxy_enabled(&mut os).unwrap());
    assert!(manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writes
        .is_empty());
}

#[test]
fn managed_state_generation_and_action_diagnostics_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SystemProxyManager::new(dir.path().to_path_buf());
    let original = Sysproxy {
        enable: false,
        host: String::new(),
        port: 0,
        bypass: String::new(),
    };
    let target = Sysproxy {
        enable: true,
        host: "127.0.0.1".into(),
        port: 18880,
        bypass: "localhost".into(),
    };

    manager
        .save_managed_state(&original, &target, false)
        .unwrap();
    let first = manager.load_managed_state().unwrap();
    assert!(!first.generation.is_empty());
    manager
        .save_managed_state(&original, &target, true)
        .unwrap();
    let second = manager.load_managed_state().unwrap();
    assert_eq!(second.generation, first.generation);
    assert!(second.applied);

    manager.record_system_proxy_action("coverage_action", "diagnose");
    let owner = crate::read_system_proxy_owner_state(dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(owner.ownership_generation, Some(first.generation.clone()));
    assert_eq!(owner.expected_proxy, Some(second.target));
    assert_eq!(owner.last_action.as_deref(), Some("diagnose"));
    let events = crate::read_recent_system_proxy_events(dir.path(), 5).unwrap();
    assert_eq!(events[0].event, "coverage_action");
    assert_eq!(events[0].ownership_generation, Some(first.generation));
}

#[test]
fn managed_target_listener_uses_persisted_target() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager
        .write_managed_state(&make_state(true, port))
        .unwrap();

    assert!(SystemProxyManager::managed_target_has_live_listener(
        dir.path()
    ));
    drop(listener);
    std::fs::remove_file(manager.state_file_path()).unwrap();
    assert!(!SystemProxyManager::managed_target_has_live_listener(
        dir.path()
    ));
}

#[test]
fn legacy_managed_state_defaults_to_applied_without_generation() {
    let state: ManagedProxyState = serde_json::from_value(serde_json::json!({
        "original": { "enable": false, "host": "", "port": 0, "bypass": "" },
        "target": { "enable": true, "host": "127.0.0.1", "port": 18880, "bypass": "" }
    }))
    .unwrap();
    assert_eq!(state.schema_version, 1);
    assert!(state.generation.is_empty());
    assert!(state.applied);
}

#[test]
fn decide_managed_state_recovery_branches() {
    let matching_current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    let mismatching_current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 6152,
        bypass: String::new(),
    };

    // applied + current matches target -> restore.
    assert_eq!(
        decide_managed_state_recovery(&matching_current, &make_state(true, 18880)),
        CrashRecoveryDecision::RestoreOriginal
    );
    // applied + current does NOT match -> preserve external.
    assert_eq!(
        decide_managed_state_recovery(&mismatching_current, &make_state(true, 18880)),
        CrashRecoveryDecision::PreserveExternal
    );
    // not applied + current does NOT match -> discard pending apply.
    assert_eq!(
        decide_managed_state_recovery(&mismatching_current, &make_state(false, 18880)),
        CrashRecoveryDecision::DiscardPendingApply
    );
    // not applied + current matches -> restore.
    assert_eq!(
        decide_managed_state_recovery(&matching_current, &make_state(false, 18880)),
        CrashRecoveryDecision::RestoreOriginal
    );
}

#[test]
fn decide_macos_managed_state_recovery_uses_service_match() {
    let current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    let state = make_state(true, 18880);

    assert_eq!(
        decide_macos_managed_state_recovery(&current, &state, Ok(true)).unwrap(),
        CrashRecoveryDecision::RestoreOriginal
    );
    // service_match false -> delegate to decide_managed_state_recovery.
    assert_eq!(
        decide_macos_managed_state_recovery(&current, &state, Ok(false)).unwrap(),
        CrashRecoveryDecision::RestoreOriginal
    );
    // Err propagates.
    assert!(decide_macos_managed_state_recovery(
        &current,
        &state,
        Err(BifrostError::Config("boom".to_string()))
    )
    .is_err());
}

#[test]
fn decide_macos_runtime_target_match_passthrough() {
    assert!(decide_macos_runtime_target_match(Ok(true)).unwrap());
    assert!(!decide_macos_runtime_target_match(Ok(false)).unwrap());
    assert!(decide_macos_runtime_target_match(Err(BifrostError::Config("x".to_string()))).is_err());
}

#[test]
fn restart_handoff_preserved_original_default_applied_field() {
    // ManagedProxyState deserialized without `applied` defaults to true.
    let json = r#"{
            "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
            "target": {"enable": true, "host": "127.0.0.1", "port": 18880, "bypass": ""}
        }"#;
    let state: ManagedProxyState = serde_json::from_str(json).unwrap();
    assert!(state.applied);
}

#[test]
fn load_last_runtime_proxy_target_reads_runtime_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(RUNTIME_FILE_NAME),
        r#"{"host": "0.0.0.0", "port": 9911}"#,
    )
    .unwrap();
    let target = load_last_runtime_proxy_target(dir.path()).unwrap();
    assert_eq!(target.port, 9911);
    // 0.0.0.0 wildcard mapped to loopback.
    assert_eq!(target.host, "127.0.0.1");
    assert!(target.enable);
}

#[test]
fn load_last_runtime_proxy_target_missing_or_invalid() {
    let dir = tempfile::tempdir().unwrap();
    // No file at all.
    assert!(load_last_runtime_proxy_target(dir.path()).is_none());
    // Port out of range / zero -> None.
    std::fs::write(
        dir.path().join(RUNTIME_FILE_NAME),
        r#"{"host": "127.0.0.1", "port": 0}"#,
    )
    .unwrap();
    assert!(load_last_runtime_proxy_target(dir.path()).is_none());
    // Missing host -> defaults to 127.0.0.1.
    std::fs::write(dir.path().join(RUNTIME_FILE_NAME), r#"{"port": 8080}"#).unwrap();
    let t = load_last_runtime_proxy_target(dir.path()).unwrap();
    assert_eq!(t.host, "127.0.0.1");
    assert_eq!(t.port, 8080);
}

#[test]
fn managed_target_listener_is_alive_false_when_disabled_or_no_listener() {
    // Disabled target -> false immediately.
    let disabled = ProxyBackup {
        enable: false,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    assert!(!managed_target_listener_is_alive(&disabled));

    // Port 0 -> false.
    let zero_port = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 0,
        bypass: String::new(),
    };
    assert!(!managed_target_listener_is_alive(&zero_port));
}

#[test]
fn system_proxy_disable_outcome_eq() {
    assert_eq!(
        SystemProxyDisableOutcome::Disabled,
        SystemProxyDisableOutcome::Disabled
    );
    assert_ne!(
        SystemProxyDisableOutcome::Disabled,
        SystemProxyDisableOutcome::OwnedByOther
    );
    assert!(format!("{:?}", SystemProxyDisableOutcome::NotEnabled).contains("NotEnabled"));
}

#[test]
fn enable_disable_unsupported_on_non_macos_windows() {
    // On Linux is_supported() is false, so enable returns a Config error.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
        assert!(manager.enable("127.0.0.1", 18880, None).is_err());
    }
}

#[test]
fn test_is_supported() {
    let supported = SystemProxyManager::is_supported();
    println!("System proxy supported: {}", supported);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    assert_eq!(supported, Sysproxy::is_support());
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    assert!(!supported);
}

#[test]
fn test_proxy_backup_serialization() {
    let backup = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: "localhost".to_string(),
    };

    let json = serde_json::to_string(&backup).unwrap();
    let restored: ProxyBackup = serde_json::from_str(&json).unwrap();

    assert_eq!(backup.enable, restored.enable);
    assert_eq!(backup.host, restored.host);
    assert_eq!(backup.port, restored.port);
    assert_eq!(backup.bypass, restored.bypass);
}

#[test]
fn drop_skips_restore_when_restart_shutdown_marker_owns_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();
    crate::write_system_proxy_shutdown_mode(
        temp_dir.path(),
        crate::SystemProxyShutdownMode::PreserveForRestart,
    )
    .unwrap();

    let mut manager = SystemProxyManager::new(temp_dir.path().to_path_buf());
    manager.original_proxy = Some(Sysproxy {
        enable: false,
        host: String::new(),
        port: 0,
        bypass: String::new(),
    });
    manager.is_set = true;

    drop(manager);

    assert!(matches!(
        crate::read_system_proxy_shutdown_mode(temp_dir.path()),
        Some(crate::SystemProxyShutdownMode::PreserveForRestart)
    ));
}

#[test]
fn proxy_backup_target_matches_loopback_aliases() {
    let backup = ProxyBackup {
        enable: true,
        host: "localhost".to_string(),
        port: 8800,
        bypass: String::new(),
    };

    assert!(backup.target_matches("127.0.0.1", 8800));
    assert!(backup.target_matches("[::1]", 8800));
    assert!(!backup.target_matches("127.0.0.1", 6152));
}

#[test]
fn proxy_backup_target_does_not_match_when_disabled() {
    let backup = ProxyBackup {
        enable: false,
        host: "127.0.0.1".to_string(),
        port: 8800,
        bypass: String::new(),
    };

    assert!(!backup.target_matches("127.0.0.1", 8800));
}

#[test]
fn proxy_bypass_match_ignores_order_case_and_empty_entries() {
    assert!(proxy_bypass_lists_match(
        " localhost;*.LOCAL,127.0.0.1,,",
        "127.0.0.1,localhost,*.local"
    ));
    assert!(!proxy_bypass_lists_match(
        "localhost,127.0.0.1",
        "localhost,127.0.0.1,corp.example"
    ));
}

#[test]
fn macos_networksetup_proxy_parser_preserves_disabled_endpoint() {
    assert_eq!(
        parse_macos_networksetup_proxy(
            "Enabled: No\nServer: dormant.proxy\nPort: 8080\nAuthenticated Proxy Enabled: 0\n"
        ),
        (false, "dormant.proxy".to_string(), 8080)
    );
    assert_eq!(
        parse_macos_networksetup_proxy("Enabled: Yes\nServer: 127.0.0.1\nPort: invalid\nnoise\n"),
        (true, "127.0.0.1".to_string(), 0)
    );
}

#[test]
fn proxy_state_match_supports_platform_and_all_service_checks() {
    let actual = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: "localhost;127.0.0.1".to_string(),
    };
    assert!(proxy_state_matches_expected(
        &actual,
        "127.0.0.1",
        18880,
        "127.0.0.1,localhost",
        None,
    ));
    assert!(!proxy_state_matches_expected(
        &actual,
        "127.0.0.1",
        8800,
        "127.0.0.1,localhost",
        None,
    ));
    assert!(proxy_state_matches_expected(
        &actual,
        "ignored-by-service-audit",
        1,
        "localhost,127.0.0.1",
        Some(true),
    ));
    assert!(!proxy_state_matches_expected(
        &actual,
        "127.0.0.1",
        18880,
        "localhost,127.0.0.1",
        Some(false),
    ));
}

#[test]
fn explicit_disable_detects_backup_that_restores_managed_target() {
    let backup = ProxyBackup {
        enable: true,
        host: "localhost".to_string(),
        port: 18880,
        bypass: "different.example".to_string(),
    };
    let target = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: "localhost,127.0.0.1".to_string(),
    };

    assert!(backup_restores_managed_target(&backup, Some(&target)));
}

#[test]
fn explicit_disable_preserves_backup_for_external_proxy() {
    let backup = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 6152,
        bypass: String::new(),
    };
    let target = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };

    assert!(!backup_restores_managed_target(&backup, Some(&target)));
}

#[test]
fn restart_handoff_preserves_recorded_original_when_all_conditions_met() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "10.0.0.1".to_string(),
            port: 7070,
            bypass: "corp.example".to_string(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };

    let preserved =
        restart_handoff_preserved_original(false, Some(&state), true, "127.0.0.1", 18880)
            .expect("should preserve recorded original");

    assert!(preserved.enable);
    assert_eq!(preserved.host, "10.0.0.1");
    assert_eq!(preserved.port, 7070);
    assert_eq!(preserved.bypass, "corp.example");
}

#[test]
fn restart_handoff_does_not_preserve_when_manager_already_set() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "10.0.0.1".to_string(),
            port: 7070,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };

    // is_set == true means the live manager owns its own original; the
    // existing in-process backup path handles preservation instead.
    assert!(
        restart_handoff_preserved_original(true, Some(&state), true, "127.0.0.1", 18880).is_none()
    );
}

#[test]
fn restart_handoff_does_not_preserve_without_existing_state() {
    assert!(restart_handoff_preserved_original(false, None, true, "127.0.0.1", 18880).is_none());
}

#[test]
fn restart_handoff_does_not_preserve_when_target_mismatches() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "10.0.0.1".to_string(),
            port: 7070,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };

    // The requested host:port does not match the on-disk managed target, so
    // this is a genuine fresh enable and the current proxy must be backed up.
    assert!(
        restart_handoff_preserved_original(false, Some(&state), true, "127.0.0.1", 6152).is_none()
    );
}

#[test]
fn restart_handoff_does_not_preserve_when_current_not_pointing_at_target() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "10.0.0.1".to_string(),
            port: 7070,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };

    // The OS proxy no longer points at the managed target, so we cannot
    // assume this is a restart handoff; fall back to backing up the current
    // proxy rather than blindly trusting stale recorded state.
    assert!(
        restart_handoff_preserved_original(false, Some(&state), false, "127.0.0.1", 18880)
            .is_none()
    );
}

#[test]
fn managed_target_listener_detects_live_loopback_port() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let target = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port,
        bypass: String::new(),
    };

    assert!(managed_target_listener_is_alive(&target));
}

#[test]
fn crash_recovery_restores_when_current_points_to_managed_target() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let current = ProxyBackup {
        enable: true,
        host: "localhost".to_string(),
        port: 18880,
        bypass: String::new(),
    };

    assert_eq!(
        decide_managed_state_recovery(&current, &state),
        CrashRecoveryDecision::RestoreOriginal
    );
}

#[test]
fn crash_recovery_preserves_external_proxy_on_different_port() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 6152,
        bypass: String::new(),
    };

    assert_eq!(
        decide_managed_state_recovery(&current, &state),
        CrashRecoveryDecision::PreserveExternal
    );
}

#[test]
fn crash_recovery_discards_pending_apply_when_target_was_never_set() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: false,
        phase: Some(ManagedSystemProxyPhase::PendingApply),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 6152,
        bypass: String::new(),
    };

    assert_eq!(
        decide_managed_state_recovery(&current, &state),
        CrashRecoveryDecision::DiscardPendingApply
    );
}

#[test]
fn crash_recovery_restores_pending_apply_when_target_is_visible() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: false,
        phase: Some(ManagedSystemProxyPhase::PendingApply),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let current = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };

    assert_eq!(
        decide_managed_state_recovery(&current, &state),
        CrashRecoveryDecision::RestoreOriginal
    );
}

#[test]
fn macos_recovery_keeps_managed_state_when_services_are_not_ready() {
    let state = ManagedProxyState {
        schema_version: 2,
        generation: "test-generation".into(),
        original: ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".to_string(),
            port: 18880,
            bypass: String::new(),
        },
        applied: true,
        phase: Some(ManagedSystemProxyPhase::Applied),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let current = ProxyBackup {
        enable: false,
        host: String::new(),
        port: 0,
        bypass: String::new(),
    };
    let error = BifrostError::Config(
        "No enabled macOS network services were returned by networksetup".to_string(),
    );

    let result = decide_macos_managed_state_recovery(&current, &state, Err(error));

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No enabled macOS network services"));
}

#[test]
fn macos_runtime_recovery_keeps_runtime_state_when_services_are_not_ready() {
    let error = BifrostError::Config(
        "No enabled macOS network services were returned by networksetup".to_string(),
    );

    let result = decide_macos_runtime_target_match(Err(error));

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No enabled macOS network services"));
}

#[cfg(target_os = "macos")]
#[test]
fn system_proxy_lock_timeout_env_defaults_and_overrides() {
    assert_eq!(
        system_proxy_lock_wait_timeout_from_env(None),
        std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
    );
    assert_eq!(
        system_proxy_lock_wait_timeout_from_env(Some("250")),
        std::time::Duration::from_millis(250)
    );
    assert_eq!(
        system_proxy_lock_wait_timeout_from_env(Some("0")),
        std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
    );
    assert_eq!(
        system_proxy_lock_wait_timeout_from_env(Some("not-a-number")),
        std::time::Duration::from_millis(DEFAULT_LOCK_WAIT_TIMEOUT_MS)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn system_proxy_lock_is_world_writable_after_creation() {
    use std::os::unix::fs::PermissionsExt;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-system-proxy-lock-mode-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    // Drop the lock before chmod-checking; releasing flock is fine, but
    // we want to inspect the persisted mode bits.
    {
        let _lock = acquire_system_proxy_file_lock(&data_dir, "test_mode_create")
            .expect("acquire fresh lock");
    }
    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let mode = std::fs::metadata(&lock_path)
        .expect("stat lock")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o666, "lock file mode should be 0o666 on creation");

    // Tighten the mode and re-acquire: the helper must heal it back to 0o666.
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))
        .expect("tighten lock mode");
    {
        let _lock =
            acquire_system_proxy_file_lock(&data_dir, "test_mode_relax").expect("re-acquire lock");
    }
    let mode = std::fs::metadata(&lock_path)
        .expect("stat lock")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o666, "lock file mode should be relaxed to 0o666");
    let _ = std::fs::remove_dir_all(data_dir);
}

#[cfg(target_os = "macos")]
#[test]
fn system_proxy_lock_rejects_symlink() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-system-proxy-lock-symlink-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let target = data_dir.join("target");
    std::fs::write(&target, "target").expect("write target");
    let lock_path = data_dir.join(LOCK_FILE_NAME);
    std::os::unix::fs::symlink(&target, &lock_path).expect("create symlink");

    let result = acquire_system_proxy_file_lock(&data_dir, "test_symlink");

    assert!(result.is_err(), "symlink lock must be rejected");
    let _ = std::fs::remove_dir_all(data_dir);
}

#[cfg(target_os = "macos")]
#[test]
fn system_proxy_file_lock_serializes_cross_process_entries() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-system-proxy-lock-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let first =
        acquire_system_proxy_file_lock(&data_dir, "test_first").expect("acquire first lock");
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_data_dir = data_dir.clone();

    let handle = std::thread::spawn(move || {
        let _second = acquire_system_proxy_file_lock(&thread_data_dir, "test_second")
            .expect("acquire second lock");
        tx.send(()).expect("send acquired");
    });

    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "second lock acquired before first lock was released"
    );
    drop(first);
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .expect("second lock acquired after first lock release");
    handle.join().expect("join lock thread");
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn last_runtime_proxy_target_reads_runtime_host_and_port() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-target-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    std::fs::write(
        data_dir.join(RUNTIME_FILE_NAME),
        r#"{"pid":12345,"host":"localhost","port":18889}"#,
    )
    .expect("write runtime");

    let target = load_last_runtime_proxy_target(&data_dir).expect("runtime target");

    assert_eq!(target.host, "localhost");
    assert_eq!(target.port, 18889);
    assert!(target.enable);
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn last_runtime_proxy_target_maps_wildcard_host_to_loopback() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-wildcard-target-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    std::fs::write(
        data_dir.join(RUNTIME_FILE_NAME),
        r#"{"pid":12345,"host":"0.0.0.0","port":18880}"#,
    )
    .expect("write runtime");

    let target = load_last_runtime_proxy_target(&data_dir).expect("runtime target");

    assert_eq!(target.host, "127.0.0.1");
    assert_eq!(target.port, 18880);
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn last_runtime_proxy_target_ignores_invalid_or_missing_port() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-invalid-target-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    std::fs::write(
        data_dir.join(RUNTIME_FILE_NAME),
        r#"{"pid":12345,"host":"127.0.0.1","port":70000}"#,
    )
    .expect("write runtime");

    assert!(load_last_runtime_proxy_target(&data_dir).is_none());
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn current_proxy_matches_last_runtime_target_with_loopback_alias() {
    let current = ProxyBackup {
        enable: true,
        host: "localhost".to_string(),
        port: 18880,
        bypass: String::new(),
    };
    let target = ProxyBackup {
        enable: true,
        host: "127.0.0.1".to_string(),
        port: 18880,
        bypass: String::new(),
    };

    assert!(current_proxy_matches_target(&current, &target));
}

#[test]
fn last_runtime_target_has_live_listener_detects_runtime_port() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-listener-target-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    std::fs::write(
        data_dir.join(RUNTIME_FILE_NAME),
        format!(r#"{{"pid":12345,"host":"127.0.0.1","port":{port}}}"#),
    )
    .expect("write runtime");

    assert!(SystemProxyManager::last_runtime_target_has_live_listener(
        &data_dir
    ));
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn last_runtime_target_has_live_listener_resolves_localhost() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bifrost-runtime-localhost-target-{unique}"));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    std::fs::write(
        data_dir.join(RUNTIME_FILE_NAME),
        format!(r#"{{"pid":12345,"host":"localhost","port":{port}}}"#),
    )
    .expect("write runtime");

    assert!(SystemProxyManager::last_runtime_target_has_live_listener(
        &data_dir
    ));
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn pending_apply_is_not_resumable_suspension() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    let mut pending = make_state(false, 18881);
    pending.phase = None;
    manager.write_managed_state(&pending).unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    let ownership = manager.read_managed_ownership().unwrap().unwrap();
    assert!(!ownership.is_suspended());
    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[test]
fn repeated_enable_state_keeps_first_original_and_generation() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SystemProxyManager::new(dir.path().to_path_buf());
    let original = Sysproxy {
        enable: true,
        host: "corporate-proxy".into(),
        port: 8080,
        bypass: "*.corp".into(),
    };
    let target = Sysproxy {
        enable: true,
        host: "127.0.0.1".into(),
        port: 18881,
        bypass: "localhost".into(),
    };
    manager
        .save_managed_state(&original, &target, false)
        .unwrap();
    let first = manager.load_managed_state().unwrap();
    // Normal -> GUI -> sudo retries can observe Bifrost itself as current.
    for applied in [false, false, true] {
        manager
            .save_managed_state(&target, &target, applied)
            .unwrap();
        let next = manager.load_managed_state().unwrap();
        assert_eq!(next.original, ProxyBackup::from(&original));
        assert_eq!(next.generation, first.generation);
    }
}

#[test]
fn applied_generation_adoption_is_read_only_and_arms_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    manager
        .write_managed_state(&make_state(true, 18881))
        .unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );
    assert!(manager.is_set);
    assert!(manager.original_proxy.is_some());
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    manager.detach_in_place();
}

#[cfg(not(target_os = "macos"))]
#[test]
fn suspended_retarget_preserves_baseline_and_fences_old_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    let initial = make_state(false, 18881);
    manager.write_managed_state(&initial).unwrap();
    assert_eq!(
        manager
            .retarget_managed_if_generation("test-generation", "127.0.0.1", 18882, None)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let after = manager.load_managed_state().unwrap();
    assert_eq!(after.phase(), ManagedSystemProxyPhase::Suspended);
    assert_eq!(after.original, initial.original);
    assert_eq!(after.target.port, 18882);
    assert_ne!(after.generation, initial.generation);
    assert!(!manager.is_set);
    assert_eq!(
        manager
            .resume_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager
            .resume_managed_if_generation(&after.generation)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    manager.detach_in_place();
}

#[cfg(not(target_os = "macos"))]
#[test]
fn generation_fenced_cleanup_rejects_replacement_runtime_and_new_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    manager
        .write_managed_state(&make_state(true, 18881))
        .unwrap();
    assert_eq!(
        manager
            .restore_managed_if_generation_guarded("test-generation", || Ok(false))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(manager.state_file_path().exists());
    assert_eq!(
        manager
            .restore_managed_if_generation("obsolete-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(manager.state_file_path().exists());
    assert_eq!(
        manager
            .restore_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert!(!manager.state_file_path().exists());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn explicit_pending_generation_reconciles_but_legacy_pending_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_path_buf());
    manager.skip_os_proxy_io = true;
    let mut state = make_state(false, 18881);
    state.phase = None;
    manager.write_managed_state(&state).unwrap();
    assert_eq!(
        manager
            .reconcile_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    state.phase = Some(ManagedSystemProxyPhase::PendingApply);
    manager.write_managed_state(&state).unwrap();
    assert_eq!(
        manager
            .reconcile_managed_if_generation("test-generation")
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert!(manager.load_managed_state().unwrap().applied);
    manager.detach_in_place();
}
