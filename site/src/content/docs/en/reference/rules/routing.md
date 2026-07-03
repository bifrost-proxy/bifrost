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

For HTTPS path-aware routing, enable TLS interception for the relevant host or use rule-level `tlsIntercept://`.
