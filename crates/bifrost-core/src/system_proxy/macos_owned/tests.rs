use super::*;
use std::collections::BTreeMap;

#[derive(Clone)]
struct Fake {
    services: Vec<Service>,
    values: BTreeMap<(String, u8), Value>,
    writes: Vec<(String, Operation)>,
    fail_write: Option<(usize, bool)>,
    fail_read_after_write: bool,
    ignore_writes: bool,
    dormant_supported: bool,
}

fn key(service: &str, field: Field) -> (String, u8) {
    (service.into(), field as u8)
}
fn protocol(enabled: bool, host: &str, port: u16) -> Value {
    Value::Protocol(Protocol {
        enabled,
        host: host.into(),
        port,
        authenticated: false,
    })
}
impl Fake {
    fn fixture() -> Self {
        let mut fake = Self {
            services: vec![
                Service {
                    name: "Wi-Fi".into(),
                    enabled: true,
                },
                Service {
                    name: "Ethernet".into(),
                    enabled: true,
                },
            ],
            values: BTreeMap::new(),
            writes: Vec::new(),
            fail_write: None,
            fail_read_after_write: false,
            ignore_writes: false,
            dormant_supported: false,
        };
        for (service, http, https, bypass) in [
            (
                "Wi-Fi",
                protocol(true, "corp-http", 8080),
                protocol(false, "dormant-https", 4443),
                vec!["*.corp".into(), "localhost".into()],
            ),
            (
                "Ethernet",
                protocol(false, "dormant-http", 3128),
                protocol(true, "corp-https", 8443),
                vec!["10.0.0.0/8".into()],
            ),
        ] {
            fake.values.insert(key(service, Field::Http), http);
            fake.values.insert(key(service, Field::Https), https);
            fake.values
                .insert(key(service, Field::Bypass), Value::Bypass(bypass));
        }
        fake
    }
}
impl Backend for Fake {
    fn supports_dormant_restore(&self) -> bool {
        self.dormant_supported
    }
    fn services(&mut self) -> Result<Vec<Service>> {
        Ok(self.services.clone())
    }
    fn read(&mut self, service: &str, field: Field) -> Result<Value> {
        if self.fail_read_after_write && !self.writes.is_empty() {
            return Err(BifrostError::Config("networksetup read interrupted".into()));
        }
        self.values
            .get(&key(service, field))
            .cloned()
            .ok_or_else(|| BifrostError::Config("networksetup unknown service".into()))
    }
    fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
        self.writes.push((service.into(), operation.clone()));
        let fail = self
            .fail_write
            .filter(|(index, _)| *index == self.writes.len());
        if fail.is_some_and(|(_, after)| !after) {
            return Err(BifrostError::Config(
                "RequiresAdmin: networksetup permission denied".into(),
            ));
        }
        if !self.ignore_writes {
            match operation {
                Operation::Endpoint { field, host, port } => {
                    self.values
                        .insert(key(service, *field), protocol(true, host, *port));
                }
                Operation::Enabled { field, enabled } => {
                    let Value::Protocol(proxy) =
                        self.values.get_mut(&key(service, *field)).unwrap()
                    else {
                        panic!("expected protocol")
                    };
                    proxy.enabled = *enabled;
                }
                Operation::DormantEndpoint {
                    field,
                    expected,
                    desired,
                } => {
                    let key = (service.into(), *field as u8);
                    if self.values.get(&key) != Some(&Value::Protocol(expected.clone())) {
                        return Err(BifrostError::Config(
                            "ProxyOwnershipChanged: mock compare failed".into(),
                        ));
                    }
                    self.values.insert(key, Value::Protocol(desired.clone()));
                }
                Operation::Bypass(domains) => {
                    self.values
                        .insert(key(service, Field::Bypass), Value::Bypass(domains.clone()));
                }
            }
        }
        if fail.is_some() {
            return Err(BifrostError::Config(
                "networksetup command interrupted after mutation".into(),
            ));
        }
        Ok(())
    }
}
fn state(fake: &mut Fake) -> ManagedProxyState {
    ManagedProxyState {
        schema_version: 3,
        generation: "generation-one".into(),
        original: ProxyBackup {
            enable: true,
            host: "corp-http".into(),
            port: 8080,
            bypass: "*.corp".into(),
        },
        target: ProxyBackup {
            enable: true,
            host: "127.0.0.1".into(),
            port: 18880,
            bypass: "localhost,127.0.0.1".into(),
        },
        applied: false,
        phase: Some(ManagedSystemProxyPhase::PendingApply),
        authorization_suppressed: false,
        macos_services: capture(fake).unwrap(),
    }
}
fn run(state: &mut ManagedProxyState, fake: &mut Fake, intent: Intent) -> Result<TransitionResult> {
    transition(state, fake, intent, |_| Ok(()))
}

#[test]
fn heterogeneous_services_protocols_and_bypass_restore_exactly() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Applied);
    assert!(fake
        .values
        .values()
        .filter_map(|v| if let Value::Protocol(p) = v {
            Some(p)
        } else {
            None
        })
        .all(|p| p.enabled && p.port == 18880));
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, before);
}

#[test]
fn manual_https_and_bypass_changes_are_preserved_while_owned_http_restores() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    let manual = protocol(true, "new-corporate-proxy", 9001);
    let bypass = Value::Bypass(vec!["*.new-corp".into()]);
    fake.values
        .insert(key("Wi-Fi", Field::Https), manual.clone());
    fake.values
        .insert(key("Wi-Fi", Field::Bypass), bypass.clone());
    let outcome = run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert!(outcome.ownership_changed);
    assert_eq!(fake.values[&key("Wi-Fi", Field::Https)], manual);
    assert_eq!(fake.values[&key("Wi-Fi", Field::Bypass)], bypass);
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        before[&key("Wi-Fi", Field::Http)]
    );
}

#[test]
fn fail_open_resume_keeps_baseline_and_generation_and_respects_manual_disable() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    run(&mut state, &mut fake, Intent::Suspend).unwrap();
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Suspended);
    assert_eq!(fake.values, before);
    let manual = protocol(false, "manual-server", 9123);
    fake.values
        .insert(key("Ethernet", Field::Http), manual.clone());
    run(&mut state, &mut fake, Intent::Resume).unwrap();
    assert_eq!(fake.values[&key("Ethernet", Field::Http)], manual);
    assert_eq!(state.generation, "generation-one");
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        before[&key("Wi-Fi", Field::Http)]
    );
    assert_eq!(fake.values[&key("Ethernet", Field::Http)], manual);
}

#[test]
fn disabled_owned_services_are_cleaned_and_new_services_are_not_claimed_on_resume() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    fake.services[0].enabled = false;
    fake.services.push(Service {
        name: "New VPN".into(),
        enabled: true,
    });
    run(&mut state, &mut fake, Intent::Suspend).unwrap();
    assert_eq!(fake.values, before);
    run(&mut state, &mut fake, Intent::Resume).unwrap();
    assert!(fake.writes.iter().all(|(name, _)| name != "New VPN"));
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, before);
}

#[test]
fn authenticated_corporate_service_is_never_modified() {
    let mut fake = Fake::fixture();
    if let Value::Protocol(proxy) = fake.values.get_mut(&key("Wi-Fi", Field::Https)).unwrap() {
        proxy.authenticated = true;
    }
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert!(fake.writes.iter().all(|(name, _)| name != "Wi-Fi"));
    for field in [Field::Http, Field::Https, Field::Bypass] {
        assert_eq!(
            fake.values[&key("Wi-Fi", field)],
            before[&key("Wi-Fi", field)]
        );
    }
}

#[test]
fn every_partial_command_can_retry_with_privilege_without_replacing_baseline() {
    for fail_index in 1..=6 {
        for after in [false, true] {
            let mut fake = Fake::fixture();
            let before = fake.values.clone();
            let mut state = state(&mut fake);
            fake.fail_write = Some((fail_index, after));
            assert!(run(&mut state, &mut fake, Intent::Apply).is_err());
            // Serialize/deserialise as a new process/privilege retry would.
            let mut restored: ManagedProxyState =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            fake.fail_write = None;
            run(&mut restored, &mut fake, Intent::Apply).unwrap();
            assert_eq!(restored.generation, "generation-one");
            assert_eq!(restored.original.host, "corp-http");
            run(&mut restored, &mut fake, Intent::Restore).unwrap();
            assert_eq!(fake.values, before, "command {fail_index}, after={after}");
        }
    }
}

#[test]
fn interrupted_readback_leaves_write_ahead_intent_for_crash_recovery() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    fake.fail_read_after_write = true;
    assert!(run(&mut state, &mut fake, Intent::Apply).is_err());
    assert!(state
        .macos_services
        .iter()
        .flat_map(|s| &s.fields)
        .any(|f| f.pending.is_some()));
    fake.fail_read_after_write = false;
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, before);
}

#[test]
fn failed_authoritative_write_prevents_os_mutation() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    let mut saves = 0;
    let error = transition(&mut state, &mut fake, Intent::Apply, |_| {
        saves += 1;
        if saves == 3 {
            return Err(std::io::Error::other("simulated fsync failure").into());
        }
        Ok(())
    })
    .unwrap_err();
    assert!(error.to_string().contains("fsync"));
    assert!(fake.writes.is_empty());
}

#[test]
fn zero_effect_success_cannot_mark_the_journal_applied() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    fake.ignore_writes = true;
    assert!(run(&mut state, &mut fake, Intent::Apply)
        .unwrap_err()
        .to_string()
        .contains("read-back mismatch"));
    assert_eq!(state.phase(), ManagedSystemProxyPhase::PendingApply);
}

#[test]
fn restored_empty_endpoint_stays_disabled_and_can_resume() {
    let mut fake = Fake::fixture();
    fake.values
        .insert(key("Wi-Fi", Field::Http), protocol(false, "", 0));
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert!(
        run(&mut state, &mut fake, Intent::Suspend)
            .unwrap()
            .incomplete_baseline
    );
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        protocol(false, "127.0.0.1", 18880)
    );
    assert!(
        !run(&mut state, &mut fake, Intent::Resume)
            .unwrap()
            .incomplete_baseline
    );
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        protocol(true, "127.0.0.1", 18880)
    );
}

#[test]
fn removed_services_keep_recovery_pending_until_service_returns() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    let removed = fake.services.remove(0);
    assert!(run(&mut state, &mut fake, Intent::Restore).is_err());
    fake.services.push(removed);
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, before);
}

#[test]
fn pending_intent_does_not_authorize_overwriting_a_third_value() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    fake.fail_write = Some((1, true));
    assert!(run(&mut state, &mut fake, Intent::Apply).is_err());
    let external = protocol(true, "external-after-crash", 5555);
    fake.values
        .insert(key("Wi-Fi", Field::Http), external.clone());
    fake.fail_write = None;
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values[&key("Wi-Fi", Field::Http)], external);
}

#[test]
fn lost_journal_never_restores_bifrost_as_its_own_original() {
    let mut fake = Fake::fixture();
    fake.values.insert(
        key("Wi-Fi", Field::Http),
        protocol(true, "127.0.0.1", 18880),
    );
    let mut state = state(&mut fake);
    sanitize_original_targets(&mut state.macos_services, &state.target);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert!(matches!(
        fake.values[&key("Wi-Fi", Field::Http)],
        Value::Protocol(Protocol { enabled: false, .. })
    ));
}

#[test]
fn legacy_snapshot_only_cleans_matching_protocol_and_never_spreads_scalar_backup() {
    let mut fake = Fake::fixture();
    fake.values.insert(
        key("Wi-Fi", Field::Http),
        protocol(true, "127.0.0.1", 18880),
    );
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    state.macos_services = legacy_journal(&mut fake, &state.target).unwrap();
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Http)],
        protocol(false, "127.0.0.1", 18880)
    );
    for field in [Field::Https, Field::Bypass] {
        assert_eq!(
            fake.values[&key("Wi-Fi", field)],
            before[&key("Wi-Fi", field)]
        );
    }
    for field in [Field::Http, Field::Https, Field::Bypass] {
        assert_eq!(
            fake.values[&key("Ethernet", field)],
            before[&key("Ethernet", field)]
        );
    }
}

#[test]
fn adoption_verifies_owned_applied_fields_without_mutating_anything() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    let writes = fake.writes.len();
    assert!(observed_owned(&state, &mut fake).unwrap());
    assert_eq!(fake.writes.len(), writes);
    fake.values.insert(
        key("Wi-Fi", Field::Https),
        protocol(false, "127.0.0.1", 18880),
    );
    assert!(!observed_owned(&state, &mut fake).unwrap());
    assert_eq!(fake.writes.len(), writes);
}

#[test]
fn interrupted_suspend_can_resume_without_recapturing_partial_baseline() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    fake.fail_write = Some((fake.writes.len() + 1, true));
    assert!(run(&mut state, &mut fake, Intent::Suspend).is_err());
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Suspending);
    fake.fail_write = None;
    run(&mut state, &mut fake, Intent::Resume).unwrap();
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values, before);
}

#[test]
fn explicit_reenable_reclaims_manual_disable_and_preserves_still_owned_baselines() {
    let mut fake = Fake::fixture();
    let before = fake.values.clone();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    let disabled = protocol(false, "127.0.0.1", 18880);
    fake.values
        .insert(key("Wi-Fi", Field::Http), disabled.clone());
    refresh_explicit_acquisition(&mut state, &mut fake).unwrap();
    assert_ne!(state.generation, "generation-one");
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert!(has_active_owned_protocol(&state));
    run(&mut state, &mut fake, Intent::Restore).unwrap();
    assert_eq!(fake.values[&key("Wi-Fi", Field::Http)], disabled);
    assert_eq!(
        fake.values[&key("Wi-Fi", Field::Https)],
        before[&key("Wi-Fi", Field::Https)]
    );
    assert_eq!(
        fake.values[&key("Ethernet", Field::Http)],
        before[&key("Ethernet", Field::Http)]
    );
}

#[test]
fn all_authenticated_services_never_count_as_enabled_ownership() {
    let mut fake = Fake::fixture();
    for value in fake.values.values_mut() {
        if let Value::Protocol(proxy) = value {
            proxy.authenticated = true;
        }
    }
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    assert!(!has_active_owned_protocol(&state));
    assert!(fake.writes.is_empty());
}

#[test]
fn cancelled_gui_authorization_stops_all_later_commands() {
    struct Cancelled(Fake, usize);
    impl Backend for Cancelled {
        fn services(&mut self) -> Result<Vec<Service>> {
            self.0.services()
        }
        fn read(&mut self, service: &str, field: Field) -> Result<Value> {
            self.0.read(service, field)
        }
        fn write(&mut self, _service: &str, _operation: &Operation) -> Result<()> {
            self.1 += 1;
            Err(BifrostError::Config(
                "UserCancelled: User cancelled authorization".into(),
            ))
        }
    }
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    let mut cancelled = Cancelled(fake, 0);
    assert!(transition(&mut state, &mut cancelled, Intent::Apply, |_| Ok(())).is_err());
    assert_eq!(cancelled.1, 1);
    assert_eq!(state.phase(), ManagedSystemProxyPhase::PendingApply);
}

#[test]
fn persisted_gui_cancel_blocks_a_new_coordinator_until_explicit_enable() {
    struct Authorized {
        fake: Fake,
        cancel: bool,
        prompts: usize,
    }
    impl Backend for Authorized {
        fn requires_authorization(&self) -> bool {
            true
        }
        fn services(&mut self) -> Result<Vec<Service>> {
            self.fake.services()
        }
        fn read(&mut self, service: &str, field: Field) -> Result<Value> {
            self.fake.read(service, field)
        }
        fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
            self.prompts += 1;
            if self.cancel {
                return Err(BifrostError::Config(
                    "UserCancelled: User cancelled authorization".into(),
                ));
            }
            self.fake.write(service, operation)
        }
    }
    let mut fake = Fake::fixture();
    let baseline = fake.values.clone();
    let mut state = state(&mut fake);
    let mut first = Authorized {
        fake,
        cancel: true,
        prompts: 0,
    };
    let mut saved = None;
    assert!(transition(&mut state, &mut first, Intent::Apply, |state| {
        saved = Some(serde_json::to_vec(state).unwrap());
        Ok(())
    })
    .is_err());
    assert_eq!(first.prompts, 1);
    let mut restored: ManagedProxyState = serde_json::from_slice(&saved.unwrap()).unwrap();
    assert!(restored.authorization_suppressed);
    assert!(restored.target.enable);
    let mut replacement = Authorized {
        fake: first.fake,
        cancel: false,
        prompts: 0,
    };
    assert!(
        transition(&mut restored, &mut replacement, Intent::Apply, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("UserCancelled:")
    );
    assert_eq!(replacement.prompts, 0);
    assert_eq!(replacement.fake.values, baseline);
    begin_explicit_acquisition(&mut restored);
    assert_ne!(restored.generation, "generation-one");
    assert!(!restored.authorization_suppressed);
    transition(&mut restored, &mut replacement, Intent::Apply, |_| Ok(())).unwrap();
    assert!(replacement.prompts > 0);
    assert!(has_active_owned_protocol(&restored));
    transition(&mut restored, &mut replacement, Intent::Restore, |_| Ok(())).unwrap();
    assert_eq!(replacement.fake.values, baseline);
}

#[test]
fn renamed_service_is_reported_unresolved_without_claiming_the_new_name() {
    let mut fake = Fake::fixture();
    let mut state = state(&mut fake);
    run(&mut state, &mut fake, Intent::Apply).unwrap();
    fake.services[0].name = "Renamed Wi-Fi".into();
    for field in [Field::Http, Field::Https, Field::Bypass] {
        let value = fake.values.remove(&key("Wi-Fi", field)).unwrap();
        fake.values.insert(key("Renamed Wi-Fi", field), value);
    }
    let error = run(&mut state, &mut fake, Intent::Restore).unwrap_err();
    assert!(error
        .to_string()
        .contains("owned service unavailable: Wi-Fi"));
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Restoring);
    assert_eq!(state.generation, "generation-one");
    assert_eq!(state.macos_services[0].name, "Wi-Fi");
    assert_eq!(
        fake.values[&key("Renamed Wi-Fi", Field::Http)],
        protocol(true, "127.0.0.1", 18880)
    );
    assert!(fake.writes.iter().all(|(name, _)| name != "Renamed Wi-Fi"));
}

#[test]
fn new_explicit_disable_allows_approved_cleanup_after_cancelled_enable() {
    struct Elevated {
        fake: Fake,
        commands: usize,
    }
    impl Backend for Elevated {
        fn requires_authorization(&self) -> bool {
            true
        }
        fn services(&mut self) -> Result<Vec<Service>> {
            self.fake.services()
        }
        fn read(&mut self, service: &str, field: Field) -> Result<Value> {
            self.fake.read(service, field)
        }
        fn write(&mut self, service: &str, operation: &Operation) -> Result<()> {
            self.commands += 1;
            self.fake.write(service, operation)
        }
    }
    let mut fake = Fake::fixture();
    let baseline = fake.values.clone();
    let mut state = state(&mut fake);
    fake.fail_write = Some((2, false));
    assert!(run(&mut state, &mut fake, Intent::Apply).is_err());
    // A permission dialog outside the backend was cancelled for that attempt.
    state.authorization_suppressed = true;
    fake.fail_write = None;
    let mut approved = Elevated { fake, commands: 0 };
    assert!(transition(&mut state, &mut approved, Intent::Restore, |_| Ok(())).is_err());
    assert_eq!(approved.commands, 0);
    assert!(state.authorization_suppressed);
    let generation = state.generation.clone();
    begin_explicit_disable(&mut state);
    assert_eq!(state.generation, generation);
    assert_eq!(state.phase(), ManagedSystemProxyPhase::Restoring);
    assert!(!state.authorization_suppressed);
    transition(&mut state, &mut approved, Intent::Restore, |_| Ok(())).unwrap();
    assert!(approved.commands > 0);
    assert_eq!(approved.fake.values, baseline);
}

mod edge_cases;

mod dormant_tests;
