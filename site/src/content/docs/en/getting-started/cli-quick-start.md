---
title: "CLI Quick Start"
description: "Task-oriented Bifrost CLI workflows."
editLink: false
---

> This page is automatically synced from `docs-en/cli-quick-start.md`.
> Language: **English** | [中文](../../getting-started/cli-quick-start)

# CLI Quick Start

This guide is organized by tasks instead of listing every flag. For the full command reference, see [CLI command reference](../reference/cli). For rule syntax, see [Rule syntax](../reference/rule-engine), [Matching patterns](../reference/patterns), and [Operation reference](../reference/operations).

## Choose a Scenario

| Scenario | First command | Key point |
| --- | --- | --- |
| Local debugging without polluting default rules | `bifrost port bind ...` | Reuse the main service and isolate rules by port. |
| Route browser or app traffic through Bifrost | `bifrost start -d` or `curl -x ...` | The background service enables the system proxy by default; use explicit proxy only for a single command. |
| Redirect an online domain to a local service | `bifrost rule add ... host://...` | HTTP can route directly; HTTPS path matching usually needs TLS interception. |
| Modify headers, status, or body | `reqHeaders://`, `resHeaders://`, `statusCode://`, `file://` | Inline values are preferred for small content. |
| Understand why a request missed rules | `bifrost traffic list/get/search` | Check matched rules, entry port, URL, and protocol. |
| Serve multiple apps or tasks | `bifrost port bind ...` | One service can host multiple isolated entry ports. |
| LAN or team access | `--access-mode`, `whitelist` | Do not expose an unauthorized proxy with `allow_all`. |
| Operate a remote Bifrost | `bifrost remote ...` | `setting` is always local; remote config requires remote execution. |
| Add a Feishu or Weixin IM channel | `bifrost im provider add ...` | Print an auth URL or QR code in the terminal and finish provider setup after scan or authorization. |

## Safe Multi-port Debugging

```bash
bifrost start -d
bifrost port bind --port 18888 --rule-text "debug.test statusCode://218 resBody://(debug)"
curl -x http://127.0.0.1:18888 http://debug.test/
```

Useful service commands:

```bash
bifrost status
bifrost status --tui
bifrost stop
bifrost restart
bifrost port list
bifrost port destroy 18888
```

## Send Traffic Through Bifrost

```bash
curl -x http://127.0.0.1:9900 http://httpbin.org/headers
curl -x http://127.0.0.1:9900 https://httpbin.org/headers
```

For TLS inspection, use the Bifrost CA. Prefer system trust stores for browsers and desktop apps. Export the CA only for tools that do not read system trust.

## Redirect a Domain to Localhost

```bash
bifrost rule add local-api -c "api.example.com host://127.0.0.1:3000"
bifrost rule enable local-api
bifrost rule active
```

## Debug HTTPS Path Rules

```bash
bifrost rule add https-path -c "api.example.com/v1/users tlsIntercept:// host://127.0.0.1:3000"
```

TLS interception priority: rule-level `tlsIntercept://` or `tlsPassthrough://`, include lists, exclude lists, then global `--intercept` / `--no-intercept`.

## Inspect Traffic

```bash
bifrost traffic list
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s
bifrost traffic get <id> --request-body --response-body
bifrost traffic get --ids 12,13,14 --request-body --response-body --format ndjson
bifrost traffic auth-status <id>
bifrost search "Bearer " --req-header
bifrost search "invalid_request_error" --res-body
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m --include response-body
bifrost traffic export <id> --as curl
bifrost traffic replay <id> --patch '/json/debug=true'
```

When using a temporary port, include `--listener-port` or `--proxy-port` filters. Traffic and export outputs currently contain captured values as-is, including Authorization, Cookie, JWT token, and other sensitive fields; a complete redaction design will be handled separately.

## Add an IM Channel

Use IM Gateway when you want Feishu or Weixin to become an agent chat entry point for Bifrost.

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu prints an authorization URL plus a terminal QR code. Weixin prints a login QR code. The CLI waits until the user authorizes or scans, then creates and connects the provider automatically. `--runner` binds the default agent runner for that IM channel. In an interactive terminal, omitting `--runner` opens a keyboard-selectable runner list; in non-interactive stdin, `--runner` is required. Provider base URLs are fixed by provider type, so `--base-url` is rejected.

If you already have credentials, use manual setup and keep secrets out of shell history with `env:NAME`:

```bash
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --runner "Claude Code"
```

After a channel connects, Bifrost sends an online notification followed by runner-aware help. All runners see channel commands such as `/help`, `/status`, `/cwd`, `/runner`, `/q`, `/rq`, and `/stop`; built-in Bifrost Agent sessions also show memory, compaction, goal, and guidance commands; external runners only show model or reasoning effort commands when the adapter supports them.

## Agent Collaboration

Install Bifrost skills, capture the real traffic chain, then let an agent summarize URLs, methods, headers, cookies, bodies, status codes, and ordering. Manually remove sensitive tokens and personal data before publishing reusable skills.
