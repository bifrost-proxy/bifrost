use super::*;

impl UpgradeBehavior {
    pub(super) fn for_runtime_owner(mut self, runtime: Option<&RuntimeInfo>) -> Self {
        // Freeze ownership before installers or Desktop shutdown can replace the
        // shared runtime marker. A Desktop-owned core has exactly one restarter.
        self.restart_proxy &= cli_owns_runtime_restart(runtime);
        self.expected_cli_port = self.expected_cli_port.or_else(|| {
            runtime
                .filter(|runtime| cli_owns_runtime_restart(Some(runtime)))
                .map(|runtime| runtime.port)
        });
        self
    }

    pub(super) fn background() -> Self {
        Self {
            restart_if_already_latest: true,
            update_desktop_app: true,
            require_desktop_app_update: true,
            restart_proxy: true,
            expected_cli_port: None,
        }
    }

    pub(super) fn interactive(skip_app: bool, skip_restart: bool) -> Self {
        Self {
            restart_if_already_latest: false,
            update_desktop_app: !skip_app,
            require_desktop_app_update: false,
            restart_proxy: !skip_restart,
            expected_cli_port: None,
        }
    }
}

pub(super) fn interactive_upgrade_behavior(skip_app: bool, skip_restart: bool) -> UpgradeBehavior {
    let mut behavior = UpgradeBehavior::interactive(skip_app, skip_restart);
    behavior.require_desktop_app_update = env_flag("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL");
    env::remove_var("BIFROST_WINDOWS_REQUIRE_DESKTOP_INTERNAL");
    behavior.expected_cli_port = env::var("BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0);
    env::remove_var("BIFROST_WINDOWS_EXPECTED_CLI_PORT_INTERNAL");
    behavior
}

pub(super) fn app_managed_upgrade_behavior() -> UpgradeBehavior {
    // `bifrost app upgrade` suppresses the recursive App companion only. It is
    // still the top-level CLI updater, so a CLI-owned running core must restart
    // onto the newly installed executable.
    let mut behavior = UpgradeBehavior::interactive(true, false);
    // The on-disk CLI can already be at the pinned target while a daemon still
    // serves the previous in-memory version. The App entrypoint must converge
    // that stale runtime just like the Admin background updater does.
    behavior.restart_if_already_latest = true;
    behavior
}
