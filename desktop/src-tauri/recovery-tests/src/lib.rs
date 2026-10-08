//! Headless host for the production Desktop recovery guard and state machine.
//! Only Tauri error/UI plumbing and process launch readiness are replaced. Tests
//! include the same regression source used by the native Desktop test target.
#![cfg(test)]
#![allow(dead_code)]

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const BACKEND_WATCHDOG_MIN_FAILURES: u32 = 4;
const BACKEND_WATCHDOG_UNHEALTHY_GRACE: Duration = Duration::from_secs(15);
const BACKEND_WATCHDOG_MAX_RECOVERIES: usize = 3;
const BACKEND_WATCHDOG_RECOVERY_WINDOW: Duration = Duration::from_secs(300);
const BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(3);
const BACKEND_KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PORT_INCREMENT_ATTEMPTS: u16 = 64;
const BACKEND_ADMIN_HOST: &str = "127.0.0.1";
const BACKEND_BIND_HOST: &str = "0.0.0.0";

mod tauri {
    pub type Result<T> = std::result::Result<T, String>;
}

#[derive(Deserialize)]
struct DesktopRuntimeMarker {
    pid: u32,
    port: u16,
}

struct BackendState {
    binary_path: PathBuf,
    data_dir: PathBuf,
    startup_session_id: String,
    expected_port: Mutex<u16>,
    port: Mutex<u16>,
    child: Mutex<Option<Child>>,
    backend_recovery_in_progress: AtomicBool,
    backend_lifecycle_epoch: AtomicU64,
    shutdown_started: AtomicBool,
    force_exit: AtomicBool,
    startup_ready: AtomicBool,
    startup_error: Mutex<Option<String>>,
}

fn test_backend_state(
    data_dir: PathBuf,
    port: u16,
    ready: bool,
    error: Option<String>,
) -> BackendState {
    BackendState {
        binary_path: PathBuf::new(),
        data_dir,
        startup_session_id: "headless-test".into(),
        expected_port: Mutex::new(port),
        port: Mutex::new(port),
        child: Mutex::new(None),
        backend_recovery_in_progress: AtomicBool::new(false),
        backend_lifecycle_epoch: AtomicU64::new(0),
        shutdown_started: AtomicBool::new(false),
        force_exit: AtomicBool::new(false),
        startup_ready: AtomicBool::new(ready),
        startup_error: Mutex::new(error),
    }
}

#[path = "../../src/backend_runtime/watchdog_policy.rs"]
mod watchdog_policy;
use watchdog_policy::*;
#[path = "../../src/backend_runtime/port_retry.rs"]
mod port_retry;
#[path = "../../src/backend_runtime/recovery.rs"]
mod recovery;
use recovery::*;

fn append_desktop_bootstrap_log(_data_dir: &Path, _message: impl AsRef<str>) {}
fn record_startup_error(state: &BackendState, error: String) {
    *state.startup_error.lock().unwrap() = Some(error);
}
fn publish_startup_ready(state: &BackendState) {
    state.startup_ready.store(true, Ordering::SeqCst);
    *state.startup_error.lock().unwrap() = None;
}
fn anyhow(message: String) -> String {
    message
}
fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}
fn read_desktop_runtime_marker(data_dir: &Path) -> Option<DesktopRuntimeMarker> {
    serde_json::from_slice(&fs::read(data_dir.join("runtime.json")).ok()?).ok()
}
fn kill_child_and_wait(
    child: &mut Child,
    _timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait()
}
fn terminate_child(mut child: Child) -> tauri::Result<()> {
    if child.try_wait().map_err(|e| e.to_string())?.is_none() {
        child.kill().map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}
fn terminate_managed_backend(state: &BackendState, _context: &str) -> tauri::Result<()> {
    if let Some(child) = state.child.lock().unwrap().take() {
        terminate_child(child)?;
    }
    Ok(())
}
// Readiness and launch are intentionally injectable. Calling the real automatic
// recovery wrapper with no binary proves launch failure without starting Bifrost.
fn is_port_available(_port: u16) -> bool {
    true
}
fn find_existing_backend_port(_data_dir: &Path, _port: u16) -> Option<u16> {
    None
}
fn start_backend(
    binary: &Path,
    _dir: &Path,
    _session: &str,
    _port: u16,
    _recovery_generation: Option<&str>,
) -> tauri::Result<Child> {
    Command::new(binary)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendWaitFailureKind {
    ChildExited,
    ChildInspection,
    TimedOut,
}
struct BackendWaitFailure {
    kind: BackendWaitFailureKind,
}
impl std::fmt::Display for BackendWaitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mock readiness failure")
    }
}
fn wait_for_backend(
    _child: &mut Child,
    _dir: &Path,
    _port: u16,
    _timeout: Duration,
) -> Result<(), BackendWaitFailure> {
    Err(BackendWaitFailure {
        kind: BackendWaitFailureKind::TimedOut,
    })
}

#[path = "../../src/tests/recovery_races.rs"]
mod recovery_races;
