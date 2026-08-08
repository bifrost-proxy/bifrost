---
title: "Request Modification"
description: "Request headers, body, and related rewrite operations."
editLink: false
---

> This page is automatically synced from `docs-en/rules/request-modification.md`.
> Language: **English** | [中文](../../../reference/rules/request-modification)

# Request Modification Rules

Request modification rules change data before it reaches upstream.

```txt
example.com reqHeaders://X-Debug=1
example.com reqHeaders://(X-Env=test&X-Mode=debug)
example.com reqCookies://session=debug
example.com urlParams://debug=true
example.com method://POST
example.com reqBody://({"debug":true})
```

Prefer inline values for small changes. Use embedded rule-file blocks for larger headers or bodies that belong to one rule. Use global Values only when content is large or shared across many rules.

Authorization should usually be set with `reqHeaders://Authorization=Bearer ...` or `reqHeaders://{headers}` rather than an ambiguous shorthand.

For single-line inline header maps, `&` separates independent headers. Template
results are expanded only after those authored boundaries are parsed, so a URL
or copied header value containing `&` remains one header value and cannot inject
another header. Referenced, multi-line, and JSON Values also preserve literal
ampersands in header values.
