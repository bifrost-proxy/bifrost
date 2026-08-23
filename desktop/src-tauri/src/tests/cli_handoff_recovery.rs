use super::*;

#[test]
fn cli_owned_upgrade_relaunch_reuses_the_target_backend_even_when_pid_is_unchanged() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let target_port = spawn_one_shot_system_server(temp_dir.path(), 456, "0.0.156");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(124),
        proxy_port: target_port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };

    let (child, port) = ensure_backend_running(
        Path::new("/must-not-launch-a-second-core"),
        temp_dir.path(),
        "hybrid-upgrade-test",
        target_port,
        Some(&marker),
    )
    .expect("the relaunched App reuses the restarted CLI-owned core");
    assert!(child.is_none());
    assert_eq!(port, target_port);
    assert!(upgrade_relaunch_uses_external_cli_backend(&marker));
    assert!(
        !upgrade_handoff_requires_backend_release(&marker),
        "the relaunch helper must preserve a CLI-owned core while the updater restarts it"
    );

    let managed_marker = DesktopUpgradeRelaunchMarker {
        old_core_pid: Some(124),
        observed_external_core_pid: None,
        ..marker.clone()
    };
    assert!(
        !upgrade_relaunch_uses_external_cli_backend(&managed_marker),
        "an App-managed core still requires release and a fresh bundled child"
    );
    assert!(upgrade_handoff_requires_backend_release(&managed_marker));

    assert!(external_cli_backend_matches_handoff(
        &marker,
        &BackendSystemIdentity {
            version: "0.0.156".to_string(),
            pid: 124,
            data_dir_fingerprint: Some(bifrost_storage::data_dir_fingerprint_for(temp_dir.path(),)),
        }
    ));

    let old_version_port = spawn_one_shot_system_server(temp_dir.path(), 457, "0.0.155");
    let old_version_marker = DesktopUpgradeRelaunchMarker {
        proxy_port: old_version_port,
        ..marker.clone()
    };
    assert!(!wait_for_external_cli_backend(
        temp_dir.path(),
        &old_version_marker,
        Duration::ZERO
    ));

    let marker_without_target = DesktopUpgradeRelaunchMarker {
        target_version: None,
        ..marker
    };
    assert!(
        !external_cli_backend_matches_handoff(
            &marker_without_target,
            &BackendSystemIdentity {
                version: "0.0.156".to_string(),
                pid: 124,
                data_dir_fingerprint: Some(bifrost_storage::data_dir_fingerprint_for(
                    temp_dir.path(),
                )),
            }
        ),
        "legacy markers without a target version still require PID rotation"
    );
}

#[test]
fn failed_cli_owned_handoff_retries_without_another_thirty_second_wait() {
    use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(124),
        proxy_port: 19900,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };
    let progress = UpgradeProgress::new(
        UpgradePhase::Failed,
        "Desktop app updated but the new core failed to start",
    )
    .with_target(Some("0.0.163".to_string()))
    .with_error(Some(
        "CLI-owned backend did not restart on port 19900".to_string(),
    ));
    write_progress(temp_dir.path(), &progress);

    assert!(failed_cli_handoff_can_retry_immediately(&marker, &progress));
    assert_eq!(
        external_cli_handoff_wait(temp_dir.path(), &marker, Duration::from_secs(30)),
        Duration::ZERO
    );

    let legacy_marker = DesktopUpgradeRelaunchMarker {
        target_version: None,
        ..marker.clone()
    };
    assert!(
        failed_cli_handoff_can_retry_immediately(&legacy_marker, &progress),
        "a failed marker written by an older Desktop version must recover without another wait"
    );
    assert_eq!(
        external_cli_handoff_wait(temp_dir.path(), &legacy_marker, Duration::from_secs(30)),
        Duration::ZERO
    );

    let unrelated = UpgradeProgress::new(UpgradePhase::Failed, "different failure")
        .with_target(Some("0.0.163".to_string()))
        .with_error(Some("installer failed".to_string()));
    assert!(!failed_cli_handoff_can_retry_immediately(
        &marker, &unrelated
    ));
}

#[test]
fn cli_owned_upgrade_relaunch_takes_over_wrong_version_core_owned_by_same_data_dir() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let target_port = spawn_system_server_on(
        super::super::BACKEND_BIND_HOST,
        temp_dir.path(),
        456,
        "0.0.162",
        2,
    );
    fs::write(
        temp_dir.path().join("runtime.json"),
        format!(r#"{{"pid":456,"port":{target_port},"runtime_start_mode":"daemon"}}"#),
    )
    .expect("write runtime marker");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(456),
        proxy_port: target_port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };

    let resolution = resolve_external_cli_backend_handoff(temp_dir.path(), &marker, Duration::ZERO)
        .expect("verified same-data-dir old core should be eligible for managed recovery");

    assert!(
        matches!(resolution, ExternalCliBackendHandoff::StartManagedFallback),
        "verified same-data-dir core should enter safe managed recovery"
    );
    let bootstrap_log =
        fs::read_to_string(temp_dir.path().join("logs/desktop-bootstrap.log")).expect("log");
    assert!(
        bootstrap_log.contains("matches this data directory runtime marker"),
        "unexpected bootstrap log:\n{bootstrap_log}"
    );
}

#[test]
fn healthy_target_backend_completes_and_clears_cli_upgrade_handoff() {
    use bifrost_core::upgrade_progress::{read_progress, UpgradePhase};

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_one_shot_system_server(temp_dir.path(), 456, "0.0.163");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(456),
        proxy_port: port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };
    write_upgrade_relaunch_marker(temp_dir.path(), &marker).expect("write marker");
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("previous handoff failed".to_string()),
    );
    *state.upgrade_relaunch.lock().expect("marker lock") = Some(marker);

    assert!(clear_backend_unavailable_if_healthy(
        &state,
        "test observed recovered target backend",
    ));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state
        .upgrade_relaunch
        .lock()
        .expect("marker lock")
        .is_none());
    assert!(!desktop_upgrade_relaunch_marker_path(temp_dir.path()).exists());
    assert_eq!(
        read_progress(temp_dir.path()).phase,
        UpgradePhase::Completed
    );
}

#[test]
fn healthy_wrong_version_backend_does_not_bypass_cli_upgrade_handoff() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_one_shot_system_server(temp_dir.path(), 457, "0.0.162");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(456),
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
        Some("previous handoff failed".to_string()),
    );
    *state.upgrade_relaunch.lock().expect("marker lock") = Some(marker);

    assert!(!clear_backend_unavailable_if_healthy(
        &state,
        "test observed wrong-version backend",
    ));
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().expect("error lock").is_some());
}

#[test]
fn healthy_target_backend_on_another_port_does_not_complete_cli_upgrade_handoff() {
    use bifrost_core::upgrade_progress::{read_progress, UpgradePhase};

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let active_port = spawn_one_shot_system_server(temp_dir.path(), 456, "0.0.163");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(456),
        proxy_port: active_port.saturating_add(1),
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };
    write_upgrade_relaunch_marker(temp_dir.path(), &marker).expect("write marker");
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        active_port,
        false,
        Some("previous handoff failed".to_string()),
    );
    *state.upgrade_relaunch.lock().expect("marker lock") = Some(marker);

    assert!(!clear_backend_unavailable_if_healthy(
        &state,
        "test observed target backend on another port",
    ));
    assert!(desktop_upgrade_relaunch_marker_path(temp_dir.path()).exists());
    assert_ne!(
        read_progress(temp_dir.path()).phase,
        UpgradePhase::Completed
    );
}
