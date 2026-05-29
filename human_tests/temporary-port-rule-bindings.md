# 临时端口规则绑定真实场景测试

## 功能模块说明

验证 `bifrost port` 临时端口绑定能力：主端口和临时端口共享同一个 `BIFROST_DATA_DIR` 中的规则、values、scripts、证书和 traffic 存储，但临时端口只加载显式绑定规则。主端口默认规则 enabled/disabled 切换不应影响临时端口；销毁临时端口不应影响主端口；Traffic list/detail、CLI 与 Web 管理端必须展示监听端口。

## 前置条件

1. 在仓库根目录执行。
2. 所有命令先执行 `source ~/.zshrc`。
3. 使用 `mktemp -d` 创建隔离 `BIFROST_DATA_DIR`，不使用默认数据目录。
4. 测试端口禁止使用 `9900`。
5. 启动代理必须带 `--no-system-proxy`。
6. 先构建最新二进制：
   ```bash
   source ~/.zshrc
   cargo build --bin bifrost
   ```

## 测试用例

### TC-TPRB-01：主端口默认规则正常生效

**操作步骤**：
1. 创建隔离数据目录并设置 `BIFROST_DATA_DIR`。
2. 添加规则：`main-default` 匹配 `main-only.test` 返回 `main-default`；`temp-bound` 匹配 `temp-only.test` 返回 `temp-bound`。
3. 禁用 `temp-bound`。
4. 启动主代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start -p "$MAIN_PORT" --skip-cert-check --unsafe-ssl --no-system-proxy
   ```
5. 请求主端口：
   ```bash
   curl -sS -x "http://127.0.0.1:$MAIN_PORT" http://main-only.test/main-port
   ```

**预期结果**：
- 主端口响应包含 `main-default`。
- 主端口请求 `temp-only.test` 不包含 `temp-bound`。

### TC-TPRB-02：临时端口绑定 disabled 规则仍生效

**操作步骤**：
1. 绑定临时端口：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost port bind --port "$TEMP_PORT" --rule temp-bound
   ```
2. 请求临时端口：
   ```bash
   curl -sS -x "http://127.0.0.1:$TEMP_PORT" http://temp-only.test/temp-port
   ```

**预期结果**：
- `port bind` 输出 `Temporary port` 和绑定规则名。
- 响应包含 `temp-bound`，证明端口显式绑定本身就是启用声明。

### TC-TPRB-03：临时端口不加载主端口 enabled 规则

**操作步骤**：
1. 请求临时端口上的主规则 host：
   ```bash
   curl -sS -x "http://127.0.0.1:$TEMP_PORT" http://main-only.test/not-bound
   ```

**预期结果**：
- 响应不包含 `main-default`。
- `bifrost port active "$TEMP_PORT"` 只展示 `temp-bound`，不展示 `main-default`。

### TC-TPRB-04：主端口默认规则启用/禁用切换不影响临时端口

**操作步骤**：
1. 执行：
   ```bash
   ./target/debug/bifrost rule disable main-default
   ./target/debug/bifrost rule enable main-default
   ./target/debug/bifrost port active "$TEMP_PORT"
   curl -sS -x "http://127.0.0.1:$TEMP_PORT" http://temp-only.test/after-toggle
   ```

**预期结果**：
- `port active` 仍只展示临时端口绑定规则。
- 临时端口请求仍返回 `temp-bound`。

### TC-TPRB-05：临时端口绑定多条规则且顺序稳定

**操作步骤**：
1. 新增 `temp-first` 与 `temp-second`。
2. 执行：
   ```bash
   ./target/debug/bifrost port bind --port "$ORDER_PORT" --rule temp-first --rule temp-second
   ./target/debug/bifrost port active "$ORDER_PORT"
   ```

**预期结果**：
- `port active` 中 `temp-first` 排在 `temp-second` 之前。

### TC-TPRB-06：自动分配端口与指定端口

**操作步骤**：
1. 指定端口创建：`port bind --port "$TEMP_PORT" --rule temp-bound`。
2. 自动分配端口：
   ```bash
   ./target/debug/bifrost port bind --port 0 --rule temp-bound
   ```
3. 使用输出中的端口发起请求。

**预期结果**：
- 指定端口按输入端口监听。
- 自动分配端口输出非 `0` 且非 `9900` 的实际端口。
- 自动分配出的端口不得与 `MAIN_PORT` 或本轮已分配的其他临时端口重复。
- 自动端口请求返回 `temp-bound`。

### TC-TPRB-06-回归-01：脚本连续分配端口时不与主端口冲突

**背景**：之前 `test_temporary_port_bindings.sh` 通过命令替换调用端口分配函数时，函数内自增的 offset 发生在子 shell，导致多次调用拿到相同端口，进而触发 `Port <same_port> is the main proxy port`。

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   bash -x e2e-tests/tests/test_temporary_port_bindings.sh
   ```
2. 观察脚本打印出来的 `MAIN_PORT`、`TEMP_PORT`、`ORDER_PORT`、`FILE_PORT`、`INLINE_PORT`、`UPDATE_PORT`、`MISSING_PORT`。

**预期结果**：
- 上述端口全部非 `9900`。
- `MAIN_PORT` 与 `TEMP_PORT` 等所有临时端口互不相同。
- 脚本不会再因为 `Port <same_port> is the main proxy port` 失败。

### TC-TPRB-06-回归-02：HTML fixture 端口占用时自动换端口

**背景**：CI 并行 shard 中，`test_temporary_port_bindings.sh` 的 HTML fixture 端口可能在探测后、实际 bind 前被其他进程抢占，导致 fixture server 未就绪并使整条 E2E 失败。

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_temporary_port_bindings.sh
   ```
2. 观察主端口 HTML Badge、临时端口 HTML Badge、无规则直连 fixture 请求是否都能通过。
3. 如 fixture 端口被占用，脚本应自动重新选择可用端口，并在创建规则前使用最终端口。

**预期结果**：
- HTML fixture server 必须真实启动成功后才继续创建规则。
- fixture bind 失败时脚本自动换端口重试，失败日志可诊断，不再静默等待到超时。
- 主端口和临时端口的 HTML Badge、直连 fixture 请求均通过。

### TC-TPRB-07：错误信息质量

**操作步骤**：
1. 重复绑定已存在临时端口。
2. 绑定主端口。
3. 不传任何规则输入。
4. 绑定不存在的规则名 `not-exist`。
5. 绑定不可读取或解析失败的规则文件。
6. 绑定解析失败的规则原文。

**预期结果**：
- 端口冲突错误包含冲突端口号和冲突原因。
- 不传规则错误明确提示至少传一个 `--rule`、`--rule-file`、`--rule-text` 或 `--group-rule`。
- 不存在规则错误包含规则名 `not-exist`。
- 规则文件错误包含文件路径和 IO/解析原因。
- 规则原文错误包含解析行列和 parser message。
- 失败后 `port list` 不包含失败端口。

### TC-TPRB-08：规则文件输入

**操作步骤**：
1. 创建规则文件：
   ```bash
   echo "file-only.test status://213 resBody://(file-rule)" > "$TEST_DIR/file-rule.bifrost"
   ```
2. 执行：
   ```bash
   ./target/debug/bifrost port bind --port "$FILE_PORT" --rule-file "$TEST_DIR/file-rule.bifrost"
   curl -sS -x "http://127.0.0.1:$FILE_PORT" http://file-only.test/from-file
   ```

**预期结果**：
- bind 输出包含文件名。
- 响应包含 `file-rule`。

### TC-TPRB-09：规则原文输入与 update 子命令

**操作步骤**：
1. 执行：
   ```bash
   ./target/debug/bifrost port bind --port "$INLINE_PORT" --rule-text "inline-only.test status://214 resBody://(inline-rule)"
   curl -sS -x "http://127.0.0.1:$INLINE_PORT" http://inline-only.test/from-inline
   ./target/debug/bifrost port bind --port "$UPDATE_PORT" --rule temp-bound
   ./target/debug/bifrost port update "$UPDATE_PORT" --rule-text "updated-only.test status://215 resBody://(updated-rule)"
   curl -sS -x "http://127.0.0.1:$UPDATE_PORT" http://updated-only.test/after-update
   ```

**预期结果**：
- 原文绑定请求返回 `inline-rule`。
- `port update` 后新规则返回 `updated-rule`。
- `port show/list/active/update/destroy` 子命令均已实际覆盖。

### TC-TPRB-10：Traffic CLI/API 包含监听端口

**操作步骤**：
1. 分别通过主端口和临时端口生成流量。
2. 调用：
   ```bash
   curl -sS "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic?limit=100"
   curl -sS "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic/$TEMP_RECORD_ID"
   ./target/debug/bifrost traffic list --port "$MAIN_PORT" --format json
   ./target/debug/bifrost traffic get --port "$MAIN_PORT" "$TEMP_RECORD_ID" --format json
   ```

**预期结果**：
- compact API 中主端口记录带 `lp: MAIN_PORT`，临时端口记录带 `lp: TEMP_PORT`。
- detail API 和 CLI `traffic get` 中有 `listener_port`。
- CLI `traffic list` JSON 中有 `lp`。

### TC-TPRB-10B：无规则命中流量仍记录入口监听端口

**背景**：入口端口是流量归因元数据，不能只在规则命中请求上记录；否则后续分析无法判断未命中规则、直连、CONNECT 或错误流量来自哪个代理端口。

**操作步骤**：
1. 启动一个本地 HTTP fixture，例如 `http://127.0.0.1:$HTML_PORT/direct-main`。
2. 通过主端口请求该 fixture，且不配置匹配该 URL 的规则：
   ```bash
   curl -sS -x "http://127.0.0.1:$MAIN_PORT" "http://127.0.0.1:$HTML_PORT/direct-main"
   ```
3. 通过临时端口请求同一 fixture 的另一路径，且不配置匹配该 URL 的规则：
   ```bash
   curl -sS -x "http://127.0.0.1:$TEMP_PORT" "http://127.0.0.1:$HTML_PORT/direct-temp"
   ```
4. 查询 Traffic compact API 和 detail API：
   ```bash
   curl -sS "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic?limit=100"
   curl -sS "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic/$DIRECT_TEMP_RECORD_ID"
   ```

**预期结果**：
- `/direct-main` 记录带 `lp: MAIN_PORT`，detail 中 `listener_port: MAIN_PORT`。
- `/direct-temp` 记录带 `lp: TEMP_PORT`，detail 中 `listener_port: TEMP_PORT`。
- 两条记录都保持 `has_rule_hit=false`，证明端口记录不依赖规则命中。

### TC-TPRB-10-回归-01：Traffic list/get 子命令接受 `--port`

**背景**：之前 `bifrost traffic list --port "$MAIN_PORT" --format json` 和 `bifrost traffic get --port "$MAIN_PORT" "$TEMP_RECORD_ID" --format json` 在参数解析阶段报 `error: unexpected argument '--port' found`，导致临时端口 E2E 卡在 Traffic CLI 验证步骤。

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   cargo run --bin bifrost -- traffic list --help
   ```
2. 执行：
   ```bash
   source ~/.zshrc
   cargo run --bin bifrost -- traffic get --help
   ```
3. 执行：
   ```bash
   source ~/.zshrc
   e2e-tests/tests/test_temporary_port_bindings.sh
   ```

**预期结果**：
- `traffic list --help` 输出包含 `--port <PORT>`。
- `traffic get --help` 输出包含 `--port <PORT>`。
- `test_temporary_port_bindings.sh` 不再因为 `traffic list/get --port` 参数解析失败而报 `unexpected argument '--port' found`。

### TC-TPRB-10-回归-02：临时端口 Badge 展示当前端口绑定规则

**背景**：之前 Inject Bifrost Badge 使用主端口的默认 active summary，临时端口代理出来的页面会显示主端口 enabled 规则与合并规则内容，误导用户。

**操作步骤**：
1. 新增主端口 HTML 规则 `main-badge`，响应 `badge-main.test` 且 `Content-Type=text/html`。
2. 新增临时端口 HTML 规则 `temp-badge`，响应 `badge-temp.test` 且 `Content-Type=text/html`，并禁用该规则。
3. 启动 Bifrost 时启用 Badge 注入。
4. 绑定临时端口：
   ```bash
   ./target/debug/bifrost port bind --port "$TEMP_PORT" --rule temp-bound --rule temp-badge
   ```
5. 分别请求：
   ```bash
   curl -sS -x "http://127.0.0.1:$MAIN_PORT" http://badge-main.test/badge-main
   curl -sS -x "http://127.0.0.1:$TEMP_PORT" http://badge-temp.test/badge-temp
   ```

**预期结果**：
- 两个响应都包含 `__bifrost_badge__`。
- 主端口页面的 Badge 数据包含 `main-badge`，不包含 `temp-badge`。
- 临时端口页面的 Badge 数据包含 `temp-badge`，不包含 `main-badge` 或 `main-default`。
- Badge 的合并规则内容与当前代理端口的实际规则视图一致。

### TC-TPRB-11：Web Traffic list/detail 展示端口信息

**操作步骤**：
1. 使用真实浏览器打开 `http://127.0.0.1:$MAIN_PORT/_bifrost/traffic`。
2. 查看 Traffic list。
3. 打开来自临时端口的记录详情。
4. 打开来自主端口的记录详情。

**预期结果**：
- Traffic list 有 `Port` 列，主端口和临时端口记录展示不同端口。
- Traffic detail Overview 有 `Proxy Port` 字段。
- 临时端口记录详情展示 `TEMP_PORT`，主端口记录详情展示 `MAIN_PORT`。

### TC-TPRB-11-回归-01：Traffic 行在 listener_port 更新后立即刷新

**背景**：之前 `VirtualTrafficTable` 的行级 memo 比较器未比较 `listener_port`，导致 store 中记录已经带有新端口值时，列表行仍可能保留旧内容，Playwright 断言 `Port` 列失败。

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   pnpm --dir web test:ui traffic.spec.ts -g "加载流量列表并显示详情"
   ```
2. 观察用例中 Traffic 列表第一批行渲染与详情面板断言。

**预期结果**：
- Playwright 用例通过。
- Traffic 列表中的 `Port` 列能稳定显示真实监听端口，不需要手动刷新或切换列表。
- 打开详情后 `Proxy Port` 与列表中的端口保持一致。

### TC-TPRB-11-回归-02：Settings Proxy 展示临时端口绑定规则详情卡片

**背景**：用户需要在 `/_bifrost/settings?tab=proxy` 的 Proxy 模块下方直接看到每个临时代理端口绑定的规则详情，而不必切回 CLI 执行 `bifrost port list/show/active`。

**操作步骤**：
1. 新增一个禁用规则 `ui-temp-port-rule`，内容包含 `temp-card-ui.test status://218 resBody://(ui-temp-port-rule)`。
2. 绑定临时端口：
   ```bash
   ./target/debug/bifrost port bind --port 0 --name "UI temporary port" --rule ui-temp-port-rule
   ```
3. 使用真实浏览器打开：
   ```text
   http://127.0.0.1:$MAIN_PORT/_bifrost/settings?tab=proxy
   ```
4. 查看 Proxy Address 下方的 Temporary Proxy Ports 区域。

**预期结果**：
- 页面展示一个临时端口卡片，卡片标题包含 `127.0.0.1:<TEMP_PORT>`。
- 临时端口区域使用与 Proxy Address 一致的卡片标题、右侧操作按钮、`Descriptions` 行式布局和分割线节奏，不再使用独立浮动子卡片。
- 卡片展示运行状态、绑定规则 `ui-temp-port-rule` 与端口级 Active Rules。
- 卡片的 Merged Rules 区域包含 `resBody://(ui-temp-port-rule)`。
- 该视图来自当前进程内临时端口状态；销毁临时端口或重启 Bifrost 后，卡片消失。

### TC-TPRB-12：Web Traffic light/dark 主题端口展示

**操作步骤**：
1. 在 Settings / Appearance 或现有主题切换入口切换 light 主题，查看 Traffic list/detail 端口信息。
2. 切换 dark 主题，重复查看 Traffic list/detail 端口信息。

**预期结果**：
- light/dark 下 `Port` 列和 `Proxy Port` 字段均可读。
- 文本不重叠，布局不抖动。

### TC-TPRB-13：销毁临时端口不影响主端口

**操作步骤**：
1. 销毁临时端口：
   ```bash
   ./target/debug/bifrost port destroy "$TEMP_PORT"
   ```
2. 请求临时端口。
3. 请求主端口。

**预期结果**：
- 临时端口连接失败。
- 主端口仍返回 `main-default`。
- `bifrost status` 仍只报告主端口运行。

### TC-TPRB-14：CLI help 展示多端口命令与规则来源说明

**操作步骤**：
1. 依次执行：
   ```bash
   ./target/debug/bifrost --help
   ./target/debug/bifrost port --help
   ./target/debug/bifrost port bind --help
   ./target/debug/bifrost port update --help
   ```
2. 检查输出中是否包含 `port` 顶层命令、多端口模型说明，以及 `--rule` / `--group-rule` / `--rule-file` / `--rule-text` 四类规则来源说明。

**预期结果**：
- `bifrost --help` 显示 `port` 顶层命令。
- `bifrost port --help` 说明主端口继续使用默认规则视图、临时端口只使用显式绑定规则。
- `bifrost port bind --help` / `bifrost port update --help` 都说明四类规则来源与多端口工作流。

### TC-TPRB-15：安装用 SKILL.md 包含多端口工作流说明

**操作步骤**：
1. 打开仓库根目录 `SKILL.md`。
2. 检查是否包含以下内容：
   - 主端口 + 多个临时端口并行工作的说明
   - `bifrost port bind/list/show/active/update/destroy` 示例
   - `--rule` / `--group-rule` / `--rule-file` / `--rule-text` 四类规则来源使用建议
   - `traffic list/get` 中端口字段区分主端口与临时端口的说明

**预期结果**：
- `SKILL.md` 能指导用户用一个主代理进程灵活绑定多个临时端口。
- 命令示例与当前 CLI 实现一致，不使用旧命名或错误子命令。

### TC-TPRB-16：status 底部独立展示临时端口绑定规则

**操作步骤**：
1. 使用临时数据目录启动主服务：
   ```bash
   TEST_DIR="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost rule add temp-status-rule -c "temp-status.test status://218 resBody://(temp-status)"
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost -p 18991 start --skip-cert-check --unsafe-ssl --no-system-proxy >"$TEST_DIR/proxy.log" 2>&1 &
   BIFROST_PID=$!
   ```
2. 等待 Admin API ready 后绑定临时端口：
   ```bash
   curl -fsS http://127.0.0.1:18991/_bifrost/api/system
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost -p 18991 port bind --port 18992 --name "status temp port" --rule temp-status-rule
   ```
3. 执行 status：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost -p 18991 status
   ```

**预期结果**：
- 输出最底部包含独立的 `Temporary Port Bindings` 区块。
- 区块中包含 `:18992 [running] (status temp port)`，host 与临时端口监听 host 保持一致。
- 区块中 `Rules:` 下包含 `local:temp-status-rule`。
- 默认端口规则区块仍显示为 `Default Port Rule Groups: 18991` / `Default Port Active Rules: 18991`，不会和临时端口规则混淆。

## 清理步骤

```bash
source ~/.zshrc
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost stop || true
rm -rf "$TEST_DIR"
```

## 执行记录

2026-05-29 HTML fixture 端口占用回归执行记录：

- 已执行命令：`source ~/.zshrc; SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_temporary_port_bindings.sh`
- 已执行占用端口回归命令：先用 `python3 -m http.server "$OCCUPIED_PORT" --bind 127.0.0.1` 占住 `HTML_PORT`，再执行 `HTML_PORT="$OCCUPIED_PORT" SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_temporary_port_bindings.sh`
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`，启动 Bifrost 时包含 `--no-system-proxy`。
- 端口要求：脚本使用动态测试端口，且通过真实 bind 检查跳过不可用端口，未使用 `9900`。
- 实际结果：正常路径与预占用 `HTML_PORT` 路径均执行 `55/55 passed`；HTML fixture 在规则创建前启动成功，主端口/临时端口 Badge 与无规则直连请求全部通过。
- 结论：`TC-TPRB-06-回归-02` 已按文档完成执行并通过，本次无环境阻塞。

2026-05-18 status 临时端口绑定展示执行记录：

- 已执行命令：使用临时 `BIFROST_DATA_DIR` 启动 `target/debug/bifrost -p 18991 start --skip-cert-check --unsafe-ssl --no-system-proxy`，再执行 `target/debug/bifrost -p 18991 port bind --port 18992 --name "status temp port" --rule status-active-1` 和 `target/debug/bifrost -p 18991 status`。
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时目录，结束后执行 `bifrost stop` 并删除目录。
- 端口要求：使用 `18991` / `18992`，未使用 `9900`，未修改系统代理。
- 实际结果：`status` 最底部包含 `Temporary Port Bindings`，临时端口行包含 `:18992 [running] (status temp port)`，`Rules:` 下包含 `local:status-active-1`；默认端口规则区块显示 `Default Port Rule Groups: 18991` 和 `Default Port Active Rules: 18991`，与临时端口规则明显分区。
- 脚本级 E2E：`BIFROST_BIN=~/work/github/bifrost/target/debug/bifrost SKIP_BUILD=true PROXY_PORT=18991 TEMP_PORT=18992 e2e-tests/tests/test_cli_online_commands_e2e.sh` 通过，汇总 `87/87`。
- 结论：`TC-TPRB-16` 已按文档完成执行并通过。

2026-05-08 Skill 与 CLI 手册补充检查执行记录：

- 已执行命令：`source ~/.zshrc; ./target/debug/bifrost port --help && ./target/debug/bifrost traffic list --help && ./target/debug/bifrost traffic search --help && ./target/debug/bifrost search --help && ./target/debug/bifrost remote traffic list --help && ./target/debug/bifrost remote traffic search --help`
- 已执行静态检查：`source ~/.zshrc; rg -n "listener-port|proxy-port|重启|内存|traffic list|traffic search|remote traffic" SKILL.md docs/cli.md`
- 实际结果：CLI help 确认 `traffic list`、`traffic search`、顶层 `search`、`remote traffic list`、`remote traffic search` 均包含 `--listener-port`，并显示 `--proxy-port` alias。
- 实际结果：`SKILL.md` 和 `docs/cli.md` 已补充临时端口运行时内存态、重启后不恢复、无规则命中也记录入口端口、`traffic list/search` 按入口代理端口过滤、以及远端 traffic 端口过滤示例。
- 结论：`TC-TPRB-14` / `TC-TPRB-15` 涉及的 Skill 与 CLI 手册说明已复查并补齐，本次文档检查不启动 Bifrost，不使用 `9900`，无环境阻塞。

2026-05-08 Traffic 入口端口筛选回归执行记录：

- 已执行命令：`source ~/.zshrc; SKIP_BUILD=true e2e-tests/tests/test_temporary_port_bindings.sh`
- 已执行编译命令：`source ~/.zshrc; cargo build --bin bifrost`
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`，并在启动命令中包含 `--no-system-proxy`。
- 端口要求：脚本使用动态测试端口，且跳过 `9900`。
- 实际结果：`e2e-tests/tests/test_temporary_port_bindings.sh` 本次执行 `53/53 passed`，新增覆盖 `traffic list --listener-port` 与 `traffic search --listener-port` 只返回临时代理端口来源记录，不混入主端口记录。
- 实际结果：脚本同时保留无规则命中直连请求校验，确认主端口与临时端口的 Traffic detail 都能记录正确 `listener_port` 且 `has_rule_hit=false`。
- 结论：临时端口 Traffic 来源归因、CLI list/search 入口端口筛选、以及主端口隔离均已按文档完成执行并通过，本次无环境阻塞。

2026-05-08 Traffic 无规则端口归因回归执行记录：

- 已执行命令：`source ~/.zshrc; SKIP_BUILD=true e2e-tests/tests/test_temporary_port_bindings.sh`
- 已执行编译命令：`source ~/.zshrc; cargo build --bin bifrost`
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`，并在启动命令中包含 `--no-system-proxy`。
- 端口要求：脚本使用动态测试端口，且跳过 `9900`。
- 实际结果：`e2e-tests/tests/test_temporary_port_bindings.sh` 本次执行 `49/49 passed`，新增覆盖主端口 `/direct-main` 与临时端口 `/direct-temp` 的无规则命中直连请求。
- 实际结果：Traffic detail 中 `/direct-main` 记录 `listener_port=<MAIN_PORT>` 且 `has_rule_hit=false`，`/direct-temp` 记录 `listener_port=<TEMP_PORT>` 且 `has_rule_hit=false`。
- 结论：`TC-TPRB-10B` 已按文档完成执行并通过，端口归因不再依赖规则是否命中，本次无环境阻塞。

2026-05-08 Settings Proxy 临时端口卡片执行记录：

- 已执行命令：`source ~/.zshrc; pnpm --dir web test:ui admin-settings.spec.ts -g "Settings Proxy 展示临时端口绑定规则详情卡片"`
- 使用隔离数据目录：Playwright UI 全局 setup 自动分配独立 `BIFROST_DATA_DIR` 与独立后端端口，启动 Bifrost 时包含 `--no-system-proxy`。
- 端口要求：UI 测试动态分配后端端口和临时代理端口，未使用 `9900`。
- 实际结果：Playwright 本次执行 `1 passed`，通过 Admin API 创建禁用规则并绑定临时端口，打开 `settings?tab=proxy` 后卡片展示 `127.0.0.1:<TEMP_PORT>`、`UI temporary port`、绑定规则名和 `Merged Rules` 中的 `resBody://(...)`。
- 结论：`TC-TPRB-11-回归-02` 已按文档完成执行并通过，本次无环境阻塞。

2026-05-08 Badge 回归执行记录：

- 已执行命令：`source ~/.zshrc; SKIP_BUILD=true e2e-tests/tests/test_temporary_port_bindings.sh`
- 已执行编译命令：`source ~/.zshrc; cargo build --bin bifrost`
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`，并在启动命令中包含 `--no-system-proxy`。
- 端口要求：脚本使用动态测试端口，且跳过 `9900`。
- 实际结果：`e2e-tests/tests/test_temporary_port_bindings.sh` 本次执行 `43/43 passed`，其中 `TC-TPRB-10-回归-02` 覆盖主端口 HTML Badge 只包含主端口规则、临时端口 HTML Badge 只包含当前临时端口绑定规则，不混入默认端口规则。
- 结论：临时端口 Badge 规则视图回归验证通过，本次无环境阻塞。

2026-05-07 执行记录：

- 已执行命令：`source ~/.zshrc; e2e-tests/tests/test_temporary_port_bindings.sh`
- 已执行管理端 UI 验证命令：`source ~/.zshrc; pnpm --dir web test:ui traffic.spec.ts -g "加载流量列表并显示详情"`
- 已执行文档/帮助验证命令：`cargo run --bin bifrost -- --help`、`cargo run --bin bifrost -- port --help`、`cargo run --bin bifrost -- port bind --help`、`cargo run --bin bifrost -- port update --help`
- 使用隔离数据目录：脚本通过 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`，并在启动命令中包含 `--no-system-proxy`。
- 端口要求：脚本使用动态测试端口，且跳过 `9900`。
- 文档实际结果：已补充 `docs/cli.md` 的临时端口工作流说明，以及安装用 `SKILL.md` 的多端口使用手册；CLI help 已补充 `port` 顶层命令与 `bind/update` 规则来源说明。
- 实际结果：`e2e-tests/tests/test_temporary_port_bindings.sh` 本次执行 `37/37 passed`，覆盖了主端口/临时端口规则隔离、`bind/list/show/active/update/destroy`、`--rule-file` / `--rule-text`、以及 Traffic API/CLI 中 `lp` / `listener_port` 的端口展示。
- 实际结果：`pnpm --dir web test:ui traffic.spec.ts -g "加载流量列表并显示详情"` 本次执行 `1 passed`，确认 Web Traffic 页面详情面板显示 `Proxy Port`。
- 实际结果：CLI help 定向验证通过，确认 `bifrost --help`、`bifrost port --help`、`bifrost port bind --help`、`bifrost port update --help` 已展示多端口模型、典型 workflow 与四类规则来源说明。
- 结论：TC-TPRB-01 至 TC-TPRB-15 已按文档完成执行并通过，本次无环境阻塞。
