# Replay Admin API 测试用例

## 功能模块说明

Replay Admin API 提供请求重放功能的管理接口，支持创建和管理重放集合（Group）、保存请求模板（Request）、执行重放请求（Execute）、查看重放历史（History）等操作。所有 API 路径前缀为 `/_bifrost/api/replay`。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保端口 8800 未被其他程序占用

---

## 测试用例

### TC-ARP-01：创建重放集合（Group）

**操作步骤**：
1. 使用 curl 创建一个新的重放集合：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/groups \
     -H "Content-Type: application/json" \
     -d '{"name":"测试集合"}'
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体为 JSON 对象，包含：
  - `id`：UUID 格式的集合 ID
  - `name`：`"测试集合"`
  - `parent_id`：`null`
  - `sort_order`：`0`
  - `created_at`：时间戳（毫秒级）
  - `updated_at`：时间戳（毫秒级）

---

### TC-ARP-02：列出所有重放集合

**前置条件**：已通过 TC-ARP-01 创建至少一个集合

**操作步骤**：
1. 使用 curl 获取集合列表：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/replay/groups
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含 `groups` 数组
- 数组中包含之前创建的 `"测试集合"`
- 每个集合对象包含 `id`、`name`、`parent_id`、`sort_order`、`created_at`、`updated_at` 字段

---

### TC-ARP-03：获取单个重放集合详情

**前置条件**：已通过 TC-ARP-01 创建集合，记录其 `id`（下称 `GROUP_ID`）

**操作步骤**：
1. 使用 curl 获取集合详情：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/replay/groups/{GROUP_ID}
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体为该集合的完整 JSON 对象，`name` 为 `"测试集合"`

---

### TC-ARP-04：更新重放集合名称

**前置条件**：已创建集合，记录 `GROUP_ID`

**操作步骤**：
1. 使用 curl 更新集合名称：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/replay/groups/{GROUP_ID} \
     -H "Content-Type: application/json" \
     -d '{"name":"重命名集合"}'
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体中 `name` 为 `"重命名集合"`
- `updated_at` 已更新为新的时间戳

---

### TC-ARP-05：在集合中创建请求

**前置条件**：已创建集合，记录 `GROUP_ID`

**操作步骤**：
1. 使用 curl 创建一个请求模板：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/requests \
     -H "Content-Type: application/json" \
     -d '{
       "group_id": "{GROUP_ID}",
       "name": "获取 httpbin 首页",
       "method": "GET",
       "url": "http://httpbin.org/get",
       "headers": [{"key":"User-Agent","value":"Bifrost-Test","enabled":true}],
       "is_saved": true
     }'
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体为请求对象，包含：
  - `id`：UUID 格式的请求 ID
  - `group_id`：等于传入的 `GROUP_ID`
  - `name`：`"获取 httpbin 首页"`
  - `method`：`"GET"`
  - `url`：`"http://httpbin.org/get"`
  - `is_saved`：`true`
  - `headers`：包含 User-Agent 头

---

### TC-ARP-06：列出集合中的请求

**前置条件**：已通过 TC-ARP-05 创建请求

**操作步骤**：
1. 使用 curl 列出指定集合的请求：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/replay/requests?group_id={GROUP_ID}&saved=true"
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含 `requests` 数组和 `total` 数字、`max_requests` 数字
- 数组中包含之前创建的请求
- `total` >= 1

---

### TC-ARP-07：执行重放请求

**操作步骤**：
1. 使用 curl 执行一个简单的 GET 重放请求：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "http://httpbin.org/get",
       "method": "GET",
       "headers": [["User-Agent","Bifrost-Replay-Test"]]
     }'
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含 `success: true` 和 `data` 对象
- `data` 包含：
  - `traffic_id`：非空字符串
  - `status`：`200`
  - `headers`：响应头数组
  - `body`：包含 httpbin 返回的 JSON 内容
  - `duration_ms`：请求耗时（毫秒）
  - `applied_rules`：空数组（未配置规则时）

---

### TC-ARP-08：执行重放请求并关联 request_id

**前置条件**：已通过 TC-ARP-05 创建请求，记录 `REQUEST_ID`

**操作步骤**：
1. 使用 curl 执行重放并关联 request_id：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "http://httpbin.org/get",
       "method": "GET",
       "headers": [["User-Agent","Bifrost-Replay-Test"]],
       "request_id": "{REQUEST_ID}"
     }'
   ```

**预期结果**：
- 返回 HTTP 200
- `success: true`
- 重放历史记录中 `request_id` 关联到指定请求

---

### TC-ARP-09：查看重放历史列表

**前置条件**：已通过 TC-ARP-07 或 TC-ARP-08 执行过重放

**操作步骤**：
1. 使用 curl 获取重放历史：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/replay/history
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含 `history` 数组、`total` 数字、`max_history` 数字
- 历史记录中包含之前执行的重放
- 每条记录包含 `id`、`request_id`、`traffic_id`、`method`、`url`、`status`、`duration_ms`、`created_at` 等字段

---

### TC-ARP-10：按 request_id 筛选重放历史

**前置条件**：已通过 TC-ARP-08 执行关联请求的重放，记录 `REQUEST_ID`

**操作步骤**：
1. 使用 curl 按 request_id 筛选历史：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/replay/history?request_id={REQUEST_ID}"
   ```

**预期结果**：
- 返回 HTTP 200
- `history` 数组中只包含关联到该 `REQUEST_ID` 的记录
- `total` 等于该请求对应的历史记录数

---

### TC-ARP-11：获取重放统计信息

**操作步骤**：
1. 使用 curl 获取重放统计：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/replay/stats
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含请求数量、历史记录数量等统计信息

---

### TC-ARP-12：删除重放历史记录

**前置条件**：已有至少一条重放历史，记录 `HISTORY_ID`

**操作步骤**：
1. 使用 curl 删除单条历史记录：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/replay/history/{HISTORY_ID}
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含成功消息 "History deleted"
- 再次查询历史列表，该记录不存在

---

### TC-ARP-13：清空指定请求的重放历史

**前置条件**：已有关联到 `REQUEST_ID` 的重放历史

**操作步骤**：
1. 使用 curl 清空该请求的历史：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8800/_bifrost/api/replay/history?request_id={REQUEST_ID}"
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含 `success: true` 和 `deleted` 数字（表示删除的记录数）
- 再次按 request_id 查询历史列表，结果为空

---

### TC-ARP-14：删除请求模板

**前置条件**：已创建请求，记录 `REQUEST_ID`

**操作步骤**：
1. 使用 curl 删除请求：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/replay/requests/{REQUEST_ID}
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含成功消息 "Request deleted"
- 再次查询该请求返回 404

---

### TC-ARP-15：删除重放集合

**前置条件**：已创建集合，记录 `GROUP_ID`

**操作步骤**：
1. 使用 curl 删除集合：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/replay/groups/{GROUP_ID}
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含成功消息 "Group deleted"
- 再次列出集合，该集合不存在
- 该集合下的所有请求也被一并删除

---

### TC-ARP-16：执行带规则配置的重放

**前置条件**：已在 Bifrost 中创建并启用至少一条转发规则

**操作步骤**：
1. 使用 curl 执行带规则的重放请求：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "http://httpbin.org/get",
       "method": "GET",
       "headers": [["User-Agent","Bifrost-Test"]],
       "rule_config": {
         "mode": "enabled",
         "selected_rules": []
       }
     }'
   ```

**预期结果**：
- 返回 HTTP 200
- `data.applied_rules` 数组包含匹配到的规则（如果有匹配规则）
- 每条规则包含 `pattern`、`protocol`、`value`、`rule_name` 等字段

---

### TC-ARP-17：移动请求到不同集合

**前置条件**：已创建两个集合 `GROUP_A` 和 `GROUP_B`，并在 `GROUP_A` 中创建了一个请求 `REQUEST_ID`

**操作步骤**：
1. 使用 curl 将请求移动到 `GROUP_B`：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/replay/requests/{REQUEST_ID}/move \
     -H "Content-Type: application/json" \
     -d '{"group_id":"{GROUP_B_ID}"}'
   ```

**预期结果**：
- 返回 HTTP 200
- 响应体包含成功消息 "Request moved"
- 再次查询 `GROUP_A` 的请求列表，该请求不存在
- 查询 `GROUP_B` 的请求列表，该请求已出现在其中

---

### TC-ARP-18：带路径前缀的转发规则不会重复拼接路径

**前置条件**：
1. 启动一个本地 HTTP Echo 服务，监听 `9000` 端口，响应体包含收到的请求 path 和 query。
2. Bifrost 使用临时数据目录启动在 `8800` 端口，并携带 `--no-system-proxy`。

**操作步骤**：
1. 使用 Replay Admin API 执行带 custom rules 的请求：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "https://internal.example.test/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg?v=1",
       "method": "GET",
       "headers": [["Accept","application/json"]],
       "rule_config": {
         "mode": "custom",
         "custom_rules": "https://internal.example.test/labor_cost/static/ http://127.0.0.1:9000/labor_cost/static/"
       },
       "timeout_ms": 15000
     }'
   ```

**预期结果**：
- 返回 HTTP 200，响应体包含 `success: true`
- `data.status` 为 `200`
- Echo 服务收到的请求 path 为 `/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg`
- Echo 服务收到的 query 为 `v=1`
- Echo 服务没有收到重复路径 `/labor_cost/static/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg`

---

### TC-ARP-19：Replay 响应 Body 规则执行 resMerge

**前置条件**：
1. 使用临时数据目录启动 Bifrost，监听 `8087`，并携带 `--unsafe-ssl --skip-cert-check --no-system-proxy`。
2. 通过 `rule add` / `rule enable` 配置一条 HTTPS API 的 `resMerge://({"test":"qwe"})` 规则，确认 `rule active` 中该规则启用。

**操作步骤**：
1. 使用 Replay Admin API 执行匹配该规则的 POST 请求：
   ```bash
   curl -s -X POST http://127.0.0.1:8087/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "https://internal.example.test/people/api/common/page_permission",
       "method": "POST",
       "headers": [["content-type","application/json"]],
       "body": "{\"key_list\":[\"page.example\"]}",
       "rule_config": {"mode":"enabled","selected_rules":[]},
       "timeout_ms": 20000
     }'
   ```
2. 解析返回 JSON 的 `data.applied_rules` 和 `data.body`。
3. 查询 `traffic list --host internal.example.test --format json-pretty`，确认 replay 记录的 `rp` 包含 `resMerge`。

**预期结果**：
- 返回 HTTP 200，响应体包含 `success: true`
- `data.status` 为 `200`
- `data.applied_rules` 中包含 `resMerge`
- `data.body` 解析为 JSON 后，顶层包含 `"test": "qwe"`
- Traffic 记录显示客户端应用为 `Bifrost Replay`，规则命中列表包含 `resMerge`

---

### TC-ARP-20：Replay 规则覆盖回归（请求修改 + 响应修改 + 内容注入）

**前置条件**：
1. 使用临时数据目录启动 Bifrost，监听 `8087`，并携带 `--unsafe-ssl --skip-cert-check --no-system-proxy`。
2. 启动一个本地 echo 上游，监听 `18087`，返回收到的路径、请求头和请求体。

**操作步骤**：
1. 使用 Replay Admin API 的 `custom` 规则模式执行请求，规则同时覆盖转发、请求头/URL/body 修改和响应头/body 修改：
   ```bash
   curl -s -X POST http://127.0.0.1:8087/_bifrost/api/replay/execute/unified \
     -H "Content-Type: application/json" \
     -d '{
       "url": "http://replay-rules.example.test/api?drop=1&old=1",
       "method": "POST",
       "headers": [["Content-Type","text/plain"],["X-Remove","1"],["X-Trace","old"]],
       "body": "{\"a\":1,\"b\":0}",
       "rule_config": {
         "mode": "custom",
         "custom_rules": "http://replay-rules.example.test/api http://127.0.0.1:18087/echo reqHeaders://(X-Trace: old) reqCookies://(sid=abc,theme=dark) delete://reqHeaders.X-Remove|urlParams.drop urlParams://(keep=2&drop=) reqType://json reqCharset://utf-8 forwardedFor://1.2.3.4 headerReplace://req.X-Trace:old=new reqCors://(origin=https://app.example.test&methods=POST&headers=X-Test) reqMerge://({\"b\":2,\"c\":3}) statusCode://201 resHeaders://X-Response:old headerReplace://res.X-Response:old=new resCookies://rid=xyz resType://json resCharset://utf-8 cache://60 attachment://report.json responseFor://mobile resCors://(origin=https://app.example.test&methods=GET,POST&headers=X-Test&credentials=false) trailers://X-Trailer:done htmlAppend://(<tail>) resReplace://<tail>= resMerge://({\"response_rule\":\"ok\"})"
       },
       "timeout_ms": 10000
     }'
   ```
2. 解析返回 JSON 的 `data.status`、`data.headers`、`data.body` 和 `data.applied_rules`。

**预期结果**：
- 返回 HTTP 200，响应体包含 `success: true`
- `data.status` 为 `201`
- `data.applied_rules` 至少包含 `reqHeaders`、`reqCookies`、`delete`、`urlParams`、`reqType`、`reqCharset`、`forwardedFor`、`headerReplace`、`reqCors`、`params`、`statusCode`、`resHeaders`、`resCookies`、`resType`、`resCharset`、`cache`、`attachment`、`responseFor`、`resCors`、`trailers`、`htmlAppend`、`resReplace`、`resMerge`
- echo 上游在 `data.body.request_body` 中看到 `{"a":1,"b":2,"c":3}`，且请求路径不再包含 `drop=1`，包含 `keep=2`
- echo 上游在 `data.body.headers` 中看到 `x-trace: new`、`content-type: application/json; charset=utf-8`、`origin: https://app.example.test`、`access-control-request-method: POST`、`access-control-request-headers: X-Test`、`cookie` 包含 `sid=abc` 和 `theme=dark`
- `data.headers` 中包含 `X-Response: new`、`Set-Cookie: rid=xyz`、`Content-Type: application/json; charset=utf-8`、`Cache-Control: max-age=60`、`Content-Disposition: attachment; filename="report.json"`、`x-bifrost-response-for: mobile`、`Access-Control-Allow-Origin: https://app.example.test` 和 `Trailer: X-Trailer`
- `data.body` 解析为 JSON 后，顶层包含 `"response_rule": "ok"`

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
