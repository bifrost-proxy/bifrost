> Language: **English** | [中文](../docs/breakpoint.md)

# Breakpoint User Guide

Breakpoint pauses selected HTTP requests or responses while Bifrost forwards traffic. You can inspect and edit headers or body, then resume delivery. It is designed for interactive API debugging, upstream parameter verification, downstream response simulation, and temporary rewrite experiments.

## How It Works

Breakpoint has two gates:

1. The Breakpoint switch in the Traffic toolbar must be enabled.
2. The request must match a rule containing `breakpoint://request` or `breakpoint://response`.

Only enabling the toolbar switch does not pause traffic. Matched pending traffic is released when Breakpoint is disabled.

## Web UI Workflow

1. Open the Web UI and go to Traffic.
2. Enable `Breakpoint` in the toolbar.
3. Add a precise rule on the Rules page.
4. Send a matching request.
5. Open the request detail in Traffic.
6. Select the Network row marked with the request/response pause indicator; the matching detail panel opens automatically.
7. Edit request method, URL/query, headers/body, or response status, headers/body.
8. Choose `Resume unchanged` or `Apply & Resume`.

The entire pending Network row uses a theme-aware pale warning background. It disappears immediately after resume, disabling Breakpoint, or timeout. Light and dark themes use their own warning tokens rather than a fixed light color.

## Rule Examples

```text
api.example.com/v1/users breakpoint://request
api.example.com/v1/users breakpoint://response
api.example.com/v1/users breakpoint://request,response
```

Supported values: `request`, `req`, `response`, `res`, `both`, `all`, and comma combinations. Keep patterns narrow to avoid blocking unrelated traffic.

## Timeout

The auto-resume timeout is configured in `Settings -> Performance`. Ordinary unknown-length bodies are read only up to the safe edit limit. Large, binary, or continuous streaming bodies are shown as header-only. Supported compressed text is decoded for editing and re-encoded before delivery.

The UI restores pending pauses through `GET /api/breakpoint/pending` after a refresh or push reconnect. On standard TLS ports, an enabled matching Breakpoint rule automatically requests scoped TLS interception unless `tlsIntercept://false` explicitly wins. HTTP/1.1 clients that omit ALPN, including some Windows Schannel flows, are detected after decryption as well. The client must trust the Bifrost CA; the toolbar reminds you when global TLS interception is off.
