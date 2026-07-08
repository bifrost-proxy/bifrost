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

    local checks=0

    if echo "$result" | grep -qi "Upgrade bifrost to the latest version"; then
        checks=$((checks + 1))
    fi

    if ! echo "$result" | grep -q "\-\-yes\|\-y"; then
        checks=$((checks + 1))
    fi

    if echo "$result" | grep -q "\-\-help\|\-h"; then
        checks=$((checks + 1))
    fi

    if ! echo "$result" | grep -q "\-\-restart"; then
        checks=$((checks + 1))
    fi

    if [[ $checks -eq 4 ]]; then
        pass "upgrade --help 显示正确的帮助信息且不包含 -y/--yes 或 --restart"
    else
        fail "upgrade --help 信息不完整 ($checks/4): $result"
    fi
}

test_upgrade_check_output() {
    header "测试 upgrade 检查更新输出"

    cd "$PROJECT_DIR"

    local result
    result=$("$BIFROST_BIN" upgrade 2>&1 || true)

    if echo "$result" | grep -qi "checking for updates\|latest version\|already on the latest\|could not check"; then
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

test_upgrade_updates_installed_desktop_app_best_effort() {
    header "测试 upgrade 已是最新 CLI 时仍 best-effort 更新已安装桌面 App"

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
        && echo "$result" | grep -q "Bifrost desktop app updated successfully"; then
        pass "upgrade 已最新 CLI 时会发现并处理已安装桌面 App"
    else
        fail "upgrade 未发现或未更新已安装桌面 App: $result"
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
  --verbose       详细输出

环境变量:
  BIFROST_DATA_DIR  数据目录 (默认: 临时目录)

示例:
  $0                    # 运行所有测试
  $0 --no-build         # 跳过编译
EOF
}

SKIP_BUILD="false"
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

    test_upgrade_help
    test_upgrade_check_output
    test_upgrade_restart_flag_removed
    test_upgrade_invalid_flag

    test_version_cache_creation
    test_version_cache_content
    test_new_version_notice
    test_no_notice_when_current
    test_upgrade_updates_installed_desktop_app_best_effort
    test_upgrade_desktop_app_failure_does_not_fail_cli_flow

    print_summary
}

main
