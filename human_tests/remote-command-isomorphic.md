# Remote Command 同构化真实场景测试

## 功能模块说明

本次改造把以下命令纳入同构化范围，并要求在真实场景下确认“本地与远端的命令语义、默认值、参数行为、格式输出、流式输出、拒绝路径”均符合预期：

- `bifrost search`
- `bifrost traffic search`
- `bifrost traffic list`
- `bifrost traffic get`
- `bifrost traffic clear`
- `bifrost remote search`
- `bifrost remote traffic search`
- `bifrost remote traffic list`
- `bifrost remote traffic get`

本文件是本次改造的主验证文档，要求逐条执行，不允许跳过。

## 前置条件

1. 仓库位于 `<REPO_ROOT>`
2. 测试端口禁止使用 `9900`
3. 启动目标 Bifrost（target client）时必须使用临时数据目录，且带 `--no-system-proxy`
4. 远程调用使用独立 caller 数据目录，避免污染已有 remote 连接状态
5. 需要一套可稳定复现的 seed traffic，覆盖：
   - GET / POST / PUT / DELETE
   - 2xx / 4xx / 5xx
   - JSON body
   - 不同 path
   - 至少一条能命中 `host/path/content-type/method/status` 过滤器的记录

建议环境：

```bash
export TARGET_DATA_DIR="$(mktemp -d /tmp/bifrost-target-XXXXXX)"
export CALLER_DATA_DIR="$(mktemp -d /tmp/bifrost-caller-XXXXXX)"
export RELAY_DATA_DIR="$(mktemp -d /tmp/bifrost-relay-XXXXXX)"
export BIFROST_TEST_PORT=8800
export BIFROST_RELAY_PORT=8686
```

建议启动命令：

```bash
BIFROST_DATA_DIR="$TARGET_DATA_DIR" cargo run --bin bifrost -- start -p "$BIFROST_TEST_PORT" --unsafe-ssl --no-system-proxy
pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p "$BIFROST_RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
```

建议 seed traffic：

```bash
MARKER_SEARCH="iso-search-$(date +%s)"
MARKER_BODY="iso-body-$(date +%s)"

curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/get?marker=${MARKER_SEARCH}"
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/status/404?marker=${MARKER_SEARCH}"
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/status/500?marker=${MARKER_SEARCH}"
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/post" -X POST -H "Content-Type: application/json" -d "{\"marker\":\"${MARKER_BODY}\",\"kind\":\"json\"}"
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/put" -X PUT -d "marker=${MARKER_BODY}"
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/delete" -X DELETE
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "https://httpbin.org/get?marker=${MARKER_SEARCH}" -k
curl -x "http://127.0.0.1:${BIFROST_TEST_PORT}" "http://httpbin.org/headers?marker=${MARKER_SEARCH}"
```

## 命令与参数覆盖矩阵

### 1. 搜索类命令覆盖

| 命令 | 必测参数/行为 | 对应用例 |
|------|---------------|---------|
| `search` | `keyword`、默认 summary 输出、默认 limit/max_results/max_scan | TC-RCI-01 |
| `traffic search` | 与顶层 `search` 结果一致 | TC-RCI-02 |
| `search` | `--url --headers --body --req-header --res-header --req-body --res-body` | TC-RCI-03 |
| `search` | `--status --method --host --path --protocol --content-type --domain` | TC-RCI-04 |
| `search` | `--format table/compact/json/json-pretty`、`--no-color` | TC-RCI-05 |
| `search` | `--max-scan --max-results` | TC-RCI-06 |
| `search` | `--interactive` 与“不传 keyword 自动进入交互” | TC-RCI-07 |
| `remote search` | `keyword`、默认 summary 输出、默认 limit/max_results/max_scan | TC-RCI-15 |
| `remote traffic search` | 与 `remote search` 结果一致 | TC-RCI-16 |
| `remote search` | `--url --headers --body --req-header --res-header --req-body --res-body` | TC-RCI-17 |
| `remote search` | `--status --method --host --path --protocol --content-type --domain` | TC-RCI-18 |
| `remote search` | `--format table/compact/json/json-pretty`、`--no-color` | TC-RCI-19 |
| `remote search` | `--max-scan --max-results` | TC-RCI-20 |
| `remote search` | 流式输出首帧在命令完成前到达 | TC-RCI-21 |
| `remote search` | 仅过滤条件、无 keyword 的合法查询 | TC-RCI-29 |

### 2. 流量列表/详情/清理命令覆盖

| 命令 | 必测参数/行为 | 对应用例 |
|------|---------------|---------|
| `traffic list` | 默认输出、默认 `limit=50`、默认 `direction=backward` | TC-RCI-08 |
| `traffic list` | `--cursor --direction --method --status --status-min --status-max --protocol --host --url --path --content-type --client-ip --client-app --has-rule-hit --is-websocket --is-sse --is-tunnel` | TC-RCI-09 |
| `traffic list` | `--format table/compact/json/json-pretty`、`--no-color` | TC-RCI-10 |
| `traffic get` | 通过 `id/seq` 获取详情、默认 `json-pretty` | TC-RCI-11 |
| `traffic get` | `--request-body --response-body --format table/compact/json/json-pretty` | TC-RCI-12 |
| `traffic get` | 省略 `id` 时交互选择 | TC-RCI-13 |
| `traffic clear` | `--ids` | TC-RCI-14 |
| `traffic clear` | `--yes` 清空全部 | TC-RCI-14 |
| `remote traffic list` | 默认输出、默认 `limit=50`、默认 `direction=backward` | TC-RCI-22 |
| `remote traffic list` | 与本地相同的全量过滤参数 | TC-RCI-23 |
| `remote traffic list` | `--format table/compact/json/json-pretty`、`--no-color` | TC-RCI-24 |
| `remote traffic get` | 通过 `id/seq` 获取详情 | TC-RCI-25 |
| `remote traffic get` | `--request-body --response-body --format table/compact/json/json-pretty --no-color` | TC-RCI-26 |
| `search`/`traffic search` | 机器可读输出不被 update notice 污染 | TC-RCI-30 |

## 测试用例

### TC-RCI-01：本地 `search` 主路径与默认值回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH"
   ```
2. 记录输出中的结果条目数与 summary

**预期结果**：
- 命令成功退出
- 输出包含命中 `MARKER_SEARCH` 的记录
- 输出末尾包含 summary
- 未显式传入参数时，行为等价于默认 `limit=50`、默认 `max_results=100`、默认 `max_scan=10000`

### TC-RCI-02：本地 `traffic search` 与顶层 `search` 结果一致

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic search "$MARKER_SEARCH" --format json
   ```
2. 对比 `results/total_matched/total_searched/has_more`

**预期结果**：
- 两个命令都成功退出
- JSON 结构一致
- 命中结果与统计值一致

### TC-RCI-03：本地 `search` 搜索范围参数回归

**操作步骤**：
1. 依次执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --url --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "Content-Type" --headers --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_BODY" --body --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "application/json" --req-header --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "Server" --res-header --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_BODY" --req-body --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "httpbin" --res-body --format json
   ```

**预期结果**：
- 所有命令成功退出
- 每个 scope 仅命中对应范围内的内容
- 未出现 scope 混淆

### TC-RCI-04：本地 `search` 过滤参数回归

**操作步骤**：
1. 依次执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --status 2xx --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --method GET --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --host httpbin.org --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --path /get --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --protocol HTTPS --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_BODY" --content-type json --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --domain httpbin.org --format json
   ```

**预期结果**：
- 每个过滤器都成功生效
- 结果只包含符合过滤条件的记录

### TC-RCI-05：本地 `search` 格式输出与 `--no-color` 回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format table
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format compact
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format json-pretty
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format table --no-color
   ```

**预期结果**：
- 各格式都能成功输出
- `json/json-pretty` 为合法 JSON
- `--no-color` 输出不包含 ANSI 颜色码

### TC-RCI-06：本地 `search --max-scan --max-results` 回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --max-scan 5 --max-results 2 --format json
   ```

**预期结果**：
- 命令成功退出
- 返回结果数不超过 2
- summary/JSON 中体现受限结果集

### TC-RCI-07：本地 `search --interactive` 与无 keyword 自动交互回归

**操作步骤**：
1. 在 TTY 中执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search --interactive
   ```
2. 再执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search
   ```

**预期结果**：
- 两次都进入交互式 TUI，而不是直接报错
- 搜索输入框可输入关键词并看到结果区

### TC-RCI-08：本地 `traffic list` 默认值回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list
   ```

**预期结果**：
- 命令成功退出
- 默认 `limit=50`
- 默认 `direction=backward`
- 输出包含 START/STATUS/METHOD/PROTO/HOST/PATH/SIZE/TIME/SEQ

### TC-RCI-09：本地 `traffic list` 全量过滤参数回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list \
     --limit 5 \
     --cursor 1 \
     --direction forward \
     --method POST \
     --status-min 200 \
     --status-max 299 \
     --protocol https \
     --host httpbin.org \
     --url /post \
     --path /post \
     --content-type application/json \
     --client-ip 127.0.0.1 \
     --client-app curl \
     --has-rule-hit false \
     --is-websocket false \
     --is-sse false \
     --is-tunnel false \
     --format json
   ```

**预期结果**：
- 命令成功退出
- JSON 合法
- 所有过滤条件都被接受并生效

### TC-RCI-10：本地 `traffic list` 格式输出与 `--no-color` 回归

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list --format table
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list --format compact
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list --format json-pretty
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic list --format table --no-color
   ```

**预期结果**：
- 各格式都可用
- `--no-color` 不输出 ANSI 颜色码

### TC-RCI-11：本地 `traffic get` 通过 `id/seq` 获取详情回归

**操作步骤**：
1. 取一条记录的 `id` 与 `seq`
2. 分别执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <id>
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <seq>
   ```

**预期结果**：
- 两个命令都成功退出
- 输出指向同一条记录
- 默认格式为 `json-pretty`

### TC-RCI-12：本地 `traffic get` body 参数与格式输出回归

**操作步骤**：
1. 选择 POST 记录，执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <seq> --request-body --response-body --format table
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <seq> --request-body --response-body --format compact
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <seq> --request-body --response-body --format json
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get <seq> --request-body --response-body --format json-pretty
   ```

**预期结果**：
- 输出包含 `request_body` 与 `response_body`
- 各格式均成功渲染

### TC-RCI-13：本地 `traffic get` 省略 `id` 交互选择回归

**操作步骤**：
1. 在交互式 TTY 中执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic get
   ```

**预期结果**：
- 命令弹出可选流量记录列表
- 选择后可正常进入详情页输出

### TC-RCI-14：本地 `traffic clear` 子命令回归

**操作步骤**：
1. 执行 `traffic clear --ids "<id1>,<id2>"`
2. 验证指定记录被删除
3. 重新生成一批流量后执行 `traffic clear --yes`
4. 验证总记录数变为 0

**预期结果**：
- `--ids` 仅删除指定记录
- `--yes` 清空全部记录

### TC-RCI-15：远端 `remote search` 主路径与默认值回归

**操作步骤**：
1. 完成 caller 与 target 的 remote connect
2. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 命令成功退出
- 输出包含命中 `MARKER_SEARCH` 的记录
- 默认 summary 行为与本地 `search` 一致

### TC-RCI-16：远端 `remote traffic search` 与 `remote search` 结果一致

**操作步骤**：
1. 分别执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format json --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic search "$MARKER_SEARCH" --format json --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 两个命令都成功退出
- JSON 结构和结果集一致

### TC-RCI-17：远端 `remote search` 搜索范围参数回归

**操作步骤**：
1. 依次执行 `--url --headers --body --req-header --res-header --req-body --res-body` 版本的 `remote search`

**预期结果**：
- 每个 scope 参数都被接受
- 结果范围与本地 `search` 对应参数一致

### TC-RCI-18：远端 `remote search` 过滤参数回归

**操作步骤**：
1. 依次执行带 `--status --method --host --path --protocol --content-type --domain` 的 `remote search`

**预期结果**：
- 每个过滤器都被接受
- 结果与本地同参数 `search` 保持一致

### TC-RCI-19：远端 `remote search` 格式输出与 `--no-color` 回归

**操作步骤**：
1. 分别执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format table --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format compact --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format json --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format json-pretty --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --format table --no-color --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 各格式都成功输出
- `--no-color` 输出不含 ANSI 颜色码

### TC-RCI-20：远端 `remote search --max-scan --max-results` 回归

**操作步骤**：
1. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search "$MARKER_SEARCH" --max-scan 5 --max-results 2 --format json --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 命令成功退出
- 返回结果数不超过 2

### TC-RCI-21：远端 `remote search` 流式输出回归

**操作步骤**：
1. 执行 `remote search`
2. 观察终端输出顺序

**预期结果**：
- 首个结果/进度输出出现在命令完成前
- 不会等到退出后才一次性输出全部结果

### TC-RCI-22：远端 `remote traffic list` 默认值回归

**操作步骤**：
1. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic list --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 命令成功退出
- 默认 `limit=50`
- 默认 `direction=backward`

### TC-RCI-23：远端 `remote traffic list` 全量过滤参数回归

**操作步骤**：
1. 执行带全量过滤参数的 `remote traffic list --format json`

**预期结果**：
- 命令成功退出
- 输出结果与本地同参数 `traffic list` 一致

### TC-RCI-24：远端 `remote traffic list` 格式输出与 `--no-color` 回归

**操作步骤**：
1. 依次执行 `--format table/compact/json/json-pretty` 和 `--no-color`

**预期结果**：
- caller 侧渲染与本地 `traffic list` 一致
- `--no-color` 输出不含 ANSI 颜色码

### TC-RCI-25：远端 `remote traffic get` 通过 `id/seq` 获取详情回归

**操作步骤**：
1. 选择一条记录，分别用 `id` 与 `seq` 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <id> --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <seq> --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 两个命令都成功退出
- 输出指向同一条记录

### TC-RCI-26：远端 `remote traffic get` body 参数与格式输出回归

**操作步骤**：
1. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <seq> --request-body --response-body --format table --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <seq> --request-body --response-body --format compact --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <seq> --request-body --response-body --format json --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic get <seq> --request-body --response-body --format json-pretty --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 请求体、响应体都能返回
- caller 侧渲染与本地 `traffic get` 一致

### TC-RCI-27：远端 `remote traffic clear` 不暴露回归

**操作步骤**：
1. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic --help
   ```

**预期结果**：
- help 包含 `list`、`get`、`search`
- help 不包含 `clear`

### TC-RCI-29：远端 `remote search` 支持仅过滤条件查询

**操作步骤**：
1. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote search --status 5xx --host httpbin.org --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```
2. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote traffic search --status 5xx --host httpbin.org --relay-url "http://127.0.0.1:${BIFROST_RELAY_PORT}"
   ```

**预期结果**：
- 两个命令都成功退出
- CLI 不会因为缺少 positional keyword 直接报参数错误
- 返回结果只包含命中 `5xx` 且 `host=httpbin.org` 的记录

### TC-RCI-30：机器可读输出不被 update notice 污染

**操作步骤**：
1. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" search "$MARKER_SEARCH" --format json | jq '.total_matched'
   ```
2. 执行：
   ```bash
   cargo run --bin bifrost -- -p "$BIFROST_TEST_PORT" traffic search "$MARKER_SEARCH" --format json | jq '.total_matched'
   ```

**预期结果**：
- 两个管道命令都成功退出
- `jq` 能正常解析输出
- stdout 中不出现版本检查或 upgrade 提示文本

## 清理步骤

1. 停止 target Bifrost 与 relay
2. 删除临时目录：
   ```bash
   rm -rf "$TARGET_DATA_DIR" "$CALLER_DATA_DIR" "$RELAY_DATA_DIR"
   ```

## 本次执行结果

测试日期：2026-04-27（本次仅执行 TC-RCI-27）

| 用例编号 | 用例名称 | 结果 | 说明 |
|------|------|------|------|
| TC-RCI-01 | 本地 `search` 主路径与默认值回归 | 待执行 |  |
| TC-RCI-02 | 本地 `traffic search` 与顶层 `search` 结果一致 | 待执行 |  |
| TC-RCI-03 | 本地 `search` 搜索范围参数回归 | 待执行 |  |
| TC-RCI-04 | 本地 `search` 过滤参数回归 | 待执行 |  |
| TC-RCI-05 | 本地 `search` 格式输出与 `--no-color` 回归 | 待执行 |  |
| TC-RCI-06 | 本地 `search --max-scan --max-results` 回归 | 待执行 |  |
| TC-RCI-07 | 本地 `search --interactive` 与无 keyword 自动交互回归 | 待执行 |  |
| TC-RCI-08 | 本地 `traffic list` 默认值回归 | 待执行 |  |
| TC-RCI-09 | 本地 `traffic list` 全量过滤参数回归 | 待执行 |  |
| TC-RCI-10 | 本地 `traffic list` 格式输出与 `--no-color` 回归 | 待执行 |  |
| TC-RCI-11 | 本地 `traffic get` 通过 `id/seq` 获取详情回归 | 待执行 |  |
| TC-RCI-12 | 本地 `traffic get` body 参数与格式输出回归 | 待执行 |  |
| TC-RCI-13 | 本地 `traffic get` 省略 `id` 交互选择回归 | 待执行 |  |
| TC-RCI-14 | 本地 `traffic clear` 子命令回归 | 待执行 |  |
| TC-RCI-15 | 远端 `remote search` 主路径与默认值回归 | 待执行 |  |
| TC-RCI-16 | 远端 `remote traffic search` 与 `remote search` 结果一致 | 待执行 |  |
| TC-RCI-17 | 远端 `remote search` 搜索范围参数回归 | 待执行 |  |
| TC-RCI-18 | 远端 `remote search` 过滤参数回归 | 待执行 |  |
| TC-RCI-19 | 远端 `remote search` 格式输出与 `--no-color` 回归 | 待执行 |  |
| TC-RCI-20 | 远端 `remote search --max-scan --max-results` 回归 | 待执行 |  |
| TC-RCI-21 | 远端 `remote search` 流式输出回归 | 待执行 |  |
| TC-RCI-22 | 远端 `remote traffic list` 默认值回归 | 待执行 |  |
| TC-RCI-23 | 远端 `remote traffic list` 全量过滤参数回归 | 待执行 |  |
| TC-RCI-24 | 远端 `remote traffic list` 格式输出与 `--no-color` 回归 | 待执行 |  |
| TC-RCI-25 | 远端 `remote traffic get` 通过 `id/seq` 获取详情回归 | 待执行 |  |
| TC-RCI-26 | 远端 `remote traffic get` body 参数与格式输出回归 | 待执行 |  |
| TC-RCI-27 | 远端 `remote traffic clear` 不暴露回归 | 通过 | 执行 `HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 PROXY_PORT=18080 HTTP_PORT=18081 HTTPS_PORT=18082 RELAY_PORT=18083 bash e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`；确认 `remote traffic --help` 包含 list/get/search 且不包含 clear，脚本汇总 35 passed / 0 failed |
| TC-RCI-29 | 远端 `remote search` 支持仅过滤条件查询 | 待执行 |  |
| TC-RCI-30 | 机器可读输出不被 update notice 污染 | 待执行 |  |
| TC-RCI-31 | CI shell shard 复用预构建 bifrost 二进制 | 通过 | 2026-05-02 执行 `cargo build --release --bin bifrost` 后运行 `BIFROST_E2E_REPORT_DIR=/tmp/bifrost-e2e-shell-shard2-skip-build BIFROST_E2E_SHELL_JOBS=16 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_HTTP_RETRIES=2 TIMEOUT=90 bash scripts/ci/local-ci.sh --skip-static --e2e-only shell --shard 2/3`，验证 `test_search_traffic_cli_isomorphic_e2e.sh` 在 `SKIP_BUILD=true` 下不再重建 release 二进制，shard 2/3 全部通过 |
