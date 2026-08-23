use std::process::Command;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn run_bifrost(data_dir: &std::path::Path, args: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .args(args)
        .env("BIFROST_DATA_DIR", data_dir)
        .env("BIFROST_DISABLE_TRAY", "1")
        .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
        .output()
        .expect("run bifrost CLI")
}

#[tokio::test(flavor = "multi_thread")]
async fn status_and_start_discover_live_admin_without_runtime_marker() {
    let server = MockServer::start().await;
    let port = server.address().port();
    let admin_pid = std::process::id();
    let data_dir = tempfile::tempdir().expect("temporary Bifrost data dir");
    let data_dir_fingerprint = bifrost_storage::data_dir_fingerprint_for(data_dir.path());
    Mock::given(method("GET"))
        .and(path("/_bifrost/api/system/overview"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "server": {"port": port},
            "system": {
                "pid": admin_pid,
                "uptime_secs": 12,
                "version": "0.0.test",
                "data_dir_fingerprint": data_dir_fingerprint
            }
        })))
        .mount(&server)
        .await;

    let port_arg = port.to_string();

    let json_status = run_bifrost(
        data_dir.path(),
        &[
            "-p".to_string(),
            port_arg.clone(),
            "status".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    assert!(
        json_status.status.success(),
        "JSON status failed: {}",
        String::from_utf8_lossy(&json_status.stderr)
    );
    let json_status: serde_json::Value =
        serde_json::from_slice(&json_status.stdout).expect("parse JSON status");
    assert_eq!(json_status["running"], true);
    assert_eq!(json_status["runtime_source"], "admin_api");
    assert_eq!(json_status["pid"], admin_pid);
    assert_eq!(json_status["listener"]["port"], port);

    let text_status = run_bifrost(
        data_dir.path(),
        &["-p".to_string(), port_arg.clone(), "status".to_string()],
    );
    assert!(text_status.status.success());
    let text_status = String::from_utf8_lossy(&text_status.stdout);
    assert!(text_status.contains("Status: Running"));
    assert!(text_status.contains("Source: Admin API fallback"));

    let start = run_bifrost(
        data_dir.path(),
        &[
            "-p".to_string(),
            port_arg,
            "start".to_string(),
            "--daemon".to_string(),
            "--yes".to_string(),
            "--skip-cert-check".to_string(),
            "--unsafe-ssl".to_string(),
            "--no-system-proxy".to_string(),
        ],
    );
    assert!(
        start.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    assert!(
        String::from_utf8_lossy(&start.stdout).contains("Reusing the live service"),
        "start should reuse the Admin-discovered service"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn start_refuses_to_take_over_bifrost_from_another_data_directory() {
    let server = MockServer::start().await;
    let port = server.address().port();
    let admin_pid = std::process::id();
    let owning_data_dir = tempfile::tempdir().expect("owning Bifrost data dir");
    let caller_data_dir = tempfile::tempdir().expect("caller Bifrost data dir");
    let owning_fingerprint = bifrost_storage::data_dir_fingerprint_for(owning_data_dir.path());
    Mock::given(method("GET"))
        .and(path("/_bifrost/api/system/overview"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "server": {"port": port},
            "system": {
                "pid": admin_pid,
                "uptime_secs": 12,
                "version": "0.0.test",
                "data_dir_fingerprint": owning_fingerprint
            }
        })))
        .mount(&server)
        .await;

    let start = run_bifrost(
        caller_data_dir.path(),
        &[
            "-p".to_string(),
            port.to_string(),
            "start".to_string(),
            "--daemon".to_string(),
            "--yes".to_string(),
            "--skip-cert-check".to_string(),
            "--unsafe-ssl".to_string(),
            "--no-system-proxy".to_string(),
        ],
    );

    assert!(!start.status.success(), "foreign runtime must be rejected");
    let stderr = String::from_utf8_lossy(&start.stderr);
    assert!(
        stderr.contains("different data directory"),
        "unexpected error: {stderr}"
    );
    assert!(
        !caller_data_dir.path().join("runtime.json").exists(),
        "foreign runtime must not be adopted"
    );

    #[cfg(not(windows))]
    {
        let restart = run_bifrost(
            caller_data_dir.path(),
            &["-p".to_string(), port.to_string(), "restart".to_string()],
        );
        assert!(
            !restart.status.success(),
            "restart must reject a foreign runtime"
        );
        let restart_error = String::from_utf8_lossy(&restart.stderr);
        assert!(
            restart_error.contains("different data directory"),
            "unexpected restart error: {restart_error}"
        );
    }

    std::fs::write(
        caller_data_dir.path().join("runtime.json"),
        serde_json::to_vec_pretty(&json!({
            "pid": admin_pid,
            "port": port,
            "host": "127.0.0.1",
            "runtime_start_mode": "daemon",
            "restartable_runtime": true
        }))
        .expect("serialize foreign runtime marker"),
    )
    .expect("write foreign runtime marker");
    std::fs::write(
        caller_data_dir.path().join("bifrost.pid"),
        admin_pid.to_string(),
    )
    .expect("write foreign pid marker");

    let stop = run_bifrost(
        caller_data_dir.path(),
        &["-p".to_string(), port.to_string(), "stop".to_string()],
    );
    assert!(!stop.status.success(), "stop must reject a foreign runtime");
    let stop_error = String::from_utf8_lossy(&stop.stderr);
    assert!(
        stop_error.contains("could not verify that it belongs to the active data directory"),
        "unexpected stop error: {stop_error}"
    );
    assert!(
        server.received_requests().await.is_some(),
        "foreign Admin service must remain alive after rejected stop"
    );
}

#[test]
fn stop_removes_a_stale_pid_marker_without_touching_a_live_service() {
    let data_dir = tempfile::tempdir().expect("temporary Bifrost data dir");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve free port");
    let port = listener.local_addr().expect("free port address").port();
    drop(listener);
    let mut exited = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("__bifrost_short_lived_process__")
        .spawn()
        .expect("spawn short-lived process");
    let stale_pid = exited.id();
    assert!(exited
        .wait()
        .expect("wait for short-lived process")
        .success());
    std::fs::write(data_dir.path().join("bifrost.pid"), stale_pid.to_string())
        .expect("write stale pid marker");

    let stop = run_bifrost(
        data_dir.path(),
        &["-p".to_string(), port.to_string(), "stop".to_string()],
    );
    assert!(
        stop.status.success(),
        "stale stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(
        String::from_utf8_lossy(&stop.stdout).contains("stale PID file removed"),
        "unexpected stop output: {}",
        String::from_utf8_lossy(&stop.stdout)
    );
    assert!(
        !data_dir.path().join("bifrost.pid").exists(),
        "stale pid marker should be removed"
    );
}
