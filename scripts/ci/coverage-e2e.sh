#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

#
# End-to-end test coverage for Bifrost.
#
# Strategy:
#   1. Build the `bifrost` binary with LLVM coverage instrumentation
#   2. Build the `bifrost-e2e` runner with LLVM coverage instrumentation
#   3. Run the E2E test suites — both the instrumented bifrost server and
#      the instrumented E2E runner produce .profraw files
#   4. Merge all .profraw files and generate an E2E-only coverage report
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
ORIGINAL_HOME="${HOME:-}"
ORIGINAL_CARGO_HOME="${CARGO_HOME:-${HOME:-}/.cargo}"
ORIGINAL_RUSTUP_HOME="${RUSTUP_HOME:-${HOME:-}/.rustup}"
ORIGINAL_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-}"
ORIGINAL_XDG_DATA_HOME="${XDG_DATA_HOME:-}"
ORIGINAL_BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-}"

cleanup_coverage_environment() {
  unset LLVM_PROFILE_FILE
  if [[ "${HOME:-}" == "$ROOT_DIR/.coverage-runtime/"* ]]; then
    rm -rf "$HOME"
  fi
  rmdir "$ROOT_DIR/.coverage-runtime" 2>/dev/null || true
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

trap cleanup_coverage_environment EXIT

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
  local sysroot host candidate
  sysroot="$(rustc --print sysroot)"
  host="$(rustc -vV | sed -n 's/^host: //p')"
  candidate="$sysroot/lib/rustlib/$host/bin/$tool_name"
  if [[ -x "$candidate" ]]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  command -v "$tool_name"
}

build_instrumented_binaries() {
  step "Building instrumented binaries"

  export CARGO_INCREMENTAL=0
  eval "$(cargo llvm-cov show-env --sh)"
  export LLVM_PROFILE_FILE="$PROFRAW_DIR/bifrost-%p-%m.profraw"

  echo -e "${BLUE}Building bifrost (instrumented)...${NC}"
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost

  echo -e "${BLUE}Building bifrost-e2e (instrumented)...${NC}"
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost-e2e

  echo -e "${GREEN}Instrumented binaries built successfully${NC}"
  echo "  bifrost:     target/release/bifrost"
  echo "  bifrost-e2e: target/release/bifrost-e2e"
}

run_e2e_suites() {
  step "Running E2E test suites with coverage instrumentation"

  export LLVM_PROFILE_FILE="$PROFRAW_DIR/e2e-%p-%m.profraw"

  local data_dir="$OUTPUT_DIR/.bifrost-data"
  local runtime_key home_dir
  runtime_key="$(printf '%s' "$OUTPUT_DIR" | cksum | awk '{print $1}')"
  # Remote-file policy denies **/target/** by design; HOME must therefore not
  # live below the default target/coverage-e2e output directory.
  home_dir="$ROOT_DIR/.coverage-runtime/home-$runtime_key"
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
  export CARGO_HOME="$ORIGINAL_CARGO_HOME"
  export RUSTUP_HOME="$ORIGINAL_RUSTUP_HOME"
  export XDG_CONFIG_HOME="$HOME/xdg-config"
  export XDG_DATA_HOME="$HOME/xdg-data"
  export BIFROST_E2E_PROTECTED_PORTS="${BIFROST_E2E_PROTECTED_PORTS:-9900}"
  if [[ -z "${NODE_BIN:-}" && -x "$ORIGINAL_HOME/.local/share/mise/installs/node/22.22.0/bin/node" ]]; then
    export NODE_BIN="$ORIGINAL_HOME/.local/share/mise/installs/node/22.22.0/bin/node"
  fi
  if [[ -n "${NODE_BIN:-}" && -x "$NODE_BIN" ]]; then
    export PATH="$(dirname "$NODE_BIN"):$PATH"
  fi
  export BIFROST_COVERAGE_E2E=1
  unset BIFROST_DETACHED_DAEMON_CHILD
  unset BIFROST_EXTERNAL_CLI_WORKER
  export BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
  export BIFROST_E2E_BIN="$ROOT_DIR/target/release/bifrost-e2e"

  echo -e "${BLUE}E2E data   :${NC} $BIFROST_DATA_DIR"
  echo -e "${BLUE}E2E home   :${NC} $HOME"
  echo -e "${BLUE}Protected  :${NC} $BIFROST_E2E_PROTECTED_PORTS"

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
    echo -e "${YELLOW}Some E2E suites failed; a diagnostic report will be generated and the command will fail.${NC}"
  fi

  cleanup_coverage_environment
  return "$had_failure"
}

merge_and_report() {
  step "Generating coverage report"

  local llvm_profdata llvm_cov profdata_path
  llvm_profdata="$(resolve_llvm_tool llvm-profdata)"
  python3 scripts/ci/coverage-sanitize-profraw.py "$PROFRAW_DIR" \
    --llvm-profdata "$llvm_profdata" \
    --json-output "$OUTPUT_DIR/profile-sanitizer.json"

  local profraw_count
  profraw_count="$(find "$PROFRAW_DIR" -name '*.profraw' 2>/dev/null | wc -l | tr -d ' ')"
  echo -e "${BLUE}Found $profraw_count .profraw files${NC}"

  if [[ "$profraw_count" -eq 0 ]]; then
    echo -e "${RED}No .profraw files found. Coverage data was not generated.${NC}"
    echo -e "${YELLOW}Check that LLVM_PROFILE_FILE was set correctly during test execution.${NC}"
    return 1
  fi

  llvm_cov="$(resolve_llvm_tool llvm-cov)"
  profdata_path="$OUTPUT_DIR/e2e.profdata"
  "$llvm_profdata" merge -sparse "$PROFRAW_DIR"/*.profraw -o "$profdata_path"

  local bifrost_bin="target/release/bifrost"
  local e2e_bin="target/release/bifrost-e2e"
  if [[ ! -x "$bifrost_bin" || ! -x "$e2e_bin" ]]; then
    echo -e "${RED}Instrumented E2E binaries are missing${NC}" >&2
    return 1
  fi

  local -a object_args=("$bifrost_bin" "-object=$e2e_bin")
  local -a report_args=(
    "-instr-profile=$profdata_path"
    '--ignore-filename-regex=(.cargo/registry|rustc/|crates/bifrost-e2e/)'
  )

  case "$FORMAT" in
    html)
      mkdir -p "$OUTPUT_DIR/html"
      echo -e "${BLUE}Generating HTML report...${NC}"
      "$llvm_cov" show "${object_args[@]}" "${report_args[@]}" \
        -format=html -output-dir="$OUTPUT_DIR/html" \
        -show-line-counts-or-regions -show-instantiations=false

      echo -e "${GREEN}HTML report generated at: $OUTPUT_DIR/html/index.html${NC}"
      if [[ "$OPEN_REPORT" -eq 1 ]]; then
        open "$OUTPUT_DIR/html/index.html" 2>/dev/null || xdg-open "$OUTPUT_DIR/html/index.html" 2>/dev/null || true
      fi
      ;;
    lcov)
      mkdir -p "$OUTPUT_DIR"
      echo -e "${BLUE}Generating LCOV report...${NC}"
      "$llvm_cov" export "${object_args[@]}" "${report_args[@]}" \
        -format=lcov > "$OUTPUT_DIR/lcov.info"

      echo -e "${GREEN}LCOV report generated at: $OUTPUT_DIR/lcov.info${NC}"
      ;;
    json)
      "$llvm_cov" export "${object_args[@]}" "${report_args[@]}" \
        -format=text > "$OUTPUT_DIR/coverage.json"
      echo -e "${GREEN}JSON report generated at: $OUTPUT_DIR/coverage.json${NC}"
      ;;
    text)
      "$llvm_cov" report "${object_args[@]}" "${report_args[@]}"
      ;;
  esac

  if [[ "$FAIL_UNDER" -gt 0 ]]; then
    echo ""
    echo -e "${BLUE}Checking coverage threshold (${FAIL_UNDER}%)...${NC}"

    local pct
    pct="$("$llvm_cov" export "${object_args[@]}" "${report_args[@]}" \
      -format=text -summary-only | python3 -c \
      'import json,sys; t=json.load(sys.stdin)["data"][0]["totals"]["lines"]; print(t["percent"])')"
    python3 - "$pct" "$FAIL_UNDER" <<'PY'
import sys
actual, floor = map(float, sys.argv[1:])
if actual < floor:
    raise SystemExit(f"coverage {actual:.2f}% is below {floor:.2f}%")
print(f"coverage {actual:.2f}% meets {floor:.2f}%")
PY
  fi
}

main() {
  step "Bifrost E2E Coverage"

  ensure_cargo_llvm_cov

  cargo llvm-cov clean --workspace

  mkdir -p "$OUTPUT_DIR"
  PROFRAW_DIR="$OUTPUT_DIR/profraw"
  rm -rf "$PROFRAW_DIR"
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

  local e2e_status=0
  run_e2e_suites || e2e_status=$?

  merge_and_report

  if [[ "$e2e_status" -ne 0 ]]; then
    echo -e "${RED}Instrumented E2E suite failed (status $e2e_status)${NC}" >&2
    exit "$e2e_status"
  fi
}

main
