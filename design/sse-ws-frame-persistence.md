# SSE / WebSocket 数据持久化与性能优化

## 背景

Bifrost 需要把每一次代理经过的 SSE / WebSocket 流量都能在 Traffic 详情页展示，同时不能让存储压力压垮长时间运行的桌面进程。旧实现有两个痛点：

1. **WebSocket frame 存储爆炸**：每条 WS 帧独立落一个文件，`ws_${id}_${seq}.bin` 单会话数万个文件，磁盘 inode 与目录列举都吃不消。
2. **SSE 详情放大**：SSE 每条 event 也被当成 frame 记录并通过 `/frames/{frame_id}` 单独回补，详情页首屏放大明显；同时活跃 SSE 又需要一份原始正文供 Body 面板消费，导致同一段字节流被写两遍。

新方案分两条主线：

- **WebSocket**：把每个连接的所有 frame payload 追加到同一个文件，元数据只记 `BodyRef::FileRange { file_id, offset, length }`，避免海量小文件。
- **SSE**：详情页专用流 `/api/traffic/{id}/sse/stream` 与 `useTrafficStore` 合并写入（详见 [`design/sse-stream-v2.md`](./sse-stream-v2.md) 与 [`design/sse-body-merge.md`](./sse-body-merge.md)），底层 SSE frame 记录逐步收敛，但短期仍并存以兼容老消费者。

本方案专门描述这两条主线的持久化与性能优化边界，并把「已经稳定」与「仍在过渡」两类事实明确拆开。

## 用户目标验证清单

### 必须实现

- WebSocket payload 存储：单连接单文件，追加写；元数据 `BodyRef::FileRange { file_id, offset, length }`。
- WebSocket 详情读取 (`/api/traffic/{id}/frames/{frame_id}`) 返回字符串（文本帧）或 Base64（二进制帧）。
- SSE 详情主链路：`/api/traffic/{id}/sse/stream` 直接读取原始字节流并广播 live 事件，不依赖 frames 二次拉取。
- SSE 存储写入节奏可配置：`sse_stream_flush_bytes`（默认 256 KB）与 `sse_stream_flush_interval_ms`（默认 1000 ms）。
- 大流量场景下（100 MB SSE / 1 万 WS 帧）Bifrost 内存增长可控，无 OOM。

### 必须不破坏

- Traffic 列表、Traffic 详情、Replay 页面对 WS / SSE 的展示保持前向兼容。
- 既有 `/api/traffic/{id}/frames` / `/api/traffic/{id}/frames/{frame_id}` 语义保留（WS 详情、外部 API 消费者仍在用）。
- HTTP 明文 / TLS 拦截路径的 Body 编码 / 解压 / 大小限制策略不变。
- 用户对 `sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 的自定义配置在升级后保留。

### 必须真实验证

- 压测：一条持续 30 分钟、每秒 100 帧的 WebSocket 会话，proxy 内存曲线平稳，磁盘目录只有 1 个 payload 文件。
- 压测：一条 100 MB 的 SSE 响应通过 proxy，详情页 Body/Messages 面板同步展示，进程 RSS 增长不超过 200 MB。
- E2E `ws_payload_persistence` 与 `replay_sse_live_stream_*` 通过。
- CLI `bifrost config set traffic.sse_stream_flush_bytes 131072` 后新流量的写入节奏符合配置。

## 产品语义

### WebSocket：追加写 + FileRange 引用

`WsPayloadStore`（`crates/bifrost-admin/src/ws_payload_store.rs`）维护「每个连接一个 payload 文件」的追加写：

- 新 WS 连接分配 `file_id`，落盘路径 `<data_dir>/ws_payloads/<file_id>.bin`。
- 每条帧写入前记录当前 offset，写入后 `length = payload_len`；把 `BodyRef::FileRange { file_id, offset, length }` 存到 frame 元数据。
- 读取时按 offset+length 精准 seek，避免全文件加载。
- 二进制帧在 API 层按需 Base64；文本帧直接返回字符串。
- 连接关闭后 payload 文件保留，供历史详情读取；触达 traffic retention 时统一清理。

### SSE：专用流 + 存储节奏可配

SSE 详情页不再消费 frames，而是从 `/api/traffic/{id}/sse/stream` 拿完整的 event 序列（live + history）。底层 SSE 存储仍写入原始字节流：

- 写入路径设置了 flush 阈值：达到 `sse_stream_flush_bytes` 或距上次 flush 超过 `sse_stream_flush_interval_ms` 才落盘，减少 fsync 次数。
- 默认 256 KB / 1000 ms（对应源码在 `crates/bifrost-admin/src/handlers/config.rs` 行 152-153、186-187、992-993、1008-1009、1102-1103、1141-1142）。
- 用户可通过 Web `Settings → Performance` 或 Admin API `PUT /api/config/performance` 调整。
- 旧 SSE frame 记录逻辑短期保留（供旧 API 消费者），前端 v2 主路径不再消费。

### 过渡状态提示

- WebSocket：本方案的目标态已成为主实现，剩余优化点在监控与 retention。
- SSE：详情页已经彻底走专用流；底层是否完全废弃 frame 语义还需要单独的 traffic v2 存储改造决策，本方案不做绝对承诺。

## 技术细节

### 关键源文件

- `crates/bifrost-admin/src/ws_payload_store.rs`：`WsPayloadStore` 结构与追加 API。
- `crates/bifrost-admin/src/state.rs`、`crates/bifrost-admin/src/lib.rs`：`AppState` 持有 `WsPayloadStore`。
- `crates/bifrost-admin/src/query_service.rs`：读取 frame 时按 `BodyRef::FileRange` seek 文件返回。
- `crates/bifrost-admin/src/push.rs`：Live WS 帧广播到 admin push 通道。
- `crates/bifrost-admin/src/handlers/config.rs`：暴露 `sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 到 REST 与配置存储。
- `crates/bifrost-admin/src/handlers/traffic.rs`：SSE 详情专用流 (`subscribe_sse_stream`，行 311+)。
- `crates/bifrost-proxy/src/proxy/http/websocket/{capture,upgrade}.rs`、`crates/bifrost-proxy/src/proxy/http/ws_decode.rs`：WS 帧拦截 + 写入 payload store。
- `crates/bifrost-proxy/src/utils/tee.rs`：SSE 与其他流式 body 的 tee 复制逻辑。
- `crates/bifrost-storage/src/{config,config_manager,unified_config}.rs`：持久化配置项。

### CLI + Web + Admin API

- CLI：
  - `bifrost config get traffic.sse_stream_flush_bytes` / `... flush_interval_ms`。
  - `bifrost config set traffic.sse_stream_flush_bytes 131072`。
- Web：`web/src/pages/Settings/tabs/PerformanceTab.tsx` 提供表单；`web/src/types/index.ts` 定义 `PerformanceConfig`；`web/src/api/config.ts` 调用后端。
- Admin API：
  - `GET /api/config/performance` 返回当前配置（含 flush bytes / interval）。
  - `PUT /api/config/performance` 更新，字段结构见 `handlers/config.rs` 行 186-187。
  - `GET /api/traffic/{id}/frames`、`/frames/{frame_id}`：WS 详情读取。
  - `GET /api/traffic/{id}/sse/stream`：SSE 详情主路径。

### Sync 边界

- 该配置项是本机 traffic 性能相关，不参与远端 sync。
- WS payload 文件与 SSE 原始字节属于本机存储，不推送到远端；远端仅接收 traffic metadata + 可选摘要。

## 阶段拆分

### Phase 1：WS 追加写落地

- 引入 `WsPayloadStore` 与 `BodyRef::FileRange`。
- 迁移 capture / upgrade 路径写入。
- 详情读取按 FileRange seek。
- 单元 + E2E 覆盖 `ws_payload_persistence`。

### Phase 2：SSE 详情专用流 + 存储节奏

- 实现 `/api/traffic/{id}/sse/stream`（详见 [`design/sse-stream-v2.md`](./sse-stream-v2.md)）。
- 暴露 flush bytes / interval 配置项（当前默认 256 KB / 1000 ms）。
- 前端合并写入 Body 与 Messages（详见 [`design/sse-body-merge.md`](./sse-body-merge.md)）。

### Phase 3：性能压测与 retention 对齐

- 压测：30 min WS / 100 MB SSE，观测 RSS 与磁盘曲线。
- Traffic retention 触达时同时清理对应 WS payload 文件。
- 增补告警：`resource_alerts.rs` 中包含 WS payload 目录大小超阈提示。

### Phase 4：文档拆分与旧路径清理

- 保持本文档描述现状 + 边界；未来若继续收敛，可以拆成两篇：`websocket-payload-append-store.md` 与 `sse-raw-stream-persistence.md`。
- 更新 `docs/getting-started.md` 与 `site/` 的 Performance 段落。
- 清理旧 SSE frame 消费方；确认前端 100% 迁移到 `/sse/stream`。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/ws_payload_store.rs`：追加写、seek 读、并发写入。
- `crates/bifrost-admin/src/handlers/config.rs`：
  - `sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 默认值验证。
  - update 请求应用后返回值与传入一致。
- `crates/bifrost-admin/src/handlers/traffic.rs`：
  - `raw_body_query_flags_are_parsed_independently`（行 179）
  - `raw_body_query_flags_use_first_raw_value`（行 284）

### E2E 测试

- `crates/bifrost-e2e/src/tests/ws_payload_persistence.rs`：验证单连接单文件、多帧顺序、Base64 编码。
- `crates/bifrost-e2e/src/tests/replay_sse.rs::replay_sse_live_stream_keeps_tail_events` / `..._done_event`：SSE 详情流不丢事件。
- `e2e-tests/tests/test_performance_config_admin_api.sh`：配置写入 / 读回验证。
- `e2e-tests/tests/test_sse_frames.sh`：SSE 原始字节写入 → API 读回一致。
- `tests/tls_interception_test.rs`：TLS 拦截下 WS/SSE 存储行为不退化。

### 真实场景测试 human_tests

- `human_tests/rules-e2e-fixtures.md`：真实 WS/SSE fixture 使用说明。
- `human_tests/api-config.md`：`sse_stream_flush_bytes` / `..._interval_ms` 修改路径验证。
- `human_tests/api-system.md`：Settings 页 Performance Tab 手工回归。
- `human_tests/api-traffic.md`：TC-ATR-24（SSE detail live 流），补新 case TC-ATR-25 覆盖 WS payload FileRange 读取一致性。
- 启动约束：临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin ws_payload_store`
- `cargo test -p bifrost-admin sse_stream`
- `cargo test -p bifrost-e2e ws_payload_persistence`
- `cargo test -p bifrost-e2e replay_sse_live_stream`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `rust-project-validate`
- 本机遵守 no-local-coverage 约定；不跑 `make coverage`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：WS 是否单文件、SSE 是否走专用流、flush 配置是否生效。
- 复核 diff：payload store、query service、config handler、proxy 拦截路径、前端 Performance Tab。
- 重点 review：Traffic retention 触达时是否同步删除 payload 文件；文本帧与二进制帧的 Base64 边界是否清晰。
- 复测：`ws_payload_persistence`、`replay_sse_live_stream_*`、Performance API E2E。

### 第 2 轮

- 修复后再复跑，手工压测 30 min WS 会话、100 MB SSE 响应。
- 检查 `resource_alerts` 是否在磁盘接近阈值时告警。
- 复核文档：`design/sse-stream-v2.md` 与 `design/sse-body-merge.md` 中 flush 参数描述保持一致。

## 风险与决策

- **单文件损坏**：单连接单 payload 文件在写入中崩溃可能导致整条会话不可读；缓解：定期 checkpoint、启动时对头部/尾部做一致性校验，损坏则回退为按 offset 尽可能恢复。
- **SSE frame 记录并存**：短期保留旧 frame 记录以兼容外部消费者，可能出现同一段字节流被记录两遍（frame + raw）；本方案接受这一短期代价，等 traffic v2 存储改造统一处理。
- **flush 参数调低带来的 IO 压力**：如果用户把 `sse_stream_flush_bytes` 调到 1 KB 且 `interval_ms` 调到 10 ms，SSD 写入会明显放大；Performance Tab 应在极端值处提示 warning。
- **旧文档描述漂移**：旧版描述的 64 KB / 200 ms 从未落地为默认值；本文档以 `handlers/config.rs` 中实际默认 (256 KB / 1000 ms) 为准，避免代码与文档脱节。
- **retention 与 payload 关系**：Traffic retention 触达时必须同步删除对应 `ws_payloads/<file_id>.bin`；否则会孤儿文件累积。

## 现状对照（2026-07-03）

- WS 侧：`WsPayloadStore` 已上线，`AppState` 与 `push` / `query_service` 均已迁移；`crates/bifrost-e2e/src/tests/ws_payload_persistence.rs` 稳定通过。
- SSE 侧：详情页专用流已上线（`/api/traffic/{id}/sse/stream`）；`sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 默认值 256 KB / 1000 ms。
- 前端：`web/src/pages/Settings/tabs/PerformanceTab.tsx` 表单已经暴露配置项；主详情页 Body/Messages 使用同源 store。
- 后续拆分文档的建议保留；不作为当前必做项。
