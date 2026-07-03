# macOS Native App

Bifrost's native macOS app is a SwiftUI/AppKit client that runs beside the existing Tauri desktop app. It does not replace `desktop/` yet. The native app owns the macOS control experience, while the Rust `bifrost` CLI remains the proxy server, TLS engine, rules engine, script runtime, storage layer, and Admin API provider.

## Architecture

```text
Bifrost native macOS app
SwiftUI / AppKit / Swift Concurrency
        |
BifrostClient
HTTP Admin API + future push channel
        |
BifrostSidecarManager
bundled bifrost CLI process
        |
Existing Rust daemon
Tokio + proxy + TLS + storage + scripts + admin
```

The first native preview uses HTTP Admin API endpoints under `/_bifrost/api/`. It deliberately avoids Rust FFI, NetworkExtension, and direct proxy data-plane code.

## Build

```bash
scripts/build-macos-native.sh --test
```

The script builds the Rust CLI sidecar, copies it to `apps/macos/.build/sidecar/bin/bifrost`, then runs SwiftPM build plus `BifrostNativeCoreChecks` for `apps/macos`.

The user-facing executable product is `Bifrost`:

```bash
swift run --package-path apps/macos Bifrost --check-icon
```

The icon check loads the bundled Bifrost app icon and verifies that the native app can set a non-empty Dock/App Switcher icon.

For the actual desktop preview, open the generated `.app` bundle:

```bash
scripts/build-macos-native.sh --skip-sidecar --test
open -n apps/macos/.build/Bifrost.app
```

Do not open the bare SwiftPM executable under `.build/.../debug/Bifrost`; macOS treats that path as a terminal executable instead of a desktop app bundle.

If only the Swift scaffold should be checked:

```bash
scripts/build-macos-native.sh --skip-sidecar --test
```

## Install

The native app depends on an installed Bifrost CLI. The CLI owns installation,
sidecar updates, and release asset discovery:

```bash
bifrost native-app status
bifrost native-app install -y --open
```

On macOS, `bifrost start` prompts to install the native app when it is missing
and the terminal is interactive. Set `BIFROST_NATIVE_APP_DISABLE_INSTALL_PROMPT=1`
to suppress the prompt in automation.
Direct `native-app install` runs prompt before writing the target app unless
`-y` is provided; Web UI, Tray, and Admin-triggered installs use `-y`.

The default install path is `/Applications/Bifrost.app`. Tests and controlled
automation can use `--install-dir`, `--source`, `--url`,
`BIFROST_NATIVE_APP_SOURCE`, or `BIFROST_NATIVE_APP_URL`.

Release assets use this naming contract:

```text
bifrost-native-v<version>-aarch64-apple-darwin.dmg
bifrost-native-v<version>-x86_64-apple-darwin.dmg
```

## Update

The native app periodically asks the running Bifrost Admin API for release
metadata. When a newer version is available, it asks the user to install it,
delegates installation to `bifrost native-app install`, and then prompts the
user to restart the app so the new bundle is loaded.

## Development Safety

Native development smoke runs must not alter the developer machine proxy state. When the sidecar is started from the Swift side, the command plan includes:

- `--skip-cert-check`
- `--no-system-proxy`
- `BIFROST_DISABLE_TRAY=1`
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
- `BIFROST_DATA_DIR` pointing at the selected Bifrost data directory

The default data directory stays `~/.bifrost` for compatibility with the current Tauri desktop, CLI behavior, and Web UI. On launch, the native app first runs `bifrost status --format json` through the bundled sidecar CLI. If an existing CLI-started daemon is already running, the native app consumes that daemon and does not start a second service. If no daemon is running, it starts `bifrost start --daemon` against the same default data directory.

## MVP Scope

- Sidecar lifecycle boundary and Admin API client
- Overview shell with backend/proxy/cert status placeholders
- Traffic shell using an AppKit `NSTableView` bridge
- Rules shell using an AppKit `NSTextView` bridge
- Build scripts and tests that prove URL construction, request headers, sidecar command arguments, and port selection

Out of scope for this scaffold: packaged `.app`, code signing, notarization, NetworkExtension, replay workbench, script IDE, device wizard, and FFI.
