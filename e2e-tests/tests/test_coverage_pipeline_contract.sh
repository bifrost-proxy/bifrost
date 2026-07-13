#!/usr/bin/env bash

set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

coverage_all="scripts/ci/coverage-all.sh"
coverage_e2e="scripts/ci/coverage-e2e.sh"
runner="scripts/run_all_e2e.sh"
serial_rules="e2e-tests/test_rules.sh"
ci_workflow=".github/workflows/ci.yml"
layered_workflow=".github/workflows/coverage-e2e.yml"
ui_full_workflow=".github/workflows/ui-e2e-full.yml"
coverage_diff="scripts/ci/coverage-diff.py"
coverage_production="scripts/ci/coverage-production.py"
proxy_coverage_shell_manifest="scripts/ci/proxy-coverage-shell-tests.txt"
shell_quality="scripts/ci/check-shell-quality.sh"
e2e_summary="scripts/ci/e2e-summary.py"
ui_critical="scripts/ci/run-ui-critical.sh"

bash -n "$coverage_all"
bash -n "$coverage_e2e"
bash -n "$runner"
bash -n "$shell_quality"
bash -n "$ui_critical"
bash scripts/ci/check-shell-syntax.sh
python3 -m unittest discover -s scripts/ci/tests -p 'test_*.py' -v

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
grep -Fq 'trap cleanup_on_exit EXIT' "$coverage_all"
grep -Fq 'export CARGO_HOME="$ORIGINAL_CARGO_HOME"' "$coverage_all"
grep -Fq 'export RUSTUP_HOME="$ORIGINAL_RUSTUP_HOME"' "$coverage_all"
grep -Fq 'export BIFROST_COVERAGE_E2E=1' "$coverage_all"
grep -Fq 'export PATH="$(dirname "$NODE_BIN"):$PATH"' "$coverage_all"
grep -Fq 'REFUSING: coverage E2E data directory is under production data' "$coverage_all"
grep -Fq 'BIFROST_E2E_PROTECTED_PORTS' "$coverage_all"
grep -Fq 'One or more instrumented E2E suites failed' "$coverage_all"
grep -Fq 'Changed production Rust line coverage' "$coverage_diff"
grep -Fq 'all exact #[cfg(test)] items excluded' "$coverage_production"
grep -Fq 'changed_lines_min = 95.0' scripts/ci/coverage-thresholds.toml
grep -Fq 'coverage-diff.py target/coverage/lcov.info' "$ci_workflow"
grep -Fq -- '--with-e2e --e2e-suite proxy' "$ci_workflow"
grep -Fq 'target/coverage/production-coverage.json' "$ci_workflow"
grep -Fq 'E2E UI (Playwright)' "$ci_workflow"
grep -Fq 'BIFROST_UI_TEST_PROFILE: critical' "$ci_workflow"
grep -Fq 'run-ui-critical.sh' "$runner"
grep -Fq -- '--skip-runner' "$ci_workflow"
grep -Fq 'check-shell-quality.sh' "$ci_workflow"
grep -Fq 'shellcheck --severity=error' "$shell_quality"
grep -Fq 'shfmt -d -i 2 -ci' "$shell_quality"
grep -Fq 'shfmt_v3.12.0_linux_amd64' "$ci_workflow"
grep -Fq 'd9fbb2a9c33d13f47e7618cf362a914d029d02a6df124064fff04fd688a745ea' "$ci_workflow"
if grep -Eq 'actions/setup-go|(^|[[:space:]])go[[:space:]]+install([[:space:]]|$)' "$ci_workflow"; then
  echo "CI must not install the Go toolchain" >&2
  exit 1
fi
tracked_go_files="$(
  git ls-files -- \
    ':(glob)**/*.go' 'go.mod' 'go.sum' 'go.work' \
    ':(glob)**/go.mod' ':(glob)**/go.sum' ':(glob)**/go.work' |
    while IFS= read -r tracked_go_file; do
      [[ ! -e "$tracked_go_file" ]] || printf '%s\n' "$tracked_go_file"
    done
)"
if [[ -n "$tracked_go_files" ]]; then
  echo "tracked Go files are forbidden:" >&2
  echo "$tracked_go_files" >&2
  exit 1
fi
legacy_go_artifacts="$(
  git ls-files -- 'e2e-tests/tests/quic_socks5_client/**' |
    while IFS= read -r legacy_go_artifact; do
      [[ ! -e "$legacy_go_artifact" ]] || printf '%s\n' "$legacy_go_artifact"
    done
)"
if [[ -n "$legacy_go_artifacts" ]]; then
  echo "legacy Go QUIC/SOCKS5 client artifacts are forbidden" >&2
  exit 1
fi
grep -Fq 'write_machine_report' "$runner"
grep -Fq 'summary.json' "$runner"
grep -Fq 'schema_version' "$e2e_summary"
python3 scripts/ci/check-e2e-capabilities.py
grep -Fq 'Proxy E2E capability contract' "$ci_workflow"
grep -Fq 'Layered E2E Coverage' "$layered_workflow"
if grep -Fq 'pull_request:' "$layered_workflow"; then
  echo "full layered coverage must not run for every pull request" >&2
  exit 1
fi
grep -Fq 'cron: "30 18 * * 0"' "$layered_workflow"
grep -Fq 'workflow_dispatch:' "$layered_workflow"
grep -Fq 'bash scripts/ci/coverage-all.sh --with-e2e' "$layered_workflow"
grep -Fq 'production-coverage.json' "$layered_workflow"
grep -Fq 'metric = "production"' scripts/ci/coverage-thresholds.toml
grep -Fq 'min = 90.0' scripts/ci/coverage-thresholds.toml
grep -A5 -F '[crates.bifrost-e2e]' scripts/ci/coverage-thresholds.toml | grep -Fq 'metric = "exempt"'
grep -Fq 'Enforcing bifrost-proxy production coverage gate' "$coverage_all"
grep -Fq 'unit-integration.json' "$layered_workflow"
grep -Fq 'e2e.json' "$layered_workflow"
grep -Fq 'Full UI E2E Audit' "$ui_full_workflow"
grep -Fq 'continue-on-error: true' "$ui_full_workflow"
grep -Fq "steps.full_ui.outcome != 'success'" "$ui_full_workflow"

if grep -Fq 'cp "$BIFROST_BIN" "$ROOT_DIR/target/release/bifrost"' "$coverage_all"; then
  echo "coverage-all must not overwrite the normal release binary" >&2
  exit 1
fi

grep -Fq 'BIFROST_E2E_BIN' "$runner"
grep -Fq 'BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"' \
  e2e-tests/tests/test_daemon_cert_check_e2e.sh
grep -Fq 'BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"' \
  e2e-tests/tests/test_daemon_log_level_e2e.sh
grep -Fq 'Page.navigate", { url: confirmUrl }' \
  e2e-tests/tests/test_rule_share_confirm_browser.sh
for daemon_test in \
  e2e-tests/tests/test_daemon_cert_check_e2e.sh \
  e2e-tests/tests/test_daemon_log_level_e2e.sh \
  e2e-tests/tests/test_stop_restart_shutdown_marker.sh; do
  grep -Fq 'daemon detachment cannot provide a bounded LLVM profile lifecycle' "$daemon_test"
done
grep -Fq '_prebuilt="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"' "$runner"
grep -Fq 'resolved_bifrost_bin=$(resolve_bifrost_release_bin' "$serial_rules"
if grep -Fq 'local BIFROST_BIN=' "$serial_rules"; then
  echo "serial rule runner shadows injected BIFROST_BIN" >&2
  exit 1
fi
grep -Fq '>"$OUTPUT_DIR/coverage.json"' "$coverage_e2e"
grep -Fq 'REFUSING: coverage E2E data directory is under production data' "$coverage_e2e"
grep -Fq 'BIFROST_E2E_PROTECTED_PORTS' "$coverage_e2e"
grep -Fq 'export HOME="$ORIGINAL_HOME"' "$coverage_e2e"
grep -Fq 'export CARGO_HOME="$ORIGINAL_CARGO_HOME"' "$coverage_e2e"
grep -Fq 'export RUSTUP_HOME="$ORIGINAL_RUSTUP_HOME"' "$coverage_e2e"
grep -Fq 'trap cleanup_coverage_environment EXIT' "$coverage_e2e"
grep -Fq 'export PATH="$(dirname "$NODE_BIN"):$PATH"' "$coverage_e2e"
grep -Fq 'Instrumented E2E suite failed' "$coverage_e2e"
grep -Fq 'export BIFROST_E2E=1' scripts/ci/run-e2e-runner.sh
grep -Fq 'shell_test_capability_group()' "$runner"
grep -Fq 'BIFROST_E2E_SHELL_TESTS' "$runner"
grep -Fq 'PROXY_COVERAGE_SHELL_MANIFEST' "$coverage_all"
grep -Fq 'rules | shell | runner | proxy' "$coverage_all"
[[ "$(wc -l <"$proxy_coverage_shell_manifest" | tr -d ' ')" -eq 13 ]]
while IFS= read -r proxy_shell_test; do
  [[ -f "e2e-tests/tests/$proxy_shell_test" ]]
done <"$proxy_coverage_shell_manifest"
manifest_filtered_shell_tests="$(
  BIFROST_E2E_SHELL_TESTS="$(paste -sd, "$proxy_coverage_shell_manifest")" \
    bash "$runner" --ci --full-shell --skip-rules --skip-runner --skip-ui \
    --skip-build --list-shell-tests
)"
[[ "$manifest_filtered_shell_tests" == "$(cat "$proxy_coverage_shell_manifest")" ]]
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
proxy_filter='test_trustworthy_traffic_metrics.sh,test_socks5_tls_rules.sh'
filtered_shell_tests="$(
  BIFROST_E2E_SHELL_TESTS="$proxy_filter" bash "$runner" \
    --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
    --list-shell-tests
)"
[[ "$filtered_shell_tests" == $'test_trustworthy_traffic_metrics.sh\ntest_socks5_tls_rules.sh' ]]
BIFROST_E2E_CAPABILITY_SHARDS=1 BIFROST_E2E_SHELL_JOBS=2 bash "$runner" \
  --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
  --shard 1/3 --check-shell-shard-balance

partition_dir="$(mktemp -d)"
trap 'rm -rf "$partition_dir"' EXIT
BIFROST_E2E_CAPABILITY_SHARDS=0 BIFROST_E2E_SHARD_INDEX=0 BIFROST_E2E_SHARD_TOTAL=0 \
  bash "$runner" --ci --full-shell --skip-rules --skip-runner --skip-ui \
  --skip-build --list-shell-tests | sort >"$partition_dir/all.txt"
for shard in 1 2 3; do
  BIFROST_E2E_CAPABILITY_SHARDS=1 BIFROST_E2E_SHELL_JOBS=2 bash "$runner" \
    --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
    --shard "$shard/3" --list-shell-tests | sort >"$partition_dir/shard-$shard.txt"
done
sort "$partition_dir"/shard-*.txt >"$partition_dir/combined.txt"
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
