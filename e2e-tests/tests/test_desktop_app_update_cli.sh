#!/bin/bash
#
# Desktop app update CLI dry-run regression tests.
#
# These checks avoid real app installation and release downloads. They verify
# the command surface and user-visible planning output for app install,
# uninstall and upgrade.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    _log_fail "bifrost binary exists" "target/debug/bifrost or target/release/bifrost" "$BIFROST_BIN"
    exit 1
fi

TEST_ROOT="$(mktemp -d)"
API_DATA_DIR=""
API_PORT=""
cleanup() {
    if [[ -n "$API_DATA_DIR" && -n "$API_PORT" ]]; then
        BIFROST_DATA_DIR="$API_DATA_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

assert_contains_text() {
    local haystack="$1"
    local needle="$2"
    local label="$3"
    if [[ "$haystack" == *"$needle"* ]]; then
        _log_pass "$label"
    else
        _log_fail "$label" "output contains: $needle" "$haystack"
        return 1
    fi
}

assert_not_contains_text() {
    local haystack="$1"
    local needle="$2"
    local label="$3"
    if [[ "$haystack" == *"$needle"* ]]; then
        _log_fail "$label" "output does not contain: $needle" "$haystack"
        return 1
    else
        _log_pass "$label"
    fi
}

run_case() {
    local label="$1"
    shift
    _log_info "$label"
    "$BIFROST_BIN" "$@" 2>&1
}

APP_DIR="${TEST_ROOT}/desktop-app"
VERSION="0.0.139"
DATA_DIR="${TEST_ROOT}/data"
export BIFROST_DATA_DIR="$DATA_DIR"

allocate_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

json_escape() {
    python3 - "$1" <<'PY'
import json, sys
print(json.dumps(sys.argv[1]))
PY
}

read_json_field() {
    local json="$1"
    local field="$2"
    python3 - "$json" "$field" <<'PY'
import json, sys
payload = sys.argv[1]
field = sys.argv[2]
try:
    data = json.loads(payload)
    print(data.get(field, ""))
except Exception:
    print("")
PY
}

install_output="$(run_case "app install dry-run" app install --dry-run --version "$VERSION" --app-dir "$APP_DIR")"
assert_contains_text "$install_output" "Desktop app install target:" "install dry-run prints target path"
assert_contains_text "$install_output" "Target version:" "install dry-run prints target version"
assert_contains_text "$install_output" "Dry run: no files will be changed." "install dry-run does not mutate system"

if [[ -e "$APP_DIR/Bifrost.app" || -e "$APP_DIR/Bifrost.exe" ]]; then
    _log_fail "install dry-run leaves no desktop app artifact" "no Bifrost.app/Bifrost.exe" "artifact exists"
    exit 1
else
    _log_pass "install dry-run leaves no desktop app artifact"
fi

upgrade_output="$(run_case "app upgrade dry-run desktop source" app upgrade --dry-run --source desktop --version "$VERSION" --app-dir "$APP_DIR")"
assert_contains_text "$upgrade_output" "Desktop app upgrade target:" "upgrade dry-run prints target path"
assert_contains_text "$upgrade_output" "Would upgrade CLI with" "desktop upgrade dry-run advertises CLI linkage"
assert_contains_text "$upgrade_output" "Would install desktop package from:" "upgrade dry-run prints package source"
assert_contains_text "$upgrade_output" "Would let the current desktop shell restart" "desktop upgrade dry-run prints shell restart plan"

cli_upgrade_output="$(run_case "app upgrade dry-run CLI source" app upgrade --dry-run --version "$VERSION" --app-dir "$APP_DIR")"
assert_contains_text "$cli_upgrade_output" "Would upgrade CLI with" "CLI upgrade dry-run keeps current CLI update plan"
assert_contains_text "$cli_upgrade_output" "Would restart the desktop app after a successful install." "CLI upgrade dry-run restarts installed desktop app"
assert_not_contains_text "$cli_upgrade_output" "Would let the current desktop shell restart" "CLI upgrade dry-run does not use desktop shell restart plan"

no_cli_output="$(run_case "app upgrade dry-run no-cli" app upgrade --dry-run --source desktop --no-cli --version "$VERSION" --app-dir "$APP_DIR")"
assert_contains_text "$no_cli_output" "Would install desktop package from:" "no-cli upgrade still plans desktop install"
assert_not_contains_text "$no_cli_output" "Would upgrade CLI with" "no-cli upgrade omits CLI linkage"

uninstall_output="$(run_case "app uninstall dry-run" app uninstall --dry-run --app-dir "$APP_DIR")"
assert_contains_text "$uninstall_output" "Desktop app path:" "uninstall dry-run prints target path"
assert_contains_text "$uninstall_output" "Dry run: would remove the desktop app only." "uninstall dry-run scopes removal to app"

API_DATA_DIR="${TEST_ROOT}/api-data"
API_INSTALL_DIR="${TEST_ROOT}/api-cli-bin"
API_PORT="$(allocate_free_port)"
_log_info "app-to-CLI HTTP install endpoint"
BIFROST_DATA_DIR="$API_DATA_DIR" \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
    "$BIFROST_BIN" start --daemon --host 127.0.0.1 -p "$API_PORT" \
    --no-system-proxy --skip-cert-check --unsafe-ssl --access-mode allow_all --yes >/tmp/bifrost-desktop-app-api-start.log 2>&1

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:${API_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
if ! curl -fsS "http://127.0.0.1:${API_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
    _log_fail "temporary Bifrost service starts for CLI install API" "admin API ready" "not ready"
    cat /tmp/bifrost-desktop-app-api-start.log || true
    exit 1
fi
_log_pass "temporary Bifrost service starts for CLI install API"

install_body="{\"install_dir\":$(json_escape "$API_INSTALL_DIR"),\"install_skills\":false}"
api_install_response="$(curl -fsS -X POST \
    -H 'Content-Type: application/json' \
    --data "$install_body" \
    "http://127.0.0.1:${API_PORT}/_bifrost/api/system/cli-install")"
api_installed="$(read_json_field "$api_install_response" installed)"
api_skills_message="$(read_json_field "$api_install_response" skills_message)"
if [[ "$api_installed" == "True" || "$api_installed" == "true" ]]; then
    _log_pass "CLI install API reports installed=true"
else
    _log_fail "CLI install API reports installed=true" "true" "$api_install_response"
    exit 1
fi
assert_contains_text "$api_skills_message" "skipped" "CLI install API can skip AI skill setup for tests"
if [[ -f "$API_INSTALL_DIR/bifrost" || -f "$API_INSTALL_DIR/bifrost.exe" ]]; then
    _log_pass "CLI install API copies binary into requested install dir"
else
    _log_fail "CLI install API copies binary into requested install dir" "bifrost binary exists" "missing"
    exit 1
fi

api_status_response="$(curl -fsS "http://127.0.0.1:${API_PORT}/_bifrost/api/system/cli-install")"
assert_contains_text "$api_status_response" "install_path" "CLI install status endpoint returns install metadata"
BIFROST_DATA_DIR="$API_DATA_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
API_DATA_DIR=""
API_PORT=""

HOST_OS="$(uname -s)"
read_progress_field() {
    local file="$1"
    local field="$2"
    python3 - "$file" "$field" <<'PY'
import json, sys
with open(sys.argv[1]) as fh:
    data = json.load(fh)
print(data.get(sys.argv[2], ""))
PY
}

if [[ "$HOST_OS" == "Darwin" ]]; then
    FAKE_APP="${TEST_ROOT}/fixtures/Bifrost.app"
    mkdir -p "$FAKE_APP/Contents/MacOS"
    cat >"$FAKE_APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Bifrost</string>
  <key>CFBundleIdentifier</key><string>dev.bifrost.test</string>
  <key>CFBundleName</key><string>Bifrost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.139</string>
  <key>CFBundleVersion</key><string>139</string>
</dict>
</plist>
PLIST
    cat >"$FAKE_APP/Contents/MacOS/Bifrost" <<'SH'
#!/bin/sh
exit 0
SH
    chmod +x "$FAKE_APP/Contents/MacOS/Bifrost"

    install_real_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app install --package "$FAKE_APP" --app-dir "$APP_DIR" -y 2>&1)"
    assert_contains_text "$install_real_output" "Desktop app install target:" "real install prints target path"
    if [[ -x "$APP_DIR/Bifrost.app/Contents/MacOS/Bifrost" ]]; then
        _log_pass "real install copies app bundle into target app dir"
    else
        _log_fail "real install copies app bundle into target app dir" "executable copied" "missing"
        exit 1
    fi

    already_current_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app upgrade --app-dir "$APP_DIR" --source desktop --no-cli --version "$VERSION" -y 2>&1)"
    assert_contains_text "$already_current_output" "Desktop app is already on target version" "desktop upgrade skips install when installed app version matches target"
    assert_not_contains_text "$already_current_output" "Downloading desktop app:" "desktop upgrade does not download when installed app version matches target"

    rm -rf "$DATA_DIR"
    mkdir -p "$DATA_DIR"
    upgrade_real_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app upgrade --package "$FAKE_APP" --app-dir "$APP_DIR" --source desktop --no-cli --version "$VERSION" -y 2>&1)"
    assert_contains_text "$upgrade_real_output" "Desktop app upgrade target:" "real upgrade prints target path"
    if [[ -x "$APP_DIR/Bifrost.app/Contents/MacOS/Bifrost" ]]; then
        _log_pass "real upgrade installs app bundle into target app dir"
    else
        _log_fail "real upgrade installs app bundle into target app dir" "executable copied" "missing"
        exit 1
    fi

    progress_file="$DATA_DIR/upgrade-progress.json"
    progress_phase="$(read_progress_field "$progress_file" phase)"
    progress_source="$(read_progress_field "$progress_file" source)"
    [[ "$progress_phase" == "completed" ]] \
        && _log_pass "real desktop upgrade writes completed progress" \
        || { _log_fail "real desktop upgrade writes completed progress" "completed" "$progress_phase"; exit 1; }
    [[ "$progress_source" == "desktop" ]] \
        && _log_pass "real desktop upgrade writes desktop progress source" \
        || { _log_fail "real desktop upgrade writes desktop progress source" "desktop" "$progress_source"; exit 1; }

    uninstall_real_output="$("$BIFROST_BIN" app uninstall --app-dir "$APP_DIR" -y 2>&1)"
    assert_contains_text "$uninstall_real_output" "Desktop app path:" "real uninstall prints target path"
    if [[ ! -e "$APP_DIR/Bifrost.app" ]]; then
        _log_pass "real uninstall removes app bundle from target app dir"
    else
        _log_fail "real uninstall removes app bundle from target app dir" "Bifrost.app removed" "still exists"
        exit 1
    fi
elif [[ "$HOST_OS" == MINGW* || "$HOST_OS" == MSYS* || "$HOST_OS" == CYGWIN* ]]; then
    FAKE_WIN_DIR="${TEST_ROOT}/fixtures/windows"
    FAKE_WIN_ZIP="${TEST_ROOT}/fixtures/bifrost-desktop.zip"
    mkdir -p "$FAKE_WIN_DIR"
    printf 'fake bifrost desktop exe\n' >"$FAKE_WIN_DIR/bifrost-desktop.exe"
    if command -v pwsh >/dev/null 2>&1; then
        pwsh -NoProfile -Command "Compress-Archive -Path '${FAKE_WIN_DIR//\'/\'\'}\\bifrost-desktop.exe' -DestinationPath '${FAKE_WIN_ZIP//\'/\'\'}' -Force"
    elif command -v powershell.exe >/dev/null 2>&1; then
        powershell.exe -NoProfile -Command "Compress-Archive -Path '${FAKE_WIN_DIR}\\bifrost-desktop.exe' -DestinationPath '${FAKE_WIN_ZIP}' -Force"
    else
        _log_warning "Windows zip install coverage skipped because PowerShell is unavailable"
        FAKE_WIN_ZIP=""
    fi

    if [[ -n "$FAKE_WIN_ZIP" && -f "$FAKE_WIN_ZIP" ]]; then
        install_real_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app install --package "$FAKE_WIN_ZIP" --app-dir "$APP_DIR" -y 2>&1)"
        assert_contains_text "$install_real_output" "Desktop app install target:" "real Windows zip install prints target path"
        if [[ -f "$APP_DIR/bifrost-desktop.exe" ]]; then
            _log_pass "real Windows zip install copies bifrost-desktop.exe into target app dir"
        else
            _log_fail "real Windows zip install copies bifrost-desktop.exe into target app dir" "bifrost-desktop.exe copied" "missing"
            exit 1
        fi

        rm -rf "$DATA_DIR"
        mkdir -p "$DATA_DIR"
        upgrade_real_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app upgrade --package "$FAKE_WIN_ZIP" --app-dir "$APP_DIR" --source desktop --no-cli --version "$VERSION" -y 2>&1)"
        assert_contains_text "$upgrade_real_output" "Desktop app upgrade target:" "real Windows zip upgrade prints target path"
        progress_file="$DATA_DIR/upgrade-progress.json"
        progress_phase="$(read_progress_field "$progress_file" phase)"
        progress_source="$(read_progress_field "$progress_file" source)"
        [[ "$progress_phase" == "completed" ]] \
            && _log_pass "real Windows desktop upgrade writes completed progress" \
            || { _log_fail "real Windows desktop upgrade writes completed progress" "completed" "$progress_phase"; exit 1; }
        [[ "$progress_source" == "desktop" ]] \
            && _log_pass "real Windows desktop upgrade writes desktop progress source" \
            || { _log_fail "real Windows desktop upgrade writes desktop progress source" "desktop" "$progress_source"; exit 1; }

        uninstall_real_output="$("$BIFROST_BIN" app uninstall --app-dir "$APP_DIR" -y 2>&1)"
        assert_contains_text "$uninstall_real_output" "Desktop app path:" "real Windows uninstall prints target path"
        if [[ ! -e "$APP_DIR/bifrost-desktop.exe" ]]; then
            _log_pass "real Windows uninstall removes bifrost-desktop.exe from target app dir"
        else
            _log_fail "real Windows uninstall removes bifrost-desktop.exe from target app dir" "bifrost-desktop.exe removed" "still exists"
            exit 1
        fi
    fi

    if [[ -n "${BIFROST_DESKTOP_REAL_MSI:-}" ]]; then
        if [[ ! -f "$BIFROST_DESKTOP_REAL_MSI" ]]; then
            _log_fail "real Windows MSI fixture exists" "$BIFROST_DESKTOP_REAL_MSI" "missing"
            exit 1
        fi
        _log_info "real Windows MSI install/uninstall regression"
        real_msi_install_output="$(BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app install --package "$BIFROST_DESKTOP_REAL_MSI" -y 2>&1)"
        assert_contains_text "$real_msi_install_output" "Desktop app install target:" "real Windows MSI install prints target path"
        WINDOWS_DEFAULT_APP_DIR="${LOCALAPPDATA}\\Bifrost"
        if [[ -f "${WINDOWS_DEFAULT_APP_DIR}\\bifrost-desktop.exe" ]]; then
            _log_pass "real Windows MSI install creates bifrost-desktop.exe in LocalAppData"
        else
            _log_fail "real Windows MSI install creates bifrost-desktop.exe in LocalAppData" "desktop exe exists" "missing"
            exit 1
        fi

        real_msi_uninstall_output="$("$BIFROST_BIN" app uninstall -y 2>&1)"
        assert_contains_text "$real_msi_uninstall_output" "Desktop app path:" "real Windows MSI uninstall prints target path"
        if [[ ! -e "${WINDOWS_DEFAULT_APP_DIR}\\bifrost-desktop.exe" ]]; then
            _log_pass "real Windows MSI uninstall removes bifrost-desktop.exe"
        else
            _log_fail "real Windows MSI uninstall removes bifrost-desktop.exe" "desktop exe removed" "still exists"
            exit 1
        fi
    fi
else
    _log_warning "real app install/upgrade/uninstall skipped on ${HOST_OS}; dry-run coverage still executed"
fi

echo ""
echo "Passed: ${PASSED_ASSERTIONS} / ${TOTAL_ASSERTIONS}"
