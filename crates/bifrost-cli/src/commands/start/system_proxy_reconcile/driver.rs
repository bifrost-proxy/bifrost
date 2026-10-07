//! The production reconcile step, with I/O injected for deterministic race tests.
use super::*;

pub(super) trait ReconcileIo: ProxyTransitions {
    fn persisted_intent(&mut self) -> bifrost_core::Result<bifrost_storage::NewSystemProxyConfig>;
    fn ownership(&mut self) -> bifrost_core::Result<Option<ManagedSystemProxyOwnership>>;
    fn observed(&mut self, host: &str, port: u16) -> SystemProxyOwnership;
    fn cancellation_generation(&mut self) -> Option<String>;
}

pub(super) struct ReconcileState {
    readiness: ReadinessWindow,
    allow_initial_acquire: bool,
    next_inspection: Instant,
    last_intent_revision: u64,
    last_policy: (SystemProxyRecoveryMode, u64),
    cancelled_generation: Option<String>,
}

impl ReconcileState {
    pub(super) fn new(config: &SystemProxyReconcileConfig, now: Instant) -> Self {
        Self {
            readiness: ReadinessWindow::default(),
            allow_initial_acquire: config.expected_generation.read().is_none(),
            next_inspection: now,
            last_intent_revision: config.startup_intent_revision,
            last_policy: config.initial_policy,
            cancelled_generation: None,
        }
    }

    pub(super) fn wake(&mut self, now: Instant) {
        self.readiness = ReadinessWindow::default();
        self.next_inspection = now;
    }

    pub(super) fn observe_and_should_inspect(
        &mut self,
        config: &SystemProxyReconcileConfig,
        port: u16,
        ready: bool,
        now: Instant,
        preliminary: Option<&bifrost_storage::NewSystemProxyConfig>,
    ) -> bool {
        self.readiness.observe(port, ready, now);
        // This read only schedules work. Intent is published only after the
        // caller takes the manager lock and rechecks the current target.
        let changed =
            preliminary.is_some_and(|value| value.intent_revision > self.last_intent_revision);
        let fail_open_due = self.last_policy.0 == SystemProxyRecoveryMode::FailOpen
            && self
                .readiness
                .fail_open_due(now, Duration::from_secs(self.last_policy.1.clamp(3, 5)));
        changed
            || (!config.desired_enabled.load(Ordering::Acquire) && now >= self.next_inspection)
            || (!ready && fail_open_due)
            || (self.readiness.recovered(now) && now >= self.next_inspection)
    }

    /// Called only while the manager writer lock is held and the probe's port,
    /// shutdown flag and shutdown marker have been revalidated by the caller.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reconcile_locked(
        &mut self,
        config: &SystemProxyReconcileConfig,
        backend: &mut impl ReconcileIo,
        before: bifrost_core::Result<Option<ManagedSystemProxyOwnership>>,
        port: u16,
        ready: bool,
        now: Instant,
    ) {
        let latest = match backend.persisted_intent() {
            Ok(latest) if latest.intent_revision >= self.last_intent_revision => Some(latest),
            Ok(_) => {
                tracing::warn!(
                    "proxy intent revision regressed; only owned safety cleanup remains allowed"
                );
                None
            }
            Err(error) => {
                tracing::warn!(%error, "cannot read proxy intent; only owned safety cleanup remains allowed");
                None
            }
        };
        if let Some(latest) = &latest {
            self.last_policy = (latest.recovery_mode, latest.recovery_grace_secs);
            if latest.intent_revision > self.last_intent_revision {
                config
                    .desired_enabled
                    .store(latest.enabled, Ordering::Release);
                self.last_intent_revision = latest.intent_revision;
                self.allow_initial_acquire = true;
                *config.expected_generation.write() = None;
                self.cancelled_generation = None;
                self.readiness = ReadinessWindow::default();
                self.readiness.observe(port, ready, now);
                self.next_inspection = now;
            }
        }
        let current = backend.ownership();
        match (before, current) {
            (Ok(before), Ok(current))
                if generation(before.as_ref()) == generation(current.as_ref()) =>
            {
                let fence = config.expected_generation.read().clone();
                let owns_fence = fence.as_deref().is_none_or(|expected| {
                    !expected.is_empty() && generation(current.as_ref()) == Some(expected)
                });
                let observed = backend.observed(&config.proxy_host, port);
                let recovered = latest.is_some() && self.readiness.recovered(now);
                let fail_open_due = self.last_policy.0 == SystemProxyRecoveryMode::FailOpen
                    && self
                        .readiness
                        .fail_open_due(now, Duration::from_secs(self.last_policy.1.clamp(3, 5)));
                let desired = latest
                    .as_ref()
                    .map(|value| effective_desired(value, config));
                let action = if owns_fence {
                    reconcile_action(
                        current.as_ref(),
                        (&config.proxy_host, port),
                        observed,
                        recovered,
                        fail_open_due,
                        self.allow_initial_acquire,
                        desired,
                    )
                } else {
                    ReconcileAction::PreserveExternal
                };
                let action = if matches!(
                    action,
                    ReconcileAction::Resume | ReconcileAction::Adopt | ReconcileAction::Acquire
                ) && self.cancelled_generation.as_deref()
                    == generation(current.as_ref())
                    && self.cancelled_generation.is_some()
                {
                    ReconcileAction::Wait
                } else {
                    action
                };
                let bypass = latest
                    .as_ref()
                    .filter(|value| value.intent_revision != config.startup_intent_revision)
                    .map(|value| value.bypass.as_str())
                    .unwrap_or(&config.system_proxy_bypass);
                let result = perform_transition(
                    backend,
                    action,
                    generation(current.as_ref()),
                    &config.proxy_host,
                    port,
                    bypass,
                );
                match result {
                    Ok(transition) if transition_applied(transition) => {
                        let mut active =
                            !matches!(action, ReconcileAction::Suspend | ReconcileAction::Release);
                        self.allow_initial_acquire = false;
                        *config.expected_generation.write() = None;
                        self.next_inspection = now + system_proxy_reconcile_interval();
                        // Cross-process intent writes do not share this mutex.
                        // Compensate a newer off before publishing success.
                        if let Ok(after) = backend.persisted_intent().and_then(|after| {
                            if after.intent_revision < self.last_intent_revision {
                                tracing::warn!("postwrite proxy intent revision regressed; refusing stale compensation");
                                Err(bifrost_core::BifrostError::Config(
                                    "postwrite proxy intent revision regressed".into(),
                                ))
                            } else {
                                Ok(after)
                            }
                        }) {
                            if after.intent_revision > self.last_intent_revision {
                                config
                                    .desired_enabled
                                    .store(after.enabled, Ordering::Release);
                                self.last_intent_revision = after.intent_revision;
                                self.allow_initial_acquire = true;
                                self.next_inspection = now;
                            }
                            if active && !effective_desired(&after, config) {
                                if let Ok(Some(owned)) = backend.ownership() {
                                    match backend.release(&owned.generation) {
                                        Ok(outcome) if transition_applied(outcome) => {
                                            active = false
                                        }
                                        outcome => {
                                            self.next_inspection = now;
                                            tracing::warn!(?outcome, "new disable remains pending after in-flight proxy transition");
                                        }
                                    }
                                }
                            }
                        }
                        config.enabled_flag.store(active, Ordering::Release);
                        tracing::info!(
                            ?action,
                            ?transition,
                            port,
                            active,
                            "system proxy transition verified"
                        );
                    }
                    Ok(transition) => {
                        if transition == GuardedSystemProxyTransition::OwnershipChanged {
                            self.allow_initial_acquire = false;
                            config.enabled_flag.store(false, Ordering::Release);
                        }
                        self.next_inspection = now + system_proxy_reconcile_interval();
                        tracing::debug!(
                            ?action,
                            ?transition,
                            port,
                            "system proxy transition preserved current owner"
                        );
                    }
                    Err(error) => {
                        if error.to_string().contains("UserCancelled") {
                            self.allow_initial_acquire = false;
                            self.cancelled_generation = backend.cancellation_generation();
                        }
                        tracing::warn!(%error, ?action, port, daemon_mode = config.daemon_mode, "system proxy transition failed; intent and recovery state retained");
                        self.next_inspection = now + Duration::from_secs(2);
                    }
                }
            }
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(%error, "system proxy ownership unavailable; refusing automatic mutation");
            }
            _ => self.readiness = ReadinessWindow::default(),
        }
    }
}

#[cfg(test)]
mod tests;
