# Desktop Core LaunchDaemon Registration Boundary

## Background

Bifrost Desktop starts an embedded `bifrost start` sidecar as the app-bound core. That core is owned by the desktop shell lifecycle: startup, watchdog recovery, port switch, and quit all flow through `desktop/src-tauri/src/main.rs`.

The CLI daemon path has a different ownership model. When CLI system proxy management is enabled, the CLI may install the macOS system-proxy cleanup LaunchDaemon at `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist` so reboot or crash cleanup can be handled outside the CLI process.

Those two ownership models must not share LaunchDaemon registration. If Desktop reuses the CLI start path without an explicit boundary, Desktop can overwrite or upgrade the CLI-registered LaunchDaemon program path with the app-bundled sidecar path. That interferes with the CLI daemon registration even though the Desktop app does not need a system-level cleanup daemon.

The reverse boundary is just as important: CLI start/daemon maintenance must recognize a live Desktop-owned runtime. If CLI decides "I did not start this service" and immediately runs the normal restart/stop path, it can kill the app-bound core under the Desktop app.

## Product Semantics

- Desktop core is app-bound. It starts and stops with Desktop and is monitored by the Desktop watchdog.
- CLI daemon is system/runtime-bound. It may register the system-proxy cleanup LaunchDaemon when system proxy is enabled.
- Desktop may still use system proxy according to user/runtime configuration, but Desktop must not install or upgrade the system-proxy cleanup LaunchDaemon.
- CLI `start` must not stop a live Desktop-owned core. If the requested port matches the Desktop runtime, CLI reuses the existing service. If the requested port differs, CLI fails with a clear message and leaves Desktop running.
- CLI managed-runtime restart helpers must not restart Desktop-owned cores after a crash or cleanup path; Desktop owns that lifecycle.
- Tray 对 Desktop-owned runtime 不提供孤立的 Service Stop。它显示 `Quit Bifrost` 并走
  Desktop graceful shutdown；只有 CLI-owned runtime 继续显示 Start/Stop Service。
- `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1` remains a separate switch. It disables system proxy use for Desktop; it is not the mechanism for preventing LaunchDaemon registration.

## Implementation

Desktop sidecar startup always injects:

```text
BIFROST_DESKTOP_CORE=1
BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1
```

`BIFROST_DESKTOP_CORE=1` makes the sidecar write `runtime_start_mode=desktop` into `runtime.json`. `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` is already respected by the CLI start path in `spawn_system_proxy_launchd_install_task`. Keeping both guards at the Desktop process boundary has two advantages:

- CLI behavior remains unchanged for `bifrost start --daemon`, `bifrost start --system-proxy`, restart, and upgrade flows.
- Desktop and CLI can share the same `bifrost start` binary without Desktop taking ownership of `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist`.
- CLI can distinguish Desktop-owned runtime from CLI-owned daemon before deciding whether a live PID may be stopped.

## Code Entrypoints

- `desktop/src-tauri/src/main.rs`
  - `desktop_backend_start_args(port)` builds the app-bound sidecar `bifrost start` argv.
  - `desktop_backend_env(data_dir)` injects `BIFROST_DATA_DIR`, `BIFROST_DESKTOP_CORE=1`, and `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`.
  - `start_backend(...)` applies both before spawning the sidecar.
- `crates/bifrost-cli/src/commands/start.rs`
  - `foreground_runtime_start_mode()` maps `BIFROST_DESKTOP_CORE=1` to `RuntimeStartMode::Desktop`.
  - `live_desktop_runtime_for_pid(...)` and `handle_live_desktop_runtime_before_start(...)` prevent CLI `start` from stopping Desktop-owned cores.
  - `spawn_system_proxy_launchd_install_task(...)` keeps the CLI registration path and honors `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL`.
- `crates/bifrost-cli/src/process.rs`
  - `RuntimeStartMode::Desktop` is app-bound and is not CLI-restartable.

## User Goal Checklist

### Must Implement

- Desktop app-bound core must not install or upgrade the macOS system-proxy cleanup LaunchDaemon.
- Desktop app-bound core must be identifiable as `runtime_start_mode=desktop`.
- CLI `start` must not stop a live Desktop core, including under `--yes`.
- CLI daemon/system-proxy registration behavior must remain unchanged.
- Desktop system-proxy enablement and Desktop LaunchDaemon registration suppression must remain separate concerns.

### Must Not Break

- Desktop startup, watchdog recovery, and port switching still launch the same sidecar binary.
- `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1` still adds `--no-system-proxy`.
- CLI `start --daemon` and runtime `system-proxy` flows can still register LaunchDaemon when the disable environment variable is absent.

### Must Verify

- Unit: Desktop sidecar env includes `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`.
- Unit: Desktop sidecar env includes `BIFROST_DESKTOP_CORE=1`.
- Unit: CLI maps `BIFROST_DESKTOP_CORE=1` to `RuntimeStartMode::Desktop`.
- Unit: CLI `start` reuses same-port Desktop runtime and rejects mismatched port without stopping Desktop.
- Unit: `RuntimeStartMode::Desktop` is not CLI-restartable.
- Unit: Desktop sidecar args do not add `--no-system-proxy` unless `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1`.
- E2E contract: focused Desktop tests run through an executable shell script.
- Human test: review sidecar logs/source contract and CLI boundary without touching real LaunchDaemon state.

## Test Plan

- `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar -- --nocapture`
- `cargo test -p bifrost-cli desktop_core --lib -- --nocapture`
- `cargo test -p bifrost-cli live_desktop_runtime --lib -- --nocapture`
- `cargo test -p bifrost-cli runtime_info_new_desktop_is_app_bound_not_cli_restartable --lib -- --nocapture`
- `bash e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh`
- `human_tests/desktop-core-daemon-registration.md`

CI note: `test_desktop_sidecar_launchd_env_contract.sh` prepares `web/dist-desktop`,
the debug CLI sidecar, and `desktop/src-tauri/resources/bin/*` before invoking
the desktop crate on macOS or other desktop-capable runners. Linux shell CI may
lack GTK/GObject development packages, so the script skips only the Tauri
desktop crate portion when `pkg-config --exists gobject-2.0` fails and still
runs the CLI ownership focused tests.

## Review/Fix/Test Notes

Round 1 should verify that the guard is applied to every Desktop sidecar spawn path, including initial startup and restart after port rebind fallback.

Round 2 should re-check that no CLI defaults were changed and that docs describe `BIFROST_DESKTOP_NO_SYSTEM_PROXY` as independent from LaunchDaemon suppression.
