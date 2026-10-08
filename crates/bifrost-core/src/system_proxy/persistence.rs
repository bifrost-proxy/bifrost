//! Crash-durable replacement of the authoritative ownership journal.
use std::io::Write;
use std::path::Path;

use crate::Result;

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "proxy state has no parent",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // A root cleanup helper must not replace a user's state with a root-only
        // file. Never widen the existing permissions or follow a state symlink.
        let metadata = std::fs::symlink_metadata(path).or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                std::fs::metadata(parent)
            } else {
                Err(error)
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "proxy state is a symlink",
            )
            .into());
        }
        if unsafe { libc::geteuid() } == 0
            && unsafe { libc::fchown(temp.as_file().as_raw_fd(), metadata.uid(), metadata.gid()) }
                != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)?;
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_is_complete_and_temporary_files_are_not_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("proxy_state.json");
        atomic_write(&state, br#"{"generation":"one"}"#).unwrap();
        std::fs::write(dir.path().join("interrupted.tmp"), b"{partial").unwrap();
        assert_eq!(std::fs::read(&state).unwrap(), br#"{"generation":"one"}"#);
        atomic_write(&state, br#"{"generation":"two"}"#).unwrap();
        assert_eq!(std::fs::read(&state).unwrap(), br#"{"generation":"two"}"#);
    }

    #[cfg(unix)]
    #[test]
    fn journal_replacement_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("external");
        std::fs::write(&target, "keep").unwrap();
        let state = dir.path().join("proxy_state.json");
        std::os::unix::fs::symlink(&target, &state).unwrap();
        assert!(atomic_write(&state, b"replace").is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
    }
    #[test]
    fn invalid_destination_does_not_leave_temporary_or_replaced_data() {
        assert!(atomic_write(Path::new(""), b"unreachable").is_err());
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("existing_directory");
        std::fs::create_dir(&destination).unwrap();
        let sentinel = destination.join("preserve");
        std::fs::write(&sentinel, b"original").unwrap();
        assert!(atomic_write(&destination, b"replacement").is_err());
        assert_eq!(std::fs::read(sentinel).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn invalid_file_name_fails_before_replacing_the_authoritative_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("proxy_state.json");
        atomic_write(&state, b"original").unwrap();
        assert!(atomic_write(&dir.path().join("x".repeat(512)), b"replacement").is_err());
        assert_eq!(std::fs::read(state).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
