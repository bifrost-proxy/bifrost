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
UPDATED_ADMIN_PASSWORD="client-e2e-updated-${TARGET_PORT}"
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
assert_contains "$(client target --help)" "Manage Bifrost Client targets" "target management help is available"
assert_contains "$(client target list)" "lan" "target list works before login"
set +e
duplicate_target_output="$(client target add lan --url "$TARGET_URL" --allow-insecure-http 2>&1)"
duplicate_target_rc=$?
insecure_target_output="$(client target add unsafe --url "$TARGET_URL" 2>&1)"
insecure_target_rc=$?
invalid_target_output="$(client target add 'bad name' --url "$TARGET_URL" --allow-insecure-http 2>&1)"
invalid_target_rc=$?
set -e
[[ "$duplicate_target_rc" -ne 0 ]] || fail "duplicate target unexpectedly succeeded"
assert_contains "$duplicate_target_output" "Already exists" "duplicate target names are rejected"
[[ "$insecure_target_rc" -ne 0 ]] || fail "plain HTTP target without acknowledgement unexpectedly succeeded"
assert_contains "$insecure_target_output" "plain HTTP exposes" "plain HTTP requires explicit LAN acknowledgement"
[[ "$invalid_target_rc" -ne 0 ]] || fail "target name with whitespace unexpectedly succeeded"
assert_contains "$invalid_target_output" "contain no whitespace" "invalid target names are rejected"
set +e
noninteractive_login_output="$(client target login lan </dev/null 2>&1)"
noninteractive_login_rc=$?
target_with_selector_output="$(client --target lan target list 2>&1)"
target_with_selector_rc=$?
unknown_command_output="$(client definitely-not-a-command 2>&1)"
unknown_command_rc=$?
set -e
[[ "$noninteractive_login_rc" -ne 0 ]] || fail "non-interactive login without --password-stdin unexpectedly succeeded"
assert_contains "$noninteractive_login_output" "requires --password-stdin" "non-interactive login fails with an actionable error"
[[ "$target_with_selector_rc" -ne 0 ]] || fail "target management unexpectedly accepted --target"
assert_contains "$target_with_selector_output" "does not accept --target" "target management rejects business target selection"
[[ "$unknown_command_rc" -ne 0 ]] || fail "unknown Client command unexpectedly succeeded"
assert_contains "$unknown_command_output" "unrecognized subcommand" "Client reports original CLI parse errors"
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

ADMIN_TOKEN="$(sed -n 's/^token = "\([^"]*\)"/\1/p' "${CALLER_DATA_DIR}/cli/admin-credentials.toml")"
[[ -n "$ADMIN_TOKEN" ]] || fail "failed to read the test-only saved Admin token"
temporary_status_json="$(BIFROST_ADMIN_TOKEN="$ADMIN_TOKEN" client --target "$TARGET_URL" status --format json)"
assert_json "$temporary_status_json" ".server.port == ${TARGET_PORT}" "temporary URL target accepts only an explicit Admin token"

credential_backup="${RUN_ROOT}/admin-credentials.backup.toml"
cp "${CALLER_DATA_DIR}/cli/admin-credentials.toml" "$credential_backup"
sed 's/expires_at = "[^"]*"/expires_at = "2000-01-01T00:00:00Z"/' \
    "$credential_backup" >"${CALLER_DATA_DIR}/cli/admin-credentials.toml"
set +e
expired_session_output="$(client status --format json 2>&1)"
expired_session_rc=$?
set -e
[[ "$expired_session_rc" -ne 0 ]] || fail "expired Client session unexpectedly succeeded"
assert_contains "$expired_session_output" "has expired" "expired saved credentials fail closed"
cp "$credential_backup" "${CALLER_DATA_DIR}/cli/admin-credentials.toml"

status_json="$(client status --format json)"
assert_json "$status_json" ".server.port == ${TARGET_PORT}" "single saved target is selected automatically"
assert_contains "$(client status --format text)" "Status: Running" "remote status supports text output"
assert_json "$(client status --format json-pretty)" ".server.port == ${TARGET_PORT}" "remote status supports pretty JSON output"

[[ -f "${CALLER_DATA_DIR}/cli/admin-targets.toml" ]] || fail "target profile was not persisted"
[[ -f "${CALLER_DATA_DIR}/cli/admin-credentials.toml" ]] || fail "credential file was not persisted"
if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* && "$(uname -s)" != CYGWIN* ]]; then
    if [[ "$(uname -s)" == "Linux" ]]; then
        credential_mode="$(stat -c '%a' "${CALLER_DATA_DIR}/cli/admin-credentials.toml")"
    else
        credential_mode="$(stat -f '%Lp' "${CALLER_DATA_DIR}/cli/admin-credentials.toml")"
    fi
    [[ "$credential_mode" == "600" ]] \
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
set +e
rename_collision_output="$(client target rename secondary lan 2>&1)"
rename_collision_rc=$?
set -e
[[ "$rename_collision_rc" -ne 0 ]] || fail "target rename collision unexpectedly succeeded"
assert_contains "$rename_collision_output" "Already exists" "target rename rejects duplicate names"
set +e
invalid_rename_output="$(client target rename secondary 'bad name' 2>&1)"
invalid_rename_rc=$?
set -e
[[ "$invalid_rename_rc" -ne 0 ]] || fail "target rename with whitespace unexpectedly succeeded"
assert_contains "$invalid_rename_output" "contain no whitespace" "target rename validates the new name"
client target remove secondary >/dev/null

EMPTY_CALLER_DATA_DIR="${RUN_ROOT}/empty-caller"
MALFORMED_CALLER_DATA_DIR="${RUN_ROOT}/malformed-caller"
mkdir -p "${MALFORMED_CALLER_DATA_DIR}/cli"
printf '%s\n' 'this is not valid = [' >"${MALFORMED_CALLER_DATA_DIR}/cli/admin-targets.toml"
set +e
no_target_output="$(BIFROST_DATA_DIR="$EMPTY_CALLER_DATA_DIR" "$BIFROST_BIN" client status 2>&1)"
no_target_rc=$?
temporary_target_output="$(client --target "http://${LAN_IP}:${ECHO_PORT}" status 2>&1)"
temporary_target_rc=$?
malformed_store_output="$(BIFROST_DATA_DIR="$MALFORMED_CALLER_DATA_DIR" "$BIFROST_BIN" client target list 2>&1)"
malformed_store_rc=$?
set -e
[[ "$no_target_rc" -ne 0 ]] || fail "Client without configured targets unexpectedly succeeded"
assert_contains "$no_target_output" "no client targets configured" "Client reports an empty target store"
[[ "$temporary_target_rc" -ne 0 ]] || fail "temporary target without token unexpectedly succeeded"
assert_contains "$temporary_target_output" "temporary target requires" "temporary targets require an explicit token"
[[ "$malformed_store_rc" -ne 0 ]] || fail "malformed target store unexpectedly loaded"
assert_contains "$malformed_store_output" "failed to parse" "malformed target state fails closed"

client rule add "$RULE_NAME" -c "client-e2e.test host://127.0.0.1:${ECHO_PORT}" >/dev/null
assert_contains "$(client rule show "$RULE_NAME")" "client-e2e.test" "remote rule CRUD uses Admin API"
assert_contains "$(client rule list)" "$RULE_NAME" "remote rule list uses Admin API"
client rule update "$RULE_NAME" -c "client-e2e.test host://127.0.0.1:${ECHO_PORT}" >/dev/null
client rule disable "$RULE_NAME" >/dev/null
client rule enable "$RULE_NAME" >/dev/null
client rule rename "$RULE_NAME" "${RULE_NAME}_renamed" >/dev/null
RULE_NAME="${RULE_NAME}_renamed"
client rule reorder "$RULE_NAME" >/dev/null
assert_contains "$(client rule active)" "$RULE_NAME" "remote active-rule summary uses Admin API"
assert_contains "$(client rule share "$RULE_NAME" https://example.com)" "https://example.com" "remote rule sharing uses Admin API"
set +e
rule_sync_output="$(client rule sync 2>&1)"
rule_sync_rc=$?
set -e
[[ "$rule_sync_rc" -ne 0 ]] || fail "Client rule sync unexpectedly ran locally"
assert_contains "$rule_sync_output" "not supported in Client mode" "Client rule sync fails closed"

client value add "$VALUE_NAME" one >/dev/null
client value update "$VALUE_NAME" two >/dev/null
assert_contains "$(client value show "$VALUE_NAME")" "two" "remote value CRUD uses Admin API"
assert_contains "$(client value list)" "$VALUE_NAME" "remote value list uses Admin API"

client script add request "$SCRIPT_NAME" -c 'log.info("client-e2e");' >/dev/null
assert_contains "$(client script show request "$SCRIPT_NAME")" "client-e2e" "remote script CRUD uses Admin API"
assert_contains "$(client script list --type request)" "$SCRIPT_NAME" "remote script list uses Admin API"
assert_contains "$(client script list)" "$SCRIPT_NAME" "remote script list supports all script types"
assert_contains "$(client script list --type response)" "No scripts found" "remote script list reports an empty selected type"
set +e
script_missing_type_output="$(client script show "$SCRIPT_NAME" 2>&1)"
script_missing_type_rc=$?
set -e
[[ "$script_missing_type_rc" -ne 0 ]] || fail "Client script show without an explicit type unexpectedly succeeded"
assert_contains "$script_missing_type_output" "requires an explicit script type" "Client script lookup fails closed without a type"
client script update request "$SCRIPT_NAME" -c 'log.info("client-e2e-updated");' >/dev/null
client script rename request "$SCRIPT_NAME" "${SCRIPT_NAME}_renamed" >/dev/null
SCRIPT_NAME="${SCRIPT_NAME}_renamed"
set +e
script_run_output="$(client script run request "$SCRIPT_NAME" 2>&1)"
script_run_rc=$?
set -e
[[ "$script_run_rc" -ne 0 ]] || fail "Client script run unexpectedly ran locally"
assert_contains "$script_run_output" "not supported in Client mode" "Client script run fails closed"

client whitelist add 192.0.2.10 >/dev/null
assert_contains "$(client whitelist list)" "192.0.2.10" "remote whitelist mutation uses Admin API"
client whitelist remove 192.0.2.10 >/dev/null
assert_contains "$(client whitelist mode)" "allow_all" "remote access mode query uses Admin API"
client whitelist mode allow_all >/dev/null
client whitelist allow-lan true >/dev/null
set +e
invalid_allow_lan_output="$(client whitelist allow-lan definitely-not-a-bool 2>&1)"
invalid_allow_lan_rc=$?
set -e
[[ "$invalid_allow_lan_rc" -ne 0 ]] || fail "invalid allow-lan value unexpectedly succeeded"
assert_contains "$invalid_allow_lan_output" "invalid value" "remote allow-lan validates boolean input"
client whitelist pending >/dev/null
set +e
approve_missing_output="$(client whitelist approve 192.0.2.20 2>&1)"
approve_missing_rc=$?
reject_missing_output="$(client whitelist reject 192.0.2.21 2>&1)"
reject_missing_rc=$?
set -e
[[ "$approve_missing_rc" -ne 0 && "$reject_missing_rc" -ne 0 ]] \
    || fail "missing pending authorization unexpectedly succeeded"
assert_contains "$approve_missing_output" "not found" "remote pending approval reports missing entries"
assert_contains "$reject_missing_output" "not found" "remote pending rejection reports missing entries"
client whitelist clear-pending >/dev/null
client whitelist add-temporary 192.0.2.22 >/dev/null
client whitelist remove-temporary 192.0.2.22 >/dev/null
pass "remote whitelist control surface uses Admin API"

printf '%s\n' 'proxy-account-pass-123' | client account add "$ACCOUNT_NAME" --password-stdin >/dev/null
assert_contains "$(client account list --json)" "$ACCOUNT_NAME" "remote proxy account mutation uses Admin API"
client account remove "$ACCOUNT_NAME" >/dev/null

[[ "$(client config get tls.enabled --json)" == "false" ]] \
    || fail "remote runtime config query returned an unexpected value"
pass "remote runtime config query works"
assert_contains "$(client metrics summary)" "Bifrost Metrics Summary" "remote metrics query works"
assert_contains "$(client sync status)" "Sync Status" "remote sync status query works"
for missing_group_args in \
    "group list --keyword missing-client-group" \
    "group show missing-client-group" \
    "group rule list missing-client-group" \
    "group rule show missing-client-group missing-rule" \
    "group rule add missing-client-group missing-rule --content client-e2e.test" \
    "group rule update missing-client-group missing-rule --content client-e2e.test" \
    "group rule delete missing-client-group missing-rule" \
    "group rule enable missing-client-group missing-rule" \
    "group rule disable missing-client-group missing-rule"
do
    set +e
    missing_group_output="$(client $missing_group_args 2>&1)"
    missing_group_rc=$?
    set -e
    [[ "$missing_group_rc" -ne 0 ]] || fail "missing remote group operation unexpectedly succeeded: $missing_group_args"
    assert_contains "$missing_group_output" "HTTP" "remote group operation reaches the target Admin API: $missing_group_args"
done

port_output="$(client port bind --port 0 --rule "$RULE_NAME")"
TEMP_PORT="$(sed -n 's/^Temporary port: .*:\([0-9][0-9]*\)$/\1/p' <<<"$port_output" | head -1)"
[[ "$TEMP_PORT" =~ ^[0-9]+$ ]] || fail "failed to parse temporary port: ${port_output}"
assert_contains "$(client port active "$TEMP_PORT")" "$RULE_NAME" "remote temporary port active rules work"
assert_contains "$(client port list)" "$TEMP_PORT" "remote temporary port list works"
assert_contains "$(client port show "$TEMP_PORT")" "$TEMP_PORT" "remote temporary port show works"
client port update "$TEMP_PORT" --rule "$RULE_NAME" --name client-e2e >/dev/null
client port destroy "$TEMP_PORT" >/dev/null
pass "remote temporary port lifecycle works"

env NO_PROXY= no_proxy= curl -fsS --proxy "http://127.0.0.1:${TARGET_PORT}" \
    "http://client-e2e.test/traffic-marker-${TARGET_PORT}" >/dev/null
traffic_json="$(client traffic list --host client-e2e.test --format json)"
assert_json "$traffic_json" '.records | length >= 1' "remote traffic list returns target records"
TRAFFIC_ID="$(jq -r '.records[0].id' <<<"$traffic_json")"
TRAFFIC_SEQUENCE="$(jq -r '.records[0].seq' <<<"$traffic_json")"
assert_json "$(client traffic get "$TRAFFIC_ID" --format json)" '.id != null' "remote traffic detail uses Admin API"
assert_json "$(client traffic get "$TRAFFIC_ID" --request-body --response-body --format json)" '.id != null' "remote traffic body fetches use Admin API"
assert_json "$(client traffic get "$TRAFFIC_SEQUENCE" --format json)" '.id != null' "remote numeric traffic sequence resolves through Admin API"
assert_json "$(client traffic get --ids "$TRAFFIC_ID" --format json)" 'length >= 1' "remote traffic batch detail uses Admin API"
client traffic auth-status "$TRAFFIC_ID" --format json >/dev/null
assert_contains "$(client traffic export "$TRAFFIC_ID" --as curl)" "curl" "remote traffic export uses Admin API"
assert_json "$(client traffic replay "$TRAFFIC_ID" --format json)" '.status != null or .status_code != null or .success != null' "remote traffic replay uses Admin API"
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
for unsupported_args in \
    "status --tui" \
    "ca info" \
    "system-proxy status" \
    "setting grant list" \
    "ai voice sources" \
    "im provider list" \
    "agent run test"
do
    set +e
    unsupported_output="$(client $unsupported_args 2>&1)"
    unsupported_rc=$?
    set -e
    [[ "$unsupported_rc" -ne 0 ]] || fail "unsupported Client command unexpectedly succeeded: $unsupported_args"
done
pass "unsupported Client capability families fail closed"

client script delete request "$SCRIPT_NAME" >/dev/null
client value delete "$VALUE_NAME" >/dev/null
client rule delete "$RULE_NAME" >/dev/null

client admin remote status >/dev/null
set +e
remote_enable_output="$(client admin remote enable 2>&1)"
remote_enable_rc=$?
set -e
[[ "$remote_enable_rc" -ne 0 ]] || fail "Client unexpectedly enabled its own remote access bootstrap"
assert_contains "$remote_enable_output" "must be enabled locally" "Client cannot bootstrap target remote access"
assert_contains "$(client admin audit)" "Admin login audit" "remote audit text output renders login records"
assert_contains "$(client admin audit --offset 999999)" "No audit records" "remote audit empty page renders correctly"
client admin audit --json >/dev/null
printf '%s\n' "$UPDATED_ADMIN_PASSWORD" | client admin passwd --password-stdin >/dev/null
client admin revoke-all >/dev/null
set +e
revoked_output="$(client status --format json 2>&1)"
revoked_rc=$?
set -e
[[ "$revoked_rc" -ne 0 ]] || fail "revoked Client token unexpectedly remained valid"
assert_contains "$revoked_output" 'Run `bifrost client target login <target>` again' "401 reports an actionable Client login error"
# Revocation and JWT issued-at values use second precision. Cross the revoke
# second before obtaining the replacement token so it is unambiguously newer.
sleep 1.1
printf '%s\n' "$UPDATED_ADMIN_PASSWORD" | client target login lan --password-stdin >/dev/null
client admin remote disable >/dev/null
client target logout lan >/dev/null
set +e
logout_output="$(client status --format json 2>&1)"
logout_rc=$?
disabled_login_output="$(printf '%s\n' "$UPDATED_ADMIN_PASSWORD" | client target login lan --password-stdin 2>&1)"
disabled_login_rc=$?
set -e
[[ "$logout_rc" -ne 0 ]] || fail "logged-out Client target unexpectedly remained usable"
assert_contains "$logout_output" "is not logged in" "logout removes only the caller's saved session"
[[ "$disabled_login_rc" -ne 0 ]] || fail "login unexpectedly succeeded after remote access was disabled"
assert_contains "$disabled_login_output" "remote Admin access is disabled" "Client remote disable closes subsequent LAN login"

echo "Client Admin CLI E2E passed (${pass_count} assertions) via ${TARGET_URL}"
