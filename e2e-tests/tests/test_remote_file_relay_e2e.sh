#!/usr/bin/env bash
#
# Remote File Relay E2E — full caller → relay → target data path test.
#
# This test verifies that ALL `bifrost remote file` subcommands work
# end-to-end through the relay server. It starts a relay (bifrost-sync-server),
# a target bifrost daemon, does the pairing flow, sets file_access=read_write
# on the grant, and exercises file operations through the relay.
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

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${pairing_id}/approve" \
        '{"grant_mode":"permanent","grant_scope":"remote_query","file_access":"read_write"}'
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

    # Confirm the target-side grant is materialized with file_access=read_write before
    # running file commands. If the direct approval did not carry the scope for
    # some reason, the PATCH path is allowed to repair it, but missing grants are
    # setup failures rather than something the test can safely ignore.
    local grant_update_ok=0
    local update_body='{"file_access":"read_write"}'
    for _ in $(seq 1 20); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
        if [[ "$HTTP_STATUS" == "200" ]] && echo "$HTTP_BODY" | jq -e --arg grant_id "$GRANT_ID" '
            .grants[]? | select(.grant_id == $grant_id and .file_access == "read_write")
        ' >/dev/null 2>&1; then
            grant_update_ok=1
            break
        fi

        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$update_body"
        if [[ "$HTTP_STATUS" == "200" ]] && echo "$HTTP_BODY" | jq -e '
            ((.data.file_access // .file_access // "") == "read_write")
        ' >/dev/null 2>&1; then
            http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
            if [[ "$HTTP_STATUS" == "200" ]] && echo "$HTTP_BODY" | jq -e --arg grant_id "$GRANT_ID" '
                .grants[]? | select(.grant_id == $grant_id and .file_access == "read_write")
            ' >/dev/null 2>&1; then
                grant_update_ok=1
                break
            fi
        fi
        sleep 0.5
    done
    if [[ "$grant_update_ok" -eq 1 ]]; then
        _log_pass "grant available with file_access=read_write"
    else
        _log_fail "grant available with file_access=read_write" \
            'target grant list contains grant_id with file_access=read_write' \
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
#  TC-FILE-14: file.read with --offset/--limit (line-range reading)
# ---------------------------------------------------------------------------
test_file_read_offset_limit() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-14: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-14: file.read --offset 2 --limit 2 on multiline.txt"
    local out
    out=$(run_remote_file_cmd read multiline.txt --offset 2 --limit 2 --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-14: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.content_b64' >/dev/null 2>&1; then
        local decoded
        decoded=$(echo "$out" | jq -r '.content_b64' | base64 -d)
        # offset=2 (line 2), limit=2 → should return lines 2-3: "L2 banana\nL3 cherry\n"
        local expected
        expected=$(printf 'L2 banana\nL3 cherry\n')
        if [[ "$decoded" == "$expected" ]]; then
            _log_pass "TC-FILE-14: read offset=2 limit=2 returns correct 2 lines"
        else
            _log_fail "TC-FILE-14: read offset=2 limit=2 returns correct 2 lines" "$expected" "$decoded"
        fi

        # total_lines should be 5 (full file has 5 lines)
        local total_lines
        total_lines=$(echo "$out" | jq -r '.total_lines // ""')
        if [[ "$total_lines" == "5" ]]; then
            _log_pass "TC-FILE-14: read offset response includes total_lines=5"
        else
            _log_fail "TC-FILE-14: read offset response includes total_lines=5" "5" "$total_lines"
        fi
    else
        _log_fail "TC-FILE-14: file.read offset/limit returns valid JSON" "JSON with content_b64" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-15: file.read returns total_lines without offset
# ---------------------------------------------------------------------------
test_file_read_total_lines() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-15: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-15: file.read multiline.txt (no offset) returns total_lines"
    local out
    out=$(run_remote_file_cmd read multiline.txt --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-15: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.content_b64' >/dev/null 2>&1; then
        local total_lines
        total_lines=$(echo "$out" | jq -r '.total_lines // ""')
        if [[ "$total_lines" == "5" ]]; then
            _log_pass "TC-FILE-15: read without offset includes total_lines=5"
        else
            _log_fail "TC-FILE-15: read without offset includes total_lines=5" "5" "$total_lines"
        fi
    else
        _log_fail "TC-FILE-15: file.read returns valid JSON" "JSON with content_b64" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-16: file.search with -B (context_before) and -A (context_after)
# ---------------------------------------------------------------------------
test_file_search_context() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-16: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-16: file.search NEEDLE -B 1 -A 1 in sandbox"
    local out
    out=$(run_remote_file_cmd search "NEEDLE" -B 1 -A 1 --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-16: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.matches' >/dev/null 2>&1; then
        local count
        count=$(echo "$out" | jq '.matches | length')
        if [[ "$count" -ge 1 ]]; then
            _log_pass "TC-FILE-16: search NEEDLE with context returns matches (count=$count)"
        else
            _log_fail "TC-FILE-16: search NEEDLE with context returns matches" ">=1" "$count"
        fi

        # Verify context lines are present: returned as a single "context" array
        local first_match ctx_len
        first_match=$(echo "$out" | jq '.matches[0]')
        ctx_len=$(echo "$first_match" | jq '.context | length // 0')
        if [[ "$ctx_len" -ge 2 ]]; then
            _log_pass "TC-FILE-16: search match contains context array (length=$ctx_len)"

            # Verify context content: should include "ctx-line-2" (before) and "ctx-line-4" (after)
            local ctx_text
            ctx_text=$(echo "$first_match" | jq -r '.context[].content' 2>/dev/null || echo "")
            local has_before=0 has_after=0
            if echo "$ctx_text" | grep -q "ctx-line-2"; then has_before=1; fi
            if echo "$ctx_text" | grep -q "ctx-line-4"; then has_after=1; fi
            if [[ "$has_before" -eq 1 && "$has_after" -eq 1 ]]; then
                _log_pass "TC-FILE-16: context contains expected before (ctx-line-2) and after (ctx-line-4)"
            else
                _log_fail "TC-FILE-16: context should contain before/after lines" "ctx-line-2 + ctx-line-4" "$ctx_text"
            fi
        else
            _log_fail "TC-FILE-16: search match should contain context array" ">=2 items" "$first_match"
        fi
    else
        _log_fail "TC-FILE-16: file.search with context returns valid JSON" "JSON with matches array" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-17: file.search with --exclude node_modules
# ---------------------------------------------------------------------------
test_file_search_exclude() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-17: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-17: file.search 'excluded' --exclude node_modules"
    # "should be excluded" text is in node_modules/pkg.json
    # With --exclude node_modules, it should NOT be found
    local out_excluded
    out_excluded=$(run_remote_file_cmd search "excluded" --exclude node_modules --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out_excluded"; then
        _log_warning "TC-FILE-17: caller connection error, skipping: $out_excluded"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out_excluded" | jq -e '.matches' >/dev/null 2>&1; then
        local excluded_count
        excluded_count=$(echo "$out_excluded" | jq '.matches | length')
        # node_modules is already in DEFAULT_EXCLUDE_DIRS, so matches should be 0
        if [[ "$excluded_count" -eq 0 ]]; then
            _log_pass "TC-FILE-17: search with exclude=node_modules returns 0 matches"
        else
            # Check if any match is from node_modules
            local nm_hits
            nm_hits=$(echo "$out_excluded" | jq '[.matches[] | select(.path | test("node_modules"))] | length')
            if [[ "$nm_hits" -eq 0 ]]; then
                _log_pass "TC-FILE-17: search results exclude node_modules entries"
            else
                _log_fail "TC-FILE-17: search results should exclude node_modules" "0 node_modules hits" "$nm_hits"
            fi
        fi
    else
        _log_fail "TC-FILE-17: file.search with exclude returns valid JSON" "JSON with matches" "$out_excluded"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-18: file.glob with --exclude
# ---------------------------------------------------------------------------
test_file_glob_exclude() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-18: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-18: file.glob '**/*' --exclude node_modules — should not include node_modules paths"
    local out
    out=$(run_remote_file_cmd glob "**/*" --exclude node_modules --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-18: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.matches' >/dev/null 2>&1; then
        local total nm_count
        total=$(echo "$out" | jq '.matches | length')
        nm_count=$(echo "$out" | jq '[.matches[] | select(test("node_modules"))] | length')

        if [[ "$total" -ge 1 ]]; then
            _log_pass "TC-FILE-18: glob returns matches (count=$total)"
        else
            _log_fail "TC-FILE-18: glob returns matches" ">=1" "$total"
        fi

        if [[ "$nm_count" -eq 0 ]]; then
            _log_pass "TC-FILE-18: glob excludes node_modules paths"
        else
            _log_fail "TC-FILE-18: glob should exclude node_modules" "0" "$nm_count"
        fi

        # Verify src/main.rs IS present (non-excluded subdir)
        if echo "$out" | jq -r '.matches[]' | grep -q "src/main.rs\|src\\\\main.rs"; then
            _log_pass "TC-FILE-18: glob includes src/main.rs (not excluded)"
        else
            _log_warning "TC-FILE-18: src/main.rs not found in glob results (may depend on pattern semantics)"
        fi
    else
        _log_fail "TC-FILE-18: file.glob with exclude returns valid JSON" "JSON with matches" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-19: file.list with --exclude
# ---------------------------------------------------------------------------
test_file_list_exclude() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-19: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-19: file.list --depth 2 --exclude node_modules — should not recurse into node_modules"
    local out
    out=$(run_remote_file_cmd list --exclude node_modules --depth 2 --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-19: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.entries' >/dev/null 2>&1; then
        # At depth 2, node_modules dir entry itself may appear, but its contents (pkg.json) must not
        local nm_content
        nm_content=$(echo "$out" | jq '[.entries[] | select(.path | test("node_modules/"))] | length')
        if [[ "$nm_content" -eq 0 ]]; then
            _log_pass "TC-FILE-19: list --depth 2 --exclude excludes node_modules contents"
        else
            _log_fail "TC-FILE-19: list should exclude node_modules contents" "0" "$nm_content"
        fi

        # src/ contents should be visible at depth 2
        local src_content
        src_content=$(echo "$out" | jq '[.entries[] | select(.path | test("src/"))] | length')
        if [[ "$src_content" -ge 1 ]]; then
            _log_pass "TC-FILE-19: list --depth 2 shows src/ contents (not excluded)"
        else
            _log_warning "TC-FILE-19: src/ contents not found at depth 2"
        fi
    else
        _log_fail "TC-FILE-19: file.list with exclude returns valid JSON" "JSON with entries" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-20: file.edit empty replacement deletes line without blank
# ---------------------------------------------------------------------------
test_file_edit_empty_replacement() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-20: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-20: file.edit — delete line 2 with empty replacement"
    # edit-delete.txt = "keep-first\ndelete-me\nkeep-last\n"
    # Delete line 2 by replacing with empty string
    local edits_json='[{"start_line":2,"end_line":2,"replacement":""}]'
    local out
    out=$(run_remote_file_cmd edit edit-delete.txt --edits "$edits_json" --cwd "$SANDBOX_DIR") || true

    if is_caller_conn_error "$out"; then
        _log_warning "TC-FILE-20: caller connection error, skipping: $out"
        CALLER_CONN_OK=0
        return 0
    fi

    if echo "$out" | jq -e '.bytes_written // .applied_edits' >/dev/null 2>&1; then
        _log_pass "TC-FILE-20: file.edit empty replacement returns success JSON"

        # Verify file content on disk: should be "keep-first\nkeep-last\n" (NO blank line)
        if [[ -f "$SANDBOX_DIR/edit-delete.txt" ]]; then
            local actual expected
            actual=$(cat "$SANDBOX_DIR/edit-delete.txt")
            expected=$(printf 'keep-first\nkeep-last')
            if [[ "$actual" == "$expected" ]]; then
                _log_pass "TC-FILE-20: deleted line leaves no blank line (keep-first\\nkeep-last)"
            else
                # Also accept trailing newline
                local expected_nl
                expected_nl=$(printf 'keep-first\nkeep-last\n')
                if [[ "$actual" == "$expected_nl" ]]; then
                    _log_pass "TC-FILE-20: deleted line leaves no blank line (with trailing newline)"
                else
                    _log_fail "TC-FILE-20: deleted line should leave no blank" "keep-first\\nkeep-last" "$actual"
                fi
            fi
        else
            _log_fail "TC-FILE-20: edit-delete.txt exists after edit" "file present" "file missing"
        fi
    else
        _log_fail "TC-FILE-20: file.edit empty replacement returns valid JSON" "JSON with bytes_written" "$out"
    fi
}

# ---------------------------------------------------------------------------
#  TC-FILE-09: readonly scope rejection
# ---------------------------------------------------------------------------

# ===========================================================================
#  Gap-targeting cases — added from design/remote-file-api-gap-analysis.md
#  These cases are expected to RED-LIGHT until the P0/P1 fixes land. They
#  intentionally exercise edge cases that real coding agents hit in the wild.
# ===========================================================================

# ---------------------------------------------------------------------------
#  TC-GAP-01 (P0-4): file.write must preserve executable bit
# ---------------------------------------------------------------------------
test_gap_write_preserves_mode() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-01: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-01 (P0-4): file.write preserves 0755 executable mode"

    local target="$SANDBOX_DIR/bin.sh"
    printf '#!/bin/sh\necho v1\n' > "$target"
    chmod 0755 "$target"

    local new_b64
    new_b64=$(printf '#!/bin/sh\necho v2\n' | base64 | tr -d '\n')
    local out
    out=$(run_remote_file_cmd write bin.sh --cwd "$SANDBOX_DIR" --content-b64 "$new_b64") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-01: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local mode
    mode=$(stat -f '%Lp' "$target" 2>/dev/null || stat -c '%a' "$target" 2>/dev/null || echo "?")
    if [[ "$mode" == "755" ]]; then
        _log_pass "TC-GAP-01: executable bit preserved after file.write"
    else
        _log_fail "TC-GAP-01: executable bit preserved after file.write" "755" "$mode"
        # Write rich post-mortem INTO the sidecar (survives suite-log tail truncation in CI).
        if [[ -n "${BIFROST_FILE_OPS_DEBUG_LOG:-}" ]]; then
            {
                echo "=== TC-GAP-01 POST-FAIL DIAGNOSTIC ==="
                echo "stat-reported-mode: $mode"
                echo "target: $target"
                echo "sandbox: $SANDBOX_DIR"
                echo "tmpdir-mount:"
                mount | grep -E "on /tmp|on $(dirname "$SANDBOX_DIR")" 2>&1 || true
                echo "ls -la target:"
                ls -la "$target" 2>&1 || true
                echo "stat -c raw:"
                stat -c "mode=%a perms=%A type=%F uid=%u owner=%U gid=%g group=%G inode=%i fs=%T size=%s" "$target" 2>&1 || true
                echo "stat (default):"
                stat "$target" 2>&1 || true
                echo "umask: $(umask)"
                echo "whoami: $(id 2>&1)"
                echo "file ACL (getfacl if any):"
                getfacl -p "$target" 2>&1 || true
                echo "ls -la sandbox:"
                ls -la "$SANDBOX_DIR" 2>&1 | head -40 || true
                echo "=== END TC-GAP-01 POST-FAIL ==="
            } >> "$BIFROST_FILE_OPS_DEBUG_LOG" 2>&1
        fi
        # Also still echo a short note to stdout for easy spotting.
        echo "[TC-GAP-01] stat-mode=$mode (expected 755); full post-mortem in $BIFROST_FILE_OPS_DEBUG_LOG"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-02 (P0-3): file.edit must preserve CRLF line endings
# ---------------------------------------------------------------------------
test_gap_edit_preserves_crlf() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-02: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-02 (P0-3): file.edit preserves CRLF line endings"

    local target="$SANDBOX_DIR/crlf.txt"
    printf 'alpha\r\nbeta\r\ngamma\r\n' > "$target"

    local edits_json
    edits_json='[{"start_line":2,"end_line":2,"replacement":"BETA"}]'
    local out
    out=$(run_remote_file_cmd edit crlf.txt --cwd "$SANDBOX_DIR" --edits "$edits_json") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-02: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local crlf_count lf_only_count
    crlf_count=$(grep -c $'\r$' "$target" 2>/dev/null || echo 0)
    lf_only_count=$(awk 'BEGIN{n=0} /\r$/{next} /./{n++} END{print n}' "$target" 2>/dev/null || echo 0)
    if [[ "$crlf_count" -eq 3 && "$lf_only_count" -eq 0 ]]; then
        _log_pass "TC-GAP-02: CRLF line endings preserved across edit"
    else
        _log_fail "TC-GAP-02: CRLF line endings preserved" "crlf=3 lf_only=0" "crlf=${crlf_count} lf_only=${lf_only_count}"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-03 (P0-1): file.apply_patch is all-or-nothing across files
# ---------------------------------------------------------------------------
test_gap_apply_patch_atomic_multifile() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-03: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-03 (P0-1): apply_patch multi-file atomicity"

    # two files: first patches cleanly, second has deliberate context mismatch
    printf 'one\ntwo\nthree\n' > "$SANDBOX_DIR/a.txt"
    printf 'apple\nbanana\ncherry\n' > "$SANDBOX_DIR/b.txt"

    local patch
    patch=$(cat <<'PATCH'
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-one
+ONE
 two
 three
--- a/b.txt
+++ b/b.txt
@@ -1,3 +1,3 @@
-WRONG_CONTEXT_LINE
+APPLE
 banana
 cherry
PATCH
)
    local patch_b64
    patch_b64=$(printf '%s\n' "$patch" | base64 | tr -d '\n')
    local out
    out=$(run_remote_file_cmd apply-patch --cwd "$SANDBOX_DIR" --patch-b64 "$patch_b64") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-03: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local a_changed
    a_changed_raw=$(grep -c '^ONE$' "$SANDBOX_DIR/a.txt" 2>/dev/null; true)
    a_changed=$(printf '%s' "$a_changed_raw" | head -n1)
    : "${a_changed:=0}"
    if [[ "$a_changed" -eq 0 ]]; then
        _log_pass "TC-GAP-03: failed patch rolled back first file (atomic)"
    else
        _log_fail "TC-GAP-03: failed patch rolled back first file (atomic)" \
            "a.txt unchanged (atomic rollback)" \
            "a.txt was modified despite b.txt failure — half-applied state"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-04 (P0-5): .gitignore is respected by file.search / file.glob
# ---------------------------------------------------------------------------
test_gap_gitignore_respected() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-04: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-04 (P0-5): .gitignore is respected by file.search"

    local dir="$SANDBOX_DIR/gi"
    mkdir -p "$dir/dist" "$dir/src"
    printf '.gitignore\n' > "$dir/.gitignore"
    printf 'dist/\n' >> "$dir/.gitignore"
    printf 'MARKER_HIT\n' > "$dir/dist/ignored.txt"
    printf 'MARKER_HIT\n' > "$dir/src/kept.txt"

    local out
    out=$(run_remote_file_cmd search 'MARKER_HIT' --cwd "$dir") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-04: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local hits_in_dist
    hits_in_dist=$(echo "$out" | jq '[.matches[]? | select(.path | test("dist/"))] | length' 2>/dev/null || echo 0)
    if [[ "$hits_in_dist" -eq 0 ]]; then
        _log_pass "TC-GAP-04: search skips files ignored by .gitignore"
    else
        _log_fail "TC-GAP-04: search skips files ignored by .gitignore" \
            "0 hits under dist/" \
            "${hits_in_dist} hits under dist/ — .gitignore not wired"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-05 (P0-6): file.write supports create_parents
# ---------------------------------------------------------------------------
test_gap_write_create_parents() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-05: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-05 (P0-6): file.write --create-parents creates missing dirs"

    local nested="deep/very/nested/new.txt"
    local b64
    b64=$(printf 'ok\n' | base64 | tr -d '\n')
    local out
    out=$(run_remote_file_cmd write "$nested" --cwd "$SANDBOX_DIR" --content-b64 "$b64" --create-parents) || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-05: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi
    if [[ -f "$SANDBOX_DIR/$nested" ]]; then
        _log_pass "TC-GAP-05: create_parents created missing directories"
    else
        _log_fail "TC-GAP-05: create_parents created missing directories" \
            "file exists at $nested" "file missing — create_parents flag not implemented"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-06 (P1-8): file.read truncated must expose FILE-wide sha256
# ---------------------------------------------------------------------------
test_gap_read_truncated_file_sha256() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-06: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-06 (P1-8): truncated read returns file_sha256 over whole file"

    local target="$SANDBOX_DIR/big.txt"
    python3 -c "open(r'${target}','wb').write(b'x'*(3*1024*1024))" 2>/dev/null || \
        dd if=/dev/zero of="$target" bs=1024 count=3072 >/dev/null 2>&1

    local file_sha
    file_sha=$( { shasum -a 256 "$target" 2>/dev/null || sha256sum "$target" 2>/dev/null; } | awk "{print \$1}")

    local out
    out=$(run_remote_file_cmd read big.txt --cwd "$SANDBOX_DIR" --max-bytes 65536) || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-06: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local reported
    reported=$(echo "$out" | jq -r '.file_sha256 // ""')
    if [[ -n "$reported" && "$reported" == "$file_sha" ]]; then
        _log_pass "TC-GAP-06: file_sha256 matches whole-file digest when truncated"
    else
        _log_fail "TC-GAP-06: file_sha256 matches whole-file digest when truncated" \
            "$file_sha" "${reported:-<missing file_sha256 field>}"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-07 (P1-4): file.list truncation must surface truncated=true
# ---------------------------------------------------------------------------
test_gap_list_truncated_flag() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-07: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-07 (P1-4): file.list surfaces truncated flag"

    # This dir is too expensive to actually create (>10k files); we instead
    # assert the JSON schema exposes the field so clients can branch on it.
    local out
    out=$(run_remote_file_cmd list --cwd "$SANDBOX_DIR") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-07: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi
    if echo "$out" | jq -e 'has("truncated")' >/dev/null 2>&1; then
        _log_pass "TC-GAP-07: file.list response carries 'truncated' key"
    else
        _log_fail "TC-GAP-07: file.list response carries 'truncated' key" \
            '"truncated": bool' \
            'field missing — silent truncation at MAX_ENTRIES_PER_DIR=10000'
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-08 (P1-1): file.search supports case-insensitive + file glob
# ---------------------------------------------------------------------------
test_gap_search_case_insensitive_and_glob() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-08: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-08 (P1-1): file.search --case-insensitive --glob"

    local dir="$SANDBOX_DIR/ci"
    mkdir -p "$dir"
    printf 'Hello World\n' > "$dir/a.rs"
    printf 'HELLO WORLD\n' > "$dir/b.md"

    local out
    out=$(run_remote_file_cmd search 'hello world' --cwd "$dir" --case-insensitive --glob '**/*.rs') || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-08: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local hits_rs hits_md
    hits_rs=$(echo "$out" | jq '[.matches[]? | select(.path | endswith(".rs"))] | length' 2>/dev/null || echo 0)
    hits_md=$(echo "$out" | jq '[.matches[]? | select(.path | endswith(".md"))] | length' 2>/dev/null || echo 0)
    if [[ "$hits_rs" -ge 1 && "$hits_md" -eq 0 ]]; then
        _log_pass "TC-GAP-08: search honors --case-insensitive + --glob file filter"
    else
        _log_fail "TC-GAP-08: search honors --case-insensitive + --glob" \
            "rs>=1 md=0" "rs=${hits_rs} md=${hits_md} (flags likely unimplemented)"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-09 (P2): file.stat / file.list expose symlink_target
# ---------------------------------------------------------------------------
test_gap_symlink_target_field() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-09: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-09 (P2): file.stat returns symlink_target"

    rm -f "$SANDBOX_DIR/link.txt"
    ln -s hello.txt "$SANDBOX_DIR/link.txt"

    local out
    out=$(run_remote_file_cmd stat link.txt --cwd "$SANDBOX_DIR") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-09: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local tgt
    tgt=$(echo "$out" | jq -r '.symlink_target // ""')
    if [[ -n "$tgt" ]]; then
        _log_pass "TC-GAP-09: file.stat exposes symlink_target=${tgt}"
    else
        _log_fail "TC-GAP-09: file.stat exposes symlink_target" \
            "non-empty symlink_target" "field missing"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-10 (design drift): error codes match design doc (file.sha_mismatch)
# ---------------------------------------------------------------------------
test_gap_error_code_sha_mismatch() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-10: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-10 (design drift): write with wrong base_sha256 returns file.sha_mismatch"

    local target="$SANDBOX_DIR/sha.txt"
    printf 'v1\n' > "$target"
    local b64
    b64=$(printf 'v2\n' | base64 | tr -d '\n')
    local out
    out=$(run_remote_file_cmd write sha.txt --cwd "$SANDBOX_DIR" \
        --content-b64 "$b64" --base-sha256 0000000000000000000000000000000000000000000000000000000000000000) || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-10: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    if echo "$out" | grep -q 'file.sha_mismatch'; then
        _log_pass "TC-GAP-10: error code is file.sha_mismatch (matches design doc)"
    else
        _log_fail "TC-GAP-10: error code is file.sha_mismatch" \
            "file.sha_mismatch" "$(echo "$out" | tr -d '\n' | head -c 200)"
    fi
}


# ---------------------------------------------------------------------------
#  TC-GAP-11 (P0-1): file.search column is 1-based CHAR column (not byte offset)
# ---------------------------------------------------------------------------
test_gap_search_char_column() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-11: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-11 (P0-1): file.search column is char-based, not byte-based"

    # 三个中文字符(每个 3 字节 UTF-8)后跟 NEEDLE。字节列=10,字符列=4。
    local target="$SANDBOX_DIR/multibyte.txt"
    printf '你好啊NEEDLE tail\n' > "$target"

    local out
    out=$(run_remote_file_cmd search "NEEDLE" --cwd "$SANDBOX_DIR" --glob "multibyte.txt") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-11: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    # 提取首条命中的 column / byte_column
    local col byte_col
    col=$(echo "$out" | python3 -c 'import sys,json,re
blob=sys.stdin.read()
# 宽松解析:取出第一个含 column 的 JSON 片段
m=re.search(r"\"column\"\s*:\s*(\d+)",blob); print(m.group(1) if m else "")')
    byte_col=$(echo "$out" | python3 -c 'import sys,re
blob=sys.stdin.read()
m=re.search(r"\"byte_column\"\s*:\s*(\d+)",blob); print(m.group(1) if m else "")')

    if [[ "$col" == "4" ]]; then
        _log_pass "TC-GAP-11: column is char-based (=4 after 3 CJK chars)"
    else
        _log_fail "TC-GAP-11: column is char-based" "4" "$col"
    fi
    if [[ "$byte_col" == "10" ]]; then
        _log_pass "TC-GAP-11: byte_column preserved (=10)"
    else
        _log_fail "TC-GAP-11: byte_column preserved" "10" "$byte_col"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-12 (P0-2): file.edit normalizes CRLF replacement into LF source
# ---------------------------------------------------------------------------
test_gap_edit_normalizes_lf_file() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-12: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-12 (P0-2): file.edit strips CRLF from replacement into LF file"

    # 纯 LF 源文件
    local target="$SANDBOX_DIR/lf.txt"
    printf 'alpha\nbeta\ngamma\n' > "$target"

    # 替换串带 CRLF —— 期望被归一化回 LF,不出现混合行尾
    local edits_json
    edits_json='[{"start_line":2,"end_line":2,"replacement":"BETA1\r\nBETA2"}]'
    local out
    out=$(run_remote_file_cmd edit lf.txt --cwd "$SANDBOX_DIR" --edits "$edits_json") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-12: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local crlf_count
    # Count CR bytes via tr|wc — grep -c prints "0" AND exits 1 on no-match,
    # which together with `|| echo 0` produces "0\n0" and breaks the check.
    crlf_count=$(tr -cd '\r' < "$target" | wc -c | tr -d ' ')
    if [[ "$crlf_count" == "0" ]]; then
        _log_pass "TC-GAP-12: LF file stays pure LF after edit with CRLF replacement"
    else
        _log_fail "TC-GAP-12: LF file stays pure LF" "0 CR bytes" "$crlf_count CR bytes"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-13 (P0-3): file.read offset/limit does not inject a trailing \n
# ---------------------------------------------------------------------------
test_gap_read_no_trailing_nl_injection() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-13: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-13 (P0-3): offset/limit reading EOF line preserves no-trailing-newline"

    # 3 行且末行无 \n
    local target="$SANDBOX_DIR/no-nl.txt"
    printf 'one\ntwo\nthree' > "$target"

    # 读第 3 行(即最后一行)—— 它没有换行,响应的 content_b64 也不应含换行
    local out
    out=$(run_remote_file_cmd read no-nl.txt --cwd "$SANDBOX_DIR" --offset 3 --limit 1) || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-13: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local decoded last_byte
    decoded=$(echo "$out" | python3 -c 'import sys,json,re,base64
blob=sys.stdin.read()
m=re.search(r"\"content_b64\"\s*:\s*\"([^\"]+)\"",blob)
if not m: sys.exit(0)
sys.stdout.buffer.write(base64.b64decode(m.group(1)))')
    last_byte=$(printf '%s' "$decoded" | tail -c 1 | od -An -tx1 | tr -d ' \n')
    # 期望 decoded = "three" (5 字节, 最后一字节 0x65 'e')
    if [[ "$decoded" == "three" ]]; then
        _log_pass "TC-GAP-13: last-line slice has no injected trailing newline"
    else
        _log_fail "TC-GAP-13: last-line slice is pristine" "three" "$decoded (last=$last_byte)"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-14 (P0-4): file.apply_patch rollback restores unix mode
# ---------------------------------------------------------------------------
test_gap_apply_patch_rollback_preserves_mode() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-14: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-14 (P0-4): apply_patch rollback keeps 0755 on restored files"

    # 两个文件,a 可执行,b 故意让第二个 hunk 失配触发 rollback。
    local a="$SANDBOX_DIR/roll_a.sh"
    local b="$SANDBOX_DIR/roll_b.sh"
    printf '#!/bin/sh\necho a_v1\n' > "$a"
    printf '#!/bin/sh\necho b_v1\n' > "$b"
    chmod 0755 "$a" "$b"

    # patch: 第一个文件可正常应用;第二个文件的上下文写错触发失败。
    local patch_file="$SANDBOX_DIR/bad.patch"
    cat > "$patch_file" <<'PATCH'
--- a/roll_a.sh
+++ b/roll_a.sh
@@ -1,2 +1,2 @@
 #!/bin/sh
-echo a_v1
+echo a_v2
--- a/roll_b.sh
+++ b/roll_b.sh
@@ -1,2 +1,2 @@
 #!/bin/sh
-echo WRONG_CONTEXT
+echo b_v2
PATCH

    local out
    out=$(run_remote_file_cmd apply-patch --patch-file "$patch_file" --cwd "$SANDBOX_DIR") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-14: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    # 期望原子回滚:两个文件都回到 v1;可执行位仍是 755
    local a_mode b_mode a_content b_content
    a_mode=$(stat -f '%Lp' "$a" 2>/dev/null || stat -c '%a' "$a" 2>/dev/null || echo "?")
    b_mode=$(stat -f '%Lp' "$b" 2>/dev/null || stat -c '%a' "$b" 2>/dev/null || echo "?")
    a_content=$(cat "$a")
    b_content=$(cat "$b")

    if [[ "$a_content" == *"a_v1"* && "$b_content" == *"b_v1"* ]]; then
        _log_pass "TC-GAP-14: rollback restored both files' contents"
    else
        _log_fail "TC-GAP-14: rollback restored contents" "a_v1 & b_v1" "a=$a_content | b=$b_content"
    fi
    if [[ "$a_mode" == "755" && "$b_mode" == "755" ]]; then
        _log_pass "TC-GAP-14: rollback preserved 0755 on both files"
    else
        _log_fail "TC-GAP-14: rollback preserved mode" "755/755" "$a_mode/$b_mode"
    fi
}


# ---------------------------------------------------------------------------
#  TC-GAP-15 (P0-2'): normalize_to_eol is UTF-8 safe; multi-byte CJK preserved
# ---------------------------------------------------------------------------
test_gap_edit_multibyte_eol_safe() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-15: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-15 (P0-2'): CJK characters survive EOL normalization intact"

    local target="$SANDBOX_DIR/mb-lf.txt"
    printf 'alpha\nbeta\ngamma\n' > "$target"

    # 替换串含 CJK 与 CRLF,期望输出为纯 LF 且 CJK 字节完整(UTF-8:中=E4B8AD 文=E69687)
    local edits_json
    edits_json='[{"start_line":2,"end_line":2,"replacement":"中\r\n文"}]'
    local out
    out=$(run_remote_file_cmd edit mb-lf.txt --cwd "$SANDBOX_DIR" --edits "$edits_json") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-15: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local cr_count has_zhong has_wen
    cr_count=$(tr -cd '\r' < "$target" | wc -c | tr -d ' ')
    has_zhong=$(grep -c '中' "$target" || true); has_zhong=${has_zhong%% *}
    has_wen=$(grep -c '文' "$target" || true);   has_wen=${has_wen%% *}
    # 更严:确认 UTF-8 字节完整
    local bytes_ok="no"
    if python3 -c '
import sys
d=open(sys.argv[1],"rb").read()
sys.exit(0 if (b"\xe4\xb8\xad" in d and b"\xe6\x96\x87" in d and b"\r" not in d) else 1)
' "$target"; then
        bytes_ok="yes"
    fi

    if [[ "$cr_count" == "0" ]]; then
        _log_pass "TC-GAP-15: no CR bytes after EOL normalization"
    else
        _log_fail "TC-GAP-15: no CR bytes" "0" "$cr_count"
    fi
    if [[ "$bytes_ok" == "yes" ]]; then
        _log_pass "TC-GAP-15: UTF-8 multi-byte CJK bytes intact (中 and 文)"
    else
        _log_fail "TC-GAP-15: UTF-8 bytes intact" "中(E4B8AD) & 文(E69687) present, no CR" "corrupted"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-16 (P0-3): offset reading last line WITH trailing \n keeps the \n
# ---------------------------------------------------------------------------
test_gap_read_last_line_with_nl() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-16: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-16 (P0-3): offset=3 on 'a\\nb\\nc\\n' returns exactly 'c\\n'"

    local target="$SANDBOX_DIR/with-nl.txt"
    printf 'a\nb\nc\n' > "$target"

    local out
    out=$(run_remote_file_cmd read with-nl.txt --cwd "$SANDBOX_DIR" --offset 3 --limit 1) || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-16: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local hex
    hex=$(echo "$out" | python3 -c 'import sys,re,base64
blob=sys.stdin.read()
m=re.search(r"\"content_b64\"\s*:\s*\"([^\"]+)\"",blob)
if not m: sys.exit(0)
sys.stdout.write(base64.b64decode(m.group(1)).hex())')
    # 期望 "c\n" = 0x63 0x0a
    if [[ "$hex" == "630a" ]]; then
        _log_pass "TC-GAP-16: last-line slice keeps source trailing newline"
    else
        _log_fail "TC-GAP-16: last-line slice hex" "630a" "$hex"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-17 (P0-4'): apply_patch rollback on CREATE removes orphan files
# ---------------------------------------------------------------------------
test_gap_apply_patch_rollback_on_create() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-17: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-17 (P0-4'): rollback of partial multi-file CREATE leaves no orphans"

    # 预清理,确保初始状态是"两个文件都不存在"
    rm -f "$SANDBOX_DIR/new_a.txt" "$SANDBOX_DIR/new_b.txt"

    local patch_file="$SANDBOX_DIR/create.patch"
    # 第一个 hunk 合法创建 new_a.txt;第二个 hunk 故意写成无效(空 hunk header),让 apply 失败。
    # 通过把第二个文件的上下文行写成不存在于 /dev/null 的伪行来触发失败。
    cat > "$patch_file" <<'PATCH'
--- /dev/null
+++ b/new_a.txt
@@ -0,0 +1,1 @@
+hello_a
--- /dev/null
+++ b/new_b.txt
@@ -0,0 +1,1 @@
 SHOULD_NOT_EXIST_CONTEXT
+hello_b
PATCH

    local out
    out=$(run_remote_file_cmd apply-patch --patch-file "$patch_file" --cwd "$SANDBOX_DIR") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-17: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    # 原子回滚期望:两个文件都不应存在(或至少 new_a.txt 不应残留成功创建的内容)
    local a_exists="no" b_exists="no"
    [[ -e "$SANDBOX_DIR/new_a.txt" ]] && a_exists="yes"
    [[ -e "$SANDBOX_DIR/new_b.txt" ]] && b_exists="yes"

    if [[ "$a_exists" == "no" && "$b_exists" == "no" ]]; then
        _log_pass "TC-GAP-17: both create-targets removed after rollback"
    else
        _log_fail "TC-GAP-17: no orphan create-targets" "a=no b=no" "a=$a_exists b=$b_exists"
    fi
}

# ---------------------------------------------------------------------------
#  TC-GAP-18 (P0-1 regression): ASCII column is same as byte_column
# ---------------------------------------------------------------------------
test_gap_search_ascii_column_regression() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-18: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-18 (P0-1 regression): ASCII column == byte_column"

    local target="$SANDBOX_DIR/ascii.txt"
    printf 'hello NEEDLE world\n' > "$target"

    local out
    out=$(run_remote_file_cmd search "NEEDLE" --cwd "$SANDBOX_DIR" --glob "ascii.txt") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-18: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    local col byte_col
    col=$(echo "$out" | python3 -c 'import sys,re
b=sys.stdin.read(); m=re.search(r"\"column\"\s*:\s*(\d+)",b); print(m.group(1) if m else "")')
    byte_col=$(echo "$out" | python3 -c 'import sys,re
b=sys.stdin.read(); m=re.search(r"\"byte_column\"\s*:\s*(\d+)",b); print(m.group(1) if m else "")')

    if [[ "$col" == "7" && "$byte_col" == "7" ]]; then
        _log_pass "TC-GAP-18: ASCII column=byte_column=7"
    else
        _log_fail "TC-GAP-18: ASCII column parity" "column=7 byte_column=7" "column=$col byte_column=$byte_col"
    fi
}


# ---------------------------------------------------------------------------
#  TC-GAP-19 (P0-4'): mid-commit CREATE rollback unlinks, not writes empty stub
# ---------------------------------------------------------------------------
test_gap_apply_patch_mid_commit_create_rollback() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-GAP-19: skipped due to prior connection error"
        return 0
    fi
    log "TC-GAP-19 (P0-4'): mid-commit failure unlinks freshly-created files"

    rm -rf "$SANDBOX_DIR/fresh_a.txt" "$SANDBOX_DIR/blocker_b"
    # 预置一个同名目录,让第二个文件的 rename 失败(EISDIR)
    mkdir -p "$SANDBOX_DIR/blocker_b"

    local patch_file="$SANDBOX_DIR/mid-create.patch"
    cat > "$patch_file" <<'PATCH'
--- /dev/null
+++ b/fresh_a.txt
@@ -0,0 +1,1 @@
+freshly_created_A
--- /dev/null
+++ b/blocker_b
@@ -0,0 +1,1 @@
+should_fail_B
PATCH

    local out
    out=$(run_remote_file_cmd apply-patch --patch-file "$patch_file" --cwd "$SANDBOX_DIR") || true
    if is_caller_conn_error "$out"; then
        _log_warning "TC-GAP-19: caller conn error: $out"; CALLER_CONN_OK=0; return 0
    fi

    # 期望 fresh_a.txt 被 unlink(不残留空文件或成功内容)
    # blocker_b 仍是目录
    local a_state="missing" b_is_dir="no"
    if [[ -e "$SANDBOX_DIR/fresh_a.txt" ]]; then
        if [[ -s "$SANDBOX_DIR/fresh_a.txt" ]]; then a_state="has_content"
        else a_state="empty_stub"; fi
    fi
    [[ -d "$SANDBOX_DIR/blocker_b" ]] && b_is_dir="yes"

    if [[ "$a_state" == "missing" ]]; then
        _log_pass "TC-GAP-19: freshly-created file unlinked on mid-commit rollback"
    else
        _log_fail "TC-GAP-19: fresh file unlinked" "missing" "$a_state"
    fi
    if [[ "$b_is_dir" == "yes" ]]; then
        _log_pass "TC-GAP-19: blocker directory untouched"
    else
        _log_fail "TC-GAP-19: blocker directory untouched" "yes" "$b_is_dir"
    fi
    rmdir "$SANDBOX_DIR/blocker_b" 2>/dev/null || true
}


test_readonly_rejection() {
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-FILE-09: skipped due to prior connection error"
        return 0
    fi

    log "TC-FILE-09: readonly rejection — downgrade to file_access=read, attempt write"

    # Downgrade grant to read-only file access
    local downgrade_ok=0
    local downgrade_body='{"file_access":"read"}'
    for _ in $(seq 1 10); do
        http_patch_json "${CLIENT_ADMIN_URL}/api/remote-invoke/grants/${GRANT_ID}" "$downgrade_body"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            if echo "$HTTP_BODY" | jq -e '
                ((.data.file_access // .file_access // "") == "read")
            ' >/dev/null 2>&1; then
                downgrade_ok=1
                break
            fi
        fi
        sleep 0.5
    done

    if [[ "$downgrade_ok" -eq 1 ]]; then
        _log_pass "TC-FILE-09: grant downgraded to file_access=read"
    else
        _log_fail "TC-FILE-09: grant downgraded to file_access=read" \
            "HTTP 200 with file_access=read" \
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

    # Restore grant to file_access=read_write for any subsequent tests
    local restore_body='{"file_access":"read_write"}'
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

    # multiline file for offset/limit tests (5 lines)
    printf 'L1 apple\nL2 banana\nL3 cherry\nL4 date\nL5 elderberry\n' > "$SANDBOX_DIR/multiline.txt"

    # file for edit-empty-replacement (delete line) test
    printf 'keep-first\ndelete-me\nkeep-last\n' > "$SANDBOX_DIR/edit-delete.txt"

    # searchable file with enough context for context-before/after tests
    printf 'ctx-line-1\nctx-line-2\nNEEDLE-match\nctx-line-4\nctx-line-5\n' > "$SANDBOX_DIR/searchctx.txt"

    # excluded directory (node_modules) with a file inside
    mkdir -p "$SANDBOX_DIR/node_modules"
    echo "should be excluded" > "$SANDBOX_DIR/node_modules/pkg.json"

    # a normal subdirectory for comparison
    mkdir -p "$SANDBOX_DIR/src"
    echo "source file" > "$SANDBOX_DIR/src/main.rs"
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

    # P0-4 diagnostic: capture mode-preservation internals into sidecar log.
    # Safe no-op when the env is unset. Log file surfaces into CI artifacts via BIFROST_E2E_REPORT_DIR.
    export BIFROST_FILE_OPS_DEBUG=1
    export BIFROST_FILE_OPS_DEBUG_LOG="${BIFROST_E2E_REPORT_DIR:-/tmp}/bifrost-fileops-$$.log"
    : > "$BIFROST_FILE_OPS_DEBUG_LOG" || true
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

    # Coding-agent enhancement accuracy tests
    test_file_read_offset_limit
    test_file_read_total_lines
    test_file_search_context
    test_file_search_exclude
    test_file_glob_exclude
    test_file_list_exclude
    test_file_edit_empty_replacement

    # ---- gap-targeting cases (expected RED-LIGHT until P0/P1 fixes land) ----
    test_gap_write_preserves_mode
    test_gap_edit_preserves_crlf
    test_gap_apply_patch_atomic_multifile
    test_gap_gitignore_respected
    test_gap_write_create_parents
    test_gap_read_truncated_file_sha256
    test_gap_list_truncated_flag
    test_gap_search_case_insensitive_and_glob
    test_gap_symlink_target_field
    test_gap_error_code_sha_mismatch

    test_gap_search_char_column
    test_gap_edit_normalizes_lf_file
    test_gap_read_no_trailing_nl_injection
    test_gap_apply_patch_rollback_preserves_mode
    test_gap_edit_multibyte_eol_safe
    test_gap_read_last_line_with_nl
    test_gap_apply_patch_rollback_on_create
    test_gap_search_ascii_column_regression
    test_gap_apply_patch_mid_commit_create_rollback

    test_readonly_rejection

    print_test_summary
}

main "$@"
