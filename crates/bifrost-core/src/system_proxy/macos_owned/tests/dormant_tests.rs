use super::*;

fn applied_empty(field: Field, host: &str, port: u16) -> (ManagedProxyState, Fake) {
    let mut fake = Fake::fixture();
    fake.services.truncate(1);
    fake.values
        .insert(key("Wi-Fi", field), protocol(false, host, port));
    fake.dormant_supported = true;
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    fake.writes.clear();
    (state, fake)
}

#[test]
fn supported_suspend_resume_restore_exact_empty_and_zero_baselines_per_protocol() {
    for field in [Field::Http, Field::Https] {
        for (host, port) in [("", 0), ("", 3128), ("dormant-corp", 0)] {
            let (mut state, mut fake) = applied_empty(field, host, port);
            let generation = state.generation.clone();
            let suspended = run(&mut state, &mut fake, Intent::Suspend).unwrap();
            assert!(!suspended.incomplete_baseline);
            assert_eq!(
                fake.values[&key("Wi-Fi", field)],
                protocol(false, host, port)
            );
            run(&mut state, &mut fake, Intent::Resume).unwrap();
            assert_eq!(
                fake.values[&key("Wi-Fi", field)],
                protocol(true, "127.0.0.1", 18880)
            );
            assert!(
                !run(&mut state, &mut fake, Intent::Restore)
                    .unwrap()
                    .incomplete_baseline
            );
            assert_eq!(
                fake.values[&key("Wi-Fi", field)],
                protocol(false, host, port)
            );
            assert_eq!(state.generation, generation);
            assert!(state.macos_services[0]
                .fields
                .iter()
                .all(|f| f.pending.is_none()));
        }
    }
}

#[test]
fn crash_after_commit_and_repeated_apply_error_keep_barrier_until_confirmed_retry() {
    let (mut state, mut fake) = applied_empty(Field::Http, "", 0);
    // Disable is write 1; committing the empty endpoint then failing is write 2.
    fake.fail_write = Some((2, true));
    let mut durable = state.clone();
    assert!(transition(&mut state, &mut fake, Intent::Restore, |s| {
        durable = s.clone();
        Ok(())
    })
    .is_err());
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        protocol(false, "", 0)
    );
    assert!(
        durable.macos_services[0].fields[0]
            .pending
            .as_ref()
            .unwrap()
            .needs_apply
    );
    state = durable;
    fake.writes.clear();
    fake.fail_write = Some((1, true));
    assert!(run(&mut state, &mut fake, Intent::Restore).is_err());
    assert!(
        state.macos_services[0].fields[0]
            .pending
            .as_ref()
            .unwrap()
            .needs_apply
    );
    assert!(
        matches!(&fake.writes[0].1, Operation::DormantEndpoint { expected, desired, .. } if expected == desired)
    );
    fake.writes.clear();
    fake.fail_write = None;
    assert!(
        !run(&mut state, &mut fake, Intent::Restore)
            .unwrap()
            .incomplete_baseline
    );
    assert!(state.macos_services[0].fields[0].pending.is_none());
    assert!(
        matches!(&fake.writes[0].1, Operation::DormantEndpoint { expected, desired, .. } if expected == desired)
    );
}

#[test]
fn pending_apply_survives_unsupported_retry_and_unrelated_field_persistence() {
    let (mut state, mut fake) = applied_empty(Field::Http, "", 0);
    fake.fail_write = Some((2, true));
    assert!(run(&mut state, &mut fake, Intent::Restore).is_err());
    fake.fail_write = None;
    fake.dormant_supported = false;
    fake.writes.clear();
    let mut snapshots = Vec::new();
    assert!(transition(&mut state, &mut fake, Intent::Restore, |s| {
        snapshots.push(s.clone());
        Ok(())
    })
    .is_err());
    assert!(snapshots.iter().all(|s| s.macos_services[0].fields[0]
        .pending
        .as_ref()
        .is_some_and(|p| p.needs_apply)));
    assert!(state.macos_services[0].fields[0].pending.is_some());
    assert!(fake.writes.is_empty());
}

struct PrewriteRace {
    fake: Fake,
    reads: usize,
    change: Value,
}
impl Backend for PrewriteRace {
    fn supports_dormant_restore(&self) -> bool {
        true
    }
    fn services(&mut self) -> Result<Vec<Service>> {
        self.fake.services()
    }
    fn read(&mut self, service: &str, field: Field) -> Result<Value> {
        self.reads += 1;
        if self.reads == 2 {
            self.fake
                .values
                .insert(key(service, field), self.change.clone());
        }
        self.fake.read(service, field)
    }
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        self.fake.write(service, operation)
    }
}
#[test]
fn prewrite_race_to_committed_after_value_cannot_skip_apply() {
    let (mut state, mut fake) = applied_empty(Field::Http, "", 0);
    state.macos_services[0].fields.truncate(1);
    // Start already disabled, so the first planned operation is the SC restore.
    let disabled = protocol(false, "127.0.0.1", 18880);
    fake.values
        .insert(key("Wi-Fi", Field::Http), disabled.clone());
    state.macos_services[0].fields[0].last_written = disabled;
    let mut os = PrewriteRace {
        fake,
        reads: 0,
        change: protocol(false, "", 0),
    };
    let mut saw_barrier = false;
    transition(&mut state, &mut os, Intent::Restore, |s| {
        let pending = &s.macos_services[0].fields[0].pending;
        saw_barrier |= pending.as_ref().is_some_and(|p| p.needs_apply);
        Ok(())
    })
    .unwrap();
    assert!(saw_barrier);
    assert_eq!(os.fake.writes.len(), 1);
    assert!(
        matches!(&os.fake.writes[0].1, Operation::DormantEndpoint { expected, desired, .. } if expected == desired)
    );
    assert!(state.macos_services[0].fields[0].pending.is_none());
}

#[test]
fn manual_change_during_pending_apply_relinquishes_without_overwriting() {
    let (mut state, mut fake) = applied_empty(Field::Http, "", 0);
    fake.fail_write = Some((2, true));
    assert!(run(&mut state, &mut fake, Intent::Restore).is_err());
    let manual = protocol(true, "manual-corp", 5555);
    fake.values
        .insert(key("Wi-Fi", Field::Http), manual.clone());
    fake.fail_write = None;
    fake.writes.clear();
    assert!(
        run(&mut state, &mut fake, Intent::Restore)
            .unwrap()
            .ownership_changed
    );
    assert_eq!(fake.values[&key("Wi-Fi", Field::Http)], manual);
    assert!(state.macos_services[0].fields[0].relinquished);
    assert!(fake.writes.is_empty());
}

#[test]
fn legacy_pending_records_default_to_no_apply_barrier() {
    let pending = PendingWrite {
        before: protocol(false, "a", 1),
        possible_after: vec![protocol(true, "b", 2)],
        needs_apply: true,
    };
    let mut json = serde_json::to_value(&pending).unwrap();
    json.as_object_mut().unwrap().remove("needs_apply");
    let legacy: PendingWrite = serde_json::from_value(json).unwrap();
    assert!(!legacy.needs_apply);
}
