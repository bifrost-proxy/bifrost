//! Read-only checks for an already attached lease. Verification must not
//! migrate journals, refresh the rollback baseline or reattach the manager.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedSystemProxyVerification {
    Verified,
    /// The generation still exists, but its owned fields need reconciliation.
    Drifted,
    OwnershipChanged,
    NotManaged,
}

impl SystemProxyManager {
    pub(super) fn attach_managed_state(&mut self, state: &ManagedProxyState) {
        self.original_proxy = Some(state.original.clone().into());
        self.attach_managed_generation(&state.generation);
    }

    pub(super) fn attach_managed_generation(&mut self, generation: &str) {
        self.attached_generation = Some(generation.into());
        self.is_set = true;
    }

    pub fn is_managed_generation_attached(&self, generation: &str) -> bool {
        self.is_set
            && !generation.is_empty()
            && self.attached_generation.as_deref() == Some(generation)
    }

    /// Verify every owned field against the current OS state without writes.
    /// A matching aggregate proxy alone cannot prove macOS service ownership.
    pub fn verify_managed_if_generation(
        &self,
        generation: &str,
    ) -> Result<ManagedSystemProxyVerification> {
        if !self.management_available() {
            return Ok(ManagedSystemProxyVerification::NotManaged);
        }
        #[cfg(target_os = "macos")]
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "verify_owned")?;
        let state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ManagedSystemProxyVerification::NotManaged);
            }
            Err(error) => return Err(error),
        };
        if generation.is_empty()
            || state.generation != generation
            || state.phase() != ManagedSystemProxyPhase::Applied
        {
            return Ok(ManagedSystemProxyVerification::OwnershipChanged);
        }
        #[cfg(target_os = "macos")]
        let matches = macos_owned::observed_owned(
            &state,
            &mut self.macos_backend(macos_command::Privilege::Direct)?,
        )?;
        #[cfg(all(not(target_os = "macos"), test))]
        let matches = if self.skip_os_proxy_io {
            macos_owned::observed_owned(
                &state,
                &mut self.macos_backend(macos_command::Privilege::Direct)?,
            )?
        } else {
            self.current_matches_backup(&state.target)?
        };
        #[cfg(all(not(target_os = "macos"), not(test)))]
        let matches = self.current_matches_backup(&state.target)?;
        Ok(if matches {
            ManagedSystemProxyVerification::Verified
        } else {
            ManagedSystemProxyVerification::Drifted
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::macos_backend::{mock_services_for_state, MockMacosState};
    use super::super::macos_owned::{Field, Value};
    use super::*;

    fn fixture() -> (tempfile::TempDir, SystemProxyManager, ManagedProxyState) {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = SystemProxyManager::new(dir.path().to_owned());
        manager.skip_os_proxy_io = true;
        let mut state = ManagedProxyState {
            schema_version: 3,
            generation: "verified-lease".into(),
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
        state.macos_services = mock_services_for_state(&state);
        manager.write_managed_state(&state).unwrap();
        (dir, manager, state)
    }

    #[test]
    fn verification_without_a_journal_is_not_managed() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SystemProxyManager::new(dir.path().to_owned());
        assert_eq!(
            manager.verify_managed_if_generation("missing").unwrap(),
            ManagedSystemProxyVerification::NotManaged
        );
        assert!(!manager.is_set());
    }

    #[test]
    fn verification_does_not_attach_a_new_manager_or_accept_an_empty_generation() {
        let (_dir, manager, state) = fixture();
        assert_eq!(
            manager.verify_managed_if_generation("").unwrap(),
            ManagedSystemProxyVerification::OwnershipChanged
        );
        assert_eq!(
            manager
                .verify_managed_if_generation(&state.generation)
                .unwrap(),
            ManagedSystemProxyVerification::Verified
        );
        assert!(!manager.is_managed_generation_attached(&state.generation));
        assert!(manager.original_proxy.is_none());
    }

    #[test]
    fn repeated_attached_verification_reads_owned_fields_without_writes_or_reattachment() {
        let (_dir, mut manager, state) = fixture();
        assert!(!manager.is_managed_generation_attached(&state.generation));
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap();
        assert!(manager.is_managed_generation_attached(&state.generation));
        let original_ptr = manager.original_proxy.as_ref().unwrap().host.as_ptr();
        let journal = std::fs::read(manager.state_file_path()).unwrap();
        for _ in 0..3 {
            assert_eq!(
                manager
                    .verify_managed_if_generation(&state.generation)
                    .unwrap(),
                ManagedSystemProxyVerification::Verified
            );
            assert_eq!(
                manager.original_proxy.as_ref().unwrap().host.as_ptr(),
                original_ptr
            );
            assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), journal);
        }
        let os = manager.mock_macos_state.lock().unwrap();
        let os = os.as_ref().unwrap();
        assert!(os.reads >= 9);
        assert!(os.writes.is_empty());
    }

    #[test]
    fn verification_is_observational_and_rejects_changed_fields_generations_and_phases() {
        for changed in [
            "http",
            "https",
            "bypass",
            "removed_service",
            "read_error",
            "generation",
            "phase",
            "missing",
            "corrupt",
        ] {
            let (_dir, manager, mut state) = fixture();
            let _backend = manager
                .macos_backend(macos_command::Privilege::Direct)
                .unwrap();
            match changed {
                "generation" => {
                    state.generation = "replacement-lease".into();
                    manager.write_managed_state(&state).unwrap();
                }
                "phase" => {
                    state.set_phase(ManagedSystemProxyPhase::PendingApply);
                    manager.write_managed_state(&state).unwrap();
                }
                "missing" => std::fs::remove_file(manager.state_file_path()).unwrap(),
                "corrupt" => std::fs::write(manager.state_file_path(), "{broken").unwrap(),
                field => {
                    let mut os = manager.mock_macos_state.lock().unwrap();
                    let os = os.as_mut().unwrap();
                    if field == "removed_service" {
                        os.services.clear();
                    } else if field == "read_error" {
                        os.values
                            .remove(&(MockMacosState::SERVICE.into(), Field::Https as u8));
                    } else if field == "bypass" {
                        os.values.insert(
                            (MockMacosState::SERVICE.into(), Field::Bypass as u8),
                            Value::Bypass(vec!["external".into()]),
                        );
                    } else {
                        let field = if field == "http" {
                            Field::Http
                        } else {
                            Field::Https
                        };
                        let Value::Protocol(proxy) = os
                            .values
                            .get_mut(&(MockMacosState::SERVICE.into(), field as u8))
                            .unwrap()
                        else {
                            panic!("expected protocol")
                        };
                        proxy.port += 1;
                    }
                }
            }
            let before = std::fs::read(manager.state_file_path()).ok();
            let outcome = manager.verify_managed_if_generation("verified-lease");
            match changed {
                "missing" => {
                    assert_eq!(outcome.unwrap(), ManagedSystemProxyVerification::NotManaged)
                }
                "corrupt" | "read_error" => assert!(outcome.is_err()),
                "generation" | "phase" => assert_eq!(
                    outcome.unwrap(),
                    ManagedSystemProxyVerification::OwnershipChanged,
                    "{changed}"
                ),
                _ => assert_eq!(
                    outcome.unwrap(),
                    ManagedSystemProxyVerification::Drifted,
                    "{changed}"
                ),
            }
            assert_eq!(std::fs::read(manager.state_file_path()).ok(), before);
            assert!(!manager.is_set());
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

    fn edit_fake_fields(manager: &SystemProxyManager, edited: &[Field]) {
        let _backend = manager
            .macos_backend(macos_command::Privilege::Direct)
            .unwrap();
        let mut os = manager.mock_macos_state.lock().unwrap();
        let os = os.as_mut().unwrap();
        for field in edited {
            let value = match field {
                Field::Bypass => Value::Bypass(vec!["*.manual".into()]),
                _ => Value::Protocol(macos_owned::Protocol {
                    enabled: true,
                    host: "manual-proxy".into(),
                    port: 8445,
                    authenticated: false,
                }),
            };
            os.values
                .insert((MockMacosState::SERVICE.into(), *field as u8), value);
        }
    }

    #[test]
    fn same_generation_manual_edits_relinquish_only_changed_fields_and_preserve_recovery() {
        use macos_owned::{Intent, Operation};
        for edited in [
            vec![Field::Https],
            vec![Field::Bypass],
            vec![Field::Https, Field::Bypass],
        ] {
            let (_dir, manager, mut state) = fixture();
            edit_fake_fields(&manager, &edited);
            assert_eq!(
                manager
                    .verify_managed_if_generation(&state.generation)
                    .unwrap(),
                ManagedSystemProxyVerification::Drifted
            );
            let manual_values = manager
                .mock_macos_state
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .values
                .clone();
            let mut os = manager
                .macos_backend(macos_command::Privilege::Direct)
                .unwrap();
            macos_owned::transition(&mut state, &mut os, Intent::Apply, |state| {
                manager.write_managed_state(state)
            })
            .unwrap();
            assert!(macos_owned::has_active_owned_protocol(&state));
            assert_eq!(state.generation, "verified-lease");
            for field in &state.macos_services[0].fields {
                assert_eq!(field.relinquished, edited.contains(&field.field));
            }
            assert!(manager
                .mock_macos_state
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .writes
                .is_empty());
            assert_eq!(
                manager
                    .verify_managed_if_generation(&state.generation)
                    .unwrap(),
                ManagedSystemProxyVerification::Verified
            );
            for intent in [Intent::Suspend, Intent::Resume] {
                macos_owned::transition(&mut state, &mut os, intent, |state| {
                    manager.write_managed_state(state)
                })
                .unwrap();
            }
            let os = manager.mock_macos_state.lock().unwrap();
            let os = os.as_ref().unwrap();
            for field in &edited {
                let key = (MockMacosState::SERVICE.into(), *field as u8);
                assert_eq!(os.values[&key], manual_values[&key]);
            }
            assert!(os.writes.iter().all(|(_, operation)| {
                let field = match operation {
                    Operation::Endpoint { field, .. }
                    | Operation::Enabled { field, .. }
                    | Operation::DormantEndpoint { field, .. } => *field,
                    Operation::Bypass(_) => Field::Bypass,
                };
                !edited.contains(&field)
            }));
            assert!(
                matches!(&os.values[&(MockMacosState::SERVICE.into(), Field::Http as u8)], Value::Protocol(proxy) if proxy.enabled && proxy.port == state.target.port)
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_manager_reconciles_manual_edits_then_recovers_remaining_owned_fields() {
        let (_dir, mut manager, state) = fixture();
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap();
        edit_fake_fields(&manager, &[Field::Https, Field::Bypass]);
        assert_eq!(
            manager
                .verify_managed_if_generation(&state.generation)
                .unwrap(),
            ManagedSystemProxyVerification::Drifted
        );
        assert_eq!(
            manager
                .reconcile_managed_if_generation(&state.generation)
                .unwrap(),
            GuardedSystemProxyTransition::Applied
        );
        assert!(manager.is_managed_generation_attached(&state.generation));
        assert_eq!(
            manager
                .verify_managed_if_generation(&state.generation)
                .unwrap(),
            ManagedSystemProxyVerification::Verified
        );
        manager
            .suspend_managed_if_generation(&state.generation)
            .unwrap();
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap();
        assert!(manager.is_managed_generation_attached(&state.generation));
        let os = manager.mock_macos_state.lock().unwrap();
        let os = os.as_ref().unwrap();
        assert!(os.writes.iter().all(|(_, operation)| matches!(
            operation,
            macos_owned::Operation::Endpoint {
                field: Field::Http,
                ..
            } | macos_owned::Operation::Enabled {
                field: Field::Http,
                ..
            }
        )));
        assert!(
            matches!(&os.values[&(MockMacosState::SERVICE.into(), Field::Https as u8)], Value::Protocol(proxy) if proxy.host == "manual-proxy" && proxy.port == 8445)
        );
        assert_eq!(
            os.values[&(MockMacosState::SERVICE.into(), Field::Bypass as u8)],
            Value::Bypass(vec!["*.manual".into()])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn failed_legacy_adoption_never_reenables_disabled_matching_endpoints() {
        for (mixed, migrated) in [(false, false), (true, false), (false, true), (true, true)] {
            let (_dir, mut manager, mut state) = fixture();
            state.schema_version = 1;
            state.phase = None;
            state.macos_services.clear();
            manager.write_managed_state(&state).unwrap();
            let _backend = manager
                .macos_backend(macos_command::Privilege::Direct)
                .unwrap();
            {
                let mut os = manager.mock_macos_state.lock().unwrap();
                let os = os.as_mut().unwrap();
                for field in [Field::Http, Field::Https] {
                    if mixed && field == Field::Http {
                        continue;
                    }
                    let Value::Protocol(proxy) = os
                        .values
                        .get_mut(&(MockMacosState::SERVICE.into(), field as u8))
                        .unwrap()
                    else {
                        panic!("expected protocol")
                    };
                    proxy.enabled = false;
                }
            }
            if migrated {
                // Simulate a migration persisted by an older resume/retarget
                // before its aggregate ownership check rejected adoption.
                state.macos_services = macos_owned::legacy_journal(
                    &mut manager
                        .macos_backend(macos_command::Privilege::Direct)
                        .unwrap(),
                    &state.target,
                )
                .unwrap();
                state.schema_version = 3;
                manager.write_managed_state(&state).unwrap();
            }
            let before = std::fs::read(manager.state_file_path()).unwrap();
            let values = manager
                .mock_macos_state
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .values
                .clone();
            for _ in 0..3 {
                assert_eq!(
                    manager
                        .reconcile_managed_if_generation(&state.generation)
                        .unwrap(),
                    GuardedSystemProxyTransition::OwnershipChanged
                );
                assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
            }
            let os = manager.mock_macos_state.lock().unwrap();
            let os = os.as_ref().unwrap();
            assert_eq!(os.values, values);
            assert!(os.writes.is_empty());
            assert!(!manager.is_managed_generation_attached(&state.generation));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fully_relinquished_protocols_do_not_repeat_journal_transitions() {
        let (_dir, mut manager, state) = fixture();
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap();
        edit_fake_fields(&manager, &[Field::Http, Field::Https]);
        assert_eq!(
            manager
                .reconcile_managed_if_generation(&state.generation)
                .unwrap(),
            GuardedSystemProxyTransition::OwnershipChanged
        );
        assert!(!manager.is_managed_generation_attached(&state.generation));
        let before = std::fs::read(manager.state_file_path()).unwrap();
        let modified = std::fs::metadata(manager.state_file_path())
            .unwrap()
            .modified()
            .unwrap();
        for _ in 0..3 {
            assert_eq!(
                manager
                    .reconcile_managed_if_generation(&state.generation)
                    .unwrap(),
                GuardedSystemProxyTransition::OwnershipChanged
            );
            assert_eq!(std::fs::read(manager.state_file_path()).unwrap(), before);
            assert_eq!(
                std::fs::metadata(manager.state_file_path())
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
        }
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
    fn attaching_generation_preserves_an_already_selected_live_baseline() {
        let (_dir, mut manager, state) = fixture();
        let live = ProxyBackup {
            host: "new-corporate-proxy".into(),
            port: 8444,
            ..state.original.clone()
        };
        manager.original_proxy = Some(live.clone().into());
        manager.attach_managed_generation(&state.generation);
        assert!(manager.is_managed_generation_attached(&state.generation));
        assert_eq!(
            ProxyBackup::from(manager.original_proxy.as_ref().unwrap()),
            live
        );
        manager.detach_in_place();
    }

    #[test]
    fn attachment_tracks_adoption_retarget_resume_and_detach() {
        let (_dir, mut manager, state) = fixture();
        manager
            .resume_managed_if_generation(&state.generation)
            .unwrap();
        assert!(manager.is_managed_generation_attached(&state.generation));
        assert!(!manager.is_managed_generation_attached(""));
        manager
            .retarget_managed_if_generation(&state.generation, "127.0.0.1", 18889, None)
            .unwrap();
        let retargeted = manager.read_managed_ownership().unwrap().unwrap();
        assert!(!manager.is_managed_generation_attached(&state.generation));
        assert!(manager.is_managed_generation_attached(&retargeted.generation));
        manager
            .suspend_managed_if_generation(&retargeted.generation)
            .unwrap();
        assert!(!manager.is_managed_generation_attached(&retargeted.generation));
        manager
            .retarget_managed_if_generation(&retargeted.generation, "127.0.0.1", 18890, None)
            .unwrap();
        let suspended_target = manager.read_managed_ownership().unwrap().unwrap();
        assert_ne!(suspended_target.generation, retargeted.generation);
        assert!(!manager.is_managed_generation_attached(&suspended_target.generation));
        manager
            .resume_managed_if_generation(&suspended_target.generation)
            .unwrap();
        assert!(manager.is_managed_generation_attached(&suspended_target.generation));
        manager.detach_in_place();
        assert!(!manager.is_managed_generation_attached(&suspended_target.generation));
    }
}
