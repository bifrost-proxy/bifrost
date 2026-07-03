---
title: "Desktop Installation and Build"
description: "Desktop app installation, local build steps, and notes."
editLink: false
---

> This page is automatically synced from `docs-en/desktop.md`.
> Language: **English** | [中文](../../getting-started/desktop)

# Desktop Installation and Build

The desktop app is built with Tauri. The package includes Web assets and starts the bundled `bifrost` CLI backend inside the app.

## Installation

### Homebrew Cask on macOS

```bash
brew tap bifrost-proxy/bifrost
brew install --cask bifrost-desktop
```

### Manual Installer

Download desktop installers from [GitHub Releases](https://github.com/bifrost-proxy/bifrost/releases). Planned artifacts include macOS `.dmg` packages for Intel and Apple Silicon, and Windows `.msi` installers for x64 and ARM64.

After installation, launch `Bifrost.app` on macOS or `Bifrost` from the Windows Start menu. The desktop app checks and installs the CA certificate asynchronously on first startup. The default data directory is `~/.bifrost`; set `BIFROST_DATA_DIR` before startup to override config, cert, log, and runtime paths.

## Build from Source

```bash
./install.sh
./install.sh --cli-only
./install.sh --desktop-only
./install.sh --app-dir ~/Applications
```

Manual build:

```bash
git clone https://github.com/bifrost-proxy/bifrost.git
cd bifrost
pnpm install
cd web && pnpm install && cd ..
pnpm run desktop:build
```

Build outputs are under `desktop/src-tauri/target/release/bundle/`.
