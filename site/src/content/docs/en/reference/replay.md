---
title: "Request Replay"
description: "Request replay capabilities, supported cases, and usage guidance."
editUrl: false
sidebar:
  label: "Request Replay"
  order: 170
---

> This page is automatically synced from `docs-en/replay.md`.
> Language: **English** | [中文](../../../reference/replay/)

# Request Replay Guide

Request replay lets you resend captured traffic from Bifrost. It is useful for comparing upstream behavior, reproducing failures, and regression testing rewrite rules.

## Recommended Workflow

1. Capture the target request through Bifrost.
2. Open the request in the Web UI Traffic detail view.
3. Use Replay to resend the request, optionally editing method, URL, headers, or body.
4. Compare status, headers, body, timing, and matched rules.

Replay works best for deterministic HTTP requests. Streaming, WebSocket, SSE, and one-time authenticated requests may require additional setup or fresh credentials.
