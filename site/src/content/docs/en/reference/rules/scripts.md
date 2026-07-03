---
title: "Script Rules"
description: "Attach scripts to Rules."
editLink: false
---

> This page is automatically synced from `docs-en/rules/scripts.md`.
> Language: **English** | [中文](../../../reference/rules/scripts)

# Script Rules

Script rules attach QuickJS scripts to request, response, decode, or binary parser phases.

```txt
example.com reqScript://my-request-script
example.com resScript://my-response-script
example.com decode://utf8
example.com bp://my-parser decode://bp
```

Scripts are stored under `~/.bifrost/scripts/<dir>/<name>.js`, where the on-disk directory is `request`, `response`, `decode`, or `parser` — not the protocol prefix. The mapping is `reqScript://` → `scripts/request/`, `resScript://` → `scripts/response/`, `decode://` → `scripts/decode/`, and `bp://` (parser) → `scripts/parser/`. Request scripts can mutate `request`, response scripts can mutate `response`, and decode/parser scripts affect stored and displayed traffic.
