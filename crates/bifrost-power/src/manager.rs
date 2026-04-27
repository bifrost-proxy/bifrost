//! [`KeepAwakeManager`] — owns the state machine for keep-awake mode.
//!
//! ## State machine (mode × assertion)
//!
//! | Transition                    | Assertion action               |
//! |-------------------------------|--------------------------------|
//! | `off → auto`                  | none (inactive, wait for call) |
//! | `off → force_on`              | acquire                        |
//! | `auto(inactive) → off`        | none                           |
//! | `auto(inactive) → force_on`   | acquire                        |
//! | `auto(active) → off`          | release                        |
//! | `auto(active) → force_on`     | none (keep active)             |
//! | `force_on → off`              | release                        |
//! | `force_on → auto`             | release (back to auto initial) |
//! | `on_remote_call` (mode=auto, inactive) | acquire               |
//! | `on_remote_call` (mode=auto, active)   | none (idempotent)     |
//! | `activate_once`               | acquire (regardless of mode)   |
//! | `release_once`                | release (regardless of mode)   |
//! | `shutdown` (via Drop)         | release                        |
//!
//! All operations are no-ops on unsupported platforms; mutating calls return
//! [`PowerError::Unsupported`].

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::platform::{acquire_assertion, is_on_battery, AssertionHandle, PlatformSupport};
use crate::{Mode, PowerError, Result, Status};

/// Callback invoked when the persisted mode should be saved.
///
/// The manager itself is storage-agnostic — the concrete writer is injected
/// by the admin layer (which owns the `ConfigManager` and the `[keepawake]`
/// section in `config.toml`).
pub trait ModePersister: Send + Sync + 'static {
    /// Persist the new mode. May be called from any thread.
    ///
    /// Implementors should be resilient to transient I/O failures and log
    /// errors — the manager only records a warning on failure (in-memory
    /// state is still updated).
    fn persist(&self, mode: Mode) -> std::result::Result<(), String>;
}

/// No-op persister used by tests and by callers that don't need persistence.
pub struct NoopPersister;

impl ModePersister for NoopPersister {
    fn persist(&self, _mode: Mode) -> std::result::Result<(), String> {
        Ok(())
    }
}

pub type SharedKeepAwakeManager = Arc<KeepAwakeManager>;

/// The actual manager. Cheap to construct and cheap to `on_remote_call`
/// (one mutex + one atomic branch).
pub struct KeepAwakeManager {
    inner: Mutex<Inner>,
    persister: Arc<dyn ModePersister>,
    platform: PlatformSupport,
}

struct Inner {
    mode: Mode,
    assertion: Option<AssertionHandle>,
    activated_at: Option<Instant>,
}

impl KeepAwakeManager {
    /// Construct a manager with the given initial mode and persister.
    ///
    /// If the platform is supported and the initial mode is `ForceOn`,
    /// the assertion is eagerly acquired. If the mode is `Auto` or `Off`
    /// the manager starts inactive.
    pub fn new(initial_mode: Mode, persister: Arc<dyn ModePersister>) -> SharedKeepAwakeManager {
        let platform = PlatformSupport::current();
        let mgr = Arc::new(Self {
            inner: Mutex::new(Inner {
                mode: initial_mode,
                assertion: None,
                activated_at: None,
            }),
            persister,
            platform,
        });

        // Eager ForceOn acquire (best-effort — failures are logged, not
        // propagated, because the manager must be usable even if the OS
        // refuses for some transient reason).
        if platform.is_supported() && initial_mode == Mode::ForceOn {
            if let Err(e) = mgr.acquire_locked() {
                warn!(error = %e, "keepawake: initial ForceOn acquire failed");
            }
        }
        mgr
    }

    /// Public accessor for test/debug. Production callers should use
    /// [`Self::status`] instead.
    pub fn platform(&self) -> PlatformSupport {
        self.platform
    }

    /// Snapshot of the current state.
    pub fn status(&self) -> Status {
        let inner = self.inner.lock();
        let on_battery = is_on_battery();
        Status {
            supported: self.platform.is_supported(),
            platform: self.platform.label(),
            mode: inner.mode,
            active: inner.assertion.is_some(),
            active_since_secs: inner
                .activated_at
                .map(|t| Instant::now().saturating_duration_since(t).as_secs()),
            on_battery,
            battery_warning: if self.platform.is_supported() && on_battery {
                Some(
                    "Lid-close sleep cannot be fully prevented while on battery. \
                     Plug in for full protection."
                        .to_string(),
                )
            } else {
                None
            },
        }
    }

    /// One-shot activate. Fails on unsupported platforms or if the OS
    /// refuses the assertion. Does **not** change the persisted mode.
    pub fn activate_once(&self) -> Result<()> {
        if !self.platform.is_supported() {
            return Err(PowerError::Unsupported(self.platform.label()));
        }
        let mut inner = self.inner.lock();
        if inner.assertion.is_some() {
            debug!("keepawake: activate_once while already active — no-op");
            return Ok(());
        }
        inner.assertion = Some(acquire_assertion()?);
        inner.activated_at = Some(Instant::now());
        info!(mode = %inner.mode, "keepawake: assertion acquired (activate_once)");
        Ok(())
    }

    /// One-shot release. Always succeeds (even on unsupported platforms —
    /// there's nothing to release). Does **not** change the persisted mode.
    pub fn release_once(&self) -> Result<()> {
        let mut inner = self.inner.lock();
        if inner.assertion.take().is_some() {
            inner.activated_at = None;
            info!(mode = %inner.mode, "keepawake: assertion released (release_once)");
        } else {
            debug!("keepawake: release_once while already inactive — no-op");
        }
        Ok(())
    }

    /// Auto-mode hook. Called at the entry of every remote-invoke command
    /// dispatch. Idempotent and O(1) in the common case (already active).
    pub fn on_remote_call(&self) {
        if !self.platform.is_supported() {
            return;
        }
        let mut inner = self.inner.lock();
        if inner.mode != Mode::Auto || inner.assertion.is_some() {
            return;
        }
        match acquire_assertion() {
            Ok(h) => {
                inner.assertion = Some(h);
                inner.activated_at = Some(Instant::now());
                info!("keepawake: auto-activated on remote call");
            }
            Err(e) => {
                warn!(error = %e, "keepawake: auto-activate failed");
            }
        }
    }

    /// Switch persisted mode. This is the *only* entry point that writes
    /// to the config file, and it also adjusts the assertion state per
    /// the transition table in the module docs.
    pub fn set_mode(&self, new_mode: Mode) -> Result<Mode> {
        let old_mode = {
            let mut inner = self.inner.lock();
            let old = inner.mode;
            if old == new_mode {
                // Still persist idempotently so the on-disk representation
                // matches what the user requested. Cheap.
                self.persist_best_effort(new_mode);
                return Ok(old);
            }
            inner.mode = new_mode;
            old
        };

        // Apply transition side-effects. Same-mode transitions are
        // filtered out at the top of set_mode, so we only handle real
        // transitions here. Order matters: the ForceOn arm must come
        // before the ForceOn→Auto release arm, etc.
        match (old_mode, new_mode) {
            // Off → Auto: nothing to do (stay inactive, wait for call).
            (Mode::Off, Mode::Auto) => {}
            // _ → ForceOn: acquire if not already active.
            (_, Mode::ForceOn) => {
                if self.platform.is_supported() && !self.is_active() {
                    if let Err(e) = self.acquire_locked() {
                        warn!(error = %e, "keepawake: acquire on set_mode(force_on) failed");
                    }
                }
            }
            // _ → Off, or ForceOn → Auto: release if active.
            (_, Mode::Off) | (Mode::ForceOn, Mode::Auto) => {
                let _ = self.release_once();
            }
            // Same-mode tuples are filtered by the early-return above;
            // this arm is only here for exhaustiveness.
            _ => {}
        }

        info!(old = %old_mode, new = %new_mode, "keepawake: mode changed");
        self.persist_best_effort(new_mode);
        Ok(old_mode)
    }

    /// Current mode without taking a full status snapshot.
    pub fn mode(&self) -> Mode {
        self.inner.lock().mode
    }

    /// Whether an assertion is currently held.
    pub fn is_active(&self) -> bool {
        self.inner.lock().assertion.is_some()
    }

    // --- internal helpers ---------------------------------------------------

    fn acquire_locked(&self) -> Result<()> {
        if !self.platform.is_supported() {
            return Err(PowerError::Unsupported(self.platform.label()));
        }
        let mut inner = self.inner.lock();
        if inner.assertion.is_some() {
            return Ok(());
        }
        inner.assertion = Some(acquire_assertion()?);
        inner.activated_at = Some(Instant::now());
        Ok(())
    }

    fn persist_best_effort(&self, mode: Mode) {
        if let Err(e) = self.persister.persist(mode) {
            warn!(error = %e, "keepawake: failed to persist mode (in-memory state is still updated)");
        }
    }
}

impl Drop for KeepAwakeManager {
    fn drop(&mut self) {
        // Explicitly release before the process tears down. `AssertionHandle`
        // would do the same in its own Drop, but clearing here makes the
        // lifecycle trace obvious.
        let mut inner = self.inner.lock();
        if inner.assertion.take().is_some() {
            info!("keepawake: manager dropped — assertion released");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PlMutex;

    #[derive(Default)]
    struct RecordingPersister {
        history: PlMutex<Vec<Mode>>,
    }
    impl ModePersister for RecordingPersister {
        fn persist(&self, mode: Mode) -> std::result::Result<(), String> {
            self.history.lock().push(mode);
            Ok(())
        }
    }

    fn make_mgr(initial: Mode) -> (SharedKeepAwakeManager, Arc<RecordingPersister>) {
        let p = Arc::new(RecordingPersister::default());
        let mgr = KeepAwakeManager::new(initial, p.clone());
        (mgr, p)
    }

    #[test]
    fn default_mode_off_is_inactive() {
        let (m, _) = make_mgr(Mode::Off);
        assert_eq!(m.mode(), Mode::Off);
        assert!(!m.is_active());
    }

    #[test]
    fn status_snapshot_contains_platform_label() {
        let (m, _) = make_mgr(Mode::Off);
        let s = m.status();
        assert_eq!(s.mode, Mode::Off);
        assert_eq!(s.platform, PlatformSupport::current().label());
        assert_eq!(s.supported, PlatformSupport::current().is_supported());
        assert!(!s.active);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn force_on_acquires_eagerly_on_macos() {
        let (m, _) = make_mgr(Mode::ForceOn);
        assert!(m.is_active(), "force_on should acquire eagerly");
        drop(m);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn unsupported_platform_never_activates() {
        let (m, _) = make_mgr(Mode::ForceOn);
        assert!(!m.is_active());
        assert!(matches!(
            m.activate_once().unwrap_err(),
            PowerError::Unsupported(_)
        ));
        // release_once is infallible even on unsupported platforms.
        m.release_once().unwrap();
    }

    #[test]
    fn on_remote_call_respects_mode() {
        let (m, _) = make_mgr(Mode::Off);
        m.on_remote_call();
        assert!(!m.is_active(), "off mode should not activate on calls");

        let (m2, _) = make_mgr(Mode::Auto);
        m2.on_remote_call();
        // On supported platforms this should activate; on others it's a no-op.
        if PlatformSupport::current().is_supported() {
            assert!(m2.is_active(), "auto mode should activate on first call");
        } else {
            assert!(!m2.is_active());
        }
        // Second call is idempotent.
        let was_active_before = m2.is_active();
        m2.on_remote_call();
        assert_eq!(m2.is_active(), was_active_before);
    }

    #[test]
    fn set_mode_persists_and_toggles_assertion() {
        let (m, p) = make_mgr(Mode::Off);
        m.set_mode(Mode::Auto).unwrap();
        assert_eq!(m.mode(), Mode::Auto);
        assert!(!m.is_active(), "auto should not immediately acquire");

        if PlatformSupport::current().is_supported() {
            m.set_mode(Mode::ForceOn).unwrap();
            assert!(m.is_active(), "force_on should acquire");
            m.set_mode(Mode::Auto).unwrap();
            assert!(!m.is_active(), "force_on -> auto should release per spec");
            m.set_mode(Mode::Off).unwrap();
            assert!(!m.is_active());
        }

        let history = p.history.lock().clone();
        assert!(
            history.len() >= 2,
            "persister should record mode changes: {history:?}"
        );
        assert_eq!(history[0], Mode::Auto);
    }

    #[test]
    fn same_mode_set_is_idempotent_but_still_persists() {
        let (m, p) = make_mgr(Mode::Auto);
        let prev = m.set_mode(Mode::Auto).unwrap();
        assert_eq!(prev, Mode::Auto);
        assert!(p.history.lock().contains(&Mode::Auto));
    }

    #[test]
    fn release_once_is_idempotent() {
        let (m, _) = make_mgr(Mode::Off);
        m.release_once().unwrap();
        m.release_once().unwrap();
        assert!(!m.is_active());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn manual_override_on_then_set_off_releases() {
        let (m, _) = make_mgr(Mode::Auto);
        m.activate_once().unwrap();
        assert!(m.is_active());
        m.set_mode(Mode::Off).unwrap();
        assert!(!m.is_active());
    }

    // Exhaustive transition matrix — the whole reason this type exists.
    #[test]
    #[cfg(target_os = "macos")]
    fn transition_matrix_macos() {
        use Mode::*;
        let transitions: &[(
            Mode,
            Mode,
            bool, /*active_before*/
            bool, /*active_after*/
        )] = &[
            // from, to, active_before, expected_active_after
            (Off, Auto, false, false),
            (Off, ForceOn, false, true),
            (Auto, Off, false, false),
            (Auto, ForceOn, false, true),
            (Auto, Off, true, false),
            (Auto, ForceOn, true, true),
            (ForceOn, Off, true, false),
            (ForceOn, Auto, true, false),
        ];
        for &(from, to, active_before, expected_after) in transitions {
            let (m, _) = make_mgr(from);
            if active_before && !m.is_active() {
                m.activate_once().unwrap();
            }
            if !active_before && m.is_active() {
                m.release_once().unwrap();
            }
            assert_eq!(
                m.is_active(),
                active_before,
                "preconditions: {from:?}->{to:?} active_before={active_before}"
            );
            m.set_mode(to).unwrap();
            assert_eq!(
                m.is_active(),
                expected_after,
                "{from:?} -> {to:?} active_before={active_before}"
            );
            assert_eq!(m.mode(), to);
        }
    }
}
