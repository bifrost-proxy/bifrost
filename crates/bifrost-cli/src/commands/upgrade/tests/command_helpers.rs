use super::*;

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
