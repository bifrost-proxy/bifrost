---
title: "Matching Patterns"
description: "Domain, path, wildcard, regex, and negated pattern behavior."
editLink: false
---

> This page is automatically synced from `docs-en/pattern.md`.
> Language: **English** | [中文](../../reference/patterns)

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
- HTTPS path matching usually requires TLS interception because CONNECT tunnels do not expose inner paths.

> ⚠️ **Negated (`!`) patterns do not work at runtime (verified, 0.0.96).** A `!X` pattern parses, but `matches_host()` always returns `false`, so a standalone `!`-prefixed pattern matches nothing — neither `X` nor non-`X` (verified: `!keep.test statusCode://249` returns no `249` for either `keep.test` or `other.test`, while the non-negated `pos.test statusCode://248` works). Don't use `!` as a router. To exclude a subset, use `excludeFilter://` instead (request-phase method/path/standard-header excludes work).

## Caveat: do not prefix wildcard patterns with a scheme

Scheme prefixes (`http://`, `https://`, `http*://`, `ws*://`, `//`) are reliable only on Domain and PathWildcard (`^`) patterns. Combined with a wildcard host pattern they misbehave:

| Form | Actual behavior |
| ---- | --------------- |
| `*.example.com` (bare, recommended) | Correctly matches one subdomain level (HTTP + HTTPS) |
| `http://*.example.com` / `https://*.example.com` | Matches, but the single `*` also spans `.`, behaving like `**` |
| `http*://*.example.com` / `ws*://*.example.com` | **Never matches** (silently) |
| `//*.example.com` | **Matches every host** (over-broad — can hijack unrelated traffic) |

Write wildcard host patterns bare. To restrict by scheme, use a Domain pattern (e.g. `http*://api.example.com`) or a PathWildcard (e.g. `^http*://example.com/api/*`) instead. See [中文 pattern.md](../../reference/patterns) for the full table.
