use super::*;

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
        download_tries: 2,
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
    // Port 0 asks the OS for a fresh ephemeral port on every bind probe. Using
    // a previously bound-and-dropped concrete port is racy on Windows: another
    // parallel test can claim it, and Windows may temporarily delay reuse even
    // when netstat shows no listener.
    wait_for_restart_ports_release(&[0]).expect("OS-selected fixture port");
    match previous_mirror {
        Some(value) => std::env::set_var("BIFROST_GITHUB_MIRROR", value),
        None => std::env::remove_var("BIFROST_GITHUB_MIRROR"),
    }
}
