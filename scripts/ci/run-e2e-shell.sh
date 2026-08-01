#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cd "$ROOT_DIR"

snapshot_e2e_job_processes() {
  [[ "${GITHUB_ACTIONS:-}" == "true" ]] || return 0
  local baseline_dir="${RUNNER_TEMP:-/tmp}"
  local baseline_file="$baseline_dir/bifrost-e2e-process-baseline-${GITHUB_RUN_ID:-local}-${GITHUB_JOB:-shell}-$$.txt"
  local current_uid
  current_uid="$(id -u)"
  ps -axo uid=,pid= 2>/dev/null |
    awk -v uid="$current_uid" '$1 == uid { print $2 }' \
      >"$baseline_file"
  export BIFROST_E2E_JOB_PROCESS_BASELINE="$baseline_file"
}

cleanup_tracked_e2e_processes() {
  bash "$ROOT_DIR/scripts/ci/cleanup-e2e-job-processes.sh" || true
}

snapshot_e2e_job_processes
trap cleanup_tracked_e2e_processes EXIT

SHARD_ARGS=""
if [[ -n "${BIFROST_E2E_SHARD_INDEX:-}" && -n "${BIFROST_E2E_SHARD_TOTAL:-}" ]]; then
  SHARD_ARGS="--shard ${BIFROST_E2E_SHARD_INDEX}/${BIFROST_E2E_SHARD_TOTAL}"
fi

# shellcheck disable=SC2086
bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build $SHARD_ARGS
