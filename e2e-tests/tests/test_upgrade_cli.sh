#!/bin/bash
#
# Bifrost Upgrade CLI 端到端测试
# 测试版本升级命令和版本检测功能
#

set -uo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY
# The E2E process must exercise a normal interactive CLI. Agent/IM gateway
# parents intentionally mark their children as external workers, which would
# redirect upgrade requests to the developer's already-running service instead
# of the isolated fixture created below.
unset BIFROST_EXTERNAL_CLI_WORKER
unset BIFROST_DETACHED_DAEMON_CHILD

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

: "${BIFROST_BIN:=${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
TEST_PROXY_PID=""
TEST_HTTP_PID=""

PASSED=0
FAILED=0
SKIPPED=0

header() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
}

info() {
    echo -e "${CYAN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

pass() {
    echo -e "  ${GREEN}✓${NC} $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    FAILED=$((FAILED + 1))
}

skip() {
    echo -e "  ${YELLOW}○${NC} $1 (skipped)"
    SKIPPED=$((SKIPPED + 1))
}

cleanup() {
    if [[ -n "$TEST_PROXY_PID" ]] && kill -0 "$TEST_PROXY_PID" 2>/dev/null; then
        kill -TERM "$TEST_PROXY_PID" 2>/dev/null || true
    fi
    if [[ -n "$TEST_HTTP_PID" ]] && kill -0 "$TEST_HTTP_PID" 2>/dev/null; then
        kill -TERM "$TEST_HTTP_PID" 2>/dev/null || true
    fi
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}

trap cleanup EXIT

check_dependencies() {
    header "检查依赖"

    if ! command -v curl &> /dev/null; then
        error "缺少依赖: curl"
        exit 1
    fi
    if ! command -v python3 &> /dev/null; then
        error "缺少依赖: python3"
        exit 1
    fi

    echo -e "${GREEN}✓${NC} 依赖检查通过"
}

build_bifrost() {
    header "检查 Bifrost 二进制"

    if [[ ! -f "$BIFROST_BIN" ]]; then
        error "二进制文件不存在: $BIFROST_BIN"
        exit 1
    fi
}

setup_test_data_dir() {
    TEST_DATA_DIR=$(mktemp -d)
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    mkdir -p "${TEST_DATA_DIR}"
    info "测试数据目录: $TEST_DATA_DIR"
}

test_upgrade_help() {
    header "测试 upgrade --help"

    local result
    result=$("$BIFROST_BIN" upgrade --help 2>&1 || true)

    local missing=()
    [[ "$result" == *"Upgrade bifrost to the latest version"* || \
       "$result" == *"upgrade bifrost to the latest version"* ]] || missing+=(description)
    [[ "$result" != *"--yes"* && "$result" != *"-y"* ]] || missing+=(removed-yes)
    [[ "$result" == *"--help"* || "$result" == *"-h"* ]] || missing+=(help-option)
    [[ "$result" == *"--local-assets"* ]] || missing+=(local-assets)
    [[ "$result" != *"--restart"* ]] || missing+=(removed-restart)

    if [[ ${#missing[@]} -eq 0 ]]; then
        pass "upgrade --help 显示本地制品入口且不包含 -y/--yes 或 --restart"
    else
        fail "upgrade --help 断言失败 (missing: $(IFS=,; echo "${missing[*]}")): $result"
    fi
}

test_upgrade_check_output() {
    header "测试 upgrade 检查更新输出"

    cd "$PROJECT_DIR"

    local result
    result=$("$BIFROST_BIN" upgrade 2>&1 || true)

    if echo "$result" | grep -qi "checking for updates\|latest version\|already on the latest\|already up to date\|could not check"; then
        pass "upgrade 命令正确检查更新"
    else
        fail "upgrade 命令输出异常: $result"
    fi
}

test_upgrade_restart_flag_removed() {
    header "测试 upgrade --restart 参数已移除"

    cd "$PROJECT_DIR"

    local result
    result=$("$BIFROST_BIN" upgrade --restart 2>&1 || true)

    if echo "$result" | grep -qi "unexpected argument.*--restart\|unrecognized option.*--restart\|error:.*--restart"; then
        pass "upgrade --restart 参数已被拒绝"
    else
        fail "upgrade --restart 应被拒绝: $result"
    fi
}

test_upgrade_invalid_flag() {
    header "测试 upgrade --invalid-flag"

    cd "$PROJECT_DIR"

    local result
    result=$("$BIFROST_BIN" upgrade --invalid-flag 2>&1 || true)
    local exit_code=$?

    if echo "$result" | grep -qi "error\|unexpected\|unknown\|unrecognized"; then
        pass "无效参数返回错误信息"
    else
        fail "无效参数未返回错误: exit_code=$exit_code, result=$result"
    fi
}

test_version_cache_creation() {
    header "测试版本缓存创建"

    setup_test_data_dir

    BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_FORCE_UPDATE_CHECK=1 "$BIFROST_BIN" status >/dev/null 2>&1 || true

    sleep 2

    local cache_file="${TEST_DATA_DIR}/version_cache.json"

    if [[ -f "$cache_file" ]]; then
        local content
        content=$(cat "$cache_file" 2>/dev/null || echo "")

        if echo "$content" | grep -q "latest_version" && echo "$content" | grep -q "checked_at"; then
            pass "版本缓存文件正确创建"
        else
            fail "版本缓存文件格式错误: $content"
        fi
    else
        skip "版本缓存文件未创建 (可能网络不可用)"
    fi
}

test_version_cache_content() {
    header "测试版本缓存内容"

    if [[ -z "$TEST_DATA_DIR" ]] || [[ ! -d "$TEST_DATA_DIR" ]]; then
        setup_test_data_dir
    fi

    local cache_file="${TEST_DATA_DIR}/version_cache.json"

    cat > "$cache_file" << 'EOF'
{
  "latest_version": "99.0.0",
  "release_highlights": [],
  "checked_at": "2099-12-31T23:59:59Z"
}
EOF

    BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_FORCE_UPDATE_CHECK=1 "$BIFROST_BIN" status >/dev/null 2>&1 || true

    local content
    content=$(cat "$cache_file" 2>/dev/null || echo "")

    if echo "$content" | grep -q "99.0.0"; then
        pass "版本缓存正确读取和使用"
    else
        fail "版本缓存未被正确使用: $content"
    fi
}

test_new_version_notice() {
    header "测试新版本提示显示"

    if [[ -z "$TEST_DATA_DIR" ]] || [[ ! -d "$TEST_DATA_DIR" ]]; then
        setup_test_data_dir
    fi

    local cache_file="${TEST_DATA_DIR}/version_cache.json"

    cat > "$cache_file" << 'EOF'
{
  "latest_version": "99.0.0",
  "release_highlights": [],
  "checked_at": "2099-12-31T23:59:59Z"
}
EOF

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_FORCE_UPDATE_CHECK=1 "$BIFROST_BIN" status 2>&1 | cat -v || true)

    local checks=0

    if echo "$result" | grep -iq "new version\|A new version"; then
        checks=$((checks + 1))
    fi

    if echo "$result" | grep -q "99\.0\.0"; then
        checks=$((checks + 1))
    fi

    if echo "$result" | grep -iq "bifrost upgrade"; then
        checks=$((checks + 1))
    fi

    if [[ $checks -ge 2 ]]; then
        pass "新版本提示正确显示 ($checks/3)"
    else
        local first_lines
        first_lines=$(echo "$result" | head -20)
        fail "新版本提示显示不完整 ($checks/3), 输出前 20 行: $first_lines"
    fi
}

test_no_notice_when_current() {
    header "测试当前版本时不显示提示"

    if [[ -z "$TEST_DATA_DIR" ]] || [[ ! -d "$TEST_DATA_DIR" ]]; then
        setup_test_data_dir
    fi

    local current_version
    current_version=$("$BIFROST_BIN" --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' | head -1 || echo "0.0.1")

    local cache_file="${TEST_DATA_DIR}/version_cache.json"

    cat > "$cache_file" << EOF
{
  "latest_version": "${current_version}",
  "release_highlights": [],
  "checked_at": "2099-12-31T23:59:59Z"
}
EOF

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_FORCE_UPDATE_CHECK=1 "$BIFROST_BIN" status 2>&1 || true)

    if echo "$result" | grep -iq "new version"; then
        fail "当版本相同时不应显示更新提示"
    else
        pass "当版本相同时正确隐藏更新提示"
    fi
}

create_fake_macos_app() {
    local app_path="$1"
    local version="$2"
    mkdir -p "$app_path/Contents/MacOS"
    cat > "$app_path/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Bifrost</string>
  <key>CFBundleIdentifier</key><string>dev.bifrost.test</string>
  <key>CFBundleName</key><string>Bifrost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
</dict>
</plist>
EOF
    cat > "$app_path/Contents/MacOS/Bifrost" <<'EOF'
#!/bin/sh
exit 0
EOF
    chmod +x "$app_path/Contents/MacOS/Bifrost"
}

test_upgrade_preserves_already_current_desktop_app() {
    header "测试 upgrade 保留已是目标版本的桌面 App"

    if [[ "$(uname -s)" != "Darwin" ]]; then
        skip "桌面 App 自动更新 best-effort 回归当前仅在 macOS 临时 .app 上执行"
        return
    fi

    local current_version
    current_version=$("$BIFROST_BIN" --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' | head -1 || echo "0.0.1")

    local root app_dir
    root=$(mktemp -d)
    app_dir="$root/app-dir"
    create_fake_macos_app "$app_dir/Bifrost.app" "$current_version"

    local result status
    set +e
    result=$(BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_APP_SKIP_RESTART=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" upgrade 2>&1)
    status=$?
    set +e

    rm -rf "$root"

    if [[ $status -ne 0 ]]; then
        fail "upgrade 已最新 CLI 时不应因桌面 App 后置更新失败退出: $result"
        return
    fi

    if echo "$result" | grep -q "Detected installed Bifrost desktop app" \
        && echo "$result" | grep -q "already on target version" \
        && echo "$result" | grep -q "installation and process state unchanged" \
        && ! echo "$result" | grep -q "Bifrost desktop app updated successfully"; then
        pass "upgrade 发现已是目标版本的 App 后不启动伴随更新"
    else
        fail "upgrade 未正确保留已是目标版本的桌面 App: $result"
    fi
}

test_upgrade_desktop_app_failure_does_not_fail_cli_flow() {
    header "测试桌面 App 更新失败不阻断 upgrade 主流程"

    if [[ "$(uname -s)" != "Darwin" ]]; then
        skip "桌面 App 更新失败 best-effort 回归当前仅在 macOS 临时 .app 上执行"
        return
    fi

    local current_version
    current_version=$("$BIFROST_BIN" --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' | head -1 || echo "0.0.1")

    local root app_dir stale_app
    root=$(mktemp -d)
    app_dir="$root/app-dir"
    stale_app="$root/stale/Bifrost.app"
    create_fake_macos_app "$app_dir/Bifrost.app" "0.0.1"
    create_fake_macos_app "$stale_app" "0.0.1"

    local result status
    set +e
    result=$(BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_APP_UPGRADE_TEST_PACKAGE="$stale_app" \
        BIFROST_APP_SKIP_RESTART=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" upgrade 2>&1)
    status=$?
    set +e

    rm -rf "$root"

    if [[ $status -ne 0 ]]; then
        fail "桌面 App 更新失败不应导致 upgrade 退出失败: $result"
        return
    fi

    if echo "$result" | grep -q "Bifrost desktop app update failed; continuing CLI upgrade" \
        && echo "$result" | grep -q "reports version v0.0.1 instead of target"; then
        pass "桌面 App 更新失败时输出原因且继续主流程"
    else
        fail "桌面 App 更新失败 warning 不完整: $result"
    fi
}

test_upgrade_streams_desktop_installer_progress() {
    header "测试 upgrade 实时展示桌面下载和安装进度"

    if [[ "$(uname -s)" != "Darwin" ]]; then
        skip "桌面下载/安装进度回归当前使用 macOS 临时 DMG、.app 与 ditto"
        return
    fi

    local current_version root app_dir package_dir dmg_path fake_bin log_file upgrade_pid status
    local http_port http_pid http_server_log server_error
    local observed_download_while_running="false"
    local observed_installer_while_running="false"
    current_version=$("$BIFROST_BIN" --version 2>&1 \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' \
        | head -1)
    root=$(mktemp -d)
    TEST_DATA_DIR="$root"
    app_dir="$root/app-dir"
    package_dir="$root/package/Bifrost.app"
    dmg_path="$root/bifrost-desktop-fixture.dmg"
    fake_bin="$root/bin"
    log_file="$root/upgrade.log"
    http_server_log="$root/http-server.log"
    mkdir -p "$fake_bin" "$root/data"
    create_fake_macos_app "$app_dir/Bifrost.app" "0.0.1"
    create_fake_macos_app "$package_dir" "$current_version"
    mkdir -p "$package_dir/Contents/Resources"
    dd if=/dev/urandom of="$package_dir/Contents/Resources/download-fixture.bin" \
        bs=1024 count=512 >/dev/null 2>&1
    if ! hdiutil create -quiet -volname BifrostUpgradeFixture -srcfolder "$root/package" \
        -ov -format UDZO "$dmg_path"; then
        rm -rf "$root"
        TEST_DATA_DIR=""
        fail "无法创建本地 Desktop DMG 下载 fixture"
        return
    fi
    cat > "$fake_bin/ditto" <<'EOF'
#!/bin/sh
echo "BIFROST_TEST_INSTALLER_PROGRESS 10%"
sleep 2
exec /usr/bin/ditto "$@"
EOF
    chmod +x "$fake_bin/ditto"

    python3 - "$dmg_path" "$root/http-ready" "$root/http-done" \
        >"$http_server_log" 2>&1 <<'PY' &
import http.server
import pathlib
import sys
import time

payload_path = pathlib.Path(sys.argv[1])
ready_path = pathlib.Path(sys.argv[2])
done_path = pathlib.Path(sys.argv[3])
payload = payload_path.read_bytes()

class SlowDownload(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/x-apple-diskimage")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        for offset in range(0, len(payload), 4096):
            self.wfile.write(payload[offset:offset + 4096])
            self.wfile.flush()
            time.sleep(0.02)
        done_path.write_text("done", encoding="utf-8")

    def log_message(self, *_args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), SlowDownload)
ready_path.write_text(str(server.server_address[1]), encoding="utf-8")
server.handle_request()
server.server_close()
PY
    http_pid=$!
    TEST_HTTP_PID="$http_pid"
    local attempt
    for ((attempt = 0; attempt < 600; attempt++)); do
        [[ -s "$root/http-ready" ]] && break
        kill -0 "$http_pid" 2>/dev/null || break
        sleep 0.05
    done
    if [[ ! -s "$root/http-ready" ]]; then
        server_error=$(tail -20 "$http_server_log" 2>/dev/null || true)
        kill -TERM "$http_pid" 2>/dev/null || true
        wait "$http_pid" 2>/dev/null || true
        TEST_HTTP_PID=""
        rm -rf "$root"
        TEST_DATA_DIR=""
        fail "本地 Desktop 慢速下载服务未就绪${server_error:+: $server_error}"
        return
    fi
    http_port=$(tr -d '[:space:]' < "$root/http-ready")
    if [[ ! "$http_port" =~ ^[0-9]+$ ]]; then
        kill -TERM "$http_pid" 2>/dev/null || true
        wait "$http_pid" 2>/dev/null || true
        TEST_HTTP_PID=""
        rm -rf "$root"
        TEST_DATA_DIR=""
        fail "本地 Desktop 慢速下载服务返回无效端口: $http_port"
        return
    fi

    PATH="$fake_bin:$PATH" \
    BIFROST_DATA_DIR="$root/data" \
    BIFROST_APP_INSTALL_DIR="$app_dir" \
    BIFROST_APP_UPGRADE_TEST_URL="http://127.0.0.1:$http_port/bifrost-desktop.dmg" \
    BIFROST_APP_SKIP_RESTART=1 \
    BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
    NO_PROXY="127.0.0.1,localhost" \
    no_proxy="127.0.0.1,localhost" \
    HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
    https_proxy= http_proxy= all_proxy= \
        "$BIFROST_BIN" upgrade >"$log_file" 2>&1 &
    upgrade_pid=$!
    TEST_PROXY_PID="$upgrade_pid"

    for ((attempt = 0; attempt < 200; attempt++)); do
        if grep -q "Downloading…" "$log_file" 2>/dev/null; then
            if kill -0 "$upgrade_pid" 2>/dev/null && [[ ! -f "$root/http-done" ]]; then
                observed_download_while_running="true"
            fi
            break
        fi
        sleep 0.05
    done
    for ((attempt = 0; attempt < 200; attempt++)); do
        if grep -q "BIFROST_TEST_INSTALLER_PROGRESS 10%" "$log_file" 2>/dev/null; then
            if kill -0 "$upgrade_pid" 2>/dev/null; then
                observed_installer_while_running="true"
            fi
            break
        fi
        sleep 0.05
    done

    set +e
    wait "$upgrade_pid"
    status=$?
    TEST_PROXY_PID=""
    if kill -0 "$http_pid" 2>/dev/null; then
        kill -TERM "$http_pid" 2>/dev/null || true
    fi
    wait "$http_pid" 2>/dev/null || true
    TEST_HTTP_PID=""
    local output installed_version download_line
    output=$(cat "$log_file")
    installed_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
        "$app_dir/Bifrost.app/Contents/Info.plist" 2>/dev/null || true)
    download_line=$(printf '%s' "$output" | tr '\r' '\n' | grep 'Downloading…' | tail -1 || true)
    rm -rf "$root"
    TEST_DATA_DIR=""

    if [[ $status -eq 0 \
        && "$observed_download_while_running" == "true" \
        && "$observed_installer_while_running" == "true" \
        && "$installed_version" == "$current_version" \
        && "$output" == *"Downloading…"* \
        && "$output" == *"/s)"* \
        && "$output" == *"Installing desktop app..."* \
        && "$output" == *"BIFROST_TEST_INSTALLER_PROGRESS 10%"* \
        && "$output" == *"Desktop app installed successfully"* \
        && "$output" == *"Bifrost desktop app updated successfully"* ]]; then
        pass "父 upgrade 在本地 DMG 下载和子安装仍运行时已实时显示进度，并把临时 App 更新到 v${current_version}；$download_line"
    else
        fail "桌面下载/安装进度未实时转发: status=$status download_live=$observed_download_while_running installer_live=$observed_installer_while_running installed=$installed_version expected=$current_version output=$output"
    fi
}

get_free_tcp_port() {
    python3 - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

read_runtime_pid() {
    local runtime_file="$1"
    python3 - "$runtime_file" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    print(json.loads(path.read_text())["pid"])
except (FileNotFoundError, KeyError, json.JSONDecodeError):
    pass
PY
}

wait_for_admin_ready() {
    local port="$1"
    local attempts="${2:-100}"
    local i
    for ((i = 0; i < attempts; i++)); do
        if curl -fsS "http://127.0.0.1:${port}/_bifrost/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

test_admin_self_update_recovers_missing_runtime_markers() {
    header "测试 Web UI self-update 在 runtime marker 丢失时仍重启旧 daemon"

    case "$(uname -s)" in
        Darwin|Linux) ;;
        *)
            skip "runtime marker 恢复 E2E 当前使用 Unix daemon 信号链路"
            return
            ;;
    esac

    local root data_dir app_dir port current_version start_output old_pid update_output update_status new_pid progress_phase
    root=$(mktemp -d)
    data_dir="$root/data"
    app_dir="$root/no-installed-app"
    port=$(get_free_tcp_port)
    current_version=$(
        "$BIFROST_BIN" --version 2>&1 \
            | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' \
            | head -1
    )
    mkdir -p "$data_dir" "$app_dir"

    start_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        "$BIFROST_BIN" start -d -y --skip-cert-check -p "$port" \
        --host 127.0.0.1 --access-mode allow_all --no-system-proxy --no-intercept 2>&1)

    if ! wait_for_admin_ready "$port"; then
        rm -rf "$root"
        fail "测试 daemon 未 ready: $start_output"
        return
    fi

    old_pid=$(read_runtime_pid "$data_dir/runtime.json")
    TEST_PROXY_PID="$old_pid"
    if [[ -z "$old_pid" ]] || ! kill -0 "$old_pid" 2>/dev/null; then
        rm -rf "$root"
        TEST_PROXY_PID=""
        fail "无法读取测试 daemon PID"
        return
    fi

    rm -f "$data_dir/runtime.json" "$data_dir/bifrost.pid"
    if ! kill -0 "$old_pid" 2>/dev/null || ! wait_for_admin_ready "$port" 5; then
        kill -TERM "$old_pid" 2>/dev/null || true
        rm -rf "$root"
        TEST_PROXY_PID=""
        fail "删除 runtime marker 后测试 daemon 不应退出"
        return
    fi

    set +e
    update_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" self-update --target "$current_version" --source admin \
        --running-proxy-pid "$old_pid" --running-proxy-port "$port" 2>&1)
    update_status=$?
    set +e

    wait_for_admin_ready "$port" || true
    new_pid=$(read_runtime_pid "$data_dir/runtime.json")
    progress_phase=$(python3 - "$data_dir/upgrade-progress.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    print(json.loads(path.read_text())["phase"])
except (FileNotFoundError, KeyError, json.JSONDecodeError):
    pass
PY
)

    if [[ -n "$new_pid" ]]; then
        TEST_PROXY_PID="$new_pid"
    fi
    BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    if [[ -n "$TEST_PROXY_PID" ]] && kill -0 "$TEST_PROXY_PID" 2>/dev/null; then
        kill -TERM "$TEST_PROXY_PID" 2>/dev/null || true
    fi
    TEST_PROXY_PID=""
    rm -rf "$root"

    if [[ $update_status -eq 0 \
        && -n "$new_pid" \
        && "$new_pid" != "$old_pid" \
        && "$progress_phase" == "completed" \
        && "$update_output" == *"Recovered missing runtime markers"* \
        && "$update_output" == *"Proxy restarted successfully with the new version"* ]]; then
        pass "Admin 传入的 PID/端口可恢复 runtime marker 并完成旧 daemon 重启"
    else
        fail "runtime marker 恢复链路失败: status=$update_status old=$old_pid new=$new_pid phase=$progress_phase output=$update_output"
    fi
}

test_admin_self_update_converts_cli_foreground_to_restarted_daemon() {
    header "测试 Web UI self-update 将 CLI 前台 core 安全接续为新 daemon"

    case "$(uname -s)" in
        Darwin|Linux) ;;
        *)
            skip "CLI foreground handoff E2E 当前使用 Unix 信号链路"
            return
            ;;
    esac

    local root data_dir app_dir port current_version old_pid update_output update_status new_pid runtime_mode
    root=$(mktemp -d)
    data_dir="$root/data"
    app_dir="$root/no-installed-app"
    port=$(get_free_tcp_port)
    current_version=$("$BIFROST_BIN" --version 2>&1 \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' \
        | head -1)
    mkdir -p "$data_dir" "$app_dir"

    BIFROST_DATA_DIR="$data_dir" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    "$BIFROST_BIN" start -y --skip-cert-check -p "$port" \
        --host 127.0.0.1 --access-mode allow_all --no-system-proxy --no-intercept \
        >"$root/core.log" 2>&1 &
    old_pid=$!
    TEST_PROXY_PID="$old_pid"

    if ! wait_for_admin_ready "$port" || ! kill -0 "$old_pid" 2>/dev/null; then
        kill -TERM "$old_pid" 2>/dev/null || true
        rm -rf "$root"
        TEST_PROXY_PID=""
        fail "CLI foreground 测试 core 未 ready"
        return
    fi

    runtime_mode=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' "$data_dir/runtime.json")
    if [[ "$runtime_mode" != "foreground" ]]; then
        kill -TERM "$old_pid" 2>/dev/null || true
        rm -rf "$root"
        TEST_PROXY_PID=""
        fail "CLI core 应记录 foreground ownership，实际为 $runtime_mode"
        return
    fi

    set +e
    update_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" self-update --target "$current_version" --source admin \
        --running-proxy-pid "$old_pid" --running-proxy-port "$port" 2>&1)
    update_status=$?
    set +e

    wait_for_admin_ready "$port" || true
    new_pid=$(read_runtime_pid "$data_dir/runtime.json")
    runtime_mode=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' "$data_dir/runtime.json" 2>/dev/null || true)
    if [[ -n "$new_pid" ]]; then
        TEST_PROXY_PID="$new_pid"
    fi
    wait "$old_pid" 2>/dev/null || true
    BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    if [[ -n "$TEST_PROXY_PID" ]] && kill -0 "$TEST_PROXY_PID" 2>/dev/null; then
        kill -TERM "$TEST_PROXY_PID" 2>/dev/null || true
    fi
    TEST_PROXY_PID=""
    rm -rf "$root"

    if [[ $update_status -eq 0 \
        && -n "$new_pid" \
        && "$new_pid" != "$old_pid" \
        && "$runtime_mode" == "daemon" \
        && "$update_output" == *"Proxy restarted successfully with the new version"* ]]; then
        pass "CLI foreground core 由 updater 接续为新版本 daemon，且未被误判为 App-owned"
    else
        fail "CLI foreground handoff 失败: status=$update_status old=$old_pid new=$new_pid mode=$runtime_mode output=$update_output"
    fi
}

test_admin_self_update_restarts_core_after_companion_app_failure() {
    header "测试 CLI-owned 后台升级在 App 伴随更新失败时仍重启 core 且不假完成"

    if [[ "$(uname -s)" != "Darwin" ]]; then
        skip "App 失败后的 core 重启回归当前仅在 macOS 临时 .app 上执行"
        return
    fi

    local root data_dir app_dir stale_app port current_version start_output old_pid update_output update_status runtime_pid progress_phase progress_error
    root=$(mktemp -d)
    data_dir="$root/data"
    app_dir="$root/app-dir"
    stale_app="$root/stale/Bifrost.app"
    port=$(get_free_tcp_port)
    current_version=$("$BIFROST_BIN" --version 2>&1 \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' \
        | head -1)
    mkdir -p "$data_dir" "$app_dir"
    create_fake_macos_app "$app_dir/Bifrost.app" "0.0.1"
    create_fake_macos_app "$stale_app" "0.0.1"

    start_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        "$BIFROST_BIN" start -d -y --skip-cert-check -p "$port" \
        --host 127.0.0.1 --access-mode allow_all --no-system-proxy --no-intercept 2>&1)
    if ! wait_for_admin_ready "$port"; then
        rm -rf "$root"
        fail "App 失败后的 core 重启测试 daemon 未 ready: $start_output"
        return
    fi
    old_pid=$(read_runtime_pid "$data_dir/runtime.json")
    TEST_PROXY_PID="$old_pid"

    set +e
    update_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_APP_UPGRADE_TEST_PACKAGE="$stale_app" \
        BIFROST_APP_SKIP_RESTART=1 \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" self-update --target "$current_version" --source admin \
        --running-proxy-pid "$old_pid" --running-proxy-port "$port" 2>&1)
    update_status=$?
    set -e

    runtime_pid=$(read_runtime_pid "$data_dir/runtime.json")
    progress_phase=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["phase"])' "$data_dir/upgrade-progress.json" 2>/dev/null || true)
    progress_error=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error", ""))' "$data_dir/upgrade-progress.json" 2>/dev/null || true)
    local still_ready="false"
    if [[ -n "$runtime_pid" && "$runtime_pid" != "$old_pid" ]] \
        && kill -0 "$runtime_pid" 2>/dev/null \
        && wait_for_admin_ready "$port" 5; then
        still_ready="true"
    fi
    BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    if kill -0 "$old_pid" 2>/dev/null; then
        kill -TERM "$old_pid" 2>/dev/null || true
    fi
    TEST_PROXY_PID=""
    rm -rf "$root"

    if [[ $update_status -ne 0 \
        && -n "$runtime_pid" \
        && "$runtime_pid" != "$old_pid" \
        && "$still_ready" == "true" \
        && "$progress_phase" == "failed" \
        && "$progress_error" == *"desktop app update command failed"* \
        && "$update_output" == *"Proxy restarted successfully with the new version"* ]]; then
        pass "App 伴随更新失败仍会重启 CLI daemon，并向 Web UI 写入 failed 而不假完成"
    else
        fail "App 失败后的 core 重启契约失败: status=$update_status old=$old_pid runtime=$runtime_pid ready=$still_ready phase=$progress_phase error=$progress_error output=$update_output"
    fi
}

test_self_update_does_not_take_over_app_owned_core() {
    header "测试 CLI updater 不接管 App-owned core 重启"

    case "$(uname -s)" in
        Darwin|Linux) ;;
        *)
            skip "App-owned core 所有权 E2E 当前使用 Unix 前台进程链路"
            return
            ;;
    esac

    local root data_dir app_dir port current_version old_pid update_output update_status runtime_pid runtime_mode progress_phase
    root=$(mktemp -d)
    data_dir="$root/data"
    app_dir="$root/no-installed-app"
    port=$(get_free_tcp_port)
    current_version=$("$BIFROST_BIN" --version 2>&1 \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?' \
        | head -1)
    mkdir -p "$data_dir" "$app_dir"

    BIFROST_DATA_DIR="$data_dir" \
    BIFROST_DESKTOP_CORE=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    "$BIFROST_BIN" start -y --skip-cert-check -p "$port" \
        --host 127.0.0.1 --access-mode allow_all --no-system-proxy --no-intercept \
        >"$root/core.log" 2>&1 &
    old_pid=$!
    TEST_PROXY_PID="$old_pid"

    if ! wait_for_admin_ready "$port" || ! kill -0 "$old_pid" 2>/dev/null; then
        kill -TERM "$old_pid" 2>/dev/null || true
        rm -rf "$root"
        TEST_PROXY_PID=""
        fail "App-owned 测试 core 未 ready"
        return
    fi

    set +e
    update_output=$(BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$app_dir" \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        BIFROST_DISABLE_TRAY=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
        "$BIFROST_BIN" self-update --target "$current_version" --source admin 2>&1)
    update_status=$?
    set +e

    runtime_pid=$(read_runtime_pid "$data_dir/runtime.json")
    runtime_mode=$(python3 - "$data_dir/runtime.json" <<'PY'
import json, pathlib, sys
try:
    print(json.loads(pathlib.Path(sys.argv[1]).read_text()).get("runtime_start_mode", ""))
except Exception:
    pass
PY
)
    progress_phase=$(python3 - "$data_dir/upgrade-progress.json" <<'PY'
import json, pathlib, sys
try:
    print(json.loads(pathlib.Path(sys.argv[1]).read_text()).get("phase", ""))
except Exception:
    pass
PY
)

    local still_ready="false"
    if kill -0 "$old_pid" 2>/dev/null && wait_for_admin_ready "$port" 5; then
        still_ready="true"
    fi
    kill -TERM "$old_pid" 2>/dev/null || true
    wait "$old_pid" 2>/dev/null || true
    TEST_PROXY_PID=""
    rm -rf "$root"

    if [[ $update_status -eq 0 \
        && "$still_ready" == "true" \
        && "$runtime_pid" == "$old_pid" \
        && "$runtime_mode" == "desktop" \
        && "$progress_phase" == "completed" \
        && "$update_output" == *"restart to the app handoff"* ]]; then
        pass "CLI updater 更新后保留 App-owned core，由 App handoff 独占重启"
    else
        fail "App-owned core 所有权失败: status=$update_status pid=$old_pid runtime_pid=$runtime_pid mode=$runtime_mode ready=$still_ready phase=$progress_phase output=$update_output"
    fi
}

print_summary() {
    header "测试总结"

    local total=$((PASSED + FAILED + SKIPPED))

    echo -e "  ${GREEN}通过${NC}: $PASSED"
    echo -e "  ${RED}失败${NC}: $FAILED"
    echo -e "  ${YELLOW}跳过${NC}: $SKIPPED"
    echo -e "  ${BLUE}总计${NC}: $total"
    echo ""

    if [[ $FAILED -eq 0 ]]; then
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${GREEN}  所有测试通过！${NC}"
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
        return 0
    else
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${RED}  $FAILED 个测试失败${NC}"
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        return 1
    fi
}

show_help() {
    cat << EOF
用法: $0 [选项]

Bifrost Upgrade CLI 端到端测试

选项:
  -h, --help      显示帮助信息
  --no-build      跳过编译步骤
  --only-runtime-marker
                  只执行 Admin runtime marker 恢复回归
  --only-runtime-ownership
                  只执行 CLI-owned / App-owned 重启所有权回归
  --only-progress-streaming
                  只执行 Desktop 下载/安装进度实时转发回归
  --verbose       详细输出

环境变量:
  BIFROST_DATA_DIR  数据目录 (默认: 临时目录)

示例:
  $0                    # 运行所有测试
  $0 --no-build         # 跳过编译
EOF
}

SKIP_BUILD="false"
ONLY_RUNTIME_MARKER="false"
ONLY_RUNTIME_OWNERSHIP="false"
ONLY_PROGRESS_STREAMING="false"
VERBOSE="false"

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        --no-build)
            SKIP_BUILD="true"
            shift
            ;;
        --only-runtime-marker)
            ONLY_RUNTIME_MARKER="true"
            shift
            ;;
        --only-runtime-ownership)
            ONLY_RUNTIME_OWNERSHIP="true"
            shift
            ;;
        --only-progress-streaming)
            ONLY_PROGRESS_STREAMING="true"
            shift
            ;;
        --verbose)
            VERBOSE="true"
            shift
            ;;
        *)
            error "未知选项: $1"
            show_help
            exit 1
            ;;
    esac
done

main() {
    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║          Bifrost Upgrade CLI 端到端测试                       ║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"

    check_dependencies
    build_bifrost

    if [[ "$ONLY_RUNTIME_MARKER" == "true" ]]; then
        test_admin_self_update_recovers_missing_runtime_markers
        print_summary
        return
    fi

    if [[ "$ONLY_RUNTIME_OWNERSHIP" == "true" ]]; then
        test_admin_self_update_recovers_missing_runtime_markers
        test_admin_self_update_converts_cli_foreground_to_restarted_daemon
        test_admin_self_update_restarts_core_after_companion_app_failure
        test_self_update_does_not_take_over_app_owned_core
        print_summary
        return
    fi

    if [[ "$ONLY_PROGRESS_STREAMING" == "true" ]]; then
        test_upgrade_streams_desktop_installer_progress
        print_summary
        return
    fi

    test_upgrade_help
    test_upgrade_check_output
    test_upgrade_restart_flag_removed
    test_upgrade_invalid_flag

    test_version_cache_creation
    test_version_cache_content
    test_new_version_notice
    test_no_notice_when_current
    test_upgrade_preserves_already_current_desktop_app
    test_upgrade_desktop_app_failure_does_not_fail_cli_flow
    test_upgrade_streams_desktop_installer_progress
    test_admin_self_update_recovers_missing_runtime_markers
    test_admin_self_update_converts_cli_foreground_to_restarted_daemon
    test_admin_self_update_restarts_core_after_companion_app_failure
    test_self_update_does_not_take_over_app_owned_core

    print_summary
}

main
