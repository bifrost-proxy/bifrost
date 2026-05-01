# SOCKS5 UDP E2E Stability

## Module

`e2e-tests/tests/test_socks5_udp.sh` and `test_socks5_udp_rules.sh` validate SOCKS5 UDP ASSOCIATE behavior and UDP rule routing.

## Problem

CI can run shell E2E suites with high parallelism. The SOCKS5 UDP scripts used a fixed `sleep 5` after spawning Bifrost and then only checked that the process was still alive. On slower runners, the process can be alive while the admin API or SOCKS5 listener is not ready yet. The first UDP ASSOCIATE test then connects immediately and fails with `Connection refused`.

The scripts also used repository-root fixed data directories. In parallel or retried runs this increases the risk of stale runtime files and makes failures harder to diagnose.

## Implementation

- Use `BIFROST_DATA_DIR` when the runner provides one, falling back to the old local path only for direct manual execution.
- After spawning Bifrost, poll both readiness surfaces:
  - admin API: `/_bifrost/api/system`
  - SOCKS5 TCP listener: `PROXY_HOST:SOCKS5_PORT`
- If readiness times out, print the proxy log before failing.
- Clean both HTTP proxy and SOCKS5 listener ports during teardown.
- Apply the same readiness gate to the rules variant after both initial start and restart.

## Validation Plan

- Unit tests: not applicable; this is shell E2E harness behavior.
- E2E tests:
  - `BIFROST_DATA_DIR=<tmp> PROXY_PORT=<free> SOCKS5_PORT=<free> bash e2e-tests/tests/test_socks5_udp.sh`
  - `BIFROST_DATA_DIR=<tmp> PROXY_PORT=<free> SOCKS5_PORT=<free> bash e2e-tests/tests/test_socks5_udp_rules.sh`
- Human test:
  - Update `human_tests/proxy-socks5.md` with a SOCKS5 UDP readiness regression case and execute it by running the corresponding E2E script with isolated ports and a temporary data directory.

## CI Impact

This removes the timing race where an alive Bifrost process is treated as ready before the SOCKS5 listener is bound. Failures should now include actionable proxy logs instead of a bare Python `Connection refused`.

## Related Frames API Harness Cleanup

The same CI artifact also showed `test_frames_admin_api.sh` printing a failed SSE traffic-generation message while still reporting the suite as passed. That is a probabilistic success risk because SSE-dependent assertions are skipped after setup failure.

The frames harness now:

- retries proxied SSE generation up to 10 times with a longer stream timeout;
- exits the suite if required WebSocket or available-SSE setup traffic cannot be generated;
- only skips SSE-dependent checks when the local SSE fixture itself failed to start.

Validation:

- `bash e2e-tests/tests/test_frames_admin_api.sh`
- human test `TC-PWS-08` in `human_tests/proxy-websocket-sse.md`
