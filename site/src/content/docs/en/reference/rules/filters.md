---
title: "Filters"
description: "Request and response filter configuration."
editUrl: false
sidebar:
  label: "Filters"
  order: 280
---

> This page is automatically synced from `docs-en/rules/filters.md`.
> Language: **English** | [中文](../../../../reference/rules/filters/)

# Filters

Filters narrow when a matched rule should execute. They are useful for method, status, header, body, URL, or regex conditions.

```txt
example.com host://127.0.0.1:3000 includeFilter://m:GET
example.com host://127.0.0.1:3000 excludeFilter:///admin/
```

Use include filters to require a condition and exclude filters to skip a condition. Keep filter chains readable and prefer precise patterns when possible.
