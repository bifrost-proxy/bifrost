---
title: "Status and Redirect"
description: "Status-code control, redirects, and response branching."
editLink: false
---

> This page is automatically synced from `docs-en/rules/status-redirect.md`.
> Language: **English** | [中文](../../../reference/rules/status-redirect)

# Status Code and Redirect Rules

Status and redirect rules control synthetic HTTP status codes and redirects.

```txt
example.com/maintenance statusCode://503
example.com/old redirect://https://example.com/new
example.com replaceStatus://200
```

`statusCode` does not contact upstream. `replaceStatus` contacts upstream and then changes the status code. Redirect rules send clients to another URL.
