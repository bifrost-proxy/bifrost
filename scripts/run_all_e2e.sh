#!/usr/bin/env bash

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
E2E_DIR="$ROOT_DIR/e2e-tests"

MODE="local"
SHELL_MODE="stable"
RUN_RULES=1
RUN_SHELL=1
RUN_RUNNER=1
RUN_UI=1
SKIP_RELEASE_BUILD=0
PLATFORM="$(uname -s)"
REPORT_DIR=""

declare -a SUITE_NAMES=()
declare -a SUITE_STATUSES=()
declare -a SUITE_LOGS=()
declare -a SUITE_REASONS=()
declare -a SUITE_DURATIONS=()

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

print_section() {
  echo
  echo "------------------------------------------------------------"
  echo "$1"
  echo "------------------------------------------------------------"
}

log_info() {
  echo "[INFO] $1"
}

log_warn() {
  echo "[WARN] $1"
}

resolve_non_shim_command() {
  local command_name="$1"
  local candidate

  local resolver="which"
  local resolver_args=("-a")
  case "$(uname -s 2>/dev/null)" in
    MINGW*|MSYS*|CYGWIN*)
      resolver="where.exe"
      resolver_args=()
      ;;
  esac

  while IFS= read -r candidate; do
    candidate="$(trim_line "$candidate")"
    [[ -n "$candidate" ]] || continue
    if [[ "$candidate" != *"/mise/shims/"* && "$candidate" != *"\\mise\\shims\\"* ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done < <("$resolver" "${resolver_args[@]}" "$command_name" 2>/dev/null)

  command -v "$command_name" 2>/dev/null || printf '%s\n' "$command_name"
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
  --skip-build        Skip release binary compilation (use pre-built binary)
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
    --skip-build)
      SKIP_RELEASE_BUILD=1
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

register_suite() {
  local name="$1"
  local status="$2"
  local log_file="$3"
  local reason="$4"
  local duration="$5"

  SUITE_NAMES+=("$name")
  SUITE_STATUSES+=("$status")
  SUITE_LOGS+=("$log_file")
  SUITE_REASONS+=("$reason")
  SUITE_DURATIONS+=("$duration")
}

trim_line() {
  local text="$1"
  text="${text#"${text%%[![:space:]]*}"}"
  text="${text%"${text##*[![:space:]]}"}"
  printf '%s\n' "$text"
}

format_command() {
  local formatted=""
  local arg

  for arg in "$@"; do
    if [[ -n "$formatted" ]]; then
      formatted+=" "
    fi
    printf -v arg '%q' "$arg"
    formatted+="$arg"
  done

  printf '%s\n' "$formatted"
}

print_runtime_context() {
  print_section "E2E Runtime Context"
  echo "Mode         : $MODE"
  echo "Shell mode   : $SHELL_MODE"
  echo "Platform     : $PLATFORM"
  echo "Root dir     : $ROOT_DIR"
  echo "E2E dir      : $E2E_DIR"
  echo "Report dir   : $REPORT_DIR"
  echo "Cargo bin    : $CARGO_BIN"
  echo "Runner port  : $BIFROST_UI_TEST_RUNNER_PORT"
  echo "UI target dir: $BIFROST_UI_TEST_TARGET_DIR"
  echo "Run rules    : $RUN_RULES"
  echo "Run shell    : $RUN_SHELL"
  echo "Run runner   : $RUN_RUNNER"
  echo "Run UI       : $RUN_UI"
  echo "Skip build   : $SKIP_RELEASE_BUILD"
}

stream_command_output() {
  local name="$1"
  local pipe_path="$2"
  local log_file="$3"

  : >"$log_file"
  tee "$log_file" <"$pipe_path" | sed "s/^/[$name] /"
}

heartbeat_while_running() {
  local name="$1"
  local command_pid="$2"
  local log_file="$3"
  local start_ts="$4"
  local tick=0

  while kill -0 "$command_pid" 2>/dev/null; do
    sleep 1
    kill -0 "$command_pid" 2>/dev/null || break
    tick=$((tick + 1))

    if (( tick < 30 )); then
      continue
    fi
    tick=0

    local now_ts
    local elapsed
    local last_line=""
    now_ts="$(date +%s)"
    elapsed="$((now_ts - start_ts))"

    if [[ -f "$log_file" ]]; then
      last_line="$(awk 'NF { line=$0 } END { print line }' "$log_file")"
      last_line="$(trim_line "$last_line")"
    fi

    if [[ -n "$last_line" ]]; then
      echo "[INFO] $name still running (${elapsed}s), last log: $last_line"
    else
      echo "[INFO] $name still running (${elapsed}s)"
    fi
  done
}

extract_failure_reason() {
  local log_file="$1"
  [[ -f "$log_file" ]] || return 0

  python3 - "$log_file" <<'PY'
import re
import sys

for _s in (sys.stdout, sys.stderr):
    if _s and hasattr(_s, "reconfigure"):
        try:
            _s.reconfigure(errors="backslashreplace")
        except Exception:
            pass

path = sys.argv[1]
ansi = re.compile(r"\x1b\[[0-9;]*m")
patterns = [
    re.compile(r"^✗\s*(.+)"),
    re.compile(r"^Error:\s*(.+)", re.IGNORECASE),
    re.compile(r"^ERROR:\s*(.+)"),
    re.compile(r"^Failed:\s*(.+)"),
    re.compile(r"^Caused by:\s*(.+)"),
    re.compile(r"^panic:?\s*(.+)", re.IGNORECASE),
]
ignore_prefixes = (
    "running ",
    "finished ",
    "compiling ",
    "building ",
    "downloaded ",
)

with open(path, "r", encoding="utf-8", errors="ignore") as fh:
    lines = [ansi.sub("", line.rstrip("\n")) for line in fh]

for line in lines:
    stripped = line.strip()
    if not stripped:
        continue
    lowered = stripped.lower()
    if lowered.startswith(ignore_prefixes):
        continue
    for pattern in patterns:
        match = pattern.match(stripped)
        if match:
            msg = match.group(1).strip() or stripped
            print(msg[:400])
            sys.exit(0)

for line in reversed(lines):
    stripped = line.strip()
    if not stripped:
        continue
    lowered = stripped.lower()
    if lowered.startswith(ignore_prefixes):
        continue
    print(stripped[:400])
    sys.exit(0)
PY
}

run_and_capture() {
  local name="$1"
  shift

  local log_slug
  log_slug="$(printf '%s' "$name" | tr ' /:' '___' | tr -cd '[:alnum:]_.-')"
  local log_file="$REPORT_DIR/${log_slug}.log"
  local start_ts
  local end_ts
  local duration
  local status
  local reason=""
  local command_pid
  local stream_pid=""
  local heartbeat_pid=""
  local watchdog_pid=""
  local command_status
  local pipe_path="$REPORT_DIR/${log_slug}.pipe"
  local suite_timeout="${BIFROST_E2E_SUITE_TIMEOUT:-900}"

  start_ts="$(date +%s)"
  rm -f "$pipe_path"
  print_section "Starting ${name}"
  echo "Command : $(format_command "$@")"
  echo "Log file: $log_file"

  if is_windows; then
    "$@" 2>&1 | tee "$log_file" | sed "s/^/[$name] /" &
    command_pid=$!
    log_info "${name} started with pid ${command_pid}"

    heartbeat_while_running "$name" "$command_pid" "$log_file" "$start_ts" &
    heartbeat_pid=$!

    (
      sleep "$suite_timeout"
      if kill -0 "$command_pid" 2>/dev/null; then
        echo "[TIMEOUT] ${name} exceeded ${suite_timeout}s limit, killing pid ${command_pid}" >&2
        kill -TERM "$command_pid" 2>/dev/null || true
        sleep 5
        kill -9 "$command_pid" 2>/dev/null || true
        kill_all_bifrost
      fi
    ) &
    watchdog_pid=$!

    if wait "$command_pid"; then
      status="passed"
    else
      command_status=$?
      if [[ "${command_status:-0}" -eq 143 || "${command_status:-0}" -eq 137 ]]; then
        status="failed"
        reason="timed out after ${suite_timeout}s"
      else
        status="failed"
        reason="$(extract_failure_reason "$log_file")"
        reason="$(trim_line "${reason:-unknown failure}")"
      fi
    fi
  else
    mkfifo "$pipe_path"

    stream_command_output "$name" "$pipe_path" "$log_file" &
    stream_pid=$!

    "$@" >"$pipe_path" 2>&1 &
    command_pid=$!
    log_info "${name} started with pid ${command_pid}"

    heartbeat_while_running "$name" "$command_pid" "$log_file" "$start_ts" &
    heartbeat_pid=$!

    (
      sleep "$suite_timeout"
      if kill -0 "$command_pid" 2>/dev/null; then
        echo "[TIMEOUT] ${name} exceeded ${suite_timeout}s limit, killing pid ${command_pid}" >&2
        kill -TERM "$command_pid" 2>/dev/null || true
        sleep 5
        kill -9 "$command_pid" 2>/dev/null || true
      fi
    ) &
    watchdog_pid=$!

    if wait "$command_pid"; then
      status="passed"
    else
      command_status=$?
      if [[ "${command_status:-0}" -eq 143 || "${command_status:-0}" -eq 137 ]]; then
        status="failed"
        reason="timed out after ${suite_timeout}s"
      else
        status="failed"
        reason="$(extract_failure_reason "$log_file")"
        reason="$(trim_line "${reason:-unknown failure}")"
      fi
    fi
  fi

  kill "$watchdog_pid" 2>/dev/null || true
  wait "$watchdog_pid" 2>/dev/null || true

  wait "$stream_pid" 2>/dev/null || true
  rm -f "$pipe_path"

  if [[ -n "$heartbeat_pid" ]]; then
    kill "$heartbeat_pid" 2>/dev/null || true
    wait "$heartbeat_pid" 2>/dev/null || true
  fi

  if is_windows; then
    kill_all_bifrost
  fi

  end_ts="$(date +%s)"
  duration="$((end_ts - start_ts))"

  register_suite "$name" "$status" "$log_file" "$reason" "$duration"

  if [[ "$status" == "passed" ]]; then
    echo "[PASS] $name (${duration}s)"
  else
    echo "[FAIL] $name (${duration}s)"
    if [[ -n "$reason" ]]; then
      echo "       reason: $reason"
    fi
    echo "       log: $log_file"
  fi

  if [[ "$status" == "passed" ]]; then
    return 0
  fi

  return "${command_status:-1}"
}

SKIP_IN_CI_TESTS=(
  "test_memory_pressure_e2e.sh"
)

is_skipped_in_ci() {
  local name="$1"
  for skipped in "${SKIP_IN_CI_TESTS[@]}"; do
    [[ "$name" == "$skipped" ]] && return 0
  done
  return 1
}

collect_shell_tests() {
  if [[ "$SHELL_MODE" == "full" ]]; then
    find "$E2E_DIR/tests" -maxdepth 1 -type f -name 'test_*.sh' -print \
      | sort \
      | while IFS= read -r script_path; do
          local name
          name="$(basename "$script_path")"
          if [[ "$MODE" == "ci" ]] && is_skipped_in_ci "$name"; then
            continue
          fi
          printf '%s\n' "$name"
        done
  else
    printf '%s\n' "${STABLE_SHELL_TESTS[@]}"
  fi
}

skip_suite() {
  local name="$1"
  local reason="$2"
  register_suite "$name" "skipped" "" "$reason" "0"
  echo "[SKIP] $name"
  echo "       reason: $reason"
}

print_log_tail() {
  local log_file="$1"
  [[ -f "$log_file" ]] || return 0
  tail -20 "$log_file" | sed 's/^/    /'
}

print_final_report() {
  local passed=0
  local failed=0
  local skipped=0
  local i

  print_section "E2E Final Report"

  for i in "${!SUITE_NAMES[@]}"; do
    case "${SUITE_STATUSES[$i]}" in
      passed) ((passed += 1)) ;;
      failed) ((failed += 1)) ;;
      skipped) ((skipped += 1)) ;;
    esac
  done

  echo "Total suites : ${#SUITE_NAMES[@]}"
  echo "Passed       : $passed"
  echo "Failed       : $failed"
  echo "Skipped      : $skipped"
  echo "Report dir   : $REPORT_DIR"

  if (( failed > 0 )); then
    print_section "Failed Suites"
    for i in "${!SUITE_NAMES[@]}"; do
      [[ "${SUITE_STATUSES[$i]}" == "failed" ]] || continue
      echo "- ${SUITE_NAMES[$i]} (${SUITE_DURATIONS[$i]}s)"
      echo "  reason: ${SUITE_REASONS[$i]:-unknown failure}"
      if [[ -n "${SUITE_LOGS[$i]}" ]]; then
        echo "  log: ${SUITE_LOGS[$i]}"
        print_log_tail "${SUITE_LOGS[$i]}"
      fi
    done
  fi

  if (( skipped > 0 )); then
    print_section "Skipped Suites"
    for i in "${!SUITE_NAMES[@]}"; do
      [[ "${SUITE_STATUSES[$i]}" == "skipped" ]] || continue
      echo "- ${SUITE_NAMES[$i]}"
      echo "  reason: ${SUITE_REASONS[$i]}"
    done
  fi
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
      case "$script_name" in
        test_system_proxy_e2e.sh|\
        test_http3_e2e.sh|\
        test_socks5_udp.sh|\
        test_socks5_udp_rules.sh|\
        test_sse_frames.sh|\
        test_websocket_frames.sh)
          return 0
          ;;
        *)
          return 1
          ;;
      esac
      ;;
    *)
      return 1
      ;;
  esac
}

run_shell_tests_parallel() {
  local max_jobs="$1"
  local shell_base_port="${BIFROST_E2E_SHELL_BASE_PORT:-15000}"
  local port_step=10

  local serial_tests=()
  local parallel_tests=()

  local MOCK_MANAGING_TESTS=(
    "test_memory_pressure_e2e.sh"
  )

  for script_name in "${shell_tests[@]}"; do
    if [[ "$SHELL_MODE" == "full" ]] && should_skip_full_shell_test "$script_name"; then
      skip_suite "shell:${script_name}" "skipped on ${PLATFORM}"
      continue
    fi

    local is_mock_managing=0
    for mm in "${MOCK_MANAGING_TESTS[@]}"; do
      if [[ "$script_name" == "$mm" ]]; then
        is_mock_managing=1
        break
      fi
    done

    if [[ "$is_mock_managing" -eq 1 ]]; then
      serial_tests+=("$script_name")
    else
      parallel_tests+=("$script_name")
    fi
  done

  if [[ ${#parallel_tests[@]} -gt 0 ]]; then
    header "Running ${#parallel_tests[@]} safe shell tests in parallel (jobs=$max_jobs)"
    _SHELL_BATCH_LIST=("${parallel_tests[@]}")
    run_shell_batch_parallel "$max_jobs" "$shell_base_port" "$port_step"
  fi

  if [[ ${#serial_tests[@]} -gt 0 ]]; then
    header "Running ${#serial_tests[@]} mock-managing shell tests serially"
    for script_name in "${serial_tests[@]}"; do
      log_info "Queue serial shell test: $script_name"
      run_and_capture "shell:${script_name}" bash "$E2E_DIR/tests/$script_name"
    done
  fi
}

run_shell_batch_parallel() {
  local max_jobs="$1"
  local base_port="$2"
  local port_step="$3"

  local pids=()
  local pid_scripts=()
  local pid_logs=()
  local pid_starts=()
  local running=0
  local completed=0
  local next_index=0
  local total=${#_SHELL_BATCH_LIST[@]}

  while [[ $completed -lt $total ]]; do
    while [[ $running -lt $max_jobs && $next_index -lt $total ]]; do
      local script_name="${_SHELL_BATCH_LIST[$next_index]}"

      local shell_port=$((base_port + next_index * port_step))
      local shell_admin_port="$shell_port"
      local shell_data_dir
      shell_data_dir="$(mktemp -d)"
      local log_slug
      log_slug="$(printf 'shell_%s' "$script_name" | tr ' /:.' '____' | tr -cd '[:alnum:]_.-')"
      local log_file="$REPORT_DIR/${log_slug}.log"
      local start_ts
      start_ts="$(date +%s)"

      log_info "Starting shell test $script_name (port=$shell_port, index=$next_index)"

      (
        ADMIN_PORT="$shell_admin_port" \
        ADMIN_HOST="127.0.0.1" \
        PROXY_PORT="$shell_port" \
        PROXY_HOST="127.0.0.1" \
        ECHO_HTTP_PORT="$((shell_port + 1))" \
        HTTP_PORT="$((shell_port + 1))" \
        ECHO_HTTPS_PORT="$((shell_port + 2))" \
        HTTPS_PORT="$((shell_port + 2))" \
        WS_PORT="$((shell_port + 3))" \
        WSS_PORT="$((shell_port + 4))" \
        SSE_PORT="$((shell_port + 5))" \
        SOCKS5_PORT="$((shell_port + 6))" \
        MOCK_ECHO_PROXY_PORT="$((shell_port + 7))" \
        ECHO_PROXY_PORT="$((shell_port + 7))" \
        BIFROST_DATA_DIR="$shell_data_dir" \
        SKIP_BUILD=true \
        bash "$E2E_DIR/tests/$script_name"
      ) > "$log_file" 2>&1 &

      pids[$next_index]=$!
      pid_scripts[$next_index]="$script_name"
      pid_logs[$next_index]="$log_file"
      pid_starts[$next_index]="$start_ts"
      running=$((running + 1))
      next_index=$((next_index + 1))
    done

    for i in "${!pids[@]}"; do
      if [[ -n "${pids[$i]:-}" ]] && ! kill -0 "${pids[$i]}" 2>/dev/null; then
        local exit_code=0
        wait "${pids[$i]}" 2>/dev/null || exit_code=$?
        local end_ts
        end_ts="$(date +%s)"
        local dur=$((end_ts - pid_starts[$i]))
        local sname="${pid_scripts[$i]}"
        local slog="${pid_logs[$i]}"

        if [[ "$exit_code" -eq 0 ]]; then
          register_suite "shell:${sname}" "passed" "$slog" "" "$dur"
          echo "[PASS] shell:${sname} (${dur}s)"
        else
          local reason
          reason="$(extract_failure_reason "$slog")"
          reason="$(trim_line "${reason:-unknown failure}")"
          register_suite "shell:${sname}" "failed" "$slog" "$reason" "$dur"
          echo "[FAIL] shell:${sname} (${dur}s)"
          echo "       reason: $reason"
          echo "       log: $slog"
        fi

        unset 'pids[i]'
        completed=$((completed + 1))
        running=$((running - 1))
      fi
    done

    if [[ $running -gt 0 ]]; then
      sleep 0.2
    fi
  done
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
export NODE_BIN="${NODE_BIN:-$(resolve_non_shim_command node)}"
export PNPM_BIN="${PNPM_BIN:-$(resolve_non_shim_command pnpm)}"
export BIFROST_UI_TEST_TARGET_DIR="${BIFROST_UI_TEST_TARGET_DIR:-$ROOT_DIR/.bifrost-ui-target}"
export BIFROST_UI_TEST_RUNNER_PORT="${BIFROST_UI_TEST_RUNNER_PORT:-18080}"
export BIFROST_E2E_ROOT="$ROOT_DIR"
export HOME="${HOME:-$ROOT_DIR/.bifrost-e2e-home}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$ROOT_DIR/.bifrost-e2e-xdg-config}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$ROOT_DIR/.bifrost-e2e-xdg-data}"
export PATH="$ROOT_DIR/e2e-tests/bin:$(dirname "$CARGO_BIN"):$(dirname "$NODE_BIN"):$(dirname "$PNPM_BIN"):$PATH"
source "$E2E_DIR/test_utils/process.sh"

mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
REPORT_DIR="${BIFROST_E2E_REPORT_DIR:-$ROOT_DIR/.e2e-reports/run-all-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$REPORT_DIR"
print_runtime_context

release_build_ok=1
ui_build_ok=1

if [[ "$RUN_RULES" -eq 1 || "$RUN_SHELL" -eq 1 ]]; then
  if [[ "$SKIP_RELEASE_BUILD" -eq 1 ]]; then
    _prebuilt="$ROOT_DIR/target/release/bifrost"
    if is_windows; then
      _prebuilt="$ROOT_DIR/target/release/bifrost.exe"
    fi
    if [[ -f "$_prebuilt" ]]; then
      log_info "Skipping release build: using pre-built binary at $_prebuilt"
      ensure_bifrost_shell_shim "release"
    else
      log_warn "Pre-built binary not found at $_prebuilt, falling back to build"
      if run_and_capture \
        "build:release-bifrost" \
        env SKIP_FRONTEND_BUILD=1 "$CARGO_BIN" build --release --bin bifrost; then
        ensure_bifrost_shell_shim "release"
      else
        release_build_ok=0
      fi
    fi
  else
    header "Building release bifrost for rule and shell E2E suites"
    if run_and_capture \
      "build:release-bifrost" \
      env SKIP_FRONTEND_BUILD=1 "$CARGO_BIN" build --release --bin bifrost; then
      ensure_bifrost_shell_shim "release"
    else
      release_build_ok=0
    fi
  fi
fi

RUNNER_BG_PID=""
RUNNER_LOG_FILE=""
RUNNER_STATUS_FILE=""
RUNNER_START_TS=""
if [[ "$RUN_RUNNER" -eq 1 ]]; then
  header "Starting bifrost-e2e custom runner (background)"
  RUNNER_JOBS="${BIFROST_E2E_RUNNER_JOBS:-1}"
  RUNNER_LOG_FILE="$REPORT_DIR/runner__bifrost-e2e.log"
  RUNNER_STATUS_FILE="$REPORT_DIR/runner__bifrost-e2e.status"
  RUNNER_START_TS="$(date +%s)"
  (
    set +e
    "$CARGO_BIN" run -p bifrost-e2e -- --port "$BIFROST_UI_TEST_RUNNER_PORT" --jobs "$RUNNER_JOBS" \
      > "$RUNNER_LOG_FILE" 2>&1
    rc=$?
    echo "$rc" > "$RUNNER_STATUS_FILE"
    exit "$rc"
  ) &
  RUNNER_BG_PID=$!
  log_info "bifrost-e2e runner started in background (PID: $RUNNER_BG_PID, jobs: $RUNNER_JOBS)"
fi

if [[ "$RUN_RULES" -eq 1 ]]; then
  header "Running rule fixture E2E suite"
  if [[ "$release_build_ok" -eq 1 ]]; then
    log_info "Invoking rule suite entrypoint: $E2E_DIR/run_all_tests_parallel.sh"
    run_and_capture \
      "rules:parallel-fixtures" \
      bash "$E2E_DIR/run_all_tests_parallel.sh" --no-build --retry-failed-once
  else
    skip_suite "rules:parallel-fixtures" "release build failed"
  fi
fi

if [[ "$RUN_SHELL" -eq 1 ]]; then
  shell_tests=()
  while IFS= read -r script_name; do
    [[ -n "$script_name" ]] && shell_tests+=("$script_name")
  done < <(collect_shell_tests)
  log_info "Shell test count: ${#shell_tests[@]}"

  shell_build_ok="$release_build_ok"

  if [[ "$shell_build_ok" -eq 1 ]]; then
    export SKIP_BUILD=true
    SHELL_JOBS="${BIFROST_E2E_SHELL_JOBS:-1}"

    if [[ "$SHELL_JOBS" -gt 1 ]]; then
      run_shell_tests_parallel "$SHELL_JOBS"
    else
      for script_name in "${shell_tests[@]}"; do
        log_info "Queue shell test: $script_name"
        if [[ "$SHELL_MODE" == "full" ]] && should_skip_full_shell_test "$script_name"; then
          skip_suite "shell:${script_name}" "skipped on ${PLATFORM}"
          continue
        fi
        run_and_capture "shell:${script_name}" bash "$E2E_DIR/tests/$script_name"
      done
    fi
  else
    for script_name in "${shell_tests[@]}"; do
      log_info "Skip shell test without execution: $script_name"
      if [[ "$SHELL_MODE" == "full" ]] && should_skip_full_shell_test "$script_name"; then
        skip_suite "shell:${script_name}" "skipped on ${PLATFORM}"
        continue
      fi
        skip_suite "shell:${script_name}" "required bifrost build failed"
    done
  fi
fi

if [[ -n "$RUNNER_BG_PID" ]]; then
  header "Waiting for bifrost-e2e runner to complete"
  RUNNER_END_TS=""
  if wait "$RUNNER_BG_PID" 2>/dev/null; then
    RUNNER_END_TS="$(date +%s)"
    RUNNER_DURATION=$((RUNNER_END_TS - RUNNER_START_TS))
    register_suite "runner:bifrost-e2e" "passed" "$RUNNER_LOG_FILE" "" "$RUNNER_DURATION"
    echo "[PASS] runner:bifrost-e2e (${RUNNER_DURATION}s)"
  else
    RUNNER_END_TS="$(date +%s)"
    RUNNER_DURATION=$((RUNNER_END_TS - RUNNER_START_TS))
    RUNNER_REASON="$(extract_failure_reason "$RUNNER_LOG_FILE")"
    RUNNER_REASON="$(trim_line "${RUNNER_REASON:-unknown failure}")"
    register_suite "runner:bifrost-e2e" "failed" "$RUNNER_LOG_FILE" "$RUNNER_REASON" "$RUNNER_DURATION"
    echo "[FAIL] runner:bifrost-e2e (${RUNNER_DURATION}s)"
    echo "       reason: $RUNNER_REASON"
    echo "       log: $RUNNER_LOG_FILE"
  fi
  if [[ -f "$RUNNER_LOG_FILE" ]]; then
    print_section "bifrost-e2e runner output"
    cat "$RUNNER_LOG_FILE"
  fi
  RUNNER_BG_PID=""
fi

if [[ "$RUN_UI" -eq 1 ]]; then
  header "Building frontend assets for Playwright E2E"
  if run_and_capture \
    "build:ui-frontend" \
    "$PNPM_BIN" --dir web run build; then
    ui_build_ok=1
  else
    ui_build_ok=0
  fi

  header "Building debug bifrost for Playwright E2E"
  if [[ "$ui_build_ok" -eq 1 ]]; then
    if run_and_capture \
      "build:ui-debug-bifrost" \
      env SKIP_FRONTEND_BUILD=1 CARGO_TARGET_DIR="$BIFROST_UI_TEST_TARGET_DIR" "$CARGO_BIN" build --bin bifrost; then
      ui_build_ok=1
    else
      ui_build_ok=0
    fi
  else
    ui_build_ok=0
    skip_suite "build:ui-debug-bifrost" "ui frontend build failed"
  fi

  header "Running Playwright UI E2E suite"
  if [[ "$ui_build_ok" -eq 1 ]]; then
    run_and_capture "ui:playwright" "$PNPM_BIN" --dir web run test:ui
  else
    skip_suite "ui:playwright" "ui debug build failed"
  fi
fi

print_final_report

if (( ${#SUITE_STATUSES[@]} > 0 )); then
  for status in "${SUITE_STATUSES[@]}"; do
    if [[ "$status" == "failed" ]]; then
      exit 1
    fi
  done
fi
