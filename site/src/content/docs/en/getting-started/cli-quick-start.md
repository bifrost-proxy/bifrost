---
title: "CLI Quick Start"
description: "Task-oriented Bifrost CLI workflows."
editUrl: false
sidebar:
  label: "CLI Quick Start"
  order: 30
---

> This page is automatically synced from `docs-en/cli-quick-start.md`.
> Language: **English** | [中文](../../../getting-started/cli-quick-start/)

# CLI Quick Start

This guide is organized by tasks instead of listing every flag. For the full command reference, see [CLI command reference](../../reference/cli/). For rule syntax, see [Rule syntax](../../reference/rule-engine/), [Matching patterns](../../reference/patterns/), and [Operation reference](../../reference/operations/).

## Choose a Scenario

| Scenario | First command | Key point |
| --- | --- | --- |
| Local debugging without polluting default rules | `bifrost port bind ...` | Reuse the main service and isolate rules by port. |
| Route browser or app traffic through Bifrost | `bifrost start` or `curl -x ...` | System proxy is enabled by default; use explicit proxy only for a single command. |
| Redirect an online domain to a local service | `bifrost rule add ... host://...` | HTTP can route directly; HTTPS path matching usually needs TLS interception. |
| Modify headers, status, or body | `reqHeaders://`, `resHeaders://`, `statusCode://`, `file://` | Inline values are preferred for small content. |
| Understand why a request missed rules | `bifrost traffic list/get/search` | Check matched rules, entry port, URL, and protocol. |
| Serve multiple apps or tasks | `bifrost port bind ...` | One service can host multiple isolated entry ports. |
| LAN or team access | `--access-mode`, `whitelist` | Do not expose an unauthorized proxy with `allow_all`. |
| Operate a remote Bifrost | `bifrost remote ...` | `setting` is always local; remote config requires remote execution. |

## Safe Multi-port Debugging

```bash
bifrost start
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
bifrost traffic get <id> --request-body --response-body
bifrost search "Bearer " --req-header
bifrost search "invalid_request_error" --res-body
```

When using a temporary port, include `--listener-port` or `--proxy-port` filters.

## Agent Collaboration

Install Bifrost skills, capture the real traffic chain, then let an agent summarize URLs, methods, headers, cookies, bodies, status codes, and ordering. Always redact sensitive tokens and personal data before publishing reusable skills.
