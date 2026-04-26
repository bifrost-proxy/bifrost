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
LIST_SHELL_TESTS=0
PLATFORM="$(uname -s)"
REPORT_DIR=""
SHARD_INDEX="${BIFROST_E2E_SHARD_INDEX:-0}"
SHARD_TOTAL="${BIFROST_E2E_SHARD_TOTAL:-0}"

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
  "test_proxy_chain_auth_e2e.sh"
  "test_cert_admin_api.sh"
  "test_performance_config_admin_api.sh"
  "test_metrics_hosts_apps_admin_api.sh"
  "test_tls_intercept_mode_api.sh"
  "test_bifrost_file_syntax_admin_api.sh"
  "test_multiline_rule_filter_e2e.sh"
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
  --shard N/M         Run only shard N of M (1-indexed). E.g. --shard 1/3
  --list-shell-tests  Print the shell tests selected by the current mode/shard and exit
  -h, --help          Show this help

Environment variables:
  BIFROST_E2E_SHARD_INDEX  Shard index (1-indexed), same as N in --shard N/M
  BIFROST_E2E_SHARD_TOTAL  Total shards, same as M in --shard N/M
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
    --list-shell-tests)
      LIST_SHELL_TESTS=1
      shift
      ;;
    --shard)
      if [[ -z "${2:-}" || ! "$2" =~ ^[0-9]+/[0-9]+$ ]]; then
        echo "Error: --shard requires N/M format (e.g. --shard 1/3)" >&2
        exit 1
      fi
      SHARD_INDEX="${2%%/*}"
      SHARD_TOTAL="${2##*/}"
      shift 2
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
  if [[ "$SHARD_TOTAL" -gt 0 ]]; then
    echo "Shard        : $SHARD_INDEX/$SHARD_TOTAL"
  fi
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

  local py=""
  if command -v python3_cmd >/dev/null 2>&1; then
    py="$(python3_cmd 2>/dev/null || true)"
  fi
  if [[ -z "${py:-}" ]]; then
    if command -v python3 >/dev/null 2>&1; then
      py="python3"
    elif command -v python >/dev/null 2>&1; then
      py="python"
    else
      return 0
    fi
  fi

  "$py" - "$log_file" <<'PY'
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
  # System proxy tests mutate host network settings and are too flaky in
  # ephemeral CI runners. Keep them as local-only full-shell coverage.
  "test_system_proxy_e2e.sh"
  # test_tls_logic_simple runs `cargo test` (debug build), redundant with the
  # dedicated `cargo test --workspace` CI job and adds 5-10 min compile time.
  "test_tls_logic_simple.sh"
)

is_skipped_in_ci() {
  local name="$1"
  for skipped in "${SKIP_IN_CI_TESTS[@]}"; do
    [[ "$name" == "$skipped" ]] && return 0
  done
  return 1
}

collect_shell_tests() {
  local all_tests=()
  if [[ "$SHELL_MODE" == "full" ]]; then
    while IFS= read -r script_path; do
      local name
      name="$(basename "$script_path")"
      if [[ "$MODE" == "ci" ]] && is_skipped_in_ci "$name"; then
        continue
      fi
      all_tests+=("$name")
    done < <(find "$E2E_DIR/tests" -maxdepth 1 -type f -name 'test_*.sh' -print | sort)
  else
    all_tests=("${STABLE_SHELL_TESTS[@]}")
  fi

  # Apply sharding if configured
  if [[ "$SHARD_TOTAL" -gt 0 && "$SHARD_INDEX" -gt 0 ]]; then
    local i=0
    for name in "${all_tests[@]}"; do
      if [[ $(( (i % SHARD_TOTAL) + 1 )) -eq "$SHARD_INDEX" ]]; then
        printf '%s\n' "$name"
      fi
      i=$((i + 1))
    done
  else
    printf '%s\n' "${all_tests[@]}"
  fi
}

if [[ "$LIST_SHELL_TESTS" -eq 1 ]]; then
  collect_shell_tests
  exit 0
fi

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
  local shell_base_port="${BIFROST_E2E_SHELL_BASE_PORT:-0}"
  local port_step=10

  local serial_tests=()
  local parallel_tests=()

  local MOCK_MANAGING_TESTS=(
    "test_memory_pressure_e2e.sh"
  )

  # PR-G-CI-FIX: isolated-after tests
  # These tests spawn long-lived bifrost/python children that escape the
  # per-test subshell trap. Run serially and call kill_all_bifrost after each
  # to prevent orphan processes from holding the parent job's wait/cleanup.
  local ISOLATED_AFTER_TESTS=(
    "test_remote_connect_overload_retry_e2e.sh"
    "test_client_process_transport_attribution.sh"
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

    # PR-G-CI-FIX: isolated-after tests
    local is_isolated_after=0
    for it in "${ISOLATED_AFTER_TESTS[@]}"; do
      if [[ "$script_name" == "$it" ]]; then
        is_isolated_after=1
        break
      fi
    done

    if [[ "$is_mock_managing" -eq 1 || "$is_isolated_after" -eq 1 ]]; then
      serial_tests+=("$script_name")
    else
      parallel_tests+=("$script_name")
    fi
  done

  if [[ ${#parallel_tests[@]} -gt 0 ]]; then
    header "Running ${#parallel_tests[@]} safe shell tests in parallel (jobs=$max_jobs)"
    local span=$(( (${#parallel_tests[@]} - 1) * port_step + 12 ))
    shell_base_port="$(pick_available_base_port "$shell_base_port" "$span")" || true
    if [[ -z "${shell_base_port:-}" || "${shell_base_port:-0}" -lt 1024 ]]; then
      shell_base_port=15000
      log_warn "pick_available_base_port failed for shell parallel tests, falling back to $shell_base_port"
    fi
    _SHELL_BATCH_LIST=("${parallel_tests[@]}")
    run_shell_batch_parallel "$max_jobs" "$shell_base_port" "$port_step"
  fi

  if [[ ${#serial_tests[@]} -gt 0 ]]; then
    header "Running ${#serial_tests[@]} mock-managing shell tests serially"
    for script_name in "${serial_tests[@]}"; do
      log_info "Queue serial shell test: $script_name"
      run_shell_test_isolated "$script_name"
      # PR-G-CI-FIX: isolated-after tests - clean up any orphan bifrost procs
      for it in "${ISOLATED_AFTER_TESTS[@]}"; do
        if [[ "$script_name" == "$it" ]]; then
          echo "[CLEANUP] post ${script_name}: killing residual bifrost processes"
          kill_all_bifrost 2>/dev/null || true
          break
        fi
      done
    done
  fi
}

run_shell_test_isolated() {
  local script_name="$1"

  # Allocate a small contiguous span and derive service ports from it.
  local base
  base="$(pick_available_base_port 0 32)" || true
  if [[ -z "${base:-}" || "${base:-0}" -lt 1024 ]]; then
    base=16000
    log_warn "pick_available_base_port failed for shell isolated test, falling back to $base"
  fi
  local shell_port="$base"

  local shell_data_dir
  mkdir -p "$E2E_SANDBOX_DIR" 2>/dev/null || true
  shell_data_dir="$(mktemp -d "$E2E_SANDBOX_DIR/shell-${script_name//\//_}-XXXXXX")"

  local echo_http="$((shell_port + 1))"
  local echo_https="$((shell_port + 2))"
  local echo_ws="$((shell_port + 3))"
  local echo_wss="$((shell_port + 4))"
  local echo_sse="$((shell_port + 5))"
  local socks5_port="$((shell_port + 6))"
  local echo_proxy="$((shell_port + 7))"

  run_and_capture "shell:${script_name}" \
    env \
      ADMIN_PORT="$shell_port" \
      ADMIN_HOST="127.0.0.1" \
      PROXY_PORT="$shell_port" \
      PROXY_HOST="127.0.0.1" \
      ECHO_HTTP_PORT="$echo_http" \
      HTTP_PORT="$echo_http" \
      MOCK_HTTP_PORT="$echo_http" \
      ECHO_HTTPS_PORT="$echo_https" \
      HTTPS_PORT="$echo_https" \
      HTTPS_MOCK_PORT="$echo_https" \
      ECHO_WS_PORT="$echo_ws" \
      WS_PORT="$echo_ws" \
      MOCK_WS_PORT="$echo_ws" \
      ECHO_WSS_PORT="$echo_wss" \
      WSS_PORT="$echo_wss" \
      ECHO_SSE_PORT="$echo_sse" \
      SSE_PORT="$echo_sse" \
      MOCK_SSE_PORT="$echo_sse" \
      SOCKS5_PORT="$socks5_port" \
      MOCK_ECHO_PROXY_PORT="$echo_proxy" \
      ECHO_PROXY_PORT="$echo_proxy" \
      SERVER_LOG_DIR="$shell_data_dir/mock-logs" \
      BIFROST_DATA_DIR="$shell_data_dir" \
      SKIP_BUILD=true \
    bash "$E2E_DIR/tests/$script_name"
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
      local log_slug
      log_slug="$(printf 'shell_%s' "$script_name" | tr ' /:.' '____' | tr -cd '[:alnum:]_.-')"
      local log_file="$REPORT_DIR/${log_slug}.log"
      local start_ts
      start_ts="$(date +%s)"

      log_info "Starting shell test $script_name (port=$shell_port, index=$next_index)"

      (
        shell_data_dir="$(mktemp -d "$E2E_SANDBOX_DIR/shell-${log_slug}-XXXXXX")"
        trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$shell_data_dir" 2>/dev/null || true' EXIT
        ADMIN_PORT="$shell_admin_port" \
        ADMIN_HOST="127.0.0.1" \
        PROXY_PORT="$shell_port" \
        PROXY_HOST="127.0.0.1" \
        ECHO_HTTP_PORT="$((shell_port + 1))" \
        HTTP_PORT="$((shell_port + 1))" \
        MOCK_HTTP_PORT="$((shell_port + 1))" \
        ECHO_HTTPS_PORT="$((shell_port + 2))" \
        HTTPS_PORT="$((shell_port + 2))" \
        HTTPS_MOCK_PORT="$((shell_port + 2))" \
        ECHO_WS_PORT="$((shell_port + 3))" \
        WS_PORT="$((shell_port + 3))" \
        MOCK_WS_PORT="$((shell_port + 3))" \
        ECHO_WSS_PORT="$((shell_port + 4))" \
        WSS_PORT="$((shell_port + 4))" \
        ECHO_SSE_PORT="$((shell_port + 5))" \
        SSE_PORT="$((shell_port + 5))" \
        MOCK_SSE_PORT="$((shell_port + 5))" \
        SOCKS5_PORT="$((shell_port + 6))" \
        MOCK_ECHO_PROXY_PORT="$((shell_port + 7))" \
        ECHO_PROXY_PORT="$((shell_port + 7))" \
        BIFROST_DATA_DIR="$shell_data_dir" \
        SERVER_LOG_DIR="$shell_data_dir/mock-logs" \
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

    # PR-G-CI-FIX: per-test timeout (kill direct pid only; avoid PGID
    # because children do not run in independent process groups and a
    # group kill would take out sibling parallel tests).
    local shell_per_test_timeout="${BIFROST_E2E_SHELL_TEST_TIMEOUT:-900}"
    local now_ts
    now_ts="$(date +%s)"
    for i in "${!pids[@]}"; do
      if [[ -z "${pids[$i]:-}" ]]; then
        continue
      fi
      local this_pid="${pids[$i]}"
      local this_start="${pid_starts[$i]}"
      local this_age=$((now_ts - this_start))
      if kill -0 "$this_pid" 2>/dev/null && [[ "$this_age" -gt "$shell_per_test_timeout" ]]; then
        echo "[TIMEOUT] shell:${pid_scripts[$i]} exceeded ${shell_per_test_timeout}s, age=${this_age}s pid=${this_pid}"
        kill -TERM "$this_pid" 2>/dev/null || true
        sleep 1
        kill -KILL "$this_pid" 2>/dev/null || true
      fi
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

E2E_SANDBOX_DIR="${BIFROST_E2E_SANDBOX_DIR:-}"
E2E_SANDBOX_AUTO="false"

e2e_cleanup() {
  set +e
  # Best-effort cleanup: background jobs + sandbox dir.
  kill $(jobs -p) 2>/dev/null || true

  if [[ "${E2E_SANDBOX_AUTO:-false}" == "true" ]] && [[ -n "${E2E_SANDBOX_DIR:-}" ]]; then
    rm -rf "$E2E_SANDBOX_DIR" 2>/dev/null || true
  fi
}

trap e2e_cleanup EXIT

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
export CARGO_BIN="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
export NODE_BIN="${NODE_BIN:-$(resolve_non_shim_command node)}"
export PNPM_BIN="${PNPM_BIN:-$(resolve_non_shim_command pnpm)}"
export BIFROST_UI_TEST_TARGET_DIR="${BIFROST_UI_TEST_TARGET_DIR:-$ROOT_DIR/.bifrost-ui-target}"
export BIFROST_UI_TEST_RUNNER_PORT="${BIFROST_UI_TEST_RUNNER_PORT:-}"
export BIFROST_E2E_ROOT="$ROOT_DIR"
mkdir -p "$ROOT_DIR/.bifrost-e2e-runs" 2>/dev/null || true
if [[ -z "${E2E_SANDBOX_DIR:-}" ]]; then
  E2E_SANDBOX_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-runs/sandbox-XXXXXX")"
  E2E_SANDBOX_AUTO="true"
fi

export HOME="${HOME:-$E2E_SANDBOX_DIR/home}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$E2E_SANDBOX_DIR/xdg-config}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$E2E_SANDBOX_DIR/xdg-data}"
export PATH="$ROOT_DIR/e2e-tests/bin:$(dirname "$CARGO_BIN"):$(dirname "$NODE_BIN"):$(dirname "$PNPM_BIN"):$PATH"
source "$E2E_DIR/test_utils/process.sh"

mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
if [[ -z "${BIFROST_UI_TEST_RUNNER_PORT:-}" ]]; then
  RUNNER_PORT_SPAN="${BIFROST_E2E_RUNNER_PORT_SPAN:-512}"
  BIFROST_UI_TEST_RUNNER_PORT="$(pick_available_base_port 0 "$RUNNER_PORT_SPAN")" || true
  if [[ -z "${BIFROST_UI_TEST_RUNNER_PORT:-}" || "${BIFROST_UI_TEST_RUNNER_PORT:-0}" -lt 1024 ]]; then
    BIFROST_UI_TEST_RUNNER_PORT=18080
    log_warn "pick_available_base_port failed or returned privileged port, falling back to $BIFROST_UI_TEST_RUNNER_PORT"
  fi
  export BIFROST_UI_TEST_RUNNER_PORT
fi

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

RUNNER_TIMEOUT="${BIFROST_E2E_RUNNER_TIMEOUT:-2400}"
RUNNER_JOBS="${BIFROST_E2E_RUNNER_JOBS:-1}"

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
        run_shell_test_isolated "$script_name"
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

if [[ "$RUN_RUNNER" -eq 1 ]]; then
  header "Running bifrost-e2e custom runner"

  # Avoid interference with shell/rules suites (some tests use broad cleanup like pkill).
  # Also give runner its own extended suite timeout.
  _prev_suite_timeout="${BIFROST_E2E_SUITE_TIMEOUT:-}"
  export BIFROST_E2E_SUITE_TIMEOUT="$RUNNER_TIMEOUT"
  run_and_capture \
    "runner:bifrost-e2e" \
    "$CARGO_BIN" run -p bifrost-e2e -- --port "$BIFROST_UI_TEST_RUNNER_PORT" --jobs "$RUNNER_JOBS" --timeout "$RUNNER_TIMEOUT"
  if [[ -n "${_prev_suite_timeout:-}" ]]; then
    export BIFROST_E2E_SUITE_TIMEOUT="$_prev_suite_timeout"
  else
    unset BIFROST_E2E_SUITE_TIMEOUT
  fi
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
