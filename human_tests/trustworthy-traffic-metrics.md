# Trustworthy Traffic Metrics

## 功能模块说明

验证 Activity、Metrics API、Traffic API 与按应用/主机统计使用可信的流量指标口径：

- 每个代理请求只计数一次。
- HTTP/HTTPS 请求 body 计入 upload bytes。
- HTTP/HTTPS/Mock 响应 body 计入 download bytes。
- QPS 与实时网速来自最近事件窗口，不依赖 UI 轮询间隔。
- Traffic detail、compact list、host/app distribution 都能读取 trusted upload/download 字段。
- SSE/streaming 响应使用真实下载字节，不被 legacy socket reconciliation 覆盖成错误口径。
- 临时代理端口流量记录 listener port 与命中规则详情，且对应 Mock 下载字节真实入账。
- WebSocket 帧流量计入 trusted upload/download，并保留 `is_websocket` 标记。
- SOCKS5 HTTP 入口计入 trusted upload/download，并保留 `socks5-http` 协议口径。
- HTTPS CONNECT 隧道计入 trusted upload/download，并保留 `is_tunnel` 与 `tunnel` 协议口径。
- 实时 QPS 与上下行速率使用固定容量时间桶，避免后台统计在高频流式流量下随事件数线性增长内存。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 已安装 `jq`、`python3`、`curl`。
- 已构建当前分支二进制：

```bash
SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
```

## 测试用例列表

### TC-TTM-01 真实 HTTP POST 上行/下行统计

操作步骤：

1. 执行：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_trustworthy_traffic_metrics.sh
```

2. 观察脚本输出中的真实 POST 断言。

预期结果：

- `proxy request counter is not double counted across HTTP, SSE, Mock, and temp port` 通过，7 个代理请求只增加 7。
- `upload bytes include real POST body` 通过。
- `traffic detail stores trusted POST upload bytes` 通过。
- `traffic detail stores trusted POST download bytes` 通过。

### TC-TTM-02 Mock 直接响应下行统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 Mock direct response 断言。

预期结果：

- `Mock GET upload bytes remain zero` 通过。
- `Mock response body is stored as trusted download bytes` 通过，Mock body 长度精确入账。
- `download bytes include Mock body` 通过。

### TC-TTM-03 SSE/streaming 下载统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 SSE response 断言。

预期结果：

- `SSE GET upload bytes include real request bytes` 通过，SSE GET 的真实请求头字节会计入上行。
- `SSE streamed response bytes are stored as trusted download bytes` 通过。
- `SSE traffic record is marked as is_sse=true` 通过。

### TC-TTM-04 临时端口流量统计与规则详情

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 temporary proxy port 断言。

预期结果：

- `temporary port Mock GET upload bytes remain zero` 通过。
- `temporary port Mock response body is trusted download bytes` 通过。
- `temporary port traffic stores listener_port` 通过，记录中的 listener port 等于脚本绑定的临时端口。
- `temporary port traffic records enabled rule details` 通过，记录包含 `trusted-metrics-temp` 命中规则。

### TC-TTM-05 WebSocket 双向流量统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 WebSocket frames through proxy 断言。

预期结果：

- `WebSocket traffic increments request counter once` 通过。
- `WebSocket traffic record is marked as is_websocket=true` 通过。
- `WebSocket upload bytes include client frames` 通过。
- `WebSocket download bytes include server frames` 通过。

### TC-TTM-06 SOCKS5 HTTP 双向流量统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 SOCKS5 proxy 断言。

预期结果：

- `SOCKS5 HTTP traffic increments request counter once` 通过。
- `SOCKS5 HTTP traffic record uses socks5-http protocol` 通过。
- `SOCKS5 HTTP upload bytes include real client request bytes` 通过。
- `SOCKS5 HTTP download bytes include real upstream response bytes` 通过。

### TC-TTM-07 HTTPS CONNECT 隧道双向流量统计

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 HTTPS CONNECT tunnel 断言。

预期结果：

- `HTTPS CONNECT tunnel increments request counter once` 通过。
- `HTTPS CONNECT traffic record is marked as is_tunnel=true` 通过。
- `HTTPS CONNECT traffic record uses tunnel protocol` 通过。
- `HTTPS CONNECT upload bytes include real tunnel client bytes` 通过。
- `HTTPS CONNECT download bytes include real tunnel server bytes` 通过。

### TC-TTM-08 QPS 与实时网速可信性

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 mixed burst 后的 Metrics API 断言。

预期结果：

- `QPS reflects the recent mixed burst` 通过，突发 7 个请求后 QPS 至少为 7。
- `upload rate reflects recent bytes` 通过。
- `download rate reflects recent bytes` 通过。

### TC-TTM-09 Host/App 分布使用 trusted bytes

操作步骤：

1. 继续执行 TC-TTM-01 中的同一脚本。
2. 观察 host/app distribution 断言。

预期结果：

- `host distribution uses trusted download bytes` 通过。
- `app distribution uses trusted download bytes` 通过。

### TC-TTM-10 实时统计固定容量性能回归

操作步骤：

1. 执行：

```bash
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin realtime_metrics -- --nocapture
```

2. 观察 `test_realtime_metrics_use_fixed_capacity_buckets_under_high_event_volume` 和 `test_realtime_metrics_bucket_window_expires_without_residual_rates` 断言。
3. 继续执行 TC-TTM-01 的真实链路脚本，确认桶化统计没有降低 QPS、上行、下行真实性。

预期结果：

- 高频写入 50,000 个实时事件后，实时统计 bucket 总数仍等于固定的 `REALTIME_BUCKET_COUNT * REALTIME_WINDOW_SHARDS`。
- 事件窗口过期后 QPS、upload rate、download rate 均归零，不残留旧流量。
- 真实链路脚本中的 `QPS reflects the recent mixed burst`、`upload rate reflects recent bytes`、`download rate reflects recent bytes` 继续通过。

## 执行记录

- 2026-07-05：执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin realtime_metrics -- --nocapture`，3 个实时统计单测全部通过，覆盖最近窗口、过期归零、固定容量 bucket。
- 2026-07-05：执行 `bash e2e-tests/tests/test_trustworthy_traffic_metrics.sh`，真实 HTTP、Mock、SSE、临时端口、WebSocket、SOCKS5、HTTPS CONNECT、QPS、上行/下行速率、Traffic detail 与 host/app 分布断言全部通过。

## 清理步骤

脚本退出时自动执行：

- 停止临时 Bifrost 实例。
- 停止临时 mock HTTP server。
- 删除临时 `BIFROST_DATA_DIR`。
- 释放临时端口。
