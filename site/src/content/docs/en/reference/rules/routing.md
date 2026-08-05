---
title: "Routing"
description: "Rule routing, forwarding, and proxy control."
editLink: false
---

> This page is automatically synced from `docs-en/rules/routing.md`.
> Language: **English** | [中文](../../../reference/rules/routing)

# Routing Rules

Routing rules decide the upstream target or downstream proxy path.

## Common Protocols

```txt
example.com host://127.0.0.1:3000
example.com xhost://127.0.0.1:3001
example.com proxy://127.0.0.1:8080
example.com https://backend.example.com
ws://example.com/socket ws://127.0.0.1:9001/socket
```

`host` redirects to a target host while preserving request path. `xhost` is a stronger redirect operation. `proxy` forwards through another proxy. `http` and `https` explicitly rewrite the upstream scheme and target. WebSocket routing uses `ws` and `wss`.

## Exact Resource Forwarding for Regex and Path-Wildcard Matchers

When the pattern is a regex (`/.../`) or a `^` path wildcard and the `http://` / `https://` target contains a non-root path, that path is treated as the complete target resource. Bifrost does not append the original directory, filename, hash, or query.

```txt
/\/component-custom-mix-eu-fest-track-load-comp-index\.[^\/]*\.js/ http://127.0.0.1:9798/component-custom-mix-eu-fest-track-load-comp-index.js
```

A request such as:

```txt
https://cdn.example.com/obj/.../component-custom-mix-eu-fest-track-load-comp-index.9230df8f.js?source=cdn
```

is forwarded directly to:

```txt
http://127.0.0.1:9798/component-custom-mix-eu-fest-track-load-comp-index.js
```

This is an internal upstream rewrite, not an HTTP 301/302 response. Use `redirect://` when the client must follow a 3xx redirect.

Compatibility boundaries:

- A regex or `^` path-wildcard target containing only an origin, such as `http://127.0.0.1:9798`, still preserves the original path and query.
- Domain, IP, and ordinary wildcard forwarding keeps the existing prefix-rewrite behavior.
- A query in the exact target URL is preserved; the original request query is not appended.
- Existing priority and first-win route selection remain unchanged.

For HTTPS path-aware routing, enable TLS interception for the relevant host or use rule-level `tlsIntercept://`.
