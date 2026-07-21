#!/bin/bash
# Desktop version-check runtime-owner regression:
# a CLI-owned core must not treat an inactive old CLI copy as its companion,
# while an App-owned core must still report a genuinely stale standalone CLI.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
fi

ROOT=""
CORE_PID=""
PORT=""

cleanup_core() {
    if [[ -n "$CORE_PID" ]] && kill -0 "$CORE_PID" 2>/dev/null; then
        kill -TERM "$CORE_PID" 2>/dev/null || true
        wait "$CORE_PID" 2>/dev/null || true
    fi
    CORE_PID=""
}

cleanup() {
    cleanup_core
    if [[ -n "$ROOT" && -d "$ROOT" ]]; then
        rm -rf "$ROOT"
    fi
}
trap cleanup EXIT

allocate_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

json_field() {
    python3 -c 'import json,sys; print(json.load(sys.stdin).get(sys.argv[1], ""))' "$1"
}

admin_curl() {
    local path="$1"
    env NO_PROXY='*' no_proxy='*' curl -fsS \
        --connect-timeout 2 --max-time 15 \
        "http://127.0.0.1:${PORT}/_bifrost${path}" 2>/dev/null
}

wait_admin_ready() {
    local attempt
    for attempt in $(seq 1 100); do
        if admin_curl /api/system >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

write_version_cache() {
    local data_dir="$1" latest="$2"
    python3 - "$data_dir/version_cache.json" "$latest" <<'PY'
import datetime, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps({
    "latest_version": sys.argv[2],
    "release_highlights": ["runtime-owner version-check regression"],
    "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}))
PY
}

write_app_fixture() {
    local app="$1" version="$2"
    mkdir -p "$app/Contents/MacOS"
    cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version##*.}</string>
</dict></plist>
PLIST
}

start_core() {
    local data_dir="$1" app_dir="$2" home_dir="$3" cli_dir="$4" core_bin="$5" owner="$6"
    PORT="$(allocate_free_port)"
    if [[ "$owner" == "desktop" ]]; then
        BIFROST_DESKTOP_CORE=1 \
        BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        HOME="$home_dir" \
        PATH="$cli_dir:/usr/bin:/bin" \
        "$core_bin" start -y --skip-cert-check -p "$PORT" --host 127.0.0.1 \
            --access-mode allow_all --no-system-proxy --no-intercept \
            >"$data_dir/core.log" 2>&1 &
    else
        env -u BIFROST_DESKTOP_CORE \
        BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        HOME="$home_dir" \
        PATH="$cli_dir:/usr/bin:/bin" \
        "$core_bin" start -y --skip-cert-check -p "$PORT" --host 127.0.0.1 \
            --access-mode allow_all --no-system-proxy --no-intercept \
            >"$data_dir/core.log" 2>&1 &
    fi
    CORE_PID=$!
    wait_admin_ready
}

main() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        _log_warning "desktop bundle version-check regression is macOS-only"
        print_test_summary
        return
    fi
    if [[ ! -x "$BIFROST_BIN" ]]; then
        _log_fail "bifrost binary exists" "executable" "$BIFROST_BIN"
        print_test_summary
        return
    fi

    ROOT="$(mktemp -d)"
    local app_dir="$ROOT/apps"
    local app="$app_dir/Bifrost.app"
    local home_dir="$ROOT/home"
    local cli_dir="$home_dir/.local/bin"
    local core_bin="$ROOT/core/bifrost"
    local running_version stale_version="0.0.1"
    mkdir -p "$cli_dir" "$(dirname "$core_bin")"
    cp "$BIFROST_BIN" "$core_bin"
    chmod +x "$core_bin"
    running_version="$("$core_bin" --version | awk '{print $2}')"
    write_app_fixture "$app" "$running_version"

    cat >"$cli_dir/bifrost" <<SH
#!/bin/sh
printf 'bifrost %s\n' '$stale_version'
SH
    chmod +x "$cli_dir/bifrost"

    local cli_owned_data="$ROOT/cli-owned-data"
    write_version_cache "$cli_owned_data" "$running_version"
    if start_core "$cli_owned_data" "$app_dir" "$home_dir" "$cli_dir" "$core_bin" cli; then
        _log_pass "CLI-owned core starts with an inactive stale CLI copy on PATH"
    else
        _log_fail "CLI-owned core starts" "Admin API ready" "$(cat "$cli_owned_data/core.log" 2>/dev/null)"
        print_test_summary
        return
    fi

    local response current latest has_update
    response="$(admin_curl '/api/system/version-check?channel=desktop')"
    current="$(printf '%s' "$response" | json_field current_version)"
    latest="$(printf '%s' "$response" | json_field latest_version)"
    has_update="$(printf '%s' "$response" | json_field has_update)"
    assert_equals "$running_version" "$current" "CLI-owned desktop check uses the serving core version"
    assert_equals "$running_version" "$latest" "CLI-owned desktop check keeps the release target"
    assert_equals "False" "$has_update" "inactive stale CLI copy does not trigger a desktop update"
    cleanup_core

    local desktop_owned_data="$ROOT/desktop-owned-data"
    write_version_cache "$desktop_owned_data" "$running_version"
    if start_core "$desktop_owned_data" "$app_dir" "$home_dir" "$cli_dir" "$core_bin" desktop; then
        _log_pass "App-owned core starts with a stale standalone CLI"
    else
        _log_fail "App-owned core starts" "Admin API ready" "$(cat "$desktop_owned_data/core.log" 2>/dev/null)"
        print_test_summary
        return
    fi

    response="$(admin_curl '/api/system/version-check?channel=desktop')"
    current="$(printf '%s' "$response" | json_field current_version)"
    latest="$(printf '%s' "$response" | json_field latest_version)"
    has_update="$(printf '%s' "$response" | json_field has_update)"
    assert_equals "$stale_version" "$current" "companion-only update reports the stale CLI version"
    assert_equals "$running_version" "$latest" "App-owned desktop check keeps the release target"
    assert_equals "True" "$has_update" "genuinely stale standalone CLI keeps unified update available"

    print_test_summary
}

main "$@"
