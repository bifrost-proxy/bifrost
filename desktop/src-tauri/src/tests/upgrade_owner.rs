use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn upgrade_relaunch_never_moves_to_another_port_when_original_is_occupied() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind((super::super::BACKEND_BIND_HOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let error = super::super::launch_backend_on_available_port(
        Path::new("/must-not-launch"),
        temp.path(),
        "fixed-port",
        port,
        false,
    )
    .expect_err("occupied upgrade port must fail");
    assert!(error
        .to_string()
        .contains("failed to find an available backend port"));
    assert!(!temp.path().join("logs/desktop-sidecar.out.log").exists());
}

#[test]
fn upgrade_failed_child_cleanup_preserves_competing_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let binary = temp.path().join("bifrost");
    let runtime = format!(
        r#"{{"pid":{},"port":{},"runtime_start_mode":"daemon"}}"#,
        std::process::id(),
        port
    );
    // The runtime marker appears AFTER the preflight, as when the CLI wins
    // the startup race. The failed Desktop child must not run shared `stop`.
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
if [ "$1" = stop ]; then
    touch "$BIFROST_DATA_DIR/stop-called"
    exit 0
fi
printf '%s' '{runtime}' > "$BIFROST_DATA_DIR/runtime.json"
echo 'Port 0.0.0.0:{port} is already in use' >&2
exit 1
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let result = super::super::launch_backend_on_available_port(
        &binary,
        temp.path(),
        "competing-runtime",
        port,
        true,
    );
    assert!(result.is_err());
    assert!(
        !temp.path().join("stop-called").exists(),
        "failed child cleanup must not invoke shared stop"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("runtime.json")).unwrap(),
        runtime
    );
    let log = fs::read_to_string(temp.path().join("logs/desktop-bootstrap.log")).unwrap();
    assert_eq!(
        log.matches("starting sidecar;").count(),
        1,
        "must not launch an adjacent server"
    );
}
