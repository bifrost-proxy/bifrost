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
        handle_upgrade(true).is_err(),
        "forged owner credentials must not bypass lock"
    );
    std::env::set_var(DESKTOP_MANAGED_SKIP_APP_ENV, "yes");
    std::env::set_var(DESKTOP_MANAGED_SKIP_RESTART_ENV, "true");
    std::env::set_var(DESKTOP_MANAGED_TARGET_ENV, env!("CARGO_PKG_VERSION"));
    assert!(env_flag(DESKTOP_MANAGED_SKIP_APP_ENV));
    assert!(env_flag(DESKTOP_MANAGED_SKIP_RESTART_ENV));
    assert!(
        handle_upgrade(true).is_err(),
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

mod review_comments;
mod spawn_retry;

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
            // Other tests temporarily replace the process-wide PATH. Use an
            // absolute executable so parallel runs cannot make this probe fail.
            &["-c", "/bin/sleep 0.08"],
            Duration::from_secs(1),
            Duration::from_millis(10),
            false,
        )
        .unwrap(),
        TimedCommandStatus::Success
    );

    let output = command_output_with_timeout_and_heartbeat(
        Path::new("/bin/sh"),
        &["-c".to_string(), "/bin/sleep 0.08; echo ready".to_string()],
        Duration::from_secs(1),
        Duration::from_millis(10),
    )
    .unwrap();
    assert_eq!(output.status, TimedCommandStatus::Success);
    assert_eq!(output.stdout.trim(), "ready");

    let handoff_output = command_output_with_timeout_and_env(
        Path::new("/bin/sh"),
        &[
            "-c".to_string(),
            format!("test \"${DESKTOP_UPGRADE_HANDOFF_ENV}\" = 1"),
        ],
        Duration::from_secs(1),
        Duration::from_millis(10),
        &[(DESKTOP_UPGRADE_HANDOFF_ENV, "1")],
        None,
    )
    .unwrap();
    assert_eq!(handoff_output.status, TimedCommandStatus::Success);
}

#[test]
#[cfg(unix)]
fn upgrade_command_status_with_timeout_does_not_block_on_hung_child() {
    let started = Instant::now();
    let status = command_status_with_timeout(
        Path::new("/bin/sh"),
        &["-c", "/bin/sleep 5"],
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

    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing");
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
fn ordered_download_bases_without_preferred_env_keeps_fallbacks() {
    with_mirror_env(None, || {
        let tuning = DownloadTuning {
            connect_timeout_secs: 1,
            download_timeout_secs: 1,
            mirror_probe_timeout_secs: 1,
            download_tries: 1,
        };
        let bases = ordered_download_bases("nonexistent/coverage-fixture", tuning);
        assert!(!bases.is_empty());
        assert!(bases.iter().any(|base| base.contains("github.com")));
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

#[cfg(unix)]
#[test]
fn full_manual_upgrade_uses_the_pinned_archive_and_verified_finish_path() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let target_triple = get_target_triple().expect("supported test target");
    let version = "99.0.1";
    let archive_root = temp
        .path()
        .join(format!("bifrost-v{version}-{target_triple}"));
    fs::create_dir_all(&archive_root).expect("create archive root");
    let archived_binary = archive_root.join("bifrost");
    fs::write(
        &archived_binary,
        format!("#!/bin/sh\necho 'bifrost {version}'\n"),
    )
    .expect("write archived CLI");
    fs::set_permissions(&archived_binary, fs::Permissions::from_mode(0o755))
        .expect("chmod archived CLI");
    let archive = temp.path().join("bifrost.tar.gz");
    let status = Command::new("tar")
        .args(["-czf"])
        .arg(&archive)
        .arg("-C")
        .arg(temp.path())
        .arg(archive_root.file_name().expect("archive root name"))
        .status()
        .expect("create fixture archive");
    assert!(status.success());

    let install_target = temp.path().join("installed-bifrost");
    fs::write(&install_target, "#!/bin/sh\necho 'bifrost 0.0.1'\n").expect("write old CLI");
    fs::set_permissions(&install_target, fs::Permissions::from_mode(0o755)).expect("chmod old CLI");
    let previous_archive = std::env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE");
    let previous_target = std::env::var_os(UPGRADE_TEST_INSTALL_TARGET_ENV);
    std::env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", &archive);
    std::env::set_var(UPGRADE_TEST_INSTALL_TARGET_ENV, &install_target);

    handle_upgrade_inner(
        UpgradeBehavior::interactive(true, true),
        Some(version.to_string()),
    )
    .expect("pinned manual upgrade");
    assert!(fs::read_to_string(&install_target)
        .expect("read installed CLI")
        .contains(version));
    assert!(!binary_backup_path(&install_target).exists());

    match previous_archive {
        Some(value) => std::env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", value),
        None => std::env::remove_var("BIFROST_UPGRADE_TEST_ARCHIVE"),
    }
    match previous_target {
        Some(value) => std::env::set_var(UPGRADE_TEST_INSTALL_TARGET_ENV, value),
        None => std::env::remove_var(UPGRADE_TEST_INSTALL_TARGET_ENV),
    }
}

#[test]
fn restart_and_download_helpers_cover_terminal_paths() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
    let previous_archive = std::env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE");
    std::env::set_var("BIFROST_DATA_DIR", temp.path());

    wait_for_restart_ports_release(&[]).expect("no restart ports");
    default_restart_system_proxy_config().expect("default proxy config");
    assert!(!release_archive_ext_candidates().is_empty());
    let _ = tar_supports_xz();
    assert!(test_upgrade_archive_override()
        .expect("missing archive override")
        .is_none());
    std::env::set_var(
        "BIFROST_UPGRADE_TEST_ARCHIVE",
        temp.path().join("invalid.txt"),
    );
    assert!(test_upgrade_archive_override().is_err());

    let tuning = DownloadTuning {
        connect_timeout_secs: 1,
        download_timeout_secs: 1,
        mirror_probe_timeout_secs: 1,
        download_tries: 1,
    };
    assert!(!probe_github_url("http://127.0.0.1:9/not-running", tuning));
    assert!(download_file_with_progress(
        "http://127.0.0.1:9/not-running",
        &temp.path().join("download"),
        tuning,
    )
    .is_err());
    print_update_info(
        "0.0.155",
        &VersionCache {
            latest_version: "0.0.156".to_string(),
            release_highlights: vec!["restart ownership".to_string()],
            checked_at: chrono::Utc::now(),
        },
    );

    match previous_data_dir {
        Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
        None => std::env::remove_var("BIFROST_DATA_DIR"),
    }
    match previous_archive {
        Some(value) => std::env::set_var("BIFROST_UPGRADE_TEST_ARCHIVE", value),
        None => std::env::remove_var("BIFROST_UPGRADE_TEST_ARCHIVE"),
    }
}

#[test]
fn download_selection_success_and_free_restart_port_are_exercised() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous_mirror = std::env::var_os("BIFROST_GITHUB_MIRROR");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("read fixture request");
            let request = String::from_utf8_lossy(&request[..read]);
            let body = if request.starts_with("HEAD ") {
                ""
            } else {
                "fixture"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
        }
    });
    let base = format!("http://{address}");
    std::env::set_var("BIFROST_GITHUB_MIRROR", &base);
    let tuning = DownloadTuning {
        connect_timeout_secs: 1,
        download_timeout_secs: 2,
        mirror_probe_timeout_secs: 1,
        download_tries: 1,
    };
    assert_eq!(
        select_fastest_github_base_from(
            vec!["http://127.0.0.1:9".to_string(), base.clone()],
            "fixture",
            tuning,
        )
        .as_deref(),
        Some(base.as_str())
    );
    assert_eq!(
        ordered_download_bases_from(
            vec!["http://127.0.0.1:9".to_string(), base.clone()],
            "fixture",
            tuning,
        )
        .first(),
        Some(&base)
    );
    assert_eq!(
        ordered_download_bases("fixture", tuning).first(),
        Some(&base)
    );
    let output = temp.path().join("downloaded");
    download_file_with_progress(&format!("{base}/fixture"), &output, tuning)
        .expect("download fixture");
    assert_eq!(fs::read_to_string(output).expect("read fixture"), "fixture");
    server.join().expect("fixture server");
    let free_listener = TcpListener::bind("127.0.0.1:0").expect("bind free port fixture");
    let free_port = free_listener.local_addr().expect("free port").port();
    drop(free_listener);
    wait_for_restart_ports_release(&[free_port]).expect("released fixture port");
    match previous_mirror {
        Some(value) => std::env::set_var("BIFROST_GITHUB_MIRROR", value),
        None => std::env::remove_var("BIFROST_GITHUB_MIRROR"),
    }
}
