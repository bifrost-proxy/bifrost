> Language: **English** | [中文](../docs/pattern.md)

# Matching Patterns

Patterns decide whether a rule applies to a request. Bifrost supports domain, IP, wildcard, regex, path, and negated patterns.

## Common Forms

```txt
example.com
example.com/api
*.example.com
**.example.com
192.168.1.1
192.168.0.0/16
/pattern/i
!*.internal.example.com
```

## Guidance

- Prefer precise domain and path patterns for debugging.
- Use wildcard patterns for families of subdomains.
- Use regex only when the simpler forms are not expressive enough.
- Negated patterns help exclude sensitive or unrelated domains.
- HTTPS path matching usually requires TLS interception because CONNECT tunnels do not expose inner paths.
