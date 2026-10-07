//! macOS lifecycle entry points share one ownership transaction engine.
use super::macos_backend::MacosBackend;
use super::macos_command::Privilege;
use super::macos_owned::{self, legacy_journal, observed_owned, Backend, Field, Intent, Value};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Acquisition {
    Explicit,
    Retry,
    IfUnmanaged,
}

fn backend(privilege: Privilege) -> MacosBackend {
    MacosBackend::native(privilege)
}

impl SystemProxyManager {
    pub fn get_current() -> Result<ProxyBackup> {
        Self::parse_macos_proxy()
            .map(|proxy| ProxyBackup::from(&proxy))
            .ok_or_else(|| {
                BifrostError::Config(
                    "networksetup/scutil could not read current macOS proxy settings".into(),
                )
            })
    }

    fn optional_managed_state(&self) -> Result<Option<ManagedProxyState>> {
        match self.load_managed_state() {
            Ok(state) => Ok(Some(state)),
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn prepare_legacy_journal(
        &self,
        state: &mut ManagedProxyState,
        os: &mut impl Backend,
    ) -> Result<()> {
        if state.macos_services.is_empty() {
            state.macos_services = legacy_journal(os, &state.target)?;
            state.schema_version = 3;
            if state.generation.is_empty() {
                state.generation = uuid::Uuid::now_v7().to_string();
            }
            self.write_managed_state(state)?;
        }
        Ok(())
    }

    fn run_owned_transition(
        &self,
        state: &mut ManagedProxyState,
        os: &mut impl Backend,
        intent: Intent,
    ) -> Result<macos_owned::TransitionResult> {
        macos_owned::transition(state, os, intent, |state| self.write_managed_state(state))
    }

    fn enable_macos(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        privilege: Privilege,
        acquisition: Acquisition,
        should_enable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "enable_macos")?;
        if !should_enable()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let mut os = self.macos_backend(privilege)?;
        let target = ProxyBackup {
            enable: true,
            host: host.into(),
            port,
            bypass: bypass.unwrap_or(DEFAULT_BYPASS).into(),
        };
        let mut state = if let Some(mut state) = self.optional_managed_state()? {
            if acquisition == Acquisition::IfUnmanaged {
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            self.prepare_legacy_journal(&mut state, &mut os)?;
            let target_changed = !state.target.target_matches(host, port);
            if target_changed && acquisition != Acquisition::Explicit {
                return Err(BifrostError::Config("System proxy is already managed under another target; use a generation-fenced retarget".into()));
            }
            if acquisition == Acquisition::Explicit
                && matches!(
                    state.phase(),
                    ManagedSystemProxyPhase::Applied | ManagedSystemProxyPhase::Suspended
                )
                && state
                    .macos_services
                    .iter()
                    .flat_map(|service| &service.fields)
                    .all(|field| field.pending.is_none())
            {
                macos_owned::refresh_explicit_acquisition(&mut state, &mut os)?;
            }
            // Pending/privilege retries keep the original snapshots/generation.
            if target_changed {
                state.generation = uuid::Uuid::now_v7().to_string();
                state.target = target;
                macos_owned::sanitize_original_targets(&mut state.macos_services, &state.target);
            } else {
                state.target.bypass = target.bypass;
            }
            state
        } else {
            let mut services = macos_owned::capture(&mut os)?;
            if acquisition == Acquisition::IfUnmanaged
                && services
                    .iter()
                    .flat_map(|service| &service.fields)
                    .any(|field| matches!(&field.before, Value::Protocol(proxy) if proxy.enabled))
            {
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            macos_owned::sanitize_original_targets(&mut services, &target);
            let original = services
                .iter()
                .flat_map(|s| &s.fields)
                .find_map(|field| {
                    if let Value::Protocol(proxy) = &field.before {
                        Some(ProxyBackup {
                            enable: proxy.enabled,
                            host: proxy.host.clone(),
                            port: proxy.port,
                            bypass: String::new(),
                        })
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    BifrostError::Config("networksetup returned no proxy snapshot".into())
                })?;
            ManagedProxyState {
                schema_version: 3,
                generation: uuid::Uuid::now_v7().to_string(),
                original,
                target,
                applied: false,
                phase: Some(ManagedSystemProxyPhase::PendingApply),
                authorization_suppressed: false,
                macos_services: services,
            }
        };
        if acquisition == Acquisition::Explicit {
            macos_owned::begin_explicit_acquisition(&mut state);
        }
        let intent = if matches!(
            state.phase(),
            ManagedSystemProxyPhase::Suspending
                | ManagedSystemProxyPhase::Suspended
                | ManagedSystemProxyPhase::Resuming
        ) {
            Intent::Resume
        } else {
            Intent::Apply
        };
        self.write_managed_state(&state)?;
        let result = self.run_owned_transition(&mut state, &mut os, intent)?;
        if !macos_owned::has_active_owned_protocol(&state) {
            self.detach_in_place();
            self.run_owned_transition(&mut state, &mut os, Intent::Restore)?;
            self.remove_managed_files_checked()?;
            return Err(BifrostError::Config("No macOS HTTP/HTTPS proxy protocols could be managed; authenticated or externally owned settings were preserved".into()));
        }
        self.attach_managed_state(&state);
        self.record_system_proxy_action("system_proxy_enabled", "enable");
        if result.ownership_changed {
            tracing::warn!("Some macOS proxy fields belong to another owner and were preserved");
        }
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn enable_if_unmanaged(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.enable_macos(
            host,
            port,
            bypass,
            Privilege::Direct,
            Acquisition::IfUnmanaged,
            || Ok(true),
        )
    }

    pub fn enable(&mut self, host: &str, port: u16, bypass: Option<&str>) -> Result<()> {
        self.enable_guarded(host, port, bypass, || Ok(true))
            .map(|_| ())
    }
    pub fn enable_with_privilege(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<()> {
        self.enable_with_privilege_guarded(host, port, bypass, || Ok(true))
            .map(|_| ())
    }
    pub fn enable_with_gui_auth(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<()> {
        self.enable_with_gui_auth_guarded(host, port, bypass, || Ok(true))
            .map(|_| ())
    }

    pub fn enable_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        should_enable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.enable_macos(
            host,
            port,
            bypass,
            Privilege::Direct,
            Acquisition::Explicit,
            should_enable,
        )
    }

    pub fn enable_with_privilege_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        should_enable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.enable_macos(
            host,
            port,
            bypass,
            Privilege::Sudo,
            Acquisition::Retry,
            should_enable,
        )
    }

    pub fn enable_with_gui_auth_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        should_enable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.enable_macos(
            host,
            port,
            bypass,
            Privilege::Gui,
            Acquisition::Retry,
            should_enable,
        )
    }

    fn remove_managed_files_checked(&self) -> Result<()> {
        for path in [self.backup_file_path(), self.state_file_path()] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if self.data_dir.exists() {
            persistence::sync_directory(&self.data_dir)?;
        }
        Ok(())
    }

    fn restore_macos_locked(
        &mut self,
        os: &mut impl Backend,
        expected: Option<(&str, u16)>,
    ) -> Result<SystemProxyDisableOutcome> {
        let mut state = match self.optional_managed_state()? {
            Some(state) => state,
            None => {
                let target = expected
                    .map(|(host, port)| ProxyBackup {
                        enable: true,
                        host: host.into(),
                        port,
                        bypass: String::new(),
                    })
                    .or_else(|| load_last_runtime_proxy_target(&self.data_dir));
                let Some(target) = target else {
                    // A backup alone is not proof that Bifrost owns the current
                    // proxy. Do not globally clear or restore arbitrary settings.
                    self.detach_in_place();
                    return Ok(SystemProxyDisableOutcome::NotEnabled);
                };
                let fields = legacy_journal(os, &target)?;
                if !fields
                    .iter()
                    .flat_map(|s| &s.fields)
                    .any(|f| !f.relinquished)
                {
                    self.detach_in_place();
                    return Ok(SystemProxyDisableOutcome::OwnedByOther);
                }
                let mut original = target.clone();
                original.enable = false;
                ManagedProxyState {
                    schema_version: 3,
                    generation: uuid::Uuid::now_v7().to_string(),
                    original,
                    target,
                    applied: true,
                    phase: Some(ManagedSystemProxyPhase::Applied),
                    authorization_suppressed: false,
                    macos_services: fields,
                }
            }
        };
        if expected.is_some_and(|(host, port)| !state.target.target_matches(host, port)) {
            return Ok(SystemProxyDisableOutcome::OwnedByOther);
        }
        self.prepare_legacy_journal(&mut state, os)?;
        let outcome = self.run_owned_transition(&mut state, os, Intent::Restore)?;
        self.remove_managed_files_checked()?;
        self.detach_in_place();
        Ok(if outcome.changed {
            SystemProxyDisableOutcome::Disabled
        } else if outcome.ownership_changed {
            SystemProxyDisableOutcome::OwnedByOther
        } else {
            SystemProxyDisableOutcome::NotEnabled
        })
    }

    fn restore_macos(
        &mut self,
        privilege: Privilege,
        expected: Option<(&str, u16)>,
    ) -> Result<SystemProxyDisableOutcome> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "restore_macos")?;
        let mut os = self.macos_backend(privilege)?;
        self.restore_macos_locked(&mut os, expected)
    }

    pub fn restore(&mut self) -> Result<()> {
        self.restore_macos(Privilege::Direct, None).map(|_| ())
    }
    pub fn restore_with_privilege(&mut self) -> Result<()> {
        self.restore_macos(Privilege::Sudo, None).map(|_| ())
    }
    pub fn restore_with_gui_auth(&mut self) -> Result<()> {
        self.restore_macos(Privilege::Gui, None).map(|_| ())
    }
    pub fn force_disable(&mut self) -> Result<()> {
        self.restore()
    }
    pub fn disable_with_privilege(&mut self) -> Result<()> {
        self.restore_with_privilege()
    }
    pub fn disable_with_gui_auth(&mut self) -> Result<()> {
        self.restore_with_gui_auth()
    }

    pub(super) fn disable_if_matches_inner(
        &mut self,
        host: &str,
        port: u16,
        _explicit: bool,
    ) -> Result<SystemProxyDisableOutcome> {
        self.restore_macos(Privilege::Direct, Some((host, port)))
    }
    pub fn disable_if_matches_with_privilege(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.restore_macos(Privilege::Sudo, Some((host, port)))
    }
    pub fn disable_if_matches_explicit_with_privilege(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_with_privilege(host, port)
    }
    pub fn disable_if_matches_with_gui_auth(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.restore_macos(Privilege::Gui, Some((host, port)))
    }
    pub fn disable_if_matches_explicit_with_gui_auth(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<SystemProxyDisableOutcome> {
        self.disable_if_matches_with_gui_auth(host, port)
    }
    pub fn disable_managed_with_privilege(&mut self) -> Result<SystemProxyDisableOutcome> {
        self.restore_macos(Privilege::Sudo, None)
    }
    pub fn disable_managed_explicit_with_privilege(&mut self) -> Result<SystemProxyDisableOutcome> {
        self.disable_managed_with_privilege()
    }

    pub fn suspend_managed_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.suspend_managed_if_generation_guarded(generation, || Ok(true))
    }

    /// Fence a retained fail-open lease against replacement runtimes while
    /// holding the same cross-process lock as every proxy mutation.
    pub fn suspend_managed_if_generation_guarded(
        &mut self,
        generation: &str,
        should_suspend: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "suspend_owned_guarded")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation || !should_suspend()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let mut os = self.macos_backend(Privilege::Direct)?;
        if state.phase() == ManagedSystemProxyPhase::Suspended {
            return Ok(if observed_owned(&state, &mut os)? {
                GuardedSystemProxyTransition::AlreadyInState
            } else {
                GuardedSystemProxyTransition::OwnershipChanged
            });
        }
        self.prepare_legacy_journal(&mut state, &mut os)?;
        let outcome = self.run_owned_transition(&mut state, &mut os, Intent::Suspend)?;
        self.detach_in_place();
        self.record_system_proxy_action("system_proxy_fail_open_suspended", "suspend");
        Ok(if outcome.ownership_changed && !outcome.changed {
            GuardedSystemProxyTransition::OwnershipChanged
        } else {
            GuardedSystemProxyTransition::Applied
        })
    }

    pub fn resume_managed_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.resume_macos(generation, Privilege::Direct)
    }

    fn resume_macos(
        &mut self,
        generation: &str,
        privilege: Privilege,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "resume_owned")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let mut os = self.macos_backend(privilege)?;
        self.prepare_legacy_journal(&mut state, &mut os)?;
        if state.phase() == ManagedSystemProxyPhase::Applied {
            if !observed_owned(&state, &mut os)? {
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            self.attach_managed_state(&state);
            return Ok(GuardedSystemProxyTransition::AlreadyInState);
        }
        if !matches!(
            state.phase(),
            ManagedSystemProxyPhase::Suspending
                | ManagedSystemProxyPhase::Suspended
                | ManagedSystemProxyPhase::Resuming
        ) {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let outcome = self.run_owned_transition(&mut state, &mut os, Intent::Resume)?;
        if !macos_owned::has_active_owned_protocol(&state) {
            self.detach_in_place();
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.attach_managed_state(&state);
        self.record_system_proxy_action("system_proxy_generation_resumed", "resume");
        Ok(if outcome.ownership_changed && !outcome.changed {
            GuardedSystemProxyTransition::OwnershipChanged
        } else {
            GuardedSystemProxyTransition::Applied
        })
    }

    /// Complete an interrupted acquisition/retarget without creating a new
    /// baseline or generation. Legacy pending records cannot prove ownership.
    pub fn reconcile_managed_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.reconcile_macos(generation, Privilege::Direct)
    }

    pub fn reconcile_managed_if_generation_with_gui_auth(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.reconcile_macos(generation, Privilege::Gui)
    }

    pub fn reconcile_managed_if_generation_with_privilege(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.reconcile_macos(generation, Privilege::Sudo)
    }

    fn reconcile_macos(
        &mut self,
        generation: &str,
        privilege: Privilege,
    ) -> Result<GuardedSystemProxyTransition> {
        let lock = acquire_system_proxy_file_lock(&self.data_dir, "reconcile_owned")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if !matches!(
            state.phase(),
            ManagedSystemProxyPhase::Applied | ManagedSystemProxyPhase::PendingApply
        ) {
            drop(lock);
            return self.resume_macos(generation, privilege);
        }
        let mut os = self.macos_backend(privilege)?;
        if state.phase() == ManagedSystemProxyPhase::Applied {
            let legacy = state.macos_services.is_empty();
            if legacy {
                // A failed legacy adoption must not persist a disabled matching
                // endpoint as evidence for a later automatic Apply.
                state.macos_services = legacy_journal(&mut os, &state.target)?;
                state.schema_version = 3;
            }
            if !macos_owned::has_active_owned_protocol(&state) {
                self.detach_in_place();
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            if observed_owned(&state, &mut os)? {
                if legacy {
                    self.write_managed_state(&state)?;
                }
                self.attach_managed_state(&state);
                return Ok(GuardedSystemProxyTransition::AlreadyInState);
            }
            if legacy
                || state
                    .macos_services
                    .iter()
                    .flat_map(|service| &service.fields)
                    .any(|field| {
                        !field.relinquished
                            && matches!(&field.last_written, Value::Protocol(proxy)
                    if !proxy.enabled || proxy.port != state.target.port
                        || !proxy_hosts_match(&proxy.host, &state.target.host))
                    })
            {
                // Older versions may already have persisted an unproven legacy
                // migration. A disabled recorded endpoint is never Apply authority.
                return Ok(GuardedSystemProxyTransition::OwnershipChanged);
            }
            // Same-generation manual edits relinquish only the changed fields.
            // The shared engine compares again before every possible write.
        } else if state.macos_services.is_empty() || state.phase.is_none() {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.run_owned_transition(&mut state, &mut os, Intent::Apply)?;
        if !macos_owned::has_active_owned_protocol(&state) {
            self.detach_in_place();
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.attach_managed_state(&state);
        // Relinquishing an edited field is a completed journal transition even
        // if the remaining owned protocols required no OS write.
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn retarget_managed_if_generation(
        &mut self,
        generation: &str,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "retarget_owned")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let mut os = self.macos_backend(Privilege::Direct)?;
        self.prepare_legacy_journal(&mut state, &mut os)?;
        if !matches!(
            state.phase(),
            ManagedSystemProxyPhase::Applied | ManagedSystemProxyPhase::Suspended
        ) || !observed_owned(&state, &mut os)?
        {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let suspended = state.phase() == ManagedSystemProxyPhase::Suspended;
        state.target = ProxyBackup {
            enable: true,
            host: host.into(),
            port,
            bypass: bypass.unwrap_or(&state.target.bypass).into(),
        };
        state.generation = uuid::Uuid::now_v7().to_string();
        if !suspended {
            state.set_phase(ManagedSystemProxyPhase::PendingApply);
        }
        self.write_managed_state(&state)?;
        if !suspended {
            self.run_owned_transition(&mut state, &mut os, Intent::Apply)?;
            self.attach_managed_state(&state);
        }
        self.record_system_proxy_action("system_proxy_generation_retargeted", "retarget");
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn suppress_managed_authorization_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.suppress_managed_authorization_if_generation_guarded(generation, || Ok(true))
    }

    pub fn suppress_managed_authorization_if_generation_guarded(
        &mut self,
        generation: &str,
        should_suppress: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "suppress_authorization")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation || !should_suppress()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if state.authorization_suppressed {
            return Ok(GuardedSystemProxyTransition::AlreadyInState);
        }
        state.authorization_suppressed = true;
        self.write_managed_state(&state)?;
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn restore_managed_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_managed_if_generation_guarded(generation, || Ok(true))
    }

    /// The caller can fence cleanup against a replacement runtime or a changed
    /// shutdown policy without teaching core about the CLI's runtime format.
    pub fn restore_managed_if_generation_guarded(
        &mut self,
        generation: &str,
        should_restore: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_macos_generation_guarded(generation, Privilege::Direct, false, should_restore)
    }

    pub fn restore_managed_if_generation_with_privilege_guarded(
        &mut self,
        generation: &str,
        should_restore: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_macos_generation_guarded(generation, Privilege::Sudo, false, should_restore)
    }

    /// Only a fresh accepted user disable request may reset cancellation. Keep
    /// the generation stable so its direct -> approved privilege retry is fenced.
    pub fn disable_managed_explicit_if_generation_guarded(
        &mut self,
        generation: &str,
        should_disable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_macos_generation_guarded(generation, Privilege::Direct, true, should_disable)
    }

    pub fn disable_managed_explicit_if_generation_with_gui_auth_guarded(
        &mut self,
        generation: &str,
        should_disable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_macos_generation_guarded(generation, Privilege::Gui, true, should_disable)
    }

    pub fn disable_managed_explicit_if_generation_with_privilege_guarded(
        &mut self,
        generation: &str,
        should_disable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_macos_generation_guarded(generation, Privilege::Sudo, true, should_disable)
    }

    fn restore_macos_generation_guarded(
        &mut self,
        generation: &str,
        privilege: Privilege,
        explicit_disable: bool,
        should_restore: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        let _lock = acquire_system_proxy_file_lock(&self.data_dir, "restore_generation_guarded")?;
        let Some(mut state) = self.optional_managed_state()? else {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        };
        if generation.is_empty() || state.generation != generation || !should_restore()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if explicit_disable {
            macos_owned::begin_explicit_disable(&mut state);
            self.write_managed_state(&state)?;
        }
        let mut os = self.macos_backend(privilege)?;
        let outcome = self.restore_macos_locked(&mut os, None)?;
        Ok(match outcome {
            SystemProxyDisableOutcome::Disabled => GuardedSystemProxyTransition::Applied,
            SystemProxyDisableOutcome::NotEnabled => GuardedSystemProxyTransition::AlreadyInState,
            SystemProxyDisableOutcome::OwnedByOther => {
                GuardedSystemProxyTransition::OwnershipChanged
            }
        })
    }

    pub fn recover_from_crash(data_dir: &Path) -> Result<()> {
        let mut manager = Self::new(data_dir.to_path_buf());
        manager.restore_macos(Privilege::Direct, None).map(|_| ())
    }
}

pub(super) fn macos_any_service_proxy_matches(host: &str, port: u16) -> Result<bool> {
    macos_services_match_with_backend(&mut backend(Privilege::Direct), host, port)
}

pub(super) fn macos_services_match_with_backend(
    os: &mut impl Backend,
    host: &str,
    port: u16,
) -> Result<bool> {
    for service in os.services()? {
        for field in [Field::Http, Field::Https] {
            if let Value::Protocol(proxy) = os.read(&service.name, field)? {
                if proxy.enabled && proxy.port == port && proxy_hosts_match(&proxy.host, host) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
pub(super) fn macos_any_service_proxy_enabled(os: &mut impl Backend) -> Result<bool> {
    for service in os.services()? {
        for field in [Field::Http, Field::Https] {
            if matches!(os.read(&service.name, field)?, Value::Protocol(proxy) if proxy.enabled) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn macos_stored_proxy_endpoint() -> Option<(String, u16)> {
    let mut os = backend(Privilege::Direct);
    for service in os.services().ok()? {
        for field in [Field::Http, Field::Https] {
            if let Value::Protocol(proxy) = os.read(&service.name, field).ok()? {
                if !proxy.host.is_empty() && proxy.port != 0 {
                    return Some((proxy.host, proxy.port));
                }
            }
        }
    }
    None
}
