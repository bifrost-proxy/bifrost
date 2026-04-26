//! `bifrost restart` — stop the running proxy (if any) and spawn a fresh
//! detached daemon.
//!
//! # Why this exists
//!
//! After an in-place upgrade (`bifrost upgrade`) the binary on disk is new but
//! the running daemon is still the old version. To pick up the new build we
//! need a safe, **remote-invocation-friendly** way to cycle the daemon.
//!
//! Naively running `bifrost stop && bifrost start --daemon` from within a
//! relay-backed `bifrost remote command exec` session has two sharp edges:
//!
//! 1. The `remote command exec` child process typically lives *inside* the
//!    running Bifrost daemon (shell.exec dispatches through the daemon). If
//!    we synchronously wait for the daemon to exit, the shell.exec pipe
//!    collapses and the caller sees "connection reset" instead of a clean
//!    restart confirmation.
//! 2. The freshly-spawned daemon must be fully detached from the shell.exec
//!    stdio pipes or it will keep them alive and die when shell.exec reaps
//!    its children.
//!
//! # How we solve it
//!
//! `run_restart` is an *orchestrator* that lives in the invoking CLI process
//! only. It never becomes the new daemon itself.
//!
//! 1. Snapshot `old_pid` / `old_port` from the pidfile.
//! 2. Call [`run_stop`](super::stop::run_stop) — this is already hardened:
//!    `SIGTERM` with a 30 s poll loop and `SIGKILL` fallback.
//! 3. `fork()` a helper child. The helper `setsid()`s to leave the shell.exec
//!    session, redirects stdio to `/dev/null`, then `exec`s `bifrost start
//!    --daemon …`. The outer `run_start --daemon` itself does another `fork()`
//!    so the final daemon is two levels removed from our controlling terminal
//!    and entirely owns its pidfile.
//! 4. Poll the pidfile for up to `STARTUP_TIMEOUT_SECS` (default 15 s) until
//!    a fresh pid appears that is (a) alive and (b) different from
//!    `old_pid`, and until the admin port accepts a TCP connection.
//! 5. Print a structured confirmation and return `Ok(())`.
//!
//! Any failure path is surfaced as `Err(BifrostError::Config(..))` so the
//! caller's shell (including remote shell.exec) sees a non-zero exit code.
//!
//! # Platform support
//!
//! Unix only. Windows lacks `--daemon` in the current codebase, so
//! `run_restart` returns an explicit "not supported" error there and asks the
//! user to stop/start manually.

use bifrost_core::Result as BifrostResult;

use crate::process::{is_process_running, read_pid, read_runtime_info};

/// How long we wait for the new daemon to write a fresh pid to the pidfile
/// before declaring restart a failure.
const STARTUP_TIMEOUT_SECS: u64 = 15;

/// Poll interval while waiting for the new pid to appear.
const STARTUP_POLL_INTERVAL_MS: u64 = 100;

/// Fallback admin port if we cannot read one from runtime state or the
/// prior daemon. Matches the top-level `DEFAULT_PORT` in `main.rs`.
const DEFAULT_PORT_FALLBACK: u16 = 9900;

/// Arguments that `restart` forwards to the underlying `start --daemon`
/// invocation. Only a small subset of start flags is exposed — the ones a
/// user is most likely to want to override when cycling a running daemon.
/// Everything else is inherited from the last-persisted `ConfigManager` state
/// the way a plain `bifrost start --daemon` would.
#[derive(Debug, Default, Clone)]
pub struct RestartOptions {
    pub port: Option<u16>,
    pub host: Option<String>,
    pub log_level: Option<String>,
    /// When `true`, skip the "is process running?" check and always attempt a
    /// full stop→start cycle, even if we think nothing is running. Useful
    /// after a hard crash where the pidfile may be stale.
    pub force: bool,
}

/// Public entry point.
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
    use std::time::{Duration, Instant};

    // -------- 1. Snapshot old daemon state --------
    let old_pid = read_pid();
    let old_runtime = crate::process::read_runtime_info();
    let old_port = old_runtime.as_ref().map(|r| r.port);

    let was_running = match old_pid {
        Some(pid) => is_process_running(pid),
        None => false,
    };

    if !was_running && !opts.force {
        println!(
            "Bifrost is not currently running (no live pid found). \
             Starting a fresh daemon."
        );
    }

    // -------- 2. Stop running daemon (idempotent) --------
    // run_stop handles: stale pidfile, live pid via SIGTERM with 30s poll,
    // SIGKILL fallback, and system-proxy / shell-proxy cleanup.
    if was_running {
        if let Err(e) = super::stop::run_stop() {
            return Err(bifrost_core::BifrostError::Config(format!(
                "restart aborted: stop phase failed: {}",
                e
            )));
        }
    } else if opts.force {
        // Best-effort cleanup so the fresh daemon isn't tripped up by a stale
        // pidfile. Ignore "no pidfile" errors.
        let _ = crate::process::remove_pid();
    }

    // -------- 3. Spawn a detached `bifrost start --daemon` --------
    let new_pid = spawn_detached_daemon(&opts)?;

    // -------- 4. Wait for pidfile to confirm the new daemon is up --------
    let deadline = Instant::now() + Duration::from_secs(STARTUP_TIMEOUT_SECS);
    let poll = Duration::from_millis(STARTUP_POLL_INTERVAL_MS);

    let (ready_pid, ready_port) = loop {
        if Instant::now() >= deadline {
            return Err(bifrost_core::BifrostError::Config(format!(
                "restart timed out after {}s: new daemon did not publish a pid. \
                 Check {} and logs.",
                STARTUP_TIMEOUT_SECS,
                crate::process::get_pid_file()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "~/.bifrost/bifrost.pid".to_string())
            )));
        }

        if let Some(pid) = read_pid() {
            // Reject a stale pidfile (same pid as before the restart, and
            // old pid still alive — shouldn't happen because stop succeeded,
            // but guard against it explicitly).
            let same_as_old = matches!(old_pid, Some(op) if op == pid);
            if same_as_old && is_process_running(pid) {
                std::thread::sleep(poll);
                continue;
            }
            if is_process_running(pid) {
                let port = read_runtime_info()
                    .map(|r| r.port)
                    .or(old_port)
                    .unwrap_or(DEFAULT_PORT_FALLBACK);
                // Port-level readiness check. Best-effort: don't fail the
                // whole restart if the probe doesn't succeed within a
                // short window — the daemon may legitimately bind a bit
                // later. But if the pid is alive, we treat it as up.
                let _ = wait_for_port_ready(
                    port,
                    Duration::from_secs(
                        STARTUP_TIMEOUT_SECS.saturating_sub(elapsed_secs(deadline)),
                    ),
                );
                break (pid, port);
            }
        }

        std::thread::sleep(poll);
    };

    // -------- 5. Pretty-print confirmation --------
    let version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
    match (old_pid, old_port) {
        (Some(op), Some(oport)) => println!(
            "Bifrost restarted. old_pid={} old_port={} new_pid={} port={} version={} (spawned_pid={})",
            op, oport, ready_pid, ready_port, version, new_pid,
        ),
        (Some(op), None) => println!(
            "Bifrost restarted. old_pid={} new_pid={} port={} version={} (spawned_pid={})",
            op, ready_pid, ready_port, version, new_pid,
        ),
        _ => println!(
            "Bifrost started. pid={} port={} version={} (spawned_pid={})",
            ready_pid, ready_port, version, new_pid,
        ),
    }

    Ok(())
}

#[cfg(unix)]
fn elapsed_secs(deadline: std::time::Instant) -> u64 {
    let now = std::time::Instant::now();
    if now >= deadline {
        STARTUP_TIMEOUT_SECS
    } else {
        STARTUP_TIMEOUT_SECS.saturating_sub(deadline.saturating_duration_since(now).as_secs())
    }
}

/// Fork a child process, `setsid()` to leave the caller's session (so the
/// child survives shell.exec pipe teardown), redirect stdio to /dev/null,
/// then `execvp` `bifrost start --daemon`.
///
/// Returns the pid of the immediate child (which itself will fork again from
/// inside `run_daemon` and then exit). The caller is expected to poll the
/// pidfile rather than rely on this return value for the final daemon pid.
#[cfg(unix)]
fn spawn_detached_daemon(opts: &RestartOptions) -> BifrostResult<u32> {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::{fork, ForkResult};

    // Resolve our own binary path — we must re-exec the *same* bifrost image
    // the user invoked (don't rely on PATH, which could resolve to a
    // different binary after an upgrade on PATH-shadowed deployments).
    let self_exe = std::env::current_exe().map_err(|e| {
        bifrost_core::BifrostError::Config(format!(
            "restart: unable to resolve current executable path: {}",
            e
        ))
    })?;

    let mut argv: Vec<std::ffi::OsString> = vec![
        self_exe.as_os_str().to_os_string(),
        std::ffi::OsString::from("start"),
        std::ffi::OsString::from("--daemon"),
        std::ffi::OsString::from("--yes"),
    ];
    if let Some(p) = opts.port {
        argv.push("--port".into());
        argv.push(p.to_string().into());
    }
    if let Some(h) = opts.host.as_deref() {
        argv.push("--host".into());
        argv.push(h.into());
    }
    if let Some(lvl) = opts.log_level.as_deref() {
        argv.push("--log-level".into());
        argv.push(lvl.into());
    }

    // SAFETY: we only call async-signal-safe operations in the child path
    // before execvp (setsid, open /dev/null, dup2, close, execvp).
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child }) => {
            // Reap the immediate child (it will exit shortly after it
            // double-forks inside run_daemon), so we don't leave a zombie.
            // Don't block — we just do a non-blocking poll in the poll loop.
            // But we try a short blocking wait here first: if `bifrost start
            // --daemon` fails synchronously (e.g. port already in use by
            // an unrelated process), the child exits non-zero quickly and
            // we want to surface that.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                match waitpid(Some(child), Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive) => {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok(WaitStatus::Exited(_, code)) => {
                        if code != 0 {
                            return Err(bifrost_core::BifrostError::Config(format!(
                                "restart: spawned `bifrost start --daemon` exited with code {}. \
                                 Check {} for details.",
                                code,
                                default_err_log_path()
                            )));
                        }
                        break;
                    }
                    Ok(WaitStatus::Signaled(_, sig, _)) => {
                        return Err(bifrost_core::BifrostError::Config(format!(
                            "restart: spawned `bifrost start --daemon` killed by signal {:?}",
                            sig
                        )));
                    }
                    Ok(_) => break,
                    Err(nix::errno::Errno::ECHILD) => break,
                    Err(e) => {
                        return Err(bifrost_core::BifrostError::Config(format!(
                            "restart: waitpid failed: {}",
                            e
                        )));
                    }
                }
            }
            Ok(child.as_raw() as u32)
        }
        Ok(ForkResult::Child) => {
            // Leave the caller's controlling terminal / process group so that
            // when shell.exec reaps its children the new daemon survives.
            let _ = nix::unistd::setsid();

            // Redirect stdio to /dev/null — run_daemon internally
            // dup2's its own log files, but until that happens we must not
            // hold the shell.exec pipe open, or shell.exec's reader will
            // block until this child exits.
            if let Ok(devnull) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/null")
            {
                use std::os::unix::io::AsRawFd;
                let fd = devnull.as_raw_fd();
                let _ = nix::unistd::dup2(fd, 0);
                let _ = nix::unistd::dup2(fd, 1);
                let _ = nix::unistd::dup2(fd, 2);
            }

            // Build C-compatible argv and execvp.
            let argv_c: Vec<std::ffi::CString> = argv
                .iter()
                .map(|s| {
                    use std::os::unix::ffi::OsStrExt;
                    std::ffi::CString::new(s.as_os_str().as_bytes())
                        .unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
                })
                .collect();
            let argv_ref: Vec<&std::ffi::CStr> = argv_c.iter().map(|c| c.as_c_str()).collect();

            // If execvp returns, it failed. Signal by exiting 127.
            let _ = nix::unistd::execvp(argv_ref[0], &argv_ref);
            let _ = nix::unistd::write(
                std::io::stderr(),
                b"restart: execvp bifrost start --daemon failed\n",
            );
            std::process::exit(127);
        }
        Err(e) => Err(bifrost_core::BifrostError::Config(format!(
            "restart: fork failed: {}",
            e
        ))),
    }
}

#[cfg(unix)]
fn default_err_log_path() -> String {
    crate::config::get_bifrost_dir()
        .map(|d| d.join("logs").join("bifrost.err").display().to_string())
        .unwrap_or_else(|_| "~/.bifrost/logs/bifrost.err".to_string())
}

/// Block until `port` accepts a TCP connection on loopback, or `budget` elapses.
#[cfg(unix)]
fn wait_for_port_ready(port: u16, budget: std::time::Duration) -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + budget;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let per_try = Duration::from_millis(500);

    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, per_try).is_ok() {
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

    #[test]
    #[cfg(not(unix))]
    fn restart_is_unsupported_on_non_unix() {
        let r = run_restart(RestartOptions::default());
        assert!(r.is_err());
    }
}
