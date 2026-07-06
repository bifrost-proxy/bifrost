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

### Pick the Right Installer

Open [GitHub Releases](https://github.com/bifrost-proxy/bifrost/releases), choose the latest release, and expand `Assets`. Desktop installer names include both platform and architecture:

| System | File | Device |
| --- | --- | --- |
| macOS Apple Silicon | `bifrost-desktop-vX.X.X-aarch64-apple-darwin.dmg` | M1/M2/M3/M4 and newer Apple Silicon Macs |
| macOS Intel | `bifrost-desktop-vX.X.X-x86_64-apple-darwin.dmg` | Intel Macs |
| Windows x64 | `bifrost-desktop-vX.X.X-x86_64-pc-windows-msvc.msi` | Most Windows laptops and desktops |
| Windows ARM64 | `bifrost-desktop-vX.X.X-aarch64-pc-windows-msvc.msi` | Windows on ARM devices |

If you only need the command line, install the CLI from [`getting-started.md`](./installation) instead.

### Homebrew Cask on macOS

```bash
brew tap bifrost-proxy/bifrost
brew install --cask bifrost-desktop
```

### Manual Installer

Download desktop installers from [GitHub Releases](https://github.com/bifrost-proxy/bifrost/releases).

#### macOS `.dmg`

1. Download the `.dmg` that matches your Mac.
2. Open the `.dmg`.
3. Drag `Bifrost.app` to `Applications`.
4. Launch Bifrost from `Applications`, Launchpad, or Spotlight.
5. If macOS warns that the app was downloaded from the internet, confirm that you want to open it. If Gatekeeper blocks an unsigned build, allow it from `System Settings -> Privacy & Security`.

#### Windows `.msi`

1. Download the `.msi` that matches your device architecture.
2. Double-click the `.msi` and follow the installer.
3. Launch `Bifrost` from the Start menu.
4. If Windows SmartScreen warns about an unknown publisher, continue only after confirming the file came from the official GitHub Releases page.

## First Launch

After installation:

- Launch `Bifrost.app` on macOS.
- Launch `Bifrost` from the Windows Start menu.
- The desktop app starts the bundled `bifrost` backend inside the app, so you do not need to run `bifrost start` first.
- The default data directory is `~/.bifrost`; desktop and CLI installs share config, certs, logs, and runtime state by default.
- The first launch checks and installs the Bifrost CA asynchronously. For HTTPS inspection, confirm that the system trusts the Bifrost CA.
- The desktop app checks for updates at startup and at most once every 6 hours. When a new version is available, it shows a notification and opens the update window.

Set `BIFROST_DATA_DIR` before startup only when you need an isolated test environment or a multi-instance debugging setup.

## Desktop and CLI Together

The desktop app includes a bundled `bifrost` backend for its proxy, rules, traffic, and settings pages. You can also install the standalone CLI for terminals, scripts, CI, and AI coding tools.

If you installed the desktop app first, open desktop Settings and click `Install CLI & Skills` in `Desktop Proxy Core`. This action:

- Installs the bundled `bifrost` CLI into the user command directory.
- Installs Bifrost AI skills for Codex, Claude Code, Trae, Cursor, and other supported tools.
- Keeps update semantics separate: desktop updates use `bifrost app upgrade`, while CLI updates use `bifrost upgrade`.

## Manage the Desktop App from CLI

`bifrost app` manages desktop installers separately from CLI updates:

```bash
bifrost app install
bifrost app upgrade
bifrost app uninstall
```

`bifrost app upgrade` installs the latest desktop package. If it detects a standalone CLI, it can also update that CLI to the latest version. Updates triggered from the desktop UI use the same progress window for download, install, and restart.

The `Install CLI & Skills` button is shown only in the Tauri desktop app. It is not shown in the browser-hosted CLI Web UI.

## Update and Uninstall

The desktop app checks for updates automatically. You can also manage it manually:

```bash
bifrost app upgrade
bifrost app uninstall
```

From a source checkout, the unified uninstall script can remove CLI and desktop artifacts:

```bash
./uninstall.sh
./uninstall.sh --purge
```

`--purge` removes local data, including rules, certs, logs, and runtime state. Use it only when you no longer need the existing configuration.

## Troubleshooting

### macOS says the app cannot be opened

Unsigned builds can trigger Gatekeeper. Confirm the installer came from official GitHub Releases, then allow it from `System Settings -> Privacy & Security`.

### Windows SmartScreen shows an unknown publisher warning

Confirm the file came from official GitHub Releases and that the architecture in the filename matches your device. Enterprise-managed machines may require administrator approval for unsigned installers.

### No traffic appears after launch

Confirm that the desktop backend is running, then check whether the system proxy is enabled and whether the target app uses the system proxy. If you configure a browser or CLI tool manually, the default proxy address is usually `127.0.0.1:9900`.

### HTTPS inspection fails

First confirm that the Bifrost CA is installed and trusted by the system. Some apps use certificate pinning or custom TLS stacks; exclude those apps or domains, or use rule-level `tlsPassthrough://`, instead of forcing global interception.

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
