use super::*;
use bifrost_core::{ManagedSystemProxyOwnership, ProxyBackup};
use std::collections::VecDeque;
use std::path::Path;

type Hook = Box<dyn FnOnce()>;
type Transition = bifrost_core::Result<GuardedSystemProxyTransition>;

struct Step {
    name: &'static str,
    result: Transition,
    before_guard: Option<Hook>,
    after_guard: Option<Hook>,
}
impl Step {
    fn ok(name: &'static str) -> Self {
        Self::outcome(name, Ok(GuardedSystemProxyTransition::Applied))
    }
    fn outcome(name: &'static str, result: Transition) -> Self {
        Self {
            name,
            result,
            before_guard: None,
            after_guard: None,
        }
    }
    fn before(mut self, hook: Hook) -> Self {
        self.before_guard = Some(hook);
        self
    }
    fn after(mut self, hook: Hook) -> Self {
        self.after_guard = Some(hook);
        self
    }
}

struct FakeProxy {
    ownership: Option<ManagedSystemProxyOwnership>,
    steps: VecDeque<Step>,
    calls: Vec<&'static str>,
    approved: bool,
    on_prompt: Option<Hook>,
    on_ownership: Option<Hook>,
    fail_ownership: bool,
}
impl FakeProxy {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            ownership: Some(ManagedSystemProxyOwnership {
                schema_version: 3,
                generation: "accepted-generation".into(),
                original: ProxyBackup {
                    enable: false,
                    host: String::new(),
                    port: 0,
                    bypass: String::new(),
                },
                target: ProxyBackup {
                    enable: true,
                    host: "127.0.0.1".into(),
                    port: 18891,
                    bypass: "localhost".into(),
                },
                applied: true,
                phase: Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Applied),
                authorization_suppressed: false,
            }),
            steps: steps.into(),
            calls: Vec::new(),
            approved: false,
            on_prompt: None,
            on_ownership: None,
            fail_ownership: false,
        }
    }
    fn run(
        &mut self,
        name: &'static str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        self.calls.push(name);
        let step = self.steps.pop_front().expect("unexpected OS operation");
        assert_eq!(step.name, name);
        if let Some(hook) = step.before_guard {
            hook();
        }
        if !guard()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if let Some(hook) = step.after_guard {
            hook();
        }
        step.result
    }
    fn assert_drained(&self) {
        assert!(
            self.steps.is_empty(),
            "an expected OS operation did not run"
        );
    }
}
impl ExplicitProxyBackend for FakeProxy {
    fn ownership(&mut self) -> bifrost_core::Result<Option<ManagedSystemProxyOwnership>> {
        if let Some(hook) = self.on_ownership.take() {
            hook();
        }
        if self.fail_ownership {
            return Err(BifrostError::Config("ownership read failed".into()));
        }
        Ok(self.ownership.clone())
    }
    fn enable_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        assert_eq!((host, port), ("127.0.0.1", 18891));
        assert!(bypass.is_some());
        self.run("enable", guard)
    }
    fn disable_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        assert_eq!(generation, "accepted-generation");
        self.run("disable", guard)
    }
    fn suspend_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        assert_eq!(generation, "accepted-generation");
        self.run("suspend", guard)
    }
    fn suppress_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        assert_eq!(generation, "accepted-generation");
        self.run("suppress", guard)
    }
    fn confirm_authorization(&mut self, prompt: &str) -> bool {
        assert!(prompt.contains("via sudo"));
        self.calls.push("prompt");
        if let Some(hook) = self.on_prompt.take() {
            hook();
        }
        self.approved
    }
    #[cfg(target_os = "macos")]
    fn enable_privileged_guarded(
        &mut self,
        _: &str,
        _: u16,
        _: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        self.run("enable_privileged", guard)
    }
    #[cfg(target_os = "macos")]
    fn disable_privileged_guarded(
        &mut self,
        _: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> Transition {
        self.run("disable_privileged", guard)
    }
}

fn error(message: &str) -> BifrostError {
    BifrostError::Config(message.into())
}
fn set_intent(path: &Path, enabled: bool, bypass: Option<String>) {
    let manager = ConfigManager::new(path.to_path_buf()).unwrap();
    futures::executor::block_on(manager.update_system_proxy_config(SystemProxyConfigUpdate {
        enabled: Some(enabled),
        bypass,
        auto_enable: None,
        recovery_mode: None,
        recovery_grace_secs: None,
    }))
    .unwrap();
}
fn toggle(path: &Path, enabled: bool) -> Hook {
    let path = path.to_path_buf();
    Box::new(move || set_intent(&path, enabled, None))
}
fn fixture() -> (tempfile::TempDir, ConfigManager) {
    let dir = tempfile::tempdir().unwrap();
    let config = ConfigManager::new(dir.path().to_path_buf()).unwrap();
    (dir, config)
}

#[test]
fn production_direct_drivers_confirm_success_and_reject_non_applied_results() {
    for enabled in [true, false] {
        for outcome in [
            GuardedSystemProxyTransition::Applied,
            GuardedSystemProxyTransition::AlreadyInState,
            GuardedSystemProxyTransition::OwnershipChanged,
            GuardedSystemProxyTransition::NotManaged,
        ] {
            let (dir, config) = fixture();
            let mut proxy = FakeProxy::new(vec![Step::outcome(
                if enabled { "enable" } else { "disable" },
                Ok(outcome),
            )]);
            let result = if enabled {
                direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None)
            } else {
                direct_disable(&mut proxy, &config)
            };
            assert_eq!(
                result.is_ok(),
                matches!(
                    outcome,
                    GuardedSystemProxyTransition::Applied
                        | GuardedSystemProxyTransition::AlreadyInState
                )
            );
            assert_eq!(
                read_system_proxy_config(dir.path()).unwrap().enabled,
                enabled
            );
            proxy.assert_drained();
        }
    }
    let mut direct = 0;
    route_explicit_proxy_command(
        None,
        |_| panic!("no live API exists"),
        || {
            direct += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(direct, 1);
}

#[test]
fn direct_os_errors_leave_saved_requests_and_ownership_errors_do_not_mutate() {
    for enabled in [true, false] {
        let (dir, config) = fixture();
        let mut proxy = FakeProxy::new(vec![Step::outcome(
            if enabled { "enable" } else { "disable" },
            Err(error("OS write failed")),
        )]);
        let result = if enabled {
            direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None)
        } else {
            direct_disable(&mut proxy, &config)
        };
        assert!(result.unwrap_err().to_string().contains("OS write failed"));
        assert_eq!(
            read_system_proxy_config(dir.path()).unwrap().enabled,
            enabled
        );
        proxy.assert_drained();
    }
    let (_dir, config) = fixture();
    let mut proxy = FakeProxy::new(Vec::new());
    proxy.fail_ownership = true;
    assert!(direct_disable(&mut proxy, &config)
        .unwrap_err()
        .to_string()
        .contains("ownership read failed"));
    assert!(proxy.calls.is_empty());
}

#[test]
fn missing_lease_disable_is_noop_but_late_newer_enable_is_reported() {
    for newer_enable in [false, true] {
        let (dir, config) = fixture();
        let mut proxy = FakeProxy::new(Vec::new());
        proxy.ownership = None;
        if newer_enable {
            proxy.on_ownership = Some(toggle(dir.path(), true));
        }
        let result = direct_disable(&mut proxy, &config);
        assert_eq!(result.is_err(), newer_enable);
        assert_eq!(
            read_system_proxy_config(dir.path()).unwrap().enabled,
            newer_enable
        );
        assert!(proxy.calls.is_empty());
    }
}

#[test]
fn cancellation_suppresses_only_current_request_and_generation() {
    for enabled in [true, false] {
        for race in ["none", "prompt", "guard", "no_lease"] {
            let (dir, config) = fixture();
            let mut steps = vec![Step::outcome(
                if enabled { "enable" } else { "disable" },
                Err(error("RequiresAdmin")),
            )];
            if matches!(race, "none" | "guard") {
                let mut step = Step::ok("suppress");
                if race == "guard" {
                    step = step.before(toggle(dir.path(), !enabled));
                }
                steps.push(step);
            }
            let mut proxy = FakeProxy::new(steps);
            if race == "prompt" {
                proxy.on_prompt = Some(toggle(dir.path(), !enabled));
            }
            if race == "no_lease" {
                proxy.on_prompt = Some(Box::new(|| {}));
                // Disable needs its original lease before the prompt; remove it only then.
                if enabled {
                    proxy.ownership = None;
                } else {
                    proxy.on_ownership = None;
                }
            }
            // The no-lease cancellation is meaningful for enable; disable's
            // initial ownership read would otherwise short-circuit normally.
            if !enabled && race == "no_lease" {
                continue;
            }
            let result = if enabled {
                direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None)
            } else {
                direct_disable(&mut proxy, &config)
            };
            assert!(result.is_err());
            assert_eq!(
                read_system_proxy_config(dir.path()).unwrap().enabled,
                if matches!(race, "prompt" | "guard") {
                    !enabled
                } else {
                    enabled
                }
            );
            assert_eq!(
                proxy
                    .calls
                    .iter()
                    .filter(|call| **call == "suppress")
                    .count(),
                usize::from(matches!(race, "none" | "guard"))
            );
            proxy.assert_drained();
        }
    }
}

#[test]
fn approved_privilege_retry_uses_platform_path_without_silent_success() {
    for enabled in [true, false] {
        let (_dir, config) = fixture();
        let mut steps = vec![Step::outcome(
            if enabled { "enable" } else { "disable" },
            Err(error("RequiresAdmin")),
        )];
        if cfg!(target_os = "macos") {
            steps.push(Step::ok(if enabled {
                "enable_privileged"
            } else {
                "disable_privileged"
            }));
        }
        let mut proxy = FakeProxy::new(steps);
        proxy.approved = true;
        let result = if enabled {
            direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None)
        } else {
            direct_disable(&mut proxy, &config)
        };
        assert_eq!(result.is_ok(), cfg!(target_os = "macos"));
        proxy.assert_drained();
    }
}

#[test]
fn late_disable_after_enable_compensates_only_matching_owned_target() {
    for kind in [
        "owned",
        "missing",
        "other_target",
        "new_enable",
        "compensation_lost",
        "compensation_failed",
        "ownership_error",
    ] {
        let (dir, config) = fixture();
        let mut steps = vec![Step::ok("enable").after(toggle(dir.path(), kind == "new_enable"))];
        if matches!(kind, "owned" | "compensation_lost" | "compensation_failed") {
            steps.push(Step::outcome(
                "suspend",
                match kind {
                    "compensation_lost" => Ok(GuardedSystemProxyTransition::OwnershipChanged),
                    "compensation_failed" => Err(error("compensation failed")),
                    _ => Ok(GuardedSystemProxyTransition::Applied),
                },
            ));
        }
        let mut proxy = FakeProxy::new(steps);
        match kind {
            "missing" => proxy.ownership = None,
            "other_target" => proxy.ownership.as_mut().unwrap().target.port += 1,
            "ownership_error" => proxy.fail_ownership = true,
            _ => {}
        }
        assert!(direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None).is_err());
        assert_eq!(
            read_system_proxy_config(dir.path()).unwrap().enabled,
            kind == "new_enable"
        );
        proxy.assert_drained();
    }
}

#[test]
fn late_enable_after_disable_and_before_guard_never_gets_replayed() {
    for before in [true, false] {
        let (dir, config) = fixture();
        let step = if before {
            Step::ok("disable").before(toggle(dir.path(), true))
        } else {
            Step::ok("disable").after(toggle(dir.path(), true))
        };
        let mut proxy = FakeProxy::new(vec![step]);
        assert!(direct_disable(&mut proxy, &config).is_err());
        assert_eq!(proxy.calls, ["disable"]);
        assert!(read_system_proxy_config(dir.path()).unwrap().enabled);
        proxy.assert_drained();
    }
    let (dir, config) = fixture();
    let path = dir.path().join("config.toml");
    let mut proxy = FakeProxy::new(vec![Step::ok("enable").after(Box::new(move || {
        std::fs::write(path, "invalid [toml").unwrap()
    }))]);
    assert!(direct_enable(&mut proxy, &config, "127.0.0.1", 18891, None).is_err());
    assert_eq!(proxy.calls, ["enable"]);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn unsupported_real_adapter_never_acquires_or_changes_os_settings() {
    assert!(!SystemProxyManager::is_supported());
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = SystemProxyManager::new(dir.path().to_path_buf());
    assert!(ExplicitProxyBackend::ownership(&mut proxy)
        .unwrap()
        .is_none());
    assert_eq!(
        ExplicitProxyBackend::enable_guarded(&mut proxy, "127.0.0.1", 18891, None, || Ok(true))
            .unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        ExplicitProxyBackend::disable_guarded(&mut proxy, "missing", || Ok(true)).unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        ExplicitProxyBackend::suspend_guarded(&mut proxy, "missing", || Ok(true)).unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert_eq!(
        ExplicitProxyBackend::suppress_guarded(&mut proxy, "missing", || Ok(true)).unwrap(),
        GuardedSystemProxyTransition::NotManaged
    );
    assert!(!dir.path().join("proxy_state.json").exists());
}

fn http_result(
    code: u16,
    response: &str,
    enabled: bool,
    bypass: Option<&str>,
) -> (bifrost_core::Result<()>, serde_json::Value) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let response = response.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    let body = serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    write!(stream, "HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                    return body;
                }
            }
        }
    });
    let result = request_admin_proxy_change(port, enabled, bypass);
    (result, server.join().unwrap())
}

#[test]
fn real_api_success_superseded_and_unconfirmed_responses_preserve_requested_body() {
    for enabled in [false, true] {
        let (result, body) = http_result(
            200,
            &format!("{{\"configured_enabled\":{enabled}}}"),
            enabled,
            Some("new-bypass"),
        );
        result.unwrap();
        assert_eq!(body["enabled"], enabled);
        assert_eq!(body["bypass"], "new-bypass");
        let (result, body) = http_result(
            200,
            &format!("{{\"configured_enabled\":{}}}", !enabled),
            enabled,
            None,
        );
        assert!(result.unwrap_err().to_string().contains("superseded"));
        assert!(body.get("bypass").is_none());
    }
    assert!(http_result(200, "not-json", true, None)
        .0
        .unwrap_err()
        .to_string()
        .contains("unconfirmed"));
    assert!(http_result(500, "{}", true, None)
        .0
        .unwrap_err()
        .to_string()
        .contains("Acceptance could not be confirmed"));
    assert!(http_result(500, "not-json", true, None)
        .0
        .unwrap_err()
        .to_string()
        .contains("Acceptance could not be confirmed"));
    assert!(
        http_result(500, "{\"configured_enabled\":true}", true, None)
            .0
            .unwrap_err()
            .to_string()
            .contains("saved request remains pending")
    );
}
