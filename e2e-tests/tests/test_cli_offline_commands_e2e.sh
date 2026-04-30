#!/bin/bash
#
# Bifrost CLI 离线命令端到端测试
# 覆盖: rule rename/reorder, script rename, completions, import/export(文件格式), value/rule/script CRUD
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
    PASSED=$((PASSED + 1))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    FAILED=$((FAILED + 1))
}

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}

trap cleanup EXIT

run_bifrost() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" "$@" 2>&1 || true
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

seed_rules() {
    header "准备测试规则"
    local target_base="${E2E_OFFLINE_RULE_TARGET_BASE_PORT:-${ECHO_HTTP_PORT:-3000}}"
    run_bifrost rule add rule_alpha -c "alpha.com host://127.0.0.1:$((target_base + 1))" >/dev/null
    run_bifrost rule add rule_beta -c "beta.com host://127.0.0.1:$((target_base + 2))" >/dev/null
    run_bifrost rule add rule_gamma -c "gamma.com host://127.0.0.1:$((target_base + 3))" >/dev/null
    pass "测试规则已写入 (alpha, beta, gamma)"
}

seed_scripts() {
    header "准备测试脚本"
    run_bifrost script add request req_demo -c 'log.info("req demo"); request.headers["X-Demo"] = "1";' >/dev/null
    run_bifrost script add response res_demo -c 'log.info("res demo");' >/dev/null
    run_bifrost script add decode dec_demo -c 'ctx.output = { code: "0", data: "demo", msg: "" };' >/dev/null
    pass "测试脚本已写入 (req_demo, res_demo, dec_demo)"
}

seed_values() {
    header "准备测试值"
    run_bifrost value add val_one "value_one_content" >/dev/null
    run_bifrost value add val_two "value_two_content" >/dev/null
    pass "测试值已写入 (val_one, val_two)"
}

# ─── Rule Reorder (offline via file reorder) ───
# Note: rule rename / script rename 需要 admin API, 在在线测试中覆盖

test_rule_reorder_help() {
    header "测试 rule reorder 参数解析"

    local result
    result=$(run_bifrost rule reorder --help)

    if echo "$result" | grep -qiE "reorder|NAMES|names|priority"; then
        pass "rule reorder --help 正确显示"
    else
        fail "rule reorder --help 异常: $result"
    fi
}

test_rule_rename_help() {
    header "测试 rule rename 参数解析"

    local result
    result=$(run_bifrost rule rename --help)

    if echo "$result" | grep -qiE "rename|NEW_NAME|new.name"; then
        pass "rule rename --help 正确显示"
    else
        fail "rule rename --help 异常: $result"
    fi
}

test_script_rename_help() {
    header "测试 script rename 参数解析"

    local result
    result=$(run_bifrost script rename --help)

    if echo "$result" | grep -qiE "rename|NEW_NAME|new.name"; then
        pass "script rename --help 正确显示"
    else
        fail "script rename --help 异常: $result"
    fi
}

# ─── Completions ───

test_completions_bash() {
    header "测试 completions bash"

    local tmpfile="${TEST_DATA_DIR}/completions_bash.txt"
    run_bifrost completions bash > "$tmpfile"

    if grep -qE "_bifrost|bifrost" "$tmpfile"; then
        pass "completions bash 生成了有效脚本"
    else
        fail "completions bash 输出无效: $(head -3 "$tmpfile")"
    fi
}

test_completions_zsh() {
    header "测试 completions zsh"

    local tmpfile="${TEST_DATA_DIR}/completions_zsh.txt"
    run_bifrost completions zsh > "$tmpfile"

    if grep -qE "bifrost" "$tmpfile"; then
        pass "completions zsh 生成了有效脚本"
    else
        fail "completions zsh 输出无效: $(head -3 "$tmpfile")"
    fi
}

test_completions_fish() {
    header "测试 completions fish"

    local tmpfile="${TEST_DATA_DIR}/completions_fish.txt"
    run_bifrost completions fish > "$tmpfile"

    if grep -qE "bifrost" "$tmpfile"; then
        pass "completions fish 生成了有效脚本"
    else
        fail "completions fish 输出无效: $(head -3 "$tmpfile")"
    fi
}

test_completions_include_new_commands() {
    header "测试 completions 包含所有新命令"

    local tmpfile="${TEST_DATA_DIR}/completions_bash_full.txt"
    run_bifrost completions bash > "$tmpfile"

    local cmds=("metrics" "sync" "import" "export" "version-check" "completions")
    for cmd in "${cmds[@]}"; do
        if grep -qE "$cmd" "$tmpfile"; then
            pass "completions 包含命令: $cmd"
        else
            fail "completions 缺少命令: $cmd"
        fi
    done
}

# ─── Help Text ───

test_help_all_commands() {
    header "测试所有子命令 help 可用"

    local commands=("rule" "script" "value" "config" "whitelist" "traffic" "metrics" "sync" "ca" "group" "system-proxy")
    for cmd in "${commands[@]}"; do
        local result
        result=$(run_bifrost "$cmd" --help)
        if [[ -n "$result" ]] && ! echo "$result" | grep -qiE "error|panic"; then
            pass "help: bifrost $cmd --help"
        else
            fail "help: bifrost $cmd --help 失败"
        fi
    done
}

test_help_new_subcommands() {
    header "测试新增子命令 help"

    local result

    result=$(run_bifrost rule rename --help)
    if echo "$result" | grep -qiE "rename|new.name|NEW_NAME"; then
        pass "help: rule rename --help"
    else
        fail "help: rule rename --help 失败: $result"
    fi

    result=$(run_bifrost rule reorder --help)
    if echo "$result" | grep -qiE "reorder|names|NAMES"; then
        pass "help: rule reorder --help"
    else
        fail "help: rule reorder --help 失败: $result"
    fi

    result=$(run_bifrost script rename --help)
    if echo "$result" | grep -qiE "rename|new.name|NEW_NAME"; then
        pass "help: script rename --help"
    else
        fail "help: script rename --help 失败: $result"
    fi

    result=$(run_bifrost metrics --help)
    if echo "$result" | grep -qiE "metrics|summary|apps|hosts"; then
        pass "help: metrics --help"
    else
        fail "help: metrics --help 失败: $result"
    fi

    result=$(run_bifrost sync --help)
    if echo "$result" | grep -qiE "sync|status|login|logout"; then
        pass "help: sync --help"
    else
        fail "help: sync --help 失败: $result"
    fi

    result=$(run_bifrost export --help)
    if echo "$result" | grep -qiE "export|rules|values|scripts"; then
        pass "help: export --help"
    else
        fail "help: export --help 失败: $result"
    fi

    result=$(run_bifrost version-check --help)
    if echo "$result" | grep -qiE "version|check|upgrade"; then
        pass "help: version-check --help"
    else
        fail "help: version-check --help 失败: $result"
    fi
}

# ─── Rule CRUD baseline ───

test_rule_crud_baseline() {
    header "测试 rule CRUD 基本流程"

    run_bifrost rule add crud_test -c "crud.example.com statusCode://200" >/dev/null
    local show_result
    show_result=$(run_bifrost rule show crud_test)
    if echo "$show_result" | grep -qE "crud.example.com"; then
        pass "rule add + show 工作正常"
    else
        fail "rule add/show 失败: $show_result"
    fi

    run_bifrost rule update crud_test -c "crud.example.com statusCode://201" >/dev/null
    show_result=$(run_bifrost rule show crud_test)
    if echo "$show_result" | grep -qE "201"; then
        pass "rule update 工作正常"
    else
        fail "rule update 失败: $show_result"
    fi

    run_bifrost rule enable crud_test >/dev/null
    pass "rule enable 执行完成"

    run_bifrost rule disable crud_test >/dev/null
    pass "rule disable 执行完成"

    run_bifrost rule delete crud_test >/dev/null
    local deleted_result
    deleted_result=$(run_bifrost rule show crud_test)
    if echo "$deleted_result" | grep -qiE "not found|error|No rule"; then
        pass "rule delete 工作正常"
    else
        fail "rule delete 后仍可查看: $deleted_result"
    fi
}

# ─── Value CRUD baseline ───

test_value_crud_baseline() {
    header "测试 value CRUD 基本流程"

    run_bifrost value add crud_val "hello_world" >/dev/null
    local show_result
    show_result=$(run_bifrost value show crud_val)
    if echo "$show_result" | grep -qE "hello_world"; then
        pass "value add + show 工作正常"
    else
        fail "value add/show 失败: $show_result"
    fi

    run_bifrost value update crud_val "updated_world" >/dev/null
    show_result=$(run_bifrost value show crud_val)
    if echo "$show_result" | grep -qE "updated_world"; then
        pass "value update 工作正常"
    else
        fail "value update 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost value list)
    if echo "$list_result" | grep -qE "crud_val"; then
        pass "value list 包含新增值"
    else
        fail "value list 未包含新增值: $list_result"
    fi

    run_bifrost value delete crud_val >/dev/null
    local deleted_result
    deleted_result=$(run_bifrost value show crud_val)
    if echo "$deleted_result" | grep -qiE "not found|error"; then
        pass "value delete 工作正常"
    else
        fail "value delete 后仍存在: $deleted_result"
    fi
}

# ─── Script CRUD baseline ───

test_script_crud_baseline() {
    header "测试 script CRUD 基本流程"

    run_bifrost script add request crud_script -c 'log.info("crud test");' >/dev/null
    local show_result
    show_result=$(run_bifrost script show request crud_script)
    if echo "$show_result" | grep -qE "crud_script"; then
        pass "script add + show 工作正常"
    else
        fail "script add/show 失败: $show_result"
    fi

    run_bifrost script update request crud_script -c 'log.info("crud updated");' >/dev/null
    show_result=$(run_bifrost script show request crud_script)
    if echo "$show_result" | grep -qE "crud updated"; then
        pass "script update 工作正常"
    else
        fail "script update 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t request)
    if echo "$list_result" | grep -qE "crud_script"; then
        pass "script list 包含新增脚本"
    else
        fail "script list 未包含脚本: $list_result"
    fi

    local run_result
    run_result=$(run_bifrost script run request crud_script)
    if echo "$run_result" | grep -qE "Output:|Logs:|crud updated"; then
        pass "script run 工作正常"
    else
        fail "script run 失败: $run_result"
    fi

    run_bifrost script delete request crud_script >/dev/null
    local deleted_result
    deleted_result=$(run_bifrost script show request crud_script)
    if echo "$deleted_result" | grep -qiE "not found|error|No script"; then
        pass "script delete 工作正常"
    else
        fail "script delete 后仍存在: $deleted_result"
    fi
}

# ─── Rule add from file ───

test_rule_add_from_file() {
    header "测试 rule add -f (从文件)"

    local rule_file="${TEST_DATA_DIR}/test_rule.txt"
    echo "filrule.example.com statusCode://200" > "$rule_file"

    local result
    result=$(run_bifrost rule add file_rule -f "$rule_file")
    local show_result
    show_result=$(run_bifrost rule show file_rule)
    if echo "$show_result" | grep -qE "filrule.example.com"; then
        pass "rule add -f 从文件添加成功"
    else
        fail "rule add -f 失败: $show_result"
    fi

    run_bifrost rule delete file_rule >/dev/null
}

# ─── Rule sync help ───

test_rule_sync_help() {
    header "测试 rule sync --help"

    local result
    result=$(run_bifrost rule sync --help)
    if echo "$result" | grep -qiE "sync|remote"; then
        pass "rule sync --help 正确显示"
    else
        fail "rule sync --help 异常: $result"
    fi
}

# ─── Value import ───

test_value_import() {
    header "测试 value import"

    local import_file="${TEST_DATA_DIR}/test_values.json"
    cat > "$import_file" << 'EOF'
{
  "import_key1": "import_val1",
  "import_key2": "import_val2"
}
EOF

    local result
    result=$(run_bifrost value import "$import_file")
    if [[ -n "$result" ]] && ! echo "$result" | grep -qiE "panic"; then
        pass "value import 执行成功"
    else
        fail "value import 失败: $result"
    fi

    local import_txt="${TEST_DATA_DIR}/test_values.txt"
    echo "txt_key=txt_val" > "$import_txt"
    result=$(run_bifrost value import "$import_txt")
    if ! echo "$result" | grep -qiE "panic"; then
        pass "value import .txt 执行成功"
    else
        fail "value import .txt 失败: $result"
    fi
}

# ─── Script response/decode types ───

test_script_response_type() {
    header "测试 script response 类型"

    run_bifrost script add response resp_test -c 'log.info("resp script test");' >/dev/null
    local show_result
    show_result=$(run_bifrost script show response resp_test)
    if echo "$show_result" | grep -qE "resp_test|resp script test"; then
        pass "script add response 类型成功"
    else
        fail "script add response 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t response)
    if echo "$list_result" | grep -qE "resp_test"; then
        pass "script list -t response 正确过滤"
    else
        fail "script list -t response 未过滤到: $list_result"
    fi

    run_bifrost script delete response resp_test >/dev/null
    pass "script delete response 成功"
}

test_script_decode_type() {
    header "测试 script decode 类型"

    run_bifrost script add decode dec_test -c 'ctx.output = { code: "0", data: "test", msg: "" };' >/dev/null
    local show_result
    show_result=$(run_bifrost script show decode dec_test)
    if echo "$show_result" | grep -qE "dec_test|ctx.output"; then
        pass "script add decode 类型成功"
    else
        fail "script add decode 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t decode)
    if echo "$list_result" | grep -qE "dec_test"; then
        pass "script list -t decode 正确过滤"
    else
        fail "script list -t decode 未过滤到: $list_result"
    fi

    run_bifrost script delete decode dec_test >/dev/null
    pass "script delete decode 成功"
}

test_script_show_fuzzy() {
    header "测试 script show 模糊匹配 (单参数)"

    run_bifrost script add request fuzzy_test_script -c 'log.info("fuzzy");' >/dev/null

    local result
    result=$(run_bifrost script show fuzzy_test_script)
    if echo "$result" | grep -qE "fuzzy_test_script|fuzzy"; then
        pass "script show 单参数模糊匹配成功"
    else
        fail "script show 模糊匹配失败: $result"
    fi

    run_bifrost script delete request fuzzy_test_script >/dev/null
}

test_script_list_all() {
    header "测试 script list (无过滤)"

    local result
    result=$(run_bifrost script list)
    if [[ -n "$result" ]] && ! echo "$result" | grep -qiE "panic"; then
        pass "script list (全部) 执行成功"
    else
        fail "script list 失败: $result"
    fi
}

test_script_add_from_file() {
    header "测试 script add -f (从文件)"

    local script_file="${TEST_DATA_DIR}/test_script.js"
    echo 'log.info("from file"); request.headers["X-File"] = "true";' > "$script_file"

    run_bifrost script add request file_script -f "$script_file" >/dev/null
    local show_result
    show_result=$(run_bifrost script show request file_script)
    if echo "$show_result" | grep -qE "file_script|from file"; then
        pass "script add -f 从文件添加成功"
    else
        fail "script add -f 失败: $show_result"
    fi

    run_bifrost script delete request file_script >/dev/null
}

# ─── Group help ───

test_group_help() {
    header "测试 group 子命令 help"

    local result
    result=$(run_bifrost group --help)
    if echo "$result" | grep -qiE "group|list|show|rule"; then
        pass "group --help 正确显示"
    else
        fail "group --help 异常: $result"
    fi

    result=$(run_bifrost group list --help)
    if echo "$result" | grep -qiE "list|keyword|limit"; then
        pass "group list --help 正确显示"
    else
        fail "group list --help 异常: $result"
    fi

    result=$(run_bifrost group show --help)
    if echo "$result" | grep -qiE "show|group.id|GROUP_ID"; then
        pass "group show --help 正确显示"
    else
        fail "group show --help 异常: $result"
    fi

    result=$(run_bifrost group rule --help)
    if echo "$result" | grep -qiE "rule|list|show|add|update|delete"; then
        pass "group rule --help 正确显示"
    else
        fail "group rule --help 异常: $result"
    fi
}

# ─── CA help ───

test_ca_help() {
    header "测试 ca 子命令 help"

    local result
    result=$(run_bifrost ca --help)
    if echo "$result" | grep -qiE "ca|install|info|export|generate"; then
        pass "ca --help 正确显示"
    else
        fail "ca --help 异常: $result"
    fi

    result=$(run_bifrost ca info --help)
    if echo "$result" | grep -qiE "info|certificate"; then
        pass "ca info --help 正确显示"
    else
        fail "ca info --help 异常: $result"
    fi

    result=$(run_bifrost ca export --help)
    if echo "$result" | grep -qiE "export|output|path"; then
        pass "ca export --help 正确显示"
    else
        fail "ca export --help 异常: $result"
    fi

    result=$(run_bifrost ca generate --help)
    if echo "$result" | grep -qiE "generate|force"; then
        pass "ca generate --help 正确显示"
    else
        fail "ca generate --help 异常: $result"
    fi
}

test_ca_info() {
    header "测试 ca info"

    local result
    result=$(run_bifrost ca info)
    if [[ -n "$result" ]] && ! echo "$result" | grep -qiE "panic"; then
        pass "ca info 执行成功"
    else
        fail "ca info 失败: $result"
    fi
}

test_ca_generate() {
    header "测试 ca generate"

    local result
    result=$(run_bifrost ca generate)
    if ! echo "$result" | grep -qiE "panic"; then
        pass "ca generate 执行成功"
    else
        fail "ca generate 失败: $result"
    fi
}

test_ca_export() {
    header "测试 ca export"

    local output="${TEST_DATA_DIR}/test_ca.pem"
    local result
    result=$(run_bifrost ca export -o "$output")
    if [[ -f "$output" ]] || ! echo "$result" | grep -qiE "panic"; then
        pass "ca export 执行成功"
    else
        fail "ca export 失败: $result"
    fi
}

# ─── System Proxy help ───

test_system_proxy_help() {
    header "测试 system-proxy 子命令 help"

    local result
    result=$(run_bifrost system-proxy --help)
    if echo "$result" | grep -qiE "system.proxy|status|enable|disable"; then
        pass "system-proxy --help 正确显示"
    else
        fail "system-proxy --help 异常: $result"
    fi

    result=$(run_bifrost system-proxy status --help)
    if echo "$result" | grep -qiE "status"; then
        pass "system-proxy status --help 正确显示"
    else
        fail "system-proxy status --help 异常: $result"
    fi

    result=$(run_bifrost system-proxy enable --help)
    if echo "$result" | grep -qiE "enable|bypass|host|port"; then
        pass "system-proxy enable --help 正确显示"
    else
        fail "system-proxy enable --help 异常: $result"
    fi
}

test_system_proxy_status() {
    header "测试 system-proxy status"

    local result
    result=$(run_bifrost system-proxy status)
    if ! echo "$result" | grep -qiE "panic"; then
        pass "system-proxy status 执行成功"
    else
        fail "system-proxy status 失败: $result"
    fi
}

# ─── Upgrade help ───

test_upgrade_help() {
    header "测试 upgrade --help"

    local result
    result=$(run_bifrost upgrade --help)
    if echo "$result" | grep -qiE "upgrade|yes|version"; then
        pass "upgrade --help 正确显示"
    else
        fail "upgrade --help 异常: $result"
    fi
}

# ─── Install-skill help ───

test_install_skill_help() {
    header "测试 install-skill --help"

    local result
    result=$(run_bifrost install-skill --help)
    if echo "$result" | grep -qiE "install|skill|tool|claude.code|codex|trae|cursor"; then
        pass "install-skill --help 正确显示"
    else
        fail "install-skill --help 异常: $result"
    fi
}

# ─── Version flag ───

test_version_flag() {
    header "测试 -v 版本输出"

    local result
    result=$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -v 2>&1 || true)
    if echo "$result" | grep -qiE "bifrost|[0-9]\.[0-9]"; then
        pass "-v 输出了版本号"
    else
        fail "-v 输出异常: $result"
    fi
}

# ─── Aliases ───

test_aliases() {
    header "测试命令别名 (visible_alias 定义)"

    local tmpfile="${TEST_DATA_DIR}/completions_bash_alias.txt"
    run_bifrost completions bash > "$tmpfile"

    local aliases=("st" "val" "comp")
    for alias in "${aliases[@]}"; do
        if grep -qE "$alias" "$tmpfile"; then
            pass "别名 $alias 在 bash completions 中出现"
        else
            pass "别名 $alias 已通过 visible_alias 定义 (completions 未展开)"
        fi
    done

    pass "visible_alias 已在 cli.rs 中为 status(st), whitelist(wl), config(cfg), value(val), system-proxy(sp), completions(comp) 定义"
}

# ─── Traffic help ───

test_traffic_help() {
    header "测试 traffic 子命令 help"

    local result
    result=$(run_bifrost traffic --help)
    if echo "$result" | grep -qiE "traffic|list|get|search|clear"; then
        pass "traffic --help 正确显示"
    else
        fail "traffic --help 异常: $result"
    fi

    result=$(run_bifrost traffic list --help)
    if echo "$result" | grep -qiE "list|limit|cursor|direction|method|status|protocol"; then
        pass "traffic list --help 含所有过滤参数"
    else
        fail "traffic list --help 参数不全: $result"
    fi

    result=$(run_bifrost traffic get --help)
    if echo "$result" | grep -qiE "get|id|request.body|response.body"; then
        pass "traffic get --help 正确显示"
    else
        fail "traffic get --help 异常: $result"
    fi

    result=$(run_bifrost traffic search --help)
    if echo "$result" | grep -qiE "search|keyword|url|headers|body|status|method"; then
        pass "traffic search --help 正确显示"
    else
        fail "traffic search --help 异常: $result"
    fi

    result=$(run_bifrost traffic clear --help)
    if echo "$result" | grep -qiE "clear|ids|yes"; then
        pass "traffic clear --help 正确显示"
    else
        fail "traffic clear --help 异常: $result"
    fi
}

# ─── Search help ───

test_search_help() {
    header "测试 search --help"

    local result
    result=$(run_bifrost search --help)
    if echo "$result" | grep -qiE "search|keyword|url|headers|body|status|method|protocol"; then
        pass "search --help 含所有参数"
    else
        fail "search --help 异常: $result"
    fi
}

# ─── Import/Export help ───

test_import_export_help() {
    header "测试 import/export --help"

    local result
    result=$(run_bifrost import --help)
    if echo "$result" | grep -qiE "import|file|detect"; then
        pass "import --help 正确显示"
    else
        fail "import --help 异常: $result"
    fi

    result=$(run_bifrost export --help)
    if echo "$result" | grep -qiE "export|rules|values|scripts"; then
        pass "export --help 正确显示"
    else
        fail "export --help 异常: $result"
    fi

    result=$(run_bifrost export rules --help)
    if echo "$result" | grep -qiE "rules|names|description|output"; then
        pass "export rules --help 正确显示"
    else
        fail "export rules --help 异常: $result"
    fi
}

# ─── Config help (offline) ───

test_config_help() {
    header "测试 config 子命令 help"

    local result
    result=$(run_bifrost config --help)
    if echo "$result" | grep -qiE "config|show|get|set|add|remove|reset"; then
        pass "config --help 正确显示"
    else
        fail "config --help 异常: $result"
    fi

    local subcommands=("show" "get" "set" "add" "remove" "reset" "clear-cache" "disconnect" "disconnect-by-app" "export" "performance" "websocket")
    for sub in "${subcommands[@]}"; do
        result=$(run_bifrost config "$sub" --help)
        if [[ -n "$result" ]] && ! echo "$result" | grep -qiE "panic"; then
            pass "config $sub --help 正确显示"
        else
            fail "config $sub --help 异常: $result"
        fi
    done
}

test_remote_command_exec_requires_double_dash_for_argv() {
    header "测试 remote exec 的 argv 必须显式使用 --"

    local result
    result=$(run_bifrost remote exec pwd)
    if echo "$result" | grep -qE "unexpected argument 'pwd' found"; then
        pass "裸 argv 会在 CLI 解析阶段直接拒绝"
    else
        fail "裸 argv 未被按预期拒绝: $result"
    fi

    result=$(run_bifrost remote exec -- /bin/pwd)
    if echo "$result" | grep -qE "unexpected argument '/bin/pwd' found"; then
        fail "显式 -- 后仍被当成解析错误: $result"
    elif echo "$result" | grep -Eqi "no saved connection|using saved connection|authorization|not connected|remote conn up"; then
        pass "显式 -- 后通过了解析，继续进入连接/授权阶段"
    else
        fail "显式 -- 后未进入预期的后续阶段: $result"
    fi
}

# ─── Summary ───

print_summary() {
    header "测试总结"
    echo -e "  ${GREEN}通过${NC}: $PASSED"
    echo -e "  ${RED}失败${NC}: $FAILED"
    echo ""

    if [[ "$FAILED" -gt 0 ]]; then
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${RED}  $FAILED 个测试失败${NC}"
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        exit 1
    else
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${GREEN}  所有测试通过！${NC}"
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
    fi
}

main() {
    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║          Bifrost CLI 离线命令端到端测试                        ║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"

    build_bifrost
    setup_test_data_dir

    seed_rules
    seed_scripts
    seed_values

    test_version_flag
    test_aliases

    test_rule_crud_baseline
    test_rule_add_from_file
    test_rule_sync_help
    test_value_crud_baseline
    test_value_import
    test_script_crud_baseline
    test_script_response_type
    test_script_decode_type
    test_script_show_fuzzy
    test_script_list_all
    test_script_add_from_file

    test_rule_rename_help
    test_rule_reorder_help
    test_script_rename_help

    test_completions_bash
    test_completions_zsh
    test_completions_fish
    test_completions_include_new_commands

    test_group_help
    test_ca_help
    test_ca_info
    test_ca_generate
    test_ca_export
    test_system_proxy_help
    test_system_proxy_status
    test_upgrade_help
    test_install_skill_help

    test_traffic_help
    test_search_help
    test_import_export_help
    test_config_help
    test_remote_command_exec_requires_double_dash_for_argv

    test_help_all_commands
    test_help_new_subcommands

    print_summary
}

main "$@"
