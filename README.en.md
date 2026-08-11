# Bifrost

<p align="center">
  <strong>High-performance HTTP/HTTPS/SOCKS5 proxy server</strong>
</p>

<p align="center">
  <a href="https://github.com/bifrost-proxy/bifrost/actions"><img src="https://github.com/bifrost-proxy/bifrost/workflows/CI/badge.svg" alt="CI Status"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/v/release/bifrost-proxy/bifrost" alt="Release"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/downloads/bifrost-proxy/bifrost/total" alt="Downloads"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
</p>

> Language: **English** | [中文](README.md)
>
> · [Documentation site](https://bifrost-proxy.github.io/)

Bifrost is a high-performance, AI-friendly proxy server written in Rust and inspired by [Whistle](https://github.com/avwo/whistle). It provides request interception, rule-based modification, TLS MITM, script extensions, traffic inspection, request replay, and Web UI management.


## Quick Start

Install the CLI and, on macOS and Windows, the desktop app with the same script:

macOS / Linux / Git Bash:

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1 | iex
```

The installer always installs the CLI and automatically installs the matching desktop app on macOS and Windows; platforms without a desktop release, such as Linux, remain CLI-only. Use `--no-desktop` in Bash/Git Bash or set `BIFROST_INSTALL_AUTO_DESKTOP=0` in Bash/PowerShell for a CLI-only install. The installer can also install and trust the CA certificate, install Bifrost AI skills, and start Bifrost as a background service. Bash and PowerShell installers probe GitHub and built-in mirrors, then use the fastest available release source. Use `BIFROST_GITHUB_MIRROR` when your network needs a preferred mirror. On Windows, the PowerShell installer adds the install directory to both the current session and the Windows User `Path`; the Git Bash installer also updates the Windows User `Path`, so newly opened PowerShell/CMD windows can run `bifrost` directly.

Install a specific version:

macOS / Linux / Git Bash:

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --version v0.0.96
```

Windows PowerShell:

```powershell
$installer = irm https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.ps1
& ([scriptblock]::Create($installer)) -Version v0.0.96
```

Install with npm:

```bash
npm i -g @bifrost-proxy/bifrost
```

More installation options: [`docs-en/getting-started.md`](docs-en/getting-started.md)

Check the proxy:

```bash
bifrost status
```

Open the Admin UI after startup:

```text
http://127.0.0.1:9900/_bifrost/
```

## AI Integration

```bash
bifrost install-skill -y
```

ASR Daily Agent can fan out one daily report into separate ChatGPT Web research conversations. Set the optional `chatgpt_project_url` on the research fan-out to file every new conversation in a specific ChatGPT Project while continuing to enforce Chat mode and the Pro model:

```json
{
  "max_questions": 8,
  "chatgpt_interface_mode": "chat",
  "chatgpt_model": "pro",
  "chatgpt_project_url": "https://chatgpt.com/g/g-p-<project-id>/project"
}
```

See [`design/asr-daily-agent-pipeline.md`](design/asr-daily-agent-pipeline.md) for the complete agent graph, research manifest, and failure boundaries.

## Features

![network.png](assets/network.png)
![scripts.png](assets/scripts.png)
![rules.png](assets/rules.png)
![replay.png](assets/replay.png)
![metrics.png](assets/metrics.png)

- High-performance proxy core based on Tokio and Hyper.
- Multi-protocol support: HTTP/1.1, HTTP/2, HTTP/3, HTTPS, SOCKS5, WebSocket, SSE, and gRPC.
- TLS interception with CA generation, dynamic certificates, rule-level intercept or passthrough, and explicit unsafe upstream certificate allowance per rule.
- Rules engine for routing, request/response rewriting, injection, delay, throttling, mock responses, and scripts.
- Built-in Web UI for rule editing, traffic inspection, script management, and request replay.
- Breakpoint support for pausing matched HTTP requests or responses, editing headers/body, then resuming.
- Resource risk warnings for body/ws file writers and file-handle pressure in the Performance page and `/_bifrost/api/system/memory`.
- QuickJS-based script sandbox with `reqScript`, `resScript`, and `decode` support.

## Development Setup

Initialize repository hooks after cloning:

```bash
bash scripts/setup-git-hooks.sh
# or
make setup
```

This sets `core.hooksPath=.githooks` for the repository. The default pre-commit hook checks workspace formatting, desktop Tauri formatting, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

## Prefer the Desktop App?

Use the desktop app when you want traffic capture, rule editing, replay, and updates without memorizing CLI commands:

1. Open [GitHub Releases](https://github.com/bifrost-proxy/bifrost/releases).
2. Choose the latest release and expand `Assets`.
3. On macOS, download the `.dmg` that matches your chip: `aarch64-apple-darwin` for Apple Silicon or `x86_64-apple-darwin` for Intel.
4. On Windows, download the `.msi`: `x86_64-pc-windows-msvc` for most PCs or `aarch64-pc-windows-msvc` for ARM devices.
5. On macOS, open the `.dmg` and drag `Bifrost.app` to `Applications`. On Windows, double-click the `.msi` and follow the installer.
6. Launch Bifrost and wait for the bundled proxy backend to start. The first launch checks and installs the Bifrost CA asynchronously.

To make the CLI available from a desktop-first install, open Settings in the desktop app and click `Install CLI & Skills` in `Desktop Proxy Core`. See [`docs-en/desktop.md`](docs-en/desktop.md) for the full guide.

## Common Commands

```bash
# Status and lifecycle
bifrost status
bifrost stop
bifrost restart

# Admin remote access and authentication
bifrost admin remote status
bifrost admin remote enable
bifrost admin passwd
bifrost admin revoke-all

# Traffic inspection
bifrost traffic list
bifrost traffic search "keyword" --method POST --host api.openai.com --path /v1/responses
bifrost search "keyword" --req-header
bifrost search "keyword" --res-body

# Add a rule
bifrost rule add local-dev --content "example.com host://127.0.0.1:3000"
```

## Documentation Index

- Documentation overview: [`docs-en/README.md`](docs-en/README.md)
- Project overview: [`docs-en/overview.md`](docs-en/overview.md)
- Installation and startup: [`docs-en/getting-started.md`](docs-en/getting-started.md)
- CLI command reference: [`docs-en/cli.md`](docs-en/cli.md)
- Desktop installation and build: [`docs-en/desktop.md`](docs-en/desktop.md)
- Rule syntax: [`docs-en/rule.md`](docs-en/rule.md)
- Operation reference: [`docs-en/operation.md`](docs-en/operation.md)
- Matching patterns: [`docs-en/pattern.md`](docs-en/pattern.md)
- Rules protocol reference: [`docs-en/rules/README.md`](docs-en/rules/README.md)
- Scripts module and script development: [`docs-en/scripts.md`](docs-en/scripts.md)
- Values guide: [`docs-en/values.md`](docs-en/values.md)
- Request replay guide: [`docs-en/replay.md`](docs-en/replay.md)
- Breakpoint user guide: [`docs-en/breakpoint.md`](docs-en/breakpoint.md)
- Project structure and modules: [`docs-en/architecture.md`](docs-en/architecture.md)
- Agent Skill installation guide: [`docs-en/agent-skill.md`](docs-en/agent-skill.md)
