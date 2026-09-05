use super::*;

#[test]
fn desktop_child_progress_watch_accepts_only_fresh_matching_activity() {
    use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};

    let temp = tempfile::tempdir().expect("tempdir");
    let mut watch = ChildProgressWatch::new(temp.path(), "0.0.162", "desktop");
    assert!(
        !watch.observe_activity(),
        "missing progress is not activity"
    );
    fs::write(
        bifrost_core::upgrade_progress::progress_file_path(temp.path()),
        "{not-json",
    )
    .expect("write invalid progress");
    assert!(
        !watch.observe_activity(),
        "invalid progress is not child activity"
    );

    let mut matching = UpgradeProgress::new(UpgradePhase::Downloading, "42%")
        .with_target(Some("0.0.162".to_string()))
        .with_source(Some("desktop".to_string()));
    matching.updated_at = "2026-07-24T10:00:00Z".to_string();
    write_progress(temp.path(), &matching);
    assert!(watch.observe_activity());
    assert!(
        !watch.observe_activity(),
        "the same progress record cannot extend the stall deadline repeatedly"
    );

    let mut wrong_source = matching.clone();
    wrong_source.source = Some("admin".to_string());
    wrong_source.updated_at = "2026-07-24T10:00:01Z".to_string();
    write_progress(temp.path(), &wrong_source);
    assert!(!watch.observe_activity());

    matching.updated_at = "2026-07-24T10:00:02Z".to_string();
    write_progress(temp.path(), &matching);
    assert!(watch.observe_activity());

    let seeded = tempfile::tempdir().expect("seeded tempdir");
    write_progress(seeded.path(), &matching);
    let mut seeded_watch = ChildProgressWatch::new(seeded.path(), "0.0.162", "desktop");
    assert!(
        !seeded_watch.observe_activity(),
        "progress that predates the child cannot extend its stall deadline"
    );
    matching.updated_at = "2026-07-24T10:00:02.500Z".to_string();
    write_progress(seeded.path(), &matching);
    assert!(seeded_watch.observe_activity());

    matching.phase = UpgradePhase::Completed;
    matching.updated_at = "2026-07-24T10:00:03Z".to_string();
    write_progress(temp.path(), &matching);
    assert!(
        !watch.observe_activity(),
        "terminal progress is not activity"
    );
}

#[test]
#[cfg(unix)]
fn desktop_child_progress_extends_the_stall_deadline() {
    use bifrost_core::upgrade_progress::{write_progress, UpgradePhase, UpgradeProgress};

    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().to_path_buf();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        let mut progress = UpgradeProgress::new(UpgradePhase::Downloading, "still moving")
            .with_target(Some("0.0.162".to_string()))
            .with_source(Some("desktop".to_string()));
        progress.updated_at = "2026-07-24T10:00:04Z".to_string();
        write_progress(&data_dir, &progress);
    });
    let output = command_output_with_timeout_and_env_inner(
        Path::new("/bin/sleep"),
        &["1.2".to_string()],
        Duration::from_secs(1),
        Duration::from_millis(25),
        &[("PATH", "/nonexistent")],
        None,
        ChildActivityWatch::StructuredProgress(ChildProgressWatch::new(
            temp.path(),
            "0.0.162",
            "desktop",
        )),
    )
    .expect("child command result");
    writer.join().expect("progress writer");
    assert_eq!(output.status, TimedCommandStatus::Success, "{output:?}");
}

#[test]
#[cfg(unix)]
fn desktop_child_streaming_wrapper_selects_structured_progress_watch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = command_output_with_timeout_and_env_streaming(
        Path::new("/bin/sh"),
        &["-c".to_string(), "true".to_string()],
        Duration::from_secs(1),
        Duration::from_millis(25),
        &[],
        None,
        Some(ChildProgressWatch::new(temp.path(), "0.0.162", "desktop")),
    )
    .expect("structured progress wrapper command");

    assert_eq!(output.status, TimedCommandStatus::Success);
}

#[test]
#[cfg(unix)]
fn desktop_child_streamed_output_extends_the_stall_deadline() {
    let output = command_output_with_timeout_and_env_inner(
        Path::new("/bin/sh"),
        &[
            "-c".to_string(),
            "sleep 0.6; printf moving; sleep 0.6".to_string(),
        ],
        Duration::from_secs(1),
        Duration::from_millis(25),
        &[],
        None,
        ChildActivityWatch::StreamedOutput,
    )
    .expect("child command result");

    assert_eq!(output.status, TimedCommandStatus::Success);
    assert_eq!(output.stdout, "moving");
}

#[test]
#[cfg(unix)]
fn desktop_child_structured_progress_ignores_chatty_stdout() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = command_output_with_timeout_and_env_inner(
        Path::new("/bin/sh"),
        &[
            "-c".to_string(),
            "while true; do printf moving; sleep 0.05; done".to_string(),
        ],
        Duration::from_millis(250),
        Duration::from_millis(25),
        &[],
        None,
        ChildActivityWatch::StructuredProgress(ChildProgressWatch::new(
            temp.path(),
            "0.0.162",
            "desktop",
        )),
    )
    .expect("child command result");

    assert_eq!(output.status, TimedCommandStatus::TimedOut);
    assert!(
        output.stdout.contains("moving"),
        "stdout is still forwarded for diagnostics"
    );
}

#[test]
fn installed_upgrade_attempts_proxy_restart_even_when_desktop_update_fails() {
    let restart_called = std::cell::Cell::new(false);
    let result = finish_installed_upgrade_steps(
        UpgradeBehavior::background(),
        || {
            Err(BifrostError::Config(
                "desktop package download timed out".to_string(),
            ))
        },
        || {
            restart_called.set(true);
            Ok(())
        },
    );

    assert!(restart_called.get());
    assert!(result
        .expect_err("desktop failure remains visible")
        .to_string()
        .contains("desktop package download timed out"));
}

#[test]
fn installed_upgrade_does_not_launch_desktop_after_proxy_restart_failure() {
    let desktop_called = std::cell::Cell::new(false);
    let error = finish_installed_upgrade_steps(
        UpgradeBehavior::background(),
        || {
            desktop_called.set(true);
            Ok(())
        },
        || Err(BifrostError::Config("restart failed".to_string())),
    )
    .expect_err("proxy restart fails");
    assert!(error.to_string().contains("restart failed"));
    assert!(
        !desktop_called.get(),
        "Desktop must not take over a failed CLI restart"
    );
}

#[test]
fn upgrade_restarts_cli_before_launching_desktop() {
    let events = std::cell::RefCell::new(Vec::new());
    finish_installed_upgrade_steps(
        UpgradeBehavior::background(),
        || {
            events.borrow_mut().push("desktop");
            Ok(())
        },
        || {
            events.borrow_mut().push("cli_ready");
            Ok(())
        },
    )
    .expect("upgrade completes");
    assert_eq!(*events.borrow(), ["cli_ready", "desktop"]);
}

#[test]
fn upgrade_freezes_desktop_ownership_before_companion_changes_runtime() {
    let runtime = RuntimeInfo::new(123, 19900, None, None, RuntimeStartMode::Desktop);
    let behavior = UpgradeBehavior::background().for_runtime_owner(Some(&runtime));
    let restart_called = std::cell::Cell::new(false);
    finish_installed_upgrade_steps(
        behavior,
        || Ok(()),
        || {
            restart_called.set(true);
            Ok(())
        },
    )
    .expect("Desktop owns its server restart");
    assert!(!restart_called.get());
    for mode in [RuntimeStartMode::Daemon, RuntimeStartMode::Foreground] {
        let runtime = RuntimeInfo::new(123, 19900, None, None, mode);
        assert!(
            UpgradeBehavior::background()
                .for_runtime_owner(Some(&runtime))
                .restart_proxy
        );
        assert!(
            !UpgradeBehavior::interactive(false, true)
                .for_runtime_owner(Some(&runtime))
                .restart_proxy
        );
    }
    assert!(
        UpgradeBehavior::background()
            .for_runtime_owner(None)
            .restart_proxy
    );
}

#[test]
fn installed_upgrade_skips_proxy_restart_when_behavior_disables_it() {
    let restart_called = std::cell::Cell::new(false);
    finish_installed_upgrade_steps(
        UpgradeBehavior::interactive(false, true),
        || Ok(()),
        || {
            restart_called.set(true);
            Ok(())
        },
    )
    .expect("desktop-only completion");
    assert!(!restart_called.get());
}

#[test]
fn download_status_messages_cover_primary_fallback_and_retry_sources() {
    assert_eq!(
        download_source_progress_message("https://github.com", 0),
        "Connecting to download source github.com…"
    );
    assert_eq!(
        download_source_progress_message("https://ghfast.top/https://github.com", 1),
        "Trying fallback download source ghfast.top…"
    );
    assert_eq!(
        download_retry_progress_message(2, 3),
        "Retrying current download source (2/3)…"
    );
}

#[test]
fn already_latest_upgrade_attempts_proxy_restart_when_desktop_update_fails() {
    let restart_called = std::cell::Cell::new(false);
    let result = finish_already_latest_upgrade_steps(
        UpgradeBehavior::background(),
        || {
            Err(BifrostError::Config(
                "desktop package download timed out".to_string(),
            ))
        },
        || {
            restart_called.set(true);
            Ok(())
        },
    );

    assert!(restart_called.get());
    assert!(result
        .expect_err("desktop failure remains visible")
        .to_string()
        .contains("desktop package download timed out"));
}

#[test]
fn already_latest_upgrade_interactive_preserves_no_restart_behavior() {
    let restart_called = std::cell::Cell::new(false);
    finish_already_latest_upgrade_steps(
        UpgradeBehavior::interactive(false, false),
        || Ok(()),
        || {
            restart_called.set(true);
            Ok(())
        },
    )
    .expect("interactive already-latest upgrade");

    assert!(!restart_called.get());
}

#[test]
fn caller_managed_relaunch_preserves_runtime_owner_and_custom_port() {
    for mode in [RuntimeStartMode::Desktop, RuntimeStartMode::Daemon] {
        let runtime = RuntimeInfo::new(456, 18888, None, None, mode);
        let marker =
            caller_managed_relaunch_marker(&runtime, 123, Path::new("/tmp/Bifrost.app"), "0.0.190");
        assert_eq!(marker["proxy_port"], 18888);
        assert_eq!(marker["old_app_pid"], 123);
        assert_eq!(marker["target_version"], "0.0.190");
        if mode == RuntimeStartMode::Desktop {
            assert_eq!(marker["old_core_pid"], 456);
            assert!(marker["observed_external_core_pid"].is_null());
        } else {
            assert!(marker["old_core_pid"].is_null());
            assert_eq!(marker["observed_external_core_pid"], 456);
        }
    }
}
