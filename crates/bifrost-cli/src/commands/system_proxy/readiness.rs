use super::*;
use std::time::Duration;

const READY_SAMPLES: u32 = 3;
const READY_STABILITY_WINDOW: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct StableReadiness {
    identity: Option<(u32, Option<u64>, u16, Option<String>)>,
    first_success: Option<Duration>,
    successes: u32,
}

impl StableReadiness {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn observe(
        &mut self,
        runtime: Option<&RuntimeInfo>,
        ready: bool,
        elapsed: Duration,
    ) -> bool {
        let Some(runtime) = runtime.filter(|_| ready) else {
            self.reset();
            return false;
        };
        let identity = (
            runtime.pid,
            runtime.started_at_ms,
            runtime.port,
            runtime.host.clone(),
        );
        if self.identity.as_ref() != Some(&identity) {
            self.reset();
            self.identity = Some(identity);
        }
        let first = *self.first_success.get_or_insert(elapsed);
        self.successes = self.successes.saturating_add(1);
        self.successes >= READY_SAMPLES && elapsed.saturating_sub(first) >= READY_STABILITY_WINDOW
    }
}

pub(super) fn same_runtime_target(left: &RuntimeInfo, right: &RuntimeInfo) -> bool {
    left.port == right.port
        && runtime_system_proxy_host(left.host.as_deref())
            == runtime_system_proxy_host(right.host.as_deref())
}

pub(super) fn same_runtime_identity(left: &RuntimeInfo, right: &RuntimeInfo) -> bool {
    left.pid == right.pid
        && left.started_at_ms == right.started_at_ms
        && same_runtime_target(left, right)
}

pub(super) fn runtime_data_plane_is_ready(runtime: &RuntimeInfo) -> bool {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::ToSocketAddrs;

    let host = runtime_system_proxy_host(runtime.host.as_deref());
    let Ok(addrs) = (host, runtime.port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        let Ok(mut stream) =
            std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500))
        else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(750)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        if stream.write_all(b"GET http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary HTTP/1.1\r\nHost: bifrost-runtime-canary.invalid\r\nConnection: close\r\n\r\n").is_err() { continue; }
        let mut line = String::new();
        if BufReader::new(stream.take(1024))
            .read_line(&mut line)
            .is_ok()
        {
            let mut parts = line.split_ascii_whitespace();
            if matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0"))
                && parts.next() == Some("204")
                && line.ends_with('\n')
            {
                return true;
            }
        }
    }
    false
}
