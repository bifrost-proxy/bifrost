#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL:=1}"
: "${BIFROST_SYSTEM_PROXY_RECONCILE_SECS:=3}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL
export BIFROST_SYSTEM_PROXY_RECONCILE_SECS
unset BIFROST_DESKTOP_CORE BIFROST_DESKTOP_APP

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "SKIP: system proxy reconcile stability requires macOS"
    exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/../test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
PROXY_PORT="${PROXY_PORT:-18889}"
TEST_ROOT="$(mktemp -d)"
export BIFROST_DATA_DIR="$TEST_ROOT/data"
SNAPSHOT_FILE="$TEST_ROOT/macos-proxy-before.tsv"
PROXY_LOG="$TEST_ROOT/proxy.log"
PROXY_PID=""
mkdir -p "$BIFROST_DATA_DIR"

network_services() {
    networksetup -listallnetworkservices 2>/dev/null | sed '1d' | sed '/^\*/d'
}

proxy_field() {
    local kind="$1"
    local service="$2"
    local field="$3"
    networksetup "-get${kind}proxy" "$service" 2>/dev/null \
        | awk -F': ' -v field="$field" '$1 == field { print $2; exit }'
}

save_proxy_snapshot() {
    local destination="${1:-$SNAPSHOT_FILE}"
    : >"$destination"
    while IFS= read -r service; do
        printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
            "$service" \
            "$(proxy_field web "$service" Enabled)" \
            "$(proxy_field web "$service" Server)" \
            "$(proxy_field web "$service" Port)" \
            "$(proxy_field web "$service" 'Authenticated Proxy Enabled')" \
            "$(proxy_field secureweb "$service" Enabled)" \
            "$(proxy_field secureweb "$service" Server)" \
            "$(proxy_field secureweb "$service" Port)" \
            "$(proxy_field secureweb "$service" 'Authenticated Proxy Enabled')" \
            >>"$destination"
    done < <(network_services)
}

restore_proxy_snapshot() {
    [[ -f "$SNAPSHOT_FILE" ]] || return 0
    while IFS='|' read -r service web_enabled web_host web_port _web_auth secure_enabled secure_host secure_port _secure_auth; do
        if [[ -n "$web_host" && "$web_port" =~ ^[0-9]+$ ]]; then
            networksetup -setwebproxy "$service" "$web_host" "$web_port" >/dev/null 2>&1 || true
        fi
        if [[ "$web_enabled" == "Yes" ]]; then
            networksetup -setwebproxystate "$service" on >/dev/null 2>&1 || true
        else
            networksetup -setwebproxystate "$service" off >/dev/null 2>&1 || true
        fi
        if [[ -n "$secure_host" && "$secure_port" =~ ^[0-9]+$ ]]; then
            networksetup -setsecurewebproxy "$service" "$secure_host" "$secure_port" >/dev/null 2>&1 || true
        fi
        if [[ "$secure_enabled" == "Yes" ]]; then
            networksetup -setsecurewebproxystate "$service" on >/dev/null 2>&1 || true
        else
            networksetup -setsecurewebproxystate "$service" off >/dev/null 2>&1 || true
        fi
    done <"$SNAPSHOT_FILE"
}

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID" || true
    fi
    restore_proxy_snapshot
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT

save_proxy_snapshot
if awk -F'|' '$2 == "Yes" || $5 == "1" || $6 == "Yes" || $9 == "1" { found = 1 } END { exit !found }' "$SNAPSHOT_FILE"; then
    echo "SKIP: existing enabled or authenticated system proxy must not be replaced by this test"
    exit 0
fi
if lsof -nP -iTCP:"$PROXY_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "proxy port $PROXY_PORT is already in use"
    exit 1
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --bin bifrost
fi

"$BIFROST_BIN" --port "$PROXY_PORT" start \
    --skip-cert-check --unsafe-ssl --system-proxy \
    --proxy-bypass "localhost,127.0.0.1,::1,*.local" \
    >"$PROXY_LOG" 2>&1 &
PROXY_PID=$!

for _ in $(seq 1 90); do
    if curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        tail -n 160 "$PROXY_LOG"
        exit 1
    fi
    sleep 0.5
done
curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null

for _ in $(seq 1 90); do
    if grep -h -q "system proxy full reconcile completed" \
        "$BIFROST_DATA_DIR"/logs/bifrost.*.log 2>/dev/null; then
        break
    fi
    sleep 0.5
done

sleep "$((BIFROST_SYSTEM_PROXY_RECONCILE_SECS * 2 + 2))"
full_reconcile_count="$({ grep -h "system proxy full reconcile completed" \
    "$BIFROST_DATA_DIR"/logs/bifrost.*.log 2>/dev/null || true; } | wc -l | tr -d ' ')"
if [[ "$full_reconcile_count" -ne 1 ]]; then
    echo "expected one full system proxy reconcile across two short cycles, got $full_reconcile_count"
    tail -n 200 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi

status="$(curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/proxy/system")"
if ! grep -q '"managed_by_bifrost":true' <<<"$status"; then
    echo "system proxy ownership changed unexpectedly: $status"
    exit 1
fi

safe_cleanup_proxy "$PROXY_PID"
PROXY_PID=""
restore_proxy_snapshot
AFTER_SNAPSHOT_FILE="$TEST_ROOT/macos-proxy-after.tsv"
save_proxy_snapshot "$AFTER_SNAPSHOT_FILE"
if ! cmp -s "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE"; then
    echo "system proxy snapshot was not restored exactly"
    diff -u "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE" || true
    exit 1
fi

echo "PASS: converged system proxy performed one full reconcile across two short cycles"
