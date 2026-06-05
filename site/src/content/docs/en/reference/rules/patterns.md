---
title: "Rule Patterns"
description: "Rule matching patterns and priority details."
editUrl: false
sidebar:
  label: "Rule Patterns"
  order: 310
---

> This page is automatically synced from `docs-en/rules/patterns.md`.
> Language: **English** | [中文](../../../../reference/rules/patterns/)

# Rule Matching Patterns

Rule patterns match requests by domain, IP, wildcard, path, regex, and negation.

```txt
example.com
example.com/api
*.example.com
**.example.com
/pattern/i
!*.internal.example.com
```

Prefer the simplest pattern that expresses the target. Use regex only when domain, path, or wildcard matching is insufficient.
