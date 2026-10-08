//! Generation-fenced retarget for the aggregate Windows/test backend.
use super::*;

impl SystemProxyManager {
    pub fn enable_guarded(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
        should_enable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        if !should_enable()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.enable(host, port, bypass)?;
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn enable_if_unmanaged(
        &mut self,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        match self.load_managed_state() {
            Ok(_) => return Ok(GuardedSystemProxyTransition::OwnershipChanged),
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        #[cfg(test)]
        if self.skip_os_proxy_io {
            return Err(BifrostError::Config(
                "Native aggregate proxy I/O is disabled for mock managers".into(),
            ));
        }
        if Self::get_current()?.enable {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        self.enable(host, port, bypass)?;
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
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
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

    pub fn restore_managed_if_generation_guarded(
        &mut self,
        generation: &str,
        should_restore: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_aggregate_generation_guarded(generation, false, should_restore)
    }

    pub fn disable_managed_explicit_if_generation_guarded(
        &mut self,
        generation: &str,
        should_disable: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        self.restore_aggregate_generation_guarded(generation, true, should_disable)
    }

    fn restore_aggregate_generation_guarded(
        &mut self,
        generation: &str,
        explicit_disable: bool,
        should_restore: impl FnOnce() -> Result<bool>,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
        };
        if generation.is_empty() || state.generation != generation || !should_restore()? {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        #[cfg(test)]
        let was_suspended = state.phase() == ManagedSystemProxyPhase::Suspended;
        if explicit_disable {
            macos_owned::begin_explicit_disable(&mut state);
            self.write_managed_state(&state)?;
        }
        #[cfg(test)]
        let owned = if self.skip_os_proxy_io {
            !was_suspended
        } else {
            self.current_matches_backup(&state.target)?
        };
        #[cfg(not(test))]
        let owned = self.current_matches_backup(&state.target)?;
        #[cfg(test)]
        let already_restored = !owned
            && if self.skip_os_proxy_io {
                was_suspended
            } else {
                self.current_matches_backup(&state.original)?
            };
        #[cfg(not(test))]
        let already_restored = !owned && self.current_matches_backup(&state.original)?;
        if owned {
            #[cfg(test)]
            if !self.skip_os_proxy_io {
                self.apply_proxy_backup(&state.original)?;
            }
            #[cfg(not(test))]
            self.apply_proxy_backup(&state.original)?;
        }
        self.remove_state_files();
        self.detach_in_place();
        Ok(if owned {
            GuardedSystemProxyTransition::Applied
        } else if already_restored {
            GuardedSystemProxyTransition::AlreadyInState
        } else {
            GuardedSystemProxyTransition::OwnershipChanged
        })
    }

    pub fn reconcile_managed_if_generation(
        &mut self,
        generation: &str,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
        };
        if generation.is_empty() || state.generation != generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        if state.phase() != ManagedSystemProxyPhase::PendingApply {
            return self.resume_managed_if_generation(generation);
        }
        if state.phase.is_none() {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        #[cfg(test)]
        let matches = self.skip_os_proxy_io
            || self.current_matches_backup(&state.target)?
            || self.current_matches_backup(&state.original)?;
        #[cfg(not(test))]
        let matches = self.current_matches_backup(&state.target)?
            || self.current_matches_backup(&state.original)?;
        if !matches {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        #[cfg(test)]
        if !self.skip_os_proxy_io {
            self.apply_proxy_backup(&state.target)?;
        }
        #[cfg(not(test))]
        self.apply_proxy_backup(&state.target)?;
        state.set_phase(ManagedSystemProxyPhase::Applied);
        self.write_managed_state(&state)?;
        self.attach_managed_state(&state);
        Ok(GuardedSystemProxyTransition::Applied)
    }

    pub fn retarget_managed_if_generation(
        &mut self,
        generation: &str,
        host: &str,
        port: u16,
        bypass: Option<&str>,
    ) -> Result<GuardedSystemProxyTransition> {
        if !self.management_available() {
            return Ok(GuardedSystemProxyTransition::NotManaged);
        }
        let mut state = match self.load_managed_state() {
            Ok(state) => state,
            Err(BifrostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GuardedSystemProxyTransition::NotManaged)
            }
            Err(error) => return Err(error),
        };
        if generation.is_empty() || state.generation != generation {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let suspended = state.phase() == ManagedSystemProxyPhase::Suspended;
        if !suspended && state.phase() != ManagedSystemProxyPhase::Applied {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
        let expected = if suspended {
            &state.original
        } else {
            &state.target
        };
        #[cfg(test)]
        let matches = self.skip_os_proxy_io || self.current_matches_backup(expected)?;
        #[cfg(not(test))]
        let matches = self.current_matches_backup(expected)?;
        if !matches {
            return Ok(GuardedSystemProxyTransition::OwnershipChanged);
        }
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
            #[cfg(test)]
            if !self.skip_os_proxy_io {
                self.apply_proxy_backup(&state.target)?;
            }
            #[cfg(not(test))]
            self.apply_proxy_backup(&state.target)?;
            state.set_phase(ManagedSystemProxyPhase::Applied);
            self.write_managed_state(&state)?;
            self.attach_managed_state(&state);
        }
        self.record_system_proxy_action("system_proxy_generation_retargeted", "retarget");
        Ok(GuardedSystemProxyTransition::Applied)
    }
}

#[cfg(test)]
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod tests;
