use super::*;
use std::collections::VecDeque;

fn error(message: &str) -> bifrost_core::BifrostError {
    bifrost_core::BifrostError::Config(message.into())
}

fn owned(suspended: bool) -> ManagedSystemProxyOwnership {
    ManagedSystemProxyOwnership {
        schema_version: 3,
        generation: "lease-1".into(),
        original: bifrost_core::system_proxy::ProxyBackup {
            enable: true,
            host: "corporate-proxy".into(),
            port: 8080,
            bypass: "*.internal".into(),
        },
        target: bifrost_core::system_proxy::ProxyBackup {
            enable: true,
            host: "127.0.0.1".into(),
            port: 18745,
            bypass: "localhost".into(),
        },
        applied: !suspended,
        phase: Some(if suspended {
            bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspended
        } else {
            bifrost_core::system_proxy::ManagedSystemProxyPhase::Applied
        }),
        authorization_suppressed: false,
    }
}

struct FakeIo {
    config: bifrost_storage::NewSystemProxyConfig,
    reads: VecDeque<bifrost_core::Result<bifrost_storage::NewSystemProxyConfig>>,
    ownership_reads: VecDeque<bifrost_core::Result<Option<ManagedSystemProxyOwnership>>>,
    lease: Option<ManagedSystemProxyOwnership>,
    observed: SystemProxyOwnership,
    outcomes: VecDeque<bifrost_core::Result<GuardedSystemProxyTransition>>,
    calls: Vec<&'static str>,
    acquired_bypass: Option<String>,
    attached_generation: Option<String>,
    verification_calls: Vec<String>,
    verification_outcomes: VecDeque<bifrost_core::Result<ManagedSystemProxyVerification>>,
    observations: usize,
}

impl FakeIo {
    fn new(lease: Option<ManagedSystemProxyOwnership>) -> Self {
        Self {
            config: bifrost_storage::NewSystemProxyConfig {
                enabled: true,
                recovery_mode: SystemProxyRecoveryMode::FailOpen,
                recovery_grace_secs: 3,
                ..Default::default()
            },
            reads: VecDeque::new(),
            ownership_reads: VecDeque::new(),
            lease,
            observed: SystemProxyOwnership::Disabled,
            outcomes: VecDeque::new(),
            calls: Vec::new(),
            acquired_bypass: None,
            attached_generation: None,
            verification_calls: Vec::new(),
            verification_outcomes: VecDeque::new(),
            observations: 0,
        }
    }

    fn transition(
        &mut self,
        action: &'static str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        self.calls.push(action);
        let result = self
            .outcomes
            .pop_front()
            .unwrap_or(Ok(GuardedSystemProxyTransition::Applied));
        if result
            .as_ref()
            .is_ok_and(|outcome| transition_applied(*outcome))
        {
            let mut next = owned(action == "suspend");
            if let Some(current) = &self.lease {
                next.generation = current.generation.clone();
                next.target = current.target.clone();
            }
            self.attached_generation = if matches!(action, "release" | "suspend") {
                None
            } else {
                Some(next.generation.clone())
            };
            self.lease = (action != "release").then_some(next);
        } else if action == "suspend" && result.is_err() {
            // A partial OS write is journaled before the error reaches the
            // coordinator. Retrying an unchanged Applied lease would miss the
            // original applied=false/Suspending recovery regression.
            let mut partial = owned(true);
            partial.phase = Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspending);
            self.lease = Some(partial);
        }
        result
    }
}

impl ProxyTransitions for FakeIo {
    fn verify(&mut self, generation: &str) -> bifrost_core::Result<ManagedSystemProxyVerification> {
        assert_eq!(self.lease.as_ref().unwrap().generation, generation);
        self.verification_calls.push(generation.into());
        self.verification_outcomes
            .pop_front()
            .unwrap_or(Ok(ManagedSystemProxyVerification::Verified))
    }
    fn suspend(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        assert_eq!(self.lease.as_ref().unwrap().generation, generation);
        self.transition("suspend")
    }
    fn reconcile(
        &mut self,
        generation: &str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        assert_eq!(self.lease.as_ref().unwrap().generation, generation);
        self.transition("reconcile")
    }
    fn release(&mut self, generation: &str) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        assert_eq!(self.lease.as_ref().unwrap().generation, generation);
        self.transition("release")
    }
    fn acquire(
        &mut self,
        host: &str,
        port: u16,
        bypass: &str,
    ) -> bifrost_core::Result<GuardedSystemProxyTransition> {
        assert_eq!((host, port), ("127.0.0.1", 18745));
        self.acquired_bypass = Some(bypass.into());
        self.transition("acquire")
    }
}

impl ReconcileIo for FakeIo {
    fn is_attached(&self, generation: &str) -> bool {
        self.attached_generation.as_deref() == Some(generation)
    }
    fn persisted_intent(&mut self) -> bifrost_core::Result<bifrost_storage::NewSystemProxyConfig> {
        match self.reads.pop_front() {
            Some(Ok(value)) => {
                self.config = value.clone();
                Ok(value)
            }
            Some(Err(error)) => Err(error),
            None => Ok(self.config.clone()),
        }
    }
    fn ownership(&mut self) -> bifrost_core::Result<Option<ManagedSystemProxyOwnership>> {
        self.ownership_reads
            .pop_front()
            .unwrap_or_else(|| Ok(self.lease.clone()))
    }
    fn observed(&mut self, _: &str, _: u16) -> SystemProxyOwnership {
        self.observations += 1;
        self.observed
    }
    fn cancellation_generation(&mut self) -> Option<String> {
        self.lease.as_ref().map(|lease| lease.generation.clone())
    }
}

fn fixture(
    now: Instant,
) -> (
    tempfile::TempDir,
    SystemProxyReconcileConfig,
    ReconcileState,
) {
    let dir = tempfile::tempdir().unwrap();
    let config = SystemProxyReconcileConfig {
        bifrost_dir: dir.path().to_path_buf(),
        system_proxy_manager: Arc::new(tokio::sync::RwLock::new(SystemProxyManager::new(
            dir.path().to_path_buf(),
        ))),
        desired_enabled: Arc::new(AtomicBool::new(true)),
        proxy_host: "127.0.0.1".into(),
        proxy_port: Arc::new(AtomicU16::new(18745)),
        wake_requested: Arc::new(AtomicBool::new(false)),
        startup_intent_revision: 0,
        initial_policy: (SystemProxyRecoveryMode::FailOpen, 3),
        expected_generation: Arc::new(parking_lot::RwLock::new(None)),
        system_proxy_bypass: "session.internal".into(),
        enabled_flag: Arc::new(AtomicBool::new(false)),
        stop_flag: Arc::new(AtomicBool::new(false)),
        daemon_mode: false,
    };
    let state = ReconcileState::new(&config, now);
    (dir, config, state)
}

fn healthy(state: &mut ReconcileState, now: Instant) {
    for offset in [2, 1, 0] {
        state
            .readiness
            .observe(18745, true, now - Duration::from_secs(offset));
    }
}

fn run(
    state: &mut ReconcileState,
    config: &SystemProxyReconcileConfig,
    io: &mut FakeIo,
    now: Instant,
    ready: bool,
) {
    let before = Ok(io.lease.clone());
    state.reconcile_locked(config, io, before, 18745, ready, now);
}

#[test]
fn partial_suspend_retries_then_health_resumes_without_losing_intent() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    config.enabled_flag.store(true, Ordering::Release);
    let mut io = FakeIo::new(Some(owned(false)));
    io.outcomes.push_back(Err(error("partial OS write")));
    assert!(!state.observe_and_should_inspect(&config, 18745, false, now, Some(&io.config)));
    let due = now + Duration::from_secs(3);
    assert!(state.observe_and_should_inspect(&config, 18745, false, due, Some(&io.config)));
    run(&mut state, &config, &mut io, due, false);
    assert!(config.enabled_flag.load(Ordering::Acquire));
    assert!(!io.lease.as_ref().unwrap().applied);
    assert_eq!(
        io.lease.as_ref().unwrap().phase,
        Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::Suspending)
    );
    assert_eq!(state.next_inspection, due + Duration::from_secs(2));
    run(
        &mut state,
        &config,
        &mut io,
        due + Duration::from_secs(2),
        false,
    );
    assert!(!config.enabled_flag.load(Ordering::Acquire));
    assert!(config.desired_enabled.load(Ordering::Acquire));
    let recovered = due + Duration::from_secs(20);
    healthy(&mut state, recovered);
    run(&mut state, &config, &mut io, recovered, true);
    assert_eq!(io.calls, ["suspend", "suspend", "reconcile"]);
    assert!(config.enabled_flag.load(Ordering::Acquire));
    assert!(io.config.enabled);
}

#[test]
fn unknown_intent_allows_owned_fail_open_but_never_resume() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    state.readiness.observe(18745, false, now);
    io.reads.push_back(Err(error("unreadable config")));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(3),
        false,
    );
    assert_eq!(io.calls, ["suspend"]);
    healthy(&mut state, now + Duration::from_secs(10));
    io.reads.push_back(Err(error("still unreadable")));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["suspend"]);
    assert!(!config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn newer_disable_after_resume_is_compensated_and_failed_release_is_retried() {
    for fail_release in [false, true] {
        let now = Instant::now();
        let (_dir, config, mut state) = fixture(now);
        let mut io = FakeIo::new(Some(owned(true)));
        healthy(&mut state, now);
        io.reads.extend([
            Ok(io.config.clone()),
            Ok(bifrost_storage::NewSystemProxyConfig {
                enabled: false,
                intent_revision: 1,
                ..io.config.clone()
            }),
        ]);
        io.outcomes
            .push_back(Ok(GuardedSystemProxyTransition::Applied));
        if fail_release {
            io.outcomes.push_back(Err(error("release interrupted")));
        }
        run(&mut state, &config, &mut io, now, true);
        assert!(!config.desired_enabled.load(Ordering::Acquire));
        assert_eq!(io.calls, ["reconcile", "release"]);
        assert_eq!(config.enabled_flag.load(Ordering::Acquire), fail_release);
        assert_eq!(state.next_inspection, now);
        if fail_release {
            run(
                &mut state,
                &config,
                &mut io,
                now + Duration::from_secs(1),
                true,
            );
            assert_eq!(io.calls, ["reconcile", "release", "release"]);
            assert!(!config.enabled_flag.load(Ordering::Acquire));
        }
    }
}

#[test]
fn cancelled_generation_waits_until_a_new_explicit_intent_is_healthy() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(true)));
    healthy(&mut state, now);
    io.outcomes.push_back(Err(error("UserCancelled")));
    run(&mut state, &config, &mut io, now, true);
    assert_eq!(state.cancelled_generation.as_deref(), Some("lease-1"));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["reconcile"]);
    io.config.intent_revision = 1;
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(11),
        true,
    );
    assert_eq!(state.cancelled_generation, None);
    assert_eq!(io.calls, ["reconcile"]);
    healthy(&mut state, now + Duration::from_secs(14));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(14),
        true,
    );
    assert_eq!(io.calls, ["reconcile", "reconcile"]);
    assert!(config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn ownership_race_or_error_never_authorizes_a_transition() {
    let now = Instant::now();
    for kind in 0..3 {
        let (_dir, config, mut state) = fixture(now);
        let mut io = FakeIo::new(Some(owned(false)));
        healthy(&mut state, now);
        let before = match kind {
            0 => Ok(None),
            1 => Err(error("pre-probe journal unreadable")),
            _ => {
                io.ownership_reads
                    .push_back(Err(error("locked journal unreadable")));
                Ok(io.lease.clone())
            }
        };
        state.reconcile_locked(&config, &mut io, before, 18745, true, now);
        assert!(io.calls.is_empty());
        if kind == 0 {
            assert!(!state.readiness.recovered(now));
        }
    }
}

#[test]
fn empty_or_changed_recovery_fence_preserves_external_ownership() {
    let now = Instant::now();
    for fence in ["", "replacement-lease"] {
        let (_dir, config, mut state) = fixture(now);
        *config.expected_generation.write() = Some(fence.into());
        config.enabled_flag.store(true, Ordering::Release);
        let mut io = FakeIo::new(Some(owned(false)));
        healthy(&mut state, now);
        run(&mut state, &config, &mut io, now, true);
        assert!(io.calls.is_empty());
        assert!(!config.enabled_flag.load(Ordering::Acquire));
        assert!(!state.allow_initial_acquire);
    }
}

#[test]
fn fresh_acquisition_uses_session_bypass_then_new_intent_uses_latest_bypass() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(None);
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    assert_eq!(io.acquired_bypass.as_deref(), Some("session.internal"));
    assert!(config.enabled_flag.load(Ordering::Acquire));
    io.lease = None;
    io.config.intent_revision = 1;
    io.config.bypass = "latest.internal".into();
    let next = now + Duration::from_secs(10);
    run(&mut state, &config, &mut io, next, true);
    assert_eq!(io.calls, ["acquire"]); // a new intent still needs hysteresis
    healthy(&mut state, next + Duration::from_secs(3));
    run(
        &mut state,
        &config,
        &mut io,
        next + Duration::from_secs(3),
        true,
    );
    assert_eq!(io.calls, ["acquire", "acquire"]);
    assert_eq!(io.acquired_bypass.as_deref(), Some("latest.internal"));
}

#[test]
fn preliminary_read_only_schedules_and_wake_resets_health() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let preliminary = bifrost_storage::NewSystemProxyConfig {
        enabled: false,
        intent_revision: 1,
        ..Default::default()
    };
    assert!(state.observe_and_should_inspect(&config, 18745, true, now, Some(&preliminary)));
    assert!(config.desired_enabled.load(Ordering::Acquire));
    healthy(&mut state, now + Duration::from_secs(3));
    assert!(state.readiness.recovered(now + Duration::from_secs(3)));
    state.wake(now + Duration::from_secs(4));
    assert!(!state.readiness.recovered(now + Duration::from_secs(4)));
    config.desired_enabled.store(false, Ordering::Release);
    assert!(state.observe_and_should_inspect(
        &config,
        18745,
        false,
        now + Duration::from_secs(5),
        None
    ));
}

#[test]
fn ownership_change_and_missing_postwrite_journal_do_not_replay_acquire() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(None);
    healthy(&mut state, now);
    io.outcomes
        .push_back(Ok(GuardedSystemProxyTransition::OwnershipChanged));
    run(&mut state, &config, &mut io, now, true);
    assert!(!state.allow_initial_acquire);
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["acquire"]);
    assert!(!config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn fail_closed_does_not_suspend_and_postwrite_read_error_does_not_erase_intent() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    io.config.recovery_mode = SystemProxyRecoveryMode::FailClosed;
    state.last_policy.0 = SystemProxyRecoveryMode::FailClosed;
    state.readiness.observe(18745, false, now);
    assert!(!state.observe_and_should_inspect(
        &config,
        18745,
        false,
        now + Duration::from_secs(10),
        Some(&io.config)
    ));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        false,
    );
    assert!(io.calls.is_empty());
    healthy(&mut state, now + Duration::from_secs(20));
    io.reads
        .extend([Ok(io.config.clone()), Err(error("postwrite read failed"))]);
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(20),
        true,
    );
    assert_eq!(io.calls, ["reconcile"]);
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert!(config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn regressed_disk_revision_cannot_authorize_acquire_resume_or_release() {
    let now = Instant::now();
    for stale_enabled in [false, true] {
        for lease in [None, Some(owned(true)), Some(owned(false))] {
            let (_dir, config, mut state) = fixture(now);
            state.last_intent_revision = 2;
            state.last_policy = (SystemProxyRecoveryMode::FailClosed, 5);
            let mut io = FakeIo::new(lease);
            io.config.intent_revision = 1;
            io.config.enabled = stale_enabled;
            io.config.recovery_mode = SystemProxyRecoveryMode::FailOpen;
            healthy(&mut state, now);
            run(&mut state, &config, &mut io, now, true);
            assert!(io.calls.is_empty());
            assert!(config.desired_enabled.load(Ordering::Acquire));
            assert_eq!(state.last_intent_revision, 2);
            assert_eq!(state.last_policy, (SystemProxyRecoveryMode::FailClosed, 5));
        }
    }
}

#[test]
fn regressed_intent_still_allows_only_last_known_fail_open_cleanup() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    state.last_intent_revision = 2;
    state.readiness.observe(18745, false, now);
    let mut io = FakeIo::new(Some(owned(false)));
    io.config.intent_revision = 1;
    io.config.enabled = false;
    io.config.recovery_mode = SystemProxyRecoveryMode::FailClosed;
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(3),
        false,
    );
    assert_eq!(io.calls, ["suspend"]);
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert_eq!(state.last_policy, (SystemProxyRecoveryMode::FailOpen, 3));
}

#[test]
fn postwrite_regressed_disable_cannot_release_a_newer_enabled_intent() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    state.last_intent_revision = 2;
    let mut io = FakeIo::new(Some(owned(true)));
    io.config.intent_revision = 2;
    io.reads.extend([
        Ok(io.config.clone()),
        Ok(bifrost_storage::NewSystemProxyConfig {
            enabled: false,
            intent_revision: 1,
            ..io.config.clone()
        }),
    ]);
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    assert_eq!(io.calls, ["reconcile"]);
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert!(config.enabled_flag.load(Ordering::Acquire));
    assert_eq!(state.last_intent_revision, 2);
}

#[test]
fn owned_transitions_without_generation_fail_before_any_backend_call() {
    let mut io = FakeIo::new(None);
    for action in [
        ReconcileAction::Resume,
        ReconcileAction::Adopt,
        ReconcileAction::Suspend,
        ReconcileAction::Release,
    ] {
        let failure =
            perform_transition(&mut io, action, None, "127.0.0.1", 18745, "").unwrap_err();
        assert!(failure.to_string().contains("missing ownership generation"));
    }
    assert!(io.calls.is_empty());
}

#[test]
fn direct_start_enable_and_configured_default_preserve_bypass() {
    let mut configured = bifrost_storage::NewSystemProxyConfig {
        enabled: false,
        bypass: "configured.internal".into(),
        ..Default::default()
    };
    assert_eq!(
        resolve_startup_system_proxy_intent(true, false, None, &configured, None),
        (true, "configured.internal".into())
    );
    configured.enabled = true;
    assert_eq!(
        resolve_startup_system_proxy_intent(false, false, None, &configured, None),
        (true, "configured.internal".into())
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn concrete_unsupported_backend_is_observational_and_cannot_acquire_or_release() {
    let (_dir, config, _state) = fixture(Instant::now());
    let mut manager = config.system_proxy_manager.blocking_write();
    let mut backend = LockedProxyBackend {
        manager: &mut manager,
        config: &config,
    };
    assert!(backend.persisted_intent().is_err());
    let persisted = bifrost_storage::ConfigManager::new(config.bifrost_dir.clone()).unwrap();
    futures::executor::block_on(persisted.update_system_proxy_config(
        bifrost_storage::SystemProxyConfigUpdate {
            enabled: Some(false),
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(!backend.persisted_intent().unwrap().enabled);
    assert!(backend.ownership().unwrap().is_none());
    assert_eq!(
        backend.observed("127.0.0.1", 18745),
        SystemProxyOwnership::Unknown
    );
    assert_eq!(backend.cancellation_generation(), None);
    assert_eq!(
        backend.verify("unowned").unwrap(),
        ManagedSystemProxyVerification::NotManaged
    );
    for result in [
        backend.suspend("unowned"),
        backend.reconcile("unowned"),
        backend.release("unowned"),
        backend.acquire("127.0.0.1", 18745, "localhost"),
    ] {
        assert_eq!(result.unwrap(), GuardedSystemProxyTransition::NotManaged);
    }
    assert!(!config.bifrost_dir.join("proxy_state.json").exists());
    assert!(!config.bifrost_dir.join("proxy_backup.json").exists());
    assert!(!backend.persisted_intent().unwrap().enabled);
}

#[test]
fn repeated_healthy_inspections_verify_without_repeating_acquire_adopt_or_resume() {
    for initial_lease in [None, Some(owned(false)), Some(owned(true))] {
        let now = Instant::now();
        let (_dir, config, mut state) = fixture(now);
        let expected_transition = if initial_lease.is_none() {
            "acquire"
        } else {
            "reconcile"
        };
        let mut io = FakeIo::new(initial_lease);
        io.observed = SystemProxyOwnership::ThisBifrost;
        healthy(&mut state, now);
        run(&mut state, &config, &mut io, now, true);
        let acquired = io.lease.clone();
        for cycle in 1..=3 {
            let next = now + system_proxy_reconcile_interval() * cycle;
            assert!(state.observe_and_should_inspect(&config, 18745, true, next, Some(&io.config)));
            run(&mut state, &config, &mut io, next, true);
            assert_eq!(io.lease, acquired);
            assert!(config.enabled_flag.load(Ordering::Acquire));
        }
        assert_eq!(io.calls, [expected_transition]);
        assert_eq!(io.verification_calls, ["lease-1", "lease-1", "lease-1"]);
        assert_eq!(io.observations, 4);
    }
}

#[test]
fn detached_replaced_or_pending_lease_must_be_adopted_before_read_only_verification() {
    for changed in ["detached", "generation", "pending"] {
        let now = Instant::now();
        let (_dir, config, mut state) = fixture(now);
        let mut io = FakeIo::new(Some(owned(false)));
        healthy(&mut state, now);
        run(&mut state, &config, &mut io, now, true);
        match changed {
            "detached" => io.attached_generation = None,
            "generation" => io.lease.as_mut().unwrap().generation = "lease-2".into(),
            _ => {
                let lease = io.lease.as_mut().unwrap();
                lease.applied = false;
                lease.phase =
                    Some(bifrost_core::system_proxy::ManagedSystemProxyPhase::PendingApply);
            }
        }
        run(
            &mut state,
            &config,
            &mut io,
            now + Duration::from_secs(10),
            true,
        );
        assert_eq!(io.calls, ["reconcile", "reconcile"], "{changed}");
        assert!(io.verification_calls.is_empty(), "{changed}");
        run(
            &mut state,
            &config,
            &mut io,
            now + Duration::from_secs(20),
            true,
        );
        assert_eq!(io.calls, ["reconcile", "reconcile"], "{changed}");
        assert_eq!(
            io.verification_calls,
            [io.lease.as_ref().unwrap().generation.clone()]
        );
    }
}

#[test]
fn read_only_verification_rejects_generation_replacement_despite_matching_aggregate_status() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    io.observed = SystemProxyOwnership::ThisBifrost;
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    io.verification_outcomes
        .push_back(Ok(ManagedSystemProxyVerification::OwnershipChanged));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["reconcile"]);
    assert_eq!(io.verification_calls, ["lease-1"]);
    assert!(!config.enabled_flag.load(Ordering::Acquire));
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert!(!state.allow_initial_acquire);
}

#[test]
fn newer_disable_during_read_only_verification_still_releases_the_owned_generation() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    io.reads.extend([
        Ok(io.config.clone()),
        Ok(bifrost_storage::NewSystemProxyConfig {
            enabled: false,
            intent_revision: 1,
            ..io.config.clone()
        }),
    ]);
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["reconcile", "release"]);
    assert_eq!(io.verification_calls, ["lease-1"]);
    assert!(io.lease.is_none());
    assert!(!config.desired_enabled.load(Ordering::Acquire));
    assert!(!config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn read_only_inspection_error_retries_without_reacquisition_or_losing_intent() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    let before = io.lease.clone();
    io.verification_outcomes
        .push_back(Err(error("cannot read owned HTTPS field")));
    let next = now + Duration::from_secs(10);
    run(&mut state, &config, &mut io, next, true);
    assert_eq!(state.next_inspection, next + Duration::from_secs(2));
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert_eq!(io.lease, before);
    run(
        &mut state,
        &config,
        &mut io,
        next + Duration::from_secs(2),
        true,
    );
    assert_eq!(io.calls, ["reconcile"]);
    assert_eq!(io.verification_calls, ["lease-1", "lease-1"]);
    assert!(config.enabled_flag.load(Ordering::Acquire));
}

#[test]
fn same_generation_field_drift_reconciles_once_then_returns_to_read_only_inspections() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    io.verification_outcomes
        .push_back(Ok(ManagedSystemProxyVerification::Drifted));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["reconcile", "reconcile"]);
    assert!(config.enabled_flag.load(Ordering::Acquire));
    assert!(config.desired_enabled.load(Ordering::Acquire));
    assert_eq!(io.lease.as_ref().unwrap().generation, "lease-1");
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(20),
        true,
    );
    assert_eq!(io.calls, ["reconcile", "reconcile"]);
    assert_eq!(io.verification_calls, ["lease-1", "lease-1"]);
}

#[test]
fn lease_disappearing_during_verification_clears_active_flag_without_reacquiring() {
    let now = Instant::now();
    let (_dir, config, mut state) = fixture(now);
    let mut io = FakeIo::new(Some(owned(false)));
    healthy(&mut state, now);
    run(&mut state, &config, &mut io, now, true);
    io.verification_outcomes
        .push_back(Ok(ManagedSystemProxyVerification::NotManaged));
    run(
        &mut state,
        &config,
        &mut io,
        now + Duration::from_secs(10),
        true,
    );
    assert_eq!(io.calls, ["reconcile"]);
    assert!(!config.enabled_flag.load(Ordering::Acquire));
    assert!(!state.allow_initial_acquire);
}
