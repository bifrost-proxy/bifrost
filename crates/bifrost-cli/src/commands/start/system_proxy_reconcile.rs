//! One OS-proxy writer per core. Wake events invalidate readiness rather than
//! racing a second writer; every observation is fenced by the current target.
use super::{should_stop_system_proxy_reconcile_for_shutdown, system_proxy_reconcile_interval};
use bifrost_core::system_proxy::{ManagedSystemProxyPhase, ManagedSystemProxyVerification};
use bifrost_core::{GuardedSystemProxyTransition, ManagedSystemProxyOwnership, SystemProxyManager};
use bifrost_storage::SystemProxyRecoveryMode;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod driver;

const PROBE_INTERVAL: Duration = Duration::from_secs(1);
const RECOVERY_HEALTHY_DURATION: Duration = Duration::from_secs(2);
const RECOVERY_HEALTHY_SAMPLES: u32 = 3;

pub(super) fn prepare_proxy_startup_markers(
    data_dir: &Path,
    automatic: bool,
) -> bifrost_core::Result<()> {
    if automatic {
        return ensure_automatic_recovery_not_stopped(data_dir, true);
    }
    // An explicit new Start supersedes a completed Stop. Automatic replacement
    // must never clear that stop request on the user's behalf.
    if let Some(
        mode @ (bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
        | bifrost_core::SystemProxyShutdownMode::BackgroundCleanup),
    ) = bifrost_core::read_system_proxy_shutdown_mode_checked(data_dir)?
    {
        bifrost_core::consume_system_proxy_shutdown_mode_if(data_dir, mode);
        if matches!(
            bifrost_core::read_system_proxy_shutdown_mode_checked(data_dir)?,
            Some(
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
                    | bifrost_core::SystemProxyShutdownMode::BackgroundCleanup
            )
        ) {
            return Err(bifrost_core::BifrostError::Config(
                "another stop request superseded startup".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn automatic_proxy_recovery_launch() -> bool {
    std::env::var_os("BIFROST_SYSTEM_PROXY_INTENT_REVISION_INTERNAL").is_some()
        || std::env::var_os("BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL").is_some()
}

pub(super) fn ensure_automatic_recovery_not_stopped(
    data_dir: &Path,
    automatic: bool,
) -> bifrost_core::Result<()> {
    if automatic
        && matches!(
            bifrost_core::read_system_proxy_shutdown_mode(data_dir),
            Some(
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
                    | bifrost_core::SystemProxyShutdownMode::BackgroundCleanup
            )
        )
    {
        return Err(bifrost_core::BifrostError::Config(
            "automatic recovery cancelled by an explicit stop".into(),
        ));
    }
    Ok(())
}

pub(super) fn resolve_startup_system_proxy_intent(
    enable_flag: bool,
    disable_flag: bool,
    bypass: Option<String>,
    configured: &bifrost_storage::NewSystemProxyConfig,
    handoff_revision: Option<&str>,
) -> (bool, String) {
    // Automatic restarts carry the revision that produced their argv. An
    // intervening user toggle wins; only direct CLI flags establish a new
    // session override against the configuration observed at actual startup.
    let stale_handoff = handoff_revision
        .is_some_and(|revision| revision.parse::<u64>().ok() != Some(configured.intent_revision));
    if stale_handoff {
        return (configured.enabled, configured.bypass.clone());
    }
    let enabled = if enable_flag {
        true
    } else if disable_flag {
        false
    } else {
        configured.enabled
    };
    (enabled, bypass.unwrap_or_else(|| configured.bypass.clone()))
}

pub(super) struct SystemProxyReconcileConfig {
    pub(super) bifrost_dir: PathBuf,
    pub(super) system_proxy_manager: Arc<tokio::sync::RwLock<SystemProxyManager>>,
    pub(super) desired_enabled: Arc<AtomicBool>,
    pub(super) proxy_host: String,
    pub(super) proxy_port: Arc<AtomicU16>,
    pub(super) wake_requested: Arc<AtomicBool>,
    pub(super) startup_intent_revision: u64,
    pub(super) initial_policy: (SystemProxyRecoveryMode, u64),
    pub(super) expected_generation: Arc<parking_lot::RwLock<Option<String>>>,
    pub(super) system_proxy_bypass: String,
    pub(super) enabled_flag: Arc<AtomicBool>,
    pub(super) stop_flag: Arc<AtomicBool>,
    pub(super) daemon_mode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SystemProxyOwnership {
    Disabled,
    ThisBifrost,
    Other,
    Unknown,
}

#[derive(Default)]
struct ReadinessWindow {
    port: Option<u16>,
    healthy_since: Option<Instant>,
    unhealthy_since: Option<Instant>,
    healthy_samples: u32,
}

impl ReadinessWindow {
    fn observe(&mut self, port: u16, ready: bool, now: Instant) {
        if self.port != Some(port) {
            *self = Self {
                port: Some(port),
                ..Self::default()
            };
        }
        if ready {
            self.unhealthy_since = None;
            self.healthy_since.get_or_insert(now);
            self.healthy_samples = self.healthy_samples.saturating_add(1);
        } else {
            self.healthy_since = None;
            self.healthy_samples = 0;
            self.unhealthy_since.get_or_insert(now);
        }
    }

    fn recovered(&self, now: Instant) -> bool {
        self.healthy_samples >= RECOVERY_HEALTHY_SAMPLES
            && self
                .healthy_since
                .is_some_and(|since| now.duration_since(since) >= RECOVERY_HEALTHY_DURATION)
    }

    fn fail_open_due(&self, now: Instant, grace: Duration) -> bool {
        self.unhealthy_since
            .is_some_and(|since| now.duration_since(since) >= grace)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconcileAction {
    Wait,
    Suspend,
    Release,
    Resume,
    Adopt,
    Acquire,
    PreserveExternal,
}

#[allow(clippy::too_many_arguments)]
fn reconcile_action(
    lease: Option<&ManagedSystemProxyOwnership>,
    target: (&str, u16),
    observed: SystemProxyOwnership,
    recovered: bool,
    fail_open_due: bool,
    allow_initial_acquire: bool,
    desired: Option<bool>,
) -> ReconcileAction {
    if desired == Some(false) {
        return if lease.is_some_and(|lease| lease.target.target_matches(target.0, target.1)) {
            ReconcileAction::Release
        } else {
            ReconcileAction::Wait
        };
    }
    if let Some(lease) = lease {
        if !lease.target.target_matches(target.0, target.1) {
            return ReconcileAction::PreserveExternal;
        }
        if !recovered {
            return if lease.phase
                != Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended)
                && fail_open_due
            {
                ReconcileAction::Suspend
            } else {
                ReconcileAction::Wait
            };
        }
        // A restored corporate proxy is expected during suspension. Let the
        // generation/field comparison decide, before generic external-owner logic.
        if desired.is_none() {
            return ReconcileAction::Wait;
        }
        if lease.is_suspended() {
            return ReconcileAction::Resume;
        }
        if lease.applied
            || lease.phase
                == Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::PendingApply)
        {
            return ReconcileAction::Adopt;
        }
    }
    if desired.is_none() || !recovered || observed == SystemProxyOwnership::Unknown {
        return ReconcileAction::Wait;
    }
    if observed == SystemProxyOwnership::Other {
        return ReconcileAction::PreserveExternal;
    }
    if allow_initial_acquire {
        ReconcileAction::Acquire
    } else {
        ReconcileAction::PreserveExternal
    }
}

pub(super) fn wait_for_reconcile(stop: &AtomicBool, data_dir: &Path, interval: Duration) -> bool {
    let until = Instant::now() + interval;
    while Instant::now() < until {
        if stop.load(Ordering::Acquire) || should_stop_system_proxy_reconcile_for_shutdown(data_dir)
        {
            stop.store(true, Ordering::Release);
            return true;
        }
        std::thread::sleep(
            Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())),
        );
    }
    false
}

fn inspect_system_proxy_ownership(host: &str, port: u16) -> SystemProxyOwnership {
    match SystemProxyManager::get_current() {
        Ok(current) if !current.enable => SystemProxyOwnership::Disabled,
        Ok(current) if current.target_matches(host, port) => SystemProxyOwnership::ThisBifrost,
        Ok(_) => SystemProxyOwnership::Other,
        Err(error) => {
            tracing::warn!(%error, host, port, "system proxy observation failed; preserving unknown OS state");
            SystemProxyOwnership::Unknown
        }
    }
}

pub(super) fn system_proxy_target_is_ready(proxy_host: &str, proxy_port: u16) -> bool {
    let host = match proxy_host {
        "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        host => host,
    };
    let Ok(address) = format!("{host}:{proxy_port}").parse::<SocketAddr>() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(700)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    if stream.write_all(b"GET http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary HTTP/1.1\r\nHost: bifrost-runtime-canary.invalid\r\nConnection: close\r\n\r\n").is_err() { return false; }
    let mut response = [0_u8; 128];
    let mut received = 0;
    let deadline = Instant::now() + Duration::from_millis(700);
    while received < response.len() && Instant::now() < deadline {
        let _ = stream.set_read_timeout(Some(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ));
        match stream.read(&mut response[received..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                received += read;
                if response[..received]
                    .windows(2)
                    .any(|window| window == b"\r\n")
                {
                    break;
                }
            }
        }
    }
    response[..received].starts_with(b"HTTP/1.1 204 ")
        && response[..received]
            .windows(2)
            .any(|window| window == b"\r\n")
}

fn transition_applied(transition: GuardedSystemProxyTransition) -> bool {
    matches!(
        transition,
        GuardedSystemProxyTransition::Applied | GuardedSystemProxyTransition::AlreadyInState
    )
}

fn probe_target_is_current(observed: u16, current: &AtomicU16) -> bool {
    observed == current.load(Ordering::Acquire)
}

fn generation(lease: Option<&ManagedSystemProxyOwnership>) -> Option<&str> {
    lease.map(|lease| lease.generation.as_str())
}

trait ProxyTransitions {
    fn verify(&mut self, generation: &str) -> bifrost_core::Result<ManagedSystemProxyVerification>;
    fn suspend(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn reconcile(&mut self, generation: &str)
        -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn release(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition>;
    fn acquire(
        &mut self,
        host: &str,
        port: u16,
        bypass: &str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition>;
}

struct LockedProxyBackend<'a> {
    manager: &'a mut SystemProxyManager,
    config: &'a SystemProxyReconcileConfig,
}
impl ProxyTransitions for LockedProxyBackend<'_> {
    fn verify(&mut self, generation: &str) -> bifrost_core::Result<ManagedSystemProxyVerification> {
        self.manager.verify_managed_if_generation(generation)
    }
    fn suspend(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.manager.suspend_managed_if_generation(generation)
    }
    fn reconcile(
        &mut self,
        generation: &str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        let result = self.manager.reconcile_managed_if_generation(generation);
        #[cfg(target_os = "macos")]
        let result = match result {
            Err(error) if error.to_string().contains("RequiresAdmin") => self
                .manager
                .reconcile_managed_if_generation_with_gui_auth(generation),
            result => result,
        };
        result
    }
    fn release(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        let config = self.config;
        self.manager
            .restore_managed_if_generation_guarded(generation, || {
                let latest =
                    bifrost_storage::read_persisted_system_proxy_config(&config.bifrost_dir)?;
                Ok(!effective_desired(&latest, config))
            })
    }
    fn acquire(
        &mut self,
        host: &str,
        port: u16,
        bypass: &str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.manager.enable_if_unmanaged(host, port, Some(bypass))
    }
}

fn perform_transition(
    backend: &mut impl ProxyTransitions,
    action: ReconcileAction,
    generation: Option<&str>,
    host: &str,
    port: u16,
    bypass: &str,
) -> bifrost_core::Result<GuardedSystemProxyTransition> {
    match action {
        ReconcileAction::Acquire => backend.acquire(host, port, bypass),
        ReconcileAction::PreserveExternal => Ok(GuardedSystemProxyTransition::OwnershipChanged),
        ReconcileAction::Wait => Ok(GuardedSystemProxyTransition::NotManaged),
        action => {
            let generation = generation.ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "missing ownership generation for proxy transition".into(),
                )
            })?;
            match action {
                ReconcileAction::Suspend => backend.suspend(generation),
                ReconcileAction::Release => backend.release(generation),
                _ => backend.reconcile(generation),
            }
        }
    }
}

fn effective_desired(
    configured: &bifrost_storage::NewSystemProxyConfig,
    config: &SystemProxyReconcileConfig,
) -> bool {
    if configured.intent_revision == config.startup_intent_revision {
        config.desired_enabled.load(Ordering::Acquire)
    } else {
        configured.enabled
    }
}

impl driver::ReconcileIo for LockedProxyBackend<'_> {
    fn is_attached(&self, generation: &str) -> bool {
        self.manager.is_managed_generation_attached(generation)
    }
    fn persisted_intent(&mut self) -> bifrost_core::Result<bifrost_storage::NewSystemProxyConfig> {
        bifrost_storage::read_persisted_system_proxy_config(&self.config.bifrost_dir)
    }
    fn ownership(&mut self) -> bifrost_core::Result<Option<ManagedSystemProxyOwnership>> {
        self.manager.ensure_managed_ownership()
    }
    fn observed(&mut self, host: &str, port: u16) -> SystemProxyOwnership {
        inspect_system_proxy_ownership(host, port)
    }
    fn cancellation_generation(&mut self) -> Option<String> {
        self.manager
            .read_managed_ownership()
            .ok()
            .flatten()
            .map(|lease| lease.generation)
    }
}

pub(super) fn spawn_system_proxy_reconcile_task(config: SystemProxyReconcileConfig) {
    if !SystemProxyManager::is_supported() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("bifrost-system-proxy-reconcile".into())
        .spawn(move || {
            // run_start owns pre-listener cleanup. Never repeat it after the API
            // can acquire a new lease.
            let mut state = driver::ReconcileState::new(&config, Instant::now());
            loop {
                if config.stop_flag.load(Ordering::Acquire)
                    || should_stop_system_proxy_reconcile_for_shutdown(&config.bifrost_dir)
                {
                    return;
                }
                if config.wake_requested.swap(false, Ordering::AcqRel) {
                    state.wake(Instant::now());
                }
                let port = config.proxy_port.load(Ordering::Acquire);
                let before = config
                    .system_proxy_manager
                    .blocking_read()
                    .read_managed_ownership();
                let ready = system_proxy_target_is_ready(&config.proxy_host, port);
                let now = Instant::now();
                let preliminary =
                    bifrost_storage::read_persisted_system_proxy_config(&config.bifrost_dir).ok();
                if state.observe_and_should_inspect(&config, port, ready, now, preliminary.as_ref())
                {
                    let mut manager = config.system_proxy_manager.blocking_write();
                    if !config.stop_flag.load(Ordering::Acquire)
                        && probe_target_is_current(port, &config.proxy_port)
                        && !should_stop_system_proxy_reconcile_for_shutdown(&config.bifrost_dir)
                    {
                        let mut backend = LockedProxyBackend {
                            manager: &mut manager,
                            config: &config,
                        };
                        state.reconcile_locked(&config, &mut backend, before, port, ready, now);
                    }
                }
                if wait_for_reconcile(&config.stop_flag, &config.bifrost_dir, PROBE_INTERVAL) {
                    return;
                }
            }
        });
}

#[cfg(target_os = "macos")]
pub(super) fn spawn_system_proxy_wake_reconcile_task(config: SystemProxyReconcileConfig) {
    let _ = std::thread::Builder::new()
        .name("bifrost-system-proxy-wake".into())
        .spawn(move || {
            let mut last = std::time::SystemTime::now();
            while !wait_for_reconcile(
                &config.stop_flag,
                &config.bifrost_dir,
                Duration::from_secs(2),
            ) {
                let now = std::time::SystemTime::now();
                if now.duration_since(last).unwrap_or_default() >= Duration::from_secs(10) {
                    config.wake_requested.store(true, Ordering::Release);
                }
                last = now;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn allocate_loopback_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    #[test]
    fn system_proxy_readiness_requires_a_successful_data_plane_canary() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let server = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let read = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).contains("/__bifrost_runtime_canary"));
            stream.write_all(b"HTTP/1.1 ").unwrap();
            std::thread::sleep(Duration::from_millis(10));
            stream
                .write_all(b"204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });

        ready_rx.recv().unwrap();
        assert!(system_proxy_target_is_ready("0.0.0.0", port));
        server.join().unwrap();

        let closed_port = allocate_loopback_port();
        assert!(!system_proxy_target_is_ready("127.0.0.1", closed_port));
        assert!(!system_proxy_target_is_ready("invalid host", 18745));
    }
    #[test]
    fn system_proxy_readiness_rejects_empty_and_non_successful_canary_responses() {
        let empty_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let empty_port = empty_listener.local_addr().unwrap().port();
        let (empty_ready_tx, empty_ready_rx) = std::sync::mpsc::sync_channel(0);
        let empty_server = std::thread::spawn(move || {
            empty_ready_tx.send(()).unwrap();
            let (stream, _) = empty_listener.accept().unwrap();
            drop(stream);
        });
        empty_ready_rx.recv().unwrap();
        assert!(!system_proxy_target_is_ready("127.0.0.1", empty_port));
        empty_server.join().unwrap();

        let rejected_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let rejected_port = rejected_listener.local_addr().unwrap().port();
        let (rejected_ready_tx, rejected_ready_rx) = std::sync::mpsc::sync_channel(0);
        let rejected_server = std::thread::spawn(move || {
            rejected_ready_tx.send(()).unwrap();
            let (mut stream, _) = rejected_listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        rejected_ready_rx.recv().unwrap();
        assert!(!system_proxy_target_is_ready("127.0.0.1", rejected_port));
        rejected_server.join().unwrap();
    }
    #[test]
    fn reconcile_wait_finishes_immediately_or_stops_on_request() {
        let temp_dir = tempfile::tempdir().unwrap();
        let stop_flag = AtomicBool::new(false);
        assert!(!wait_for_reconcile(
            &stop_flag,
            temp_dir.path(),
            Duration::ZERO,
        ));

        stop_flag.store(true, Ordering::Release);
        assert!(wait_for_reconcile(
            &stop_flag,
            temp_dir.path(),
            Duration::from_secs(1),
        ));
        assert!(stop_flag.load(Ordering::Acquire));

        let stop_flag = AtomicBool::new(false);
        assert!(!wait_for_reconcile(
            &stop_flag,
            temp_dir.path(),
            Duration::from_millis(1),
        ));

        let stop_flag = AtomicBool::new(false);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
        )
        .unwrap();
        assert!(wait_for_reconcile(
            &stop_flag,
            temp_dir.path(),
            Duration::from_secs(1),
        ));
        assert!(stop_flag.load(Ordering::Acquire));
    }
    fn lease(applied: bool, suspended: bool, port: u16) -> ManagedSystemProxyOwnership {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3, "generation": "lease-1",
            "original": {"enable": true, "host": "corporate-proxy", "port": 8080, "bypass": "*.internal"},
            "target": {"enable": true, "host": "127.0.0.1", "port": port, "bypass": "localhost"},
            "applied": applied,
            "phase": if suspended { "suspended" } else if applied { "applied" } else { "pending_apply" }
        })).unwrap()
    }

    #[test]
    fn original_external_proxy_does_not_prevent_guarded_suspended_resume() {
        let suspended = lease(false, true, 18745);
        assert_eq!(
            reconcile_action(
                Some(&suspended),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Other,
                true,
                false,
                false,
                Some(true)
            ),
            ReconcileAction::Resume
        );
        // An unowned external proxy never becomes an unconditional acquire.
        assert_eq!(
            reconcile_action(
                None,
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Other,
                true,
                false,
                true,
                Some(true)
            ),
            ReconcileAction::PreserveExternal
        );
    }

    #[test]
    fn retired_port_probe_cannot_suspend_or_resume_new_target() {
        let current = AtomicU16::new(18746);
        assert!(!probe_target_is_current(18745, &current));
        assert!(probe_target_is_current(18746, &current));
        let new_lease = lease(true, false, 18746);
        assert_eq!(
            reconcile_action(
                Some(&new_lease),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::ThisBifrost,
                false,
                true,
                false,
                Some(true)
            ),
            ReconcileAction::PreserveExternal
        );
    }

    #[test]
    fn desired_disable_retries_release_and_never_resumes_suspended_lease() {
        for suspended in [false, true] {
            let owned = lease(!suspended, suspended, 18745);
            assert_eq!(
                reconcile_action(
                    Some(&owned),
                    ("127.0.0.1", 18745),
                    SystemProxyOwnership::Other,
                    true,
                    true,
                    true,
                    Some(false)
                ),
                ReconcileAction::Release
            );
        }
        assert_eq!(
            reconcile_action(
                None,
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Other,
                true,
                true,
                true,
                Some(false)
            ),
            ReconcileAction::Wait
        );
    }

    #[test]
    fn unknown_os_state_and_manual_release_do_not_trigger_fresh_acquisition() {
        assert_eq!(
            reconcile_action(
                None,
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Unknown,
                true,
                false,
                true,
                Some(true)
            ),
            ReconcileAction::Wait
        );
        assert_eq!(
            reconcile_action(
                None,
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Disabled,
                true,
                false,
                false,
                Some(true)
            ),
            ReconcileAction::PreserveExternal
        );
        assert_eq!(
            reconcile_action(
                None,
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Disabled,
                true,
                false,
                true,
                Some(true)
            ),
            ReconcileAction::Acquire
        );
    }

    #[test]
    fn only_verified_applied_transitions_update_effective_success() {
        assert!(transition_applied(GuardedSystemProxyTransition::Applied));
        assert!(transition_applied(
            GuardedSystemProxyTransition::AlreadyInState
        ));
        assert!(!transition_applied(
            GuardedSystemProxyTransition::OwnershipChanged
        ));
        assert!(!transition_applied(
            GuardedSystemProxyTransition::NotManaged
        ));
    }

    #[test]
    fn readiness_hysteresis_rejects_flaps_and_resets_on_target_change() {
        let now = Instant::now();
        let mut window = ReadinessWindow::default();
        window.observe(18745, true, now);
        window.observe(18745, true, now + Duration::from_secs(1));
        assert!(!window.recovered(now + Duration::from_secs(1)));
        window.observe(18745, true, now + Duration::from_secs(2));
        assert!(window.recovered(now + Duration::from_secs(2)));
        window.observe(18745, false, now + Duration::from_secs(3));
        assert!(!window.recovered(now + Duration::from_secs(3)));
        assert!(!window.fail_open_due(now + Duration::from_secs(5), Duration::from_secs(3)));
        assert!(window.fail_open_due(now + Duration::from_secs(6), Duration::from_secs(3)));
        window.observe(18746, true, now + Duration::from_secs(7));
        assert!(!window.recovered(now + Duration::from_secs(7)));
        assert!(!window.fail_open_due(now + Duration::from_secs(20), Duration::from_secs(3)));
    }

    #[test]
    fn fail_closed_and_unready_suspended_proxy_do_not_mutate_os() {
        let active = lease(true, false, 18745);
        assert_eq!(
            reconcile_action(
                Some(&active),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::ThisBifrost,
                false,
                false,
                false,
                Some(true)
            ),
            ReconcileAction::Wait
        );
        assert_eq!(
            reconcile_action(
                Some(&active),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::ThisBifrost,
                false,
                true,
                false,
                Some(true)
            ),
            ReconcileAction::Suspend
        );
        let suspended = lease(false, true, 18745);
        assert_eq!(
            reconcile_action(
                Some(&suspended),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Other,
                false,
                true,
                false,
                Some(true)
            ),
            ReconcileAction::Wait
        );
    }
    #[test]
    fn automatic_restart_flags_cannot_freeze_an_intervening_user_toggle() {
        let mut config = bifrost_storage::NewSystemProxyConfig {
            intent_revision: 2,
            bypass: "latest.internal".into(),
            enabled: false,
            ..Default::default()
        };
        assert_eq!(
            resolve_startup_system_proxy_intent(
                true,
                false,
                Some("stale".into()),
                &config,
                Some("1")
            ),
            (false, "latest.internal".into())
        );
        config.enabled = true;
        assert_eq!(
            resolve_startup_system_proxy_intent(false, true, None, &config, Some("1")),
            (true, "latest.internal".into())
        );
        assert_eq!(
            resolve_startup_system_proxy_intent(
                false,
                true,
                Some("session".into()),
                &config,
                Some("2")
            ),
            (false, "session".into())
        );
        assert!(!resolve_startup_system_proxy_intent(false, true, None, &config, None).0);
        assert!(resolve_startup_system_proxy_intent(false, true, None, &config, Some("invalid")).0);
    }
    struct FaultingBackend {
        lease: ManagedSystemProxyOwnership,
        suspend_attempts: usize,
        fail_first_suspend: bool,
        reconcile_attempts: usize,
    }
    impl ProxyTransitions for FaultingBackend {
        fn verify(&mut self, _: &str) -> bifrost_core::Result<ManagedSystemProxyVerification> {
            panic!("this fixture only exercises interrupted transitions")
        }
        fn suspend(&mut self, _: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
            self.suspend_attempts += 1;
            self.lease.applied = false;
            self.lease.phase =
                Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspending);
            if self.fail_first_suspend && self.suspend_attempts == 1 {
                return Err(bifrost_core::BifrostError::Config(
                    "injected partial OS failure".into(),
                ));
            }
            self.lease.phase = Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended);
            Ok(GuardedSystemProxyTransition::Applied)
        }
        fn reconcile(&mut self, _: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
            self.reconcile_attempts += 1;
            self.lease.applied = true;
            self.lease.phase = Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Applied);
            Ok(GuardedSystemProxyTransition::Applied)
        }
        fn release(
            &mut self,
            generation: &str,
        ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
            self.suspend(generation)
        }
        fn acquire(
            &mut self,
            _: &str,
            _: u16,
            _: &str,
        ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
            panic!("must not reacquire a journaled transition")
        }
    }
    #[test]
    fn partial_suspend_failure_is_retried_by_production_transition_driver() {
        let mut backend = FaultingBackend {
            lease: lease(true, false, 18745),
            suspend_attempts: 0,
            fail_first_suspend: true,
            reconcile_attempts: 0,
        };
        for attempt in 0..2 {
            let action = reconcile_action(
                Some(&backend.lease),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::ThisBifrost,
                false,
                true,
                false,
                Some(true),
            );
            assert_eq!(action, ReconcileAction::Suspend);
            let result = perform_transition(
                &mut backend,
                action,
                Some("lease-1"),
                "127.0.0.1",
                18745,
                "localhost",
            );
            assert_eq!(result.is_ok(), attempt == 1);
        }
        assert_eq!(backend.suspend_attempts, 2);
        assert_eq!(
            backend.lease.phase,
            Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended)
        );
    }
    #[test]
    fn interrupted_apply_completes_without_new_acquisition() {
        let mut backend = FaultingBackend {
            lease: lease(false, false, 18745),
            suspend_attempts: 0,
            fail_first_suspend: false,
            reconcile_attempts: 0,
        };
        let action = reconcile_action(
            Some(&backend.lease),
            ("127.0.0.1", 18745),
            SystemProxyOwnership::Other,
            true,
            false,
            false,
            Some(true),
        );
        assert_eq!(action, ReconcileAction::Adopt);
        assert!(perform_transition(
            &mut backend,
            action,
            Some("lease-1"),
            "127.0.0.1",
            18745,
            "localhost"
        )
        .is_ok());
        assert_eq!(backend.reconcile_attempts, 1);
    }
    #[test]
    fn unknown_intent_allows_only_owned_fail_open_cleanup() {
        let mut backend = FaultingBackend {
            lease: lease(true, false, 18745),
            suspend_attempts: 0,
            fail_first_suspend: false,
            reconcile_attempts: 0,
        };
        let action = reconcile_action(
            Some(&backend.lease),
            ("127.0.0.1", 18745),
            SystemProxyOwnership::ThisBifrost,
            false,
            true,
            false,
            None,
        );
        assert_eq!(action, ReconcileAction::Suspend);
        assert!(perform_transition(
            &mut backend,
            action,
            Some("lease-1"),
            "127.0.0.1",
            18745,
            ""
        )
        .is_ok());
        assert_eq!(
            reconcile_action(
                Some(&backend.lease),
                ("127.0.0.1", 18745),
                SystemProxyOwnership::Other,
                true,
                false,
                false,
                None
            ),
            ReconcileAction::Wait
        );
    }
    #[test]
    fn automatic_child_honors_stop_before_listener_publication() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_automatic_recovery_not_stopped(dir.path(), true).is_ok());
        for mode in [
            bifrost_core::SystemProxyShutdownMode::BackgroundCleanup,
            bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
        ] {
            bifrost_core::write_system_proxy_shutdown_mode(dir.path(), mode).unwrap();
            assert!(ensure_automatic_recovery_not_stopped(dir.path(), true).is_err());
            assert!(ensure_automatic_recovery_not_stopped(dir.path(), false).is_ok());
        }
    }
    #[test]
    fn explicit_start_clears_prior_stop_but_automatic_start_cannot() {
        let dir = tempfile::tempdir().unwrap();
        for mode in [
            bifrost_core::SystemProxyShutdownMode::ForegroundCleanup,
            bifrost_core::SystemProxyShutdownMode::BackgroundCleanup,
        ] {
            bifrost_core::write_system_proxy_shutdown_mode(dir.path(), mode).unwrap();
            assert!(prepare_proxy_startup_markers(dir.path(), true).is_err());
            assert_eq!(
                bifrost_core::read_system_proxy_shutdown_mode(dir.path()),
                Some(mode)
            );
            assert!(prepare_proxy_startup_markers(dir.path(), false).is_ok());
            assert!(bifrost_core::read_system_proxy_shutdown_mode(dir.path()).is_none());
        }
    }
}
