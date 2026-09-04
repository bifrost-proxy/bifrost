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
| Route terminal tools and install CA variables | `bifrost cli-proxy enable` | Detects Bash, Zsh, Fish, or PowerShell and manages a removable profile block. |
| Redirect an online domain to a local service | `bifrost rule add ... host://...` | HTTP can route directly; HTTPS path matching usually needs TLS interception. |
| Modify headers, status, or body | `reqHeaders://`, `resHeaders://`, `statusCode://`, `file://` | Inline values are preferred for small content. |
| Understand why a request missed rules | `bifrost traffic list/get/search` | Check matched rules, entry port, URL, and protocol. |
| Serve multiple apps or tasks | `bifrost port bind ...` | One service can host multiple isolated entry ports. |
| LAN or team access | `--access-mode`, `whitelist` | Do not expose an unauthorized proxy with `allow_all`. |
| Administer a Bifrost directly by IP or domain | `bifrost client ...` | Use the target Admin API for traffic, rules, config, and status. |
| Operate remote files, shells, or processes | `bifrost remote ...` | `setting` is always local; OS-level work requires Remote Invoke. |
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

## Administer Another Bifrost Directly

On the target, set an Admin password and enable remote Admin locally:

```bash
bifrost admin passwd
bifrost admin remote enable
```

On the caller, save and log in to the target, then prefix an existing business command with `client --target <name>`:

```bash
bifrost client target add devbox \
  --url http://10.0.0.8:9900 \
  --allow-insecure-http
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
bifrost client --target devbox status --format json
bifrost client --target devbox traffic list --limit 20
bifrost client --target devbox rule list
```

Use plain HTTP only on a trusted LAN; use HTTPS, a VPN, or an SSH tunnel across untrusted networks. Client manages the target Bifrost Admin API but cannot operate target files, shells, or processes. Use `bifrost remote` explicitly for those tasks. See the [Client remote administration guide](../reference/client-admin) for target selection, security, supported commands, and troubleshooting.

## Send Traffic Through Bifrost

```bash
curl -x http://127.0.0.1:9900 http://httpbin.org/headers
curl -x http://127.0.0.1:9900 https://httpbin.org/headers
```

For terminal-only proxying with CA trust variables, use the dedicated profile command:

```bash
bifrost cli-proxy enable
bifrost cli-proxy enable --shell zsh --no-proxy "localhost,127.0.0.1,::1,*.local"
# Open a new shell or reload the profiles printed above.
bifrost cli-proxy disable
```

It supports Bash, Zsh, Fish, and PowerShell and prints complete manual setup or removal instructions if profile editing fails. Normal exit or crash cleanup removes managed blocks; restart handoff preserves them for the replacement process. `start --cli-proxy` remains a compatibility path, while new setup should use `cli-proxy enable`.

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

TLS interception priority is `Rules > Domain > App > Client IP > Global`. Within each scope, passthrough takes priority over force intercept. For example, a domain in `--intercept-exclude` remains a CONNECT tunnel even when its browser matches `--app-intercept-include`.

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

After a channel connects, Bifrost sends an online notification followed by runner-aware help. All external runners see channel commands such as `/help`, `/status`, `/cwd`, `/runner`, `/q`, `/rq`, and `/stop`; model and reasoning-effort commands are shown only when the adapter supports them.

## Agent Collaboration

Install Bifrost skills, capture the real traffic chain, then let an agent summarize URLs, methods, headers, cookies, bodies, status codes, and ordering. Manually remove sensitive tokens and personal data before publishing reusable skills.
