# Trustworthy Traffic Metrics

## 背景

Activity 页面展示的请求数、QPS、上行、下行、网速和按应用/主机统计，需要来自真实代理链路，而不是受 UI 轮询频率、legacy size 字段或 Mock 分支遗漏影响。

旧实现存在三类风险：

- 实时 QPS 和网速按两次 `get_current` 之间的 counter 差计算，展示值会受前端轮询间隔和 snapshot cache 影响。
- `request_size` / `response_size` 同时承载 body size、headers + body size、socket 总量等不同语义，按应用/主机聚合时不够可信。
- Mock/direct status、HTTPS mock、WebSocket、SOCKS、SSE 等路径对累计 metrics 和 traffic record 字段的写入不一致。

## 指标口径

| 指标 | 可信来源 | 说明 |
| --- | --- | --- |
| Total requests | `MetricsCollector::increment_requests_by_type` | 每个代理请求只在协议处理分支入账一次；server 入口不再额外增加，避免双计数。 |
| QPS | 最近 1 秒 request event 窗口 | 每个 request event 进入固定容量时间桶，`get_current` 按最近 1000ms 统计，不依赖 UI 轮询间隔。 |
| Upload bytes | `TrafficRecord.upload_bytes` / `MetricsCollector.bytes_sent` | HTTP/HTTPS body 使用请求 body 长度；WebSocket/SOCKS/tunnel 使用 socket send bytes。 |
| Download bytes | `TrafficRecord.download_bytes` / `MetricsCollector.bytes_received` | HTTP/HTTPS body 使用响应 body 长度；WebSocket/SOCKS/tunnel 使用 socket receive bytes；SSE 按流式累计更新。 |
| Upload/download rate | 最近 1 秒 byte event 窗口 | 每次 bytes sent/received 入账时聚合到固定容量时间桶，`get_current` 按最近 1000ms 统计。 |
| App/host distribution | `SUM(upload_bytes)` / `SUM(download_bytes)` | 不再使用 legacy `request_size` / `response_size` 聚合。 |

`request_size` 和 `response_size` 保留为兼容字段，用于旧 UI、搜索摘要或原始记录展示；新统计和 Activity fallback 使用 `upload_bytes` / `download_bytes`。

## 数据迁移

SQLite schema version 从 12 升级到 13：

- 新增 `traffic_records.upload_bytes INTEGER NOT NULL DEFAULT 0`
- 新增 `traffic_records.download_bytes INTEGER NOT NULL DEFAULT 0`
- 迁移时对旧记录做保守 backfill：`upload_bytes = request_size`、`download_bytes = response_size`

这样旧记录仍能在 Activity 和聚合中显示非零统计，新记录则写入更准确的可信字段。

## 链路覆盖

- HTTP/HTTPS 正常请求：request body 写入 upload，final response body 写入 download。
- Mock/direct status：请求 body 计入 upload，mock response body 计入 download，同时更新 `MetricsCollector`。
- SSE/streaming：流式完成或 TeeBody drop 时用真实 body 累计更新 download。
- CONNECT tunnel、WebSocket、SOCKS TCP/UDP：关闭或中间更新时用 socket send/receive bytes 回写 upload/download。
- Replay/import：初始化 traffic record 时同步写入 upload/download，避免新字段缺失。
- Push/search/list compact：新增 compact `up` / `down` 字段，前端 mapping 优先使用新字段，缺失时 fallback 到 legacy size。

## 性能设计

实时 QPS 与上下行速率使用 50ms bucket、保留 10 秒、16 shard 的固定容量窗口：

- 每个 request/byte event 只做一次 shard 选择、一次短锁和整数累加。
- 内存占用固定为 `REALTIME_BUCKET_COUNT * REALTIME_WINDOW_SHARDS`，不会随事件数量线性增长。
- `get_current` 只扫描固定数量 bucket，并继续保留 250ms snapshot cache。
- 总请求数、总上行、总下行仍由 atomic counter 精确维护；固定桶只影响实时速率窗口。

这降低了高频 WebSocket/SOCKS/tunnel chunk 场景下原 per-event `VecDeque` 的内存增长和单锁竞争风险。

## 性能实测

2026-07-05 使用 `target/release/bifrost`、隔离 `BIFROST_DATA_DIR`、本地 upstream、`--no-system-proxy`、`--skip-cert-check`、`--unsafe-ssl` 执行真实 HTTP proxy 压测：

| 场景 | 结果 |
| --- | --- |
| Idle 常驻 | 静置 20 秒后采样 30 秒，平均 CPU 约 4.8%，RSS 不增长。 |
| 200 QPS / 30 秒 | 6000/6000 成功，实际 199.7 QPS，p50 44ms、p95 77ms、p99 104ms，Bifrost 进程平均 CPU 约 28.4%，RSS 峰值约 94MB。 |
| 50 / 100 / 200 QPS 阶梯 | 三档均 100% 成功；平均 CPU 约 16.1% / 21.2% / 28.2%，p95 约 56ms / 57ms / 71ms。 |

结论：

- 固定桶实时统计没有破坏 QPS/速率真实性；Metrics API 在 200 QPS 期间报告平均 QPS 约 196、峰值约 216，请求总数与客户端成功数一致。
- 当前版本在 idle 常驻场景可以接近 5% CPU 目标。
- 200 QPS 完整代理与完整 traffic 记录场景仍明显高于 10% CPU 目标。剩余主要成本不在 Activity 展示或 Metrics API 轮询，而在完整请求代理路径、traffic record 构建、异步批量 traffic DB 持久化、compact/cache 维护和每请求连接处理。
- 后续若要让 200 QPS 也低于 10%，需要新增高吞吐轻量记录策略，例如保留全局 metrics 精确计数但对 traffic detail 做采样、按 host/app 聚合增量写入、或提供可配置的无 body/detail 记录模式。

## 验证计划

- 单元测试：
  - `test_realtime_metrics_use_recent_event_window` 验证 QPS 和网速来自最近事件窗口，过窗后归零。
  - `test_realtime_metrics_bucket_window_expires_without_residual_rates` 验证固定时间桶过窗后不残留旧流量。
  - `test_realtime_metrics_use_fixed_capacity_buckets_under_high_event_volume` 验证高频事件不会扩大实时统计内存结构。
  - `test_metrics_aggregates_use_trusted_upload_download_bytes` 验证 app/host 聚合与 compact summary 使用 trusted bytes。
- E2E：
  - 启动真实 Bifrost 临时实例。
  - 通过代理发起真实 HTTP POST，验证请求数、upload/download 累计、QPS 和速率。
  - 通过 Mock 规则发起直接响应，验证 Mock response body 进入 download。
- human_tests：
  - 使用真实 CLI、临时 data dir、临时 upstream server 和 Admin API 执行用户可感知验证。

## 残余边界

- 对于历史记录，迁移只能按旧字段 backfill，无法恢复旧版本未采集到的真实 body/socket 方向字节。
- 实时速率使用 1 秒窗口，适合 Activity/Tray 这类近实时展示；长期趋势仍应读取历史 snapshot 或 traffic DB 聚合。
