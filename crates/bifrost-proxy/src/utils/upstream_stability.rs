use rand::Rng;
use std::future::Future;
use std::io;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::{Notify, Semaphore};

const DEFAULT_CONNECT_CONCURRENCY: usize = 64;
const BACKOFF_BASE: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(2);

static UPSTREAM_STABILITY: LazyLock<Arc<UpstreamStability>> = LazyLock::new(|| {
    Arc::new(UpstreamStability::new(positive_env_or_default(
        "BIFROST_UPSTREAM_CONNECT_CONCURRENCY",
        DEFAULT_CONNECT_CONCURRENCY,
    )))
});

#[derive(Debug, Default)]
struct RecoveryState {
    resource_failures: u32,
    retry_at: Option<Instant>,
    probe_in_flight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryAction {
    Ready { probe: bool },
    Sleep(Duration),
    WaitForProbe,
}

impl RecoveryState {
    fn action(&mut self, now: Instant) -> RecoveryAction {
        if self.resource_failures == 0 {
            return RecoveryAction::Ready { probe: false };
        }

        if let Some(retry_at) = self.retry_at {
            if retry_at > now {
                return RecoveryAction::Sleep(retry_at.duration_since(now));
            }
        }

        if self.probe_in_flight {
            RecoveryAction::WaitForProbe
        } else {
            self.probe_in_flight = true;
            RecoveryAction::Ready { probe: true }
        }
    }

    fn record_resource_failure(&mut self, now: Instant, jitter: Duration) {
        self.resource_failures = self.resource_failures.saturating_add(1);
        self.retry_at = Some(now + backoff_delay(self.resource_failures) + jitter);
        self.probe_in_flight = false;
    }

    fn record_probe_completion(&mut self) {
        self.resource_failures = 0;
        self.retry_at = None;
        self.probe_in_flight = false;
    }

    fn cancel_probe(&mut self) {
        if self.probe_in_flight {
            self.probe_in_flight = false;
            self.retry_at = Some(Instant::now());
        }
    }
}

pub(crate) struct UpstreamStability {
    connect_limit: Arc<Semaphore>,
    recovery: Mutex<RecoveryState>,
    recovery_notify: Arc<Notify>,
}

impl UpstreamStability {
    fn new(connect_concurrency: usize) -> Self {
        Self {
            connect_limit: Arc::new(Semaphore::new(connect_concurrency.max(1))),
            recovery: Mutex::new(RecoveryState::default()),
            recovery_notify: Arc::new(Notify::new()),
        }
    }

    fn recovery_state(&self) -> MutexGuard<'_, RecoveryState> {
        self.recovery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn begin_attempt(self: &Arc<Self>) -> NetworkAttempt {
        loop {
            let notified = self.recovery_notify.notified();
            let action = self.recovery_state().action(Instant::now());
            match action {
                RecoveryAction::Ready { probe } => {
                    return NetworkAttempt {
                        controller: Arc::clone(self),
                        probe,
                        completed: false,
                    };
                }
                RecoveryAction::Sleep(delay) => tokio::time::sleep(delay).await,
                RecoveryAction::WaitForProbe => notified.await,
            }
        }
    }

    async fn connect<A>(self: &Arc<Self>, address: A) -> io::Result<TcpStream>
    where
        A: ToSocketAddrs,
    {
        self.run_connect(|| TcpStream::connect(address)).await
    }

    async fn run_connect<F, Fut, T>(self: &Arc<Self>, connect: F) -> io::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = io::Result<T>>,
    {
        let _permit = self
            .connect_limit
            .acquire()
            .await
            .expect("upstream connect limiter should not be closed");
        let attempt = self.begin_attempt().await;
        let result = connect().await;
        match &result {
            Ok(_) => attempt.record_success(),
            Err(error) => attempt.record_error(Some(error.kind()), &error.to_string()),
        }
        result
    }
}

pub(crate) struct NetworkAttempt {
    controller: Arc<UpstreamStability>,
    probe: bool,
    completed: bool,
}

impl NetworkAttempt {
    pub(crate) fn record_success(mut self) {
        if self.probe {
            self.controller.recovery_state().record_probe_completion();
            self.controller.recovery_notify.notify_waiters();
        }
        self.completed = true;
    }

    pub(crate) fn record_error(mut self, kind: Option<io::ErrorKind>, error_message: &str) {
        if is_resource_pressure_error(kind, error_message) {
            let delay = backoff_delay(
                self.controller
                    .recovery_state()
                    .resource_failures
                    .saturating_add(1),
            );
            let jitter_ceiling = (delay.as_millis() as u64 / 4).max(1);
            let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..=jitter_ceiling));
            self.controller
                .recovery_state()
                .record_resource_failure(Instant::now(), jitter);
            self.controller.recovery_notify.notify_waiters();
        } else if self.probe {
            self.controller.recovery_state().record_probe_completion();
            self.controller.recovery_notify.notify_waiters();
        }
        self.completed = true;
    }
}

impl Drop for NetworkAttempt {
    fn drop(&mut self) {
        if self.probe && !self.completed {
            self.controller.recovery_state().cancel_probe();
            self.controller.recovery_notify.notify_waiters();
        }
    }
}

pub(crate) async fn begin_network_attempt() -> NetworkAttempt {
    UPSTREAM_STABILITY.begin_attempt().await
}

pub(crate) async fn connect_tcp<A>(address: A) -> io::Result<TcpStream>
where
    A: ToSocketAddrs,
{
    UPSTREAM_STABILITY.connect(address).await
}

fn positive_env_or_default(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn backoff_delay(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(5);
    BACKOFF_BASE
        .saturating_mul(1u32 << exponent)
        .min(BACKOFF_MAX)
}

fn is_resource_pressure_error(kind: Option<io::ErrorKind>, error_message: &str) -> bool {
    let error_message = error_message.to_ascii_lowercase();
    matches!(
        kind,
        Some(io::ErrorKind::AddrNotAvailable | io::ErrorKind::OutOfMemory)
    ) || error_message.contains("cannot assign requested address")
        || error_message.contains("address not available")
        || error_message.contains("too many open files")
        || error_message.contains("resource temporarily unavailable")
        || error_message.contains("no buffer space available")
        || platform_resource_errno_text(&error_message)
}

#[cfg(target_os = "macos")]
fn platform_resource_errno_text(error_message: &str) -> bool {
    error_message.contains("os error 24")
        || error_message.contains("os error 49")
        || error_message.contains("os error 55")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_resource_errno_text(error_message: &str) -> bool {
    error_message.contains("os error 24")
}

#[cfg(not(unix))]
fn platform_resource_errno_text(_error_message: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn recovery_state_uses_exponential_backoff_and_single_probe() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        assert_eq!(state.action(now), RecoveryAction::Ready { probe: false });

        state.record_resource_failure(now, Duration::ZERO);
        assert_eq!(
            state.action(now),
            RecoveryAction::Sleep(Duration::from_millis(100))
        );
        assert_eq!(
            state.action(now + Duration::from_millis(100)),
            RecoveryAction::Ready { probe: true }
        );
        assert_eq!(
            state.action(now + Duration::from_millis(100)),
            RecoveryAction::WaitForProbe
        );

        state.record_resource_failure(now, Duration::ZERO);
        assert_eq!(
            state.action(now),
            RecoveryAction::Sleep(Duration::from_millis(200))
        );
    }

    #[test]
    fn successful_probe_resets_recovery_state() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.record_resource_failure(now, Duration::ZERO);
        assert_eq!(
            state.action(now + Duration::from_millis(100)),
            RecoveryAction::Ready { probe: true }
        );
        state.record_probe_completion();
        assert_eq!(
            state.action(now + Duration::from_millis(100)),
            RecoveryAction::Ready { probe: false }
        );
    }

    #[test]
    fn resource_pressure_classification_is_narrow() {
        assert!(is_resource_pressure_error(
            Some(io::ErrorKind::AddrNotAvailable),
            "ordinary"
        ));
        assert!(is_resource_pressure_error(
            Some(io::ErrorKind::Other),
            "No buffer space available (os error 55)"
        ));
        assert!(!is_resource_pressure_error(
            Some(io::ErrorKind::ConnectionRefused),
            "connection refused"
        ));
        assert!(!is_resource_pressure_error(
            Some(io::ErrorKind::TimedOut),
            "connection timed out"
        ));
    }

    #[tokio::test]
    async fn connect_limit_serializes_excess_connection_attempts() {
        let controller = Arc::new(UpstreamStability::new(1));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let run = |controller: Arc<UpstreamStability>,
                   active: Arc<AtomicUsize>,
                   peak: Arc<AtomicUsize>| async move {
            controller
                .run_connect(|| async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, io::Error>(())
                })
                .await
                .unwrap();
        };

        tokio::join!(
            run(
                Arc::clone(&controller),
                Arc::clone(&active),
                Arc::clone(&peak)
            ),
            run(controller, active, Arc::clone(&peak))
        );
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_probe_allows_a_replacement_probe() {
        let controller = Arc::new(UpstreamStability::new(1));
        {
            let mut state = controller.recovery_state();
            state.record_resource_failure(Instant::now() - Duration::from_secs(1), Duration::ZERO);
        }

        let probe = controller.begin_attempt().await;
        assert!(probe.probe);
        drop(probe);

        let replacement = controller.begin_attempt().await;
        assert!(replacement.probe);
        replacement.record_success();
    }
}
