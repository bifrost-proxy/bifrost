#!/bin/bash
# Real App-owned upgrade orchestration regression:
# Admin request -> desktop channel -> standalone CLI child -> App install.

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

cleanup() {
    if [[ -n "$CORE_PID" ]] && kill -0 "$CORE_PID" 2>/dev/null; then
        kill -TERM "$CORE_PID" 2>/dev/null || true
        wait "$CORE_PID" 2>/dev/null || true
    fi
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
    local method="$1" path="$2"
    env NO_PROXY='*' no_proxy='*' curl -fsS -X "$method" \
        --connect-timeout 2 --max-time 15 \
        "http://127.0.0.1:${PORT}/_bifrost${path}" 2>/dev/null
}

wait_admin_ready() {
    local attempt
    for attempt in $(seq 1 100); do
        if admin_curl GET /api/system >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

write_app_fixture() {
    local app="$1" version="$2"
    mkdir -p "$app/Contents/MacOS"
    cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>Bifrost</string>
  <key>CFBundleIdentifier</key><string>dev.bifrost.test</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version##*.}</string>
</dict></plist>
PLIST
    printf '#!/bin/sh\nexit 0\n' >"$app/Contents/MacOS/Bifrost"
    chmod +x "$app/Contents/MacOS/Bifrost"
}

main() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        _log_warning "App-owned real bundle upgrade is macOS-only; runtime ownership unit/E2E still runs cross-platform"
        print_test_summary
        return
    fi
    if [[ ! -x "$BIFROST_BIN" ]]; then
        _log_fail "bifrost binary exists" "executable" "$BIFROST_BIN"
        print_test_summary
        return
    fi

    ROOT="$(mktemp -d)"
    local data_dir="$ROOT/data"
    local app_dir="$ROOT/apps"
    local old_app="$app_dir/Bifrost.app"
    local package="$ROOT/package/Bifrost.app"
    local home_dir="$ROOT/home"
    local cli_dir="$home_dir/.local/bin"
    local cli_log="$ROOT/cli-invocation.log"
    local install_bin="$ROOT/core/bifrost"
    local target_version="99.0.1"
    mkdir -p "$data_dir" "$cli_dir" "$(dirname "$install_bin")"
    cp "$BIFROST_BIN" "$install_bin"
    chmod +x "$install_bin"
    write_app_fixture "$old_app" "0.0.1"
    write_app_fixture "$package" "$target_version"

    cat >"$cli_dir/bifrost" <<'SH'
#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'bifrost %s\n' "$BIFROST_TEST_CLI_VERSION"
  exit 0
fi
printf 'args=%s\nskip_app=%s\nskip_restart=%s\ntarget=%s\n' "$*" \
  "$BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP" \
  "$BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART" \
  "$BIFROST_DESKTOP_MANAGED_UPGRADE_TARGET_VERSION" \
  >"$BIFROST_TEST_CLI_LOG"
exit 0
SH
    chmod +x "$cli_dir/bifrost"

    python3 - "$data_dir/version_cache.json" "$target_version" <<'PY'
import datetime, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps({
    "latest_version": sys.argv[2],
    "release_highlights": ["app-owned upgrade e2e"],
    "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}))
PY

    PORT="$(allocate_free_port)"
    BIFROST_DATA_DIR="$data_dir" \
    BIFROST_DESKTOP_CORE=1 \
    BIFROST_APP_INSTALL_DIR="$app_dir" \
    BIFROST_APP_UPGRADE_TEST_PACKAGE="$package" \
    BIFROST_TEST_CLI_LOG="$cli_log" \
    BIFROST_TEST_CLI_VERSION="$target_version" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    HOME="$home_dir" \
    PATH="$cli_dir:/usr/bin:/bin" \
    "$install_bin" start -y --skip-cert-check -p "$PORT" --host 127.0.0.1 \
        --access-mode allow_all --no-system-proxy --no-intercept \
        >"$ROOT/core.log" 2>&1 &
    CORE_PID=$!

    if wait_admin_ready && kill -0 "$CORE_PID" 2>/dev/null; then
        _log_pass "App-owned core starts in foreground and serves Admin API"
    else
        _log_fail "App-owned core starts" "ready" "$(cat "$ROOT/core.log" 2>/dev/null)"
        print_test_summary
        return
    fi

    local runtime_mode
    runtime_mode="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' "$data_dir/runtime.json")"
    assert_equals "desktop" "$runtime_mode" "runtime marker records desktop ownership"

    local version_response current_version has_update
    version_response="$(admin_curl GET '/api/system/version-check?channel=desktop')"
    current_version="$(printf '%s' "$version_response" | json_field current_version)"
    has_update="$(printf '%s' "$version_response" | json_field has_update)"
    assert_equals "0.0.1" "$current_version" "desktop version check uses installed App version"
    assert_equals "True" "$has_update" "desktop version check reports the App update"

    local cli_version_response cli_current_version binary_version
    cli_version_response="$(admin_curl GET '/api/system/version-check?channel=cli')"
    cli_current_version="$(printf '%s' "$cli_version_response" | json_field current_version)"
    binary_version="$("$install_bin" --version | awk '{print $2}')"
    assert_equals "$binary_version" "$cli_current_version" "CLI version check remains component-specific"

    local browser_status browser_body pre_upgrade_version
    browser_status="$(env NO_PROXY='*' no_proxy='*' curl -sS \
        -o "$ROOT/browser-upgrade.json" -w '%{http_code}' -X POST \
        --connect-timeout 2 --max-time 15 \
        "http://127.0.0.1:${PORT}/_bifrost/api/system/upgrade?channel=cli")"
    browser_body="$(cat "$ROOT/browser-upgrade.json")"
    assert_equals "409" "$browser_status" "browser caller cannot start a desktop-owned restart handoff"
    [[ "$browser_body" == *"must be upgraded from the Bifrost desktop app"* ]] \
        && _log_pass "browser rejection explains the desktop-shell ownership boundary" \
        || _log_fail "browser rejection is actionable" "desktop app guidance" "$browser_body"
    pre_upgrade_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$old_app/Contents/Info.plist")"
    assert_equals "0.0.1" "$pre_upgrade_version" "rejected browser request does not mutate the App"
    [[ ! -e "$cli_log" ]] \
        && _log_pass "rejected browser request does not launch the standalone CLI" \
        || _log_fail "rejected browser request does not launch CLI" "no invocation log" "$(cat "$cli_log")"

    local start_response source
    start_response="$(admin_curl POST '/api/system/upgrade?channel=desktop')"
    source="$(printf '%s' "$start_response" | json_field source)"
    assert_equals "desktop" "$source" "desktop shell dispatches the desktop orchestrator"

    local phase=""
    for _ in $(seq 1 200); do
        local progress
        progress="$(admin_curl GET /api/system/upgrade/progress || true)"
        phase="$(printf '%s' "$progress" | json_field phase 2>/dev/null || true)"
        [[ "$phase" == "restarting" || "$phase" == "failed" ]] && break
        sleep 0.1
    done
    assert_equals "restarting" "$phase" "App-owned unified upgrade waits for the Tauri handoff"

    local installed_version
    installed_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$old_app/Contents/Info.plist")"
    assert_equals "$target_version" "$installed_version" "App package is replaced with the target version"
    if [[ -s "$cli_log" ]]; then
        _log_pass "App orchestrator invokes the standalone CLI upgrade"
    else
        _log_fail "App orchestrator invokes the standalone CLI upgrade" "non-empty log" "missing"
    fi
    local cli_invocation
    cli_invocation="$(cat "$cli_log" 2>/dev/null)"
    [[ "$cli_invocation" == *"args=upgrade -y"* ]] \
        && _log_pass "standalone CLI receives the upgrade command" \
        || _log_fail "standalone CLI receives the upgrade command" "upgrade -y" "$cli_invocation"
    [[ "$cli_invocation" == *"skip_app=1"* && "$cli_invocation" == *"skip_restart=1"* ]] \
        && _log_pass "desktop-managed CLI child cannot recurse or restart App core" \
        || _log_fail "desktop-managed CLI child is isolated" "both ownership flags" "$cli_invocation"
    [[ "$cli_invocation" == *"target=$target_version"* ]] \
        && _log_pass "desktop-managed CLI child is pinned to the App target version" \
        || _log_fail "desktop-managed CLI child target is pinned" "$target_version" "$cli_invocation"

    if kill -0 "$CORE_PID" 2>/dev/null && wait_admin_ready; then
        _log_pass "App-owned core remains alive until the Tauri handoff owns restart"
    else
        _log_fail "App-owned core remains alive" "same PID alive" "core exited"
    fi

    print_test_summary
}

main "$@"
