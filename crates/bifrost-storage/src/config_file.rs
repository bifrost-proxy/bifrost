use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use bifrost_core::{BifrostError, Result};
use fs2::FileExt;
use serde::Deserialize;

use crate::unified_config::SystemProxyConfig;

#[derive(Deserialize)]
struct ProxyIntentFile {
    #[serde(default)]
    system_proxy: SystemProxyConfig,
}

/// Read current durable proxy intent without initialization, migration or writes.
/// Callers must preserve their prior state on an unavailable or invalid file.
pub fn read_persisted_system_proxy_config(data_dir: &Path) -> Result<SystemProxyConfig> {
    read_system_proxy_config_file(&data_dir.join("config.toml"))
}

fn read_system_proxy_config_file(path: &Path) -> Result<SystemProxyConfig> {
    let content = std::fs::read_to_string(path)?;
    toml::from_str::<ProxyIntentFile>(&content)
        .map(|config| config.system_proxy)
        .map_err(|error| BifrostError::Config(format!("Invalid system proxy intent: {error}")))
}

pub(crate) fn read_optional_system_proxy_config(path: &Path) -> Result<Option<SystemProxyConfig>> {
    match read_system_proxy_config_file(path) {
        Ok(config) => Ok(Some(config)),
        Err(BifrostError::Io(error)) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn lock_config_file(data_dir: &Path) -> Result<File> {
    lock_config_file_with_timeout(data_dir, Duration::from_secs(5))
}

fn lock_config_file_with_timeout(data_dir: &Path, timeout: Duration) -> Result<File> {
    let lock = open_config_lock(data_dir)?;
    let started = Instant::now();
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(lock),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                if started.elapsed() >= timeout {
                    return Err(BifrostError::Config(
                        "Timed out waiting for config writer lock".to_string(),
                    ));
                }
                std::thread::sleep(
                    Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn open_config_lock(data_dir: &Path) -> Result<File> {
    let path = data_dir.join(".config-write.lock");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        // flock supports a read-only descriptor. A root-created 0644 lock
        // remains usable by the owner of the data directory, without making
        // unrelated existing files writable or repairing unknown lock objects.
        let mut read = OpenOptions::new();
        read.read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = match read.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o644)
                    .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&path)
                {
                    Ok(file) => {
                        file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
                        file
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => read.open(&path)?,
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(BifrostError::Config(
                "Refusing non-regular or hard-linked config lock".to_string(),
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(BifrostError::Config(
                "Refusing non-regular config lock".to_string(),
            ));
        }
        Ok(file)
    }
}

pub(crate) fn replace_config_file(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    preserve_config_metadata(temp.as_file(), path)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| BifrostError::Io(error.error))?;
    // Persist the directory entry as well as file bytes before acknowledging
    // intent. Windows does not support opening directories via std::fs::File.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn preserve_config_metadata(file: &File, path: &Path) -> Result<()> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(BifrostError::Config(
            "Refusing to replace non-regular config file".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        if unsafe { libc::geteuid() } == 0
            && unsafe { libc::fchown(file.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    file.set_permissions(metadata.permissions())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_invalid_config_reads_never_create_or_migrate_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_persisted_system_proxy_config(temp.path()).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        std::fs::write(temp.path().join("config.toml"), "invalid [toml").unwrap();
        assert!(read_persisted_system_proxy_config(temp.path()).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn legacy_revision_defaults_to_zero() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            "[system_proxy]\nenabled = false\n",
        )
        .unwrap();
        let intent = read_persisted_system_proxy_config(temp.path()).unwrap();
        assert!(!intent.enabled);
        assert_eq!(intent.intent_revision, 0);
    }
    #[test]
    fn contended_config_lock_has_bounded_wait() {
        let temp = tempfile::tempdir().unwrap();
        let _first = lock_config_file(temp.path()).unwrap();
        let error = lock_config_file_with_timeout(temp.path(), Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("Timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_read_only_lock_works_without_changing_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".config-write.lock");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let _lock = lock_config_file(temp.path()).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_lock_rejects_symlinks_and_hardlinks_without_modifying_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let lock = temp.path().join(".config-write.lock");
        std::fs::write(&target, "unchanged").unwrap();
        std::os::unix::fs::symlink(&target, &lock).unwrap();
        assert!(lock_config_file(temp.path()).is_err());
        std::fs::remove_file(&lock).unwrap();
        std::fs::hard_link(&target, &lock).unwrap();
        assert!(lock_config_file(temp.path()).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "unchanged");
    }
    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_existing_config_mode_and_owner() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        replace_config_file(&path, "new").unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.permissions().mode() & 0o777, 0o640);
        assert_eq!(after.uid(), before.uid());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }
}
