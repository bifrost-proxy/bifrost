---
title: "Rule Patterns"
description: "Rule matching patterns and priority details."
editLink: false
---

> This page is automatically synced from `docs-en/rules/patterns.md`.
> Language: **English** | [中文](../../../reference/rules/patterns)

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

A wildcard host can be followed by a boundary-aware URL fragment. `*/get_domains/v5` matches the exact path, its query, and child paths on a single-label host such as `api`, but not `/get_domains/v50`. Use `**/get_domains/v5` when the host may contain dots, such as `api.example.com`.

> ⚠️ **Negation (`!`) is non-functional at runtime (verified, 0.0.96).** `matches_host()` for a `!X` pattern always returns `false`, so a standalone `!`-prefixed rule (e.g. `!www.example.com host://127.0.0.1`) matches no request and forwards nothing. Use `excludeFilter://` to subtract a subset from a positively matched set instead — e.g. `svc.test host://127.0.0.1 excludeFilter:///account` (verified: `/account` falls through to the real backend, other paths forward).
