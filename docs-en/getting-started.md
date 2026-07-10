> Language: **English** | [中文](../docs/getting-started.md)

# Installation and Startup

This guide summarizes installation, startup, Admin UI access, environment variables, and uninstallation.

## Choose an Installation Path

| Scenario | Recommended path | Notes |
| --- | --- | --- |
| Install both CLI and desktop in one step | One-line installer | macOS and Windows get CLI + App; platforms such as Linux get the CLI. |
| Use Bifrost only from terminals, scripts, or CI | CLI-only | Add `--no-desktop` to the installer, or use Homebrew/npm. |
| Hack on Bifrost itself | Build from source | Use `./install.sh` or manually build the Rust/Web/Tauri artifacts. |

## Install the CLI + Desktop App with One Script

### One-line Install

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

The script always installs the `bifrost` CLI. On macOS and Windows it then uses that CLI to install the matching desktop app version; platforms without desktop release assets, such as Linux, remain CLI-only. It also performs first-run setup: it installs and trusts the Bifrost CA certificate, installs supported Bifrost AI skills, and starts Bifrost as a background service. Bash and PowerShell installers probe GitHub direct access and built-in mirrors, then use the fastest available release source. In restricted networks, set `BIFROST_GITHUB_MIRROR` or tune `BIFROST_MIRROR_PROBE_TIMEOUT`.

Common options:

```bash
# Install into a custom directory
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --dir /usr/local/bin

# Install a specific version
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --version v0.2.0

# Install the CLI only; skip the desktop app
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --no-desktop

# Keep CLI + App installation, but skip CA, skills, and service startup
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --no-post-install
```

In Windows PowerShell, set `$env:BIFROST_INSTALL_AUTO_DESKTOP = "0"` before invoking the remote script for a CLI-only install. When running the script from a local file, `-NoDesktop` is also available.

### Homebrew on macOS

```bash
brew tap bifrost-proxy/bifrost
brew install bifrost
```

### npm

```bash
npm i -g @bifrost-proxy/bifrost
```

### Build from Source

Requirements: Rust 1.70+, Cargo, Node.js 22+, and pnpm.

```bash
git clone https://github.com/bifrost-proxy/bifrost.git
cd bifrost
./install.sh

# Or build manually
cd web && pnpm install && pnpm build && cd ..
cargo build --release
```

### Manual Download

Download prebuilt binaries from [GitHub Releases](https://github.com/bifrost-proxy/bifrost/releases).

## Install the Desktop App

The desktop app is built with Tauri. The installer bundles the Web UI and the `bifrost` proxy backend, so it is the easiest path for users who want local traffic debugging, rule editing, replay, and updates without starting from CLI commands.

### Download the Latest Package Directly

Click the installer that matches your device. The page uses the current release naming rules to generate a direct download link.

<div class="desktop-download-grid" data-vp-download-status="loading">
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="mac-arm">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/e20a102ae0da492baf4d3b81a9a03c22.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>macOS Apple Silicon</strong>
    <span>M1 / M2 / M3 / M4</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="mac-intel">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/e20a102ae0da492baf4d3b81a9a03c22.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>macOS Intel</strong>
    <span>Intel Mac</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="win-x64">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/62baaf31cd8147699516852865782600.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>Windows</strong>
    <span>x64 installer</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="win-arm">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/62baaf31cd8147699516852865782600.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>Windows ARM64</strong>
    <span>ARM installer</span>
  </a>
</div>

<p class="desktop-download-status" data-vp-download-message>Preparing current release links...</p>

### macOS Install

1. Open the downloaded `.dmg`.
2. Drag `Bifrost.app` to `Applications`.
3. Launch Bifrost from `Applications` or Launchpad.
4. If macOS warns that the app was downloaded from the internet, confirm that you want to open it. If Gatekeeper blocks an unsigned build, allow it from `System Settings -> Privacy & Security`.

### Windows Install

1. Double-click the downloaded `.msi`.
2. Follow the installer.
3. Launch `Bifrost` from the Start menu.
4. If Windows SmartScreen warns about an unknown publisher, continue only after confirming the file came from the official `bifrost-proxy/bifrost` GitHub Releases page.

### First Launch Checklist

The desktop app starts the bundled `bifrost` backend inside the app and opens the local management interface. On first launch it checks and installs the Bifrost CA asynchronously; for HTTPS inspection, confirm that the CA is trusted by the system. The default data directory remains `~/.bifrost`.

To make Bifrost available from terminals and AI coding tools after a desktop-first install, open Settings in the desktop app and click `Install CLI & Skills` in `Desktop Proxy Core`.

For the full desktop installation, update, uninstall, and source build guide, see [`desktop.md`](./desktop.md).

## Verify Installation

```bash
command -v bifrost
bifrost --version
```

## Start the Proxy

```bash
# Start in the background. Default listener: 0.0.0.0:9900
bifrost start -d

# Custom port and host
bifrost -p 9000 -H 127.0.0.1 start

# Enable HTTP and SOCKS5
bifrost -p 9900 --socks5-port 1080 start

# Intercept TLS by domain
bifrost start --intercept-include "*.api.local"

# Foreground mode for live logs
bifrost start
```

`bifrost start -d` starts Bifrost as a background service and enables the system proxy by default, so browsers and desktop apps can enter Bifrost without extra configuration. TLS interception is opt-in; prefer domain allowlists, app allowlists, or rule-level `tlsIntercept://` instead of global interception. `--no-system-proxy` is for CI, test sandboxes, or explicit diagnostics where you do not want Bifrost to change system networking; it is not the normal first-run path.

## Admin UI

After startup, open:

```text
http://127.0.0.1:9900/_bifrost/
```

Common API roots include `/_bifrost/api/rules/*`, `/_bifrost/api/values/*`, `/_bifrost/api/traffic/*`, `/_bifrost/api/scripts/*`, and `/_bifrost/api/replay/*`.

## Environment Variables

| Variable | Description | Default |
| --- | --- | --- |
| `BIFROST_DATA_DIR` | Data directory | `~/.bifrost` |
| `RUST_LOG` | Logging level and filters | `info` |
| `WEB_PORT` | Web UI development server port | `3000` |

## Uninstall

```bash
./uninstall.sh
./uninstall.sh --purge
```
