# Transparent Response Headers

## Module

This document covers the proxy response forwarding path when no response body rule, response script, DevTools bridge injection, badge injection, or trailer injection needs to rewrite the body.

## Problem

A no-rule request can still show different `original_response_headers` and `response_headers` in Traffic. Request `586` exposed this with a plain HTTP response whose upstream returned:

- `content-type: application/json`
- `content-length: 168`
- `connection: close`

The request had `has_rule_hit=false`, but the stored final response headers dropped `content-length`. The root cause was the skip-body-processing branch always normalizing as an unknown-length stream, so it removed `Content-Length` even when the upstream length was known and the body was forwarded unchanged.

## Implementation

- Keep the existing `normalize_res_headers` behavior for unknown streams, trailers, no-body statuses, and fully buffered body rewrites.
- Add a small selection helper for streaming responses:
  - if trailers are being added, use `StreamWithTrailers`;
  - if the final response headers still contain a parseable `Content-Length`, use `StreamWithLength`;
  - otherwise use `Stream`.
- The length is computed after response header rules are applied, so explicit header deletion or invalid length continues to fall back to unknown streaming.
- No relay, admin API, storage schema, or WebUI changes are required.

## Dependencies

- `crates/bifrost-proxy/src/proxy/http/handler.rs`
- `crates/bifrost-proxy/src/proxy/http/body_metadata.rs` (`BodyMode::StreamWithLength` and `normalize_res_headers`)
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` (same selection helper applied on the HTTPS tunnel path)
- Existing Traffic recording fields: `original_response_headers`, `response_headers`, `has_rule_hit`

## Test Plan

- Unit tests:
  - `test_streaming_res_body_mode_preserves_known_length_without_trailers` verifies mode selection prefers `StreamWithLength` only when safe.
  - `test_normalize_res_headers_preserves_stream_content_length_when_known` verifies known-length streaming removes `Transfer-Encoding` and preserves the final `Content-Length`.
  - `test_normalize_res_headers_removes_content_length_for_unknown_stream` verifies unknown streaming still removes `Content-Length`.
- E2E tests:
  - `e2e-tests/tests/test_no_rule_content_length_transparency.sh` starts a local origin with an explicit `Content-Length`, starts Bifrost with an isolated data dir and `--no-system-proxy`, sends a no-rule request through the proxy, and asserts:
    - client response keeps `Content-Length`;
    - Traffic detail has `has_rule_hit=false`;
    - original response headers contain `content-length`;
    - final response headers are either not stored as modified headers, or contain the same `content-length` if stored.
- Human tests:
  - `human_tests/proxy-rules-advanced.md` adds `TC-PRA-61` for the same no-rule transparency regression.

## Validation Requirements

- Run the targeted proxy unit tests.
- Run `bash e2e-tests/tests/test_no_rule_content_length_transparency.sh`.
- Execute `TC-PRA-61` from `human_tests/proxy-rules-advanced.md` and record the result.
- Before commit, run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, and the local CI check appropriate for proxy Rust changes.
