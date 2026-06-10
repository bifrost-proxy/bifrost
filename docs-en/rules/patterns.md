> Language: **English** | [中文](../../docs/rules/patterns.md)

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

> ⚠️ **Negation (`!`) is non-functional at runtime (verified, 0.0.96).** `matches_host()` for a `!X` pattern always returns `false`, so a standalone `!`-prefixed rule (e.g. `!www.example.com host://127.0.0.1`) matches no request and forwards nothing. Use `excludeFilter://` to subtract a subset from a positively matched set instead — e.g. `svc.test host://127.0.0.1 excludeFilter:///account` (verified: `/account` falls through to the real backend, other paths forward).
