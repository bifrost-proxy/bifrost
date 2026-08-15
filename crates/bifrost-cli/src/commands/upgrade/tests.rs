use super::*;

mod download_helpers;

#[test]
fn older_discovery_result_cannot_downgrade_the_desktop_companion() {
    assert_eq!(
        companion_target_without_cli_upgrade("0.0.181-alpha.9", "0.0.181-alpha.7"),
        "0.0.181-alpha.9"
    );
    assert_eq!(
        companion_target_without_cli_upgrade("0.0.181-alpha.9", "0.0.181-alpha.10"),
        "0.0.181-alpha.10"
    );
}

#[test]
fn windows_desktop_discovery_includes_per_user_and_machine_msi_locations() {
    let candidates = windows_desktop_app_install_candidates_from_roots([
        PathBuf::from(r"C:\Users\tester\AppData\Local"),
        PathBuf::from(r"C:\Program Files"),
        PathBuf::from(r"c:/program files"),
        PathBuf::from(r"C:\Program Files (x86)"),
    ]);

    assert_eq!(
        candidates,
        vec![
            PathBuf::from(r"C:\Users\tester\AppData\Local")
                .join("Bifrost")
                .join("bifrost-desktop.exe"),
            PathBuf::from(r"C:\Program Files")
                .join("Bifrost")
                .join("bifrost-desktop.exe"),
            PathBuf::from(r"C:\Program Files (x86)")
                .join("Bifrost")
                .join("bifrost-desktop.exe"),
        ]
    );
}

#[test]
fn windows_upgrade_handoff_uses_staged_target_and_original_parent_pid() {
    let deferred = WindowsDeferredInstall {
        staged_binary: PathBuf::from(
            r"C:\Users\tester\AppData\Local\bifrost\bin\.bifrost.exe.pending.42",
        ),
        target_path: PathBuf::from(r"C:\Users\tester\AppData\Local\bifrost\bin\bifrost.exe"),
        target_version: "0.0.181-alpha.8".to_string(),
    };
    let restart = vec![
        "start".to_string(),
        "-d".to_string(),
        "--proxy-bypass".to_string(),
        "localhost,127.0.0.1".to_string(),
    ];
    let status = Path::new(r"C:\Users\tester\AppData\Local\Temp\upgrade.status");
    let handoff_ready =
        Path::new(r"C:\Users\tester\AppData\Local\bifrost\bin\.bifrost-upgrade-handoff.9876.ready");
    let args =
        windows_upgrade_handoff_args(&deferred, Some(&restart), 9876, Some(status), handoff_ready);
    let args: Vec<_> = args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();

    assert_eq!(args[0], "windows-upgrade-handoff");
    assert_eq!(args[1..3], ["--parent-pid", "9876"]);
    assert!(args.windows(2).any(|pair| {
        pair[0] == "--pending-path" && pair[1] == deferred.staged_binary.to_string_lossy()
    }));
    assert!(args.windows(2).any(|pair| {
        pair[0] == "--target-path" && pair[1] == deferred.target_path.to_string_lossy()
    }));
    assert!(args
        .windows(2)
        .any(|pair| { pair[0] == "--target-version" && pair[1] == deferred.target_version }));
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "--restart-arg")
            .count(),
        restart.len()
    );
    assert!(args.windows(2).any(|pair| {
        pair[0] == "--deferred-status-path" && pair[1] == status.to_string_lossy()
    }));
    assert!(args.windows(2).any(|pair| {
        pair[0] == "--handoff-ready-path" && pair[1] == handoff_ready.to_string_lossy()
    }));
}

#[test]
fn windows_deferred_desktop_companion_runs_from_the_staged_replacement() {
    let deferred = WindowsDeferredInstall {
        staged_binary: PathBuf::from(
            r"C:\Users\tester\AppData\Local\bifrost\bin\.bifrost.exe.pending.42",
        ),
        target_path: PathBuf::from(r"C:\Users\tester\AppData\Local\bifrost\bin\bifrost.exe"),
        target_version: "0.0.181-alpha.23".to_string(),
    };

    assert_eq!(
        windows_deferred_desktop_companion_executable(&deferred),
        deferred.staged_binary.as_path()
    );
    assert_ne!(
        windows_deferred_desktop_companion_executable(&deferred),
        deferred.target_path.as_path(),
        "the old running target cannot execute fixes from the downloaded version"
    );
}

#[test]
fn windows_upgrade_handoff_spawn_retries_sharing_violations() {
    let mut attempts = 0;
    let value = spawn_windows_upgrade_handoff_with_retry(|| {
        attempts += 1;
        if attempts < 4 {
            Err(io::Error::from_raw_os_error(32))
        } else {
            Ok("scheduled")
        }
    })
    .expect("freshly copied staged binary eventually becomes executable");
    assert_eq!(value, "scheduled");
    assert_eq!(attempts, 4);
}

#[test]
fn windows_upgrade_handoff_spawn_fails_fast_for_non_sharing_errors() {
    let mut attempts = 0;
    let error = spawn_windows_upgrade_handoff_with_retry::<()>(|| {
        attempts += 1;
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked"))
    })
    .expect_err("non-Windows-style errors must not be retried");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(attempts, 1);
}

#[test]
fn windows_upgrade_handoff_waits_for_helper_ready_after_staged_child_exits() {
    let mut ready_checks = 0;
    let mut wait_checks = 0;
    let mut sleeps = 0;
    let outcome = wait_for_windows_upgrade_handoff_ready_with(
        5,
        || {
            ready_checks += 1;
            Ok(ready_checks == 3)
        },
        || {
            wait_checks += 1;
            Ok((wait_checks == 1).then_some(0))
        },
        |status| *status == 0,
        |_| sleeps += 1,
    )
    .expect("poll helper readiness");

    assert_eq!(outcome, WindowsUpgradeHandoffReady::Ready);
    assert_eq!(wait_checks, 1, "successful child exit is remembered");
    assert_eq!(sleeps, 2);
}

#[test]
fn windows_upgrade_handoff_reports_staged_child_failure_before_timeout() {
    let outcome = wait_for_windows_upgrade_handoff_ready_with(
        5,
        || Ok(false),
        || Ok(Some(17)),
        |status| *status == 0,
        |_| panic!("failed staged child must not be retried"),
    )
    .expect("poll staged child");

    assert_eq!(outcome, WindowsUpgradeHandoffReady::Failed(17));
}

#[test]
fn windows_upgrade_handoff_command_is_hidden_and_preserves_hyphenated_restart_args() {
    use clap::{CommandFactory, Parser};

    let cli = crate::cli::Cli::parse_from([
        "bifrost",
        "windows-upgrade-handoff",
        "--parent-pid",
        "4321",
        "--pending-path",
        r"C:\bifrost\.bifrost.exe.pending.42",
        "--target-path",
        r"C:\bifrost\bifrost.exe",
        "--target-version",
        "0.0.181-alpha.8",
        "--handoff-ready-path",
        r"C:\bifrost\.bifrost-upgrade-handoff.4321.ready",
        "--restart-arg",
        "start",
        "--restart-arg",
        "--no-system-proxy",
    ]);
    let Some(crate::cli::Commands::WindowsUpgradeHandoff {
        parent_pid,
        restart_arg,
        ..
    }) = cli.command
    else {
        panic!("expected hidden Windows handoff command")
    };
    assert_eq!(parent_pid, 4321);
    assert_eq!(restart_arg, ["start", "--no-system-proxy"]);

    let help = crate::cli::Cli::command().render_help().to_string();
    assert!(!help.contains("windows-upgrade-handoff"));
}

#[test]
fn windows_upgrade_handoff_request_accepts_only_the_running_staged_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("bifrost.exe");
    let pending = dir.path().join(".bifrost.exe.pending.4321");
    let handoff_ready = dir.path().join(".bifrost-upgrade-handoff.4321.ready");
    std::fs::write(&pending, b"replacement").expect("write pending binary");

    validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        &target,
        "0.0.181-alpha.8",
        &pending,
        &handoff_ready,
    )
    .expect("valid staged handoff");

    let wrong_pending = dir.path().join(".bifrost.exe.pending.9999");
    std::fs::write(&wrong_pending, b"replacement").expect("write wrongly named pending binary");
    let error = validate_windows_upgrade_handoff_request(
        4321,
        &wrong_pending,
        &target,
        "0.0.181-alpha.8",
        &wrong_pending,
        &handoff_ready,
    )
    .expect_err("pending name must be derived from target and parent PID");
    assert!(error.to_string().contains("must be named"));

    let arbitrary_target = dir.path().join("updater.exe");
    let arbitrary_pending = dir.path().join(".updater.exe.pending.4321");
    std::fs::write(&arbitrary_pending, b"replacement").expect("write arbitrary pending binary");
    let error = validate_windows_upgrade_handoff_request(
        4321,
        &arbitrary_pending,
        &arbitrary_target,
        "0.0.181-alpha.8",
        &arbitrary_pending,
        &handoff_ready,
    )
    .expect_err("handoff must not become an arbitrary executable replacement primitive");
    assert!(error.to_string().contains("only replace bifrost.exe"));

    let other_exe = dir.path().join("other-staged.exe");
    std::fs::write(&other_exe, b"other").expect("write other executable");
    let error = validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        &target,
        "0.0.181-alpha.8",
        &other_exe,
        &handoff_ready,
    )
    .expect_err("handoff must execute from pending binary");
    assert!(error.to_string().contains("must run from the staged"));
}

#[test]
fn windows_upgrade_handoff_request_rejects_unsafe_metadata_and_cross_directory_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let other_dir = tempfile::tempdir().expect("other tempdir");
    let pending = dir.path().join(".bifrost.exe.pending.4321");
    let handoff_ready = dir.path().join(".bifrost-upgrade-handoff.4321.ready");
    std::fs::write(&pending, b"replacement").expect("write pending binary");

    let zero_pid = validate_windows_upgrade_handoff_request(
        0,
        &pending,
        &dir.path().join("bifrost.exe"),
        "0.0.181-alpha.8",
        &pending,
        &handoff_ready,
    )
    .expect_err("zero PID must be rejected");
    assert!(zero_pid.to_string().contains("non-zero parent PID"));

    let invalid_version = validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        &dir.path().join("bifrost.exe"),
        "0.0.181\n-Command",
        &pending,
        &handoff_ready,
    )
    .expect_err("control characters in target version must be rejected");
    assert!(invalid_version
        .to_string()
        .contains("invalid target version"));

    let cross_directory = validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        &other_dir.path().join("bifrost.exe"),
        "0.0.181-alpha.8",
        &pending,
        &handoff_ready,
    )
    .expect_err("cross-directory replacement must be rejected");
    assert!(cross_directory
        .to_string()
        .contains("must share the same directory"));

    let foreign_handoff_ready = other_dir.path().join(".bifrost-upgrade-handoff.4321.ready");
    let foreign_ready_error = validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        &dir.path().join("bifrost.exe"),
        "0.0.181-alpha.8",
        &pending,
        &foreign_handoff_ready,
    )
    .expect_err("handoff readiness marker cannot target another directory");
    assert!(foreign_ready_error
        .to_string()
        .contains("ready path must be"));

    let missing_target_name = validate_windows_upgrade_handoff_request(
        4321,
        &pending,
        Path::new(""),
        "0.0.181-alpha.8",
        &pending,
        &handoff_ready,
    )
    .expect_err("target path without a file name must be rejected");
    assert!(missing_target_name.to_string().contains("valid file name"));

    let relative_pending = Path::new(".bifrost.exe.pending.4321");
    let relative_target = Path::new("bifrost.exe");
    let missing_pending_parent = validate_windows_upgrade_handoff_request(
        4321,
        relative_pending,
        relative_target,
        "0.0.181-alpha.8",
        relative_pending,
        &handoff_ready,
    )
    .expect_err("relative staging path without a parent must be rejected");
    assert!(missing_pending_parent
        .to_string()
        .contains("staging path has no parent"));

    let pending_with_parent = Path::new("dir/.bifrost.exe.pending.4321");
    let missing_target_parent = validate_windows_upgrade_handoff_request(
        4321,
        pending_with_parent,
        relative_target,
        "0.0.181-alpha.8",
        pending_with_parent,
        &handoff_ready,
    )
    .expect_err("relative target path without a parent must be rejected");
    assert!(missing_target_parent
        .to_string()
        .contains("target path has no parent"));
}

#[test]
fn windows_upgrade_file_cleanup_retries_sharing_errors() {
    let path = Path::new("C:/fixture/.bifrost.exe.pending.42");
    let mut attempts = 0;
    let mut delays = Vec::new();

    remove_windows_upgrade_file_with(
        path,
        |actual_path| {
            assert_eq!(actual_path, path);
            attempts += 1;
            if attempts < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        |delay| delays.push(delay),
        true,
    )
    .expect("sharing violation is retried until cleanup succeeds");

    assert_eq!(attempts, 3);
    assert_eq!(
        delays,
        [Duration::from_millis(25), Duration::from_millis(35)]
    );
}

#[test]
fn windows_upgrade_file_cleanup_outlasts_the_previous_short_retry_budget() {
    let path = Path::new("C:/fixture/.bifrost.exe.pending.42");
    let mut attempts = 0;
    let mut delays = Vec::new();

    remove_windows_upgrade_file_with(
        path,
        |_| {
            attempts += 1;
            if attempts < WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        |delay| delays.push(delay),
        true,
    )
    .expect("cleanup must outlast transient scanners that exceed the old budget");

    assert_eq!(attempts, WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS);
    assert_eq!(delays.len(), WINDOWS_UPGRADE_CLEANUP_MAX_ATTEMPTS - 1);
    assert!(
        delays.iter().copied().sum::<Duration>() >= Duration::from_secs(30),
        "cleanup retry budget must cover long-lived antivirus and indexer handles"
    );
}

#[test]
fn windows_upgrade_file_cleanup_handles_missing_and_terminal_errors() {
    remove_windows_upgrade_file_with(
        Path::new("missing"),
        |_| Err(io::Error::from(io::ErrorKind::NotFound)),
        |_| panic!("missing file must not sleep"),
        true,
    )
    .expect("missing cleanup target is already clean");

    let error = remove_windows_upgrade_file_with(
        Path::new("denied"),
        |_| Err(io::Error::from_raw_os_error(5)),
        |_| panic!("non-Windows cleanup must not retry"),
        false,
    )
    .expect_err("terminal cleanup error remains visible");
    assert!(error.to_string().contains("denied"));
}

#[test]
fn test_detect_install_method_returns_valid_variant() {
    let method = detect_install_method();
    match method {
        InstallMethod::Homebrew
        | InstallMethod::Npm
        | InstallMethod::Pnpm
        | InstallMethod::Script
        | InstallMethod::Manual(_)
        | InstallMethod::Unknown => {}
    }
}

#[test]
fn homebrew_restart_uses_stable_launcher_outside_versioned_cellar() {
    assert_eq!(
        homebrew_launcher_for_executable(Path::new(
            "/opt/homebrew/Cellar/bifrost/0.0.155/bin/bifrost"
        )),
        PathBuf::from("/opt/homebrew/bin/bifrost")
    );
    assert_eq!(
        homebrew_launcher_for_executable(Path::new(
            "/usr/local/Cellar/bifrost/0.0.155/bin/bifrost"
        )),
        PathBuf::from("/usr/local/bin/bifrost")
    );
    assert_eq!(
        homebrew_launcher_for_executable(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/bifrost/0.0.155/bin/bifrost"
        )),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/bifrost")
    );
}

#[cfg(unix)]
#[test]
fn homebrew_upgrade_commands_are_bounded_and_verify_formula_target() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_HOMEBREW_UPGRADE_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::upgrade::tests::homebrew_upgrade_commands_are_bounded_and_verify_formula_target",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated Homebrew upgrade test");
        assert!(status.success());
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let brew = dir.path().join("brew");
    let git = dir.path().join("git");
    fs::write(
        &brew,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --repository) echo '{}' ;;\n  reinstall) exit 0 ;;\n  info) echo '{{\"formulae\":[{{\"installed\":[{{\"version\":\"0.0.156\"}}]}}]}}' ;;\nesac\n",
            dir.path().display()
        ),
    )
    .unwrap();
    fs::write(&git, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", dir.path());

    upgrade_via_homebrew("0.0.156").expect("fake Homebrew target verified");
}

#[cfg(unix)]
#[test]
fn homebrew_upgrade_fallback_and_verification_failures_are_bounded() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_HOMEBREW_FAILURE_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::upgrade::tests::homebrew_upgrade_fallback_and_verification_failures_are_bounded",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated Homebrew failure test");
        assert!(status.success());
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let brew = dir.path().join("brew");
    let git = dir.path().join("git");
    fs::write(
        &brew,
        "#!/bin/sh\n\
         case \"$1\" in\n\
           --repository) echo /missing/tap ;;\n\
           reinstall)\n\
             if [ \"$BIFROST_TEST_BREW_MODE\" = fail-all ]; then exit 9; fi\n\
             if [ \"$2\" = --build-from-source ]; then exit 0; fi\n\
             exit 7 ;;\n\
           info)\n\
             if [ \"$BIFROST_TEST_BREW_MODE\" = bad-info ]; then exit 8; fi\n\
             echo '{\"formulae\":[{\"installed\":[{\"version\":\"0.0.155\"}]}]}' ;;\n\
         esac\n",
    )
    .expect("write fake brew");
    fs::write(&git, "#!/bin/sh\nexit 1\n").expect("write fake git");
    fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).expect("chmod brew");
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).expect("chmod git");
    std::env::set_var("PATH", dir.path());

    upgrade_via_homebrew("0.0.156")
        .expect("source-build fallback completes before final CLI verification");
    std::env::set_var("BIFROST_TEST_BREW_MODE", "bad-info");
    upgrade_via_homebrew("0.0.156")
        .expect("unreadable metadata is deferred to final CLI verification");
    std::env::set_var("BIFROST_TEST_BREW_MODE", "fail-all");
    assert!(upgrade_via_homebrew("0.0.156").is_err());
}

#[test]
fn test_install_method_display() {
    assert_eq!(InstallMethod::Homebrew.to_string(), "Homebrew");
    assert_eq!(InstallMethod::Npm.to_string(), "npm");
    assert_eq!(InstallMethod::Pnpm.to_string(), "pnpm");
    assert_eq!(InstallMethod::Script.to_string(), "Install script");
    assert_eq!(
        InstallMethod::Manual(PathBuf::from("/usr/local/bin/bifrost")).to_string(),
        "Manual (/usr/local/bin/bifrost)"
    );
    assert_eq!(InstallMethod::Unknown.to_string(), "Unknown");
}

#[cfg(unix)]
#[test]
fn upgrade_behavior_executes_companion_and_runtime_ownership_branches() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_UPGRADE_BEHAVIOR_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::upgrade::tests::upgrade_behavior_executes_companion_and_runtime_ownership_branches",
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
        super::super::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV,
        super::super::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV,
        WEBVIEW_UPGRADE_ORIGIN_ENV,
        "BIFROST_UPGRADE_TEST_LATEST_VERSION",
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

    let parent_lock = super::super::upgrade_background::try_acquire_upgrade_lock(&data_dir)
        .expect("open parent upgrade lock")
        .expect("own parent upgrade lock");
    std::env::set_var(
        super::super::upgrade_background::PARENT_UPGRADE_LOCK_TOKEN_ENV,
        "forged-token",
    );
    std::env::set_var(
        super::super::upgrade_background::PARENT_UPGRADE_LOCK_OWNER_PID_ENV,
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

#[test]
fn cli_upgrade_restarts_foreground_and_daemon_but_not_desktop_runtime() {
    assert!(cli_owns_runtime_restart(None));
    let daemon = RuntimeInfo::new(
        12345,
        19900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Daemon,
    );
    let desktop = RuntimeInfo::new(
        12346,
        19900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Desktop,
    );
    let foreground = RuntimeInfo::new(
        12347,
        19900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Foreground,
    );
    assert!(cli_owns_runtime_restart(Some(&daemon)));
    assert!(!cli_owns_runtime_restart(Some(&desktop)));
    assert!(cli_owns_runtime_restart(Some(&foreground)));
}

#[test]
fn self_update_accepts_admin_running_proxy_hint_as_a_complete_pair() {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    let cli = Cli::parse_from([
        "bifrost",
        "self-update",
        "--source",
        "admin",
        "--running-proxy-pid",
        "12345",
        "--running-proxy-port",
        "18890",
    ]);
    match cli.command {
        Some(Commands::SelfUpdate {
            running_proxy_pid,
            running_proxy_port,
            ..
        }) => {
            assert_eq!(running_proxy_pid, Some(12345));
            assert_eq!(running_proxy_port, Some(18890));
        }
        _ => panic!("Expected SelfUpdate command"),
    }

    assert!(
        Cli::try_parse_from(["bifrost", "self-update", "--running-proxy-pid", "12345",]).is_err()
    );
    assert_eq!(
        RunningProxyHint::from_parts(Some(12345), Some(18890)),
        Some(RunningProxyHint {
            pid: 12345,
            port: 18890,
        })
    );
    assert_eq!(RunningProxyHint::from_parts(Some(12345), None), None);
}

#[test]
fn running_proxy_hint_requires_exact_live_listener_before_recovering_marker() {
    let hint = RunningProxyHint {
        pid: 12345,
        port: 18890,
    };

    prepare_running_proxy_marker(None).unwrap();
    assert!(prepare_running_proxy_marker_with(
        Some(hint),
        None,
        |_| false,
        |_| panic!("listener lookup must not run for a dead pid"),
        |_| panic!("dead pid must not write runtime markers"),
    )
    .is_err());
    assert!(prepare_running_proxy_marker_with(
        Some(hint),
        None,
        |_| true,
        |_| Some(54321),
        |_| panic!("mismatched listener must not write runtime markers"),
    )
    .is_err());

    let existing = RuntimeInfo::new(
        hint.pid,
        hint.port,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Daemon,
    );
    prepare_running_proxy_marker_with(
        Some(hint),
        Some(existing),
        |_| true,
        |_| None,
        |_| panic!("matching runtime marker must not be rewritten"),
    )
    .unwrap();

    let foreground = RuntimeInfo::new(
        hint.pid,
        hint.port,
        Some(18891),
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Foreground,
    )
    .with_system_proxy(true, "localhost");
    let normalized = std::cell::RefCell::new(None);
    prepare_running_proxy_marker_with(
        Some(hint),
        Some(foreground),
        |_| true,
        |_| Some(hint.pid),
        |runtime| {
            normalized.replace(Some(runtime.clone()));
            Ok(())
        },
    )
    .unwrap();
    let normalized = normalized
        .into_inner()
        .expect("foreground marker normalized for detached restart");
    assert_eq!(normalized.start_mode, RuntimeStartMode::Daemon);
    assert_eq!(normalized.socks5_port, Some(18891));
    assert_eq!(normalized.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(normalized.system_proxy_enabled, Some(true));
    assert_eq!(normalized.system_proxy_bypass.as_deref(), Some("localhost"));

    let recovered = std::cell::RefCell::new(None);
    prepare_running_proxy_marker_with(
        Some(hint),
        None,
        |_| true,
        |_| Some(hint.pid),
        |runtime| {
            recovered.replace(Some(runtime.clone()));
            Ok(())
        },
    )
    .unwrap();
    let recovered = recovered.into_inner().expect("recovered runtime marker");
    assert_eq!(recovered.pid, hint.pid);
    assert_eq!(recovered.port, hint.port);
    assert_eq!(recovered.start_mode, RuntimeStartMode::Daemon);
    assert_eq!(recovered.host.as_deref(), Some("127.0.0.1"));
    assert!(recovered.started_at_ms.is_none());
    assert!(recovered.binary_path.is_none());

    assert!(handle_background_upgrade(
        Some(RunningProxyHint {
            pid: i32::MAX as u32,
            port: 1,
        }),
        None,
    )
    .is_err());
}

#[test]
fn script_installs_use_the_target_aware_atomic_upgrade_path() {
    let source = include_str!("../upgrade.rs");

    assert!(source.contains(
        "InstallMethod::Script => upgrade_manual(&restart_executable, &cache.latest_version)"
    ));
}

mod cli_alias;
mod command_helpers;
mod review_comments;
mod spawn_retry;
mod upgrade_recovery;

#[test]
fn test_glibc_2_38_requires_musl_for_upgrade() {
    assert!(glibc_requires_musl_fallback(Some((2, 38))));
}

#[test]
fn test_glibc_2_39_keeps_gnu_for_upgrade() {
    assert!(!glibc_requires_musl_fallback(Some((2, 39))));
}

#[test]
fn test_unknown_glibc_requires_musl_for_upgrade() {
    assert!(glibc_requires_musl_fallback(None));
}

#[test]
fn test_build_restart_args_with_runtime_info() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 8080,
        socks5_port: Some(1080),
        host: Some("0.0.0.0".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: Some(false),
        system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "8080",
            "--host",
            "0.0.0.0",
            "--socks5-port",
            "1080",
            "--no-system-proxy"
        ]
    );
}

#[test]
fn test_build_restart_args_default_host_skipped() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 19900,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: Some(false),
        system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "19900",
            "--no-system-proxy"
        ]
    );
}

#[test]
fn test_build_restart_args_no_runtime_info_uses_default_config_system_proxy() {
    let default_system_proxy = RestartSystemProxyConfig {
        enabled: true,
        bypass: "localhost,127.0.0.1,::1,*.local".to_string(),
    };
    let args = build_restart_args(
        RestartArgsSource::DefaultConfig,
        None,
        Some(&default_system_proxy),
    );
    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "--system-proxy",
            "--proxy-bypass",
            "localhost,127.0.0.1,::1,*.local"
        ]
    );
}

#[test]
fn test_build_restart_args_no_runtime_info_preserves_disabled_default_config_system_proxy() {
    let default_system_proxy = RestartSystemProxyConfig {
        enabled: false,
        bypass: "localhost,127.0.0.1,::1,*.local".to_string(),
    };
    let args = build_restart_args(
        RestartArgsSource::DefaultConfig,
        None,
        Some(&default_system_proxy),
    );
    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "--no-system-proxy"
        ]
    );
}

#[test]
fn upgrade_restart_port_from_runtime_defaults_to_9900() {
    assert_eq!(restart_ports_from_runtime(None), vec![9900]);
}

#[test]
fn upgrade_restart_ports_from_runtime_uses_runtime_ports() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 18891,
        socks5_port: Some(18892),
        host: Some("0.0.0.0".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
    };

    assert_eq!(restart_ports_from_runtime(Some(&info)), vec![18891, 18892]);
}

#[test]
fn upgrade_restart_executable_uses_install_target_for_manual_install() {
    let target_path = PathBuf::from("/tmp/bifrost-upgrade-target/bin/bifrost");

    assert_eq!(
        restart_executable_for_install_method(&InstallMethod::Manual(target_path.clone()))
            .expect("restart executable"),
        target_path
    );
}

#[test]
fn upgrade_restart_executable_uses_path_for_homebrew_install() {
    assert_eq!(
        restart_executable_for_install_method(&InstallMethod::Homebrew)
            .expect("restart executable"),
        PathBuf::from("bifrost")
    );
}

#[test]
fn test_build_restart_args_no_host() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 8800,
        socks5_port: None,
        host: None,
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: Some(false),
        system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);
    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "8800",
            "--no-system-proxy"
        ]
    );
}

#[test]
fn test_build_restart_args_preserves_system_proxy_snapshot() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 19900,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: Some(false),
        system_proxy_bypass: Some("runtime-bypass-ignored-by-snapshot".to_string()),
    };
    let snapshot = RuntimeSystemProxySnapshot {
        bypass: "localhost,127.0.0.1,*.local".to_string(),
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), Some(&snapshot), None);

    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "19900",
            "--system-proxy",
            "--proxy-bypass",
            "localhost,127.0.0.1,*.local"
        ]
    );
}

#[test]
fn test_build_restart_args_preserves_runtime_system_proxy_request() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 19900,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: Some(true),
        system_proxy_bypass: Some("localhost,127.0.0.1,*.local".to_string()),
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);

    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "19900",
            "--system-proxy",
            "--proxy-bypass",
            "localhost,127.0.0.1,*.local"
        ]
    );
}

#[test]
fn test_build_restart_args_defaults_to_no_system_proxy_for_legacy_runtime() {
    let info = crate::process::RuntimeInfo {
        pid: 12345,
        port: 19900,
        socks5_port: None,
        host: Some("127.0.0.1".to_string()),
        started_at_ms: None,
        start_mode: Default::default(),
        restartable_runtime: false,
        binary_path: None,
        system_proxy_enabled: None,
        system_proxy_bypass: None,
    };

    let args = build_restart_args(RestartArgsSource::Runtime(&info), None, None);

    assert_eq!(
        args,
        vec![
            "start",
            "-d",
            "-y",
            "--skip-cert-check",
            "-p",
            "19900",
            "--no-system-proxy"
        ]
    );
}

#[test]
fn upgrade_download_progress_formats_percent_and_size() {
    let started = Instant::now() - Duration::from_secs(2);
    let line = download_progress_line(512, Some(1024), started);

    assert!(line.contains("50.0%"));
    assert!(line.contains("512 B/1.0 KiB"));
    assert!(line.contains("/s"));
}

#[test]
fn upgrade_github_path_url_joins_mirror_and_release_path() {
    assert_eq!(
        github_path_url(
            "https://ghfast.top/https://github.com/",
            "bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
        ),
        "https://ghfast.top/https://github.com/bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
    );
}

#[test]
fn upgrade_mirror_display_name_hides_full_path() {
    assert_eq!(
        mirror_display_name("https://ghfast.top/https://github.com"),
        "ghfast.top"
    );
}

#[test]
fn upgrade_archive_candidates_prefer_xz_then_keep_gz_compatibility() {
    assert_eq!(
        archive_ext_candidates_for_os("macos", true, false),
        vec!["tar.xz", "tar.gz"]
    );
    assert_eq!(
        archive_ext_candidates_for_os("linux", true, true),
        vec!["tar.gz"]
    );
    assert_eq!(
        archive_ext_candidates_for_os("windows", true, false),
        vec!["zip"]
    );
}

#[test]
fn archive_ext_from_path_accepts_supported_upgrade_archives() {
    assert_eq!(
        archive_ext_from_path(Path::new("bifrost.tar.xz")),
        Some("tar.xz")
    );
    assert_eq!(
        archive_ext_from_path(Path::new("bifrost.tar.gz")),
        Some("tar.gz")
    );
    assert_eq!(archive_ext_from_path(Path::new("bifrost.zip")), Some("zip"));
    assert_eq!(archive_ext_from_path(Path::new("bifrost.gz")), None);
}

#[test]
fn upgrade_archive_validation_rejects_invalid_tar_xz_before_extract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("broken.tar.xz");
    std::fs::write(&archive, b"not an xz archive").expect("write archive");

    assert!(validate_downloaded_archive(&archive, "tar.xz").is_err());
}

#[test]
fn upgrade_install_binary_atomically_replaces_existing_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("new-bifrost");
    let target = dir.path().join("bifrost");
    std::fs::write(&source, b"new binary").expect("write source");
    std::fs::write(&target, b"old binary").expect("write target");

    install_binary_atomically(&source, &target, "0.0.156").expect("install atomically");

    assert_eq!(std::fs::read(&target).expect("read target"), b"new binary");
    assert!(!unique_temp_binary_path(&target).exists());
    assert_eq!(
        std::fs::read(binary_backup_path(&target)).expect("read backup"),
        b"old binary"
    );

    cleanup_binary_backup(&target);
    assert!(!binary_backup_path(&target).exists());
}

#[test]
fn upgrade_restore_binary_backup_restores_previous_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("new-bifrost");
    let target = dir.path().join("bifrost");
    std::fs::write(&source, b"new binary").expect("write source");
    std::fs::write(&target, b"old binary").expect("write target");

    install_binary_atomically(&source, &target, "0.0.156").expect("install atomically");
    assert_eq!(std::fs::read(&target).expect("read target"), b"new binary");

    assert!(restore_binary_backup(&target).expect("restore backup"));
    assert_eq!(std::fs::read(&target).expect("read target"), b"old binary");
    assert!(!binary_backup_path(&target).exists());
}

#[cfg(unix)]
#[test]
fn upgrade_target_version_match_cleans_previous_binary_backup() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("bifrost");
    let backup = binary_backup_path(&target);
    std::fs::write(&target, "#!/bin/sh\necho 'bifrost 0.0.156'\n").expect("write matching target");
    std::fs::write(&backup, "#!/bin/sh\necho 'bifrost 0.0.155'\n").expect("write previous binary");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
        .expect("chmod target");

    verify_installed_cli_target_version_or_restore(&target, "0.0.156")
        .expect("matching target version must pass");
    assert_eq!(
        std::fs::read_to_string(&target).expect("read verified target"),
        "#!/bin/sh\necho 'bifrost 0.0.156'\n"
    );
    assert!(!backup.exists());
}

#[test]
fn upgrade_post_install_skill_messages_cover_all_statuses() {
    assert!(
        post_upgrade_skill_install_message(TimedCommandStatus::Success)
            .contains("installed successfully")
    );
    assert!(
        post_upgrade_skill_install_message(TimedCommandStatus::Failure).contains("retry manually")
    );
    assert!(post_upgrade_skill_install_message(TimedCommandStatus::TimedOut).contains("timed out"));
}

#[test]
fn upgrade_post_install_skill_args_cover_all_supported_tools() {
    assert_eq!(
        POST_UPGRADE_SKILL_INSTALL_ARGS,
        &["install-skill", "--tool", "all", "-y"]
    );
}

#[test]
fn upgrade_desktop_app_install_path_uses_override_dir() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous = std::env::var_os("BIFROST_APP_INSTALL_DIR");
    std::env::set_var("BIFROST_APP_INSTALL_DIR", temp.path());
    let path = desktop_app_install_candidates()
        .into_iter()
        .next()
        .expect("app path");
    match previous {
        Some(value) => std::env::set_var("BIFROST_APP_INSTALL_DIR", value),
        None => std::env::remove_var("BIFROST_APP_INSTALL_DIR"),
    }

    #[cfg(target_os = "macos")]
    assert_eq!(path, temp.path().join("Bifrost.app"));
    #[cfg(target_os = "windows")]
    assert_eq!(path, temp.path().join("bifrost-desktop.exe"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    assert_eq!(path, temp.path().join("Bifrost"));
}
