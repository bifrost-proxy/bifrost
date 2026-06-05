---
title: "URL Manipulation"
description: "URL, path, and query-parameter rewrite capabilities."
editUrl: false
sidebar:
  label: "URL Manipulation"
  order: 240
---

> This page is automatically synced from `docs-en/rules/url-manipulation.md`.
> Language: **English** | [中文](../../../../reference/rules/url-manipulation/)

# URL Manipulation Rules

URL manipulation rules add, replace, or remove URL components.

```txt
example.com urlParams://debug=true
example.com urlParams://(version: 2)
example.com urlReplace://v1=v2
example.com urlReplace://(/v\d+/=v2)
```

Use backticks when runtime template variables are needed, for example `urlParams://` with `${now}` or `${randomUUID}`. Embedded values are recommended for multi-parameter configuration.
