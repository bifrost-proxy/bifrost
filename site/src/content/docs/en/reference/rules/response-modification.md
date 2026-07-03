---
title: "Response Modification"
description: "Response headers, body, and related rewrite operations."
editLink: false
---

> This page is automatically synced from `docs-en/rules/response-modification.md`.
> Language: **English** | [中文](../../../reference/rules/response-modification)

# Response Modification Rules

Response modification rules change upstream responses before the client receives them.

```txt
example.com resHeaders://X-Proxy=Bifrost
example.com statusCode://503
example.com replaceStatus://200
example.com resBody://({"ok":true})
example.com file://./fixtures/mock.json
```

Use `statusCode` to return a synthetic status without contacting upstream. Use `replaceStatus` when the upstream request should still be sent but the returned status should be changed. Use `file`, `tpl`, or `resBody` for mock bodies.
