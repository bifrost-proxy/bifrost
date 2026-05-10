#!/usr/bin/env bash

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

RELAY_PORT=""
RELAY_PID=""
RELAY_LOG=""
RELAY_URL=""

TARGET_DATA_DIR=""
CALLER_DATA_DIR=""
CALLER_CONNECT_PID=""
CALLER_CONNECT_LOG=""

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""
CLIENT_ADMIN_URL=""
CLIENT_INSTANCE_ID=""
CLIENT_INSTANCE_SHORT=""
GRANT_ID=""
PYTHON_BIN=""

log() {
    echo "[remote-shell-stream-e2e] $*"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

http_request() {
    local url="$1"
    local method="${2:-GET}"
    local data="${3:-}"

    local headers_file body_file err_file
    headers_file="$(mktemp)"
    body_file="$(mktemp)"
    err_file="$(mktemp)"

    local curl_args=(
        -sS
        -X "$method"
        --max-time 20
        -D "$headers_file"
        -o "$body_file"
        -w '%{http_code}'
    )

    if [[ -n "$data" ]]; then
        curl_args+=(-H "Content-Type: application/json" --data-binary "$data")
    fi

    HTTP_STATUS="$(curl "${curl_args[@]}" "$url" 2>"$err_file")" || HTTP_STATUS="000"
    HTTP_HEADERS="$(tr -d '\r' <"$headers_file")"
    HTTP_BODY="$(cat "$body_file")"

    rm -f "$headers_file" "$body_file" "$err_file"
}

http_get() {
    http_request "$1" "GET"
}

http_post_json() {
    http_request "$1" "POST" "$2"
}

http_patch_json() {
    http_request "$1" "PATCH" "$2"
}

prepare_bifrost_bin() {
    BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
    if [[ "$BIFROST_BIN" == "$REPO_DIR/target/release/bifrost" && "${SKIP_BUILD:-}" != "true" ]]; then
        local need_build=0
        if [[ ! -x "$BIFROST_BIN" ]] \
            || [[ "$REPO_DIR/Cargo.toml" -nt "$BIFROST_BIN" ]] \
            || [[ "$REPO_DIR/Cargo.lock" -nt "$BIFROST_BIN" ]]; then
            need_build=1
        elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$BIFROST_BIN" -print -quit | grep -q .; then
            need_build=1
        fi

        if [[ "$need_build" -eq 1 ]]; then
            log "Building release bifrost binary..."
            (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null)
        fi
    fi

    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "bifrost binary not found at $BIFROST_BIN" >&2
        exit 1
    fi
}

start_local_relay() {
    RELAY_PORT="$(pick_free_port)"
    RELAY_LOG="$(mktemp)"
    local relay_data_dir
    relay_data_dir="$(mktemp -d)"
    local relay_exec
    relay_exec="$(sync_server_exec "$SYNC_SERVER_DIR")"

    log "Starting relay on port $RELAY_PORT..."
    (
        cd "$SYNC_SERVER_DIR" && \
            eval "$relay_exec" -p "$RELAY_PORT" -d "$relay_data_dir" --enable-remote-invoke
    ) >"$RELAY_LOG" 2>&1 &
    RELAY_PID=$!
    RELAY_URL="http://127.0.0.1:${RELAY_PORT}"

    local waited=0
    while [[ $waited -lt 120 ]]; do
        if curl -s -o /dev/null -w '%{http_code}' \
            "${RELAY_URL}/v4/remote-invoke/client/register" 2>/dev/null | grep -q "4[0-9][0-9]\|200"; then
            log "Relay ready (pid=$RELAY_PID)"
            return 0
        fi
        if ! kill -0 "$RELAY_PID" 2>/dev/null; then
            echo "Relay exited early:" >&2
            cat "$RELAY_LOG" >&2 || true
            exit 1
        fi
        sleep 0.5
        waited=$((waited + 1))
    done

    echo "Relay did not become ready:" >&2
    tail -50 "$RELAY_LOG" >&2 || true
    exit 1
}

wait_for_worker_ready() {
    local ready=0
    for _ in $(seq 1 30); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/status"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            local state
            state="$(echo "$HTTP_BODY" | jq -r '.state // ""')"
            if [[ "$state" == "Connected" ]]; then
                ready=1
                break
            fi
        fi
        sleep 1
    done

    if [[ "$ready" -eq 1 ]]; then
        _log_pass "remote invoke worker connected to relay"
    else
        _log_fail "remote invoke worker connected to relay" "Connected" "${HTTP_BODY:-<empty>}"
        return 1
    fi
}

configure_shell_policies() {
    log "Configuring target Shell Access policies..."
    BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" setting shell policy add \
        --id stream-shell \
        --name "Stream Shell" \
        --mode shell_text \
        --pattern '^(?s:.*)$' \
        --shell /bin/sh \
        --stdin \
        --timeout-ms 10000 >/dev/null

    BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" setting shell policy add \
        --id stream-argv \
        --name "Stream Argv" \
        --mode argv_exec \
        --program "$PYTHON_BIN" \
        --timeout-ms 10000 >/dev/null

    _log_pass "target Shell Access includes shell_text and argv_exec streaming policies"
}

pair_and_upgrade_grant() {
    local sync_user_id sync_password pair_code pairing_id

    sync_user_id="remote_shell_stream_${RANDOM}"
    sync_password="remote_shell_stream_123"

    http_post_json "${RELAY_URL}/v4/sso/register" \
        "{\"user_id\":\"${sync_user_id}\",\"password\":\"${sync_password}\",\"nickname\":\"Remote Shell Stream E2E\"}"
    assert_status "200" "$HTTP_STATUS" "relay 注册测试用户应返回 200" || return 1

    local relay_sync_token
    relay_sync_token="$(echo "$HTTP_BODY" | jq -r '.data.token // ""')"
    assert_not_empty "$relay_sync_token" "relay 注册后 token 不应为空" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/sync/session" "{\"token\":\"${relay_sync_token}\"}"
    assert_status "200" "$HTTP_STATUS" "保存 sync session 应返回 200" || return 1

    wait_for_worker_ready || return 1

    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/identity"
    assert_status "200" "$HTTP_STATUS" "remote-invoke identity 应返回 200" || return 1
    CLIENT_INSTANCE_ID="$(echo "$HTTP_BODY" | jq -r '.instance_id // ""')"
    CLIENT_INSTANCE_SHORT="${CLIENT_INSTANCE_ID:0:12}"
    assert_not_empty "$CLIENT_INSTANCE_ID" "client instance_id 不应为空" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
    assert_status "200" "$HTTP_STATUS" "进入 discovery 模式应返回 200" || return 1
    pair_code="$(echo "$HTTP_BODY" | jq -r '.session.pair_code // ""')"
    assert_not_empty "$pair_code" "pair_code 不应为空" || return 1

    CALLER_CONNECT_LOG="$(mktemp)"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up "$pair_code" --relay-url "$RELAY_URL" \
        >"$CALLER_CONNECT_LOG" 2>&1 &
    CALLER_CONNECT_PID=$!

    local pairing_found=0
    for _ in $(seq 1 30); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
        pairing_id="$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id // ""')"
        if [[ -n "$pairing_id" ]]; then
            pairing_found=1
            break
        fi
        sleep 1
    done
    assert_equals "1" "$pairing_found" "pending pairing should arrive on target" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${pairing_id}/approve" '{"grant_mode":"permanent"}'
    assert_status "200" "$HTTP_STATUS" "approve pairing should return 200" || return 1

    local connect_ok=0
    for _ in $(seq 1 30); do
        if ! kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
            local connect_exit=0
            if wait "$CALLER_CONNECT_PID"; then
                connect_exit=0
            else
                connect_exit=$?
            fi
            if [[ "$connect_exit" -eq 0 ]]; then
                connect_ok=1
            fi
            break
        fi
        sleep 1
    done
    assert_equals "1" "$connect_ok" "caller remote conn up should succeed" || return 1
    CALLER_CONNECT_PID=""

    if grep -q "Connected! Authorization granted" "$CALLER_CONNECT_LOG"; then
        _log_pass "caller connect log includes authorization success"
    else
        _log_fail "caller connect log includes authorization success" \
            "Connected! Authorization granted" "$(cat "$CALLER_CONNECT_LOG")"
        return 1
    fi

    local caller_connections_file=""
    for _ in $(seq 1 20); do
        caller_connections_file="$(find "$CALLER_DATA_DIR" -name remote-connections.json -print -quit)"
        if [[ -n "$caller_connections_file" ]]; then
            GRANT_ID="$(jq -r '.connections[0].grant_id // ""' "$caller_connections_file")"
            if [[ -n "$GRANT_ID" ]]; then
                break
            fi
        fi
        sleep 0.5
    done
    assert_not_empty "$GRANT_ID" "grant_id 不应为空" || return 1

    local grant_update_ok=0
    local update_body='{"grant_scope":"remote_shell_exec","policy_binding":{"mode":"all"},"interactive_allowed":false,"stdin_allowed":true}'
    for _ in $(seq 1 20); do
        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$update_body"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            if echo "$HTTP_BODY" | jq -e '
                ((.data.grant_scope // .grant_scope // "") == "remote_shell_exec")
                and ((.data.policy_binding.mode // .policy_binding.mode // "") == "all")
            ' >/dev/null 2>&1; then
                grant_update_ok=1
                break
            fi
        fi
        sleep 0.5
    done
    if [[ "$grant_update_ok" -eq 1 ]]; then
        _log_pass "grant upgraded to remote_shell_exec with mode=all"
    elif [[ "$HTTP_STATUS" == "400" && "$HTTP_BODY" == *"grant '${GRANT_ID}' not found"* ]]; then
        _log_warning "target local grant was not materialized yet; fallback to the pair-created default shell grant"
    else
        _log_fail "grant upgraded to remote_shell_exec with mode=all" \
            'HTTP 200 with grant_scope=remote_shell_exec and policy_binding.mode=all' \
            "status=${HTTP_STATUS} body=${HTTP_BODY}"
        return 1
    fi

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"
    assert_status "200" "$HTTP_STATUS" "退出 discovery 模式应返回 200" || return 1
}

latest_shell_exec_started_at() {
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
    if [[ "$HTTP_STATUS" != "200" ]]; then
        echo "0"
        return 0
    fi
    echo "$HTTP_BODY" | jq -r '
        (.calls // [])
        | map(select(
            (.command_kind // .command.kind // .command.command // .command // "") == "shell.exec"
        ))
        | sort_by(.started_at // 0)
        | reverse
        | .[0].started_at // 0
    '
}

latest_shell_exec_call_after() {
    local prev_started_at="$1"
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
    if [[ "$HTTP_STATUS" != "200" ]]; then
        return 1
    fi
    echo "$HTTP_BODY" | jq -c --argjson prev "$prev_started_at" '
        (.calls // [])
        | map(select(
            ((.command_kind // .command.kind // .command.command // .command // "") == "shell.exec")
            and ((.started_at // 0) > $prev)
        ))
        | sort_by(.started_at // 0)
        | reverse
        | .[0] // empty
    '
}

wait_for_stream_marker_while_running() {
    local log_file="$1"
    local marker="$2"
    local pid="$3"
    local message="$4"

    for _ in $(seq 1 25); do
        if grep -q "$marker" "$log_file" 2>/dev/null; then
            if kill -0 "$pid" 2>/dev/null; then
                return 0
            fi
            return 2
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.2
    done

    return 3
}

assert_recent_call_metadata() {
    local prev_started_at="$1"
    local expected_policy_id="$2"
    local expected_exec_mode="$3"
    local label_prefix="$4"
    local call_json

    for _ in $(seq 1 20); do
        call_json="$(latest_shell_exec_call_after "$prev_started_at")"
        if [[ -n "$call_json" ]]; then
            break
        fi
        sleep 0.5
    done
    assert_not_empty "$call_json" "${label_prefix}: Recent Calls should contain the new shell.exec entry" || return 1

    local exit_code stdout_digest policy_id exec_mode
    exit_code="$(echo "$call_json" | jq -r '.exit_code // ""')"
    stdout_digest="$(echo "$call_json" | jq -r '.stdout_digest // ""')"
    policy_id="$(echo "$call_json" | jq -r '.policy_id // ""')"
    exec_mode="$(echo "$call_json" | jq -r '.exec_mode // ""')"

    assert_equals "0" "$exit_code" "${label_prefix}: Recent Calls exit_code should be 0" || return 1
    assert_equals "$expected_policy_id" "$policy_id" "${label_prefix}: Recent Calls should record policy_id" || return 1
    assert_equals "$expected_exec_mode" "$exec_mode" "${label_prefix}: Recent Calls should record exec_mode" || return 1

    if [[ "$stdout_digest" =~ ^[0-9a-f]{40}$ ]]; then
        _log_pass "${label_prefix}: Recent Calls should record stdout_digest"
    else
        _log_fail "${label_prefix}: Recent Calls should record stdout_digest" "40-char sha1" "$stdout_digest"
        return 1
    fi
}

run_shell_text_streaming_case() {
    local prev_started_at shell_log shell_pid shell_exit shell_output stream_status

    log "Running shell_text streaming regression..."
    prev_started_at="$(latest_shell_exec_started_at)"
    shell_log="$(mktemp)"

    (
        BIFROST_DATA_DIR="$CALLER_DATA_DIR" exec "$BIFROST_BIN" remote exec \
            --relay-url "$RELAY_URL" \
            --client-id "$CLIENT_INSTANCE_SHORT" \
            --shell-text "printf shell-one; /bin/sleep 1; printf shell-two"
    ) >"$shell_log" 2>&1 &
    shell_pid=$!

    stream_status=0
    if ! wait_for_stream_marker_while_running "$shell_log" "shell-one" "$shell_pid" \
        "shell_text first chunk should stream before command exit"; then
        stream_status=$?
    fi

    if wait "$shell_pid"; then
        shell_exit=0
    else
        shell_exit=$?
    fi
    shell_output="$(cat "$shell_log")"

    if [[ "$stream_status" -eq 0 ]]; then
        _log_pass "shell_text first chunk should stream before command exit"
    else
        _log_fail "shell_text first chunk should stream before command exit" \
            "marker=shell-one before exit" "exit=${shell_exit} output=${shell_output}"
        return 1
    fi
    if [[ "$shell_exit" -eq 0 ]]; then
        _log_pass "shell_text command should exit successfully"
    else
        _log_fail "shell_text command should exit successfully" "0" "exit=${shell_exit} output=${shell_output}"
        return 1
    fi
    assert_body_contains "shell-oneshell-two" "$shell_output" "shell_text command should preserve streamed stdout content" || return 1
    assert_recent_call_metadata "$prev_started_at" "stream-shell" "shell_text" "shell_text" || return 1

    rm -f "$shell_log"
}

run_argv_streaming_case() {
    local prev_started_at argv_log argv_pid argv_exit argv_output stream_status

    log "Running argv_exec streaming regression..."
    prev_started_at="$(latest_shell_exec_started_at)"
    argv_log="$(mktemp)"

    (
        BIFROST_DATA_DIR="$CALLER_DATA_DIR" exec "$BIFROST_BIN" remote exec \
            --relay-url "$RELAY_URL" \
            --client-id "$CLIENT_INSTANCE_SHORT" \
            -- "$PYTHON_BIN" -u -c 'import sys,time;sys.stdout.write("argv-one");sys.stdout.flush();time.sleep(1.0);sys.stdout.write("argv-two");sys.stdout.flush()'
    ) >"$argv_log" 2>&1 &
    argv_pid=$!

    stream_status=0
    if ! wait_for_stream_marker_while_running "$argv_log" "argv-one" "$argv_pid" \
        "argv_exec first chunk should stream before command exit"; then
        stream_status=$?
    fi

    if wait "$argv_pid"; then
        argv_exit=0
    else
        argv_exit=$?
    fi
    argv_output="$(cat "$argv_log")"

    if [[ "$stream_status" -eq 0 ]]; then
        _log_pass "argv_exec first chunk should stream before command exit"
    else
        _log_fail "argv_exec first chunk should stream before command exit" \
            "marker=argv-one before exit" "exit=${argv_exit} output=${argv_output}"
        return 1
    fi
    if [[ "$argv_exit" -eq 0 ]]; then
        _log_pass "argv_exec command should exit successfully"
    else
        _log_fail "argv_exec command should exit successfully" "0" "exit=${argv_exit} output=${argv_output}"
        return 1
    fi
    assert_equals "argv-oneargv-two" "$argv_output" "argv_exec command should preserve full stdout" || return 1
    assert_recent_call_metadata "$prev_started_at" "stream-argv" "argv_exec" "argv_exec" || return 1

    rm -f "$argv_log"
}

run_stdin_first_frame_case() {
    local prev_started_at stdin_log stdin_exit stdin_output

    log "Running stdin first-frame regression..."
    prev_started_at="$(latest_shell_exec_started_at)"
    stdin_log="$(mktemp)"

    if printf 'EARLY_STDIN_OK\n' | BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec \
        --relay-url "$RELAY_URL" \
        --client-id "$CLIENT_INSTANCE_SHORT" \
        --stdin \
        --shell-text "$PYTHON_BIN -u -c 'import sys; print(\"READY\", flush=True); print(sys.stdin.readline().strip(), flush=True)'" \
        >"$stdin_log" 2>&1; then
        stdin_exit=0
    else
        stdin_exit=$?
    fi
    stdin_output="$(cat "$stdin_log")"

    if [[ "$stdin_exit" -eq 0 ]]; then
        _log_pass "stdin first-frame command should exit successfully"
    else
        _log_fail "stdin first-frame command should exit successfully" "0" "exit=${stdin_exit} output=${stdin_output}"
        return 1
    fi
    assert_body_contains "READY" "$stdin_output" "stdin first-frame command should start remote reader" || return 1
    assert_body_contains "EARLY_STDIN_OK" "$stdin_output" "stdin first frame should reach remote process" || return 1
    assert_recent_call_metadata "$prev_started_at" "stream-shell" "shell_text" "stdin-first-frame" || return 1

    rm -f "$stdin_log"
}

cleanup() {
    if [[ -n "${CALLER_CONNECT_PID:-}" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "${RELAY_PID:-}" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    [[ -n "${RELAY_LOG:-}" ]] && rm -f "$RELAY_LOG" 2>/dev/null || true
    [[ -n "${CALLER_CONNECT_LOG:-}" ]] && rm -f "$CALLER_CONNECT_LOG" 2>/dev/null || true
    [[ -n "${TARGET_DATA_DIR:-}" ]] && rm -rf "$TARGET_DATA_DIR" 2>/dev/null || true
    [[ -n "${CALLER_DATA_DIR:-}" ]] && rm -rf "$CALLER_DATA_DIR" 2>/dev/null || true
}
trap cleanup EXIT

main() {
    require_cmd cargo
    require_cmd curl
    require_cmd jq
    require_cmd node

    PYTHON_BIN="$(python3_cmd)"
    assert_not_empty "$PYTHON_BIN" "python3 executable should be available" || exit 1

    TARGET_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-shell-target-XXXXXX")"
    CALLER_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-shell-caller-XXXXXX")"

    export ADMIN_HOST="127.0.0.1"
    export ADMIN_PORT="${ADMIN_PORT:-$(pick_free_port)}"
    export ADMIN_PATH_PREFIX="/_bifrost"
    export BIFROST_DATA_DIR="$TARGET_DATA_DIR"

    prepare_bifrost_bin
    start_local_relay

    mkdir -p "$TARGET_DATA_DIR"
    cat >"$TARGET_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF

    log "Starting target bifrost on port $ADMIN_PORT..."
    admin_start_bifrost
    CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

    configure_shell_policies
    pair_and_upgrade_grant
    run_shell_text_streaming_case
    run_argv_streaming_case
    run_stdin_first_frame_case

    print_test_summary
}

main "$@"
