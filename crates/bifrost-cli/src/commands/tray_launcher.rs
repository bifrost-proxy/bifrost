use std::path::{Path, PathBuf};
use std::process::Command;

use super::tray::TRAY_SUBCOMMAND;

pub fn should_launch_tray(no_tray: bool) -> bool {
    if cfg!(target_os = "linux") {
        return false;
    }
    if no_tray {
        return false;
    }
    if std::env::var("BIFROST_DISABLE_TRAY").as_deref() == Ok("1") {
        return false;
    }
    true
}

pub fn find_tray_binary() -> Option<PathBuf> {
    // Escape hatch: allow overriding the tray binary explicitly. It must be a
    // bifrost-compatible binary that understands the hidden `__tray` subcommand.
    if let Ok(path) = std::env::var("BIFROST_TRAY_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
        tracing::warn!(path = %path, "BIFROST_TRAY_BIN set but file not found");
    }

    // Busybox-style multi-call: re-exec the current `bifrost` binary as the tray
    // process via the hidden `__tray` subcommand. No separate artifact needed.
    match std::env::current_exe() {
        Ok(exe) => Some(exe),
        Err(e) => {
            tracing::warn!(error = %e, "failed to resolve current_exe for tray");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn launch_tray_helper(
    tray_bin: &Path,
    data_dir: &Path,
    runtime_file: &Path,
    pid: u32,
    admin_url: Option<&str>,
    port: Option<u16>,
    bifrost_bin: Option<&Path>,
    start_args: &[String],
) {
    let mut cmd = Command::new(tray_bin);
    cmd.arg(TRAY_SUBCOMMAND);
    cmd.arg("--data-dir")
        .arg(data_dir)
        .arg("--runtime-file")
        .arg(runtime_file)
        .arg("--parent-pid")
        .arg(pid.to_string());

    if let Some(url) = admin_url {
        cmd.arg("--admin-url").arg(url);
    }

    if let Some(p) = port {
        cmd.arg("--port").arg(p.to_string());
    }

    if let Some(bin) = bifrost_bin {
        cmd.arg("--bifrost-bin").arg(bin);
    }

    for a in start_args {
        cmd.arg("--start-args").arg(a);
    }

    // Detach from parent process
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }

    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(
                tray_pid = child.id(),
                tray_bin = %tray_bin.display(),
                "tray helper launched"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                tray_bin = %tray_bin.display(),
                "failed to launch tray helper"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_launch_tray_disabled_by_flag() {
        assert!(!should_launch_tray(true));
    }

    #[test]
    fn test_should_launch_tray_enabled_on_non_linux() {
        if cfg!(target_os = "linux") {
            assert!(!should_launch_tray(false));
        } else {
            // Only true if BIFROST_DISABLE_TRAY is not set
            let prev = std::env::var("BIFROST_DISABLE_TRAY").ok();
            std::env::remove_var("BIFROST_DISABLE_TRAY");
            assert!(should_launch_tray(false));
            if let Some(v) = prev {
                std::env::set_var("BIFROST_DISABLE_TRAY", v);
            }
        }
    }

    #[test]
    fn test_should_launch_tray_disabled_by_env() {
        let prev = std::env::var("BIFROST_DISABLE_TRAY").ok();
        std::env::set_var("BIFROST_DISABLE_TRAY", "1");
        assert!(!should_launch_tray(false));
        match prev {
            Some(v) => std::env::set_var("BIFROST_DISABLE_TRAY", v),
            None => std::env::remove_var("BIFROST_DISABLE_TRAY"),
        }
    }

    #[test]
    fn test_find_tray_binary_from_env() {
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("bifrost-tray");
        std::fs::write(&bin_path, "").unwrap();
        std::env::set_var("BIFROST_TRAY_BIN", bin_path.to_str().unwrap());
        let result = find_tray_binary();
        assert_eq!(result, Some(bin_path));
        std::env::remove_var("BIFROST_TRAY_BIN");
    }

    #[test]
    fn test_find_tray_binary_env_missing_file() {
        std::env::set_var("BIFROST_TRAY_BIN", "/nonexistent/path/bifrost-tray");
        let result = find_tray_binary();
        // Falls through to sibling check
        std::env::remove_var("BIFROST_TRAY_BIN");
        // Result depends on whether current exe has a sibling - just verify no panic
        let _ = result;
    }
}
