use super::super::{
    configure_backend_restart_stop_command, open_backend_recovery_circuit,
    runtime_markers_belong_to_exited_pid, BACKEND_STOP_TIMEOUT,
};
use super::test_backend_state;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
