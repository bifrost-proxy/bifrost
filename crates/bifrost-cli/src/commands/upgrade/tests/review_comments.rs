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
