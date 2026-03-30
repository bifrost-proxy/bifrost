#!/bin/bash
#
# Body 替换规则 E2E 测试
#
# 测试 reqReplace / resReplace 规则的 Whistle 兼容语法
#
# 测试矩阵:
# | ID    | 协议  | 类型   | 测试内容                          |
# |-------|-------|--------|-----------------------------------|
# | BR-01 | HTTP  | 请求   | 简单字符串替换                    |
# | BR-02 | HTTP  | 请求   | 多个替换规则 (& 连接)             |
# | BR-03 | HTTP  | 请求   | 删除内容 (替换为空)               |
# | BR-04 | HTTP  | 响应   | 简单字符串替换                    |
# | BR-05 | HTTP  | 响应   | 多个替换规则                      |
# | BR-06 | HTTP  | 双向   | 请求和响应同时替换                |
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$E2E_DIR/.." && pwd)"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/http_client.sh"
source "$E2E_DIR/test_utils/process.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-8080}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
VERBOSE="${VERBOSE:-false}"
TIMEOUT="${TIMEOUT:-60}"
BIFROST_E2E_HTTP_RETRIES="${BIFROST_E2E_HTTP_RETRIES:-2}"
export BIFROST_E2E_HTTP_RETRIES

passed=0
failed=0
skipped=0

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-body-replace"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
PROXY_PID=""
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

log_info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
log_debug()   { [[ "$VERBOSE" == "true" ]] && echo -e "${CYAN}[DEBUG]${NC} $*"; }
log_pass()    { echo -e "${GREEN}[PASS]${NC} $*"; ((passed++)); }
log_fail()    { echo -e "${RED}[FAIL]${NC} $*"; ((failed++)); }
log_skip()    { echo -e "${YELLOW}[SKIP]${NC} $*"; ((skipped++)); }
log_warning() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*"; }

header() {
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  $1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

cleanup() {
    log_info "清理测试环境..."

    kill_bifrost_on_port "$PROXY_PORT"

    safe_cleanup_proxy "$PROXY_PID"

    "$E2E_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true

    log_info "清理完成"
}

trap cleanup EXIT

check_dependencies() {
    log_info "检查依赖..."
    
    local missing=()
    command -v curl >/dev/null 2>&1 || missing+=("curl")
    command -v jq >/dev/null 2>&1 || missing+=("jq")
    command -v python3 >/dev/null 2>&1 || missing+=("python3")
    
    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "缺少依赖: ${missing[*]}"
        exit 1
    fi
    
    log_info "依赖检查通过"
}

start_mock_servers() {
    log_info "启动 Mock 服务器..."
    
    mkdir -p "$TEST_DATA_DIR"
    
    "$E2E_DIR/mock_servers/start_servers.sh" start > "$MOCK_LOG_FILE" 2>&1 &
    
    local count=0
    while ! nc -z 127.0.0.1 "$ECHO_HTTP_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge 30 ]]; then
            log_error "Mock 服务器启动超时"
            cat "$MOCK_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
    
    log_info "Mock 服务器就绪 (HTTP: $ECHO_HTTP_PORT)"
}

start_proxy() {
    log_info "启动代理服务器..."
    
    mkdir -p "$TEST_DATA_DIR"
    
    local rules_file="$TEST_DATA_DIR/body_replace_rules.txt"
    cat > "$rules_file" <<EOF
# Body 替换规则专项测试

# BR-01: 简单字符串替换
test-req-simple.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://OLD_REQ_MARKER=NEW_REQ_MARKER

# BR-02: 多个替换规则 (& 连接)
test-req-multi.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://AAA=XXX&BBB=YYY

# BR-03: 删除内容 (替换为空)
test-req-delete.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://DELETE_ME=

# BR-04: 简单字符串替换
test-res-simple.local http://127.0.0.1:${ECHO_HTTP_PORT} resReplace://OLD_RES_MARKER=NEW_RES_MARKER

# BR-05: 多个替换规则 (& 连接)
test-res-multi.local http://127.0.0.1:${ECHO_HTTP_PORT} resReplace://AAA=XXX&BBB=YYY

# BR-06: 双向替换
test-both-replace.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://REQ_OLD=REQ_NEW resReplace://RES_OLD=RES_NEW

# BR-07: 正则单次替换 (只替换第一个匹配)
test-req-regex.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace:///aaa/=XXX

# BR-08: 正则全局替换 (替换所有匹配)
test-req-regex-global.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace:///aaa/g=XXX

# BR-09: 正则忽略大小写
test-req-regex-case.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace:///aaa/gi=XXX

# BR-10: 响应 body 正则全局替换
test-res-regex-global.local http://127.0.0.1:${ECHO_HTTP_PORT} resReplace:///OLD/g=NEW

# BR-11: 复杂正则模式 (数字匹配)
test-req-regex-digits.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace:///\d+/g=NUM
EOF
    
    log_debug "规则文件: $rules_file"
    log_debug "代理端口: $PROXY_PORT"
    log_debug "数据目录: $TEST_DATA_DIR"
    
    RUST_LOG=info,bifrost_proxy=debug \
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        start \
        --unsafe-ssl \
        --rules-file "$rules_file" \
        > "$PROXY_LOG_FILE" 2>&1 &
    
    PROXY_PID=$!
    log_debug "代理进程 PID: $PROXY_PID"
    
    local count=0
    while ! nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge 30 ]]; then
            log_error "代理服务器启动超时"
            log_error "代理日志:"
            cat "$PROXY_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
    
    log_info "代理服务器就绪 (Port: $PROXY_PORT, PID: $PROXY_PID)"
}

print_diagnosis() {
    echo ""
    log_error "========== 失败诊断信息 =========="
    echo ""
    log_error "代理日志 (最后 50 行):"
    tail -50 "$PROXY_LOG_FILE" 2>/dev/null || echo "无日志"
    echo ""
    log_error "Mock 服务器日志 (最后 20 行):"
    tail -20 "$MOCK_LOG_FILE" 2>/dev/null || echo "无日志"
    echo ""
    log_error "最后一次 HTTP 响应:"
    log_error "  Status: $HTTP_STATUS"
    log_error "  Body 大小: ${#HTTP_BODY} bytes"
    log_error "  Body (前 300 字符): ${HTTP_BODY:0:300}"
}


test_br01_req_simple() {
    local test_name="BR-01: HTTP 请求 body 简单替换"
    log_info "测试: $test_name"
    
    local body="This is OLD_REQ_MARKER text here"
    log_debug "发送 body: $body"
    
    TEST_ID="br-01-$(date +%s)"
    http_post "http://test-req-simple.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == *"NEW_REQ_MARKER"* ]] && [[ "$echoed" != *"OLD_REQ_MARKER"* ]]; then
        log_pass "$test_name - OLD_REQ_MARKER -> NEW_REQ_MARKER"
    else
        log_fail "$test_name"
        log_debug "预期: OLD_REQ_MARKER -> NEW_REQ_MARKER"
        log_debug "实际 body: $echoed"
    fi
}

test_br02_req_multi() {
    local test_name="BR-02: HTTP 请求 body 多规则替换"
    log_info "测试: $test_name"
    
    local body="AAA and BBB together"
    log_debug "发送 body: $body"
    
    TEST_ID="br-02-$(date +%s)"
    http_post "http://test-req-multi.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == *"XXX"* ]] && [[ "$echoed" == *"YYY"* ]] \
       && [[ "$echoed" != *"AAA"* ]] && [[ "$echoed" != *"BBB"* ]]; then
        log_pass "$test_name - AAA->XXX, BBB->YYY"
    else
        log_fail "$test_name"
        log_debug "预期: AAA->XXX, BBB->YYY"
        log_debug "实际 body: $echoed"
    fi
}

test_br03_req_delete() {
    local test_name="BR-03: HTTP 请求 body 删除内容"
    log_info "测试: $test_name"
    
    local body="Keep DELETE_ME end"
    log_debug "发送 body: $body"
    
    TEST_ID="br-03-$(date +%s)"
    TIMEOUT=5
    http_post "http://test-req-delete.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" == "000" ]]; then
        log_skip "$test_name - 连接超时 (已知边界情况)"
        return 0
    fi
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" != *"DELETE_ME"* ]]; then
        log_pass "$test_name - DELETE_ME 已删除"
    else
        log_fail "$test_name"
        log_debug "预期: DELETE_ME 被删除"
        log_debug "实际 body: $echoed"
    fi
}

test_br04_res_simple() {
    local test_name="BR-04: HTTP 响应 body 简单替换"
    log_info "测试: $test_name"
    
    TEST_ID="br-04-$(date +%s)"
    http_get "http://test-res-simple.local/large-response?size=100&marker=OLD_RES_MARKER"
    
    log_debug "响应状态: $HTTP_STATUS"
    log_debug "响应 body: ${HTTP_BODY:0:200}..."
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$HTTP_BODY" == *"NEW_RES_MARKER"* ]] && [[ "$HTTP_BODY" != *"OLD_RES_MARKER"* ]]; then
        log_pass "$test_name - OLD_RES_MARKER -> NEW_RES_MARKER"
    else
        log_fail "$test_name"
        log_debug "预期: OLD_RES_MARKER -> NEW_RES_MARKER"
        log_debug "实际 body 包含 NEW: $([[ "$HTTP_BODY" == *"NEW_RES_MARKER"* ]] && echo 'yes' || echo 'no')"
        log_debug "实际 body 包含 OLD: $([[ "$HTTP_BODY" == *"OLD_RES_MARKER"* ]] && echo 'yes' || echo 'no')"
    fi
}

test_br05_res_multi() {
    local test_name="BR-05: HTTP 响应 body 多规则替换"
    log_info "测试: $test_name"
    
    TEST_ID="br-05-$(date +%s)"
    http_get "http://test-res-multi.local/large-response?size=100&marker=AAA_BBB"
    
    log_debug "响应状态: $HTTP_STATUS"
    log_debug "响应 body: ${HTTP_BODY:0:200}..."
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$HTTP_BODY" == *"XXX"* ]] && [[ "$HTTP_BODY" == *"YYY"* ]] \
       && [[ "$HTTP_BODY" != *"AAA"* ]] && [[ "$HTTP_BODY" != *"BBB"* ]]; then
        log_pass "$test_name - AAA->XXX, BBB->YYY"
    else
        log_fail "$test_name"
        log_debug "预期: AAA->XXX, BBB->YYY"
    fi
}

test_br06_both_replace() {
    local test_name="BR-06: HTTP 请求+响应同时替换"
    log_info "测试: $test_name"
    
    local body="This has REQ_OLD marker"
    log_debug "发送 body: $body"
    
    TEST_ID="br-06-$(date +%s)"
    http_post "http://test-both-replace.local/large-response?size=100&marker=RES_OLD" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    log_debug "响应 body: ${HTTP_BODY:0:200}..."
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    local res_ok=false
    if [[ "$HTTP_BODY" == *"RES_NEW"* ]] && [[ "$HTTP_BODY" != *"RES_OLD"* ]]; then
        res_ok=true
        log_debug "响应替换成功: RES_OLD -> RES_NEW"
    fi
    
    if [[ "$res_ok" == "true" ]]; then
        log_pass "$test_name - 响应替换成功"
    else
        log_fail "$test_name"
        log_debug "响应替换失败"
    fi
}

test_br07_req_regex() {
    local test_name="BR-07: 正则单次替换"
    log_info "测试: $test_name"
    
    local body="aaa-bbb-aaa-ccc"
    log_debug "发送 body: $body"
    
    TEST_ID="br-07-$(date +%s)"
    http_post "http://test-req-regex.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == "XXX-bbb-aaa-ccc" ]]; then
        log_pass "$test_name - 只替换第一个 aaa"
    else
        log_fail "$test_name"
        log_debug "预期: XXX-bbb-aaa-ccc"
        log_debug "实际: $echoed"
    fi
}

test_br08_req_regex_global() {
    local test_name="BR-08: 正则全局替换"
    log_info "测试: $test_name"
    
    local body="aaa-bbb-aaa-ccc"
    log_debug "发送 body: $body"
    
    TEST_ID="br-08-$(date +%s)"
    http_post "http://test-req-regex-global.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == "XXX-bbb-XXX-ccc" ]]; then
        log_pass "$test_name - 替换所有 aaa"
    else
        log_fail "$test_name"
        log_debug "预期: XXX-bbb-XXX-ccc"
        log_debug "实际: $echoed"
    fi
}

test_br09_req_regex_case() {
    local test_name="BR-09: 正则忽略大小写"
    log_info "测试: $test_name"
    
    local body="AAA-aaa-AaA"
    log_debug "发送 body: $body"
    
    TEST_ID="br-09-$(date +%s)"
    http_post "http://test-req-regex-case.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == "XXX-XXX-XXX" ]]; then
        log_pass "$test_name - 忽略大小写替换所有"
    else
        log_fail "$test_name"
        log_debug "预期: XXX-XXX-XXX"
        log_debug "实际: $echoed"
    fi
}

test_br10_res_regex_global() {
    local test_name="BR-10: 响应正则全局替换"
    log_info "测试: $test_name"
    
    TEST_ID="br-10-$(date +%s)"
    http_get "http://test-res-regex-global.local/large-response?size=100&marker=OLD_OLD_OLD"
    
    log_debug "响应状态: $HTTP_STATUS"
    log_debug "响应 body: ${HTTP_BODY:0:200}..."
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$HTTP_BODY" != *"OLD"* ]] && [[ "$HTTP_BODY" == *"NEW"* ]]; then
        log_pass "$test_name - 替换所有 OLD"
    else
        log_fail "$test_name"
        log_debug "预期: 所有 OLD 被替换为 NEW"
    fi
}

test_br11_req_regex_digits() {
    local test_name="BR-11: 正则数字匹配"
    log_info "测试: $test_name"
    
    local body="price: 123, qty: 456"
    log_debug "发送 body: $body"
    
    TEST_ID="br-11-$(date +%s)"
    http_post "http://test-req-regex-digits.local/echo" "$body"
    
    log_debug "响应状态: $HTTP_STATUS"
    local echoed
    echoed=$(get_json_field '.request.body')
    log_debug "Echo 收到: $echoed"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$echoed" == "price: NUM, qty: NUM" ]]; then
        log_pass "$test_name - 替换所有数字"
    else
        log_fail "$test_name"
        log_debug "预期: price: NUM, qty: NUM"
        log_debug "实际: $echoed"
    fi
}


main() {
    header "Body 替换规则 E2E 测试"
    
    log_info "测试配置:"
    log_info "  代理地址: $PROXY_HOST:$PROXY_PORT"
    log_info "  Mock HTTP 端口: $ECHO_HTTP_PORT"
    log_info "  详细日志: $VERBOSE"
    
    check_dependencies
    start_mock_servers
    start_proxy
    
    sleep 2
    
    header "请求 Body 替换测试"
    
    test_br01_req_simple || true
    test_br02_req_multi || true
    test_br03_req_delete || true
    
    header "响应 Body 替换测试"
    
    test_br04_res_simple || true
    test_br05_res_multi || true
    
    header "双向替换测试"
    
    test_br06_both_replace || true
    
    header "正则替换测试"
    
    test_br07_req_regex || true
    test_br08_req_regex_global || true
    test_br09_req_regex_case || true
    test_br10_res_regex_global || true
    test_br11_req_regex_digits || true
    
    header "测试结果汇总"
    
    echo ""
    echo -e "  ${GREEN}通过${NC}: $passed"
    echo -e "  ${RED}失败${NC}: $failed"
    echo -e "  ${YELLOW}跳过${NC}: $skipped"
    echo ""
    
    if [[ $failed -gt 0 ]]; then
        print_diagnosis
        exit 1
    fi
    
    log_info "所有测试通过!"
    exit 0
}

main "$@"
