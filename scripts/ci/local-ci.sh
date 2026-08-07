#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SKIP_E2E=0
SKIP_STATIC=0
SKIP_DEPS_AUDIT=0
SKIP_CHANGED_COVERAGE=0
E2E_ONLY=""
RUN_COVERAGE=0
COVERAGE_FORMAT="text"
COVERAGE_GATE=0
COVERAGE_FAIL_UNDER=90
SHARD_SPEC=""

raise_fd_limit() {
  local target="${BIFROST_LOCAL_CI_FD_LIMIT:-4096}"
  local current
  current="$(ulimit -n 2>/dev/null || echo 0)"
  if [[ "$current" =~ ^[0-9]+$ && "$current" -lt "$target" ]]; then
    ulimit -n "$target" 2>/dev/null || true
  fi
}

raise_fd_limit

usage() {
  cat <<'EOF'
Usage: scripts/ci/local-ci.sh [options]

Local CI validation script — mirrors the GitHub Actions CI pipeline.
Run this before pushing to avoid CI failures.

Options:
  --skip-e2e          Skip all E2E tests (only run fmt/clippy/test)
  --skip-static       Skip fmt/clippy/test (only run E2E)
  --skip-deps-audit   Skip Rust dependency audit (cargo-deny + cargo-udeps)
  --skip-changed-coverage
                      Skip the changed production Rust 95% local preflight
  --e2e-only TYPE     Run only a specific E2E suite: rules, shell, runner, platform
  --shard N/M         Run only shard N of M for shell E2E (e.g. --shard 1/3)
  --coverage          Run unit-test coverage report after tests
  --coverage-html     Run unit-test coverage and open HTML report
  --coverage-gate     Run unified unit+integration coverage gate (fails below 90% line coverage)
  -h, --help          Show this help

Examples:
  scripts/ci/local-ci.sh                    # Run everything
  scripts/ci/local-ci.sh --skip-e2e         # Quick static checks + unit tests
  scripts/ci/local-ci.sh --e2e-only rules   # Only E2E rules suite
  scripts/ci/local-ci.sh --coverage         # Run everything + coverage report
  scripts/ci/local-ci.sh --coverage-html    # Run everything + HTML coverage
  scripts/ci/local-ci.sh --coverage-gate    # Enforce 90% line-coverage gate
  scripts/ci/local-ci.sh --skip-changed-coverage # Only for a documented platform/E2E exception
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-e2e)
      SKIP_E2E=1
      shift
      ;;
    --skip-static)
      SKIP_STATIC=1
      shift
      ;;
    --skip-deps-audit)
      SKIP_DEPS_AUDIT=1
      shift
      ;;
    --skip-changed-coverage)
      SKIP_CHANGED_COVERAGE=1
      shift
      ;;
    --e2e-only)
      E2E_ONLY="$2"
      shift 2
      ;;
    --shard)
      if [[ -z "${2:-}" || ! "$2" =~ ^[0-9]+/[0-9]+$ ]]; then
        echo "Error: --shard requires N/M format (e.g. --shard 1/3)" >&2
        exit 1
      fi
      SHARD_SPEC="$2"
      shift 2
      ;;
    --coverage)
      RUN_COVERAGE=1
      COVERAGE_FORMAT="text"
      shift
      ;;
    --coverage-html)
      RUN_COVERAGE=1
      COVERAGE_FORMAT="html"
      shift
      ;;
    --coverage-gate)
      RUN_COVERAGE=1
      COVERAGE_GATE=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
declare -a RESULTS=()

register_result() {
  local name="$1" status="$2"
  RESULTS+=("${status}|${name}")
  case "$status" in
    PASS) ((PASS_COUNT += 1)) ;;
    FAIL) ((FAIL_COUNT += 1)) ;;
    SKIP) ((SKIP_COUNT += 1)) ;;
  esac
}

step() {
  echo ""
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
  echo -e "${BLUE}  $1${NC}"
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

run_step() {
  local name="$1"
  shift
  step "$name"
  if "$@"; then
    echo -e "${GREEN}✓ PASS: ${name}${NC}"
    register_result "$name" "PASS"
    return 0
  else
    echo -e "${RED}✗ FAIL: ${name}${NC}"
    register_result "$name" "FAIL"
    return 1
  fi
}

print_report() {
  echo ""
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
  echo -e "${BLUE}  Local CI Report${NC}"
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
  echo ""

  for entry in "${RESULTS[@]}"; do
    local status="${entry%%|*}"
    local name="${entry#*|}"
    case "$status" in
      PASS) echo -e "  ${GREEN}✓${NC} $name" ;;
      FAIL) echo -e "  ${RED}✗${NC} $name" ;;
      SKIP) echo -e "  ${YELLOW}○${NC} $name (skipped)" ;;
    esac
  done

  echo ""
  echo -e "  Total: $((PASS_COUNT + FAIL_COUNT + SKIP_COUNT))  ${GREEN}Passed: ${PASS_COUNT}${NC}  ${RED}Failed: ${FAIL_COUNT}${NC}  ${YELLOW}Skipped: ${SKIP_COUNT}${NC}"
  echo ""

  if [[ "$FAIL_COUNT" -gt 0 ]]; then
    echo -e "${RED}CI validation FAILED — do NOT push until all checks pass.${NC}"
    return 1
  else
    echo -e "${GREEN}CI validation PASSED — safe to push.${NC}"
    return 0
  fi
}

HAD_FAILURE=0
NEEDS_RELEASE_BUILD=0

if [[ "$SKIP_E2E" -eq 0 ]]; then
  if [[ -z "$E2E_ONLY" || "$E2E_ONLY" == "rules" || "$E2E_ONLY" == "shell" || "$E2E_ONLY" == "platform" ]]; then
    NEEDS_RELEASE_BUILD=1
  fi
fi

if [[ "$SKIP_STATIC" -eq 0 ]]; then
  run_step "E2E shell CI coverage guard" bash scripts/ci/check-e2e-shell-ci-coverage.sh || HAD_FAILURE=1
  run_step "cargo fmt (workspace)" cargo fmt --all -- --check || HAD_FAILURE=1
  run_step "cargo fmt (desktop)" cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check || HAD_FAILURE=1
  if [[ "$SKIP_CHANGED_COVERAGE" -eq 0 ]]; then
    run_step "Changed production Rust coverage (95%)" make coverage-changed || HAD_FAILURE=1
  else
    register_result "Changed production Rust coverage (95%)" "SKIP"
  fi
  run_step "cargo clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings || HAD_FAILURE=1
  run_step "cargo test (workspace)" cargo test --workspace --all-features || HAD_FAILURE=1
  if [[ "$SKIP_DEPS_AUDIT" -eq 0 ]]; then
    run_step "Rust dependency audit" bash scripts/ci/rust-dependency-audit.sh || HAD_FAILURE=1
  else
    register_result "Rust dependency audit" "SKIP"
  fi
else
  register_result "cargo fmt (workspace)" "SKIP"
  register_result "cargo fmt (desktop)" "SKIP"
  register_result "Changed production Rust coverage (95%)" "SKIP"
  register_result "cargo clippy" "SKIP"
  register_result "cargo test (workspace)" "SKIP"
  register_result "Rust dependency audit" "SKIP"
fi

if [[ "$NEEDS_RELEASE_BUILD" -eq 1 ]]; then
  run_step "cargo build (release bifrost)" env SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost || HAD_FAILURE=1
else
  register_result "cargo build (release bifrost)" "SKIP"
fi

if [[ "$SKIP_E2E" -eq 0 ]]; then
  if [[ -z "$E2E_ONLY" || "$E2E_ONLY" == "rules" ]]; then
    run_step "E2E rules" bash scripts/ci/run-e2e-rules.sh || HAD_FAILURE=1
  else
    register_result "E2E rules" "SKIP"
  fi

  if [[ -z "$E2E_ONLY" || "$E2E_ONLY" == "shell" ]]; then
    if [[ -n "$SHARD_SPEC" ]]; then
      run_step "E2E shell (shard ${SHARD_SPEC})" \
        env "BIFROST_E2E_SHARD_INDEX=${SHARD_SPEC%%/*}" "BIFROST_E2E_SHARD_TOTAL=${SHARD_SPEC##*/}" \
        bash scripts/ci/run-e2e-shell.sh || HAD_FAILURE=1
    else
      run_step "E2E shell" bash scripts/ci/run-e2e-shell.sh || HAD_FAILURE=1
    fi
  else
    register_result "E2E shell" "SKIP"
  fi

  if [[ -z "$E2E_ONLY" || "$E2E_ONLY" == "runner" ]]; then
    run_step "E2E runner" bash scripts/ci/run-e2e-runner.sh || HAD_FAILURE=1
  else
    register_result "E2E runner" "SKIP"
  fi

  if [[ -z "$E2E_ONLY" || "$E2E_ONLY" == "platform" ]]; then
    run_step "E2E platform" bash scripts/ci/run-e2e-platform.sh || HAD_FAILURE=1
  else
    register_result "E2E platform" "SKIP"
  fi
else
  register_result "E2E rules" "SKIP"
  register_result "E2E shell" "SKIP"
  register_result "E2E runner" "SKIP"
  register_result "E2E platform" "SKIP"
fi

if [[ "$RUN_COVERAGE" -eq 1 ]]; then
  if [[ "$COVERAGE_GATE" -eq 1 ]]; then
    run_step "Coverage gate (unit+integration, ≥${COVERAGE_FAIL_UNDER}%)" \
      bash scripts/ci/coverage-all.sh --text --fail-under "$COVERAGE_FAIL_UNDER" ||
      HAD_FAILURE=1
  else
    COV_ARGS=()
    if [[ "$COVERAGE_FORMAT" == "html" ]]; then
      COV_ARGS+=(--open)
    fi
    run_step "Unit-test coverage" bash scripts/ci/coverage.sh "--$COVERAGE_FORMAT" "${COV_ARGS[@]}" || HAD_FAILURE=1
  fi
else
  register_result "Unit-test coverage" "SKIP"
fi

print_report
exit $HAD_FAILURE
