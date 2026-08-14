use super::*;

#[test]
fn already_current_desktop_companion_skips_child_and_handoff() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let app = resolve_desktop_app_path(temp.path());
    let contents = app.join("Contents");
    std::fs::create_dir_all(&contents).expect("create app Contents");
    std::fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.156</string>
</dict></plist>"#,
    )
    .expect("write app version");

    let previous_app_dir = std::env::var_os("BIFROST_APP_INSTALL_DIR");
    let previous_origin = std::env::var_os(WEBVIEW_UPGRADE_ORIGIN_ENV);
    std::env::set_var("BIFROST_APP_INSTALL_DIR", temp.path());
    std::env::set_var(WEBVIEW_UPGRADE_ORIGIN_ENV, "1");
    assert!(!take_desktop_handoff_scheduled());

    update_desktop_app_after_upgrade(&temp.path().join("must-not-run"), "0.0.156")
        .expect("already-current App skips missing companion executable");
    assert!(!take_desktop_handoff_scheduled());

    match previous_app_dir {
        Some(value) => std::env::set_var("BIFROST_APP_INSTALL_DIR", value),
        None => std::env::remove_var("BIFROST_APP_INSTALL_DIR"),
    }
    match previous_origin {
        Some(value) => std::env::set_var(WEBVIEW_UPGRADE_ORIGIN_ENV, value),
        None => std::env::remove_var(WEBVIEW_UPGRADE_ORIGIN_ENV),
    }
}

#[test]
fn desktop_shutdown_targets_shell_instead_of_bundled_core() {
    let app_path = Path::new("/Applications/Bifrost.app");
    let bundled_core = app_path.join("Contents/Resources/resources/bin/bifrost");
    let desktop_shell = app_path.join("Contents/MacOS/bifrost-desktop");

    #[cfg(target_os = "macos")]
    assert_eq!(desktop_shell_executable(app_path), desktop_shell);

    assert_eq!(
        select_running_desktop_shell_process(
            &desktop_shell,
            [
                (79401, bundled_core.clone()),
                (79399, desktop_shell.clone()),
            ],
        ),
        Some((79399, desktop_shell.clone())),
        "the internal shutdown argument must be sent to the desktop shell even when the bundled core is discovered first"
    );
    assert_eq!(
        select_running_desktop_shell_process(&desktop_shell, [(79401, bundled_core)]),
        None,
        "the bundled CLI core must never receive the desktop-only shutdown argument"
    );
}

#[cfg(unix)]
#[test]
fn desktop_process_paths_accept_canonical_aliases() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let canonical_root = temp.path().join("private");
    let canonical_app = canonical_root.join("Bifrost.app");
    let canonical_shell = canonical_app.join("Contents/MacOS/bifrost-desktop");
    std::fs::create_dir_all(canonical_shell.parent().expect("shell parent"))
        .expect("create canonical App");
    std::fs::write(&canonical_shell, b"fixture").expect("write canonical shell");

    let alias_root = temp.path().join("var");
    symlink(&canonical_root, &alias_root).expect("create App path alias");
    let alias_app = alias_root.join("Bifrost.app");
    let alias_shell = alias_app.join("Contents/MacOS/bifrost-desktop");

    assert!(path_is_within(&canonical_shell, &alias_app));
    assert_eq!(
        select_running_desktop_shell_process(&alias_shell, [(79402, canonical_shell.clone())],),
        Some((79402, canonical_shell)),
        "macOS process discovery must tolerate /var and /private/var aliases"
    );
}

#[test]
fn macos_legacy_shutdown_targets_exact_app_path() {
    use std::ffi::OsString;

    assert_eq!(
        macos_desktop_quit_args(Path::new("/Users/test/Applications/Bifrost \"Canary\".app",)),
        vec![
            OsString::from("-e"),
            OsString::from("on run argv"),
            OsString::from("-e"),
            OsString::from("set appPath to item 1 of argv"),
            OsString::from("-e"),
            OsString::from("using terms from application \"Finder\""),
            OsString::from("-e"),
            OsString::from("tell application appPath to quit"),
            OsString::from("-e"),
            OsString::from("end using terms from"),
            OsString::from("-e"),
            OsString::from("end run"),
            OsString::from("--"),
            OsString::from("/Users/test/Applications/Bifrost \"Canary\".app"),
        ],
        "legacy App versions must be quit by exact bundle path without ambiguous name lookup"
    );
}

#[test]
fn desktop_handoff_flag_requires_matching_restarting_progress() {
    use bifrost_core::upgrade_progress::{UpgradePhase, UpgradeProgress};

    let temp = tempfile::tempdir().expect("tempdir");
    for (phase, source, target, expected) in [
        (UpgradePhase::Completed, "desktop", "0.0.156", false),
        (UpgradePhase::Restarting, "admin", "0.0.156", false),
        (UpgradePhase::Restarting, "desktop", "0.0.155", false),
        (UpgradePhase::Restarting, "desktop", "0.0.156", true),
    ] {
        bifrost_core::upgrade_progress::write_progress(
            temp.path(),
            &UpgradeProgress::new(phase, "child result")
                .with_source(Some(source.to_string()))
                .with_target(Some(target.to_string())),
        );
        assert_eq!(
            child_scheduled_desktop_handoff(temp.path(), "0.0.156"),
            expected,
            "phase={phase:?} source={source} target={target}"
        );
    }
}

#[test]
fn failed_companion_restores_only_a_shell_that_was_shut_down() {
    let app = Path::new("/tmp/Bifrost.app");
    let relaunches = std::cell::Cell::new(0);
    let untouched = restore_desktop_after_failed_app_upgrade(
        app,
        false,
        BifrostError::Config("companion failed".to_string()),
        |_| {
            relaunches.set(relaunches.get() + 1);
            Ok(())
        },
    );
    assert_eq!(untouched.to_string(), "Config error: companion failed");
    assert_eq!(relaunches.get(), 0);

    let restored = restore_desktop_after_failed_app_upgrade(
        app,
        true,
        BifrostError::Config("companion failed".to_string()),
        |path| {
            assert_eq!(path, app);
            relaunches.set(relaunches.get() + 1);
            Ok(())
        },
    );
    assert_eq!(restored.to_string(), "Config error: companion failed");
    assert_eq!(relaunches.get(), 1);

    let combined = restore_desktop_after_failed_app_upgrade(
        app,
        true,
        BifrostError::Config("companion failed".to_string()),
        |_| {
            relaunches.set(relaunches.get() + 1);
            Err(BifrostError::Config("open failed".to_string()))
        },
    );
    assert_eq!(relaunches.get(), 2);
    assert!(combined.to_string().contains("companion failed"));
    assert!(combined
        .to_string()
        .contains("previous desktop shell relaunch also failed"));
    assert!(combined.to_string().contains("open failed"));
}

#[test]
fn desktop_app_upgrade_wrappers_handle_an_absent_shell() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join("Missing-Bifrost.app");

    assert!(!desktop_app_is_running(&app));
    shutdown_running_desktop_for_app_upgrade(&app)
        .expect("an absent desktop shell already has released its files");
}

#[cfg(target_os = "macos")]
#[test]
fn failed_companion_uses_production_macos_relaunch_command() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    let marker = temp.path().join("relaunch-marker");
    let fake_open = bin.join("open");
    std::fs::write(
        &fake_open,
        "#!/bin/sh\nprintf '%s' \"$1\" > \"$BIFROST_TEST_RELAUNCH_MARKER\"\n",
    )
    .expect("write fake open");
    std::fs::set_permissions(&fake_open, std::fs::Permissions::from_mode(0o755))
        .expect("make fake open executable");

    let previous_path = std::env::var_os("PATH");
    let previous_marker = std::env::var_os("BIFROST_TEST_RELAUNCH_MARKER");
    std::env::set_var("PATH", &bin);
    std::env::set_var("BIFROST_TEST_RELAUNCH_MARKER", &marker);
    let app = temp.path().join("Bifrost.app");
    let error = restore_desktop_after_failed_companion(
        &app,
        true,
        BifrostError::Config("companion failed".to_string()),
        crate::commands::app::restart_desktop_app,
    );
    assert_eq!(error.to_string(), "Config error: companion failed");
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(&marker).expect("fake open records App path"),
        app.to_string_lossy()
    );

    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    match previous_marker {
        Some(value) => std::env::set_var("BIFROST_TEST_RELAUNCH_MARKER", value),
        None => std::env::remove_var("BIFROST_TEST_RELAUNCH_MARKER"),
    }
}

#[cfg(unix)]
#[test]
fn cli_restart_stops_live_process_and_strips_detached_marker() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    const CHILD_ENV: &str = "BIFROST_TEST_CLI_RESTART_CHILD";
    const TEST_NAME: &str = "commands::upgrade::tests::review_comments::cli_restart_stops_live_process_and_strips_detached_marker";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env_remove("BIFROST_DATA_DIR")
            .status()
            .expect("spawn isolated CLI restart test");
        assert!(status.success(), "isolated CLI restart test failed");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    std::env::set_var("BIFROST_DATA_DIR", temp.path());
    std::env::set_var(crate::commands::start::DETACHED_DAEMON_CHILD_ENV, "1");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve restart port");
    let port = listener.local_addr().expect("restart port").port();
    drop(listener);

    let mut proxy = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn temporary proxy process");
    let runtime = RuntimeInfo::new(
        proxy.id(),
        port,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Daemon,
    );
    write_runtime_info(&runtime).expect("write temporary runtime marker");

    let restart = temp.path().join("restart-success");
    std::fs::write(
        &restart,
        "#!/bin/sh\n[ -z \"$BIFROST_DETACHED_DAEMON_CHILD\" ] || exit 9\nexit 0\n",
    )
    .expect("write restart helper");
    std::fs::set_permissions(&restart, std::fs::Permissions::from_mode(0o755))
        .expect("chmod restart helper");

    let result = maybe_restart_running_proxy(&restart);
    if result.is_err() {
        let _ = proxy.kill();
    }
    let _ = proxy.wait();
    result.expect("CLI-owned runtime restarts through detached helper");
    assert!(!is_process_running(runtime.pid));
}

#[test]
#[cfg(unix)]
fn parent_lock_credential_is_only_inherited_by_managed_companion() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let _owner = crate::commands::upgrade_background::try_acquire_upgrade_lock(temp.path())
        .expect("open parent lock")
        .expect("own parent lock");
    let token_env = crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV;
    let owner_pid_env = crate::commands::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV;
    let assertion = format!("test -n \"${token_env}\" && test -n \"${owner_pid_env}\"");

    let ordinary = command_output_with_timeout(
        Path::new("/bin/sh"),
        &["-c".to_string(), assertion.clone()],
        Duration::from_secs(1),
    )
    .expect("ordinary helper exits");
    assert_eq!(ordinary.status, TimedCommandStatus::Failure);

    let managed = command_output_with_timeout_and_env(
        Path::new("/bin/sh"),
        &["-c".to_string(), assertion],
        Duration::from_secs(1),
        Duration::from_millis(10),
        &[],
        Some(temp.path()),
    )
    .expect("managed helper exits");
    assert_eq!(managed.status, TimedCommandStatus::Success);
}

#[test]
fn windows_deferred_install_pins_target_and_respects_parent_progress_ownership() {
    let source = concat!(
        include_str!("../../upgrade.rs"),
        include_str!("../restart.rs")
    );
    for contract in [
        "update_desktop_companion(&restart_executable, &cache.latest_version, behavior)?;",
        "stop_tray_helper_before_windows_deferred_install(&data_dir);",
        "Wait-TargetPathWritable $TargetPath 120",
        "function Invoke-FileOperationWithRetry",
        "Invoke-FileOperationWithRetry \"removing old CLI\"",
        "Invoke-FileOperationWithRetry \"installing replacement CLI\"",
        "Invoke-FileOperationWithRetry \"restoring previous CLI\"",
        "Invoke-FileOperationWithRetry \"removing failed replacement staging file\"",
        "let result = schedule_windows_deferred_install_inner(&deferred_install, restart_args);",
        "cleanup_staged_binary_after_schedule(&deferred_install.staged_binary, result)",
        "installed CLI reports '$versionOutput' instead of target",
        "restored previous CLI after replacement failure",
        "[System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)",
        "$ProgressPath.tmp.$PID.$([Guid]::NewGuid().ToString('N'))",
        "for ($attempt = 0; $attempt -lt 100; $attempt++)",
        "$win32Code -notin @(5, 32, 33)",
        "Start-Sleep -Milliseconds (2 + ($attempt % 7))",
        "function Read-JsonWithRetry([string]$Path)",
        "$previous = Read-JsonWithRetry $ProgressPath",
        "Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue",
        "target_version = if ($TargetVersion)",
        "target_version: _target_version.to_string()",
        ".arg(\"-TargetVersion\")",
        ".arg(&deferred_install.target_version)",
        ".arg(\"-Source\")",
        "if ($PublishProgress -eq 0)",
        ".arg(\"-PublishProgress\")",
        "mark_deferred_install_scheduled();",
        "Write-DeferredStatus \"ok\"",
        "Write-DeferredStatus \"error: $errorMessage\"",
        "Write-DeferredStatus \"pending:$PID\"",
        "Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue",
        "foreach ($cleanupPath in @($RestartArgsPath, $PSCommandPath))",
        "Invoke-FileOperationWithRetry \"removing helper scratch file\"",
        "command.creation_flags(CREATE_NO_WINDOW)",
        "Write-UpgradeProgress \"completed\" \"Upgrade complete\" $null",
        "Write-UpgradeProgress \"failed\" \"Upgrade failed\" $errorMessage",
    ] {
        assert!(source.contains(contract), "missing contract: {contract}");
    }
}

#[test]
fn failed_windows_helper_schedule_removes_staged_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let staged = dir.path().join(".bifrost.exe.pending.1234");
    std::fs::write(&staged, b"replacement").expect("write staged binary");

    let error = cleanup_staged_binary_after_schedule::<()>(
        &staged,
        Err(BifrostError::Config("helper setup failed".to_string())),
    )
    .expect_err("schedule error remains visible");

    assert!(error.to_string().contains("helper setup failed"));
    assert!(!staged.exists(), "failed scheduling must not leak staging");
}

#[test]
fn successful_windows_helper_schedule_transfers_staging_ownership() {
    let dir = tempfile::tempdir().expect("tempdir");
    let staged = dir.path().join(".bifrost.exe.pending.1234");
    std::fs::write(&staged, b"replacement").expect("write staged binary");

    cleanup_staged_binary_after_schedule(&staged, Ok(())).expect("successful schedule");

    assert!(staged.exists(), "spawned helper owns the staged binary");
}

#[test]
fn failed_windows_helper_schedule_preserves_setup_and_cleanup_errors() {
    let dir = tempfile::tempdir().expect("tempdir");

    let error = cleanup_staged_binary_after_schedule::<()>(
        dir.path(),
        Err(BifrostError::Config("helper setup failed".to_string())),
    )
    .expect_err("cleanup failure remains visible with the setup failure");

    let message = error.to_string();
    assert!(message.contains("helper setup failed"));
    assert!(message.contains("additionally failed to clean staged Windows upgrade binary"));
}

#[test]
fn windows_helper_artifact_cleanup_attempts_every_path_before_returning_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let removable = dir.path().join("removable.args");
    std::fs::write(&removable, "restart args").expect("write removable fixture");

    let error = cleanup_windows_upgrade_artifacts(&[dir.path(), &removable])
        .expect_err("a directory cannot be removed with remove_file");

    assert!(error
        .to_string()
        .contains(&dir.path().display().to_string()));
    assert!(
        !removable.exists(),
        "later artifacts must still be cleaned after an earlier failure"
    );
}
#[test]
fn background_upgrade_restarts_when_disk_binary_is_already_latest() {
    let background = UpgradeBehavior::background();
    assert!(background.restart_if_already_latest);
    assert!(background.update_desktop_app);
    assert!(background.require_desktop_app_update);
    assert!(background.restart_proxy);

    let desktop_managed = UpgradeBehavior::interactive(true, true);
    assert!(!desktop_managed.restart_if_already_latest);
    assert!(!desktop_managed.update_desktop_app);
    assert!(!desktop_managed.require_desktop_app_update);
    assert!(!desktop_managed.restart_proxy);

    let manual = UpgradeBehavior::interactive(false, false);
    assert!(!manual.restart_if_already_latest);
    assert!(manual.update_desktop_app);
    assert!(!manual.require_desktop_app_update);
    assert!(manual.restart_proxy);

    let direct_app = app_managed_upgrade_behavior();
    assert!(direct_app.restart_if_already_latest);
    assert!(!direct_app.update_desktop_app);
    assert!(direct_app.restart_proxy);

    assert!(!take_desktop_handoff_scheduled());
    mark_desktop_handoff_scheduled();
    assert!(take_desktop_handoff_scheduled());
    assert!(!take_desktop_handoff_scheduled());
}

#[test]
fn upgrade_post_install_desktop_app_args_disable_cli_recursion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = temp.path().join("first-app");
    let active = temp.path().join("active-app");
    std::fs::write(&first, b"first").expect("write first app");
    std::fs::write(&active, b"active").expect("write active app");
    assert_eq!(
        select_installed_desktop_app_path([first, active.clone()], |path| path == active),
        Some(active),
        "the running App copy wins over the first global install candidate"
    );

    assert_eq!(
        post_upgrade_desktop_app_args(
            "0.0.145",
            Some(Path::new("/Users/test/Applications")),
            DesktopCompanionMode::CallerManaged,
        ),
        vec![
            "app",
            "upgrade",
            "--no-cli",
            "--source",
            "cli-upgrade",
            "--version",
            "0.0.145",
            "--app-dir",
            "/Users/test/Applications",
            "-y"
        ]
    );

    let handoff = post_upgrade_desktop_app_args(
        "0.0.145",
        Some(Path::new(r"C:\Program Files\Bifrost")),
        DesktopCompanionMode::DesktopHandoff,
    );
    assert_eq!(handoff[4], "desktop");
    assert_eq!(
        desktop_companion_environment(DesktopCompanionMode::CallerManaged),
        Vec::<(&str, &str)>::new()
    );
    assert_eq!(
        desktop_companion_environment(DesktopCompanionMode::DesktopHandoff),
        vec![(DESKTOP_UPGRADE_HANDOFF_ENV, "1")]
    );
    assert_eq!(
        desktop_companion_mode(true, true, true),
        DesktopCompanionMode::DesktopHandoff,
        "a WebView-originated Windows update delegates a running shell to Tauri"
    );
    assert_eq!(
        desktop_companion_mode(true, true, false),
        DesktopCompanionMode::CallerManaged,
        "a terminal-originated update must not wait for a nonexistent WebView handoff"
    );
    assert_eq!(
        desktop_companion_mode(true, false, true),
        DesktopCompanionMode::CallerManaged
    );
    assert_eq!(
        desktop_companion_mode(false, true, true),
        DesktopCompanionMode::CallerManaged
    );
    assert!(should_request_desktop_shutdown_before_update(
        true, true, false
    ));
    assert!(!should_request_desktop_shutdown_before_update(
        true, true, true
    ));
    assert!(!should_request_desktop_shutdown_before_update(
        true, false, false
    ));
    assert!(!should_request_desktop_shutdown_before_update(
        false, true, false
    ));
    assert!(windows_paths_match(
        Path::new(r"C:\Users\Eden\Bifrost.exe"),
        Path::new("c:/users/eden/bifrost.exe"),
    ));
}
