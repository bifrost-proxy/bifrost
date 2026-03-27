#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RULES_DIR="${SCRIPT_DIR}/rules"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RESULTS_DIR="${SCRIPT_DIR}/.test_results"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

header() { echo -e "\n${CYAN}══════════════════════════════════════════════════════════════${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}══════════════════════════════════════════════════════════════${NC}\n"; }
info() { echo -e "${BLUE}ℹ${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

usage() {
    echo "用法: $0 [选项]"
    echo ""
    echo "并行执行端到端测试，共享 mock 服务器，每个测试使用独立的代理端口和数据目录。"
    echo ""
    echo "选项:"
    echo "  -h, --help         显示帮助信息"
    echo "  -j, --jobs N       并行任务数 (默认: CPU 核心数)"
    echo "  -c, --category CAT 只运行指定分类的测试"
    echo "  --no-build         跳过编译步骤"
    echo "  --base-port PORT   起始端口号 (默认: 9000)"
    echo "  -v, --verbose      详细输出"
    echo ""
    echo "示例:"
    echo "  $0                     # 使用默认并行度运行所有测试"
    echo "  $0 -j 4                # 使用 4 个并行任务"
    echo "  $0 -c forwarding       # 只运行转发测试"
    exit 0
}

detect_cpu_count() {
    if command -v nproc >/dev/null 2>&1; then
        nproc
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.ncpu
    else
        echo 4
    fi
}

FIXTURE_ONLY_RULES=(
    "admin_api/create_proxy_rule.txt"
    "admin_api/list_contains_created_rule.txt"
    "admin_api/rule_count_multiline.txt"
    "admin_api/update_mock_file_rule.txt"
    "advanced/body_replace.txt"
    "advanced/body_size_strategy.txt"
    "advanced/header_replace.txt"
    "advanced/large_body.txt"
    "forwarding/nextoncall_rules.txt"
    "hot_reload/status_201.txt"
    "hot_reload/status_202.txt"
    "http3/http3_e2e.txt"
    "http3/http3_mitm_tls_header.txt"
    "http3/http3_rules_header.txt"
    "regression/rule_semantics_split_parsing.txt"
    "replay/delete_header.txt"
    "replay/host_redirect.txt"
    "replay/method_post.txt"
    "replay/multiple_rules.txt"
    "replay/referer.txt"
    "replay/req_body.txt"
    "replay/req_cookies.txt"
    "replay/req_headers.txt"
    "replay/response_modification.txt"
    "replay/sse_req_headers.txt"
    "replay/url_params.txt"
    "replay/user_agent.txt"
    "replay/websocket_req_headers.txt"
    "request_modify/req_res_script.txt"
    "runtime/client_process_transport_attribution.txt"
    "socks5/block_domain.txt"
    "socks5/host_redirect.txt"
    "socks5_tls/compare_header_mode.txt"
    "socks5_tls/host_redirect.txt"
    "socks5_tls/mock_response.txt"
    "socks5_tls/res_header.txt"
    "socks5_udp/dns_redirect_domain.txt"
    "socks5_udp/dns_redirect_ip.txt"
    "system_proxy/basic_forwarding.txt"
    "tls/intercept_header_injection.txt"
    "tls/passthrough_localhost.txt"
    "values/status_code_value.txt"
    "websocket/decode_utf8_searchable.txt"
)

should_skip_rule_fixture() {
    local rel_path="$1"
    local fixture

    for fixture in "${FIXTURE_ONLY_RULES[@]}"; do
        if [[ "$rel_path" == "$fixture" ]]; then
            return 0
        fi
    done

    return 1
}

collect_test_files() {
    local category="$1"

    if [[ -n "$category" ]]; then
        if [[ -d "${RULES_DIR}/${category}" ]]; then
            find "${RULES_DIR}/${category}" -name "*.txt" -type f 2>/dev/null | sort | while read -r rule_file; do
                local rel_path="${rule_file#$RULES_DIR/}"
                if should_skip_rule_fixture "$rel_path"; then
                    continue
                fi
                echo "$rule_file"
            done
        else
            echo -e "${RED}✗${NC} 分类不存在: $category" >&2
            exit 1
        fi
    else
        find "$RULES_DIR" -name "*.txt" -type f 2>/dev/null | sort | while read -r rule_file; do
            local rel_path="${rule_file#$RULES_DIR/}"
            if should_skip_rule_fixture "$rel_path"; then
                continue
            fi
            echo "$rule_file"
        done
    fi
}

build_proxy_once() {
    if [[ "$SKIP_BUILD" == "true" ]]; then
        if [[ ! -x "${PROJECT_DIR}/target/release/bifrost" ]]; then
            echo -e "${RED}✗${NC} 跳过编译但二进制文件不存在，请先编译"
            exit 1
        fi
        echo -e "${GREEN}✓${NC} 跳过编译，使用现有二进制"
        return 0
    fi

    info "编译代理服务器 (cargo build --release)..."
    cd "$PROJECT_DIR"
    if ! cargo build --release --bin bifrost; then
        echo -e "${RED}✗${NC} 编译失败"
        exit 1
    fi
    echo -e "${GREEN}✓${NC} 编译完成"
}

run_single_test() {
    local rule_file="$1"
    local test_index="$2"
    local proxy_port=$((BASE_PORT + test_index))
    local rel_path="${rule_file#$RULES_DIR/}"
    local result_file="${RESULTS_DIR}/result_${test_index}.txt"
    local log_file="${RESULTS_DIR}/log_${test_index}.txt"
    local data_dir="${RESULTS_DIR}/data_${test_index}"

    # 并行场景下，首次 HTTPS 请求可能因为证书/密钥生成 + CPU 竞争导致耗时显著增加。
    # test_utils/http_client.sh 默认 TIMEOUT=10s，容易在高并发下触发 curl 000。
    local timeout="${TIMEOUT:-60}"

    mkdir -p "$data_dir"

    {
        echo "TEST_FILE=$rel_path"
        echo "PROXY_PORT=$proxy_port"

        local test_id="${rel_path}:${proxy_port}"
        if TIMEOUT="$timeout" TEST_ID="$test_id" "$SCRIPT_DIR/test_rules.sh" \
            --no-build \
            --use-binary \
            --skip-mock-servers \
            -p "$proxy_port" \
            -d "$data_dir" \
            "$rule_file" > "$log_file" 2>&1; then
            echo "STATUS=passed"
        else
            echo "STATUS=failed"
        fi

        local passed=$(grep "^Passed:" "$log_file" 2>/dev/null | tail -1 | perl -pe 's/\e\[[0-9;]*m//g' | sed 's/.*: *//' | tr -d '[:space:]' || echo "0")
        local failed=$(grep "^Failed:" "$log_file" 2>/dev/null | tail -1 | perl -pe 's/\e\[[0-9;]*m//g' | sed 's/.*: *//' | tr -d '[:space:]' || echo "0")
        echo "PASSED=${passed:-0}"
        echo "FAILED=${failed:-0}"
    } > "$result_file"
}

print_progress() {
    local completed="$1"
    local total="$2"
    local width=50
    local percent=$((completed * 100 / total))
    local filled=$((completed * width / total))
    local empty=$((width - filled))

    printf "\r${CYAN}进度: [${NC}"
    printf "%${filled}s" | tr ' ' '█'
    printf "%${empty}s" | tr ' ' '░'
    printf "${CYAN}] %3d%% (%d/%d)${NC}" "$percent" "$completed" "$total"
}

aggregate_results() {
    local total_passed=0
    local total_failed=0
    local failed_suites=()
    local passed_suites=()

    for result_file in "${RESULTS_DIR}"/result_*.txt; do
        [[ -f "$result_file" ]] || continue

        local test_file=""
        local status=""
        local passed=0
        local failed=0

        while IFS='=' read -r key value; do
            case "$key" in
                TEST_FILE) test_file="$value" ;;
                STATUS) status="$value" ;;
                PASSED) passed="${value:-0}" ;;
                FAILED) failed="${value:-0}" ;;
            esac
        done < "$result_file"

        total_passed=$((total_passed + passed))
        total_failed=$((total_failed + failed))

        if [[ "$status" == "failed" ]]; then
            failed_suites+=("$test_file")
        else
            passed_suites+=("$test_file")
        fi
    done

    header "最终测试结果"

    echo -e "总断言数: $((total_passed + total_failed))"
    echo -e "通过: ${GREEN}${total_passed}${NC}"
    echo -e "失败: ${RED}${total_failed}${NC}"
    echo ""
    echo -e "测试套件: ${GREEN}${#passed_suites[@]} 通过${NC} / ${RED}${#failed_suites[@]} 失败${NC}"
    echo ""

    if [[ ${#failed_suites[@]} -gt 0 ]]; then
        echo -e "${RED}失败的测试套件:${NC}"
        for suite in "${failed_suites[@]}"; do
            echo "  - $suite"
            if [[ "$VERBOSE" == "true" ]]; then
                for f in "${RESULTS_DIR}"/result_*.txt; do
                    if grep -q "TEST_FILE=$suite" "$f" 2>/dev/null; then
                        local idx="${f##*result_}"
                        idx="${idx%.txt}"
                        local log_file="${RESULTS_DIR}/log_${idx}.txt"
                        if [[ -f "$log_file" ]]; then
                            echo -e "    ${YELLOW}最后 20 行日志:${NC}"
                            tail -20 "$log_file" | sed 's/^/      /'
                        fi
                        break
                    fi
                done
            fi
        done
        echo ""
    fi

    if [[ ${#failed_suites[@]} -eq 0 ]]; then
        echo -e "${GREEN}═══════════════════════════════════════${NC}"
        echo -e "${GREEN}  ✓ 所有测试通过！${NC}"
        echo -e "${GREEN}═══════════════════════════════════════${NC}"
        return 0
    else
        echo -e "${RED}═══════════════════════════════════════${NC}"
        echo -e "${RED}  ✗ 有 ${#failed_suites[@]} 个测试套件失败${NC}"
        echo -e "${RED}═══════════════════════════════════════${NC}"
        return 1
    fi
}

cleanup() {
    info "清理资源..."
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
}

is_http_echo_ready() {
    curl -sf "http://127.0.0.1:3000/health" >/dev/null 2>&1
}

JOBS=$(detect_cpu_count)
CATEGORY=""
SKIP_BUILD="false"
VERBOSE="false"
BASE_PORT=9000

pick_available_base_port() {
    local requested_base_port="$1"
    local suite_count="$2"
    local attempt=0

    # 需要保证 [base_port, base_port + suite_count - 1] 这一段端口都空闲，
    # 否则并行运行时会出现“误连旧进程/绑定失败”等随机用例失败。
    while [[ $attempt -lt 30 ]]; do
        local candidate=$((requested_base_port + attempt * 100))
        local ok=true

        for ((p=candidate; p<candidate + suite_count; p++)); do
            if lsof -i ":${p}" -t >/dev/null 2>&1; then
                ok=false
                break
            fi
        done

        if [[ "$ok" == "true" ]]; then
            echo "$candidate"
            return 0
        fi

        attempt=$((attempt + 1))
    done

    # 回退：返回原值，由后续单测自行报错。
    echo "$requested_base_port"
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help)
                usage
                ;;
            -j|--jobs)
                JOBS="$2"
                shift 2
                ;;
            -c|--category)
                CATEGORY="$2"
                shift 2
                ;;
            --no-build)
                SKIP_BUILD="true"
                shift
                ;;
            --base-port)
                BASE_PORT="$2"
                shift 2
                ;;
            -v|--verbose)
                VERBOSE="true"
                shift
                ;;
            *)
                warn "未知参数: $1"
                shift
                ;;
        esac
    done
}

main() {
    parse_args "$@"

    header "Bifrost 并行端到端测试运行器"
    echo "并行任务数: $JOBS"
    echo "起始端口: $BASE_PORT"
    if [[ -n "$CATEGORY" ]]; then
        echo "测试分类: $CATEGORY"
    fi
    echo ""

    rm -rf "$RESULTS_DIR"
    mkdir -p "$RESULTS_DIR"

    trap cleanup EXIT

    header "编译代理服务器"
    build_proxy_once

    header "启动共享 Mock 服务器"
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
    sleep 1
    "$SCRIPT_DIR/mock_servers/start_servers.sh" start-bg
    sleep 2

    if is_http_echo_ready; then
        echo -e "${GREEN}✓${NC} Mock 服务器已启动 (所有测试共享)"
    else
        echo -e "${RED}✗${NC} Mock 服务器启动失败"
        exit 1
    fi

    header "收集测试文件"
    local test_files=()
    while IFS= read -r file; do
        [[ -n "$file" ]] && test_files+=("$file")
    done < <(collect_test_files "$CATEGORY")

    local total_suites=${#test_files[@]}

    if [[ $total_suites -eq 0 ]]; then
        warn "没有找到测试文件"
        exit 0
    fi

    # 如果默认端口段被占用（例如本机已有 bifrost 或其它服务占用 9000），
    # 自动选择一个可用的连续端口段，避免并行测试误连旧进程/绑定失败。
    local selected_base_port
    selected_base_port=$(pick_available_base_port "$BASE_PORT" "$total_suites")
    if [[ "$selected_base_port" != "$BASE_PORT" ]]; then
        warn "起始端口 ${BASE_PORT} 的端口段已被占用，自动切换到 ${selected_base_port}"
    fi
    BASE_PORT="$selected_base_port"

    info "找到 $total_suites 个测试套件，使用 $JOBS 个并行任务"

    header "执行并行测试"

    local pids=()
    local completed=0
    local running=0
    local next_index=0

    while [[ $completed -lt $total_suites ]]; do
        while [[ $running -lt $JOBS && $next_index -lt $total_suites ]]; do
            run_single_test "${test_files[$next_index]}" "$next_index" &
            pids[$next_index]=$!
            running=$((running + 1))
            next_index=$((next_index + 1))
        done

        for i in "${!pids[@]}"; do
            if [[ -n "${pids[$i]}" ]] && ! kill -0 "${pids[$i]}" 2>/dev/null; then
                wait "${pids[$i]}" 2>/dev/null || true
                unset 'pids[i]'
                completed=$((completed + 1))
                running=$((running - 1))
                print_progress "$completed" "$total_suites"
            fi
        done

        sleep 0.1
    done

    echo ""
    echo ""

    aggregate_results
}

main "$@"
