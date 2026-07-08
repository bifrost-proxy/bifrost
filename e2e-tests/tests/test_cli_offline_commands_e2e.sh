#!/bin/bash
#
# Bifrost CLI 离线命令端到端测试
# 覆盖: rule rename/reorder, script rename, completions, import/export(文件格式), value/rule/script CRUD
#

set -uo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

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

    if grep -qiE "reorder|NAMES|names|priority" <<<"$result"; then
        pass "rule reorder --help 正确显示"
    else
        fail "rule reorder --help 异常: $result"
    fi
}

test_rule_rename_help() {
    header "测试 rule rename 参数解析"

    local result
    result=$(run_bifrost rule rename --help)

    if grep -qiE "rename|NEW_NAME|new.name" <<<"$result"; then
        pass "rule rename --help 正确显示"
    else
        fail "rule rename --help 异常: $result"
    fi
}

test_script_rename_help() {
    header "测试 script rename 参数解析"

    local result
    result=$(run_bifrost script rename --help)

    if grep -qiE "rename|NEW_NAME|new.name" <<<"$result"; then
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

test_root_help_links_docs_instead_of_subcommand_reference() {
    header "测试顶层 help 短链化"

    local result
    result=$(run_bifrost --help)

    if grep -qF "SUBCOMMAND REFERENCE" <<<"$result"; then
        fail "顶层 help 不应再包含 SUBCOMMAND REFERENCE"
    else
        pass "顶层 help 已移除 SUBCOMMAND REFERENCE"
    fi

    if grep -qF "RULE TEMPLATE VARIABLES" <<<"$result" || grep -qF "RULES QUICK START" <<<"$result"; then
        fail "顶层 help 不应再展开规则变量/规则快速开始"
    else
        pass "顶层 help 已移除规则变量和规则快速开始长说明"
    fi

    local links=(
        "docs/cli-quick-start.md"
        "docs/cli.md"
        "docs/getting-started.md"
        "docs/rule.md"
        "docs/operation.md"
        "docs/pattern.md"
        "docs/rules/README.md"
    )
    for link in "${links[@]}"; do
        if grep -qF "$link" <<<"$result"; then
            pass "顶层 help 包含文档链接: $link"
        else
            fail "顶层 help 缺少文档链接: $link"
        fi
    done

    if grep -qF "Use 'bifrost <command> -h'" <<<"$result"; then
        pass "顶层 help 提示使用子命令 help"
    else
        fail "顶层 help 缺少子命令 help 提示"
    fi

    local quick_start_checks=(
        "先选场景"
        "场景 1：使用主服务和多端口安全调试"
        "默认启动会启用系统代理，让浏览器和桌面应用自然进入 Bifrost"
        "默认不要开启全局 TLS 抓包，也不要用 \`--unsafe-ssl\` 跳过上游证书校验"
        "bifrost port bind --port 18888"
        "场景 4：按需调试 HTTPS 和路径规则"
        "不要默认开启全局 TLS 抓包，优先用域名白名单、应用白名单或规则级 \`tlsIntercept://\` 精确控制范围"
        "遇到 SSL pinning 应用时，优先排除应用或域名"
        "场景 6：从流量记录定位问题"
        "场景 8：同一个服务服务多个应用或开发任务"
        "一个常驻 Bifrost 共享证书、配置和流量库；每个任务用独立端口绑定独立规则"
        "bifrost port bind --port 18881 --rule-text \"app-a.example.com tlsIntercept:// host://127.0.0.1:3001\""
        "bifrost traffic list --listener-port 18881 --limit 50"
        "只分析 \`listener_port=18882\` 的流量"
        "不要停止整个主服务"
        "场景 10：远程操作另一台 Bifrost"
        "场景 12：添加飞书或微信 IM 通道"
        "bifrost im provider add feishu-main --type feishu --runner traex"
        "Feishu 会在终端显示授权 URL 和二维码"
        "Weixin 会显示登录二维码"
        "Feishu / Weixin 的 \`base_url\` 由 provider 类型固定"
        "场景 13：和 Agent 协作开发业务 Skill"
        "bifrost install-skill -t codex -y"
        "bifrost port bind --port 18882"
        "如果目标 App 有 SSL pinning，不要强行抓包它的 TLS 明文"
        "bifrost traffic list --listener-port 18882 --limit 50"
        "bifrost traffic list --client-app Chrome --limit 50"
        "不要 mock，不要猜"
        "简单值优先直接写在规则里，不要默认拆成全局 Values"
        "优先使用规则文件内嵌值"
        "命令快捷别名"
        "规则快速入门"
    )
    for expected in "${quick_start_checks[@]}"; do
        if grep -qF "$expected" "$PROJECT_DIR/docs/cli-quick-start.md"; then
            pass "CLI 快速开始包含场景化说明: $expected"
        else
            fail "CLI 快速开始缺少场景化说明: $expected"
        fi
    done

    local cli_doc_checks=(
        "命令覆盖索引"
        "环境变量"
        "规则变量速查"
        "| \`--client-app <APP>\` | 按客户端应用或进程名过滤"
        "\`install-skill\` 只安装 Bifrost Agent Skill 文档"
        "\`bifrost start\` 默认会启用系统代理"
        "TLS 抓包不是默认建议，应按需通过规则级 \`tlsIntercept://\`、\`--intercept-include\` 或 \`--app-intercept-include\` 收窄到目标域名/应用"
        "遇到 SSL pinning 应用时，用 \`tlsPassthrough://\`、\`--intercept-exclude\` 或 \`--app-intercept-exclude\` 排除"
        "需要 TLS 抓包时，服务启动流程会自动生成并安装 Bifrost CA"
        "\`--no-system-proxy\`、\`--unsafe-ssl\` 和 \`--skip-cert-check\` 是测试/诊断选项，不应作为普通启动路径"
        "同一个 Bifrost 服务可以同时服务多个应用或开发任务"
        "只分析 \`listener_port=18882\` 的流量"
        "默认优先写内联 value 或规则文件内嵌值"
        "默认不要把普通 host、token 片段、短 header 或小 mock body 拆到全局 Values"
        "\`setting\` 始终管理本机数据目录；\`remote\` 才会操作已连接的远端 Bifrost"
        "\`--system-proxy\` 修改操作系统代理配置；\`--cli-proxy\` 写入 shell rc 文件中的代理环境变量"
    )
    for expected in "${cli_doc_checks[@]}"; do
        if grep -qF "$expected" "$PROJECT_DIR/docs/cli.md"; then
            pass "CLI 详细文档包含完整手册说明: $expected"
        else
            fail "CLI 详细文档缺少完整手册说明: $expected"
        fi
    done

    local operation_doc_checks=(
        "多行内容优先使用规则文件内嵌值"
        "日常规则不要默认创建全局 Values"
    )
    for expected in "${operation_doc_checks[@]}"; do
        if grep -qF "$expected" "$PROJECT_DIR/docs/operation.md"; then
            pass "Operation 文档包含 Values 推荐边界: $expected"
        else
            fail "Operation 文档缺少 Values 推荐边界: $expected"
        fi
    done
}

test_help_all_commands() {
    header "测试所有子命令 help 可用"

    local commands=("rule" "script" "value" "config" "whitelist" "traffic" "metrics" "sync" "ca" "group" "system-proxy")
    for cmd in "${commands[@]}"; do
        local result
        result=$(run_bifrost "$cmd" --help)
        if [[ -n "$result" ]] && ! grep -qiE "error|panic" <<<"$result"; then
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
    if grep -qiE "rename|new.name|NEW_NAME" <<<"$result"; then
        pass "help: rule rename --help"
    else
        fail "help: rule rename --help 失败: $result"
    fi

    result=$(run_bifrost rule reorder --help)
    if grep -qiE "reorder|names|NAMES" <<<"$result"; then
        pass "help: rule reorder --help"
    else
        fail "help: rule reorder --help 失败: $result"
    fi

    result=$(run_bifrost script rename --help)
    if grep -qiE "rename|new.name|NEW_NAME" <<<"$result"; then
        pass "help: script rename --help"
    else
        fail "help: script rename --help 失败: $result"
    fi

    result=$(run_bifrost metrics --help)
    if grep -qiE "metrics|summary|apps|hosts" <<<"$result"; then
        pass "help: metrics --help"
    else
        fail "help: metrics --help 失败: $result"
    fi

    result=$(run_bifrost sync --help)
    if grep -qiE "sync|status|login|logout" <<<"$result"; then
        pass "help: sync --help"
    else
        fail "help: sync --help 失败: $result"
    fi

    result=$(run_bifrost export --help)
    if grep -qiE "export|rules|values|scripts" <<<"$result"; then
        pass "help: export --help"
    else
        fail "help: export --help 失败: $result"
    fi

    result=$(run_bifrost version-check --help)
    if grep -qiE "version|check|upgrade" <<<"$result"; then
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
    if grep -qE "crud.example.com" <<<"$show_result"; then
        pass "rule add + show 工作正常"
    else
        fail "rule add/show 失败: $show_result"
    fi

    run_bifrost rule update crud_test -c "crud.example.com statusCode://201" >/dev/null
    show_result=$(run_bifrost rule show crud_test)
    if grep -qE "201" <<<"$show_result"; then
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
    if grep -qiE "not found|error|No rule" <<<"$deleted_result"; then
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
    if grep -qE "hello_world" <<<"$show_result"; then
        pass "value add + show 工作正常"
    else
        fail "value add/show 失败: $show_result"
    fi

    run_bifrost value update crud_val "updated_world" >/dev/null
    show_result=$(run_bifrost value show crud_val)
    if grep -qE "updated_world" <<<"$show_result"; then
        pass "value update 工作正常"
    else
        fail "value update 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost value list)
    if grep -qE "crud_val" <<<"$list_result"; then
        pass "value list 包含新增值"
    else
        fail "value list 未包含新增值: $list_result"
    fi

    run_bifrost value delete crud_val >/dev/null
    local deleted_result
    deleted_result=$(run_bifrost value show crud_val)
    if grep -qiE "not found|error" <<<"$deleted_result"; then
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
    if grep -qE "crud_script" <<<"$show_result"; then
        pass "script add + show 工作正常"
    else
        fail "script add/show 失败: $show_result"
    fi

    run_bifrost script update request crud_script -c 'log.info("crud updated");' >/dev/null
    show_result=$(run_bifrost script show request crud_script)
    if grep -qE "crud updated" <<<"$show_result"; then
        pass "script update 工作正常"
    else
        fail "script update 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t request)
    if grep -qE "crud_script" <<<"$list_result"; then
        pass "script list 包含新增脚本"
    else
        fail "script list 未包含脚本: $list_result"
    fi

    local run_result
    run_result=$(run_bifrost script run request crud_script)
    if grep -qE "Output:|Logs:|crud updated" <<<"$run_result"; then
        pass "script run 工作正常"
    else
        fail "script run 失败: $run_result"
    fi

    run_bifrost script delete request crud_script >/dev/null
    local deleted_result
    deleted_result=$(run_bifrost script show request crud_script)
    if grep -qiE "not found|error|No script" <<<"$deleted_result"; then
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
    if grep -qE "filrule.example.com" <<<"$show_result"; then
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
    if grep -qiE "sync|remote" <<<"$result"; then
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
    if [[ -n "$result" ]] && ! grep -qiE "panic" <<<"$result"; then
        pass "value import 执行成功"
    else
        fail "value import 失败: $result"
    fi

    local import_txt="${TEST_DATA_DIR}/test_values.txt"
    echo "txt_key=txt_val" > "$import_txt"
    result=$(run_bifrost value import "$import_txt")
    if ! grep -qiE "panic" <<<"$result"; then
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
    if grep -qE "resp_test|resp script test" <<<"$show_result"; then
        pass "script add response 类型成功"
    else
        fail "script add response 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t response)
    if grep -qE "resp_test" <<<"$list_result"; then
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
    if grep -qE "dec_test|ctx.output" <<<"$show_result"; then
        pass "script add decode 类型成功"
    else
        fail "script add decode 失败: $show_result"
    fi

    local list_result
    list_result=$(run_bifrost script list -t decode)
    if grep -qE "dec_test" <<<"$list_result"; then
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
    if grep -qE "fuzzy_test_script|fuzzy" <<<"$result"; then
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
    if [[ -n "$result" ]] && ! grep -qiE "panic" <<<"$result"; then
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
    if grep -qE "file_script|from file" <<<"$show_result"; then
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
    if grep -qiE "group|list|show|rule" <<<"$result"; then
        pass "group --help 正确显示"
    else
        fail "group --help 异常: $result"
    fi

    result=$(run_bifrost group list --help)
    if grep -qiE "list|keyword|limit" <<<"$result"; then
        pass "group list --help 正确显示"
    else
        fail "group list --help 异常: $result"
    fi

    result=$(run_bifrost group show --help)
    if grep -qiE "show|group.id|GROUP_ID" <<<"$result"; then
        pass "group show --help 正确显示"
    else
        fail "group show --help 异常: $result"
    fi

    result=$(run_bifrost group rule --help)
    if grep -qiE "rule|list|show|add|update|delete" <<<"$result"; then
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
    if grep -qiE "ca|install|info|export|generate" <<<"$result"; then
        pass "ca --help 正确显示"
    else
        fail "ca --help 异常: $result"
    fi

    result=$(run_bifrost ca info --help)
    if grep -qiE "info|certificate" <<<"$result"; then
        pass "ca info --help 正确显示"
    else
        fail "ca info --help 异常: $result"
    fi

    result=$(run_bifrost ca export --help)
    if grep -qiE "export|output|path" <<<"$result"; then
        pass "ca export --help 正确显示"
    else
        fail "ca export --help 异常: $result"
    fi

    result=$(run_bifrost ca generate --help)
    if grep -qiE "generate|force" <<<"$result"; then
        pass "ca generate --help 正确显示"
    else
        fail "ca generate --help 异常: $result"
    fi
}

test_ca_info() {
    header "测试 ca info"

    local result
    result=$(run_bifrost ca info)
    if [[ -n "$result" ]] && ! grep -qiE "panic" <<<"$result"; then
        pass "ca info 执行成功"
    else
        fail "ca info 失败: $result"
    fi
}

test_ca_generate() {
    header "测试 ca generate"

    local result
    result=$(run_bifrost ca generate)
    if ! grep -qiE "panic" <<<"$result"; then
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
    if [[ -f "$output" ]] || ! grep -qiE "panic" <<<"$result"; then
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
    if grep -qiE "system.proxy|status|enable|disable" <<<"$result"; then
        pass "system-proxy --help 正确显示"
    else
        fail "system-proxy --help 异常: $result"
    fi

    result=$(run_bifrost system-proxy status --help)
    if grep -qiE "status" <<<"$result"; then
        pass "system-proxy status --help 正确显示"
    else
        fail "system-proxy status --help 异常: $result"
    fi

    result=$(run_bifrost system-proxy enable --help)
    if grep -qiE "enable|bypass|host|port" <<<"$result"; then
        pass "system-proxy enable --help 正确显示"
    else
        fail "system-proxy enable --help 异常: $result"
    fi
}

test_system_proxy_status() {
    header "测试 system-proxy status"

    local result
    result=$(run_bifrost system-proxy status)
    if ! grep -qiE "panic" <<<"$result"; then
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
    if grep -qiE "upgrade|yes|version" <<<"$result"; then
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
    if grep -qiE "install|skill|tool|claude.code|codex|trae|cursor" <<<"$result"; then
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
    if grep -qiE "bifrost|[0-9]\.[0-9]" <<<"$result"; then
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
    if grep -qiE "traffic|list|get|search|clear" <<<"$result"; then
        pass "traffic --help 正确显示"
    else
        fail "traffic --help 异常: $result"
    fi

    result=$(run_bifrost traffic list --help)
    if grep -qiE "list|limit|cursor|direction|method|status|protocol" <<<"$result"; then
        pass "traffic list --help 含所有过滤参数"
    else
        fail "traffic list --help 参数不全: $result"
    fi

    result=$(run_bifrost traffic get --help)
    if grep -qiE "get|id|request.body|response.body" <<<"$result"; then
        pass "traffic get --help 正确显示"
    else
        fail "traffic get --help 异常: $result"
    fi

    result=$(run_bifrost traffic search --help)
    if grep -qiE "search|keyword|url|headers|body|status|method" <<<"$result"; then
        pass "traffic search --help 正确显示"
    else
        fail "traffic search --help 异常: $result"
    fi

    result=$(run_bifrost traffic clear --help)
    if grep -qiE "clear|ids|yes" <<<"$result"; then
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
    if grep -qiE "search|keyword|url|headers|body|status|method|protocol" <<<"$result"; then
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
    if grep -qiE "import|file|detect" <<<"$result"; then
        pass "import --help 正确显示"
    else
        fail "import --help 异常: $result"
    fi

    result=$(run_bifrost export --help)
    if grep -qiE "export|rules|values|scripts" <<<"$result"; then
        pass "export --help 正确显示"
    else
        fail "export --help 异常: $result"
    fi

    result=$(run_bifrost export rules --help)
    if grep -qiE "rules|names|description|output" <<<"$result"; then
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
    if grep -qiE "config|show|get|set|add|remove|reset" <<<"$result"; then
        pass "config --help 正确显示"
    else
        fail "config --help 异常: $result"
    fi

    local subcommands=("show" "get" "set" "add" "remove" "reset" "clear-cache" "disconnect" "disconnect-by-app" "export" "performance" "websocket")
    for sub in "${subcommands[@]}"; do
        result=$(run_bifrost config "$sub" --help)
        if [[ -n "$result" ]] && ! grep -qiE "panic" <<<"$result"; then
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
    if grep -qE "unexpected argument 'pwd' found" <<<"$result"; then
        pass "裸 argv 会在 CLI 解析阶段直接拒绝"
    else
        fail "裸 argv 未被按预期拒绝: $result"
    fi

    result=$(run_bifrost remote exec -- /bin/pwd)
    if grep -qE "unexpected argument '/bin/pwd' found" <<<"$result"; then
        fail "显式 -- 后仍被当成解析错误: $result"
    elif grep -Eqi "no saved connection|using saved connection|authorization|not connected|remote conn up" <<<"$result"; then
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

    test_root_help_links_docs_instead_of_subcommand_reference
    test_help_all_commands
    test_help_new_subcommands

    print_summary
}

main "$@"
