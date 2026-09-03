#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUN_ROOT="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-client-admin.XXXXXX")"
export BIFROST_DATA_DIR="${RUN_ROOT}/bootstrap"

# Shared helper installs the mandatory tray/login-page guards and provides
# dynamic ports plus PID-scoped cleanup.
source "${ROOT_DIR}/e2e-tests/test_utils/process.sh"

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required command: $1" >&2
        exit 1
    }
}

require_command curl
require_command jq
require_command python3

BIFROST_BIN="${BIFROST_BIN:-${ROOT_DIR}/target/release/bifrost}"
TARGET_DATA_DIR="${RUN_ROOT}/target"
CALLER_DATA_DIR="${RUN_ROOT}/caller"
TARGET_LOG="${RUN_ROOT}/target.log"
ECHO_LOG="${RUN_ROOT}/echo.log"
CAPTURE_OUTPUT="${RUN_ROOT}/capture.json"
TARGET_PID=""
ECHO_PID=""
CAPTURE_PID=""
TARGET_PORT="$(allocate_free_port)"
ECHO_PORT="$(allocate_free_port)"
ADMIN_PASSWORD="client-e2e-pass-${TARGET_PORT}"
RULE_NAME="client_admin_e2e_${TARGET_PORT}"
VALUE_NAME="CLIENT_ADMIN_E2E_${TARGET_PORT}"
SCRIPT_NAME="client_admin_e2e_${TARGET_PORT}"
ACCOUNT_NAME="client-e2e-${TARGET_PORT}"

pass_count=0

pass() {
    echo "[PASS] $*"
    pass_count=$((pass_count + 1))
}

fail() {
    echo "[FAIL] $*" >&2
    return 1
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local message="$3"
    if grep -Fq -- "$needle" <<<"$haystack"; then
        pass "$message"
    else
        echo "output: ${haystack}" >&2
        fail "${message}: missing '${needle}'"
    fi
}

assert_json() {
    local body="$1"
    local expression="$2"
    local message="$3"
    if jq -e "$expression" >/dev/null 2>&1 <<<"$body"; then
        pass "$message"
    else
        echo "body: ${body}" >&2
        fail "${message}: jq expression '${expression}' failed"
    fi
}

get_non_loopback_ip() {
    local ip=""
    ip="$(ip route get 1.1.1.1 2>/dev/null | awk '{for (i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}' || true)"
    if [[ -z "$ip" ]] && command -v route >/dev/null 2>&1 && command -v ipconfig >/dev/null 2>&1; then
        local interface=""
        interface="$(route -n get default 2>/dev/null | awk '/interface:/ {print $2; exit}' || true)"
        [[ -z "$interface" ]] || ip="$(ipconfig getifaddr "$interface" 2>/dev/null || true)"
    fi
    if [[ -z "$ip" ]]; then
        ip="$(hostname -I 2>/dev/null | awk '{print $1}' || true)"
    fi
    if [[ -z "$ip" ]]; then
        ip="$(python3 - <<'PY'
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    sock.connect(("1.1.1.1", 80))
    candidate = sock.getsockname()[0]
    if not candidate.startswith("127."):
        print(candidate)
finally:
    sock.close()
PY
        )"
    fi
    [[ -n "$ip" && "$ip" != 127.* ]] || return 1
    printf '%s' "$ip"
}

cleanup() {
    [[ -z "$CAPTURE_PID" ]] || terminate_process_tree "$CAPTURE_PID" || true
    [[ -z "$ECHO_PID" ]] || terminate_process_tree "$ECHO_PID" || true
    [[ -z "$TARGET_PID" ]] || terminate_process_tree "$TARGET_PID" 2 || true
    local attempt
    for attempt in 1 2 3 4 5; do
        rm -rf "$RUN_ROOT" 2>/dev/null || true
        [[ -e "$RUN_ROOT" ]] || break
        sleep 0.2
    done
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
    (cd "$ROOT_DIR" && cargo build --release --bin bifrost)
fi
[[ -x "$BIFROST_BIN" ]] || { echo "missing bifrost binary: $BIFROST_BIN" >&2; exit 1; }

mkdir -p "$TARGET_DATA_DIR" "$CALLER_DATA_DIR"
mark_e2e_data_root "$RUN_ROOT"

LAN_IP="$(get_non_loopback_ip)" || {
    echo "no non-loopback IP is available; Client LAN E2E requires a real interface" >&2
    exit 1
}
TARGET_URL="http://${LAN_IP}:${TARGET_PORT}"

python3 "${ROOT_DIR}/e2e-tests/mock_servers/http_echo_server.py" "$ECHO_PORT" >"$ECHO_LOG" 2>&1 &
ECHO_PID=$!
wait_for_http_ready "http://127.0.0.1:${ECHO_PORT}/health" 30 0.2 || {
    tail -100 "$ECHO_LOG" >&2 || true
    exit 1
}

BIFROST_DATA_DIR="$TARGET_DATA_DIR" \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    SKIP_FRONTEND_BUILD=1 \
    "$BIFROST_BIN" -H 0.0.0.0 -p "$TARGET_PORT" start \
        --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"$TARGET_LOG" 2>&1 &
TARGET_PID=$!
wait_for_http_ready "http://127.0.0.1:${TARGET_PORT}/_bifrost/api/auth/status" 60 0.2 || {
    tail -200 "$TARGET_LOG" >&2 || true
    exit 1
}

printf '%s\n' "$ADMIN_PASSWORD" | BIFROST_DATA_DIR="$TARGET_DATA_DIR" \
    "$BIFROST_BIN" admin passwd --password-stdin >/dev/null

client() {
    env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY -u http_proxy -u https_proxy -u all_proxy \
        BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" client "$@"
}

client target add lan --url "$TARGET_URL" --allow-insecure-http >/dev/null
set +e
remote_disabled_output="$(printf '%s\n' "$ADMIN_PASSWORD" | client target login lan --password-stdin 2>&1)"
remote_disabled_rc=$?
set -e
[[ "$remote_disabled_rc" -ne 0 ]] || fail "login unexpectedly succeeded before Admin Remote Access was enabled"
assert_contains "$remote_disabled_output" "remote Admin access is disabled" "Client cannot bootstrap disabled Admin Remote Access"

BIFROST_DATA_DIR="$TARGET_DATA_DIR" "$BIFROST_BIN" admin remote enable >/dev/null
set +e
wrong_password_output="$(printf '%s\n' wrong-password | client target login lan --password-stdin 2>&1)"
wrong_password_rc=$?
set -e
[[ "$wrong_password_rc" -ne 0 ]] || fail "login unexpectedly accepted a wrong password"
assert_contains "$wrong_password_output" "Invalid credentials" "wrong Admin password is rejected"

printf '%s\n' "$ADMIN_PASSWORD" | client target login lan --password-stdin >/dev/null
pass "saved target login over a non-loopback LAN address"
assert_contains "$(client target list)" "lan" "target list shows the saved Client target"
assert_json "$(client target show lan)" '.name == "lan" and .logged_in == true' "target show reports saved login state without exposing the token"

status_json="$(client status --format json)"
assert_json "$status_json" ".server.port == ${TARGET_PORT}" "single saved target is selected automatically"

[[ -f "${CALLER_DATA_DIR}/cli/admin-targets.toml" ]] || fail "target profile was not persisted"
[[ -f "${CALLER_DATA_DIR}/cli/admin-credentials.toml" ]] || fail "credential file was not persisted"
if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* && "$(uname -s)" != CYGWIN* ]]; then
    [[ "$(stat -f '%Lp' "${CALLER_DATA_DIR}/cli/admin-credentials.toml" 2>/dev/null || stat -c '%a' "${CALLER_DATA_DIR}/cli/admin-credentials.toml")" == "600" ]] \
        || fail "credential file permissions are not 0600"
    pass "saved JWT uses a separate 0600 credential file"
fi

client target add duplicate --url "$TARGET_URL" --allow-insecure-http >/dev/null
set +e
multi_output="$(client status --format json 2>&1)"
multi_rc=$?
set -e
[[ "$multi_rc" -ne 0 ]] || fail "multiple targets without --target unexpectedly succeeded"
assert_contains "$multi_output" "multiple client targets configured" "non-TTY multi-target selection requires --target"
explicit_json="$(client --target lan status --format json)"
assert_json "$explicit_json" ".server.port == ${TARGET_PORT}" "explicit saved target selection works"
env_json="$(BIFROST_CLIENT_TARGET=lan client status --format json)"
assert_json "$env_json" ".server.port == ${TARGET_PORT}" "BIFROST_CLIENT_TARGET selects a saved target"
client target rename duplicate secondary >/dev/null
assert_contains "$(client target list)" "secondary" "saved target rename preserves the profile"
client target remove secondary >/dev/null

client rule add "$RULE_NAME" -c "client-e2e.test host://127.0.0.1:${ECHO_PORT}" >/dev/null
assert_contains "$(client rule show "$RULE_NAME")" "client-e2e.test" "remote rule CRUD uses Admin API"
client rule enable "$RULE_NAME" >/dev/null

client value add "$VALUE_NAME" one >/dev/null
client value update "$VALUE_NAME" two >/dev/null
assert_contains "$(client value show "$VALUE_NAME")" "two" "remote value CRUD uses Admin API"

client script add request "$SCRIPT_NAME" -c 'log.info("client-e2e");' >/dev/null
assert_contains "$(client script show request "$SCRIPT_NAME")" "client-e2e" "remote script CRUD uses Admin API"

client whitelist add 192.0.2.10 >/dev/null
assert_contains "$(client whitelist list)" "192.0.2.10" "remote whitelist mutation uses Admin API"
client whitelist remove 192.0.2.10 >/dev/null

printf '%s\n' 'proxy-account-pass-123' | client account add "$ACCOUNT_NAME" --password-stdin >/dev/null
assert_contains "$(client account list --json)" "$ACCOUNT_NAME" "remote proxy account mutation uses Admin API"
client account remove "$ACCOUNT_NAME" >/dev/null

[[ "$(client config get tls.enabled --json)" == "false" ]] \
    || fail "remote runtime config query returned an unexpected value"
pass "remote runtime config query works"
assert_contains "$(client metrics summary)" "Bifrost Metrics Summary" "remote metrics query works"
assert_contains "$(client sync status)" "Sync Status" "remote sync status query works"

port_output="$(client port bind --port 0 --rule "$RULE_NAME")"
TEMP_PORT="$(sed -n 's/^Temporary port: .*:\([0-9][0-9]*\)$/\1/p' <<<"$port_output" | head -1)"
[[ "$TEMP_PORT" =~ ^[0-9]+$ ]] || fail "failed to parse temporary port: ${port_output}"
assert_contains "$(client port active "$TEMP_PORT")" "$RULE_NAME" "remote temporary port active rules work"
client port destroy "$TEMP_PORT" >/dev/null
pass "remote temporary port lifecycle works"

env NO_PROXY= no_proxy= curl -fsS --proxy "http://127.0.0.1:${TARGET_PORT}" \
    "http://client-e2e.test/traffic-marker-${TARGET_PORT}" >/dev/null
traffic_json="$(client traffic list --host client-e2e.test --format json)"
assert_json "$traffic_json" '.records | length >= 1' "remote traffic list returns target records"
search_json="$(client search "traffic-marker-${TARGET_PORT}" --format json)"
assert_json "$search_json" '.results | length >= 1' "remote SSE search returns target records"

client capture wait --host client-e2e.test --path capture-marker --timeout 10s --format json \
    >"$CAPTURE_OUTPUT" 2>"${RUN_ROOT}/capture.err" &
CAPTURE_PID=$!
sleep 0.5
env NO_PROXY= no_proxy= curl -fsS --proxy "http://127.0.0.1:${TARGET_PORT}" \
    "http://client-e2e.test/capture-marker" >/dev/null
wait "$CAPTURE_PID"
CAPTURE_PID=""
assert_json "$(cat "$CAPTURE_OUTPUT")" '.matched == true' "remote capture stream receives authenticated traffic"

set +e
local_only_output="$(client start 2>&1)"
local_only_rc=$?
remote_output="$(client remote conn status 2>&1)"
remote_rc=$?
set -e
[[ "$local_only_rc" -ne 0 ]] || fail "Client start unexpectedly executed"
assert_contains "$local_only_output" "service lifecycle commands require local access" "local-only lifecycle is rejected before dispatch"
[[ "$remote_rc" -ne 0 ]] || fail "nested Remote Invoke unexpectedly executed"
assert_contains "$remote_output" "Remote Invoke is a separate transport" "Client and Remote Invoke remain separate"

client script delete request "$SCRIPT_NAME" >/dev/null
client value delete "$VALUE_NAME" >/dev/null
client rule delete "$RULE_NAME" >/dev/null

client admin audit --json >/dev/null
client admin revoke-all >/dev/null
set +e
revoked_output="$(client status --format json 2>&1)"
revoked_rc=$?
set -e
[[ "$revoked_rc" -ne 0 ]] || fail "revoked Client token unexpectedly remained valid"
assert_contains "$revoked_output" 'Run `bifrost client target login <target>` again' "401 reports an actionable Client login error"
printf '%s\n' "$ADMIN_PASSWORD" | client target login lan --password-stdin >/dev/null
client target logout lan >/dev/null
set +e
logout_output="$(client status --format json 2>&1)"
logout_rc=$?
set -e
[[ "$logout_rc" -ne 0 ]] || fail "logged-out Client target unexpectedly remained usable"
assert_contains "$logout_output" "is not logged in" "logout removes only the caller's saved session"

echo "Client Admin CLI E2E passed (${pass_count} assertions) via ${TARGET_URL}"
