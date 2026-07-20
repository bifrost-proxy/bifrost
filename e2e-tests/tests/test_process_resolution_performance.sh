#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

allocate_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

PROXY_PORT="${PROXY_PORT:-$(allocate_port)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-$(allocate_port)}"
REQUEST_COUNT="${REQUEST_COUNT:-1000}"
CONCURRENCY="${CONCURRENCY:-16}"
CACHE_STRESS_COUNT="${CACHE_STRESS_COUNT:-0}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DATA_DIR=""
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$PROXY_PID"
    kill_pid "$UPSTREAM_PID"
    if [[ -n "$UPSTREAM_PID" ]]; then
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "$BIFROST_BIN" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    (cd "$PROJECT_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
}

wait_admin_ready() {
    local attempts=0
    while [[ "$attempts" -lt 120 ]]; do
        if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/diagnostics/process-resolver" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            break
        fi
        sleep 0.5
        attempts=$((attempts + 1))
    done

    echo "Bifrost did not become ready. Log:" >&2
    sed -n '1,240p' "${TEST_DATA_DIR}/proxy.log" >&2 2>/dev/null || true
    return 1
}

wait_upstream_ready() {
    local attempts=0
    while [[ "$attempts" -lt 120 ]]; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --connect-timeout 1 --max-time 3 \
            "http://127.0.0.1:${UPSTREAM_PORT}/" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$UPSTREAM_PID" ]] && ! kill -0 "$UPSTREAM_PID" 2>/dev/null; then
            break
        fi
        sleep 0.25
        attempts=$((attempts + 1))
    done

    echo "Upstream did not become ready. Log:" >&2
    sed -n '1,160p' "${TEST_DATA_DIR}/upstream.log" >&2 2>/dev/null || true
    return 1
}

start_fixture() {
    TEST_DATA_DIR="$(mktemp -d)"
    mark_e2e_data_root "$TEST_DATA_DIR"

    python3 -m http.server "$UPSTREAM_PORT" --bind 127.0.0.1 \
        --directory "$TEST_DATA_DIR" >"${TEST_DATA_DIR}/upstream.log" 2>&1 &
    UPSTREAM_PID=$!
    wait_upstream_ready

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start \
        -y \
        --access-mode allow_all \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    wait_admin_ready
}

read_diagnostics() {
    curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/diagnostics/process-resolver"
}

main() {
    build_bifrost
    start_fixture

    local before after
    before="$(read_diagnostics)"
    jq -e '
        .lookup_requests_total >= 0 and
        .positive_cache_hits_total >= 0 and
        .negative_cache_hits_total >= 0 and
        .connection_cache_entries >= 0 and
        .connection_cache_peak_entries >= 0 and
        .connection_cache_evictions_total >= 0 and
        .connection_cache_expired_total >= 0 and
        .pid_cache_entries >= 0 and
        .pid_cache_peak_entries >= 0 and
        .pid_cache_evictions_total >= 0 and
        .pid_cache_expired_total >= 0 and
        .snapshot_hits_total >= 0 and
        .snapshot_misses_total >= 0 and
        .snapshot_refreshes_total >= 0 and
        .snapshot_refresh_failures_total >= 0 and
        .scan_duration_total_us >= 0 and
        .scan_duration_max_us >= 0 and
        .scanned_pids_total >= 0 and
        .scanned_fds_total >= 0 and
        .resolved_total >= 0 and
        .unresolved_total >= 0
    ' <<<"$before" >/dev/null

    seq "$REQUEST_COUNT" | xargs -P "$CONCURRENCY" -I{} \
        curl -fsS -o /dev/null \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"

    after="$(read_diagnostics)"
    if [[ "$(jq -r '.lookup_requests_total' <<<"$before")" != "$(jq -r '.lookup_requests_total' <<<"$after")" ]]; then
        echo "Admin requests unexpectedly triggered client-process lookups" >&2
        echo "before=${before}" >&2
        echo "after=${after}" >&2
        exit 1
    fi
    if [[ "$(jq -r '.snapshot_refreshes_total' <<<"$before")" != "$(jq -r '.snapshot_refreshes_total' <<<"$after")" ]]; then
        echo "Admin requests unexpectedly triggered socket snapshot refreshes" >&2
        echo "before=${before}" >&2
        echo "after=${after}" >&2
        exit 1
    fi

    local external_status after_external_proxy
    external_status="$(curl --noproxy "" -sS -o /dev/null -w '%{http_code}' \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/_bifrost/api/proxy/address")"
    if [[ "$external_status" != "404" ]]; then
        echo "External upstream path resembling Admin API was misrouted: status=${external_status}" >&2
        exit 1
    fi
    after_external_proxy="$(read_diagnostics)"
    if [[ "$(jq -r '.lookup_requests_total' <<<"$after_external_proxy")" -le "$(jq -r '.lookup_requests_total' <<<"$after")" ]]; then
        echo "External Admin-like upstream path unexpectedly skipped client-process lookup" >&2
        echo "before_external=${after}" >&2
        echo "after_external=${after_external_proxy}" >&2
        exit 1
    fi

    curl --noproxy "" -fsS -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/" >/dev/null

    local socks_status
    socks_status="$(curl --noproxy "" -sS -o /dev/null -w '%{http_code}' \
        --socks5-hostname "127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/?transport=socks5")"
    if [[ "$socks_status" != "200" ]]; then
        echo "SOCKS5 process-attribution path failed to forward: status=${socks_status}" >&2
        exit 1
    fi

    local before_burst after_burst lookup_delta refresh_delta burst_count
    burst_count=128
    before_burst="$(read_diagnostics)"
    seq "$burst_count" | xargs -P "$CONCURRENCY" -I{} \
        curl --noproxy "" -fsS -o /dev/null \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/?burst={}"
    after_burst="$(read_diagnostics)"
    lookup_delta=$((
        $(jq -r '.lookup_requests_total' <<<"$after_burst") -
        $(jq -r '.lookup_requests_total' <<<"$before_burst")
    ))
    refresh_delta=$((
        $(jq -r '.snapshot_refreshes_total' <<<"$after_burst") -
        $(jq -r '.snapshot_refreshes_total' <<<"$before_burst")
    ))
    if [[ "$lookup_delta" -le 0 ]]; then
        echo "Concurrent ordinary proxy requests unexpectedly skipped process lookup" >&2
        exit 1
    fi
    if [[ "$refresh_delta" -ge "$burst_count" ]]; then
        echo "Snapshot generations were not shared across the concurrent burst" >&2
        echo "lookups=${lookup_delta} refreshes=${refresh_delta} requests=${burst_count}" >&2
        exit 1
    fi

    local stress_summary="disabled"
    if [[ "$CACHE_STRESS_COUNT" -gt 0 ]]; then
        local before_stress after_stress evictions_delta expired_delta peak_entries current_entries
        before_stress="$(read_diagnostics)"
        seq "$CACHE_STRESS_COUNT" | xargs -P "$CONCURRENCY" -I{} \
            curl --noproxy "" -fsS -o /dev/null \
            -x "http://127.0.0.1:${PROXY_PORT}" \
            "http://127.0.0.1:${UPSTREAM_PORT}/?cache-stress={}"
        after_stress="$(read_diagnostics)"
        evictions_delta=$((
            $(jq -r '.connection_cache_evictions_total' <<<"$after_stress") -
            $(jq -r '.connection_cache_evictions_total' <<<"$before_stress")
        ))
        expired_delta=$((
            $(jq -r '.connection_cache_expired_total' <<<"$after_stress") -
            $(jq -r '.connection_cache_expired_total' <<<"$before_stress")
        ))
        peak_entries="$(jq -r '.connection_cache_peak_entries' <<<"$after_stress")"
        current_entries="$(jq -r '.connection_cache_entries' <<<"$after_stress")"
        if [[ "$peak_entries" -gt 16384 || "$current_entries" -gt 16384 ]]; then
            echo "Connection cache exceeded its hard capacity" >&2
            echo "after_stress=${after_stress}" >&2
            exit 1
        fi
        if [[ "$CACHE_STRESS_COUNT" -gt 16384 && "$evictions_delta" -le 0 && "$expired_delta" -le 0 ]]; then
            echo "High-cardinality stress did not exercise TTL expiry or hard-cap eviction" >&2
            echo "before_stress=${before_stress}" >&2
            echo "after_stress=${after_stress}" >&2
            exit 1
        fi
        stress_summary="requests=${CACHE_STRESS_COUNT} evictions=${evictions_delta} expired=${expired_delta} peak=${peak_entries} current=${current_entries}"
    fi

    local metrics
    metrics="$(curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/metrics")"
    if jq -e '
        has("snapshot_refreshes_total") or
        has("scan_duration_total_us") or
        has("scanned_pids_total") or
        has("scanned_fds_total")
    ' <<<"$metrics" >/dev/null; then
        echo "Detailed process-resolution diagnostics leaked into main metrics" >&2
        echo "$metrics" >&2
        exit 1
    fi

    echo "Process-resolution performance E2E passed: admin_requests=${REQUEST_COUNT} concurrency=${CONCURRENCY}"
    echo "before=${before}"
    echo "after=${after}"
    echo "after_external_proxy=${after_external_proxy}"
    echo "burst_lookups=${lookup_delta} burst_snapshot_refreshes=${refresh_delta} burst_requests=${burst_count}"
    echo "socks_status=${socks_status}"
    echo "cache_stress=${stress_summary}"
}

main "$@"
