#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY


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
CHECK_SHELL_SHARD_BALANCE=0
PLATFORM="$(uname -s)"
REPORT_DIR=""
SHARD_INDEX="${BIFROST_E2E_SHARD_INDEX:-0}"
SHARD_TOTAL="${BIFROST_E2E_SHARD_TOTAL:-0}"
EXTRACT_FAILURE_REASON_LOG=""

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
  "test_sync_login_direct_e2e.sh"
  "test_upgrade_tls_trust_e2e.sh"
  "test_setting_ssh_key_cli.sh"
  "test_ssh_key_file_policy_migration.sh"
  "test_multiline_rule_filter_e2e.sh"
  "test_replay_rules.sh"
  "test_remote_file_api_e2e.sh"
  "test_remote_file_relay_e2e.sh"
  "test_remote_invoke_v5_session_refresh_e2e.sh"
)

# These tests spawn long-lived children that can escape the per-test subshell
# trap. They must run outside the parallel batch and receive a sandbox-scoped
# cleanup after execution, regardless of the configured shell job count.
ISOLATED_AFTER_TESTS=(
  "test_cli_start_interactive_restart_e2e.sh"
  "test_remote_connect_overload_retry_e2e.sh"
  "test_remote_invoke_e2e.sh"
  "test_remote_file_relay_e2e.sh"
  "test_remote_invoke_recent_calls_args_preview_e2e.sh"
  "test_remote_invoke_recent_calls_persistence_e2e.sh"
  "test_remote_invoke_ssh_e2e.sh"
  "test_remote_relay_tls_trust_e2e.sh"
  "test_remote_relay_url_fallback_e2e.sh"
  "test_remote_search_traffic_cli_isomorphic_e2e.sh"
  "test_client_process_transport_attribution.sh"
  "test_remote_job_real_e2e.sh"
  "test_remote_invoke_v5_session_refresh_e2e.sh"
  "test_remote_shell_exec_streaming_e2e.sh"
  "test_traffic_db_e2e.sh"
  "test_openai_like_sse_search_e2e.sh"
  "test_stop_restart_shutdown_marker.sh"
  "test_e2e_process_cleanup_isolation.sh"
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

is_working_cargo_command() {
  local candidate="$1"
  local version

  [[ -n "$candidate" ]] || return 1
  version="$("$candidate" --version 2>/dev/null || true)"
  [[ "$version" == cargo\ * ]]
}

resolve_cargo_command() {
  local candidate

  if command -v rustup >/dev/null 2>&1; then
    candidate="$(rustup which cargo 2>/dev/null || true)"
    if is_working_cargo_command "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  fi

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
    if [[ "$candidate" == *"/mise/shims/"* || "$candidate" == *"\\mise\\shims\\"* ]]; then
      continue
    fi
    if is_working_cargo_command "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done < <("$resolver" "${resolver_args[@]}" cargo 2>/dev/null)

  printf 'cargo\n'
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
  --check-shell-shard-balance
                      Print weighted shell shard loads and fail if max/min drift
                      exceeds BIFROST_E2E_SHARD_BALANCE_MAX_DIFF_PCT
                      (default: 10; capability shards: 15)
  -h, --help          Show this help

Environment variables:
  BIFROST_E2E_SHARD_INDEX  Shard index (1-indexed), same as N in --shard N/M
  BIFROST_E2E_SHARD_TOTAL  Total shards, same as M in --shard N/M
  BIFROST_E2E_SHELL_TESTS  Optional comma-separated exact shell test names
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
    --check-shell-shard-balance)
      CHECK_SHELL_SHARD_BALANCE=1
      shift
      ;;
    --extract-failure-reason)
      if [[ -z "${2:-}" ]]; then
        echo "Error: --extract-failure-reason requires a log file path" >&2
        exit 1
      fi
      EXTRACT_FAILURE_REASON_LOG="$2"
      shift 2
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

if [[ "${BIFROST_E2E_CAPABILITY_SHARDS:-0}" == "1" && "$SHARD_TOTAL" -ne 3 ]]; then
  echo "Error: capability shell sharding requires exactly 3 shards" >&2
  exit 1
fi

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
  echo "Rustc bin    : ${RUSTC:-rustc}"
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
  local interval="${BIFROST_E2E_HEARTBEAT_INTERVAL:-30}"

  while kill -0 "$command_pid" 2>/dev/null; do
    sleep "$interval"
    kill -0 "$command_pid" 2>/dev/null || break

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
    re.compile(r"^(TimeoutError|AssertionError|ReferenceError|TypeError|SyntaxError):\s*(.+)"),
    re.compile(r"^([A-Za-z][A-Za-z0-9_.]*\.(?:launch|goto|click|fill|waitFor|waitForFunction|evaluate|newPage|newContext|textContent):\s*.+)"),
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
    "preserving failed test root",
    "--- ",
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
            groups = [group for group in match.groups() if group]
            msg = " ".join(groups).strip() or stripped
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

if [[ -n "${EXTRACT_FAILURE_REASON_LOG:-}" ]]; then
  extract_failure_reason "$EXTRACT_FAILURE_REASON_LOG"
  exit 0
fi

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
  local timeout_marker="$REPORT_DIR/${log_slug}.timed-out"
  local command_status
  local pipe_path="$REPORT_DIR/${log_slug}.pipe"
  local suite_timeout="${BIFROST_E2E_SUITE_TIMEOUT:-${BIFROST_E2E_SHELL_TEST_TIMEOUT:-900}}"

  start_ts="$(date +%s)"
  rm -f "$pipe_path"
  rm -f "$timeout_marker"
  print_section "Starting ${name}"
  echo "Command : $(format_command "$@")"
  echo "Log file: $log_file"

  if is_windows; then
    : >"$log_file"
    "$@" >"$log_file" 2>&1 &
    command_pid=$!
    log_info "${name} started with pid ${command_pid}"

    (
      tail -n +1 -f "$log_file" | sed "s/^/[$name] /"
    ) &
    stream_pid=$!

    heartbeat_while_running "$name" "$command_pid" "$log_file" "$start_ts" &
    heartbeat_pid=$!

    (
      sleep "$suite_timeout"
      if kill -0 "$command_pid" 2>/dev/null; then
        echo "[TIMEOUT] ${name} exceeded ${suite_timeout}s limit, killing pid ${command_pid}" >&2
        : >"$timeout_marker"
        terminate_process_tree "$command_pid" 1
        kill_all_bifrost
      fi
    ) &
    watchdog_pid=$!

    if wait "$command_pid"; then
      status="passed"
    else
      command_status=$?
      if [[ -f "$timeout_marker" || "${command_status:-0}" -eq 143 || "${command_status:-0}" -eq 137 ]]; then
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
        : >"$timeout_marker"
        terminate_process_tree "$command_pid" 1
      fi
    ) &
    watchdog_pid=$!

    if wait "$command_pid"; then
      status="passed"
    else
      command_status=$?
      if [[ -f "$timeout_marker" || "${command_status:-0}" -eq 143 || "${command_status:-0}" -eq 137 ]]; then
        status="failed"
        reason="timed out after ${suite_timeout}s"
      else
        status="failed"
        reason="$(extract_failure_reason "$log_file")"
        reason="$(trim_line "${reason:-unknown failure}")"
      fi
    fi
  fi

  if [[ -f "$timeout_marker" ]]; then
    wait "$watchdog_pid" 2>/dev/null || true
  else
    kill "$watchdog_pid" 2>/dev/null || true
    wait "$watchdog_pid" 2>/dev/null || true
  fi

  if [[ -f "$timeout_marker" && -n "$stream_pid" ]]; then
    # A timed-out descendant may have inherited the FIFO writer. Do not let a
    # diagnostic stream outlive the per-suite budget and hang the whole job.
    sleep 1
    if kill -0 "$stream_pid" 2>/dev/null; then
      kill_process_tree "$stream_pid"
      kill "$stream_pid" 2>/dev/null || true
    fi
  elif is_windows && [[ -n "$stream_pid" ]]; then
    if kill -0 "$stream_pid" 2>/dev/null; then
      kill_process_tree "$stream_pid"
      # MSYS bash job PIDs are not always Windows process IDs. Keep the shell
      # kill fallback after taskkill so a completed command cannot leave the
      # diagnostic tail alive and block wait below until the outer job timeout.
      kill "$stream_pid" 2>/dev/null || true
    fi
  fi

  wait "$stream_pid" 2>/dev/null || true
  rm -f "$pipe_path"
  rm -f "$timeout_marker"

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
  # This full Remote Invoke relay regression connects to the deployed
  # ByteDance relay and requires a local logged-in sync token plus internal
  # network reachability. CI runners cannot access that environment, so keep it
  # as an explicit local/PPE release-gate script.
  "test_remote_invoke_ppe_full_e2e.sh"
  # System proxy tests mutate host network settings and are too flaky in
  # ephemeral CI runners. Keep them as local-only full-shell coverage.
  "test_system_proxy_e2e.sh"
  # This regression constructs a real local upgrade from a historical installed
  # binary. CI runners do not carry the previous release binary, so keep it as
  # explicit local coverage.
  "test_upgrade_local_restart_e2e.sh"
  # Aggregated security hardening wrapper runs many cargo unit filters, installer
  # checks, sync relay tests, web build, and functional shell coverage. These
  # paths are covered by dedicated CI jobs and the standalone functional shell
  # script; keep the aggregate wrapper for local/release-gate execution.
  "test_security_hardening.sh"
  # test_tls_logic_simple runs `cargo test` (debug build), redundant with the
  # dedicated `cargo test --workspace` CI job and adds 5-10 min compile time.
  "test_tls_logic_simple.sh"
  # These shell wrappers are pure Rust compile/test contract checks, not shell
  # E2E flows. Keep them local-only so CI shell jobs do not compile again after
  # the dedicated unit/integration jobs already covered the Rust paths.
  "test_chatgpt_web_shared_profile.sh"
  "test_utf8_safe_preview_e2e.sh"
  # These desktop contract/lifecycle wrappers require either a costly Tauri
  # compile or an interactive macOS WindowServer session. Keep them as explicit
  # local desktop release validation; CI still compiles the production path and
  # runs the focused ownership unit tests.
  "test_desktop_open_requests_contract.sh"
  "test_desktop_sidecar_launchd_env_contract.sh"
  "test_desktop_service_ownership_lifecycle.sh"
  # ASR/voice runtime tests may initialize local models, native audio stacks, or
  # external model downloads. Keep all ASR capability validation local-only so
  # CI never fails because a runtime dependency or model host is unavailable.
  "test_asr_admin_csrf.sh"
  "test_asr_daily_agent_template.sh"
  "test_asr_diarization_cli.sh"
  "test_asr_model_autonomy.sh"
  "test_asr_platform_gating.sh"
  "test_asr_speech_pipeline_orchestrator_real_service.sh"
  "test_asr_task_append_during_run.sh"
  "test_asr_task_cli.sh"
  "test_asr_task_pause_resume.sh"
  "test_asr_task_startup_recovery.sh"
  "test_asr_task_tui.sh"
  "test_asr_voiceprint_enroll_cli.sh"
  "test_qwen3_asr_local_server.sh"
  "test_qwen3_asr_runtime_guards.sh"
  "test_voice_input_runtime.sh"
  "test_voice_wake_actions.sh"
)

is_skipped_in_ci() {
  local name="$1"
  for skipped in "${SKIP_IN_CI_TESTS[@]}"; do
    [[ "$name" == "$skipped" ]] && return 0
  done
  return 1
}

# Approximate per-test wall-clock weight (seconds), derived from observed CI
# shell E2E run logs. Used by collect_shell_tests to balance shard durations
# via a longest-processing-time greedy partition. Unlisted tests fall back to
# a small default; exact values are not critical, only relative ordering is.
shell_test_weight() {
  case "$1" in
    test_desktop_open_requests_contract.sh) echo 620 ;;
    test_sync_github_gist_expired_status_e2e.sh) echo 302 ;;
    test_im_gateway_long_reply_delivery_regression.sh) echo 178 ;;
    test_tls_intercept_e2e.sh) echo 170 ;;
    test_skill_creator_flow.sh) echo 148 ;;
    test_remote_file_relay_e2e.sh) echo 132 ;;
    test_http3_e2e.sh) echo 120 ;;
    test_cli_online_commands_e2e.sh) echo 109 ;;
    test_client_process_transport_attribution.sh) echo 103 ;;
    test_im_online_notification_runner_context.sh) echo 87 ;;
    test_chatgpt_web_behavior_artifacts.sh) echo 85 ;;
    test_remote_invoke_e2e.sh) echo 75 ;;
    test_security_hardening_functional.sh) echo 72 ;;
    test_remote_invoke_ssh_e2e.sh) echo 59 ;;
    test_devtools_page_bridge_api.sh) echo 52 ;;
    test_group_sync_e2e.sh) echo 46 ;;
    test_upgrade_admin_api_restart_e2e.sh) echo 43 ;;
    test_replay_websocket_frames.sh) echo 41 ;;
    test_traffic_persistence_e2e.sh) echo 40 ;;
    test_group_sync_no_logstorm_e2e.sh) echo 38 ;;
    test_sse_frames.sh) echo 38 ;;
    test_body_cache_sync_cleanup_admin_api.sh) echo 33 ;;
    test_traffic_push_e2e.sh) echo 32 ;;
    test_total_size_cleanup_admin_api.sh) echo 31 ;;
    test_frames_admin_api.sh) echo 29 ;;
    test_req_res_script_e2e.sh) echo 26 ;;
    test_traffic_db_e2e.sh) echo 28 ;;
    test_large_body_protection.sh) echo 25 ;;
    test_breakpoint_performance_guard.sh) echo 24 ;;
    test_remote_search_traffic_cli_isomorphic_e2e.sh) echo 24 ;;
    test_socks5_tls_rules.sh) echo 24 ;;
    test_super_performance_mode.sh) echo 24 ;;
    test_search_traffic_cli_isomorphic_e2e.sh) echo 23 ;;
    test_im_gateway_traex_model_slash.sh) echo 22 ;;
    test_remote_shell_exec_streaming_e2e.sh) echo 22 ;;
    test_temporary_port_bindings.sh) echo 22 ;;
    test_upgrade_restart_e2e.sh) echo 22 ;;
    test_replay_rules.sh) echo 22 ;;
    test_site_docs_sync.sh) echo 21 ;;
    test_remote_job_real_e2e.sh) echo 20 ;;
    test_websocket_frames.sh) echo 17 ;;
    test_upgrade_tls_trust_e2e.sh) echo 16 ;;
    *) echo 8 ;;
  esac
}

shell_test_runs_serial_in_parallel_shell_job() {
  case "$1" in
    test_memory_pressure_e2e.sh|\
    test_large_body_protection.sh|\
    test_cli_start_interactive_restart_e2e.sh|\
    test_body_cache_sync_cleanup_admin_api.sh|\
    test_process_resolution_performance.sh|\
    test_super_performance_mode.sh|\
    test_upgrade_tls_trust_e2e.sh|\
    test_remote_connect_overload_retry_e2e.sh|\
    test_client_process_transport_attribution.sh|\
    test_remote_job_real_e2e.sh|\
    test_remote_invoke_v5_session_refresh_e2e.sh|\
    test_remote_shell_exec_streaming_e2e.sh|\
    test_traffic_db_e2e.sh|\
    test_openai_like_sse_search_e2e.sh|\
    test_stop_restart_shutdown_marker.sh|\
    test_asr_model_autonomy.sh|\
    test_asr_task_pause_resume.sh|\
    test_chatgpt_web_behavior_artifacts.sh|\
    test_http3_e2e.sh|\
    test_im_gateway_long_reply_delivery_regression.sh|\
    test_qwen3_asr_local_server.sh|\
    test_qwen3_asr_runtime_guards.sh|\
    test_skill_creator_flow.sh|\
    test_sync_login_direct_e2e.sh|\
    test_utf8_safe_preview_e2e.sh|\
    test_voice_input_runtime.sh)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

# Stable macOS capability shards. Tests stay with the product surface they
# exercise, while the existing per-test weights still balance serial and
# parallel lanes inside each capability job.
#
# 1: proxy-core   - proxy protocols, rules, traffic, TLS, admin/runtime safety
# 2: remote       - remote invoke/file/SSH/relay plus sync/install/desktop
# 3: agent-extensions - Agent, IM, skills, upgrade/UI and remaining extensions
shell_test_capability_group() {
  case "$1" in
    test_remote_*|\
    test_setting_ssh_key_cli.sh|\
    test_ssh_key_*|\
    test_status_tui_remote_invoke_panel.sh|\
    test_client_process_transport_attribution.sh|\
    test_cli_online_commands_e2e.sh|\
    test_sync_*|\
    test_group_sync_*|\
    test_security_hardening_functional.sh|\
    test_e2e_scripts_disable_sync_login_prompt.sh|\
    test_install_*|\
    test_desktop_*|\
    test_windows_*)
      echo 2
      ;;
    test_admin_*|\
    test_badge_*|\
    test_bifrost_file_syntax_admin_api.sh|\
    test_body_*|\
    test_bp_*|\
    test_breakpoint_*|\
    test_cert_*|\
    test_cli_foreground_*|\
    test_cli_offline_*|\
    test_cli_online_*|\
    test_cli_proxy_*|\
    test_cli_start_*|\
    test_client_process_*|\
    test_daemon_*|\
    test_frames_*|\
    test_header_*|\
    test_host_*|\
    test_http*|\
    test_large_body_*|\
    test_metrics_*|\
    test_multiline_rule_*|\
    test_network_account_*|\
    test_no_rule_*|\
    test_openai_like_sse_*|\
    test_pac_*|\
    test_performance_*|\
    test_port_*|\
    test_proxy_*|\
    test_replay_*|\
    test_req_res_*|\
    test_res_body_*|\
    test_rule_*|\
    test_rules_*|\
    test_script_*|\
    test_scripts_*|\
    test_search_traffic_*|\
    test_security_*|\
    test_server_config_*|\
    test_socks5_*|\
    test_sse_*|\
    test_startup_listener_*|\
    test_stop_restart_*|\
    test_super_performance_*|\
    test_system_admin_*|\
    test_temporary_port_*|\
    test_tls_*|\
    test_total_size_*|\
    test_traffic_*|\
    test_trustworthy_traffic_*|\
    test_unsafe_ssl_*|\
    test_userpass_*|\
    test_values_*|\
    test_websocket_*|\
    test_whitelist_*)
      echo 1
      ;;
    *)
      echo 3
      ;;
  esac
}

use_capability_shell_shards() {
  [[ "${BIFROST_E2E_CAPABILITY_SHARDS:-0}" == "1" && "$SHARD_TOTAL" -eq 3 ]]
}

shell_parallel_job_count() {
  local jobs="${BIFROST_E2E_SHELL_JOBS:-2}"
  if [[ ! "$jobs" =~ ^[0-9]+$ || "$jobs" -lt 1 ]]; then
    jobs=1
  fi
  echo "$jobs"
}

collect_shell_shard_assignments() {
  local all_tests=()
  local name
  while IFS= read -r name; do
    [[ -n "$name" ]] && all_tests+=("$name")
  done < <(collect_all_shell_tests)

  local parallel_jobs
  parallel_jobs="$(shell_parallel_job_count)"

  local shard_serial_load=()
  local shard_count=()
  local shard_lane_load=()
  local s lane
  for ((s = 0; s < SHARD_TOTAL; s++)); do
    shard_serial_load[s]=0
    shard_count[s]=0
    for ((lane = 0; lane < parallel_jobs; lane++)); do
      shard_lane_load[$((s * parallel_jobs + lane))]=0
    done
  done

  local weighted=()
  local phase weight
  for name in "${all_tests[@]}"; do
    weight="$(shell_test_weight "$name")"
    if shell_test_runs_serial_in_parallel_shell_job "$name"; then
      phase="serial"
    else
      phase="parallel"
    fi
    weighted+=("$(printf '%08d\t%s\t%s' "$weight" "$phase" "$name")")
  done

  local w
  while IFS=$'\t' read -r w phase name; do
    [[ -z "$name" ]] && continue
    local weight_num=$((10#$w))
    local best_shard=0
    local best_lane=0
    local best_projected=-1
    local best_current=-1
    local best_count=-1
    local shard_start=0
    local shard_end="$SHARD_TOTAL"

    if use_capability_shell_shards; then
      local capability_group
      capability_group="$(shell_test_capability_group "$name")"
      shard_start=$((capability_group - 1))
      shard_end="$capability_group"
    fi

    for ((s = shard_start; s < shard_end; s++)); do
      local max_lane_load=0
      local min_lane=0
      local min_lane_load="${shard_lane_load[$((s * parallel_jobs))]}"
      for ((lane = 0; lane < parallel_jobs; lane++)); do
        local lane_load="${shard_lane_load[$((s * parallel_jobs + lane))]}"
        (( lane_load > max_lane_load )) && max_lane_load="$lane_load"
        if [[ "$lane_load" -lt "$min_lane_load" ]]; then
          min_lane_load="$lane_load"
          min_lane="$lane"
        fi
      done

      local current_wall=$(( shard_serial_load[s] + max_lane_load ))
      local projected_wall
      local candidate_lane=0
      if [[ "$phase" == "serial" ]]; then
        projected_wall=$(( shard_serial_load[s] + weight_num + max_lane_load ))
      else
        local projected_lane_load=$(( min_lane_load + weight_num ))
        local projected_max_lane="$max_lane_load"
        (( projected_lane_load > projected_max_lane )) && projected_max_lane="$projected_lane_load"
        projected_wall=$(( shard_serial_load[s] + projected_max_lane ))
        candidate_lane="$min_lane"
      fi

      if [[ "$best_projected" -lt 0 || \
            "$projected_wall" -lt "$best_projected" || \
            ( "$projected_wall" -eq "$best_projected" && "$current_wall" -lt "$best_current" ) || \
            ( "$projected_wall" -eq "$best_projected" && "$current_wall" -eq "$best_current" && "${shard_count[s]}" -lt "$best_count" ) ]]; then
        best_projected="$projected_wall"
        best_current="$current_wall"
        best_count="${shard_count[s]}"
        best_shard="$s"
        best_lane="$candidate_lane"
      fi
    done

    if [[ "$phase" == "serial" ]]; then
      shard_serial_load[best_shard]=$(( shard_serial_load[best_shard] + weight_num ))
    else
      shard_lane_load[$((best_shard * parallel_jobs + best_lane))]=$(( shard_lane_load[$((best_shard * parallel_jobs + best_lane))] + weight_num ))
    fi
    shard_count[best_shard]=$(( shard_count[best_shard] + 1 ))
    printf '%s\t%s\t%s\t%s\t%s\n' "$((best_shard + 1))" "$best_lane" "$phase" "$weight_num" "$name"
  done < <(printf '%s\n' "${weighted[@]}" | sort -t$'\t' -k1,1nr -k3,3)
}

collect_all_shell_tests() {
  local all_tests=()
  if [[ -n "${BIFROST_E2E_SHELL_TESTS:-}" ]]; then
    local requested=()
    local name
    IFS=',' read -r -a requested <<<"$BIFROST_E2E_SHELL_TESTS"
    for name in "${requested[@]}"; do
      if [[ -z "$name" || "$name" == */* || ! -f "$E2E_DIR/tests/$name" ]]; then
        echo "Error: invalid BIFROST_E2E_SHELL_TESTS entry: $name" >&2
        return 1
      fi
      all_tests+=("$name")
    done
  elif [[ "$SHELL_MODE" == "full" ]]; then
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

  printf '%s\n' "${all_tests[@]}"
}

collect_shell_tests() {
  local all_tests=()
  while IFS= read -r name; do
    [[ -n "$name" ]] && all_tests+=("$name")
  done < <(collect_all_shell_tests)

  # Apply sharding if configured (duration-aware greedy partition).
  # CI shell jobs run safe tests in a bounded parallel batch and then run
  # lock-sensitive/cargo-heavy tests serially. Balance the estimated wall clock
  # for that execution model instead of raw total test weight.
  if [[ "$SHARD_TOTAL" -gt 0 && "$SHARD_INDEX" -gt 0 ]]; then
    local assignment_shard _assignment_lane _assignment_phase _assignment_weight
    while IFS=$'\t' read -r assignment_shard _assignment_lane _assignment_phase _assignment_weight name; do
      [[ -z "$name" ]] && continue
      if [[ "$assignment_shard" -eq "$SHARD_INDEX" ]]; then
        printf '%s\n' "$name"
      fi
    done < <(collect_shell_shard_assignments)
  else
    printf '%s\n' "${all_tests[@]}"
  fi
}

check_shell_shard_balance() {
  if [[ "$SHARD_TOTAL" -le 1 ]]; then
    echo "Error: --check-shell-shard-balance requires --shard N/M with M > 1 or BIFROST_E2E_SHARD_TOTAL > 1" >&2
    return 1
  fi

  local default_threshold=10
  if use_capability_shell_shards; then
    default_threshold=15
  fi
  local threshold="${BIFROST_E2E_SHARD_BALANCE_MAX_DIFF_PCT:-$default_threshold}"
  local all_tests=()
  local name
  while IFS= read -r name; do
    [[ -n "$name" ]] && all_tests+=("$name")
  done < <(collect_all_shell_tests)

  local parallel_jobs
  parallel_jobs="$(shell_parallel_job_count)"
  local shard_serial_load=()
  local shard_count=()
  local shard_lane_load=()
  local s
  for ((s = 0; s < SHARD_TOTAL; s++)); do
    shard_serial_load[s]=0
    shard_count[s]=0
    local lane
    for ((lane = 0; lane < parallel_jobs; lane++)); do
      shard_lane_load[$((s * parallel_jobs + lane))]=0
    done
  done

  local assignment_shard assignment_lane assignment_phase assignment_weight
  while IFS=$'\t' read -r assignment_shard assignment_lane assignment_phase assignment_weight name; do
    [[ -z "$name" ]] && continue
    local shard_index=$(( assignment_shard - 1 ))
    if [[ "$assignment_phase" == "serial" ]]; then
      shard_serial_load[shard_index]=$(( shard_serial_load[shard_index] + assignment_weight ))
    else
      shard_lane_load[$((shard_index * parallel_jobs + assignment_lane))]=$(( shard_lane_load[$((shard_index * parallel_jobs + assignment_lane))] + assignment_weight ))
    fi
    shard_count[shard_index]=$(( shard_count[shard_index] + 1 ))
  done < <(collect_shell_shard_assignments)

  local shard_wall=()
  local min=-1
  local max=0
  local total=0
  for ((s = 0; s < SHARD_TOTAL; s++)); do
    local max_lane_load=0
    local lane_loads=()
    for ((lane = 0; lane < parallel_jobs; lane++)); do
      local lane_load="${shard_lane_load[$((s * parallel_jobs + lane))]}"
      lane_loads+=("$lane_load")
      (( lane_load > max_lane_load )) && max_lane_load="$lane_load"
    done
    shard_wall[s]=$(( shard_serial_load[s] + max_lane_load ))
    [[ "$min" -lt 0 || "${shard_wall[s]}" -lt "$min" ]] && min="${shard_wall[s]}"
    (( shard_wall[s] > max )) && max="${shard_wall[s]}"
    total=$(( total + shard_wall[s] ))
    echo "shard $((s + 1))/$SHARD_TOTAL estimated_wall=${shard_wall[s]}s serial=${shard_serial_load[s]}s parallel_lanes=$(IFS=,; echo "${lane_loads[*]}") tests=${shard_count[s]}"
  done

  local diff=$(( max - min ))
  local pct
  pct="$(awk -v diff="$diff" -v total="$total" -v count="$SHARD_TOTAL" 'BEGIN { avg = total / count; if (avg > 0) { printf "%.1f", diff / avg * 100 } else { printf "0.0" } }')"
  echo "shell shard balance: estimated_wall_diff=${diff}s pct_of_avg=${pct}% threshold=${threshold}% parallel_jobs=${parallel_jobs}"

  awk -v pct="$pct" -v threshold="$threshold" 'BEGIN { exit (pct <= threshold ? 0 : 1) }'
}

if [[ "$LIST_SHELL_TESTS" -eq 1 ]]; then
  collect_shell_tests
  exit 0
fi

if [[ "$CHECK_SHELL_SHARD_BALANCE" -eq 1 ]]; then
  check_shell_shard_balance
  exit $?
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

  write_machine_report
}

write_machine_report() {
  local ledger="$REPORT_DIR/summary.tsv"
  local output="${BIFROST_E2E_SUMMARY_JSON:-$REPORT_DIR/summary.json}"
  local i

  : > "$ledger"
  for i in "${!SUITE_NAMES[@]}"; do
    printf '%s\t%s\t%s\t%s\t%s\n' \
      "${SUITE_STATUSES[$i]}" \
      "$(tsv_clean "${SUITE_NAMES[$i]}")" \
      "${SUITE_DURATIONS[$i]}" \
      "$(tsv_clean "${SUITE_LOGS[$i]}")" \
      "$(tsv_clean "${SUITE_REASONS[$i]}")" >> "$ledger"
  done

  python3 scripts/ci/e2e-summary.py "$ledger" "$output" \
    --metadata "platform=$PLATFORM" \
    --metadata "mode=$MODE" \
    --metadata "shell_mode=$SHELL_MODE" \
    --metadata "shard=${SHARD_INDEX}/${SHARD_TOTAL}"
}

tsv_clean() {
  local value="$1"
  value="${value//$'\t'/ }"
  value="${value//$'\n'/\\n}"
  printf '%s' "$value"
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

  # These tests transfer very large payloads and can produce 100MB+ responses.
  # Keep them out of the parallel shell batch so hosted runners do not drop the
  # proxy mid-suite under resource pressure.
  local RESOURCE_HEAVY_TESTS=(
    "test_large_body_protection.sh"
  )

  # These tests own a long-lived Python fixture and wait for it during a
  # hosted-runner cold start. macOS CI showed that starting them beside another
  # proxy-heavy suite can leave the child alive but unscheduled long enough to
  # miss its readiness deadline. Keep them in the serial lane so readiness
  # measures the fixture itself rather than sibling resource contention.
  local STARTUP_SENSITIVE_TESTS=(
    "test_body_cache_sync_cleanup_admin_api.sh"
    "test_process_resolution_performance.sh"
    "test_super_performance_mode.sh"
    "test_upgrade_tls_trust_e2e.sh"
  )

  # PR-G-CI-FIX: isolated-after tests
  # These tests spawn long-lived bifrost/python children that escape the
  # per-test subshell trap. Run serially and clean only Bifrost PIDs recorded
  # under this run's marked sandbox. Never kill host-wide Bifrost processes.
  # Remote shell streaming owns a relay, target bifrost, caller process, and
  # SSE worker; traffic DB and OpenAI-like SSE search own bifrost processes
  # plus mock traffic generators. Linux CI has observed these tests stall when
  # they run inside the parallel shell batch under shard load.
  # Remote invoke pairing/file/SSH suites also share relay grant state, caller
  # connection caches, and short-lived local relay processes. Keep the whole
  # remote pairing family out of the parallel batch; otherwise one sibling can
  # revoke grants, occupy pair slots, or race a local relay callback while
  # another sibling is approving a pairing.
  # Some shell tests run cargo check/test/run internally. If they run inside the
  # parallel batch, they can spend most of the per-test timeout blocked on
  # Cargo's shared target artifact lock while sibling tests compile. Keep them
  # serial so the timeout measures test work instead of lock contention.
  local CARGO_HEAVY_TESTS=(
    "test_asr_model_autonomy.sh"
    "test_asr_task_pause_resume.sh"
    "test_chatgpt_web_behavior_artifacts.sh"
    "test_client_process_transport_attribution.sh"
    "test_http3_e2e.sh"
    "test_im_gateway_long_reply_delivery_regression.sh"
    "test_qwen3_asr_local_server.sh"
    "test_qwen3_asr_runtime_guards.sh"
    "test_skill_creator_flow.sh"
    "test_sync_login_direct_e2e.sh"
    "test_utf8_safe_preview_e2e.sh"
    "test_voice_input_runtime.sh"
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

    local is_resource_heavy=0
    for rt in "${RESOURCE_HEAVY_TESTS[@]}"; do
      if [[ "$script_name" == "$rt" ]]; then
        is_resource_heavy=1
        break
      fi
    done

    local is_startup_sensitive=0
    for st in "${STARTUP_SENSITIVE_TESTS[@]}"; do
      if [[ "$script_name" == "$st" ]]; then
        is_startup_sensitive=1
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

    local is_cargo_heavy=0
    for ct in "${CARGO_HEAVY_TESTS[@]}"; do
      if [[ "$script_name" == "$ct" ]]; then
        is_cargo_heavy=1
        break
      fi
    done

    if [[ "$is_mock_managing" -eq 1 || "$is_resource_heavy" -eq 1 || "$is_startup_sensitive" -eq 1 || "$is_isolated_after" -eq 1 || "$is_cargo_heavy" -eq 1 ]]; then
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
    header "Running ${#serial_tests[@]} lock-sensitive shell tests serially"
    for script_name in "${serial_tests[@]}"; do
      log_info "Queue serial shell test: $script_name"
      run_shell_test_isolated "$script_name"
    done
  fi

  return 0
}

run_shell_test_isolated() {
  local script_name="$1"
  local cleanup_after=0
  local isolated_test
  for isolated_test in "${ISOLATED_AFTER_TESTS[@]}"; do
    if [[ "$script_name" == "$isolated_test" ]]; then
      cleanup_after=1
      break
    fi
  done

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
  mark_e2e_data_root "$shell_data_dir"

  local echo_http="$((shell_port + 1))"
  local echo_https="$((shell_port + 2))"
  local echo_ws="$((shell_port + 3))"
  local echo_wss="$((shell_port + 4))"
  local echo_sse="$((shell_port + 5))"
  local socks5_port="$((shell_port + 6))"
  local echo_proxy="$((shell_port + 7))"

  local test_status=0
  run_and_capture "shell:${script_name}" \
    env \
      BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}" \
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
    bash "$E2E_DIR/tests/$script_name" || test_status=$?

  if [[ "$cleanup_after" -eq 1 ]]; then
    # The directory and its ownership marker are local to this function, so
    # cleanup cannot accidentally target another checkout or the live service.
    echo "[CLEANUP] post ${script_name}: killing sandbox-owned bifrost processes"
    kill_bifrost_in_data_root "$shell_data_dir" 2>/dev/null || true
  fi

  return "$test_status"
}

run_shell_batch_parallel() {
  local max_jobs="$1"
  local base_port="$2"
  local port_step="$3"

  local pids=()
  local pid_child_files=()
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
      local child_pid_file="$REPORT_DIR/${log_slug}.child.pid"
      local start_ts
      start_ts="$(date +%s)"

      log_info "Starting shell test $script_name (port=$shell_port, index=$next_index)"

      (
        shell_data_dir="$(mktemp -d "$E2E_SANDBOX_DIR/shell-${log_slug}-XXXXXX")"
        mark_e2e_data_root "$shell_data_dir"
        trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$shell_data_dir" 2>/dev/null || true' EXIT
        if command -v setsid >/dev/null 2>&1; then
          setsid -w env \
            BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}" \
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
            bash "$E2E_DIR/tests/$script_name" &
        else
          env \
            BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}" \
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
            bash "$E2E_DIR/tests/$script_name" &
        fi
        child_pid=$!
        echo "$child_pid" >"$child_pid_file"
        wait "$child_pid"
      ) > "$log_file" 2>&1 &

      pids[$next_index]=$!
      pid_child_files[$next_index]="$child_pid_file"
      pid_scripts[$next_index]="$script_name"
      pid_logs[$next_index]="$log_file"
      pid_starts[$next_index]="$start_ts"
      running=$((running + 1))
      next_index=$((next_index + 1))
    done

    # PR-G-CI-FIX: per-test timeout. On Linux, child tests run under setsid so
    # timeout cleanup can terminate the entire child process group without
    # affecting sibling parallel tests. Other platforms fall back to direct pid.
    local shell_per_test_timeout="${BIFROST_E2E_SHELL_TEST_TIMEOUT:-900}"
    local now_ts
    now_ts="$(date +%s)"
    for i in "${!pids[@]}"; do
      if [[ -z "${pids[$i]:-}" ]]; then
        continue
      fi
      local this_pid="${pids[$i]}"
      local this_child_pid=""
      if [[ -n "${pid_child_files[$i]:-}" && -f "${pid_child_files[$i]}" ]]; then
        this_child_pid="$(cat "${pid_child_files[$i]}" 2>/dev/null || true)"
      fi
      local this_start="${pid_starts[$i]}"
      local this_age=$((now_ts - this_start))
      if kill -0 "$this_pid" 2>/dev/null && [[ "$this_age" -gt "$shell_per_test_timeout" ]]; then
        echo "[TIMEOUT] shell:${pid_scripts[$i]} exceeded ${shell_per_test_timeout}s, age=${this_age}s pid=${this_pid}"
        if [[ -n "$this_child_pid" ]]; then
          kill -TERM "-$this_child_pid" 2>/dev/null || kill -TERM "$this_child_pid" 2>/dev/null || true
        fi
        kill -TERM "$this_pid" 2>/dev/null || true
        sleep 1
        if [[ -n "$this_child_pid" ]]; then
          kill -KILL "-$this_child_pid" 2>/dev/null || kill -KILL "$this_child_pid" 2>/dev/null || true
        fi
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
        [[ -n "${pid_child_files[$i]:-}" ]] && rm -f "${pid_child_files[$i]}" 2>/dev/null || true
        unset 'pid_child_files[i]'
        completed=$((completed + 1))
        running=$((running - 1))
      fi
    done

    if [[ $running -gt 0 ]]; then
      sleep 0.2
    fi
  done

  return 0
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
export CARGO_BIN="${CARGO_BIN:-$(resolve_cargo_command)}"
if [[ -z "${RUSTC:-}" ]] && command -v rustup >/dev/null 2>&1; then
  RUSTC="$(rustup which rustc 2>/dev/null || true)"
  if [[ -n "$RUSTC" ]]; then
    export RUSTC
  else
    unset RUSTC
  fi
fi
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
export BIFROST_E2E_SANDBOX_DIR="$E2E_SANDBOX_DIR"
source "$E2E_DIR/test_utils/process.sh" || exit 1
mark_e2e_data_root "$E2E_SANDBOX_DIR" || exit 1

export HOME="${HOME:-$E2E_SANDBOX_DIR/home}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$E2E_SANDBOX_DIR/xdg-config}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$E2E_SANDBOX_DIR/xdg-data}"
export PATH="$ROOT_DIR/e2e-tests/bin:$(dirname "$CARGO_BIN"):$(dirname "$NODE_BIN"):$(dirname "$PNPM_BIN"):$PATH"

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
    _prebuilt="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
    if is_windows && [[ -z "${BIFROST_BIN:-}" ]]; then
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
  if [[ -n "${BIFROST_E2E_BIN:-}" ]]; then
    if [[ ! -x "$BIFROST_E2E_BIN" ]]; then
      register_suite \
        "runner:bifrost-e2e" \
        "failed" \
        "" \
        "BIFROST_E2E_BIN is not executable: $BIFROST_E2E_BIN" \
        "0"
    else
      run_and_capture \
        "runner:bifrost-e2e" \
        "$BIFROST_E2E_BIN" --port "$BIFROST_UI_TEST_RUNNER_PORT" --jobs "$RUNNER_JOBS" --timeout "$RUNNER_TIMEOUT"
    fi
  else
    run_and_capture \
      "runner:bifrost-e2e" \
      "$CARGO_BIN" run -p bifrost-e2e -- --port "$BIFROST_UI_TEST_RUNNER_PORT" --jobs "$RUNNER_JOBS" --timeout "$RUNNER_TIMEOUT"
  fi
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
    if [[ "${BIFROST_UI_TEST_PROFILE:-full}" == "critical" ]]; then
      run_and_capture "ui:playwright-critical" bash scripts/ci/run-ui-critical.sh
    else
      run_and_capture "ui:playwright" "$PNPM_BIN" --dir web run test:ui
    fi
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
