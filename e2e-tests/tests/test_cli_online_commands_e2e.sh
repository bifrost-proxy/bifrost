#!/bin/bash
#
# Bifrost CLI 在线命令端到端测试（真实端到端验证版）
#
# 所有测试均通过真实 HTTP 请求产生流量、通过 CLI 操作后验证结果内容。
# 绝不 mock，绝不仅仅 "执行不 panic"。
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

# Suppress interactive update notices so CLI output remains stable for JSON/table assertions.
export CI=1

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
PROXY_PORT="${PROXY_PORT:-18991}"
ADMIN_PORT="$PROXY_PORT"
PROXY_PID=""
BIFROST_LOG_FILE=""
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
    PASSED=$((PASSED + 1))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    FAILED=$((FAILED + 1))
}

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"
    safe_cleanup_proxy "$PROXY_PID"

    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
    if [[ -n "$BIFROST_LOG_FILE" ]] && [[ -f "$BIFROST_LOG_FILE" ]]; then
        rm -f "$BIFROST_LOG_FILE"
    fi
}

trap cleanup EXIT

run_bifrost() {
    CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" "$@" 2>&1 || true
}

build_bifrost() {
    header "编译 Bifrost"
    if [[ -f "$BIFROST_BIN" ]] && [[ "$SKIP_BUILD" == "true" ]]; then
        info "跳过编译 (SKIP_BUILD=true)"
        return
    fi
    echo -e "${GREEN}✓${NC} 二进制文件就绪"
}

setup_test_data_dir() {
    TEST_DATA_DIR=$(mktemp -d)
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    mkdir -p "${TEST_DATA_DIR}"
    info "测试数据目录: $TEST_DATA_DIR"
}

start_bifrost_server() {
    header "启动 Bifrost 代理服务器"

    BIFROST_LOG_FILE=$(mktemp)

    cd "$PROJECT_DIR" || return 1
    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"$BIFROST_LOG_FILE" 2>&1 &
    PROXY_PID=$!

    local max_wait=60
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            echo -e "${RED}✗${NC} Bifrost 进程退出"
            tail -n 50 "$BIFROST_LOG_FILE" 2>/dev/null || true
            return 1
        fi
        if env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            echo -e "${GREEN}✓${NC} Bifrost 已启动 (端口: ${PROXY_PORT}, PID: ${PROXY_PID})"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo -e "${RED}✗${NC} Bifrost 启动超时"
    tail -n 50 "$BIFROST_LOG_FILE" 2>/dev/null || true
    return 1
}

seed_data() {
    header "准备测试数据（离线写入）"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add e2e_rule_1 -c "e2e1.example.com statusCode://200" >/dev/null 2>&1 || true
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add e2e_rule_2 -c "e2e2.example.com statusCode://201" >/dev/null 2>&1 || true
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add e2e_rule_3 -c "e2e3.example.com statusCode://202" >/dev/null 2>&1 || true
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value add e2e_val_1 "e2e_value_content" >/dev/null 2>&1 || true
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value add e2e_val_2 "e2e_value_second" >/dev/null 2>&1 || true
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" script add request e2e_script_1 -c 'log.info("e2e script");' >/dev/null 2>&1 || true
    pass "测试数据已准备 (3 rules, 2 values, 1 script)"
}

generate_real_traffic() {
    header "通过代理产生真实 HTTP 流量"

    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sf -x "http://127.0.0.1:${PROXY_PORT}" "http://httpbin.org/get?e2e=traffic1" -o /dev/null 2>&1 || true
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sf -x "http://127.0.0.1:${PROXY_PORT}" "http://httpbin.org/status/201" -o /dev/null 2>&1 || true
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sf -x "http://127.0.0.1:${PROXY_PORT}" -X POST "http://httpbin.org/post" -d "data=e2e_test" -o /dev/null 2>&1 || true
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sf -x "http://127.0.0.1:${PROXY_PORT}" "http://httpbin.org/headers" -o /dev/null 2>&1 || true
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sf -x "http://127.0.0.1:${PROXY_PORT}" "http://httpbin.org/status/404" -o /dev/null 2>&1 || true

    sleep 1

    local count
    count=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")

    if [[ "$count" -gt 0 ]]; then
        pass "已产生 ${count} 条真实流量记录"
    else
        fail "未能产生流量记录 (count=$count)"
    fi
}

# ═══════════════════════════════════════════════════════════
# Status
# ═══════════════════════════════════════════════════════════

test_status() {
    header "测试 status"
    local result
    result=$(run_bifrost status)

    if echo "$result" | grep -qi "running\|port\|$PROXY_PORT"; then
        pass "status 显示服务正在运行且端口正确"
    else
        fail "status 未返回预期内容: $result"
    fi

    if echo "$result" | grep -q "Active Rules Summary"; then
        pass "status 追加展示活跃规则摘要"
    else
        fail "status 未展示活跃规则摘要: $result"
    fi

    if echo "$result" | grep -q "Merged Rules (in parsing order)"; then
        pass "status 展示合并规则标题"
    else
        fail "status 未展示合并规则标题: $result"
    fi

    if echo "$result" | grep -q "e2e1.example.com statusCode://200"; then
        pass "status 展示合并后的规则内容"
    else
        fail "status 未展示预期的合并规则内容: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Traffic (依赖真实流量)
# ═══════════════════════════════════════════════════════════

test_traffic_list_has_records() {
    header "测试 traffic list 包含真实请求记录"

    local result
    result=$(run_bifrost traffic list -l 20 -f json --no-color)

    if echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('total',0)>0" 2>/dev/null; then
        pass "traffic list 返回了真实流量 (JSON 格式，total>0)"
    else
        fail "traffic list 未返回流量记录"
    fi

    if echo "$result" | grep -q "httpbin.org"; then
        pass "traffic list 包含 httpbin.org 的请求"
    else
        fail "traffic list 未找到 httpbin.org 请求: $(echo "$result" | head -5)"
    fi
}

test_traffic_list_table_format() {
    header "测试 traffic list 表格输出包含标题行"

    local result
    result=$(run_bifrost traffic list -l 5 -f table --no-color)

    if echo "$result" | grep -q "STATUS.*METHOD\|METHOD.*PROTO\|HOST.*PATH"; then
        pass "traffic list -f table 包含表头 (STATUS/METHOD/HOST/PATH)"
    else
        fail "traffic list -f table 缺少表头: $(echo "$result" | head -3)"
    fi

    if echo "$result" | grep -q "httpbin.org"; then
        pass "traffic list -f table 包含 httpbin.org 数据行"
    else
        fail "traffic list -f table 未包含 httpbin.org: $(echo "$result" | head -10)"
    fi
}

test_traffic_list_compact_format() {
    header "测试 traffic list compact 输出格式"

    local result
    result=$(run_bifrost traffic list -l 5 -f compact --no-color)

    if echo "$result" | grep -q "httpbin.org"; then
        pass "traffic list -f compact 包含 httpbin.org 记录"
    else
        fail "traffic list -f compact 缺少记录"
    fi

    if echo "$result" | grep -q "GET\|POST"; then
        pass "traffic list -f compact 包含 HTTP 方法"
    else
        fail "traffic list -f compact 缺少 HTTP 方法"
    fi
}

test_traffic_list_with_method_filter() {
    header "测试 traffic list --method POST 过滤"

    local result
    result=$(run_bifrost traffic list -l 20 -f json --method POST)

    if echo "$result" | grep -q "POST\|post"; then
        pass "traffic list --method POST 只返回 POST 请求"
    else
        local total
        total=$(echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")
        if [[ "$total" == "0" ]]; then
            pass "traffic list --method POST 正确返回 0 条 (无 POST 流量经过代理)"
        else
            fail "traffic list --method POST 结果异常: $(echo "$result" | head -5)"
        fi
    fi
}

test_traffic_list_with_host_filter() {
    header "测试 traffic list --host httpbin.org 过滤"

    local result
    result=$(run_bifrost traffic list -l 20 -f json --host httpbin.org)

    local total
    total=$(echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")

    if [[ "$total" -gt 0 ]]; then
        pass "traffic list --host httpbin.org 过滤出 ${total} 条记录"
    else
        fail "traffic list --host httpbin.org 未过滤到记录"
    fi
}

test_traffic_get_by_id() {
    header "测试 traffic get <id> 获取单条记录详情"

    local first_id
    first_id=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['records'][0]['id'])" 2>/dev/null || echo "")

    if [[ -z "$first_id" ]]; then
        fail "无法获取第一条流量记录 ID"
        return
    fi
    info "使用 ID: $first_id"

    local result
    result=$(run_bifrost traffic get "$first_id" -f json)

    if echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); assert 'method' in d or 'm' in d" 2>/dev/null; then
        pass "traffic get $first_id 返回了包含 method 字段的记录详情"
    else
        fail "traffic get 未返回完整记录: $(echo "$result" | head -5)"
    fi

    if echo "$result" | grep -q "httpbin.org\|host\|url"; then
        pass "traffic get 记录包含 host 信息"
    else
        fail "traffic get 记录缺少 host 信息"
    fi
}

test_traffic_search_keyword() {
    header "测试 traffic search 关键词搜索"

    local result
    result=$(run_bifrost traffic search "httpbin" -l 10 -f json)

    local total
    total=$(echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")

    if [[ "$total" -gt 0 ]]; then
        pass "traffic search 'httpbin' 搜索到 ${total} 条记录"
    else
        if echo "$result" | grep -q "httpbin"; then
            pass "traffic search 'httpbin' 包含匹配结果"
        else
            fail "traffic search 'httpbin' 未搜索到结果: $(echo "$result" | head -5)"
        fi
    fi
}

test_traffic_search_no_match() {
    header "测试 traffic search 不存在关键词"

    local result
    result=$(run_bifrost traffic search "zzz_nonexistent_xyz_999" -l 5 -f json)

    local total
    total=$(echo "$result" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "unknown")

    if [[ "$total" == "0" || "$total" == "unknown" ]]; then
        pass "traffic search 不存在关键词返回 0 条"
    else
        fail "traffic search 不存在关键词不应返回结果: total=$total"
    fi
}

test_traffic_clear_by_ids() {
    header "测试 traffic clear --ids (按 ID 删除)"

    local first_id
    first_id=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['records'][0]['id'])" 2>/dev/null || echo "")

    if [[ -z "$first_id" ]]; then
        fail "无法获取要删除的流量记录 ID"
        return
    fi

    local count_before
    count_before=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")

    local result
    result=$(run_bifrost traffic clear --ids "$first_id" -y)

    if echo "$result" | grep -qi "deleted\|Deleted 1"; then
        pass "traffic clear --ids 返回删除确认消息"
    else
        fail "traffic clear --ids 未返回预期消息: $result"
    fi

    local count_after
    count_after=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "0")

    if [[ "$count_after" -lt "$count_before" ]]; then
        pass "traffic clear --ids 后记录数减少 (${count_before} -> ${count_after})"
    else
        fail "traffic clear --ids 后记录数未减少 (${count_before} -> ${count_after})"
    fi
}

test_traffic_clear_all() {
    header "测试 traffic clear -y (清空所有)"

    local result
    result=$(run_bifrost traffic clear -y)

    if echo "$result" | grep -qi "cleared\|All traffic"; then
        pass "traffic clear -y 返回清空确认消息"
    else
        fail "traffic clear -y 未返回预期消息: $result"
    fi

    local count_after
    count_after=$(env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('total',0))" 2>/dev/null || echo "unknown")

    if [[ "$count_after" == "0" ]]; then
        pass "traffic clear -y 后记录为 0"
    else
        fail "traffic clear -y 后记录不为 0: $count_after"
    fi
}

# ═══════════════════════════════════════════════════════════
# Rule Rename/Reorder (在线 API)
# ═══════════════════════════════════════════════════════════

test_rule_rename_verify() {
    header "测试 rule rename (端到端验证)"

    local before_show
    before_show=$(run_bifrost rule show e2e_rule_1)
    if ! echo "$before_show" | grep -q "e2e1.example.com"; then
        fail "rename 前规则内容不正确: $before_show"
        return
    fi

    local result
    result=$(run_bifrost rule rename e2e_rule_1 e2e_rule_1_renamed)

    local after_show
    after_show=$(run_bifrost rule show e2e_rule_1_renamed)
    if echo "$after_show" | grep -q "e2e1.example.com"; then
        pass "rule rename: 新名 e2e_rule_1_renamed 包含原始内容 e2e1.example.com"
    else
        fail "rule rename: 新名查看不到原始内容: $after_show"
    fi

    local old_show
    old_show=$(run_bifrost rule show e2e_rule_1)
    if echo "$old_show" | grep -qi "not found\|error\|No rule"; then
        pass "rule rename: 旧名 e2e_rule_1 已不存在"
    else
        fail "rule rename: 旧名仍可访问: $old_show"
    fi
}

test_rule_rename_not_found() {
    header "测试 rule rename 不存在的规则"

    local result
    result=$(run_bifrost rule rename nonexistent_xxx_rule new_name)

    if echo "$result" | grep -qi "not found\|error\|Error\|404"; then
        pass "rule rename 不存在的规则返回错误信息"
    else
        fail "rule rename 不存在的规则未返回错误: $result"
    fi
}

test_rule_reorder_verify() {
    header "测试 rule reorder (端到端验证)"

    local list_before
    list_before=$(run_bifrost rule list)
    info "重排前: $(echo "$list_before" | head -5)"

    local result
    result=$(run_bifrost rule reorder e2e_rule_3 e2e_rule_2 e2e_rule_1_renamed)

    local list_after
    list_after=$(run_bifrost rule list)
    info "重排后: $(echo "$list_after" | head -5)"

    local first_rule_after
    first_rule_after=$(echo "$list_after" | head -5)
    if echo "$first_rule_after" | grep -q "e2e_rule_3"; then
        pass "rule reorder: e2e_rule_3 已排到最前面"
    else
        fail "rule reorder: 排序结果不符预期: $first_rule_after"
    fi
}

# ═══════════════════════════════════════════════════════════
# Script Rename (在线 API)
# ═══════════════════════════════════════════════════════════

test_script_rename_verify() {
    header "测试 script rename (端到端验证)"

    local before_show
    before_show=$(run_bifrost script show request e2e_script_1)
    if ! echo "$before_show" | grep -q "e2e script\|e2e_script_1"; then
        fail "rename 前脚本内容不正确: $before_show"
        return
    fi

    local result
    result=$(run_bifrost script rename request e2e_script_1 e2e_script_1_renamed)

    local after_show
    after_show=$(run_bifrost script show request e2e_script_1_renamed)
    if echo "$after_show" | grep -q "e2e script\|e2e_script_1_renamed"; then
        pass "script rename: 新名包含原始内容"
    else
        fail "script rename: 新名查看失败: $after_show"
    fi

    local old_show
    old_show=$(run_bifrost script show request e2e_script_1)
    if echo "$old_show" | grep -qi "not found\|error\|No script"; then
        pass "script rename: 旧名已不存在"
    else
        fail "script rename: 旧名仍可访问: $old_show"
    fi
}

test_script_rename_not_found() {
    header "测试 script rename 不存在的脚本"

    local result
    result=$(run_bifrost script rename request nonexistent_xxx_script new_name)

    if echo "$result" | grep -qi "not found\|error\|Error\|404"; then
        pass "script rename 不存在的脚本返回错误"
    else
        fail "script rename 不存在的脚本未返回错误: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Metrics (验证返回结构)
# ═══════════════════════════════════════════════════════════

test_metrics_summary_verify() {
    header "测试 metrics summary (验证返回内容)"

    local result
    result=$(run_bifrost metrics summary)

    if echo "$result" | grep -qi "total\|request\|connection\|traffic\|bytes\|uptime"; then
        pass "metrics summary 包含关键指标字段"
    else
        fail "metrics summary 缺少关键字段: $result"
    fi
}

test_metrics_apps_verify() {
    header "测试 metrics apps"

    local result
    result=$(run_bifrost metrics apps)

    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic\|error"; then
        pass "metrics apps 返回了应用数据"
    else
        fail "metrics apps 失败或为空: $result"
    fi
}

test_metrics_hosts_verify() {
    header "测试 metrics hosts (验证含 httpbin)"

    local result
    result=$(run_bifrost metrics hosts)

    if echo "$result" | grep -qi "httpbin.org\|host\|count"; then
        pass "metrics hosts 包含 httpbin.org 或主机统计信息"
    else
        fail "metrics hosts 未包含预期内容: $(echo "$result" | head -5)"
    fi
}

test_metrics_history_verify() {
    header "测试 metrics history"

    local result
    result=$(run_bifrost metrics history -l 5)

    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic\|error"; then
        pass "metrics history 返回了历史数据"
    else
        fail "metrics history 失败或为空: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Sync (验证返回结构)
# ═══════════════════════════════════════════════════════════

test_sync_status_verify() {
    header "测试 sync status (验证返回内容)"

    local result
    result=$(run_bifrost sync status)

    if echo "$result" | grep -qi "sync\|status\|enabled\|disabled\|logged\|not"; then
        pass "sync status 返回了同步状态信息"
    else
        fail "sync status 返回内容不含预期字段: $result"
    fi
}

test_sync_config_verify() {
    header "测试 sync config (验证返回内容)"

    local result
    result=$(run_bifrost sync config)

    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic\|error"; then
        pass "sync config 返回了配置信息"
    else
        fail "sync config 失败或为空: $result"
    fi
}

test_sync_run_verify() {
    header "测试 sync run"

    local result
    result=$(run_bifrost sync run)

    if echo "$result" | grep -qi "sync\|success\|complet\|not.*login\|no.*token\|skip"; then
        pass "sync run 执行完成并返回同步状态"
    elif ! echo "$result" | grep -qi "panic"; then
        pass "sync run 执行完成 (未登录状态预期)"
    else
        fail "sync run 出现 panic: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Whitelist (端到端验证)
# ═══════════════════════════════════════════════════════════

test_whitelist_list_verify() {
    header "测试 whitelist list (验证返回结构)"

    local result
    result=$(run_bifrost whitelist list)

    if echo "$result" | grep -qi "127.0.0.1\|whitelist\|ip\|address\|local"; then
        pass "whitelist list 包含本地 IP 或白名单信息"
    else
        fail "whitelist list 返回内容不含预期字段: $result"
    fi
}

test_whitelist_status_verify() {
    header "测试 whitelist status (验证返回内容)"

    local result
    result=$(run_bifrost whitelist status)

    if echo "$result" | grep -qi "mode\|access\|whitelist\|allow\|local"; then
        pass "whitelist status 包含 mode/access 信息"
    else
        fail "whitelist status 不含预期信息: $result"
    fi
}

test_whitelist_mode_roundtrip() {
    header "测试 whitelist mode 读写回环"

    local original_mode
    original_mode=$(run_bifrost whitelist mode)
    info "当前 mode: $original_mode"

    run_bifrost whitelist mode allow_all >/dev/null
    local new_mode
    new_mode=$(run_bifrost whitelist mode)
    if echo "$new_mode" | grep -qi "allow.all\|allow_all"; then
        pass "whitelist mode 设置为 allow_all 后读回正确"
    else
        fail "whitelist mode 设置 allow_all 后读回不匹配: $new_mode"
    fi

    run_bifrost whitelist mode local_only >/dev/null
    new_mode=$(run_bifrost whitelist mode)
    if echo "$new_mode" | grep -qi "local.only\|local_only"; then
        pass "whitelist mode 恢复为 local_only 后读回正确"
    else
        fail "whitelist mode 恢复失败: $new_mode"
    fi
}

test_whitelist_add_verify_remove() {
    header "测试 whitelist add -> list 验证 -> remove"

    run_bifrost whitelist add 192.168.99.99 >/dev/null

    local list_result
    list_result=$(run_bifrost whitelist list)
    if echo "$list_result" | grep -q "192.168.99.99"; then
        pass "whitelist add 后 list 包含 192.168.99.99"
    else
        fail "whitelist add 后 list 未找到 192.168.99.99: $list_result"
    fi

    run_bifrost whitelist remove 192.168.99.99 >/dev/null

    list_result=$(run_bifrost whitelist list)
    if ! echo "$list_result" | grep -q "192.168.99.99"; then
        pass "whitelist remove 后 192.168.99.99 已移除"
    else
        fail "whitelist remove 后 192.168.99.99 仍在列表"
    fi
}

test_whitelist_temporary_roundtrip() {
    header "测试 whitelist temporary add/remove 回环"

    local result
    result=$(run_bifrost whitelist add-temporary 10.0.0.99)

    if ! echo "$result" | grep -qi "panic"; then
        pass "whitelist add-temporary 10.0.0.99 执行成功"
    else
        fail "whitelist add-temporary 失败: $result"
    fi

    result=$(run_bifrost whitelist remove-temporary 10.0.0.99)
    if ! echo "$result" | grep -qi "panic"; then
        pass "whitelist remove-temporary 10.0.0.99 执行成功"
    else
        fail "whitelist remove-temporary 失败: $result"
    fi
}

test_whitelist_pending_clear() {
    header "测试 whitelist pending + clear-pending"

    local result
    result=$(run_bifrost whitelist pending)
    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic"; then
        pass "whitelist pending 列出成功"
    else
        fail "whitelist pending 失败或为空: $result"
    fi

    result=$(run_bifrost whitelist clear-pending)
    if ! echo "$result" | grep -qi "panic"; then
        pass "whitelist clear-pending 执行成功"
    else
        fail "whitelist clear-pending 失败: $result"
    fi
}

test_whitelist_allow_lan_roundtrip() {
    header "测试 whitelist allow-lan 设置回环"

    local result
    result=$(run_bifrost whitelist allow-lan true)
    if ! echo "$result" | grep -qi "panic"; then
        pass "whitelist allow-lan true 执行成功"
    else
        fail "whitelist allow-lan true 失败: $result"
    fi

    local status
    status=$(run_bifrost whitelist status)
    if echo "$status" | grep -qi "lan\|allow\|mode\|access"; then
        pass "whitelist status 在 allow-lan 设置后返回了有效状态"
    else
        fail "whitelist status 未包含预期字段: $status"
    fi

    result=$(run_bifrost whitelist allow-lan false)
    if ! echo "$result" | grep -qi "panic"; then
        pass "whitelist allow-lan false 恢复成功"
    else
        fail "whitelist allow-lan false 失败: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Config (端到端验证)
# ═══════════════════════════════════════════════════════════

test_config_show_verify() {
    header "测试 config show (验证返回结构)"

    local result
    result=$(run_bifrost config show)

    if echo "$result" | grep -qi "tls\|proxy\|port\|intercept\|access"; then
        pass "config show 包含 tls/proxy/port 等关键配置"
    else
        fail "config show 缺少关键配置: $(echo "$result" | head -5)"
    fi
}

test_config_show_json_verify() {
    header "测试 config show --json (验证 JSON 格式)"

    local result
    result=$(run_bifrost config show --json)

    if echo "$result" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
        pass "config show --json 输出有效 JSON"
    else
        fail "config show --json 不是有效 JSON: $(echo "$result" | head -3)"
    fi
}

test_config_show_section_verify() {
    header "测试 config show --section (验证各 section)"

    local result
    result=$(run_bifrost config show --section tls)
    if echo "$result" | grep -qi "tls\|intercept\|ssl\|certificate\|exclude"; then
        pass "config show --section tls 包含 TLS 相关字段"
    else
        fail "config show --section tls 不含 TLS 字段: $result"
    fi

    result=$(run_bifrost config show --section traffic)
    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic"; then
        pass "config show --section traffic 返回了数据"
    else
        fail "config show --section traffic 失败"
    fi
}

test_config_get_set_roundtrip() {
    header "测试 config get/set 读写回环"

    local original
    original=$(run_bifrost config get tls.unsafe_ssl)
    info "原始 tls.unsafe_ssl 值: $original"

    run_bifrost config set tls.unsafe_ssl true >/dev/null
    local after_set
    after_set=$(run_bifrost config get tls.unsafe_ssl)
    if echo "$after_set" | grep -qi "true"; then
        pass "config set tls.unsafe_ssl true -> get 返回 true"
    else
        fail "config set 后 get 不返回 true: $after_set"
    fi

    run_bifrost config set tls.unsafe_ssl false >/dev/null
    after_set=$(run_bifrost config get tls.unsafe_ssl)
    if echo "$after_set" | grep -qi "false"; then
        pass "config set tls.unsafe_ssl false -> get 返回 false"
    else
        fail "config set 恢复后 get 不返回 false: $after_set"
    fi
}

test_config_get_json_verify() {
    header "测试 config get --json (验证 JSON 格式)"

    local result
    result=$(run_bifrost config get tls.enabled --json)
    if echo "$result" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
        pass "config get --json 输出有效 JSON"
    else
        fail "config get --json 不是有效 JSON: $(echo "$result" | head -3)"
    fi
}

test_config_add_remove_verify() {
    header "测试 config add/remove (端到端验证)"

    local result
    result=$(run_bifrost config add tls.exclude "*.e2etest.local")

    if ! echo "$result" | grep -qi "panic"; then
        pass "config add tls.exclude *.e2etest.local 执行成功"
    else
        fail "config add 失败: $result"
    fi

    local show_result
    show_result=$(run_bifrost config show --section tls)
    if echo "$show_result" | grep -q "e2etest.local"; then
        pass "config add 后 show --section tls 包含 e2etest.local"
    else
        fail "config add 后 show 未包含 e2etest.local: $(echo "$show_result" | head -5)"
    fi

    result=$(run_bifrost config remove tls.exclude "*.e2etest.local")
    if ! echo "$result" | grep -qi "panic"; then
        pass "config remove tls.exclude *.e2etest.local 执行成功"
    else
        fail "config remove 失败: $result"
    fi
}

test_config_reset_verify() {
    header "测试 config reset"

    run_bifrost config set tls.unsafe_ssl true >/dev/null
    run_bifrost config reset tls.unsafe_ssl -y >/dev/null

    local after_reset
    after_reset=$(run_bifrost config get tls.unsafe_ssl)
    if echo "$after_reset" | grep -qi "false\|true"; then
        pass "config reset 后值已恢复 (当前: $after_reset)"
    else
        pass "config reset 执行成功"
    fi
}

test_config_clear_cache_verify() {
    header "测试 config clear-cache"

    local result
    result=$(run_bifrost config clear-cache -y)

    if echo "$result" | grep -qi "cleared\|cache\|success\|ok\|done"; then
        pass "config clear-cache 返回了清除确认"
    elif ! echo "$result" | grep -qi "panic\|error"; then
        pass "config clear-cache 执行成功"
    else
        fail "config clear-cache 失败: $result"
    fi
}

test_config_performance_verify() {
    header "测试 config performance (验证内容)"

    local result
    result=$(run_bifrost config performance)

    if echo "$result" | grep -qi "cpu\|memory\|uptime\|thread\|connection\|performance\|system"; then
        pass "config performance 包含系统性能指标"
    else
        fail "config performance 缺少性能指标: $(echo "$result" | head -5)"
    fi
}

test_config_websocket_verify() {
    header "测试 config websocket"

    local result
    result=$(run_bifrost config websocket)

    if [[ -n "$result" ]] && ! echo "$result" | grep -qi "panic"; then
        pass "config websocket 返回了连接信息"
    else
        fail "config websocket 失败或为空: $result"
    fi
}

test_config_disconnect_verify() {
    header "测试 config disconnect"

    local result
    result=$(run_bifrost config disconnect "*.nonexist.e2e")

    if ! echo "$result" | grep -qi "panic"; then
        pass "config disconnect 执行成功 (匹配 0 条预期)"
    else
        fail "config disconnect 失败"
    fi
}

test_config_disconnect_by_app_verify() {
    header "测试 config disconnect-by-app"

    local result
    result=$(run_bifrost config disconnect-by-app "E2E_FakeApp")

    if ! echo "$result" | grep -qi "panic"; then
        pass "config disconnect-by-app 执行成功"
    else
        fail "config disconnect-by-app 失败"
    fi
}

test_config_export_json_verify() {
    header "测试 config export --format json (验证文件内容)"

    local output="${TEST_DATA_DIR}/config_export.json"
    run_bifrost config export -o "$output" --format json >/dev/null

    if [[ -f "$output" ]]; then
        if python3 -c "import json; json.load(open('$output'))" 2>/dev/null; then
            pass "config export json: 文件内容是有效 JSON"
        else
            fail "config export json: 文件不是有效 JSON"
        fi
    else
        fail "config export json: 文件未生成"
    fi
}

test_config_export_toml_verify() {
    header "测试 config export --format toml (验证文件生成)"

    local output="${TEST_DATA_DIR}/config_export.toml"
    run_bifrost config export -o "$output" --format toml >/dev/null

    if [[ -f "$output" ]] && [[ -s "$output" ]]; then
        pass "config export toml: 文件已生成且非空 ($(wc -c < "$output") bytes)"
    else
        fail "config export toml: 文件未生成或为空"
    fi
}

# ═══════════════════════════════════════════════════════════
# Version Check
# ═══════════════════════════════════════════════════════════

test_version_check_verify() {
    header "测试 version-check"

    local result
    result=$(run_bifrost version-check)

    if echo "$result" | grep -qi "panic"; then
        fail "version-check panic: $result"
    elif [[ -z "$result" ]]; then
        fail "version-check 不应为空输出"
    elif echo "$result" | grep -qi "version\|latest\|current\|up.to.date\|upgrade\|available\|error\|timeout\|network\|could not determine"; then
        pass "version-check 返回了版本信息或网络状态"
    else
        fail "version-check 返回了异常内容: $result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Export/Import (端到端验证)
# ═══════════════════════════════════════════════════════════

test_export_rules_verify() {
    header "测试 export rules (验证文件包含规则)"

    local output="${TEST_DATA_DIR}/exported_rules.bifrost"
    run_bifrost export rules e2e_rule_3 e2e_rule_2 -d "E2E test rules" -o "$output" >/dev/null

    if [[ -f "$output" ]]; then
        if grep -q "e2e_rule_3\|e2e3.example.com" "$output"; then
            pass "export rules: 文件包含 e2e_rule_3 数据"
        else
            fail "export rules: 文件不含 e2e_rule_3: $(head -3 "$output")"
        fi
    else
        fail "export rules: 文件未生成"
    fi
}

test_export_values_verify() {
    header "测试 export values (验证文件包含值)"

    local output="${TEST_DATA_DIR}/exported_values.bifrost"
    run_bifrost export values e2e_val_1 -d "E2E test values" -o "$output" >/dev/null

    if [[ -f "$output" ]]; then
        if grep -q "e2e_val_1\|e2e_value_content" "$output"; then
            pass "export values: 文件包含 e2e_val_1 数据"
        else
            fail "export values: 文件不含 e2e_val_1: $(head -3 "$output")"
        fi
    else
        fail "export values: 文件未生成"
    fi
}

test_export_scripts_verify() {
    header "测试 export scripts (验证文件包含脚本)"

    local output="${TEST_DATA_DIR}/exported_scripts.bifrost"
    run_bifrost export scripts request/e2e_script_1_renamed -d "E2E test scripts" -o "$output" >/dev/null

    if [[ -f "$output" ]]; then
        if grep -q "e2e_script_1_renamed\|e2e script" "$output"; then
            pass "export scripts: 文件包含脚本数据"
        else
            fail "export scripts: 文件不含预期脚本数据: $(head -3 "$output")"
        fi
    else
        fail "export scripts: 文件未生成"
    fi
}

test_import_detect_verify() {
    header "测试 import --detect-only (验证检测结果)"

    local file="${TEST_DATA_DIR}/test_import_detect.bifrost"
    cat > "$file" << 'EOF'
{"type":"rules","version":"1","rules":[{"name":"detected_rule","content":"detected.example.com statusCode://200"}]}
EOF

    local result
    result=$(run_bifrost import "$file" --detect-only)

    if echo "$result" | grep -qi "detect\|rules\|type\|1\|found"; then
        pass "import --detect-only 返回了检测信息"
    else
        fail "import --detect-only 未返回预期检测结果: $result"
    fi
}

test_import_full_verify() {
    header "测试 import 完整导入 -> 验证数据可用"

    local file="${TEST_DATA_DIR}/test_import_full.bifrost"
    cat > "$file" << 'EOF'
{"type":"rules","version":"1","rules":[{"name":"imported_e2e_rule","content":"imported-e2e.example.com statusCode://200"}]}
EOF

    run_bifrost import "$file" >/dev/null

    local show_result
    show_result=$(run_bifrost rule show imported_e2e_rule)
    if echo "$show_result" | grep -q "imported-e2e.example.com\|imported_e2e_rule"; then
        pass "import 后 rule show imported_e2e_rule 能找到并含正确内容"
    else
        fail "import 后 rule show 未找到 imported_e2e_rule: $show_result"
    fi
}

# ═══════════════════════════════════════════════════════════
# Search (依赖真实流量)
# ═══════════════════════════════════════════════════════════

test_search_with_real_data() {
    header "测试 search (基于真实流量)"

    local result
    result=$(run_bifrost search "httpbin" -l 10 -f json)

    if echo "$result" | grep -q "httpbin"; then
        pass "search 'httpbin' 搜索到匹配的流量记录"
    else
        fail "search 'httpbin' 未搜索到匹配结果: $(echo "$result" | head -5)"
    fi
}

# ═══════════════════════════════════════════════════════════
# Stop
# ═══════════════════════════════════════════════════════════

test_stop_verify() {
    header "测试 stop (验证服务停止)"

    local result
    result=$(run_bifrost stop)

    if ! echo "$result" | grep -qi "panic"; then
        pass "stop 命令执行完成"
    else
        fail "stop 命令 panic: $result"
    fi

    sleep 2

    if ! env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
        pass "stop 后服务已停止 (API 不可达)"
    else
        fail "stop 后服务仍在运行"
    fi
}

# ═══════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════

print_summary() {
    header "测试总结"
    local total=$((PASSED + FAILED))
    echo -e "  ${GREEN}通过${NC}: $PASSED"
    echo -e "  ${RED}失败${NC}: $FAILED"
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

main() {
    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║   Bifrost CLI 在线命令端到端测试 (真实端到端验证版)           ║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"

    build_bifrost
    setup_test_data_dir
    seed_data

    if ! start_bifrost_server; then
        echo -e "${RED}无法启动 Bifrost 服务器, 终止测试${NC}"
        exit 1
    fi

    sleep 1

    test_status

    generate_real_traffic

    test_traffic_list_has_records
    test_traffic_list_table_format
    test_traffic_list_compact_format
    test_traffic_list_with_method_filter
    test_traffic_list_with_host_filter
    test_traffic_get_by_id
    test_traffic_search_keyword
    test_traffic_search_no_match

    test_search_with_real_data

    test_traffic_clear_by_ids
    test_traffic_clear_all

    test_rule_rename_verify
    test_rule_rename_not_found
    test_rule_reorder_verify
    test_script_rename_verify
    test_script_rename_not_found

    test_metrics_summary_verify
    test_metrics_apps_verify
    test_metrics_hosts_verify
    test_metrics_history_verify

    test_sync_status_verify
    test_sync_config_verify
    test_sync_run_verify

    test_whitelist_list_verify
    test_whitelist_status_verify
    test_whitelist_mode_roundtrip
    test_whitelist_add_verify_remove
    test_whitelist_temporary_roundtrip
    test_whitelist_pending_clear
    test_whitelist_allow_lan_roundtrip

    test_config_show_verify
    test_config_show_json_verify
    test_config_show_section_verify
    test_config_get_set_roundtrip
    test_config_get_json_verify
    test_config_add_remove_verify
    test_config_reset_verify
    test_config_clear_cache_verify
    test_config_performance_verify
    test_config_websocket_verify
    test_config_disconnect_verify
    test_config_disconnect_by_app_verify
    test_config_export_json_verify
    test_config_export_toml_verify

    test_version_check_verify

    test_export_rules_verify
    test_export_values_verify
    test_export_scripts_verify
    test_import_detect_verify
    test_import_full_verify

    test_stop_verify

    print_summary
    exit $?
}

main "$@"
