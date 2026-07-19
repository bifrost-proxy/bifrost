use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::cli_binary_name;

const CLI_VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn desktop_app_version_for_version_check() -> Option<String> {
    desktop_app_installation_from_candidates(desktop_app_install_candidates())
        .map(|(_, version)| version)
}

fn desktop_app_installation_from_candidates(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<(PathBuf, String)> {
    candidates
        .into_iter()
        .find_map(|path| installed_desktop_app_version(&path).map(|version| (path, version)))
}

fn desktop_app_install_candidates() -> Vec<PathBuf> {
    let override_dir = env::var_os("BIFROST_APP_INSTALL_DIR").map(PathBuf::from);
    let current_exe = env::current_exe().ok();
    let home = env::var_os("HOME").map(PathBuf::from);
    desktop_app_install_candidates_for_context(override_dir, current_exe.as_deref(), home)
}

fn desktop_app_install_candidates_for_context(
    override_dir: Option<PathBuf>,
    current_exe: Option<&Path>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(dir) = override_dir {
        return vec![resolve_desktop_app_path(&dir)];
    }

    #[cfg(target_os = "macos")]
    {
        let mut candidates = Vec::new();
        if let Some(active_bundle) = current_exe.and_then(macos_app_bundle_from_executable) {
            candidates.push(active_bundle);
        }
        for candidate in [
            Some(PathBuf::from("/Applications/Bifrost.app")),
            home.map(|home| home.join("Applications").join("Bifrost.app")),
        ]
        .into_iter()
        .flatten()
        {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        candidates
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (current_exe, home);
        let mut candidates = Vec::new();
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Bifrost")
                    .join("bifrost-desktop.exe"),
            );
        }
        candidates
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (current_exe, home);
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn macos_app_bundle_from_executable(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("Bifrost.app"))
        .map(Path::to_path_buf)
}

fn resolve_desktop_app_path(app_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        app_dir.join("Bifrost.app")
    }
    #[cfg(target_os = "windows")]
    {
        app_dir.join("bifrost-desktop.exe")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        app_dir.join("Bifrost")
    }
}

fn installed_desktop_app_version(install_path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let plist_path = install_path.join("Contents").join("Info.plist");
        let plist = plist::Value::from_file(plist_path).ok()?;
        let dict = plist.as_dictionary()?;
        ["CFBundleShortVersionString", "CFBundleVersion"]
            .into_iter()
            .find_map(|key| dict.get(key).and_then(|value| value.as_string()))
            .map(str::to_string)
    }
    #[cfg(target_os = "windows")]
    {
        if !install_path.is_file() {
            return None;
        }
        let script = r#"
param([string]$Path)
$info = (Get-Item -LiteralPath $Path).VersionInfo
if ($info.ProductVersion) { $info.ProductVersion } elseif ($info.FileVersion) { $info.FileVersion }
"#;
        let powershell = if Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            "powershell.exe"
        } else {
            "pwsh"
        };
        let output = Command::new(powershell)
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .arg(install_path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!version.is_empty()).then_some(version)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = install_path;
        None
    }
}

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

    #[cfg(target_os = "macos")]
    fn write_app_version(app: &Path, version: &str) {
        let contents = app.join("Contents");
        fs::create_dir_all(&contents).expect("create app Contents");
        fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .expect("write app version");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn desktop_version_check_reads_installed_app_version_from_override_dir() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("tempdir");
        write_app_version(&temp.path().join("Bifrost.app"), "0.0.144");

        let previous = env::var_os("BIFROST_APP_INSTALL_DIR");
        env::set_var("BIFROST_APP_INSTALL_DIR", temp.path());
        let version = desktop_app_version_for_version_check();
        match previous {
            Some(value) => env::set_var("BIFROST_APP_INSTALL_DIR", value),
            None => env::remove_var("BIFROST_APP_INSTALL_DIR"),
        }

        assert_eq!(version.as_deref(), Some("0.0.144"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn desktop_version_check_prefers_the_app_bundle_that_launched_the_core() {
        let temp = tempfile::tempdir().expect("tempdir");
        let active_app = temp.path().join("active/Bifrost.app");
        let home = temp.path().join("home");
        let fallback_app = home.join("Applications/Bifrost.app");
        write_app_version(&active_app, "0.0.143");
        write_app_version(&fallback_app, "0.0.144");

        let active_executable = active_app.join("Contents/MacOS/bifrost");
        let candidates =
            desktop_app_install_candidates_for_context(None, Some(&active_executable), Some(home));
        assert_eq!(candidates.first(), Some(&active_app));
        let (selected_app, selected_version) = desktop_app_installation_from_candidates(candidates)
            .expect("select running App bundle");
        assert_eq!(selected_app, active_app);
        assert_eq!(selected_version, "0.0.143");
    }

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
