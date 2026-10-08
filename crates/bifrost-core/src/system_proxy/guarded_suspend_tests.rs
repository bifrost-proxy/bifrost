use super::*;

fn fixture() -> (tempfile::TempDir, SystemProxyManager) {
    let directory = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(directory.path().to_owned());
    manager.skip_os_proxy_io = true;
    manager
        .write_managed_state(&ManagedProxyState {
            schema_version: 3,
            generation: "lease-a".into(),
            original: ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".into(),
                port: 18883,
                bypass: "localhost".into(),
            },
            applied: true,
            phase: Some(ManagedSystemProxyPhase::Applied),
            authorization_suppressed: false,
            macos_services: Vec::new(),
        })
        .unwrap();
    (directory, manager)
}

#[test]
fn rejected_suspend_predicate_preserves_authoritative_journal() {
    let (_directory, mut manager) = fixture();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    let called = std::cell::Cell::new(false);
    assert_eq!(
        manager
            .suspend_managed_if_generation_guarded("lease-a", || {
                called.set(true);
                Ok(false)
            })
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(called.get());
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    assert!(!manager.is_set());
}

#[test]
fn stale_generation_does_not_evaluate_suspend_predicate() {
    let (_directory, mut manager) = fixture();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .suspend_managed_if_generation_guarded("old-lease", || {
                panic!("stale helpers must not reach the runtime predicate")
            })
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[test]
fn predicate_failure_preserves_journal_and_success_retains_suspended_lease() {
    let (_directory, mut manager) = fixture();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert!(manager
        .suspend_managed_if_generation_guarded("lease-a", || {
            Err(BifrostError::Config("runtime identity unavailable".into()))
        })
        .is_err());
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    assert_eq!(
        manager
            .suspend_managed_if_generation_guarded("lease-a", || Ok(true))
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let retained = manager.load_managed_state().unwrap();
    assert_eq!(retained.generation, "lease-a");
    assert_eq!(retained.phase(), ManagedSystemProxyPhase::Suspended);
    assert!(!retained.applied);
    assert!(!manager.is_set());
}
