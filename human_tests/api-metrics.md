# Metrics 管理 API 测试用例

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 服务启动成功后，确认管理端可访问：`http://127.0.0.1:8800/_bifrost/`
3. 产生一些流量数据以便验证指标统计（可通过 curl 发送若干代理请求，或直接测试空数据下的默认响应）

---

## 测试用例

### TC-AME-01：获取当前指标快照 — 基本结构验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics | jq .
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 对象包含以下顶层字段：
  - `timestamp`（数字，毫秒级时间戳）
  - `memory_used`（数字，进程 RSS，单位 bytes）
  - `memory_total`（数字，系统总内存，单位 bytes）
  - `cpu_usage`（浮点数，CPU 使用率百分比）
  - `total_requests`（数字，累计请求数）
  - `active_connections`（数字，当前活跃连接数）
  - `bytes_sent`（数字）
  - `bytes_received`（数字）
  - `bytes_sent_rate`（浮点数，发送速率）
  - `bytes_received_rate`（浮点数，接收速率）
  - `qps`（浮点数，每秒请求数）
  - `max_qps`（浮点数）
  - `max_bytes_sent_rate`（浮点数）
  - `max_bytes_received_rate`（浮点数）

---

### TC-AME-02：获取当前指标快照 — 协议分类统计验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics | jq '{http, https, tunnel, ws, wss, h3, socks5}'
   ```

**预期结果**：
- 返回 JSON 包含 `http`、`https`、`tunnel`、`ws`、`wss`、`h3`、`socks5` 七个协议分类对象
- 每个协议对象包含以下字段：
  - `requests`（数字）
  - `bytes_sent`（数字）
  - `bytes_received`（数字）
  - `active_connections`（数字）
- 刚启动时各协议 `requests` 值为 0 或与实际流量一致

---

### TC-AME-03：获取当前指标快照 — memory 和 CPU 值合理性

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics | jq '{memory_used, memory_total, cpu_usage}'
   ```

**预期结果**：
- `memory_used` > 0（进程必须占用内存）
- `memory_total` > `memory_used`（系统总内存大于进程使用量）
- `cpu_usage` >= 0（CPU 使用率为非负数）

---

### TC-AME-04：获取指标历史记录 — 默认无 limit 参数

**操作步骤**：
1. 等待至少 5 秒以确保采集器产生了一些历史数据
2. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/history | jq 'length'
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 数组
- 数组长度 >= 1（至少有一条历史快照）
- 每条记录结构与 `/api/metrics` 返回的 `MetricsSnapshot` 一致

---

### TC-AME-05：获取指标历史记录 — 指定 limit 参数

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/metrics/history?limit=5" | jq 'length'
   ```

**预期结果**：
- 返回 JSON 数组长度 <= 5
- 数组中每条记录结构完整，包含 `timestamp`、`memory_used`、`qps` 等字段

---

### TC-AME-06：获取指标历史记录 — limit=1 只返回一条

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/metrics/history?limit=1" | jq 'length'
   ```

**预期结果**：
- 返回 JSON 数组长度为 1
- 该条记录的 `timestamp` 为最近的采集时间点

---

### TC-AME-07：获取指标历史记录 — limit=50

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/metrics/history?limit=50" | jq 'length'
   ```

**预期结果**：
- 返回 JSON 数组长度 <= 50
- 记录按时间顺序排列

---

### TC-AME-08：获取应用维度统计 — 基本结构验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/apps | jq .
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 数组
- 每个元素包含以下字段：
  - `app_name`（字符串，应用名称）
  - `requests`（数字，请求总数）
  - `active_connections`（数字，活跃连接数）
  - `bytes_sent`（数字）
  - `bytes_received`（数字）
  - `http_requests`（数字）
  - `https_requests`（数字）
  - `tunnel_requests`（数字）
  - `ws_requests`（数字）
  - `wss_requests`（数字）
  - `h3_requests`（数字）
  - `socks5_requests`（数字）

---

### TC-AME-09：获取应用维度统计 — 无流量时返回空数组

**前置条件**：刚启动的全新服务，未产生任何代理流量

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/apps | jq 'length'
   ```

**预期结果**：
- 返回 JSON 数组长度为 0（无流量时无应用统计）

---

### TC-AME-10：获取应用维度统计 — 按请求数降序排列

**前置条件**：通过代理产生了来自不同应用的流量

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/apps | jq '.[0].requests, .[-1].requests'
   ```

**预期结果**：
- 数组按 `requests` 字段降序排列
- 第一个元素的 `requests` >= 最后一个元素的 `requests`

---

### TC-AME-11：获取主机维度统计 — 基本结构验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/hosts | jq .
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 数组
- 每个元素包含以下字段：
  - `host`（字符串，主机名）
  - `requests`（数字，请求总数）
  - `active_connections`（数字，活跃连接数）
  - `bytes_sent`（数字）
  - `bytes_received`（数字）
  - `http_requests`（数字）
  - `https_requests`（数字）
  - `tunnel_requests`（数字）
  - `ws_requests`（数字）
  - `wss_requests`（数字）
  - `h3_requests`（数字）
  - `socks5_requests`（数字）

---

### TC-AME-12：获取主机维度统计 — 无流量时返回空数组

**前置条件**：刚启动的全新服务，未产生任何代理流量

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/hosts | jq 'length'
   ```

**预期结果**：
- 返回 JSON 数组长度为 0

---

### TC-AME-13：获取主机维度统计 — 按请求数降序排列

**前置条件**：通过代理产生了访问不同主机的流量

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/metrics/hosts | jq '.[0].requests, .[-1].requests'
   ```

**预期结果**：
- 数组按 `requests` 字段降序排列
- 第一个元素的 `requests` >= 最后一个元素的 `requests`

---

### TC-AME-14：不支持的 HTTP 方法返回 405

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/metrics
   ```

**预期结果**：
- HTTP 状态码为 405（Method Not Allowed）

---

### TC-AME-15：不存在的 metrics 子路径返回 404

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8800/_bifrost/api/metrics/nonexistent
   ```

**预期结果**：
- HTTP 状态码为 404（Not Found）

---

### TC-AME-16：实时指标包含服务端派生字段

**操作步骤**：
1. 在隔离端口启动当前源码构建并生成至少一条本地代理流量。
2. 执行：
   ```bash
   curl -s http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/metrics | jq -e '
     .total_traffic_bytes == (.bytes_sent + .bytes_received) and
     (.memory_usage_percent | type == "number")
   '
   ```

**预期结果**：
- `total_traffic_bytes` 由服务端给出并等于累计上下行之和。
- `memory_usage_percent` 由服务端给出；前端不需要相除计算。

---

### TC-AME-17：Applications / Hosts 返回服务端汇总且兼容旧响应

**操作步骤**：
1. 通过隔离代理生成访问本地 HTTP mock 的流量。
2. 分别请求 `/api/metrics/apps`、`/api/metrics/hosts`，确认旧接口仍是数组。
3. 分别追加 `?include_summary=true`，执行：
   ```bash
   jq -e '
     (.items | type == "array") and
     (.summary.total == (.items | length)) and
     (.summary.requests == ([.items[].requests] | add)) and
     (.summary.total_traffic_bytes == (.summary.bytes_sent + .summary.bytes_received))
   '
   ```

**预期结果**：
- 旧数组响应保持兼容。
- 新响应中的应用/主机数、请求数和总流量由服务端汇总，页面无需 `length` 或 `reduce`。
- 运行期读取内存增量桶，不执行全表 `GROUP BY`。

---

### TC-AME-18：Metrics WebSocket 最快每秒推送并携带落库记录数

**操作步骤**：
1. 使用 `metrics_interval_ms=500` 订阅隔离实例的 `/api/push`，持续记录 3.2 秒。
2. 检查每条 `metrics_update.data`。

**预期结果**：
- 初始快照之外，单客户端最快每秒一帧；3.2 秒窗口总帧数为 1–4。
- 每帧包含数字类型 `recorded_traffic`，其语义是当前落库保留记录数。
- 每帧 `metrics.total_traffic_bytes == bytes_sent + bytes_received`，并包含数字类型 `memory_usage_percent`。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```

## 执行记录

2026-08-06 服务端 Metrics 派生字段、内存汇总与 WebSocket 推送执行记录：

- 已执行用例：`TC-AME-16`、`TC-AME-17`、`TC-AME-18`。
- 隔离边界：真实进程分别使用代理/管理端口 `18991`、`19924`，本地 mock 端口 `33001`、`33202`，临时 `BIFROST_DATA_DIR` 与 `--no-system-proxy`；未连接、停止或修改正式 `9900` 实例，脚本结束后已停止进程并清理临时目录。
- Applications / Hosts：`PROXY_PORT=18991 ADMIN_PORT=18991 ECHO_HTTP_PORT=33001 bash e2e-tests/tests/test_metrics_hosts_apps_admin_api.sh` 通过；旧接口保持数组，`include_summary=true` 的 `total`、`requests`、`bytes_sent`、`bytes_received`、`total_traffic_bytes` 与 items 一致。
- WebSocket：最终复测 `ADMIN_PORT=19926 PROXY_PORT=19926 MOCK_HTTP_PORT=33204 bash e2e-tests/tests/test_traffic_push_e2e.sh` 通过 `4/4`。本机无 `websocat` 时自动使用 Node 22 原生 WebSocket 探针；3.2 秒收到 2 帧，且任意相邻 Metrics 快照的服务端时间戳间隔均不小于 900ms，符合“初始帧 + 最快每秒一帧”边界；每帧均包含 `recorded_traffic`、`memory_usage_percent`，且 `total_traffic_bytes` 等于累计上下行之和。
- 结论：三个新增用例全部通过；Metrics API/Push 的派生与汇总字段均由服务端提供，Applications / Hosts 常态读取内存增量桶，前端无需统计计算。
