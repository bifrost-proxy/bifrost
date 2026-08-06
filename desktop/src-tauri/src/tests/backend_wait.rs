use super::*;

#[test]
fn wait_for_backend_reports_child_exit_without_waiting_for_timeout() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let port = listener.local_addr().expect("reserved addr").port();
    drop(listener);

    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--list", "--format", "terse"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn short-lived child");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let started_at = Instant::now();
    let error = wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(5))
        .expect_err("short-lived child should fail readiness wait");

    assert!(
        started_at.elapsed() < Duration::from_secs(2),
        "child exit should short-circuit the readiness timeout"
    );
    assert_eq!(error.kind, BackendWaitFailureKind::ChildExited);
    assert!(error.to_string().contains("exited before becoming ready"));
}

#[cfg(unix)]
#[test]
fn wait_for_backend_ignores_health_from_unrelated_process() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let stop = Arc::new(AtomicBool::new(false));
    let (port, health_server) = spawn_persistent_health_server(stop.clone());
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("sleep 0.2")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn short-lived child");

    let error = wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(3))
        .expect_err("external health server must not satisfy managed child readiness");

    stop.store(true, Ordering::SeqCst);
    health_server.join().expect("health server thread");
    assert_eq!(error.kind, BackendWaitFailureKind::ChildExited);
}

#[cfg(unix)]
#[test]
fn wait_for_backend_accepts_health_from_matching_runtime_child() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let stop = Arc::new(AtomicBool::new(false));
    let (port, health_server) = spawn_persistent_health_server(stop.clone());
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("sleep 3")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn long-lived child");
    fs::write(
        temp_dir.path().join("runtime.json"),
        format!(r#"{{"pid":{},"port":{}}}"#, child.id(), port),
    )
    .expect("write runtime marker");

    wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(3))
        .expect("matching runtime marker should satisfy readiness");

    let _ = child.kill();
    let _ = child.wait();
    stop.store(true, Ordering::SeqCst);
    health_server.join().expect("health server thread");
}

#[test]
fn poll_managed_backend_exit_reports_exited_child() {
    let child = Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn test child");
    let _ = child.wait_with_output();

    let state = BackendState {
        binary_path: PathBuf::new(),
        data_dir: PathBuf::new(),
        config_path: PathBuf::new(),
        startup_session_id: "test-session".to_string(),
        launcher_only: false,
        expected_port: Mutex::new(0),
        port: Mutex::new(0),
        child: Mutex::new(Some(
            Command::new("sh")
                .arg("-c")
                .arg("exit 0")
                .spawn()
                .expect("spawn managed child"),
        )),
        shutdown_started: AtomicBool::new(false),
        force_exit: AtomicBool::new(false),
        backend_recovery_in_progress: AtomicBool::new(false),
        startup_ready: AtomicBool::new(false),
        startup_error: Mutex::new(None),
        main_webview_loaded: AtomicBool::new(false),
        main_window_ready: AtomicBool::new(false),
        handoff_started: AtomicBool::new(false),
        handoff_completed: AtomicBool::new(false),
        launcher_overlay: Mutex::new(None),
        pending_open_requests: Mutex::new(Vec::new()),
        upgrade_relaunch: Mutex::new(None),
    };

    {
        let mut child_guard = state.child.lock().expect("child lock");
        let child = child_guard.as_mut().expect("child");
        let _ = child.wait();
    }

    let exited = poll_managed_backend_exit(&state)
        .expect("managed child inspection")
        .expect("exited child reason");
    assert!(exited.detail.contains("exited with status"));
    assert_ne!(exited.pid, 0);
    assert!(state.child.lock().expect("child lock").is_none());
    assert!(!state.backend_recovery_in_progress.load(Ordering::SeqCst));
}
