#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RULES_DIR="${SCRIPT_DIR}/rules/pattern"

source "$SCRIPT_DIR/test_utils/assert.sh"
source "$SCRIPT_DIR/test_utils/http_client.sh"
source "$SCRIPT_DIR/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18080}"
PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY="http://${PROXY_HOST}:${PROXY_PORT}"

ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}"

TEST_DATA_DIR=""
PROXY_PID=""

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

header() {
    echo -e "\n${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  $1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}\n"
}

info() { echo -e "${BLUE}ℹ${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }
pass() { echo -e "${GREEN}✓${NC} $1"; }
fail() { echo -e "${RED}✗${NC} $1"; }

cleanup() {
    info "清理测试环境..."

    if [[ -n "$PROXY_PID" ]]; then
        info "停止测试代理 (PID: $PROXY_PID)..."
        safe_cleanup_proxy "$PROXY_PID"
    fi

    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true

    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        info "清理临时目录: $TEST_DATA_DIR"
        rm -rf "$TEST_DATA_DIR"
    fi
}

trap cleanup EXIT

setup_test_env() {
    header "设置测试环境"

    TEST_DATA_DIR=$(mktemp -d)
    info "临时数据目录: $TEST_DATA_DIR"

    mkdir -p "${TEST_DATA_DIR}"/{rules,values,certs}

    cat > "${TEST_DATA_DIR}/config.toml" << 'TOML'
[access]
mode = "local_only"
whitelist = []
allow_lan = false
intercept_exclude = []
TOML

    pass "测试环境已创建"
}

check_dependencies() {
    header "检查依赖"

if ! command -v curl &> /dev/null; then
        fail "curl 未安装"
        exit 1
    fi
    pass "curl 已安装"

    if ! command -v python3 &> /dev/null; then
        fail "python3 未安装"
        exit 1
    fi
    pass "python3 已安装"

    if ! command -v jq &> /dev/null; then
        warn "jq 未安装 (部分断言将被跳过)"
    else
        pass "jq 已安装"
fi
}

build_proxy() {
    header "编译代理服务器"

    if [[ -f "${PROJECT_DIR}/target/release/bifrost" ]]; then
        local mod_time=$(stat -f %m "${PROJECT_DIR}/target/release/bifrost" 2>/dev/null || stat -c %Y "${PROJECT_DIR}/target/release/bifrost" 2>/dev/null)
    local now=$(date +%s)
        local age=$((now - mod_time))

        if [[ $age -lt 86400 ]]; then
            pass "使用已编译的代理 (编译于 $((age / 60)) 分钟前)"
    return 0
        fi
    fi

    info "正在编译代理服务器..."
    cd "$PROJECT_DIR"
    cargo build --release --bin bifrost 2>&1 | tail -5
    pass "代理服务器编译完成"
}

start_echo_servers() {
header "启动 Echo 服务器"

    if curl -s "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
        pass "HTTP Echo 服务器已在运行 (端口: ${ECHO_HTTP_PORT})"
        return 0
    fi

HTTP_PORT="${ECHO_HTTP_PORT}" HTTPS_PORT="${ECHO_HTTPS_PORT}" \
    "$SCRIPT_DIR/mock_servers/start_servers.sh" start-bg

    sleep 2

    if curl -s "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
pass "HTTP Echo 服务器已启动 (端口: ${ECHO_HTTP_PORT})"
    else
        fail "HTTP Echo 服务器启动失败"
        exit 1
    fi
}

start_proxy_with_rules() {
    local rules_file="$1"

    if [[ ! -f "$rules_file" ]]; then
    fail "规则文件不存在: $rules_file"
        return 1
    fi

    if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
        info "停止现有测试代理..."
        safe_cleanup_proxy "$PROXY_PID"
        sleep 1
    fi

    kill_bifrost_on_port "${PROXY_PORT}"

    info "启动代理 (端口: ${PROXY_PORT}, 数据目录: ${TEST_DATA_DIR})..."

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    # 可选系统代理参数
    local extra_flags=()
    if [[ "${ENABLE_SYSTEM_PROXY:-}" == "true" ]]; then
        extra_flags+=(--system-proxy)
        local bypass_val="${SYSTEM_PROXY_BYPASS:-localhost,127.0.0.1,::1,*.local}"
        extra_flags+=(--proxy-bypass "$bypass_val")
    fi

    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${PROJECT_DIR}/target/release/bifrost" \
        --port "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl \
        --rules-file "${rules_file}" "${extra_flags[@]}" &
    PROXY_PID=$!

    local max_wait=10
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if curl -s --proxy "$PROXY" --connect-timeout 1 http://example.com >/dev/null 2>&1; then
            pass "代理服务器已启动 (PID: $PROXY_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    fail "代理服务器启动超时"
    return 1
}

test_pattern_match() {
    local description="$1"
    local host_header="$2"
    local path="$3"
    local should_match="$4"

    local test_url
    if [[ "$should_match" == "true" ]]; then
        test_url="http://127.0.0.1:${ECHO_HTTP_PORT}${path}"
    else
        test_url="http://non-existent-domain.invalid${path}"
    fi

    local curl_args=(
        -s
        -w '%{http_code}'
        --max-time 3
        --proxy "$PROXY"
        -o /dev/null
    )

    if [[ -n "$host_header" ]]; then
        curl_args+=(-H "Host: $host_header")
    fi

    curl_args+=("$test_url")

    local status
    status=$(curl "${curl_args[@]}" 2>/dev/null) || status="000"

    if [[ "$should_match" == "true" ]]; then
        if [[ "$status" =~ ^2[0-9]{2}$ ]]; then
            _log_pass "$description → 匹配成功 (状态码: $status)"
        else
            _log_fail "$description" "匹配成功 (2xx)" "状态码: $status"
        fi
    else
        if [[ "$status" == "000" ]] || [[ ! "$status" =~ ^2[0-9]{2}$ ]]; then
            _log_pass "$description → 正确不匹配"
        else
            _log_fail "$description" "不匹配 (非 2xx)" "状态码: $status (意外匹配)"
        fi
    fi
}

test_domain_wildcard() {
    header "域名通配符测试"

    start_proxy_with_rules "${RULES_DIR}/domain_wildcard.txt" || return 1

    echo ""
    echo -e "${YELLOW}【DW-01】单星 * 匹配单级子域 (不含点)${NC}"
    test_pattern_match "www.single-level.local 应匹配 *.single-level.local" \
        "www.single-level.local" "/test" "true"
    test_pattern_match "api.single-level.local 应匹配 *.single-level.local" \
        "api.single-level.local" "/test" "true"
    test_pattern_match "a.b.single-level.local 不应匹配 *.single-level.local (多级)" \
        "a.b.single-level.local" "/test" "false"

    echo ""
    echo -e "${YELLOW}【DW-02】双星 ** 匹配多级子域 (可含点)${NC}"
    test_pattern_match "www.multi-level.local 应匹配 **.multi-level.local" \
        "www.multi-level.local" "/test" "true"
    test_pattern_match "a.b.multi-level.local 应匹配 **.multi-level.local" \
        "a.b.multi-level.local" "/test" "true"
    test_pattern_match "deep.nested.sub.multi-level.local 应匹配 **.multi-level.local" \
        "deep.nested.sub.multi-level.local" "/test" "true"

    echo ""
    echo -e "${YELLOW}【DW-03】\$ 前缀 + 单星${NC}"
    test_pattern_match "www.dollar-single.local 应匹配 \$*.dollar-single.local" \
        "www.dollar-single.local" "/test" "true"

    echo ""
    echo -e "${YELLOW}【DW-04】\$ 前缀 + 双星${NC}"
    test_pattern_match "a.b.dollar-multi.local 应匹配 \$**.dollar-multi.local" \
        "a.b.dollar-multi.local" "/test" "true"

    echo ""
    echo -e "${YELLOW}【DW-07】多个单星 *.*.domain.local${NC}"
    test_pattern_match "a.b.double-single-star.local 应匹配 *.*.double-single-star.local" \
        "a.b.double-single-star.local" "/test" "true"
    test_pattern_match "a.b.c.double-single-star.local 不应匹配 *.*.double-single-star.local (三级)" \
        "a.b.c.double-single-star.local" "/test" "false"
}

test_path_wildcard() {
    header "路径通配符测试 (^前缀)"

    start_proxy_with_rules "${RULES_DIR}/path_wildcard.txt" || return 1

    echo ""
    echo -e "${YELLOW}【PW-01】单星 * 匹配单个路径段 (不含 / 和 ?)${NC}"
    test_pattern_match "/api/users/info 应匹配 ^path-single.local/api/*/info" \
        "path-single.local" "/api/users/info" "true"
    test_pattern_match "/api/products/info 应匹配 ^path-single.local/api/*/info" \
        "path-single.local" "/api/products/info" "true"
    test_pattern_match "/api/a/b/info 不应匹配 ^path-single.local/api/*/info (多段)" \
        "path-single.local" "/api/a/b/info" "false"

    echo ""
    echo -e "${YELLOW}【PW-02】双星 ** 匹配多路径段 (不含 ?)${NC}"
    test_pattern_match "/api/users 应匹配 ^path-double.local/api/**" \
        "path-double.local" "/api/users" "true"
    test_pattern_match "/api/users/123/details 应匹配 ^path-double.local/api/**" \
        "path-double.local" "/api/users/123/details" "true"

    echo ""
    echo -e "${YELLOW}【PW-03】三星 *** 匹配任意内容 (含 ?)${NC}"
    test_pattern_match "/api/users 应匹配 ^path-triple.local/api/***" \
        "path-triple.local" "/api/users" "true"
    test_pattern_match "/api/users?id=1 应匹配 ^path-triple.local/api/***" \
        "path-triple.local" "/api/users?id=1" "true"
    test_pattern_match "/api/a/b?x=1&y=2 应匹配 ^path-triple.local/api/***" \
        "path-triple.local" "/api/a/b?x=1&y=2" "true"

    echo ""
    echo -e "${YELLOW}【PW-04】多个单星组合${NC}"
    test_pattern_match "/v1/users/items/123/details 应匹配 ^path-multi.local/v1/*/items/*/details" \
        "path-multi.local" "/v1/users/items/123/details" "true"

    echo ""
    echo -e "${YELLOW}【PW-05】带固定后缀的单星${NC}"
    test_pattern_match "/files/config.json 应匹配 ^path-suffix.local/files/*.json" \
        "path-suffix.local" "/files/config.json" "true"
    test_pattern_match "/files/data.json 应匹配 ^path-suffix.local/files/*.json" \
        "path-suffix.local" "/files/data.json" "true"
}

test_port_wildcard() {
    header "端口通配符测试"

    start_proxy_with_rules "${RULES_DIR}/port_wildcard.txt" || return 1

    echo ""
    echo -e "${YELLOW}【PT-01】端口前缀匹配 8*${NC}"
    test_pattern_match "port-prefix.local:8080 应匹配 port-prefix.local:8*" \
        "port-prefix.local:8080" "/test" "true"
    test_pattern_match "port-prefix.local:8888 应匹配 port-prefix.local:8*" \
        "port-prefix.local:8888" "/test" "true"
    test_pattern_match "port-prefix.local:9000 不应匹配 port-prefix.local:8*" \
        "port-prefix.local:9000" "/test" "false"

    echo ""
    echo -e "${YELLOW}【PT-02】端口后缀匹配 *80${NC}"
    test_pattern_match "port-suffix.local:80 应匹配 port-suffix.local:*80" \
        "port-suffix.local:80" "/test" "true"
    test_pattern_match "port-suffix.local:8080 应匹配 port-suffix.local:*80" \
        "port-suffix.local:8080" "/test" "true"

    echo ""
    echo -e "${YELLOW}【PT-03】端口中间匹配 8*8${NC}"
    test_pattern_match "port-middle.local:88 应匹配 port-middle.local:8*8" \
        "port-middle.local:88" "/test" "true"
    test_pattern_match "port-middle.local:808 应匹配 port-middle.local:8*8" \
        "port-middle.local:808" "/test" "true"
    test_pattern_match "port-middle.local:8888 应匹配 port-middle.local:8*8" \
        "port-middle.local:8888" "/test" "true"
}

test_protocol_wildcard() {
    header "协议通配符测试"

    start_proxy_with_rules "${RULES_DIR}/protocol_wildcard.txt" || return 1

    echo ""
    echo -e "${YELLOW}【PR-01】HTTP 协议通配符 http*://${NC}"
    test_pattern_match "http://proto-http.local 应匹配 http*://proto-http.local" \
        "proto-http.local" "/test" "true"

    echo ""
    echo -e "${YELLOW}【PR-07】协议通配符 + 路径${NC}"
    test_pattern_match "http://proto-path.local/api/users 应匹配 http*://proto-path.local/api/*" \
        "proto-path.local" "/api/users" "true"
}

run_all_tests() {
    reset_test_stats

    test_domain_wildcard
    test_path_wildcard
    test_port_wildcard
    test_protocol_wildcard

    print_test_summary
}

main() {
    header "Bifrost Pattern 端到端测试"
    echo "测试代理端口: $PROXY_PORT"
    echo "Echo 服务端口: $ECHO_HTTP_PORT"
    echo ""

    check_dependencies
    build_proxy
    setup_test_env
    start_echo_servers
    run_all_tests
}

main "$@"
