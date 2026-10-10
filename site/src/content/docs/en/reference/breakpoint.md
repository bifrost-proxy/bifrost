---
title: "Breakpoint"
description: "Pause, inspect, edit, and resume matched HTTP traffic."
editLink: false
---

> This page is automatically synchronized from `docs-en/breakpoint.md`.
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
5. The first matching pause after enabling the gate selects the request and opens its phase detail once.
6. Later concurrent hits highlight their rows without stealing focus. Select them manually; drafts survive selection changes and push reconnects for the same pause.
7. Edit request method, URL/query, headers/body, or response status, headers/body.
8. Choose `Resume unchanged` or `Apply & Resume`.

The entire pending Network row uses a theme-aware pale warning background, including rows shown by Fuzzy Search, and keeps the request/response phase indicator. It disappears immediately after resume, disabling Breakpoint, or timeout. Light and dark themes use their own warning tokens rather than a fixed light color.

The “Only paused breakpoints” checkbox combines with regular filters and Fuzzy Search. Resumed, expired, and gate-released rows leave this view immediately.

## Rule Examples

```text
api.example.com/v1/users breakpoint://request
api.example.com/v1/users breakpoint://response
api.example.com/v1/users breakpoint://request,response
```

Supported values: `request`, `req`, `response`, `res`, `both`, `all`, and comma combinations. Keep patterns narrow to avoid blocking unrelated traffic.

## Timeout

The auto-resume timeout is configured in `Settings -> Performance`. A response body is editable when it has an explicit safe `Content-Length` within the byte limit, including binary data. Unknown-length, large, or continuous streaming responses pause immediately at the header stage and are shown as header-only, preserving the original stream after resume. Supported compression is decoded for UTF-8/Base64 editing and re-encoded before delivery. Unknown or invalid compression exposes bounded raw bytes as Base64. Response status edits must be 200–599; informational 1xx responses cannot be submitted as final responses. Changing a response status to 204 or 304 clears the payload and removes `Content-Length` / `Transfer-Encoding`.

The countdown derives the remaining duration from the server time and deadline returned by the proxy, so clock skew between a remote Web UI and the proxy does not make the pause appear expired early.

The UI restores pending pauses through `GET /api/breakpoint/pending` after a refresh or push reconnect. On standard and nonstandard TLS ports, an enabled matching Breakpoint rule automatically requests scoped TLS interception unless `tlsIntercept://false` explicitly wins. HTTP/1.1 clients that omit ALPN, including some Windows Schannel flows, are detected after decryption as well. The client must trust the Bifrost CA; the toolbar reminds you when global TLS interception is off.

## Body Format and Validation

Select UTF-8 or Base64 in the body editor. `body_encoding` is `utf8` or `base64`; `body_representation` is `decoded` for supported compression and `raw` for unknown encodings. Base64 preserves bytes that cannot be decoded as UTF-8. Raw edits retain the original content encoding.

Capture and edits share the byte limit: 1 MiB by default, at most 10 MiB. Invalid Base64, oversized edits, edits to omitted/bodyless bodies, and unsupported encoding conversions return an error while the pause stays available for correction. Infinite streams are never fully buffered. Declared-length capture has a two-second deadline; a stalled sender becomes header-only, and resume replays the captured prefix plus the remaining original stream. HEAD response bodies are not editable. Edited 204 and 304 responses discard payloads before delivery and follow bodyless HTTP semantics.
