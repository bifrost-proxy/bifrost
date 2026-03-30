#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

pick_free_port() {
    python3 - <<'PY'
import socket

s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$(pick_free_port)}"
ADMIN_HOST="$PROXY_HOST"
ADMIN_PORT="$PROXY_PORT"
SSE_HOST="${SSE_HOST:-127.0.0.1}"
SSE_PORT="${SSE_PORT:-$(pick_free_port)}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
SSE_PROXY="http://${PROXY_HOST}:${PROXY_PORT}"
SSE_TARGET="http://${SSE_HOST}:${SSE_PORT}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
BIFROST_DATA_DIR=""
BIFROST_PID=""
SSE_SERVER_PID=""
BIFROST_LOG_FILE=""
VALID_TRAFFIC_ID=""

cleanup() {
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi

    safe_cleanup_proxy "$BIFROST_PID"

    if [[ -n "$SSE_SERVER_PID" ]] && kill -0 "$SSE_SERVER_PID" 2>/dev/null; then
        kill_pid "$SSE_SERVER_PID"
        wait_pid "$SSE_SERVER_PID"
    fi

    if [[ -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR"
    fi

    if [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]]; then
        rm -f "$BIFROST_LOG_FILE"
    fi
}

trap cleanup EXIT

wait_for_admin_ready() {
    local timeout="${1:-60}"
    local waited=0
    while [[ "$waited" -lt "$timeout" ]]; do
        if curl -sf "${ADMIN_BASE_URL}/api/system" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$BIFROST_PID" ]] && ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    return 1
}

start_sse_server() {
    python3 "${PROJECT_DIR}/e2e-tests/mock_servers/sse_echo_server.py" --port "$SSE_PORT" \
        > /dev/null 2>&1 &
    SSE_SERVER_PID=$!

    local waited=0
    while [[ "$waited" -lt 50 ]]; do
        if curl -sf "${SSE_TARGET}/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done

    _log_fail "SSE mock server started" "healthy server" "health check failed"
    return 1
}

start_bifrost() {
    BIFROST_DATA_DIR="$(mktemp -d "${PROJECT_DIR}/.bifrost-e2e-openai-search.XXXXXX")"
    BIFROST_LOG_FILE="$(mktemp)"
    export BIFROST_DATA_DIR

    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl \
        >"$BIFROST_LOG_FILE" 2>&1 &
    BIFROST_PID=$!

    if wait_for_admin_ready 60; then
        _log_pass "Bifrost admin ready"
        return 0
    fi

    _log_fail "Bifrost admin ready" "server listening" "startup failed"
    tail -100 "$BIFROST_LOG_FILE" || true
    return 1
}

wait_for_traffic_id_by_url() {
    local url_pattern="$1"
    local timeout="${2:-10}"
    local waited=0
    while [[ "$waited" -lt $((timeout * 10)) ]]; do
        local traffic_id
        traffic_id=$(curl -sf "${ADMIN_BASE_URL}/api/traffic?limit=50" | jq -r --arg pattern "$url_pattern" '
            [.records[]
             | select((.url // .p // .path // "") | contains($pattern))]
            | sort_by(.sequence)
            | last
            | .id // empty
        ')
        if [[ -n "$traffic_id" ]]; then
            echo "$traffic_id"
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

post_response_body_search() {
    local keyword="$1"
    curl -sf -X POST \
        -H "Content-Type: application/json" \
        -d "$(jq -nc --arg keyword "$keyword" '{
            keyword: $keyword,
            scope: {
                all: false,
                url: false,
                request_headers: false,
                response_headers: false,
                request_body: false,
                response_body: true
            },
            filters: {},
            limit: 20
        }')" \
        "${ADMIN_BASE_URL}/api/search"
}

wait_for_search_hit() {
    local keyword="$1"
    local expected_id="$2"
    local timeout="${3:-10}"
    local waited=0
    while [[ "$waited" -lt $((timeout * 10)) ]]; do
        local response
        response="$(post_response_body_search "$keyword")" || return 1
        local total
        total="$(echo "$response" | jq -r '.total_matched // (.results | length)')"
        local contains_expected
        contains_expected="$(echo "$response" | jq -r --arg id "$expected_id" '
            [.results[].record.id == $id] | any
        ')"
        if [[ "$total" -ge 1 && "$contains_expected" == "true" ]]; then
            echo "$response"
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

wait_for_search_empty() {
    local keyword="$1"
    local timeout="${2:-10}"
    local waited=0
    while [[ "$waited" -lt $((timeout * 10)) ]]; do
        local response
        response="$(post_response_body_search "$keyword")" || return 1
        local total
        total="$(echo "$response" | jq -r '.total_matched // (.results | length)')"
        if [[ "$total" == "0" ]]; then
            echo "$response"
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

fetch_sse_via_proxy() {
    local path="$1"
    curl --max-time 10 -sfN -x "$SSE_PROXY" "${SSE_TARGET}${path}" >/dev/null
}

test_search_api_and_cli_use_derived_sse_body() {
    fetch_sse_via_proxy "/sse/openai"

    local traffic_id
    traffic_id="$(wait_for_traffic_id_by_url "/sse/openai" 10)" || {
        _log_fail "traffic captured for OpenAI-like SSE" "captured traffic id" "not found"
        return 1
    }
    VALID_TRAFFIC_ID="$traffic_id"
    _log_pass "traffic captured for OpenAI-like SSE"

    local content_response
    content_response="$(wait_for_search_hit "searchable-content" "$traffic_id" 10)" || {
        _log_fail "response body search hits assembled content" "matched result" "no match"
        return 1
    }
    assert_json_field '.results[0].matches[0].field' "response_body" "$content_response" \
        "search API returns response_body match"

    local reasoning_response
    reasoning_response="$(wait_for_search_hit "reasoning-e2e" "$traffic_id" 10)" || {
        _log_fail "response body search hits assembled reasoning content" "matched result" "no match"
        return 1
    }
    assert_body_contains "\"record\"" "$reasoning_response" \
        "search API returns record payload for reasoning hit"

    local cli_output
    cli_output="$("$BIFROST_BIN" -p "$PROXY_PORT" search "reasoning-e2e" --res-body --format json --no-color)" || {
        _log_fail "CLI search succeeds" "exit code 0" "command failed"
        return 1
    }

    local cli_contains_expected
    cli_contains_expected="$(echo "$cli_output" | jq -r --arg id "$traffic_id" '
        [.results[].id == $id] | any
    ')"
    if [[ "$cli_contains_expected" == "true" ]]; then
        _log_pass "CLI search reuses derived response body search"
    else
        _log_fail "CLI search reuses derived response body search" "record ${traffic_id}" "$cli_output"
        return 1
    fi
}

test_invalid_openai_like_sse_falls_back_safely() {
    fetch_sse_via_proxy "/sse/openai-invalid"

    local traffic_id
    traffic_id="$(wait_for_traffic_id_by_url "/sse/openai-invalid" 10)" || {
        _log_fail "traffic captured for invalid OpenAI-like SSE" "captured traffic id" "not found"
        return 1
    }
    _log_pass "traffic captured for invalid OpenAI-like SSE"

    local system_status
    system_status="$(curl -s -o /dev/null -w "%{http_code}" "${ADMIN_BASE_URL}/api/system")"
    assert_status 200 "$system_status" "admin server stays healthy after invalid SSE"

    local invalid_response
    invalid_response="$(wait_for_search_empty "zqxinvalid-merge" 5)" || {
        _log_fail "invalid OpenAI-like SSE does not produce assembled search hit" "0 matches" "unexpected match"
        return 1
    }
    assert_json_field ".total_matched" "0" "$invalid_response" \
        "invalid OpenAI-like SSE falls back to empty derived content"

    local valid_response
    valid_response="$(wait_for_search_hit "searchable-content" "$VALID_TRAFFIC_ID" 5)" || {
        _log_fail "search still works after invalid SSE" "previous valid result" "search failed"
        return 1
    }
    assert_body_contains "\"results\"" "$valid_response" \
        "search continues to return results after invalid SSE"
}

main() {
    start_sse_server || exit 1
    start_bifrost || exit 1

    test_search_api_and_cli_use_derived_sse_body || exit 1
    test_invalid_openai_like_sse_falls_back_safely || exit 1

    print_test_summary || exit 1
}

main "$@"
