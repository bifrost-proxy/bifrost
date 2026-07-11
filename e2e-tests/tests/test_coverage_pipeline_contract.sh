#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

coverage_all="scripts/ci/coverage-all.sh"
coverage_e2e="scripts/ci/coverage-e2e.sh"
runner="scripts/run_all_e2e.sh"
serial_rules="e2e-tests/test_rules.sh"
ci_workflow=".github/workflows/ci.yml"

bash -n "$coverage_all"
bash -n "$coverage_e2e"
bash -n "$runner"
bash scripts/ci/check-shell-syntax.sh

grep -Fq 'cargo llvm-cov show-env --sh' "$coverage_all"
grep -Fq 'unit-integration.json' "$coverage_all"
grep -Fq 'e2e.json' "$coverage_all"
grep -Fq 'snapshot_profiles "$unit_profiles"' "$coverage_all"
grep -Fq 'restore_profiles "$unit_profiles"' "$coverage_all"
grep -Fq 'PROFILE_ROOT="$ROOT_DIR/target/llvm-cov-target"' "$coverage_all"
grep -Fq 'export CARGO_TARGET_DIR="$PROFILE_ROOT"' "$coverage_all"
grep -Fq 'export BIFROST_BIN=' "$coverage_all"
grep -Fq 'export BIFROST_E2E_BIN=' "$coverage_all"
grep -Fq 'prepare_isolated_e2e_environment' "$coverage_all"
grep -Fq 'restore_tool_environment' "$coverage_all"
grep -Fq 'REFUSING: coverage E2E data directory is under production data' "$coverage_all"
grep -Fq 'BIFROST_E2E_PROTECTED_PORTS' "$coverage_all"
grep -Fq 'One or more instrumented E2E suites failed' "$coverage_all"

if grep -Fq 'cp "$BIFROST_BIN" "$ROOT_DIR/target/release/bifrost"' "$coverage_all"; then
  echo "coverage-all must not overwrite the normal release binary" >&2
  exit 1
fi

grep -Fq 'BIFROST_E2E_BIN' "$runner"
grep -Fq '_prebuilt="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"' "$runner"
grep -Fq 'resolved_bifrost_bin=$(resolve_bifrost_release_bin' "$serial_rules"
if grep -Fq 'local BIFROST_BIN=' "$serial_rules"; then
  echo "serial rule runner shadows injected BIFROST_BIN" >&2
  exit 1
fi
grep -Fq '> "$OUTPUT_DIR/coverage.json"' "$coverage_e2e"
grep -Fq 'REFUSING: coverage E2E data directory is under production data' "$coverage_e2e"
grep -Fq 'BIFROST_E2E_PROTECTED_PORTS' "$coverage_e2e"
grep -Fq 'export HOME="$ORIGINAL_HOME"' "$coverage_e2e"
grep -Fq 'Instrumented E2E suite failed' "$coverage_e2e"
grep -Fq 'shell_test_capability_group()' "$runner"
grep -Fq 'use_capability_shell_shards()' "$runner"
grep -Fq 'capability: proxy-core' "$ci_workflow"
grep -Fq 'capability: remote' "$ci_workflow"
grep -Fq 'capability: agent-extensions' "$ci_workflow"
grep -Fq 'BIFROST_E2E_SHARD_TOTAL: "3"' "$ci_workflow"
grep -Fq 'BIFROST_E2E_CAPABILITY_SHARDS: "1"' "$ci_workflow"
if grep -Fq 'save-if: always()' "$ci_workflow" ||
  grep -Fq 'save-if: ${{ always() }}' "$ci_workflow"; then
  echo "rust-cache save-if must use a valid constant boolean expression" >&2
  exit 1
fi
grep -Fq 'save-if: ${{ true }}' "$ci_workflow"
BIFROST_E2E_CAPABILITY_SHARDS=1 BIFROST_E2E_SHELL_JOBS=2 bash "$runner" \
  --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
  --shard 1/3 --check-shell-shard-balance

partition_dir="$(mktemp -d)"
trap 'rm -rf "$partition_dir"' EXIT
BIFROST_E2E_CAPABILITY_SHARDS=0 BIFROST_E2E_SHARD_INDEX=0 BIFROST_E2E_SHARD_TOTAL=0 \
  bash "$runner" --ci --full-shell --skip-rules --skip-runner --skip-ui \
  --skip-build --list-shell-tests | sort > "$partition_dir/all.txt"
for shard in 1 2 3; do
  BIFROST_E2E_CAPABILITY_SHARDS=1 BIFROST_E2E_SHELL_JOBS=2 bash "$runner" \
    --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
    --shard "$shard/3" --list-shell-tests | sort > "$partition_dir/shard-$shard.txt"
done
sort "$partition_dir"/shard-*.txt > "$partition_dir/combined.txt"
cmp -s "$partition_dir/all.txt" "$partition_dir/combined.txt"
[[ "$(uniq -d "$partition_dir/combined.txt" | wc -l | tr -d ' ')" -eq 0 ]]
grep -Fxq 'test_http3_e2e.sh' "$partition_dir/shard-1.txt"
grep -Fxq 'test_remote_invoke_e2e.sh' "$partition_dir/shard-2.txt"
grep -Fxq 'test_group_sync_e2e.sh' "$partition_dir/shard-2.txt"
grep -Fxq 'test_cli_online_commands_e2e.sh' "$partition_dir/shard-2.txt"
grep -Fxq 'test_agent_builtin_status_runtime.sh' "$partition_dir/shard-3.txt"
grep -Fxq 'test_im_gateway_long_reply_delivery_regression.sh' "$partition_dir/shard-3.txt"
if grep -Fxq 'test_desktop_open_requests_contract.sh' "$partition_dir/all.txt" ||
  grep -Fxq 'test_desktop_sidecar_launchd_env_contract.sh' "$partition_dir/all.txt"; then
  echo "desktop compile-only contract wrappers must stay out of CI shell shards" >&2
  exit 1
fi

if grep -Fq 'Some E2E suites had failures, but coverage data was still collected' "$coverage_e2e"; then
  echo "coverage-e2e still masks E2E failures" >&2
  exit 1
fi

echo "Coverage pipeline contract: PASS"
