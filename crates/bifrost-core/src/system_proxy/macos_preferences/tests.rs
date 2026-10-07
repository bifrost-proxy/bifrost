use super::*;
use serde_json::{json, Map, Value};

struct Fake {
    saved: Map<String, Value>,
    staged: Option<Map<String, Value>>,
    calls: Vec<&'static str>,
    fail: Option<&'static str>,
    fail_commit_after_effect: bool,
    locked: bool,
    services: Vec<String>,
    drift: Option<Protocol>,
}
impl Fake {
    fn new(field: Field) -> Self {
        let mut saved = json!({
            "HTTPEnable": 0, "HTTPProxy": "127.0.0.1", "HTTPPort": 18880,
            "HTTPSEnable": 0, "HTTPSProxy": "127.0.0.1", "HTTPSPort": 18880,
            "ExceptionsList": ["*.corp", "localhost"],
            "SOCKSEnable": 1, "SOCKSProxy": "corp-socks", "SOCKSPort": 1080,
            "ProxyAutoConfigEnable": 1, "ProxyAutoConfigURLString": "https://corp/pac",
            "__INACTIVE__": "preserve raw representation",
            "VendorExtension": {"opaque": [1, "two", true]}
        })
        .as_object()
        .unwrap()
        .clone();
        if field == Field::Https {
            saved.insert("HTTPEnable".into(), json!(1));
        }
        Self {
            saved,
            staged: None,
            calls: vec![],
            fail: None,
            fail_commit_after_effect: false,
            locked: false,
            services: vec!["Wi-Fi".into()],
            drift: None,
        }
    }
    fn step(&mut self, name: &'static str) -> Result<()> {
        self.calls.push(name);
        if self.fail == Some(name) {
            Err(BifrostError::Config(format!("injected {name}")))
        } else {
            Ok(())
        }
    }
}
fn protocol(enabled: bool, host: &str, port: u16) -> Protocol {
    Protocol {
        enabled,
        host: host.into(),
        port,
        authenticated: false,
    }
}
fn request(field: Field, host: &str, port: u16) -> Request {
    Request {
        service: "Wi-Fi".into(),
        field,
        expected: protocol(false, "127.0.0.1", 18880),
        desired: protocol(false, host, port),
        deadline_millis: u64::MAX,
        owner: originating_process().unwrap(),
        lock: current_lock_identity().unwrap(),
    }
}
fn prefix(field: Field) -> &'static str {
    match field {
        Field::Http => "HTTP",
        Field::Https => "HTTPS",
        Field::Bypass => panic!(),
    }
}
impl Preferences for Fake {
    type Dictionary = Map<String, Value>;
    fn lock(&mut self) -> Result<()> {
        self.step("lock")?;
        self.locked = true;
        Ok(())
    }
    fn unlock(&mut self) -> Result<()> {
        self.locked = false;
        self.step("unlock")
    }
    fn read(&mut self, service: &str) -> Result<Self::Dictionary> {
        assert!(self.locked);
        self.step("read")?;
        if self
            .services
            .iter()
            .filter(|name| name.as_str() == service)
            .count()
            != 1
        {
            return Err(BifrostError::Config("missing/duplicate service".into()));
        }
        Ok(self.saved.clone())
    }
    fn protocol(&self, d: &Self::Dictionary, field: Field) -> Result<Protocol> {
        protocol_from_properties(field, |key| {
            Ok(match d.get(key) {
                None => Property::Missing,
                Some(Value::String(value)) => Property::Text(value.clone()),
                Some(Value::Number(value)) if value.as_i64().is_some() => {
                    Property::Number(value.as_i64().unwrap())
                }
                _ => Property::Other,
            })
        })
    }
    fn replace_endpoint(
        &mut self,
        mut dictionary: Self::Dictionary,
        field: Field,
        desired: &Protocol,
    ) -> Result<()> {
        assert!(self.locked);
        self.step("set")?;
        let prefix = prefix(field);
        for (key, value) in [
            (
                format!("{prefix}Proxy"),
                (!desired.host.is_empty()).then(|| json!(desired.host)),
            ),
            (
                format!("{prefix}Port"),
                (desired.port != 0).then(|| json!(desired.port)),
            ),
        ] {
            if let Some(value) = value {
                dictionary.insert(key, value);
            } else {
                dictionary.remove(&key);
            }
        }
        self.staged = Some(dictionary);
        Ok(())
    }
    fn commit(&mut self) -> Result<()> {
        assert!(self.locked);
        if self.fail_commit_after_effect {
            self.saved = self.staged.take().unwrap();
        }
        self.step("commit")?;
        self.saved = self.staged.take().unwrap();
        Ok(())
    }
    fn apply(&mut self) -> Result<()> {
        assert!(self.locked);
        self.step("apply")
    }
    fn synchronize(&mut self) {
        self.calls.push("synchronize");
        if let Some(drift) = self.drift.take() {
            self.saved.insert("HTTPProxy".into(), json!(drift.host));
            self.saved.insert("HTTPPort".into(), json!(drift.port));
        }
    }
}

#[test]
fn clears_only_selected_dormant_endpoint_and_preserves_opaque_dictionary_values() {
    for field in [Field::Http, Field::Https] {
        for (host, port) in [("", 0), ("", 3128), ("dormant-corp", 0)] {
            let mut api = Fake::new(field);
            let before = api.saved.clone();
            let request = request(field, host, port);
            assert_eq!(restore(&mut api, &request).unwrap(), Outcome::Applied);
            assert!(!api.locked);
            assert_eq!(api.protocol(&api.saved, field).unwrap(), request.desired);
            for (key, value) in before {
                if key != format!("{}Proxy", prefix(field))
                    && key != format!("{}Port", prefix(field))
                {
                    assert_eq!(api.saved[&key], value, "{key}");
                }
            }
            assert_eq!(
                api.calls,
                [
                    "lock",
                    "read",
                    "set",
                    "commit",
                    "apply",
                    "synchronize",
                    "read",
                    "unlock"
                ]
            );
        }
    }
}

#[test]
fn manual_endpoint_or_enable_changes_under_lock_are_preserved_without_writes() {
    for (key, value) in [
        ("HTTPProxy", json!("manual")),
        ("HTTPPort", json!(4444)),
        ("HTTPEnable", json!(1)),
    ] {
        let mut api = Fake::new(Field::Http);
        api.saved.insert(key.into(), value);
        let before = api.saved.clone();
        assert_eq!(
            restore(&mut api, &request(Field::Http, "", 0)).unwrap(),
            Outcome::OwnershipChanged
        );
        assert_eq!(api.saved, before);
        assert_eq!(api.calls, ["lock", "read", "unlock"]);
    }
}

#[test]
fn malformed_types_duplicate_missing_services_and_invalid_requests_fail_closed() {
    for (key, value) in [
        ("HTTPProxy", json!(false)),
        ("HTTPPort", json!(-1)),
        ("HTTPPort", json!(65536)),
        ("HTTPEnable", json!(2)),
        ("HTTPProxyAuthenticated", json!(true)),
        ("HTTPUser", json!(["x"])),
        ("HTTPUser", json!("")),
        ("HTTPUser", json!("manual-user")),
        ("HTTPProxyAuthenticated", json!(1)),
        ("HTTPProxyPassword", json!("opaque")),
    ] {
        let mut api = Fake::new(Field::Http);
        api.saved.insert(key.into(), value);
        let before = api.saved.clone();
        assert!(restore(&mut api, &request(Field::Http, "", 0)).is_err());
        assert!(!api.locked);
        assert_eq!(api.saved, before);
        assert!(!api.calls.contains(&"set"));
    }
    for services in [
        vec![],
        vec!["Wi-Fi".into(), "Wi-Fi".into()],
        vec!["Renamed".into()],
    ] {
        let mut api = Fake::new(Field::Http);
        api.services = services;
        assert!(restore(&mut api, &request(Field::Http, "", 0)).is_err());
        assert_eq!(api.calls, ["lock", "read", "unlock"]);
    }
    for field in [Field::Http, Field::Bypass] {
        let mut api = Fake::new(Field::Http);
        let invalid = request(field, "valid-host", 8000);
        assert!(restore(&mut api, &invalid).is_err());
        assert!(api.calls.is_empty());
    }
}

#[test]
fn failures_keep_partial_commit_observable_and_retries_reapply_even_after_equal_readback() {
    for failed in ["lock", "read", "set", "commit", "apply", "unlock"] {
        let mut api = Fake::new(Field::Http);
        let before = api.saved.clone();
        api.fail = Some(failed);
        assert!(restore(&mut api, &request(Field::Http, "", 0)).is_err());
        assert!(!api.locked);
        if !["apply", "unlock"].contains(&failed) {
            assert_eq!(api.saved, before);
        }
        if failed != "lock" {
            assert_eq!(api.calls.last(), Some(&"unlock"));
        }
    }
    for failed in ["commit", "apply"] {
        let mut api = Fake::new(Field::Http);
        api.fail = Some(failed);
        api.fail_commit_after_effect = failed == "commit";
        let mut request = request(Field::Http, "", 0);
        assert!(restore(&mut api, &request).is_err());
        assert_eq!(
            api.protocol(&api.saved, Field::Http).unwrap(),
            request.desired
        );
        api.fail = None;
        api.fail_commit_after_effect = false;
        api.calls.clear();
        request.expected = request.desired.clone();
        assert_eq!(restore(&mut api, &request).unwrap(), Outcome::Applied);
        assert_eq!(
            api.calls,
            ["lock", "read", "apply", "synchronize", "read", "unlock"]
        );
    }
}

#[test]
fn readback_mismatch_and_unit_helper_dispatch_never_claim_success() {
    let mut api = Fake::new(Field::Http);
    api.drift = Some(protocol(false, "manual", 1234));
    assert!(restore(&mut api, &request(Field::Http, "", 0))
        .unwrap_err()
        .to_string()
        .contains("read-back"));
    assert!(!api.locked);
    assert!(dispatch(["ordinary-command".into()]).is_none());
    let payload = serde_json::to_string(&request(Field::Http, "", 0)).unwrap();
    assert!(dispatch([HELPER_ARGUMENT.into(), payload.clone().into()])
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("forbidden"));
    for args in [
        vec![HELPER_ARGUMENT.into()],
        vec![HELPER_ARGUMENT.into(), "{}".into()],
        vec![HELPER_ARGUMENT.into(), payload.into(), "extra".into()],
    ] {
        assert!(dispatch(args).unwrap().is_err());
    }
}

#[test]
fn helper_deadline_is_self_capped_and_origin_identity_detects_pid_reuse() {
    assert_eq!(bounded_deadline(1000, u64::MAX).unwrap(), 9000);
    assert_eq!(bounded_deadline(1000, 1050).unwrap(), 1050);
    assert!(bounded_deadline(1000, 1000).is_err());
    assert!(bounded_deadline(1000, 999).is_err());
    let owner = ProcessIdentity {
        pid: 1234,
        started_sec: 500,
        started_usec: 7,
    };
    assert!(verify_process_identity(&owner, &owner).is_ok());
    for replacement in [
        ProcessIdentity {
            pid: 1235,
            ..owner.clone()
        },
        ProcessIdentity {
            started_sec: 501,
            ..owner.clone()
        },
        ProcessIdentity {
            started_usec: 8,
            ..owner.clone()
        },
    ] {
        assert!(verify_process_identity(&owner, &replacement).is_err());
    }
    let invalid = ProcessIdentity { pid: 0, ..owner };
    assert!(verify_process_identity(&invalid, &invalid).is_err());
}

#[cfg(unix)]
#[test]
fn unrelated_non_utf8_arguments_are_untouched_and_helper_payload_is_rejected() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = std::ffi::OsString::from_vec(vec![0xff]);
    assert!(dispatch([invalid.clone()]).is_none());
    assert!(dispatch(["ordinary-command".into(), invalid.clone()]).is_none());
    assert!(dispatch([HELPER_ARGUMENT.into(), invalid])
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("not UTF-8"));
}
