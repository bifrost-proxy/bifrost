# Rule Filter and Routing Regression Fixes

## Status

Verified against current implementation as of 2026-06-17. All items in this
doc are shipped; no planned-but-unshipped work remains. Key landing points:

- Non-regex path filter prefix matching: `crates/bifrost-core/src/rule/filter.rs`
  (see `test_parse_plain_path_filter_matches_prefix` and
  `test_parse_whistle_style_wildcard_path_prefix_filter`).
- Whistle-style wildcard URL filters: `crates/bifrost-core/src/rule/filter.rs`
  + resolver tests in `crates/bifrost-core/src/rule/resolver/tests.rs`
  (`test_exclude_filter_whistle_style_wildcard_path_prefix`).
- `upstreamUnsafeSsl://true` protocol: parsing in
  `crates/bifrost-core/src/protocol.rs` and `syntax.rs`, CLI rule parsing in
  `crates/bifrost-cli/src/parsing/rules.rs`, proxy enforcement and error
  hint in `crates/bifrost-proxy/src/proxy/http/handler.rs`.
- `NetworkRecord` diagnostic fields (`actual_url`, `actual_host`,
  `listener_port`, `has_rule_hit`, `error_message`): defined in
  `crates/bifrost-core/src/bifrost_file/types.rs` and round-tripped through
  `crates/bifrost-admin/src/handlers/bifrost_file.rs`.
- `actual_url` / `actual_host` population for HTTP and tunnel/large-body
  paths: `crates/bifrost-proxy/src/proxy/http/handler.rs`,
  `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`,
  `crates/bifrost-proxy/src/proxy/socks/tcp.rs`.
- Rule docs updated: `docs/rule.md`, `docs/operation.md`,
  `docs/rules/routing.md`, `docs/rules/README.md`, `docs-en/rule.md`,
  `docs-en/operation.md`, top-level `README.md`.
- Regression fixture excluded from generic runner via `FIXTURE_ONLY_RULES`
  in `e2e-tests/run_all_tests_parallel.sh`; dedicated script
  `e2e-tests/tests/test_rule_filter_routing_diagnostics.sh` drives the
  end-to-end assertions.
- Human test: `human_tests/rule-filter-routing-diagnostics.md`, indexed in
  `human_tests/readme.md`.

## Scope

This change addresses two rule debugging regressions found from real exported
network packages:

- Bare path filters must keep regular prefix semantics so `/account` matches
  `/account-center/...`; callers that need path-segment boundaries should use
  regex filters.
- Whistle-compatible URL wildcard filters such as `excludeFilter://*/api` and
  `excludeFilter://*/alice/*` must be accepted. Whistle documents unprefixed
  filters as request URL patterns and uses `excludeFilter://*/static
  excludeFilter://*/api` for local forwarding exclusions.
- Network `.bifrost` exports preserved `matched_rules`, but omitted routing
  diagnostics such as `actual_url`, `actual_host`, `has_rule_hit`, and
  `error_message`, making a matched-but-failed upstream route look like a rule
  miss.

The `lf6-cdn2-tos.bytegoofy.com ... 10.37.102.138:8081` case is a matched rule:
the upstream target requires HTTPS, while a bare `host:port` target is treated
as `host://` and routed as HTTP unless the rule explicitly uses
`https://10.37.102.138:8081`.

The `qianchuan.jinritemai.com ... excludeFilter://...` case is a single rule
with many exclusion filters. Each exclusion must stay active in the same rule,
and a plain path exclusion such as `excludeFilter:///account` must use regular
prefix semantics for `/account-center`, `/account/...`, and query-bearing paths.
For internal HTTPS upstreams with an untrusted certificate, the rule can opt in
with `upstreamUnsafeSsl://true` instead of requiring global `--unsafe-ssl`.

## Implementation

- Change non-regex path filters like `/account` from substring matching to
  regular path-prefix matching:
  - match `/account`
  - match `/account/...`
  - match `/account?...`
  - match `/account-center`
  - do not match `/v1/account`
- Keep regex path filters such as `/api/` unchanged for path-segment boundaries
  or other stricter matching.
- Add Whistle-style wildcard URL filters for unprefixed filter values that
  contain `*` or `?`:
  - `*/alice/*` matches full request URLs containing `/alice/`
  - `*/api` matches `/api`, `/api?...`, and `/api/...`, but not `/apiary`
  - `?` matches exactly one URL character
- Add `upstreamUnsafeSsl://true` as a control protocol that sets a per-rule
  upstream HTTPS certificate verification exception.
- Extend `NetworkRecord` exports with optional diagnostic fields already stored
  on `TrafficRecord`.
- Preserve those fields on network import so imported feedback packages show
  the same rule-hit and routing context as the original traffic detail.
- Populate `actual_url` and `actual_host` consistently for normal HTTP traffic
  and streaming/large-body traffic records when host/path rules change the
  upstream target.
- Update rule docs to describe the prefix behavior and point users to explicit
  `https://` targets for HTTPS upstreams on non-standard ports.

## Validation Plan

- Unit tests:
  - path filter segment boundaries in `bifrost-core`
  - Whistle-style wildcard URL filters in `bifrost-core`
  - long `excludeFilter` chains parse every filter and exclude by regular
    prefix semantics
  - `upstreamUnsafeSsl://true` is parsed and merged into proxy resolved rules
  - network export writer serializes diagnostic fields in `bifrost-core`
  - network import/export mapping preserves diagnostics in `bifrost-admin`
- E2E:
  - real proxy request where `/account-center` is included by
    `includeFilter:///account`
  - long qianchuan-style `excludeFilter` chain where excluded paths fall
    through to a later host rule
  - Whistle-style `excludeFilter://*/api` and `excludeFilter://*/alice/*`
    where excluded requests fall through to a later host rule
  - per-rule `upstreamUnsafeSsl://true` succeeds against a self-signed HTTPS
    upstream while the same target without the rule fails
  - traffic detail and bifrost-file network export include `actual_url`,
    `actual_host`, `listener_port`, and `has_rule_hit`
  - the dedicated regression fixture is excluded from the generic parallel
    rules runner, because the generic runner intentionally does not implement
    arbitrary `includeFilter`/`excludeFilter` behavior assertions
- Human tests:
  - `human_tests/rule-filter-routing-diagnostics.md` covers the reported
    regressions and must be executed immediately after authoring.

## Review/Fix/Test Loop

Round 1:
- Recheck the user goals and exported package evidence.
- Review rule filter matching, network export/import mappings, docs, and tests.
- Run focused unit tests and the new E2E/human checks.

Round 2:
- Recheck the latest diff after any round 1 fixes.
- Verify docs and `human_tests/readme.md` index are synchronized.
- Rerun the focused tests plus workspace validation before commit.
