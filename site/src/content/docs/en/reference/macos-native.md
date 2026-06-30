---
title: "macOS Native App"
description: "Automatically synced English macOS Native App documentation from docs-en/macos-native.md."
editLink: false
---

> This page is automatically synced from `docs-en/macos-native.md`.
# macOS Native App

Bifrost Mac Native is a SwiftUI/AppKit client that runs beside the existing Tauri desktop app. It does not replace `desktop/` yet. The native app owns the macOS control experience, while the Rust `bifrost` CLI remains the proxy server, TLS engine, rules engine, script runtime, storage layer, and Admin API provider.

## Architecture

```text
Bifrost Mac Native
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

The script builds the Rust CLI sidecar, copies it to `apps/macos/.build/sidecar/bin/bifrost`, then runs SwiftPM build plus `BifrostMacCoreChecks` for `apps/macos`.

If only the Swift scaffold should be checked:

```bash
scripts/build-macos-native.sh --skip-sidecar --test
```

## Development Safety

Native development smoke runs must not alter the developer machine proxy state. When the sidecar is started from the Swift side, the command plan includes:

- `--skip-cert-check`
- `--no-system-proxy`
- `BIFROST_DISABLE_TRAY=1`
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
- `BIFROST_DATA_DIR` pointing at the selected Bifrost data directory

The default data directory stays `~/.bifrost` for compatibility with the current Tauri desktop and CLI behavior.

## MVP Scope

- Sidecar lifecycle boundary and Admin API client
- Overview shell with backend/proxy/cert status placeholders
- Traffic shell using an AppKit `NSTableView` bridge
- Rules shell using an AppKit `NSTextView` bridge
- Build scripts and tests that prove URL construction, request headers, sidecar command arguments, and port selection

Out of scope for this scaffold: packaged `.app`, code signing, notarization, NetworkExtension, replay workbench, script IDE, device wizard, and FFI.
