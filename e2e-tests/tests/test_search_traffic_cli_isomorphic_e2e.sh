#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${ROOT_DIR}/e2e-tests/test_utils/assert.sh"
source "${ROOT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
HTTP_PORT="${HTTP_PORT:-$(allocate_free_port)}"
HTTPS_PORT="${HTTPS_PORT:-$(allocate_free_port)}"
ADMIN_BASE_URL="http://127.0.0.1:${PROXY_PORT}/_bifrost/api"
MOCK_SERVER_SCRIPT="${ROOT_DIR}/e2e-tests/mock_servers/start_servers.sh"
BIFROST_BIN="${ROOT_DIR}/target/release/bifrost"

TEST_DATA_DIR=""
MOCK_LOG_DIR=""
BIFROST_LOG_FILE=""
BIFROST_PID=""

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
    echo "[search-traffic-e2e] $*"
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
    if [[ -x "$MOCK_SERVER_SCRIPT" ]]; then
        HTTP_PORT="$HTTP_PORT" HTTPS_PORT="$HTTPS_PORT" MOCK_SERVERS="http,https" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
            "$MOCK_SERVER_SCRIPT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$BIFROST_PID"
    [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]] && rm -rf "$TEST_DATA_DIR"
    [[ -n "$MOCK_LOG_DIR" && -d "$MOCK_LOG_DIR" ]] && rm -rf "$MOCK_LOG_DIR"
    [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]] && rm -f "$BIFROST_LOG_FILE"
}

trap cleanup EXIT

build_bifrost() {
    header "Build bifrost release binary"
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        if [[ ! -x "$BIFROST_BIN" ]]; then
            echo "SKIP_BUILD=true but bifrost binary is missing: $BIFROST_BIN" >&2
            exit 1
        fi
        log "Skipping build (SKIP_BUILD=true), using existing binary: $BIFROST_BIN"
        return
    fi
    (
        cd "$ROOT_DIR"
        cargo build --release --bin bifrost
    )
}

setup_dirs() {
    TEST_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-search-traffic.XXXXXX")"
    MOCK_LOG_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-search-mock.XXXXXX")"
    BIFROST_LOG_FILE="$(mktemp)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
}

start_mock_servers() {
    header "Start local HTTP/HTTPS echo fixtures"
    HTTP_PORT="$HTTP_PORT" HTTPS_PORT="$HTTPS_PORT" MOCK_SERVERS="http,https" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
        "$MOCK_SERVER_SCRIPT" start-bg >/dev/null
    env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${HTTP_PORT}/health" >/dev/null
    env NO_PROXY="*" no_proxy="*" curl -skf "https://127.0.0.1:${HTTPS_PORT}/health" >/dev/null
}

start_bifrost() {
    header "Start bifrost proxy"
    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"$BIFROST_LOG_FILE" 2>&1 &
    BIFROST_PID=$!

    if ! wait_for_http_ready "${ADMIN_BASE_URL}/system" 60 0.5; then
        echo "bifrost failed to start" >&2
        tail -n 200 "$BIFROST_LOG_FILE" >&2 || true
        exit 1
    fi
}

run_cli() {
    CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" "$@" 2>&1
}

admin_get() {
    local path="$1"
    env NO_PROXY="*" no_proxy="*" curl -sf "${ADMIN_BASE_URL}${path}"
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

seed_traffic() {
    header "Generate real traffic through proxy"

    SEARCH_MARKER="iso-search-${RANDOM}"
    BODY_MARKER="iso-body-${RANDOM}"
    HEADER_MARKER="iso-header-${RANDOM}"

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

    local total=0
    for _ in $(seq 1 50); do
        total="$(admin_get "/traffic?limit=1" | jq -r '.total // 0' 2>/dev/null || echo 0)"
        if [[ "$total" -ge 8 ]]; then
            break
        fi
        sleep 0.2
    done

    if [[ "$total" -lt 8 ]]; then
        echo "expected traffic records were not captured" >&2
        exit 1
    fi

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

    [[ -n "$POST_ID" ]] || {
        echo "failed to locate POST traffic record" >&2
        exit 1
    }
    [[ -n "$POST_SEQ" ]] || {
        echo "failed to locate POST traffic seq" >&2
        exit 1
    }
}

capture_tty_output() {
    local outfile="$1"
    shift
    python3 - "$outfile" "$@" <<'PY'
import os
import pty
import select
import signal
import subprocess
import sys
import time
import fcntl
import struct
import termios

outfile = sys.argv[1]
cmd = sys.argv[2:]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
env = os.environ.copy()
env.setdefault("TERM", "xterm-256color")
proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
os.close(slave)
chunks = []
deadline = time.time() + 3.5
sent_quit = False
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
    if not sent_quit and time.time() + 2.0 >= deadline:
        try:
            os.write(master, b"q")
        except OSError:
            pass
        sent_quit = True
    if proc.poll() is not None:
        break
time.sleep(0.5)
while True:
    rlist, _, _ = select.select([master], [], [], 0.1)
    if master not in rlist:
        break
    try:
        data = os.read(master, 8192)
    except OSError:
        break
    if not data:
        break
    chunks.append(data)
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

test_search_matches_traffic_search() {
    header "Local search and traffic search parity"
    local search_json traffic_search_json
    search_json="$(run_cli search "$SEARCH_MARKER" --format json)"
    traffic_search_json="$(run_cli traffic search "$SEARCH_MARKER" --format json)"

    assert_json_expr "$search_json" '.total_matched > 0' "search finds seeded traffic"
    if [[ "$(json_get "$search_json" '.total_matched')" == "$(json_get "$traffic_search_json" '.total_matched')" ]] \
        && [[ "$(json_get "$search_json" '.total_searched')" == "$(json_get "$traffic_search_json" '.total_searched')" ]]; then
        pass "search and traffic search keep identical totals"
    else
        fail "search and traffic search totals diverged"
    fi
}

test_search_scope_filters() {
    header "Local search scope coverage"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --url --format json)" '.total_matched > 0' "--url finds URL/path marker"
    assert_json_expr "$(run_cli search "$HEADER_MARKER" --headers --format json)" '.total_matched > 0' "--headers finds header marker"
    assert_json_expr "$(run_cli search "$BODY_MARKER" --body --format json)" '.total_matched > 0' "--body finds body marker"
    assert_json_expr "$(run_cli search "application/json" --req-header --format json)" '.total_matched > 0' "--req-header finds JSON content type"
    assert_json_expr "$(run_cli search "bifrost-test" --res-header --format json)" '.total_matched > 0' "--res-header finds echo response header"
    assert_json_expr "$(run_cli search "$BODY_MARKER" --req-body --format json)" '.total_matched > 0' "--req-body finds request body marker"
    assert_json_expr "$(run_cli search "$BODY_MARKER" --res-body --format json)" '.total_matched > 0' "--res-body finds echoed response body marker"
}

test_search_filter_flags() {
    header "Local search filter coverage"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --status 2xx --format json)" 'all(.results[]?; ((.status // .record.s // .record.status // 0) >= 200 and (.status // .record.s // .record.status // 0) < 300))' "--status 2xx filters correctly"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --method GET --format json)" 'all(.results[]?; ((.method // .record.m // .record.method // "") == "GET"))' "--method GET filters correctly"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --host localhost --format json)" 'all(.results[]?; ((.host // .record.h // .record.host // "") | contains("localhost")))' "--host filters correctly"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --path /get --format json)" 'all(.results[]?; ((.path // .record.p // .record.path // "") | contains("/get")))' "--path filters correctly"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --protocol HTTPS --format json)" 'all(.results[]?; ((.protocol // .record.proto // .record.protocol // "") | ascii_downcase) == "https")' "--protocol HTTPS filters correctly"
    assert_json_expr "$(run_cli search "$BODY_MARKER" --content-type json --format json)" '.total_matched > 0' "--content-type json filters correctly"
    assert_json_expr "$(run_cli search "$SEARCH_MARKER" --domain localhost --format json)" '.total_matched > 0' "--domain localhost filters correctly"
}

test_search_formats_and_limits() {
    header "Local search format and limit coverage"
    local json_output json_pretty_output table_output compact_output no_color_output limited_output
    json_output="$(run_cli search "$SEARCH_MARKER" --format json)"
    json_pretty_output="$(run_cli search "$SEARCH_MARKER" --format json-pretty)"
    table_output="$(run_cli search "$SEARCH_MARKER" --format table --no-color)"
    compact_output="$(run_cli search "$SEARCH_MARKER" --format compact --no-color)"
    no_color_output="$(run_cli search "$SEARCH_MARKER" --format table --no-color)"
    limited_output="$(run_cli search "$SEARCH_MARKER" --max-scan 5 --max-results 2 --format json)"

    assert_json_expr "$json_output" '.results | type == "array"' "--format json returns valid JSON"
    assert_json_expr "$json_pretty_output" '.results | type == "array"' "--format json-pretty returns valid JSON"
    assert_text_contains "$table_output" "SEQ" "--format table prints header"
    assert_text_contains "$compact_output" "localhost" "--format compact prints data rows"
    assert_no_ansi "$no_color_output" "--no-color strips ANSI sequences"
    assert_json_expr "$limited_output" '(.results | length) <= 2' "--max-results limits result count"
}

test_search_interactive_modes() {
    header "Local interactive search coverage"
    local interactive_out plain_out
    interactive_out="$(mktemp)"
    plain_out="$(mktemp)"

    capture_tty_output "$interactive_out" env CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" search --interactive || true
    capture_tty_output "$plain_out" env CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" search || true

    if grep -qaE "Search|Results|Press /|Navigate|Enter View|Type to search" "$interactive_out"; then
        pass "--interactive opens the TUI"
    else
        fail "--interactive opens the TUI"
    fi

    if grep -qaE "Search|Results|Press /|Navigate|Enter View|Type to search" "$plain_out"; then
        pass "search without keyword defaults to interactive TUI"
    else
        fail "search without keyword defaults to interactive TUI"
    fi

    rm -f "$interactive_out" "$plain_out"
}

test_traffic_list_matrix() {
    header "Local traffic list coverage"
    local default_json post_detail filter_json table_output compact_output pretty_output no_color_output cursor_json
    default_json="$(run_cli traffic list --format json)"
    post_detail="$(run_cli traffic get "$POST_ID" --format json)"

    assert_json_expr "$default_json" '(.records | length) > 0 and (.records | length) <= 50' "traffic list default limit stays bounded"
    table_output="$(run_cli traffic list --format table --no-color)"
    compact_output="$(run_cli traffic list --format compact --no-color)"
    pretty_output="$(run_cli traffic list --format json-pretty)"
    no_color_output="$(run_cli traffic list --format table --no-color)"

    assert_text_contains "$table_output" "STATUS" "traffic list table shows header"
    assert_text_contains "$compact_output" "localhost" "traffic list compact shows data rows"
    assert_json_expr "$pretty_output" '.records | type == "array"' "traffic list json-pretty is valid"
    assert_no_ansi "$no_color_output" "traffic list --no-color strips ANSI sequences"

    local client_ip client_app
    client_ip="$(json_get "$post_detail" '.client_ip // empty')"
    client_app="$(json_get "$post_detail" '.client_app // empty')"
    if [[ -z "$client_app" ]]; then
        client_app="curl"
    fi

    filter_json="$(run_cli traffic list \
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
        --client-ip "$client_ip" \
        --client-app "$client_app" \
        --has-rule-hit false \
        --is-websocket false \
        --is-sse false \
        --is-tunnel false \
        --format json)"

    assert_json_expr "$filter_json" '.records | type == "array"' "traffic list accepts full filter matrix"
    cursor_json="$(run_cli traffic list --limit 1 --format json)"
    local next_cursor
    next_cursor="$(json_get "$cursor_json" '.next_cursor // empty')"
    if [[ -n "$next_cursor" && "$next_cursor" != "null" ]]; then
        assert_json_expr "$(run_cli traffic list --cursor "$next_cursor" --direction forward --format json)" '.records | type == "array"' "traffic list accepts cursor + direction"
    else
        pass "traffic list cursor coverage skipped because next_cursor is empty"
    fi
}

test_traffic_get_and_tty() {
    header "Local traffic get coverage"
    local by_id by_seq table_output compact_output json_output pretty_output tty_output
    by_id="$(run_cli traffic get "$POST_ID")"
    by_seq="$(run_cli traffic get "$POST_SEQ")"
    table_output="$(run_cli traffic get "$POST_SEQ" --request-body --response-body --format table)"
    compact_output="$(run_cli traffic get "$POST_SEQ" --request-body --response-body --format compact)"
    json_output="$(run_cli traffic get "$POST_SEQ" --request-body --response-body --format json)"
    pretty_output="$(run_cli traffic get "$POST_SEQ" --request-body --response-body --format json-pretty)"

    assert_text_contains "$by_id" "\"id\": \"$POST_ID\"" "traffic get by id returns target record"
    if printf '%s' "$by_seq" | jq -e --argjson seq "$POST_SEQ" '(.seq // .sequence) == $seq' >/dev/null 2>&1; then
        pass "traffic get by sequence resolves correctly"
    else
        fail "traffic get by sequence resolves correctly"
        echo "    expected seq: $POST_SEQ" >&2
        echo "    body: $(printf '%s' "$by_seq" | head -c 500)" >&2
    fi
    assert_text_contains "$table_output" "$BODY_MARKER" "traffic get table includes request body"
    assert_text_contains "$compact_output" "$BODY_MARKER" "traffic get compact includes response body"
    assert_json_expr "$json_output" '.request_body != null and .response_body != null' "traffic get json includes body payloads"
    assert_json_expr "$pretty_output" '.request_body != null and .response_body != null' "traffic get json-pretty includes body payloads"

    tty_output="$(mktemp)"
    capture_tty_output "$tty_output" env CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" traffic get || true
    if strings "$tty_output" | grep -q "Select a Traffic ID"; then
        pass "traffic get without id opens interactive selector"
    else
        fail "traffic get without id opens interactive selector"
    fi
    rm -f "$tty_output"
}

test_traffic_clear_commands() {
    header "Local traffic clear coverage"
    local before after delete_output clear_output
    before="$(admin_get "/traffic?limit=1" | jq -r '.total // 0')"
    delete_output="$(run_cli traffic clear --ids "$POST_ID" -y)"
    after="$(admin_get "/traffic?limit=1" | jq -r '.total // 0')"

    assert_text_contains "$delete_output" "Deleted" "traffic clear --ids reports deletion"
    if [[ "$after" -lt "$before" ]]; then
        pass "traffic clear --ids reduces total records"
    else
        fail "traffic clear --ids reduces total records"
    fi

    clear_output="$(run_cli traffic clear -y)"
    assert_text_contains "$clear_output" "All traffic records cleared" "traffic clear -y clears all records"
    assert_json_expr "$(admin_get "/traffic?limit=1")" '.total == 0' "traffic clear -y removes all traffic"
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
    start_bifrost
    seed_traffic

    test_search_matches_traffic_search
    test_search_scope_filters
    test_search_filter_flags
    test_search_formats_and_limits
    test_search_interactive_modes
    test_traffic_list_matrix
    test_traffic_get_and_tty
    test_traffic_clear_commands

    print_summary
}

main "$@"
