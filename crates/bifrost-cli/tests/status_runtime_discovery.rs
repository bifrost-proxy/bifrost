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
    Mock::given(method("GET"))
        .and(path("/_bifrost/api/system/overview"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "server": {"port": port},
            "system": {
                "pid": admin_pid,
                "uptime_secs": 12,
                "version": "0.0.test"
            }
        })))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().expect("temporary Bifrost data dir");
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
