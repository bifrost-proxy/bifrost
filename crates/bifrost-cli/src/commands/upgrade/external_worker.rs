use bifrost_core::{BifrostError, EXTERNAL_CLI_WORKER_ENV};
use colored::Colorize;
use std::time::Duration;

use crate::process::{discover_bifrost_runtime, read_runtime_info, RuntimeInfo};

const ADMIN_UPGRADE_REQUEST_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminUpgradeDelegationOutcome {
    Scheduled,
    AlreadyInProgress,
    AlreadyCurrent,
}

pub(super) fn is_external_cli_worker() -> bool {
    super::env_flag(EXTERNAL_CLI_WORKER_ENV)
}

/// An external CLI worker is owned by the running Bifrost daemon. Performing
/// an inline upgrade from that process tree is self-defeating: stopping the old
/// daemon also kills the worker, Codex, and the nested updater before it can
/// install and restart Bifrost. Hand the request back to Admin, which starts the
/// existing detached `self-update` orchestrator outside the worker process
/// group.
pub(super) fn delegate_upgrade() -> Result<(), BifrostError> {
    let runtime = read_runtime_info().and_then(|runtime| {
        let discovered = discover_bifrost_runtime(runtime.port)?;
        (discovered.pid == runtime.pid).then_some(runtime)
    });
    let outcome = delegate_upgrade_with(runtime, request_admin_upgrade)?;
    match outcome {
        AdminUpgradeDelegationOutcome::Scheduled => println!(
            "{}",
            "✓ Upgrade scheduled by the running Bifrost service; it will restart automatically."
                .bright_green()
                .bold()
        ),
        AdminUpgradeDelegationOutcome::AlreadyInProgress => println!(
            "{}",
            "✓ An upgrade is already in progress in the running Bifrost service.".bright_green()
        ),
        AdminUpgradeDelegationOutcome::AlreadyCurrent => {
            println!("{}", "✓ Bifrost is already up to date.".bright_green())
        }
    }
    Ok(())
}

fn delegate_upgrade_with<F>(
    runtime: Option<RuntimeInfo>,
    request: F,
) -> Result<AdminUpgradeDelegationOutcome, BifrostError>
where
    F: FnOnce(u16) -> Result<(u16, String), String>,
{
    let runtime = runtime.ok_or_else(|| {
        BifrostError::Config(
            "Cannot safely schedule an upgrade from an external CLI worker because the owning Bifrost service is not reachable. The running service was left unchanged."
                .to_string(),
        )
    })?;
    let (status, body) = request(runtime.port).map_err(|error| {
        BifrostError::Config(format!(
            "Failed to schedule the upgrade through the running Bifrost service: {error}. The running service was left unchanged."
        ))
    })?;
    classify_admin_upgrade_response(status, &body).map_err(BifrostError::Config)
}

fn request_admin_upgrade(port: u16) -> Result<(u16, String), String> {
    let url = format!("http://127.0.0.1:{port}/_bifrost/api/system/upgrade?channel=cli");
    let result = bifrost_core::direct_ureq_agent_builder()
        .timeout(Duration::from_secs(ADMIN_UPGRADE_REQUEST_TIMEOUT_SECS))
        .build()
        .post(&url)
        .call();
    match result {
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(error) => Err(format!("POST {url} failed: {error}")),
    }
}

fn classify_admin_upgrade_response(
    status: u16,
    body: &str,
) -> Result<AdminUpgradeDelegationOutcome, String> {
    if status == 202 {
        return Ok(AdminUpgradeDelegationOutcome::Scheduled);
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
        return Ok(AdminUpgradeDelegationOutcome::AlreadyInProgress);
    }
    if status == 409 && message == "No update available" {
        return Ok(AdminUpgradeDelegationOutcome::AlreadyCurrent);
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
    use std::process::Command;

    #[test]
    fn delegation_classifies_admin_responses() {
        assert_eq!(
            classify_admin_upgrade_response(202, r#"{"phase":"checking"}"#).unwrap(),
            AdminUpgradeDelegationOutcome::Scheduled
        );
        assert_eq!(
            classify_admin_upgrade_response(
                409,
                r#"{"error":"An upgrade is already in progress","status":409}"#,
            )
            .unwrap(),
            AdminUpgradeDelegationOutcome::AlreadyInProgress
        );
        assert_eq!(
            classify_admin_upgrade_response(
                409,
                r#"{"error":"No update available","status":409}"#,
            )
            .unwrap(),
            AdminUpgradeDelegationOutcome::AlreadyCurrent
        );

        let error = classify_admin_upgrade_response(500, r#"{"error":"spawn failed"}"#)
            .expect_err("unexpected Admin failure must not be treated as success");
        assert!(error.contains("HTTP 500: spawn failed"));
        assert!(error.contains("left unchanged"));
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
        let unreachable = delegate_upgrade_with(Some(runtime.clone()), |port| {
            assert_eq!(port, 19876);
            Err("connection refused".to_string())
        })
        .expect_err("unreachable Admin must not fall back to inline upgrade");
        assert!(unreachable.to_string().contains("connection refused"));
        assert!(unreachable.to_string().contains("left unchanged"));

        let scheduled = delegate_upgrade_with(Some(runtime), |port| {
            assert_eq!(port, 19876);
            Ok((202, r#"{"phase":"checking"}"#.to_string()))
        })
        .expect("Admin accepts delegated upgrade");
        assert_eq!(scheduled, AdminUpgradeDelegationOutcome::Scheduled);
    }

    #[test]
    fn marker_accepts_documented_truthy_values() {
        const CHILD_ENV: &str = "BIFROST_TEST_EXTERNAL_WORKER_ENV_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::external_worker::tests::marker_accepts_documented_truthy_values",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated environment test");
            assert!(status.success());
            return;
        }

        for value in ["1", "true", "yes"] {
            std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value);
            assert!(is_external_cli_worker(), "value={value}");
        }
        for value in ["0", "false", "no", ""] {
            std::env::set_var(EXTERNAL_CLI_WORKER_ENV, value);
            assert!(!is_external_cli_worker(), "value={value}");
        }
        std::env::remove_var(EXTERNAL_CLI_WORKER_ENV);
        assert!(!is_external_cli_worker());
    }

    #[test]
    fn handle_upgrade_uses_live_admin_without_inline_fallback() {
        const CHILD_ENV: &str = "BIFROST_TEST_EXTERNAL_WORKER_HANDLE_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::external_worker::tests::handle_upgrade_uses_live_admin_without_inline_fallback",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .env_remove(EXTERNAL_CLI_WORKER_ENV)
                .status()
                .expect("spawn isolated delegated upgrade test");
            assert!(status.success());
            return;
        }

        fn serve_request(listener: &TcpListener, status: &str, body: &str) {
            let (mut stream, _) = listener.accept().expect("accept Admin request");
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).expect("read Admin request");
            assert!(count > 0);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write Admin response");
        }

        fn run_case(status: &'static str, body: &'static str) -> Result<(), BifrostError> {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Admin");
            let port = listener.local_addr().expect("mock Admin address").port();
            write_runtime_info(&RuntimeInfo::new(
                std::process::id(),
                port,
                None,
                Some("127.0.0.1".to_string()),
                RuntimeStartMode::Daemon,
            ))
            .expect("write runtime");
            let server = std::thread::spawn(move || {
                let overview = format!(
                    r#"{{"server":{{"port":{port}}},"system":{{"pid":{},"uptime_secs":1,"version":"test"}}}}"#,
                    std::process::id()
                );
                serve_request(&listener, "200 OK", &overview);
                serve_request(&listener, status, body);
            });
            let result = super::super::handle_upgrade(true);
            server.join().expect("join mock Admin");
            result
        }

        let data_dir = tempfile::tempdir().expect("temp data dir");
        std::env::set_var("BIFROST_DATA_DIR", data_dir.path());
        std::env::set_var(EXTERNAL_CLI_WORKER_ENV, "1");

        run_case("202 Accepted", r#"{"phase":"checking"}"#).expect("scheduled upgrade");
        run_case(
            "409 Conflict",
            r#"{"error":"An upgrade is already in progress","status":409}"#,
        )
        .expect("already running is idempotent success");
        run_case(
            "409 Conflict",
            r#"{"error":"No update available","status":409}"#,
        )
        .expect("already current is idempotent success");
        let rejected = run_case(
            "500 Internal Server Error",
            r#"{"error":"spawn failed","status":500}"#,
        )
        .expect_err("unexpected Admin error must stay fail closed");
        assert!(rejected.to_string().contains("spawn failed"));

        std::env::remove_var("BIFROST_DATA_DIR");
        std::env::remove_var(EXTERNAL_CLI_WORKER_ENV);
    }
}
