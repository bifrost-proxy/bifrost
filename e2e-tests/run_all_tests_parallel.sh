#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RULES_DIR="${SCRIPT_DIR}/rules"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RESULTS_DIR="${SCRIPT_DIR}/.test_results"
source "$SCRIPT_DIR/test_utils/process.sh"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

header() { echo -e "\n${CYAN}══════════════════════════════════════════════════════════════${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}══════════════════════════════════════════════════════════════${NC}\n"; }
info() { echo -e "${BLUE}ℹ${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

truthy() {
    local value="${1:-}"
    value="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')"
    [[ "$value" == "1" || "$value" == "true" || "$value" == "yes" || "$value" == "on" ]]
}

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

ensure_cargo_on_path() {
    if command -v cargo >/dev/null 2>&1; then
        return 0
    fi

    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        export PATH="${HOME}/.cargo/bin:${PATH}"
    fi
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

FIXTURE_ONLY_RULES=(
    "comprehensive_test.txt"
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
        if ! resolve_bifrost_release_bin >/dev/null 2>&1; then
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

    local timeout="${TIMEOUT:-60}"
    local fixture_timeout="${BIFROST_E2E_FIXTURE_TIMEOUT:-180}"

    mkdir -p "$data_dir"

    local http_retries="${BIFROST_E2E_HTTP_RETRIES:-2}"

    {
        echo "TEST_FILE=$rel_path"
        echo "PROXY_PORT=$proxy_port"

        local test_id="${rel_path}:${proxy_port}"
        local test_pid
        TIMEOUT="$timeout" TEST_ID="$test_id" BIFROST_E2E_HTTP_RETRIES="$http_retries" "$SCRIPT_DIR/test_rules.sh" \
            --no-build \
            --use-binary \
            --skip-mock-servers \
            -p "$proxy_port" \
            -d "$data_dir" \
            "$rule_file" > "$log_file" 2>&1 &
        test_pid=$!

        local watchdog_pid=""
        (
            sleep "$fixture_timeout"
            if kill -0 "$test_pid" 2>/dev/null; then
                echo "[TIMEOUT] fixture ${rel_path} exceeded ${fixture_timeout}s on port ${proxy_port}" >> "$log_file"
                kill -TERM "$test_pid" 2>/dev/null || true
                sleep 3
                kill -9 "$test_pid" 2>/dev/null || true
            fi
        ) &
        watchdog_pid=$!

        if wait "$test_pid" 2>/dev/null; then
            echo "STATUS=passed"
        else
            echo "STATUS=failed"
        fi

        kill "$watchdog_pid" 2>/dev/null || true
        wait "$watchdog_pid" 2>/dev/null || true

        local passed=$(grep "^Passed:" "$log_file" 2>/dev/null | tail -1 | perl -pe 's/\e\[[0-9;]*m//g' | sed 's/.*: *//' | tr -d '[:space:]' || echo "0")
        local failed=$(grep "^Failed:" "$log_file" 2>/dev/null | tail -1 | perl -pe 's/\e\[[0-9;]*m//g' | sed 's/.*: *//' | tr -d '[:space:]' || echo "0")
        echo "PASSED=${passed:-0}"
        echo "FAILED=${failed:-0}"

        if is_windows; then
            kill_bifrost_on_port "$proxy_port"
        fi
    } > "$result_file"
}

print_progress() {
    local completed="$1"
    local total="$2"
    local fixture_name="${3:-}"
    local width=50
    local percent=$((completed * 100 / total))
    local filled=$((completed * width / total))
    local empty=$((width - filled))

    if [[ -t 1 ]]; then
        printf "\r${CYAN}进度: [${NC}"
        printf "%${filled}s" | tr ' ' '█'
        printf "%${empty}s" | tr ' ' '░'
        printf "${CYAN}] %3d%% (%d/%d)${NC}" "$percent" "$completed" "$total"
    else
        local bar=""
        bar+=$(printf "%${filled}s" | tr ' ' '█')
        bar+=$(printf "%${empty}s" | tr ' ' '░')
        if [[ -n "$fixture_name" ]]; then
            printf "进度: [%s] %3d%% (%d/%d) %s\n" "$bar" "$percent" "$completed" "$total" "$fixture_name"
        else
            printf "进度: [%s] %3d%% (%d/%d)\n" "$bar" "$percent" "$completed" "$total"
        fi
    fi
}

print_failure_diagnostics() {
    local log_file="$1"
    [[ -f "$log_file" ]] || return 0

    local clean_log
    clean_log=$(mktemp)
    perl -pe 's/\e\[[0-9;]*m//g' "$log_file" > "$clean_log"

    local failure_excerpt
    failure_excerpt=$(awk '
        /│ 规则:/ { current_rule=$0; next }
        /【测试】/ { current_test=$0; next }
        /^✗ / {
            if (current_rule != "") print current_rule;
            if (current_test != "") print current_test;
            print $0;
            for (i = 0; i < 2; i++) {
                if (getline line) print line;
            }
            print "";
        }
    ' "$clean_log")

    if [[ -n "$failure_excerpt" ]]; then
        echo -e "    ${YELLOW}失败断言摘录:${NC}"
        printf '%s\n' "$failure_excerpt" | tail -60 | sed 's/^/      /'
    fi

    local warning_excerpt
    warning_excerpt=$(rg -n "请求速度测试第|响应速度测试第|超时: " "$clean_log" -S 2>/dev/null || true)
    if [[ -n "$warning_excerpt" ]]; then
        echo -e "    ${YELLOW}测速诊断摘录:${NC}"
        printf '%s\n' "$warning_excerpt" | tail -20 | sed 's/^/      /'
    fi

    rm -f "$clean_log"
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
            value="${value%$'\r'}"
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
                            print_failure_diagnostics "$log_file"
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
    if is_windows; then
        kill_all_bifrost
    fi
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
}

collect_failed_result_indices() {
    local result_file

    for result_file in "${RESULTS_DIR}"/result_*.txt; do
        [[ -f "$result_file" ]] || continue

        local status=""
        status=$(grep '^STATUS=' "$result_file" 2>/dev/null | tail -1 | cut -d'=' -f2-)
        if [[ "$status" != "failed" ]]; then
            continue
        fi

        local idx="${result_file##*result_}"
        idx="${idx%.txt}"
        printf '%s\n' "$idx"
    done
}

ensure_mock_servers_alive() {
    if is_http_echo_ready; then
        return 0
    fi

    warn "Mock 服务器不可用，尝试重启..."
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
    sleep 1
    "$SCRIPT_DIR/mock_servers/start_servers.sh" start-bg
    sleep 2

    if is_http_echo_ready; then
        echo -e "${GREEN}✓${NC} Mock 服务器已重启"
        return 0
    fi

    echo -e "${RED}✗${NC} Mock 服务器重启失败"
    return 1
}

retry_failed_suites_once() {
    local failed_indices=()
    while IFS= read -r idx; do
        [[ -n "$idx" ]] && failed_indices+=("$idx")
    done < <(collect_failed_result_indices)

    if [[ ${#failed_indices[@]} -eq 0 ]]; then
        return 0
    fi

    local retry_budget="${BIFROST_E2E_RETRY_BUDGET_SECS:-300}"
    local retry_start=$SECONDS
    local retried=0
    local skipped=0

    local max_retry_suites="${BIFROST_E2E_MAX_RETRY_SUITES:-10}"
    if [[ ${#failed_indices[@]} -gt $max_retry_suites ]]; then
        warn "失败套件过多 (${#failed_indices[@]} > ${max_retry_suites})，仅重试前 ${max_retry_suites} 个"
        failed_indices=("${failed_indices[@]:0:$max_retry_suites}")
    fi

    header "串行重试失败套件"
    info "首次运行失败 ${#failed_indices[@]} 个套件，按原端口逐个重试 (时间预算: ${retry_budget}s)"

    if is_windows; then
        info "Windows: 重试前清理残留 bifrost 进程..."
        kill_all_bifrost
        sleep 3
    fi

    if ! ensure_mock_servers_alive; then
        warn "Mock 服务器不可用，跳过全部重试"
        return 0
    fi

    local retry_fixture_timeout="${BIFROST_E2E_RETRY_FIXTURE_TIMEOUT:-${BIFROST_E2E_FIXTURE_TIMEOUT:-120}}"
    local saved_fixture_timeout="${BIFROST_E2E_FIXTURE_TIMEOUT:-}"
    export BIFROST_E2E_FIXTURE_TIMEOUT="$retry_fixture_timeout"

    local idx
    for idx in "${failed_indices[@]}"; do
        local elapsed=$(( SECONDS - retry_start ))
        if [[ $elapsed -ge $retry_budget ]]; then
            skipped=$(( ${#failed_indices[@]} - retried ))
            warn "重试时间预算已用尽 (${elapsed}s >= ${retry_budget}s)，跳过剩余 ${skipped} 个套件"
            break
        fi

        local remaining=$(( retry_budget - elapsed ))
        if [[ $remaining -lt 30 ]]; then
            skipped=$(( ${#failed_indices[@]} - retried ))
            warn "剩余时间不足 (${remaining}s < 30s)，跳过剩余 ${skipped} 个套件"
            break
        fi

        local result_file="${RESULTS_DIR}/result_${idx}.txt"
        local log_file="${RESULTS_DIR}/log_${idx}.txt"
        local data_dir="${RESULTS_DIR}/data_${idx}"
        local rule_rel=""
        local rule_file=""

        rule_rel=$(grep '^TEST_FILE=' "$result_file" 2>/dev/null | tail -1 | cut -d'=' -f2-)
        rule_file="${RULES_DIR}/${rule_rel}"

        if [[ -z "$rule_rel" || ! -f "$rule_file" ]]; then
            warn "无法定位失败套件 ${idx} 对应的规则文件，跳过重试"
            retried=$((retried + 1))
            continue
        fi

        info "重试 ${rule_rel} (proxy_port=$((BASE_PORT + idx))) [${elapsed}s/${retry_budget}s]"
        rm -rf "$data_dir" "$log_file" "$result_file"
        if is_windows; then
            kill_bifrost_on_port "$((BASE_PORT + idx))"
        fi
        run_single_test "$rule_file" "$idx"
        retried=$((retried + 1))

        local status=""
        status=$(grep '^STATUS=' "$result_file" 2>/dev/null | tail -1 | cut -d'=' -f2-)
        if [[ "$status" == "passed" ]]; then
            echo -e "${GREEN}✓${NC} 重试通过: ${rule_rel}"
        else
            warn "重试仍失败: ${rule_rel}"
            if [[ -f "$log_file" ]]; then
                print_failure_diagnostics "$log_file"
                echo -e "    ${YELLOW}最后 20 行日志:${NC}"
                tail -20 "$log_file" | sed 's/^/      /'
            fi
        fi
    done

    if [[ -n "$saved_fixture_timeout" ]]; then
        export BIFROST_E2E_FIXTURE_TIMEOUT="$saved_fixture_timeout"
    else
        unset BIFROST_E2E_FIXTURE_TIMEOUT
    fi
}

is_http_echo_ready() {
    curl -sf "http://127.0.0.1:3000/health" >/dev/null 2>&1
}

ensure_cargo_on_path

JOBS="${BIFROST_E2E_RULE_JOBS:-$(detect_cpu_count)}"
CATEGORY=""
SKIP_BUILD="false"
VERBOSE="false"
BASE_PORT=9000
RETRY_FAILED_ONCE="false"

pick_available_base_port() {
    local requested_base_port="$1"
    local suite_count="$2"
    local attempt=0

    while [[ $attempt -lt 30 ]]; do
        local candidate=$((requested_base_port + attempt * 100))
        local ok=true

        if is_windows; then
            local used_ports
            used_ports=$(netstat.exe -ano 2>/dev/null \
                | awk '$1 == "TCP" && $4 == "LISTENING" { split($2, a, ":"); print a[length(a)] }' \
                | tr -d '\r' | sort -un | tr '\n' ',' || true)
            for ((p=candidate; p<candidate + suite_count; p++)); do
                if [[ ",$used_ports," == *",$p,"* ]]; then
                    ok=false
                    break
                fi
            done
        else
            for ((p=candidate; p<candidate + suite_count; p++)); do
                if lsof -i ":${p}" -t >/dev/null 2>&1; then
                    ok=false
                    break
                fi
            done
        fi

        if [[ "$ok" == "true" ]]; then
            echo "$candidate"
            return 0
        fi

        attempt=$((attempt + 1))
    done

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
            --retry-failed-once)
                RETRY_FAILED_ONCE="true"
                shift
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
    if truthy "${BIFROST_E2E_RETRY_FAILED_ONCE:-false}"; then
        RETRY_FAILED_ONCE="true"
    fi

    header "Bifrost 并行端到端测试运行器"
    echo "并行任务数: $JOBS"
    echo "起始端口: $BASE_PORT"
    if [[ -n "$CATEGORY" ]]; then
        echo "测试分类: $CATEGORY"
    fi
    if [[ "$RETRY_FAILED_ONCE" == "true" ]]; then
        echo "失败重试: 开启（串行重试一次）"
    fi
    echo ""

    rm -rf "$RESULTS_DIR"
    mkdir -p "$RESULTS_DIR"

    trap cleanup EXIT

    header "编译代理服务器"
    build_proxy_once

    header "启动共享 Mock 服务器"
    info "停止可能存在的旧 Mock 服务器..."
    "$SCRIPT_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
    sleep 1
    info "启动 Mock 服务器 (后台模式)..."
    "$SCRIPT_DIR/mock_servers/start_servers.sh" start-bg
    info "等待 Mock 服务器就绪..."
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

    info "找到 $total_suites 个测试套件"

    info "选择可用端口段..."
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
                local fixture_rel="${test_files[$i]#$RULES_DIR/}"
                local result_status=""
                local rf="${RESULTS_DIR}/result_${i}.txt"
                if [[ -f "$rf" ]]; then
                    result_status=$(grep "^STATUS=" "$rf" 2>/dev/null | head -1 | cut -d= -f2)
                fi
                local label="${fixture_rel}"
                if [[ "$result_status" == "passed" ]]; then
                    label="✓ ${fixture_rel}"
                elif [[ "$result_status" == "failed" ]]; then
                    label="✗ ${fixture_rel}"
                fi
                print_progress "$completed" "$total_suites" "$label"
            fi
        done

        sleep 0.1
    done

    echo ""
    echo ""

    if ! aggregate_results; then
        if [[ "$RETRY_FAILED_ONCE" == "true" ]]; then
            retry_failed_suites_once
            echo ""
            aggregate_results
        else
            return 1
        fi
    fi
}

main "$@"
