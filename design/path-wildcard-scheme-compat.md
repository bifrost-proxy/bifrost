# Path Wildcard Scheme Compatibility

## Module

Path wildcard matchers handle patterns that start with `^`, for example:

```text
^example.com/assets/*/index.html
```

Historically users also wrote scheme-qualified path wildcard patterns:

```text
^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html
```

The matcher should keep these rules valid instead of requiring users to remove
the scheme.

## Implementation

`PathWildcardMatcher` now parses an optional protocol prefix after `^`:

| Pattern prefix | Regex protocol prefix |
| --- | --- |
| no scheme | `^https?://` |
| `http://` | `^http://` |
| `https://` | `^https://` |
| `http*://` | `^https?://` |
| `//` | `^https?://` |

The path wildcard conversion remains unchanged after the optional scheme is
removed. Single `*` still captures one path segment without `/` or `?`, `**`
captures path content without `?`, and `***` captures the rest.

When a scheme-qualified or scheme-less `^...` path wildcard rule forwards to a
target URL that contains a path, that target path is used exactly. For example,
this rule:

```text
^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html http://127.0.0.1:8999/approvals
```

forwards matching requests to:

```text
http://127.0.0.1:8999/approvals
```

It does not append the original CDN path after `/approvals`.

## Dependencies

- `regex` in `bifrost-core`
- Existing `Matcher` trait and rule parser behavior

## Test Plan

Unit tests:

- `test_explicit_https_path_wildcard_support`: verifies the concrete approvals
  legacy rule matches HTTPS and does not match HTTP.
- `test_explicit_http_path_wildcard_support`: verifies `^http://...` remains
  HTTP-only.
- `test_scheme_wildcard_path_wildcard_support`: verifies `^http*://...` and
  `^//...` match both HTTP and HTTPS.
- Existing `test_https_support`: verifies scheme-less `^example.com/...` still
  matches both HTTP and HTTPS.

E2E tests:

- `matcher_scheme_qualified_path_wildcard`: starts an isolated proxy and local
  upstream, then verifies a scheme-qualified `^http://...` path wildcard rule
  forwards a real request to the fixed target path.

Human tests:

- Update `human_tests/proxy-http-https.md` with a regression case for the
  legacy `^https://...` approvals rule.
- Execute the case against an isolated proxy on a non-9900 port with
  `--no-system-proxy`.
- Update `human_tests/readme.md` case count and scope.

## Validation

- `cargo test -p bifrost-core explicit_https_path_wildcard`
- `cargo test -p bifrost-core path_wildcard`
- `cargo test -p bifrost-e2e matcher_scheme_qualified_path_wildcard`
- `cargo test --workspace --all-features`
- Final Rust validation via `rust-project-validate` after E2E and human tests.

## Documentation

No public CLI syntax changes are introduced. This is a backward compatibility
fix for existing `^https://...` and `^http://...` path wildcard rules.
