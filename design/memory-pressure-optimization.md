# 代理内存压力优化设计方案（Memory Pressure Optimization）

## 背景

Bifrost 代理进程在高并发、大 body、TLS 解包 / 不解包混合场景下曾出现 RSS 从 100 MB 涨到 3 GB 的事故。除了单点大请求撑爆内存外，长时间运行的 SSE / WebSocket 缓冲、连接监控历史帧堆积、BodyStore 过期文件不清理都会让内存水位持续抬升。

本设计围绕"body 三态处理 + 缓存周期清理 + 内存指标可观测"三条主线，覆盖已经落地的能力，并把仍在规划的分层存储 / BodyRef / 严格 SSE 上限清晰标记 planned，防止文档退化为提案汇编，也避免测试与实现错配。

## 用户目标验证清单

### 必须实现（已落地）

- Traffic 配置存在 `max_body_probe_size`（默认 64 KiB，`crates/bifrost-storage/src/unified_config.rs:410`）并接入 `AdminState`。
- 超过阈值的 request/response body 走流式转发，跳过 body 规则和脚本，Traffic detail 标记截断。
- `BodyStore` 提供 `start_body_cleanup_task`（`crates/bifrost-admin/src/body_store.rs`），按 `retention_days` 周期清理过期 body 文件。
- `FrameStore` 提供 `start_frame_cleanup_task`（`crates/bifrost-admin/src/frame_store.rs`）与按连接粒度的 LRU/上限。
- `ConnectionMonitor::with_config` 支持配置 `preview_limit` 与 `max_frames_per_connection`（`crates/bifrost-admin/src/connection_monitor.rs`）。
- `MetricsCollector` + `MetricsSnapshot { memory_used, memory_total, cpu_usage, qps, ... }` 通过 `GET /_bifrost/api/metrics` 暴露 RSS 与历史；`GET /_bifrost/api/system/overview` 打包为总览。
- WebSocket / SSE payload 存储的 `retention_days` 可配置化并支持热更新（`crates/bifrost-admin/src/ws_payload_store.rs:100-395`）。

### 计划中（planned, not shipped）

- 严格的"三层存储 + BodyRef"分流：未命中规则的解包流量直接落 DB/文件、内存只留索引。
- 整连接级 SSE 事件缓冲合计上限（当前只有 per-frame `preview_limit`）。
- 脚本执行的 body 体积短路阈值与内存副本回收策略。
- 规则引擎匹配缓存 + 失效策略、压缩内容按需解压路径。

### 必须不破坏

- 命中规则且需修改 body 的请求依旧能完整拿到内存中的 body。
- Traffic 列表/搜索/详情不因 recent_cache 限容出现明显延迟增长。
- SSE / WS 帧订阅通道仍能推送最近内容，历史内容通过 body/file 层按需拉取。
- Metrics 采集本身不能显著抬高 CPU / RSS（要求 `/metrics` 单次调用 < 5 ms）。

### 必须真实验证

- 压测：TLS 解包、TLS 不解包、HTTP、SSE、WebSocket 各 10 000 请求 / 100 长连接（WebSocket 使用内置 Python 客户端替代 websocat）。
- 采样 `/_bifrost/api/metrics` 的 `memory_used`，观察 RSS 峰值与稳态。
- 单元测试覆盖 body probe 上限、frame cleanup、body cleanup、metrics snapshot。

## 产品语义

### Body 处理的三态

| 状态 | 判定 | 行为 |
| --- | --- | --- |
| in-memory | Content-Length 已知且 `<= max_body_probe_size` | 完整读入内存，参与 body 规则/脚本/detail |
| bounded probe | Content-Length 未知（chunked/SSE/stream） | 仅读 probe 窗口，超阈值流式转发 |
| passthrough | Content-Length 已知且 `> max_body_probe_size` | 直接流式转发，跳过 body 规则/脚本，detail 标记截断 |

### 缓存清理三条线

1. `BodyStore`：`start_body_cleanup_task` 周期扫描，按 `retention_days` 淘汰过期文件与内存 inline。
2. `FrameStore`：`start_frame_cleanup_task` 按连接粒度限容与过期清理，配合 `metadata_cache: LruCache` 限住内存。
3. `WsPayloadStore`：`WsPayloadStoreState.retention_days` 支持 patch 热更新（`ws_payload_store.rs:263-289`），运行时可以调整保留时长。

### 可观测：Metrics

- `MetricsSnapshot { timestamp, memory_used, memory_total, cpu_usage, qps, ... }`（`web/src/types/index.ts:480`）。
- `GET /_bifrost/api/metrics`：返回当前快照。
- `GET /_bifrost/api/system/overview`：包 `metrics + connections + ...`。
- 历史序列见 `useMetricsStore`（前端 store）与 `MetricsTab.tsx` 展示。

## 技术细节

### 已落地组件

- `crates/bifrost-storage/src/unified_config.rs:410`：`traffic.max_body_probe_size`
- `crates/bifrost-admin/src/body_store.rs`：`start_body_cleanup_task`、`BodyStore`、`BodyRef` 二元存储（inline + file）
- `crates/bifrost-admin/src/frame_store.rs`：`start_frame_cleanup_task`、`FrameStore`、`FrameStoreStats.metadata_cache_len`
- `crates/bifrost-admin/src/connection_monitor.rs`：`ConnectionMonitor::with_config(preview_limit, max_frames_per_connection)`
- `crates/bifrost-admin/src/ws_payload_store.rs`：`WsPayloadStore` 热更新 `retention_days`
- `crates/bifrost-admin/src/metrics.rs`：`MetricsCollector` + `MetricsSnapshot`
- `crates/bifrost-admin/src/lib.rs:71-83`：对外暴露 `start_body_cleanup_task` / `start_frame_cleanup_task`

### 规划中组件（planned）

- 三层存储路由：内存 (规则命中) → DB (未命中已解包) → 文件 (超阈值)。
- `BodyRef` 抽象：把 inline / file / DB 三态统一为查询引用，供搜索、回放、按需拉取使用。
- SSE 整连接级缓冲合计上限：超限即丢缓冲、记录告警，与 `max_body_probe_size` 保持同源阈值。
- 规则匹配结果缓存 + 版本失效。
- 压缩内容流式解压 + 索引化。

## CLI / Web / Admin API

### Admin API

- `GET /_bifrost/api/metrics`：单次 metrics 快照。
- `GET /_bifrost/api/system/overview`：包含 `metrics` 完整字段。
- `PATCH /_bifrost/api/config`：热更新 `traffic.max_body_probe_size`、`ws_payload_store.retention_days` 等。

### CLI

- `bifrost config get traffic.max_body_probe_size`
- `bifrost config set traffic.max_body_probe_size <bytes>`
- `bifrost status`：包含 RSS/CPU 展示（tray/system_stats）。

### Web

- Settings → Metrics tab：折线图展示 `memory_used`、`cpu_usage`、`qps` 历史。
- Settings → Traffic：`max_body_probe_size` 输入。
- Traffic detail：超阈值时 body 面板显示 `body truncated at <n> bytes`。

## Sync 边界

- `max_body_probe_size`、`retention_days` 走 `unified_config` 同步，逐字段热更新；不做跨设备 sync。
- Metrics 快照与历史属本机运行时数据，不入 sync。
- BodyStore / FrameStore 文件属本机磁盘产物，不通过 sync 分发。

## Phase 1-4

### Phase 1：Body probe 与流式转发（已落地）

- 引入 `max_body_probe_size` 与全链路 probe 上限接入（HTTP tunnel / SOCKS / server 中间件）。
- 单测：`crates/bifrost-storage/src/config_manager.rs:1163` 验证 patch 生效。

### Phase 2：BodyStore / FrameStore / WsPayloadStore 清理（已落地）

- `start_body_cleanup_task` + `retention_days`。
- `start_frame_cleanup_task` + `metadata_cache: LruCache`。
- `WsPayloadStore` 热更新 `retention_days`。

### Phase 3：Metrics 与可观测（已落地）

- `MetricsCollector` 采集 RSS / CPU / QPS。
- `/_bifrost/api/metrics` 与 `/_bifrost/api/system/overview` 暴露快照。
- 前端 Metrics tab 折线图 + system overview 展示。

### Phase 4：分层存储 + BodyRef + 整连接级 SSE 上限（planned）

1. 抽象 `BodyRef { Inline, File, Db }`，搜索/回放/详情统一走 BodyRef。
2. 未命中规则的解包流量直接落 DB/文件，内存仅存索引与滚动窗口。
3. 为 SSE / WS 引入连接级缓冲合计上限，超限 drop 并记录告警。
4. 规则匹配缓存 + 失效策略，减少高 QPS 场景 CPU。

## 测试方案

### 单元测试（已存在或将补）

| 位置 | 断言 |
| --- | --- |
| `crates/bifrost-storage/src/config_manager.rs:1163` | `max_body_probe_size: Some(910)` patch 生效 |
| `crates/bifrost-admin/src/body_store.rs` | 过期 body 文件被清理 |
| `crates/bifrost-admin/src/frame_store.rs` | `FrameStoreStats.metadata_cache_len <= LRU 上限` |
| `crates/bifrost-admin/src/ws_payload_store.rs:263` | `retention_days` patch 生效 |
| `crates/bifrost-admin/src/metrics.rs` | `MetricsSnapshot.memory_used > 0` |

### 集成测试

- `human_tests/api-metrics.md`：验证 `/api/metrics` `memory_used`、`memory_total`、`cpu_usage`、`qps` 字段（`api-metrics.md:28, 68-105`）。
- `human_tests/api-system.md`：验证 `/api/system/overview` 的 `metrics.memory_used` > 0（`api-system.md:112, 176-181`）。

### 压测（TLS 解包 / 不解包 / SSE / WS）

采样脚本示例：

```bash
curl -s 'http://127.0.0.1:8800/_bifrost/api/metrics' \
  | jq '{ts:.timestamp, rss_bytes:.memory_used, rss_mb:(.memory_used/1024/1024)}'
```

| 场景 | 请求量 | 目标 |
| --- | --- | --- |
| HTTP | 10 000 请求 | RSS 峰值 <= 300 MB |
| TLS 解包 | 10 000 请求 | RSS 峰值 <= 300 MB |
| TLS 不解包 | 10 000 请求 | RSS 峰值 <= 300 MB（当前 721 MB，未达标，需 Phase 4） |
| SSE | 100 连接 × 1000 事件 | RSS 峰值 <= 300 MB（当前 558 MB，未达标，需 Phase 4） |
| WebSocket | 50 连接 × 500 消息（Python 客户端） | 成功率 100%，RSS 稳定 |

## Review / Fix / Test 闭环

- **第 1 轮**：核对 body probe、body/frame cleanup task、metrics snapshot 字段是否与代码一致；跑受影响单元测试。
- **第 2 轮**：基于最新 diff 复查 `human_tests/api-metrics.md` / `human_tests/api-system.md` / `human_tests/memory-sqlite-cache-optimization.md` 与索引一致性；重放压测采样 RSS。
- **第 3 轮（按需）**：Phase 4 落地时补充 BodyRef、SSE 连接级上限、规则匹配缓存对应测试。

## 校验要求

- 优先执行受影响单元与集成测试：
  - `cargo test -p bifrost-admin body_store:: frame_store:: ws_payload_store:: metrics::`
  - `cargo test -p bifrost-storage config_manager::`
- 再执行 `rust-project-validate`：fmt / clippy / `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在最终范围需要完整本地 CI 时执行。

## 风险与决策

| 风险 | 决策 |
| --- | --- |
| Passthrough 大 body 跳过规则/脚本，业务逻辑差异 | 语义可解释；detail 明确标记截断；阈值可配置化 |
| SSE 连接级上限未落地 → 长连接内存仍可能失控 | Phase 4 目标；当前依赖 per-frame `preview_limit` + 及时清理连接监控 |
| 严格分层存储 + BodyRef 未落地 → 未命中解包流量仍占内存 | Phase 4 目标；当前 BodyStore inline+file 已缓解，仍需 DB 分流 |
| Metrics 采集频率过高导致 CPU 抖动 | `MetricsCollector` 采集周期可配置；默认 1s 快照 |
| WebSocket 压测缺少 websocat | 用内置 Python WS 客户端替代，作为长期方案 |
| retention_days 调低后历史检索窗口变短 | 通过 Admin API 热更新，用户可根据磁盘空间自行取舍 |

## 文档更新要求

- 更新 `human_tests/api-metrics.md`、`human_tests/api-system.md`、`human_tests/memory-sqlite-cache-optimization.md`。
- 更新 `human_tests/readme.md` 索引。
- README / 协议 / Hook 文档暂不修改；Phase 4 落地后再补 CLI/Web 变更说明。
