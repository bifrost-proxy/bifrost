---
title: "Project Structure"
description: "Repository layout and core module descriptions."
editLink: false
---

> This page is automatically synced from `docs-en/architecture.md`.
> Language: **English** | [中文](../../reference/architecture)

# Project Structure and Modules

Bifrost is organized as a Rust workspace plus Web, desktop, documentation, and test packages.

```text
.
├── crates/
│   ├── bifrost-core/
│   ├── bifrost-command/
│   ├── bifrost-proxy/
│   ├── bifrost-tls/
│   ├── bifrost-storage/
│   ├── bifrost-script/
│   ├── bifrost-admin/
│   ├── bifrost-cli/
│   ├── bifrost-power/
│   ├── bifrost-e2e/
│   ├── bifrost-tests/
│   ├── bifrost-sync/
│   ├── agent/
│   └── skills/
├── web/
├── desktop/
├── docs/
├── docs-en/
├── site/
├── e2e-tests/
└── tests/
```

## Module Summary

- `bifrost-core`: rule parsing, matchers, and protocol definitions.
- `bifrost-command`: shared remote invoke command payloads and result models.
- `bifrost-proxy`: HTTP, HTTPS, SOCKS5, WebSocket, and tunnel handling.
- `bifrost-tls`: CA generation, dynamic certificate signing, and caching.
- `bifrost-storage`: persisted config, rules, Values, and state.
- `bifrost-script`: QuickJS script engine and sandbox runtime.
- `bifrost-admin`: Admin API and static Web UI serving.
- `bifrost-cli`: command-line service lifecycle, rules, traffic, and config management.
- `bifrost-power`: macOS keep-awake integration.
- `bifrost-e2e` and `bifrost-tests`: test runners and helpers.
- `bifrost-sync`: remote sync for rules and config.
- `agent`: agent runtime and built-in tools.
- `skills`: Agent Skill templates installed by `bifrost install-skill`.
