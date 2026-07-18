use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::cli_binary_name;

const CLI_VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn standalone_cli_version_for_version_check() -> Option<String> {
    let cli_path = find_standalone_cli_install()?;
    read_cli_version_for_version_check(&cli_path, CLI_VERSION_CHECK_TIMEOUT)
}

fn find_standalone_cli_install() -> Option<PathBuf> {
    let current_exe = env::current_exe()
        .ok()
        .map(|path| fs::canonicalize(&path).unwrap_or(path));
    let mut candidates = Vec::new();
    if let Some(paths) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&paths).map(|dir| dir.join(cli_binary_name())));
    }
    if let Some(dir) = env::var_os("BIFROST_INSTALL_DIR") {
        candidates.push(PathBuf::from(dir).join(cli_binary_name()));
    }
    #[cfg(unix)]
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.extend([
            home.join(".local/bin/bifrost"),
            home.join(".bifrost/bin/bifrost"),
            home.join(".cargo/bin/bifrost"),
        ]);
    }
    #[cfg(unix)]
    {
        candidates.extend([
            PathBuf::from("/opt/homebrew/bin/bifrost"),
            PathBuf::from("/usr/local/bin/bifrost"),
        ]);
    }
    #[cfg(windows)]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local_app_data).join("bifrost/bin/bifrost.exe"));
        }
        if let Some(user_profile) = env::var_os("USERPROFILE") {
            candidates.push(PathBuf::from(user_profile).join(".local/bin/bifrost.exe"));
        }
    }
    find_standalone_cli_install_from(candidates, current_exe.as_deref())
}

fn find_standalone_cli_install_from(
    candidates: impl IntoIterator<Item = PathBuf>,
    current_exe: Option<&Path>,
) -> Option<PathBuf> {
    candidates.into_iter().find(|candidate| {
        if !candidate.is_file() {
            return false;
        }
        let canonical = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone());
        current_exe
            .map(|current| canonical != current)
            .unwrap_or(true)
    })
}

fn read_cli_version_for_version_check(cli_path: &Path, timeout: Duration) -> Option<String> {
    if !cli_path.is_file() {
        return None;
    }
    let mut stdout = tempfile::tempfile().ok()?;
    let mut child = Command::new(cli_path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().ok()?))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return None,
        }
    };
    if !status.success() {
        return None;
    }
    let _ = stdout.seek(SeekFrom::Start(0));
    let mut output = String::new();
    stdout.read_to_string(&mut output).ok()?;
    let version = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("bifrost "))?
        .trim();
    (!version.is_empty()).then(|| version.trim_start_matches('v').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_cli(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, script).expect("write CLI fixture");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod CLI fixture");
    }

    #[cfg(unix)]
    #[test]
    fn cli_version_probe_reads_real_binary_and_rejects_failure_or_timeout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cli = temp.path().join("bifrost");
        write_cli(&cli, "#!/bin/sh\nprintf 'bifrost 0.0.155\\n'\n");
        assert_eq!(
            read_cli_version_for_version_check(&cli, Duration::from_secs(1)).as_deref(),
            Some("0.0.155")
        );

        write_cli(&cli, "#!/bin/sh\nexit 7\n");
        assert!(read_cli_version_for_version_check(&cli, Duration::from_secs(1)).is_none());
        write_cli(&cli, "#!/bin/sh\nexec sleep 2\n");
        assert!(read_cli_version_for_version_check(&cli, Duration::from_millis(50)).is_none());
        assert!(read_cli_version_for_version_check(
            &temp.path().join("missing"),
            Duration::from_secs(1)
        )
        .is_none());
    }

    #[test]
    fn standalone_cli_candidates_skip_missing_and_running_core() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = temp.path().join("current");
        let standalone = temp.path().join("standalone");
        fs::write(&current, "current").expect("write current");
        fs::write(&standalone, "standalone").expect("write standalone");
        let current_canonical = fs::canonicalize(&current).expect("canonical current");
        assert_eq!(
            find_standalone_cli_install_from(
                [
                    temp.path().join("missing"),
                    current.clone(),
                    standalone.clone()
                ],
                Some(&current_canonical)
            ),
            Some(standalone)
        );
        assert_eq!(
            find_standalone_cli_install_from([current], Some(&current_canonical)),
            None
        );
    }
}
