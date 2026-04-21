# Push WebSocket API 测试用例

## 功能模块说明

Push WebSocket API 提供实时数据推送能力。客户端通过 WebSocket 连接到 `/_bifrost/api/push` 端点，可订阅不同类型的数据推送，包括流量更新（traffic）、概览信息（overview）、性能指标（metrics）、历史数据（history）等。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保已安装 `websocat`（WebSocket 命令行客户端）或使用等效工具：
   ```bash
   brew install websocat
   ```
3. 确保已安装 Node.js，并已执行过仓库依赖安装（`web/node_modules` 可用）
4. 确保端口 8800 未被其他程序占用

---

## 测试用例

### TC-APU-01：建立 WebSocket 连接并接收 connected 消息

**操作步骤**：
1. 使用 websocat 连接到 Push WebSocket 端点：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push"
   ```

**预期结果**：
- WebSocket 连接成功建立
- 收到一条 JSON 消息，`type` 字段为 `"connected"`
- `data` 字段包含 `client_id`（正整数）和 `message` 字段（值为 `"WebSocket connection established"`）
- 示例：`{"type":"connected","data":{"client_id":1,"message":"WebSocket connection established"}}`

---

### TC-APU-02：使用 need_overview 参数订阅概览数据

**操作步骤**：
1. 使用 websocat 连接并传入 need_overview 参数：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push?need_overview=true"
   ```
2. 等待约 5 秒（overview 推送间隔为 5 秒）

**预期结果**：
- 首先收到 `connected` 消息
- 随后收到 `type` 为 `"overview_update"` 的消息
- `data` 字段包含：
  - `system`：系统信息对象
  - `metrics`：性能指标对象
  - `rules`：规则信息，含 `total` 和 `enabled` 字段
  - `traffic`：流量信息，含 `recorded` 字段
  - `server`：服务器信息，含 `port`（值为 `8800`）和 `admin_url`

---

### TC-APU-03：使用 need_metrics 参数订阅性能指标

**操作步骤**：
1. 使用 websocat 连接并传入 need_metrics 参数：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push?need_metrics=true"
   ```
2. 等待约 1 秒（metrics 默认推送间隔为 1000ms）

**预期结果**：
- 首先收到 `connected` 消息
- 随后周期性收到 `type` 为 `"metrics_update"` 的消息
- `data` 字段包含 `metrics` 对象

---

### TC-APU-04：使用 need_history 参数订阅历史数据

**操作步骤**：
1. 使用 websocat 连接并传入 need_history 参数：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push?need_history=true"
   ```
2. 等待约 5 秒（history 推送间隔为 5 秒）

**预期结果**：
- 首先收到 `connected` 消息
- 随后收到 `type` 为 `"history_update"` 的消息
- `data` 字段包含 `history` 数组

---

### TC-APU-05：组合多个订阅参数

**操作步骤**：
1. 使用 websocat 连接并传入多个订阅参数：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push?need_overview=true&need_metrics=true&need_history=true"
   ```
2. 等待约 6 秒

**预期结果**：
- 首先收到 `connected` 消息
- 随后同时收到 `overview_update`、`metrics_update` 和 `history_update` 三种类型的推送消息
- 各推送消息按各自的间隔独立发送

---

### TC-APU-06：订阅 need_traffic 接收流量增量更新

**操作步骤**：
1. 使用 websocat 连接并传入 need_traffic 参数：
   ```bash
   websocat "ws://127.0.0.1:8800/_bifrost/api/push?need_traffic=true"
   ```
2. 在另一个终端通过代理发起 HTTP 请求以产生流量：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 观察 WebSocket 连接中的消息

**预期结果**：
- 收到 `connected` 消息
- 收到初始 `traffic_delta` 消息（包含已有流量的快照）
- 当新流量产生后，收到新的 `type` 为 `"traffic_delta"` 的消息
- `data` 字段包含：
  - `inserts`：新增记录数组
  - `updates`：更新记录数组
  - `has_more`：布尔值
  - `server_total`：服务端流量总数
  - `server_sequence`：服务端序列号

---

### TC-APU-07：通过 WebSocket 消息动态更新订阅

**操作步骤**：
1. 使用 websocat 连接（不带订阅参数）：
   ```bash
   websocat "ws://127.0.0.1:8800/_bifrost/api/push"
   ```
2. 收到 `connected` 消息后，发送 JSON 消息更新订阅：
   ```json
   {"need_overview":true,"need_metrics":true}
   ```
3. 等待约 5 秒

**预期结果**：
- 连接成功并收到 `connected` 消息
- 发送订阅更新后，开始收到 `overview_update` 和 `metrics_update` 推送消息
- 在发送订阅更新前不会收到这些推送

---

### TC-APU-08：使用 x_client_id 参数标识客户端

**操作步骤**：
1. 使用 websocat 连接并传入 x_client_id 参数：
   ```bash
   websocat -1 "ws://127.0.0.1:8800/_bifrost/api/push?x_client_id=test-client-001&need_overview=true"
   ```

**预期结果**：
- WebSocket 连接成功建立
- 收到 `connected` 消息
- 后续正常收到订阅的推送消息

---

### TC-APU-09：自定义 metrics_interval_ms 参数

**操作步骤**：
1. 使用 websocat 连接并设置 metrics 推送间隔为 2000ms：
   ```bash
   websocat "ws://127.0.0.1:8800/_bifrost/api/push?need_metrics=true&metrics_interval_ms=2000"
   ```
2. 观察 metrics_update 消息的推送频率

**预期结果**：
- 收到 `connected` 消息
- `metrics_update` 消息大约每 2 秒推送一次（而非默认的 1 秒）
- 间隔在 200ms ~ 5000ms 范围内有效

---

### TC-APU-10：非 WebSocket 请求到 push 端点返回错误

**操作步骤**：
1. 使用 curl 直接请求 push 端点（不使用 WebSocket 协议）：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/push
   ```

**预期结果**：
- 返回 HTTP 400 Bad Request
- 响应体包含错误信息 "Invalid upgrade header"

---

### TC-APU-11：通过 Bifrost 自身 HTTP 代理访问管理端 Push WebSocket

**操作步骤**：
1. 保持 Bifrost 运行在 `127.0.0.1:8800`，管理端地址为 `http://127.0.0.1:8800/_bifrost/`
2. 执行以下命令，通过 `http://127.0.0.1:8800` 代理访问 `ws://localhost:8800/_bifrost/api/push`：
   ```bash
   node e2e-tests/test_utils/ws_via_http_proxy.js \
     "http://127.0.0.1:8800" \
     "ws://localhost:8800/_bifrost/api/push?x_client_id=human-proxy-regression&need_overview=true&metrics_interval_ms=500" \
     connected \
     overview_update
   ```

**预期结果**：
- WebSocket 连接成功建立，不出现握手失败或立即断开的情况
- 输出中包含 `type` 为 `"connected"` 的消息
- 输出中包含 `type` 为 `"overview_update"` 的消息
- 管理端不会因为该连接持续打印重连错误

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
