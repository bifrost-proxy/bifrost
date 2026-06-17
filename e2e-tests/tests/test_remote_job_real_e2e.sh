#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

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
RELAY_DATA_DIR=""
RELAY_URL=""

TARGET_DATA_DIR=""
CALLER_DATA_DIR=""
FRESH_CALLER_DATA_DIR=""
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
WORK_DIR=""

log() {
    echo "[remote-job-real-e2e] $*"
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
    RELAY_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-job-relay-XXXXXX")"
    local relay_exec
    relay_exec="$(sync_server_exec "$SYNC_SERVER_DIR")"

    log "Starting relay on port $RELAY_PORT..."
    (
        cd "$SYNC_SERVER_DIR" && \
            eval "$relay_exec" -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
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
    log "Configuring target Shell Access policy..."
    BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" setting shell policy add \
        --id remote-job-shell \
        --name "Remote Job Shell" \
        --mode shell_text \
        --pattern '^(?s:.*)$' \
        --shell /bin/sh \
        --stdin \
        --timeout-ms 60000 >/dev/null

    _log_pass "target Shell Access includes long-running shell_text policy"
}

pair_and_upgrade_grant() {
    local sync_user_id sync_password pair_code pairing_id

    sync_user_id="remote_job_real_${RANDOM}"
    sync_password="remote_job_real_123"

    http_post_json "${RELAY_URL}/v4/sso/register" \
        "{\"user_id\":\"${sync_user_id}\",\"password\":\"${sync_password}\",\"nickname\":\"Remote Job Real E2E\"}"
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
    assert_equals "1" "$grant_update_ok" "grant upgraded to remote_shell_exec with mode=all" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"
    assert_status "200" "$HTTP_STATUS" "退出 discovery 模式应返回 200" || return 1
}

extract_call_id() {
    local file="$1"
    grep -E '^call_id=' "$file" | head -1 | cut -d= -f2-
}

assert_no_plain_relay_token() {
    local file="$1"
    local label="$2"
    if grep -E 'relay_token=|--relay-token' "$file" >/dev/null 2>&1; then
        _log_fail "$label" "no relay token in user-visible output" "$(cat "$file")"
        return 1
    fi
    _log_pass "$label"
}

remote_job_cache_file() {
    find "$CALLER_DATA_DIR" -name remote-jobs.json -print -quit
}

assert_cache_contains_job() {
    local call_id="$1"
    local expected_status="$2"
    local cache_file
    cache_file="$(remote_job_cache_file)"
    assert_not_empty "$cache_file" "remote-jobs.json should exist in caller data dir" || return 1

    jq -e --arg call_id "$call_id" --arg relay_url "$RELAY_URL" --arg status "$expected_status" '
        (.jobs // [])
        | map(select(.call_id == $call_id))
        | .[0] as $job
        | ($job.relay_url == $relay_url)
          and ($job.relay_token_encrypted | type == "string")
          and ($job.relay_token_encrypted | contains("ciphertext"))
          and ($job.status == $status)
    ' "$cache_file" >/dev/null
    if [[ $? -eq 0 ]]; then
        _log_pass "remote job cache stores encrypted token, relay url, and status=${expected_status}"
    else
        _log_fail "remote job cache stores encrypted token, relay url, and status=${expected_status}" \
            "cache entry for ${call_id}" "$(cat "$cache_file")"
        return 1
    fi

    if jq -e --arg call_id "$call_id" '
        has("relay_token")
        or ((.jobs // []) | map(select(.call_id == $call_id and has("relay_token"))) | length > 0)
    ' "$cache_file" >/dev/null 2>&1; then
        _log_fail "remote job cache should not use legacy plaintext relay_token fields" \
            "no relay_token field" "$(cat "$cache_file")"
        return 1
    fi
    _log_pass "remote job cache should not use legacy plaintext relay_token fields"
}

wait_for_target_call_exit() {
    local call_id="$1"
    local expected_exit="$2"
    local call_json exit_code status
    for _ in $(seq 1 40); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            call_json="$(echo "$HTTP_BODY" | jq -c --arg call_id "$call_id" '
                (.calls // []) | map(select(.call_id == $call_id)) | .[0] // empty
            ')"
            if [[ -n "$call_json" ]]; then
                exit_code="$(echo "$call_json" | jq -r '.exit_code // ""')"
                status="$(echo "$call_json" | jq -r '.status // ""')"
                if [[ "$exit_code" == "$expected_exit" ]]; then
                    _log_pass "target Recent Calls records ${call_id} exit_code=${expected_exit}"
                    return 0
                fi
                if [[ "$status" == "exited" && "$exit_code" != "$expected_exit" ]]; then
                    break
                fi
            fi
        fi
        sleep 0.5
    done
    _log_fail "target Recent Calls records ${call_id} exit_code=${expected_exit}" \
        "exit_code=${expected_exit}" "status=${HTTP_STATUS} body=${HTTP_BODY}"
    return 1
}

start_detached_job() {
    local name="$1"
    local command="$2"
    local output_file="$3"

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec \
        --relay-url "$RELAY_URL" \
        --client-id "$CLIENT_INSTANCE_SHORT" \
        --env "JOB_ENV=relay-cache-ok" \
        --detach \
        --shell-text "$command" \
        >"$output_file" 2>&1

    assert_body_contains "Detached remote job started" "$(cat "$output_file")" "${name}: detach output should confirm job start" || return 1
    assert_body_contains "watch: bifrost remote job watch" "$(cat "$output_file")" "${name}: detach output should print call_id-only watch hint" || return 1
    assert_no_plain_relay_token "$output_file" "${name}: detach output should not expose relay token" || return 1
}

run_success_job_case() {
    local detach_out list_out status_out logs_out watch_out noverify_out
    local logs_file watch_file call_id status_text success_cmd

    log "Running successful detached remote job case..."
    detach_out="$WORK_DIR/success.detach.out"
    list_out="$WORK_DIR/success.list.out"
    status_out="$WORK_DIR/success.status.out"
    logs_out="$WORK_DIR/success.logs.out"
    watch_out="$WORK_DIR/success.watch.out"
    noverify_out="$WORK_DIR/success.noverify.out"
    logs_file="$WORK_DIR/success.logs.txt"
    watch_file="$WORK_DIR/success.watch.txt"
    success_cmd="$PYTHON_BIN -u -c 'import os,time; print(\"JOB_OK_BEGIN\", flush=True); [print(\"JOB_OK_TICK_%d\" % i, flush=True) or time.sleep(0.7) for i in range(4)]; print(\"JOB_OK_ENV=\" + os.environ.get(\"JOB_ENV\", \"\"), flush=True); print(\"JOB_OK_END\", flush=True)'"

    start_detached_job "success job" "$success_cmd" "$detach_out" || return 1
    call_id="$(extract_call_id "$detach_out")"
    assert_not_empty "$call_id" "success job call_id should be printed" || return 1
    assert_cache_contains_job "$call_id" "running" || return 1

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job list >"$list_out" 2>&1
    assert_body_contains "$call_id" "$(cat "$list_out")" "remote job list should show success job call_id" || return 1
    assert_no_plain_relay_token "$list_out" "remote job list should not expose relay token" || return 1

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job status "$call_id" --wait-ms 100 >"$status_out" 2>&1
    status_text="$(cat "$status_out")"
    if [[ "$status_text" == *"status=running"* || "$status_text" == *"status=exited"* ]]; then
        _log_pass "remote job status should resolve from local cache without relay args"
    else
        _log_fail "remote job status should resolve from local cache without relay args" \
            "status=running or status=exited" "$status_text"
        return 1
    fi
    assert_no_plain_relay_token "$status_out" "remote job status should not expose relay token" || return 1

    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job logs "$call_id" --output-file "$logs_file" >"$logs_out" 2>&1
    local logs_exit=$?
    set -e
    if [[ "$logs_exit" -ne 0 ]]; then
        _log_fail "remote job logs should exit successfully" "0" "exit=${logs_exit} output=$(cat "$logs_out")"
        return 1
    fi
    assert_body_contains "JOB_OK_BEGIN" "$(cat "$logs_file")" "remote job logs --output-file should include first stdout marker" || return 1
    assert_body_contains "JOB_OK_ENV=relay-cache-ok" "$(cat "$logs_file")" "remote job logs should preserve remote env output" || return 1
    assert_body_contains "JOB_OK_END" "$(cat "$logs_file")" "remote job logs --output-file should include final stdout marker" || return 1
    assert_no_plain_relay_token "$logs_out" "remote job logs should not expose relay token" || return 1

    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job logs "$call_id" --no-verify-digest >"$noverify_out" 2>&1
    local noverify_exit=$?
    set -e
    if [[ "$noverify_exit" -ne 0 ]]; then
        _log_fail "remote job logs --no-verify-digest should exit successfully" "0" "exit=${noverify_exit} output=$(cat "$noverify_out")"
        return 1
    fi
    assert_body_contains "JOB_OK_END" "$(cat "$noverify_out")" "remote job logs --no-verify-digest should replay completed stdout" || return 1

    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job watch "$call_id" --output-file "$watch_file" >"$watch_out" 2>&1
    local watch_exit=$?
    set -e
    if [[ "$watch_exit" -ne 0 ]]; then
        _log_fail "remote job watch should exit successfully for success job" "0" "exit=${watch_exit} output=$(cat "$watch_out")"
        return 1
    fi
    assert_body_contains "JOB_OK_END" "$(cat "$watch_file")" "remote job watch should replay completed stdout to output file" || return 1
    assert_cache_contains_job "$call_id" "exited" || return 1

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job status "$call_id" --wait-ms 1000 >"$status_out" 2>&1
    assert_body_contains "status=exited" "$(cat "$status_out")" "remote job status should report terminal status after watch" || return 1
    assert_body_contains "exit_code=0" "$(cat "$status_out")" "remote job status should report exit_code=0" || return 1

    wait_for_target_call_exit "$call_id" "0" || return 1
}

run_failure_job_case() {
    local detach_out watch_out status_out list_out watch_file call_id failure_cmd watch_exit

    log "Running failing detached remote job case..."
    detach_out="$WORK_DIR/fail.detach.out"
    watch_out="$WORK_DIR/fail.watch.out"
    status_out="$WORK_DIR/fail.status.out"
    list_out="$WORK_DIR/fail.list.out"
    watch_file="$WORK_DIR/fail.watch.txt"
    failure_cmd="$PYTHON_BIN -u -c 'import sys,time; print(\"JOB_FAIL_BEGIN\", flush=True); time.sleep(0.5); print(\"JOB_FAIL_END\", flush=True); sys.exit(7)'"

    start_detached_job "failure job" "$failure_cmd" "$detach_out" || return 1
    call_id="$(extract_call_id "$detach_out")"
    assert_not_empty "$call_id" "failure job call_id should be printed" || return 1

    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job watch "$call_id" --output-file "$watch_file" >"$watch_out" 2>&1
    watch_exit=$?
    set -e
    assert_equals "7" "$watch_exit" "remote job watch should return the real remote non-zero exit code" || return 1
    assert_body_contains "JOB_FAIL_BEGIN" "$(cat "$watch_file")" "failing job watch should capture first marker" || return 1
    assert_body_contains "JOB_FAIL_END" "$(cat "$watch_file")" "failing job watch should capture final marker" || return 1

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job status "$call_id" --wait-ms 1000 >"$status_out" 2>&1
    assert_body_contains "status=exited" "$(cat "$status_out")" "failing job status should report terminal status" || return 1
    assert_body_contains "exit_code=7" "$(cat "$status_out")" "failing job status should report exit_code=7" || return 1

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job list >"$list_out" 2>&1
    assert_body_contains "$call_id" "$(cat "$list_out")" "remote job list should show failing job call_id" || return 1
    assert_body_contains "exited" "$(cat "$list_out")" "remote job list should show terminal failing job status" || return 1
    wait_for_target_call_exit "$call_id" "7" || return 1
}

run_error_contract_cases() {
    local first_call_id old_arg_out fresh_out help_out
    first_call_id="$(jq -r '(.jobs // [])[0].call_id // ""' "$(remote_job_cache_file)")"
    old_arg_out="$WORK_DIR/old-arg.out"
    fresh_out="$WORK_DIR/fresh-caller.out"
    help_out="$WORK_DIR/job-help.out"

    log "Running CLI contract error cases..."
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job status --help >"$help_out" 2>&1
    if grep -q -- "--relay-token" "$help_out"; then
        _log_fail "remote job status help should not expose --relay-token" \
            "no --relay-token" "$(cat "$help_out")"
        return 1
    fi
    _log_pass "remote job status help should not expose --relay-token"

    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote job status "$first_call_id" --relay-token fake >"$old_arg_out" 2>&1
    local old_arg_exit=$?
    BIFROST_DATA_DIR="$FRESH_CALLER_DATA_DIR" "$BIFROST_BIN" remote job status "$first_call_id" >"$fresh_out" 2>&1
    local fresh_exit=$?
    set -e

    if [[ "$old_arg_exit" -ne 0 ]] && grep -q "unexpected argument '--relay-token'" "$old_arg_out"; then
        _log_pass "legacy manual --relay-token argument should be rejected"
    else
        _log_fail "legacy manual --relay-token argument should be rejected" \
            "non-zero with unexpected argument" "exit=${old_arg_exit} output=$(cat "$old_arg_out")"
        return 1
    fi

    if [[ "$fresh_exit" -ne 0 ]] && grep -q "no local remote job cache" "$fresh_out"; then
        _log_pass "fresh caller without local job cache should fail with actionable message"
    else
        _log_fail "fresh caller without local job cache should fail with actionable message" \
            "no local remote job cache" "exit=${fresh_exit} output=$(cat "$fresh_out")"
        return 1
    fi
    assert_body_contains "bifrost remote job list" "$(cat "$fresh_out")" "fresh caller cache miss should suggest remote job list" || return 1
}

cleanup() {
    if [[ "${KEEP_REMOTE_JOB_E2E_TMP:-}" == "1" ]]; then
        echo "[remote-job-real-e2e] preserving tmp dirs:" >&2
        echo "  TARGET_DATA_DIR=$TARGET_DATA_DIR" >&2
        echo "  CALLER_DATA_DIR=$CALLER_DATA_DIR" >&2
        echo "  FRESH_CALLER_DATA_DIR=$FRESH_CALLER_DATA_DIR" >&2
        echo "  RELAY_DATA_DIR=$RELAY_DATA_DIR" >&2
        echo "  RELAY_LOG=$RELAY_LOG" >&2
        echo "  WORK_DIR=$WORK_DIR" >&2
        return
    fi
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
    [[ -n "${FRESH_CALLER_DATA_DIR:-}" ]] && rm -rf "$FRESH_CALLER_DATA_DIR" 2>/dev/null || true
    [[ -n "${RELAY_DATA_DIR:-}" ]] && rm -rf "$RELAY_DATA_DIR" 2>/dev/null || true
    [[ -n "${WORK_DIR:-}" ]] && rm -rf "$WORK_DIR" 2>/dev/null || true
}
trap cleanup EXIT

main() {
    require_cmd cargo
    require_cmd curl
    require_cmd jq
    require_cmd node

    PYTHON_BIN="$(python3_cmd)"
    assert_not_empty "$PYTHON_BIN" "python3 executable should be available" || exit 1

    TARGET_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-job-target-XXXXXX")"
    CALLER_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-job-caller-XXXXXX")"
    FRESH_CALLER_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-job-fresh-caller-XXXXXX")"
    WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-job-work-XXXXXX")"

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
    run_success_job_case
    run_failure_job_case
    run_error_contract_cases

    print_test_summary
}

main "$@"
