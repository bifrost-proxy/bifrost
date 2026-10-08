//! Aggregate journal control-flow tests. Native Windows I/O is never invoked.
use super::*;

fn fixture() -> (tempfile::TempDir, SystemProxyManager) {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_owned());
    manager.skip_os_proxy_io = true;
    (dir, manager)
}

fn state(phase: ManagedSystemProxyPhase) -> ManagedProxyState {
    ManagedProxyState {
        schema_version: 3,
        generation: "lease".into(),
        original: ProxyBackup {
            enable: true,
            host: "corp-proxy".into(),
            port: 8443,
            bypass: "*.corp".into(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".into(),
            port: 18886,
            bypass: "localhost".into(),
        },
        applied: phase == ManagedSystemProxyPhase::Applied,
        phase: Some(phase),
        authorization_suppressed: false,
        macos_services: Vec::new(),
    }
}

#[test]
fn unsupported_platform_never_evaluates_action_predicates_or_writes_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_owned());
    assert_eq!(
        manager
            .enable_guarded("127.0.0.1", 18886, None, || panic!("no supported backend"))
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager
            .enable_if_unmanaged("127.0.0.1", 18886, None)
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation("lease")
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager.restore_managed_if_generation("lease").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager.reconcile_managed_if_generation("lease").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager
            .retarget_managed_if_generation("lease", "127.0.0.1", 18887, None)
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn missing_and_corrupt_journals_are_never_adopted_or_overwritten() {
    let (_dir, mut manager) = fixture();
    assert_eq!(
        manager
            .suppress_managed_authorization_if_generation("lease")
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager.restore_managed_if_generation("lease").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager.reconcile_managed_if_generation("lease").unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager
            .retarget_managed_if_generation("lease", "127.0.0.1", 18887, None)
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    // Linux reaches the real unsupported-platform fence, never sysproxy I/O.
    assert!(manager
        .enable_if_unmanaged("127.0.0.1", 18886, None)
        .is_err());
    assert!(manager
        .enable_guarded("127.0.0.1", 18886, None, || Ok(true))
        .is_err());
    std::fs::write(manager.state_file_path(), b"{interrupted").unwrap();
    assert!(manager
        .suppress_managed_authorization_if_generation("lease")
        .is_err());
    assert!(manager.restore_managed_if_generation("lease").is_err());
    assert!(manager.reconcile_managed_if_generation("lease").is_err());
    assert!(manager
        .retarget_managed_if_generation("lease", "127.0.0.1", 18887, None)
        .is_err());
    assert!(manager
        .enable_if_unmanaged("127.0.0.1", 18886, None)
        .is_err());
    assert_eq!(
        std::fs::read(manager.state_file_path()).unwrap(),
        b"{interrupted"
    );
}

#[test]
fn stale_generation_or_cleanup_phase_cannot_retarget_or_reconcile() {
    let (_dir, mut manager) = fixture();
    manager
        .write_managed_state(&state(ManagedSystemProxyPhase::Restoring))
        .unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .retarget_managed_if_generation("stale", "127.0.0.1", 18887, None)
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager.reconcile_managed_if_generation("stale").unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager
            .retarget_managed_if_generation("lease", "127.0.0.1", 18887, None)
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager.reconcile_managed_if_generation("lease").unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        manager
            .enable_if_unmanaged("127.0.0.1", 18886, None)
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
}

#[test]
fn applied_retarget_preserves_original_and_adopts_only_the_new_generation() {
    let (_dir, mut manager) = fixture();
    let before = state(ManagedSystemProxyPhase::Applied);
    manager.write_managed_state(&before).unwrap();
    assert_eq!(
        manager
            .retarget_managed_if_generation("lease", "localhost", 18887, Some("localhost,*.dev"))
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let after = manager.load_managed_state().unwrap();
    assert_eq!(after.original, before.original);
    assert_eq!(after.target.port, 18887);
    assert_eq!(after.target.bypass, "localhost,*.dev");
    assert_eq!(after.phase(), ManagedSystemProxyPhase::Applied);
    assert_ne!(after.generation, "lease");
    assert!(manager.is_set());
    assert_eq!(
        manager
            .reconcile_managed_if_generation(&after.generation)
            .unwrap(),
        GuardedSystemProxyTransition::AlreadyInState
    );
    manager.detach_in_place();
}

#[test]
fn backup_and_applied_marker_are_atomic_round_trips_and_suspension_is_explicit() {
    let (_dir, manager) = fixture();
    let pending = state(ManagedSystemProxyPhase::PendingApply);
    let original: Sysproxy = pending.original.clone().into();
    manager.save_backup(&original).unwrap();
    assert_eq!(
        ProxyBackup::from(&manager.load_backup().unwrap()),
        pending.original
    );
    manager.write_managed_state(&pending).unwrap();
    manager.mark_managed_state_applied().unwrap();
    assert_eq!(
        manager.load_managed_state().unwrap().phase(),
        ManagedSystemProxyPhase::Applied
    );
    let applied = std::fs::read(manager.state_file_path()).unwrap();
    manager.mark_managed_state_applied().unwrap();
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), applied);
    for phase in [
        ManagedSystemProxyPhase::PendingApply,
        ManagedSystemProxyPhase::Applied,
        ManagedSystemProxyPhase::Suspending,
        ManagedSystemProxyPhase::Suspended,
        ManagedSystemProxyPhase::Resuming,
        ManagedSystemProxyPhase::Restoring,
    ] {
        let record = state(phase);
        let ownership = ManagedSystemProxyOwnership::from(record.clone());
        assert_eq!(
            ownership.is_suspended(),
            matches!(
                phase,
                ManagedSystemProxyPhase::Suspending
                    | ManagedSystemProxyPhase::Suspended
                    | ManagedSystemProxyPhase::Resuming
            )
        );
        if matches!(
            phase,
            ManagedSystemProxyPhase::PendingApply
                | ManagedSystemProxyPhase::Suspending
                | ManagedSystemProxyPhase::Resuming
        ) {
            assert!(guarded_suspend_allowed(&record, "lease", true));
        }
    }
}

#[test]
fn rejected_or_failed_intent_predicates_leave_existing_ownership_untouched() {
    let (_dir, mut manager) = fixture();
    manager
        .write_managed_state(&state(ManagedSystemProxyPhase::Applied))
        .unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    assert_eq!(
        manager
            .enable_guarded("127.0.0.1", 18887, None, || Ok(false))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(manager
        .enable_guarded("127.0.0.1", 18887, None, || Err(BifrostError::Config(
            "intent read failed".into()
        )))
        .is_err());
    assert_eq!(
        manager
            .restore_managed_if_generation_guarded("lease", || Ok(false))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert!(manager
        .restore_managed_if_generation_guarded("lease", || Err(BifrostError::Config(
            "intent read failed".into()
        )))
        .is_err());
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
    assert!(!manager.is_set());
}

#[test]
fn empty_generation_and_legacy_unapplied_state_cannot_authorize_reconcile() {
    let (_dir, mut manager) = fixture();
    let mut journal = state(ManagedSystemProxyPhase::PendingApply);
    journal.phase = None;
    manager.write_managed_state(&journal).unwrap();
    let before = std::fs::read(manager.state_file_path()).unwrap();
    for generation in ["", "lease"] {
        assert_eq!(
            manager.reconcile_managed_if_generation(generation).unwrap(),
            GuardedSystemProxyTransition::OwnershipChanged
        );
        assert_eq!(
            manager
                .retarget_managed_if_generation(generation, "127.0.0.1", 18887, None)
                .unwrap(),
            GuardedSystemProxyTransition::OwnershipChanged
        );
    }
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
    assert!(manager.mock_macos_state.lock().unwrap().is_none());
}
