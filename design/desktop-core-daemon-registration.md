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
- Every Admin system overview exposes a SHA-256 fingerprint of the canonical active data directory. Markerless discovery may grant lifecycle ownership only when that fingerprint matches the caller's active data directory; a legacy service without the field remains manageable only through an already-matching runtime marker.
- Desktop may reuse a healthy markerless Bifrost only on the configured preferred port and only when the data-directory fingerprint matches. It does not claim that process, does not fall back to `preferred + 1`, and does not stop the reused process when the shell exits.
- A health response alone never clears Desktop's startup/manual-recovery gate. Recovery also requires a non-empty Core identity and either a matching managed child, a matching runtime marker, or the existing markerless-preferred-port rule. Markerless recovery requires the exact current data-directory fingerprint; marker/child-backed legacy Cores may omit the field, but an explicit mismatch is always rejected. This prevents an unrelated listener on the remembered port from being treated as the Desktop Core without breaking legacy marker compatibility.
- If the preferred port is won by another process between the availability check and sidecar bind, Desktop recognizes the bind error appended by that launch attempt and retries the next candidate even when the competing listener has already disappeared by the post-exit check.
- Runtime marker writes and removals are serialized. Cleanup from an old PID removes only markers that still name that PID, so a late shutdown cannot erase the replacement daemon's markers.
- Tray 对 Desktop-owned runtime 不提供普通的 Service Stop。它显示 `Quit Bifrost` 并走
  Desktop graceful shutdown；如果 Desktop 已异常消失，仅允许 Tray 在 owner mode、PID
  和进程启动时间精确匹配时，以同一 data-dir 完成内部授权 stop。只有 CLI-owned runtime
  继续显示 Start/Stop Service，普通 CLI 仍不能停止 live Desktop-owned runtime。
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
  - `discover_bifrost_runtime` is the strict markerless discovery path and requires a matching data-directory fingerprint.
  - `recover_bifrost_runtime` accepts a healthy matching legacy marker for upgrade compatibility, otherwise uses strict discovery and repairs missing markers.
  - marker mutations use `runtime.lock` and expected-PID cleanup.
- `desktop/src-tauri/src/runtime_ownership.rs`
  - marker-backed reuse still requires marker/PID/health agreement.
  - markerless reuse is restricted to the preferred port, a healthy Admin endpoint, and an exact data-directory fingerprint.
  - manual recovery applies the identity/ownership boundary before making the WebView ready, preserving marker-backed compatibility for legacy identities without a fingerprint.
- `desktop/src-tauri/src/backend_runtime.rs`
  - sidecar bind-race detection reads only stderr appended after the current spawn, so stale historical errors cannot trigger a false port fallback.
- `desktop/src-tauri/src/backend_runtime/watchdog.rs`
  - a healthy Admin probe can start recovery validation, but cannot bypass the identity/ownership gate.

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
- Unit: foreign or fingerprint-less markerless services are never adopted.
- Unit: a health-only or foreign-data-directory listener cannot clear the manual-start gate, while a matching markerless preferred-port Core can.
- Unit: only a bind-conflict line appended by the current sidecar launch authorizes post-exit fallback to the next port.
- Unit: late cleanup for PID A cannot remove replacement markers for PID B.
- E2E: deleting lifecycle markers still lets same-profile start/stop/restart recover the preferred-port service, while another data directory fails closed.
- E2E: Desktop reuses the same-profile markerless preferred-port service without launching a fallback port or stopping that service on shell exit.
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
