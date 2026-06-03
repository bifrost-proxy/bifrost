#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

#
# End-to-end test coverage for Bifrost.
#
# Strategy:
#   1. Build the `bifrost` binary with LLVM coverage instrumentation
#   2. Build the `bifrost-e2e` runner with LLVM coverage instrumentation
#   3. Run the E2E test suites — both the instrumented bifrost server and
#      the instrumented E2E runner produce .profraw files
#   4. Merge all .profraw files and generate a unified coverage report
#
# This captures which code paths in both the proxy server and the test
# framework are actually exercised during E2E testing.
#
# Prerequisites:
#   cargo install cargo-llvm-cov
#   rustup component add llvm-tools-preview
#
# Usage:
#   bash scripts/ci/coverage-e2e.sh [options]
#
# Options:
#   --html             Generate HTML report
#   --lcov             Generate LCOV report
#   --json             Generate JSON summary
#   --fail-under PCT   Fail if line coverage < PCT%
#   --open             Open HTML report in browser
#   --output-dir DIR   Output directory (default: target/coverage-e2e)
#   --suite SUITE      Run specific E2E suite: rules, shell, runner, platform (default: all)
#   --skip-build       Skip instrumented build (reuse existing)
#   -h, --help         Show this help

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

FORMAT="text"
FAIL_UNDER=0
OPEN_REPORT=0
OUTPUT_DIR="target/coverage-e2e"
SUITE=""
SKIP_BUILD=0
PROFRAW_DIR=""

usage() {
  sed -n '2,/^$/s/^# \?//p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --html)       FORMAT="html"; shift ;;
    --lcov)       FORMAT="lcov"; shift ;;
    --json)       FORMAT="json"; shift ;;
    --fail-under) FAIL_UNDER="$2"; shift 2 ;;
    --open)       FORMAT="html"; OPEN_REPORT=1; shift ;;
    --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
    --suite)      SUITE="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help)    usage; exit 0 ;;
    *)            echo -e "${RED}Unknown option: $1${NC}" >&2; usage; exit 1 ;;
  esac
done

ensure_cargo_llvm_cov() {
  if ! command -v cargo-llvm-cov &>/dev/null; then
    echo -e "${YELLOW}cargo-llvm-cov not found. Installing...${NC}"
    cargo install cargo-llvm-cov
  fi

  if ! rustup component list --installed 2>/dev/null | grep -q llvm-tools; then
    echo -e "${YELLOW}llvm-tools-preview not found. Installing...${NC}"
    rustup component add llvm-tools-preview
  fi
}

step() {
  echo ""
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
  echo -e "${BLUE}  $1${NC}"
  echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

resolve_llvm_tool() {
  local tool_name="$1"
  local llvm_tools_dir

  llvm_tools_dir="$(cargo llvm-cov show-env 2>/dev/null | grep LLVM_COV | head -1 | sed 's/.*="\(.*\)"/\1/' | xargs dirname 2>/dev/null || true)"

  if [[ -n "$llvm_tools_dir" && -x "$llvm_tools_dir/$tool_name" ]]; then
    echo "$llvm_tools_dir/$tool_name"
    return
  fi

  if command -v "$tool_name" &>/dev/null; then
    command -v "$tool_name"
    return
  fi

  echo "$tool_name"
}

build_instrumented_binaries() {
  step "Building instrumented binaries"

  export CARGO_INCREMENTAL=0
  export RUSTFLAGS="-C instrument-coverage"
  export LLVM_PROFILE_FILE="$PROFRAW_DIR/bifrost-%p-%m.profraw"

  echo -e "${BLUE}Building bifrost (instrumented)...${NC}"
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost

  echo -e "${BLUE}Building bifrost-e2e (instrumented)...${NC}"
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost-e2e

  unset RUSTFLAGS

  echo -e "${GREEN}Instrumented binaries built successfully${NC}"
  echo "  bifrost:     target/release/bifrost"
  echo "  bifrost-e2e: target/release/bifrost-e2e"
}

run_e2e_suites() {
  step "Running E2E test suites with coverage instrumentation"

  export LLVM_PROFILE_FILE="$PROFRAW_DIR/e2e-%p-%m.profraw"

  local data_dir="$OUTPUT_DIR/.bifrost-data"
  mkdir -p "$data_dir"
  export BIFROST_DATA_DIR="$data_dir"

  local run_all=0
  if [[ -z "$SUITE" ]]; then
    run_all=1
  fi

  local had_failure=0

  if [[ "$run_all" -eq 1 || "$SUITE" == "rules" ]]; then
    echo -e "${BLUE}Running E2E rules suite...${NC}"
    bash scripts/ci/run-e2e-rules.sh || had_failure=1
  fi

  if [[ "$run_all" -eq 1 || "$SUITE" == "shell" ]]; then
    echo -e "${BLUE}Running E2E shell suite...${NC}"
    bash scripts/ci/run-e2e-shell.sh || had_failure=1
  fi

  if [[ "$run_all" -eq 1 || "$SUITE" == "runner" ]]; then
    echo -e "${BLUE}Running E2E runner suite...${NC}"
    bash scripts/ci/run-e2e-runner.sh || had_failure=1
  fi

  if [[ "$run_all" -eq 1 || "$SUITE" == "platform" ]]; then
    echo -e "${BLUE}Running E2E platform suite...${NC}"
    bash scripts/ci/run-e2e-platform.sh || had_failure=1
  fi

  if [[ "$had_failure" -ne 0 ]]; then
    echo -e "${YELLOW}Some E2E suites had failures, but coverage data was still collected.${NC}"
  fi

  unset LLVM_PROFILE_FILE
}

merge_and_report() {
  step "Generating coverage report"

  local profraw_count
  profraw_count="$(find "$PROFRAW_DIR" -name '*.profraw' 2>/dev/null | wc -l | tr -d ' ')"
  echo -e "${BLUE}Found $profraw_count .profraw files${NC}"

  if [[ "$profraw_count" -eq 0 ]]; then
    echo -e "${RED}No .profraw files found. Coverage data was not generated.${NC}"
    echo -e "${YELLOW}Check that LLVM_PROFILE_FILE was set correctly during test execution.${NC}"
    return 1
  fi

  local llvm_profdata
  local llvm_cov
  llvm_profdata="$(resolve_llvm_tool llvm-profdata)"
  llvm_cov="$(resolve_llvm_tool llvm-cov)"

  local profdata_path="$OUTPUT_DIR/e2e.profdata"

  echo -e "${BLUE}Merging profile data...${NC}"
  "$llvm_profdata" merge -sparse "$PROFRAW_DIR"/*.profraw -o "$profdata_path"

  local bifrost_bin="target/release/bifrost"
  local e2e_bin="target/release/bifrost-e2e"

  local -a object_args=()
  [[ -x "$bifrost_bin" ]] && object_args+=("-object=$bifrost_bin")
  [[ -x "$e2e_bin" ]] && object_args+=("-object=$e2e_bin")

  if [[ ${#object_args[@]} -eq 0 ]]; then
    echo -e "${RED}No instrumented binaries found.${NC}"
    return 1
  fi

  local -a ignore_patterns=(
    "--ignore-filename-regex=.cargo/registry"
    "--ignore-filename-regex=rustc/"
    "--ignore-filename-regex=crates/bifrost-e2e/"
  )

  case "$FORMAT" in
    html)
      mkdir -p "$OUTPUT_DIR/html"
      echo -e "${BLUE}Generating HTML report...${NC}"
      "$llvm_cov" show \
        "${object_args[@]}" \
        "-instr-profile=$profdata_path" \
        "${ignore_patterns[@]}" \
        -format=html \
        -output-dir="$OUTPUT_DIR/html" \
        -show-line-counts-or-regions \
        -show-instantiations=false

      echo -e "${GREEN}HTML report generated at: $OUTPUT_DIR/html/index.html${NC}"
      if [[ "$OPEN_REPORT" -eq 1 ]]; then
        open "$OUTPUT_DIR/html/index.html" 2>/dev/null || xdg-open "$OUTPUT_DIR/html/index.html" 2>/dev/null || true
      fi
      ;;
    lcov)
      mkdir -p "$OUTPUT_DIR"
      echo -e "${BLUE}Generating LCOV report...${NC}"
      "$llvm_cov" export \
        "${object_args[@]}" \
        "-instr-profile=$profdata_path" \
        "${ignore_patterns[@]}" \
        -format=lcov \
        > "$OUTPUT_DIR/lcov.info"

      echo -e "${GREEN}LCOV report generated at: $OUTPUT_DIR/lcov.info${NC}"
      ;;
    json)
      "$llvm_cov" export \
        "${object_args[@]}" \
        "-instr-profile=$profdata_path" \
        "${ignore_patterns[@]}" \
        -format=text \
        -summary-only
      ;;
    text)
      "$llvm_cov" report \
        "${object_args[@]}" \
        "-instr-profile=$profdata_path" \
        "${ignore_patterns[@]}"
      ;;
  esac

  if [[ "$FAIL_UNDER" -gt 0 ]]; then
    echo ""
    echo -e "${BLUE}Checking coverage threshold (${FAIL_UNDER}%)...${NC}"

    local summary
    summary="$("$llvm_cov" export \
      "${object_args[@]}" \
      "-instr-profile=$profdata_path" \
      "${ignore_patterns[@]}" \
      -format=text \
      -summary-only 2>/dev/null)"

    local covered
    local total
    local pct

    covered="$(echo "$summary" | python3 -c "
import json, sys
d = json.load(sys.stdin)
t = d['data'][0]['totals']['lines']
print(t['covered'])
" 2>/dev/null || echo "0")"

    total="$(echo "$summary" | python3 -c "
import json, sys
d = json.load(sys.stdin)
t = d['data'][0]['totals']['lines']
print(t['count'])
" 2>/dev/null || echo "1")"

    if [[ "$total" -eq 0 ]]; then
      pct=0
    else
      pct=$(( (covered * 100) / total ))
    fi

    if [[ "$pct" -lt "$FAIL_UNDER" ]]; then
      echo -e "${RED}Coverage ${pct}% is below threshold ${FAIL_UNDER}%${NC}"
      return 1
    else
      echo -e "${GREEN}Coverage ${pct}% meets threshold ${FAIL_UNDER}%${NC}"
    fi
  fi
}

main() {
  step "Bifrost E2E Coverage"

  ensure_cargo_llvm_cov

  mkdir -p "$OUTPUT_DIR"
  PROFRAW_DIR="$OUTPUT_DIR/profraw"
  mkdir -p "$PROFRAW_DIR"

  echo -e "${BLUE}Format     :${NC} $FORMAT"
  echo -e "${BLUE}Fail-under :${NC} ${FAIL_UNDER}%"
  echo -e "${BLUE}Output dir :${NC} $OUTPUT_DIR"
  echo -e "${BLUE}Suite      :${NC} ${SUITE:-all}"
  echo -e "${BLUE}Profraw dir:${NC} $PROFRAW_DIR"

  if [[ "$SKIP_BUILD" -eq 0 ]]; then
    build_instrumented_binaries
  else
    echo -e "${YELLOW}Skipping build (using existing instrumented binaries)${NC}"
  fi

  run_e2e_suites

  merge_and_report
}

main
