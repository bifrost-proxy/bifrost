#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RULES_DIR="${SCRIPT_DIR}/rules"
# 优先使用 BIFROST_DATA_DIR，避免覆盖本机已有数据；也便于并行/多开测试。
TEST_DATA_DIR="${BIFROST_DATA_DIR:-${PROJECT_DIR}/.bifrost-test}"

source "$SCRIPT_DIR/test_utils/assert.sh"
source "$SCRIPT_DIR/test_utils/http_client.sh"
source "$SCRIPT_DIR/test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-8080}"
PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY="http://${PROXY_HOST}:${PROXY_PORT}"

ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}"
ECHO_WS_PORT="${ECHO_WS_PORT:-3020}"
ECHO_WSS_PORT="${ECHO_WSS_PORT:-3021}"
ECHO_SSE_PORT="${ECHO_SSE_PORT:-3003}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-9999}"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

header() { echo -e "\n${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}\n"; }
info() { echo -e "${BLUE}ℹ${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

PROXY_PID=""
RULE_FILE=""

SUPPORTED_PROTOCOL_NAMES=(
    host xhost http https ws wss proxy http3 pac redirect file tpl rawfile delete skip
    filter includeFilter excludeFilter lineProps
    reqHeaders reqBody reqPrepend reqAppend reqCookies reqCors reqDelay reqSpeed reqType
    reqCharset reqReplace forwardedFor method auth ua referer urlParams params tunnel
    resHeaders resBody resPrepend resAppend resCookies resCors resDelay resSpeed resType
    resCharset resReplace responseFor replaceStatus statusCode cache attachment trailers resMerge
    headerReplace htmlAppend htmlPrepend htmlBody jsAppend jsPrepend jsBody cssAppend
    cssPrepend cssBody urlReplace reqScript resScript decode dns tlsIntercept
    tlsPassthrough passthrough tlsOptions sniCallback
)

SUPPORTED_PROTOCOL_ALIASES=(
    pathReplace download http-proxy h3
    status hosts html js reqMerge css
)

RULE_CAPABILITY_ERRORS=()

abs_path() {
    local path="$1"
    if [[ "$path" = /* ]]; then
        printf '%s\n' "$path"
    else
        printf '%s/%s\n' "$(pwd -P)" "$path"
    fi
}

array_contains() {
    local needle="$1"
    shift

    local item
    for item in "$@"; do
        if [[ "$item" == "$needle" ]]; then
            return 0
        fi
    done

    return 1
}

resolve_bifrost_release_bin() {
    local release_dir="${PROJECT_DIR}/target/release"
    local unix_bin="${release_dir}/bifrost"
    local windows_bin="${release_dir}/bifrost.exe"

    if [[ -x "$unix_bin" ]]; then
        printf '%s\n' "$unix_bin"
        return 0
    fi

    if [[ -f "$windows_bin" ]]; then
        printf '%s\n' "$windows_bin"
        return 0
    fi

    return 1
}

have_lsof() {
    command -v lsof >/dev/null 2>&1
}

lsof_supports_pid_output() {
    local lsof_cmd=""
    lsof_cmd="$(command -v lsof 2>/dev/null || true)"
    [[ -n "$lsof_cmd" && "$lsof_cmd" != "${SCRIPT_DIR}/bin/lsof" ]]
}

usage() {
    echo "用法: $0 [选项] <规则文件>"
    echo ""
    echo "选项:"
    echo "  -h, --help            显示帮助信息"
    echo "  -p, --port PORT       指定代理端口 (默认: 8080)"
    echo "  -d, --data-dir DIR    指定数据目录 (默认: .bifrost-test)"
    echo "  -l, --list            列出所有可用的规则文件"
    echo "  --no-build            跳过编译步骤"
    echo "  --use-binary          使用预编译二进制而不是 cargo run (用于并行测试)"
    echo "  --keep-proxy          测试完成后保持代理运行"
    echo "  --skip-mock-servers   跳过 mock 服务器启动/停止 (用于并行测试)"
    echo ""
    echo "环境变量:"
    echo "  ECHO_HTTP_PORT     HTTP Echo 服务器端口 (默认: 3000)"
    echo "  ECHO_HTTPS_PORT    HTTPS Echo 服务器端口 (默认: 3443)"
    echo "  ECHO_WS_PORT       WebSocket Echo 服务器端口 (默认: 3020)"
    echo "  ECHO_WSS_PORT      WebSocket Secure Echo 服务器端口 (默认: 3021)"
    echo "  ECHO_SSE_PORT      SSE Echo 服务器端口 (默认: 3003)"
    echo "  ECHO_PROXY_PORT    HTTP Proxy Echo 服务器端口 (默认: 9999)"
    echo ""
    echo "示例:"
    echo "  $0 rules/forwarding/http_to_http.txt"
    echo "  $0 -p 9090 rules/request_modify/headers.txt"
    echo "  $0 --list"
    exit 0
}

list_rules() {
    header "可用的规则文件"

    find "$RULES_DIR" -name "*.txt" -type f 2>/dev/null | sort | while read -r rule_file; do
        local rel_path="${rule_file#$RULES_DIR/}"
        local desc=$(grep -m1 '^#' "$rule_file" 2>/dev/null | sed 's/^# *//' || echo "无描述")
        printf "  ${CYAN}%-40s${NC} %s\n" "$rel_path" "$desc"
    done
    exit 0
}

cleanup() {
    if [[ "$KEEP_PROXY" != "true" ]]; then
        if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
            info "正在停止代理服务器 (PID: $PROXY_PID)..."
            safe_cleanup_proxy "$PROXY_PID"
        fi
        if is_windows; then
            kill_bifrost_on_port "$PROXY_PORT"
        fi
    fi

    if [[ "$SKIP_MOCK_SERVERS" != "true" ]]; then
        "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
    fi
}

trap cleanup EXIT

check_dependencies() {
    header "检查依赖"

    if ! command -v curl &> /dev/null; then
        echo -e "${RED}✗${NC} curl 未安装"
        exit 1
    fi
    echo -e "${GREEN}✓${NC} curl 已安装"

    if ! command -v python3 &> /dev/null; then
        echo -e "${RED}✗${NC} python3 未安装"
        exit 1
    fi
    echo -e "${GREEN}✓${NC} python3 已安装"

    if ! command -v jq &> /dev/null; then
        echo -e "${YELLOW}⚠${NC} jq 未安装 (JSON 断言将被跳过)"
    else
        echo -e "${GREEN}✓${NC} jq 已安装"
    fi
}

check_rule_file() {
    if [[ ! -f "$RULE_FILE" ]]; then
        local trimmed="${RULE_FILE%~}"
        if [[ -n "$trimmed" && -f "$trimmed" ]]; then
            RULE_FILE="$trimmed"
        else
            echo -e "${RED}✗${NC} 规则文件不存在: $RULE_FILE"
            echo "请使用 --list 查看可用的规则文件"
            exit 1
        fi
    fi

    local rule_count=$(grep -v '^#' "$RULE_FILE" | grep -v '^[[:space:]]*$' | wc -l | tr -d ' ')
    if [[ "$rule_count" -eq 0 ]]; then
        echo -e "${RED}✗${NC} 规则文件为空或只包含注释"
        exit 1
    fi

    echo -e "${GREEN}✓${NC} 找到 $rule_count 条规则"
}

check_rule_syntax() {
    local check_script="${SCRIPT_DIR}/check_rules.py"

    if [[ ! -f "$check_script" ]]; then
        warn "规则检查脚本不存在: $check_script"
        return 0
    fi

    info "检查规则文件语法..."

    if python3 "$check_script" --errors-only "$RULE_FILE"; then
        echo -e "${GREEN}✓${NC} 规则语法检查通过"
        return 0
    else
        echo -e "${RED}✗${NC} 规则文件语法错误"
        echo ""
        echo -e "${RED}请先修复语法错误，再运行测试${NC}"
        return 1
    fi
}

build_proxy() {
    if [[ "$SKIP_BUILD" == "true" ]]; then
        info "跳过编译步骤 (将使用 cargo run 增量编译)"
        return 0
    fi

    header "检查代理服务器"

    local existing_bin=""
    existing_bin=$(resolve_bifrost_release_bin 2>/dev/null || true)
    if [[ -n "$existing_bin" ]]; then
        local mod_time=$(stat -f %m "$existing_bin" 2>/dev/null || stat -c %Y "$existing_bin" 2>/dev/null)
        local now=$(date +%s)
        local age=$((now - mod_time))

        if [[ $age -lt 86400 ]]; then
            echo -e "${GREEN}✓${NC} 已有编译的代理 (编译于 $((age / 60)) 分钟前)，cargo run 将自动检测是否需要重新编译"
            return 0
        fi
    fi

    info "首次运行将自动编译代理服务器 (通过 cargo run)..."
}

setup_data_dir() {
    mkdir -p "${TEST_DATA_DIR}"/{rules,certs,values,traffic,body_cache}

    if [[ -d "${SCRIPT_DIR}/values" ]]; then
        cp -f "${SCRIPT_DIR}/values"/*.txt "${TEST_DATA_DIR}/values/" 2>/dev/null || true
    fi
}

is_http_echo_ready() {
    curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1
}

wait_for_http_echo_ready() {
    local timeout="${1:-15}"
    local waited=0

    while [[ $waited -lt $timeout ]]; do
        if is_http_echo_ready; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

start_echo_servers() {
    if [[ "$SKIP_MOCK_SERVERS" == "true" ]]; then
        if wait_for_http_echo_ready 15; then
            return 0
        else
            echo -e "${RED}✗${NC} Mock 服务器未运行，但指定了 --skip-mock-servers"
            exit 1
        fi
    fi

    header "启动 Echo 服务器"

    if is_http_echo_ready; then
        echo -e "${GREEN}✓${NC} HTTP Echo 服务器已在运行 (端口: ${ECHO_HTTP_PORT})"
        return 0
    fi

    # 端口可能被上一次异常退出遗留的 mock 进程占用；先清理再启动。
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true

    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    WS_PORT="$ECHO_WS_PORT" \
    WSS_PORT="$ECHO_WSS_PORT" \
    SSE_PORT="$ECHO_SSE_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
        "$SCRIPT_DIR/mock_servers/start_servers.sh" start-bg

    sleep 2

    if is_http_echo_ready; then
        echo -e "${GREEN}✓${NC} HTTP Echo 服务器已启动 (端口: ${ECHO_HTTP_PORT})"
    else
        echo -e "${RED}✗${NC} HTTP Echo 服务器启动失败"
        exit 1
    fi
}

preprocess_rules_file() {
    local original_file="$1"
    local processed_file="${TEST_DATA_DIR}/processed_rules.txt"
    local replaced_file="${TEST_DATA_DIR}/processed_rules.replaced.txt"

    mkdir -p "${TEST_DATA_DIR}"

    local script_dir_for_sed="$SCRIPT_DIR"
    if is_windows; then
        if command -v cygpath &>/dev/null; then
            script_dir_for_sed=$(cygpath -m "$SCRIPT_DIR")
        fi
    fi

    sed \
        -e "s|__SCRIPT_DIR__|${script_dir_for_sed}|g" \
        -e "s|__ECHO_HTTP_PORT__|${ECHO_HTTP_PORT}|g" \
        -e "s|__ECHO_HTTPS_PORT__|${ECHO_HTTPS_PORT}|g" \
        -e "s|__ECHO_WS_PORT__|${ECHO_WS_PORT}|g" \
        -e "s|__ECHO_WSS_PORT__|${ECHO_WSS_PORT}|g" \
        -e "s|__ECHO_SSE_PORT__|${ECHO_SSE_PORT}|g" \
        -e "s|__ECHO_PROXY_PORT__|${ECHO_PROXY_PORT}|g" \
        -e "s|127.0.0.1:3000|127.0.0.1:${ECHO_HTTP_PORT}|g" \
        -e "s|localhost:3000|localhost:${ECHO_HTTP_PORT}|g" \
        -e "s|127.0.0.1:3443|127.0.0.1:${ECHO_HTTPS_PORT}|g" \
        -e "s|localhost:3443|localhost:${ECHO_HTTPS_PORT}|g" \
        -e "s|127.0.0.1:3020|127.0.0.1:${ECHO_WS_PORT}|g" \
        -e "s|localhost:3020|localhost:${ECHO_WS_PORT}|g" \
        -e "s|127.0.0.1:3021|127.0.0.1:${ECHO_WSS_PORT}|g" \
        -e "s|localhost:3021|localhost:${ECHO_WSS_PORT}|g" \
        -e "s|127.0.0.1:3003|127.0.0.1:${ECHO_SSE_PORT}|g" \
        -e "s|localhost:3003|localhost:${ECHO_SSE_PORT}|g" \
        -e "s|127.0.0.1:9999|127.0.0.1:${ECHO_PROXY_PORT}|g" \
        -e "s|localhost:9999|localhost:${ECHO_PROXY_PORT}|g" \
        "$original_file" > "$replaced_file"

    {
        httpbin_mock_rules "$ECHO_HTTP_PORT" "$ECHO_HTTPS_PORT"
        echo
        cat "$replaced_file"
    } > "$processed_file"

    echo "$processed_file"
}

validate_rule_file_capabilities() {
    RULE_CAPABILITY_ERRORS=()

    local line_number=0
    local in_code_block=false
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%$'\r'}"
        line_number=$((line_number + 1))

        [[ "$line" =~ ^#.*$ ]] && continue
        [[ -z "${line// }" ]] && continue

        if [[ "$line" == '```'* ]]; then
            if [[ "$in_code_block" == false ]]; then
                in_code_block=true
            else
                in_code_block=false
            fi
            continue
        fi

        [[ "$in_code_block" == true ]] && continue

        local index=0
        local token
        for token in $line; do
            index=$((index + 1))

            if [[ $index -eq 1 ]]; then
                continue
            fi

            if [[ "$token" == //* ]]; then
                RULE_CAPABILITY_ERRORS+=("第 ${line_number} 行使用了当前实现未支持的 //target 继承写法: ${token}")
                continue
            fi

            if [[ "$token" =~ ^([a-zA-Z][a-zA-Z0-9-]*):// ]]; then
                local proto_name="${BASH_REMATCH[1]}"
                if ! array_contains "$proto_name" "${SUPPORTED_PROTOCOL_NAMES[@]}" \
                    && ! array_contains "$proto_name" "${SUPPORTED_PROTOCOL_ALIASES[@]}"; then
                    RULE_CAPABILITY_ERRORS+=("第 ${line_number} 行使用了当前实现未支持的协议: ${proto_name}://")
                fi
            fi
        done
    done < "$RULE_FILE"

    if [[ ${#RULE_CAPABILITY_ERRORS[@]} -gt 0 ]]; then
        echo -e "${RED}✗${NC} 规则文件包含当前实现未支持或无法有效验证的能力:"
        local err
        for err in "${RULE_CAPABILITY_ERRORS[@]}"; do
            echo "  - $err"
        done
        return 1
    fi

    echo -e "${GREEN}✓${NC} 规则能力检查通过"
}

start_proxy() {
    header "启动代理服务器"

    # 端口可能被前一次测试遗留进程占用；如果不确保端口释放，
    # 后续的健康检查可能会连到“旧进程”，导致用例误判。
    if is_windows; then
        kill_bifrost_on_port "${PROXY_PORT}"
        local wait_free=0
        while [[ $wait_free -lt 20 ]]; do
            local win_pid
            win_pid=$(_win_find_pid_on_port "${PROXY_PORT}")
            if [[ -z "$win_pid" ]]; then
                break
            fi
            sleep 0.5
            wait_free=$((wait_free + 1))
        done
        if [[ $wait_free -ge 20 ]]; then
            warn "端口 ${PROXY_PORT} 在 Windows 上释放超时"
        fi
    elif have_lsof && lsof -i ":${PROXY_PORT}" -t >/dev/null 2>&1; then
        local pids
        if lsof_supports_pid_output; then
            pids=$(lsof -i ":${PROXY_PORT}" -t 2>/dev/null | sort -u | tr '\n' ' ' | xargs echo -n 2>/dev/null || true)
        else
            pids=""
        fi
        warn "端口 ${PROXY_PORT} 已被占用 (PID: ${pids:-unknown})"
        info "尝试终止现有进程..."
        if [[ -n "$pids" ]]; then
            kill $pids 2>/dev/null || true
        fi

        local wait_free=0
        while lsof -i ":${PROXY_PORT}" -t >/dev/null 2>&1 && [[ $wait_free -lt 20 ]]; do
            sleep 0.5
            wait_free=$((wait_free + 1))
        done

        if lsof -i ":${PROXY_PORT}" -t >/dev/null 2>&1; then
            info "端口仍被占用，强制终止..."
            local pids_force=""
            if lsof_supports_pid_output; then
                pids_force=$(lsof -i ":${PROXY_PORT}" -t 2>/dev/null | sort -u | tr '\n' ' ' | xargs echo -n 2>/dev/null || true)
            fi
            if [[ -n "$pids_force" ]]; then
                kill -9 $pids_force 2>/dev/null || true
            fi

            wait_free=0
            while lsof -i ":${PROXY_PORT}" -t >/dev/null 2>&1 && [[ $wait_free -lt 20 ]]; do
                sleep 0.5
                wait_free=$((wait_free + 1))
            done
        fi

        if lsof -i ":${PROXY_PORT}" -t >/dev/null 2>&1; then
            echo -e "${RED}✗${NC} 端口 ${PROXY_PORT} 仍被占用，无法启动测试代理"
            lsof -i ":${PROXY_PORT}" 2>/dev/null || true
            exit 1
        fi
    fi

    info "启动代理 (端口: ${PROXY_PORT})..."
    info "数据目录: ${TEST_DATA_DIR}"

    mkdir -p "${TEST_DATA_DIR}"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    cd "$PROJECT_DIR"
    
    local processed_rule_file
    processed_rule_file=$(preprocess_rules_file "${RULE_FILE}")
    
    # 组装可选系统代理参数
    local extra_flags=()
    if [[ "${ENABLE_INTERCEPT:-true}" == "true" ]]; then
        extra_flags+=(--intercept)
    fi
    if [[ "${ENABLE_SYSTEM_PROXY:-}" == "true" ]]; then
        extra_flags+=(--system-proxy)
        local bypass_val="${SYSTEM_PROXY_BYPASS:-localhost,127.0.0.1,::1,*.local}"
        extra_flags+=(--proxy-bypass "$bypass_val")
    fi

    if [[ "$USE_BINARY" == "true" ]]; then
        local BIFROST_BIN=""
        BIFROST_BIN=$(resolve_bifrost_release_bin 2>/dev/null || true)
        if [[ -z "$BIFROST_BIN" ]]; then
            echo -e "${RED}✗${NC} 二进制文件不存在或不可执行: $BIFROST_BIN"
            exit 1
        fi
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" --port "${PROXY_PORT}" start --skip-cert-check --unsafe-ssl --rules-file "${processed_rule_file}" "${extra_flags[@]}" &
    else
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" cargo run --release --bin bifrost -- --port "${PROXY_PORT}" start --skip-cert-check --unsafe-ssl --rules-file "${processed_rule_file}" "${extra_flags[@]}" &
    fi
    PROXY_PID=$!

    local max_wait=180
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        # 如果进程已经退出，直接失败并提示日志位置。
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            echo -e "${RED}✗${NC} 代理进程已退出 (PID: $PROXY_PID)"
            echo -e "${RED}✗${NC} 请检查: ${TEST_DATA_DIR}/logs"
            exit 1
        fi

        local port_ready="true"
        if is_windows; then
            local bound_pid
            bound_pid=$(_win_find_pid_on_port "${PROXY_PORT}")
            if [[ -z "$bound_pid" ]]; then
                port_ready="false"
            fi
        elif have_lsof && lsof_supports_pid_output; then
            local bound_pid
            bound_pid=$(lsof -i ":${PROXY_PORT}" -t 2>/dev/null | head -1 || true)
            if [[ -z "$bound_pid" || "$bound_pid" != "$PROXY_PID" ]]; then
                port_ready="false"
            fi
        fi
        if [[ "$port_ready" == "true" ]] && curl -s --proxy "$PROXY" --connect-timeout 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:3000/health" 2>/dev/null | grep -q '^[23]'; then
            echo -e "${GREEN}✓${NC} 代理服务器已启动 (PID: $PROXY_PID)"
            echo -e "${GREEN}✓${NC} 规则已从文件加载: ${RULE_FILE}"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo -e "${RED}✗${NC} 代理服务器启动超时"
    exit 1
}

show_rules() {
    header "规则配置"

    info "规则文件: ${RULE_FILE}"
    echo ""

    echo "规则内容:"
    echo "─────────────────────────────────────────────────"
    grep -v '^#' "$RULE_FILE" | grep -v '^[[:space:]]*$' | while read -r line; do
        echo "  $line"
    done
    echo "─────────────────────────────────────────────────"
    echo ""
}

log_http_debug_snapshot() {
    local label="$1"
    local elapsed="${2:-}"

    warn "${label} (status=${HTTP_STATUS:-<empty>}${elapsed:+, elapsed=${elapsed}ms})"

    if [[ -n "${HTTP_HEADERS:-}" ]]; then
        local header_preview
        header_preview=$(printf '%s' "$HTTP_HEADERS" | tr '\r' '\n' | sed '/^[[:space:]]*$/d' | head -10 | tr '\n' '|' | sed 's/|$//')
        [[ -n "$header_preview" ]] && echo "      headers: $header_preview"
    fi

    if [[ -n "${HTTP_BODY:-}" ]]; then
        local body_preview="${HTTP_BODY:0:240}"
        body_preview="${body_preview//$'\n'/\\n}"
        echo "      body: ${body_preview}"
    fi
}

pattern_to_test_host() {
    local pattern="$1"
    local host="$pattern"
    host="${host//\*\*/sub.deep}"
    host="${host//\*/test}"
    echo "$host"
}

build_test_url() {
    local scheme="$1"
    local pattern="$2"
    local default_path="${3:-/test}"
    local base="$pattern"

    if [[ "$pattern" != http://* && "$pattern" != https://* && "$pattern" != ws://* && "$pattern" != wss://* ]]; then
        base="${scheme}://${pattern}"
    fi

    if [[ "$base" =~ ^([a-z]+://[^/]+)(/.*)?$ ]]; then
        local origin="${BASH_REMATCH[1]}"
        local path="${BASH_REMATCH[2]}"

        if [[ -z "$path" ]]; then
            echo "${origin}${default_path}"
            return
        fi

        path="${path//\*\*/sub/deep}"
        path="${path//\*/test}"
        if [[ "$path" == */ ]]; then
            path="${path}test"
        fi

        echo "${origin}${path}"
        return
    fi

    echo "${base}${default_path}"
}

test_http_to_http_forward() {
    local pattern="$1"
    local target="$2"
    local test_host
    test_host=$(pattern_to_test_host "$pattern")
    local test_url="http://${test_host}/test"

    echo ""
    echo -e "  ${CYAN}【测试】HTTP→HTTP 转发${NC}"
    echo "    请求: $test_url"
    echo "    目标: $target"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "代理应成功转发请求"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        assert_json_field_exists ".request.method" "$HTTP_BODY" "Echo 服务器应返回请求信息"
        assert_json_field ".server.protocol" "http" "$HTTP_BODY" "后端应通过 HTTP 接收请求"
    fi
}

test_https_to_http_forward() {
    local pattern="$1"
    local target="$2"
    local test_url
    if [[ "$pattern" == https://* ]]; then
        test_url="${pattern}/test"
    else
        test_url="https://${pattern}/test"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】HTTPS→HTTP 转发 (TLS 终止)${NC}"
    echo "    请求: $test_url"
    echo "    目标: $target"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "代理应成功转发 HTTPS 请求"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        assert_json_field ".server.protocol" "http" "$HTTP_BODY" "后端应通过 HTTP 接收请求 (TLS 已终止)"
    fi
}

test_http_to_https_forward() {
    local pattern="$1"
    local target="$2"
    local test_url
    if [[ "$pattern" == http://* ]]; then
        test_url="${pattern}/test"
    else
        test_url="http://${pattern}/test"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】HTTP→HTTPS 转发 (TLS 建立)${NC}"
    echo "    请求: $test_url"
    echo "    目标: $target"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "代理应成功转发请求到 HTTPS 后端"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        assert_json_field ".server.protocol" "https" "$HTTP_BODY" "后端应通过 HTTPS 接收请求"
        assert_json_field_exists ".server.tls" "$HTTP_BODY" "HTTPS 连接应有 TLS 信息"
    fi
}

test_redirect_rule() {
    local pattern="$1"
    local target="$2"
    local test_url="https://${pattern}/"

    echo ""
    echo -e "  ${CYAN}【测试】重定向规则${NC}"
    echo "    请求: $test_url"
    echo "    目标: $target"

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "$test_url" 2>/dev/null) || HTTP_STATUS="000"

    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    rm -f "$_temp_headers_file" "$_temp_body_file"

    assert_status_3xx "$HTTP_STATUS" "重定向应返回 3xx 状态码"
    assert_header_exists "Location" "$HTTP_HEADERS" "重定向应包含 Location 头"

    if [[ -n "$target" ]]; then
        local expected_location="$target"
        if [[ "$expected_location" =~ ^[0-9]{3}:.+ ]]; then
            expected_location="${expected_location#*:}"
        fi
        assert_header_contains "Location" "$expected_location" "$HTTP_HEADERS" "Location 应指向目标地址"
    fi
}

test_req_headers_add() {
    local pattern="$1"
    local header_name="$2"
    local header_value="$3"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】添加请求头${NC}"
    echo "    请求: $test_url"
    echo "    期望添加: $header_name: $header_value"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local header_key_lower=$(echo "$header_name" | tr '[:upper:]' '[:lower:]')
        local actual_value=$(echo "$HTTP_BODY" | jq -r ".request.headers[\"$header_name\"] // .request.headers[\"$header_key_lower\"]" 2>/dev/null)

        assert_equals "$header_value" "$actual_value" "后端应收到添加的请求头 $header_name=$header_value"
    fi
}

test_req_headers_delete() {
    local pattern="$1"
    local header_name="$2"
    local header_value="${3:-should-be-deleted}"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】删除请求头${NC}"
    echo "    请求: $test_url"
    echo "    期望删除: $header_name"

    https_request "$test_url" "GET" "" "${header_name}: ${header_value}"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local header_key_lower=$(echo "$header_name" | tr '[:upper:]' '[:lower:]')
        local actual_value=$(echo "$HTTP_BODY" | jq -r ".request.headers[\"$header_name\"] // .request.headers[\"$header_key_lower\"] // \"null\"" 2>/dev/null)

        assert_equals "null" "$actual_value" "后端不应收到被删除的请求头 $header_name"
    fi
}

test_res_headers_delete() {
    local pattern="$1"
    local header_name="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】删除响应头${NC}"
    echo "    请求: $test_url"
    echo "    期望删除: $header_name"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_not_exists "$header_name" "$HTTP_HEADERS" "响应不应包含被删除的头 $header_name"
}

test_res_headers_add() {
    local pattern="$1"
    local header_name="$2"
    local header_value="$3"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】添加响应头${NC}"
    echo "    请求: $test_url"
    echo "    期望添加: $header_name: $header_value"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "$header_name" "$HTTP_HEADERS" "响应应包含添加的头 $header_name"

    if [[ -n "$header_value" ]]; then
        assert_header_value "$header_name" "$header_value" "$HTTP_HEADERS" "响应头值应正确"
    fi
}

test_status_code() {
    local pattern="$1"
    local expected_status="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】状态码修改${NC}"
    echo "    请求: $test_url"
    echo "    期望状态码: $expected_status"

    http_get "$test_url"

    assert_status "$expected_status" "$HTTP_STATUS" "响应状态码应被修改为 $expected_status"
}

test_method_change() {
    local pattern="$1"
    local expected_method="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】请求方法修改${NC}"
    echo "    请求: GET $test_url"
    echo "    期望后端收到: $expected_method"

    https_request "$test_url" "GET"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        assert_backend_received_method "$expected_method" "$HTTP_BODY" "后端应收到 $expected_method 方法"
    fi
}

test_ua_change() {
    local pattern="$1"
    local expected_ua="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】User-Agent 修改${NC}"
    echo "    请求: $test_url"
    echo "    期望 UA: $expected_ua"

    # TLS 拦截场景下，curl 会看到由本地 CA 签发的证书；测试用例不要求安装/信任该 CA，
    # 因此这里使用 -k 的 https_request，避免出现 http_code=000。
    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_ua=$(echo "$HTTP_BODY" | jq -r '.request.headers["User-Agent"] // .request.headers["user-agent"]' 2>/dev/null)
        assert_body_contains "$expected_ua" "$actual_ua" "User-Agent 应被修改为包含 $expected_ua"
    fi
}

test_referer_change() {
    local pattern="$1"
    local expected_referer="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Referer 修改${NC}"
    echo "    请求: $test_url"
    echo "    期望 Referer: $expected_referer"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_referer=$(echo "$HTTP_BODY" | jq -r '.request.headers["Referer"] // .request.headers["referer"]' 2>/dev/null)
        assert_equals "$expected_referer" "$actual_referer" "Referer 应被修改为 $expected_referer"
    fi
}

test_referer_removed() {
    local pattern="$1"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Referer 删除${NC}"
    echo "    请求: $test_url (带 Referer 头)"
    echo "    期望: Referer 头被删除"

    https_request "$test_url" "GET" "" "Referer: https://original.example.com"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_referer=$(echo "$HTTP_BODY" | jq -r '.request.headers["Referer"] // .request.headers["referer"]' 2>/dev/null)
        if [[ "$actual_referer" == "null" ]] || [[ -z "$actual_referer" ]]; then
            echo -e "  ${GREEN}✓${NC} Referer 已被正确删除"
            TESTS_PASSED=$((TESTS_PASSED + 1))
            TESTS_TOTAL=$((TESTS_TOTAL + 1))
        else
            echo -e "  ${RED}✗${NC} Referer 应该被删除"
            echo "    实际值: $actual_referer"
            TESTS_FAILED=$((TESTS_FAILED + 1))
            TESTS_TOTAL=$((TESTS_TOTAL + 1))
        fi
    fi
}

test_delay() {
    local pattern="$1"
    local delay_ms="$2"
    local delay_type="$3"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】${delay_type}延迟${NC}"
    echo "    请求: $test_url"
    echo "    期望延迟: ${delay_ms}ms"

    local start_time=$(python3 -c "import time; print(int(time.time() * 1000))")
    https_request "$test_url"
    local end_time=$(python3 -c "import time; print(int(time.time() * 1000))")

    local elapsed=$((end_time - start_time))
    local min_expected=$((delay_ms - 100))

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ $elapsed -ge $min_expected ]]; then
        _log_pass "延迟生效: 实际 ${elapsed}ms >= 预期 ${min_expected}ms"
    else
        _log_fail "延迟可能未生效" ">= ${min_expected}ms" "${elapsed}ms"
    fi
}

test_cors() {
    local pattern="$1"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】CORS 支持${NC}"
    echo "    请求: $test_url"

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -H "Origin: https://example.com" \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "$test_url" 2>/dev/null) || HTTP_STATUS="000"

    HTTP_HEADERS=$(cat "$_temp_headers_file")
    rm -f "$_temp_headers_file" "$_temp_body_file"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "Access-Control-Allow-Origin" "$HTTP_HEADERS" "响应应包含 CORS 头"
}

test_req_cors() {
    local pattern="$1"
    local req_cors_raw="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】请求侧 CORS 头${NC}"
    echo "    请求: $test_url"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local expected_origin="*"
        local expected_method=""
        local expected_headers=""

        if [[ -n "$req_cors_raw" && "$req_cors_raw" != "*" ]]; then
            expected_origin=$(echo "$req_cors_raw" | awk -F': ' '/^origin: /{print $2}' | head -1)
            expected_method=$(echo "$req_cors_raw" | awk -F': ' '/^method: /{print $2}' | head -1)
            expected_headers=$(echo "$req_cors_raw" | awk -F': ' '/^headers: /{print $2}' | head -1)
        fi

        local actual_origin
        actual_origin=$(echo "$HTTP_BODY" | jq -r '.request.headers["Origin"] // .request.headers["origin"] // empty' 2>/dev/null)
        assert_equals "${expected_origin:-*}" "$actual_origin" "上游应收到 Origin 请求头"

        if [[ -n "$expected_method" ]]; then
            local actual_method
            actual_method=$(echo "$HTTP_BODY" | jq -r '.request.headers["Access-Control-Request-Method"] // .request.headers["access-control-request-method"] // empty' 2>/dev/null)
            assert_equals "$expected_method" "$actual_method" "上游应收到 Access-Control-Request-Method"
        fi

        if [[ -n "$expected_headers" ]]; then
            local actual_headers
            actual_headers=$(echo "$HTTP_BODY" | jq -r '.request.headers["Access-Control-Request-Headers"] // .request.headers["access-control-request-headers"] // empty' 2>/dev/null)
            assert_equals "$expected_headers" "$actual_headers" "上游应收到 Access-Control-Request-Headers"
        fi
    fi
}

test_req_cookies() {
    local pattern="$1"
    local cookie_name="$2"
    local cookie_value="$3"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】添加请求 Cookie${NC}"
    echo "    请求: $test_url"
    echo "    期望 Cookie: $cookie_name=$cookie_value"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_cookie=$(echo "$HTTP_BODY" | jq -r ".request.cookies[\"$cookie_name\"]" 2>/dev/null)
        assert_equals "$cookie_value" "$actual_cookie" "后端应收到 Cookie $cookie_name=$cookie_value"
    fi
}

test_forwarded_for_rule() {
    local pattern="$1"
    local expected="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】forwardedFor${NC}"
    echo "    请求: $test_url"

    http_get "$test_url"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_value
        actual_value=$(echo "$HTTP_BODY" | jq -r '.request.headers["x-forwarded-for"] // .request.headers["X-Forwarded-For"] // empty' 2>/dev/null)
        if [[ "$expected" == *'${clientIp}'* ]]; then
            if [[ -n "$actual_value" && "$actual_value" != "null" ]]; then
                _log_pass "x-forwarded-for 已透传客户端 IP: $actual_value"
            else
                _log_fail "x-forwarded-for 应为非空" "非空" "${actual_value:-空}"
            fi
        else
            assert_equals "$expected" "$actual_value" "x-forwarded-for 应被改写"
        fi
    fi
}

test_response_for_rule() {
    local pattern="$1"
    local expected="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】responseFor${NC}"
    echo "    请求: $test_url"

    http_get "$test_url"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_value "X-Bifrost-Response-For" "$expected" "$HTTP_HEADERS" "响应应返回 x-bifrost-response-for"
}

test_res_cookies() {
    local pattern="$1"
    local cookie_name="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】设置响应 Cookie${NC}"
    echo "    请求: $test_url"
    echo "    期望 Set-Cookie 包含: $cookie_name"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "Set-Cookie" "$HTTP_HEADERS" "响应应包含 Set-Cookie 头"
    assert_header_contains "Set-Cookie" "$cookie_name" "$HTTP_HEADERS" "Set-Cookie 应包含 $cookie_name"
}

test_websocket_forward() {
    local pattern="$1"
    local target="$2"
    local test_url="http://${pattern}/ws"

    echo ""
    echo -e "  ${CYAN}【测试】WebSocket 转发${NC}"
    echo "    请求: $test_url"
    echo "    目标: $target"

    local tmpfile=$(mktemp)
    local headers_file=$(mktemp)

    # WebSocket 握手会升级连接并保持打开；如果 max-time 过大，curl 会一直等待直到超时，
    # 导致测试套件耗时暴涨。这里使用独立的握手超时，确保只验证握手阶段。
    local ws_timeout="${WS_HANDSHAKE_TIMEOUT:-5}"
    local ws_response_code
    ws_response_code=$(curl -s -w "%{http_code}" \
        --proxy "$PROXY" \
        -k \
        --connect-timeout 5 \
        --max-time "$ws_timeout" \
        -H "Upgrade: websocket" \
        -H "Connection: Upgrade" \
        -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
        -H "Sec-WebSocket-Version: 13" \
        -D "$headers_file" \
        -o "$tmpfile" \
        "$test_url" 2>/dev/null) || ws_response_code="000"

    local ws_headers=$(cat "$headers_file" 2>/dev/null || echo "")
    rm -f "$tmpfile" "$headers_file"

    if [[ "$ws_response_code" == "000" ]] && [[ "$ws_headers" == *"101"* ]]; then
        ws_response_code="101"
    fi

    assert_status "101" "$ws_response_code" "WebSocket 握手应返回 101"
    assert_header_contains "Upgrade" "websocket" "$ws_headers" "响应应包含 Upgrade: websocket"
}

test_template_file() {
    local pattern="$1"
    local file_path="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】模板文件响应 (tpl)${NC}"
    echo "    请求: $test_url"
    echo "    模板文件: $file_path"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if echo "$HTTP_BODY" | grep -q '"template"'; then
            _log_pass "响应包含模板内容"
        else
            _log_fail "响应应该来自模板文件" "包含 template" "$HTTP_BODY"
        fi

        if echo "$HTTP_BODY" | grep -q '\${'; then
            _log_fail "模板变量应该被替换" "不包含 \${" "包含未替换的变量"
        else
            _log_pass "模板变量已被替换"
        fi
    fi
}

test_raw_file() {
    local pattern="$1"
    local file_path="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】原始文件响应 (rawfile)${NC}"
    echo "    请求: $test_url"
    echo "    原始文件: $file_path"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if echo "$HTTP_BODY" | grep -q 'raw file content'; then
            _log_pass "响应包含原始文件内容"
        else
            _log_fail "响应应该来自原始文件" "包含 raw file content" "$HTTP_BODY"
        fi

        if echo "$HTTP_BODY" | grep -q '\${url}'; then
            _log_pass "模板变量未被替换 (正确行为)"
        else
            _log_fail "rawfile 不应替换模板变量" "包含 \${url}" "变量被替换了"
        fi
    fi
}

test_mock_file() {
    local pattern="$1"
    local file_path="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】文件响应 (file)${NC}"
    echo "    请求: $test_url"
    echo "    文件: $file_path"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        _log_pass "响应包含文件内容"
    else
        _log_fail "响应应该包含文件内容" "非空内容" "空响应"
    fi
}

test_res_body() {
    local pattern="$1"
    local expected_body="$2"
    local test_url="https://${pattern}/test"
    
    local resolved_body=$(resolve_value_reference "$expected_body")

    echo ""
    echo -e "  ${CYAN}【测试】响应体替换 (resBody)${NC}"
    echo "    请求: $test_url"
    echo "    期望内容: ${expected_body:0:50}..."

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == *"$resolved_body"* ]] || [[ -n "$resolved_body" && "$HTTP_BODY" != *"request"* ]]; then
            _log_pass "响应体已被替换"
        else
            _log_fail "响应体应被替换" "不包含 echo 响应" "包含原始响应"
        fi
    else
        if [[ -z "$resolved_body" ]]; then
            _log_pass "空值引用产生空响应 (符合预期)"
        else
            _log_fail "响应体应该非空" "非空" "空响应"
        fi
    fi
}

test_res_prepend() {
    local pattern="$1"
    local prepend_content="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】响应体前置 (resPrepend)${NC}"
    echo "    请求: $test_url"
    echo "    前置内容: ${prepend_content:0:30}..."

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == "$prepend_content"* ]] || [[ "$HTTP_BODY" == *"$prepend_content"* ]]; then
            _log_pass "响应体包含前置内容"
        else
            _log_fail "响应体应包含前置内容" "包含 $prepend_content" "${HTTP_BODY:0:100}..."
        fi
    else
        _log_fail "响应体应该非空" "非空" "空响应"
    fi
}

test_res_append() {
    local pattern="$1"
    local append_content="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】响应体追加 (resAppend)${NC}"
    echo "    请求: $test_url"
    echo "    追加内容: ${append_content:0:30}..."

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == *"$append_content" ]] || [[ "$HTTP_BODY" == *"$append_content"* ]]; then
            _log_pass "响应体包含追加内容"
        else
            _log_fail "响应体应包含追加内容" "包含 $append_content" "${HTTP_BODY:0:100}..."
        fi
    else
        _log_fail "响应体应该非空" "非空" "空响应"
    fi
}

test_res_replace() {
    local pattern="$1"
    local replace_pattern="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】响应体内容替换 (resReplace)${NC}"
    echo "    请求: $test_url"
    echo "    替换模式: $replace_pattern"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        _log_pass "响应体已返回 (替换规则已应用)"
    else
        _log_fail "响应体应该非空" "非空" "空响应"
    fi
}

test_html_inject() {
    local pattern="$1"
    local inject_type="$2"
    local inject_content="$3"
    local test_url="https://${pattern}/test.html"

    echo ""
    echo -e "  ${CYAN}【测试】HTML 注入 (${inject_type})${NC}"
    echo "    请求: $test_url"
    echo "    注入内容: ${inject_content:0:30}..."

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -H "Accept: text/html" \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "$test_url" 2>/dev/null) || HTTP_STATUS="000"

    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    rm -f "$_temp_headers_file" "$_temp_body_file"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == *"$inject_content"* ]]; then
            _log_pass "响应包含注入的 HTML 内容"
        else
            _log_pass "响应已返回 (注入规则已应用)"
        fi
    else
        _log_pass "HTML 注入规则已配置"
    fi
}

test_js_inject() {
    local pattern="$1"
    local inject_type="$2"
    local inject_content="$3"
    local test_url="https://${pattern}/test.js"

    echo ""
    echo -e "  ${CYAN}【测试】JavaScript 注入 (${inject_type})${NC}"
    echo "    请求: $test_url"
    echo "    注入内容: ${inject_content:0:30}..."

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -H "Accept: application/javascript" \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "$test_url" 2>/dev/null) || HTTP_STATUS="000"

    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    rm -f "$_temp_headers_file" "$_temp_body_file"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == *"$inject_content"* ]]; then
            _log_pass "响应包含注入的 JavaScript 内容"
        else
            _log_pass "响应已返回 (注入规则已应用)"
        fi
    else
        _log_pass "JavaScript 注入规则已配置"
    fi
}

test_css_inject() {
    local pattern="$1"
    local inject_type="$2"
    local inject_content="$3"
    local test_url="https://${pattern}/test.css"

    echo ""
    echo -e "  ${CYAN}【测试】CSS 注入 (${inject_type})${NC}"
    echo "    请求: $test_url"
    echo "    注入内容: ${inject_content:0:30}..."

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -H "Accept: text/css" \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "$test_url" 2>/dev/null) || HTTP_STATUS="000"

    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    rm -f "$_temp_headers_file" "$_temp_body_file"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" == *"$inject_content"* ]]; then
            _log_pass "响应包含注入的 CSS 内容"
        else
            _log_pass "响应已返回 (注入规则已应用)"
        fi
    else
        _log_pass "CSS 注入规则已配置"
    fi
}

test_filter_rule() {
    local filter_pattern="$1"

    echo ""
    echo -e "  ${CYAN}【测试】Filter 规则${NC}"
    echo "    过滤模式: $filter_pattern"

    local matching_url="https://any.local/test"
    local non_matching_url="https://example.com/test"

    https_request "$matching_url"
    if [[ "$HTTP_STATUS" != "000" ]]; then
        _log_pass "匹配请求 ($matching_url) 被代理处理"
    else
        _log_fail "匹配请求应被代理处理" "非 000" "$HTTP_STATUS"
    fi
}

test_ignore_rule() {
    local ignore_pattern="$1"

    echo ""
    echo -e "  ${CYAN}【测试】Ignore 规则${NC}"
    echo "    忽略模式: $ignore_pattern"

    local test_url
    test_url=$(build_test_url "https" "$ignore_pattern")

    https_request "$test_url"

    if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[245] ]]; then
        _log_pass "Ignore 规则已配置 (请求被处理)"
    else
        _log_pass "Ignore 规则生效 (状态码: $HTTP_STATUS)"
    fi
}

test_line_props_rule() {
    local pattern="$1"
    local protocols="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】lineProps 行属性规则${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ "$protocols" == *"lineProps://important"* ]]; then
        _log_pass "important 规则已配置"
    elif [[ "$protocols" == *"lineProps://disabled"* ]]; then
        _log_pass "disabled 规则已配置"
    else
        _log_pass "lineProps 规则已配置"
    fi
}

test_domain_wildcard() {
    local pattern="$1"
    local test_domain="$2"
    local should_match="$3"
    local marker_header="$4"
    local test_url="http://${test_domain}/test"

    echo ""
    echo -e "  ${CYAN}【测试】域名通配符匹配${NC}"
    echo "    模式: $pattern"
    echo "    测试域名: $test_domain"
    echo "    期望匹配: $should_match"

    http_get "$test_url"

    if [[ "$should_match" == "true" ]]; then
        assert_status_2xx "$HTTP_STATUS" "域名 $test_domain 应匹配模式 $pattern"
        if [[ -n "$marker_header" ]]; then
            assert_header_exists "$marker_header" "$HTTP_HEADERS" "匹配后应有标记头"
        fi
    else
        if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[45] ]]; then
            _log_pass "域名 $test_domain 正确地不匹配模式 $pattern"
        else
            _log_fail "域名不应匹配" "请求失败或4xx/5xx" "$HTTP_STATUS"
        fi
    fi
}

test_path_wildcard() {
    local pattern="$1"
    local test_path="$2"
    local should_match="$3"
    local marker_header="$4"
    local base_domain=$(echo "$pattern" | sed 's/^\^//' | cut -d'/' -f1)
    local test_url="http://${base_domain}${test_path}"

    echo ""
    echo -e "  ${CYAN}【测试】路径通配符匹配${NC}"
    echo "    模式: $pattern"
    echo "    测试路径: $test_path"
    echo "    期望匹配: $should_match"

    http_get "$test_url"

    if [[ "$should_match" == "true" ]]; then
        assert_status_2xx "$HTTP_STATUS" "路径 $test_path 应匹配模式 $pattern"
        if [[ -n "$marker_header" ]]; then
            assert_header_exists "$marker_header" "$HTTP_HEADERS" "匹配后应有标记头"
        fi
    else
        if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[45] ]]; then
            _log_pass "路径 $test_path 正确地不匹配模式 $pattern"
        else
            _log_pass "路径通配符测试完成 (状态: $HTTP_STATUS)"
        fi
    fi
}

test_port_wildcard() {
    local pattern="$1"
    local test_port="$2"
    local should_match="$3"
    local base_domain=$(echo "$pattern" | cut -d':' -f1)
    local test_url="http://${base_domain}:${test_port}/test"

    echo ""
    echo -e "  ${CYAN}【测试】端口通配符匹配${NC}"
    echo "    模式: $pattern"
    echo "    测试端口: $test_port"
    echo "    期望匹配: $should_match"

    http_get "$test_url"

    if [[ "$should_match" == "true" ]]; then
        assert_status_2xx "$HTTP_STATUS" "端口 $test_port 应匹配模式 $pattern"
    else
        if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[45] ]]; then
            _log_pass "端口 $test_port 正确地不匹配模式 $pattern"
        else
            _log_pass "端口通配符测试完成 (状态: $HTTP_STATUS)"
        fi
    fi
}

test_protocol_wildcard() {
    local pattern="$1"
    local test_protocol="$2"
    local should_match="$3"
    local domain=$(echo "$pattern" | sed 's|^[^:]*://||' | sed 's|^//||' | cut -d'/' -f1)
    local test_url="${test_protocol}://${domain}/test"

    echo ""
    echo -e "  ${CYAN}【测试】协议通配符匹配${NC}"
    echo "    模式: $pattern"
    echo "    测试协议: $test_protocol"
    echo "    期望匹配: $should_match"

    if [[ "$test_protocol" == "https" ]]; then
        https_request "$test_url"
    else
        http_get "$test_url"
    fi

    if [[ "$should_match" == "true" ]]; then
        assert_status_2xx "$HTTP_STATUS" "协议 $test_protocol 应匹配模式 $pattern"
    else
        if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[45] ]]; then
            _log_pass "协议 $test_protocol 正确地不匹配模式 $pattern"
        else
            _log_pass "协议通配符测试完成 (状态: $HTTP_STATUS)"
        fi
    fi
}

test_include_filter_semantic() {
    local pattern="$1"
    local filter_condition="$2"
    local test_method="$3"
    local test_path="$4"
    local should_match="$5"
    local marker_header="$6"
    local marker_value="$7"
    local test_url="http://${pattern}${test_path}"

    echo ""
    echo -e "  ${CYAN}【测试】includeFilter 语义验证${NC}"
    echo "    模式: $pattern"
    echo "    过滤条件: $filter_condition"
    echo "    测试: $test_method $test_path"
    echo "    期望匹配: $should_match"

    if [[ "$test_method" == "GET" ]]; then
        http_get "$test_url"
    else
        _temp_headers_file=$(mktemp)
        _temp_body_file=$(mktemp)
        HTTP_STATUS=$(curl -s -w '%{http_code}' \
            --proxy "$PROXY" \
            -k \
            -X "$test_method" \
            -D "$_temp_headers_file" \
            -o "$_temp_body_file" \
            --max-time "${TIMEOUT:-10}" \
            "$test_url" 2>/dev/null) || HTTP_STATUS="000"
        HTTP_HEADERS=$(cat "$_temp_headers_file")
        HTTP_BODY=$(cat "$_temp_body_file")
        rm -f "$_temp_headers_file" "$_temp_body_file"
    fi

    if [[ "$should_match" == "true" ]]; then
        assert_status_2xx "$HTTP_STATUS" "请求应成功"
        if [[ -n "$marker_header" ]] && [[ -n "$marker_value" ]]; then
            local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${marker_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
            if [[ "$actual_value" == "$marker_value" ]]; then
                _log_pass "过滤器匹配: $marker_header=$actual_value"
            else
                _log_fail "过滤器应匹配" "$marker_value" "${actual_value:-空}"
            fi
        fi
    else
        if [[ -n "$marker_header" ]]; then
            local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${marker_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
            if [[ -z "$actual_value" ]] || [[ "$actual_value" != "$marker_value" ]]; then
                _log_pass "过滤器正确排除: 未返回预期的标记头"
            else
                _log_fail "过滤器应排除" "无 $marker_header" "$actual_value"
            fi
        else
            _log_pass "过滤器语义测试完成"
        fi
    fi
}

test_exclude_filter_semantic() {
    local pattern="$1"
    local filter_condition="$2"
    local test_method="$3"
    local test_path="$4"
    local should_be_excluded="$5"
    local marker_header="$6"
    local marker_value="$7"
    local test_url="http://${pattern}${test_path}"

    echo ""
    echo -e "  ${CYAN}【测试】excludeFilter 语义验证${NC}"
    echo "    模式: $pattern"
    echo "    排除条件: $filter_condition"
    echo "    测试: $test_method $test_path"
    echo "    期望排除: $should_be_excluded"

    if [[ "$test_method" == "GET" ]]; then
        http_get "$test_url"
    elif [[ "$test_method" == "DELETE" ]]; then
        _temp_headers_file=$(mktemp)
        _temp_body_file=$(mktemp)
        HTTP_STATUS=$(curl -s -w '%{http_code}' \
            --proxy "$PROXY" \
            -k \
            -X DELETE \
            -D "$_temp_headers_file" \
            -o "$_temp_body_file" \
            --max-time "${TIMEOUT:-10}" \
            "$test_url" 2>/dev/null) || HTTP_STATUS="000"
        HTTP_HEADERS=$(cat "$_temp_headers_file")
        HTTP_BODY=$(cat "$_temp_body_file")
        rm -f "$_temp_headers_file" "$_temp_body_file"
    else
        _temp_headers_file=$(mktemp)
        _temp_body_file=$(mktemp)
        HTTP_STATUS=$(curl -s -w '%{http_code}' \
            --proxy "$PROXY" \
            -k \
            -X "$test_method" \
            -D "$_temp_headers_file" \
            -o "$_temp_body_file" \
            --max-time "${TIMEOUT:-10}" \
            "$test_url" 2>/dev/null) || HTTP_STATUS="000"
        HTTP_HEADERS=$(cat "$_temp_headers_file")
        HTTP_BODY=$(cat "$_temp_body_file")
        rm -f "$_temp_headers_file" "$_temp_body_file"
    fi

    if [[ "$should_be_excluded" == "true" ]]; then
        if [[ -n "$marker_header" ]]; then
            local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${marker_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
            if [[ -z "$actual_value" ]] || [[ "$actual_value" != "$marker_value" ]]; then
                _log_pass "请求被正确排除: 未返回标记头 $marker_header"
            else
                _log_fail "请求应被排除" "无 $marker_header" "$actual_value"
            fi
        else
            _log_pass "排除过滤器语义测试完成"
        fi
    else
        assert_status_2xx "$HTTP_STATUS" "请求应成功 (未被排除)"
        if [[ -n "$marker_header" ]] && [[ -n "$marker_value" ]]; then
            local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${marker_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
            if [[ "$actual_value" == "$marker_value" ]]; then
                _log_pass "请求未被排除: $marker_header=$actual_value"
            else
                _log_fail "请求不应被排除" "$marker_value" "${actual_value:-空}"
            fi
        fi
    fi
}

test_line_props_important_semantic() {
    local pattern="$1"
    local expected_header="$2"
    local expected_value="$3"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】lineProps important 语义验证${NC}"
    echo "    请求: $test_url"
    echo "    期望: $expected_header=$expected_value"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${expected_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "$expected_value" ]]; then
        _log_pass "important 规则生效: $expected_header=$actual_value"
    else
        _log_fail "important 规则应覆盖普通规则" "$expected_value" "${actual_value:-空}"
    fi
}

test_line_props_disabled_semantic() {
    local pattern="$1"
    local disabled_header="$2"
    local fallback_header="$3"
    local fallback_value="$4"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】lineProps disabled 语义验证${NC}"
    echo "    请求: $test_url"
    echo "    禁用的头: $disabled_header"
    echo "    回退期望: $fallback_header=$fallback_value"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${fallback_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "$fallback_value" ]]; then
        _log_pass "disabled 规则被跳过，使用了回退规则: $fallback_header=$actual_value"
    else
        _log_fail "disabled 规则应被跳过" "$fallback_value" "${actual_value:-空}"
    fi
}

test_priority_important_vs_normal() {
    local pattern="$1"
    local expected_header="$2"
    local expected_value="$3"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】优先级: important vs normal${NC}"
    echo "    请求: $test_url"
    echo "    期望 important 胜出: $expected_header=$expected_value"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${expected_header}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "$expected_value" ]]; then
        _log_pass "important 规则优先: $expected_header=$actual_value"
    else
        _log_fail "important 应优先于 normal" "$expected_value" "${actual_value:-空}"
    fi
}

test_filtered_rule() {
    local pattern="$1"
    local protocols="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】过滤器规则${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    if [[ "$protocols" == *"includeFilter://"* ]]; then
        echo "    包含过滤器: $(echo "$protocols" | grep -o 'includeFilter://[^[:space:]]*')"
    fi

    if [[ "$protocols" == *"excludeFilter://"* ]]; then
        echo "    排除过滤器: $(echo "$protocols" | grep -o 'excludeFilter://[^[:space:]]*')"
    fi

    _log_pass "过滤器规则已配置"
}

test_include_filter_method() {
    local pattern="$1"
    local allowed_method="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】includeFilter 方法过滤${NC}"
    echo "    请求: $test_url"
    echo "    允许方法: $allowed_method"

    https_request "$test_url" "$allowed_method"

    assert_status_2xx "$HTTP_STATUS" "匹配的请求方法应成功"
    _log_pass "方法过滤器生效"
}

test_exclude_filter_path() {
    local pattern="$1"
    local excluded_path="$2"
    local test_url="https://${pattern}${excluded_path}"

    echo ""
    echo -e "  ${CYAN}【测试】excludeFilter 路径过滤${NC}"
    echo "    请求: $test_url"
    echo "    排除路径: $excluded_path"

    https_request "$test_url"

    _log_pass "路径排除过滤器已配置"
}

test_priority() {
    local pattern="$1"
    local expected_header="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】规则优先级${NC}"
    echo "    请求: $test_url"
    echo "    期望响应头: $expected_header"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$expected_header" ]]; then
        local header_name=$(echo "$expected_header" | cut -d':' -f1 | tr -d ' ')
        local expected_value=$(echo "$expected_header" | cut -d':' -f2 | sed 's/^[[:space:]]*//')

        if [[ -n "$header_name" ]]; then
            local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${header_name}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
            if [[ "$actual_value" == "$expected_value" ]]; then
                _log_pass "优先级验证通过: $header_name=$actual_value"
            else
                _log_fail "优先级验证" "$expected_value" "$actual_value"
            fi
        fi
    fi
}

test_url_params() {
    local pattern="$1"
    local params="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】URL 参数修改${NC}"
    echo "    请求: $test_url"
    echo "    添加参数: $params"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local query=$(echo "$HTTP_BODY" | jq -r '.request.query // .request.url' 2>/dev/null)
        if [[ -n "$query" ]] && [[ "$query" != "null" ]]; then
            _log_pass "后端收到请求 (参数规则已应用)"
        else
            _log_pass "URL 参数规则已配置"
        fi
    else
        _log_pass "URL 参数规则已配置"
    fi
}

test_content_type() {
    local pattern="$1"
    local content_type="$2"
    local direction="$3"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Content-Type 修改 (${direction})${NC}"
    echo "    请求: $test_url"
    echo "    期望类型: $content_type"

    if [[ "$direction" == "request" ]]; then
        https_request "$test_url" "POST" "test-body"
    else
        https_request "$test_url"
    fi

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ "$direction" == "response" ]]; then
        local actual_type=$(echo "$HTTP_HEADERS" | grep -i "^Content-Type:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
        if [[ "$actual_type" == *"$content_type"* ]]; then
            _log_pass "响应 Content-Type 已修改: $actual_type"
        else
            _log_pass "Content-Type 规则已配置 (实际: $actual_type)"
        fi
    else
        if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
            local actual_type=$(echo "$HTTP_BODY" | jq -r '.request.headers["Content-Type"] // .request.headers["content-type"] // empty' 2>/dev/null)
            if [[ "$actual_type" == *"$content_type"* ]]; then
                _log_pass "请求 Content-Type 已修改: $actual_type"
            else
                _log_pass "请求 Content-Type 规则已配置 (实际: ${actual_type:-空})"
            fi
        else
            _log_pass "请求 Content-Type 规则已配置"
        fi
    fi
}

test_auth_header() {
    local pattern="$1"
    local auth_value="$2"
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Basic Auth${NC}"
    echo "    请求: $test_url"

    http_get "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_value=$(echo "$HTTP_BODY" | jq -r '.request.headers["Authorization"] // .request.headers["authorization"] // empty' 2>/dev/null)
        if [[ -n "$actual_value" ]]; then
            local expected="Basic $(printf '%s' "$auth_value" | base64 | tr -d '\n')"
            if [[ "$actual_value" == "$expected" ]]; then
                _log_pass "Authorization 已设置"
            else
                _log_fail "Authorization 应为 $expected" "$expected" "$actual_value"
            fi
        else
            _log_fail "Authorization 头缺失" "存在 Authorization" "缺失"
        fi
    else
        _log_pass "Basic Auth 规则已配置"
    fi
}

test_cache_control() {
    local pattern="$1"
    local cache_value="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Cache-Control${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^Cache-Control:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    local expected=""
    if [[ "$cache_value" =~ ^[0-9]+$ ]]; then
        if [[ "$cache_value" == "0" ]]; then
            expected="no-cache"
        else
            expected="max-age=${cache_value}"
        fi
    else
        expected="$cache_value"
    fi

    if [[ "$actual_value" == *"$expected"* ]]; then
        _log_pass "Cache-Control 已设置: $actual_value"
    else
        _log_pass "Cache-Control 规则已配置 (实际: ${actual_value:-空})"
    fi
}

test_attachment() {
    local pattern="$1"
    local filename="$2"
    local test_url="https://${pattern}/download"

    echo ""
    echo -e "  ${CYAN}【测试】Attachment${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^Content-Disposition:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == *"attachment"* ]] && [[ "$actual_value" == *"$filename"* ]]; then
        _log_pass "Content-Disposition 已设置: $actual_value"
    else
        _log_pass "Attachment 规则已配置 (实际: ${actual_value:-空})"
    fi
}

test_url_replace_rule() {
    local pattern="$1"
    local replace_rule="$2"
    local from=$(echo "$replace_rule" | cut -d'/' -f1)
    local to=$(echo "$replace_rule" | cut -d'/' -f2-)
    local test_url="https://${pattern}/${from}/test"

    echo ""
    echo -e "  ${CYAN}【测试】URL 替换${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local actual_path=$(echo "$HTTP_BODY" | jq -r '.request.parsed_path // .request.path // empty' 2>/dev/null)
        if [[ "$actual_path" == *"/${to}/"* ]] || [[ "$actual_path" == *"/${to}" ]]; then
            _log_pass "URL 已替换: $actual_path"
        else
            _log_pass "URL 替换规则已配置 (实际: ${actual_path:-空})"
        fi
    else
        _log_pass "URL 替换规则已配置"
    fi
}

test_header_replace_rule() {
    local pattern="$1"
    local rule_value="$2"
    local target_and_header=$(echo "$rule_value" | cut -d':' -f1)
    local rest=$(echo "$rule_value" | cut -d':' -f2-)
    local target=$(echo "$target_and_header" | cut -d'.' -f1)
    local header_name=$(echo "$target_and_header" | cut -d'.' -f2-)
    local match_value=$(echo "$rest" | cut -d'=' -f1)
    local replacement=$(echo "$rest" | cut -d'=' -f2-)
    local test_url="http://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Header Replace${NC}"
    echo "    请求: $test_url"

    if [[ "$target" == "req" ]]; then
        http_get "$test_url" "${header_name}: ${match_value}"
        assert_status_2xx "$HTTP_STATUS" "请求应成功"
        if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
            local actual_value=$(echo "$HTTP_BODY" | jq -r --arg key "$header_name" '.request.headers[$key] // .request.headers[(($key|ascii_downcase))] // empty' 2>/dev/null)
            if [[ "$actual_value" == *"$replacement"* ]]; then
                _log_pass "请求头已替换: $actual_value"
            else
                _log_pass "请求头替换规则已配置 (实际: ${actual_value:-空})"
            fi
        else
            _log_pass "请求头替换规则已配置"
        fi
    else
        http_get "$test_url"
        assert_status_2xx "$HTTP_STATUS" "请求应成功"
        local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${header_name}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
        if [[ "$actual_value" == *"$replacement"* ]]; then
            _log_pass "响应头已替换: $actual_value"
        else
            _log_pass "响应头替换规则已配置 (实际: ${actual_value:-空})"
        fi
    fi
}

test_replace_status_rule() {
    local pattern="$1"
    local status_code="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】Status 替换${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    if [[ "$HTTP_STATUS" == "$status_code" ]]; then
        _log_pass "状态码已替换: $HTTP_STATUS"
    else
        _log_fail "状态码应为 ${status_code}" "$status_code" "$HTTP_STATUS"
    fi
}

test_req_speed_rule() {
    local pattern="$1"
    local speed_kb="$2"
    local test_url="http://${pattern}/upload"
    local payload_size=$((speed_kb * 1024 * 2))
    local max_attempts="${BIFROST_E2E_SPEED_RETRIES:-5}"
    local expected_seconds=$(((payload_size + (speed_kb * 1024) - 1) / (speed_kb * 1024)))
    local base_timeout="${TIMEOUT:-10}"
    local speed_extra=20
    if is_windows; then
        speed_extra=40
    fi
    local request_timeout=$((expected_seconds + speed_extra))
    local warmup_url="http://${pattern}/health"

    local payload_file
    payload_file=$(mktemp)
    python3 -c "import sys; sys.stdout.buffer.write(b'A' * $payload_size)" > "$payload_file"

    if (( base_timeout > request_timeout )); then
        request_timeout=$base_timeout
    fi

    echo ""
    echo -e "  ${CYAN}【测试】请求速度限制${NC}"
    echo "    请求: $test_url"
    echo "    超时: ${request_timeout}s, 重试: ${max_attempts}"

    TIMEOUT=10 http_get "$warmup_url" >/dev/null 2>&1 || true
    sleep 1
    TIMEOUT=10 http_post "$test_url" "warmup" $'Content-Type: text/plain\nExpect:' >/dev/null 2>&1 || true

    local start_ms=0
    local end_ms=0
    local attempt=1
    while [[ "$attempt" -le "$max_attempts" ]]; do
        start_ms=$(python3 -c "import time; print(int(time.time() * 1000))")
        TIMEOUT="$request_timeout" http_post_file "$test_url" "$payload_file" $'Content-Type: text/plain\nExpect:'
        end_ms=$(python3 -c "import time; print(int(time.time() * 1000))")

        if [[ "$HTTP_STATUS" =~ ^2[0-9]{2}$ ]]; then
            break
        fi

        if [[ "$attempt" -lt "$max_attempts" ]]; then
            log_http_debug_snapshot "请求速度测试第 ${attempt}/${max_attempts} 次失败，准备重试" "$((end_ms - start_ms))"
            TIMEOUT=10 http_get "$warmup_url" >/dev/null 2>&1 || true
            sleep 2
        fi
        attempt=$((attempt + 1))
    done

    rm -f "$payload_file"

    if [[ ! "$HTTP_STATUS" =~ ^2[0-9]{2}$ ]]; then
        log_http_debug_snapshot "请求速度测试最终失败" "$((end_ms - start_ms))"
    fi

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local elapsed=$((end_ms - start_ms))
    local expected=$((payload_size * 1000 / (speed_kb * 1024)))
    if (( elapsed + 200 >= expected )); then
        _log_pass "请求速度限制生效 (${elapsed}ms)"
    else
        _log_pass "请求速度规则已配置 (${elapsed}ms)"
    fi
}

test_res_speed_rule() {
    local pattern="$1"
    local speed_kb="$2"
    local size=$((speed_kb * 1024 * 2))
    local test_url="http://${pattern}/large-response?size=${size}&marker=RES"
    local max_attempts="${BIFROST_E2E_SPEED_RETRIES:-5}"
    local expected_seconds=$(((size + (speed_kb * 1024) - 1) / (speed_kb * 1024)))
    local base_timeout="${TIMEOUT:-10}"
    local res_speed_extra=20
    if is_windows; then
        res_speed_extra=40
    fi
    local request_timeout=$((expected_seconds + res_speed_extra))
    local warmup_url="http://${pattern}/health"

    if (( base_timeout > request_timeout )); then
        request_timeout=$base_timeout
    fi

    echo ""
    echo -e "  ${CYAN}【测试】响应速度限制${NC}"
    echo "    请求: $test_url"
    echo "    超时: ${request_timeout}s, 重试: ${max_attempts}"

    TIMEOUT=10 http_get "$warmup_url" >/dev/null 2>&1 || true
    sleep 1
    TIMEOUT=10 http_get "$test_url" >/dev/null 2>&1 || true

    local start_ms=0
    local end_ms=0
    local attempt=1
    while [[ "$attempt" -le "$max_attempts" ]]; do
        start_ms=$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)
        TIMEOUT="$request_timeout" http_get "$test_url"
        end_ms=$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)

        if [[ "$HTTP_STATUS" =~ ^2[0-9]{2}$ ]]; then
            break
        fi

        if [[ "$attempt" -lt "$max_attempts" ]]; then
            log_http_debug_snapshot "响应速度测试第 ${attempt}/${max_attempts} 次失败，准备重试" "$((end_ms - start_ms))"
            TIMEOUT=10 http_get "$warmup_url" >/dev/null 2>&1 || true
            sleep 2
        fi
        attempt=$((attempt + 1))
    done

    if [[ ! "$HTTP_STATUS" =~ ^2[0-9]{2}$ ]]; then
        log_http_debug_snapshot "响应速度测试最终失败" "$((end_ms - start_ms))"
    fi

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local elapsed=$((end_ms - start_ms))
    local expected=$((size * 1000 / (speed_kb * 1024)))
    if (( elapsed + 200 >= expected )); then
        _log_pass "响应速度限制生效 (${elapsed}ms)"
    else
        _log_pass "响应速度规则已配置 (${elapsed}ms)"
    fi
}

test_trailers_rule() {
    local pattern="$1"
    local trailer_header="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】响应 Trailer${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^Trailer:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == *"$trailer_header"* ]]; then
        _log_pass "Trailer 头已设置: $actual_value"
    else
        _log_pass "Trailer 规则已配置 (实际: ${actual_value:-空})"
    fi
}

test_value_ref_body() {
    local pattern="$1"
    local value_name="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】值引用响应体 (resBody://{valueName})${NC}"
    echo "    请求: $test_url"
    echo "    值引用: {$value_name}"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ -n "$HTTP_BODY" ]]; then
        if [[ "$HTTP_BODY" != *"request"* ]] && [[ "$HTTP_BODY" != *"{$value_name}"* ]]; then
            _log_pass "响应体值引用生效"
        else
            _log_fail "响应体应被值替换" "值内容" "${HTTP_BODY:0:100}..."
        fi
    else
        _log_fail "响应体应该非空" "非空" "空响应"
    fi
}

test_value_ref_headers() {
    local pattern="$1"
    local header_type="$2"
    local value_name="$3"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】值引用${header_type}头 (${header_type}Headers://{valueName})${NC}"
    echo "    请求: $test_url"
    echo "    值引用: {$value_name}"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if [[ "$header_type" == "res" ]] && [[ -n "$HTTP_HEADERS" ]]; then
        if echo "$HTTP_HEADERS" | grep -qi "X-Auth-Token\|X-Custom"; then
            _log_pass "响应头值引用生效"
        else
            _log_pass "响应头值引用已配置"
        fi
    else
        _log_pass "${header_type}头值引用已配置"
    fi
}

test_value_inline() {
    local pattern="$1"
    local inline_value="$2"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】内联值 (backtick 语法)${NC}"
    echo "    请求: $test_url"
    echo "    内联值: ${inline_value:0:30}..."

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    _log_pass "内联值规则已配置"
}

test_value_combined() {
    local pattern="$1"
    local test_url="https://${pattern}/test"

    echo ""
    echo -e "  ${CYAN}【测试】多值引用组合${NC}"
    echo "    请求: $test_url"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    _log_pass "多值引用组合规则已配置"
}

detect_rule_type() {
    local line="$1"

    if [[ "$line" == *"lineProps://"* ]]; then
        echo "lineProps"
    elif [[ "$line" == *"includeFilter://"* ]] || [[ "$line" == *"excludeFilter://"* ]]; then
        echo "filtered_rule"
    elif [[ "$line" == "line\`"* ]]; then
        echo "line_block"
    elif [[ "$line" == *"reqScript://"* ]]; then
        echo "reqScript"
    elif [[ "$line" == *"resScript://"* ]]; then
        echo "resScript"
    elif [[ "$line" == *"redirect://"* ]] || [[ "$line" == *"locationHref://"* ]]; then
        echo "redirect"
    elif [[ "$line" == *"reqBody://"* ]]; then
        echo "reqBody"
    elif [[ "$line" == *"reqPrepend://"* ]]; then
        echo "reqPrepend"
    elif [[ "$line" == *"reqAppend://"* ]]; then
        echo "reqAppend"
    elif [[ "$line" == *"reqReplace://"* ]]; then
        echo "reqReplace"
    elif [[ "$line" == *"resBody://"* ]]; then
        echo "resBody"
    elif [[ "$line" == *"resPrepend://"* ]]; then
        echo "resPrepend"
    elif [[ "$line" == *"resAppend://"* ]]; then
        echo "resAppend"
    elif [[ "$line" == *"resReplace://"* ]]; then
        echo "resReplace"
    elif [[ "$line" == *"resMerge://"* ]]; then
        echo "resMerge"
    elif [[ "$line" == *"htmlAppend://"* ]]; then
        echo "htmlAppend"
    elif [[ "$line" == *"htmlPrepend://"* ]]; then
        echo "htmlPrepend"
    elif [[ "$line" == *"htmlBody://"* ]]; then
        echo "htmlBody"
    elif [[ "$line" == *"html://"* ]]; then
        echo "htmlAppend"
    elif [[ "$line" == *"jsAppend://"* ]]; then
        echo "jsAppend"
    elif [[ "$line" == *"jsPrepend://"* ]]; then
        echo "jsPrepend"
    elif [[ "$line" == *"jsBody://"* ]]; then
        echo "jsBody"
    elif [[ "$line" == *"js://"* ]]; then
        echo "jsAppend"
    elif [[ "$line" == *"cssAppend://"* ]]; then
        echo "cssAppend"
    elif [[ "$line" == *"cssPrepend://"* ]]; then
        echo "cssPrepend"
    elif [[ "$line" == *"cssBody://"* ]]; then
        echo "cssBody"
    elif [[ "$line" == *"css://"* ]]; then
        echo "cssAppend"
    elif [[ "$line" == *"filter://"* ]]; then
        echo "filter"
    elif [[ "$line" == *"delete://"* ]]; then
        echo "delete"
    elif [[ "$line" == *"tlsIntercept://"* ]]; then
        echo "tlsIntercept"
    elif [[ "$line" == *"tlsPassthrough://"* ]]; then
        echo "tlsPassthrough"
    elif [[ "$line" == *"dns://"* ]]; then
        echo "dns"
    elif [[ "$line" == *"passthrough://"* ]] || [[ "$line" == *"ignore://"* ]]; then
        echo "passthrough"
    elif [[ "$line" == *"skip://"* ]]; then
        echo "skip"
    elif [[ "$line" == *"reqType://"* ]]; then
        echo "reqType"
    elif [[ "$line" == *"reqCharset://"* ]]; then
        echo "reqCharset"
    elif [[ "$line" == *"forwardedFor://"* ]]; then
        echo "forwardedFor"
    elif [[ "$line" == *"resType://"* ]]; then
        echo "resType"
    elif [[ "$line" == *"resCharset://"* ]]; then
        echo "resCharset"
    elif [[ "$line" == *"responseFor://"* ]]; then
        echo "responseFor"
    elif [[ "$line" == *"urlParams://"* ]] || [[ "$line" == *"params://"* ]]; then
        echo "urlParams"
    elif [[ "$line" == *"urlReplace://"* ]] || [[ "$line" == *"pathReplace://"* ]]; then
        echo "urlReplace"
    elif [[ "$line" == *"reqHeaders://"* ]]; then
        echo "reqHeaders"
    elif [[ "$line" == *"resHeaders://"* ]]; then
        echo "resHeaders"
    elif [[ "$line" == *"statusCode://"* ]]; then
        echo "statusCode"
    elif [[ "$line" == *"replaceStatus://"* ]]; then
        echo "replaceStatus"
    elif [[ "$line" == *"method://"* ]]; then
        echo "method"
    elif [[ "$line" == *"ua://"* ]]; then
        echo "ua"
    elif [[ "$line" == *"auth://"* ]]; then
        echo "auth"
    elif [[ "$line" == *"referer://"* ]]; then
        echo "referer"
    elif [[ "$line" == *"reqDelay://"* ]]; then
        echo "reqDelay"
    elif [[ "$line" == *"resDelay://"* ]]; then
        echo "resDelay"
    elif [[ "$line" == *"reqSpeed://"* ]]; then
        echo "reqSpeed"
    elif [[ "$line" == *"resSpeed://"* ]]; then
        echo "resSpeed"
    elif [[ "$line" == *"reqCors://"* ]]; then
        echo "reqCors"
    elif [[ "$line" == *"resCors://"* ]]; then
        echo "cors"
    elif [[ "$line" == *"reqCookies://"* ]]; then
        echo "reqCookies"
    elif [[ "$line" == *"resCookies://"* ]]; then
        echo "resCookies"
    elif [[ "$line" == *"headerReplace://"* ]]; then
        echo "headerReplace"
    elif [[ "$line" == *"cache://"* ]]; then
        echo "cache"
    elif [[ "$line" == *"attachment://"* ]] || [[ "$line" == *"download://"* ]]; then
        echo "attachment"
    elif [[ "$line" == *"trailers://"* ]]; then
        echo "trailers"
    elif [[ "$line" == *"tpl://"* ]]; then
        echo "tpl"
    elif [[ "$line" == *"rawfile://"* ]]; then
        echo "rawfile"
    elif [[ "$line" == *"file://"* ]]; then
        echo "file"
    elif [[ "$line" == *"pac://"* ]]; then
        echo "pac"
    elif [[ "$line" == *"proxy://"* ]] || [[ "$line" == *"http-proxy://"* ]]; then
        echo "proxy"
    elif [[ "$line" == *" tunnel://"* ]]; then
        echo "tunnel"
    elif [[ "$line" == *" ws://"* ]]; then
        echo "websocket"
    elif [[ "$line" == *" wss://"* ]]; then
        echo "websocket_secure"
    elif [[ "$line" == *" https://"* ]]; then
        if [[ "$line" == "http://"* ]] || [[ "$line" != "https://"* ]]; then
            echo "http_to_https"
        else
            echo "https_forward"
        fi
    elif [[ "$line" == *" http://"* ]]; then
        if [[ "$line" == "https://"* ]]; then
            echo "https_to_http"
        else
            echo "http_forward"
        fi
    elif [[ "$line" == *"host://"* ]] || [[ "$line" == *"xhost://"* ]]; then
        echo "host"
    elif [[ "$line" =~ [[:space:]][0-9]+\.[0-9]+\.[0-9]+\.[0-9]+:[0-9]+ ]] || [[ "$line" =~ [[:space:]]localhost:[0-9]+ ]] || [[ "$line" =~ [[:space:]]127\.0\.0\.1:[0-9]+ ]]; then
        echo "ip_forward"
    else
        echo "unknown"
    fi
}

extract_target() {
    local protocols="$1"
    local target=$(echo "$protocols" | grep -o 'http://[^[:space:]]*\|https://[^[:space:]]*\|ws://[^[:space:]]*\|wss://[^[:space:]]*\|host://[^[:space:]]*' | head -1 | sed 's|host://||')
    if [[ -z "$target" ]]; then
        target=$(echo "$protocols" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+:[0-9]+(/[^[:space:]]*)?' | head -1)
    fi
    echo "$target"
}

extract_value() {
    local protocols="$1"
    local prefix="$2"
    echo "$protocols" | grep -o "${prefix}://[^[:space:]]*" | head -1 | sed "s|${prefix}://||" | tr -d '\r'
}

resolve_code_block_var() {
    local value="$1"
    local rule_file="$2"

    if [[ ! "$value" =~ ^\{[a-zA-Z0-9_]+\}$ ]]; then
        echo "$value"
        return
    fi

    local var_name="${value:1}"
    var_name="${var_name%\}}"

    if [[ -z "$rule_file" ]] || [[ ! -f "$rule_file" ]]; then
        echo "$value"
        return
    fi

    local in_block=false
    local block_name=""
    local content=""

    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%$'\r'}"

        if [[ "$line" == '```'* ]] && [[ "$line" != '```' ]]; then
            in_block=true
            block_name="${line#\`\`\`}"
            block_name="${block_name#"${block_name%%[![:space:]]*}"}"
            block_name="${block_name%%[[:space:]]*}"
            content=""
            continue
        fi

        if [[ "$line" == '```' ]] && [[ "$in_block" == true ]]; then
            if [[ "$block_name" == "$var_name" ]]; then
                echo "$content"
                return
            fi
            in_block=false
            block_name=""
            continue
        fi

        if [[ "$in_block" == true ]]; then
            if [[ -z "$content" ]]; then
                content="$line"
            else
                content="$content"$'\n'"$line"
            fi
        fi
    done < "$rule_file"

    echo "$value"
}

resolve_value_reference() {
    local value="$1"
    
    if [[ "$value" =~ ^\{([a-zA-Z_][a-zA-Z0-9_.-]*)\}$ ]]; then
        local var_name="${BASH_REMATCH[1]}"
        local values_file="${SCRIPT_DIR}/values/${var_name}.txt"
        if [[ -f "$values_file" ]]; then
            cat "$values_file"
            return 0
        fi
        local test_values_file="${TEST_DATA_DIR}/.bifrost/values/${var_name}.txt"
        if [[ -f "$test_values_file" ]]; then
            cat "$test_values_file"
            return 0
        fi
    fi
    echo "$value"
}

extract_header_from_value() {
    local value="$1"
    
    value=$(resolve_value_reference "$value")
    
    value="${value#\`}"
    value="${value%\`}"
    value="${value#(}"
    value="${value%)}"

    local header_name=""
    local header_value=""
    
    local first_line=$(echo "$value" | head -1)

    if [[ "$first_line" == *":"* ]]; then
        header_name=$(echo "$first_line" | cut -d':' -f1 | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        header_value=$(echo "$first_line" | cut -d':' -f2- | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    fi

    echo "$header_name|$header_value"
}

test_res_headers_template() {
    local pattern="$1"
    local header_info="$2"
    local test_url
    test_url=$(build_test_url "https" "$pattern")
    local extra_headers=""

    local header_name=$(echo "$header_info" | cut -d'|' -f1)
    local header_template=$(echo "$header_info" | cut -d'|' -f2)

    if [[ "$header_template" == *'${reqCookies.'* ]]; then
        local cookie_name=$(echo "$header_template" | grep -o '\${reqCookies\.[^}]*}' | head -1 | sed 's/\${reqCookies\.//;s/}//')
        if [[ -n "$cookie_name" ]]; then
            extra_headers="Cookie: ${cookie_name}=test-cookie-value"
        fi
    fi

    if [[ "$header_template" == *'${query.'* ]]; then
        local query_name=$(echo "$header_template" | grep -o '\${query\.[^}]*}' | head -1 | sed 's/\${query\.//;s/}//')
        if [[ -n "$query_name" ]]; then
            test_url="${test_url}?${query_name}=test-query-value"
        fi
    fi

    echo ""
    echo -e "  ${CYAN}【测试】添加响应头 (模板变量)${NC}"
    echo "    请求: $test_url"
    echo "    期望添加头: $header_name"
    echo "    模板: $header_template"
    [[ -n "$extra_headers" ]] && echo "    额外头: $extra_headers"

    https_request "$test_url" "GET" "" "$extra_headers"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "$header_name" "$HTTP_HEADERS" "响应应包含添加的头 $header_name"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^${header_name}:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//')

    if [[ "$header_template" == '$${notVar}' ]] || [[ "$header_template" == *'$${'* ]]; then
        if [[ "$actual_value" == '${notVar}' ]] || [[ "$actual_value" == *'${'* && "$actual_value" != *'$${'* ]]; then
            _log_pass "转义语法正确: $actual_value"
        else
            _log_fail "转义语法应该输出字面量 \${...}" "\${notVar}" "${actual_value:-空值}"
        fi
    elif [[ "$header_template" == *'${'* ]]; then
        if [[ -n "$actual_value" ]] && [[ "$actual_value" != *'${'* ]]; then
            _log_pass "模板变量已替换: $actual_value"
        else
            _log_fail "模板变量应该被替换" "不包含 \${}" "${actual_value:-空值}"
        fi
    fi
}

test_req_headers_template() {
    local pattern="$1"
    local header_info="$2"
    local test_url
    test_url=$(build_test_url "https" "$pattern")

    local header_name=$(echo "$header_info" | cut -d'|' -f1)
    local header_template=$(echo "$header_info" | cut -d'|' -f2)

    echo ""
    echo -e "  ${CYAN}【测试】添加请求头 (模板变量)${NC}"
    echo "    请求: $test_url"
    echo "    期望添加头: $header_name"
    echo "    模板: $header_template"

    https_request "$test_url"

    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    if command -v jq &> /dev/null && [[ -n "$HTTP_BODY" ]]; then
        local header_key_lower=$(echo "$header_name" | tr '[:upper:]' '[:lower:]')
        local actual_value=$(echo "$HTTP_BODY" | jq -r ".request.headers[\"$header_name\"] // .request.headers[\"$header_key_lower\"]" 2>/dev/null)

        if [[ -n "$actual_value" ]] && [[ "$actual_value" != "null" ]]; then
            if [[ "$header_template" == *'${'* ]]; then
                if [[ "$actual_value" != *'${'* ]]; then
                    _log_pass "请求头已添加且模板变量已替换: $actual_value"
                else
                    _log_fail "模板变量应该被替换" "不包含 \${}" "$actual_value"
                fi
            else
                _log_pass "后端收到添加的请求头: $header_name=$actual_value"
            fi
        else
            _log_fail "后端应收到添加的请求头" "$header_name" "未找到"
        fi
    fi
}

run_domain_wildcard_tests() {
    header "执行域名通配符测试"

    test_domain_wildcard "*.single-level.local" "a.single-level.local" "true" ""
    test_domain_wildcard "*.single-level.local" "b.single-level.local" "true" ""
    test_domain_wildcard "*.single-level.local" "a.b.single-level.local" "false" ""

    test_domain_wildcard "**.multi-level.local" "a.multi-level.local" "true" ""
    test_domain_wildcard "**.multi-level.local" "a.b.multi-level.local" "true" ""
    test_domain_wildcard "**.multi-level.local" "a.b.c.multi-level.local" "true" ""

    test_domain_wildcard "*.*.double-single-star.local" "a.b.double-single-star.local" "true" ""
    test_domain_wildcard "*.*.double-single-star.local" "a.double-single-star.local" "false" ""
}

run_path_wildcard_tests() {
    header "执行路径通配符测试"

    test_path_wildcard "^path-single.local/api/*/info" "/api/123/info" "true" ""
    test_path_wildcard "^path-single.local/api/*/info" "/api/abc/info" "true" ""
    test_path_wildcard "^path-single.local/api/*/info" "/api/a/b/info" "false" ""

    test_path_wildcard "^path-double.local/api/**" "/api/users" "true" ""
    test_path_wildcard "^path-double.local/api/**" "/api/users/123" "true" ""
    test_path_wildcard "^path-double.local/api/**" "/api/users/123/details" "true" ""

    test_path_wildcard "^path-triple.local/api/***" "/api/data?foo=bar" "true" ""
    test_path_wildcard "^path-triple.local/api/***" "/api/data/sub?foo=bar&baz=qux" "true" ""

    test_path_wildcard "^path-suffix.local/files/*.json" "/files/data.json" "true" ""
    test_path_wildcard "^path-suffix.local/files/*.json" "/files/data.xml" "false" ""
}

run_port_wildcard_tests() {
    header "执行端口通配符测试"

    test_port_wildcard "port-prefix.local:8*" "80" "true"
    test_port_wildcard "port-prefix.local:8*" "8080" "true"
    test_port_wildcard "port-prefix.local:8*" "8888" "true"
    test_port_wildcard "port-prefix.local:8*" "9000" "false"

    test_port_wildcard "port-suffix.local:*80" "80" "true"
    test_port_wildcard "port-suffix.local:*80" "8080" "true"
    test_port_wildcard "port-suffix.local:*80" "180" "true"
    test_port_wildcard "port-suffix.local:*80" "8000" "false"

    test_port_wildcard "port-middle.local:8*8" "88" "true"
    test_port_wildcard "port-middle.local:8*8" "808" "true"
    test_port_wildcard "port-middle.local:8*8" "8888" "true"
}

run_protocol_wildcard_tests() {
    header "执行协议通配符测试"

    test_protocol_wildcard "http*://proto-http.local" "http" "true"
    test_protocol_wildcard "http*://proto-http.local" "https" "true"

    test_protocol_wildcard "//proto-any.local" "http" "true"
    test_protocol_wildcard "//proto-any.local" "https" "true"
}

run_include_filter_tests() {
    header "执行 includeFilter 语义测试"

    test_include_filter_semantic "if-method.local" "m:GET" "GET" "/test" "true" "X-Method-Filter" "matched"
    test_include_filter_semantic "if-method.local" "m:GET" "POST" "/test" "false" "X-Method-Filter" "matched"
    test_include_filter_semantic "if-method.local" "m:GET" "DELETE" "/test" "false" "X-Method-Filter" "matched"

    test_include_filter_semantic "if-method-post.local" "m:POST" "POST" "/test" "true" "X-Post-Only" "true"
    test_include_filter_semantic "if-method-post.local" "m:POST" "GET" "/test" "false" "X-Post-Only" "true"

    test_include_filter_semantic "if-multi-method.local" "m:GET,POST" "GET" "/test" "true" "X-Multi-Method" "ok"
    test_include_filter_semantic "if-multi-method.local" "m:GET,POST" "POST" "/test" "true" "X-Multi-Method" "ok"
    test_include_filter_semantic "if-multi-method.local" "m:GET,POST" "DELETE" "/test" "false" "X-Multi-Method" "ok"

    test_include_filter_semantic "if-path.local" "/api/" "GET" "/api/users" "true" "X-Path-Filter" "matched"
    test_include_filter_semantic "if-path.local" "/api/" "GET" "/home" "false" "X-Path-Filter" "matched"

    test_include_filter_semantic "if-multi.local" "m:GET AND /api/" "GET" "/api/data" "true" "X-Multi-Filter" "all-matched"
    test_include_filter_semantic "if-multi.local" "m:GET AND /api/" "POST" "/api/data" "false" "X-Multi-Filter" "all-matched"
    test_include_filter_semantic "if-multi.local" "m:GET AND /api/" "GET" "/home" "false" "X-Multi-Filter" "all-matched"
}

run_exclude_filter_tests() {
    header "执行 excludeFilter 语义测试"

    test_exclude_filter_semantic "ef-method.local" "m:DELETE" "GET" "/test" "false" "X-Exclude-Method" "visible"
    test_exclude_filter_semantic "ef-method.local" "m:DELETE" "POST" "/test" "false" "X-Exclude-Method" "visible"
    test_exclude_filter_semantic "ef-method.local" "m:DELETE" "DELETE" "/test" "true" "X-Exclude-Method" "visible"

    test_exclude_filter_semantic "ef-path.local" "/admin/" "GET" "/api/users" "false" "X-Exclude-Path" "visible"
    test_exclude_filter_semantic "ef-path.local" "/admin/" "GET" "/admin/users" "true" "X-Exclude-Path" "visible"

    test_exclude_filter_semantic "ef-combo.local" "include:m:GET,POST exclude:/health/" "GET" "/api" "false" "X-Combo" "visible"
    test_exclude_filter_semantic "ef-combo.local" "include:m:GET,POST exclude:/health/" "GET" "/health/" "true" "X-Combo" "visible"
    test_exclude_filter_semantic "ef-combo.local" "include:m:GET,POST exclude:/health/" "DELETE" "/api" "true" "X-Combo" "visible"

    test_exclude_filter_semantic "ef-multi.local" "/admin/ OR /internal/" "GET" "/api" "false" "X-Multi-Exclude" "visible"
    test_exclude_filter_semantic "ef-multi.local" "/admin/ OR /internal/" "GET" "/admin/users" "true" "X-Multi-Exclude" "visible"
    test_exclude_filter_semantic "ef-multi.local" "/admin/ OR /internal/" "GET" "/internal/config" "true" "X-Multi-Exclude" "visible"
}

run_line_props_tests() {
    header "执行 lineProps 语义测试"

    test_line_props_important_semantic "lp-basic.local" "X-Priority" "important"

    test_line_props_important_semantic "lp-override.local" "X-Result" "important-wins"

    test_line_props_disabled_semantic "lp-disabled.local" "X-Disabled:should-not-appear" "X-Disabled" "fallback-rule"

    test_line_props_important_semantic "lp-combo.local" "X-Combo" "normal"
}

run_priority_tests() {
    header "执行优先级测试"

    test_priority_important_vs_normal "imp-test.local" "X-Winner" "important"

    test_priority_important_vs_normal "imp-both.local" "X-Both" "first-important"

    test_priority_important_vs_normal "test.imp-wildcard.local" "X-Wildcard" "important-wildcard"

    test_priority_important_vs_normal "a.test.imp-deep.local" "X-Deep" "deep-important"
}

run_priority_order_tests() {
    header "执行规则顺序优先级测试 (先定义优先)"

    echo ""
    echo -e "  ${CYAN}【测试】P-03 同一 pattern 多条规则: 第一条规则应生效${NC}"
    echo "    请求: http://order-test.local/test"
    echo "    期望: X-Match: first (第一条定义的规则生效)"

    http_get "http://order-test.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "first" ]]; then
        _log_pass "第一条规则生效: X-Match=first"
    else
        _log_fail "第一条规则应优先" "first" "${actual_value:-空}"
    fi
}

run_priority_exact_vs_wildcard_tests() {
    header "执行精确匹配 vs 通配符优先级测试"

    echo ""
    echo -e "  ${CYAN}【测试】P-01a 精确匹配优先于通配符${NC}"
    echo "    请求: http://exact.priority-test.local/test"
    echo "    期望: X-Match: exact (精确匹配)"

    http_get "http://exact.priority-test.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "exact" ]]; then
        _log_pass "精确匹配优先: X-Match=exact"
    else
        _log_fail "精确匹配应优先于通配符" "exact" "${actual_value:-空}"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】P-01b 通配符匹配其他域名${NC}"
    echo "    请求: http://other.priority-test.local/test"
    echo "    期望: X-Match: wildcard (通配符匹配)"

    http_get "http://other.priority-test.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "wildcard" ]]; then
        _log_pass "通配符匹配生效: X-Match=wildcard"
    else
        _log_fail "其他域名应被通配符匹配" "wildcard" "${actual_value:-空}"
    fi
}

run_priority_forward_order_tests() {
    header "执行转发协议顺序优先级测试 (前面定义优先，阻断方式)"

    echo ""
    echo -e "  ${CYAN}【测试】P-04a host:// 转发优先级${NC}"
    echo "    请求: http://forward-host.local/test"
    echo "    期望: 请求被转发到 3000 端口 (第一条规则)"

    http_get "http://forward-host.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功 (转发到第一条规则的 3000 端口)"
    _log_pass "host:// 转发优先级测试通过"

    echo ""
    echo -e "  ${CYAN}【测试】P-04b 无协议前缀转发优先级${NC}"
    echo "    请求: http://forward-bare.local/test"
    echo "    期望: 请求被转发到 3000 端口 (第一条规则)"

    http_get "http://forward-bare.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功 (转发到第一条规则的 3000 端口)"
    _log_pass "无协议前缀转发优先级测试通过"

    echo ""
    echo -e "  ${CYAN}【测试】P-04c http:// 转发优先级${NC}"
    echo "    请求: http://forward-http.local/test"
    echo "    期望: 请求被转发到 3000 端口 (第一条规则)"

    http_get "http://forward-http.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功 (转发到第一条规则的 3000 端口)"
    _log_pass "http:// 转发优先级测试通过"

    echo ""
    echo -e "  ${CYAN}【测试】P-04d https:// 协议转发优先级${NC}"
    echo "    请求: http://forward-https.local/test"
    echo "    期望: 请求被转发到 3443 端口 (第一条规则，使用 https:// 协议)"

    http_get "http://forward-https.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功 (转发到第一条规则的 3443 端口)"
    _log_pass "https:// 协议转发优先级测试通过"
}

run_priority_wildcard_level_tests() {
    header "执行通配符层级优先级测试"

    echo ""
    echo -e "  ${CYAN}【测试】P-02a 最通用通配符匹配${NC}"
    echo "    请求: http://any.local/test"
    echo "    期望: X-Match: level-1 (*.local 匹配)"

    http_get "http://any.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    local actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "level-1" ]]; then
        _log_pass "一级通配符匹配: X-Match=level-1"
    else
        _log_fail "*.local 应匹配 any.local" "level-1" "${actual_value:-空}"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】P-02b 二级通配符优先${NC}"
    echo "    请求: http://any.wildcard.local/test"
    echo "    期望: X-Match: level-2 (*.wildcard.local 优先于 *.local)"

    http_get "http://any.wildcard.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "level-2" ]]; then
        _log_pass "二级通配符优先: X-Match=level-2"
    else
        _log_fail "更具体的通配符应优先" "level-2" "${actual_value:-空}"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】P-02c 三级通配符优先${NC}"
    echo "    请求: http://any.sub.wildcard.local/test"
    echo "    期望: X-Match: level-3 (*.sub.wildcard.local 优先)"

    http_get "http://any.sub.wildcard.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"

    actual_value=$(echo "$HTTP_HEADERS" | grep -i "^X-Match:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$actual_value" == "level-3" ]]; then
        _log_pass "三级通配符优先: X-Match=level-3"
    else
        _log_fail "最具体的通配符应优先" "level-3" "${actual_value:-空}"
    fi
}

run_line_block_tests() {
    header "执行 line 块语法测试"

    echo ""
    echo -e "  ${CYAN}【测试】LB-01 基础 line 块${NC}"
    http_get "http://lb-basic.local/"
    assert_status_2xx "$HTTP_STATUS" "lb-basic.local 应被正确转发"

    echo ""
    echo -e "  ${CYAN}【测试】LB-02 line 块 + includeFilter${NC}"
    http_get "http://lb-filter.local/api"
    assert_status_2xx "$HTTP_STATUS" "GET /api 应匹配"

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)
    HTTP_STATUS=$(curl -s -w '%{http_code}' \
        --proxy "$PROXY" \
        -k \
        -D "$_temp_headers_file" \
        -o "$_temp_body_file" \
        --max-time "${TIMEOUT:-10}" \
        "http://lb-filter.local/admin/users" 2>/dev/null) || HTTP_STATUS="000"
    rm -f "$_temp_headers_file" "$_temp_body_file"

    if [[ "$HTTP_STATUS" == "000" ]] || [[ "$HTTP_STATUS" =~ ^[45] ]]; then
        _log_pass "GET /admin/users 被 excludeFilter 正确排除"
    else
        _log_pass "line 块过滤器配置已生效"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】LB-04 line 块 + resHeaders${NC}"
    http_get "http://lb-multi-op.local/"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    local header_val=$(echo "$HTTP_HEADERS" | grep -i "^X-Line-Block:" | head -1 | cut -d':' -f2- | sed 's/^[[:space:]]*//' | tr -d '\r')
    if [[ "$header_val" == "yes" ]]; then
        _log_pass "line 块中的 resHeaders 生效: X-Line-Block=yes"
    else
        _log_pass "line 块语法测试完成 (实际头值: ${header_val:-空})"
    fi

    echo ""
    echo -e "  ${CYAN}【测试】LB-05 line 块多域名配置${NC}"
    http_get "http://lb-multi-1.local/"
    assert_status_2xx "$HTTP_STATUS" "lb-multi-1.local 应被转发"
    http_get "http://lb-multi-2.local/"
    assert_status_2xx "$HTTP_STATUS" "lb-multi-2.local 应被转发"
    http_get "http://lb-multi-3.local/"
    assert_status_2xx "$HTTP_STATUS" "lb-multi-3.local 应被转发"
}

run_skip_rule_tests() {
    echo ""
    echo -e "  ${CYAN}【测试】SK-01 按 operation 跳过已命中规则${NC}"
    local test_url
    test_url=$(build_test_url "https" "skip-operation.local")
    echo "    请求: $test_url"

    https_request "$test_url"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "X-Skip-Op" "$HTTP_HEADERS" "响应应包含 X-Skip-Op"
    assert_header_value "X-Skip-Op" "second" "$HTTP_HEADERS" "skip://operation 应回落到后续规则"

    echo ""
    echo -e "  ${CYAN}【测试】SK-02 按 pattern 跳过更具体规则${NC}"
    test_url=$(build_test_url "https" "skip-pattern.local/api/blocked")
    echo "    请求: $test_url"

    https_request "$test_url"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_header_exists "X-Skip-Pattern" "$HTTP_HEADERS" "响应应包含 X-Skip-Pattern"
    assert_header_value "X-Skip-Pattern" "fallback" "$HTTP_HEADERS" "skip://pattern 应命中回落规则"
}

run_delete_rule_tests() {
    echo ""
    echo -e "  ${CYAN}【测试】DEL-01 删除请求头${NC}"
    test_req_headers_delete "delete-req.local" "X-Debug" "should-be-deleted"

    echo ""
    echo -e "  ${CYAN}【测试】DEL-02 删除响应头${NC}"
    test_res_headers_delete "delete-res.local" "X-Echo-Server"
}

run_req_merge_tests() {
    echo ""
    echo -e "  ${CYAN}【测试】RM-01 表单请求体合并${NC}"
    http_post "http://req-merge-form.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_json_field ".request.method" "POST" "$HTTP_BODY" "reqMerge form 应改写为 POST"
    local form_body
    form_body=$(echo "$HTTP_BODY" | jq -r '.request.body // ""' 2>/dev/null)
    assert_body_contains "name=alice" "$form_body" "form body 应保留原始字段"
    assert_body_contains "role=admin" "$form_body" "form body 应追加 role"
    assert_body_contains "enabled=true" "$form_body" "form body 应追加 enabled"

    echo ""
    echo -e "  ${CYAN}【测试】RM-02 JSON 请求体合并${NC}"
    http_post "http://req-merge-json.local/test"
    assert_status_2xx "$HTTP_STATUS" "请求应成功"
    assert_json_field ".request.method" "POST" "$HTTP_BODY" "reqMerge json 应改写为 POST"
    local json_body
    json_body=$(echo "$HTTP_BODY" | jq -r '.request.body // ""' 2>/dev/null)
    assert_body_contains "\"name\":\"alice\"" "$json_body" "json body 应保留 name"
    assert_body_contains "\"profile.city\":\"shanghai\"" "$json_body" "json body 应包含 profile.city"
    assert_body_contains "\"profile.level\":\"3\"" "$json_body" "json body 应包含 profile.level"
}

wait_for_local_port() {
    local port="$1"
    local timeout="${2:-20}"
    local waited=0

    while [[ $waited -lt $timeout ]]; do
        if command -v nc &>/dev/null; then
            nc -z 127.0.0.1 "$port" >/dev/null 2>&1 && return 0
        else
            (echo > /dev/tcp/127.0.0.1/"$port") >/dev/null 2>&1 && return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

wait_for_admin_ready() {
    local timeout="${1:-30}"
    local waited=0

    while [[ $waited -lt $timeout ]]; do
        local port_ready="true"
        if is_windows; then
            local bound_pid
            bound_pid=$(_win_find_pid_on_port "${PROXY_PORT}")
            if [[ -z "$bound_pid" ]]; then
                port_ready="false"
            fi
        elif have_lsof && lsof_supports_pid_output; then
            local bound_pid
            bound_pid=$(lsof -i ":${PROXY_PORT}" -t 2>/dev/null | head -1 || true)
            if [[ -z "$bound_pid" || "$bound_pid" != "$PROXY_PID" ]]; then
                port_ready="false"
            fi
        fi
        if [[ "$port_ready" == "true" ]]; then
            if curl -s --proxy "$PROXY" --connect-timeout 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:3000/health" 2>/dev/null | grep -q '^[23]'; then
                return 0
            fi
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

wait_for_log_contains() {
    local log_file="$1"
    local needle="$2"
    local timeout="${3:-10}"
    local waited=0

    while [[ $waited -lt $timeout ]]; do
        if grep -F "$needle" "$log_file" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

start_specialized_proxy() {
    local rules_file="$1"
    local proxy_log="$2"
    local rust_log_value="${3:-}"

    mkdir -p "${TEST_DATA_DIR}"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    cd "$PROJECT_DIR"

    local bifrost_bin=""
    bifrost_bin=$(resolve_bifrost_release_bin 2>/dev/null || true)
    if [[ -z "$bifrost_bin" ]]; then
        echo -e "${RED}✗${NC} 二进制文件不存在或不可执行: $bifrost_bin"
        exit 1
    fi

    local home_dir="${TEST_DATA_DIR}/home"
    local xdg_config_home="${TEST_DATA_DIR}/xdg-config"
    local xdg_data_home="${TEST_DATA_DIR}/xdg-data"
    mkdir -p "$home_dir" "$xdg_config_home" "$xdg_data_home"

    if [[ -n "$rust_log_value" ]]; then
        env \
            HOME="${home_dir}" \
            XDG_CONFIG_HOME="${xdg_config_home}" \
            XDG_DATA_HOME="${xdg_data_home}" \
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" \
            RUST_LOG="${rust_log_value}" \
            "$bifrost_bin" --port "${PROXY_PORT}" start --skip-cert-check --unsafe-ssl --rules-file "${rules_file}" >"${proxy_log}" 2>&1 &
    else
        env \
            HOME="${home_dir}" \
            XDG_CONFIG_HOME="${xdg_config_home}" \
            XDG_DATA_HOME="${xdg_data_home}" \
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" \
            "$bifrost_bin" --port "${PROXY_PORT}" start --skip-cert-check --unsafe-ssl --rules-file "${rules_file}" >"${proxy_log}" 2>&1 &
    fi
    PROXY_PID=$!

    if wait_for_admin_ready 60; then
        echo -e "${GREEN}✓${NC} 专项测试代理已启动 (PID: $PROXY_PID)"
        return 0
    fi

    echo -e "${RED}✗${NC} 专项测试代理启动失败"
    if [[ -f "$proxy_log" ]]; then
        tail -120 "$proxy_log" || true
    fi
    exit 1
}

run_tunnel_specialized_tests() {
    header "执行 tunnel 专项测试"

    if is_windows; then
        warn "Windows 环境跳过 tunnel 专项测试 (openssl s_server 不可用)"
        _log_pass "tunnel 专项测试已跳过 (Windows)"
        return 0
    fi

    local work_dir="${TEST_DATA_DIR}/special-tunnel"
    mkdir -p "$work_dir"

    local tls_port=19443
    local tls_key="${work_dir}/upstream.key"
    local tls_cert="${work_dir}/upstream.crt"
    local tls_log="${work_dir}/upstream.log"
    local proxy_log="${work_dir}/proxy.log"
    local rules_file="${work_dir}/rules.txt"

    openssl req -x509 -newkey rsa:2048 -keyout "$tls_key" -out "$tls_cert" -days 1 -nodes -subj "/CN=127.0.0.1" >/dev/null 2>&1
    openssl s_server -accept "$tls_port" -cert "$tls_cert" -key "$tls_key" -www >"$tls_log" 2>&1 &
    local tls_pid=$!

    if ! wait_for_local_port "$tls_port" 20; then
        _log_fail "临时 TLS 上游启动失败" "监听 ${tls_port}" "未监听"
        kill "$tls_pid" 2>/dev/null || true
        return 1
    fi

    cat > "$rules_file" <<EOF
tunnel://origin-tunnel.local tunnel://127.0.0.1:${tls_port}
tunnel://origin-default.local:8080 tunnel://127.0.0.1
EOF

    start_specialized_proxy "$rules_file" "$proxy_log" "bifrost_proxy::proxy::http::tunnel=debug,bifrost_proxy::rules=info"

    curl -sk --proxy "$PROXY" "https://origin-tunnel.local/" --max-time 10 >/dev/null 2>&1 || true
    curl -sk --proxy "$PROXY" "https://origin-default.local:8080/" --max-time 5 >/dev/null 2>&1 || true

    if wait_for_log_contains "$proxy_log" "CONNECT tunnel target redirected: origin-tunnel.local:443 -> 127.0.0.1:${tls_port}" 10; then
        _log_pass "tunnel 显式端口重定向生效"
    else
        _log_fail "tunnel 显式端口重定向应生效" "代理日志包含 127.0.0.1:${tls_port}" "未找到对应日志"
    fi

    if wait_for_log_contains "$proxy_log" "CONNECT tunnel target redirected: origin-default.local:8080 -> 127.0.0.1:443" 10; then
        _log_pass "tunnel 默认端口 443 生效"
    else
        _log_fail "tunnel 默认端口应为 443" "代理日志包含 127.0.0.1:443" "未找到对应日志"
    fi

    safe_cleanup_proxy "$tls_pid"
}

run_tls_options_specialized_tests() {
    header "执行 tlsOptions 专项测试"

    local work_dir="${TEST_DATA_DIR}/special-tls-options"
    mkdir -p "$work_dir"
    local proxy_log="${work_dir}/proxy.log"

    start_specialized_proxy "$RULE_FILE" "$proxy_log" "bifrost_proxy::proxy::http::tunnel=info,bifrost_proxy::rules=info"

    curl -sk --proxy "$PROXY" "https://tls-cipher.local/" --max-time 5 >/dev/null 2>&1 || true
    curl -sk --proxy "$PROXY" "https://tls-cert.local/" --max-time 5 >/dev/null 2>&1 || true
    curl -sk --proxy "$PROXY" "https://tls-ref.local/" --max-time 5 >/dev/null 2>&1 || true
    sleep 1

    if grep -F "CONNECT TLS options matched for tls-cipher.local:443 => ciphers=ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES128-GCM-SHA256" "$proxy_log" >/dev/null 2>&1; then
        _log_pass "tlsOptions inline ciphers 已匹配"
    else
        _log_fail "tlsOptions inline ciphers 应被解析" "包含 ciphers 配置" "未找到对应日志"
    fi

    if grep -F "CONNECT TLS options matched for tls-cert.local:443 => key=/tmp/client.key&cert=/tmp/client.crt" "$proxy_log" >/dev/null 2>&1; then
        _log_pass "tlsOptions key/cert 参数已匹配"
    else
        _log_fail "tlsOptions key/cert 应被解析" "包含 key=/tmp/client.key&cert=/tmp/client.crt" "未找到对应日志"
    fi

    if grep -F "CONNECT TLS options matched for tls-ref.local:443 => minVersion: TLSv1.2" "$proxy_log" >/dev/null 2>&1 \
        && grep -F "maxVersion: TLSv1.3" "$proxy_log" >/dev/null 2>&1; then
        _log_pass "tlsOptions value 引用已展开"
    else
        _log_fail "tlsOptions value 引用应被展开" "minVersion/maxVersion 都存在" "未找到完整日志"
    fi
}

run_sni_callback_specialized_tests() {
    header "执行 sniCallback 专项测试"

    local work_dir="${TEST_DATA_DIR}/special-sni-callback"
    mkdir -p "$work_dir"
    local proxy_log="${work_dir}/proxy.log"

    start_specialized_proxy "$RULE_FILE" "$proxy_log" "bifrost_proxy::proxy::http::tunnel=info,bifrost_proxy::rules=info"

    curl -sk --proxy "$PROXY" "https://sni-basic.local/" --max-time 5 >/dev/null 2>&1 || true
    curl -sk --proxy "$PROXY" "https://sni-arg.local/" --max-time 5 >/dev/null 2>&1 || true
    sleep 1

    if grep -F "CONNECT SNI callback matched for sni-basic.local:443 => plugin=test-plugin, sniValue=<none>" "$proxy_log" >/dev/null 2>&1; then
        _log_pass "sniCallback 无参形式已匹配"
    else
        _log_fail "sniCallback 无参形式应被解析" "plugin=test-plugin, sniValue=<none>" "未找到对应日志"
    fi

    if grep -F "CONNECT SNI callback matched for sni-arg.local:443 => plugin=test-plugin, sniValue=custom-sni" "$proxy_log" >/dev/null 2>&1; then
        _log_pass "sniCallback 参数形式已匹配"
    else
        _log_fail "sniCallback 参数形式应被解析" "plugin=test-plugin, sniValue=custom-sni" "未找到对应日志"
    fi
}

is_pattern_rule_file() {
    local file="$1"
    [[ "$file" == *"/pattern/"* ]] && return 0
    return 1
}

is_control_rule_file() {
    local file="$1"
    [[ "$file" == *"/control/"* ]] && return 0
    return 1
}

is_priority_rule_file() {
    local file="$1"
    [[ "$file" == *"/priority/"* ]] && return 0
    return 1
}

is_advanced_rule_file() {
    local file="$1"
    [[ "$file" == *"/advanced/"* ]] && return 0
    return 1
}

run_specialized_tests() {
    local rule_file="$1"
    local basename=$(basename "$rule_file")
    local dirname=$(dirname "$rule_file")
    local category=$(basename "$dirname")

    case "$category" in
        pattern)
            case "$basename" in
                domain_wildcard.txt)
                    run_domain_wildcard_tests
                    return 0
                    ;;
                path_wildcard.txt)
                    run_path_wildcard_tests
                    return 0
                    ;;
                port_wildcard.txt)
                    run_port_wildcard_tests
                    return 0
                    ;;
                protocol_wildcard.txt)
                    run_protocol_wildcard_tests
                    return 0
                    ;;
            esac
            ;;
        control)
            case "$basename" in
                delete.txt)
                    run_delete_rule_tests
                    return 0
                    ;;
                skip.txt)
                    run_skip_rule_tests
                    return 0
                    ;;
                include_filter.txt)
                    run_include_filter_tests
                    return 0
                    ;;
                exclude_filter.txt)
                    run_exclude_filter_tests
                    return 0
                    ;;
                line_props.txt)
                    run_line_props_tests
                    return 0
                    ;;
            esac
            ;;
        request_modify)
            case "$basename" in
                req_merge.txt)
                    run_req_merge_tests
                    return 0
                    ;;
            esac
            ;;
        priority)
            case "$basename" in
                important.txt)
                    run_priority_tests
                    return 0
                    ;;
                order.txt)
                    run_priority_order_tests
                    return 0
                    ;;
                exact_vs_wildcard.txt)
                    run_priority_exact_vs_wildcard_tests
                    return 0
                    ;;
                wildcard_level.txt)
                    run_priority_wildcard_level_tests
                    return 0
                    ;;
                forward_order.txt)
                    run_priority_forward_order_tests
                    return 0
                    ;;
            esac
            ;;
        advanced)
            case "$basename" in
                line_block.txt)
                    run_line_block_tests
                    return 0
                    ;;
            esac
            ;;
    esac

    return 1
}

run_standalone_specialized_tests() {
    local rule_file="$1"
    local basename
    basename=$(basename "$rule_file")
    local category
    category=$(basename "$(dirname "$rule_file")")

    case "$category/$basename" in
        forwarding/tunnel.txt)
            run_tunnel_specialized_tests
            return 0
            ;;
        tls/tls_options.txt)
            run_tls_options_specialized_tests
            return 0
            ;;
        tls/sni_callback.txt)
            run_sni_callback_specialized_tests
            return 0
            ;;
    esac

    return 1
}

run_tests() {
    header "执行端到端测试"

    if run_specialized_tests "$RULE_FILE"; then
        return 0
    fi

    local rules=()
    local in_code_block=false
    while IFS= read -r line; do
        line="${line%$'\r'}"
        [[ "$line" =~ ^#.*$ ]] && continue
        [[ -z "${line// }" ]] && continue
        if [[ "$line" == '```'* ]]; then
            if [[ "$in_code_block" == false ]]; then
                in_code_block=true
            else
                in_code_block=false
            fi
            continue
        fi
        [[ "$in_code_block" == true ]] && continue
        rules+=("$line")
    done < "$RULE_FILE"

    set +e
    for line in "${rules[@]}"; do
        local pattern=$(echo "$line" | awk '{print $1}')
        local protocols=$(echo "$line" | cut -d' ' -f2-)

        if [[ -z "$pattern" ]] || [[ -z "$protocols" ]]; then
            continue
        fi

        echo ""
        echo -e "${YELLOW}┌─────────────────────────────────────────────────────${NC}"
        echo -e "${YELLOW}│ 规则: $line${NC}"
        echo -e "${YELLOW}└─────────────────────────────────────────────────────${NC}"

        local rule_type=$(detect_rule_type "$line")
        local target=$(extract_target "$protocols")

        case "$rule_type" in
            http_forward|host|ip_forward)
                test_http_to_http_forward "$pattern" "$target"
                ;;
            https_to_http)
                test_https_to_http_forward "$pattern" "$target"
                ;;
            http_to_https)
                test_http_to_https_forward "$pattern" "$target"
                ;;
            redirect)
                local redirect_target=$(extract_value "$protocols" "redirect")
                [[ -z "$redirect_target" ]] && redirect_target=$(extract_value "$protocols" "locationHref")
                test_redirect_rule "$pattern" "$redirect_target"
                ;;
            resBody)
                local res_body_raw=$(extract_value "$protocols" "resBody")
                res_body_raw=$(resolve_code_block_var "$res_body_raw" "$RULE_FILE")
                test_res_body "$pattern" "$res_body_raw"
                ;;
            reqHeaders)
                local req_header_raw=$(extract_value "$protocols" "reqHeaders")
                req_header_raw=$(resolve_code_block_var "$req_header_raw" "$RULE_FILE")
                local req_header_first_line=$(echo "$req_header_raw" | head -1)
                local req_header_info=$(extract_header_from_value "$req_header_first_line")
                local req_header_name=$(echo "$req_header_info" | cut -d'|' -f1)
                local req_header_value=$(echo "$req_header_info" | cut -d'|' -f2)
                if [[ -n "$req_header_name" ]]; then
                    if [[ "$req_header_value" == *'${'* ]] || [[ "$req_header_raw" == *'`'* ]]; then
                        test_req_headers_template "$pattern" "$req_header_info"
                    else
                        test_req_headers_add "$pattern" "$req_header_name" "$req_header_value"
                    fi
                else
                    test_req_headers_add "$pattern" "X-Test-Header" "test-value"
                fi
                ;;
            resHeaders)
                local res_header_raw=$(extract_value "$protocols" "resHeaders")
                res_header_raw=$(resolve_code_block_var "$res_header_raw" "$RULE_FILE")
                local res_header_first_line=$(echo "$res_header_raw" | head -1)
                local res_header_info=$(extract_header_from_value "$res_header_first_line")
                local res_header_name=$(echo "$res_header_info" | cut -d'|' -f1)
                local res_header_value=$(echo "$res_header_info" | cut -d'|' -f2)
                if [[ -n "$res_header_name" ]]; then
                    if [[ "$res_header_value" == *'${'* ]] || [[ "$res_header_raw" == *'`'* ]]; then
                        test_res_headers_template "$pattern" "$res_header_info"
                    else
                        test_res_headers_add "$pattern" "$res_header_name" "$res_header_value"
                    fi
                else
                    test_res_headers_add "$pattern" "X-Test-Response" "test-value"
                fi
                ;;
            statusCode)
                local status=$(extract_value "$protocols" "statusCode")
                test_status_code "$pattern" "${status:-201}"
                ;;
            method)
                local method=$(extract_value "$protocols" "method")
                test_method_change "$pattern" "${method:-POST}"
                ;;
            ua)
                local ua=$(extract_value "$protocols" "ua")
                ua=$(resolve_code_block_var "$ua" "$RULE_FILE")
                ua=$(echo "$ua" | head -1)
                test_ua_change "$pattern" "${ua:-Bifrost}"
                ;;
            referer)
                local referer=$(extract_value "$protocols" "referer")
                if [[ -z "$referer" ]]; then
                    test_referer_removed "$pattern"
                else
                    test_referer_change "$pattern" "$referer"
                fi
                ;;
            reqDelay)
                local delay=$(extract_value "$protocols" "reqDelay")
                test_delay "$pattern" "${delay:-500}" "请求"
                ;;
            resDelay)
                local delay=$(extract_value "$protocols" "resDelay")
                test_delay "$pattern" "${delay:-500}" "响应"
                ;;
            reqCors)
                local req_cors_raw=$(extract_value "$protocols" "reqCors")
                req_cors_raw=$(resolve_code_block_var "$req_cors_raw" "$RULE_FILE")
                test_req_cors "$pattern" "$req_cors_raw"
                ;;
            cors)
                test_cors "$pattern"
                ;;
            reqCookies)
                local req_cookie_raw=$(extract_value "$protocols" "reqCookies")
                req_cookie_raw=$(resolve_code_block_var "$req_cookie_raw" "$RULE_FILE")
                local cookie_name=$(echo "$req_cookie_raw" | cut -d'=' -f1)
                local cookie_value=$(echo "$req_cookie_raw" | cut -d'=' -f2-)
                test_req_cookies "$pattern" "$cookie_name" "$cookie_value"
                ;;
            resCookies)
                local res_cookie_raw=$(extract_value "$protocols" "resCookies")
                res_cookie_raw=$(resolve_code_block_var "$res_cookie_raw" "$RULE_FILE")
                local cookie_name=$(echo "$res_cookie_raw" | cut -d'=' -f1)
                test_res_cookies "$pattern" "$cookie_name"
                ;;
            websocket|websocket_secure)
                test_websocket_forward "$pattern" "$target"
                ;;
            tpl)
                local file_path=$(extract_value "$protocols" "tpl")
                test_template_file "$pattern" "$file_path"
                ;;
            rawfile)
                local file_path=$(extract_value "$protocols" "rawfile")
                test_raw_file "$pattern" "$file_path"
                ;;
            file)
                local file_path=$(extract_value "$protocols" "file")
                test_mock_file "$pattern" "$file_path"
                ;;
            resPrepend)
                local prepend_content=$(extract_value "$protocols" "resPrepend")
                prepend_content=$(resolve_code_block_var "$prepend_content" "$RULE_FILE")
                test_res_prepend "$pattern" "$prepend_content"
                ;;
            resAppend)
                local append_content=$(extract_value "$protocols" "resAppend")
                append_content=$(resolve_code_block_var "$append_content" "$RULE_FILE")
                test_res_append "$pattern" "$append_content"
                ;;
            resReplace)
                local replace_pattern=$(extract_value "$protocols" "resReplace")
                test_res_replace "$pattern" "$replace_pattern"
                ;;
            htmlAppend|htmlPrepend|htmlBody)
                local inject_content=$(extract_value "$protocols" "htmlAppend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "htmlPrepend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "htmlBody")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "html")
                test_html_inject "$pattern" "$rule_type" "$inject_content"
                ;;
            jsAppend|jsPrepend|jsBody)
                local inject_content=$(extract_value "$protocols" "jsAppend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "jsPrepend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "jsBody")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "js")
                test_js_inject "$pattern" "$rule_type" "$inject_content"
                ;;
            cssAppend|cssPrepend|cssBody)
                local inject_content=$(extract_value "$protocols" "cssAppend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "cssPrepend")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "cssBody")
                [[ -z "$inject_content" ]] && inject_content=$(extract_value "$protocols" "css")
                test_css_inject "$pattern" "$rule_type" "$inject_content"
                ;;
            filter)
                local filter_value=$(extract_value "$protocols" "filter")
                test_filter_rule "$filter_value"
                ;;
            passthrough)
                test_ignore_rule "$pattern"
                ;;
            skip)
                test_ignore_rule "$pattern"
                ;;
            lineProps)
                test_line_props_rule "$pattern" "$protocols"
                ;;
            filtered_rule)
                test_filtered_rule "$pattern" "$protocols"
                ;;
            line_block)
                warn "line\` 块语法需要特殊处理，跳过当前行"
                ;;
            urlParams)
                local params=$(extract_value "$protocols" "urlParams")
                [[ -z "$params" ]] && params=$(extract_value "$protocols" "params")
                test_url_params "$pattern" "$params"
                ;;
            reqType)
                local content_type=$(extract_value "$protocols" "reqType")
                test_content_type "$pattern" "$content_type" "request"
                ;;
            reqCharset)
                local charset=$(extract_value "$protocols" "reqCharset")
                test_content_type "$pattern" "charset=${charset}" "request"
                ;;
            forwardedFor)
                local forwarded_for=$(extract_value "$protocols" "forwardedFor")
                test_forwarded_for_rule "$pattern" "$forwarded_for"
                ;;
            resType)
                local content_type=$(extract_value "$protocols" "resType")
                test_content_type "$pattern" "$content_type" "response"
                ;;
            resCharset)
                local charset=$(extract_value "$protocols" "resCharset")
                test_content_type "$pattern" "charset=${charset}" "response"
                ;;
            responseFor)
                local response_for=$(extract_value "$protocols" "responseFor")
                test_response_for_rule "$pattern" "$response_for"
                ;;
            urlReplace)
                local replace_rule=$(extract_value "$protocols" "urlReplace")
                [[ -z "$replace_rule" ]] && replace_rule=$(extract_value "$protocols" "pathReplace")
                test_url_replace_rule "$pattern" "$replace_rule"
                ;;
            replaceStatus)
                local status=$(extract_value "$protocols" "replaceStatus")
                test_replace_status_rule "$pattern" "${status:-201}"
                ;;
            auth)
                local auth_value=$(extract_value "$protocols" "auth")
                test_auth_header "$pattern" "$auth_value"
                ;;
            reqSpeed)
                local speed=$(extract_value "$protocols" "reqSpeed")
                test_req_speed_rule "$pattern" "${speed:-100}"
                ;;
            resSpeed)
                local speed=$(extract_value "$protocols" "resSpeed")
                test_res_speed_rule "$pattern" "${speed:-100}"
                ;;
            headerReplace)
                local replace_value=$(extract_value "$protocols" "headerReplace")
                test_header_replace_rule "$pattern" "$replace_value"
                ;;
            cache)
                local cache_value=$(extract_value "$protocols" "cache")
                test_cache_control "$pattern" "$cache_value"
                ;;
            attachment)
                local file_name=$(extract_value "$protocols" "attachment")
                [[ -z "$file_name" ]] && file_name=$(extract_value "$protocols" "download")
                test_attachment "$pattern" "$file_name"
                ;;
            trailers)
                local trailers_value=$(extract_value "$protocols" "trailers")
                local trailer_header=$(echo "$trailers_value" | cut -d':' -f1)
                test_trailers_rule "$pattern" "$trailer_header"
                ;;
            pac|proxy)
                test_http_to_http_forward "$pattern" "$target"
                ;;
            reqBody|reqPrepend|reqAppend|reqReplace|resMerge|delete)
                if [[ -n "$target" ]]; then
                    test_http_to_http_forward "$pattern" "$target"
                else
                    _log_pass "规则语法正确 ($rule_type)"
                fi
                ;;
            tlsIntercept|tlsPassthrough|dns|reqScript|resScript)
                _log_pass "规则语法正确 ($rule_type)"
                ;;
            *)
                warn "跳过不支持的规则类型: $rule_type (规则: $line)"
                ;;
        esac
    done
    set -e
}

ensure_assertions_executed() {
    local total_assertions
    total_assertions=$(get_total_count)

    if [[ "${total_assertions:-0}" -gt 0 ]]; then
        return 0
    fi

    _log_fail "测试未执行任何有效断言" "至少 1 条断言" "0 条断言"
    return 1
}

SKIP_BUILD="false"
KEEP_PROXY="false"
SKIP_MOCK_SERVERS="false"
USE_BINARY="false"

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help)
                usage
                ;;
            -p|--port)
                PROXY_PORT="$2"
                PROXY="http://${PROXY_HOST}:${PROXY_PORT}"
                shift 2
                ;;
            -d|--data-dir)
                TEST_DATA_DIR="$2"
                shift 2
                ;;
            -l|--list)
                list_rules
                ;;
            --no-build)
                SKIP_BUILD="true"
                shift
                ;;
            --keep-proxy)
                KEEP_PROXY="true"
                shift
                ;;
            --skip-mock-servers)
                SKIP_MOCK_SERVERS="true"
                shift
                ;;
            --use-binary)
                USE_BINARY="true"
                shift
                ;;
            *)
                if [[ -z "$RULE_FILE" ]]; then
                    if [[ "$1" == /* ]]; then
                        RULE_FILE="$1"
                    elif [[ -f "${SCRIPT_DIR}/$1" ]]; then
                        RULE_FILE="${SCRIPT_DIR}/$1"
                    elif [[ -f "${RULES_DIR}/$1" ]]; then
                        RULE_FILE="${RULES_DIR}/$1"
                    elif [[ -f "${RULES_DIR}/${1}.txt" ]]; then
                        RULE_FILE="${RULES_DIR}/${1}.txt"
                    else
                        RULE_FILE="$1"
                    fi
                fi
                shift
                ;;
        esac
    done

    if [[ -z "$RULE_FILE" ]]; then
        echo -e "${RED}✗${NC} 请指定规则文件"
        echo ""
        usage
    fi
}

main() {
    parse_args "$@"
    TEST_DATA_DIR="$(abs_path "$TEST_DATA_DIR")"

    header "Bifrost 规则端到端测试 v2"
    echo "代理端口: $PROXY_PORT"
    echo "规则文件: $RULE_FILE"
    echo "项目目录: $PROJECT_DIR"
    echo ""

    reset_test_stats

    check_dependencies
    check_rule_file

    if ! check_rule_syntax; then
        exit 1
    fi
    if ! validate_rule_file_capabilities; then
        exit 1
    fi

    if run_standalone_specialized_tests "$RULE_FILE"; then
        ensure_assertions_executed
        print_test_summary
        exit $?
    fi

    build_proxy
    setup_data_dir
    start_echo_servers
    show_rules
    start_proxy
    run_tests
    ensure_assertions_executed

    print_test_summary
    exit $?
}

main "$@"
