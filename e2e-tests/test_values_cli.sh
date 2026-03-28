#!/bin/bash
#
# Bifrost Values 系统端到端测试
# 测试 Values CLI 命令、API 端点和值引用功能
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
source "$SCRIPT_DIR/test_utils/process.sh"
VALUES_DIR="${SCRIPT_DIR}/values"
TEST_DATA_DIR="${SCRIPT_DIR}/test_data"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PROXY_PORT="${PROXY_PORT:-8080}"
PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY="http://${PROXY_HOST}:${PROXY_PORT}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
TEST_VALUES_DIR=""
PROXY_PID=""

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
    ((PASSED++))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    ((FAILED++))
}

skip() {
    echo -e "  ${YELLOW}○${NC} $1 (skipped)"
    ((SKIPPED++))
}

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        info "Stopping proxy server (PID: $PROXY_PID)..."
        safe_cleanup_proxy "$PROXY_PID"
    fi

    if [[ -n "$TEST_VALUES_DIR" ]] && [[ -d "$TEST_VALUES_DIR" ]]; then
        rm -rf "$TEST_VALUES_DIR"
    fi

    local mock_pids
    mock_pids=$(pgrep -f "http_echo_server.py" 2>/dev/null || true)
    if [[ -n "$mock_pids" ]]; then
        echo "$mock_pids" | xargs kill 2>/dev/null || true
    fi
}

trap cleanup EXIT

check_dependencies() {
    header "检查依赖"

    local missing=()

    if ! command -v curl &> /dev/null; then
        missing+=("curl")
    fi

    if ! command -v python3 &> /dev/null; then
        missing+=("python3")
    fi

    if ! command -v jq &> /dev/null; then
        warn "jq 未安装，部分 JSON 断言将被跳过"
    fi

    if [[ ${#missing[@]} -gt 0 ]]; then
        error "缺少依赖: ${missing[*]}"
        exit 1
    fi

    echo -e "${GREEN}✓${NC} 依赖检查通过"
}

build_bifrost() {
    header "编译 Bifrost"

    if [[ -f "$BIFROST_BIN" ]] && [[ "$SKIP_BUILD" == "true" ]]; then
        info "跳过编译 (--no-build)"
        return
    fi

    info "编译 bifrost..."
    cd "$PROJECT_DIR"
    cargo build --release --bin bifrost 2>&1 | tail -5
    echo -e "${GREEN}✓${NC} 编译完成"
}

setup_test_values_dir() {
    TEST_VALUES_DIR=$(mktemp -d)
    export BIFROST_DATA_DIR="$TEST_VALUES_DIR"
    mkdir -p "${TEST_VALUES_DIR}/values"
    info "测试数据目录: $TEST_VALUES_DIR"
}

start_echo_server() {
    header "启动 Echo 服务器"

    if lsof -i ":${ECHO_HTTP_PORT}" -t >/dev/null 2>&1; then
        info "Echo 服务器已在端口 ${ECHO_HTTP_PORT} 运行"
        return
    fi

    python3 "${SCRIPT_DIR}/mock_servers/http_echo_server.py" --port "${ECHO_HTTP_PORT}" &
    local echo_pid=$!

    sleep 1

    if curl -s "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Echo 服务器已启动 (端口: ${ECHO_HTTP_PORT})"
    else
        error "Echo 服务器启动失败"
        exit 1
    fi
}

test_cli_value_set_get() {
    header "测试 CLI: value add/show (兼容 set/get)"

    info "设置值 test_key=test_value"
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value add test_key "test_value" 2>&1

    info "获取值 test_key"
    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value show test_key 2>&1 || true)

    if echo "$result" | grep -q "test_value"; then
        pass "value add/show 工作正常"
    else
        fail "value show 未返回预期值: $result"
    fi
}

test_cli_value_update() {
    header "测试 CLI: value update"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set update_key "original" 2>&1
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value update update_key "updated" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get update_key 2>&1 || true)

    if echo "$result" | grep -q "updated" && ! echo "$result" | grep -q "original"; then
        pass "value update 正确更新了已有值"
    else
        fail "value update 未正确更新已有值: $result"
    fi
}

test_cli_value_update_not_found() {
    header "测试 CLI: value update 不存在的值"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value update missing_key "updated" 2>&1 || true)

    if echo "$result" | grep -qi "not found"; then
        pass "value update 对不存在的值返回了明确错误"
    else
        fail "value update 缺少不存在场景的错误提示: $result"
    fi
}

test_cli_value_list() {
    header "测试 CLI: value list"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set key1 "value1" 2>&1
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set key2 "value2" 2>&1
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set key3 "value3" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value list 2>&1 || true)

    local count=0
    if echo "$result" | grep -q "key1"; then ((count++)); fi
    if echo "$result" | grep -q "key2"; then ((count++)); fi
    if echo "$result" | grep -q "key3"; then ((count++)); fi

    if [[ $count -ge 3 ]]; then
        pass "value list 列出了所有值 ($count 个)"
    else
        fail "value list 未列出所有值 (找到 $count 个): $result"
    fi
}

test_cli_value_delete() {
    header "测试 CLI: value delete"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set delete_me "to_be_deleted" 2>&1

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value delete delete_me 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get delete_me 2>&1 || true)

    if echo "$result" | grep -qi "not found\|error\|no value"; then
        pass "value delete 正确删除了值"
    else
        fail "value delete 后值仍然存在: $result"
    fi
}

test_cli_value_import_txt() {
    header "测试 CLI: value import (.txt)"

    local test_file="${TEST_VALUES_DIR}/import_test.txt"
    echo "imported_txt_content" > "$test_file"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value import "$test_file" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get import_test 2>&1 || true)

    if echo "$result" | grep -q "imported_txt_content"; then
        pass "value import (.txt) 工作正常"
    else
        fail "value import (.txt) 未正确导入: $result"
    fi
}

test_cli_value_import_json() {
    header "测试 CLI: value import (.json)"

    local test_file="${TEST_VALUES_DIR}/import_test.json"
    cat > "$test_file" << 'EOF'
{
  "json_key1": "json_value1",
  "json_key2": "json_value2"
}
EOF

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value import "$test_file" 2>&1

    local result1
    result1=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get json_key1 2>&1 || true)

    local result2
    result2=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get json_key2 2>&1 || true)

    if echo "$result1" | grep -q "json_value1" && echo "$result2" | grep -q "json_value2"; then
        pass "value import (.json) 工作正常"
    else
        fail "value import (.json) 未正确导入: $result1, $result2"
    fi
}

test_cli_value_import_kv() {
    header "测试 CLI: value import (.kv)"

    local test_file="${TEST_VALUES_DIR}/import_test.kv"
    cat > "$test_file" << 'EOF'
# Comment line
kv_key1=kv_value1
kv_key2=kv_value2
EOF

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value import "$test_file" 2>&1

    local result1
    result1=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get kv_key1 2>&1 || true)

    if echo "$result1" | grep -q "kv_value1"; then
        pass "value import (.kv) 工作正常"
    else
        fail "value import (.kv) 未正确导入: $result1"
    fi
}

test_values_file_loading() {
    header "测试: 从文件导入值"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value import "${VALUES_DIR}/authHeaders.txt" 2>&1
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value import "${VALUES_DIR}/mockResponse.txt" 2>&1

    local result1
    result1=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get authHeaders 2>&1 || true)

    local result2
    result2=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get mockResponse 2>&1 || true)

    if echo "$result1" | grep -q "X-Auth-Token" && echo "$result2" | grep -q "mock"; then
        pass "从文件正确导入了值"
    else
        fail "从文件导入失败: authHeaders=$result1, mockResponse=$result2"
    fi
}

test_value_multiline() {
    header "测试: 多行值"

    local multiline_value="line1
line2
line3"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set multiline "$multiline_value" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get multiline 2>&1 || true)

    if echo "$result" | grep -q "line1" && echo "$result" | grep -q "line2"; then
        pass "多行值正确存储和读取"
    else
        fail "多行值处理失败: $result"
    fi
}

test_value_special_characters() {
    header "测试: 特殊字符值"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set special_chars "test=value&foo=bar" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get special_chars 2>&1 || true)

    if echo "$result" | grep -q "test=value"; then
        pass "特殊字符值正确处理"
    else
        fail "特殊字符值处理失败: $result"
    fi
}

test_value_unicode() {
    header "测试: Unicode 值"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set unicode "中文测试-日本語-한국어" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get unicode 2>&1 || true)

    if echo "$result" | grep -q "中文测试"; then
        pass "Unicode 值正确处理"
    else
        fail "Unicode 值处理失败: $result"
    fi
}

test_value_empty() {
    header "测试: 空值"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set empty_value "" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get empty_value 2>&1 || true)

    if ! echo "$result" | grep -qi "error\|not found"; then
        pass "空值正确处理"
    else
        fail "空值处理失败: $result"
    fi
}

test_value_overwrite() {
    header "测试: 值覆盖"

    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set overwrite_key "original" 2>&1
    BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value set overwrite_key "updated" 2>&1

    local result
    result=$(BIFROST_DATA_DIR="$TEST_VALUES_DIR" "$BIFROST_BIN" value get overwrite_key 2>&1 || true)

    if echo "$result" | grep -q "updated" && ! echo "$result" | grep -q "original"; then
        pass "值覆盖正确工作"
    else
        fail "值覆盖失败: $result"
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

Bifrost Values 系统端到端测试

选项:
  -h, --help      显示帮助信息
  --no-build      跳过编译步骤
  --cli-only      只运行 CLI 测试
  --verbose       详细输出

环境变量:
  BIFROST_DATA_DIR  数据目录 (默认: 临时目录)
  PROXY_PORT        代理端口 (默认: 8080)
  ECHO_HTTP_PORT    Echo 服务器端口 (默认: 3000)

示例:
  $0                    # 运行所有测试
  $0 --no-build         # 跳过编译
  $0 --cli-only         # 只运行 CLI 测试
EOF
}

SKIP_BUILD="false"
CLI_ONLY="false"
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
        --cli-only)
            CLI_ONLY="true"
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
    echo -e "${CYAN}║          Bifrost Values 系统端到端测试                        ║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"

    check_dependencies
    build_bifrost
    setup_test_values_dir

    test_cli_value_set_get
    test_cli_value_update
    test_cli_value_update_not_found
    test_cli_value_list
    test_cli_value_delete
    test_cli_value_import_txt
    test_cli_value_import_json
    test_cli_value_import_kv

    test_values_file_loading

    test_value_multiline
    test_value_special_characters
    test_value_unicode
    test_value_empty
    test_value_overwrite

    print_summary
}

main
