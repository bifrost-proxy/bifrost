use super::{SystemProxyShutdownMode, SHUTDOWN_MODE_FILE_NAME, SHUTDOWN_MODE_TTL_SECS};
use crate::Result;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::Path;

// Shutdown markers have their own lock: ownership operations may already hold
// the proxy lock when reading one. Never acquire the ownership lock here.
struct ShutdownMarkerLock(File);

impl ShutdownMarkerLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        Self::acquire_with_timeout(data_dir, std::time::Duration::from_secs(2))
    }

    fn acquire_with_timeout(data_dir: &Path, timeout: std::time::Duration) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(".system_proxy_shutdown_mode.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o666)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(crate::BifrostError::Config(
                "Refusing non-regular shutdown marker lock".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.nlink() != 1 {
                return Err(crate::BifrostError::Config(
                    "Refusing hard-linked shutdown marker lock".into(),
                ));
            }
            // The lock contains no data and lives inside the protected Bifrost
            // directory. Match the proxy ownership lock's mixed root/user mode.
            if metadata.permissions().mode() & 0o777 != 0o666 {
                file.set_permissions(std::fs::Permissions::from_mode(0o666))?;
            }
        }
        let started = std::time::Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= timeout {
                        return Err(crate::BifrostError::Config(
                            "Timed out waiting for shutdown marker lock".into(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for ShutdownMarkerLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn open_marker(data_dir: &Path, write: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create(write)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(data_dir.join(SHUTDOWN_MODE_FILE_NAME))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other(
            "Refusing non-regular shutdown marker",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(std::io::Error::other(
                "Refusing hard-linked shutdown marker",
            ));
        }
        if write && unsafe { libc::geteuid() } == 0 {
            let owner = std::fs::metadata(data_dir)?;
            if unsafe { libc::fchown(file.as_raw_fd(), owner.uid(), owner.gid()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(file)
}

fn write_locked(data_dir: &Path, mode: SystemProxyShutdownMode) -> Result<()> {
    use std::io::Write;
    let mut file = open_marker(data_dir, true)?;
    file.set_len(0)?;
    file.write_all(mode.as_str().as_bytes())?;
    Ok(())
}

fn read_locked(data_dir: &Path) -> Result<Option<SystemProxyShutdownMode>> {
    use std::io::Read;
    let mut file = match open_marker(data_dir, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file
        .metadata()?
        .modified()?
        .elapsed()
        .ok()
        .is_some_and(|elapsed| elapsed.as_secs() > SHUTDOWN_MODE_TTL_SECS)
    {
        std::fs::remove_file(data_dir.join(SHUTDOWN_MODE_FILE_NAME))?;
        return Ok(None);
    }
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    SystemProxyShutdownMode::from_str(&value)
        .map(Some)
        .ok_or_else(|| crate::BifrostError::Config("Invalid system proxy shutdown marker".into()))
}

pub fn write_system_proxy_shutdown_mode(
    data_dir: &Path,
    mode: SystemProxyShutdownMode,
) -> Result<()> {
    let _lock = ShutdownMarkerLock::acquire(data_dir)?;
    write_locked(data_dir, mode)
}

/// A stale helper may request a restart, but it must never overwrite an
/// explicit stop which arrived after the helper's initial preflight check.
pub fn try_write_system_proxy_restart_handoff(data_dir: &Path) -> Result<bool> {
    let _lock = ShutdownMarkerLock::acquire(data_dir)?;
    if matches!(
        read_locked(data_dir)?,
        Some(
            SystemProxyShutdownMode::BackgroundCleanup | SystemProxyShutdownMode::ForegroundCleanup
        )
    ) {
        return Ok(false);
    }
    write_locked(data_dir, SystemProxyShutdownMode::PreserveForRestart)?;
    Ok(true)
}

pub fn read_system_proxy_shutdown_mode(data_dir: &Path) -> Option<SystemProxyShutdownMode> {
    match read_system_proxy_shutdown_mode_checked(data_dir) {
        Ok(mode) => mode,
        Err(error) => {
            tracing::warn!(%error, "unknown shutdown intent; treating it as a stop request");
            Some(SystemProxyShutdownMode::ForegroundCleanup)
        }
    }
}

pub fn read_system_proxy_shutdown_mode_checked(
    data_dir: &Path,
) -> Result<Option<SystemProxyShutdownMode>> {
    match std::fs::symlink_metadata(data_dir.join(SHUTDOWN_MODE_FILE_NAME)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let _lock = ShutdownMarkerLock::acquire(data_dir)?;
    read_locked(data_dir)
}

pub fn consume_system_proxy_shutdown_mode(data_dir: &Path) -> Option<SystemProxyShutdownMode> {
    consume_matching(data_dir, None)
}

/// Compare and consume under the same lock used by marker writers. A newer
/// stop request must survive cleanup of an earlier restart handoff.
pub fn consume_system_proxy_shutdown_mode_if(
    data_dir: &Path,
    expected: SystemProxyShutdownMode,
) -> Option<SystemProxyShutdownMode> {
    consume_matching(data_dir, Some(expected))
}

fn consume_matching(
    data_dir: &Path,
    expected: Option<SystemProxyShutdownMode>,
) -> Option<SystemProxyShutdownMode> {
    if !data_dir.join(SHUTDOWN_MODE_FILE_NAME).exists() {
        return None;
    }
    let _lock = ShutdownMarkerLock::acquire(data_dir).ok()?;
    let mode = read_locked(data_dir).ok()??;
    if expected.is_some_and(|expected| mode != expected) {
        return None;
    }
    std::fs::remove_file(data_dir.join(SHUTDOWN_MODE_FILE_NAME)).ok()?;
    Some(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_restart_cleanup_never_consumes_new_stop() {
        for mode in [
            SystemProxyShutdownMode::BackgroundCleanup,
            SystemProxyShutdownMode::ForegroundCleanup,
        ] {
            let dir = tempfile::tempdir().unwrap();
            assert!(try_write_system_proxy_restart_handoff(dir.path()).unwrap());
            write_system_proxy_shutdown_mode(dir.path(), mode).unwrap();
            assert_eq!(
                consume_system_proxy_shutdown_mode_if(
                    dir.path(),
                    SystemProxyShutdownMode::PreserveForRestart
                ),
                None
            );
            assert!(!try_write_system_proxy_restart_handoff(dir.path()).unwrap());
            assert_eq!(read_system_proxy_shutdown_mode(dir.path()), Some(mode));
        }
    }

    #[test]
    fn matching_restart_handoff_can_be_consumed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(try_write_system_proxy_restart_handoff(dir.path()).unwrap());
        assert_eq!(
            consume_system_proxy_shutdown_mode_if(
                dir.path(),
                SystemProxyShutdownMode::PreserveForRestart
            ),
            Some(SystemProxyShutdownMode::PreserveForRestart)
        );
        assert_eq!(read_system_proxy_shutdown_mode(dir.path()), None);
    }

    #[test]
    fn racing_stop_and_handoff_always_leave_stop() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    let _ = try_write_system_proxy_restart_handoff(dir.path());
                });
                scope.spawn(|| {
                    write_system_proxy_shutdown_mode(
                        dir.path(),
                        SystemProxyShutdownMode::ForegroundCleanup,
                    )
                    .unwrap();
                });
                scope.spawn(|| {
                    consume_system_proxy_shutdown_mode_if(
                        dir.path(),
                        SystemProxyShutdownMode::PreserveForRestart,
                    );
                });
            }
        });
        assert_eq!(
            read_system_proxy_shutdown_mode(dir.path()),
            Some(SystemProxyShutdownMode::ForegroundCleanup)
        );
    }

    #[test]
    fn malformed_marker_is_an_unknown_stop_not_absence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SHUTDOWN_MODE_FILE_NAME), "corrupt").unwrap();
        assert!(read_system_proxy_shutdown_mode_checked(dir.path()).is_err());
        assert_eq!(
            read_system_proxy_shutdown_mode(dir.path()),
            Some(SystemProxyShutdownMode::ForegroundCleanup)
        );
        assert!(try_write_system_proxy_restart_handoff(dir.path()).is_err());
    }

    #[test]
    fn marker_lock_wait_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let _held = ShutdownMarkerLock::acquire(dir.path()).unwrap();
        let started = std::time::Instant::now();
        assert!(ShutdownMarkerLock::acquire_with_timeout(
            dir.path(),
            std::time::Duration::from_millis(20)
        )
        .is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn shared_lock_mode_is_safe_and_links_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".system_proxy_shutdown_mode.lock");
        let held = ShutdownMarkerLock::acquire(dir.path()).unwrap();
        assert_eq!(
            held.0.metadata().unwrap().permissions().mode() & 0o777,
            0o666
        );
        drop(held);
        std::fs::remove_file(&lock_path).unwrap();
        let target = dir.path().join("unrelated");
        std::fs::write(&target, "preserve").unwrap();
        symlink(&target, &lock_path).unwrap();
        assert!(ShutdownMarkerLock::acquire(dir.path()).is_err());
        std::fs::remove_file(&lock_path).unwrap();
        std::fs::hard_link(&target, &lock_path).unwrap();
        assert!(ShutdownMarkerLock::acquire(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
    }

    #[cfg(unix)]
    #[test]
    fn privileged_marker_paths_never_follow_links_or_truncate_other_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        let marker = dir.path().join(SHUTDOWN_MODE_FILE_NAME);
        std::fs::write(&target, "preserve").unwrap();
        symlink(&target, &marker).unwrap();
        assert!(write_system_proxy_shutdown_mode(
            dir.path(),
            SystemProxyShutdownMode::ForegroundCleanup
        )
        .is_err());
        assert!(read_system_proxy_shutdown_mode_checked(dir.path()).is_err());
        std::fs::remove_file(&marker).unwrap();
        std::fs::hard_link(&target, &marker).unwrap();
        assert!(write_system_proxy_shutdown_mode(
            dir.path(),
            SystemProxyShutdownMode::ForegroundCleanup
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
    }
    #[test]
    fn future_marker_timestamp_does_not_erase_stop_intent() {
        let dir = tempfile::tempdir().unwrap();
        write_system_proxy_shutdown_mode(dir.path(), SystemProxyShutdownMode::ForegroundCleanup)
            .unwrap();
        let future = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        );
        filetime::set_file_mtime(dir.path().join(SHUTDOWN_MODE_FILE_NAME), future).unwrap();
        assert_eq!(
            read_system_proxy_shutdown_mode_checked(dir.path()).unwrap(),
            Some(SystemProxyShutdownMode::ForegroundCleanup)
        );
    }
    #[test]
    fn expired_marker_is_removed_and_parent_path_errors_remain_errors() {
        let dir = tempfile::tempdir().unwrap();
        write_system_proxy_shutdown_mode(dir.path(), SystemProxyShutdownMode::ForegroundCleanup)
            .unwrap();
        let marker = dir.path().join(SHUTDOWN_MODE_FILE_NAME);
        filetime::set_file_mtime(&marker, filetime::FileTime::from_unix_time(1, 0)).unwrap();
        assert!(read_system_proxy_shutdown_mode_checked(dir.path())
            .unwrap()
            .is_none());
        assert!(!marker.exists());
        let invalid_directory = dir.path().join("regular-file");
        std::fs::write(&invalid_directory, b"preserve").unwrap();
        assert!(read_system_proxy_shutdown_mode_checked(&invalid_directory).is_err());
        assert_eq!(std::fs::read(invalid_directory).unwrap(), b"preserve");
    }

    #[cfg(unix)]
    #[test]
    fn fifo_marker_and_lock_are_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = |path: &Path| {
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        };
        let marker = dir.path().join(SHUTDOWN_MODE_FILE_NAME);
        fifo(&marker);
        assert!(read_locked(dir.path())
            .unwrap_err()
            .to_string()
            .contains("non-regular"));
        std::fs::remove_file(marker).unwrap();
        fifo(&dir.path().join(".system_proxy_shutdown_mode.lock"));
        assert!(ShutdownMarkerLock::acquire(dir.path())
            .err()
            .unwrap()
            .to_string()
            .contains("non-regular"));
    }
}
