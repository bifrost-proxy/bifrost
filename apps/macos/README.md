# Bifrost Native macOS App

This directory contains the macOS native client scaffold. The app is intentionally a control plane: SwiftUI and AppKit provide the interface, while the existing Rust `bifrost` CLI remains the proxy sidecar and data plane.

## Local Build

```bash
scripts/build-macos-native.sh --test
```

The build script prepares a local sidecar copy under `apps/macos/.build/sidecar/bin/bifrost` and then runs SwiftPM build plus `BifrostNativeCoreChecks`. If `xcodegen` is installed, `Project.yml` can be used to generate an Xcode project later, but SwiftPM is the required reproducible path for this scaffold.

The app's user-facing executable product is `Bifrost`:

```bash
swift run --package-path apps/macos Bifrost --check-icon
```

For the real desktop preview, open the generated app bundle:

```bash
scripts/build-macos-native.sh --skip-sidecar --test
open -n apps/macos/.build/Bifrost.app
```

Opening `.build/.../debug/Bifrost` directly launches a bare terminal executable, not the desktop app bundle.

## Scope

- Keep `desktop/` Tauri unchanged for cross-platform desktop builds.
- Use Admin API calls under `/_bifrost/api/` for control and inspection.
- Reuse an existing default-data-dir `bifrost` daemon when the CLI or another desktop surface already started one; only start the bundled sidecar as `--daemon` when no service is running.
- Start the Rust sidecar with explicit `--no-system-proxy` and `BIFROST_DISABLE_TRAY=1` defaults during native development smoke tests.
- Add high-performance AppKit bridge points for the future Traffic table and rule editor.
