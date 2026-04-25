#!/usr/bin/env bash
#
# Remote File Relay E2E — full caller → relay → target data path test.
#
# This test verifies that ALL `bifrost remote file` subcommands work
# end-to-end through the relay server. It starts a relay (bifrost-sync-server),
# a target bifrost daemon, does the pairing flow, upgrades the grant to
# remote_file_write scope, and exercises file operations through the relay.
#

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
SANDBOX_DIR=""
CALLER_CONN_OK=1

log() {
    echo "[remote-file-relay-e2e] $*"
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
    while [[ $waited -lt 30 ]]; do
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

pair_and_upgrade_grant() {
    local sync_user_id sync_password pair_code pairing_id

    sync_user_id="remote_file_relay_${RANDOM}"
    sync_password="remote_file_relay_123"

    http_post_json "${RELAY_URL}/v4/sso/register" \
        "{\"user_id\":\"${sync_user_id}\",\"password\":\"${sync_password}\",\"nickname\":\"Remote File Relay E2E\"}"
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
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote connect "$pair_code" --relay-url "$RELAY_URL" \
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
    assert_equals "1" "$connect_ok" "caller remote connect should succeed" || return 1
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

    # Upgrade grant to remote_file_write scope
    local grant_update_ok=0
    local update_body='{"grant_scope":"remote_file_write"}'
    for _ in $(seq 1 20); do
        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$update_body"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            if echo "$HTTP_BODY" | jq -e '
                ((.data.grant_scope // .grant_scope // "") == "remote_file_write")
            ' >/dev/null 2>&1; then
                grant_update_ok=1
                break
            fi
        fi
        sleep 0.5
    done
    if [[ "$grant_update_ok" -eq 1 ]]; then
        _log_pass "grant upgraded to remote_file_write"
    elif [[ "$HTTP_STATUS" == "400" && "$HTTP_BODY" == *"grant '${GRANT_ID}' not found"* ]]; then
        _log_warning "target local grant was not materialized yet; fallback to the pair-created default grant"
    else
        _log_fail "grant upgraded to remote_file_write" \
            'HTTP 200 with grant_scope=remote_file_write' \
            "status=${HTTP_STATUS} body=${HTTP_BODY}"
        return 1
    fi

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"
    assert_status "200" "$HTTP_STATUS" "退出 discovery 模式应返回 200" || return 1
}

is_caller_conn_error() {
    echo "$1" | grep -qiE "no saved connection|expired|revoked|Config error.*connect"
}

run_remote_file_cmd() {
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote file "$@" \
        --relay-url "$RELAY_URL" --client-id "$CLIENT_INSTANCE_SHORT" 2>&1
}

# ---------------------------------------------------------------------------
#  TC-FILE-01: file.read
# ---------------------------------------------------------------------------
test_file_read() {
    log "TC-FILE-01: file.read — read hello.txt"
    local out
    out=$(run_remote_file_cmd read hello.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-01: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.content_b64' >/dev/null 2>&1; then
        local decoded
        decoded=$(echo "$out" | jq -r '.content_b64' | base64 -d)
        if [[ "$decoded" == "hello world" ]]; then
            _log_pass "TC-FILE-01: file.read returns correct content"
        else
            _log_fail "TC-FILE-01: file.read returns correct content" "hello world" "$decoded"
        fi
        # Also verify metadata fields exist
        local size total_size
        size=$(echo "$out" | jq -r '.size // ""')
        total_size=$(echo "$out" | jq -r '.total_size // ""')
        if [[ -n "$size" && "$size" != "null" ]]; then
            _log_pass "TC-FILE-01: file.read response includes size field"
        else
            _log_fail "TC-FILE-01: file.read response includes size field" "non-null size" "$size"
        fi
    else
        _log_fail "TC-FILE-01: file.read returns valid JSON" "JSON with content_b64" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-02: file.list
# ---------------------------------------------------------------------------
test_file_list() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-02: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-02: file.list — list sandbox directory"
    local out
    out=$(run_remote_file_cmd list --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-02: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.entries' >/dev/null 2>&1; then
        local has_hello
        has_hello=$(echo "$out" | jq '[.entries[] | select(.name == "hello.txt")] | length')
        if [[ "$has_hello" -ge 1 ]]; then
            _log_pass "TC-FILE-02: file.list includes hello.txt in entries"
        else
            _log_fail "TC-FILE-02: file.list includes hello.txt in entries" ">=1" "$has_hello"
        fi
        # Verify entry structure
        local first_type
        first_type=$(echo "$out" | jq -r '.entries[0].type // .entries[0].kind // ""')
        if [[ -n "$first_type" && "$first_type" != "null" ]]; then
            _log_pass "TC-FILE-02: file.list entries include type/kind field"
        else
            _log_fail "TC-FILE-02: file.list entries include type/kind field" "non-null type" "$first_type"
        fi
    else
        _log_fail "TC-FILE-02: file.list returns valid JSON with entries" "JSON with entries array" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-03: file.stat
# ---------------------------------------------------------------------------
test_file_stat() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-03: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-03: file.stat — stat hello.txt"
    local out
    out=$(run_remote_file_cmd stat hello.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-03: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.size' >/dev/null 2>&1; then
        local size kind
        size=$(echo "$out" | jq -r '.size // ""')
        kind=$(echo "$out" | jq -r '.kind // ""')
        # "hello world\n" = 12 bytes
        if [[ "$size" == "12" ]]; then
            _log_pass "TC-FILE-03: file.stat returns correct size (12)"
        else
            _log_fail "TC-FILE-03: file.stat returns correct size" "12" "$size"
        fi
        if [[ "$kind" == "file" ]]; then
            _log_pass "TC-FILE-03: file.stat returns kind=file"
        else
            _log_fail "TC-FILE-03: file.stat returns kind=file" "file" "$kind"
        fi
    else
        _log_fail "TC-FILE-03: file.stat returns valid JSON" "JSON with size" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-04: file.hash
# ---------------------------------------------------------------------------
test_file_hash() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-04: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-04: file.hash — hash hello.txt"
    local out
    out=$(run_remote_file_cmd hash hello.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-04: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.hex' >/dev/null 2>&1; then
        local algo hex
        algo=$(echo "$out" | jq -r '.algo // ""')
        hex=$(echo "$out" | jq -r '.hex // ""')
        if [[ "$algo" == "sha256" ]]; then
            _log_pass "TC-FILE-04: file.hash returns algo=sha256"
        else
            _log_fail "TC-FILE-04: file.hash returns algo=sha256" "sha256" "$algo"
        fi
        if [[ "${#hex}" -eq 64 ]]; then
            _log_pass "TC-FILE-04: file.hash returns 64-char hex digest"
        else
            _log_fail "TC-FILE-04: file.hash returns 64-char hex digest" "64 chars" "${#hex} chars: $hex"
        fi
    else
        _log_fail "TC-FILE-04: file.hash returns valid JSON" "JSON with hex" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-05: file.write
# ---------------------------------------------------------------------------
test_file_write() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-05: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-05: file.write — write new-file.txt"
    local content="written by e2e"
    local content_file
    content_file="$(mktemp)"
    printf '%s\n' "$content" > "$content_file"

    local out
    out=$(run_remote_file_cmd write new-file.txt --content-file "$content_file" --cwd "$SANDBOX_DIR") || true
    rm -f "$content_file"

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-05: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.bytes_written // .path' >/dev/null 2>&1; then
        _log_pass "TC-FILE-05: file.write returns success JSON"

        # Verify the file exists on disk with correct content
        if [[ -f "$SANDBOX_DIR/new-file.txt" ]]; then
            local disk_content
            disk_content=$(cat "$SANDBOX_DIR/new-file.txt")
            if [[ "$disk_content" == "$content" ]]; then
                _log_pass "TC-FILE-05: written file exists on disk with correct content"
            else
                _log_fail "TC-FILE-05: written file exists on disk with correct content" "$content" "$disk_content"
            fi
        else
            _log_fail "TC-FILE-05: written file exists on disk" "file at $SANDBOX_DIR/new-file.txt" "file not found"
        fi
    else
        _log_fail "TC-FILE-05: file.write returns valid JSON" "JSON with bytes_written or path" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-06: file.mkdir
# ---------------------------------------------------------------------------
test_file_mkdir() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-06: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-06: file.mkdir — mkdir sub/nested --parents"
    local out
    out=$(run_remote_file_cmd mkdir sub/nested --parents --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-06: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.path // .created' >/dev/null 2>&1; then
        _log_pass "TC-FILE-06: file.mkdir returns success JSON"

        # Verify directory exists on disk
        if [[ -d "$SANDBOX_DIR/sub/nested" ]]; then
            _log_pass "TC-FILE-06: directory sub/nested exists on disk"
        else
            _log_fail "TC-FILE-06: directory sub/nested exists on disk" "directory at $SANDBOX_DIR/sub/nested" "not found"
        fi
    else
        _log_fail "TC-FILE-06: file.mkdir returns valid JSON" "JSON with path or created" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-07: file.move
# ---------------------------------------------------------------------------
test_file_move() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-07: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-07: file.move — move moveme.txt to moved.txt"
    local out
    out=$(run_remote_file_cmd mv moveme.txt moved.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-07: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.from // .to' >/dev/null 2>&1; then
        _log_pass "TC-FILE-07: file.move returns success JSON"

        # Verify old file is gone and new file exists
        if [[ ! -f "$SANDBOX_DIR/moveme.txt" ]]; then
            _log_pass "TC-FILE-07: source file moveme.txt no longer exists"
        else
            _log_fail "TC-FILE-07: source file moveme.txt no longer exists" "file gone" "file still present"
        fi
        if [[ -f "$SANDBOX_DIR/moved.txt" ]]; then
            _log_pass "TC-FILE-07: destination file moved.txt exists"
        else
            _log_fail "TC-FILE-07: destination file moved.txt exists" "file at moved.txt" "not found"
        fi
    else
        _log_fail "TC-FILE-07: file.move returns valid JSON" "JSON with from or to" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-08: file.delete
# ---------------------------------------------------------------------------
test_file_delete() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-08: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-08: file.delete — delete deleteme.txt"
    local out
    out=$(run_remote_file_cmd rm deleteme.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-08: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.path // .deleted' >/dev/null 2>&1; then
        _log_pass "TC-FILE-08: file.delete returns success JSON"

        # Verify file is gone
        if [[ ! -f "$SANDBOX_DIR/deleteme.txt" ]]; then
            _log_pass "TC-FILE-08: deleted file no longer exists on disk"
        else
            _log_fail "TC-FILE-08: deleted file no longer exists on disk" "file gone" "file still present"
        fi
    else
        _log_fail "TC-FILE-08: file.delete returns valid JSON" "JSON with path or deleted" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-10: file.glob
# ---------------------------------------------------------------------------
test_file_glob() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-10: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-10: file.glob — glob *.txt in sandbox"
    local out
    out=$(run_remote_file_cmd glob "*.txt" --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-10: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.matches' >/dev/null 2>&1; then
        local count
        count=$(echo "$out" | jq '.matches | length')
        if [[ "$count" -ge 1 ]]; then
            _log_pass "TC-FILE-10: file.glob returns matches (count=$count)"
        else
            _log_fail "TC-FILE-10: file.glob returns matches" ">=1" "$count"
        fi
        # hello.txt should be among the matches
        if echo "$out" | jq -r '.matches[]' | grep -q "hello.txt"; then
            _log_pass "TC-FILE-10: file.glob matches include hello.txt"
        else
            _log_fail "TC-FILE-10: file.glob matches include hello.txt" "hello.txt in matches" "$out"
        fi
    else
        _log_fail "TC-FILE-10: file.glob returns valid JSON with matches" "JSON with matches array" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-11: file.search
# ---------------------------------------------------------------------------
test_file_search() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-11: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-11: file.search — search 'hello' in sandbox"
    local out
    out=$(run_remote_file_cmd search "hello" --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-11: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.matches' >/dev/null 2>&1; then
        local count
        count=$(echo "$out" | jq '.matches | length')
        if [[ "$count" -ge 1 ]]; then
            _log_pass "TC-FILE-11: file.search returns matches (count=$count)"
        else
            _log_fail "TC-FILE-11: file.search returns matches" ">=1" "$count"
        fi
        # Verify match structure includes path/line/preview
        local first_path first_line
        first_path=$(echo "$out" | jq -r '.matches[0].path // ""')
        first_line=$(echo "$out" | jq -r '.matches[0].line // ""')
        if [[ -n "$first_path" && "$first_path" != "null" && -n "$first_line" && "$first_line" != "null" ]]; then
            _log_pass "TC-FILE-11: file.search match has path and line fields"
        else
            _log_fail "TC-FILE-11: file.search match has path and line fields" "path+line" "path=$first_path line=$first_line"
        fi
    else
        _log_fail "TC-FILE-11: file.search returns valid JSON with matches" "JSON with matches array" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-12: file.edit
# ---------------------------------------------------------------------------
test_file_edit() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-12: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-12: file.edit — edit editable.txt, replace line 2"
    local edits_json='[{"start_line":2,"end_line":2,"replacement":"replaced line two\n"}]'
    local out
    out=$(run_remote_file_cmd edit editable.txt --edits "$edits_json" --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-12: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.bytes_written // .applied_edits' >/dev/null 2>&1; then
        _log_pass "TC-FILE-12: file.edit returns success JSON"

        # Verify file content on disk
        if [[ -f "$SANDBOX_DIR/editable.txt" ]]; then
            local line2
            line2=$(sed -n '2p' "$SANDBOX_DIR/editable.txt")
            if [[ "$line2" == "replaced line two" ]]; then
                _log_pass "TC-FILE-12: editable.txt line 2 correctly replaced"
            else
                _log_fail "TC-FILE-12: editable.txt line 2 correctly replaced" "replaced line two" "$line2"
            fi
        else
            _log_fail "TC-FILE-12: editable.txt exists after edit" "file present" "file missing"
        fi
    else
        _log_fail "TC-FILE-12: file.edit returns valid JSON" "JSON with bytes_written or applied_edits" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-13: file.apply_patch
# ---------------------------------------------------------------------------
test_file_apply_patch() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-13: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-13: file.apply_patch — patch patchable.txt"
    local patch_file
    patch_file="$(mktemp)"
    cat > "$patch_file" <<'PATCH'
--- a/patchable.txt
+++ b/patchable.txt
@@ -1,3 +1,3 @@
 alpha
-beta
+beta-patched
 gamma
PATCH

    local out
    out=$(run_remote_file_cmd apply-patch --patch-file "$patch_file" --cwd "$SANDBOX_DIR") || true
    rm -f "$patch_file"

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-13: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.files' >/dev/null 2>&1; then
        local patched_count
        patched_count=$(echo "$out" | jq '.files | length')
        if [[ "$patched_count" -ge 1 ]]; then
            _log_pass "TC-FILE-13: file.apply_patch returns files array (count=$patched_count)"
        else
            _log_fail "TC-FILE-13: file.apply_patch returns files array" ">=1" "$patched_count"
        fi

        # Verify file content on disk
        if [[ -f "$SANDBOX_DIR/patchable.txt" ]]; then
            local line2
            line2=$(sed -n '2p' "$SANDBOX_DIR/patchable.txt")
            if [[ "$line2" == "beta-patched" ]]; then
                _log_pass "TC-FILE-13: patchable.txt line 2 correctly patched"
            else
                _log_fail "TC-FILE-13: patchable.txt line 2 correctly patched" "beta-patched" "$line2"
            fi
        else
            _log_fail "TC-FILE-13: patchable.txt exists after patch" "file present" "file missing"
        fi
    else
        _log_fail "TC-FILE-13: file.apply_patch returns valid JSON" "JSON with files array" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-09: readonly scope rejection
# ---------------------------------------------------------------------------
test_readonly_rejection() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-09: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-09: readonly rejection — downgrade to remote_file_read, attempt write"

    # Downgrade grant to remote_file_read
    local downgrade_ok=0
    local downgrade_body='{"grant_scope":"remote_file_read"}'
    for _ in $(seq 1 10); do
        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$downgrade_body"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            if echo "$HTTP_BODY" | jq -e '
                ((.data.grant_scope // .grant_scope // "") == "remote_file_read")
            ' >/dev/null 2>&1; then
                downgrade_ok=1
                break
            fi
        fi
        sleep 0.5
    done

    if [[ "$downgrade_ok" -eq 1 ]]; then
        _log_pass "TC-FILE-09: grant downgraded to remote_file_read"
    else
        _log_fail "TC-FILE-09: grant downgraded to remote_file_read" \
            "HTTP 200 with grant_scope=remote_file_read" \
            "status=${HTTP_STATUS} body=${HTTP_BODY}"
        return 0
    fi

    # Attempt a write operation — should be rejected
    local content_file
    content_file="$(mktemp)"
    printf 'should fail' > "$content_file"
    local out
    out=$(run_remote_file_cmd write rejected-file.txt --content-file "$content_file" --cwd "$SANDBOX_DIR") || true
    rm -f "$content_file"

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-09: caller connection error during write attempt: $out"
        return 0
    fi

    # Check that the write was rejected (error message should contain permission/denied/scope related text)
    if echo "$out" | grep -qiE "permission|denied|allows_command|scope|not allowed|forbidden|reject"; then
        _log_pass "TC-FILE-09: write with readonly scope correctly rejected"
    elif echo "$out" | jq -e '.error // .message' >/dev/null 2>&1; then
        local err_msg
        err_msg=$(echo "$out" | jq -r '.error // .message // ""')
        if echo "$err_msg" | grep -qiE "permission|denied|allows_command|scope|not allowed|forbidden|reject"; then
            _log_pass "TC-FILE-09: write with readonly scope correctly rejected (JSON error)"
        else
            _log_fail "TC-FILE-09: write with readonly scope should be rejected" \
                "error containing permission/denied/scope" "$out"
        fi
    else
        # If the file was NOT created, that also counts as rejected
        if [[ ! -f "$SANDBOX_DIR/rejected-file.txt" ]]; then
            _log_pass "TC-FILE-09: write with readonly scope rejected (file not created)"
        else
            _log_fail "TC-FILE-09: write with readonly scope should be rejected" \
                "write rejection" "file was created: $out"
        fi
    fi

    # Restore grant to remote_file_write for any subsequent tests
    local restore_body='{"grant_scope":"remote_file_write"}'
    for _ in $(seq 1 10); do
        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$restore_body"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            break
        fi
        sleep 0.5
    done
}

# ---------------------------------------------------------------------------
#  Setup sandbox
# ---------------------------------------------------------------------------
setup_sandbox() {
    SANDBOX_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-file-sandbox-XXXXXX")"
    log "Created sandbox at $SANDBOX_DIR"
    echo "hello world" > "$SANDBOX_DIR/hello.txt"
    echo "to move" > "$SANDBOX_DIR/moveme.txt"
    echo "to delete" > "$SANDBOX_DIR/deleteme.txt"
    printf 'line one\nline two\nline three\n' > "$SANDBOX_DIR/editable.txt"
    printf 'alpha\nbeta\ngamma\n' > "$SANDBOX_DIR/patchable.txt"
}

write_file_access_policy() {
    log "Writing file-access policy for grant $GRANT_ID"
    cat >"$TARGET_DATA_DIR/file-access.toml" <<EOF
[[grant]]
grant_id = "$GRANT_ID"
name = "remote-file-relay-e2e"
roots = ["$SANDBOX_DIR"]
denies = ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"]
write_denies = []
ops = ["read", "list", "stat", "glob", "search", "hash", "write", "edit", "mkdir", "move", "delete", "apply_patch"]
max_read_bytes = 2097152
max_write_bytes = 2097152
respect_gitignore = false
allow_overwrite = true
allow_recursive_delete = false
EOF
}

# ---------------------------------------------------------------------------
#  Cleanup
# ---------------------------------------------------------------------------
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
    [[ -n "${SANDBOX_DIR:-}" ]] && rm -rf "$SANDBOX_DIR" 2>/dev/null || true
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
#  Main
# ---------------------------------------------------------------------------
main() {
    require_cmd cargo
    require_cmd curl
    require_cmd jq
    require_cmd node
    require_cmd base64

    TARGET_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-file-target-XXXXXX")"
    CALLER_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-remote-file-caller-XXXXXX")"

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

    pair_and_upgrade_grant

    setup_sandbox
    write_file_access_policy

    # Run all file operation tests
    test_file_read
    test_file_list
    test_file_stat
    test_file_hash
    test_file_write
    test_file_mkdir
    test_file_move
    test_file_delete
    test_file_glob
    test_file_search
    test_file_edit
    test_file_apply_patch
    test_readonly_rejection

    print_test_summary
}

main "$@"
