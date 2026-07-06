# Desktop Open Requests

## Goal

Tray actions and OS entry points should prefer the installed Bifrost desktop app without losing the existing Web UI fallback.

Required behavior:

- `Open Traffic`, `Open Rules`, and `Open Settings` in the tray first open `bifrost://open/<route>`.
- If the desktop app is not installed or the OS cannot resolve the protocol, tray falls back to the current Web UI URL.
- The desktop app is strictly single-instance. A second launch forwards deep links and `.bifrost` files to the already running instance instead of starting another UI/backend.
- The desktop bundle registers the `bifrost://` protocol and `.bifrost` file association.
- `.bifrost` files opened through the OS should import through the same parser as drag-and-drop. Network/capture files route to Traffic; rules files route to Rules.
- `.bifrost` imports must preview before writing: rules show rule name, status, details, and content; multi-request network packages show request count, domains, and representative requests; single-request network packages reuse the Network detail view for review. Import only starts after confirmation.

## Contract

Tray route links:

| Tray item | App link | Web fallback |
| --- | --- | --- |
| Open Traffic | `bifrost://open/traffic` | `<admin>/_bifrost/traffic` |
| Open Rules | `bifrost://open/rules` | `<admin>/_bifrost/rules` |
| Open Settings | `bifrost://open/settings` | `<admin>/_bifrost/settings` |

Desktop accepted open requests:

- `bifrost://open/traffic`
- `bifrost://open/rules`
- `bifrost://open/settings`
- `bifrost://traffic`, `bifrost://rules`, `bifrost://settings` for short-form compatibility
- `file:///.../*.bifrost`

Unknown protocol routes are ignored and logged. Non-`.bifrost` file arguments are ignored. `.bifrost` files are capped at 50 MiB before being read into the WebView import bridge.

## Implementation Notes

- `tauri-plugin-single-instance` is installed before runtime setup so the OS can forward second-instance arguments into the primary process.
- `tauri-plugin-deep-link` handles `bifrost://` events. On Windows/Linux the runtime registers schemes dynamically; on macOS the bundle metadata is the source of truth.
- Desktop open events are queued in `BackendState.pending_open_requests` before emitting `desktop://open-request`. This avoids losing a request when the WebView has not mounted its listener yet.
- Web consumes pending requests through `get_pending_desktop_open_requests` and listens for `desktop://open-request`.
- `.bifrost` import reuses `previewFile` and `importFile` from the shared Web import helpers, so drag-and-drop, file picker, and OS file-open share preview, confirmation, store refresh, toast behavior, and target route mapping.

## Validation Plan

- Rust unit:
  - `cargo test -p bifrost-cli tray::` verifies tray menu actions and app link formatting.
  - `cargo test --manifest-path desktop/src-tauri/Cargo.toml open_requests` verifies deep-link parsing, route allowlist, and `.bifrost` file URL parsing.
- Web:
  - `pnpm --dir web test:unit src/api/bifrost-file.test.ts` verifies `.bifrost` preview/import/export API calls use the CSRF-aware client.
  - `pnpm --dir web run build:desktop` verifies the Tauri WebView bridge compiles.
- E2E contract:
  - `bash e2e-tests/tests/test_desktop_open_requests_contract.sh` checks the config/dependency contract and runs the focused Rust guards.
- Human:
  - `human_tests/desktop-open-requests.md` covers installed-app routing, fallback, `.bifrost` import, and strict single-instance behavior.
