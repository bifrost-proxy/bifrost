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
- HTTPS path matching usually requires TLS interception because CONNECT tunnels do not expose inner paths.

## Wildcard host with a URL-fragment path

A wildcard host may be followed by a normal path fragment. When the path itself has no wildcard, it is a boundary-aware prefix: it matches the exact path, its query string, and child paths, but not a longer lookalike path.

```txt
*/get_domains/v5   # matches host `api`; does not match /get_domains/v50
**/get_domains/v5  # also matches dotted hosts such as api.example.com
```

A single `*` in the host does not span `.`, while `**` does. Path wildcards such as `example.com/api/*` retain their existing behavior; use the leading `^` form for strict path-segment wildcard semantics.

Captured values may be used in multiple headers with an ampersand-separated
inline map: `reqHeaders://(X-Version=$1&X-ID=$2)`. Use JSON or a multi-line Value
when the resulting header value itself must contain a literal `&`.

> ⚠️ **Negated (`!`) patterns do not work at runtime (verified, 0.0.96).** A `!X` pattern parses, but `matches_host()` always returns `false`, so a standalone `!`-prefixed pattern matches nothing — neither `X` nor non-`X` (verified: `!keep.test statusCode://249` returns no `249` for either `keep.test` or `other.test`, while the non-negated `pos.test statusCode://248` works). Don't use `!` as a router. To exclude a subset, use `excludeFilter://` instead (request-phase method/path/standard-header excludes work).

## Caveat: do not prefix wildcard patterns with a scheme

Scheme prefixes (`http://`, `https://`, `http*://`, `ws*://`, `//`) are reliable only on Domain and PathWildcard (`^`) patterns. Combined with a wildcard host pattern they misbehave:

| Form | Actual behavior |
| ---- | --------------- |
| `*.example.com` (bare, recommended) | Correctly matches one subdomain level (HTTP + HTTPS) |
| `http://*.example.com` / `https://*.example.com` | Matches, but the single `*` also spans `.`, behaving like `**` |
| `http*://*.example.com` / `ws*://*.example.com` | **Never matches** (silently) |
| `//*.example.com` | **Matches every host** (over-broad — can hijack unrelated traffic) |

Write wildcard host patterns bare. To restrict by scheme, use a Domain pattern (e.g. `http*://api.example.com`) or a PathWildcard (e.g. `^http*://example.com/api/*`) instead. See [中文 pattern.md](../docs/pattern.md) for the full table.
