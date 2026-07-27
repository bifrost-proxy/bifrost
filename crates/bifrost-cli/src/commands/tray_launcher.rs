use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fs2::FileExt;

use super::tray::TRAY_SUBCOMMAND;
use bifrost_core::EXTERNAL_CLI_WORKER_ENV;

pub fn should_launch_tray(no_tray: bool, data_dir: &Path) -> bool {
    if cfg!(target_os = "linux") {
        return false;
    }
    if no_tray {
        return false;
    }
    if std::env::var("BIFROST_DISABLE_TRAY").as_deref() == Ok("1") {
        return false;
    }
    tray_enabled_by_config(data_dir)
}

pub(crate) fn tray_enabled_by_config(data_dir: &Path) -> bool {
    let path = data_dir.join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to read tray config; keeping tray enabled"
            );
            return true;
        }
    };

    match toml::from_str::<bifrost_storage::UnifiedConfig>(&content) {
        Ok(config) => config.tray.enabled,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to parse tray config; keeping tray enabled"
            );
            true
        }
    }
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
    if let Some(existing_pid) = existing_tray_helper_pid(data_dir) {
        tracing::info!(
            tray_pid = existing_pid,
            data_dir = %data_dir.display(),
            "tray helper already running; skipping launch"
        );
        return;
    }

    let mut cmd = Command::new(tray_bin);
    configure_tray_helper_environment(&mut cmd);
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

fn configure_tray_helper_environment(command: &mut Command) {
    command.env_remove(EXTERNAL_CLI_WORKER_ENV);
}

fn existing_tray_helper_pid(data_dir: &Path) -> Option<u32> {
    if !tray_lock_is_held(data_dir) {
        return None;
    }
    Some(read_tray_pid(data_dir).unwrap_or(0))
}

fn tray_lock_is_held(data_dir: &Path) -> bool {
    let lock_path = data_dir.join("tray.lock");
    if let Some(parent) = lock_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            tracing::warn!(
                path = %parent.display(),
                error = %error,
                "failed to create tray data dir before checking existing helper"
            );
            return false;
        }
    }

    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(
                path = %lock_path.display(),
                error = %error,
                "failed to open tray lock before launching helper"
            );
            return false;
        }
    };

    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => true,
        Err(error) => {
            tracing::warn!(
                path = %lock_path.display(),
                error = %error,
                "failed to probe tray lock before launching helper"
            );
            false
        }
    }
}

fn read_tray_pid(data_dir: &Path) -> Option<u32> {
    let pid_path = data_dir.join("tray.pid");
    std::fs::read_to_string(pid_path)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
}

pub fn stop_tray_helper(data_dir: &Path) {
    let Some(mut pid) = existing_tray_helper_pid(data_dir) else {
        let _ = fs::remove_file(data_dir.join("tray.pid"));
        return;
    };

    if pid == 0 {
        let started = Instant::now();
        while pid == 0 && started.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(50));
            pid = read_tray_pid(data_dir).unwrap_or(0);
        }
        if pid == 0 {
            tracing::warn!(
                data_dir = %data_dir.display(),
                "tray helper lock is held but tray.pid is missing; cannot stop helper by pid"
            );
            return;
        }
    }

    terminate_tray_helper_pid(pid);
    if !wait_for_tray_helper_exit(pid, Duration::from_secs(3)) {
        force_kill_tray_helper_pid(pid);
    }
    if wait_for_tray_helper_exit(pid, Duration::from_secs(2)) {
        let _ = fs::remove_file(data_dir.join("tray.pid"));
    }
}

#[cfg(unix)]
fn terminate_tray_helper_pid(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[cfg(windows)]
fn terminate_tray_helper_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return;
    }
    unsafe {
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_tray_helper_pid(_pid: u32) {}

#[cfg(unix)]
fn force_kill_tray_helper_pid(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn force_kill_tray_helper_pid(pid: u32) {
    terminate_tray_helper_pid(pid);
}

#[cfg(not(any(unix, windows)))]
fn force_kill_tray_helper_pid(_pid: u32) {}

fn wait_for_tray_helper_exit(pid: u32, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if !is_tray_helper_pid_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(unix)]
fn is_tray_helper_pid_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn is_tray_helper_pid_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }

    let mut exit_code = 0_u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe {
        CloseHandle(handle);
    }

    ok != 0 && exit_code == STILL_ACTIVE as u32
}

#[cfg(not(any(unix, windows)))]
fn is_tray_helper_pid_running(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Stdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct LockHolderChild {
        child: Child,
    }

    impl LockHolderChild {
        fn id(&self) -> u32 {
            self.child.id()
        }
    }

    impl Drop for LockHolderChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn spawn_lock_holder(data_dir: &Path, write_pid: bool) -> LockHolderChild {
        let test_exe = std::env::current_exe().unwrap();
        let ready_path = data_dir.join("tray-lock-child-ready");
        let mut child = Command::new(test_exe)
            .arg("--exact")
            .arg("commands::tray_launcher::tests::tray_lock_child_process")
            .arg("--nocapture")
            .env("BIFROST_TRAY_LOCK_CHILD_DIR", data_dir)
            .env("BIFROST_TRAY_LOCK_CHILD_READY", &ready_path)
            .env(
                "BIFROST_TRAY_LOCK_CHILD_WRITE_PID",
                if write_pid { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ready_path.exists() {
                return LockHolderChild { child };
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("tray lock child exited before ready: {status}");
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        let _ = child.kill();
        let _ = child.wait();
        panic!("timed out waiting for tray lock child");
    }

    #[test]
    fn tray_lock_child_process() {
        let Ok(data_dir) = std::env::var("BIFROST_TRAY_LOCK_CHILD_DIR") else {
            return;
        };
        let data_dir = PathBuf::from(data_dir);
        let ready_path = PathBuf::from(std::env::var("BIFROST_TRAY_LOCK_CHILD_READY").unwrap());
        std::fs::create_dir_all(&data_dir).unwrap();

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join("tray.lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();

        if std::env::var("BIFROST_TRAY_LOCK_CHILD_WRITE_PID").as_deref() == Ok("1") {
            std::fs::write(data_dir.join("tray.pid"), std::process::id().to_string()).unwrap();
        } else {
            let _ = std::fs::remove_file(data_dir.join("tray.pid"));
        }

        std::fs::write(ready_path, "ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
        let _ = lock.unlock();
    }

    #[test]
    fn test_should_launch_tray_disabled_by_flag() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_launch_tray(true, dir.path()));
    }

    #[test]
    fn tray_helper_command_clears_inherited_external_worker_marker() {
        let mut command = Command::new("bifrost");
        command.env(EXTERNAL_CLI_WORKER_ENV, "leaked-worker-role");

        configure_tray_helper_environment(&mut command);

        assert!(command.get_envs().any(|(key, value)| {
            key == std::ffi::OsStr::new(EXTERNAL_CLI_WORKER_ENV) && value.is_none()
        }));
    }

    #[test]
    fn test_should_launch_tray_enabled_on_non_linux() {
        let _guard = env_lock();
        if cfg!(target_os = "linux") {
            assert!(!should_launch_tray(
                false,
                tempfile::tempdir().unwrap().path()
            ));
        } else {
            // Only true if BIFROST_DISABLE_TRAY is not set
            let prev = std::env::var("BIFROST_DISABLE_TRAY").ok();
            std::env::remove_var("BIFROST_DISABLE_TRAY");
            let dir = tempfile::tempdir().unwrap();
            assert!(should_launch_tray(false, dir.path()));
            if let Some(v) = prev {
                std::env::set_var("BIFROST_DISABLE_TRAY", v);
            }
        }
    }

    #[test]
    fn test_should_launch_tray_disabled_by_env() {
        let _guard = env_lock();
        let prev = std::env::var("BIFROST_DISABLE_TRAY").ok();
        std::env::set_var("BIFROST_DISABLE_TRAY", "1");
        let dir = tempfile::tempdir().unwrap();
        assert!(!should_launch_tray(false, dir.path()));
        match prev {
            Some(v) => std::env::set_var("BIFROST_DISABLE_TRAY", v),
            None => std::env::remove_var("BIFROST_DISABLE_TRAY"),
        }
    }

    #[test]
    fn test_should_launch_tray_disabled_by_config() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[tray]\nenabled = false\n").unwrap();
        assert!(!should_launch_tray(false, dir.path()));
    }

    #[test]
    fn test_find_tray_binary_from_env() {
        let _guard = env_lock();
        let prev = std::env::var("BIFROST_TRAY_BIN").ok();
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("bifrost-tray");
        std::fs::write(&bin_path, "").unwrap();
        std::env::set_var("BIFROST_TRAY_BIN", bin_path.to_str().unwrap());
        let result = find_tray_binary();
        assert_eq!(result, Some(bin_path));
        match prev {
            Some(v) => std::env::set_var("BIFROST_TRAY_BIN", v),
            None => std::env::remove_var("BIFROST_TRAY_BIN"),
        }
    }

    #[test]
    fn test_find_tray_binary_env_missing_file() {
        let _guard = env_lock();
        let prev = std::env::var("BIFROST_TRAY_BIN").ok();
        std::env::set_var("BIFROST_TRAY_BIN", "/nonexistent/path/bifrost-tray");
        let result = find_tray_binary();
        // Falls through to sibling check
        match prev {
            Some(v) => std::env::set_var("BIFROST_TRAY_BIN", v),
            None => std::env::remove_var("BIFROST_TRAY_BIN"),
        }
        // Result depends on whether current exe has a sibling - just verify no panic
        let _ = result;
    }

    #[test]
    fn test_existing_tray_helper_pid_requires_active_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tray.pid"), "12345").unwrap();
        assert_eq!(existing_tray_helper_pid(dir.path()), None);

        let child = spawn_lock_holder(dir.path(), true);
        assert_eq!(existing_tray_helper_pid(dir.path()), Some(child.id()));
    }

    #[test]
    fn test_existing_tray_helper_pid_skips_when_lock_held_without_pid() {
        let dir = tempfile::tempdir().unwrap();
        let _child = spawn_lock_holder(dir.path(), false);
        assert_eq!(existing_tray_helper_pid(dir.path()), Some(0));
    }
}
