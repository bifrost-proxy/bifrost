---
title: "Scripting"
description: "QuickJS scripting capabilities, restrictions, and usage."
editLink: false
---

> This page is automatically synced from `docs-en/scripts.md`.
> Language: **English** | [中文](../../reference/scripting)

# Scripts Management and Development Guide

The Admin UI Scripts module manages request, response, decode, and parser scripts. Scripts are stored on disk and executed by the QuickJS sandbox.

## Script Types

1. Request Script: runs before forwarding to upstream and can modify method, headers, or body.
2. Response Script: runs after receiving upstream response and can modify status, headers, or body.
3. Decode Script: decodes, redacts, or formats body content before display and persistence.
4. Parser Script: used with `bp://...` and `decode://bp` for binary protocol parsing. It affects stored and displayed traffic, not the actual client/upstream stream.

## Naming

Script names map to `{data_dir}/scripts/{type}/{name}.js`. Names may include `/` for directory hierarchy. They must be non-empty, at most 128 characters, must not start or end with `/`, must not contain `..` or `//`, and may only use letters, digits, `-`, `_`, and `/`.

## Runtime Objects

- `ctx`: request id, script name, script type, Values, matched rules, and phase.
- `log` / `console`: logs visible in the UI.
- `file`: sandboxed file API.
- `net`: optional network API with limits.
- `request` / `response`: phase-specific mutable traffic objects.

QuickJS execution is synchronous; `async/await` is not supported.
