---
title: "Scripting"
description: "QuickJS scripting capabilities, restrictions, and usage."
editLink: false
---

> This page is automatically synced from `docs-en/scripts.md`.
> Language: **English** | [中文](../../reference/scripting)

# Scripts Management and Development Guide

The Admin UI Scripts module manages request, response, streaming response, decode, and parser scripts. Scripts are stored on disk and executed by the QuickJS sandbox. For rules generated or shared through a CLI or AI, prefer inline script blocks in the rule itself; the Scripts page is intended primarily for interactive tests, manual editing, and reusable local named scripts.

## Script Types

1. Request Script: runs before forwarding to upstream and can modify method, headers, or body.
2. Response Script: runs after receiving upstream response and can modify status, headers, or body.
3. Streaming Response Script: runs through `resStreamScript://` and incrementally transforms or mocks true SSE output without collecting the response.
4. Decode Script: decodes, redacts, or formats body content before display and persistence.
5. Parser Script: used with `bp://...` and `decode://bp` for binary protocol parsing. It affects stored and displayed traffic, not the actual client/upstream stream.

## Naming

Script names map to `{data_dir}/scripts/{type}/{name}.js`. Names may include `/` for directory hierarchy. They must be non-empty, at most 128 characters, must not start or end with `/`, must not contain `..` or `//`, and may only use letters, digits, `-`, `_`, and `/`.

## Runtime Objects

- `ctx`: request id, script name, script type, Values, matched rules, and phase.
- `log` / `console`: logs visible in the UI.
- `file`: sandboxed file API.
- `net`: optional network API with limits.
- `request` / `response`: phase-specific mutable traffic objects.

QuickJS execution is synchronous; `async/await` is not supported.

## Streaming Responses with `resStreamScript`

Use `resStreamScript://{script_name}` or an inline rule block when an SSE response must remain incremental. Each HTTP response stream gets its own persistent QuickJS context, so top-level variables and closures survive across events in the same stream while separate requests remain isolated.

Two true streaming modes are available:

- **Transform** requires an upstream `Content-Type: text/event-stream` response. Bifrost calls `stream.onEvent(event)` for each complete SSE event and sends the returned output immediately, without waiting for upstream EOF or `[DONE]`.
- **Mock** calls `stream.next()` repeatedly. Each result is sent before the optional `delayMs` is applied, so the response is not pre-generated or buffered as a complete body.

Transform example:

```javascript
stream.mode = "transform";
let index = 0;

stream.onEvent = (event) => {
  index += 1;
  return {
    event: "mapped.delta",
    data: JSON.stringify({ index, upstream: event.data }),
  };
};

stream.onEnd = () => "data: [DONE]\n\n";
```

Mock example:

```javascript
stream.mode = "mock";
let index = 0;

stream.next = () => {
  index += 1;
  return {
    output: { event: "mock.delta", data: String(index) },
    delayMs: 100,
    done: index === 3,
  };
};
```

`stream.next()`, `stream.onEvent(event)`, and `stream.onEnd()` may return a raw SSE string, an SSE event object, an array, a step object with `output` / `outputs`, or `null` / `undefined` for no output. A complete input event is limited to 16 MiB; oversize events and callback failures after streaming starts produce an explicit SSE error instead of silent truncation.

Streaming callbacks remain synchronous and do not support `async/await`. The sandbox `timeout_ms` restarts for each JavaScript callback and limits only that callback's CPU time; time spent waiting for the next upstream event does not accumulate. A matched result may contain only one `resStreamScript`, and it cannot be combined with `resScript`, which collects the complete body.

See the [Script Rules reference](./rules/scripts) for inline blocks, event fields, return shapes, response headers, and composition limits.
