#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-timeout-cleanup.XXXXXX")"
export BIFROST_E2E_SANDBOX_DIR="$TEST_ROOT"
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"

if [[ "${BIFROST_E2E_TIMEOUT_CLEANUP_FIXTURE:-0}" == "1" ]]; then
  child_pid_file="${BIFROST_E2E_TIMEOUT_CLEANUP_CHILD_PID_FILE:?child pid file is required}"
  trap '' TERM
  (
    trap '' TERM
    while :; do sleep 1; done
  ) &
  child_pid=$!
  printf '%s\n' "$child_pid" >"$child_pid_file"
  echo "timeout-cleanup-probe-ready"
  wait "$child_pid"
  exit 0
fi

NESTED_LOG="$TEST_ROOT/nested-runner.log"
CHILD_PID_PATH="$TEST_ROOT/child.pid"
NESTED_REPORT_DIR="$TEST_ROOT/reports"
fixture_child_pid=""

cleanup() {
  if [[ -s "$CHILD_PID_PATH" ]]; then
    fixture_child_pid="$(cat "$CHILD_PID_PATH" 2>/dev/null || true)"
  fi
  if [[ -n "$fixture_child_pid" ]] && kill -0 "$fixture_child_pid" 2>/dev/null; then
    kill_process_tree "$fixture_child_pid" 2>/dev/null || true
  fi
  rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

start_seconds="$(date +%s)"
set +e
BIFROST_BIN=/usr/bin/true \
BIFROST_UI_TEST_RUNNER_PORT=18080 \
BIFROST_E2E_REPORT_DIR="$NESTED_REPORT_DIR" \
BIFROST_E2E_SHELL_TESTS=test_e2e_runner_timeout_cleanup.sh \
BIFROST_E2E_SHELL_JOBS=1 \
BIFROST_E2E_SUITE_TIMEOUT=1 \
BIFROST_E2E_TIMEOUT_CLEANUP_FIXTURE=1 \
BIFROST_E2E_TIMEOUT_CLEANUP_CHILD_PID_FILE="$CHILD_PID_PATH" \
  bash "$ROOT_DIR/scripts/run_all_e2e.sh" \
    --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
    >"$NESTED_LOG" 2>&1
nested_status=$?
set -e
elapsed_seconds="$(( $(date +%s) - start_seconds ))"

[[ "$nested_status" -ne 0 ]]
[[ -s "$CHILD_PID_PATH" ]]
fixture_child_pid="$(cat "$CHILD_PID_PATH")"
grep -Fq 'timeout-cleanup-probe-ready' "$NESTED_REPORT_DIR/shell_test_e2e_runner_timeout_cleanup.sh.log"
grep -Fq 'reason: timed out after 1s' "$NESTED_LOG"

if kill -0 "$fixture_child_pid" 2>/dev/null; then
  echo "timed-out fixture child is still alive: $fixture_child_pid" >&2
  exit 1
fi
if [[ "$elapsed_seconds" -gt 30 ]]; then
  echo "runner timeout cleanup exceeded bounded budget: ${elapsed_seconds}s" >&2
  exit 1
fi

fixture_child_pid=""
echo "E2E runner timeout cleanup: PASS (${elapsed_seconds}s)"
