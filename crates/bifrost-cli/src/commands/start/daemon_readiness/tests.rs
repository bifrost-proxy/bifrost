use super::*;
use std::net::TcpListener;
use std::sync::mpsc;

fn runtime(pid: u32, port: u16, host: &str) -> RuntimeInfo {
    serde_json::from_value(serde_json::json!({
        "pid": pid, "port": port, "host": host, "socks5_port": null,
        "runtime_start_mode": "daemon", "restartable_runtime": true,
        "started_at_ms": 10_000,
    }))
    .unwrap()
}

fn write_marker(path: &Path, runtime: &RuntimeInfo) -> Vec<u8> {
    // Deliberately avoid write_runtime_info: these tests only own fake marker
    // files and loopback sockets, never system-proxy lifecycle diagnostics.
    let bytes = serde_json::to_vec(runtime).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn bound_listener_waits_for_delayed_complete_runtime_marker() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let waiter_path = path.clone();
    let (observed_tx, observed_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let expected = SpawnedDaemon {
            pid: 123,
            host: "127.0.0.1",
            port,
            started_at_ms: Some(10_000),
            runtime_file: &waiter_path,
            previous_runtime: None,
        };
        let timeout = Duration::from_secs(5);
        wait_for_readiness(
            123,
            timeout,
            Instant::now() + timeout,
            || Ok(None),
            |budget| {
                let ready = expected.is_ready(budget);
                observed_tx.send(ready).unwrap();
                if !ready {
                    release_rx.recv_timeout(timeout).unwrap();
                }
                ready
            },
        )
    });

    // The former TCP-only readiness check succeeds throughout these two
    // incomplete-marker phases. The waiter must remain blocked in both.
    assert!(TcpStream::connect(listener.local_addr().unwrap()).is_ok());
    assert!(!observed_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    std::fs::write(&path, b"{\"pid\":123,").unwrap();
    release_tx.send(()).unwrap();
    assert!(!observed_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    write_marker(&path, &runtime(123, port, "127.0.0.1"));
    release_tx.send(()).unwrap();
    assert!(observed_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    waiter.join().unwrap().unwrap();
}

#[test]
fn marker_must_be_fresh_and_match_the_spawned_daemon() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let valid = runtime(123, port, "127.0.0.1");
    let previous = write_marker(&path, &valid);
    let mut expected = SpawnedDaemon {
        pid: 123,
        host: "127.0.0.1",
        port,
        started_at_ms: None,
        runtime_file: &path,
        previous_runtime: Some(&previous),
    };
    assert!(
        !expected.is_ready(Duration::from_secs(1)),
        "stale same-PID marker"
    );
    let mut fresh = valid.clone();
    fresh.started_at_ms = None;
    write_marker(&path, &fresh);
    assert!(
        expected.is_ready(Duration::from_secs(1)),
        "fresh marker without OS start time"
    );

    expected.previous_runtime = None;
    expected.started_at_ms = Some(10_000);
    for field in ["pid", "port", "host", "mode", "restartable", "start_time"] {
        let mut foreign = valid.clone();
        match field {
            "pid" => foreign.pid += 1,
            "port" => foreign.port = if port == 1 { 2 } else { 1 },
            "host" => foreign.host = Some("127.0.0.2".into()),
            "mode" => foreign.start_mode = crate::process::RuntimeStartMode::Desktop,
            "restartable" => foreign.restartable_runtime = false,
            "start_time" => foreign.started_at_ms = Some(1),
            _ => unreachable!(),
        }
        write_marker(&path, &foreign);
        assert!(
            !expected.is_ready(Duration::from_secs(1)),
            "foreign {field}"
        );
    }
    write_marker(&path, &valid);
    assert!(!expected.is_ready(Duration::ZERO));
    assert!(expected.is_ready(Duration::from_secs(1)));
    drop(listener);
    assert!(
        !expected.is_ready(Duration::from_millis(50)),
        "closed listener"
    );
}

#[test]
fn readiness_preserves_wildcard_and_custom_bind_hosts() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    for host in ["0.0.0.0", "127.0.0.1", "invalid-host"] {
        write_marker(&path, &runtime(123, port, host));
        let expected = SpawnedDaemon {
            pid: 123,
            host,
            port,
            started_at_ms: Some(10_000),
            runtime_file: &path,
            previous_runtime: None,
        };
        assert_eq!(
            expected.is_ready(Duration::from_secs(1)),
            host != "invalid-host"
        );
    }
    assert_eq!(normalize_host("[::1]"), "::1");
    assert_eq!(normalize_host("::1"), "::1");
    assert_eq!(
        readiness_address("[::1]", port),
        Some(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)))
    );
    assert_eq!(
        readiness_address("[fe80::1%1]", port),
        format!("[fe80::1%1]:{port}").parse().ok()
    );
    assert!(readiness_address("[fe80::1%1]", port).is_some());
}

#[test]
fn readiness_rejects_child_exit_and_never_extends_its_deadline() {
    let budget = Duration::from_secs(5);
    let error = wait_for_readiness(
        123,
        budget,
        Instant::now() + budget,
        || Ok(Some("exited".into())),
        |_| panic!("must check child first"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Daemon exited"));

    let mut polls = 0;
    let error = wait_for_readiness(
        123,
        budget,
        Instant::now() + budget,
        || {
            polls += 1;
            Ok((polls == 2).then(|| "exited".into()))
        },
        |_| true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("Daemon exited"));

    let error = wait_for_readiness(
        123,
        budget,
        Instant::now() + budget,
        || Err(std::io::Error::other("poll failed")),
        |_| true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("poll failed"));

    let budget = Duration::from_millis(20);
    let started = Instant::now();
    let deadline = started + budget;
    let error = wait_for_readiness(
        123,
        budget,
        deadline,
        || Ok(None),
        |remaining| {
            assert!(remaining <= budget);
            false
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not become ready"));
    assert!(Instant::now() >= deadline);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "readiness must not reset its budget while polling"
    );

    let error = wait_for_readiness(
        123,
        budget,
        Instant::now() + budget,
        || Ok(None),
        |remaining| {
            std::thread::sleep(remaining + Duration::from_millis(1));
            true
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("did not become ready"),
        "late success must not reset the deadline"
    );

    let mut polls = 0;
    let error = wait_for_readiness(
        123,
        budget,
        Instant::now() + budget,
        || {
            polls += 1;
            if polls == 2 {
                std::thread::sleep(budget + Duration::from_millis(1));
            }
            Ok(None)
        },
        |_| true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not become ready"));

    let error = wait_for_readiness(
        123,
        Duration::ZERO,
        Instant::now(),
        || panic!("deadline already expired"),
        |_| panic!("deadline already expired"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not become ready"));
}

#[test]
fn exited_owned_child_cannot_be_replaced_by_a_live_listener() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    // An empty test filter is a portable short-lived child. Never launch the
    // real CLI or terminate any process found through the fixture's port.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "nonexistent_daemon_readiness_fixture"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child.wait().unwrap();
    write_marker(&path, &runtime(child.id(), port, "127.0.0.1"));
    let error = wait_for_detached_daemon_ready(
        &mut child,
        "127.0.0.1",
        port,
        &path,
        None,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Daemon exited"));
    assert!(TcpStream::connect(listener.local_addr().unwrap()).is_ok());
}
