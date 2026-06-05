---
title: "Rule Engine"
description: "Bifrost rule system overview and configuration entry points."
editUrl: false
sidebar:
  label: "Rule Engine"
  order: 120
---

> This page is automatically synced from `docs-en/rule.md`.
> Language: **English** | [中文](../../../reference/rule-engine/)

# Rule Syntax

A Bifrost rule line combines a pattern with one or more operations and optional filters or line properties.

```txt
pattern operation [operations...] [filters...] [lineProps://...]
```

## Examples

```txt
example.com host://127.0.0.1:3000
api.example.com reqHeaders://x-debug=1
chatgpt.com http3://
internal-api.example.test https://10.37.102.138:8080 upstreamUnsafeSsl://true
api.example.com/v1/users tlsIntercept:// host://127.0.0.1:3000
```

## Rule Types

- Routing operations such as `host://`, `proxy://`, `http://`, `https://`, `ws://`, and `wss://` decide where traffic goes.
- Modification operations such as `reqHeaders://`, `resHeaders://`, `statusCode://`, `reqBody://`, and `resBody://` change traffic.
- Filters narrow when a rule applies.
- Line properties control metadata such as disabled or important status.

Read [Operation reference](../operations/) and [Rules protocol reference](../rules/) for details.
