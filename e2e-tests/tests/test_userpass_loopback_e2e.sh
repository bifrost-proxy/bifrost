#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX
if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
export ADMIN_PORT

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
UPSTREAM_PORT=""
UPSTREAM_URL=""
UPSTREAM_PID=""
UPSTREAM_LOG=""

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

start_http_fixture() {
    UPSTREAM_PORT="$(allocate_free_port)"
    UPSTREAM_URL="http://127.0.0.1:${UPSTREAM_PORT}"
    UPSTREAM_LOG="$(mktemp)"

    python3 -u - "$UPSTREAM_PORT" >"$UPSTREAM_LOG" 2>&1 <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "ok": True,
            "path": self.path,
            "via": "bifrost-userpass-e2e",
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    UPSTREAM_PID=$!

    for _ in $(seq 1 80); do
        if curl --noproxy "*" -fsS --max-time 2 "$UPSTREAM_URL/health" >/dev/null 2>&1; then
            log_info "Started local upstream fixture on ${UPSTREAM_URL}"
            return 0
        fi
        if ! kill -0 "$UPSTREAM_PID" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done

    log_fail "Timed out waiting for local upstream fixture on ${UPSTREAM_URL}"
    [[ -n "$UPSTREAM_LOG" ]] && tail -80 "$UPSTREAM_LOG" >&2 || true
    return 1
}

stop_http_fixture() {
    if [[ -n "$UPSTREAM_PID" ]] && kill -0 "$UPSTREAM_PID" 2>/dev/null; then
        kill "$UPSTREAM_PID" 2>/dev/null || true
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
    [[ -n "$UPSTREAM_LOG" ]] && rm -f "$UPSTREAM_LOG" 2>/dev/null || true
    UPSTREAM_PID=""
    UPSTREAM_LOG=""
}

run_test() {
    local test_name="$1"
    local test_func="$2"

    TESTS_RUN=$((TESTS_RUN + 1))
    log_info "Running test: $test_name"

    if $test_func; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_pass "$test_name"
        return 0
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_fail "$test_name"
        return 1
    fi
}

ORIGINAL_WHITELIST_RESPONSE=""

save_state() {
    ORIGINAL_WHITELIST_RESPONSE=$(admin_get "/api/whitelist")
}

restore_state() {
    set_userpass_config '{"enabled":false,"accounts":[],"loopback_requires_auth":false}' >/dev/null 2>&1

    if [[ -n "$ORIGINAL_WHITELIST_RESPONSE" ]]; then
        local mode
        mode=$(echo "$ORIGINAL_WHITELIST_RESPONSE" | jq -r '.mode // "local_only"')
        set_whitelist_mode "$mode" >/dev/null 2>&1
    fi
}

enable_userpass() {
    local loopback_requires_auth="${1:-false}"
    set_userpass_config "{
        \"enabled\": true,
        \"accounts\": [
            {\"username\": \"testuser\", \"password\": \"testpass123\", \"enabled\": true}
        ],
        \"loopback_requires_auth\": $loopback_requires_auth
    }"
}

disable_userpass() {
    set_userpass_config '{"enabled": false, "accounts": [], "loopback_requires_auth": false}'
}

bifrost_cli() {
    local bifrost_bin="${BIFROST_BIN:-}"
    if [[ -z "$bifrost_bin" ]]; then
        if [[ -x "$ADMIN_CLIENT_REPO_DIR/target/release/bifrost" ]]; then
            bifrost_bin="$ADMIN_CLIENT_REPO_DIR/target/release/bifrost"
        elif [[ -x "$ADMIN_CLIENT_REPO_DIR/target/debug/bifrost" ]]; then
            bifrost_bin="$ADMIN_CLIENT_REPO_DIR/target/debug/bifrost"
        else
            bifrost_bin="$ADMIN_CLIENT_REPO_DIR/target/release/bifrost"
        fi
    fi
    HOME="$ADMIN_CLIENT_HOME_DIR" \
        XDG_CONFIG_HOME="$ADMIN_CLIENT_XDG_CONFIG_HOME" \
        XDG_DATA_HOME="$ADMIN_CLIENT_XDG_DATA_HOME" \
        BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
        "$bifrost_bin" -p "$(admin_port)" "$@"
}

restart_bifrost_without_user_env() {
    local saved_user="${USER-}"
    local saved_username="${USERNAME-}"
    local saved_userprofile="${USERPROFILE-}"

    if [[ -n "$ADMIN_CLIENT_BIFROST_PID" ]] && kill -0 "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null; then
        safe_cleanup_proxy "$ADMIN_CLIENT_BIFROST_PID"
    fi
    [[ -n "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]] && rm -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null || true
    ADMIN_CLIENT_BIFROST_PID=""
    ADMIN_CLIENT_BIFROST_LOG_FILE=""

    unset USER USERNAME USERPROFILE
    local rc=0
    admin_start_bifrost || rc=$?
    [[ -n "$saved_user" ]] && export USER="$saved_user"
    [[ -n "$saved_username" ]] && export USERNAME="$saved_username"
    [[ -n "$saved_userprofile" ]] && export USERPROFILE="$saved_userprofile"
    return "$rc"
}

proxy_http_status() {
    local proxy_user_arg=""
    if [[ -n "${1:-}" ]]; then
        proxy_user_arg="--proxy-user $1"
    fi
    curl -s -o /dev/null -w '%{http_code}' \
        --noproxy "" \
        --proxy "http://127.0.0.1:${ADMIN_PORT}" \
        $proxy_user_arg \
        --max-time 10 \
        "${UPSTREAM_URL}/get" 2>/dev/null
}

proxy_http_capture() {
    local output_file="$1"
    local proxy_user_arg=""
    if [[ -n "${2:-}" ]]; then
        proxy_user_arg="--proxy-user $2"
    fi
    curl -s -i \
        --noproxy "" \
        --proxy "http://127.0.0.1:${ADMIN_PORT}" \
        $proxy_user_arg \
        --max-time 10 \
        "${UPSTREAM_URL}/get" >"$output_file" 2>&1
}

test_userpass_config_api() {
    enable_userpass false >/dev/null 2>&1
    sleep 0.3

    local response
    response=$(admin_get "/api/whitelist")
    local userpass_enabled
    userpass_enabled=$(echo "$response" | jq -r '.userpass.enabled')
    if [[ "$userpass_enabled" != "true" ]]; then
        log_fail "Userpass should be enabled after setting, got: $userpass_enabled"
        return 1
    fi

    local loopback_requires_auth
    loopback_requires_auth=$(echo "$response" | jq -r '.userpass.loopback_requires_auth')
    if [[ "$loopback_requires_auth" != "false" ]]; then
        log_fail "loopback_requires_auth should be false, got: $loopback_requires_auth"
        return 1
    fi

    local account_count
    account_count=$(echo "$response" | jq '.userpass.accounts | length')
    if [[ "$account_count" -lt 1 ]]; then
        log_fail "Should have at least 1 account, got: $account_count"
        return 1
    fi

    local username
    username=$(echo "$response" | jq -r '.userpass.accounts[0].username')
    if [[ "$username" != "testuser" ]]; then
        log_fail "Username should be 'testuser', got: $username"
        return 1
    fi

    local has_password
    has_password=$(echo "$response" | jq -r '.userpass.accounts[0].has_password')
    if [[ "$has_password" != "true" ]]; then
        log_fail "has_password should be true, got: $has_password"
        return 1
    fi

    enable_userpass true >/dev/null 2>&1
    sleep 0.3

    response=$(admin_get "/api/whitelist")
    loopback_requires_auth=$(echo "$response" | jq -r '.userpass.loopback_requires_auth')
    if [[ "$loopback_requires_auth" != "true" ]]; then
        log_fail "loopback_requires_auth should be true after update, got: $loopback_requires_auth"
        return 1
    fi

    disable_userpass >/dev/null 2>&1
    sleep 0.3

    response=$(admin_get "/api/whitelist")
    userpass_enabled=$(echo "$response" | jq -r '.userpass.enabled')
    if [[ "$userpass_enabled" != "false" ]]; then
        log_fail "Userpass should be disabled after clearing, got: $userpass_enabled"
        return 1
    fi

    return 0
}

test_account_cli_multi_account_traffic_and_encrypted_config() {
    disable_userpass >/dev/null 2>&1
    sleep 0.3

    local out_dir="$BIFROST_DATA_DIR/account-cli-e2e"
    mkdir -p "$out_dir"

    printf 'alpha-secret-123\n' | bifrost_cli account add alice --password-stdin --enable-auth >"$out_dir/add-alice.out" 2>&1 || {
        cat "$out_dir/add-alice.out"
        log_fail "account add alice failed"
        return 1
    }
    printf 'beta-secret-123\n' | bifrost_cli account add bob --password-stdin >"$out_dir/add-bob.out" 2>&1 || {
        cat "$out_dir/add-bob.out"
        log_fail "account add bob failed"
        return 1
    }
    printf 'disabled-secret-123\n' | bifrost_cli account add disabled-user --password-stdin --disabled >"$out_dir/add-disabled.out" 2>&1 || {
        cat "$out_dir/add-disabled.out"
        log_fail "account add disabled-user failed"
        return 1
    }
    printf 'bifrost-local-secret:not-json\n' | bifrost_cli account add prefix-user --password-stdin >"$out_dir/add-prefix.out" 2>&1 || {
        cat "$out_dir/add-prefix.out"
        log_fail "account add prefix-user failed"
        return 1
    }

    local response
    response=$(admin_get "/api/whitelist")
    if [[ "$(echo "$response" | jq -r '.userpass.enabled')" != "true" ]]; then
        log_fail "account add --enable-auth should enable userpass"
        return 1
    fi
    if [[ "$(echo "$response" | jq -r '[.userpass.accounts[] | select(.username=="alice" or .username=="bob" or .username=="disabled-user" or .username=="prefix-user")] | length')" != "4" ]]; then
        log_fail "all four CLI accounts should be visible"
        return 1
    fi
    if [[ "$(echo "$response" | jq -r '.userpass.accounts[] | select(.username=="disabled-user") | .enabled')" != "false" ]]; then
        log_fail "disabled-user should be created disabled"
        return 1
    fi

    bifrost_cli account list --json >"$out_dir/list.json" 2>&1 || {
        cat "$out_dir/list.json"
        log_fail "account list --json failed"
        return 1
    }
    if [[ "$(jq -r '[.accounts[] | select(.has_password == true)] | length' "$out_dir/list.json")" -lt "4" ]]; then
        log_fail "account list --json should expose has_password for all accounts"
        return 1
    fi

    bifrost_cli account disable >"$out_dir/disable-auth.out" 2>&1 || {
        cat "$out_dir/disable-auth.out"
        log_fail "account disable failed"
        return 1
    }
    response=$(admin_get "/api/whitelist")
    if [[ "$(echo "$response" | jq -r '.userpass.enabled')" != "false" ]]; then
        log_fail "account disable should disable global userpass auth"
        return 1
    fi
    if [[ "$(echo "$response" | jq -r '.userpass.accounts | length')" != "4" ]]; then
        log_fail "account disable must preserve all configured accounts"
        return 1
    fi
    if [[ "$(echo "$response" | jq -r '[.userpass.accounts[] | select(.has_password == true)] | length')" != "4" ]]; then
        log_fail "account disable must preserve every stored password"
        return 1
    fi
    bifrost_cli account list --json >"$out_dir/list-disabled.json" 2>&1 || {
        cat "$out_dir/list-disabled.json"
        log_fail "account list after global disable failed"
        return 1
    }
    if [[ "$(jq -r '.enabled' "$out_dir/list-disabled.json")" != "false" || "$(jq -r '.accounts | length' "$out_dir/list-disabled.json")" != "4" ]]; then
        log_fail "account list should report disabled auth without dropping accounts"
        return 1
    fi
    bifrost_cli account enable >"$out_dir/enable-auth.out" 2>&1 || {
        cat "$out_dir/enable-auth.out"
        log_fail "account enable failed"
        return 1
    }
    response=$(admin_get "/api/whitelist")
    if [[ "$(echo "$response" | jq -r '.userpass.enabled')" != "true" || "$(echo "$response" | jq -r '.userpass.accounts | length')" != "4" ]]; then
        log_fail "account enable should restore global auth with all accounts intact"
        return 1
    fi

    local raw_config="$BIFROST_DATA_DIR/config.toml"
    if grep -Eq 'alpha-secret-123|beta-secret-123|disabled-secret-123|bifrost-local-secret:not-json' "$raw_config"; then
        log_fail "config.toml should not contain plaintext account password"
        return 1
    fi
    if ! grep -q 'bifrost-local-secret:' "$raw_config"; then
        log_fail "config.toml should contain encrypted local secret envelope"
        return 1
    fi
    if [[ ! -f "$BIFROST_DATA_DIR/local_config_secret.key" ]]; then
        log_fail "local config secret key should be persisted beside config.toml"
        return 1
    fi

    if ! restart_bifrost_without_user_env; then
        log_fail "Bifrost should restart with the same data dir when USER-style env vars are absent"
        return 1
    fi
    bifrost_cli account set-loopback-auth true >"$out_dir/restart-loopback.out" 2>&1 || {
        cat "$out_dir/restart-loopback.out"
        log_fail "account config should remain readable after changed-env restart"
        return 1
    }
    local prefix_status
    prefix_status=$(proxy_http_status "prefix-user:bifrost-local-secret:not-json")
    if [[ "$prefix_status" != "200" ]]; then
        log_fail "reserved-prefix password should authenticate after restart (got $prefix_status)"
        return 1
    fi

    bifrost_cli account set-loopback-auth true >"$out_dir/loopback.out" 2>&1 || {
        cat "$out_dir/loopback.out"
        log_fail "account set-loopback-auth failed"
        return 1
    }
    sleep 0.3
    local http_status
    http_status=$(proxy_http_status)
    if [[ "$http_status" != "407" ]]; then
        log_fail "account set-loopback-auth true should require credentials (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "alice:alpha-secret-123")
    if [[ "$http_status" != "200" ]]; then
        log_fail "alice credentials should authenticate through real HTTP proxy traffic (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "bob:beta-secret-123")
    if [[ "$http_status" != "200" ]]; then
        log_fail "bob credentials should authenticate through real HTTP proxy traffic (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "bob:wrong-password")
    if [[ "$http_status" != "407" ]]; then
        log_fail "wrong bob password should be rejected with 407 (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "disabled-user:disabled-secret-123")
    if [[ "$http_status" != "407" ]]; then
        log_fail "disabled account should be rejected with 407 (got $http_status)"
        return 1
    fi

    printf 'beta-secret-456\n' | bifrost_cli account update bob --password-stdin >"$out_dir/update-bob-password.out" 2>&1 || {
        cat "$out_dir/update-bob-password.out"
        log_fail "account update bob password failed"
        return 1
    }
    http_status=$(proxy_http_status "bob:beta-secret-123")
    if [[ "$http_status" != "407" ]]; then
        log_fail "old bob password should stop working after update (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "bob:beta-secret-456")
    if [[ "$http_status" != "200" ]]; then
        log_fail "new bob password should authenticate after update (got $http_status)"
        return 1
    fi
    bifrost_cli account update bob --disable >"$out_dir/disable-bob.out" 2>&1 || {
        cat "$out_dir/disable-bob.out"
        log_fail "account update bob --disable failed"
        return 1
    }
    response=$(admin_get "/api/whitelist")
    if [[ "$(echo "$response" | jq -r '.userpass.accounts[] | select(.username=="bob") | .enabled')" != "false" ]]; then
        log_fail "account update --disable should disable bob"
        return 1
    fi
    http_status=$(proxy_http_status "bob:beta-secret-456")
    if [[ "$http_status" != "407" ]]; then
        log_fail "disabled bob should be rejected with 407 (got $http_status)"
        return 1
    fi
    bifrost_cli account update bob --enable >"$out_dir/enable-bob.out" 2>&1 || {
        cat "$out_dir/enable-bob.out"
        log_fail "account update bob --enable failed"
        return 1
    }
    http_status=$(proxy_http_status "bob:beta-secret-456")
    if [[ "$http_status" != "200" ]]; then
        log_fail "re-enabled bob should authenticate again (got $http_status)"
        return 1
    fi
    if grep -q 'beta-secret-456' "$raw_config"; then
        log_fail "updated account password should also be encrypted at rest"
        return 1
    fi

    bifrost_cli account remove alice >"$out_dir/remove.out" 2>&1 || {
        cat "$out_dir/remove.out"
        log_fail "account remove failed"
        return 1
    }
    response=$(admin_get "/api/whitelist")
    if [[ "$(echo "$response" | jq '[.userpass.accounts[] | select(.username=="alice")] | length')" != "0" ]]; then
        log_fail "alice should be removed"
        return 1
    fi
    http_status=$(proxy_http_status "alice:alpha-secret-123")
    if [[ "$http_status" != "407" ]]; then
        log_fail "removed alice credentials should be rejected with 407 (got $http_status)"
        return 1
    fi
    http_status=$(proxy_http_status "bob:beta-secret-456")
    if [[ "$http_status" != "200" ]]; then
        log_fail "remaining bob account should keep authenticating after alice removal (got $http_status)"
        return 1
    fi

    return 0
}

test_http_proxy_bruteforce_limit_and_success_reset() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status "testuser:testpass123")
    if [[ "$http_status" != "200" ]]; then
        log_fail "initial valid auth should reset any prior failure count (got $http_status)"
        return 1
    fi

    for _ in $(seq 1 5); do
        http_status=$(proxy_http_status "testuser:wrong-before-reset")
        if [[ "$http_status" != "407" ]]; then
            log_fail "wrong credentials before reset should return 407 (got $http_status)"
            return 1
        fi
    done

    http_status=$(proxy_http_status "testuser:testpass123")
    if [[ "$http_status" != "200" ]]; then
        log_fail "valid auth should succeed and reset failure count after 5 failures (got $http_status)"
        return 1
    fi

    for _ in $(seq 1 9); do
        http_status=$(proxy_http_status "testuser:wrong-after-reset")
        if [[ "$http_status" != "407" ]]; then
            log_fail "post-reset wrong credentials should return 407 before threshold (got $http_status)"
            return 1
        fi
    done

    http_status=$(proxy_http_status "testuser:testpass123")
    if [[ "$http_status" != "200" ]]; then
        log_fail "valid auth after 9 post-reset failures should still succeed (got $http_status)"
        return 1
    fi

    for _ in $(seq 1 10); do
        http_status=$(proxy_http_status "testuser:wrong-to-ban")
        if [[ "$http_status" != "407" ]]; then
            log_fail "wrong credentials up to ban threshold should return 407 (got $http_status)"
            return 1
        fi
    done

    local banned_output="$BIFROST_DATA_DIR/account-cli-e2e/banned-response.out"
    mkdir -p "$(dirname "$banned_output")"
    proxy_http_capture "$banned_output" "testuser:testpass123" || true
    if ! grep -q "429 Too Many Requests" "$banned_output"; then
        log_fail "banned request should return 429"
        cat "$banned_output"
        return 1
    fi
    if ! grep -qi "Retry-After: 300" "$banned_output"; then
        log_fail "banned response should include Retry-After: 300"
        cat "$banned_output"
        return 1
    fi
    if ! grep -q "Too many failed authentication attempts" "$banned_output"; then
        log_fail "banned response body should explain failed authentication attempts"
        cat "$banned_output"
        return 1
    fi

    return 0
}

test_loopback_no_auth_default() {
    enable_userpass false >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status)

    log_debug "HTTP proxy without auth (loopback_requires_auth=false): $http_status"

    if [[ "$http_status" == "407" ]]; then
        log_fail "Loopback HTTP proxy should NOT require auth when loopback_requires_auth=false (got 407)"
        return 1
    fi

    log_info "Status: $http_status (not 407, OK)"
    return 0
}

test_loopback_with_auth_also_works() {
    enable_userpass false >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status "testuser:testpass123")

    log_debug "HTTP proxy with valid auth: $http_status"

    if [[ "$http_status" == "407" ]]; then
        log_fail "HTTP proxy with valid credentials should NOT return 407"
        return 1
    fi

    log_info "Status: $http_status (not 407, OK)"
    return 0
}

test_loopback_requires_auth_on_returns_407_without_creds() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status)

    log_debug "HTTP proxy without auth (loopback_requires_auth=true): $http_status"

    if [[ "$http_status" != "407" ]]; then
        log_fail "Loopback should require auth when loopback_requires_auth=true (expected 407, got $http_status)"
        return 1
    fi

    log_info "Status: $http_status (407 as expected)"
    return 0
}

test_loopback_requires_auth_on_passes_with_valid_creds() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status "testuser:testpass123")

    log_debug "HTTP proxy with valid auth (loopback_requires_auth=true): $http_status"

    if [[ "$http_status" == "407" ]]; then
        log_fail "Loopback with valid credentials should NOT return 407 even with loopback_requires_auth=true"
        return 1
    fi

    log_info "Status: $http_status (not 407, OK)"
    return 0
}

test_loopback_requires_auth_on_rejects_wrong_creds() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(proxy_http_status "testuser:wrongpassword")

    log_debug "HTTP proxy with wrong auth (loopback_requires_auth=true): $http_status"

    if [[ "$http_status" != "407" ]]; then
        log_fail "Wrong credentials should return 407 (got $http_status)"
        return 1
    fi

    log_info "Status: $http_status (407 as expected)"
    return 0
}

test_loopback_https_connect_no_auth_default() {
    enable_userpass false >/dev/null 2>&1
    sleep 0.5

    local http_status
    http_status=$(curl -s -o /dev/null -w '%{http_code}' \
        --proxy "http://127.0.0.1:${ADMIN_PORT}" \
        --max-time 10 \
        -k \
        "https://httpbin.org/get" 2>/dev/null)

    log_debug "HTTPS CONNECT without auth (loopback_requires_auth=false): $http_status"

    if [[ "$http_status" == "407" ]]; then
        log_fail "HTTPS CONNECT should NOT require auth when loopback_requires_auth=false (got 407)"
        return 1
    fi

    log_info "Status: $http_status (not 407, OK)"
    return 0
}

test_loopback_https_connect_requires_auth_on() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local output
    output=$(curl -v -o /dev/null -w '%{http_code}' \
        --proxy "http://127.0.0.1:${ADMIN_PORT}" \
        --max-time 10 \
        -k \
        "https://httpbin.org/get" 2>&1)

    log_debug "HTTPS CONNECT without auth (loopback_requires_auth=true) output: $output"

    if echo "$output" | grep -q "407 Proxy Authentication Required"; then
        log_info "CONNECT correctly rejected with 407"
        return 0
    fi

    log_fail "HTTPS CONNECT should require auth when loopback_requires_auth=true (407 not found in output)"
    return 1
}

test_admin_api_still_works_with_userpass_enabled() {
    enable_userpass true >/dev/null 2>&1
    sleep 0.5

    local response
    response=$(admin_get "/api/system")

    if [[ -z "$response" || "$response" == "null" ]]; then
        log_fail "Admin API should still be accessible with userpass enabled"
        return 1
    fi

    local version
    version=$(echo "$response" | jq -r '.version // empty')
    if [[ -z "$version" ]]; then
        log_fail "Admin API returned invalid response: $response"
        return 1
    fi

    log_info "Admin API returned version: $version"
    return 0
}

test_toggle_loopback_requires_auth() {
    enable_userpass false >/dev/null 2>&1
    sleep 0.3

    local http_status
    http_status=$(proxy_http_status)
    if [[ "$http_status" == "407" ]]; then
        log_fail "Phase 1: loopback_requires_auth=false, should not get 407"
        return 1
    fi
    log_debug "Phase 1 (off): $http_status"

    enable_userpass true >/dev/null 2>&1
    sleep 0.3

    http_status=$(proxy_http_status)
    if [[ "$http_status" != "407" ]]; then
        log_fail "Phase 2: loopback_requires_auth=true, should get 407 (got $http_status)"
        return 1
    fi
    log_debug "Phase 2 (on): $http_status"

    http_status=$(proxy_http_status "testuser:testpass123")
    if [[ "$http_status" == "407" ]]; then
        log_fail "Phase 2b: valid creds should pass (got 407)"
        return 1
    fi
    log_debug "Phase 2b (on+creds): $http_status"

    enable_userpass false >/dev/null 2>&1
    sleep 0.3

    http_status=$(proxy_http_status)
    if [[ "$http_status" == "407" ]]; then
        log_fail "Phase 3: loopback_requires_auth=false again, should not get 407"
        return 1
    fi
    log_debug "Phase 3 (off again): $http_status"

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Userpass Loopback Auth E2E Test Summary"
    echo "======================================"
    echo "Tests Run:    $TESTS_RUN"
    echo "Tests Passed: $TESTS_PASSED"
    echo "Tests Failed: $TESTS_FAILED"
    echo "======================================"

    if [[ $TESTS_FAILED -eq 0 ]]; then
        echo "All tests passed!"
        return 0
    else
        echo "Some tests failed!"
        return 1
    fi
}

main() {
    trap 'restore_state; stop_http_fixture; admin_cleanup_bifrost' EXIT

    if ! admin_ensure_bifrost; then
        log_fail "Admin server is not reachable and failed to start"
        exit 1
    fi
    if ! start_http_fixture; then
        log_fail "Local upstream fixture failed to start"
        exit 1
    fi

    log_info "Starting Userpass Loopback Auth E2E Tests"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    log_info "Upstream: $UPSTREAM_URL"
    echo ""

    save_state

    run_test "Userpass Config API (with loopback_requires_auth)" test_userpass_config_api
    run_test "Account CLI multi-account CRUD, real proxy traffic, and encrypted config" test_account_cli_multi_account_traffic_and_encrypted_config
    run_test "Loopback No Auth Required (default)" test_loopback_no_auth_default
    run_test "Loopback With Auth Also Works" test_loopback_with_auth_also_works
    run_test "Loopback Requires Auth ON - Returns 407 Without Creds" test_loopback_requires_auth_on_returns_407_without_creds
    run_test "Loopback Requires Auth ON - Passes With Valid Creds" test_loopback_requires_auth_on_passes_with_valid_creds
    run_test "Loopback Requires Auth ON - Rejects Wrong Creds" test_loopback_requires_auth_on_rejects_wrong_creds
    run_test "Loopback HTTPS CONNECT No Auth (default)" test_loopback_https_connect_no_auth_default
    run_test "Loopback HTTPS CONNECT Requires Auth ON" test_loopback_https_connect_requires_auth_on
    run_test "Admin API Still Works With Userpass Enabled" test_admin_api_still_works_with_userpass_enabled
    run_test "Toggle loopback_requires_auth On/Off Cycle" test_toggle_loopback_requires_auth
    run_test "HTTP Proxy brute-force limit and success reset" test_http_proxy_bruteforce_limit_and_success_reset

    restore_state

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
