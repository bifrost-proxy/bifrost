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
RELAY_URL=""
RELAY_DATA_DIR=""
TARGET_DATA_DIR=""
LOCAL_DATA_DIR=""
LOCAL_ADMIN_PORT=""
LOCAL_ADMIN_LOG=""
LOCAL_ADMIN_PID=""
CALLER_CONNECT_PID=""
CALLER_CONNECT_LOG=""
CLIENT_ADMIN_URL=""
CLIENT_INSTANCE_ID=""
CLIENT_INSTANCE_SHORT=""
HTTP_STATUS=""
HTTP_BODY=""
HTTP_HEADERS=""

log() {
    echo "[remote-v5-session-refresh-e2e] $*"
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
    local curl_args=(-sS -X "$method" --max-time 20 -D "$headers_file" -o "$body_file" -w '%{http_code}')
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
    RELAY_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-v5-relay-XXXXXX")"
    local relay_exec
    relay_exec="$(sync_server_exec "$SYNC_SERVER_DIR")"
    log "Starting relay on port $RELAY_PORT..."
    (
        cd "$SYNC_SERVER_DIR" && \
            eval "$relay_exec" -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
    ) >"$RELAY_LOG" 2>&1 &
    RELAY_PID=$!
    RELAY_URL="http://127.0.0.1:${RELAY_PORT}"

    for _ in $(seq 1 120); do
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
    done
    echo "Relay did not become ready:" >&2
    tail -80 "$RELAY_LOG" >&2 || true
    exit 1
}

start_local_bifrost() {
    LOCAL_ADMIN_PORT="$(pick_free_port)"
    LOCAL_ADMIN_LOG="$(mktemp)"
    mkdir -p "$LOCAL_DATA_DIR"
    cat >"$LOCAL_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF
    log "Starting local bifrost on port $LOCAL_ADMIN_PORT..."
    BIFROST_DATA_DIR="$LOCAL_DATA_DIR" "$BIFROST_BIN" -H 127.0.0.1 -p "$LOCAL_ADMIN_PORT" start \
        -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy >"$LOCAL_ADMIN_LOG" 2>&1 &
    LOCAL_ADMIN_PID=$!
    for _ in $(seq 1 90); do
        if curl -fsS "http://127.0.0.1:${LOCAL_ADMIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            log "Local bifrost ready (pid=$LOCAL_ADMIN_PID)"
            return 0
        fi
        if ! kill -0 "$LOCAL_ADMIN_PID" 2>/dev/null; then
            echo "Local bifrost exited early:" >&2
            cat "$LOCAL_ADMIN_LOG" >&2 || true
            exit 1
        fi
        sleep 0.5
    done
    echo "Local bifrost did not become ready:" >&2
    tail -80 "$LOCAL_ADMIN_LOG" >&2 || true
    exit 1
}

wait_for_worker_ready() {
    for _ in $(seq 1 30); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/status"
        if [[ "$HTTP_STATUS" == "200" ]] && [[ "$(echo "$HTTP_BODY" | jq -r '.state // ""')" == "Connected" ]]; then
            _log_pass "target remote invoke worker connected to relay"
            return 0
        fi
        sleep 1
    done
    _log_fail "target remote invoke worker connected to relay" "Connected" "${HTTP_BODY:-<empty>}"
    return 1
}

configure_target_shell_policy() {
    BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" setting shell policy add \
        --id v5-refresh-shell \
        --name "V5 Refresh Shell" \
        --mode shell_text \
        --pattern '^(?s:.*)$' \
        --shell /bin/sh \
        --cwd "$TARGET_DATA_DIR" \
        --timeout-ms 10000 >/dev/null
    _log_pass "target shell_text policy configured"
}

pair_and_approve() {
    local sync_user_id sync_password pair_code pairing_id relay_sync_token
    sync_user_id="remote_v5_refresh_${RANDOM}"
    sync_password="remote_v5_refresh_123"

    http_post_json "${RELAY_URL}/v4/sso/register" \
        "{\"user_id\":\"${sync_user_id}\",\"password\":\"${sync_password}\",\"nickname\":\"Remote V5 Refresh E2E\"}"
    assert_status "200" "$HTTP_STATUS" "relay user registration should return 200" || return 1
    relay_sync_token="$(echo "$HTTP_BODY" | jq -r '.data.token // ""')"
    assert_not_empty "$relay_sync_token" "relay sync token should not be empty" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/sync/session" "{\"token\":\"${relay_sync_token}\"}"
    assert_status "200" "$HTTP_STATUS" "target sync session save should return 200" || return 1
    wait_for_worker_ready || return 1

    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/identity"
    assert_status "200" "$HTTP_STATUS" "target identity should return 200" || return 1
    CLIENT_INSTANCE_ID="$(echo "$HTTP_BODY" | jq -r '.instance_id // ""')"
    CLIENT_INSTANCE_SHORT="${CLIENT_INSTANCE_ID:0:12}"
    assert_not_empty "$CLIENT_INSTANCE_ID" "target client instance id should not be empty" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
    assert_status "200" "$HTTP_STATUS" "target discovery enter should return 200" || return 1
    pair_code="$(echo "$HTTP_BODY" | jq -r '.session.pair_code // ""')"
    assert_not_empty "$pair_code" "pair code should not be empty" || return 1

    CALLER_CONNECT_LOG="$(mktemp)"
    BIFROST_DATA_DIR="$LOCAL_DATA_DIR" "$BIFROST_BIN" remote conn up "$pair_code" --relay-url "$RELAY_URL" \
        >"$CALLER_CONNECT_LOG" 2>&1 &
    CALLER_CONNECT_PID=$!

    for _ in $(seq 1 30); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
        pairing_id="$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id // ""')"
        [[ -n "$pairing_id" ]] && break
        sleep 1
    done
    assert_not_empty "$pairing_id" "pending pairing should arrive on target" || return 1

    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${pairing_id}/approve" \
        '{"grant_mode":"permanent","grant_scope":"remote_shell_exec","file_access":"read_write"}'
    assert_status "200" "$HTTP_STATUS" "approve pairing should return 200" || return 1

    local connect_ok=0
    for _ in $(seq 1 30); do
        if ! kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
            if wait "$CALLER_CONNECT_PID"; then connect_ok=1; fi
            break
        fi
        sleep 1
    done
    CALLER_CONNECT_PID=""
    assert_equals "1" "$connect_ok" "caller remote conn up should succeed" || {
        cat "$CALLER_CONNECT_LOG" >&2 || true
        return 1
    }
}

run_remote_shell_text() {
    local shell_text="$1"
    local expected="$2"
    local label="$3"
    local out_file exit_code output
    out_file="$(mktemp)"
    if BIFROST_DATA_DIR="$LOCAL_DATA_DIR" "$BIFROST_BIN" remote exec \
        --relay-url "$RELAY_URL" \
        --client-id "$CLIENT_INSTANCE_SHORT" \
        --shell-text "$shell_text" >"$out_file" 2>&1; then
        exit_code=0
    else
        exit_code=$?
    fi
    output="$(cat "$out_file")"
    rm -f "$out_file"
    assert_equals "0" "$exit_code" "$label should exit successfully" || {
        echo "$output" >&2
        return 1
    }
    assert_body_contains "$expected" "$output" "$label should include expected stdout" || return 1
}

expire_grant_session_tokens() {
    log "Forcing relay and local grant_session_token expiry..."
    python3 - "$RELAY_DATA_DIR/bifrost-sync.db" "$LOCAL_DATA_DIR/remote-connections.json" <<'PY'
import json
import sqlite3
import sys

db_path, conn_path = sys.argv[1], sys.argv[2]
with sqlite3.connect(db_path) as db:
    db.execute(
        "UPDATE bifrost_remote_invoke_grants SET session_token_expires_at = ? WHERE status = ?",
        ("2000-01-01T00:00:00Z", "active"),
    )
    db.commit()

with open(conn_path, "r", encoding="utf-8") as f:
    data = json.load(f)
for conn in data.get("connections", []):
    conn["grant_session_expires_at"] = "2000-01-01T00:00:00Z"
with open(conn_path, "w", encoding="utf-8") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
    f.write("\n")
PY
    _log_pass "relay and local session expiry forced"
}

cleanup() {
    if [[ -n "${CALLER_CONNECT_PID:-}" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "${LOCAL_ADMIN_PID:-}" ]] && kill -0 "$LOCAL_ADMIN_PID" 2>/dev/null; then
        kill "$LOCAL_ADMIN_PID" 2>/dev/null || true
        wait "$LOCAL_ADMIN_PID" 2>/dev/null || true
    fi
    if [[ -n "${RELAY_PID:-}" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    [[ -n "${RELAY_LOG:-}" ]] && rm -f "$RELAY_LOG" 2>/dev/null || true
    [[ -n "${LOCAL_ADMIN_LOG:-}" ]] && rm -f "$LOCAL_ADMIN_LOG" 2>/dev/null || true
    [[ -n "${CALLER_CONNECT_LOG:-}" ]] && rm -f "$CALLER_CONNECT_LOG" 2>/dev/null || true
    [[ -n "${RELAY_DATA_DIR:-}" ]] && rm -rf "$RELAY_DATA_DIR" 2>/dev/null || true
    [[ -n "${TARGET_DATA_DIR:-}" ]] && rm -rf "$TARGET_DATA_DIR" 2>/dev/null || true
    [[ -n "${LOCAL_DATA_DIR:-}" ]] && rm -rf "$LOCAL_DATA_DIR" 2>/dev/null || true
}
trap cleanup EXIT

main() {
    require_cmd cargo
    require_cmd curl
    require_cmd jq
    require_cmd node
    require_cmd python3

    TARGET_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-v5-target-XXXXXX")"
    LOCAL_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-v5-local-XXXXXX")"

    export ADMIN_HOST="127.0.0.1"
    export ADMIN_PORT="${ADMIN_PORT:-$(pick_free_port)}"
    export ADMIN_PATH_PREFIX="/_bifrost"
    export BIFROST_DATA_DIR="$TARGET_DATA_DIR"

    prepare_bifrost_bin
    start_local_relay
    start_local_bifrost

    mkdir -p "$TARGET_DATA_DIR"
    cat >"$TARGET_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF

    log "Starting target bifrost on port $ADMIN_PORT..."
    admin_start_bifrost
    CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

    configure_target_shell_policy
    pair_and_approve
    run_remote_shell_text 'printf FIRST_OK' 'FIRST_OK' 'initial remote exec'
    expire_grant_session_tokens

    local decomposed_shell_text
    decomposed_shell_text="$(python3 - <<'PY'
print('printf "Cafe\u0301_REFRESH_OK"')
PY
)"
    run_remote_shell_text "$decomposed_shell_text" '_REFRESH_OK' 'refreshed remote exec with decomposed Unicode PoP body'

    print_test_summary
}

main "$@"
