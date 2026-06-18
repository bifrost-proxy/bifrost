# Traffic 管理 API 测试用例

## 功能模块说明

验证 Bifrost Traffic 管理 API 的完整功能，包括流量记录的列表查询、筛选过滤、增量更新、详情查看（含请求/响应头部及计时信息）、请求/响应体获取、WebSocket 帧查看、SSE 流式推送等。所有 API 均以 `http://127.0.0.1:8800/_bifrost/api/traffic` 为基础路径。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保端口 8800 可用且服务已正常启动
3. 产生一些流量记录（通过代理发送 HTTP 请求）：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   curl -x http://127.0.0.1:8800 -X POST http://httpbin.org/post -d '{"key":"value"}' -H "Content-Type: application/json"
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/404
   ```

---

## 测试用例

### TC-ATR-01：获取流量列表（默认参数）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/traffic | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含：
  - `records`: 数组，包含流量记录摘要
  - `total`: 整数，表示总记录数
  - `has_more`: 布尔值，表示是否有更多记录
- 每条记录至少包含 `id`、`method`、`url`、`status`、`host` 等字段
- 默认返回最多 100 条记录

---

### TC-ATR-02：按 method 筛选流量

**前置条件**：已通过前置步骤产生 GET 和 POST 请求

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?method=POST" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录数组中，所有记录的 `method` 字段均为 `"POST"`
- 验证过滤结果：
  ```bash
  curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?method=POST" | jq '.records[].method' | sort -u
  ```
  输出仅包含 `"POST"`

---

### TC-ATR-03：按 status 筛选流量

**前置条件**：已通过前置步骤产生不同状态码的请求

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?status=404" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `status` 字段均为 `404`

---

### TC-ATR-04：按 host 筛选流量

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?host=httpbin" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `host` 字段包含 `"httpbin"` 子串

---

### TC-ATR-05：按 url 筛选流量

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?url=/get" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `url` 字段包含 `"/get"` 子串

---

### TC-ATR-06：按 content_type 筛选流量

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?content_type=json" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `content_type` 字段包含 `"json"` 子串

---

### TC-ATR-07：按 protocol 筛选流量

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?protocol=HTTP" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `protocol` 字段包含 `"HTTP"`

---

### TC-ATR-08：按 has_rule_hit 筛选流量

**操作步骤**：
1. 执行以下命令（筛选未命中规则的流量）：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?has_rule_hit=false" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录均为未命中任何规则的流量

---

### TC-ATR-09：按 client_ip 筛选流量

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?client_ip=127.0.0.1" | jq '.records | length'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录中，所有记录的 `client_ip` 字段包含 `"127.0.0.1"`

---

### TC-ATR-10：使用 limit 和 offset（cursor）分页

**操作步骤**：
1. 先获取第一页（2 条）：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?limit=2" | jq '{total: .total, count: (.records | length), has_more: .has_more}'
   ```
2. 如果有更多记录，使用返回的 `server_sequence` 作为 cursor 获取下一页：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?limit=2&cursor=<server_sequence>&direction=forward" | jq '{count: (.records | length), has_more: .has_more}'
   ```

**预期结果**：
- 第一页：返回 2 条记录，`has_more` 为 `true`（如果总记录数大于 2）
- 第二页：返回后续记录，分页正常工作

---

### TC-ATR-11：获取增量更新

**操作步骤**：
1. 先获取当前流量列表，记录 `server_sequence` 值：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?limit=1" | jq .server_sequence
   ```
2. 使用 `after_seq` 参数获取增量更新：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/updates?after_seq=<server_sequence>" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含：
  - `new_records`: 数组，包含自指定 sequence 之后的新记录
  - `updated_records`: 数组，包含已更新的记录
  - `has_more`: 布尔值
  - `server_total`: 整数
  - `server_sequence`: 整数

---

### TC-ATR-12：获取流量详情（含 headers 和 timing 信息）

**前置条件**：已产生至少一条流量记录

**操作步骤**：
1. 先获取一条流量记录的 ID：
   ```bash
   TRAFFIC_ID=$(curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?limit=1" | jq -r '.records[0].id')
   echo $TRAFFIC_ID
   ```
2. 获取流量详情：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含完整的流量详情：
  - `id`: 与请求的 ID 一致
  - `method`: HTTP 方法（如 `"GET"`）
  - `url`: 请求完整 URL
  - `host`: 目标主机
  - `status`: HTTP 状态码
  - `request_headers`: 请求头数组
  - `response_headers`: 响应头数组
  - `timing`: 计时信息对象（包含请求开始时间、连接时间等）
  - `request_size`: 请求体大小
  - `response_size`: 响应体大小
  - `content_type`: 响应内容类型

---

### TC-ATR-13：获取请求体

**前置条件**：已产生至少一条带请求体的流量记录（如 POST 请求）

**操作步骤**：
1. 先获取 POST 请求的流量 ID：
   ```bash
   TRAFFIC_ID=$(curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?method=POST&limit=1" | jq -r '.records[0].id')
   echo $TRAFFIC_ID
   ```
2. 获取请求体：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/request-body" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含：
  - `success`: `true`
  - `data`: 请求体内容字符串（如之前发送的 `{"key":"value"}`）

---

### TC-ATR-14：获取响应体

**前置条件**：已产生至少一条流量记录

**操作步骤**：
1. 先获取一条流量记录的 ID：
   ```bash
   TRAFFIC_ID=$(curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?limit=1" | jq -r '.records[0].id')
   echo $TRAFFIC_ID
   ```
2. 获取响应体：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/response-body" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含：
  - `success`: `true`
  - `data`: 响应体内容字符串

---

### TC-ATR-15：清除所有流量记录

**前置条件**：已有至少一条流量记录

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/traffic | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"All traffic data cleared successfully"`

**验证步骤**：
1. 重新查询流量列表：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/traffic | jq .total
   ```
2. 确认 `total` 为 `0`（或仅包含正在进行的活跃连接）

---

### TC-ATR-16：获取 WebSocket 帧列表

**前置条件**：需要通过代理建立一个 WebSocket 连接并产生帧数据

**操作步骤**：
1. 通过代理建立 WebSocket 连接（使用 websocat 或类似工具）：
   ```bash
   echo "hello" | websocat --ws-c-uri=ws://echo.websocket.org -1 ws://127.0.0.1:8800
   ```
   （如果 websocat 不可用，可使用任意 WebSocket 客户端通过代理连接）
2. 获取该 WebSocket 连接的流量 ID：
   ```bash
   TRAFFIC_ID=$(curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?is_websocket=true&limit=1" | jq -r '.records[0].id')
   echo $TRAFFIC_ID
   ```
3. 获取 WebSocket 帧列表：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/frames" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 数组（或对象），包含 WebSocket 帧数据
- 每帧包含帧类型、方向、数据内容等信息

---

### TC-ATR-17：获取 WebSocket 单帧详情

**前置条件**：已通过 TC-ATR-16 获取到 WebSocket 帧列表

**操作步骤**：
1. 使用帧 ID 获取单帧详情：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/frames/1" | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象，包含该帧的完整信息（帧类型、方向、数据载荷等）

---

### TC-ATR-18：订阅 WebSocket 帧流（SSE）

**前置条件**：存在一个活跃的 WebSocket 连接

**操作步骤**：
1. 使用 curl 订阅帧流（设置超时避免无限等待）：
   ```bash
   curl -s --max-time 5 "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/frames/stream"
   ```

**预期结果**：
- HTTP 状态码 200
- Content-Type 为 `text/event-stream`
- 返回 SSE 格式的流式数据（如果有新帧产生，会实时推送）
- 数据格式为 `id: <seq>\ndata: <json>\n\n`

---

### TC-ATR-19：订阅 SSE 流式事件

**前置条件**：需要通过代理访问一个 SSE 端点产生流量记录

**操作步骤**：
1. 通过代理访问一个 SSE 端点（如果有可用的 SSE 服务）：
   ```bash
   curl -x http://127.0.0.1:8800 --max-time 3 http://sse-test-server/events 2>/dev/null
   ```
2. 获取 SSE 流量记录的 ID：
   ```bash
   TRAFFIC_ID=$(curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?is_sse=true&limit=1" | jq -r '.records[0].id')
   echo $TRAFFIC_ID
   ```
3. 订阅 SSE 流式事件：
   ```bash
   curl -s --max-time 5 "http://127.0.0.1:8800/_bifrost/api/traffic/$TRAFFIC_ID/sse/stream"
   ```

**预期结果**：
- HTTP 状态码 200
- Content-Type 为 `text/event-stream`
- 返回 SSE 格式的流式数据，每个事件包含：
  - `seq`: 序列号
  - `ts`: 时间戳
  - `data`: 事件数据内容
  - `event`: 事件类型（如有）
- 连接关闭后会收到 `event: finish` 的合成结束事件

---

### TC-ATR-20：获取不存在的流量记录返回 404

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" http://127.0.0.1:8800/_bifrost/api/traffic/nonexistent-id-12345
   ```

**预期结果**：
- HTTP 状态码 404
- 返回 JSON 包含错误信息 `"Traffic record 'nonexistent-id-12345' not found"`

---

### TC-ATR-21：获取不存在记录的请求体返回 404

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" http://127.0.0.1:8800/_bifrost/api/traffic/nonexistent-id-12345/request-body
   ```

**预期结果**：
- HTTP 状态码 404
- 返回 JSON 包含错误信息 `"Traffic record 'nonexistent-id-12345' not found"`

---

### TC-ATR-22：获取不存在记录的响应体返回 404

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" http://127.0.0.1:8800/_bifrost/api/traffic/nonexistent-id-12345/response-body
   ```

**预期结果**：
- HTTP 状态码 404
- 返回 JSON 包含错误信息 `"Traffic record 'nonexistent-id-12345' not found"`

---

### TC-ATR-23：组合多个筛选条件查询流量

**前置条件**：已产生多种类型的流量记录

**操作步骤**：
1. 执行以下命令（同时按 method 和 host 筛选）：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/traffic?method=GET&host=httpbin&limit=5" | jq '{total: .total, count: (.records | length)}'
   ```

**预期结果**：
- HTTP 状态码 200
- 返回的记录同时满足所有筛选条件：`method` 为 `GET` 且 `host` 包含 `httpbin`
- 返回记录数不超过 5 条

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
---

### TC-ATR-PR260-01：回归 — malformed clear 请求不会清空全部流量

**关联修复**：PR #260 阻塞问题，`DELETE /_bifrost/api/traffic` 收到非空但非法 JSON body 时必须返回 400，不能退化为 clear-all。

**操作步骤**：
1. 按前置条件启动临时 Bifrost，并产生至少 2 条 traffic record。
2. 记录当前流量数量：
   ```bash
   before=$(curl -s http://127.0.0.1:8800/_bifrost/api/traffic | jq '.records | length')
   ```
3. 发送 malformed clear 请求：
   ```bash
   status=$(curl -s -o /tmp/bifrost-clear-malformed.json -w '%{http_code}' \
  -X DELETE http://127.0.0.1:8800/_bifrost/api/traffic \
  -H 'Content-Type: application/json' \
  -d '{"ids":["broken"')
   ```
4. 再次查询流量数量：
   ```bash
   after=$(curl -s http://127.0.0.1:8800/_bifrost/api/traffic | jq '.records | length')
   ```

**预期结果**：
- `status` 为 `400`。
- `/tmp/bifrost-clear-malformed.json` 中包含非法 JSON 错误信息。
- `after` 与 `before` 一致，已有流量记录没有被清空。

**本次执行记录（2026-06-18）**：
- 已补充单元测试覆盖 `parse_clear_traffic_request_body`：malformed JSON 返回错误、空 body 保持 clear-all 语义、合法 ids 正常解析。
- 受限于本轮修复优先在远端仓库提交，完整临时代理手工链路待 CI/人工环境继续执行。

---

### TC-ATR-PR260-02：回归 — 二进制性能模式仍保留 traffic metadata record

**关联修复**：PR #260 阻塞问题，默认开启 binary traffic performance mode 时，二进制响应可以跳过 body 持久化，但必须保留 traffic metadata record。

**操作步骤**：
1. 按前置条件启动临时 Bifrost，确认 `binary_traffic_performance_mode=true`。
2. 通过代理请求一个返回 `Content-Type: application/octet-stream` 的测试端点。
3. 查询 traffic list，并按 URL 或 content type 定位该请求。
4. 对定位到的记录执行 `traffic get` / export 验证 metadata 可用。

**预期结果**：
- traffic list 中可以看到该二进制请求。
- 记录至少包含 method、url、status、request headers、response headers、timing、content type 等 metadata。
- response body 可以不持久化，但不能整条记录消失。

**本次执行记录（2026-06-18）**：
- 已补充/调整单元测试，确认 metrics-only forwarding 对 binary fast path 返回 false，避免跳过 metadata record 创建。
- 完整代理链路待 CI/人工环境继续执行。
