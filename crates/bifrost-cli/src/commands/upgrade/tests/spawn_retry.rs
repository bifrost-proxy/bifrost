use super::*;

#[cfg(unix)]
#[test]
fn upgrade_target_version_mismatch_restores_previous_binary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("bifrost");
    let backup = binary_backup_path(&target);
    // Use a native executable instead of a shell-script fixture. Under LLVM
    // coverage's parallel test load, the script occasionally failed before it
    // could print its version, so the test exercised the generic command-error
    // rollback branch rather than the intended successful-command mismatch.
    // `/usr/bin/true --version` exits successfully on both macOS and Linux and
    // can never report the pinned Bifrost target version.
    std::fs::copy("/usr/bin/true", &target).expect("copy mismatched executable");
    std::fs::write(&backup, "#!/bin/sh\necho 'bifrost 0.0.155'\n").expect("write previous binary");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
        .expect("chmod target");
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o755))
        .expect("chmod backup");

    let error = verify_installed_cli_target_version_or_restore(&target, "0.0.156")
        .expect_err("wrong target version must fail");
    assert!(
        error.to_string().contains("instead of target v0.0.156"),
        "unexpected verification error: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read restored target"),
        "#!/bin/sh\necho 'bifrost 0.0.155'\n"
    );
    assert!(!backup.exists());
}

#[test]
#[cfg(unix)]
fn upgrade_command_spawn_retries_text_file_busy_then_succeeds() {
    let mut attempts = 0;
    let result = spawn_upgrade_command_with_retry(|| {
        attempts += 1;
        if attempts < 3 {
            Err(std::io::Error::from_raw_os_error(
                TEXT_FILE_BUSY_RAW_OS_ERROR,
            ))
        } else {
            Ok("spawned")
        }
    });

    assert_eq!(
        result.expect("transient executable busy should recover"),
        "spawned"
    );
    assert_eq!(attempts, 3);
}

#[test]
fn upgrade_command_spawn_does_not_retry_non_transient_error() {
    let mut attempts = 0;
    let error = spawn_upgrade_command_with_retry(|| -> std::io::Result<()> {
        attempts += 1;
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing upgrade command",
        ))
    })
    .expect_err("non-transient spawn error must be returned");

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(attempts, 1);
}

#[test]
#[cfg(unix)]
fn upgrade_command_spawn_stops_after_text_file_busy_retry_limit() {
    let mut attempts = 0;
    let error = spawn_upgrade_command_with_retry(|| -> std::io::Result<()> {
        attempts += 1;
        Err(std::io::Error::from_raw_os_error(
            TEXT_FILE_BUSY_RAW_OS_ERROR,
        ))
    })
    .expect_err("persistent executable busy must be returned");

    assert_eq!(error.raw_os_error(), Some(TEXT_FILE_BUSY_RAW_OS_ERROR));
    assert_eq!(attempts, UPGRADE_COMMAND_SPAWN_MAX_ATTEMPTS);
}
