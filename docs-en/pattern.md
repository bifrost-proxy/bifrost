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

## Caveat: do not prefix wildcard patterns with a scheme

Scheme prefixes (`http://`, `https://`, `http*://`, `ws*://`, `//`) are reliable only on Domain and PathWildcard (`^`) patterns. Combined with a wildcard host pattern they misbehave:

| Form | Actual behavior |
| ---- | --------------- |
| `*.example.com` (bare, recommended) | Correctly matches one subdomain level (HTTP + HTTPS) |
| `http://*.example.com` / `https://*.example.com` | Matches, but the single `*` also spans `.`, behaving like `**` |
| `http*://*.example.com` / `ws*://*.example.com` | **Never matches** (silently) |
| `//*.example.com` | **Matches every host** (over-broad — can hijack unrelated traffic) |

Write wildcard host patterns bare. To restrict by scheme, use a Domain pattern (e.g. `http*://api.example.com`) or a PathWildcard (e.g. `^http*://example.com/api/*`) instead. See [中文 pattern.md](../docs/pattern.md) for the full table.
