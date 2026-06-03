#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18891}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
PROXY_PID=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]]; then
        export BIFROST_DATA_DIR="$TEST_DATA_DIR"
        "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$PROXY_PID"
    kill_bifrost_on_port "$PROXY_PORT"
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "$BIFROST_BIN" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    (cd "$PROJECT_DIR" && cargo build --release --bin bifrost)
}

wait_admin_ready() {
    local waited=0
    while [[ "$waited" -lt 60 ]]; do
        if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            break
        fi
        sleep 0.5
        waited=$((waited + 1))
    done

    echo "Bifrost did not become ready. Log:"
    cat "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true
    return 1
}

start_proxy() {
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    wait_admin_ready
}

response_header() {
    local file="$1"
    local name="$2"
    awk -v target="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')" '
        BEGIN { FS=":" }
        NF >= 2 {
            key=tolower($1)
            if (key == target) {
                value=substr($0, index($0, ":") + 1)
                gsub(/^[ \t\r]+|[ \t\r]+$/, "", value)
                print value
                exit
            }
        }
    ' "$file"
}

request_static() {
    local path="$1"
    local accept_encoding="$2"
    local header_file="$3"
    local body_file="$4"

    curl -sS -D "$header_file" -o "$body_file" \
        -H "Accept-Encoding: ${accept_encoding}" \
        -w "%{http_code}" \
        "http://127.0.0.1:${PROXY_PORT}${path}"
}

assert_gzip_webui_response() {
    local path="$1"
    local label="$2"
    local headers="${TEST_DATA_DIR}/${label}.headers"
    local body="${TEST_DATA_DIR}/${label}.body"
    local decoded="${TEST_DATA_DIR}/${label}.decoded"
    local status

    status="$(request_static "$path" "gzip" "$headers" "$body")"
    assert_status "200" "$status" "${label}: gzip client receives 200" || return 1
    assert_equals "gzip" "$(response_header "$headers" "content-encoding")" "${label}: Content-Encoding is gzip" || return 1
    assert_equals "Accept-Encoding" "$(response_header "$headers" "vary")" "${label}: Vary is Accept-Encoding" || return 1
    gzip -dc "$body" > "$decoded" || {
        _log_fail "${label}: gzip body decompresses" "valid gzip" "decompression failed"
        return 1
    }
    assert_body_contains "<!doctype html" "$(tr '[:upper:]' '[:lower:]' < "$decoded")" "${label}: decoded body is WebUI HTML"
}

assert_non_gzip_upgrade_required() {
    local headers="${TEST_DATA_DIR}/identity.headers"
    local body="${TEST_DATA_DIR}/identity.body"
    local status

    status="$(request_static "/_bifrost/" "identity" "$headers" "$body")"
    assert_status "426" "$status" "non-gzip client receives 426" || return 1
    assert_equals "Accept-Encoding" "$(response_header "$headers" "vary")" "non-gzip response varies on Accept-Encoding" || return 1
    assert_body_contains "gzip-capable browser" "$(cat "$body")" "non-gzip response asks user to upgrade browser"
}

main() {
    build_bifrost || { echo "编译失败"; exit 1; }

    TEST_DATA_DIR="$(mktemp -d)"
    kill_bifrost_on_port "$PROXY_PORT"
    start_proxy || exit 1

    assert_gzip_webui_response "/_bifrost/" "index"
    assert_gzip_webui_response "/_bifrost/rules/not-a-real-asset" "spa_fallback"
    assert_non_gzip_upgrade_required

    print_test_summary || exit 1
}

main "$@"
