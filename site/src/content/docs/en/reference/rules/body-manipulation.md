---
title: "Body Manipulation"
description: "Request and response body manipulation capabilities."
editUrl: false
sidebar:
  label: "Body Manipulation"
  order: 250
---

> This page is automatically synced from `docs-en/rules/body-manipulation.md`.
> Language: **English** | [中文](../../../../reference/rules/body-manipulation/)

# Body Manipulation Rules

Body manipulation rules modify request or response bodies.

```txt
example.com reqBody://({"debug":true})
example.com resBody://(mock response)
example.com file://./fixtures/response.json
example.com tpl://./fixtures/template.html
```

For large, binary, or streaming bodies, prefer file-based mocks or scripts. Keep body rewrites scoped to precise patterns to avoid accidental broad modification.
