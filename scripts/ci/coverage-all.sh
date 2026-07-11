#!/usr/bin/env bash
#
# Unified coverage for the Bifrost workspace.
#
# Produces truthful per-layer reports plus a merged report:
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
#   * The unit+integration profile is snapshotted before E2E starts.
#   * If --with-e2e is passed, debug binaries are built in the same llvm-cov
#     target/profile as the unit tests and passed explicitly to the E2E harness.
#     Keeping one codegen profile is essential: release and debug coverage
#     counters cannot be truthfully merged.
#   * Reports are emitted as unit-integration.json, e2e.json, and coverage.json
#     (the union used by the gate).
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
#   --e2e-suite NAME   Limit E2E to rules, shell, or runner (requires --with-e2e)
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

raise_fd_limit() {
  local target="${BIFROST_COVERAGE_FD_LIMIT:-4096}"
  local current
  current="$(ulimit -n 2>/dev/null || echo 0)"
  if [[ "$current" =~ ^[0-9]+$ && "$current" -lt "$target" ]]; then
    ulimit -n "$target" 2>/dev/null || true
  fi
}

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
E2E_SUITE="${BIFROST_COVERAGE_E2E_SUITE:-}"
ORIGINAL_HOME="${HOME:-}"
ORIGINAL_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-}"
ORIGINAL_XDG_DATA_HOME="${XDG_DATA_HOME:-}"
ORIGINAL_BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-}"
PROFILE_ROOT="$ROOT_DIR/target/llvm-cov-target"

usage() { sed -n '2,/^$/s/^# \?//p' "$0"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --json)       WANT_JSON=1; shift ;;
    --lcov)       WANT_LCOV=1; shift ;;
    --html)       WANT_HTML=1; shift ;;
    --text)       WANT_TEXT=1; shift ;;
    --with-e2e)   WITH_E2E=1; shift ;;
    --e2e-suite)  E2E_SUITE="$2"; shift 2 ;;
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

if [[ -n "$E2E_SUITE" ]]; then
  case "$E2E_SUITE" in
    rules|shell|runner) ;;
    *) echo -e "${RED}Unknown E2E suite: $E2E_SUITE${NC}" >&2; exit 2 ;;
  esac
  if [[ "$WITH_E2E" -ne 1 ]]; then
    echo -e "${RED}--e2e-suite requires --with-e2e${NC}" >&2
    exit 2
  fi
fi

if [[ "$WITH_E2E" -eq 1 && -n "$PACKAGE" ]]; then
  echo -e "${RED}--with-e2e cannot be combined with --package; E2E exercises the workspace binary${NC}" >&2
  exit 2
fi

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
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY
export SKIP_FRONTEND_BUILD=1

SCOPE_ARGS=()
if [[ -n "$PACKAGE" ]]; then
  SCOPE_ARGS=(--package "$PACKAGE")
else
  SCOPE_ARGS=(--workspace)
fi

snapshot_profiles() {
  local destination="$1"
  mkdir -p "$destination"
  local profile
  local found=0
  shopt -s nullglob
  for profile in "$PROFILE_ROOT"/*.profraw; do
    mv "$profile" "$destination/"
    found=1
  done
  shopt -u nullglob
  if [[ "$found" -ne 1 ]]; then
    echo -e "${RED}No coverage profiles were produced${NC}" >&2
    return 1
  fi
}

restore_profiles() {
  local source_dir="$1"
  local profile
  shopt -s nullglob
  for profile in "$source_dir"/*.profraw; do
    cp "$profile" "$PROFILE_ROOT/"
  done
  shopt -u nullglob
}

prepare_isolated_e2e_environment() {
  local data_dir="$OUTPUT_DIR/.bifrost-data"
  local home_dir="$OUTPUT_DIR/home"
  local data_abs
  local production_abs=""

  mkdir -p "$data_dir" "$home_dir/xdg-config" "$home_dir/xdg-data"
  data_abs="$(cd "$data_dir" && pwd -P)"
  if [[ -n "$ORIGINAL_HOME" && -d "$ORIGINAL_HOME" ]]; then
    production_abs="$(cd "$ORIGINAL_HOME" && pwd -P)/.bifrost"
  fi
  if [[ -n "$production_abs" \
      && ( "$data_abs" == "$production_abs" || "$data_abs" == "$production_abs/"* ) ]]; then
    echo -e "${RED}REFUSING: coverage E2E data directory is under production data: $data_abs${NC}" >&2
    return 1
  fi

  export BIFROST_DATA_DIR="$data_abs"
  export HOME="$(cd "$home_dir" && pwd -P)"
  export XDG_CONFIG_HOME="$HOME/xdg-config"
  export XDG_DATA_HOME="$HOME/xdg-data"
  export BIFROST_E2E_PROTECTED_PORTS="${BIFROST_E2E_PROTECTED_PORTS:-9900}"
  echo -e "${BLUE}E2E data :${NC} $BIFROST_DATA_DIR"
  echo -e "${BLUE}E2E home :${NC} $HOME"
  echo -e "${BLUE}Protected:${NC} $BIFROST_E2E_PROTECTED_PORTS"
}

restore_tool_environment() {
  export HOME="$ORIGINAL_HOME"
  if [[ -n "$ORIGINAL_XDG_CONFIG_HOME" ]]; then
    export XDG_CONFIG_HOME="$ORIGINAL_XDG_CONFIG_HOME"
  else
    unset XDG_CONFIG_HOME
  fi
  if [[ -n "$ORIGINAL_XDG_DATA_HOME" ]]; then
    export XDG_DATA_HOME="$ORIGINAL_XDG_DATA_HOME"
  else
    unset XDG_DATA_HOME
  fi
  if [[ -n "$ORIGINAL_BIFROST_DATA_DIR" ]]; then
    export BIFROST_DATA_DIR="$ORIGINAL_BIFROST_DATA_DIR"
  else
    unset BIFROST_DATA_DIR
  fi
}

run_selected_e2e() {
  local -a suites=(rules shell runner)
  if [[ -n "$E2E_SUITE" ]]; then
    suites=("$E2E_SUITE")
  fi

  local suite
  local status=0
  for suite in "${suites[@]}"; do
    echo -e "${BLUE}-> run-e2e-${suite}${NC}"
    bash "scripts/ci/run-e2e-${suite}.sh" || status=$?
  done
  return "$status"
}

main() {
  step "Bifrost Unified Coverage"
  raise_fd_limit
  ensure_tooling
  mkdir -p "$OUTPUT_DIR"

  echo -e "${BLUE}Scope    :${NC} ${PACKAGE:-workspace}"
  echo -e "${BLUE}E2E      :${NC} $([[ $WITH_E2E -eq 1 ]] && echo yes || echo no)"
  echo -e "${BLUE}E2E suite:${NC} ${E2E_SUITE:-all}"
  echo -e "${BLUE}Jobs     :${NC} $JOBS"
  echo -e "${BLUE}Output   :${NC} $OUTPUT_DIR"

  # 1. Clean previous profile data so the merge is deterministic.
  step "Resetting previous coverage profile data"
  cargo llvm-cov clean --workspace

  # 2. Unit + integration tests, keep raw profiles (--no-report).
  step "Running unit + integration tests with instrumentation"
  cargo llvm-cov "${SCOPE_ARGS[@]}" --all-features --no-report --jobs "$JOBS"

  if [[ "$WANT_JSON" -eq 1 ]]; then
    cargo llvm-cov report --json --output-path "$OUTPUT_DIR/unit-integration.json"
    echo -e "${GREEN}Unit+integration JSON : $OUTPUT_DIR/unit-integration.json${NC}"
  fi

  # 3. Optional E2E: build and run binaries with the exact same instrumentation
  # profile used by the unit/integration tests. Mixing release E2E profiles with
  # debug unit profiles produces incompatible counters and a false zero-percent
  # E2E layer.
  local e2e_status=0
  if [[ "$WITH_E2E" -eq 1 ]]; then
    local unit_profiles="$OUTPUT_DIR/profiles/unit-integration"
    snapshot_profiles "$unit_profiles"

    step "Building E2E binaries with llvm-cov instrumentation"
    eval "$(cargo llvm-cov show-env --sh)"
    export CARGO_TARGET_DIR="$PROFILE_ROOT"
    export CARGO_LLVM_COV_TARGET_DIR="$PROFILE_ROOT"
    export CARGO_LLVM_COV_BUILD_DIR="$PROFILE_ROOT"
    export LLVM_PROFILE_FILE="$PROFILE_ROOT/bifrost-e2e-%p-%16m.profraw"
    cargo build --bin bifrost --bin bifrost-e2e --jobs "$JOBS"
    export BIFROST_BIN="$PROFILE_ROOT/debug/bifrost"
    export BIFROST_E2E_BIN="$PROFILE_ROOT/debug/bifrost-e2e"
    prepare_isolated_e2e_environment

    step "Running E2E suites with instrumented binaries"
    run_selected_e2e || e2e_status=$?
    restore_tool_environment

    local e2e_profiles="$OUTPUT_DIR/profiles/e2e"
    snapshot_profiles "$e2e_profiles"
    restore_profiles "$e2e_profiles"
    if [[ "$WANT_JSON" -eq 1 ]]; then
      cargo llvm-cov report --json --output-path "$OUTPUT_DIR/e2e.json"
      echo -e "${GREEN}E2E JSON : $OUTPUT_DIR/e2e.json${NC}"
    fi
    restore_profiles "$unit_profiles"
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

  if [[ "$e2e_status" -ne 0 ]]; then
    echo -e "${RED}One or more instrumented E2E suites failed (status $e2e_status)${NC}" >&2
    exit "$e2e_status"
  fi
}

main
