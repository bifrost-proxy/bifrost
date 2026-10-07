use super::super::macos_backend::MockMacosState;
use super::super::macos_command::Privilege;
use super::super::macos_owned::{self, Backend, Field, Intent, Protocol, Value};
use super::*;

fn protocol(enabled: bool, host: &str, port: u16) -> Value {
    Value::Protocol(Protocol {
        enabled,
        host: host.into(),
        port,
        authenticated: false,
    })
}

fn fixture() -> (tempfile::TempDir, SystemProxyManager, ManagedProxyState) {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SystemProxyManager::new(dir.path().to_owned());
    manager.skip_os_proxy_io = true;
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let state = ManagedProxyState {
        schema_version: 3,
        generation: "incomplete-generation".into(),
        original: ProxyBackup {
            enable: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".into(),
            port: 18880,
            bypass: "localhost".into(),
        },
        applied: false,
        phase: Some(ManagedSystemProxyPhase::PendingApply),
        authorization_suppressed: false,
        macos_services: macos_owned::capture(&mut os).unwrap(),
    };
    (dir, manager, state)
}

fn transition(
    manager: &SystemProxyManager,
    state: &mut ManagedProxyState,
    intent: Intent,
) -> TransitionResult {
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    macos_owned::transition(state, &mut os, intent, |s| manager.write_managed_state(s)).unwrap()
}

fn retire_incomplete(manager: &mut SystemProxyManager, state: &mut ManagedProxyState) {
    transition(manager, state, Intent::Apply);
    manager.is_set = true;
    manager.original_proxy = Some(state.original.clone().into());
    let outcome = transition(manager, state, Intent::Restore);
    assert!(outcome.incomplete_baseline);
    let error = manager.finish_macos_restore(state, outcome).unwrap_err();
    assert!(error.to_string().contains("IncompleteRestore:"));
}

#[test]
fn incomplete_final_restore_archives_original_before_retiring_active_ownership() {
    let (_dir, mut manager, mut state) = fixture();
    let original = state.macos_services.clone();
    retire_incomplete(&mut manager, &mut state);
    assert!(!manager.is_set());
    assert!(manager.read_managed_ownership().unwrap().is_none());
    assert!(!manager.state_file_path().exists());
    let records = manager.incomplete_restores().unwrap();
    assert_eq!(records.records.len(), 1);
    let archived = &records.records[0];
    for (before, after) in original.iter().zip(&archived.macos_services) {
        for (before, after) in before.fields.iter().zip(&after.fields) {
            assert_eq!(before.before, after.before);
        }
    }
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    for field in [Field::Http, Field::Https] {
        assert_eq!(
            os.read(MockMacosState::SERVICE, field).unwrap(),
            protocol(false, "127.0.0.1", 18880)
        );
    }
    let writes = manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writes
        .len();
    assert_eq!(
        manager
            .restore_managed_if_generation(&state.generation)
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes
            .len(),
        writes
    );
}

#[test]
fn safe_incomplete_suspension_retains_active_lease_and_resumes_without_recapture() {
    let (_dir, manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Suspend);
    assert!(outcome.incomplete_baseline);
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Suspended);
    assert!(manager.state_file_path().exists());
    assert!(!manager.incomplete_restores_path().exists());
    assert!(!transition(&manager, &mut state, Intent::Resume).incomplete_baseline);
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Applied);
    assert_eq!(
        state.macos_services[0].fields[0].before,
        protocol(false, "", 0)
    );
}

#[test]
fn archive_failure_preserves_authoritative_snapshot_and_does_not_repeat_os_writes() {
    let (_dir, mut manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    std::fs::create_dir(manager.incomplete_restores_path()).unwrap();
    assert!(manager.finish_macos_restore(&state, outcome).is_err());
    assert!(manager.state_file_path().exists());
    assert_eq!(
        manager.load_managed_state().unwrap().macos_services,
        state.macos_services
    );
    let writes = manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writes
        .len();
    std::fs::remove_dir(manager.incomplete_restores_path()).unwrap();
    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(!outcome.changed);
    assert!(manager
        .finish_macos_restore(&state, outcome)
        .unwrap_err()
        .to_string()
        .contains("IncompleteRestore:"));
    assert_eq!(
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes
            .len(),
        writes
    );
}

#[test]
fn later_acquisition_reuses_only_matching_originals_and_preserves_manual_fields() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let manual = protocol(false, "manual-corp", 8443);
    manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .values
        .insert(
            (MockMacosState::SERVICE.into(), Field::Https as u8),
            manual.clone(),
        );
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut fresh = macos_owned::capture(&mut os).unwrap();
    manager
        .reuse_incomplete_restore_baselines(&mut fresh)
        .unwrap();
    assert_eq!(
        fresh[0]
            .fields
            .iter()
            .find(|f| f.field == Field::Http)
            .unwrap()
            .before,
        protocol(false, "", 0)
    );
    assert_eq!(
        fresh[0]
            .fields
            .iter()
            .find(|f| f.field == Field::Https)
            .unwrap()
            .before,
        manual
    );
    assert_eq!(
        fresh[0]
            .fields
            .iter()
            .find(|f| f.field == Field::Http)
            .unwrap()
            .last_written,
        protocol(false, "127.0.0.1", 18880)
    );
    state.generation = "new-acquisition".into();
    state.macos_services = fresh;
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(manager.finish_macos_restore(&state, outcome).is_err());
    assert_eq!(manager.incomplete_restores().unwrap().records.len(), 2);
    assert_eq!(
        os.read(MockMacosState::SERVICE, Field::Https).unwrap(),
        manual
    );
}

#[test]
fn archived_disabled_endpoint_cannot_recreate_a_legacy_active_journal() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut legacy = macos_owned::legacy_journal(&mut os, &state.target).unwrap();
    assert!(legacy
        .iter()
        .flat_map(|s| &s.fields)
        .any(|f| !f.relinquished));
    manager.relinquish_archived_fields(&mut legacy).unwrap();
    assert!(legacy
        .iter()
        .flat_map(|s| &s.fields)
        .all(|f| f.relinquished));
    assert!(!manager.state_file_path().exists());
}

#[test]
fn full_restore_reports_success_and_creates_no_incomplete_archive() {
    let (_dir, mut manager, mut state) = fixture();
    for field in state.macos_services.iter_mut().flat_map(|s| &mut s.fields) {
        if matches!(field.field, Field::Http | Field::Https) {
            field.before = protocol(false, "corp-dormant", 8443);
        }
    }
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(!outcome.incomplete_baseline);
    assert_eq!(
        manager.finish_macos_restore(&state, outcome).unwrap(),
        SystemProxyDisableOutcome::Disabled
    );
    assert!(!manager.incomplete_restores_path().exists());
}

#[test]
fn relinquished_or_interrupted_archived_fields_never_supply_an_original() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let mut records = manager.incomplete_restores().unwrap();
    let fields = &mut records.records[0].macos_services[0].fields;
    fields
        .iter_mut()
        .find(|f| f.field == Field::Http)
        .unwrap()
        .relinquished = true;
    let https = fields.iter_mut().find(|f| f.field == Field::Https).unwrap();
    https.pending = Some(macos_owned::PendingWrite {
        before: https.last_written.clone(),
        possible_after: vec![https.before.clone()],
        needs_apply: false,
    });
    persistence::atomic_write(
        &manager.incomplete_restores_path(),
        &serde_json::to_vec(&records).unwrap(),
    )
    .unwrap();
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut fresh = macos_owned::capture(&mut os).unwrap();
    let before = fresh.clone();
    manager
        .reuse_incomplete_restore_baselines(&mut fresh)
        .unwrap();
    assert_eq!(fresh, before);
}

#[cfg(target_os = "macos")]
#[test]
fn actual_manager_can_reacquire_and_resume_after_terminal_incomplete_cleanup() {
    let (_dir, mut manager, _state) = fixture();
    manager.enable("127.0.0.1", 18880, None).unwrap();
    let old = manager
        .read_managed_ownership()
        .unwrap()
        .unwrap()
        .generation;
    assert!(matches!(manager.restore_managed_if_generation(&old),
        Err(BifrostError::Config(message)) if message.starts_with("IncompleteRestore:")));
    let archive = std::fs::read(manager.incomplete_restores_path()).unwrap();
    assert_eq!(
        manager
            .enable_guarded("127.0.0.1", 18881, None, || Ok(false))
            .unwrap(),
        GuardedSystemProxyTransition::OwnershipChanged
    );
    assert_eq!(
        std::fs::read(manager.incomplete_restores_path()).unwrap(),
        archive
    );
    assert!(!manager.state_file_path().exists());
    // A stale runtime target must not re-adopt the retired disabled endpoints.
    manager.disable_if_matches("127.0.0.1", 18880).unwrap();
    assert!(!manager.state_file_path().exists());
    assert_eq!(
        manager
            .enable_if_unmanaged("127.0.0.1", 18881, None)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    let state = manager.load_managed_state().unwrap();
    assert_ne!(state.generation, old);
    assert_eq!(
        state.macos_services[0].fields[0].before,
        protocol(false, "", 0)
    );
    assert_eq!(
        manager
            .suspend_managed_if_generation(&state.generation)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    assert_eq!(
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap(),
        GuardedSystemProxyTransition::Applied
    );
    manager.detach_in_place();
}

#[test]
fn disabled_service_omitted_by_newer_generation_uses_latest_evidence_for_that_field() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let mut records = manager.incomplete_restores().unwrap();
    let mut ethernet = records.records[0].macos_services[0].clone();
    ethernet.name = "Disabled Ethernet".into();
    records.records[0].macos_services.push(ethernet.clone());
    let mut newer = state.clone();
    newer.generation = "newer-wifi-only".into();
    records.records.push(newer);
    persistence::atomic_write(
        &manager.incomplete_restores_path(),
        &serde_json::to_vec(&records).unwrap(),
    )
    .unwrap();
    for field in &mut ethernet.fields {
        field.before = field.last_written.clone();
    }
    let mut capture = vec![ethernet.clone()];
    manager
        .reuse_incomplete_restore_baselines(&mut capture)
        .unwrap();
    assert_eq!(capture[0].fields[0].before, protocol(false, "", 0));
    manager.relinquish_archived_fields(&mut capture).unwrap();
    assert!(capture[0]
        .fields
        .iter()
        .filter(|f| matches!(f.field, Field::Http | Field::Https))
        .all(|f| f.relinquished));
    // A newer observation that gave up this field must supersede old evidence.
    ethernet
        .fields
        .iter_mut()
        .for_each(|field| field.relinquished = true);
    records
        .records
        .last_mut()
        .unwrap()
        .macos_services
        .push(ethernet.clone());
    persistence::atomic_write(
        &manager.incomplete_restores_path(),
        &serde_json::to_vec(&records).unwrap(),
    )
    .unwrap();
    ethernet
        .fields
        .iter_mut()
        .for_each(|field| field.relinquished = false);
    let mut capture = vec![ethernet];
    let before = capture.clone();
    manager
        .reuse_incomplete_restore_baselines(&mut capture)
        .unwrap();
    assert_eq!(capture, before);
    manager.relinquish_archived_fields(&mut capture).unwrap();
    assert!(capture[0].fields.iter().all(|field| field.relinquished));
}

#[test]
fn retired_evidence_cannot_authorize_disabling_a_manual_reenable() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let current = protocol(true, "127.0.0.1", 18880);
    manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .values
        .insert(
            (MockMacosState::SERVICE.into(), Field::Http as u8),
            current.clone(),
        );
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut legacy = macos_owned::legacy_journal(&mut os, &state.target).unwrap();
    assert!(!legacy[0].fields[0].relinquished);
    manager.relinquish_archived_fields(&mut legacy).unwrap();
    assert!(legacy[0].fields.iter().all(|field| field.relinquished));
    assert_eq!(
        os.read(MockMacosState::SERVICE, Field::Http).unwrap(),
        current
    );
    let mut fresh = macos_owned::capture(&mut os).unwrap();
    manager
        .reuse_incomplete_restore_baselines(&mut fresh)
        .unwrap();
    assert_eq!(fresh[0].fields[0].before, current);
}

#[test]
fn enabled_empty_or_zero_original_is_excluded_without_changing_its_service() {
    for (host, port) in [("", 0), ("", 8443), ("invalid-original", 0)] {
        let (_dir, manager, mut state) = fixture();
        let original = protocol(true, host, port);
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .values
            .insert(
                (MockMacosState::SERVICE.into(), Field::Http as u8),
                original.clone(),
            );
        let mut os = manager.macos_backend(Privilege::Direct).unwrap();
        state.macos_services = macos_owned::capture(&mut os).unwrap();
        assert!(state.macos_services[0]
            .fields
            .iter()
            .all(|field| field.relinquished));
        let outcome = transition(&manager, &mut state, Intent::Apply);
        assert!(outcome.ownership_changed);
        assert!(!outcome.changed);
        assert!(!macos_owned::has_active_owned_protocol(&state));
        assert_eq!(
            os.read(MockMacosState::SERVICE, Field::Http).unwrap(),
            original
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
}

#[test]
fn preexisting_enabled_invalid_original_reports_error_without_toggling_owned_routing() {
    for enabled in [false, true] {
        let (_dir, manager, mut state) = fixture();
        state.macos_services[0].fields[0].before = protocol(true, "", 0);
        transition(&manager, &mut state, Intent::Apply);
        let current = protocol(enabled, "127.0.0.1", 18880);
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .values
            .insert(
                (MockMacosState::SERVICE.into(), Field::Http as u8),
                current.clone(),
            );
        state.macos_services[0].fields[0].last_written = current.clone();
        let before = manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes
            .len();
        let mut os = manager.macos_backend(Privilege::Direct).unwrap();
        assert!(
            macos_owned::transition(&mut state, &mut os, Intent::Restore, |state| manager
                .write_managed_state(state))
            .is_err()
        );
        assert!(manager.state_file_path().exists());
        assert!(!manager.incomplete_restores_path().exists());
        assert_eq!(
            os.read(MockMacosState::SERVICE, Field::Http).unwrap(),
            current
        );
        assert!(!manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes[before..]
            .iter()
            .any(|(_, operation)| matches!(
                operation,
                macos_owned::Operation::Enabled {
                    field: Field::Http,
                    ..
                } | macos_owned::Operation::Endpoint {
                    field: Field::Http,
                    ..
                }
            )));
    }
}

#[test]
fn identical_consecutive_evidence_coalesces_and_distinct_originals_are_retained() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let original = state.macos_services.clone();
    state.generation = "same-evidence-new-generation".into();
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut fresh = macos_owned::capture(&mut os).unwrap();
    manager
        .reuse_incomplete_restore_baselines(&mut fresh)
        .unwrap();
    state.macos_services = fresh;
    retire_incomplete(&mut manager, &mut state);
    let records = manager.incomplete_restores().unwrap();
    assert_eq!(records.records.len(), 1);
    assert_eq!(
        records.records[0].generation,
        "same-evidence-new-generation"
    );
    assert_eq!(records.records[0].macos_services, original);
    state.generation = "different-original".into();
    state.macos_services[0].fields[0].before = protocol(false, "", 8443);
    retire_incomplete(&mut manager, &mut state);
    let records = manager.incomplete_restores().unwrap();
    assert_eq!(records.records.len(), 2);
    assert_eq!(records.records[0].macos_services, original);
    assert_eq!(
        records.records[1].macos_services[0].fields[0].before,
        protocol(false, "", 8443)
    );
}

#[test]
fn interrupted_retirement_preserves_both_records_and_retry_does_not_duplicate_evidence() {
    let (_dir, mut manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    std::fs::create_dir(manager.backup_file_path()).unwrap();
    assert!(manager.finish_macos_restore(&state, outcome).is_err());
    assert_eq!(manager.incomplete_restores().unwrap().records.len(), 1);
    assert!(manager.state_file_path().exists());
    std::fs::remove_dir(manager.backup_file_path()).unwrap();
    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(!outcome.changed);
    assert!(manager
        .finish_macos_restore(&state, outcome)
        .unwrap_err()
        .to_string()
        .contains("IncompleteRestore:"));
    assert!(!manager.state_file_path().exists());
    assert_eq!(manager.incomplete_restores().unwrap().records.len(), 1);
}

#[cfg(unix)]
#[test]
fn failed_atomic_archive_write_preserves_the_active_original_and_external_file() {
    let (dir, mut manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    let external = dir.path().join("external-record");
    let existing = serde_json::to_vec(&IncompleteRestores::default()).unwrap();
    std::fs::write(&external, &existing).unwrap();
    std::os::unix::fs::symlink(&external, manager.incomplete_restores_path()).unwrap();
    assert!(manager.finish_macos_restore(&state, outcome).is_err());
    assert!(manager.state_file_path().exists());
    assert_eq!(std::fs::read(external).unwrap(), existing);
    assert_eq!(
        manager.load_managed_state().unwrap().macos_services,
        state.macos_services
    );
}

#[test]
fn malformed_archive_preserves_active_journal_and_blocks_baseline_reuse() {
    let (_dir, mut manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    manager.attach_managed_state(&state);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(outcome.incomplete_baseline);
    let journal = std::fs::read(manager.state_file_path()).unwrap();
    let backup = serde_json::to_vec(&state.original).unwrap();
    std::fs::write(manager.backup_file_path(), &backup).unwrap();
    let malformed = b"{\"records\":[";
    std::fs::write(manager.incomplete_restores_path(), malformed).unwrap();
    let os_before = manager.mock_macos_state.lock().unwrap().clone().unwrap();

    let error = manager.finish_macos_restore(&state, outcome).unwrap_err();
    assert!(error
        .to_string()
        .contains("Invalid incomplete proxy restoration record"));
    assert!(manager.is_managed_generation_attached(&state.generation));
    assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), journal);
    assert_eq!(std::fs::read(manager.backup_file_path()).unwrap(), backup);
    assert_eq!(
        std::fs::read(manager.incomplete_restores_path()).unwrap(),
        malformed
    );
    let mut captured = state.macos_services.clone();
    assert!(manager
        .reuse_incomplete_restore_baselines(&mut captured)
        .is_err());
    assert_eq!(captured, state.macos_services);
    assert!(manager.relinquish_archived_fields(&mut captured).is_err());
    assert_eq!(captured, state.macos_services);
    let os_after = manager.mock_macos_state.lock().unwrap().clone().unwrap();
    assert_eq!(os_after.reads, os_before.reads);
    assert_eq!(os_after.writes, os_before.writes);
    assert_eq!(os_after.values, os_before.values);
    manager.detach_in_place();
}

#[test]
fn archived_baseline_and_retirement_do_not_cross_network_service_names() {
    let (_dir, mut manager, mut state) = fixture();
    retire_incomplete(&mut manager, &mut state);
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    let mut captured = macos_owned::capture(&mut os).unwrap();
    let mut unrelated = captured[0].clone();
    unrelated.name = "Unrelated Ethernet".into();
    captured.push(unrelated.clone());

    manager
        .reuse_incomplete_restore_baselines(&mut captured)
        .unwrap();
    assert_eq!(captured[0].fields[0].before, protocol(false, "", 0));
    assert_eq!(captured[1], unrelated);
    manager.relinquish_archived_fields(&mut captured).unwrap();
    assert!(captured[0].fields.iter().all(|field| field.relinquished));
    assert_eq!(captured[1], unrelated);
}

#[test]
fn interrupted_retirement_updates_same_generation_after_external_field_change() {
    let (_dir, mut manager, mut state) = fixture();
    transition(&manager, &mut state, Intent::Apply);
    let outcome = transition(&manager, &mut state, Intent::Restore);
    std::fs::create_dir(manager.backup_file_path()).unwrap();
    assert!(manager.finish_macos_restore(&state, outcome).is_err());
    assert_eq!(manager.incomplete_restores().unwrap().records.len(), 1);
    assert!(
        !manager.incomplete_restores().unwrap().records[0].macos_services[0].fields[0].relinquished
    );
    let manual = protocol(true, "manual-corp", 8443);
    let writes_before = manager
        .mock_macos_state
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writes
        .clone();
    {
        let mut os = manager.mock_macos_state.lock().unwrap();
        let os = os.as_mut().unwrap();
        os.values.insert(
            (MockMacosState::SERVICE.into(), Field::Http as u8),
            manual.clone(),
        );
    }
    std::fs::remove_dir(manager.backup_file_path()).unwrap();

    let outcome = transition(&manager, &mut state, Intent::Restore);
    assert!(outcome.ownership_changed);
    assert!(outcome.incomplete_baseline);
    assert!(!outcome.changed);
    assert!(manager
        .finish_macos_restore(&state, outcome)
        .unwrap_err()
        .to_string()
        .contains("IncompleteRestore:"));
    let archive = manager.incomplete_restores().unwrap();
    assert_eq!(archive.records.len(), 1);
    assert_eq!(archive.records[0].generation, state.generation);
    assert_eq!(archive.records[0].macos_services, state.macos_services);
    assert!(archive.records[0].macos_services[0].fields[0].relinquished);
    assert!(!manager.state_file_path().exists());
    assert!(manager.read_managed_ownership().unwrap().is_none());
    let mut os = manager.macos_backend(Privilege::Direct).unwrap();
    assert_eq!(
        os.read(MockMacosState::SERVICE, Field::Http).unwrap(),
        manual
    );
    assert_eq!(
        manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .writes,
        writes_before
    );
}

#[test]
fn no_write_restore_retires_ownership_and_reports_external_changes() {
    for external_change in [false, true] {
        let (_dir, mut manager, mut state) = fixture();
        manager.write_managed_state(&state).unwrap();
        manager.attach_managed_state(&state);
        std::fs::write(
            manager.backup_file_path(),
            serde_json::to_vec(&state.original).unwrap(),
        )
        .unwrap();
        if external_change {
            manager
                .mock_macos_state
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .values
                .insert(
                    (MockMacosState::SERVICE.into(), Field::Http as u8),
                    protocol(true, "manual-corp", 8443),
                );
        }
        let values_before = manager
            .mock_macos_state
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .values
            .clone();
        let outcome = transition(&manager, &mut state, Intent::Restore);
        assert!(!outcome.changed);
        assert!(!outcome.incomplete_baseline);
        assert_eq!(outcome.ownership_changed, external_change);
        assert_eq!(
            manager.finish_macos_restore(&state, outcome).unwrap(),
            if external_change {
                SystemProxyDisableOutcome::OwnedByOther
            } else {
                SystemProxyDisableOutcome::NotEnabled
            }
        );
        assert!(!manager.is_set());
        assert!(!manager.is_managed_generation_attached(&state.generation));
        assert!(manager.original_proxy.is_none());
        assert!(!manager.state_file_path().exists());
        assert!(!manager.backup_file_path().exists());
        assert!(!manager.incomplete_restores_path().exists());
        let os = manager.mock_macos_state.lock().unwrap();
        let os = os.as_ref().unwrap();
        assert!(os.writes.is_empty());
        assert_eq!(os.values, values_before);
    }
}
