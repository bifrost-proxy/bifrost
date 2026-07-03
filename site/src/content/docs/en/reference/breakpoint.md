---
title: "Breakpoint"
description: "Pause, inspect, edit, and resume matched HTTP traffic."
editLink: false
---

> This page is automatically synced from `docs-en/breakpoint.md`.
> Language: **English** | [中文](../../reference/breakpoint)

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
5. Open the request detail in Traffic.
6. Inspect or edit headers and body in the pause panel.
7. Click `Resume`.

## Rule Examples

```text
api.example.com/v1/users breakpoint://request
api.example.com/v1/users breakpoint://response
api.example.com/v1/users breakpoint://request,response
```

Supported values: `request`, `req`, `response`, `res`, `both`, `all`, and comma combinations. Keep patterns narrow to avoid blocking unrelated traffic.

## Timeout

The auto-resume timeout is configured in `Settings -> Performance`. Large, binary, or streaming bodies may be shown as header-only to avoid excessive buffering.
