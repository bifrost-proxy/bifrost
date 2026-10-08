use super::*;
use fs2::FileExt;
use std::fs::{File, OpenOptions};

fn current_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .ok_or_else(|| BifrostError::Config("Could not determine home directory".into()))
}

fn lock_profiles(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o666)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(BifrostError::Config("Invalid CLI profile lock".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = file.metadata()?;
        if metadata.nlink() != 1 {
            return Err(BifrostError::Config(
                "Refusing linked CLI profile lock".into(),
            ));
        }
        if metadata.permissions().mode() & 0o777 != 0o666 {
            file.set_permissions(std::fs::Permissions::from_mode(0o666))?;
        }
    }
    let started = std::time::Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error)
                if error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
                    && started.elapsed() < std::time::Duration::from_secs(2) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(error) => return Err(error.into()),
        }
    }
}

impl CliProxyEnvironmentManager {
    pub(super) fn lock_profiles(&self) -> Result<File> {
        lock_profiles(&self.profile_lock_path)
    }

    /// Only remove standalone blocks belonging to this data directory and
    /// endpoint. The runtime predicate is evaluated while profile writers are
    /// excluded, both before preparing changes and immediately before writing.
    pub fn disable_for_runtime_guarded(
        data_dir: &Path,
        host: &str,
        port: u16,
        should_cleanup: impl FnMut() -> Result<bool>,
    ) -> Result<Vec<PathBuf>> {
        Self::disable_for_runtime_in_home(&current_home()?, data_dir, host, port, should_cleanup)
    }

    fn disable_for_runtime_in_home(
        home: &Path,
        data_dir: &Path,
        host: &str,
        port: u16,
        mut should_cleanup: impl FnMut() -> Result<bool>,
    ) -> Result<Vec<PathBuf>> {
        let mut manager = Self::with_paths(
            CliProxyShell::Bash,
            Self::all_supported_paths_for_home(home),
        );
        manager.profile_lock_path = home.join(".bifrost_cli_proxy_profiles.lock");
        let _lock = manager.lock_profiles()?;
        if !should_cleanup()? {
            return Ok(Vec::new());
        }
        let host = host.trim_matches(['[', ']']);
        let proxy_url = if host.contains(':') {
            format!("http://[{host}]:{port}")
        } else {
            format!("http://{host}:{port}")
        };
        let cert_dir = data_dir.join("certs");
        let cert_dir = cert_dir.canonicalize().unwrap_or(cert_dir);
        let cert_dir = cert_dir.to_string_lossy();
        let prepared = manager.prepare_updates(|_path, content| {
            let (Some(start), Some(end)) = marker_bounds(&content)? else {
                return Ok(None);
            };
            let block = &content[start..end];
            let matches = [
                CliProxyShell::Bash,
                CliProxyShell::Fish,
                CliProxyShell::PowerShell,
            ]
            .into_iter()
            .any(|shell| {
                PROXY_ENV_VARS.iter().all(|name| {
                    block
                        .lines()
                        .any(|line| line == format_assignment(shell, name, &proxy_url))
                }) && block
                    .lines()
                    .any(|line| line == format_assignment(shell, "BIFROST_CA_DIR", &cert_dir))
            });
            if matches {
                Ok(Some(remove_marked_block(&content)?))
            } else {
                Ok(None)
            }
        })?;
        if !should_cleanup()? {
            return Ok(Vec::new());
        }
        // A user/editor not using our lock still must not have its newer text
        // overwritten by a stale prepared whole-file replacement.
        for item in &prepared {
            if std::fs::read_to_string(&item.path).ok() != item.original {
                return Ok(Vec::new());
            }
        }
        write_prepared_updates(prepared)
    }
}

pub(super) fn default_profile_lock_path(paths: &[PathBuf]) -> PathBuf {
    paths
        .first()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| Path::new("."))
        .join(".bifrost_cli_proxy_profiles.lock")
}

pub(super) fn current_home_profile_lock_path() -> Result<PathBuf> {
    Ok(current_home()?.join(".bifrost_cli_proxy_profiles.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_block(home: &Path, data: &Path, host: &str, port: u16) -> String {
        let config = CliProxyEnvironmentConfig {
            proxy_url: format!("http://{host}:{port}"),
            no_proxy: "localhost".into(),
            ca_file: data.join("certs/ca.crt"),
            ca_bundle: data.join("certs/bundle.pem"),
            ca_dir: data.join("certs"),
        };
        let block = generate_config_block(CliProxyShell::Bash, &config).unwrap();
        std::fs::write(home.join(".bashrc"), format!("user-setting\n{block}")).unwrap();
        block
    }

    #[test]
    fn standalone_only_crash_cleanup_needs_no_os_lease() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        write_block(home.path(), data.path(), "127.0.0.1", 18891);
        assert!(!data.path().join("proxy_state.json").exists());
        let changed = CliProxyEnvironmentManager::disable_for_runtime_in_home(
            home.path(),
            data.path(),
            "127.0.0.1",
            18891,
            || Ok(true),
        )
        .unwrap();
        assert_eq!(changed, [home.path().join(".bashrc")]);
        assert_eq!(
            std::fs::read_to_string(home.path().join(".bashrc")).unwrap(),
            "user-setting"
        );
    }

    #[test]
    fn replacement_runtime_and_foreign_ca_or_target_keep_profiles() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        for (ca_dir, port) in [(foreign.path(), 18891), (data.path(), 18892)] {
            let block = write_block(home.path(), ca_dir, "127.0.0.1", port);
            let changed = CliProxyEnvironmentManager::disable_for_runtime_in_home(
                home.path(),
                data.path(),
                "127.0.0.1",
                18891,
                || Ok(true),
            )
            .unwrap();
            assert!(changed.is_empty());
            assert!(std::fs::read_to_string(home.path().join(".bashrc"))
                .unwrap()
                .contains(&block));
        }
        let block = write_block(home.path(), data.path(), "127.0.0.1", 18891);
        let mut calls = 0;
        let changed = CliProxyEnvironmentManager::disable_for_runtime_in_home(
            home.path(),
            data.path(),
            "127.0.0.1",
            18891,
            || {
                calls += 1;
                Ok(calls == 1)
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert!(changed.is_empty());
        assert!(std::fs::read_to_string(home.path().join(".bashrc"))
            .unwrap()
            .contains(&block));
    }

    #[test]
    fn mixed_external_proxy_fields_are_not_removed() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        write_block(home.path(), data.path(), "127.0.0.1", 18891);
        let path = home.path().join(".bashrc");
        let edited = std::fs::read_to_string(&path).unwrap().replace(
            "export HTTPS_PROXY='http://127.0.0.1:18891'",
            "export HTTPS_PROXY='http://corporate.proxy:8080'",
        );
        std::fs::write(&path, &edited).unwrap();
        assert!(CliProxyEnvironmentManager::disable_for_runtime_in_home(
            home.path(),
            data.path(),
            "127.0.0.1",
            18891,
            || Ok(true)
        )
        .unwrap()
        .is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
    }

    #[test]
    fn guard_and_concurrent_editor_preserve_profiles() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        write_block(home.path(), data.path(), "127.0.0.1", 18891);
        let path = home.path().join(".bashrc");
        let original = std::fs::read_to_string(&path).unwrap();
        assert!(CliProxyEnvironmentManager::disable_for_runtime_in_home(
            home.path(),
            data.path(),
            "127.0.0.1",
            18891,
            || Ok(false)
        )
        .unwrap()
        .is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let mut calls = 0;
        let edited = format!("{original}\nnew-user-setting\n");
        assert!(CliProxyEnvironmentManager::disable_for_runtime_in_home(
            home.path(),
            data.path(),
            "127.0.0.1",
            18891,
            || {
                calls += 1;
                if calls == 2 {
                    std::fs::write(&path, &edited).unwrap();
                }
                Ok(true)
            }
        )
        .unwrap()
        .is_empty());
        assert_eq!(calls, 2);
        assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
    }

    #[test]
    fn ipv6_target_cleanup_matches_canonical_cert_directory() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir(data.path().join("certs")).unwrap();
        // The real enable command writes canonical CA paths; macOS temp roots
        // may pass through /var -> /private/var before reaching that path.
        let canonical_data = data.path().canonicalize().unwrap();
        write_block(home.path(), &canonical_data, "[::1]", 18891);
        assert_eq!(
            CliProxyEnvironmentManager::disable_for_runtime_in_home(
                home.path(),
                data.path(),
                "::1",
                18891,
                || Ok(true)
            )
            .unwrap(),
            [home.path().join(".bashrc")]
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join(".bashrc")).unwrap(),
            "user-setting"
        );
    }

    #[test]
    fn profile_lock_contention_has_a_bounded_timeout_and_remains_reusable() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("profile.lock");
        let held = lock_profiles(&path).unwrap();
        let start = std::time::Instant::now();
        let error = lock_profiles(&path).unwrap_err().to_string();
        assert!(start.elapsed() >= std::time::Duration::from_secs(2));
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        assert!(!error.is_empty());
        drop(held);
        assert!(lock_profiles(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn profile_locks_reject_fifo_symlinks_and_hard_links() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("profile.lock");
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(lock_profiles(&path)
            .unwrap_err()
            .to_string()
            .contains("Invalid CLI profile lock"));
        std::fs::remove_file(&path).unwrap();
        let target = home.path().join("unrelated");
        std::fs::write(&target, "preserve").unwrap();
        symlink(&target, &path).unwrap();
        assert!(lock_profiles(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&target, &path).unwrap();
        assert!(lock_profiles(&path)
            .unwrap_err()
            .to_string()
            .contains("Refusing linked"));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
    }
}
