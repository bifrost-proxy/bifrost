#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
    (cd "$PROJECT_ROOT" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
elif [[ ! -x "$BIFROST_BIN" ]]; then
    echo "BIFROST_BIN is not executable: $BIFROST_BIN" >&2
    exit 1
fi

PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-$((19090 + ($$ % 1000)))}"
TARGET_PORT="${TARGET_PORT:-$((21090 + ($$ % 1000)))}"
DATA_DIR="${DATA_DIR:-${PROJECT_ROOT}/.bifrost-test-admin-virtual-host-$$}"
ADMIN_BASE_URL="http://${PROXY_HOST}:${PROXY_PORT}/_bifrost"
PROXY_PID=""
TARGET_PID=""

export BIFROST_DATA_DIR="$DATA_DIR"

log_info() { echo "[INFO] $*"; }
log_fail() { echo "[FAIL] $*"; }

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [[ -n "${PROXY_PID:-}" ]]; then
        safe_cleanup_proxy "$PROXY_PID" || true
    fi
    if [[ -n "${TARGET_PID:-}" ]]; then
        kill_pid "$TARGET_PID" || true
        wait_pid "$TARGET_PID" || true
    fi
    rm -rf "$DATA_DIR"
    echo "Cleanup done"
}

trap cleanup EXIT

wait_for_admin_ready() {
    local waited=0
    while [[ $waited -lt 60 ]]; do
        if curl -sf --connect-timeout 2 --max-time 5 "${ADMIN_BASE_URL}/api/auth/status" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            log_fail "Proxy exited while waiting for admin readiness"
            tail -80 "$DATA_DIR/proxy.log" || true
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    log_fail "Admin API did not become ready"
    tail -80 "$DATA_DIR/proxy.log" || true
    return 1
}

wait_for_target_ready() {
    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf --connect-timeout 2 --max-time 5 "http://127.0.0.1:${TARGET_PORT}/ordinary-target" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    log_fail "Target HTTP server did not become ready"
    cat "$DATA_DIR/target.log" || true
    return 1
}

start_services() {
    mkdir -p "$DATA_DIR/target"
    printf 'ordinary-target-ok\n' > "$DATA_DIR/target/ordinary-target"

    log_info "Starting ordinary HTTP target on port $TARGET_PORT"
    (
        cd "$DATA_DIR/target"
        python3 -m http.server "$TARGET_PORT" --bind 127.0.0.1
    ) > "$DATA_DIR/target.log" 2>&1 &
    TARGET_PID=$!
    wait_for_target_ready

    log_info "Starting Bifrost on port $PROXY_PORT with data dir $DATA_DIR"
    RUST_LOG=info "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        start --host "$PROXY_HOST" --yes --unsafe-ssl --skip-cert-check --no-system-proxy \
        > "$DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    wait_for_admin_ready
}

assert_admin_html_via_proxy() {
    local url="$1"
    local body
    if ! body="$(env NO_PROXY="" no_proxy="" curl -fsS -k --compressed --connect-timeout 2 --max-time 10 \
        -x "http://${PROXY_HOST}:${PROXY_PORT}" \
        "$url")"; then
        log_fail "Admin virtual host request failed for $url"
        tail -120 "$DATA_DIR/proxy.log" || true
        return 1
    fi

    if [[ "$body" != *"Bifrost"* ]]; then
        log_fail "Expected admin HTML for $url"
        printf '%s\n' "$body" | head -20
        tail -80 "$DATA_DIR/proxy.log" || true
        return 1
    fi
    log_info "Admin virtual host returned Bifrost HTML for $url"
}

assert_direct_host_header_admin_html() {
    local body
    if ! body="$(curl -fsS --noproxy '*' --compressed --connect-timeout 2 --max-time 10 \
        -H "Host: bifrost.local" \
        "http://${PROXY_HOST}:${PROXY_PORT}/")"; then
        log_fail "Direct Host: bifrost.local request failed"
        tail -120 "$DATA_DIR/proxy.log" || true
        return 1
    fi

    if [[ "$body" != *"Bifrost"* ]]; then
        log_fail "Expected direct Host: bifrost.local admin HTML"
        printf '%s\n' "$body" | head -20
        tail -80 "$DATA_DIR/proxy.log" || true
        return 1
    fi
    log_info "Direct Host: bifrost.local returned Bifrost HTML"
}

assert_default_system_proxy_bypass_keeps_virtual_host_routable() {
    local status
    if ! status="$(curl -fsS --connect-timeout 2 --max-time 10 "${ADMIN_BASE_URL}/api/proxy/system")"; then
        log_fail "Failed to load system proxy status"
        tail -120 "$DATA_DIR/proxy.log" || true
        return 1
    fi

    if [[ "$status" == *"*.local"* ]]; then
        log_fail "Default system proxy bypass must not include *.local: $status"
        return 1
    fi
    if [[ "$status" != *"localhost,127.0.0.1,::1"* ]]; then
        log_fail "Default system proxy bypass did not include loopback entries: $status"
        return 1
    fi
    log_info "Default system proxy bypass keeps bifrost.local proxy-routable"
}

assert_ordinary_proxy_target_still_works() {
    local body
    if ! body="$(env NO_PROXY="" no_proxy="" curl -fsS --connect-timeout 2 --max-time 10 \
        -x "http://${PROXY_HOST}:${PROXY_PORT}" \
        "http://127.0.0.1:${TARGET_PORT}/ordinary-target")"; then
        log_fail "Ordinary proxy target request failed"
        tail -120 "$DATA_DIR/proxy.log" || true
        return 1
    fi

    if [[ "$body" != "ordinary-target-ok" ]]; then
        log_fail "Expected ordinary proxy target body, got: $body"
        tail -80 "$DATA_DIR/proxy.log" || true
        return 1
    fi
    log_info "Ordinary proxy target still routes through proxy"
}

main() {
    start_services

    assert_admin_html_via_proxy "http://bifrost.local/"
    assert_admin_html_via_proxy "https://bifrost.local/"
    assert_admin_html_via_proxy "http://bifrost.local:${PROXY_PORT}/"
    assert_direct_host_header_admin_html
    assert_default_system_proxy_bypass_keeps_virtual_host_routable
    assert_ordinary_proxy_target_still_works

    log_info "Admin virtual host proxy E2E passed"
}

main "$@"
