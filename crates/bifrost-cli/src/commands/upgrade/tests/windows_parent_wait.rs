#[test]
fn deferred_parent_wait_uses_captured_process_and_preserves_timeout() {
    let source = include_str!("../restart.rs");
    let helper = include_str!("../windows_parent_wait.ps1");
    assert!(source.contains("include_str!(\"windows_parent_wait.ps1\")"));
    assert!(source.contains("Wait-UpgradeParentExit $ParentPid 120000"));
    assert!(helper.contains("$parent.WaitForExit($TimeoutMilliseconds)"));
    assert!(helper.contains("$parent.Dispose()"));
    assert_eq!(helper.matches("Get-Process -Id $ParentPid").count(), 1);
    assert!(!source.contains("if (Get-Process -Id $ParentPid"));
}

#[cfg(windows)]
#[test]
fn deferred_parent_wait_handles_retained_exit_and_bounded_live_parent() {
    let dir = tempfile::tempdir().expect("isolated PowerShell fixture");
    let script = dir.path().join("parent-wait.ps1");
    std::fs::write(
        &script,
        concat!(
            "$ErrorActionPreference = 'Stop'\n",
            include_str!("../windows_parent_wait.ps1"),
            include_str!("windows_parent_wait.ps1"),
        ),
    )
    .expect("write parent-wait regression");
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .output()
        .expect("run Windows parent-wait regression");
    assert!(
        output.status.success(),
        "parent-wait regression failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for outcome in [
        "retained-exit-ok",
        "live-timeout-ok",
        "exit-during-wait-ok",
        "missing-parent-ok",
    ] {
        assert!(stdout.contains(outcome), "missing outcome: {outcome}");
    }
}
