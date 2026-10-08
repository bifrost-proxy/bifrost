use super::*;

fn mocked_manager() -> (tempfile::TempDir, SystemProxyManager) {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_owned());
    manager.skip_os_proxy_io = true;
    (dir, manager)
}

#[test]
fn mock_configuration_selects_only_shared_fake_adapters_for_every_privilege() {
    let (_dir, manager) = mocked_manager();
    for privilege in [Privilege::Direct, Privilege::Gui, Privilege::Sudo] {
        let mut os = manager.macos_backend(privilege).unwrap();
        assert!(matches!(&os, MacosBackend::Mock { .. }));
        os.write(
            MockMacosState::SERVICE,
            &Operation::Endpoint {
                field: Field::Http,
                host: "127.0.0.1".into(),
                port: 18888,
            },
        )
        .unwrap();
    }
    let state = manager.mock_macos_state.lock().unwrap();
    assert_eq!(state.as_ref().unwrap().writes.len(), 3);
    assert!(
        matches!(&state.as_ref().unwrap().values[&(MockMacosState::SERVICE.into(), Field::Http as u8)], Value::Protocol(proxy) if proxy.enabled && proxy.port == 18888)
    );
}

#[test]
fn invalid_mock_journal_fails_without_falling_back_to_native_adapter() {
    let (_dir, manager) = mocked_manager();
    std::fs::write(manager.state_file_path(), "{broken").unwrap();
    for privilege in [Privilege::Direct, Privilege::Gui, Privilege::Sudo] {
        assert!(manager.macos_backend(privilege).is_err());
    }
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
}

#[test]
fn native_proxy_processes_fail_closed_before_spawn_in_unit_tests() {
    for program in [
        "/usr/sbin/networksetup",
        "/usr/sbin/scutil",
        "/usr/bin/osascript",
        "/usr/bin/sudo",
    ] {
        let error = super::super::macos_command::run_bounded(
            program,
            &["must-not-execute"],
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("forbidden in unit tests"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn actual_macos_manager_uses_fake_for_explicit_pending_retarget_and_cleanup() {
    let (_dir, mut manager) = mocked_manager();
    // Exercise the actual manager, journal and shared per-service transitions.
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    assert!(os.services().is_ok());
    manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .next_write_error = Some("RequiresAdmin: injected permission failure".into());
    assert!(manager.enable("127.0.0.1", 18888, None).is_err());
    let generation = manager
        .read_managed_ownership()
        .unwrap()
        .unwrap()
        .generation;
    assert_eq!(
        manager
            .reconcile_managed_if_generation_with_gui_auth(&generation)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert_eq!(
        manager
            .retarget_managed_if_generation(&generation, "127.0.0.1", 18889, None)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let generation = manager
        .read_managed_ownership()
        .unwrap()
        .unwrap()
        .generation;
    assert_eq!(
        manager.suspend_managed_if_generation(&generation).unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert_eq!(
        manager.resume_managed_if_generation(&generation).unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert!(matches!(
        manager
            .disable_managed_explicit_if_generation_with_privilege_guarded(&generation, || Ok(true)),
        Err(BifrostError::Config(message)) if message.starts_with("IncompleteRestore:")
    ));
    assert!(!manager.is_set());
    assert!(manager
        .data_dir
        .join("system_proxy_incomplete_restores.json")
        .exists());
    assert!(!manager.state_file_path().exists());
    assert!(
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes
            .len()
            > 6
    );
}

#[cfg(target_os = "macos")]
#[test]
fn unwinding_after_mock_adoption_restores_only_the_shared_fake_os() {
    let (_dir, mut manager) = mocked_manager();
    manager.enable("127.0.0.1", 18888, None).unwrap();
    let state = manager.mock_macos_state.clone();
    let before = state.lock().unwrap().as_ref().unwrap().writes.len();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _manager = manager;
        panic!("simulate a unit-test assertion failure after acquisition");
    }));
    assert!(panic.is_err());
    let state = state.lock().unwrap();
    let state = state.as_ref().unwrap();
    assert!(state.writes.len() > before);
    for field in [Field::Http, Field::Https] {
        assert!(
            matches!(&state.values[&(MockMacosState::SERVICE.into(), field as u8)], Value::Protocol(proxy) if !proxy.enabled)
        );
    }
}

#[test]
fn adopted_mock_manager_restores_only_fake_os_on_normal_and_unwinding_drop() {
    for unwind in [false, true] {
        let (dir, mut manager) = mocked_manager();
        let mut journal = ManagedProxyState {
            schema_version: 3,
            generation: "mock-drop-generation".into(),
            original: ProxyBackup {
                enable: true,
                host: "corp-proxy".into(),
                port: 8443,
                bypass: "*.corp".into(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".into(),
                port: 18888,
                bypass: "localhost".into(),
            },
            applied: true,
            phase: Some(ManagedSystemProxyPhase::Applied),
            authorization_suppressed: false,
            macos_services: Vec::new(),
        };
        journal.macos_services = mock_services_for_state(&journal);
        manager.write_managed_state(&journal).unwrap();
        let _backend = manager.macos_backend(Privilege::Direct).unwrap();
        assert_eq!(
            manager
                .resume_managed_if_generation(&journal.generation)
                .unwrap(),
            GuardedSystemProxyTransition::AlreadyInState
        );
        assert!(manager.is_set());
        let fake_os = manager.mock_macos_state.clone();
        if unwind {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _manager = manager;
                panic!("simulate an assertion failure after mock adoption");
            }));
            assert!(panic.is_err());
        } else {
            drop(manager);
        }
        assert!(!dir.path().join(STATE_FILE_NAME).exists());
        let fake_os = fake_os.lock().unwrap();
        let fake_os = fake_os.as_ref().unwrap();
        for field in [Field::Http, Field::Https] {
            assert!(
                matches!(&fake_os.values[&(MockMacosState::SERVICE.into(), field as u8)], Value::Protocol(proxy) if proxy.enabled && proxy.host == "corp-proxy" && proxy.port == 8443)
            );
        }
        assert_eq!(
            fake_os.values[&(MockMacosState::SERVICE.into(), Field::Bypass as u8)],
            Value::Bypass(vec!["*.corp".into()])
        );
        assert_eq!(fake_os.writes.len(), 3);
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn aggregate_mock_acquisition_and_legacy_mutations_fail_closed_before_native_io() {
    let (dir, mut manager) = mocked_manager();
    for result in [
        manager.enable("127.0.0.1", 18888, None).map(|_| ()),
        manager
            .enable_if_unmanaged("127.0.0.1", 18888, None)
            .map(|_| ()),
        manager.force_disable_without_file_lock(),
        manager
            .disable_if_matches_inner("127.0.0.1", 18888, false)
            .map(|_| ()),
        manager.apply_proxy_backup(&ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        }),
    ] {
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("disabled for mock managers"));
    }
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn constructing_native_adapter_and_checking_authorization_do_not_run_commands() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SystemProxyManager::new(dir.path().to_owned());
    // Construction and this predicate are pure. Never query or write this adapter.
    for (privilege, expected) in [
        (Privilege::Direct, false),
        (Privilege::Gui, true),
        (Privilege::Sudo, true),
    ] {
        let adapter = manager.macos_backend(privilege).unwrap();
        assert!(matches!(adapter, MacosBackend::Native(_)));
        assert_eq!(adapter.requires_authorization(), expected);
    }
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn uninitialized_and_poisoned_mock_adapters_fail_closed_for_every_operation() {
    let (_dir, manager) = mocked_manager();
    let mut adapter = MacosBackend::Mock {
        state: manager.mock_macos_state.clone(),
        privilege: Privilege::Gui,
    };
    let operation = Operation::Enabled {
        field: Field::Http,
        enabled: false,
    };
    for error in [
        adapter.services().unwrap_err(),
        adapter
            .read(MockMacosState::SERVICE, Field::Http)
            .unwrap_err(),
        adapter
            .write(MockMacosState::SERVICE, &operation)
            .unwrap_err(),
    ] {
        assert!(error.to_string().contains("uninitialized"));
    }
    let shared = manager.mock_macos_state.clone();
    let panic = std::panic::catch_unwind(move || {
        let _guard = shared.lock().unwrap();
        panic!("simulate a failed fixture update while holding the mock lock");
    });
    assert!(panic.is_err());
    for privilege in [Privilege::Direct, Privilege::Gui, Privilege::Sudo] {
        assert!(
            matches!(manager.macos_backend(privilege), Err(error) if error.to_string().contains("poisoned"))
        );
    }
    for error in [
        adapter.services().unwrap_err(),
        adapter
            .read(MockMacosState::SERVICE, Field::Http)
            .unwrap_err(),
        adapter
            .write(MockMacosState::SERVICE, &operation)
            .unwrap_err(),
    ] {
        assert!(error.to_string().contains("poisoned"));
    }
    assert!(!manager.state_file_path().exists());
}

#[test]
fn mock_rejects_unknown_or_non_protocol_fields_without_corrupting_snapshot() {
    let (_dir, manager) = mocked_manager();
    let mut adapter = manager.macos_backend(Privilege::Direct).unwrap();
    assert!(adapter.read("missing service", Field::Http).is_err());
    let original = manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .values
        .clone();
    for operation in [
        Operation::Endpoint {
            field: Field::Bypass,
            host: "unexpected".into(),
            port: 18888,
        },
        Operation::Enabled {
            field: Field::Bypass,
            enabled: true,
        },
    ] {
        assert!(adapter
            .write(MockMacosState::SERVICE, &operation)
            .unwrap_err()
            .to_string()
            .contains("Invalid mock protocol"));
    }
    let state = manager.mock_macos_state.lock().unwrap();
    assert_eq!(state.as_ref().unwrap().values, original);
    assert_eq!(state.as_ref().unwrap().writes.len(), 2);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn mock_cleanup_with_missing_or_corrupt_journal_never_attempts_native_io() {
    let (_dir, mut manager) = mocked_manager();
    manager.is_set = true;
    manager.restore_mock_aggregate().unwrap();
    assert!(!manager.is_set());
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
    std::fs::write(manager.state_file_path(), b"{broken").unwrap();
    assert!(manager.restore_mock_aggregate().is_err());
    assert_eq!(
        std::fs::read(manager.state_file_path()).unwrap(),
        b"{broken"
    );
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
}

#[test]
fn unapplied_aggregate_journal_seeds_original_not_target_into_mock_os() {
    let original = ProxyBackup {
        enable: true,
        host: "corp-proxy".into(),
        port: 8443,
        bypass: "*.corp".into(),
    };
    let journal = ManagedProxyState {
        schema_version: 3,
        generation: "pending".into(),
        original: original.clone(),
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".into(),
            port: 18888,
            bypass: "localhost".into(),
        },
        applied: false,
        phase: Some(ManagedSystemProxyPhase::PendingApply),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    };
    let state = MockMacosState::from_journal(Some(&journal));
    for field in [Field::Http, Field::Https, Field::Bypass] {
        assert_eq!(
            state.values[&(MockMacosState::SERVICE.into(), field as u8)],
            mock_value(&original, field)
        );
    }
    assert!(state.writes.is_empty());
    assert_eq!(state.reads, 0);
    #[cfg(not(target_os = "macos"))]
    {
        // Cleanup can migrate an aggregate journal that has no service entries.
        // Its pending target was never applied, so the original needs no writes.
        let (_dir, mut manager) = mocked_manager();
        manager.write_managed_state(&journal).unwrap();
        manager.restore_mock_aggregate().unwrap();
        assert!(!manager.state_file_path().exists());
        let fake = manager.mock_macos_state.lock().unwrap();
        assert_eq!(fake.as_ref().unwrap().values, state.values);
        assert!(fake.as_ref().unwrap().writes.is_empty());
    }
}

#[test]
fn injected_mock_write_failure_is_consumed_once_and_retry_uses_same_state() {
    let (_dir, manager) = mocked_manager();
    let mut adapter = manager.macos_backend(Privilege::Sudo).unwrap();
    let original = {
        let mut state = manager.mock_macos_state.lock().unwrap();
        let state = state.as_mut().unwrap();
        state.next_write_error = Some("RequiresAdmin: one-shot mock failure".into());
        state.values.clone()
    };
    let operation = Operation::Enabled {
        field: Field::Http,
        enabled: true,
    };
    assert!(adapter
        .write(MockMacosState::SERVICE, &operation)
        .unwrap_err()
        .to_string()
        .contains("RequiresAdmin"));
    assert_eq!(
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .values,
        original
    );
    adapter.write(MockMacosState::SERVICE, &operation).unwrap();
    assert!(
        matches!(adapter.read(MockMacosState::SERVICE, Field::Http).unwrap(), Value::Protocol(proxy) if proxy.enabled)
    );
    let state = manager.mock_macos_state.lock().unwrap();
    assert!(state.as_ref().unwrap().next_write_error.is_none());
    assert_eq!(state.as_ref().unwrap().writes.len(), 2);
}
