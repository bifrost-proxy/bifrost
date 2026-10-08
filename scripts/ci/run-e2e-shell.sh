#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cd "$ROOT_DIR"

record_e2e_job_root() {
  [[ "${GITHUB_ACTIONS:-}" == "true" ]] || return 0
  # Record this live entrypoint, not a machine-wide before/after PID list.
  # Missing identity disables the best-effort job cleanup, not the tests.
  export BIFROST_E2E_JOB_ROOT_PID="$$"
  if ! BIFROST_E2E_JOB_ROOT_START="$(LC_ALL=C ps -p "$$" -o lstart= 2>/dev/null | awk '{$1=$1; print}')"; then
    BIFROST_E2E_JOB_ROOT_START=""
  fi
  export BIFROST_E2E_JOB_ROOT_START
}

cleanup_tracked_e2e_processes() {
  bash "$ROOT_DIR/scripts/ci/cleanup-e2e-job-processes.sh" || true
}

record_e2e_job_root
trap cleanup_tracked_e2e_processes EXIT

SHARD_ARGS=""
if [[ -n "${BIFROST_E2E_SHARD_INDEX:-}" && -n "${BIFROST_E2E_SHARD_TOTAL:-}" ]]; then
  SHARD_ARGS="--shard ${BIFROST_E2E_SHARD_INDEX}/${BIFROST_E2E_SHARD_TOTAL}"
fi

# shellcheck disable=SC2086
bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build $SHARD_ARGS
