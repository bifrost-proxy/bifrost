# CLI 流量查看与搜索命令测试用例

## 功能模块说明

本文档覆盖 Bifrost CLI 的流量查看与搜索相关命令，包括：
- `bifrost traffic list` — 列出流量记录（支持多种筛选条件）
- `bifrost traffic get <id>` — 查看流量详情（支持请求体/响应体）
- `bifrost traffic search <keyword>` / `bifrost search <keyword>` — 搜索流量记录（支持作用域与过滤器）
- `bifrost traffic clear` — 清除流量记录
- `bifrost search --interactive` — 交互式 TUI 搜索模式

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 通过代理产生一批流量记录（至少包含不同方法、不同状态码的请求）：
   ```bash
   # GET 请求 — 200
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   # GET 请求 — 404
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/404
   # POST 请求 — 200（带 JSON body）
   curl -x http://127.0.0.1:8800 http://httpbin.org/post -X POST -H "Content-Type: application/json" -d '{"keyword":"bifrost-test-data"}'
   # PUT 请求 — 200
   curl -x http://127.0.0.1:8800 http://httpbin.org/put -X PUT -d 'hello=world'
   # DELETE 请求 — 200
   curl -x http://127.0.0.1:8800 http://httpbin.org/delete -X DELETE
   # GET 请求 — 500
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/500
   # HTTPS GET 请求（需要 --unsafe-ssl 已启用或安装 CA）
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   # GET 请求带特殊路径
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 确保端口 8800 未被其他进程占用

---

## 测试用例

### TC-CTS-01：traffic list 默认列出流量记录

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出表格包含列标题：START、STATUS、METHOD、PROTO、HOST、PATH、SIZE、TIME、SEQ
- 列出前置条件中产生的流量记录
- 默认最多显示 50 条记录
- 底部显示 Total 数量和 ServerSeq 信息

---

### TC-CTS-02：traffic list --method GET 按方法筛选

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --method GET
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出的所有记录 METHOD 列均为 `GET`
- 不包含 POST、PUT、DELETE 等方法的记录

---

### TC-CTS-03：traffic list --status-min 400 按最小状态码筛选

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --status-min 400
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出的所有记录 STATUS 列均 >= 400
- 应至少包含前置条件中产生的 404 和 500 状态码的记录
- 不包含 200 等状态码的记录

---

### TC-CTS-04：traffic list --limit 3 限制返回数量

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --limit 3
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 表格中最多显示 3 条流量记录
- 如果总记录数超过 3 条，底部提示 `... more records available. Use --cursor/--direction to paginate.`

---

### TC-CTS-05：traffic list 多条件组合筛选

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --method GET --host httpbin.org --limit 10
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出的所有记录 METHOD 列均为 `GET` 且 HOST 列包含 `httpbin.org`
- 最多显示 10 条记录

---

### TC-CTS-06：traffic list --format json 输出 JSON 格式

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --format json
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出为合法 JSON 格式
- JSON 中包含 `records` 数组和 `total` 字段
- 可通过 `jq` 工具正常解析：`cargo run --bin bifrost -- -p 8800 traffic list --format json | jq .total`

---

### TC-CTS-07：traffic list --format compact 紧凑输出

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --format compact
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 每条记录以单行紧凑格式输出，包含时间、状态码、方法、主机、路径、序号
- 格式示例：`HH:MM:SS 200 GET httpbin.org/get #123`

---

### TC-CTS-08：traffic get <id> 查看流量详情

**操作步骤**：
1. 先获取一条流量记录的序号：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --limit 1 --format json | jq -r '.records[0].seq'
   ```
2. 使用获得的序号（假设为 `N`）查看详情：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic get N
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 默认以 json-pretty 格式输出完整的流量详情
- 输出包含以下字段：`id`、`method`、`host`、`url`、`path`、`status`、`protocol`、`duration_ms`、`request_size`、`response_size`、`request_headers`、`response_headers`

---

### TC-CTS-09：traffic get <id> --request-body 包含请求体

**操作步骤**：
1. 先找到前置条件中 POST 请求的序号：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --method POST --format json | jq -r '.records[0].seq'
   ```
2. 使用获得的序号查看详情（包含请求体）：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic get <seq> --request-body
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出中包含 `request_body` 字段
- `request_body` 的内容应包含前置条件中发送的 JSON 数据 `{"keyword":"bifrost-test-data"}`

---

### TC-CTS-10：traffic get <id> --response-body 包含响应体

**操作步骤**：
1. 获取一条 GET 请求的序号：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --method GET --format json | jq -r '.records[0].seq'
   ```
2. 使用获得的序号查看详情（包含响应体）：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic get <seq> --response-body
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出中包含 `response_body` 字段
- `response_body` 的内容为目标服务器返回的实际响应数据

---

### TC-CTS-11：traffic get <id> --request-body --response-body 同时包含请求体和响应体

**操作步骤**：
1. 获取 POST 请求的序号
2. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic get <seq> --request-body --response-body
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出中同时包含 `request_body` 和 `response_body` 字段
- 两个字段均有实际内容

---

### TC-CTS-12：traffic get <id> -f table 以表格格式查看详情

**操作步骤**：
1. 获取一条流量记录的序号
2. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic get <seq> -f table
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出为人类可读的详情格式，包含：
  - `── Request Detail ──` 标题行
  - URL、Status、Protocol、Duration、Size、Host、Client 等字段
  - Request Headers 和 Response Headers 列表

---

### TC-CTS-13：traffic search "keyword" 基本关键字搜索

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic search "httpbin"
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出表格包含列标题：SEQ、STATUS、METHOD、PROTO、HOST、PATH、SIZE、TIME
- 搜索结果中所有记录的 HOST 或 PATH 包含 `httpbin`
- 底部显示搜索统计：`Found X matches (scanned Y records, ...)`

---

### TC-CTS-14：search "keyword" 顶层别名搜索

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin"
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出结果与 `traffic search "httpbin"` 完全一致
- `search` 作为 `traffic search` 的顶层别名正常工作

---

### TC-CTS-15：search --method POST 按方法过滤搜索结果

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --method POST
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的 METHOD 列均为 `POST`
- 仅包含前置条件中发送的 POST 请求

---

### TC-CTS-16：search --host httpbin.org 按主机过滤搜索

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "get" --host httpbin.org
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的 HOST 列包含 `httpbin.org`
- 结果中包含关键字 `get` 的匹配

---

### TC-CTS-17：search --path /post 按路径过滤搜索

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --path /post
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的路径包含 `/post`
- 结果数量少于不带 `--path` 过滤时的数量

---

### TC-CTS-18：search --req-header 搜索请求头

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "application/json" --req-header
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索范围仅限于请求头
- 结果应包含前置条件中发送的带 `Content-Type: application/json` 请求头的 POST 请求
- 匹配位置提示显示 `request_headers` 字段

---

### TC-CTS-19：search --res-body 搜索响应体

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin.org" --res-body
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索范围仅限于响应体
- 匹配位置提示显示 `response_body` 字段

---

### TC-CTS-20：search --headers 搜索所有头部（请求头 + 响应头）

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "Content-Type" --headers
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索范围包含请求头和响应头
- 匹配位置提示可能显示 `request_headers` 或 `response_headers`

---

### TC-CTS-21：search --body 搜索所有体（请求体 + 响应体）

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "bifrost-test-data" --body
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索范围包含请求体和响应体
- 结果应包含前置条件中 POST 请求发送的包含 `bifrost-test-data` 关键字的记录
- 匹配位置提示显示 `request_body` 或 `response_body`

---

### TC-CTS-22：search --status 2xx 按状态码范围过滤

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --status 2xx
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的 STATUS 均为 200-299 范围内
- 不包含 404、500 等状态码的记录

---

### TC-CTS-23：search --status 4xx 筛选客户端错误

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --status 4xx
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的 STATUS 均为 400-499 范围内
- 应包含前置条件中产生的 404 记录

---

### TC-CTS-24：search --protocol HTTPS 按协议过滤

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --protocol HTTPS
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的 PROTO 列为 `https`
- 应包含前置条件中通过 HTTPS 发送的请求

---

### TC-CTS-25：search --domain httpbin.org 按域名过滤

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "get" --domain httpbin.org
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的域名匹配 `httpbin.org`
- `--domain` 过滤器正常生效

---

### TC-CTS-26：search --content-type json 按内容类型过滤

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --content-type json
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果中所有记录的响应内容类型包含 `json`
- httpbin.org 返回的 JSON 响应应被匹配

---

### TC-CTS-27：search --format json 以 JSON 格式输出搜索结果

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --format json
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出为合法 JSON 格式
- JSON 中包含 `results` 数组、`total_matched`、`total_searched`、`has_more` 字段
- 每个结果项包含 `id`、`seq`、`method`、`host`、`path`、`status`、`matches` 等字段

---

### TC-CTS-28：search --format compact 紧凑格式输出搜索结果

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --format compact
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 每条搜索结果以单行紧凑格式输出
- 包含序号、状态码、方法、主机、路径信息

---

### TC-CTS-29：search 无匹配结果时的提示

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "absolutely_nonexistent_keyword_xyz_12345"
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 输出提示 `No results found for 'absolutely_nonexistent_keyword_xyz_12345'`
- 显示扫描记录数和搜索范围信息

---

### TC-CTS-30：search --max-scan 和 --max-results 控制搜索范围

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --max-scan 5 --max-results 2
   ```

**预期结果**：
- 命令成功执行，退出码为 0
- 搜索结果最多返回 2 条匹配记录
- 底部统计信息中 scan range 显示为 5，max results 显示为 2

---

### TC-CTS-31：traffic clear --ids 删除指定流量记录

**操作步骤**：
1. 先获取两条记录的 ID：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --limit 2 --format json | jq -r '.records[].id'
   ```
2. 使用获得的 ID（假设为 `id1,id2`）删除指定记录：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic clear --ids "id1,id2"
   ```
3. 再次列出流量记录确认删除：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --format json | jq .total
   ```

**预期结果**：
- 删除命令输出 `Deleted 2 traffic record(s).`
- 再次列出流量记录时，总数减少 2 条
- 被删除的 ID 不再出现在列表中

---

### TC-CTS-32：traffic clear 清除全部流量记录

**操作步骤**：
1. 确认当前有流量记录：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --format json | jq .total
   ```
2. 执行清除全部记录命令（使用 --yes 跳过确认）：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic clear --yes
   ```
3. 再次列出流量记录确认：
   ```bash
   cargo run --bin bifrost -- -p 8800 traffic list --format json | jq .total
   ```

**预期结果**：
- 清除命令输出 `All traffic records cleared.`
- 清除后列出流量记录，total 为 0
- 记录列表为空

---

### TC-CTS-33：search --interactive 交互式 TUI 搜索模式

**前置条件**：先重新产生一些流量数据（如果 TC-CTS-32 已清除）：
```bash
curl -x http://127.0.0.1:8800 http://httpbin.org/get
curl -x http://127.0.0.1:8800 http://httpbin.org/post -X POST -d '{"test":"interactive"}'
```

**操作步骤**：
1. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8800 search "httpbin" --interactive
   ```

**预期结果**：
- 进入全屏 TUI 交互界面
- 顶部显示搜索栏，包含关键字 `httpbin`
- 中间显示搜索结果列表，包含 SEQ、STATUS、METHOD、HOST、PATH、MATCH 列
- 底部显示操作提示：`↑/k ↓/j Navigate │ Enter View │ /,s Search │ r Refresh │ q Quit`
- 使用 `↑`/`↓` 或 `j`/`k` 可上下移动选择
- 按 `Enter` 可查看选中记录的详情视图（含 Overview、Request Headers、Response Headers、Body 四个 Tab）
- 在详情视图中按 `Tab` 可切换 Tab 页
- 按 `Esc` 或 `q` 返回列表 / 退出
- 按 `/` 或 `s` 进入搜索编辑模式，可修改关键字后按 `Enter` 重新搜索

---

### TC-CTS-34：traffic list 服务未启动时的错误提示

**操作步骤**：
1. 停止 Bifrost 服务（或使用一个未运行的端口）
2. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8801 traffic list
   ```

**预期结果**：
- 命令执行失败
- 错误信息包含连接失败提示，类似 `Failed to connect to Bifrost admin API`
- 提示用户启动代理：`Is the proxy server running?` 和 `Hint: Start the proxy with: bifrost start`

---

### TC-CTS-35：search 服务未启动时的错误提示

**操作步骤**：
1. 确保端口 8801 上没有运行 Bifrost 服务
2. 执行命令：
   ```bash
   cargo run --bin bifrost -- -p 8801 search "test"
   ```

**预期结果**：
- 命令执行失败
- 输出包含 `Bifrost proxy is not running on port 8801`
- 提示用户启动代理：`Start it with: bifrost start -p 8801`

---

### TC-CTS-36：traffic get 不存在的记录 ID

**操作步骤**：
1. 执行命令（使用一个不存在的 ID）：
   ```bash
   echo "" | cargo run --bin bifrost -- -p 8800 traffic get nonexistent_id_12345
   ```

**预期结果**：
- 命令执行失败
- 输出包含 `not found` 相关错误提示

---

### TC-CTS-37：traffic list/search 按代理入口端口筛选

**前置条件**：
- 使用隔离数据目录启动 Bifrost，启动参数必须包含 `--no-system-proxy`。
- 准备主端口和一个临时代理端口，并分别产生 traffic 记录。

**操作步骤**：
1. 使用 list 按入口端口筛选：
   ```bash
   cargo run --bin bifrost -- -p "$MAIN_PORT" traffic list --listener-port "$TEMP_PORT" --format json
   ```
2. 使用 search 按入口端口筛选：
   ```bash
   cargo run --bin bifrost -- -p "$MAIN_PORT" traffic search "port" --listener-port "$TEMP_PORT" --format json
   ```
3. 验证别名可解析：
   ```bash
   cargo run --bin bifrost -- -p "$MAIN_PORT" traffic list --proxy-port "$TEMP_PORT" --format json
   cargo run --bin bifrost -- -p "$MAIN_PORT" search "temp-port" --proxy-port "$TEMP_PORT" --format json
   ```

**预期结果**：
- `traffic list --listener-port` 只返回 `lp=$TEMP_PORT` 的记录，不包含主端口记录。
- `traffic search --listener-port` 只返回临时端口记录，不包含主端口记录。
- `--proxy-port` 与 `--listener-port` 行为一致。
- `traffic list --port "$MAIN_PORT"` 仍表示 Admin API 端口，不作为入口端口筛选。

---

## 执行记录

2026-05-08 代理入口端口筛选执行记录：

- 已执行命令：`source ~/.zshrc; cargo test -p bifrost-cli`
- 已执行命令：`source ~/.zshrc; cargo build --bin bifrost`
- 已执行命令：`source ~/.zshrc; SKIP_BUILD=true e2e-tests/tests/test_temporary_port_bindings.sh`
- 实际结果：`cargo test -p bifrost-cli` 全部通过，覆盖 `traffic list --listener-port`、`traffic search --listener-port`、`--proxy-port` 别名、远端 traffic list/search 参数透传。
- 实际结果：临时端口 E2E 在隔离 `BIFROST_DATA_DIR` 和动态端口下执行，Bifrost 启动参数包含 `--no-system-proxy`，未使用 `9900`。
- 实际结果：`traffic list --listener-port "$TEMP_PORT"` 返回 compact JSON 中的 `"lp":$TEMP_PORT`，不包含主端口 `"lp":$MAIN_PORT`；`traffic search "port" --listener-port "$TEMP_PORT"` 返回 `/temp-port`，不包含 `/main-port`。
- 结论：`TC-CTS-37` 已按文档完成执行并通过，本次无环境阻塞。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
