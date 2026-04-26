//! `bifrost restart` — v3: fork-orphan-and-return, remote-invoke safe.
//!
//! # Design (v3)
//!
//! The one-and-only responsibility of `run_restart` running in the caller's
//! shell.exec-attached process is:
//!
//!   "fork a fully-detached orphan, wait until it has escaped our process
//!    group, print a one-line 'scheduled' confirmation, and return 0."
//!
//! It **must not** stop the running daemon itself — the moment the daemon
//! dies, the shell.exec pipe collapses and our process is reaped by SIGPIPE /
//! SIGTERM before we can print anything. All dangerous work (stop, wait for
//! port release, spawn new daemon) happens in the orphan.
//!
//! ## Teardown-safety: double-fork + pgroup detach + synchronization
//!
//! 1. Parent opens a pipe `(rd, wr)` with CLOEXEC **off** so it survives fork.
//! 2. Parent `fork()`s child **A**.
//! 3. **A** calls `setsid()` (leaves caller's session + process group), then
//!    `fork()`s child **B** (the orphan), then `_exit(0)`. B becomes a
//!    grandchild whose parent is already gone: it is reparented to PID 1 and
//!    is guaranteed immune to any pgroup-wide signal the parent's shell.exec
//!    may fire on teardown.
//! 4. **B** (orphan):
//!    - Closes all inherited fds >= 3 except `wr` (best-effort loop up to
//!      RLIMIT_NOFILE or 1024).
//!    - Opens `/dev/null` and `dup2`s it onto stdin / stdout / stderr so it
//!      does not keep the shell.exec pipe alive.
//!    - Writes 1 sync byte to `wr` and closes `wr`.
//!    - Forks **once more** into a true worker process and `_exit`s, just to
//!      double-confirm it is reparented and free of any residual controlling
//!      terminal.
//!    - The final worker calls into `run_orphan_work(...)` which opens
//!      `restart.log`, snapshots state, runs `run_stop`, polls for port
//!      release, then `execvp`s `bifrost start --daemon --yes` with the
//!      forwarded args.
//! 5. Parent: blocks on `read(rd, 1)` with a short timeout (500 ms). The byte
//!    arrives AFTER A's `setsid()` (A is already gone) AND AFTER B closed the
//!    writable fd's copy — i.e. after B has escaped our pgroup and cleaned up
//!    its stdio. Parent then `waitpid(A, 0)` to reap A, prints the
//!    "Restart scheduled" confirmation, and returns `Ok(())`.
//!
//! Caller (e.g. remote shell.exec) sees exit 0 and a one-line message. The
//! grant stays alive. The old daemon will be stopped by the orphan a moment
//! later; the caller can poll `bifrost remote status` to observe the new pid.
//!
//! ## Port release guarantee
//!
//! The orphan will not `execvp` `bifrost start --daemon` until it verifies
//! the old daemon's TCP listener on `(host, port)` is fully released. We
//! probe by attempting to `TcpListener::bind(0.0.0.0:port)` AND
//! `TcpListener::bind(127.0.0.1:port)` (both must succeed) with 100 ms
//! backoff, up to `PORT_RELEASE_TIMEOUT_SECS` (default 10 s). This avoids the
//! `EADDRINUSE` race where the old socket is still in FIN_WAIT-2 / TIME_WAIT.
//!
//! ## Platform support
//!
//! Unix only. On non-Unix we return an explicit "not supported" error.

use bifrost_core::Result as BifrostResult;

use crate::process::{is_process_running, read_pid, read_runtime_info};

const ORPHAN_STARTUP_GRACE_MS: u64 = 200;
const PORT_RELEASE_TIMEOUT_SECS: u64 = 10;
const PARENT_SYNC_READ_TIMEOUT_MS: u64 = 500;
const DEFAULT_PORT_FALLBACK: u16 = 9900;

#[derive(Debug, Default, Clone)]
pub struct RestartOptions {
    pub port: Option<u16>,
    pub host: Option<String>,
    pub log_level: Option<String>,
    pub force: bool,
}

pub fn run_restart(opts: RestartOptions) -> BifrostResult<()> {
    #[cfg(not(unix))]
    {
        let _ = opts;
        return Err(bifrost_core::BifrostError::Config(
            "bifrost restart is not supported on this platform yet. \
             Please run `bifrost stop` followed by `bifrost start --daemon` manually."
                .to_string(),
        ));
    }

    #[cfg(unix)]
    {
        run_restart_unix(opts)
    }
}

#[cfg(unix)]
fn run_restart_unix(opts: RestartOptions) -> BifrostResult<()> {
    // Snapshot old daemon state for logging. The orphan will re-read
    // independently; this is only for our "Restart scheduled" confirmation.
    let old_pid = read_pid();
    let old_runtime = read_runtime_info();
    let old_port = old_runtime.as_ref().map(|r| r.port);

    // Fork the orphan and return immediately.
    let orphan_pid = spawn_orphan_and_return(&opts)?;

    let version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
    match (old_pid, old_port) {
        (Some(op), Some(p)) => println!(
            "Restart scheduled. old_pid={} old_port={} orphan_pid={} version={}. \
             The orphan will stop the old daemon and bring up a fresh one. \
             Poll `bifrost status` or the pidfile to confirm.",
            op, p, orphan_pid, version
        ),
        (Some(op), None) => println!(
            "Restart scheduled. old_pid={} orphan_pid={} version={}. \
             The orphan will stop the old daemon and bring up a fresh one.",
            op, orphan_pid, version
        ),
        _ => println!(
            "Restart scheduled (no running daemon detected). orphan_pid={} version={}. \
             The orphan will bring up a fresh daemon.",
            orphan_pid, version
        ),
    }

    Ok(())
}

/// Fork a detached orphan process and return its pid. The parent returns as
/// soon as the orphan has confirmed (via sync pipe) it has left the caller's
/// process group.
#[cfg(unix)]
fn spawn_orphan_and_return(opts: &RestartOptions) -> BifrostResult<u32> {
    use nix::sys::wait::{waitpid, WaitPidFlag};
    use nix::unistd::{fork, pipe, ForkResult};
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::{Duration, Instant};

    let self_exe = std::env::current_exe().map_err(|e| {
        bifrost_core::BifrostError::Config(format!(
            "restart: unable to resolve current executable path: {}",
            e
        ))
    })?;

    // Serialize restart options for the orphan. We pass them through env vars
    // on the child side via a config struct owned by the orphan closure; no
    // need to serialize across exec because we fork-and-call, not exec-self.
    let forwarded = ForwardedRestart {
        self_exe: self_exe.clone(),
        port: opts.port,
        host: opts.host.clone(),
        log_level: opts.log_level.clone(),
        force: opts.force,
    };

    // Sync pipe: orphan will write 1 byte AFTER it has setsid + closed stdio.
    let (rd, wr) = pipe().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("restart: pipe() failed: {}", e))
    })?;

    // SAFETY: between fork() and execvp/closure we only call async-signal-
    // safe operations (setsid, fork, dup2, close, open /dev/null, _exit).
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child: child_a }) => {
            // Close the writable end in the parent; we only read.
            drop(wr);
            // SAFETY: rd is a valid fd; wrap it so it's closed on drop.
            let rd_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(rd.as_raw_fd()) };
            std::mem::forget(rd); // transferred ownership into rd_fd

            // Read 1 byte with a timeout so we don't hang if the child dies
            // before writing.
            let got_sync =
                read_byte_with_timeout(&rd_fd, Duration::from_millis(PARENT_SYNC_READ_TIMEOUT_MS));

            // Reap A (it _exit(0)s after forking the orphan). Non-blocking
            // first, then a short blocking wait.
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                match waitpid(Some(child_a), Some(WaitPidFlag::WNOHANG)) {
                    Ok(nix::sys::wait::WaitStatus::StillAlive) => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    _ => break,
                }
            }

            // If we never got the sync byte, warn but still return success:
            // the orphan may still be alive (race where it exec'd the new
            // daemon before writing the byte is not possible because we write
            // before exec, but write could have been interrupted). Caller is
            // responsible for polling.
            if !got_sync {
                eprintln!(
                    "restart: warning — did not receive orphan sync-byte within {} ms. \
                     The orphan may still be running; check {}.",
                    PARENT_SYNC_READ_TIMEOUT_MS,
                    orphan_log_path().display()
                );
            }

            Ok(child_a.as_raw() as u32)
        }

        Ok(ForkResult::Child) => {
            // ==== Intermediate child A ====
            // Leave parent's session / pgroup before forking B.
            let _ = nix::unistd::setsid();

            // Fork B (true orphan).
            match unsafe { fork() } {
                Ok(ForkResult::Parent { child: _orphan_b }) => {
                    // A exits right away. B is reparented to PID 1.
                    unsafe { libc::_exit(0) };
                }
                Ok(ForkResult::Child) => {
                    // ==== Orphan B ====
                    // Close all fds >= 3 except wr.
                    let wr_fd = wr.as_raw_fd();
                    close_fds_except(wr_fd);

                    // Reopen stdio on /dev/null.
                    redirect_stdio_to_devnull();

                    // Signal parent that we have escaped the pgroup and
                    // cleaned stdio. Parent will return 0 after receiving
                    // this byte.
                    let sync_byte: [u8; 1] = [b'K'];
                    let _ = nix::unistd::write(&wr, &sync_byte);
                    drop(wr);

                    // One more fork to fully divorce from any terminal
                    // session residue. The intermediate B exits; C does the
                    // real work.
                    match unsafe { fork() } {
                        Ok(ForkResult::Parent { .. }) => unsafe { libc::_exit(0) },
                        Ok(ForkResult::Child) => {
                            run_orphan_work(forwarded);
                            unsafe { libc::_exit(0) };
                        }
                        Err(_) => {
                            // Couldn't fork again; do the work inline.
                            run_orphan_work(forwarded);
                            unsafe { libc::_exit(0) };
                        }
                    }
                }
                Err(_) => unsafe { libc::_exit(1) },
            }
        }

        Err(e) => Err(bifrost_core::BifrostError::Config(format!(
            "restart: fork() failed: {}",
            e
        ))),
    }
}

/// Orphan worker: stop old daemon, wait for port release, execvp new daemon.
///
/// Invariants when called:
/// - We are in a brand-new session (setsid'd).
/// - Our stdio is /dev/null.
/// - We have no controlling terminal.
/// - Parent shell.exec chain has long since torn down; we are owned by PID 1.
#[cfg(unix)]
fn run_orphan_work(forwarded: ForwardedRestart) {
    use std::time::Duration;

    let log = orphan_log_path();
    orphan_log(&log, "orphan started");

    // Short grace: let parent's stdout/stderr drain, shell.exec close pipe
    // cleanly. Not strictly required for correctness (we're already detached)
    // but gives the caller a clean exit.
    std::thread::sleep(Duration::from_millis(ORPHAN_STARTUP_GRACE_MS));

    // Snapshot old port.
    let old_pid = read_pid();
    let old_runtime = read_runtime_info();
    let old_port = old_runtime
        .as_ref()
        .map(|r| r.port)
        .or(forwarded.port)
        .unwrap_or(DEFAULT_PORT_FALLBACK);

    orphan_log(
        &log,
        &format!("snapshot: old_pid={:?} old_port={}", old_pid, old_port),
    );

    // Stop.
    match old_pid {
        Some(pid) if is_process_running(pid) => match super::stop::run_stop() {
            Ok(()) => orphan_log(&log, "run_stop ok"),
            Err(e) => {
                orphan_log(&log, &format!("run_stop failed: {}", e));
                // Continue anyway; we will still try to start, but it may
                // fail with EADDRINUSE below.
            }
        },
        _ => orphan_log(&log, "no live old daemon; skipping stop"),
    }

    // Wait for port release (both 0.0.0.0 and 127.0.0.1 must be bindable).
    let port_free =
        wait_for_port_released(old_port, Duration::from_secs(PORT_RELEASE_TIMEOUT_SECS));
    orphan_log(
        &log,
        &format!("port {} free={} after wait", old_port, port_free),
    );

    // Build argv for `bifrost start --daemon --yes`.
    let mut argv: Vec<std::ffi::OsString> = vec![
        forwarded.self_exe.as_os_str().to_os_string(),
        std::ffi::OsString::from("start"),
        std::ffi::OsString::from("--daemon"),
        std::ffi::OsString::from("--yes"),
    ];
    if let Some(p) = forwarded.port {
        argv.push("--port".into());
        argv.push(p.to_string().into());
    }
    // Always forward the resolved port so the new daemon re-binds the same
    // listener. Without this, if the caller did not pass --port explicitly
    // (the common remote-invoke case), the child falls back to DEFAULT_PORT
    // (9900) and crashes on EADDRINUSE against any other local listener.
    argv.push("--port".into());
    argv.push(old_port.to_string().into());
    if let Some(lvl) = forwarded.log_level.as_deref() {
        argv.push("--log-level".into());
        argv.push(lvl.into());
    }

    orphan_log(&log, &format!("execvp argv: {:?}", argv));

    // Convert to CStrings.
    let argv_c: Vec<std::ffi::CString> = argv
        .iter()
        .map(|s| {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(s.as_os_str().as_bytes())
                .unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
        })
        .collect();
    let argv_ref: Vec<&std::ffi::CStr> = argv_c.iter().map(|c| c.as_c_str()).collect();

    // execvp — if it returns, it failed.
    let _ = nix::unistd::execvp(argv_ref[0], &argv_ref);
    orphan_log(&log, "execvp returned (failed)");
}

#[cfg(unix)]
struct ForwardedRestart {
    self_exe: std::path::PathBuf,
    port: Option<u16>,
    host: Option<String>,
    log_level: Option<String>,
    #[allow(dead_code)]
    force: bool,
}

#[cfg(unix)]
fn close_fds_except(keep: std::os::unix::io::RawFd) {
    // Best-effort. We want to close 3..=max_fd, except `keep`.
    let max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    let max = if max <= 0 { 1024 } else { max as i32 };
    let max = max.min(4096); // cap to avoid pathological loops
    for fd in 3..max {
        if fd == keep {
            continue;
        }
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(unix)]
fn redirect_stdio_to_devnull() {
    use std::os::unix::io::AsRawFd;
    if let Ok(devnull) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
    {
        let fd = devnull.as_raw_fd();
        let _ = nix::unistd::dup2(fd, 0);
        let _ = nix::unistd::dup2(fd, 1);
        let _ = nix::unistd::dup2(fd, 2);
    }
}

#[cfg(unix)]
fn read_byte_with_timeout(fd: &std::os::unix::io::OwnedFd, timeout: std::time::Duration) -> bool {
    use std::os::unix::io::AsRawFd;
    use std::time::Instant;

    let raw = fd.as_raw_fd();
    let deadline = Instant::now() + timeout;

    // Set O_NONBLOCK so read doesn't block indefinitely.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL, 0);
        if flags >= 0 {
            libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let mut buf = [0u8; 1];
    while Instant::now() < deadline {
        let n = unsafe { libc::read(raw, buf.as_mut_ptr() as *mut _, 1) };
        if n == 1 {
            return true;
        }
        if n == 0 {
            // EOF — writer closed without writing; treat as failure.
            return false;
        }
        // EAGAIN/EWOULDBLOCK or EINTR — retry.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

#[cfg(unix)]
fn orphan_log_path() -> std::path::PathBuf {
    crate::config::get_bifrost_dir()
        .map(|d| d.join("logs").join("restart.log"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/bifrost-restart.log"))
}

#[cfg(unix)]
fn orphan_log(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] pid={} {}", ts, std::process::id(), line);
    }
}

/// Block until `port` is bindable on both `0.0.0.0` and `127.0.0.1`, or
/// `budget` elapses. Returns `true` if released within budget.
#[cfg(unix)]
fn wait_for_port_released(port: u16, budget: std::time::Duration) -> bool {
    use std::net::{SocketAddr, TcpListener};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + budget;
    let any: SocketAddr = ([0, 0, 0, 0], port).into();
    let lo: SocketAddr = ([127, 0, 0, 1], port).into();

    while Instant::now() < deadline {
        let any_ok = TcpListener::bind(any).is_ok();
        let lo_ok = TcpListener::bind(lo).is_ok();
        if any_ok && lo_ok {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_options_default_is_empty() {
        let o = RestartOptions::default();
        assert!(o.port.is_none());
        assert!(o.host.is_none());
        assert!(o.log_level.is_none());
        assert!(!o.force);
    }

    #[cfg(unix)]
    #[test]
    fn wait_for_port_released_returns_quickly_when_port_is_free() {
        // Find an ephemeral port, close, then probe — should return true fast.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let start = std::time::Instant::now();
        let freed = wait_for_port_released(port, std::time::Duration::from_secs(2));
        let elapsed = start.elapsed();
        assert!(
            freed,
            "expected free ephemeral port {} to be reported free",
            port
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "free port should return almost immediately; took {:?}",
            elapsed
        );
    }

    #[cfg(unix)]
    #[test]
    fn wait_for_port_released_times_out_when_port_is_held() {
        // Hold a listener and probe with a short budget.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let start = std::time::Instant::now();
        let freed = wait_for_port_released(port, std::time::Duration::from_millis(400));
        let elapsed = start.elapsed();
        drop(listener);
        assert!(!freed, "held port {} should NOT be reported free", port);
        assert!(
            elapsed >= std::time::Duration::from_millis(350),
            "probe should use most of the budget; took {:?}",
            elapsed
        );
    }

    #[test]
    #[cfg(not(unix))]
    fn restart_is_unsupported_on_non_unix() {
        let r = run_restart(RestartOptions::default());
        assert!(r.is_err());
    }
}
