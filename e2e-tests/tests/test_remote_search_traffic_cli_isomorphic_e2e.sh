#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SYNC_SERVER_DIR="${ROOT_DIR}/packages/bifrost-sync-server"

source "${ROOT_DIR}/e2e-tests/test_utils/assert.sh"
source "${ROOT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
HTTP_PORT="${HTTP_PORT:-$(allocate_free_port)}"
HTTPS_PORT="${HTTPS_PORT:-$(allocate_free_port)}"
RELAY_PORT="${RELAY_PORT:-$(allocate_free_port)}"
ADMIN_BASE_URL="http://127.0.0.1:${PROXY_PORT}/_bifrost/api"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
MOCK_SERVER_SCRIPT="${ROOT_DIR}/e2e-tests/mock_servers/start_servers.sh"
BIFROST_BIN="${ROOT_DIR}/target/release/bifrost"

TARGET_DATA_DIR=""
CALLER_DATA_DIR=""
RELAY_DATA_DIR=""
MOCK_LOG_DIR=""
TARGET_LOG_FILE=""
RELAY_LOG_FILE=""
CALLER_CONNECT_LOG=""
TARGET_PID=""
RELAY_PID=""
CALLER_CONNECT_PID=""

SYNC_USER_ID=""
SYNC_PASSWORD=""
RELAY_TOKEN=""
CLIENT_INSTANCE_ID=""

SEARCH_MARKER=""
BODY_MARKER=""
HEADER_MARKER=""
POST_ID=""
POST_SEQ=""

PASS_COUNT=0
FAIL_COUNT=0

header() {
    echo
    echo "============================================================"
    echo "$1"
    echo "============================================================"
}

log() {
    echo "[remote-search-traffic-e2e] $*"
}

pass() {
    echo "  [PASS] $*"
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    echo "  [FAIL] $*"
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

cleanup() {
    if [[ -n "$CALLER_CONNECT_PID" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    if [[ -x "$MOCK_SERVER_SCRIPT" ]]; then
        HTTP_PORT="$HTTP_PORT" HTTPS_PORT="$HTTPS_PORT" MOCK_SERVERS="http,https" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
            "$MOCK_SERVER_SCRIPT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$TARGET_PID"
    if [[ -n "$RELAY_PID" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    [[ -n "$TARGET_DATA_DIR" && -d "$TARGET_DATA_DIR" ]] && rm -rf "$TARGET_DATA_DIR"
    [[ -n "$CALLER_DATA_DIR" && -d "$CALLER_DATA_DIR" ]] && rm -rf "$CALLER_DATA_DIR"
    [[ -n "$RELAY_DATA_DIR" && -d "$RELAY_DATA_DIR" ]] && rm -rf "$RELAY_DATA_DIR"
    [[ -n "$MOCK_LOG_DIR" && -d "$MOCK_LOG_DIR" ]] && rm -rf "$MOCK_LOG_DIR"
    [[ -n "$TARGET_LOG_FILE" && -f "$TARGET_LOG_FILE" ]] && rm -f "$TARGET_LOG_FILE"
    [[ -n "$RELAY_LOG_FILE" && -f "$RELAY_LOG_FILE" ]] && rm -f "$RELAY_LOG_FILE"
    [[ -n "$CALLER_CONNECT_LOG" && -f "$CALLER_CONNECT_LOG" ]] && rm -f "$CALLER_CONNECT_LOG"
}

trap cleanup EXIT

build_bifrost() {
    header "Build bifrost release binary"
    (
        cd "$ROOT_DIR"
        cargo build --release --bin bifrost
    )
}

setup_dirs() {
    TARGET_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-target.XXXXXX")"
    CALLER_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-caller.XXXXXX")"
    RELAY_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-relay.XXXXXX")"
    MOCK_LOG_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-mock.XXXXXX")"
    TARGET_LOG_FILE="$(mktemp)"
    RELAY_LOG_FILE="$(mktemp)"
    CALLER_CONNECT_LOG="$(mktemp)"

    cat >"${TARGET_DATA_DIR}/config.toml" <<EOF
[sync]
remote_base_url = "${RELAY_URL}"
EOF
}

start_mock_servers() {
    header "Start local HTTP/HTTPS echo fixtures"
    HTTP_PORT="$HTTP_PORT" HTTPS_PORT="$HTTPS_PORT" MOCK_SERVERS="http,https" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
        "$MOCK_SERVER_SCRIPT" start-bg >/dev/null
    env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${HTTP_PORT}/health" >/dev/null
    env NO_PROXY="*" no_proxy="*" curl -skf "https://127.0.0.1:${HTTPS_PORT}/health" >/dev/null
}

start_relay() {
    header "Start local relay"
    (
        cd "$SYNC_SERVER_DIR"
        pnpm exec tsx src/cli.ts -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
    ) >"$RELAY_LOG_FILE" 2>&1 &
    RELAY_PID=$!

    for _ in $(seq 1 60); do
        local status
        status="$(curl -s -o /dev/null -w '%{http_code}' "${RELAY_URL}/v4/remote-invoke/client/register" 2>/dev/null || true)"
        if [[ "$status" =~ ^(200|4[0-9][0-9])$ ]]; then
            return 0
        fi
        if ! kill -0 "$RELAY_PID" 2>/dev/null; then
            echo "relay exited early" >&2
            tail -n 200 "$RELAY_LOG_FILE" >&2 || true
            exit 1
        fi
        sleep 0.5
    done

    echo "relay did not become ready" >&2
    tail -n 200 "$RELAY_LOG_FILE" >&2 || true
    exit 1
}

start_target_bifrost() {
    header "Start target bifrost"
    BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"$TARGET_LOG_FILE" 2>&1 &
    TARGET_PID=$!

    if ! wait_for_http_ready "${ADMIN_BASE_URL}/system" 90 0.5; then
        echo "target bifrost failed to start" >&2
        tail -n 200 "$TARGET_LOG_FILE" >&2 || true
        exit 1
    fi
}

json_get() {
    local json="$1"
    local filter="$2"
    printf '%s' "$json" | jq -r "$filter"
}

assert_json_expr() {
    local json="$1"
    local expr="$2"
    local message="$3"
    if printf '%s' "$json" | jq -e "$expr" >/dev/null 2>&1; then
        pass "$message"
    else
        fail "$message"
        echo "    jq expr: $expr" >&2
        echo "    body: $(printf '%s' "$json" | head -c 500)" >&2
    fi
}

assert_text_contains() {
    local text="$1"
    local needle="$2"
    local message="$3"
    if printf '%s' "$text" | grep -q -- "$needle"; then
        pass "$message"
    else
        fail "$message"
        echo "    expected substring: $needle" >&2
        echo "    output: $(printf '%s' "$text" | head -n 20)" >&2
    fi
}

assert_no_ansi() {
    local text="$1"
    local message="$2"
    if printf '%s' "$text" | perl -ne 'exit 1 if /\x1b\[[0-9;]*[A-Za-z]/; END { exit 0 }'; then
        pass "$message"
    else
        fail "$message"
    fi
}

capture_tty_output() {
    local outfile="$1"
    shift
    python3 - "$outfile" "$@" <<'PY'
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

outfile = sys.argv[1]
cmd = sys.argv[2:]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
env = os.environ.copy()
env.setdefault("TERM", "xterm-256color")
proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
os.close(slave)
chunks = []
deadline = time.time() + 20.0
while time.time() < deadline:
    rlist, _, _ = select.select([master], [], [], 0.2)
    if master in rlist:
        try:
            data = os.read(master, 8192)
        except OSError:
            break
        if not data:
            break
        chunks.append(data)
    if proc.poll() is not None:
        break
if proc.poll() is None:
    try:
        os.kill(proc.pid, signal.SIGINT)
    except ProcessLookupError:
        pass
try:
    proc.wait(timeout=2)
except subprocess.TimeoutExpired:
    proc.kill()
    proc.wait(timeout=2)
os.close(master)
with open(outfile, "wb") as fh:
    fh.write(b"".join(chunks))
PY
}

admin_get() {
    local path="$1"
    env NO_PROXY="*" no_proxy="*" curl -sf "${ADMIN_BASE_URL}${path}"
}

admin_post_json() {
    local path="$1"
    local body="$2"
    env NO_PROXY="*" no_proxy="*" curl -sf -X POST -H "Content-Type: application/json" \
        "${ADMIN_BASE_URL}${path}" -d "$body"
}

relay_post_json() {
    local path="$1"
    local body="$2"
    env NO_PROXY="*" no_proxy="*" curl -sf -X POST -H "Content-Type: application/json" \
        "${RELAY_URL}${path}" -d "$body"
}

bootstrap_remote_invoke() {
    header "Bootstrap relay auth and worker registration"
    SYNC_USER_ID="remote_query_e2e_${RANDOM}"
    SYNC_PASSWORD="remote_query_e2e_123"

    RELAY_TOKEN="$(relay_post_json "/v4/sso/register" "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"${SYNC_PASSWORD}\",\"nickname\":\"Remote Query E2E\"}" | jq -r '.data.token // ""')"
    [[ -n "$RELAY_TOKEN" ]] || {
        echo "failed to register relay user" >&2
        exit 1
    }

    admin_post_json "/sync/session" "{\"token\":\"${RELAY_TOKEN}\"}" >/dev/null

    for _ in $(seq 1 60); do
        local status_json
        status_json="$(admin_get "/remote-invoke/status" 2>/dev/null || true)"
        if [[ -n "$status_json" ]] && [[ "$(json_get "$status_json" '.state // ""')" == "Connected" ]]; then
            break
        fi
        sleep 1
    done

    CLIENT_INSTANCE_ID="$(admin_get "/remote-invoke/identity" | jq -r '.instance_id // ""')"
    [[ -n "$CLIENT_INSTANCE_ID" ]] || {
        echo "failed to fetch client instance id" >&2
        exit 1
    }
}

pair_caller() {
    header "Pair caller with target remote client"
    local pair_code pending_json pairing_id connect_exit

    pair_code="$(admin_post_json "/remote-invoke/discovery/enter" "{}" | jq -r '.session.pair_code // ""')"
    [[ -n "$pair_code" ]] || {
        echo "failed to get pair code" >&2
        exit 1
    }

    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote connect "$pair_code" --relay-url "$RELAY_URL" \
        >"$CALLER_CONNECT_LOG" 2>&1 &
    CALLER_CONNECT_PID=$!

    for _ in $(seq 1 60); do
        pending_json="$(admin_get "/remote-invoke/pairings/pending" 2>/dev/null || true)"
        pairing_id="$(printf '%s' "$pending_json" | jq -r '.pairings[0].pairing_id // ""' 2>/dev/null || true)"
        if [[ -n "$pairing_id" ]]; then
            break
        fi
        sleep 1
    done

    [[ -n "$pairing_id" ]] || {
        echo "pairing request did not arrive" >&2
        cat "$CALLER_CONNECT_LOG" >&2 || true
        exit 1
    }

    admin_post_json "/remote-invoke/pairings/${pairing_id}/approve" '{"grant_mode":"permanent"}' >/dev/null

    wait "$CALLER_CONNECT_PID" || connect_exit=$?
    connect_exit="${connect_exit:-0}"
    CALLER_CONNECT_PID=""

    if [[ "$connect_exit" -ne 0 ]] || ! grep -q "Connected! Authorization granted" "$CALLER_CONNECT_LOG"; then
        echo "remote connect failed" >&2
        cat "$CALLER_CONNECT_LOG" >&2 || true
        exit 1
    fi

    admin_post_json "/remote-invoke/discovery/exit" "{}" >/dev/null || true
}

run_remote_cli() {
    CI=1 BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote "$@" --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1
}

seed_traffic() {
    header "Generate real traffic through target proxy"

    SEARCH_MARKER="remote-search-${RANDOM}"
    BODY_MARKER="remote-body-${RANDOM}"
    HEADER_MARKER="remote-header-${RANDOM}"

    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -H "X-E2E-Marker: ${HEADER_MARKER}" \
        "http://localhost:${HTTP_PORT}/get?marker=${SEARCH_MARKER}" >/dev/null
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        "http://localhost:${HTTP_PORT}/status/404?marker=${SEARCH_MARKER}" >/dev/null || true
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        "http://localhost:${HTTP_PORT}/status/500?marker=${SEARCH_MARKER}" >/dev/null || true
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -H "Content-Type: application/json" \
        -H "X-E2E-Marker: ${HEADER_MARKER}" \
        -d "{\"marker\":\"${BODY_MARKER}\",\"kind\":\"json\"}" \
        "http://localhost:${HTTP_PORT}/post" >/dev/null
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -X PUT -d "marker=${BODY_MARKER}" \
        "http://localhost:${HTTP_PORT}/put" >/dev/null
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -X DELETE "http://localhost:${HTTP_PORT}/delete?marker=${SEARCH_MARKER}" >/dev/null
    env NO_PROXY="" no_proxy="" curl -skf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        "https://localhost:${HTTPS_PORT}/get?marker=${SEARCH_MARKER}" >/dev/null
    env NO_PROXY="" no_proxy="" curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -H "X-E2E-Marker: ${HEADER_MARKER}" \
        "http://localhost:${HTTP_PORT}/headers?marker=${SEARCH_MARKER}" >/dev/null

    for _ in $(seq 1 50); do
        local total
        total="$(admin_get "/traffic?limit=1" | jq -r '.total // 0' 2>/dev/null || echo 0)"
        if [[ "$total" -ge 8 ]]; then
            break
        fi
        sleep 0.2
    done

    POST_ID="$(admin_get "/traffic?limit=50" | jq -r '
        (.records // [])
        | map(select(((.method // .m // "") | ascii_downcase) == "post" and ((.path // .p // "") | contains("/post"))))
        | sort_by(.seq // .sequence // 0)
        | last
        | .id // empty
    ')"
    POST_SEQ="$(admin_get "/traffic?limit=50" | jq -r --arg id "$POST_ID" '
        (.records // [])
        | map(select(.id == $id))
        | .[0].seq // .[0].sequence // empty
    ')"

    [[ -n "$POST_ID" ]] || exit 1
    [[ -n "$POST_SEQ" ]] || exit 1
}

test_remote_search_parity() {
    header "Remote search parity"
    local search_json traffic_search_json filter_only_json
    search_json="$(run_remote_cli search "$SEARCH_MARKER" --format json)"
    traffic_search_json="$(run_remote_cli traffic search "$SEARCH_MARKER" --format json)"
    filter_only_json="$(run_remote_cli search --status 5xx --host localhost --format json)"

    assert_json_expr "$search_json" '.total_matched > 0' "remote search finds seeded traffic"
    if [[ "$(json_get "$search_json" '.total_matched')" == "$(json_get "$traffic_search_json" '.total_matched')" ]] \
        && [[ "$(json_get "$search_json" '.total_searched')" == "$(json_get "$traffic_search_json" '.total_searched')" ]]; then
        pass "remote search and remote traffic search keep identical totals"
    else
        fail "remote search and remote traffic search totals diverged"
    fi
    assert_json_expr "$filter_only_json" 'all(.results[]?; ((.status // .record.status // 0) >= 500 and (.status // .record.status // 0) < 600))' "remote search supports filter-only 5xx query"
}

test_remote_search_scope_and_filters() {
    header "Remote search scope and filter coverage"
    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --url --format json)" '.total_matched > 0' "remote --url finds URL/path marker"
    assert_json_expr "$(run_remote_cli search "$HEADER_MARKER" --headers --format json)" '.total_matched > 0' "remote --headers finds header marker"
    assert_json_expr "$(run_remote_cli search "$BODY_MARKER" --body --format json)" '.total_matched > 0' "remote --body finds body marker"
    assert_json_expr "$(run_remote_cli search "application/json" --req-header --format json)" '.total_matched > 0' "remote --req-header finds JSON content type"
    assert_json_expr "$(run_remote_cli search "bifrost-test" --res-header --format json)" '.total_matched > 0' "remote --res-header finds echo response header"
    assert_json_expr "$(run_remote_cli search "$BODY_MARKER" --req-body --format json)" '.total_matched > 0' "remote --req-body finds request body marker"
    assert_json_expr "$(run_remote_cli search "$BODY_MARKER" --res-body --format json)" '.total_matched > 0' "remote --res-body finds echoed response body marker"

    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --method GET --format json)" 'all(.results[]?; ((.method // .record.method // "") == "GET"))' "remote --method GET filters correctly"
    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --host localhost --format json)" 'all(.results[]?; ((.host // .record.host // "") | contains("localhost")))' "remote --host filters correctly"
    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --path /get --format json)" 'all(.results[]?; ((.path // .record.path // "") | contains("/get")))' "remote --path filters correctly"
    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --protocol HTTPS --format json)" 'all(.results[]?; ((.protocol // .record.protocol // "") | ascii_downcase) == "https")' "remote --protocol HTTPS filters correctly"
    assert_json_expr "$(run_remote_cli search "$BODY_MARKER" --content-type json --format json)" '.total_matched > 0' "remote --content-type json filters correctly"
    assert_json_expr "$(run_remote_cli search "$SEARCH_MARKER" --domain localhost --format json)" '.total_matched > 0' "remote --domain localhost filters correctly"
}

test_remote_search_formats_and_streaming() {
    header "Remote search format and streaming coverage"
    local json_output pretty_output table_output compact_output no_color_output stream_log
    json_output="$(run_remote_cli search "$SEARCH_MARKER" --format json)"
    pretty_output="$(run_remote_cli search "$SEARCH_MARKER" --format json-pretty)"
    table_output="$(run_remote_cli search "$SEARCH_MARKER" --format table --no-color)"
    compact_output="$(run_remote_cli search "$SEARCH_MARKER" --format compact --no-color)"
    no_color_output="$(run_remote_cli search "$SEARCH_MARKER" --format table --no-color)"

    assert_json_expr "$json_output" '.results | type == "array"' "remote --format json returns valid JSON"
    assert_json_expr "$pretty_output" '.results | type == "array"' "remote --format json-pretty returns valid JSON"
    assert_text_contains "$table_output" "SEQ" "remote --format table prints header"
    assert_text_contains "$compact_output" "localhost" "remote --format compact prints rows"
    assert_no_ansi "$no_color_output" "remote --no-color strips ANSI sequences"

    stream_log="$(mktemp)"
    capture_tty_output "$stream_log" env CI=1 BIFROST_DATA_DIR="$CALLER_DATA_DIR" \
        "$BIFROST_BIN" remote search "$SEARCH_MARKER" --format table \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" || true
    if grep -qaE "Searching\\.{3}|Found [0-9]+ matches|${SEARCH_MARKER}" "$stream_log"; then
        pass "remote search emits streaming progress before completion"
    else
        fail "remote search emits streaming progress before completion"
    fi
    rm -f "$stream_log"
}

test_remote_traffic_list_get_clear() {
    header "Remote traffic list/get/clear coverage"
    local default_json table_output compact_output pretty_output no_color_output filter_json by_id by_seq json_output reject_output
    default_json="$(run_remote_cli traffic list --format json || true)"
    table_output="$(run_remote_cli traffic list --format table --no-color || true)"
    compact_output="$(run_remote_cli traffic list --format compact --no-color || true)"
    pretty_output="$(run_remote_cli traffic list --format json-pretty || true)"
    no_color_output="$(run_remote_cli traffic list --format table --no-color || true)"

    assert_json_expr "$default_json" '(.records | length) > 0 and (.records | length) <= 50' "remote traffic list default limit stays bounded"
    assert_text_contains "$table_output" "STATUS" "remote traffic list table shows header"
    assert_text_contains "$compact_output" "localhost" "remote traffic list compact shows rows"
    assert_json_expr "$pretty_output" '.records | type == "array"' "remote traffic list json-pretty is valid"
    assert_no_ansi "$no_color_output" "remote traffic list --no-color strips ANSI sequences"

    filter_json="$(run_remote_cli traffic list \
        --limit 5 \
        --cursor 1 \
        --direction forward \
        --method POST \
        --status-min 200 \
        --status-max 299 \
        --protocol http \
        --host localhost \
        --url /post \
        --path /post \
        --content-type application/json \
        --has-rule-hit false \
        --is-websocket false \
        --is-sse false \
        --is-tunnel false \
        --format json || true)"
    assert_json_expr "$filter_json" '.records | type == "array"' "remote traffic list accepts full filter matrix"

    by_id="$(run_remote_cli traffic get "$POST_ID" --format json-pretty || true)"
    by_seq="$(run_remote_cli traffic get "$POST_SEQ" --request-body --response-body --format json || true)"
    json_output="$(run_remote_cli traffic get "$POST_SEQ" --request-body --response-body --format json-pretty || true)"

    assert_text_contains "$by_id" "\"id\": \"$POST_ID\"" "remote traffic get by id returns target record"
    if printf '%s' "$by_seq" | jq -e --argjson seq "$POST_SEQ" '(.seq // .sequence) == $seq' >/dev/null 2>&1; then
        pass "remote traffic get by sequence resolves correctly"
    else
        fail "remote traffic get by sequence resolves correctly"
        echo "    output: $(printf '%s' "$by_seq" | head -n 20)" >&2
    fi
    assert_json_expr "$json_output" '.request_body != null and .response_body != null' "remote traffic get includes body payloads"

    reject_output="$(run_remote_cli traffic clear --ids "$POST_ID" --yes || true)"
    assert_text_contains "$reject_output" "traffic.clear" "remote traffic clear remains rejected by default"
}

print_summary() {
    header "Summary"
    echo "passed: ${PASS_COUNT}"
    echo "failed: ${FAIL_COUNT}"
    if [[ "$FAIL_COUNT" -ne 0 ]]; then
        return 1
    fi
    return 0
}

main() {
    build_bifrost
    setup_dirs
    start_mock_servers
    start_relay
    start_target_bifrost
    bootstrap_remote_invoke
    pair_caller
    seed_traffic

    test_remote_search_parity
    test_remote_search_scope_and_filters
    test_remote_search_formats_and_streaming
    test_remote_traffic_list_get_clear

    print_summary
}

main "$@"
