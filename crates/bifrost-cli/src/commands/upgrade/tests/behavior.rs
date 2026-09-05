use super::*;

#[cfg(unix)]
#[test]
fn upgrade_behavior_executes_companion_and_runtime_ownership_branches() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_UPGRADE_BEHAVIOR_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::upgrade::tests::behavior::upgrade_behavior_executes_companion_and_runtime_ownership_branches",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated upgrade behavior test");
        assert!(status.success(), "isolated upgrade behavior test failed");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("apps");
    let data_dir = temp.path().join("data");
    std::fs::create_dir_all(&app_dir).expect("create app dir");
    std::fs::create_dir_all(&data_dir).expect("create data dir");

    let env_keys = [
        "BIFROST_APP_INSTALL_DIR",
        "BIFROST_DATA_DIR",
        DESKTOP_MANAGED_SKIP_APP_ENV,
        DESKTOP_MANAGED_SKIP_RESTART_ENV,
        DESKTOP_MANAGED_TARGET_ENV,
        crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV,
        crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV,
        WEBVIEW_UPGRADE_ORIGIN_ENV,
        "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        "BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL",
        "BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL",
    ];
    let previous: Vec<_> = env_keys
        .iter()
        .map(|key| ((*key).to_string(), std::env::var_os(key)))
        .collect();
    std::env::set_var("BIFROST_APP_INSTALL_DIR", &app_dir);
    std::env::set_var("BIFROST_DATA_DIR", &data_dir);
    std::env::set_var(
        "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        env!("CARGO_PKG_VERSION"),
    );

    let app_path = desktop_app_install_candidates()
        .into_iter()
        .next()
        .expect("app install candidate");
    if app_path
        .extension()
        .is_some_and(|extension| extension == "app")
    {
        std::fs::create_dir_all(&app_path).expect("create app bundle");
    } else {
        std::fs::write(&app_path, b"fixture").expect("write app fixture");
    }

    let success = temp.path().join("success-cli");
    std::fs::write(&success, "#!/bin/sh\nexit 0\n").expect("write success helper");
    std::fs::set_permissions(&success, std::fs::Permissions::from_mode(0o755))
        .expect("chmod success helper");
    let failure = temp.path().join("failure-cli");
    std::fs::write(&failure, "#!/bin/sh\necho app-failed >&2\nexit 7\n")
        .expect("write failure helper");
    std::fs::set_permissions(&failure, std::fs::Permissions::from_mode(0o755))
        .expect("chmod failure helper");

    #[cfg(target_os = "macos")]
    {
        use std::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut runtime = RuntimeInfo::new(
            std::process::id(),
            port,
            None,
            None,
            RuntimeStartMode::Daemon,
        );
        let marker_path = data_dir.join("desktop-upgrade-relaunch.json");
        // A dead marker and a reused PID without a live Admin endpoint must
        // both leave Desktop's normal stale-runtime recovery available.
        drop(listener);
        for pid in [u32::MAX, std::process::id()] {
            runtime.pid = pid;
            write_runtime_info(&runtime).unwrap();
            update_desktop_app_after_upgrade(&success, "99.0.1").unwrap();
            assert!(
                !marker_path.exists(),
                "stale runtime must not become a handoff"
            );
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        runtime.port = listener.local_addr().unwrap().port();
        let port = runtime.port;
        let pid = runtime.pid;
        let fingerprint = bifrost_storage::data_dir_fingerprint();
        let responder = std::thread::spawn(move || {
            // Wrong profile, mismatched PID, then seven healthy observations.
            for (response_pid, profile) in [
                (pid, "foreign".to_string()),
                (pid + 1, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint.clone()),
                (pid, fingerprint),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with("GET /_bifrost/api/system/overview "));
                let body = format!(
                    r#"{{"server":{{"port":{port}}},"system":{{"pid":{response_pid},"uptime_secs":1,"version":"99.0.1","data_dir_fingerprint":"{profile}"}}}}"#
                );
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        write_runtime_info(&runtime).expect("snapshot CLI owner");
        for _ in 0..2 {
            update_desktop_app_after_upgrade(&success, "99.0.1").unwrap();
            assert!(
                !marker_path.exists(),
                "foreign identity must not become a handoff"
            );
        }
        for mode in [RuntimeStartMode::Daemon, RuntimeStartMode::Desktop] {
            runtime.start_mode = mode;
            write_runtime_info(&runtime).unwrap();
            handle_upgrade_inner(
                UpgradeBehavior::interactive(true, true),
                Some(env!("CARGO_PKG_VERSION").to_string()),
            )
            .expect("already-current entrypoint validates the original live owner");
            assert_eq!(read_runtime_info().unwrap().start_mode, mode);
        }
        runtime.start_mode = RuntimeStartMode::Daemon;
        write_runtime_info(&runtime).unwrap();
        let frozen = UpgradeBehavior::background().for_runtime_owner(Some(&runtime));
        assert_eq!(live_cli_runtime_for_handoff(port).unwrap().pid, runtime.pid);
        update_desktop_companion(&success, "99.0.1", frozen)
            .expect("verified owner is carried into the companion snapshot");
        update_desktop_app_after_upgrade(&success, "99.0.1")
            .expect("caller-managed handoff success");
        let marker_path = data_dir.join("desktop-upgrade-relaunch.json");
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker["proxy_port"], port);
        assert_eq!(marker["observed_external_core_pid"], std::process::id());
        assert!(marker["old_core_pid"].is_null());
        update_desktop_app_after_upgrade(&failure, "99.0.1").expect_err("failed install");
        assert!(
            !marker_path.exists(),
            "failed caller-managed install removes its own handoff"
        );
        std::fs::create_dir(&marker_path).expect("simulate unwritable marker destination");
        let error = update_desktop_app_after_upgrade(&success, "99.0.1")
            .expect_err("marker persistence failure must abort before launching Desktop");
        assert!(matches!(error, BifrostError::Io(_)));
        assert!(marker_path.is_dir());
        std::fs::remove_dir(&marker_path).unwrap();
        responder.join().unwrap();
        crate::process::remove_pid(runtime.pid).expect("remove both isolated runtime markers");
        let error = finish_installed_upgrade(&success, "99.0.1", frozen)
            .expect_err("owner vanishing during install must block Desktop continuation");
        assert!(error
            .to_string()
            .contains("disappeared or changed during upgrade"));
        assert!(!marker_path.exists());
        finish_already_latest_upgrade_for_method(
            "99.0.1",
            frozen,
            &InstallMethod::Manual(success.clone()),
        )
        .expect_err("already-current upgrade must also preserve the frozen owner");
        // An owner disappearing after validation still leaves an explicit CLI
        // marker; Desktop must not fall back to normal managed startup.
        update_desktop_app_with_runtime(&success, "99.0.1", Some(runtime.clone())).unwrap();
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker["observed_external_core_pid"], runtime.pid);
        assert_eq!(marker["proxy_port"], port);
        std::fs::remove_file(&marker_path).unwrap();
    }
    update_desktop_app_after_upgrade(&success, "99.0.1").expect("strict app success");
    let error =
        update_desktop_app_after_upgrade(&failure, "99.0.1").expect_err("strict app failure");
    assert!(error.to_string().contains("app-failed"));
    assert!(
        update_desktop_app_after_upgrade(&temp.path().join("missing"), "99.0.1")
            .expect_err("spawn error")
            .to_string()
            .contains("could not run")
    );
    update_desktop_app_after_upgrade_best_effort(&failure, "99.0.1");

    update_desktop_companion(&failure, "99.0.1", UpgradeBehavior::interactive(true, true))
        .expect("desktop-managed child skips app");
    update_desktop_companion(&success, "99.0.1", UpgradeBehavior::background())
        .expect("background requires successful app");
    update_desktop_companion(
        &failure,
        "99.0.1",
        UpgradeBehavior::interactive(false, true),
    )
    .expect("manual app update remains best effort");
    std::env::set_var("BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL", "19999");
    let missing_owner = interactive_upgrade_behavior(false, true).for_runtime_owner(None);
    assert_eq!(missing_owner.expected_cli_port, Some(19999));
    assert!(std::env::var_os("BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL").is_none());
    update_desktop_companion(&success, "99.0.1", missing_owner).expect_err(
        "Windows continuation must retain an owner that vanished before its invocation",
    );
    std::env::remove_var("BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL");
    std::env::set_var("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL", "1");
    let continuation = interactive_upgrade_behavior(false, true);
    assert!(std::env::var_os("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL").is_none());
    assert!(!continuation.restart_proxy);
    update_desktop_companion(&failure, "99.0.1", continuation)
        .expect_err("deferred background companion failure must propagate");
    update_desktop_companion(&success, "99.0.1", continuation)
        .expect("deferred background companion success");
    std::env::set_var("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL", "0");
    update_desktop_companion(
        &failure,
        "99.0.1",
        interactive_upgrade_behavior(false, true),
    )
    .expect("deferred interactive companion remains best effort");
    std::env::remove_var("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL");

    std::fs::remove_file(&app_path)
        .or_else(|_| std::fs::remove_dir_all(&app_path))
        .ok();
    finish_already_latest_upgrade(
        env!("CARGO_PKG_VERSION"),
        UpgradeBehavior::interactive(true, true),
    )
    .expect("desktop-managed already-latest is a no-op");
    finish_already_latest_upgrade(env!("CARGO_PKG_VERSION"), UpgradeBehavior::background())
        .expect("background already-latest restarts when no app is installed");
    assert!(finish_already_latest_upgrade_for_method(
        env!("CARGO_PKG_VERSION"),
        UpgradeBehavior::background(),
        &InstallMethod::Unknown,
    )
    .is_err());
    finish_already_latest_upgrade_for_method(
        env!("CARGO_PKG_VERSION"),
        UpgradeBehavior::interactive(false, true),
        &InstallMethod::Unknown,
    )
    .expect("manual unknown install remains best effort");
    finish_installed_upgrade(
        &success,
        env!("CARGO_PKG_VERSION"),
        UpgradeBehavior::interactive(true, true),
    )
    .expect("desktop-managed installed finish skips app and restart");

    let desktop_runtime = RuntimeInfo::new(
        std::process::id(),
        19900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Desktop,
    );
    write_runtime_info(&desktop_runtime).expect("write desktop runtime");
    maybe_restart_running_proxy(&success).expect("desktop owns restart handoff");
    finish_installed_upgrade(
        &success,
        env!("CARGO_PKG_VERSION"),
        UpgradeBehavior {
            restart_if_already_latest: false,
            update_desktop_app: false,
            require_desktop_app_update: false,
            restart_proxy: true,
            expected_cli_port: None,
        },
    )
    .expect("installed desktop runtime leaves restart to app");

    let free_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    let free_port = free_listener.local_addr().expect("local addr").port();
    drop(free_listener);
    assert!(prepare_running_proxy_marker(Some(RunningProxyHint {
        pid: std::process::id(),
        port: free_port,
    }))
    .is_err());

    let parent_lock = crate::commands::upgrade_background::try_acquire_upgrade_lock(&data_dir)
        .expect("open parent upgrade lock")
        .expect("own parent upgrade lock");
    std::env::set_var(
        crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV,
        "forged-token",
    );
    std::env::set_var(
        crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV,
        std::process::id().to_string(),
    );
    assert!(
        handle_upgrade(true, None).is_err(),
        "forged owner credentials must not bypass lock"
    );
    std::env::set_var(DESKTOP_MANAGED_SKIP_APP_ENV, "yes");
    std::env::set_var(DESKTOP_MANAGED_SKIP_RESTART_ENV, "true");
    std::env::set_var(DESKTOP_MANAGED_TARGET_ENV, env!("CARGO_PKG_VERSION"));
    assert!(env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));
    assert!(env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV));
    assert!(
        handle_upgrade(true, None).is_err(),
        "managed flags plus forged credentials must not bypass lock"
    );
    drop(parent_lock);
    std::env::remove_var(DESKTOP_MANAGED_SKIP_APP_ENV);
    assert!(!env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));

    for (key, value) in previous {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
