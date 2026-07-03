# SSE 详情专用流方案 v2

## 背景

Bifrost Traffic 详情页需要展示 SSE（Server-Sent Events）响应：既包括正在进行的活跃流式响应（例如 GPT/Claude/Trae 的对话流），也包括历史已完成的响应重放。旧实现走 `/traffic/{id}/frames` + 前端轮询，存在三个真实问题：

1. **首屏放大**：一次 GPT 会话若包含 200 条 event，前端需要 200 次 `/frames/{frame_id}` 请求才能拿全 Messages。
2. **Body 与 Messages 双源**：`/frames` 只返回结构化字段，正文里的 SSE 原始字节需要另走 `/traffic/{id}` 的 `response_body`，两处数据可能因为存储时机不同而不一致。
3. **尾部丢失**：Windows CI 上偶发的 EOF 延迟会让消费方在活跃流量结束后仍等待收尾事件，超过 per-test timeout。

新方案 v2 引入一个专用的服务端接口 `GET /api/traffic/{id}/sse/stream`，把 SSE 详情从 frames 中解耦。它同时承载 live tail 和 history replay，让前端一次订阅就能拿全所有 event，并在结束时下发 synthetic finish 让消费方主动 close，规避管理端 SSE HTTP 连接 EOF 不及时的问题。

## 用户目标验证清单

### 必须实现

- 提供 `GET /api/traffic/{id}/sse/stream` 接口，支持 live 与 history 两种模式共用同一 handler。
- 支持 `from=begin`（默认，全量重放）与 `from=tail`（仅推送新事件）。
- 支持 `batch=1` 把连续多条 event 合并成一条下发帧，降低前端渲染压力。
- 每条事件带 `seq` 单调序号，前端可按 `seq` 去重与断线续传（v2 只做 seq 幂等，不做 `from=since_seq` 续传）。
- 流量结束后，服务端下发一条 synthetic `finish` 事件，通知消费方所有事件已发完；消费方随后可主动关闭连接。
- OpenAI 风格 `data: [DONE]` 与其他 upstream 终止事件必须被完整下发。
- Windows CI 上的 e2e 测试用「事件计数 + finish 标记」自主关闭订阅，不再依赖 SSE 连接自然 EOF。

### 必须不破坏

- `/api/traffic/{id}/frames`、`/api/traffic/{id}/frames/{frame_id}` 保留原语义，供 WebSocket 详情与既有 API 消费者使用。
- 普通 `GET /api/traffic/{id}` 的 `response_body` 字段与 SSE stream 内容一致（同源，参考 [`design/sse-body-merge.md`](./sse-body-merge.md)）。
- Body 编码 / 解压 / 大小限制策略与其他 HTTP 流量一致，不因 SSE 走特例。
- CLI `bifrost traffic get` 与 Admin API 响应结构保持稳定。

### 必须真实验证

- Web UI 打开活跃 GPT 会话，Messages Tab 首屏无 `/frames` 请求，Body 与 Messages 同步增长。
- E2E `replay_sse_live_stream_keeps_tail_events` / `replay_sse_live_stream_keeps_done_event` 通过。
- CLI 通过 mock SSE upstream 拉一段 100 事件流量，Body 与 Messages 字节一致，事件 `seq` 严格 0~99。

## 产品语义

### 一份流，两种消费模式

`GET /api/traffic/{id}/sse/stream` 内部按流量当前状态自适应：

- 若 traffic 仍在活跃：从存储里回放已有事件，再挂载 broadcast subscriber 接收新事件；结束时下发 synthetic finish 并关闭。
- 若 traffic 已结束：一次性回放所有事件，末尾下发 synthetic finish 并关闭。

消费方不需要区分 live/history，行为对称。

### `from=begin` 与 `from=tail`

- `from=begin`（默认）：先重放全部历史事件（含 `seq`），再进入 live 阶段。适合详情页首次打开。
- `from=tail`：跳过历史，只推送订阅时刻之后的新事件。适合已经从 `/traffic/{id}` 拿到 `response_body` 的场景，只需要增量。

### `batch=1` 合并策略

后端在活跃阶段的短时间窗口内如果积压多个 event，可以把它们序列化成一个 JSON 数组，一次 SSE 下发。默认关闭；前端在批量场景下（例如 GPT-4o 高频 token）显式打开。

### 幂等 `seq`

每条事件从 0 开始按到达顺序单调递增。前端记录 `lastAppendedSeq`，重连或断线再连时按 `seq` 去重；服务端不承担续传状态，简化实现。

### Synthetic finish

流量结束时下发：

```
event: finish
data: {"seq":<last_seq>,"reason":"upstream_eof"|"proxy_cancel"|"client_disconnect"}
```

消费方看到 `event: finish` 后可以立即 `EventSource.close()`，避免等待 HTTP 连接 EOF。这是本方案对 CI 稳定性最关键的一条约定。

## 技术细节

### 服务端 handler

- 位置：`crates/bifrost-admin/src/handlers/traffic.rs`
  - 行 76：`else if let Some((id, after)) = rest.split_once("/sse/stream")` 路由分发。
  - 行 82：`Method::GET => subscribe_sse_stream(state, id, req.uri().query()).await`。
  - 行 311：`async fn subscribe_sse_stream`。
  - 行 384~447：`SseStreamFrom` / `parse_sse_stream_from` / `SseStreamOptions` / `parse_sse_stream_options`。
- 依赖：`crates/bifrost-storage`（读取历史 SSE 事件）、tokio broadcast（推送 live 事件）、`crates/bifrost-proxy` 中的 SSE 拦截路径（写入 broadcast）。

### 事件格式

- 每条 SSE 输出：
  - `event: message`
  - `data: {"seq":N,"id":..,"event":..,"data":..,"retry":..,"raw":".."}`
- Finish 输出：
  - `event: finish`
  - `data: {"seq":N,"reason":".."}`
- `raw` 字段是该事件对应的原始字节（含 `event:` / `data:` / `id:` 行与末尾空行），用于前端追加到 `responseBody`。

### 前端订阅

- Hook：`useSseStreamSubscription(id)` 负责建立 `EventSource /api/traffic/{id}/sse/stream?from=begin&batch=1`。
- Store：`useTrafficStore` 暴露 `appendSseEvent` / `appendSseResponseBody` / `resetSseState`（详见 [`design/sse-body-merge.md`](./sse-body-merge.md)）。
- Replay：`useReplayStore` 与 `web/src/pages/Replay/components/MessagesPanel.tsx` 使用同一份订阅逻辑。
- 关闭策略：收到 `event: finish` 立即 close；network error 走指数退避重连，重连仍带 `from=begin` 并依赖 `seq` 去重。

### 存储与 broadcast 关系

- SSE 事件由 proxy 侧拦截并写入 traffic 存储；同时通过 tokio broadcast 通知订阅者。
- 存储层保留原始 raw 字节以及结构化字段，供历史回放。
- `sse_stream_flush_bytes`（默认 256 KB）与 `sse_stream_flush_interval_ms`（默认 1000 ms）控制存储写入节奏，见 `crates/bifrost-admin/src/handlers/config.rs` 行 152-153 / 186-187 / 992-993 / 1008-1009 / 1102-1103 / 1141-1142。
- 这两个参数由 `web/src/pages/Settings/tabs/PerformanceTab.tsx` 暴露给用户，能通过 `PUT /api/config/performance` 修改。

### CLI + Web + Admin API

- CLI：
  - `bifrost traffic get <id>`：返回 body（包含 SSE 原文）。
  - `bifrost traffic get <id> --include body,headers`：与 Web Body 一致。
  - `bifrost capture sse <id>`（若已实现）走同一 stream 接口 tail 消费。
- Web：详情页 Body Tab + Messages Tab 从同一 store 读，SSE 订阅由容器层管理。
- Admin API：
  - `GET /api/traffic/{id}/sse/stream?from=begin|tail&batch=0|1`：本方案定义。
  - `GET /api/traffic/{id}/frames`：保留，SSE 详情不再消费。
  - `GET /api/traffic/{id}`：`response_body` 含 SSE 原文。

### Sync 边界

- 该 stream 接口属于本地管理端，不参与 remote sync；远端展示流量走既有 traffic export 通道。
- Broadcast subscriber 只对本地 admin 客户端有效；push proxy websocket（`crates/bifrost-admin/src/push.rs`）仍走原本的 push 通道。

## 阶段拆分

### Phase 1：接口与查询解析

- 定义 `SseStreamFrom` / `SseStreamOptions`，实现 `parse_sse_stream_options` / `parse_sse_stream_from`。
- 单测覆盖 query 解析边界。
- 路由挂到 `/api/traffic/{id}/sse/stream`。

### Phase 2：live + history 复用同一 handler

- 读取历史事件回放。
- 挂 broadcast subscriber 接收 live 事件。
- 完成时下发 synthetic finish。
- `batch=1` 合并策略。

### Phase 3：前端订阅链路

- `useSseStreamSubscription` hook；store 端 `appendSseEvent` + `appendSseResponseBody`。
- 主详情页、Replay 页复用同一 hook。
- 关闭策略：finish 事件主动 close，避免依赖 HTTP EOF。

### Phase 4：CI 稳定性与文档

- `crates/bifrost-e2e` 中的 `replay_sse_live_stream_*` 用「事件计数 + finish」自主关闭。
- 更新 `human_tests/api-traffic.md` TC-ATR-24。
- 更新 `docs/getting-started.md` 与 `site/src/content/docs/getting-started/installation.md` 的 SSE 章节。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/traffic.rs`：
  - `raw_body_query_flags_are_parsed_independently`（行 179）
  - `raw_body_query_flags_use_first_raw_value`（行 284）
  - 新增：`parse_sse_stream_from_defaults_to_begin`、`parse_sse_stream_options_enables_batch_when_requested`。
- `crates/bifrost-admin/src/handlers/config.rs`：验证 `sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 默认值与 update 语义。

### E2E 测试

- `crates/bifrost-e2e/src/tests/replay_sse.rs`：
  - `replay_sse_live_stream_keeps_tail_events`（行 23-26、344）
  - `replay_sse_live_stream_keeps_done_event`（行 29-32、464）
  - 断言 URL 使用 `{}/traffic/{}/sse/stream?from=begin&batch=1`（行 193）。
- `e2e-tests/tests/test_sse_frames.sh`：curl 走真实 mock upstream。
- `e2e-tests/mock_servers/sse_echo_server.py`：可控 SSE upstream。

### 真实场景测试 human_tests

- `human_tests/api-traffic.md` TC-ATR-24：SSE detail live 流不丢尾部事件、OpenAI `[DONE]` 事件、synthetic finish 均正常。
- `human_tests/proxy-websocket-sse.md`：
  - TC-PWS-SSE-01 首屏 Body/Messages 一致；
  - TC-PWS-SSE-07 手工在 Web 触发一条 GPT 会话，Messages 到达 `[DONE]` 后 EventSource 立即 close，Network 面板中 SSE 连接 pending 立即结束。
- 服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin sse_stream`
- `cargo test -p bifrost-e2e replay_sse_live_stream_keeps_tail_events`
- `cargo test -p bifrost-e2e replay_sse_live_stream_keeps_done_event`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `rust-project-validate`
- 本机 no-local-coverage 约定：不跑 `make coverage`，依赖远端 CI。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：live + history 是否共用；finish 是否稳定下发；seq 是否连续；batch 合并是否可选。
- 复核 diff：`handlers/traffic.rs`、`bifrost-proxy` SSE 拦截、前端 hook + store 是否全部涉及。
- 重点 review：Windows CI 上 SSE HTTP 连接 EOF 延迟是否被 finish 事件绕过；`from=tail` 是否可能丢掉订阅瞬间同时到达的事件。
- 复测：`replay_sse_live_stream_*`、`test_sse_frames.sh`、Web 手工。

### 第 2 轮

- 修复后再复跑，人工 GPT/Claude 会话验证 3 分钟不掉事件、finish 后前端立即 close。
- 检查 `PerformanceTab` 中修改 `sse_stream_flush_bytes` / `sse_stream_flush_interval_ms` 后新 traffic 的写入节奏是否符合预期。
- 复核文档：`design/sse-body-merge.md` / `design/sse-messages-panel.md` / `design/sse-ws-frame-persistence.md` 是否引用一致。

## 风险与决策

- **旧 frames 是否彻底删除**：v2 不删除 `/frames`，只是详情页主路径不再消费，避免破坏 WebSocket 详情与外部 API 集成。彻底删除放到 traffic v2 存储改造时再讨论。
- **`payload_preview` 兜底**：服务端仍写入 payload_preview 供旧客户端兜底；前端 v2 忽略该字段。
- **`from=since_seq` 续传**：v2 不做，`from=begin` + seq 去重已经覆盖断线重连；后续如遇到长会话回放代价过高再引入。
- **Broadcast lag / lost**：tokio broadcast 有容量上限，慢消费者可能丢包；handler 在检测 lag 时应主动 close 并让前端重连（`from=begin`）而不是静默丢数据。
- **管理端 SSE HTTP EOF**：Windows platform 层的 EOF 传递可能延迟，本方案通过 synthetic finish + 客户端主动 close 完全绕过；测试客户端不能依赖 EOF，否则会 hit per-test timeout。

## 现状对照（2026-07-03）

- 服务端 `/traffic/{id}/sse/stream` 已实现于 `crates/bifrost-admin/src/handlers/traffic.rs`（行 76、82、311+、384+、410+）。
- 存储层 flush 参数默认值 256 KB / 1000 ms（`handlers/config.rs`）。
- 前端订阅链路已落地：`web/src/stores/useTrafficStore.ts`、`useReplayStore.ts`、`web/src/pages/Replay/components/MessagesPanel.tsx`（行 41-42 从 `useReplayStore` 读 `sseEvents`/`wsMessages`）。
- E2E 覆盖：`crates/bifrost-e2e/src/tests/replay_sse.rs` 的 `replay_sse_live_stream_keeps_tail_events` / `replay_sse_live_stream_keeps_done_event`；断言 URL `sse/stream?from=begin&batch=1`；synthetic finish 与 `[DONE]` 均已验证（行 405、455）。
- 详情页主路径已解耦，但底层部分 SSE frame 记录逻辑仍存在，`design/sse-ws-frame-persistence.md` 中给出对应说明。
