---
title: "Operation Reference"
description: "Rule operation value formats and common usage."
editUrl: false
sidebar:
  label: "Operation Reference"
  order: 130
---

> This page is automatically synced from `docs-en/operation.md`.
> Language: **English** | [中文](../../../reference/operations/)

# Operation Reference

Operations define what Bifrost does after a pattern matches. Values can be inline, parenthesized, embedded in the rule file, or referenced from global Values depending on the protocol.

## Value Forms

```txt
pattern reqHeaders://X-Debug=1
pattern resBody://(inline body with spaces)
pattern file://{mock-response.json}
pattern urlParams://`t=${now}&id=${randomUUID}`
```

## Categories

- Routing: `host`, `xhost`, `proxy`, `http`, `https`, `ws`, `wss`, `pac`.
- Request modification: `reqHeaders`, `reqCookies`, `urlParams`, `reqBody`, `method`, `auth`.
- Response modification: `resHeaders`, `resCookies`, `statusCode`, `replaceStatus`, `resBody`, `file`, `tpl`.
- Timing and throttling: `reqDelay`, `resDelay`, `reqSpeed`, `resSpeed`.
- Scripts: `reqScript`, `resScript`, `decode`, `bp`.
- TLS and upstream behavior: `tlsIntercept`, `tlsPassthrough`, `upstreamUnsafeSsl`.

Use command-specific help and the detailed rule documents for exact value syntax.
