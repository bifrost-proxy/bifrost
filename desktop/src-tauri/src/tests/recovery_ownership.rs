use super::super::{
    backend_candidate_is_trusted_for_recovery, clear_backend_unavailable_if_healthy,
    configure_backend_restart_stop_command, current_time_millis, log_dir,
    open_backend_recovery_circuit, runtime_markers_belong_to_exited_pid, sidecar_stderr_offset,
    sidecar_stderr_reports_port_conflict_since, BackendRecoveryCandidate, BackendSystemIdentity,
    DesktopRuntimeMarker, DesktopUpgradeRelaunchMarker, BACKEND_STOP_TIMEOUT,
};
use super::test_backend_state;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

fn spawn_one_shot_health_server() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind health server");
    let port = listener.local_addr().expect("health server addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
        }
    });
    port
}

fn spawn_system_identity_server(
    data_dir: &Path,
    pid: u32,
    version: &str,
    request_count: usize,
) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind system server");
    let port = listener.local_addr().expect("system server addr").port();
    let data_dir_fingerprint = bifrost_storage::data_dir_fingerprint_for(data_dir);
    let body = format!(
        r#"{{"version":"{version}","pid":{pid},"data_dir_fingerprint":"{data_dir_fingerprint}"}}"#
    );
    thread::spawn(move || {
        for _ in 0..request_count {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

#[test]
fn health_only_external_backend_cannot_clear_manual_start_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_one_shot_health_server();
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some(
            "Bifrost service is not running. Start the service from Bifrost Desktop to continue."
                .to_string(),
        ),
    );

    assert!(!clear_backend_unavailable_if_healthy(
        &state,
        "test observed recovered backend",
    ));
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert!(state
        .startup_error
        .lock()
        .expect("startup error lock")
        .is_some());
}

#[test]
fn matching_markerless_backend_clears_manual_start_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_system_identity_server(temp_dir.path(), 456, "0.0.188", 2);
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("Bifrost service is not running.".to_string()),
    );

    assert!(clear_backend_unavailable_if_healthy(
        &state,
        "test observed matching recovered backend",
    ));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().expect("error lock").is_none());
}

#[test]
fn healthy_backend_still_clears_manual_start_gate_during_app_managed_upgrade() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_system_identity_server(temp_dir.path(), 456, "0.0.163", 2);
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: current_time_millis(),
        old_app_pid: 123,
        old_core_pid: Some(456),
        observed_external_core_pid: None,
        proxy_port: port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("previous app-managed handoff failed".to_string()),
    );
    *state.upgrade_relaunch.lock().expect("marker lock") = Some(marker);

    assert!(clear_backend_unavailable_if_healthy(
        &state,
        "test observed app-managed recovered backend",
    ));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().expect("error lock").is_none());
}

#[test]
fn unhealthy_external_backend_keeps_manual_start_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let port = listener.local_addr().expect("reserved addr").port();
    drop(listener);
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("Bifrost service is not running.".to_string()),
    );

    assert!(!clear_backend_unavailable_if_healthy(
        &state,
        "test observed missing backend",
    ));
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert_eq!(
        state
            .startup_error
            .lock()
            .expect("startup error lock")
            .as_deref(),
        Some("Bifrost service is not running.")
    );
}

#[test]
fn bind_conflict_detection_reads_only_new_sidecar_stderr() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let logs = log_dir(temp_dir.path());
    fs::create_dir_all(&logs).expect("create logs");
    let stderr_path = logs.join("desktop-sidecar.err.log");
    fs::write(
        &stderr_path,
        "Error: Network error: Port 0.0.0.0:9900 is already in use\n",
    )
    .expect("write old stderr");
    let offset = sidecar_stderr_offset(temp_dir.path());

    assert!(!sidecar_stderr_reports_port_conflict_since(
        temp_dir.path(),
        9900,
        offset
    ));
    let mut stderr = fs::OpenOptions::new()
        .append(true)
        .open(&stderr_path)
        .expect("open stderr");
    writeln!(
        stderr,
        "Error: Network error: Port 0.0.0.0:9901 is already in use"
    )
    .expect("append stderr");

    assert!(!sidecar_stderr_reports_port_conflict_since(
        temp_dir.path(),
        9900,
        offset
    ));
    assert!(sidecar_stderr_reports_port_conflict_since(
        temp_dir.path(),
        9901,
        offset
    ));
}

#[test]
fn recovery_requires_current_data_directory_identity() {
    let identity = BackendSystemIdentity {
        version: "0.0.188".to_string(),
        pid: 456,
        data_dir_fingerprint: Some("foreign-data-dir".to_string()),
    };

    assert!(!backend_candidate_is_trusted_for_recovery(
        BackendRecoveryCandidate {
            runtime: None,
            has_any_runtime_marker: false,
            managed_child_pid: None,
            candidate_port: 9900,
            preferred_port: 9900,
            identity: Some(&identity),
            expected_data_dir_fingerprint: "current-data-dir",
            healthy: true,
        }
    ));

    let runtime = DesktopRuntimeMarker {
        pid: 456,
        port: 9900,
        health_port: None,
        start_mode: Some("desktop".to_string()),
    };
    assert!(!backend_candidate_is_trusted_for_recovery(
        BackendRecoveryCandidate {
            runtime: Some(&runtime),
            has_any_runtime_marker: true,
            managed_child_pid: None,
            candidate_port: 9900,
            preferred_port: 9900,
            identity: Some(&identity),
            expected_data_dir_fingerprint: "current-data-dir",
            healthy: true,
        }
    ));
}

#[test]
fn recovery_keeps_legacy_marker_backed_identity_compatibility() {
    let runtime = DesktopRuntimeMarker {
        pid: 456,
        port: 9900,
        health_port: None,
        start_mode: Some("desktop".to_string()),
    };
    let legacy_identity = BackendSystemIdentity {
        version: "0.0.150".to_string(),
        pid: 456,
        data_dir_fingerprint: None,
    };

    assert!(backend_candidate_is_trusted_for_recovery(
        BackendRecoveryCandidate {
            runtime: Some(&runtime),
            has_any_runtime_marker: true,
            managed_child_pid: None,
            candidate_port: 9900,
            preferred_port: 9900,
            identity: Some(&legacy_identity),
            expected_data_dir_fingerprint: "current-data-dir",
            healthy: true,
        }
    ));
    assert!(!backend_candidate_is_trusted_for_recovery(
        BackendRecoveryCandidate {
            runtime: None,
            has_any_runtime_marker: false,
            managed_child_pid: None,
            candidate_port: 9900,
            preferred_port: 9900,
            identity: Some(&legacy_identity),
            expected_data_dir_fingerprint: "current-data-dir",
            healthy: true,
        }
    ));
}

#[test]
fn desktop_backend_restart_stop_preserves_system_proxy_handoff() {
    let data_dir = Path::new("/tmp/bifrost-desktop-restart");
    let mut command = Command::new("bifrost");
    configure_backend_restart_stop_command(&mut command, data_dir);

    let env = command.get_envs().collect::<Vec<_>>();
    assert!(env.iter().any(|(name, value)| {
        *name == OsStr::new("BIFROST_DESKTOP_RESTART_STOP_INTERNAL")
            && *value == Some(OsStr::new("1"))
    }));
}

#[test]
fn synchronous_stop_timeout_covers_cli_termination_budget() {
    assert!(BACKEND_STOP_TIMEOUT >= Duration::from_secs(30));
}

#[test]
fn recovery_circuit_marks_backend_unavailable_for_manual_recovery() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = test_backend_state(temp_dir.path().to_path_buf(), 19900, false, None);
    state.startup_ready.store(true, Ordering::SeqCst);

    open_backend_recovery_circuit(&state, "recovery circuit open".to_string());

    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert_eq!(
        state
            .startup_error
            .lock()
            .expect("startup error")
            .as_deref(),
        Some("recovery circuit open")
    );
    let log = fs::read_to_string(temp_dir.path().join("logs/desktop-bootstrap.log"))
        .expect("bootstrap log");
    assert!(log.contains("desktop backend bootstrap failed: recovery circuit open"));
}

#[test]
fn exited_managed_runtime_markers_require_exact_pid_match() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        temp_dir.path().join("runtime.json"),
        r#"{"pid":4321,"port":19900,"runtime_start_mode":"desktop"}"#,
    )
    .expect("runtime marker");
    fs::write(temp_dir.path().join("bifrost.pid"), "4321").expect("pid marker");

    assert!(runtime_markers_belong_to_exited_pid(temp_dir.path(), 4321).expect("matching markers"));
    let error = runtime_markers_belong_to_exited_pid(temp_dir.path(), 9876)
        .expect_err("mismatched marker must block recovery");
    assert!(error
        .to_string()
        .contains("instead of confirmed exited pid"));

    fs::write(
        temp_dir.path().join("runtime.json"),
        r#"{"pid":4321,"port":19900,"runtime_start_mode":"desktop"}"#,
    )
    .expect("runtime marker reset");
    fs::write(temp_dir.path().join("bifrost.pid"), "9876").expect("mismatched pid marker");
    let error = runtime_markers_belong_to_exited_pid(temp_dir.path(), 4321)
        .expect_err("mismatched pid marker must block recovery");
    assert!(error.to_string().contains("pid marker belongs to pid=9876"));

    fs::write(temp_dir.path().join("runtime.json"), "not-json").expect("invalid runtime marker");
    let error = runtime_markers_belong_to_exited_pid(temp_dir.path(), 4321)
        .expect_err("invalid runtime marker must block recovery");
    assert!(error.to_string().contains("failed to parse runtime marker"));

    fs::remove_file(temp_dir.path().join("runtime.json")).expect("remove runtime marker");
    fs::write(temp_dir.path().join("bifrost.pid"), "not-a-pid").expect("invalid pid marker");
    let error = runtime_markers_belong_to_exited_pid(temp_dir.path(), 4321)
        .expect_err("invalid pid marker must block recovery");
    assert!(error.to_string().contains("failed to parse pid marker"));
}

#[test]
fn exited_managed_runtime_cleanup_accepts_absent_markers() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    assert!(!runtime_markers_belong_to_exited_pid(temp_dir.path(), 4321)
        .expect("absent markers are already clean"));
}
