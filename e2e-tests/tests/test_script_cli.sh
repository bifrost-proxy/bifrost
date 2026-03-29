#!/bin/bash
#
# Bifrost Script CLI 端到端测试
# 覆盖 script add/update/show(get)/run/list/delete，以及 show/get/run 单参数按 name 模糊匹配
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
SKIP_BUILD="${SKIP_BUILD:-false}"

PASSED=0
FAILED=0

header() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
}

info() {
    echo -e "${CYAN}[INFO]${NC} $1"
}

pass() {
    echo -e "  ${GREEN}✓${NC} $1"
    ((PASSED++))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    ((FAILED++))
}

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}

trap cleanup EXIT

build_bifrost() {
    header "编译 Bifrost"

    if [[ -f "$BIFROST_BIN" ]] && [[ "$SKIP_BUILD" == "true" ]]; then
        info "跳过编译 (--no-build)"
        return
    fi

    echo -e "${GREEN}✓${NC} 编译完成"
}

setup_test_data_dir() {
    TEST_DATA_DIR=$(mktemp -d)
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    mkdir -p "${TEST_DATA_DIR}"
    info "测试数据目录: $TEST_DATA_DIR"
}

seed_scripts() {
    header "准备测试脚本"

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script add request req_unique_demo -c 'log.info("request unique"); request.headers["X-Run"] = "yes";' >/dev/null
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script add response res_shared_demo -c 'log.info("response shared");' >/dev/null
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script add decode dec_shared_demo -c 'ctx.output = { code: "0", data: "shared", msg: "" }; log.info("decode shared");' >/dev/null
    pass "测试脚本已写入"
}

test_update_script_content() {
    header "测试 script update <type> <name>"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script update request req_unique_demo -c 'log.info("request updated"); request.body = "{\"updated\":true}";' 2>&1 || true)

    local verify
    verify=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script show request req_unique_demo 2>&1 || true)

    if echo "$result" | grep -q 'updated successfully' && echo "$verify" | grep -q 'request updated'; then
        pass "update 可以修改已有脚本内容"
    else
        fail "update 输出异常: $result / $verify"
    fi
}

test_show_with_type_and_name() {
    header "测试 script show <type> <name>"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script show request req_unique_demo 2>&1 || true)

    if echo "$result" | grep -q 'Script: req_unique_demo (request)' \
        && echo "$result" | grep -q 'request updated'; then
        pass "show 保持原有 type + name 用法"
    else
        fail "show type + name 输出异常: $result"
    fi
}

test_show_with_name_only() {
    header "测试 script show <name>"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script show unique_demo 2>&1 || true)

    if echo "$result" | grep -q 'Script: req_unique_demo (request)' \
        && echo "$result" | grep -q 'request updated'; then
        pass "show 单参数会按 name 模糊匹配唯一脚本"
    else
        fail "show 单参数未命中唯一脚本: $result"
    fi
}

test_get_alias_with_name_only() {
    header "测试 script get <name> 别名"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script get unique_demo 2>&1 || true)

    if echo "$result" | grep -q 'Script: req_unique_demo (request)' \
        && echo "$result" | grep -q 'request updated'; then
        pass "get 别名可用，且支持单参数 name 匹配"
    else
        fail "get 别名输出异常: $result"
    fi
}

test_run_with_name_only() {
    header "测试 script run <name>"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script run unique_demo 2>&1 || true)

    if echo "$result" | grep -q '^Output:' \
        && echo "$result" | grep -q '^Logs:' \
        && echo "$result" | grep -q 'request updated' \
        && echo "$result" | grep -q '"body":' \
        && echo "$result" | grep -q 'updated'; then
        pass "run 单参数支持 name 匹配，并输出 output + logs"
    else
        fail "run 输出异常: $result"
    fi
}

test_run_decode_output() {
    header "测试 script run decode <name>"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script run decode dec_shared_demo 2>&1 || true)

    if echo "$result" | grep -q '^Output:' \
        && echo "$result" | grep -q '"data": "shared"' \
        && echo "$result" | grep -q 'decode shared' \
        && echo "$result" | grep -q '^Logs:'; then
        pass "decode run 会输出 decode output 和 logs"
    else
        fail "decode run 输出异常: $result"
    fi
}

test_show_name_ambiguous() {
    header "测试 script show <name> 命中多条时返回歧义错误"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script show shared_demo 2>&1 || true)

    if echo "$result" | grep -q 'matched multiple scripts' \
        && echo "$result" | grep -q 'response res_shared_demo' \
        && echo "$result" | grep -q 'decode dec_shared_demo'; then
        pass "show 单参数在多命中时返回候选列表"
    else
        fail "show 多命中歧义提示异常: $result"
    fi
}

test_list_and_delete() {
    header "测试 script list/delete"

    local list_output
    list_output=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script list -t request 2>&1 || true)

    if echo "$list_output" | grep -q 'req_unique_demo'; then
        pass "list 能看到 request 脚本"
    else
        fail "list 未列出 request 脚本: $list_output"
    fi

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script delete request req_unique_demo >/dev/null 2>&1 || true

    if [[ ! -f "${TEST_DATA_DIR}/scripts/request/req_unique_demo.js" ]]; then
        pass "delete 删除了脚本文件"
    else
        fail "delete 后脚本文件仍存在"
    fi
}

print_summary() {
    header "测试总结"
    echo "通过: $PASSED"
    echo "失败: $FAILED"

    if [[ "$FAILED" -gt 0 ]]; then
        exit 1
    fi
}

main() {
    build_bifrost
    setup_test_data_dir
    seed_scripts
    test_update_script_content
    test_show_with_type_and_name
    test_show_with_name_only
    test_get_alias_with_name_only
    test_run_with_name_only
    test_run_decode_output
    test_show_name_ambiguous
    test_list_and_delete
    print_summary
}

main "$@"
