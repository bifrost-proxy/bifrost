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
    assert_eq!(
        manager
            .disable_managed_explicit_if_generation_with_privilege_guarded(&generation, || Ok(true))
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
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
