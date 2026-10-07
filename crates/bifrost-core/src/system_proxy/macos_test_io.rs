//! Explicit fixture routing for a separately compiled CI binary. Normal release
//! builds contain no runtime fake-I/O switch; Cargo features never enable this.
use crate::{BifrostError, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

// Reachable only in the explicitly compiled fixture router. The shell fixture
// checks this signature without executing a candidate before its read probe.
const CAPABILITY: &str = "BIFROST_PROXY_TEST_IO_CAPABILITY_V1";

pub(super) const FIXTURE_TAG: &str = "bifrost-proxy-test-io-v1";
const SCRIPT_HEADER: &[u8] = b"#!/bin/bash\n# bifrost-proxy-test-io-v1\n";

pub(super) struct Fixture {
    directory: PathBuf,
    state: PathBuf,
    log: PathBuf,
}

fn invalid(message: impl std::fmt::Display) -> BifrostError {
    BifrostError::Config(format!(
        "{CAPABILITY}: TestProxyIoUnavailable: {message}; refusing native proxy I/O"
    ))
}

impl Fixture {
    #[cfg(all(bifrost_proxy_test_io, not(test)))]
    pub(super) fn from_env() -> Result<Self> {
        let required = |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| invalid(format!("missing {name}")))
        };
        Ok(Self {
            directory: required("BIFROST_PROXY_TEST_IO_DIR")?,
            state: required("BIFROST_FAKE_SYSTEM_PROXY_STATE")?,
            log: required("BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG")?,
        })
    }

    pub(super) fn command(&self, requested: &str) -> Result<PathBuf> {
        let name = match requested {
            "/usr/sbin/networksetup" => "networksetup",
            "/usr/sbin/scutil" => "scutil",
            // Privileged fake mutations must go through the same fixture
            // networksetup. Never invoke sudo or interpret an AppleScript.
            _ => return Err(invalid(format!("unsupported test command {requested}"))),
        };
        if !self.directory.is_absolute() || !self.state.is_absolute() || !self.log.is_absolute() {
            return Err(invalid("fixture paths must be absolute"));
        }
        let metadata = std::fs::symlink_metadata(&self.directory).map_err(invalid)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("fixture directory must be a real directory"));
        }
        let directory = self.directory.canonicalize().map_err(invalid)?;
        let parent = directory
            .parent()
            .ok_or_else(|| invalid("fixture has no parent"))?;
        let marker = regular_file(&directory.join(".bifrost-proxy-test-io"))?;
        if std::fs::read_to_string(marker).map_err(invalid)?.trim() != FIXTURE_TAG {
            return Err(invalid("fixture marker is missing or invalid"));
        }
        for file in [&self.state, &self.log] {
            let file = regular_file(file)?;
            if file.parent() != Some(parent) {
                return Err(invalid(
                    "state and log must belong to the fixture's private parent directory",
                ));
            }
        }
        // Validate both tools on every command, not merely the selected one.
        // A partial/missing fixture is never permission to use native tools.
        for program in ["networksetup", "scutil"] {
            let path = regular_file(&directory.join(program))?;
            if path.parent() != Some(directory.as_path()) {
                return Err(invalid("fixture command escaped its directory"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&path)
                    .map_err(invalid)?
                    .permissions()
                    .mode()
                    & 0o111
                    == 0
                {
                    return Err(invalid("fixture command is not executable"));
                }
            }
            let mut header = vec![0u8; SCRIPT_HEADER.len()];
            std::fs::File::open(path)
                .map_err(invalid)?
                .read_exact(&mut header)
                .map_err(invalid)?;
            if header != SCRIPT_HEADER {
                return Err(invalid("fixture command is not a tagged test script"));
            }
        }
        Ok(directory.join(name))
    }
}

fn regular_file(path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).map_err(invalid)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("fixture files must be regular files, not links"));
    }
    path.canonicalize().map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Fixture) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("fake-bin");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join(".bifrost-proxy-test-io"), FIXTURE_TAG).unwrap();
        for name in ["networksetup", "scutil"] {
            let path = directory.join(name);
            std::fs::write(&path, [SCRIPT_HEADER, b"exit 0\n"].concat()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let state = root.path().join("state");
        let log = root.path().join("log");
        std::fs::write(&state, b"disabled").unwrap();
        std::fs::write(&log, b"").unwrap();
        (
            root,
            Fixture {
                directory,
                state,
                log,
            },
        )
    }

    #[test]
    fn explicit_fixture_selects_only_tagged_private_commands() {
        let (_root, fixture) = fixture();
        for name in ["networksetup", "scutil"] {
            assert_eq!(
                fixture.command(&format!("/usr/sbin/{name}")).unwrap(),
                fixture.directory.canonicalize().unwrap().join(name)
            );
        }
        for privileged in [
            "/usr/bin/sudo",
            "/usr/bin/osascript",
            "networksetup",
            "/bin/sh",
        ] {
            assert!(fixture
                .command(privileged)
                .unwrap_err()
                .to_string()
                .contains("refusing native"));
        }
    }

    #[test]
    fn incomplete_or_malformed_fixture_never_falls_back() {
        for missing in ["scutil", "networksetup", ".bifrost-proxy-test-io"] {
            let (_root, fixture) = fixture();
            std::fs::remove_file(fixture.directory.join(missing)).unwrap();
            assert!(fixture.command("/usr/sbin/networksetup").is_err());
        }
        let (_root, mut fixture) = fixture();
        std::fs::write(fixture.directory.join("scutil"), b"not a fixture").unwrap();
        assert!(fixture.command("/usr/sbin/scutil").is_err());
        fixture.directory = PathBuf::from("relative-fixture");
        assert!(fixture.command("/usr/sbin/scutil").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_native_commands_and_external_state_are_rejected() {
        let (_root, mut fixture) = fixture();
        std::fs::remove_file(fixture.directory.join("networksetup")).unwrap();
        std::os::unix::fs::symlink(
            "/usr/sbin/networksetup",
            fixture.directory.join("networksetup"),
        )
        .unwrap();
        assert!(fixture.command("/usr/sbin/networksetup").is_err());
        let outside = tempfile::NamedTempFile::new().unwrap();
        fixture.state = outside.path().to_owned();
        assert!(fixture.command("/usr/sbin/scutil").is_err());
    }
}
