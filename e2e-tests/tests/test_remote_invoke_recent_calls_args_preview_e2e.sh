#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

log() { echo "[remote-invoke-args-preview-e2e] $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd curl
require_cmd jq
require_cmd cargo
require_cmd npx

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""
CURL_ERROR=""

http_request() {
    local url="$1"
    local method="${2:-GET}"
    local data="${3:-}"
    local extra_headers="${4:-}"

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

    if [[ -n "$extra_headers" ]]; then
        while IFS= read -r header; do
            [[ -n "$header" ]] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    CURL_ERROR=""
    HTTP_STATUS="$(curl "${curl_args[@]}" "$url" 2>"$err_file")" || HTTP_STATUS="000"
    HTTP_HEADERS="$(cat "$headers_file" | tr -d '\r')"
    HTTP_BODY="$(cat "$body_file")"
    CURL_ERROR="$(cat "$err_file")"

    rm -f "$headers_file" "$body_file" "$err_file"
}

http_get() {
    http_request "$1" "GET" "" "${2:-}"
}

http_post_json() {
    http_request "$1" "POST" "$2" "${3:-}"
}

RELAY_PORT="$(pick_free_port)"
ADMIN_PORT="$(pick_free_port)"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$REPO_DIR/.bifrost-e2e-remote-invoke-args-preview-$RANDOM}"
CALLER_DATA_DIR="$(mktemp -d)"
RELAY_LOG="$(mktemp)"
RELAY_DATA_DIR="$(mktemp -d)"
CALLER_CONNECT_LOG="$(mktemp)"
SEARCH_LOG="$(mktemp)"
RELAY_PID=""
CALLER_CONNECT_PID=""

cleanup() {
    if [[ -n "$CALLER_CONNECT_PID" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "$RELAY_PID" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    rm -rf "$BIFROST_DATA_DIR" "$CALLER_DATA_DIR" "$RELAY_DATA_DIR"
    rm -f "$RELAY_LOG" "$CALLER_CONNECT_LOG" "$SEARCH_LOG"
}
trap cleanup EXIT

log "Starting local relay on port $RELAY_PORT"
RELAY_EXEC="$(sync_server_exec "$SYNC_SERVER_DIR")"
(cd "$SYNC_SERVER_DIR" && eval "$RELAY_EXEC" -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke) >"$RELAY_LOG" 2>&1 &
RELAY_PID=$!

for _ in $(seq 1 40); do
    code="$(curl -s -o /dev/null -w '%{http_code}' "${RELAY_URL}/v4/remote-invoke/client/register" 2>/dev/null || true)"
    if [[ "$code" =~ ^(200|4..) ]]; then
        break
    fi
    sleep 0.5
done
if ! kill -0 "$RELAY_PID" 2>/dev/null; then
    cat "$RELAY_LOG" >&2
    exit 1
fi

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
    NEED_BUILD=0
    if [[ ! -x "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.toml" -nt "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.lock" -nt "$REPO_DIR/target/release/bifrost" ]]; then
        NEED_BUILD=1
    elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$REPO_DIR/target/release/bifrost" -print -quit | grep -q .; then
        NEED_BUILD=1
    fi

    if [[ "$NEED_BUILD" -eq 1 ]]; then
        log "Building release bifrost"
        (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null)
    fi
fi
BIFROST_BIN="$REPO_DIR/target/release/bifrost"

mkdir -p "$BIFROST_DATA_DIR"
cat > "$BIFROST_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF

export ADMIN_PORT
export ADMIN_HOST="0.0.0.0"
export ADMIN_PATH_PREFIX
export BIFROST_DATA_DIR
log "Starting bifrost admin on port $ADMIN_PORT"
admin_start_bifrost

SYNC_USER_ID="remote_invoke_args_preview_${RANDOM}"
SYNC_PASSWORD="remote_invoke_args_preview_123"

http_post_json "${RELAY_URL}/v4/sso/register" "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"${SYNC_PASSWORD}\",\"nickname\":\"Args Preview E2E\"}"
assert_status "200" "$HTTP_STATUS" "relay 注册测试用户应返回 200"
RELAY_SYNC_TOKEN="$(echo "$HTTP_BODY" | jq -r '.data.token // ""')"
assert_not_empty "$RELAY_SYNC_TOKEN" "relay token 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/sync/session" "{\"token\":\"${RELAY_SYNC_TOKEN}\"}"
assert_status "200" "$HTTP_STATUS" "保存 sync session 应返回 200"

WORKER_READY=0
for _ in $(seq 1 40); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/status"
    if [[ "$HTTP_STATUS" == "200" ]] && [[ "$(echo "$HTTP_BODY" | jq -r '.state // ""')" == "Connected" ]]; then
        WORKER_READY=1
        break
    fi
    sleep 1
done
if [[ "$WORKER_READY" -ne 1 ]]; then
    _log_fail "remote invoke worker 未连接 relay" "state=Connected" "${HTTP_BODY:-<empty>}"
    exit 1
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/identity"
assert_status "200" "$HTTP_STATUS" "读取 client identity 应返回 200"
CLIENT_INSTANCE_ID="$(echo "$HTTP_BODY" | jq -r '.instance_id // ""')"
assert_not_empty "$CLIENT_INSTANCE_ID" "client instance_id 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
assert_status "200" "$HTTP_STATUS" "进入 discovery 应返回 200"
PAIR_CODE="$(echo "$HTTP_BODY" | jq -r '.session.pair_code // ""')"
assert_not_empty "$PAIR_CODE" "pair_code 不应为空"

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote connect "$PAIR_CODE" --relay-url "$RELAY_URL" >"$CALLER_CONNECT_LOG" 2>&1 &
CALLER_CONNECT_PID=$!

PAIRING_ID=""
for _ in $(seq 1 30); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
    PAIRING_ID="$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id // ""')"
    [[ -n "$PAIRING_ID" ]] && break
    sleep 1
done
assert_not_empty "$PAIRING_ID" "pairing_id 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${PAIRING_ID}/approve" '{"grant_mode":"permanent"}'
assert_status "200" "$HTTP_STATUS" "批准配对应返回 200"

wait "$CALLER_CONNECT_PID"
CONNECT_EXIT=$?
CALLER_CONNECT_PID=""
if [[ "$CONNECT_EXIT" -ne 0 ]]; then
    _log_fail "remote connect 失败" "exit 0" "$(cat "$CALLER_CONNECT_LOG")"
    exit 1
fi

REMOTE_MARKER="recent-calls-preview-${RANDOM}"
curl -sS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "http://httpbin.org/anything/${REMOTE_MARKER}" >/dev/null
sleep 2

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote search "$REMOTE_MARKER" \
    --relay-url "$RELAY_URL" \
    --client-id "${CLIENT_INSTANCE_ID:0:12}" \
    --max-results 5 \
    --max-scan 50 >"$SEARCH_LOG" 2>&1

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "读取 Recent Calls API 应返回 200"

LATEST_SEARCH_MASKED_ARGS_JSON="$(echo "$HTTP_BODY" | jq -r --arg marker "$REMOTE_MARKER" '
    (.calls // [])
    | map(select(
        (.command_summary.command_preview // "") == "search.stream"
        and (
            (.command_summary.masked_args_json // "") | contains($marker)
        )
    ))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].command_summary.masked_args_json // ""
')"

assert_not_empty "$LATEST_SEARCH_MASKED_ARGS_JSON" "search.stream 的 masked_args_json 不应为空"

if echo "$LATEST_SEARCH_MASKED_ARGS_JSON" | jq -e --arg marker "$REMOTE_MARKER" '
    .keyword == $marker
    and .max_results == 5
    and .max_scan == 50
' >/dev/null 2>&1; then
    _log_pass "TC-RI-ARGS-01: Recent Calls 会展示 search.stream 的参数摘要"
else
    _log_fail "TC-RI-ARGS-01: Recent Calls 参数摘要缺少 query/max_results/max_scan" \
        'keyword=<marker>, max_results=5, max_scan=50' \
        "$LATEST_SEARCH_MASKED_ARGS_JSON"
    exit 1
fi

log "Recent Calls args preview E2E passed"
