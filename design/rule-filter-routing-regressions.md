# Rule Filter and Routing Regression Fixes

## Scope

This change addresses two rule debugging regressions found from real exported
network packages:

- Bare path filters must keep regular prefix semantics so `/account` matches
  `/account-center/...`; callers that need path-segment boundaries should use
  regex filters.
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
