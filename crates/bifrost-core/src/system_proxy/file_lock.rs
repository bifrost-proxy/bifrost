#[cfg(target_os = "macos")]
use super::*;
#[cfg(not(target_os = "macos"))]
use super::{Path, Result};

#[cfg(target_os = "macos")]
pub(super) struct SystemProxyFileLock {
    // Unregister before closing. No explicit LOCK_UN: a Direct one-shot child
    // may still own a duplicate after parent death/timeout. Only the final
    // close releases that open-file-description's lock.
    _scope: super::macos_operation_lock::Scope,
    _file: File,
}

#[cfg(target_os = "macos")]
pub(super) fn acquire_system_proxy_file_lock(
    data_dir: &Path,
    context: &'static str,
) -> Result<SystemProxyFileLock> {
    std::fs::create_dir_all(data_dir)?;
    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let file = match open_system_proxy_lock_file(data_dir, true) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(BifrostError::Config(format!(
                "RequiresAdmin: system proxy ownership lock is not writable ({error}); repair it explicitly with administrator rights using bifrost system-proxy repair-lock --data-dir {}",
                data_dir.display()
            )));
        }
        Err(error) => return Err(error.into()),
    };
    tracing::info!(
        data_dir = %data_dir.display(),
        lock_path = %lock_path.display(),
        context,
        "waiting for system proxy cross-process file lock"
    );
    wait_for_system_proxy_file_lock(&file, data_dir, &lock_path, context)?;
    tracing::info!(
        data_dir = %data_dir.display(),
        lock_path = %lock_path.display(),
        context,
        "acquired system proxy cross-process file lock"
    );
    let scope = super::macos_operation_lock::Scope::register(&file)?;
    Ok(SystemProxyFileLock {
        _scope: scope,
        _file: file,
    })
}

#[cfg(target_os = "macos")]
pub(super) fn wait_for_system_proxy_file_lock(
    file: &File,
    data_dir: &Path,
    lock_path: &Path,
    context: &'static str,
) -> Result<()> {
    let timeout = system_proxy_lock_wait_timeout();
    let started = Instant::now();
    let mut next_log_at = Duration::ZERO;

    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        let would_block = error.raw_os_error() == Some(libc::EWOULDBLOCK)
            || error.raw_os_error() == Some(libc::EAGAIN);
        if !would_block {
            return Err(error.into());
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(BifrostError::Config(format!(
                "Timed out after {}ms waiting for system proxy cross-process file lock (context={context}, data_dir={}, lock_path={}). Another Bifrost process may be stuck while changing macOS system proxy settings.",
                timeout.as_millis(),
                data_dir.display(),
                lock_path.display()
            )));
        }

        if elapsed >= next_log_at {
            tracing::warn!(
                data_dir = %data_dir.display(),
                lock_path = %lock_path.display(),
                context,
                elapsed_ms = elapsed.as_millis(),
                timeout_ms = timeout.as_millis(),
                "still waiting for system proxy cross-process file lock"
            );
            next_log_at = elapsed + Duration::from_millis(LOCK_WAIT_LOG_INTERVAL_MS);
        }

        let remaining = timeout.saturating_sub(elapsed);
        std::thread::sleep(std::cmp::min(
            Duration::from_millis(LOCK_WAIT_POLL_MS),
            remaining,
        ));
    }
}

#[cfg(target_os = "macos")]
pub(super) fn system_proxy_lock_wait_timeout() -> Duration {
    system_proxy_lock_wait_timeout_from_env(
        std::env::var("BIFROST_SYSTEM_PROXY_LOCK_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn system_proxy_lock_wait_timeout_from_env(value: Option<&str>) -> Duration {
    let timeout_ms = value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LOCK_WAIT_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

#[cfg(target_os = "macos")]
pub(super) fn open_system_proxy_lock_file(data_dir: &Path, create: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .truncate(false)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    if create {
        options.create(true).mode(0o666);
    }
    let file = options.open(&lock_path)?;
    relax_lock_file_mode_if_needed(&file, &lock_path)?;
    Ok(file)
}

#[cfg(target_os = "macos")]
pub fn repair_system_proxy_lock_permissions(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let _ = open_system_proxy_lock_file(data_dir, true)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn repair_system_proxy_lock_permissions(_data_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn relax_lock_file_mode_if_needed(file: &File, lock_path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::other(format!(
            "Refusing to use non-regular system proxy lock file: {}",
            lock_path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(std::io::Error::other(format!(
            "Refusing to use hard-linked system proxy lock file: {}",
            lock_path.display()
        )));
    }

    let current_mode = metadata.permissions().mode() & 0o777;
    if current_mode != 0o666 {
        let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o666) };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        tracing::info!(
            target: "bifrost_core::system_proxy",
            lock_path = %lock_path.display(),
            previous_mode = format!("{:o}", current_mode),
            "relaxed system proxy lock file permissions to 0666"
        );
    }
    Ok(())
}
