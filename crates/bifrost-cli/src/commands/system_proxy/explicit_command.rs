use super::*;
use bifrost_core::{BifrostError, GuardedSystemProxyTransition, SystemProxyManager};
use bifrost_storage::NewSystemProxyConfig;

/// Private OS/authorization boundary. The command drivers below stay identical
/// for the real backend and deterministic tests; no test changes OS settings.
pub(super) trait ExplicitProxyBackend {
    fn ownership(
        &mut self,
    ) -> bifrost_core::Result<Option<bifrost_core::ManagedSystemProxyOwnership>>;
    fn enable_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn disable_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn suspend_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn suppress_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn confirm_authorization(&mut self, prompt: &str) -> bool;
    #[cfg(target_os = "macos")]
    fn enable_privileged_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    #[cfg(target_os = "macos")]
    fn disable_privileged_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
}

impl ExplicitProxyBackend for SystemProxyManager {
    fn ownership(
        &mut self,
    ) -> bifrost_core::Result<Option<bifrost_core::ManagedSystemProxyOwnership>> {
        self.read_managed_ownership()
    }
    fn enable_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        SystemProxyManager::enable_guarded(self, host, port, bypass, guard)
    }
    fn disable_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.disable_managed_explicit_if_generation_guarded(generation, guard)
    }
    fn suspend_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.suspend_managed_if_generation_guarded(generation, guard)
    }
    fn suppress_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.suppress_managed_authorization_if_generation_guarded(generation, guard)
    }
    fn confirm_authorization(&mut self, prompt: &str) -> bool {
        dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()
            .unwrap_or(false)
    }
    #[cfg(target_os = "macos")]
    fn enable_privileged_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.enable_with_privilege_guarded(host, port, bypass, guard)
    }
    #[cfg(target_os = "macos")]
    fn disable_privileged_guarded(
        &mut self,
        generation: &str,
        guard: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.disable_managed_explicit_if_generation_with_privilege_guarded(generation, guard)
    }
}

pub(super) fn route_explicit_proxy_command(
    live_port: Option<u16>,
    api: impl FnOnce(u16) -> bifrost_core::Result<()>,
    direct: impl FnOnce() -> bifrost_core::Result<()>,
) -> bifrost_core::Result<()> {
    if let Some(port) = live_port {
        // A timeout does not prove that the API rejected the request. Replaying
        // it directly could undo a newer accepted toggle after the timeout.
        api(port)
    } else {
        direct()
    }
}

pub(super) fn request_admin_proxy_change(
    port: u16,
    enabled: bool,
    bypass: Option<&str>,
) -> bifrost_core::Result<()> {
    let url = format!("http://127.0.0.1:{port}/_bifrost/api/proxy/system");
    let mut body = serde_json::json!({"enabled": enabled});
    if let Some(bypass) = bypass {
        body["bypass"] = serde_json::Value::String(bypass.into());
    }
    let response = bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .put(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
    match response {
        Ok(response) => {
            let status: serde_json::Value = response.into_json().map_err(|error|
                BifrostError::Config(format!("Bifrost API returned an unconfirmed proxy result: {error}")))?;
            if status["configured_enabled"].as_bool() != Some(enabled) {
                return Err(BifrostError::Config("Bifrost API proxy request was superseded or unconfirmed; refresh status".into()));
            }
            println!("✓ System proxy {} via running Bifrost", if enabled { "enabled" } else { "disabled" });
            Ok(())
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_json::<serde_json::Value>().ok();
            let message = body.as_ref().and_then(|body| body.get("message"))
                .and_then(|value| value.as_str()).unwrap_or("request was not confirmed");
            let state = match body.as_ref().and_then(|body| body["configured_enabled"].as_bool()) {
                Some(configured) if configured == enabled => "The saved request remains pending.",
                Some(_) => "The request was superseded or not accepted.",
                None => "Acceptance could not be confirmed.",
            };
            Err(BifrostError::Config(format!("Bifrost API proxy request failed (HTTP {code}): {message}. {state} No direct replay was attempted.")))
        }
        Err(error) => Err(BifrostError::Config(format!("Bifrost API proxy request outcome is unconfirmed: {error}. No direct replay was attempted."))),
    }
}

pub(super) fn accepted_intent_is_current(
    data_dir: &std::path::Path,
    accepted: &NewSystemProxyConfig,
) -> bifrost_core::Result<bool> {
    let latest = read_system_proxy_config(data_dir)?;
    Ok(latest.intent_revision == accepted.intent_revision
        && latest.enabled == accepted.enabled
        && latest.bypass == accepted.bypass)
}

fn require_applied(transition: GuardedSystemProxyTransition) -> bifrost_core::Result<()> {
    if matches!(
        transition,
        GuardedSystemProxyTransition::Applied | GuardedSystemProxyTransition::AlreadyInState
    ) {
        Ok(())
    } else {
        Err(BifrostError::Config(
            "System proxy request was superseded or ownership changed; left unchanged".into(),
        ))
    }
}

fn suppress_cancelled_authorization(
    manager: &mut impl ExplicitProxyBackend,
    data_dir: &std::path::Path,
    accepted: &NewSystemProxyConfig,
) -> bifrost_core::Result<()> {
    if accepted_intent_is_current(data_dir, accepted)? {
        if let Some(ownership) = manager.ownership()? {
            let outcome = manager.suppress_guarded(&ownership.generation, || {
                accepted_intent_is_current(data_dir, accepted)
            })?;
            if outcome == GuardedSystemProxyTransition::OwnershipChanged {
                return Err(BifrostError::Config(
                    "The cancelled request was superseded; newer proxy intent was left unchanged"
                        .into(),
                ));
            }
        }
    }
    Err(BifrostError::Config("Authorization was cancelled; the saved request remains pending without automatic permission prompts".into()))
}

pub(super) fn direct_enable(
    manager: &mut impl ExplicitProxyBackend,
    config: &ConfigManager,
    host: &str,
    port: u16,
    bypass: Option<String>,
) -> bifrost_core::Result<()> {
    with_accepted_system_proxy_intent(config, true, bypass, |accepted| {
        let result = manager.enable_guarded(host, port, Some(&accepted.bypass), || {
            accepted_intent_is_current(config.data_dir(), accepted)
        });
        let transition = match result {
            Err(error) if error.to_string().contains("RequiresAdmin") => {
                let approved = manager.confirm_authorization("Try enabling via sudo now?");
                if !approved {
                    return suppress_cancelled_authorization(manager, config.data_dir(), accepted);
                }
                #[cfg(target_os = "macos")]
                {
                    manager.enable_privileged_guarded(host, port, Some(&accepted.bypass), || {
                        accepted_intent_is_current(config.data_dir(), accepted)
                    })?
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err(error);
                }
            }
            result => result?,
        };
        require_applied(transition)?;
        let latest = read_system_proxy_config(config.data_dir())?;
        if latest.intent_revision != accepted.intent_revision {
            if !latest.enabled {
                if let Some(ownership) = manager
                    .ownership()?
                    .filter(|ownership| ownership.target.target_matches(host, port))
                {
                    require_applied(manager.suspend_guarded(&ownership.generation, || {
                        accepted_intent_is_current(config.data_dir(), &latest)
                    })?)?;
                }
            }
            return Err(BifrostError::Config(
                "System proxy request was superseded while applying OS settings".into(),
            ));
        }
        println!(
            "✓ System proxy enabled: {host}:{port} (bypass: {})",
            accepted.bypass
        );
        Ok(())
    })
}

pub(super) fn direct_disable(
    manager: &mut impl ExplicitProxyBackend,
    config: &ConfigManager,
) -> bifrost_core::Result<()> {
    with_accepted_system_proxy_intent(config, false, None, |accepted| {
        let Some(ownership) = manager.ownership()? else {
            if !accepted_intent_is_current(config.data_dir(), accepted)? {
                return Err(BifrostError::Config(
                    "System proxy disable was superseded".into(),
                ));
            }
            println!(
                "No managed system proxy remains; external proxy settings were left unchanged."
            );
            return Ok(());
        };
        let result = manager.disable_guarded(&ownership.generation, || {
            accepted_intent_is_current(config.data_dir(), accepted)
        });
        let transition = match result {
            Err(error) if error.to_string().contains("RequiresAdmin") => {
                let approved = manager.confirm_authorization("Try disabling via sudo now?");
                if !approved {
                    return suppress_cancelled_authorization(manager, config.data_dir(), accepted);
                }
                #[cfg(target_os = "macos")]
                {
                    manager.disable_privileged_guarded(&ownership.generation, || {
                        accepted_intent_is_current(config.data_dir(), accepted)
                    })?
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err(error);
                }
            }
            result => result?,
        };
        require_applied(transition)?;
        if !accepted_intent_is_current(config.data_dir(), accepted)? {
            return Err(BifrostError::Config(
                "System proxy disable was superseded while applying OS settings".into(),
            ));
        }
        println!("✓ Managed system proxy disabled");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn read_http_request_body(reader: impl Read) -> Vec<u8> {
        let mut reader = BufReader::new(reader);
        let mut content_length = None;
        loop {
            let mut line = String::new();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "request ended before its headers were complete"
            );
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
        }
        let mut body = vec![0; content_length.expect("request must have Content-Length")];
        reader.read_exact(&mut body).unwrap();
        body
    }

    #[test]
    fn http_fixture_consumes_fragmented_headers_and_body() {
        let body = b"{\"enabled\":false}";
        let request = format!(
            "PUT /_bifrost/api/proxy/system HTTP/1.1\r\ncOnTeNt-LeNgTh: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        // Force separate reads at every possible header/body split, including
        // the boundary where ureq writes the request prelude before its body.
        for split in 1..request.len() {
            let (first, rest) = request.as_bytes().split_at(split);
            assert_eq!(
                read_http_request_body(first.chain(rest)),
                body,
                "request split at byte {split}"
            );
        }
    }

    #[test]
    fn accepted_http_failure_after_newer_toggle_never_runs_direct_fallback() {
        for enabled in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let manager = ConfigManager::new(dir.path().to_path_buf()).unwrap();
            persist_system_proxy_config(&manager, enabled, None).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let data_dir = dir.path().to_path_buf();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                // Drain the entire PUT before replying. A single TCP read can
                // leave the body unread and closing can reset the response.
                let body = read_http_request_body(&mut stream);
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["enabled"], enabled);
                let config = ConfigManager::new(data_dir).unwrap();
                persist_system_proxy_config(&config, !enabled, None).unwrap();
                let body = serde_json::json!({"configured_enabled": !enabled, "message": "superseded while verifying"}).to_string();
                write!(stream, "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let mut fallbacks = 0;
            let result = route_explicit_proxy_command(
                Some(port),
                |port| request_admin_proxy_change(port, enabled, None),
                || {
                    fallbacks += 1;
                    Ok(())
                },
            );
            server.join().unwrap();
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("superseded"),
                "unexpected API error: {error}"
            );
            assert_eq!(fallbacks, 0);
            assert_eq!(
                read_system_proxy_config(dir.path()).unwrap().enabled,
                !enabled
            );
        }
    }

    #[test]
    fn live_api_connection_failure_is_unconfirmed_without_direct_fallback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut fallbacks = 0;
        let result = route_explicit_proxy_command(
            Some(port),
            |port| request_admin_proxy_change(port, true, None),
            || {
                fallbacks += 1;
                Ok(())
            },
        );
        assert!(result.unwrap_err().to_string().contains("unconfirmed"));
        assert_eq!(fallbacks, 0);
    }
}

#[cfg(test)]
mod driver_tests;
