use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use bifrost_core::{BifrostError, Result};

use super::detached_daemon_readiness_host;
use crate::process::RuntimeInfo;

pub(super) fn wait_for_detached_daemon_ready(
    child: &mut Child,
    host: &str,
    port: u16,
    runtime_file: &Path,
    previous_runtime: Option<&[u8]>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let pid = child.id();
    let expected = SpawnedDaemon {
        pid,
        host,
        port,
        started_at_ms: bifrost_core::get_process_start_time_ms(pid),
        runtime_file,
        previous_runtime,
    };
    wait_for_readiness(
        pid,
        timeout,
        deadline,
        || {
            child
                .try_wait()
                .map(|status| status.map(|status| status.to_string()))
        },
        |remaining| expected.is_ready(remaining),
    )
}

struct SpawnedDaemon<'a> {
    pid: u32,
    host: &'a str,
    port: u16,
    started_at_ms: Option<u64>,
    runtime_file: &'a Path,
    previous_runtime: Option<&'a [u8]>,
}

impl SpawnedDaemon<'_> {
    fn is_ready(&self, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        let Ok(content) = std::fs::read(self.runtime_file) else {
            return false;
        };
        if self.previous_runtime == Some(content.as_slice()) {
            return false;
        }
        let Ok(runtime) = serde_json::from_slice::<RuntimeInfo>(&content) else {
            return false;
        };
        // Startup publishes this marker only after managed listener readiness.
        // Requiring fresh bytes also rejects a stale same-PID marker if process
        // start-time lookup is unavailable. Do not require Admin authentication
        // or a second health service for a custom interface bind.
        if runtime.pid != self.pid
            || runtime.port != self.port
            || !runtime.restartable_daemon()
            || runtime.host.as_deref().map(normalize_host) != Some(normalize_host(self.host))
            || matches!(
                bifrost_core::start_times_match(runtime.started_at_ms, self.started_at_ms),
                bifrost_core::StartTimeMatch::Mismatch { .. }
            )
        {
            return false;
        }
        let Some(address) = readiness_address(self.host, self.port) else {
            return false;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        !remaining.is_zero()
            && TcpStream::connect_timeout(&address, remaining.min(Duration::from_millis(400)))
                .is_ok()
    }
}

fn readiness_address(host: &str, port: u16) -> Option<SocketAddr> {
    let host = detached_daemon_readiness_host(host);
    // Match startup's literal socket-address parsing, including IPv6 scopes;
    // never add unbounded DNS resolution to the startup deadline.
    format!("{host}:{port}").parse().ok()
}

fn normalize_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

fn wait_for_readiness(
    pid: u32,
    timeout: Duration,
    deadline: Instant,
    mut child_status: impl FnMut() -> std::io::Result<Option<String>>,
    mut is_ready: impl FnMut(Duration) -> bool,
) -> Result<()> {
    while Instant::now() < deadline {
        if let Some(status) = child_status()? {
            return Err(BifrostError::Network(format!(
                "Daemon exited before the proxy listener became ready (PID: {pid}, status: {status})"
            )));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if is_ready(remaining) && Instant::now() < deadline {
            // The child can exit while the readiness probe is in flight.
            if let Some(status) = child_status()? {
                return Err(BifrostError::Network(format!(
                    "Daemon exited before the proxy listener became ready (PID: {pid}, status: {status})"
                )));
            }
            if Instant::now() >= deadline {
                break;
            }
            println!("Daemon started with PID: {pid}");
            return Ok(());
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
    Err(BifrostError::Network(format!(
        "Daemon did not become ready within {}s (PID: {pid})",
        timeout.as_secs(),
    )))
}

#[cfg(test)]
mod tests;
