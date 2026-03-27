#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
E2E_DIR="$ROOT_DIR/e2e-tests"

MODE="local"
SHELL_MODE="stable"
RUN_RULES=1
RUN_SHELL=1
RUN_RUNNER=1
RUN_UI=1
PLATFORM="$(uname -s)"

STABLE_SHELL_TESTS=(
  "test_rules_admin_api.sh"
  "test_values_admin_api.sh"
  "test_scripts_admin_api.sh"
  "test_system_admin_api.sh"
  "test_proxy_admin_api.sh"
  "test_cert_admin_api.sh"
  "test_performance_config_admin_api.sh"
  "test_metrics_hosts_apps_admin_api.sh"
  "test_tls_intercept_mode_api.sh"
  "test_bifrost_file_syntax_admin_api.sh"
)

header() {
  echo
  echo "==> $1"
}

usage() {
  cat <<'EOF'
Usage: scripts/run_all_e2e.sh [options]

Options:
  --ci                Run the CI-oriented full suite
  --full-shell        Run the broader shell suite except explicitly excluded tests
  --skip-rules        Skip e2e-tests/run_all_tests_parallel.sh
  --skip-shell        Skip shell E2E scripts
  --skip-runner       Skip cargo run -p bifrost-e2e
  --skip-ui           Skip Playwright UI E2E
  -h, --help          Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ci)
      MODE="ci"
      SHELL_MODE="full"
      shift
      ;;
    --full-shell)
      SHELL_MODE="full"
      shift
      ;;
    --skip-rules)
      RUN_RULES=0
      shift
      ;;
    --skip-shell)
      RUN_SHELL=0
      shift
      ;;
    --skip-runner)
      RUN_RUNNER=0
      shift
      ;;
    --skip-ui)
      RUN_UI=0
      shift
      ;;
    -h|--help)
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

run_shell_test() {
  local script_name="$1"
  header "Running shell E2E: ${script_name}"
  bash "$E2E_DIR/tests/$script_name"
}

should_skip_full_shell_test() {
  local script_name="$1"

  case "$PLATFORM" in
    Darwin)
      return 1
      ;;
    Linux)
      [[ "$script_name" == "test_system_proxy_e2e.sh" ]]
      return
      ;;
    MINGW*|MSYS*|CYGWIN*)
      return 1
      ;;
    *)
      return 1
      ;;
  esac
}

ensure_bifrost_shell_shim() {
  local profile_dir="$1"
  local binary_dir="$ROOT_DIR/target/$profile_dir"
  local exe_path="$binary_dir/bifrost.exe"
  local shim_path="$binary_dir/bifrost"

  if [[ ! -f "$exe_path" || -e "$shim_path" ]]; then
    return 0
  fi

  cat > "$shim_path" <<'EOF'
#!/usr/bin/env bash
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$SCRIPT_DIR/bifrost.exe" "$@"
EOF
  chmod +x "$shim_path"
}

cd "$ROOT_DIR"

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
export CARGO_BIN="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
export BIFROST_UI_TEST_TARGET_DIR="${BIFROST_UI_TEST_TARGET_DIR:-$ROOT_DIR/.bifrost-ui-target}"
export BIFROST_UI_TEST_RUNNER_PORT="${BIFROST_UI_TEST_RUNNER_PORT:-18080}"
export BIFROST_E2E_ROOT="$ROOT_DIR"
export HOME="${HOME:-$ROOT_DIR/.bifrost-e2e-home}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$ROOT_DIR/.bifrost-e2e-xdg-config}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$ROOT_DIR/.bifrost-e2e-xdg-data}"
export PATH="$ROOT_DIR/e2e-tests/bin:$(dirname "$CARGO_BIN"):$PATH"

mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

if [[ "$RUN_RULES" -eq 1 || "$RUN_SHELL" -eq 1 ]]; then
  header "Building release bifrost for rule and shell E2E suites"
  SKIP_FRONTEND_BUILD=1 "$CARGO_BIN" build --release --bin bifrost
  ensure_bifrost_shell_shim "release"
fi

if [[ "$RUN_SHELL" -eq 1 && "$SHELL_MODE" == "full" ]]; then
  header "Building debug bifrost for shell E2E compatibility"
  SKIP_FRONTEND_BUILD=1 "$CARGO_BIN" build --bin bifrost
  ensure_bifrost_shell_shim "debug"
fi

if [[ "$RUN_RULES" -eq 1 ]]; then
  header "Running rule fixture E2E suite"
  bash "$E2E_DIR/run_all_tests_parallel.sh"
fi

if [[ "$RUN_SHELL" -eq 1 ]]; then
  if [[ "$SHELL_MODE" == "full" ]]; then
    while IFS= read -r script_path; do
      script_name="$(basename "$script_path")"
      if should_skip_full_shell_test "$script_name"; then
        continue
      fi
      run_shell_test "$script_name"
    done < <(find "$E2E_DIR/tests" -maxdepth 1 -type f -name 'test_*.sh' | sort)
  else
    for script_name in "${STABLE_SHELL_TESTS[@]}"; do
      run_shell_test "$script_name"
    done
  fi
fi

if [[ "$RUN_RUNNER" -eq 1 ]]; then
  header "Running bifrost-e2e custom runner"
  "$CARGO_BIN" run -p bifrost-e2e -- --port "$BIFROST_UI_TEST_RUNNER_PORT"
fi

if [[ "$RUN_UI" -eq 1 ]]; then
  header "Building debug bifrost for Playwright E2E"
  CARGO_TARGET_DIR="$BIFROST_UI_TEST_TARGET_DIR" "$CARGO_BIN" build --bin bifrost

  header "Running Playwright UI E2E suite"
  pnpm --dir web run test:ui
fi
