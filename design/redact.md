# Secrets Redaction Layer (`bifrost-admin::redact`)

> Default-on protection for CLI output (`bifrost traffic get`, `bifrost search`) so that
> automation pipelines and ad-hoc human debugging never leak `Authorization`,
> `Cookie`, JWT, API keys, or other secret material into terminals, log files
> and AI assistant transcripts.

## Goals

1. Make the **default** `bifrost traffic get <id>` output safe to paste into
   chat, logs or tickets.
2. Allow easy opt-out (`--show-secrets`) for legitimate debugging.
3. Allow a "summary-only" mode (`--extract-auth-summary`) that shows
   *whether* a request was authenticated, *who* it belonged to (JWT `sub`),
   *when* the token expires, and *which* cookie names were present — without
   ever revealing the secret bytes.
4. Provide an emergency kill-switch via env var `BIFROST_REDACT=off` for
   upgrade periods, so existing scripts that depend on the previous behaviour
   can keep working without code change.

## Scope

This wave touches **only CLI output paths**. The proxy still stores the raw
traffic (so capture/replay continue to work). Redaction happens at the moment
of formatting/printing on the local `bifrost` command.

Affected surfaces:

| Surface                       | Behaviour                                           |
| ----------------------------- | --------------------------------------------------- |
| `bifrost traffic get`         | Headers & JSON body redacted by default              |
| `bifrost traffic get --show-secrets` | Disable redaction                              |
| `bifrost traffic get --extract-auth-summary` | Redacted + appended `auth_summary` block |
| `bifrost search` (any subset) | Same default; same flags                            |
| Env `BIFROST_REDACT=off`      | Both `traffic get` and `search` skip redaction      |

Out of scope for this wave (tracked separately):

* `commands/remote.rs` (Bifrost Remote Invoke clients) — owned by another agent.
* WebUI rendering — handled by frontend wave.
* `bifrost-admin` HTTP handlers (search/handlers/auth_inspect/capture/replay) —
  intentionally untouched per the task whitelist; redaction is purely
  client-side in this wave.

## Header Redaction Rules

A header is replaced with `<REDACTED:len=N>` (character count of the original
value) when any of:

* Name (case-insensitive) is `Authorization`, `Proxy-Authorization`,
  `Cookie`, `Set-Cookie`.
* Name (case-insensitive) starts with `x-tt-passport-` (internal passport
  cookies).
* Name (case-insensitive) matches the regex
  `(?i)^(x-)?(.*-)?(token|secret|jwt|api[-_]?key|session[-_]?id|csrf)$`.

`Content-Length`, `Content-Type` and other diagnostic headers are **always**
preserved.

## JSON Body Redaction Rules

Body redaction runs only when `Content-Type` contains `json` (case-insensitive).
For non-JSON bodies we deliberately leave the bytes alone — heuristic edits to
binary or text-of-unknown-format would silently corrupt user output and is
worse than no redaction.

For matched JSON, we walk the tree and replace the value of any object key
whose **lower-cased** form matches
`(?i)(password|token|secret|jwt|api_?key|access_?key|sk-|refresh_?token)`:

* String value → `"<REDACTED:len=N>"` (preserves shape, keeps JSON Schema
  validation happy).
* Number / boolean → stringified into `<REDACTED:len=N>` (these are rare; we
  prefer not to silently drop fields).
* Object / array → recurse (still redact deeper sensitive keys).

Non-sensitive keys recurse unchanged.

## Auth Summary

`auth_summary_from_headers` builds:

```text
AuthSummary {
    has_jwt: bool,
    has_cookie: bool,
    jwt_user_id: Option<String>,
    jwt_exp_unix: Option<i64>,
    cookie_names: Vec<String>,
    host: Option<String>,
}
```

* `Bearer <jwt>` → `has_jwt=true`. We base64url-decode the JWT payload and
  extract `sub` (fallback: `user_id`, `uid`). `exp` is read directly. Any
  decode/parse failure leaves the field `None` — we never throw.
* `Authorization: <other>` → `has_jwt=true` (some clients use `Token` etc.),
  but `jwt_user_id` stays `None`.
* `Cookie: a=1; b=2; a=3` → `has_cookie=true`, `cookie_names=["a","b"]`
  (sorted + deduped).
* `Set-Cookie: sid=...; Path=/` → `has_cookie=true`, `cookie_names=["sid"]`.
* `Host` / `:authority` → captured as `host`.

`AuthSummary` never carries the raw token, cookie value, or signature.

## CLI Wiring

* `bifrost traffic get` and `bifrost search` gain
  `--show-secrets` and `--extract-auth-summary` flags.
* `traffic get`'s `render_traffic_detail_value` mutates `request_headers`,
  `response_headers`, `original_*_headers`, `request_body`, and
  `response_body` according to the resolved `RedactOptions`.
* When `--extract-auth-summary` is set, an `auth_summary` field is appended
  to the JSON output (and printed as a `── Auth Summary ──` block in
  table/compact mode).
* `bifrost search`'s match preview is **not** redacted in this wave because
  the search engine output already lives entirely server-side; we add a TODO
  marker + `RedactOptions` plumbing so the P0-3 wave (which adds bodies to
  `search`) can call `redact_body_text` before printing.

### Env kill switch

If the env var `BIFROST_REDACT` is set to `off`, `0`, `false`, or `no`
(case-insensitive), the CLI behaves as if `--show-secrets` had been passed
to every invocation. This is intended for the migration window only and
exists so users with brittle scripts can `export BIFROST_REDACT=off` rather
than rewriting them.

The kill switch is **CLI-only**. The library API in `bifrost-admin::redact`
does not read env — callers must opt out explicitly. This keeps redaction
deterministic for the WebUI / sync paths once they adopt the library.

## Breaking Change Note

This is a **breaking change** for any automation that parses the raw
`Authorization` / `Cookie` values out of `bifrost traffic get --format
json-pretty`. Mitigation:

1. Add `--show-secrets` to the offending command.
2. Or set `BIFROST_REDACT=off` once at the shell.
3. Or, preferably, migrate to `--extract-auth-summary` which gives
   structured *facts* about auth without leaking the bytes.

We deliberately ship this change in the same release as the new
`bifrost auth-status` (P1-4) and `bifrost replay` (P2-6) commands so that
users moving to the diagnostics suite get safe-by-default behaviour from
day one.

## Relation to Sibling Waves

* **P1-4 `auth_inspect`** (admin-side) returns a structured auth diagnostic.
  When the admin endpoint becomes available, the CLI may switch to calling
  it directly. Until then, `auth_summary_from_headers` keeps the
  client-side computation simple and dependency-free.
* **P2-6 `replay`** intentionally needs the *raw* request bytes (replaying
  with a redacted `<REDACTED:...>` `Authorization` would obviously break).
  The replay path therefore does **not** call the redactor — it fetches the
  raw record directly from `bifrost-admin`. This is why redaction lives in
  the CLI rendering layer only.
* **P0-3 search bodies** will add body fetch to `bifrost search`. That wave
  will import `redact_body_bytes` / `redact_body_text` and `RedactOptions`
  to redact those bodies in the same default-on manner.

## Test Matrix

Unit tests in `redact.rs`:

* Header redaction — including `Authorization`, `Cookie`, `X-Api-Key`,
  `X-Csrf-Token`, `X-Session-Id`, `X-JWT`, `X-Tt-Passport-Cookie`, and that
  benign headers (`Content-Type`, `X-Request-Id`) are preserved.
* JSON body redaction — nested objects, arrays, key case-insensitivity,
  non-JSON passthrough, invalid-JSON passthrough.
* `auth_summary_from_headers` — Bearer JWT, broken JWT, cookies only, no
  auth header at all.
* `show_secrets=true` bypass.

`human_tests/redact.md` covers four end-to-end scenarios:

1. Default `traffic get` output is redacted.
2. `--show-secrets` returns originals.
3. `--extract-auth-summary` adds the structured block.
4. `BIFROST_REDACT=off` env disables redaction without flag changes.
