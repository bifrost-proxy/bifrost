use super::*;

#[test]
fn test_detect_install_method_returns_valid_variant() {
    let method = detect_install_method();
    match method {
        InstallMethod::Homebrew
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
    assert_eq!(InstallMethod::Script.to_string(), "Install script");
    assert_eq!(
        InstallMethod::Manual(PathBuf::from("/usr/local/bin/bifrost")).to_string(),
        "Manual (/usr/local/bin/bifrost)"
    );
    assert_eq!(InstallMethod::Unknown.to_string(), "Unknown");
}

#[test]
fn test_cli_upgrade_rejects_restart_flag() {
    use crate::cli::Cli;
    use clap::Parser;

    let result = Cli::try_parse_from(["bifrost", "upgrade", "--restart"]);
    assert!(result.is_err(), "--restart should be removed from upgrade");
}

#[test]
fn test_cli_upgrade_hidden_yes_flag_is_accepted() {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    let cli = Cli::parse_from(["bifrost", "upgrade", "-y"]);
    match cli.command {
        Some(Commands::Upgrade { yes }) => {
            assert!(yes);
        }
        _ => panic!("Expected Upgrade command"),
    }
}

#[test]
fn test_cli_upgrade_no_flags() {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    let cli = Cli::parse_from(["bifrost", "upgrade"]);
    match cli.command {
        Some(Commands::Upgrade { yes }) => {
            assert!(!yes);
        }
        _ => panic!("Expected Upgrade command"),
    }
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
        9900,
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

    std::env::set_var(DESKTOP_MANAGED_SKIP_APP_ENV, "yes");
    std::env::set_var(DESKTOP_MANAGED_SKIP_RESTART_ENV, "true");
    assert!(env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));
    assert!(env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV));
    handle_upgrade(true).expect("desktop-managed CLI wrapper");
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
        9900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Daemon,
    );
    let desktop = RuntimeInfo::new(
        12346,
        9900,
        None,
        Some("127.0.0.1".to_string()),
        RuntimeStartMode::Desktop,
    );
    let foreground = RuntimeInfo::new(
        12347,
        9900,
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
fn windows_deferred_install_waits_for_tray_unlock_and_reports_terminal_progress() {
    let source = concat!(include_str!("../upgrade.rs"), include_str!("restart.rs"));
    assert!(source.contains(
        "update_desktop_companion(&restart_executable, &cache.latest_version, behavior)?;"
    ));
    assert!(source.contains("stop_tray_helper_before_windows_deferred_install(&data_dir);"));
    assert!(source.contains("Wait-TargetPathWritable $TargetPath 120"));
    assert!(source.contains("Copy-Item -LiteralPath $TargetPath -Destination $backupPath -Force"));
    assert!(source.contains("installed CLI reports '$versionOutput' instead of target"));
    assert!(source.contains("restored previous CLI after replacement failure"));
    assert!(source.contains("[System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)"));
    assert!(source.contains("Get-Content -LiteralPath $ProgressPath -Raw -Encoding UTF8"));
    assert!(source.contains("target_version = if ($TargetVersion)"));
    assert!(source.contains(".arg(\"-TargetVersion\")"));
    assert!(source.contains(".arg(\"-Source\")"));
    assert!(source.contains("mark_deferred_install_scheduled();"));
    assert!(source.contains("Write-UpgradeProgress \"completed\" \"Upgrade complete\" $null"));
    assert!(source.contains("Write-UpgradeProgress \"failed\" \"Upgrade failed\" $errorMessage"));
}

#[test]
fn script_installs_use_the_target_aware_atomic_upgrade_path() {
    let source = include_str!("../upgrade.rs");

    assert!(source.contains(
        "InstallMethod::Script => upgrade_manual(&restart_executable, &cache.latest_version)"
    ));
}

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
        port: 9900,
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
            "9900",
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
        port: 9900,
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
            "9900",
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
        port: 9900,
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
            "9900",
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
        port: 9900,
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
            "9900",
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

    install_binary_atomically(&source, &target).expect("install atomically");

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

    install_binary_atomically(&source, &target).expect("install atomically");
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

#[cfg(unix)]
#[test]
fn upgrade_target_version_mismatch_restores_previous_binary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("bifrost");
    let backup = binary_backup_path(&target);
    std::fs::write(&target, "#!/bin/sh\necho 'bifrost 9.9.9'\n").expect("write mismatched target");
    std::fs::write(&backup, "#!/bin/sh\necho 'bifrost 0.0.155'\n").expect("write previous binary");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
        .expect("chmod target");
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o755))
        .expect("chmod backup");

    let error = verify_installed_cli_target_version_or_restore(&target, "0.0.156")
        .expect_err("wrong target version must fail");
    assert!(error.to_string().contains("instead of target v0.0.156"));
    assert_eq!(
        std::fs::read_to_string(&target).expect("read restored target"),
        "#!/bin/sh\necho 'bifrost 0.0.155'\n"
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
fn upgrade_post_install_desktop_app_args_disable_cli_recursion() {
    assert_eq!(
        post_upgrade_desktop_app_args("0.0.145", Some(Path::new("/Users/test/Applications"))),
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

#[test]
fn upgrade_desktop_app_failure_reason_prefers_stderr() {
    let output = TimedCommandOutput {
        status: TimedCommandStatus::Failure,
        stdout: "stdout detail".to_string(),
        stderr: "stderr detail".to_string(),
    };

    assert_eq!(summarize_command_output(&output), "stderr detail");
}

#[test]
#[cfg(unix)]
fn upgrade_command_status_with_timeout_reports_success_and_failure() {
    assert_eq!(
        command_status_with_timeout(
            Path::new("/bin/sh"),
            &["-c", "exit 0"],
            Duration::from_secs(1)
        )
        .unwrap(),
        TimedCommandStatus::Success
    );

    assert_eq!(
        command_status_with_timeout(
            Path::new("/bin/sh"),
            &["-c", "exit 7"],
            Duration::from_secs(1)
        )
        .unwrap(),
        TimedCommandStatus::Failure
    );

    assert_eq!(
        command_status_with_timeout_and_heartbeat(
            Path::new("/bin/sh"),
            &["-c", "sleep 0.08"],
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .unwrap(),
        TimedCommandStatus::Success
    );

    let output = command_output_with_timeout_and_heartbeat(
        Path::new("/bin/sh"),
        &["-c".to_string(), "sleep 0.08; echo ready".to_string()],
        Duration::from_secs(1),
        Duration::from_millis(10),
    )
    .unwrap();
    assert_eq!(output.status, TimedCommandStatus::Success);
    assert_eq!(output.stdout.trim(), "ready");
}

#[test]
#[cfg(unix)]
fn upgrade_command_status_with_timeout_does_not_block_on_hung_child() {
    let started = Instant::now();
    let status = command_status_with_timeout(
        Path::new("/bin/sh"),
        &["-c", "sleep 5"],
        Duration::from_millis(50),
    )
    .unwrap();

    assert_eq!(status, TimedCommandStatus::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "timeout helper should return promptly"
    );
}

#[test]
#[cfg(unix)]
fn installed_cli_version_must_match_the_pinned_target() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let previous_archive = std::env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE");
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing");
    std::env::set_var(
        "BIFROST_UPGRADE_TEST_ARCHIVE",
        temp.path().join("fixture.tar.xz"),
    );
    verify_installed_cli_target_version(&missing, "0.0.156")
        .expect("explicit test archive bypasses post-install probe");
    std::env::remove_var("BIFROST_UPGRADE_TEST_ARCHIVE");
    assert!(verify_installed_cli_target_version(&missing, "0.0.156").is_err());
    let matching = temp.path().join("matching");
    let stale = temp.path().join("stale");
    let failing = temp.path().join("failing");
    fs::write(&matching, "#!/bin/sh\necho 'bifrost 0.0.156'\n").unwrap();
    fs::write(&stale, "#!/bin/sh\necho 'bifrost 0.0.155'\n").unwrap();
    fs::write(&failing, "#!/bin/sh\necho broken >&2\nexit 7\n").unwrap();
    fs::set_permissions(&matching, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();

    verify_installed_cli_target_version(&matching, "0.0.156")
        .expect("matching installed CLI version");
    assert!(verify_installed_cli_target_version(&stale, "0.0.156").is_err());
    assert!(verify_installed_cli_target_version(&failing, "0.0.156").is_err());

    if let Some(value) = previous_archive {
        std::env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", value);
    }
}

#[test]
fn upgrade_download_tuning_parses_positive_values() {
    let tuning = DownloadTuning {
        connect_timeout_secs: parse_positive_u64(Some("7"), DOWNLOAD_CONNECT_TIMEOUT_SECS),
        download_timeout_secs: parse_positive_u64(Some("90"), DOWNLOAD_TIMEOUT_SECS),
        mirror_probe_timeout_secs: parse_positive_u64(Some("3"), MIRROR_PROBE_TIMEOUT_SECS),
        download_tries: parse_positive_usize(Some("4"), DOWNLOAD_TRIES),
    };

    assert_eq!(
        tuning,
        DownloadTuning {
            connect_timeout_secs: 7,
            download_timeout_secs: 90,
            mirror_probe_timeout_secs: 3,
            download_tries: 4,
        }
    );
}

#[test]
fn upgrade_download_tuning_rejects_invalid_values() {
    assert_eq!(parse_positive_u64(Some("0"), 5), 5);
    assert_eq!(parse_positive_u64(Some("abc"), 5), 5);
    assert_eq!(parse_positive_usize(Some("0"), 2), 2);
    assert_eq!(parse_positive_usize(Some("abc"), 2), 2);
}

#[test]
fn parse_positive_u64_parses_and_trims() {
    assert_eq!(parse_positive_u64(Some("42"), 7), 42);
    assert_eq!(parse_positive_u64(Some(" 5 "), 7), 5);
}

#[test]
fn parse_positive_u64_uses_default_for_zero_invalid_and_none() {
    assert_eq!(parse_positive_u64(Some("0"), 7), 7);
    assert_eq!(parse_positive_u64(Some("abc"), 7), 7);
    assert_eq!(parse_positive_u64(None, 7), 7);
}

#[test]
fn parse_positive_usize_parses_and_trims() {
    assert_eq!(parse_positive_usize(Some("3"), 1), 3);
    assert_eq!(parse_positive_usize(Some(" 8 "), 1), 8);
}

#[test]
fn parse_positive_usize_uses_default_for_zero_invalid_and_none() {
    assert_eq!(parse_positive_usize(Some("0"), 2), 2);
    assert_eq!(parse_positive_usize(Some("zzz"), 2), 2);
    assert_eq!(parse_positive_usize(None, 2), 2);
}

#[test]
fn musl_fallback_triple_is_defined_for_gnu_targets() {
    assert_eq!(
        get_musl_fallback_triple("x86_64-unknown-linux-gnu").as_deref(),
        Some("x86_64-unknown-linux-musl")
    );
    assert_eq!(
        get_musl_fallback_triple("aarch64-unknown-linux-gnu").as_deref(),
        Some("aarch64-unknown-linux-musl")
    );
}

#[test]
fn musl_fallback_triple_is_none_for_other_targets() {
    assert!(get_musl_fallback_triple("x86_64-apple-darwin").is_none());
}

#[test]
fn human_bytes_formats_small_values() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(1), "1 B");
    assert_eq!(human_bytes(1023), "1023 B");
}

#[test]
fn human_bytes_formats_larger_units() {
    assert_eq!(human_bytes(1024), "1.0 KiB");
    assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
}

#[test]
fn download_progress_line_without_total_omits_percentage() {
    let started = Instant::now() - Duration::from_secs(1);
    let line = download_progress_line(2048, None, started);
    assert!(line.contains("Downloading…"));
    assert!(line.contains("2.0 KiB"));
    assert!(!line.contains("%"));
}

fn with_mirror_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let key = "BIFROST_GITHUB_MIRROR";
    let prev = std::env::var(key).ok();
    match value {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    let result = f();
    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    result
}

#[test]
fn github_mirror_bases_respects_preferred_env() {
    with_mirror_env(Some("https://example.com/github"), || {
        let bases = github_mirror_bases();
        assert!(!bases.is_empty());
        assert_eq!(bases[0], "https://example.com/github");
        assert!(bases.iter().any(|b| b.contains("github.com")));
    });
}

#[test]
fn mirror_display_name_strips_scheme_and_path() {
    assert_eq!(
        mirror_display_name("https://ghfast.top/https://github.com"),
        "ghfast.top"
    );
    assert_eq!(mirror_display_name("http://foo.bar/"), "foo.bar");
    assert_eq!(mirror_display_name("plain-host"), "plain-host");
}

#[test]
fn github_path_url_normalizes_slashes() {
    assert_eq!(
        github_path_url("https://github.com/", "/owner/repo/releases"),
        "https://github.com/owner/repo/releases"
    );
    assert_eq!(
        github_path_url("https://github.com", "owner/repo"),
        "https://github.com/owner/repo"
    );
}

#[test]
fn version_comparison_is_newer_version_behaviour() {
    assert!(is_newer_version("0.0.1", "0.0.2"));
    assert!(!is_newer_version("0.1.0", "0.1.0"));
    assert!(!is_newer_version("0.2.0", "0.1.9"));
}
