use super::*;

#[test]
fn unavailable_services_do_not_start_a_transaction_or_replace_the_phase() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    for service in &mut fake.services {
        service.enabled = false;
    }
    assert!(capture(&mut fake)
        .unwrap_err()
        .to_string()
        .contains("No enabled"));
    fake.services.clear();
    state.set_phase(ManagedSystemProxyPhase::Applied);
    let mut saves = 0;
    assert!(transition(&mut state, &mut fake, Intent::Restore, |_| {
        saves += 1;
        Ok(())
    })
    .is_err());
    assert_eq!(saves, 0);
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Applied);
    assert!(fake.writes.is_empty());
}

#[test]
fn ownership_audit_rejects_missing_services_pending_and_non_target_snapshots() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    state.set_phase(ManagedSystemProxyPhase::Applied);
    assert!(!observed_owned(&state, &mut fake).unwrap());
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    let field = &mut state.macos_services[0].fields[0];
    field.pending = Some(PendingWrite {
        before: field.before.clone(),
        possible_after: vec![field.last_written.clone()],
        needs_apply: false,
    });
    assert!(!observed_owned(&state, &mut fake).unwrap());
    state.macos_services[0].fields[0].pending = None;
    fake.services.remove(0);
    assert!(!observed_owned(&state, &mut fake).unwrap());
    assert!(!Value::Bypass(vec!["localhost".into()]).equivalent(&protocol(false, "", 0)));
}

#[test]
fn explicit_acquisition_captures_new_services_and_missing_fields_read_only() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    state.macos_services.pop();
    state.macos_services[0]
        .fields
        .retain(|field| field.field != Field::Bypass);
    refresh_explicit_acquisition(&mut state, &mut fake).unwrap();
    assert_ne!(state.generation, "generation-one");
    assert_eq!(state.macos_services.len(), 2);
    assert_eq!(state.macos_services[0].fields.len(), 3);
    assert!(fake.writes.is_empty());
    let baseline = fake.values.clone();
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, baseline);
}

#[test]
fn newly_authenticated_service_is_relinquished_as_a_unit() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    if let Value::Protocol(proxy) = fake.values.get_mut(&key("Wi-Fi", Field::Https)).unwrap() {
        proxy.authenticated = true;
    }
    let baseline = fake.values.clone();
    refresh_explicit_acquisition(&mut state, &mut fake).unwrap();
    assert!(state.macos_services[0]
        .fields
        .iter()
        .all(|field| field.relinquished));
    assert_ne!(state.generation, "generation-one");
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert!(fake.writes.iter().all(|(name, _)| name != "Wi-Fi"));
    for field in [Field::Http, Field::Https, Field::Bypass] {
        assert_eq!(
            fake.values[&key("Wi-Fi", field)],
            baseline[&key("Wi-Fi", field)]
        );
    }
}

struct RacingBackend {
    fake: Fake,
    http_reads: usize,
    replacement: Value,
    after_write: bool,
}
impl Backend for RacingBackend {
    fn services(&mut self) -> Result<Vec<Service>> {
        self.fake.services()
    }
    fn read(&mut self, service: &str, field: Field) -> Result<Value> {
        if service == "Wi-Fi" && field == Field::Http {
            self.http_reads += 1;
            if !self.after_write && self.http_reads == 2 {
                self.fake
                    .values
                    .insert(key(service, field), self.replacement.clone());
            }
        }
        self.fake.read(service, field)
    }
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        self.fake.write(service, operation)?;
        if self.after_write
            && service == "Wi-Fi"
            && matches!(
                operation,
                Operation::Endpoint {
                    field: Field::Http,
                    ..
                }
            )
        {
            self.fake
                .values
                .insert(key(service, Field::Http), self.replacement.clone());
        }
        Ok(())
    }
}

#[test]
fn manual_change_during_journal_sync_prevents_the_stale_os_write() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    let replacement = protocol(true, "new-owner", 9111);
    let mut racing = RacingBackend {
        fake,
        http_reads: 0,
        replacement: replacement.clone(),
        after_write: false,
    };
    let result = transition(&mut state, &mut racing, Intent::Apply, |_| Ok(())).unwrap();
    assert!(result.ownership_changed);
    assert_eq!(racing.fake.values[&key("Wi-Fi", Field::Http)], replacement);
    assert!(racing
        .fake
        .writes
        .iter()
        .all(|(name, operation)| name != "Wi-Fi"
            || !matches!(
                operation,
                Operation::Endpoint {
                    field: Field::Http,
                    ..
                } | Operation::Enabled {
                    field: Field::Http,
                    ..
                }
            )));
}

#[test]
fn foreign_readback_is_relinquished_and_preserved_on_recovery() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    let replacement = protocol(true, "changed-during-command", 9222);
    let mut racing = RacingBackend {
        fake,
        http_reads: 0,
        replacement: replacement.clone(),
        after_write: true,
    };
    assert!(
        transition(&mut state, &mut racing, Intent::Apply, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("read-back mismatch")
    );
    assert!(state.macos_services[0].fields[0].relinquished);
    transition(&mut state, &mut racing, Intent::Restore, |_| Ok(())).unwrap();
    assert_eq!(racing.fake.values[&key("Wi-Fi", Field::Http)], replacement);
}

#[test]
fn direct_recovery_can_run_without_reopening_cancelled_authorization() {
    let mut fake = Fake::fixture();
    let baseline = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    state.authorization_suppressed = true;
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, baseline);
    assert!(state.authorization_suppressed);
}

#[test]
fn failed_cancel_persistence_reports_both_failures_and_stops_the_batch() {
    struct Cancelled(Fake, usize);
    impl Backend for Cancelled {
        fn services(&mut self) -> Result<Vec<Service>> {
            self.0.services()
        }
        fn read(&mut self, service: &str, field: Field) -> Result<Value> {
            self.0.read(service, field)
        }
        fn write(&mut self, _: &str, _: &Operation) -> Result<()> {
            self.1 += 1;
            Err(BifrostError::Config(
                "UserCancelled: permission request cancelled".into(),
            ))
        }
    }
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    let mut cancelled = Cancelled(fake, 0);
    let error = transition(&mut state, &mut cancelled, Intent::Apply, |state| {
        if state.authorization_suppressed {
            Err(std::io::Error::other("durable store unavailable").into())
        } else {
            Ok(())
        }
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("UserCancelled"));
    assert!(error.contains("failed to persist authorization suppression"));
    assert!(error.contains("durable store unavailable"));
    assert_eq!(cancelled.1, 1);
}
