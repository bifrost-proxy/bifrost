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
//! later; the caller can poll `bifrost remote conn status` to observe the new pid.
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

#[cfg(unix)]
use crate::process::{
    capture_runtime_system_proxy_snapshot, is_process_running, read_pid, read_runtime_info,
    RuntimeSystemProxySnapshot,
};

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
    use std::os::unix::io::{FromRawFd, IntoRawFd, OwnedFd};
    use std::time::{Duration, Instant};

    let self_exe = std::env::current_exe().map_err(|e| {
        bifrost_core::BifrostError::Config(format!(
            "restart: unable to resolve current executable path: {}",
            e
        ))
    })?;

    // Package restart options into a struct that the orphan worker will own
    // after fork. No env vars / no re-exec of self are involved — we just
    // fork, hand the struct to run_orphan_work, then execvp `bifrost start`.
    let forwarded = ForwardedRestart {
        self_exe: self_exe.clone(),
        port: opts.port,
        host: opts.host.clone(),
        log_level: opts.log_level.clone(),
        force: opts.force,
    };

    // Sync pipe: orphan will write 1 byte AFTER it has setsid + closed stdio.
    //
    // We convert both ends into *raw* fds immediately. OwnedFd's Drop would
    // otherwise run on both sides of every fork() and silently double-close
    // the shared fd-table entries. With raw fds, each side closes exactly
    // the fd it owns via libc::close and we keep the whole handoff
    // async-signal-safe.
    let (rd_owned, wr_owned) = pipe().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("restart: pipe() failed: {}", e))
    })?;
    let rd_raw = rd_owned.into_raw_fd();
    let wr_raw = wr_owned.into_raw_fd();

    // SAFETY: between fork() and execvp/closure we only call async-signal-
    // safe operations (setsid, fork, libc::dup2, libc::close, libc::open
    // /dev/null, libc::write, _exit). No Rust Drop impls are run in the
    // child post-fork because we only handle raw fds.
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child: child_a }) => {
            // Parent only reads: close wr_raw and rebuild rd_raw into an
            // OwnedFd for RAII close on function return.
            unsafe { libc::close(wr_raw) };
            // SAFETY: rd_raw came straight out of our own into_raw_fd() on
            // an OwnedFd we just created; nobody else has a reference to it.
            let rd_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(rd_raw) };

            // Read 1 byte with a timeout so we don't hang if the child dies
            // before writing.
            let got_sync =
                read_byte_with_timeout(&rd_fd, Duration::from_millis(PARENT_SYNC_READ_TIMEOUT_MS));

            // Reap A (it _exit(0)s after forking the orphan). Non-blocking
            // first, then a short blocking wait.
            let deadline = Instant::now() + Duration::from_millis(300);
            while let Ok(nix::sys::wait::WaitStatus::StillAlive) =
                waitpid(Some(child_a), Some(WaitPidFlag::WNOHANG))
            {
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
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
            // A has no business with the read end — close it immediately
            // so only the orphan B (which inherits wr_raw) can keep the
            // pipe alive.
            unsafe { libc::close(rd_raw) };

            // Leave parent's session / pgroup before forking B.
            let _ = nix::unistd::setsid();

            // Fork B (true orphan).
            match unsafe { fork() } {
                Ok(ForkResult::Parent { child: _orphan_b }) => {
                    // A exits right away. B is reparented to PID 1. Closing
                    // wr_raw in A is important: otherwise the parent would
                    // block on read() until A itself exits, racing the
                    // waitpid loop below and making `got_sync` flap.
                    unsafe { libc::close(wr_raw) };
                    unsafe { libc::_exit(0) };
                }
                Ok(ForkResult::Child) => {
                    // ==== Orphan B ====
                    // Close every inherited fd >= 3 except wr_raw.
                    close_fds_except(wr_raw);

                    // Reopen stdio on /dev/null.
                    redirect_stdio_to_devnull();

                    // Signal parent that we have escaped the pgroup and
                    // cleaned stdio. Use libc::write (async-signal-safe)
                    // instead of going through nix::unistd::write, which
                    // borrows an OwnedFd we'd rather not construct here.
                    let sync_byte: [u8; 1] = [b'K'];
                    unsafe {
                        libc::write(wr_raw, sync_byte.as_ptr() as *const _, 1);
                        libc::close(wr_raw);
                    }

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
    let system_proxy_snapshot = capture_runtime_system_proxy_snapshot(old_runtime.as_ref());
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
        Some(pid) if is_process_running(pid) => match super::stop::run_stop_for_restart() {
            Ok(()) => orphan_log(&log, "run_stop ok"),
            Err(e) => {
                orphan_log(&log, &format!("run_stop failed: {}", e));
                if !forwarded.force {
                    abort_restart_handoff(
                        &log,
                        "stop failed before restart handoff",
                        old_pid,
                        system_proxy_snapshot.is_some(),
                    );
                    return;
                }
                orphan_log(
                    &log,
                    "run_stop failed but --force was requested; continuing with restart handoff",
                );
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

    // P2-2: do NOT execvp the new daemon if the port is still occupied after
    // the full budget. Exec'ing would just crash with EADDRINUSE and leave
    // the user with no daemon at all (we already stopped the old one).
    //
    // `--force` (set by the operator) is an explicit override: bring up the
    // new daemon anyway and let it surface the bind failure; this is mostly
    // useful for debugging or recovering from a wedged pidfile.
    if !port_free && !forwarded.force {
        orphan_log(
            &log,
            &format!(
                "CRITICAL: port {} still occupied after {}s; aborting restart                  to avoid an EADDRINUSE crash. Re-run `bifrost restart --force`                  if you want to try anyway.",
                old_port, PORT_RELEASE_TIMEOUT_SECS
            ),
        );
        abort_restart_handoff(
            &log,
            "port did not release before restart handoff",
            old_pid,
            system_proxy_snapshot.is_some(),
        );
        return;
    }

    // Build argv for `bifrost start --daemon --yes`.
    //
    // Port resolution order: explicit --port from caller > runtime-file old
    // port > DEFAULT_PORT_FALLBACK. We only push --port once to keep the
    // argv clean (multiple --port entries would be last-wins but noisy).
    let resolved_port = forwarded.port.unwrap_or(old_port);
    let mut argv: Vec<std::ffi::OsString> = vec![
        forwarded.self_exe.as_os_str().to_os_string(),
        std::ffi::OsString::from("start"),
        std::ffi::OsString::from("--daemon"),
        std::ffi::OsString::from("--yes"),
        std::ffi::OsString::from("--port"),
        resolved_port.to_string().into(),
    ];
    if let Some(h) = forwarded.host.as_deref() {
        argv.push("--host".into());
        argv.push(h.into());
    }
    if let Some(lvl) = forwarded.log_level.as_deref() {
        argv.push("--log-level".into());
        argv.push(lvl.into());
    }
    append_system_proxy_start_args(&mut argv, system_proxy_snapshot.as_ref());

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
    abort_restart_handoff(
        &log,
        "execvp failed after restart handoff",
        old_pid,
        system_proxy_snapshot.is_some(),
    );
}

#[cfg(unix)]
struct ForwardedRestart {
    self_exe: std::path::PathBuf,
    port: Option<u16>,
    host: Option<String>,
    log_level: Option<String>,
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
fn append_system_proxy_start_args(
    argv: &mut Vec<std::ffi::OsString>,
    snapshot: Option<&RuntimeSystemProxySnapshot>,
) {
    if let Some(snapshot) = snapshot {
        argv.push("--system-proxy".into());
        argv.push("--proxy-bypass".into());
        argv.push(snapshot.bypass.clone().into());
    }
}

#[cfg(unix)]
fn abort_restart_handoff(
    log: &std::path::Path,
    reason: &str,
    old_pid: Option<u32>,
    preserved_system_proxy: bool,
) {
    orphan_log(log, &format!("aborting restart handoff: {reason}"));
    if let Ok(data_dir) = crate::config::get_bifrost_dir() {
        let _ = bifrost_core::consume_system_proxy_shutdown_mode(&data_dir);
        let old_runtime_still_alive = old_pid.is_some_and(is_process_running);
        if preserved_system_proxy && !old_runtime_still_alive {
            match bifrost_core::SystemProxyManager::recover_from_crash(&data_dir) {
                Ok(()) => orphan_log(log, "system proxy recovered after aborted restart handoff"),
                Err(error) => orphan_log(
                    log,
                    &format!("system proxy recovery failed after aborted restart handoff: {error}"),
                ),
            }
        } else {
            orphan_log(
                log,
                &format!(
                    "system proxy recovery skipped after aborted restart handoff: preserved_system_proxy={preserved_system_proxy} old_runtime_still_alive={old_runtime_still_alive}"
                ),
            );
        }
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
        // The OS can hand the just-released ephemeral port to another
        // concurrently running test or daemon. Retry a few candidates so this
        // test checks our wait logic instead of a transient port race.
        let mut attempts = Vec::new();
        for _ in 0..10 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let start = std::time::Instant::now();
            let freed = wait_for_port_released(port, std::time::Duration::from_secs(2));
            let elapsed = start.elapsed();
            attempts.push((port, freed, elapsed));
            if freed {
                assert!(
                    elapsed < std::time::Duration::from_millis(500),
                    "free port should return almost immediately; took {:?}",
                    elapsed
                );
                return;
            }
        }
        panic!("expected one free ephemeral port to be reported free; attempts={attempts:?}");
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

    #[cfg(unix)]
    #[test]
    fn append_system_proxy_start_args_preserves_bypass() {
        let mut argv = vec![
            std::ffi::OsString::from("bifrost"),
            std::ffi::OsString::from("start"),
            std::ffi::OsString::from("--daemon"),
        ];
        let snapshot = RuntimeSystemProxySnapshot {
            bypass: "localhost,127.0.0.1,*.local".to_string(),
        };

        append_system_proxy_start_args(&mut argv, Some(&snapshot));

        assert_eq!(
            argv,
            vec![
                std::ffi::OsString::from("bifrost"),
                std::ffi::OsString::from("start"),
                std::ffi::OsString::from("--daemon"),
                std::ffi::OsString::from("--system-proxy"),
                std::ffi::OsString::from("--proxy-bypass"),
                std::ffi::OsString::from("localhost,127.0.0.1,*.local"),
            ]
        );
    }

    #[test]
    #[cfg(not(unix))]
    fn restart_is_unsupported_on_non_unix() {
        let r = run_restart(RestartOptions::default());
        assert!(r.is_err());
    }
}
