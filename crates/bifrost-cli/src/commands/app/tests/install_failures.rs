use super::*;

#[cfg(unix)]
#[test]
fn app_owned_upgrade_persists_cli_failure_before_touching_the_app() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "BIFROST_TEST_APP_CLI_FAILURE_CHILD";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "commands::app::tests::install_failures::app_owned_upgrade_persists_cli_failure_before_touching_the_app",
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
    const CHILD_ENV: &str = "BIFROST_TEST_APP_UPGRADE_LOCK_ERROR_CHILD";
    const TEST_NAME: &str = "commands::app::tests::install_failures::app_upgrade_lock_errors_and_restart_override_are_reported";
    if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
        let temp = tempfile::tempdir().expect("tempdir");
        let invalid_parent = temp.path().join("not-a-directory");
        fs::write(&invalid_parent, "file").expect("write invalid data parent");
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("BIFROST_DATA_DIR", invalid_parent.join("data"))
            .status()
            .expect("spawn isolated App upgrade lock test");
        assert!(status.success(), "isolated App upgrade lock test failed");
        return;
    }

    let previous_skip_restart = std::env::var_os("BIFROST_APP_SKIP_RESTART");

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
                "commands::app::tests::install_failures::caller_managed_app_install_uses_copy_fallback_and_skips_desktop_restart",
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
