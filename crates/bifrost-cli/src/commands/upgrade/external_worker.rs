use bifrost_core::{start_times_match, BifrostError, StartTimeMatch, EXTERNAL_CLI_WORKER_ENV};
use colored::Colorize;
use std::time::Duration;

use crate::config::get_bifrost_dir;
use crate::process::{discover_bifrost_runtime, read_runtime_info, RuntimeInfo, RuntimeStartMode};

const ADMIN_UPGRADE_REQUEST_TIMEOUT_SECS: u64 = 45;
const DESKTOP_UPGRADE_ORIGIN_HEADER: &str = "x-bifrost-desktop-upgrade-origin";

type AdminUpgradeDelegationOutcome = &'static str;
const SCHEDULED: AdminUpgradeDelegationOutcome =
    "✓ Upgrade scheduled by the running Bifrost service; it will restart automatically.";
const ALREADY_IN_PROGRESS: AdminUpgradeDelegationOutcome =
    "✓ An upgrade is already in progress in the running Bifrost service.";
const ALREADY_CURRENT: AdminUpgradeDelegationOutcome = "✓ Bifrost is already up to date.";

pub(super) fn is_external_cli_worker() -> bool {
    super::env_flag(EXTERNAL_CLI_WORKER_ENV)
}

/// Delegate out of the daemon-owned worker process tree before replacing Bifrost.
pub(super) fn delegate_upgrade() -> Result<(), BifrostError> {
    let runtime = read_runtime_info().and_then(|runtime| {
        let discovered = discover_bifrost_runtime(runtime.port)?;
        runtime_identity_matches(&runtime, &discovered).then_some(runtime)
    });
    let outcome = delegate_upgrade_with(runtime, request_admin_upgrade)?;
    println!("{}", outcome.bright_green().bold());
    Ok(())
}

fn runtime_identity_matches(recorded: &RuntimeInfo, discovered: &RuntimeInfo) -> bool {
    let start_time = start_times_match(recorded.started_at_ms, discovered.started_at_ms);
    recorded.pid == discovered.pid
        && recorded.port == discovered.port
        && matches!(start_time, StartTimeMatch::Match | StartTimeMatch::Unknown)
}

fn delegate_upgrade_with(
    runtime: Option<RuntimeInfo>,
    request: impl FnOnce(&RuntimeInfo) -> Result<(u16, String), String>,
) -> Result<AdminUpgradeDelegationOutcome, BifrostError> {
    let runtime = runtime.ok_or_else(|| {
        BifrostError::Config(
            "Cannot safely schedule an upgrade from an external CLI worker because the owning Bifrost service is not reachable. The running service was left unchanged."
                .to_string(),
        )
    })?;
    let (status, body) = request(&runtime).map_err(|error| {
        BifrostError::Config(format!(
            "Failed to schedule the upgrade through the running Bifrost service: {error}. The running service was left unchanged."
        ))
    })?;
    classify_admin_upgrade_response(status, &body).map_err(BifrostError::Config)
}

fn request_admin_upgrade(runtime: &RuntimeInfo) -> Result<(u16, String), String> {
    let (channel, desktop_origin_token) = if runtime.start_mode == RuntimeStartMode::Desktop {
        let data_dir = get_bifrost_dir()
            .map_err(|error| format!("failed to resolve the Bifrost data directory: {error}"))?;
        let token = bifrost_core::upgrade_progress::issue_desktop_upgrade_origin_token(&data_dir)
            .map_err(|error| {
            format!("failed to authorize the desktop upgrade handoff: {error}")
        })?;
        ("desktop", Some(token))
    } else {
        ("cli", None)
    };
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/system/upgrade?channel={channel}",
        runtime.port
    );
    let agent = bifrost_core::direct_ureq_agent_builder()
        .timeout(Duration::from_secs(ADMIN_UPGRADE_REQUEST_TIMEOUT_SECS))
        .build();
    let mut request = agent.post(&url);
    if let Some(token) = desktop_origin_token.as_deref() {
        request = request.set(DESKTOP_UPGRADE_ORIGIN_HEADER, token);
    }
    let response = match request.call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(format!("POST {url} failed: {error}")),
    };
    Ok((
        response.status(),
        response.into_string().unwrap_or_default(),
    ))
}

fn classify_admin_upgrade_response(
    status: u16,
    body: &str,
) -> Result<AdminUpgradeDelegationOutcome, String> {
    if status == 202 {
        return Ok(SCHEDULED);
    }

    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(|message| message.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_string());
    if status == 409 && message == "An upgrade is already in progress" {
        return Ok(ALREADY_IN_PROGRESS);
    }
    if status == 409 && message == "No update available" {
        return Ok(ALREADY_CURRENT);
    }

    let detail = if message.is_empty() {
        "empty response".to_string()
    } else {
        message
    };
    Err(format!(
        "Bifrost Admin rejected the delegated upgrade with HTTP {status}: {detail}. The running service was left unchanged"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{write_runtime_info, RuntimeStartMode};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn delegation_classifies_admin_responses() {
        assert_eq!(
            classify_admin_upgrade_response(202, r#"{"phase":"checking"}"#).unwrap(),
            SCHEDULED
        );
        assert_eq!(
            classify_admin_upgrade_response(
                409,
                r#"{"error":"An upgrade is already in progress","status":409}"#,
            )
            .unwrap(),
            ALREADY_IN_PROGRESS
        );
        assert_eq!(
            classify_admin_upgrade_response(
                409,
                r#"{"error":"No update available","status":409}"#,
            )
            .unwrap(),
            ALREADY_CURRENT
        );

        let error = classify_admin_upgrade_response(500, r#"{"error":"spawn failed"}"#)
            .expect_err("unexpected Admin failure must not be treated as success");
        assert!(error.contains("HTTP 500: spawn failed"));
        assert!(error.contains("left unchanged"));

        let unavailable = classify_admin_upgrade_response(
            503,
            r#"{"error":"Unable to determine the latest Bifrost version","status":503}"#,
        )
        .expect_err("an unavailable version lookup must not be reported as already current");
        assert!(unavailable.contains("HTTP 503"));
        assert!(unavailable.contains("Unable to determine"));

        let empty = classify_admin_upgrade_response(500, "")
            .expect_err("an empty Admin failure must remain a failure");
        assert!(empty.contains("empty response"));
    }

    #[test]
    fn delegation_is_fail_closed() {
        let missing_runtime = delegate_upgrade_with(None, |_| {
            panic!("Admin request must not run without a live runtime")
        })
        .expect_err("missing owner must fail without inline upgrade");
        assert!(missing_runtime.to_string().contains("left unchanged"));

        let runtime = RuntimeInfo::new(
            std::process::id(),
            19876,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );
        let unreachable = delegate_upgrade_with(Some(runtime.clone()), |runtime| {
            assert_eq!(runtime.port, 19876);
            Err("connection refused".to_string())
        })
        .expect_err("unreachable Admin must not fall back to inline upgrade");
        assert!(unreachable.to_string().contains("connection refused"));
        assert!(unreachable.to_string().contains("left unchanged"));

        let scheduled = delegate_upgrade_with(Some(runtime), |runtime| {
            assert_eq!(runtime.port, 19876);
            Ok((202, r#"{"phase":"checking"}"#.to_string()))
        })
        .expect("Admin accepts delegated upgrade");
        assert_eq!(scheduled, SCHEDULED);
    }

    #[test]
    fn runtime_identity_rejects_pid_reuse() {
        let mut recorded = RuntimeInfo::new(
            12345,
            19876,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );
        recorded.started_at_ms = Some(10_000);
        let mut discovered = recorded.clone();
        discovered.started_at_ms = Some(11_500);
        assert!(runtime_identity_matches(&recorded, &discovered));

        discovered.started_at_ms = Some(13_000);
        assert!(!runtime_identity_matches(&recorded, &discovered));
        discovered.started_at_ms = None;
        discovered.pid += 1;
        assert!(!runtime_identity_matches(&recorded, &discovered));
    }

    #[test]
    fn marker_accepts_documented_truthy_values() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(EXTERNAL_CLI_WORKER_ENV);

        for value in ["1", "true", "yes"] {
            std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value);
            assert!(is_external_cli_worker(), "value={value}");
        }
        for value in ["0", "false", "no", ""] {
            std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value);
            assert!(!is_external_cli_worker(), "value={value}");
        }
        match previous {
            Some(value) => std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value),
            None => std::env::remove_var(EXTERNAL_CLI_WORKER_ENV),
        }
    }

    #[test]
    fn handle_upgrade_uses_live_admin_without_inline_fallback() {
        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();

        fn serve_request(listener: &TcpListener, status: &str, body: &str) -> String {
            let (mut stream, _) = listener.accept().expect("accept Admin request");
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).expect("read Admin request");
            assert!(count > 0);
            let request = String::from_utf8_lossy(&request[..count]).into_owned();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write Admin response");
            request
        }

        fn run_case(
            start_mode: RuntimeStartMode,
            status: &'static str,
            body: &'static str,
        ) -> Result<(), BifrostError> {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Admin");
            let port = listener.local_addr().expect("mock Admin address").port();
            let mut runtime = RuntimeInfo::new(
                std::process::id(),
                port,
                None,
                Some("127.0.0.1".to_string()),
                start_mode,
            );
            runtime.started_at_ms = None;
            write_runtime_info(&runtime).expect("write runtime");
            let server = std::thread::spawn(move || {
                let overview = format!(
                    r#"{{"server":{{"port":{port}}},"system":{{"pid":{},"uptime_secs":1,"version":"test"}}}}"#,
                    std::process::id()
                );
                serve_request(&listener, "200 OK", &overview);
                let upgrade_request = serve_request(&listener, status, body);
                let request_lower = upgrade_request.to_ascii_lowercase();
                match start_mode {
                    RuntimeStartMode::Desktop => {
                        assert!(request_lower
                            .starts_with("post /_bifrost/api/system/upgrade?channel=desktop "));
                        assert!(request_lower.contains("x-bifrost-desktop-upgrade-origin:"));
                    }
                    _ => {
                        assert!(request_lower
                            .starts_with("post /_bifrost/api/system/upgrade?channel=cli "));
                        assert!(!request_lower.contains("x-bifrost-desktop-upgrade-origin:"));
                    }
                }
            });
            let result = super::super::handle_upgrade(true);
            server.join().expect("join mock Admin");
            result
        }

        let data_dir = tempfile::tempdir().expect("temp data dir");
        let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
        let previous_marker = std::env::var_os(EXTERNAL_CLI_WORKER_ENV);
        std::env::set_var("BIFROST_DATA_DIR", data_dir.path());
        std::env::set_var(EXTERNAL_CLI_WORKER_ENV, "1");

        run_case(
            RuntimeStartMode::Daemon,
            "202 Accepted",
            r#"{"phase":"checking"}"#,
        )
        .expect("scheduled upgrade");
        run_case(
            RuntimeStartMode::Desktop,
            "202 Accepted",
            r#"{"phase":"checking"}"#,
        )
        .expect("desktop handoff scheduled upgrade");
        run_case(
            RuntimeStartMode::Daemon,
            "409 Conflict",
            r#"{"error":"An upgrade is already in progress","status":409}"#,
        )
        .expect("already running is idempotent success");
        run_case(
            RuntimeStartMode::Daemon,
            "409 Conflict",
            r#"{"error":"No update available","status":409}"#,
        )
        .expect("already current is idempotent success");
        let rejected = run_case(
            RuntimeStartMode::Daemon,
            "500 Internal Server Error",
            r#"{"error":"spawn failed","status":500}"#,
        )
        .expect_err("unexpected Admin error must stay fail closed");
        assert!(rejected.to_string().contains("spawn failed"));

        match previous_data_dir {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
        match previous_marker {
            Some(value) => std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value),
            None => std::env::remove_var(EXTERNAL_CLI_WORKER_ENV),
        }
    }

    #[test]
    fn admin_request_reports_transport_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve unavailable port");
        let port = listener.local_addr().expect("listener address").port();
        drop(listener);
        let runtime = RuntimeInfo::new(
            std::process::id(),
            port,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        );
        let error = request_admin_upgrade(&runtime).expect_err("closed port must fail");
        assert!(error.contains("POST http://127.0.0.1:"));
    }
}
