# Headless Desktop recovery regressions

This test-only host compiles the production modules directly:

- `../src/backend_runtime/recovery.rs`: lifecycle guard, identity snapshots, guarded termination, automatic replacement and retry results
- `../src/backend_runtime/watchdog_policy.rs`: availability classification and bounded recovery budget
- `../src/backend_runtime/port_retry.rs`: launch policy and ownership-preserving failure cleanup
- `../src/tests/recovery_races.rs`: the same regression source included in the native Desktop test suite

Run from the repository root:

```sh
cargo test --manifest-path desktop/src-tauri/recovery-tests/Cargo.toml
cargo clippy --manifest-path desktop/src-tauri/recovery-tests/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path desktop/src-tauri/recovery-tests/Cargo.toml --all -- --check
```

The host replaces only Tauri error/UI plumbing and launch/readiness dependencies.
Barrier tests use short-lived owned `sleep` children in temporary directories;
replacement results use injected launch closures. No Bifrost service is started,
no production proxy port is used, and no OS proxy settings are changed. Tests
use empty temporary ownership state so the real core generation API cannot write
OS settings. Temporary children are killed and reaped by each test.

Regression coverage includes a completed manual replacement between a watchdog
probe and its action, changed PID/start identity/epoch/ports/runtime markers,
foreign-runtime bind races, shutdown during a slow launch, failed-launch retry
scheduling, circuit half-open timing, and unavailable Admin/data-plane signals
without trustworthy scheduler metadata.

This host does not replace native Desktop validation. Run the native Desktop
suite and platform smoke tests on a machine with Tauri's platform dependencies.
In particular, the actual GUI shutdown coordinator, macOS proxy service writes,
health HTTP probes and real CLI startup handshake require their own integration
checks. A headless pass must not be described as a full Desktop build pass.

## Recovery contract

A slow probe is scoped to one Desktop lifecycle epoch, owned child PID/start
identity, active/preferred ports, runtime/PID marker contents and proxy lease.
The guard is acquired again and that snapshot is checked before termination;
the same guard covers termination, replacement and publication. A failed kill
retains the child handle. Manual startup/rebind and shutdown invalidate old
observations. Shutdown cancels retry scheduling and owns any child that finishes
launching while shutdown is pending.

Automatic replacement stays on the last active port, has null stdin and never
uses directory-wide stop/cleanup. It passes
`BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL` to the CLI: a captured lease
limits OS changes to that generation; an empty token forbids fresh ownership.
Manual startup removes the token. If old-runtime cleanup removes a lease or its
markers, a pending owned-child retry may continue without claiming a new lease.
A different generation or a foreign runtime/PID marker cancels it.

A failed replacement schedules another attempt after the retry delay; the
rolling budget opens a circuit and schedules a half-open attempt after cooldown.
The existing core generation API handles fail-open suspension. It remains
responsible for rejecting changed OS proxy ownership. Admin and data-plane
failure counts as unavailable even when scheduler metadata is missing or belongs
to another PID; such evidence never by itself authorizes killing the child.
