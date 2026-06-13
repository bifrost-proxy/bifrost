# CLI 规则管理命令测试用例

## 功能模块说明

本文档覆盖 Bifrost CLI `rule` 子命令的完整功能测试，包括规则的增删改查、启用/禁用、重命名、排序、活跃规则查看、同步、多操作组合、过滤器（includeFilter/excludeFilter）以及规则属性（lineProps）等场景。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保端口 8800 未被其他进程占用
3. 确保 `.bifrost-test` 目录不存在或已清空（以获得干净的初始状态）
4. 后续所有 `bifrost` 命令均需在同一数据目录下执行，示例：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- rule list
   ```
   为简化描述，后续用 `bifrost` 指代 `BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost --`

---

## 测试用例

### TC-CRM-01：初始状态下列出规则为空

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- 输出 `No rules found.`

---

### TC-CRM-02：通过 --content 添加简单规则

**操作步骤**：
1. 执行 `bifrost rule add test-host -c "example.com host://127.0.0.1:3000"`

**预期结果**：
- 输出 `Rule 'test-host' added successfully.`

---

### TC-CRM-03：添加规则后列表显示正确

**前置条件**：已通过 TC-CRM-02 添加规则

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- 输出包含 `Rules (1):`
- 输出包含 `test-host [enabled]`

---

### TC-CRM-04：通过 --file 从文件添加规则

**操作步骤**：
1. 创建临时规则文件：
   ```bash
   cat > /tmp/bifrost-test-rule.txt << 'EOF'
   api.example.com host://127.0.0.1:4000
   cdn.example.com host://127.0.0.1:5000
   EOF
   ```
2. 执行 `bifrost rule add test-file -f /tmp/bifrost-test-rule.txt`

**预期结果**：
- 输出 `Rule 'test-file' added successfully.`

---

### TC-CRM-05：查看规则详情（show）

**前置条件**：已通过 TC-CRM-04 添加 test-file 规则

**操作步骤**：
1. 执行 `bifrost rule show test-file`

**预期结果**：
- 输出包含 `Rule: test-file`
- 输出包含 `Status: enabled`
- 输出包含 `Content:`
- 输出包含 `api.example.com host://127.0.0.1:4000`
- 输出包含 `cdn.example.com host://127.0.0.1:5000`

---

### TC-CRM-06：通过 get 别名查看规则详情

**前置条件**：已添加 test-host 规则

**操作步骤**：
1. 执行 `bifrost rule get test-host`

**预期结果**：
- 输出与 `bifrost rule show test-host` 一致
- 输出包含 `Rule: test-host`
- 输出包含 `Status: enabled`
- 输出包含 `example.com host://127.0.0.1:3000`

---

### TC-CRM-07：通过 --content 更新规则内容

**前置条件**：已添加 test-host 规则

**操作步骤**：
1. 执行 `bifrost rule update test-host -c "example.com host://127.0.0.1:8080"`

**预期结果**：
- 输出 `Rule 'test-host' updated successfully.`

---

### TC-CRM-08：验证更新后的规则内容

**前置条件**：已通过 TC-CRM-07 更新规则

**操作步骤**：
1. 执行 `bifrost rule show test-host`

**预期结果**：
- 输出包含 `example.com host://127.0.0.1:8080`
- 不再包含旧内容 `127.0.0.1:3000`

---

### TC-CRM-09：通过 --file 更新规则内容

**前置条件**：已添加 test-file 规则

**操作步骤**：
1. 创建新的规则文件：
   ```bash
   cat > /tmp/bifrost-test-rule-updated.txt << 'EOF'
   api.example.com host://127.0.0.1:9000
   EOF
   ```
2. 执行 `bifrost rule update test-file -f /tmp/bifrost-test-rule-updated.txt`

**预期结果**：
- 输出 `Rule 'test-file' updated successfully.`
- 执行 `bifrost rule show test-file` 后内容为 `api.example.com host://127.0.0.1:9000`，不再包含 `cdn.example.com`

---

### TC-CRM-10：禁用规则

**前置条件**：已添加 test-host 规则且状态为 enabled

**操作步骤**：
1. 执行 `bifrost rule disable test-host`

**预期结果**：
- 输出 `Rule 'test-host' disabled.`

---

### TC-CRM-11：验证禁用后规则列表状态

**前置条件**：已通过 TC-CRM-10 禁用规则

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- 输出包含 `test-host [disabled]`
- 输出包含 `test-file [enabled]`

---

### TC-CRM-12：启用规则

**前置条件**：test-host 规则处于 disabled 状态

**操作步骤**：
1. 执行 `bifrost rule enable test-host`

**预期结果**：
- 输出 `Rule 'test-host' enabled.`

---

### TC-CRM-13：验证启用后规则状态

**前置条件**：已通过 TC-CRM-12 启用规则

**操作步骤**：
1. 执行 `bifrost rule show test-host`

**预期结果**：
- 输出包含 `Status: enabled`

---

### TC-CRM-14：重命名规则

**前置条件**：已添加 test-host 规则，Bifrost 服务正在运行

**操作步骤**：
1. 执行 `bifrost rule rename test-host my-proxy-rule`

**预期结果**：
- 输出 `Rule 'test-host' renamed to 'my-proxy-rule'.`

---

### TC-CRM-15：验证重命名后规则列表

**前置条件**：已通过 TC-CRM-14 重命名规则

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- 输出包含 `my-proxy-rule [enabled]`
- 不再包含 `test-host`

---

### TC-CRM-16：重排序规则优先级

**前置条件**：存在 my-proxy-rule 和 test-file 两条规则，Bifrost 服务正在运行

**操作步骤**：
1. 执行 `bifrost rule reorder test-file my-proxy-rule`

**预期结果**：
- 输出包含 `Rules reordered successfully:`
- 输出包含 `1. test-file`
- 输出包含 `2. my-proxy-rule`

---

### TC-CRM-17：验证重排序后规则顺序

**前置条件**：已通过 TC-CRM-16 重排序

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- `test-file` 显示在 `my-proxy-rule` 之前

---

### TC-CRM-18：查看活跃规则摘要（无活跃规则）

**前置条件**：所有规则均已禁用，Bifrost 服务正在运行

**操作步骤**：
1. 执行 `bifrost rule disable my-proxy-rule`
2. 执行 `bifrost rule disable test-file`
3. 执行 `bifrost rule active`

**预期结果**：
- 输出包含 `Active Rules Summary`
- 输出包含 `No active rules.`

---

### TC-CRM-19：查看活跃规则摘要（有活跃规则）

**前置条件**：Bifrost 服务正在运行

**操作步骤**：
1. 执行 `bifrost rule enable my-proxy-rule`
2. 执行 `bifrost rule enable test-file`
3. 执行 `bifrost rule active`

**预期结果**：
- 输出包含 `Active Rules Summary`
- 输出包含 `Total active: 2 rule file(s)`
- 输出包含 `My Rules`
- 输出包含 `my-proxy-rule` 和 `test-file`
- 输出包含 `Merged Rules (in parsing order)` 部分

---

### TC-CRM-20：删除规则

**操作步骤**：
1. 执行 `bifrost rule delete my-proxy-rule`

**预期结果**：
- 输出 `Rule 'my-proxy-rule' deleted successfully.`

---

### TC-CRM-21：验证删除后规则列表

**前置条件**：已通过 TC-CRM-20 删除 my-proxy-rule

**操作步骤**：
1. 执行 `bifrost rule list`

**预期结果**：
- 输出包含 `Rules (1):`
- 仅包含 `test-file [enabled]`
- 不包含 `my-proxy-rule`

---

### TC-CRM-22：添加不提供 --content 和 --file 时报错

**操作步骤**：
1. 执行 `bifrost rule add empty-rule`

**预期结果**：
- 命令执行失败
- 错误信息包含 `Either --content or --file must be provided`

---

### TC-CRM-23：查看不存在的规则时报错

**操作步骤**：
1. 执行 `bifrost rule show non-existent-rule`

**预期结果**：
- 命令执行失败
- 输出包含错误信息（规则不存在相关提示）

---

### TC-CRM-24：删除不存在的规则时报错

**操作步骤**：
1. 执行 `bifrost rule delete non-existent-rule`

**预期结果**：
- 命令执行失败
- 输出包含错误信息（规则不存在相关提示）

---

### TC-CRM-25：添加包含多个操作的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add multi-ops -c "example.com host://127.0.0.1:3000 reqHeaders://X-Custom=test resHeaders://X-Debug=1"
   ```

**预期结果**：
- 输出 `Rule 'multi-ops' added successfully.`
- 执行 `bifrost rule show multi-ops` 后内容包含 `host://127.0.0.1:3000`、`reqHeaders://X-Custom=test`、`resHeaders://X-Debug=1`

---

### TC-CRM-26：添加包含 includeFilter 的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add filter-include -c "example.com host://127.0.0.1:3000 includeFilter://m:GET"
   ```

**预期结果**：
- 输出 `Rule 'filter-include' added successfully.`
- 执行 `bifrost rule show filter-include` 后内容包含 `includeFilter://m:GET`

---

### TC-CRM-27：添加包含 excludeFilter 的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add filter-exclude -c "example.com host://127.0.0.1:3000 excludeFilter:///admin/"
   ```

**预期结果**：
- 输出 `Rule 'filter-exclude' added successfully.`
- 执行 `bifrost rule show filter-exclude` 后内容包含 `excludeFilter:///admin/`

---

### TC-CRM-28：添加同时包含 includeFilter 和 excludeFilter 的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add filter-both -c "example.com host://127.0.0.1:3000 includeFilter://m:GET,POST excludeFilter:///health/"
   ```

**预期结果**：
- 输出 `Rule 'filter-both' added successfully.`
- 执行 `bifrost rule show filter-both` 后内容同时包含 `includeFilter://m:GET,POST` 和 `excludeFilter:///health/`

---

### TC-CRM-29：添加包含 lineProps://important 的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add high-priority -c "example.com host://127.0.0.1:3000 lineProps://important"
   ```

**预期结果**：
- 输出 `Rule 'high-priority' added successfully.`
- 执行 `bifrost rule show high-priority` 后内容包含 `lineProps://important`

---

### TC-CRM-30：添加包含 lineProps://disabled 的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add disabled-line -c "example.com host://127.0.0.1:3000 lineProps://disabled"
   ```

**预期结果**：
- 输出 `Rule 'disabled-line' added successfully.`
- 执行 `bifrost rule show disabled-line` 后内容包含 `lineProps://disabled`

---

### TC-CRM-31：添加包含 lineProps://important,disabled 组合属性的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add combo-props -c "example.com host://127.0.0.1:3000 lineProps://important,disabled"
   ```

**预期结果**：
- 输出 `Rule 'combo-props' added successfully.`
- 执行 `bifrost rule show combo-props` 后内容包含 `lineProps://important,disabled`

---

### TC-CRM-32：添加包含 statusCode 操作的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add mock-404 -c "example.com/not-found statusCode://404"
   ```

**预期结果**：
- 输出 `Rule 'mock-404' added successfully.`
- 执行 `bifrost rule show mock-404` 后内容包含 `statusCode://404`

---

### TC-CRM-33：添加包含 file 操作的 mock 响应规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add mock-body -c 'example.com/api file://({\"code\":0,\"data\":null})'
   ```

**预期结果**：
- 输出 `Rule 'mock-body' added successfully.`
- 执行 `bifrost rule show mock-body` 后内容包含 `file://`

---

### TC-CRM-34：通过文件添加多行规则（包含过滤器和属性）

**操作步骤**：
1. 创建包含复杂规则的文件：
   ```bash
   cat > /tmp/bifrost-complex-rule.txt << 'EOF'
   example.com host://127.0.0.1:3000 includeFilter://m:GET lineProps://important
   api.example.com reqHeaders://Authorization=Bearer-test123 excludeFilter:///public/
   *.cdn.example.com host://127.0.0.1:5000
   EOF
   ```
2. 执行 `bifrost rule add complex-rule -f /tmp/bifrost-complex-rule.txt`

**预期结果**：
- 输出 `Rule 'complex-rule' added successfully.`
- 执行 `bifrost rule show complex-rule` 后内容包含三行规则
- 第一行包含 `includeFilter://m:GET` 和 `lineProps://important`
- 第二行包含 `reqHeaders://` 和 `excludeFilter:///public/`
- 第三行包含通配符 `*.cdn.example.com`

---

### TC-CRM-35：添加包含多个操作+过滤器+属性的完整规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add full-featured -c "example.com host://127.0.0.1:3000 reqHeaders://X-Proxy=Bifrost resHeaders://X-Debug=true includeFilter://m:GET,POST excludeFilter:///admin/ lineProps://important"
   ```

**预期结果**：
- 输出 `Rule 'full-featured' added successfully.`
- 执行 `bifrost rule show full-featured` 后内容包含：
  - `host://127.0.0.1:3000`
  - `reqHeaders://X-Proxy=Bifrost`
  - `resHeaders://X-Debug=true`
  - `includeFilter://m:GET,POST`
  - `excludeFilter:///admin/`
  - `lineProps://important`

---

### TC-CRM-36：添加包含 proxy 操作的链式代理规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add chain-proxy -c "example.com proxy://127.0.0.1:7890"
   ```

**预期结果**：
- 输出 `Rule 'chain-proxy' added successfully.`
- 执行 `bifrost rule show chain-proxy` 后内容包含 `proxy://127.0.0.1:7890`

---

### TC-CRM-37：添加包含 http3 操作的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add h3-rule -c "chatgpt.com http3://"
   ```

**预期结果**：
- 输出 `Rule 'h3-rule' added successfully.`
- 执行 `bifrost rule show h3-rule` 后内容包含 `http3://`

---

### TC-CRM-38：添加包含 redirect 操作的规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add redirect-rule -c "old.example.com redirect://https://new.example.com"
   ```

**预期结果**：
- 输出 `Rule 'redirect-rule' added successfully.`
- 执行 `bifrost rule show redirect-rule` 后内容包含 `redirect://https://new.example.com`

---

### TC-CRM-39：规则同步（未配置远程服务）

**操作步骤**：
1. 执行 `bifrost rule sync`

**预期结果**：
- 输出包含 `Starting rules sync...`
- 输出包含 `Remote:` 和 `Enabled:` 信息
- 由于未配置远程同步服务，可能输出同步失败的信息（如连接失败或未启用）

---

### TC-CRM-40：对同一规则连续启用/禁用切换

**前置条件**：已添加 test-file 规则

**操作步骤**：
1. 执行 `bifrost rule disable test-file`
2. 执行 `bifrost rule show test-file`，确认 `Status: disabled`
3. 执行 `bifrost rule enable test-file`
4. 执行 `bifrost rule show test-file`，确认 `Status: enabled`
5. 执行 `bifrost rule disable test-file`
6. 执行 `bifrost rule show test-file`，确认 `Status: disabled`

**预期结果**：
- 每次 disable 后 show 显示 `Status: disabled`
- 每次 enable 后 show 显示 `Status: enabled`
- 状态切换稳定可靠，无异常

---

### TC-CRM-41：添加包含路径过滤的 includeFilter 规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add path-filter -c "example.com host://127.0.0.1:3000 includeFilter:///api/v1/"
   ```

**预期结果**：
- 输出 `Rule 'path-filter' added successfully.`
- 执行 `bifrost rule show path-filter` 后内容包含 `includeFilter:///api/v1/`

---

### TC-CRM-42：添加包含状态码过滤的 includeFilter 规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add status-filter -c "example.com resHeaders://X-Debug=1 includeFilter://s:200-299"
   ```

**预期结果**：
- 输出 `Rule 'status-filter' added successfully.`
- 执行 `bifrost rule show status-filter` 后内容包含 `includeFilter://s:200-299`

---

### TC-CRM-43：添加包含请求头过滤的 includeFilter 规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add header-filter -c "example.com host://127.0.0.1:3000 includeFilter://h:X-Custom-Header"
   ```

**预期结果**：
- 输出 `Rule 'header-filter' added successfully.`
- 执行 `bifrost rule show header-filter` 后内容包含 `includeFilter://h:X-Custom-Header`

---

### TC-CRM-44：添加包含客户端 IP 过滤的 includeFilter 规则

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add ip-filter -c "example.com host://127.0.0.1:3000 includeFilter://i:192.168.1.0/24"
   ```

**预期结果**：
- 输出 `Rule 'ip-filter' added successfully.`
- 执行 `bifrost rule show ip-filter` 后内容包含 `includeFilter://i:192.168.1.0/24`

---

### TC-CRM-45：批量清理所有测试规则

**操作步骤**：
1. 执行 `bifrost rule list` 获取所有规则名称
2. 对列表中的每条规则执行 `bifrost rule delete <name>`
3. 执行 `bifrost rule list`

**预期结果**：
- 所有删除命令均输出 `deleted successfully`
- 最终 `bifrost rule list` 输出 `No rules found.`

---

### TC-CRM-46：有效规则新增返回 JSON 语法检查报告

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add syntax-valid-cli --json -c "example.com host://127.0.0.1:3000"
   ```

**预期结果**：
- 命令退出码为 0
- 输出 JSON 包含 `"success": true`
- 输出 JSON 包含 `"saved": true`
- 输出 JSON 包含 `"syntax.valid": true`
- 输出 JSON 包含 `"syntax.rule_count": 1`

---

### TC-CRM-47：缺失引用规则新增失败且不落盘

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add syntax-invalid-cli --json -c "@missing-shared-rule"
   ```
2. 执行：
   ```bash
   bifrost rule show syntax-invalid-cli
   ```

**预期结果**：
- 第 1 步命令退出码为 2
- 第 1 步输出 JSON 包含 `"success": false`
- 第 1 步输出 JSON 包含 `"saved": false`
- 第 1 步输出 JSON 包含 `"syntax.valid": false`
- 第 1 步输出 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步返回规则不存在错误，说明无效规则没有落盘

---

### TC-CRM-48：缺失引用规则更新失败且保留旧内容

**前置条件**：已添加规则 `syntax-preserve-cli`，内容为 `preserve.example.com host://127.0.0.1:3001`

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule update syntax-preserve-cli --json -c "@missing-update-reference"
   ```
2. 执行：
   ```bash
   bifrost rule show syntax-preserve-cli
   ```

**预期结果**：
- 第 1 步命令退出码为 2
- 第 1 步输出 JSON 包含 `"saved": false`
- 第 1 步输出 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步仍显示 `preserve.example.com host://127.0.0.1:3001`

---

### TC-CRM-49：allow-invalid 显式保存无效规则并返回语法错误

**操作步骤**：
1. 执行：
   ```bash
   bifrost rule add syntax-allowed-cli --json --allow-invalid -c "@missing-allowed-reference"
   ```
2. 执行：
   ```bash
   bifrost rule show syntax-allowed-cli
   ```

**预期结果**：
- 第 1 步命令退出码为 0
- 第 1 步输出 JSON 包含 `"success": true`
- 第 1 步输出 JSON 包含 `"saved": true`
- 第 1 步输出 JSON 包含 `"syntax.valid": false`
- 第 1 步输出 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步显示 `@missing-allowed-reference`

**执行结果（2026-06-12，本地开发分支）**：
- ✅ PASS：执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_cli.sh`。脚本使用临时 `BIFROST_DATA_DIR` 和当前分支 debug 二进制，验证有效新增返回 JSON `saved=true` / `syntax.valid=true`，缺失 `@` 引用新增退出码为 2 且不落盘，缺失引用更新退出码为 2 且旧内容保持不变，`--allow-invalid` 可保存无效规则但返回 `syntax.valid=false` / `E020`。

---

### TC-CRM-50：legacy `filter://` 控制标记不被语法检查误判为未知协议

**操作步骤**：
1. 使用当前分支 release 二进制执行：
   ```bash
   BIFROST_DATA_DIR="$TEST_ROOT/data" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   PROXY_PORT=18920 \
   ECHO_HTTP_PORT=18921 \
   ECHO_HTTPS_PORT=18922 \
   ECHO_WS_PORT=18923 \
   ECHO_WSS_PORT=18924 \
   ECHO_SSE_PORT=18925 \
   ECHO_PROXY_PORT=18926 \
   bash e2e-tests/test_rules.sh --use-binary --no-build -p 18920 -d "$TEST_ROOT/data" rules/control/filter.txt
   ```

**预期结果**：
- 规则语法检查通过，`filter://` 不返回 `E002 Unknown protocol`
- 代理能正常启动，加载 `*.local host://127.0.0.1:<echo-port>` 规则
- `control/filter.txt` 中的请求断言通过，失败数为 0

**执行结果（2026-06-13，本地开发分支）**：
- ✅ PASS：先执行 `env SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 生成当前分支 release 二进制，再执行上述单 fixture 命令。输出显示规则语法检查 `✓ 通过: 1 个`，代理启动成功，`Filter 规则` 断言通过，汇总为 `Total: 1 / Passed: 1 / Failed: 0`。

---

## 清理

测试完成后清理临时数据和临时文件：
```bash
rm -rf .bifrost-test
rm -f /tmp/bifrost-test-rule.txt
rm -f /tmp/bifrost-test-rule-updated.txt
rm -f /tmp/bifrost-complex-rule.txt
```
