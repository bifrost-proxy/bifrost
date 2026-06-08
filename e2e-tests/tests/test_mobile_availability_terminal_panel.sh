#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-mobile-terminal.XXXXXX")"
BIFROST_PID=""

cleanup() {
    if [[ -n "$BIFROST_PID" ]] && kill -0 "$BIFROST_PID" 2>/dev/null; then
        kill "$BIFROST_PID" 2>/dev/null || true
        wait "$BIFROST_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

unused_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

wait_http() {
    local url="$1"
    for _ in {1..80}; do
        if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_log_contains() {
    local file="$1"
    local pattern="$2"
    for _ in {1..80}; do
        if [[ -f "$file" ]] && grep -q "$pattern" "$file"; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

if [[ "${SKIP_BUILD:-false}" != "true" || ! -x "$BIFROST_BIN" ]]; then
    cargo build --bin bifrost >/dev/null
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi

PORT="$(unused_port)"
DATA_DIR="${TEST_DIR}/data"
LOG_FILE="${TEST_DIR}/startup.log"
mkdir -p "$DATA_DIR"

_log_info "case: foreground start prints mobile availability panel by default"
BIFROST_DATA_DIR="$DATA_DIR" \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_MOBILE_AVAILABILITY_PANEL_FORCE=1 \
"$BIFROST_BIN" start -p "$PORT" --unsafe-ssl --skip-cert-check --no-system-proxy \
    >"$LOG_FILE" 2>&1 &
BIFROST_PID="$!"

if ! wait_http "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address"; then
    cat "$LOG_FILE" >&2 || true
    _log_fail "admin api ready" "reachable" "unreachable"
    exit 1
fi

if ! wait_log_contains "$LOG_FILE" "MOBILE AVAILABILITY CHECK"; then
    cat "$LOG_FILE" >&2 || true
    _log_fail "mobile availability panel shown" "MOBILE AVAILABILITY CHECK" "missing"
    exit 1
fi

if ! curl -fsS --noproxy '*' "http://127.0.0.1:${PORT}/_bifrost/tp" | grep -q "Bifrost Availability Check"; then
    cat "$LOG_FILE" >&2 || true
    _log_fail "short trust probe URL opens landing page" "Bifrost Availability Check" "missing"
    exit 1
fi
_log_pass "short trust probe URL opens landing page"

grep -q "Access Control:" "$LOG_FILE"
_log_pass "mobile panel includes Access Control section"

grep -q "shortcut: y = allow current, n = deny current" "$LOG_FILE"
_log_pass "mobile panel includes simple terminal approval shortcuts"

if grep -q "Target:        127.0.0.1" "$LOG_FILE"; then
    cat "$LOG_FILE" >&2 || true
    _log_fail "loopback is not shown as mobile target" "no 127.0.0.1 target" "found"
    exit 1
fi
_log_pass "loopback is not shown as mobile target"

if grep -Eiq "Demo|process check|chatgpt_web startup auth" "$LOG_FILE"; then
    cat "$LOG_FILE" >&2 || true
    _log_fail "startup console excludes unrelated process-check noise" "no noise" "found"
    exit 1
fi
_log_pass "startup console excludes unrelated process-check noise"

kill "$BIFROST_PID" 2>/dev/null || true
wait "$BIFROST_PID" 2>/dev/null || true
BIFROST_PID=""

echo "[mobile-availability-terminal] PASS"
