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
