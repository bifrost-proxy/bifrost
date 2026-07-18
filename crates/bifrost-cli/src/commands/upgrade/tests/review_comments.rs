use super::*;

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
        "Copy-Item -LiteralPath $TargetPath -Destination $backupPath -Force",
        "installed CLI reports '$versionOutput' instead of target",
        "restored previous CLI after replacement failure",
        "[System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)",
        "Get-Content -LiteralPath $ProgressPath -Raw -Encoding UTF8",
        "target_version = if ($TargetVersion)",
        "target_version: _target_version.to_string()",
        ".arg(\"-TargetVersion\")",
        ".arg(&deferred_install.target_version)",
        ".arg(\"-Source\")",
        "if ($PublishProgress -eq 0)",
        ".arg(\"-PublishProgress\")",
        "mark_deferred_install_scheduled();",
        "Write-UpgradeProgress \"completed\" \"Upgrade complete\" $null",
        "Write-UpgradeProgress \"failed\" \"Upgrade failed\" $errorMessage",
    ] {
        assert!(source.contains(contract), "missing contract: {contract}");
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
        desktop_companion_mode(true, true),
        DesktopCompanionMode::DesktopHandoff
    );
    assert_eq!(
        desktop_companion_mode(true, false),
        DesktopCompanionMode::CallerManaged
    );
    assert_eq!(
        desktop_companion_mode(false, true),
        DesktopCompanionMode::CallerManaged
    );
    assert!(windows_paths_match(
        Path::new(r"C:\Users\Eden\Bifrost.exe"),
        Path::new("c:/users/eden/bifrost.exe"),
    ));
}
