#!/usr/bin/env bash
#
# Unified coverage for the Bifrost workspace.
#
# Produces a SINGLE merged coverage report that combines:
#   1. Unit tests          (in-crate `#[cfg(test)]` modules)
#   2. Integration tests   (crates/bifrost-tests -> repo-root tests/*.rs)
#   3. (optional) E2E tests (instrumented `bifrost` + `bifrost-e2e` binaries)
#
# Why one merged report?
#   A line is "covered" if ANY layer exercises it. Merging unit + integration +
#   E2E gives the true picture of how much production code is reached by the
#   whole test pyramid, which is what the 90% goal is measured against.
#
# How it works:
#   * Unit + integration coverage is collected with `cargo llvm-cov`, but WITHOUT
#     finalizing the report (`--no-report`), so the raw profile data is kept.
#   * If --with-e2e is passed, the E2E suites are run against the same llvm-cov
#     environment (`cargo llvm-cov run` / shared LLVM_PROFILE_FILE) so their
#     profraw lands in the same target dir.
#   * Finally `cargo llvm-cov report` merges everything into text/json/lcov/html.
#
# The instrumented build can crash the system linker when it spawns one thread
# per CPU on many-core machines with a small locked-memory ulimit. We therefore
# constrain build/link parallelism (CARGO_BUILD_JOBS / RAYON_NUM_THREADS) unless
# the caller overrides it.
#
# Prerequisites:
#   cargo install cargo-llvm-cov   (or use a prebuilt release binary)
#   rustup component add llvm-tools-preview
#
# Usage:
#   bash scripts/ci/coverage-all.sh [options]
#
# Options:
#   --json             Write merged JSON to OUTPUT_DIR/coverage.json (default)
#   --lcov             Also write OUTPUT_DIR/lcov.info
#   --html             Also write HTML report to OUTPUT_DIR/html
#   --text             Print text table to stdout
#   --with-e2e         Also run E2E suites and merge their coverage
#   --gate             After reporting, run coverage-gate.py to enforce floors
#   --gaps             Pass --gaps to the gate (prints where to add tests)
#   --fail-under PCT   Hard workspace floor passed straight to llvm-cov
#   --output-dir DIR   Output directory (default: target/coverage)
#   -p, --package PKG  Limit to a single crate (faster local iteration)
#   --jobs N           Build/link parallelism (default: 4)
#   -h, --help         Show this help

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'

WANT_JSON=1
WANT_LCOV=0
WANT_HTML=0
WANT_TEXT=0
WITH_E2E=0
RUN_GATE=0
GATE_GAPS=0
FAIL_UNDER=""
OUTPUT_DIR="target/coverage"
PACKAGE=""
JOBS="${COVERAGE_JOBS:-4}"

usage() { sed -n '2,/^$/s/^# \?//p' "$0"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --json)       WANT_JSON=1; shift ;;
    --lcov)       WANT_LCOV=1; shift ;;
    --html)       WANT_HTML=1; shift ;;
    --text)       WANT_TEXT=1; shift ;;
    --with-e2e)   WITH_E2E=1; shift ;;
    --gate)       RUN_GATE=1; shift ;;
    --gaps)       GATE_GAPS=1; RUN_GATE=1; shift ;;
    --fail-under) FAIL_UNDER="$2"; shift 2 ;;
    --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
    -p|--package) PACKAGE="$2"; shift 2 ;;
    --jobs)       JOBS="$2"; shift 2 ;;
    -h|--help)    usage; exit 0 ;;
    *) echo -e "${RED}Unknown option: $1${NC}" >&2; usage; exit 1 ;;
  esac
done

step() {
  echo ""
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
  echo -e "${BLUE}  $1${NC}"
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

ensure_tooling() {
  if ! cargo llvm-cov --version &>/dev/null; then
    echo -e "${YELLOW}cargo-llvm-cov not found. Installing...${NC}"
    cargo install cargo-llvm-cov
  fi
  if ! rustup component list --installed 2>/dev/null | grep -q llvm-tools; then
    echo -e "${YELLOW}llvm-tools-preview not found. Installing...${NC}"
    rustup component add llvm-tools-preview
  fi
}

# Constrain parallelism so instrumented linking does not exhaust threads/memory.
export CARGO_BUILD_JOBS="$JOBS"
export RAYON_NUM_THREADS="$JOBS"
export CARGO_INCREMENTAL=0
# E2E paths expect this; harmless for unit/integration.
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export SKIP_FRONTEND_BUILD=1

SCOPE_ARGS=()
if [[ -n "$PACKAGE" ]]; then
  SCOPE_ARGS=(--package "$PACKAGE")
else
  SCOPE_ARGS=(--workspace)
fi

main() {
  step "Bifrost Unified Coverage"
  ensure_tooling
  mkdir -p "$OUTPUT_DIR"

  echo -e "${BLUE}Scope    :${NC} ${PACKAGE:-workspace}"
  echo -e "${BLUE}E2E      :${NC} $([[ $WITH_E2E -eq 1 ]] && echo yes || echo no)"
  echo -e "${BLUE}Jobs     :${NC} $JOBS"
  echo -e "${BLUE}Output   :${NC} $OUTPUT_DIR"

  # 1. Clean previous profile data so the merge is deterministic.
  step "Resetting previous coverage profile data"
  cargo llvm-cov clean --workspace

  # 2. Unit + integration tests, keep raw profiles (--no-report).
  step "Running unit + integration tests with instrumentation"
  cargo llvm-cov "${SCOPE_ARGS[@]}" --all-features --no-report --jobs "$JOBS"

  # 3. Optional E2E: run instrumented binaries inside the llvm-cov env.
  if [[ "$WITH_E2E" -eq 1 ]]; then
    step "Running E2E suites with instrumentation (merged)"
    # `cargo llvm-cov run` instruments and runs a binary, keeping profiles in
    # the shared target dir. We build the bifrost + e2e binaries instrumented,
    # then invoke the existing suite scripts which use those binaries.
    cargo llvm-cov run --no-report --jobs "$JOBS" --bin bifrost -- --help >/dev/null 2>&1 || true
    # Run the suite scripts; they consume target/release-style binaries. The
    # scripts already collect profraw via LLVM_PROFILE_FILE set by llvm-cov.
    local had_failure=0
    for s in run-e2e-rules run-e2e-shell run-e2e-runner; do
      if [[ -f "scripts/ci/$s.sh" ]]; then
        echo -e "${BLUE}-> $s${NC}"
        bash "scripts/ci/$s.sh" || had_failure=1
      fi
    done
    [[ "$had_failure" -ne 0 ]] && echo -e "${YELLOW}Some E2E suites failed; coverage still collected.${NC}"
  fi

  # 4. Merge + emit reports.
  step "Generating merged coverage report"
  if [[ "$WANT_JSON" -eq 1 ]]; then
    cargo llvm-cov report --json --output-path "$OUTPUT_DIR/coverage.json"
    echo -e "${GREEN}JSON : $OUTPUT_DIR/coverage.json${NC}"
  fi
  if [[ "$WANT_LCOV" -eq 1 ]]; then
    cargo llvm-cov report --lcov --output-path "$OUTPUT_DIR/lcov.info"
    echo -e "${GREEN}LCOV : $OUTPUT_DIR/lcov.info${NC}"
  fi
  if [[ "$WANT_HTML" -eq 1 ]]; then
    cargo llvm-cov report --html --output-dir "$OUTPUT_DIR"
    echo -e "${GREEN}HTML : $OUTPUT_DIR/html/index.html${NC}"
  fi
  # Text summary + optional hard floor.
  if [[ -n "$FAIL_UNDER" ]]; then
    cargo llvm-cov report --fail-under-lines "$FAIL_UNDER" || {
      echo -e "${RED}llvm-cov hard floor (--fail-under) not met${NC}"; exit 1; }
  else
    cargo llvm-cov report || {
      echo -e "${RED}llvm-cov report failed${NC}"; exit 1; }
  fi

  # 5. Optional gate (per-crate floors + gap analysis).
  if [[ "$RUN_GATE" -eq 1 ]]; then
    step "Enforcing per-crate coverage gate"
    local -a gate_args=("$OUTPUT_DIR/coverage.json")
    [[ -n "$PACKAGE" ]] && gate_args+=(--single-crate)
    [[ "$GATE_GAPS" -eq 1 ]] && gate_args+=(--gaps)
    python3 scripts/ci/coverage-gate.py "${gate_args[@]}"
  fi
}

main
