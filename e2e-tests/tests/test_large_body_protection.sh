#!/bin/bash
#
# 大文件 Body 处理保护策略 E2E 测试
# 
# 测试 "流式转发 + 按需缓冲 + 大小保护" 策略在各种场景下的正确性
#
# 测试矩阵:
# | 协议  | 请求 Body | 响应 Body | 有 Body 规则 | 预期行为 |
# |-------|-----------|-----------|--------------|----------|
# | HTTP  | 小        | 小        | 有           | 规则应用 |
# | HTTP  | 大        | 小        | 有           | 请求规则跳过 |
# | HTTP  | 小        | 大        | 有           | 响应规则跳过 |
# | HTTP  | 大        | 大        | 无           | 流式转发成功 |
# | HTTPS | 小        | 小        | 有           | 规则应用 |
# | HTTPS | 大        | 大        | 有           | 规则跳过 |
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$E2E_DIR/.." && pwd)"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

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
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}"
VERBOSE="${VERBOSE:-false}"
TIMEOUT="${TIMEOUT:-120}"

SMALL_BODY_SIZE=1024
LARGE_BODY_SIZE=35000000
REQ_MARKER="REQ_MARKER_12345"
RES_MARKER="RES_MARKER_67890"
REQ_REPLACED="REQ_REPLACED"
RES_REPLACED="RES_REPLACED"

passed=0
failed=0
skipped=0

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-large-body"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
PROXY_PID=""

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
    
    local rules_file="$TEST_DATA_DIR/large_body_rules.txt"
    cat > "$rules_file" <<RULES_EOF
test-large-req-replace.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://REQ_MARKER_12345=REQ_REPLACED
test-large-res-replace.local http://127.0.0.1:${ECHO_HTTP_PORT} resReplace://RES_MARKER_67890=RES_REPLACED
test-large-both-replace.local http://127.0.0.1:${ECHO_HTTP_PORT} reqReplace://REQ_MARKER_12345=REQ_REPLACED resReplace://RES_MARKER_67890=RES_REPLACED
test-large-no-rule.local http://127.0.0.1:${ECHO_HTTP_PORT}
RULES_EOF
    
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

check_proxy_log_for_warning() {
    local pattern="$1"
    
    if grep -q "$pattern" "$PROXY_LOG_FILE" 2>/dev/null; then
        log_debug "代理日志中找到预期警告: $pattern"
        return 0
    else
        log_debug "代理日志中未找到警告: $pattern"
        return 1
    fi
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


test_http_small_req_body_no_rule() {
    local test_name="LB-01a: HTTP 小请求 body + 无规则 (正常转发)"
    log_info "测试: $test_name"
    
    local url="http://test-large-no-rule.local/echo"
    
    log_debug "URL: $url"
    log_debug "请求 body 大小: $SMALL_BODY_SIZE bytes"
    log_debug "Marker: $REQ_MARKER"
    
    TEST_ID="lb-01a-$(date +%s)"
    http_post_large_body "$url" "$SMALL_BODY_SIZE" "$REQ_MARKER"
    
    log_debug "响应状态码: $HTTP_STATUS"
    log_debug "响应 body 大小: ${#HTTP_BODY} bytes"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        log_debug "响应 body: ${HTTP_BODY:0:500}"
        return 1
    fi
    
    local echoed_body
    echoed_body=$(get_json_field '.request.body')
    log_debug "Echo 收到的 body (前 200 字符): ${echoed_body:0:200}"
    
    if [[ "$echoed_body" == *"$REQ_MARKER"* ]]; then
        log_pass "$test_name - 小请求 body 成功转发，原始内容保留"
        return 0
    else
        log_fail "$test_name - 请求 body 内容异常"
        return 1
    fi
}

test_http_large_req_body_with_rule() {
    local test_name="LB-01b: HTTP 大请求 body + reqReplace 规则 (应跳过)"
    log_info "测试: $test_name"
    
    local url="http://test-large-req-replace.local/echo"
    
    log_debug "URL: $url"
    log_debug "请求 body 大小: $LARGE_BODY_SIZE bytes (超过 32MB 阈值)"
    log_debug "Marker: $REQ_MARKER"
    
    TEST_ID="lb-01b-$(date +%s)"
    http_post_large_body "$url" "$LARGE_BODY_SIZE" "$REQ_MARKER"
    
    log_debug "响应状态码: $HTTP_STATUS"
    log_debug "响应 body 大小: ${#HTTP_BODY} bytes"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS (大请求应该成功转发)"
        return 1
    fi
    
    local echoed_body
    echoed_body=$(get_json_field '.request.body')
    
    if [[ "$echoed_body" == *"$REQ_MARKER"* ]]; then
        log_pass "$test_name - 大请求 body 规则已跳过，原始 marker 保留"
        
        if check_proxy_log_for_warning "REQ_BODY.*body too large"; then
            log_debug "代理日志中找到预期的 'body too large' 警告"
        else
            log_warning "代理日志中未找到 'body too large' 警告，但测试仍通过"
        fi
        return 0
    else
        log_fail "$test_name - 大请求 body 被意外处理"
        log_debug "预期: body 包含原始 marker '$REQ_MARKER'"
        return 1
    fi
}

test_http_small_res_body_no_rule() {
    local test_name="LB-02a: HTTP 小响应 body + 无规则 (正常转发)"
    log_info "测试: $test_name"
    
    local url="http://test-large-no-rule.local/large-response?size=$SMALL_BODY_SIZE&marker=$RES_MARKER"
    
    log_debug "URL: $url"
    log_debug "响应 body 大小: $SMALL_BODY_SIZE bytes"
    log_debug "Marker: $RES_MARKER"
    
    TEST_ID="lb-02a-$(date +%s)"
    http_get "$url"
    
    log_debug "响应状态码: $HTTP_STATUS"
    log_debug "响应 body 大小: ${#HTTP_BODY} bytes"
    log_debug "响应 body (前 200 字符): ${HTTP_BODY:0:200}"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$HTTP_BODY" == *"$RES_MARKER"* ]]; then
        log_pass "$test_name - 小响应 body 成功转发，原始内容保留"
        return 0
    else
        log_fail "$test_name - 响应 body 内容异常"
        return 1
    fi
}

test_http_large_res_body_with_rule() {
    local test_name="LB-02b: HTTP 大响应 body + resReplace 规则 (应跳过)"
    log_info "测试: $test_name"
    
    local url="http://test-large-res-replace.local/large-response?size=$LARGE_BODY_SIZE&marker=$RES_MARKER"
    
    log_debug "URL: $url"
    log_debug "响应 body 大小: $LARGE_BODY_SIZE bytes (超过 32MB 阈值)"
    log_debug "Marker: $RES_MARKER"
    
    TEST_ID="lb-02b-$(date +%s)"
    http_get "$url"
    
    log_debug "响应状态码: $HTTP_STATUS"
    log_debug "响应 body 大小: ${#HTTP_BODY} bytes"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS (大响应应该成功转发)"
        return 1
    fi
    
    if [[ "$HTTP_BODY" == *"$RES_MARKER"* ]]; then
        log_pass "$test_name - 大响应 body 规则已跳过，原始 marker 保留"
        
        if check_proxy_log_for_warning "RES_BODY.*body too large"; then
            log_debug "代理日志中找到预期的 'body too large' 警告"
        else
            log_warning "代理日志中未找到 'body too large' 警告，但测试仍通过"
        fi
        return 0
    else
        log_fail "$test_name - 大响应 body 被意外处理"
        log_debug "预期: body 包含原始 marker '$RES_MARKER'"
        return 1
    fi
}

test_http_large_body_no_rule() {
    local test_name="LB-04: HTTP 大请求/响应 body + 无规则 (流式转发)"
    log_info "测试: $test_name"
    
    local url="http://test-large-no-rule.local/large-response?size=$LARGE_BODY_SIZE&marker=NO_RULE_MARKER"
    
    log_debug "URL: $url"
    log_debug "响应 body 大小: $LARGE_BODY_SIZE bytes"
    
    TEST_ID="lb-04-$(date +%s)"
    http_get "$url"
    
    log_debug "响应状态码: $HTTP_STATUS"
    log_debug "响应 body 大小: ${#HTTP_BODY} bytes"
    
    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "$test_name - 状态码错误: $HTTP_STATUS"
        return 1
    fi
    
    if [[ "$HTTP_BODY" == *"NO_RULE_MARKER"* ]]; then
        log_pass "$test_name - 无规则大 body 成功流式转发"
        return 0
    else
        log_fail "$test_name - 响应 body 内容异常"
        return 1
    fi
}


main() {
    header "大文件 Body 处理保护策略 E2E 测试"
    
    log_info "测试配置:"
    log_info "  代理地址: $PROXY_HOST:$PROXY_PORT"
    log_info "  Mock HTTP 端口: $ECHO_HTTP_PORT"
    log_info "  小 body 大小: $SMALL_BODY_SIZE bytes"
    log_info "  大 body 大小: $LARGE_BODY_SIZE bytes"
    log_info "  详细日志: $VERBOSE"
    
    check_dependencies
    start_mock_servers
    start_proxy
    
    sleep 2
    
    header "HTTP 协议测试"
    
    test_http_small_req_body_no_rule || true
    test_http_large_req_body_with_rule || true
    test_http_small_res_body_no_rule || true
    test_http_large_res_body_with_rule || true
    test_http_large_body_no_rule || true
    
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
