#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

TEMP_PORT_BASE="${TEMP_PORT_BASE:-$((20000 + ((RANDOM + $$) % 20000)))}"
TEMP_PORT_OFFSET=0

allocate_local_test_port() {
    local __outvar="$1"
    local candidate
    while true; do
        candidate=$((TEMP_PORT_BASE + TEMP_PORT_OFFSET))
        TEMP_PORT_OFFSET=$((TEMP_PORT_OFFSET + 1))
        if [[ "$candidate" -eq 9900 || "$candidate" -gt 60999 ]]; then
            continue
        fi
        if command -v lsof >/dev/null 2>&1 && lsof -ti :"$candidate" >/dev/null 2>&1; then
            continue
        fi
        printf -v "$__outvar" '%s' "$candidate"
        return 0
    done
}

assign_local_test_port() {
    local var_name="$1"
    allocate_local_test_port "$var_name"
}

if [[ -z "${MAIN_PORT:-}" ]]; then
    assign_local_test_port MAIN_PORT
fi
if [[ -z "${TEMP_PORT:-}" ]]; then
    assign_local_test_port TEMP_PORT
fi
if [[ -z "${ORDER_PORT:-}" ]]; then
    assign_local_test_port ORDER_PORT
fi
if [[ -z "${FILE_PORT:-}" ]]; then
    assign_local_test_port FILE_PORT
fi
if [[ -z "${INLINE_PORT:-}" ]]; then
    assign_local_test_port INLINE_PORT
fi
if [[ -z "${UPDATE_PORT:-}" ]]; then
    assign_local_test_port UPDATE_PORT
fi
if [[ -z "${MISSING_PORT:-}" ]]; then
    assign_local_test_port MISSING_PORT
fi
if [[ -z "${HTML_PORT:-}" ]]; then
    assign_local_test_port HTML_PORT
fi
BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
if [[ ! -x "$BIFROST_BIN" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
fi
TEST_DATA_DIR=""
PROXY_PID=""
HTML_PID=""

cleanup() {
    if [[ -n "${TEST_DATA_DIR}" ]]; then
        for port in "${TEMP_PORT}" "${ORDER_PORT}" "${FILE_PORT}" "${INLINE_PORT}" "${UPDATE_PORT}"; do
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" port destroy "${port}" >/dev/null 2>&1 || true
        done
        if [[ -n "${AUTO_PORT:-}" ]]; then
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" port destroy "${AUTO_PORT}" >/dev/null 2>&1 || true
        fi
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$PROXY_PID"
    safe_cleanup_proxy "$HTML_PID"
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "$BIFROST_BIN" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    cargo build --bin bifrost
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
}

wait_for_admin() {
    local waited=0
    until curl -fsS "http://127.0.0.1:${MAIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; do
        if [[ "$waited" -gt 100 ]]; then
            echo "proxy did not become ready"
            cat "${TEST_DATA_DIR}/proxy.log" || true
            return 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
}

start_proxy() {
    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" start \
        --host 127.0.0.1 \
        -p "${MAIN_PORT}" \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        > "${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    wait_for_admin
}

start_html_server() {
    python3 - "$HTML_PORT" <<'PY' >/dev/null 2>&1 &
import http.server
import sys

port = int(sys.argv[1])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"<html><body>badge fixture</body></html>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
    HTML_PID=$!
    for _ in {1..60}; do
        if curl -fsS "http://127.0.0.1:${HTML_PORT}/index.html" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    echo "html server did not become ready"
    return 1
}

proxy_body() {
    local port="$1"
    local url="$2"
    curl -sS --max-time 5 -x "http://127.0.0.1:${port}" "${url}" || true
}

traffic_json() {
    curl -fsS "http://127.0.0.1:${MAIN_PORT}/_bifrost/api/traffic?limit=100"
}

traffic_detail_json() {
    local id="$1"
    curl -fsS "http://127.0.0.1:${MAIN_PORT}/_bifrost/api/traffic/${id}"
}

record_id_for_path_and_port() {
    local path="$1"
    local port="$2"
    python3 -c '
import json, sys
path, port = sys.argv[1], int(sys.argv[2])
data = json.load(sys.stdin)
for item in data.get("records", []):
    if item.get("p") == path and item.get("lp") == port:
        print(item.get("id", ""))
        raise SystemExit(0)
raise SystemExit(1)
' "$path" "$port"
}

wait_for_record() {
    local path="$1"
    local port="$2"
    local id=""
    for _ in {1..40}; do
        if id="$(traffic_json | record_id_for_path_and_port "$path" "$port" 2>/dev/null)"; then
            if [[ -n "$id" ]]; then
                echo "$id"
                return 0
            fi
        fi
        sleep 0.25
    done
    return 1
}

extract_auto_port() {
    sed -n 's/^Temporary port: .*:\([0-9][0-9]*\)$/\1/p' "$1" | head -1
}

main() {
    build_bifrost
    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"

    "$BIFROST_BIN" rule add main-default -c "main-only.test status://209 resBody://(main-default)"
    "$BIFROST_BIN" rule add temp-bound -c "temp-only.test status://210 resBody://(temp-bound)"
    "$BIFROST_BIN" rule add main-badge -c "badge-main.test host://127.0.0.1:${HTML_PORT}"
    "$BIFROST_BIN" rule add temp-badge -c "badge-temp.test host://127.0.0.1:${HTML_PORT}"
    "$BIFROST_BIN" rule disable temp-bound
    "$BIFROST_BIN" rule disable temp-badge

    start_html_server
    start_proxy

    "$BIFROST_BIN" port bind --port "${TEMP_PORT}" --rule temp-bound --rule temp-badge > "${TEST_DATA_DIR}/bind.log"
    assert_body_contains "Temporary port" "$(cat "${TEST_DATA_DIR}/bind.log")" "port bind output"
    "$BIFROST_BIN" port show "${TEMP_PORT}" > "${TEST_DATA_DIR}/show.log"
    assert_body_contains "temp-bound" "$(cat "${TEST_DATA_DIR}/show.log")" "port show includes bound rule"
    "$BIFROST_BIN" port list > "${TEST_DATA_DIR}/ports.log"
    assert_body_contains "${TEMP_PORT}" "$(cat "${TEST_DATA_DIR}/ports.log")" "port list includes specified port"

    local main_body temp_body
    main_body="$(proxy_body "${MAIN_PORT}" "http://main-only.test/main-port")"
    assert_body_contains "main-default" "$main_body" "main port should use default enabled rule"

    temp_body="$(proxy_body "${TEMP_PORT}" "http://temp-only.test/temp-port")"
    assert_body_contains "temp-bound" "$temp_body" "temporary port should use explicitly bound disabled rule"

    local main_badge_body temp_badge_body
    main_badge_body="$(proxy_body "${MAIN_PORT}" "http://badge-main.test/badge-main")"
    assert_body_contains "__bifrost_badge__" "$main_badge_body" "main port HTML should receive Bifrost badge"
    assert_body_contains "main-badge" "$main_badge_body" "main port badge should include main active rule"
    if [[ "$main_badge_body" == *"temp-badge"* ]]; then
        _log_fail "main port badge ignores temporary bound rule" "no temp-badge" "$main_badge_body"
        return 1
    fi
    _log_pass "main port badge ignores temporary bound rule"

    temp_badge_body="$(proxy_body "${TEMP_PORT}" "http://badge-temp.test/badge-temp")"
    assert_body_contains "__bifrost_badge__" "$temp_badge_body" "temporary port HTML should receive Bifrost badge"
    assert_body_contains "temp-badge" "$temp_badge_body" "temporary port badge should include bound rule"
    if [[ "$temp_badge_body" == *"main-badge"* || "$temp_badge_body" == *"main-default"* ]]; then
        _log_fail "temporary port badge ignores main active rules" "no main-badge/main-default" "$temp_badge_body"
        return 1
    fi
    _log_pass "temporary port badge ignores main active rules"

    local temp_main_body main_temp_body
    temp_main_body="$(proxy_body "${TEMP_PORT}" "http://main-only.test/not-bound")"
    if [[ "$temp_main_body" == *"main-default"* ]]; then
        _log_fail "temporary port ignores main enabled rules" "no main-default" "$temp_main_body"
        return 1
    fi
    _log_pass "temporary port ignores main enabled rules"

    main_temp_body="$(proxy_body "${MAIN_PORT}" "http://temp-only.test/disabled-on-main")"
    if [[ "$main_temp_body" == *"temp-bound"* ]]; then
        _log_fail "main port ignores disabled temp rule" "no temp-bound" "$main_temp_body"
        return 1
    fi
    _log_pass "main port ignores disabled temp rule"

    local direct_main_body direct_temp_body
    direct_main_body="$(proxy_body "${MAIN_PORT}" "http://127.0.0.1:${HTML_PORT}/direct-main")"
    assert_body_contains "badge fixture" "$direct_main_body" "main port direct no-rule request should proxy"
    direct_temp_body="$(proxy_body "${TEMP_PORT}" "http://127.0.0.1:${HTML_PORT}/direct-temp")"
    assert_body_contains "badge fixture" "$direct_temp_body" "temporary port direct no-rule request should proxy"

    "$BIFROST_BIN" rule disable main-default
    "$BIFROST_BIN" rule enable main-default
    "$BIFROST_BIN" port active "${TEMP_PORT}" > "${TEST_DATA_DIR}/active.log"
    assert_body_contains "temp-bound" "$(cat "${TEST_DATA_DIR}/active.log")" "temporary active summary includes bound rule"
    if grep -q "main-default" "${TEST_DATA_DIR}/active.log"; then
        _log_fail "temporary active summary ignores default rule toggles" "no main-default" "$(cat "${TEST_DATA_DIR}/active.log")"
        return 1
    fi
    _log_pass "temporary active summary ignores default rule toggles"

    "$BIFROST_BIN" rule add temp-first -c "order.test status://211 resBody://(first)"
    "$BIFROST_BIN" rule add temp-second -c "order.test status://212 resBody://(second)"
    "$BIFROST_BIN" port bind --port "${ORDER_PORT}" --rule temp-first --rule temp-second > "${TEST_DATA_DIR}/order-bind.log"
    "$BIFROST_BIN" port active "${ORDER_PORT}" > "${TEST_DATA_DIR}/order-active.log"
    local first_line second_line
    first_line="$(grep -n "temp-first" "${TEST_DATA_DIR}/order-active.log" | head -1 | cut -d: -f1)"
    second_line="$(grep -n "temp-second" "${TEST_DATA_DIR}/order-active.log" | head -1 | cut -d: -f1)"
    if [[ -z "$first_line" || -z "$second_line" || "$first_line" -ge "$second_line" ]]; then
        _log_fail "temporary port preserves binding order" "temp-first before temp-second" "$(cat "${TEST_DATA_DIR}/order-active.log")"
        return 1
    fi
    _log_pass "temporary port preserves binding order"

    "$BIFROST_BIN" port bind --port 0 --rule temp-bound > "${TEST_DATA_DIR}/auto-bind.log"
    AUTO_PORT="$(extract_auto_port "${TEST_DATA_DIR}/auto-bind.log")"
    if [[ -z "${AUTO_PORT}" || "${AUTO_PORT}" == "0" || "${AUTO_PORT}" == "9900" ]]; then
        _log_fail "auto allocated temporary port" "non-zero non-9900 port" "$(cat "${TEST_DATA_DIR}/auto-bind.log")"
        return 1
    fi
    assert_body_contains "temp-bound" "$(proxy_body "${AUTO_PORT}" "http://temp-only.test/auto")" "auto allocated port should proxy bound rule"

    if "$BIFROST_BIN" port bind --port "${TEMP_PORT}" --rule temp-bound > "${TEST_DATA_DIR}/conflict.log" 2>&1; then
        _log_fail "duplicate temporary port fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "${TEMP_PORT}" "$(cat "${TEST_DATA_DIR}/conflict.log")" "conflict error includes port"
    assert_body_contains "already exists" "$(cat "${TEST_DATA_DIR}/conflict.log")" "conflict error explains duplicate binding"

    if "$BIFROST_BIN" port bind --port "${MAIN_PORT}" --rule temp-bound > "${TEST_DATA_DIR}/main-conflict.log" 2>&1; then
        _log_fail "main port conflict fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "${MAIN_PORT}" "$(cat "${TEST_DATA_DIR}/main-conflict.log")" "main port conflict includes port"
    assert_body_contains "main proxy port" "$(cat "${TEST_DATA_DIR}/main-conflict.log")" "main port conflict explains cause"

    if "$BIFROST_BIN" port bind --port "${MISSING_PORT}" > "${TEST_DATA_DIR}/no-rule.log" 2>&1; then
        _log_fail "missing rule input fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "At least one --rule" "$(cat "${TEST_DATA_DIR}/no-rule.log")" "no-rule error says a rule input is required"
    assert_body_contains "--rule-file" "$(cat "${TEST_DATA_DIR}/no-rule.log")" "no-rule error names rule-file input"
    assert_body_contains "--rule-text" "$(cat "${TEST_DATA_DIR}/no-rule.log")" "no-rule error names rule-text input"

    local missing_file="${TEST_DATA_DIR}/does-not-exist.bifrost"
    if "$BIFROST_BIN" port bind --port "${MISSING_PORT}" --rule-file "${missing_file}" > "${TEST_DATA_DIR}/missing-file.log" 2>&1; then
        _log_fail "missing rule-file binding fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "${missing_file}" "$(cat "${TEST_DATA_DIR}/missing-file.log")" "missing rule-file error includes path"
    assert_body_contains "could not be read" "$(cat "${TEST_DATA_DIR}/missing-file.log")" "missing rule-file error explains read failure"

    if "$BIFROST_BIN" port bind --port "${MISSING_PORT}" --rule-text "" > "${TEST_DATA_DIR}/empty-inline.log" 2>&1; then
        _log_fail "empty inline rule binding fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "Inline rule content is empty" "$(cat "${TEST_DATA_DIR}/empty-inline.log")" "empty inline rule error is specific"

    if "$BIFROST_BIN" port bind --port "${MISSING_PORT}" --rule not-exist > "${TEST_DATA_DIR}/missing.log" 2>&1; then
        _log_fail "missing rule binding fails" "non-zero exit" "success"
        return 1
    fi
    assert_body_contains "not-exist" "$(cat "${TEST_DATA_DIR}/missing.log")" "missing rule error names missing rule"
    "$BIFROST_BIN" port list > "${TEST_DATA_DIR}/ports-after-missing.log"
    if grep -q "${MISSING_PORT}" "${TEST_DATA_DIR}/ports-after-missing.log"; then
        _log_fail "missing rule does not leave binding" "port absent" "$(cat "${TEST_DATA_DIR}/ports-after-missing.log")"
        return 1
    fi
    _log_pass "missing rule does not leave binding"

    local rule_file="${TEST_DATA_DIR}/file-rule.bifrost"
    printf '%s\n' "file-only.test status://213 resBody://(file-rule)" > "${rule_file}"
    "$BIFROST_BIN" port bind --port "${FILE_PORT}" --rule-file "${rule_file}" > "${TEST_DATA_DIR}/file-bind.log"
    assert_body_contains "file-rule.bifrost" "$(cat "${TEST_DATA_DIR}/file-bind.log")" "rule-file binding output includes file name"
    assert_body_contains "file-rule" "$(proxy_body "${FILE_PORT}" "http://file-only.test/from-file")" "rule file binding should proxy"

    "$BIFROST_BIN" port bind --port "${INLINE_PORT}" --rule-text "inline-only.test status://214 resBody://(inline-rule)" > "${TEST_DATA_DIR}/inline-bind.log"
    assert_body_contains "inline:inline-only.test" "$(cat "${TEST_DATA_DIR}/inline-bind.log")" "rule-text binding output includes inline preview"
    assert_body_contains "inline-rule" "$(proxy_body "${INLINE_PORT}" "http://inline-only.test/from-inline")" "inline rule binding should proxy"

    "$BIFROST_BIN" port bind --port "${UPDATE_PORT}" --rule temp-bound > "${TEST_DATA_DIR}/update-bind.log"
    "$BIFROST_BIN" port update "${UPDATE_PORT}" --rule-text "updated-only.test status://215 resBody://(updated-rule)" > "${TEST_DATA_DIR}/update.log"
    assert_body_contains "inline:updated-only.test" "$(cat "${TEST_DATA_DIR}/update.log")" "port update accepts rule text"
    assert_body_contains "updated-rule" "$(proxy_body "${UPDATE_PORT}" "http://updated-only.test/after-update")" "updated temporary port should use new inline rule"

    local temp_record_id main_record_id direct_temp_record_id direct_main_record_id
    temp_record_id="$(wait_for_record "/temp-port" "${TEMP_PORT}")"
    main_record_id="$(wait_for_record "/main-port" "${MAIN_PORT}")"
    direct_temp_record_id="$(wait_for_record "/direct-temp" "${TEMP_PORT}")"
    direct_main_record_id="$(wait_for_record "/direct-main" "${MAIN_PORT}")"
    assert_body_contains "\"lp\":${TEMP_PORT}" "$(traffic_json)" "Traffic API compact records include temporary listener port"
    assert_body_contains "\"lp\":${MAIN_PORT}" "$(traffic_json)" "Traffic API compact records include main listener port"
    assert_body_contains "\"listener_port\":${TEMP_PORT}" "$(traffic_detail_json "${temp_record_id}")" "Traffic detail includes temporary listener port"
    assert_body_contains "\"listener_port\":${MAIN_PORT}" "$(traffic_detail_json "${main_record_id}")" "Traffic detail includes main listener port"
    assert_body_contains "\"listener_port\":${TEMP_PORT}" "$(traffic_detail_json "${direct_temp_record_id}")" "Traffic detail includes temporary listener port for no-rule request"
    assert_body_contains "\"listener_port\":${MAIN_PORT}" "$(traffic_detail_json "${direct_main_record_id}")" "Traffic detail includes main listener port for no-rule request"
    assert_body_contains "\"has_rule_hit\":false" "$(traffic_detail_json "${direct_temp_record_id}")" "temporary no-rule request remains marked as no rule hit"
    assert_body_contains "\"has_rule_hit\":false" "$(traffic_detail_json "${direct_main_record_id}")" "main no-rule request remains marked as no rule hit"
    "$BIFROST_BIN" traffic list --port "${MAIN_PORT}" --format json > "${TEST_DATA_DIR}/traffic-list.json"
    assert_body_contains "\"lp\":${TEMP_PORT}" "$(cat "${TEST_DATA_DIR}/traffic-list.json")" "CLI traffic list JSON includes temporary listener port"
    "$BIFROST_BIN" traffic list --port "${MAIN_PORT}" --listener-port "${TEMP_PORT}" --format json > "${TEST_DATA_DIR}/traffic-list-temp-port.json"
    assert_body_contains "\"lp\":${TEMP_PORT}" "$(cat "${TEST_DATA_DIR}/traffic-list-temp-port.json")" "CLI traffic list filters by temporary listener port"
    if grep -q "\"lp\":${MAIN_PORT}" "${TEST_DATA_DIR}/traffic-list-temp-port.json"; then
        _log_fail "CLI traffic list excludes main listener port when filtering temporary port" "no main listener port rows" "found main listener port"
        return 1
    fi
    _log_pass "CLI traffic list excludes main listener port when filtering temporary port"
    "$BIFROST_BIN" traffic search "port" --listener-port "${TEMP_PORT}" --format json --max-results 20 --max-scan 200 > "${TEST_DATA_DIR}/traffic-search-temp-port.json"
    assert_body_contains "/temp-port" "$(cat "${TEST_DATA_DIR}/traffic-search-temp-port.json")" "CLI traffic search filters by temporary listener port"
    if grep -q "/main-port" "${TEST_DATA_DIR}/traffic-search-temp-port.json"; then
        _log_fail "CLI traffic search excludes main listener port when filtering temporary port" "no main listener port rows" "found main listener port"
        return 1
    fi
    _log_pass "CLI traffic search excludes main listener port when filtering temporary port"
    "$BIFROST_BIN" traffic get --port "${MAIN_PORT}" "${temp_record_id}" --format json > "${TEST_DATA_DIR}/traffic-get.json"
    assert_body_contains "\"listener_port\":${TEMP_PORT}" "$(cat "${TEST_DATA_DIR}/traffic-get.json")" "CLI traffic get JSON includes temporary listener port"

    "$BIFROST_BIN" port destroy "${TEMP_PORT}"
    if curl -sS --max-time 2 -x "http://127.0.0.1:${TEMP_PORT}" "http://temp-only.test/" >/dev/null 2>&1; then
        _log_fail "temporary port destroyed" "connection failure" "request succeeded"
        return 1
    fi
    _log_pass "temporary port destroyed"

    main_body="$(proxy_body "${MAIN_PORT}" "http://main-only.test/main-after-destroy")"
    assert_body_contains "main-default" "$main_body" "main port should remain available after temporary destroy"

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
