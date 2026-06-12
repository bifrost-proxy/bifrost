# Test Coverage Mechanism

This document describes how Bifrost measures and enforces test coverage across
the **whole test pyramid** — unit tests, integration tests, and end-to-end
(E2E) tests — and the workflow for driving every crate to the **90% line
coverage** goal.

## TL;DR

```bash
# 1. Measure everything (unit + integration), merge, emit JSON, enforce floors,
#    and print exactly where to add tests:
bash scripts/ci/coverage-all.sh --json --gate --gaps

# 2. Include E2E suites in the merged number:
bash scripts/ci/coverage-all.sh --with-e2e --json --gate

# 3. Iterate on a single crate (fast):
bash scripts/ci/coverage-all.sh -p bifrost-core --json --gaps

# 4. Open an HTML report locally:
bash scripts/ci/coverage-all.sh -p bifrost-core --html
open target/coverage/html/index.html
```

## What gets measured

| Layer | Source | How it is captured |
|---|---|---|
| **Unit tests** | `#[cfg(test)]` modules inside each crate | `cargo llvm-cov` instruments the crate and runs `cargo test` |
| **Integration tests** | `crates/bifrost-tests` → repo-root `tests/*.rs` | same `cargo llvm-cov` run (they are normal test targets) |
| **E2E tests** | `scripts/ci/run-e2e-*.sh` driving the `bifrost` + `bifrost-e2e` binaries | binaries are built **instrumented**; their `.profraw` is merged in (`--with-e2e`) |

All layers write LLVM profile data into the **same** target directory, then a
single `cargo llvm-cov report` merges them. **A line counts as covered if *any*
layer exercises it** — so the merged number reflects the real reach of the
whole test suite, which is what the 90% goal is measured against.

## The mechanism — files

| File | Role |
|---|---|
| `scripts/ci/coverage-all.sh` | **Entry point.** Collects unit + integration (+ optional E2E) coverage, merges, emits text/JSON/LCOV/HTML, optionally runs the gate. |
| `scripts/ci/coverage-gate.py` | **Gate & gap analysis.** Reads the merged JSON, maps files → crates, enforces per-crate + workspace floors, and prints prioritized gaps. |
| `scripts/ci/coverage-thresholds.toml` | **The contract.** Per-crate line-coverage *floors* (a ratchet) + the workspace goal. CI fails if coverage drops below a floor. |
| `scripts/ci/coverage.sh` | Legacy unit-only helper (still works). |
| `scripts/ci/coverage-e2e.sh` | Legacy E2E-only helper (still works). |

> Prerequisites: `rustup component add llvm-tools-preview` and
> `cargo install cargo-llvm-cov` (the scripts auto-install if missing). On
> many-core machines the instrumented linker can crash if it spawns one thread
> per CPU against a small `max locked memory` ulimit — the scripts therefore
> constrain build/link parallelism via `--jobs` (default 4). Override with
> `--jobs N` or the `COVERAGE_JOBS` env var.

## The 90% gate & ratchet

`coverage-thresholds.toml` declares:

* `settings.default` — the project goal (**90%**), used for any crate without
  an explicit floor and for the gap report's target line.
* `settings.workspace_min` — an aggregate floor across the whole workspace.
* `[crates.<name>].min` — a per-crate **floor**. The gate fails if the crate
  falls below it.

The floors act as a **ratchet**: they are seeded at the current baseline so CI
goes green today, and are raised as tests are added. A floor must never be
lowered without explicit justification — that is how the workspace converges on
90% without regressing.

## Workflow: driving a crate to 90%

1. **Find the gaps.**
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --json --gaps
   ```
   The gap report lists the files furthest below target, sorted by
   uncovered-line count (biggest wins first), plus a per-crate "investment
   priority" total.

2. **See which lines are red.** Generate HTML and open the file:
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --html
   open target/coverage/html/index.html
   ```

3. **Add targeted tests** for the uncovered branches/functions. Prefer
   behavior-driven tests over line-chasing; the merged report rewards real
   integration/E2E coverage too.

4. **Re-measure and bump the floor.**
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --json --gate
   ```
   Once the crate is ≥ 90%, set its `[crates.<crate>].min = 90.0` in
   `coverage-thresholds.toml` so it can never regress.

5. **Repeat** until every crate's floor is 90 — at which point the floors equal
   the goal and the workspace is at 90% by construction.

## CI integration

The `coverage` job in `.github/workflows/ci.yml`:

1. installs `llvm-tools-preview` + `cargo-llvm-cov`,
2. runs `scripts/ci/coverage-all.sh --json --lcov --gate --gaps`,
3. writes a Markdown summary to the GitHub job summary, and
4. uploads `coverage.json` + `lcov.info` as artifacts (consumable by Codecov,
   Coveralls, or local inspection).

The job **fails the build** if any crate or the workspace aggregate drops below
its configured floor. To raise the bar, add tests and bump the floors — never
weaken the gate to make it pass.
