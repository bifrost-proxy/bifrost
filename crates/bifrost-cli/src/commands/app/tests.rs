use super::*;

#[test]
fn release_asset_name_uses_desktop_prefix_and_target() {
    assert_eq!(
        release_asset_name("0.0.138", DesktopTarget::MacosAarch64),
        "bifrost-desktop-v0.0.138-aarch64-apple-darwin.dmg"
    );
    assert_eq!(
        release_asset_name("0.0.138", DesktopTarget::WindowsX64),
        "bifrost-desktop-v0.0.138-x86_64-pc-windows-msvc.msi"
    );
}

#[test]
fn app_owned_windows_installers_are_deferred_until_desktop_shutdown() {
    assert!(should_defer_desktop_install(
        "desktop",
        Path::new("Bifrost.msi"),
        true,
        true,
    ));
    assert!(should_defer_desktop_install(
        "desktop",
        Path::new("Bifrost.EXE"),
        true,
        true,
    ));
    assert!(!should_defer_desktop_install(
        "cli",
        Path::new("Bifrost.msi"),
        true,
        true,
    ));
    assert!(!should_defer_desktop_install(
        "desktop",
        Path::new("Bifrost.msi"),
        false,
        true,
    ));
    assert!(!should_defer_desktop_install(
        "desktop",
        Path::new("Bifrost.zip"),
        true,
        true,
    ));
    assert!(!should_defer_desktop_install(
        "desktop",
        Path::new("Bifrost.msi"),
        true,
        false,
    ));

    let pending = PendingDesktopInstall {
        schema_version: DESKTOP_PENDING_INSTALL_SCHEMA_VERSION,
        created_at_ms: 123,
        package_path: "Bifrost.msi".to_string(),
        target_version: "0.0.156".to_string(),
        package_owned_by_updater: true,
    };
    let encoded = serde_json::to_string(&pending).expect("encode pending installer");
    assert_eq!(
        serde_json::from_str::<PendingDesktopInstall>(&encoded).expect("decode pending installer"),
        pending
    );
    let caller_owned = serde_json::from_str::<PendingDesktopInstall>(
        r#"{"schema_version":1,"created_at_ms":123,"package_path":"Bifrost.msi","target_version":"0.0.156"}"#,
    )
    .expect("decode legacy caller-owned pending installer");
    assert!(!caller_owned.package_owned_by_updater);
    assert_eq!(
        DESKTOP_PENDING_INSTALL_FILE,
        "desktop-upgrade-pending-install.json"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let mut active_pending = pending;
    active_pending.created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    fs::write(
        temp.path().join(DESKTOP_PENDING_INSTALL_FILE),
        serde_json::to_vec(&active_pending).expect("encode active marker"),
    )
    .expect("write active marker");
    assert!(desktop_pending_install_guard_is_active(temp.path()));
    assert!(
        crate::commands::upgrade_background::try_acquire_upgrade_lock(temp.path())
            .expect("pending handoff is normal contention")
            .is_none(),
        "a CLI/tray updater must not race the deferred desktop installer"
    );
    active_pending.created_at_ms = 1;
    fs::write(
        temp.path().join(DESKTOP_PENDING_INSTALL_FILE),
        serde_json::to_vec(&active_pending).expect("encode stale marker"),
    )
    .expect("write stale marker");
    assert!(!desktop_pending_install_guard_is_active(temp.path()));
    assert!(
        crate::commands::upgrade_background::try_acquire_upgrade_lock(temp.path())
            .expect("stale marker does not error")
            .is_some(),
        "an abandoned marker must not block upgrades forever"
    );

    fs::write(temp.path().join(DESKTOP_PENDING_INSTALL_FILE), b"not-json")
        .expect("write malformed marker");
    assert!(!desktop_pending_install_guard_is_active(temp.path()));

    active_pending.schema_version = DESKTOP_PENDING_INSTALL_SCHEMA_VERSION + 1;
    fs::write(
        temp.path().join(DESKTOP_PENDING_INSTALL_FILE),
        serde_json::to_vec(&active_pending).expect("encode unsupported marker"),
    )
    .expect("write unsupported marker");
    assert!(!desktop_pending_install_guard_is_active(temp.path()));
}

#[cfg(not(target_os = "windows"))]
#[test]
fn non_windows_desktop_install_is_never_deferred() {
    assert!(!should_defer_current_desktop_install(
        "desktop",
        Path::new("Bifrost.msi"),
    ));
}

#[test]
fn nested_cli_upgrade_does_not_publish_terminal_app_progress() {
    assert!(!should_write_app_progress("cli-upgrade"));
    assert!(should_write_app_progress("desktop"));
    assert!(should_write_app_progress("cli"));
    write_app_progress(
        UpgradePhase::Completed,
        "must be ignored",
        Some("0.0.156".to_string()),
        "cli-upgrade",
        None,
        None,
    );
}

#[test]
fn top_level_app_upgrade_owns_the_shared_lock_but_nested_companion_skips_it() {
    const CHILD_ENV: &str = "BIFROST_TEST_APP_UPGRADE_LOCK_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::app::tests::top_level_app_upgrade_owns_the_shared_lock_but_nested_companion_skips_it",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env_remove("BIFROST_DATA_DIR")
            .status()
            .expect("spawn isolated App upgrade lock test");
        assert!(status.success(), "isolated App upgrade lock test failed");
        return;
    }

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
    let previous_parent_lock = std::env::var_os(PARENT_UPGRADE_LOCK_HELD_ENV);
    std::env::remove_var(PARENT_UPGRADE_LOCK_HELD_ENV);
    std::env::set_var("BIFROST_DATA_DIR", temp.path());
    let owner = crate::commands::upgrade_background::try_acquire_upgrade_lock(temp.path())
        .expect("open upgrade lock")
        .expect("own upgrade lock");

    let error = acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect_err("concurrent top-level App upgrade must be rejected");
    assert!(error.to_string().contains("already running"));
    let progress = bifrost_core::upgrade_progress::read_progress(temp.path());
    assert_eq!(progress.phase, UpgradePhase::Failed);
    assert_eq!(progress.source.as_deref(), Some("desktop"));
    assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));

    assert!(
        acquire_top_level_app_upgrade_lock(CALLER_MANAGED_PROGRESS_SOURCE, "0.0.156")
            .expect_err("visible source alone cannot bypass the lock")
            .to_string()
            .contains("already running")
    );
    std::env::set_var(PARENT_UPGRADE_LOCK_HELD_ENV, "1");
    assert!(
        acquire_top_level_app_upgrade_lock(CALLER_MANAGED_PROGRESS_SOURCE, "0.0.156")
            .expect("private managed-child marker bypasses its parent's lock")
            .is_none()
    );
    drop(owner);
    assert!(acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect("top-level App upgrade acquires released lock")
        .is_some());

    match previous_data_dir {
        Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
        None => std::env::remove_var("BIFROST_DATA_DIR"),
    }
    match previous_parent_lock {
        Some(value) => std::env::set_var(PARENT_UPGRADE_LOCK_HELD_ENV, value),
        None => std::env::remove_var(PARENT_UPGRADE_LOCK_HELD_ENV),
    }
}

#[test]
fn direct_app_upgrade_pins_cli_to_the_resolved_app_target() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let keys = [
        "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        "BIFROST_UPGRADE_TEST_ARCHIVE",
        DESKTOP_MANAGED_SKIP_APP_ENV,
        DESKTOP_MANAGED_SKIP_RESTART_ENV,
    ];
    let previous = keys
        .iter()
        .map(|key| ((*key).to_string(), std::env::var_os(key)))
        .collect::<Vec<_>>();
    std::env::set_var("BIFROST_UPGRADE_TEST_LATEST_VERSION", "99.0.0");
    std::env::set_var(
        "BIFROST_UPGRADE_TEST_ARCHIVE",
        std::env::temp_dir().join("missing-pinned-app-upgrade.tar.xz"),
    );
    std::env::remove_var(DESKTOP_MANAGED_SKIP_APP_ENV);
    std::env::remove_var(DESKTOP_MANAGED_SKIP_RESTART_ENV);

    upgrade_cli_if_present("cli", env!("CARGO_PKG_VERSION"))
        .expect("resolved App target overrides a later latest-version observation");

    for (key, value) in previous {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn desktop_managed_cli_upgrade_cannot_reenter_app_or_restart_its_core() {
    const CHILD_ENV: &str = "BIFROST_TEST_DESKTOP_MANAGED_CLI_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::app::tests::desktop_managed_cli_upgrade_cannot_reenter_app_or_restart_its_core",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated desktop-managed CLI test");
        assert!(status.success(), "isolated desktop-managed CLI test failed");
        return;
    }

    let command = desktop_managed_cli_upgrade_command(Path::new("/tmp/bifrost"), "0.0.156");
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["upgrade", "-y"]);
    let envs: std::collections::HashMap<_, _> = command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    assert_eq!(
        envs.get(DESKTOP_MANAGED_SKIP_APP_ENV).map(String::as_str),
        Some("1")
    );
    assert_eq!(
        envs.get(DESKTOP_MANAGED_SKIP_RESTART_ENV)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        envs.get(DESKTOP_MANAGED_TARGET_ENV).map(String::as_str),
        Some("0.0.156")
    );
    assert_eq!(
        envs.get(PARENT_UPGRADE_LOCK_HELD_ENV).map(String::as_str),
        Some("1")
    );

    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = dir.path().join("bifrost");
        std::fs::write(
            &cli,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'bifrost 0.0.156'; fi\nexit 0\n",
        )
        .expect("write fake cli");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");
        assert!(run_desktop_managed_cli_upgrade(
            &cli,
            "0.0.156",
            "desktop",
            Duration::from_secs(2),
        )
        .expect("run fake cli")
        .success());
        verify_installed_cli_target_version(&cli, "0.0.156")
            .expect("fake CLI reports pinned version");
        assert!(verify_installed_cli_target_version_with_timeout(
            &cli,
            "0.0.157",
            Duration::from_millis(80),
        )
        .is_err());
        let deferred_cli = dir.path().join("deferred-bifrost");
        std::fs::write(&deferred_cli, "#!/bin/sh\necho 'bifrost 0.0.155'\n")
            .expect("write old deferred cli");
        std::fs::set_permissions(&deferred_cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod deferred cli");
        let replacement = deferred_cli.clone();
        let replacer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(replacement, "#!/bin/sh\necho 'bifrost 0.0.156'\n")
                .expect("replace deferred cli");
        });
        verify_installed_cli_target_version_with_timeout(
            &deferred_cli,
            "0.0.156",
            Duration::from_secs(2),
        )
        .expect("version probe waits for deferred replacement");
        replacer.join().expect("deferred replacer");
        let slow_cli = dir.path().join("slow-bifrost");
        std::fs::write(&slow_cli, "#!/bin/sh\nexec sleep 2\n").expect("write slow fake cli");
        std::fs::set_permissions(&slow_cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod slow fake cli");
        let timeout_error = run_desktop_managed_cli_upgrade(
            &slow_cli,
            "0.0.156",
            "desktop",
            Duration::from_millis(50),
        )
        .expect_err("hung desktop-managed CLI must time out");
        assert!(timeout_error.to_string().contains("timed out"));
        let version_timeout = verify_installed_cli_target_version_with_timeout(
            &slow_cli,
            "0.0.156",
            Duration::from_millis(50),
        )
        .expect_err("hung CLI version probe must time out");
        assert!(version_timeout.to_string().contains("timed out"));
        std::env::set_var("PATH", dir.path());
        std::env::set_var("BIFROST_INSTALL_DIR", dir.path());
        upgrade_cli_if_present("desktop", "0.0.156")
            .expect("desktop orchestrator upgrades located CLI");
        std::env::remove_var("BIFROST_INSTALL_DIR");
    }
}

#[cfg(unix)]
#[test]
fn desktop_installer_command_has_output_heartbeat_and_timeout() {
    let mut success = Command::new("/bin/sh");
    success.args(["-c", "printf 'mounted'; printf 'note' >&2"]);
    let output = run_desktop_install_command_output_with_timeout(
        success,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
        Duration::from_secs(1),
        Duration::from_millis(10),
    )
    .expect("installer command succeeds");
    assert!(output.status.success());
    assert_eq!(output.stdout, "mounted");
    assert_eq!(output.stderr, "note");

    let mut hung = Command::new("/bin/sh");
    hung.args(["-c", "exec sleep 2"]);
    let error = run_desktop_install_command_output_with_timeout(
        hung,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
        Duration::from_millis(50),
        Duration::from_millis(10),
    )
    .expect_err("hung installer must be terminated");
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn windows_msi_args_force_per_user_install_and_write_log() {
    let package = PathBuf::from(r"C:\Users\eden\AppData\Local\Temp\bifrost desktop.msi");
    let log = PathBuf::from(r"C:\Users\eden\AppData\Local\Temp\bifrost-msi.log");
    let args = windows_msi_install_args(&package, &log);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(args[0], "/i");
    assert_eq!(args[1], package.to_string_lossy());
    assert!(args.iter().any(|arg| arg == "/qn"));
    assert!(args.iter().any(|arg| arg == "/norestart"));
    assert!(args.iter().any(|arg| arg == "ALLUSERS=2"));
    assert!(args.iter().any(|arg| arg == "MSIINSTALLPERUSER=1"));
    assert!(args.iter().any(|arg| arg == "/l*v"));
    assert_eq!(
        args.last().expect("log path argument"),
        &log.to_string_lossy()
    );
}

#[test]
fn windows_msi_uninstall_args_force_per_user_uninstall_and_write_log() {
    let log = PathBuf::from(r"C:\Users\eden\AppData\Local\Temp\bifrost-uninstall.log");
    let args = windows_msi_uninstall_args("{7A327F4B-BA3C-4751-BB9E-AB2796C1224E}", &log);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(args[0], "/x");
    assert_eq!(args[1], "{7A327F4B-BA3C-4751-BB9E-AB2796C1224E}");
    assert!(args.iter().any(|arg| arg == "ALLUSERS=2"));
    assert!(args.iter().any(|arg| arg == "MSIINSTALLPERUSER=1"));
    assert!(args.iter().any(|arg| arg == "/l*v"));
    assert_eq!(
        args.last().expect("log path argument"),
        &log.to_string_lossy()
    );
}

#[test]
fn windows_msi_log_path_sanitizes_package_name() {
    let package = PathBuf::from("bifrost desktop:arm64.msi");
    let log_path = windows_msi_log_path(&package);
    let file_name = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("log file name");

    assert!(file_name.starts_with("bifrost-desktop-msi-"));
    assert!(file_name.ends_with("bifrost_desktop_arm64.msi.log"));
}

#[test]
fn windows_registry_parser_finds_matching_msi_product_code() {
    let reg_output = r#"
HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\{OTHER}
    DisplayName    REG_SZ    Bifrost
    InstallLocation    REG_SZ    C:\Users\eden\AppData\Local\Other\
    UninstallString    REG_SZ    MsiExec.exe /X{11111111-1111-1111-1111-111111111111}

HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\{7A327F4B-BA3C-4751-BB9E-AB2796C1224E}
    DisplayName    REG_SZ    Bifrost
    DisplayVersion    REG_SZ    0.0.139
    InstallLocation    REG_SZ    C:\Users\eden\AppData\Local\Bifrost\
    UninstallString    REG_SZ    MsiExec.exe /X{7A327F4B-BA3C-4751-BB9E-AB2796C1224E}
"#;

    assert_eq!(
        parse_windows_msi_product_code_for_install_dir(
            reg_output,
            "c:\\users\\eden\\appdata\\local\\bifrost"
        ),
        Some("{7A327F4B-BA3C-4751-BB9E-AB2796C1224E}".to_string())
    );
}

#[test]
fn windows_path_normalization_ignores_case_quotes_and_trailing_slash() {
    assert_eq!(
        normalize_windows_path_for_compare(Path::new(r"C:\Users\Eden\AppData\Local\Bifrost\")),
        r"c:\users\eden\appdata\local\bifrost"
    );
    assert_eq!(
        normalize_windows_path_for_compare_str(r#""C:/Users/Eden/AppData/Local/Bifrost/""#),
        r"c:\users\eden\appdata\local\bifrost"
    );
}

#[test]
fn windows_msi_log_summary_prefers_actionable_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = dir.path().join("msi.log");
    fs::write(
        &log,
        "Product: Bifrost -- Installation failed.\n\
             Error 1925. You do not have sufficient privileges.\n\
             Action ended: InstallFinalize. Return value 3.\n",
    )
    .expect("write log");

    assert_eq!(
        read_windows_msi_log_summary(&log),
        Some("MSI detail: Action ended: InstallFinalize. Return value 3.".to_string())
    );
}

#[test]
fn macos_app_path_is_bundle_under_install_dir() {
    let dir = PathBuf::from("/Applications");
    let path = resolve_app_path(&dir);
    if cfg!(target_os = "macos") {
        assert_eq!(path, PathBuf::from("/Applications/Bifrost.app"));
    }
}

#[test]
fn macos_app_dir_from_exe_path_finds_running_bundle_parent() {
    let exe = PathBuf::from("/Users/eden/Applications/Bifrost.app/Contents/Resources/bin/bifrost");
    assert_eq!(
        macos_app_dir_from_exe_path(&exe),
        Some(PathBuf::from("/Users/eden/Applications"))
    );
}

#[test]
fn versions_equal_accepts_optional_v_prefix() {
    assert!(versions_equal("0.0.139", "v0.0.139"));
    assert!(versions_equal(" v0.0.139 ", "0.0.139"));
    assert!(!versions_equal("0.0.138", "0.0.139"));
}

#[test]
fn macos_post_install_version_verification_rejects_stale_bundle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join(MACOS_APP_BUNDLE);
    let contents = app.join("Contents");
    fs::create_dir_all(&contents).expect("create Contents");
    fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key><string>0.0.140</string>
  <key>CFBundleVersion</key><string>140</string>
</dict>
</plist>
"#,
        )
        .expect("write plist");

    let error = verify_installed_desktop_target_version(&app, "0.0.141")
        .expect_err("stale app bundle should be rejected");
    assert!(error.to_string().contains("reports version v0.0.140"));

    let missing_version_app = temp.path().join("MissingVersion.app");
    fs::create_dir_all(&missing_version_app).expect("create app without plist");
    let missing_error = verify_installed_desktop_target_version(&missing_version_app, "0.0.141")
        .expect_err("unverifiable app bundle should be rejected");
    assert!(missing_error
        .to_string()
        .contains("does not report an installed version"));
}

#[test]
fn macos_installed_app_version_reads_info_plist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join(MACOS_APP_BUNDLE);
    let contents = app.join("Contents");
    fs::create_dir_all(&contents).expect("create Contents");
    fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key><string>0.0.139</string>
  <key>CFBundleVersion</key><string>139</string>
</dict>
</plist>
"#,
        )
        .expect("write plist");

    assert_eq!(
        installed_desktop_app_version(&app).as_deref(),
        Some("0.0.139")
    );
    assert!(installed_desktop_app_is_target_version(&app, "v0.0.139"));
}

#[test]
fn macos_app_swap_preserves_old_bundle_when_staging_is_invalid() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join(MACOS_APP_BUNDLE);
    let contents = target.join("Contents");
    fs::create_dir_all(&contents).expect("create old Contents");
    fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.155</string>
</dict></plist>"#,
    )
    .expect("write old plist");
    let invalid_package = temp.path().join("Package.app");
    fs::create_dir_all(invalid_package.join("Contents")).expect("create invalid staged package");

    assert!(copy_dir_replace(
        &invalid_package,
        &target,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .is_err());
    assert_eq!(
        installed_desktop_app_version(&target).as_deref(),
        Some("0.0.155"),
        "the old App remains launchable until a staged target is verified"
    );

    let backup = temp.path().join(format!(".{}.backup", MACOS_APP_BUNDLE));
    fs::rename(&target, &backup).expect("simulate interruption after backup rename");
    assert!(!target.exists());
    assert!(copy_dir_replace(
        &invalid_package,
        &target,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .is_err());
    assert_eq!(
        installed_desktop_app_version(&target).as_deref(),
        Some("0.0.155"),
        "the next attempt restores an interrupted swap before staging"
    );
}

#[test]
fn app_bundle_install_atomically_replaces_verified_target_and_cleans_backup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("package").join(MACOS_APP_BUNDLE);
    let source_contents = source.join("Contents");
    fs::create_dir_all(&source_contents).expect("create source Contents");
    fs::write(
        source_contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.156</string>
</dict></plist>"#,
    )
    .expect("write source plist");
    fs::write(source_contents.join("payload"), "new").expect("write source payload");

    let install_dir = temp.path().join("install");
    let target = install_dir.join(MACOS_APP_BUNDLE);
    let target_contents = target.join("Contents");
    fs::create_dir_all(&target_contents).expect("create old Contents");
    fs::write(
        target_contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.155</string>
</dict></plist>"#,
    )
    .expect("write old plist");

    install_desktop_package(
        &source,
        &install_dir,
        &target,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect("install verified app bundle");

    assert_eq!(
        installed_desktop_app_version(&target).as_deref(),
        Some("0.0.156")
    );
    assert_eq!(
        fs::read_to_string(target.join("Contents/payload")).expect("read payload"),
        "new"
    );
    let backup = install_dir.join(format!(".{}.backup", MACOS_APP_BUNDLE));
    assert!(!backup.exists(), "successful swap must remove its backup");
    copy_dir_replace(&target, &target, "0.0.156", CALLER_MANAGED_PROGRESS_SOURCE)
        .expect("same verified source and target is already complete");

    let no_parent = copy_dir_replace(
        &target,
        Path::new(""),
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect_err("empty target has no parent directory");
    assert!(no_parent.to_string().contains("target has no parent"));
}

#[test]
fn app_owned_upgrade_runs_the_full_verified_package_transaction() {
    const CHILD_ENV: &str = "BIFROST_TEST_APP_PACKAGE_TRANSACTION_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::app::tests::app_owned_upgrade_runs_the_full_verified_package_transaction",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .env(DESKTOP_UPGRADE_HANDOFF_ENV, "1")
                .status()
                .expect("spawn isolated App package transaction test");
        assert!(status.success());
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().join("data");
    let source = temp.path().join("package").join(MACOS_APP_BUNDLE);
    let contents = source.join("Contents");
    fs::create_dir_all(&contents).expect("create package Contents");
    fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.156</string>
</dict></plist>"#,
    )
    .expect("write package plist");
    std::env::set_var("BIFROST_DATA_DIR", &data_dir);

    install_or_upgrade_app(AppInstallRequest {
        operation: AppOperation::Upgrade,
        package: Some(source),
        app_dir: Some(temp.path().join("install")),
        version: Some("0.0.156".to_string()),
        include_cli: false,
        source: Some("desktop".to_string()),
        dry_run: false,
        yes: true,
    })
    .expect("App-owned package transaction");

    let progress = bifrost_core::upgrade_progress::read_progress(&data_dir);
    assert_eq!(progress.phase, UpgradePhase::Restarting);
    assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));
    assert_eq!(progress.source.as_deref(), Some("desktop"));
}

#[cfg(unix)]
#[test]
fn app_owned_upgrade_persists_cli_failure_before_touching_the_app() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_APP_CLI_FAILURE_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::app::tests::app_owned_upgrade_persists_cli_failure_before_touching_the_app",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated App CLI failure test");
        assert!(status.success());
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    let data = temp.path().join("data");
    fs::create_dir_all(&bin).expect("create bin");
    let cli = bin.join("bifrost");
    fs::write(&cli, "#!/bin/sh\nexit 7\n").expect("write failing CLI");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("chmod CLI");
    std::env::set_var("PATH", &bin);
    std::env::set_var("BIFROST_INSTALL_DIR", &bin);
    std::env::set_var("BIFROST_DATA_DIR", &data);

    let error = install_or_upgrade_app(AppInstallRequest {
        operation: AppOperation::Upgrade,
        package: Some(temp.path().join(MACOS_APP_BUNDLE)),
        app_dir: Some(temp.path().join("install")),
        version: Some("0.0.156".to_string()),
        include_cli: true,
        source: Some("desktop".to_string()),
        dry_run: false,
        yes: true,
    })
    .expect_err("CLI failure aborts App-owned transaction");
    assert!(error.to_string().contains("status"));
    let progress = bifrost_core::upgrade_progress::read_progress(&data);
    assert_eq!(progress.phase, UpgradePhase::Failed);
    assert!(progress
        .error
        .as_deref()
        .is_some_and(|error| error.contains("status")));
    assert!(!temp.path().join("install").exists());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn unsupported_native_installers_fail_without_mutating_the_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join(MACOS_APP_BUNDLE);
    let dmg = temp.path().join("bifrost.dmg");
    fs::write(&dmg, "fake").expect("write dmg");
    let error = install_desktop_package(
        &dmg,
        temp.path(),
        &target,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect_err("non-macOS host rejects dmg");
    assert!(error.to_string().contains("only be installed on macOS"));
    assert!(!target.exists());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn unsupported_windows_installers_fail_without_mutating_the_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("bifrost-desktop.exe");
    for extension in ["exe", "msi"] {
        let package = temp.path().join(format!("bifrost.{extension}"));
        fs::write(&package, "fake").expect("write package");
        let error = install_desktop_package(
            &package,
            temp.path(),
            &target,
            "0.0.156",
            CALLER_MANAGED_PROGRESS_SOURCE,
        )
        .expect_err("non-Windows host rejects Windows installer");
        assert!(error.to_string().contains("only be installed on Windows"));
    }
    assert!(!target.exists());
}

#[test]
fn app_upgrade_lock_errors_and_restart_override_are_reported() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let invalid_parent = temp.path().join("not-a-directory");
    fs::write(&invalid_parent, "file").expect("write invalid data parent");
    let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
    let previous_skip_restart = std::env::var_os("BIFROST_APP_SKIP_RESTART");
    std::env::set_var("BIFROST_DATA_DIR", invalid_parent.join("data"));

    let error = acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect_err("invalid data directory must reject App upgrade ownership");
    assert!(error
        .to_string()
        .contains("Failed to acquire the cross-process upgrade lock"));

    std::env::remove_var("BIFROST_APP_SKIP_RESTART");
    assert!(!skip_desktop_restart());
    std::env::set_var("BIFROST_APP_SKIP_RESTART", "yes");
    assert!(skip_desktop_restart());
    std::env::set_var("BIFROST_APP_SKIP_RESTART", "no");
    assert!(!skip_desktop_restart());

    match previous_data_dir {
        Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
        None => std::env::remove_var("BIFROST_DATA_DIR"),
    }
    match previous_skip_restart {
        Some(value) => std::env::set_var("BIFROST_APP_SKIP_RESTART", value),
        None => std::env::remove_var("BIFROST_APP_SKIP_RESTART"),
    }
}

#[cfg(unix)]
#[test]
fn app_cli_version_probe_reports_nonzero_exit() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let cli = temp.path().join("bifrost");
    fs::write(&cli, "#!/bin/sh\nexit 7\n").expect("write failing CLI");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("chmod failing CLI");

    let error = read_installed_cli_version_with_timeout(&cli, Duration::from_secs(10))
        .expect_err("non-zero CLI version probe must fail");
    let message = error.to_string();
    assert!(
        message.contains("status"),
        "expected non-zero exit status error, got: {message}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn caller_managed_app_install_uses_copy_fallback_and_skips_desktop_restart() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_CALLER_MANAGED_APP_INSTALL_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::app::tests::caller_managed_app_install_uses_copy_fallback_and_skips_desktop_restart",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated caller-managed App install test");
        assert!(status.success(), "isolated App install test failed");
        return;
    }

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    let ditto = bin.join("ditto");
    fs::write(&ditto, "#!/bin/sh\nexit 1\n").expect("write failing ditto");
    fs::set_permissions(&ditto, fs::Permissions::from_mode(0o755)).expect("chmod ditto");

    let source = temp.path().join("package").join(MACOS_APP_BUNDLE);
    let contents = source.join("Contents");
    fs::create_dir_all(&contents).expect("create package Contents");
    fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.156</string>
</dict></plist>"#,
    )
    .expect("write package plist");

    let previous_path = std::env::var_os("PATH");
    let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
    let previous_skip_restart = std::env::var_os("BIFROST_APP_SKIP_RESTART");
    std::env::set_var("PATH", &bin);
    std::env::set_var("BIFROST_DATA_DIR", temp.path().join("data"));
    std::env::set_var("BIFROST_APP_SKIP_RESTART", "1");

    let install_dir = temp.path().join("install");
    install_or_upgrade_app(AppInstallRequest {
        operation: AppOperation::Upgrade,
        package: Some(source),
        app_dir: Some(install_dir.clone()),
        version: Some("0.0.156".to_string()),
        include_cli: false,
        source: Some(CALLER_MANAGED_PROGRESS_SOURCE.to_string()),
        dry_run: false,
        yes: true,
    })
    .expect("caller-managed App install");
    assert!(installed_desktop_app_is_target_version(
        &install_dir.join(MACOS_APP_BUNDLE),
        "0.0.156"
    ));

    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    match previous_data_dir {
        Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
        None => std::env::remove_var("BIFROST_DATA_DIR"),
    }
    match previous_skip_restart {
        Some(value) => std::env::set_var("BIFROST_APP_SKIP_RESTART", value),
        None => std::env::remove_var("BIFROST_APP_SKIP_RESTART"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn invalid_dmg_exercises_native_installer_failure_without_mutating_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let package = temp.path().join("invalid.dmg");
    fs::write(&package, "not a dmg").expect("write invalid dmg");
    let target = temp.path().join(MACOS_APP_BUNDLE);

    let error = install_macos_dmg(
        &package,
        temp.path(),
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect_err("invalid dmg must fail to attach");
    assert!(error.to_string().contains("failed to attach dmg"));
    assert!(!target.exists());
}
