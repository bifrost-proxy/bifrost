use super::*;

fn fixture() -> (tempfile::TempDir, SystemProxyManager) {
    let directory = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(directory.path().to_owned());
    manager.skip_os_proxy_io = true;
    manager
        .write_managed_state(&ManagedProxyState {
            schema_version: 3,
            generation: "authorization-generation".into(),
            original: ProxyBackup {
                enable: false,
                host: String::new(),
                port: 0,
                bypass: String::new(),
            },
            target: ProxyBackup {
                enable: true,
                host: "127.0.0.1".into(),
                port: 18884,
                bypass: "localhost".into(),
            },
            applied: false,
            phase: Some(ManagedSystemProxyPhase::PendingApply),
            authorization_suppressed: false,
            macos_services: Vec::new(),
        })
        .unwrap();
    (directory, manager)
}

#[test]
fn authorization_suppression_is_durable_without_disabling_desired_proxy() {
    let (_directory, mut manager) = fixture();
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation("authorization-generation")
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let ownership = manager.read_managed_ownership().unwrap().unwrap();
    assert!(ownership.authorization_suppressed);
    assert!(ownership.target.enable);
    assert_eq!(ownership.phase, Some(ManagedSystemProxyPhase::PendingApply));
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation("authorization-generation")
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[test]
fn stale_dialog_cannot_suppress_a_new_explicit_generation() {
    let (_directory, mut manager) = fixture();
    manager
        .suppress_managed_authorization_if_generation("authorization-generation")
        .unwrap();
    let mut state = manager.load_managed_state().unwrap();
    macos_owned::begin_explicit_acquisition(&mut state);
    manager.write_managed_state(&state).unwrap();
    assert_ne!(state.generation, "authorization-generation");
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation("authorization-generation")
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(
        !manager
            .read_managed_ownership()
            .unwrap()
            .unwrap()
            .authorization_suppressed
    );
}

#[test]
fn rejected_explicit_enable_predicate_does_not_clear_suppression() {
    let (_directory, mut manager) = fixture();
    manager
        .suppress_managed_authorization_if_generation("authorization-generation")
        .unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .enable_guarded("127.0.0.1", 18884, None, || Ok(false))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[test]
fn old_records_default_to_unsuppressed_authorization() {
    let (_directory, manager) = fixture();
    let mut value = serde_json::to_value(manager.load_managed_state().unwrap()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("authorization_suppressed");
    let state: ManagedProxyState = serde_json::from_value(value.clone()).unwrap();
    let ownership: ManagedSystemProxyOwnership = serde_json::from_value(value).unwrap();
    assert!(!state.authorization_suppressed);
    assert!(!ownership.authorization_suppressed);
}

#[test]
fn late_cancel_for_superseded_intent_cannot_suppress_same_generation() {
    let (_directory, mut manager) = fixture();
    let mut state = manager.load_managed_state().unwrap();
    macos_owned::begin_explicit_disable(&mut state);
    manager.write_managed_state(&state).unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    let cancelled_enable_revision = "enable-revision";
    let current_revision = "new-disable-revision";
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation_guarded(
                "authorization-generation",
                || Ok(cancelled_enable_revision == current_revision)
            )
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    assert!(
        !manager
            .read_managed_ownership()
            .unwrap()
            .unwrap()
            .authorization_suppressed
    );
}

#[test]
fn rejected_explicit_disable_does_not_reset_authorization_cancellation() {
    let (_directory, mut manager) = fixture();
    manager
        .suppress_managed_authorization_if_generation("authorization-generation")
        .unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .disable_managed_explicit_if_generation_guarded("authorization-generation", || Ok(
                false
            ))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn aggregate_cleanup_of_verified_suspended_original_is_already_complete() {
    let (_directory, mut manager) = fixture();
    let mut state = manager.load_managed_state().unwrap();
    state.set_phase(ManagedSystemProxyPhase::Suspended);
    manager.write_managed_state(&state).unwrap();
    assert_eq!(
        manager
            .restore_managed_if_generation_guarded("authorization-generation", || Ok(true))
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );
    assert!(!manager.state_file_path().exists());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn accepted_explicit_disable_finishes_previously_cancelled_lease() {
    let (_directory, mut manager) = fixture();
    manager
        .suppress_managed_authorization_if_generation("authorization-generation")
        .unwrap();
    assert_eq!(
        manager
            .disable_managed_explicit_if_generation_guarded("authorization-generation", || Ok(true))
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert!(!manager.state_file_path().exists());
}
