---
title: "Filters"
description: "Request and response filter configuration."
editUrl: false
sidebar:
  label: "Filters"
  order: 280
---

> This page is automatically synced from `docs-en/rules/filters.md`.
> Language: **English** | [中文](../../../../reference/rules/filters/)

# Filters

Filters narrow when a matched rule should execute, using method, header, IP, URL, or path/regex conditions.

```txt
example.com host://127.0.0.1:3000 includeFilter://m:GET
example.com host://127.0.0.1:3000 excludeFilter:///admin/
```

Use include filters to require a condition and exclude filters to skip a condition.

> ⚠️ **Several filter prefixes are inert at runtime in 0.0.96 (verified) — do not rely on them:**
> - **`s:` (status) and `resH:` (response header)** are never evaluated: `includeFilter://s:` / `includeFilter://resH:` lock the rule to never apply (even when the status/header actually matches), and `excludeFilter://s:` / `excludeFilter://resH:` never exclude. They run in the request phase where the response is not yet known.
> - **`h:` / `reqH:` only match standard request headers** (e.g. `User-Agent`, `Accept`, `Host`). `Content-Type` and custom headers (`X-*`) are not evaluated — verified `h:Content-Type=json` and `h:X-Custom-Header` apply regardless of the actual request, while `h:User-Agent=curl` works.
> - **`b:` (body) does not filter.** The documented `b:/regex/` form is fail-closed on include (rule never matches) and never excludes; a bare `b:foo` value is ignored.
>
> Working request-phase filters: `m:` (method), `i:` (client IP), `/path` and `/regex/` (path), `h:`/`reqH:` for standard headers, and host/URL filters. For status/body/response-header filtering use `bifrost search` instead.
