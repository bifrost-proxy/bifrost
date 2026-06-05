---
title: "Request Modification"
description: "Request headers, body, and related rewrite operations."
editUrl: false
sidebar:
  label: "Request Modification"
  order: 220
---

> This page is automatically synced from `docs-en/rules/request-modification.md`.
> Language: **English** | [中文](../../../../reference/rules/request-modification/)

# Request Modification Rules

Request modification rules change data before it reaches upstream.

```txt
example.com reqHeaders://X-Debug=1
example.com reqCookies://session=debug
example.com urlParams://debug=true
example.com method://POST
example.com reqBody://({"debug":true})
```

Prefer inline values for small changes. Use embedded rule-file blocks for larger headers or bodies that belong to one rule. Use global Values only when content is large or shared across many rules.

Authorization should usually be set with `reqHeaders://Authorization=Bearer ...` or `reqHeaders://{headers}` rather than an ambiguous shorthand.
