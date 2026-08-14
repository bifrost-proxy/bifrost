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
fn desktop_release_url_test_override_requires_explicit_guard() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let previous_allow = std::env::var_os("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES");
    let previous_url = std::env::var_os("BIFROST_APP_UPGRADE_TEST_URL");
    let override_url = "http://127.0.0.1:12345/desktop.dmg";

    std::env::set_var("BIFROST_APP_UPGRADE_TEST_URL", override_url);
    std::env::remove_var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES");
    match release_asset_url("0.0.161") {
        Ok(url) => assert_ne!(url, override_url),
        Err(error) => assert!(error.to_string().contains("supported on macOS and Windows")),
    }

    std::env::set_var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES", "1");
    assert_eq!(release_asset_url("0.0.161").unwrap(), override_url);

    match previous_allow {
        Some(value) => std::env::set_var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES", value),
        None => std::env::remove_var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES"),
    }
    match previous_url {
        Some(value) => std::env::set_var("BIFROST_APP_UPGRADE_TEST_URL", value),
        None => std::env::remove_var("BIFROST_APP_UPGRADE_TEST_URL"),
    }
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    active_pending.created_at_ms = now_ms.saturating_sub(11 * 60 * 1000);
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
    active_pending.created_at_ms =
        now_ms.saturating_sub(DESKTOP_PENDING_INSTALL_STALE_AFTER_MS + 1);
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
fn pending_desktop_handoff_lock_conflict_preserves_restarting_progress() {
    const CHILD_ENV: &str = "BIFROST_TEST_PENDING_DESKTOP_HANDOFF_CHILD";
    const TEST_NAME: &str =
        "commands::app::tests::pending_desktop_handoff_lock_conflict_preserves_restarting_progress";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env_remove("BIFROST_DATA_DIR")
            .status()
            .expect("spawn isolated pending desktop handoff test");
        assert!(status.success(), "isolated pending handoff test failed");
        return;
    }

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous_data_dir = std::env::var_os("BIFROST_DATA_DIR");
    let previous_parent_token = std::env::var_os(PARENT_UPGRADE_LOCK_TOKEN_ENV);
    let previous_parent_pid = std::env::var_os(PARENT_UPGRADE_LOCK_OWNER_PID_ENV);
    let previous_handoff = std::env::var_os(DESKTOP_UPGRADE_HANDOFF_ENV);
    std::env::set_var("BIFROST_DATA_DIR", temp.path());
    std::env::remove_var(PARENT_UPGRADE_LOCK_TOKEN_ENV);
    std::env::remove_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV);
    std::env::remove_var(DESKTOP_UPGRADE_HANDOFF_ENV);

    let pending = PendingDesktopInstall {
        schema_version: DESKTOP_PENDING_INSTALL_SCHEMA_VERSION,
        created_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64,
        package_path: "Bifrost.msi".to_string(),
        target_version: "0.0.156".to_string(),
        package_owned_by_updater: true,
    };
    fs::write(
        temp.path().join(DESKTOP_PENDING_INSTALL_FILE),
        serde_json::to_vec(&pending).expect("encode pending marker"),
    )
    .expect("write pending marker");
    write_progress(
        temp.path(),
        &UpgradeProgress::new(UpgradePhase::Restarting, "Restart desktop to finish update")
            .with_target(Some("0.0.156".to_string()))
            .with_source(Some("desktop".to_string())),
    );

    let error = acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect_err("pending desktop handoff blocks a second App updater");
    assert!(error.to_string().contains("handoff is already pending"));
    let progress = bifrost_core::upgrade_progress::read_progress(temp.path());
    assert_eq!(progress.phase, UpgradePhase::Restarting);
    assert_eq!(progress.source.as_deref(), Some("desktop"));
    assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));

    match previous_data_dir {
        Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
        None => std::env::remove_var("BIFROST_DATA_DIR"),
    }
    match previous_parent_token {
        Some(value) => std::env::set_var(PARENT_UPGRADE_LOCK_TOKEN_ENV, value),
        None => std::env::remove_var(PARENT_UPGRADE_LOCK_TOKEN_ENV),
    }
    match previous_parent_pid {
        Some(value) => std::env::set_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV, value),
        None => std::env::remove_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV),
    }
    match previous_handoff {
        Some(value) => std::env::set_var(DESKTOP_UPGRADE_HANDOFF_ENV, value),
        None => std::env::remove_var(DESKTOP_UPGRADE_HANDOFF_ENV),
    }
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
    const TEST_NAME: &str = "commands::app::tests::top_level_app_upgrade_owns_the_shared_lock_but_nested_companion_skips_it";
    let role = std::env::var(CHILD_ENV).ok();
    if role.is_none() {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "owner")
            .env_remove("BIFROST_DATA_DIR")
            .status()
            .expect("spawn isolated App upgrade lock test");
        assert!(status.success(), "isolated App upgrade lock test failed");
        return;
    }
    if role.as_deref() == Some("managed-child") {
        assert!(
            crate::commands::upgrade_background::parent_upgrade_lock_is_valid(
                &bifrost_storage::data_dir()
            )
        );
        assert!(
            acquire_top_level_app_upgrade_lock(CALLER_MANAGED_PROGRESS_SOURCE, "0.0.156")
                .expect("validated managed child reuses parent lock")
                .is_none()
        );
        std::env::set_var(DESKTOP_UPGRADE_HANDOFF_ENV, "1");
        assert!(acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
            .expect("validated desktop handoff child reuses parent lock")
            .is_none());
        return;
    }

    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    std::env::remove_var(PARENT_UPGRADE_LOCK_TOKEN_ENV);
    std::env::remove_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV);
    std::env::remove_var(DESKTOP_UPGRADE_HANDOFF_ENV);
    std::env::set_var("BIFROST_DATA_DIR", temp.path());
    let owner = crate::commands::upgrade_background::try_acquire_upgrade_lock(temp.path())
        .expect("open upgrade lock")
        .expect("own upgrade lock");
    bifrost_core::upgrade_progress::write_progress(
        temp.path(),
        &UpgradeProgress::new(UpgradePhase::Downloading, "Tray owner is downloading")
            .with_target(Some("0.0.155".to_string()))
            .with_source(Some("tray".to_string())),
    );

    let error = acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect_err("concurrent top-level App upgrade must be rejected");
    assert!(error.to_string().contains("already running"));
    let progress = bifrost_core::upgrade_progress::read_progress(temp.path());
    assert_eq!(progress.phase, UpgradePhase::Downloading);
    assert_eq!(progress.source.as_deref(), Some("tray"));
    assert_eq!(progress.target_version.as_deref(), Some("0.0.155"));
    assert_eq!(progress.message, "Tray owner is downloading");

    assert!(
        acquire_top_level_app_upgrade_lock(CALLER_MANAGED_PROGRESS_SOURCE, "0.0.156")
            .expect_err("visible source alone cannot bypass the lock")
            .to_string()
            .contains("already running")
    );
    std::env::set_var(PARENT_UPGRADE_LOCK_TOKEN_ENV, "forged-token");
    std::env::set_var(
        PARENT_UPGRADE_LOCK_OWNER_PID_ENV,
        std::process::id().to_string(),
    );
    assert!(
        acquire_top_level_app_upgrade_lock(CALLER_MANAGED_PROGRESS_SOURCE, "0.0.156")
            .expect_err("forged credentials cannot reuse another process's lock")
            .to_string()
            .contains("already running")
    );
    std::env::remove_var(PARENT_UPGRADE_LOCK_TOKEN_ENV);
    std::env::remove_var(PARENT_UPGRADE_LOCK_OWNER_PID_ENV);

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "managed-child")
        .env("BIFROST_DATA_DIR", temp.path())
        .envs(
            crate::commands::upgrade_background::parent_upgrade_lock_child_environment(temp.path()),
        )
        .status()
        .expect("spawn validated managed child");
    assert!(status.success(), "validated managed child failed");

    drop(owner);
    assert!(acquire_top_level_app_upgrade_lock("desktop", "0.0.156")
        .expect("top-level App upgrade acquires released lock")
        .is_some());
}

#[test]
fn direct_app_upgrade_pins_cli_to_the_resolved_app_target() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let data_dir = tempfile::tempdir().expect("isolated upgrade data dir");
    let keys = [
        "BIFROST_DATA_DIR",
        "BIFROST_UPGRADE_TEST_LATEST_VERSION",
        "BIFROST_UPGRADE_TEST_ARCHIVE",
        DESKTOP_MANAGED_SKIP_APP_ENV,
        DESKTOP_MANAGED_SKIP_RESTART_ENV,
    ];
    let previous = keys
        .iter()
        .map(|key| ((*key).to_string(), std::env::var_os(key)))
        .collect::<Vec<_>>();
    std::env::set_var("BIFROST_DATA_DIR", data_dir.path());
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
        let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
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

    let lock_dir = tempfile::tempdir().expect("lock tempdir");
    std::env::set_var("BIFROST_DATA_DIR", lock_dir.path());
    let _owner = crate::commands::upgrade_background::try_acquire_upgrade_lock(lock_dir.path())
        .expect("open parent lock")
        .expect("own parent lock");
    let deferred_status = lock_dir.path().join("deferred.status");
    let command =
        desktop_managed_cli_upgrade_command(Path::new("/tmp/bifrost"), "0.0.156", &deferred_status);
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
        envs.get(DESKTOP_MANAGED_DEFERRED_STATUS_ENV)
            .map(String::as_str),
        Some(deferred_status.to_string_lossy().as_ref())
    );
    let expected_owner_pid = std::process::id().to_string();
    assert_eq!(
        envs.get(PARENT_UPGRADE_LOCK_OWNER_PID_ENV)
            .map(String::as_str),
        Some(expected_owner_pid.as_str())
    );
    assert!(envs
        .get(PARENT_UPGRADE_LOCK_TOKEN_ENV)
        .is_some_and(|token| !token.is_empty()));

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
            Duration::from_secs(10),
        )
        .expect("run fake cli")
        .success());

        let deferred_cli = dir.path().join("deferred-upgrade-bifrost");
        std::fs::write(
            &deferred_cli,
            "#!/bin/sh\nprintf pending > \"$BIFROST_DESKTOP_MANAGED_DEFERRED_STATUS\"\n(sleep 0.15; printf ok > \"$BIFROST_DESKTOP_MANAGED_DEFERRED_STATUS\") &\nexit 0\n",
        )
        .expect("write deferred fake cli");
        std::fs::set_permissions(&deferred_cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod deferred fake cli");
        let started = Instant::now();
        assert!(run_desktop_managed_cli_upgrade(
            &deferred_cli,
            "0.0.156",
            "desktop",
            Duration::from_secs(10),
        )
        .expect("wait for deferred fake CLI helper")
        .success());
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "parent returned before deferred helper terminal status"
        );
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
        let replacement_target = deferred_cli.clone();
        let replacement_source = deferred_cli.with_extension("replacement");
        let replacer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(&replacement_source, "#!/bin/sh\necho 'bifrost 0.0.156'\n")
                .expect("write deferred CLI replacement");
            std::fs::set_permissions(&replacement_source, std::fs::Permissions::from_mode(0o755))
                .expect("chmod deferred CLI replacement");
            std::fs::rename(replacement_source, replacement_target)
                .expect("atomically replace deferred CLI");
        });
        let verification = verify_installed_cli_target_version_with_timeout(
            &deferred_cli,
            "0.0.156",
            Duration::from_secs(10),
        );
        replacer.join().expect("deferred replacer");
        verification.expect("version probe waits for deferred replacement");
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
        let version_timeout = version_timeout.to_string();
        assert!(version_timeout.contains("timed out after"));
        assert!(!version_timeout.contains("timed out after 0 seconds"));

        let deferred_status = dir.path().join("helper.status");
        std::fs::write(&deferred_status, "pending").expect("write pending helper status");
        let status_for_helper = deferred_status.clone();
        let helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(status_for_helper, "ok").expect("finish helper status");
        });
        let mut heartbeat = Instant::now() + Duration::from_secs(10);
        wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(2),
            &mut heartbeat,
        )
        .expect("parent waits for deferred helper terminal state");
        helper.join().expect("helper status writer");

        std::fs::write(&deferred_status, "pending").expect("write heartbeat helper status");
        let status_for_heartbeat = deferred_status.clone();
        let helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            std::fs::write(status_for_heartbeat, "ok").expect("finish heartbeat helper status");
        });
        let mut immediate_heartbeat = Instant::now();
        wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(2),
            &mut immediate_heartbeat,
        )
        .expect("pending helper publishes a waiting heartbeat");
        helper.join().expect("heartbeat helper status writer");
        assert!(immediate_heartbeat > Instant::now());

        std::fs::write(&deferred_status, "error: access denied")
            .expect("write failed helper status");
        let helper_error = wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
        )
        .expect_err("helper failure must propagate without a version-probe timeout");
        assert!(helper_error.to_string().contains("access denied"));

        std::fs::write(&deferred_status, "unexpected").expect("write unknown helper status");
        let unknown_error = wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
        )
        .expect_err("unknown helper status must fail closed");
        assert!(unknown_error.to_string().contains("unknown status"));

        std::fs::write(&deferred_status, "pending:999999").expect("write exited helper status");
        let exited_helper_error = wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
        )
        .expect_err("an exited helper without a terminal status must fail immediately");
        assert!(exited_helper_error
            .to_string()
            .contains("exited without publishing a terminal status"));

        std::fs::write(&deferred_status, "pending:not-a-pid").expect("write invalid helper status");
        let invalid_pid_error = wait_for_desktop_managed_deferred_install_with_mode(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
            true,
            Duration::from_secs(1),
        )
        .expect_err("an invalid helper PID must fail closed");
        assert!(invalid_pid_error
            .to_string()
            .contains("invalid pending status"));

        std::fs::write(&deferred_status, format!("pending:{}", std::process::id()))
            .expect("write live helper status");
        let live_helper_timeout = wait_for_desktop_managed_deferred_install_with_mode(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now(),
            &mut heartbeat,
            true,
            Duration::from_secs(1),
        )
        .expect_err("a live helper still respects the parent deadline");
        assert!(live_helper_timeout.to_string().contains("did not finish"));

        std::fs::write(&deferred_status, "awaiting").expect("write awaiting helper status");
        let handshake_error = wait_for_desktop_managed_deferred_install_with_mode(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
            true,
            Duration::ZERO,
        )
        .expect_err("a legacy helper without a terminal artifact must respect the deadline");
        assert!(handshake_error.to_string().contains("did not finish"));

        let legacy_status = dir.path().join("legacy.status");
        let legacy_ready = dir.path().join("legacy.ok");
        let legacy_log = dir.path().join("legacy.log");
        std::fs::write(&legacy_status, "awaiting").expect("write legacy awaiting status");
        let ready_for_helper = legacy_ready.clone();
        let legacy_helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(ready_for_helper, "ok").expect("write legacy ready marker");
        });
        wait_for_desktop_managed_deferred_install_with_artifacts(
            &legacy_status,
            &legacy_ready,
            &legacy_log,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
            true,
            Duration::ZERO,
            SystemTime::UNIX_EPOCH,
        )
        .expect("legacy helper ready marker completes the wait");
        legacy_helper.join().expect("legacy helper");

        std::fs::remove_file(&legacy_ready).expect("remove legacy ready marker");
        std::fs::write(
            &legacy_log,
            "timestamp waiting\ntimestamp ERROR: access denied by legacy helper\n",
        )
        .expect("write legacy failure log");
        let legacy_error = wait_for_desktop_managed_deferred_install_with_artifacts(
            &legacy_status,
            &legacy_ready,
            &legacy_log,
            "0.0.156",
            "desktop",
            Instant::now() + Duration::from_secs(1),
            &mut heartbeat,
            true,
            Duration::ZERO,
            SystemTime::UNIX_EPOCH,
        )
        .expect_err("legacy helper error must propagate");
        assert!(legacy_error
            .to_string()
            .contains("access denied by legacy helper"));

        std::fs::write(&deferred_status, "pending").expect("write stuck helper status");
        let timeout_error = wait_for_desktop_managed_deferred_install(
            &deferred_status,
            "0.0.156",
            "desktop",
            Instant::now(),
            &mut heartbeat,
        )
        .expect_err("stuck helper must respect the parent deadline");
        assert!(timeout_error.to_string().contains("did not finish"));

        let missing_cli = dir.path().join("missing-bifrost");
        let spawn_status = dir.path().join("spawn.status");
        let spawn_error = run_desktop_managed_cli_upgrade_with_status_path(
            &missing_cli,
            "0.0.156",
            "desktop",
            Duration::from_secs(1),
            spawn_status.clone(),
        )
        .expect_err("missing CLI must fail to spawn");
        assert!(matches!(spawn_error, BifrostError::Io(_)));
        assert!(
            !spawn_status.exists(),
            "failed spawn status must be cleaned"
        );

        let invalid_parent = dir.path().join("not-a-directory");
        std::fs::write(&invalid_parent, "file").expect("write invalid status parent");
        let parent_error = run_desktop_managed_cli_upgrade_with_status_path(
            &cli,
            "0.0.156",
            "desktop",
            Duration::from_secs(1),
            invalid_parent.join("status"),
        )
        .expect_err("status path below a file must fail before spawning");
        assert!(matches!(parent_error, BifrostError::Io(_)));

        let timeout_status = dir.path().join("timeout.status");
        let timeout_error = run_desktop_managed_cli_upgrade_with_status_path(
            &slow_cli,
            "0.0.156",
            "desktop",
            Duration::from_millis(50),
            timeout_status.clone(),
        )
        .expect_err("hung child must time out through explicit status path");
        assert!(timeout_error.to_string().contains("timed out"));
        assert!(!timeout_status.exists(), "timeout status must be cleaned");

        let failed_cli = dir.path().join("failed-bifrost");
        std::fs::write(
            &failed_cli,
            "#!/bin/sh\npending=\"$(dirname \"$0\")/.$(basename \"$0\").pending.$$\"\nprintf replacement > \"$pending\"\nexit 1\n",
        )
        .expect("write failed fake cli");
        std::fs::set_permissions(&failed_cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod failed fake cli");
        let failed_status = dir.path().join("failed.status");
        let failed_child = run_desktop_managed_cli_upgrade_with_status_path(
            &failed_cli,
            "0.0.156",
            "desktop",
            Duration::from_secs(10),
            failed_status.clone(),
        )
        .expect("failed legacy child remains an exit status after cleanup");
        assert!(!failed_child.success());
        assert!(
            !failed_status.exists(),
            "failed child status must be cleaned"
        );
        assert!(
            !std::fs::read_dir(dir.path())
                .expect("read temp dir")
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".failed-bifrost.pending.")),
            "failed legacy child staging must be cleaned by the parent"
        );
        std::env::set_var("PATH", dir.path());
        std::env::set_var("BIFROST_INSTALL_DIR", dir.path());
        upgrade_cli_if_present("desktop", "0.0.156")
            .expect("desktop orchestrator upgrades located CLI");
        std::env::remove_var("BIFROST_INSTALL_DIR");
    }
}

#[test]
fn configured_cli_install_precedes_path_copy() {
    const CHILD_ENV: &str = "BIFROST_TEST_CONFIGURED_CLI_PRIORITY_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::app::tests::configured_cli_install_precedes_path_copy",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated candidate priority test");
        assert!(status.success(), "isolated candidate priority test failed");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let configured_dir = temp.path().join("configured");
    let path_dir = temp.path().join("path");
    fs::create_dir_all(&configured_dir).expect("configured dir");
    fs::create_dir_all(&path_dir).expect("path dir");
    let configured_cli = configured_dir.join(cli_binary_name());
    let path_cli = path_dir.join(cli_binary_name());
    fs::write(&configured_cli, b"configured").expect("configured cli");
    fs::write(&path_cli, b"path").expect("path cli");
    std::env::set_var("BIFROST_INSTALL_DIR", &configured_dir);
    std::env::set_var("PATH", &path_dir);

    assert_eq!(find_standalone_cli_install(), Some(configured_cli));
}

#[cfg(windows)]
#[test]
fn official_windows_cli_precedes_path_copies() {
    let _guard = crate::commands::UPGRADE_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let local_app_data = temp.path().join("local-app-data");
    let cargo_bin = temp.path().join("cargo-bin");
    let previous_local_app_data = std::env::var_os("LOCALAPPDATA");
    let previous_path = std::env::var_os("PATH");
    let previous_install_dir = std::env::var_os("BIFROST_INSTALL_DIR");
    std::env::set_var("LOCALAPPDATA", &local_app_data);
    std::env::set_var("PATH", &cargo_bin);
    std::env::remove_var("BIFROST_INSTALL_DIR");

    let candidates = standalone_cli_candidates();
    assert_eq!(
        candidates[0],
        local_app_data.join("bifrost/bin/bifrost.exe")
    );
    assert_eq!(candidates[2], cargo_bin.join("bifrost.exe"));

    match previous_local_app_data {
        Some(value) => std::env::set_var("LOCALAPPDATA", value),
        None => std::env::remove_var("LOCALAPPDATA"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    match previous_install_dir {
        Some(value) => std::env::set_var("BIFROST_INSTALL_DIR", value),
        None => std::env::remove_var("BIFROST_INSTALL_DIR"),
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
        Duration::from_secs(5),
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
        Duration::from_millis(10),
    )
    .expect_err("hung installer must be terminated");
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn desktop_download_progress_writes_refresh_and_final_line() {
    let mut output = Vec::new();
    write_terminal_download_progress("Downloading… 50.0%", false, &mut output)
        .expect("write refresh");
    write_terminal_download_progress("Downloading… 100.0%", true, &mut output)
        .expect("write final line");
    assert_eq!(
        output,
        b"\rDownloading\xe2\x80\xa6 50.0%\rDownloading\xe2\x80\xa6 100.0%\n"
    );
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
fn windows_desktop_install_transaction_restores_previous_install_on_verification_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let install_dir = temp.path().join("Bifrost");
    let app = install_dir.join("bifrost-desktop.exe");
    fs::create_dir_all(&install_dir).expect("create old install");
    fs::write(&app, "old-app").expect("write old app");
    fs::write(install_dir.join("uninstall.exe"), "old-uninstaller").expect("write old uninstaller");

    let error = run_windows_desktop_install_transaction(&install_dir, || {
        fs::write(&app, "wrong-new-app")?;
        fs::write(install_dir.join("new-sidecar.dll"), "new")?;
        Err::<(), BifrostError>(BifrostError::Config(
            "installed version v0.0.155 instead of v0.0.156".to_string(),
        ))
    })
    .expect_err("wrong installed version must roll back");

    assert!(error.to_string().contains("previous desktop app restored"));
    assert_eq!(
        fs::read_to_string(&app).expect("read restored app"),
        "old-app"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("uninstall.exe")).expect("read restored uninstaller"),
        "old-uninstaller"
    );
    assert!(!install_dir.join("new-sidecar.dll").exists());
}

#[test]
fn windows_desktop_install_transaction_removes_failed_first_install() {
    let temp = tempfile::tempdir().expect("tempdir");
    let install_dir = temp.path().join("Bifrost");

    let error = run_windows_desktop_install_transaction(&install_dir, || {
        fs::create_dir_all(&install_dir)?;
        fs::write(install_dir.join("bifrost-desktop.exe"), "wrong-new-app")?;
        Err::<(), BifrostError>(BifrostError::Config(
            "installed package could not be verified".to_string(),
        ))
    })
    .expect_err("unverified first install must be removed");

    assert!(error.to_string().contains("failed desktop install removed"));
    assert!(!install_dir.exists());
}

#[test]
fn windows_desktop_install_transaction_keeps_verified_install() {
    let temp = tempfile::tempdir().expect("tempdir");
    let install_dir = temp.path().join("Bifrost");
    let app = install_dir.join("bifrost-desktop.exe");
    fs::create_dir_all(&install_dir).expect("create old install");
    fs::write(&app, "old-app").expect("write old app");

    run_windows_desktop_install_transaction(&install_dir, || {
        fs::write(&app, "verified-new-app")?;
        Ok::<(), BifrostError>(())
    })
    .expect("verified install commits");

    assert_eq!(
        fs::read_to_string(&app).expect("read committed app"),
        "verified-new-app"
    );
}

#[test]
fn windows_desktop_install_snapshot_rejects_target_without_parent() {
    let snapshot = WindowsDesktopInstallSnapshot {
        install_dir: PathBuf::new(),
        backup: tempfile::tempdir().expect("backup tempdir"),
        had_previous_install: false,
    };

    let error = snapshot
        .restore()
        .expect_err("parentless install path cannot be restored");
    assert!(error.to_string().contains("has no parent"));
}

#[test]
fn windows_desktop_install_cleanup_removes_stale_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let stale = temp.path().join("failed-upgrade");
    fs::write(&stale, "stale").expect("write stale file");

    remove_path_if_exists(&stale).expect("remove stale file");
    assert!(!stale.exists());
}

#[cfg(unix)]
#[test]
fn windows_desktop_install_transaction_reports_rollback_failure_and_restores_failed_tree() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let install_dir = temp.path().join("Bifrost");
    fs::create_dir_all(&install_dir).expect("create install");
    fs::write(install_dir.join("bifrost-desktop.exe"), "old-app").expect("write old app");
    let backup = tempfile::tempdir().expect("backup tempdir");
    symlink("missing-source", backup.path().join("broken-sidecar"))
        .expect("create broken backup entry");
    let snapshot = WindowsDesktopInstallSnapshot {
        install_dir: install_dir.clone(),
        backup,
        had_previous_install: true,
    };

    let error = finish_windows_desktop_install_transaction::<()>(
        snapshot,
        Err(BifrostError::Config("wrong installed version".to_string())),
    )
    .expect_err("rollback copy failure must be reported");

    assert!(error
        .to_string()
        .contains("failed to restore previous desktop app"));
    assert_eq!(
        fs::read_to_string(install_dir.join("bifrost-desktop.exe"))
            .expect("failed tree restored after rollback error"),
        "old-app"
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

#[test]
fn caller_managed_upgrade_shuts_down_only_a_running_desktop() {
    assert!(should_shutdown_running_desktop_before_install(
        AppOperation::Upgrade,
        false,
        true,
    ));
    assert!(!should_shutdown_running_desktop_before_install(
        AppOperation::Upgrade,
        true,
        true,
    ));
    assert!(!should_shutdown_running_desktop_before_install(
        AppOperation::Upgrade,
        false,
        false,
    ));
    assert!(!should_shutdown_running_desktop_before_install(
        AppOperation::Install,
        false,
        true,
    ));
}

#[test]
fn desktop_shutdown_and_install_failure_orchestration_covers_all_outcomes() {
    let shutdowns = std::cell::Cell::new(0);
    let skipped =
        shutdown_running_desktop_before_install(AppOperation::Install, false, true, || {
            shutdowns.set(shutdowns.get() + 1);
            Ok(())
        })
        .expect("install does not stop an existing desktop");
    assert!(!skipped);
    assert_eq!(shutdowns.get(), 0);

    let stopped =
        shutdown_running_desktop_before_install(AppOperation::Upgrade, false, true, || {
            shutdowns.set(shutdowns.get() + 1);
            Ok(())
        })
        .expect("direct upgrade stops the existing desktop");
    assert!(stopped);
    assert_eq!(shutdowns.get(), 1);

    let shutdown_error =
        shutdown_running_desktop_before_install(AppOperation::Upgrade, false, true, || {
            Err(BifrostError::Config("shutdown failed".to_string()))
        })
        .expect_err("shutdown failure aborts the install");
    assert!(shutdown_error.to_string().contains("shutdown failed"));

    let app = Path::new("/tmp/Bifrost.app");
    let relaunches = std::cell::Cell::new(0);
    restore_desktop_on_install_failure(app, true, Ok(()), |_| {
        relaunches.set(relaunches.get() + 1);
        Ok(())
    })
    .expect("successful install does not need recovery");
    assert_eq!(relaunches.get(), 0);

    let install_error = restore_desktop_on_install_failure(
        app,
        true,
        Err(BifrostError::Config("install failed".to_string())),
        |path| {
            assert_eq!(path, app);
            relaunches.set(relaunches.get() + 1);
            Ok(())
        },
    )
    .expect_err("failed install returns the original error after recovery");
    assert!(install_error.to_string().contains("install failed"));
    assert_eq!(relaunches.get(), 1);
}

#[cfg(unix)]
#[test]
fn app_cli_version_probe_reports_nonzero_exit() {
    let error =
        read_installed_cli_version_with_timeout(Path::new("false"), Duration::from_secs(10))
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
