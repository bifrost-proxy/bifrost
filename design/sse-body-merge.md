# SSE 正文合并方案（Body ↔ Messages 同源）

## 背景

早期 Bifrost 的 SSE 详情页依赖 `/frames/{frame_id}` 逐条回补消息，同时把 `payload_preview` 拷贝到 `responseBody` 用作首屏兜底。这条链路存在两个真实问题：

1. Messages 面板首屏必须等所有 frame 请求返回，一条 100 行的 GPT/Claude 流式响应会触发 100 次 HTTP GET。
2. Body 和 Messages 是两个独立数据源，正文里的 `payload_preview` 与 frames 里的完整 payload 会因为 trim/换行不同而对不齐，用户在 Body/Messages 之间切换会看到细微差异，Replay 时也会因为 Body 缺失原始尾部而 diff 出错。

新方案把「SSE 详情页的原始流」收敛成一份数据源：
- 服务端提供 `/traffic/{id}/sse/stream` 结构化事件流（活跃或历史都可拉）。
- 前端订阅到事件后，一份写入 Messages 面板（结构化），另一份 `raw` append 到 `responseBody`（原始字节）。
- Body 面板永远等于「浏览器视角所见的原始 SSE 字节流」，Messages 面板永远等于「同一字节流的结构化解析」。

本方案专门描述这个「Body 与 Messages 同源合并」的实现语义与验收边界。与之相关的服务端流细节请见 [`design/sse-stream-v2.md`](./sse-stream-v2.md)；持久化 raw 字节的边界请见 [`design/sse-ws-frame-persistence.md`](./sse-ws-frame-persistence.md)。

## 用户目标验证清单

### 必须实现

- SSE 详情页打开时 Messages 与 Body 首屏一致，不再依赖 `/frames/*` 二次拉取。
- 活跃 SSE 连接下，Messages 与 Body 同步增长；两个面板的最后一条事件文本必须字节相同（除去 SSE 事件之间的 `\n\n` 分隔符差异）。
- 已结束的 SSE 连接：Messages 面板由前端本地 parse `responseBody` 得到，不再订阅流。
- OpenAI 风格 `data: [DONE]` 事件必须同时出现在 Messages（作为最后一条 event）和 Body（作为最后一段原文）。
- 合并写入是幂等的：即便订阅链路重连回放同一段事件，Body 与 Messages 都不会重复。
- Replay 面板复用同一份 store（`useReplayStore.sseEvents`），MessagesPanel 复挂载不丢事件。

### 必须不破坏

- 非 SSE 流量的 Response Body/Messages 行为不变。
- `/api/traffic/{id}/frames` 接口仍保留（WebSocket 详情、既有 API 消费者仍在用），只是 SSE 详情不再依赖它做首屏。
- Body 的解压/编码/大小限制策略与其他 HTTP 流量保持一致，不因 SSE 走特例路径。
- Traffic 列表的 status/size 字段不因合并策略变化。

### 必须真实验证

- CLI 用 `curl` 命中一个真实 SSE upstream，`bifrost traffic get <id>` 与 Web UI Body/Messages 三处内容一致。
- E2E `replay_sse_live_stream_keeps_tail_events` 与 `replay_sse_live_stream_keeps_done_event` 通过。
- 手工回归 `human_tests/proxy-websocket-sse.md`：Messages 首屏无 `/frames` 请求、Body 与 Messages 同步增长。

## 产品语义

### Body 是 raw 视图，Messages 是结构化视图

Bifrost 内部把「一次 SSE 响应」当成一段字节流：`event:` / `data:` / `id:` / `retry:` 行以及事件之间的空行都属于原始字节。Body 面板负责展示这段字节流，Messages 面板负责按 SSE 语义把它拆成一条一条 `SseEvent { id, event, data, retry, raw }`。

因此合并规则只有一条：**Messages 的每一条 event 必须来自同一段字节流，Body 必须包含所有 Messages 事件对应的 raw 字节**。前端不允许在 Messages 里凭空构造 event，也不允许 Body 只保留 `data:` 而丢弃事件分隔行。

### 活跃 vs 已结束

- 活跃（`traffic.state == "streaming"`）：前端建立 `EventSource /traffic/{id}/sse/stream?from=begin&batch=1`，服务端推送 JSON `{ id, event, data, retry, raw, seq }`，前端 append 到 `sseEvents` 并把 `raw` append 到 `responseBody`。
- 已结束（`traffic.state == "completed"` 或 `error`）：不再订阅流，前端调用 `parseSSEBody(responseBody)` 本地解析成 `sseEvents`，Body 直接展示 `responseBody`。

这个切换由 `useTrafficStore` 中的 traffic 状态字段控制，MessagesPanel 与 BodyPanel 都从同一个 store 读取，不需要各自维护订阅生命周期。

### 幂等 append 规则

`appendSseResponseBody(raw: string)`（`web/src/stores/useTrafficStore.ts`）按 `seq` 单调递增判断是否已经写入：

- 服务端每条事件带 `seq`（自 SSE 流开始的 0-based 序号）。
- 前端记录 `lastAppendedSeq`；如果新事件的 `seq <= lastAppendedSeq`，直接丢弃 raw append。
- Messages append 同样按 `seq` 去重，避免 SSE 断线重连时出现幽灵事件。

## 技术细节

### 服务端 `/traffic/{id}/sse/stream`

- 定义位置：`crates/bifrost-admin/src/handlers/traffic.rs`（`subscribe_sse_stream`，行 311+）。
- 查询参数：
  - `from=begin|tail`：默认 `begin`，即从流开始位置全量重放。`tail` 仅推送订阅之后的新事件。
  - `batch=1`：允许把连续的小事件合并成一条 SSE 帧下发，降低前端渲染压力。默认关闭。
- 响应格式：SSE，`event: message` + `data: <json>`，`data` 中包含 `{ id, event, data, retry, raw, seq }`。
- 关闭条件：traffic 已结束且事件全部下发完成时，服务端发送一条 `event: finish`（synthetic finish）并关闭连接。

### 前端合并链路

- Store：`web/src/stores/useTrafficStore.ts` 暴露 `sseEvents`、`responseBody`、`appendSseEvent(event)`、`appendSseResponseBody(raw)`、`resetSseState()`。
- Hook：`web/src/hooks/useSseStreamSubscription.ts`（若已存在）负责建立 `EventSource`、处理 `error` 时的指数退避重连、以及在 unmount 时 close。
- BodyPanel：`web/src/components/TrafficDetail/panes/Body/index.tsx` 直接读 `responseBody`，不感知合并逻辑。
- MessagesPanel：`web/src/components/TrafficDetail/panes/Messages/index.tsx` 直接读 `sseEvents`，同样不感知合并逻辑。
- Replay 复用：`web/src/pages/Replay/components/MessagesPanel.tsx` 从 `useReplayStore.sseEvents` 读，语义与主详情页一致。

### CLI + Web + Admin API

- CLI：`bifrost traffic get <id>` 输出 body 字段包含 SSE 原文；`bifrost traffic get <id> --include body,headers` 与 Web Body 一致。SSE live tail 走 `bifrost capture sse <id>`（如已实现）。
- Web：详情页 Body 与 Messages 两个 tab 共享同一数据。
- Admin API：
  - `GET /api/traffic/{id}`：`response_body` 即 raw SSE。
  - `GET /api/traffic/{id}/sse/stream`：live/history 事件流，本方案的核心接口。
  - `GET /api/traffic/{id}/frames`：保留，但 SSE 详情不再消费。

### Sync 边界

- 该合并逻辑只影响本地 traffic 展示；不参与远端 sync。
- 远端 traffic 分享（如果启用）应带上 `response_body` 与 `sse_events` 序列化结果，避免接收端再连本地 SSE stream。

## 阶段拆分

### Phase 1：服务端专用流接口

- 新增 `/traffic/{id}/sse/stream` handler；实现 `from=begin/tail`、`batch` 与 synthetic finish。
- 单元测试覆盖 query parser（`parse_sse_stream_options`、`parse_sse_stream_from`，位于 `handlers/traffic.rs`）。

### Phase 2：前端 store 与订阅收敛

- `useTrafficStore` 引入 `appendSseEvent` / `appendSseResponseBody` / `resetSseState` 三个动作。
- 详情页统一从 store 消费，删除按 frame 拉取的旧代码。
- Replay 页面复用同一 store 接口。

### Phase 3：Messages 与 Body 同步保活

- TrafficDetail 主布局改用 Splitter（详见 [`design/sse-messages-panel.md`](./sse-messages-panel.md)）。
- Response Panel `keepAliveTabs` 常驻 `Messages`，避免面板 unmount 清空。

### Phase 4：清理旧路径与文档

- 删除或标注 `payload_preview` 兜底逻辑。
- 更新 `docs/getting-started.md`、`site/src/content/docs/getting-started/installation.md` 中 SSE 段落。
- 归档旧 `sse-body-merge` 中「frames+payload_preview」的过渡说明。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/traffic.rs`：
  - `raw_body_query_flags_are_parsed_independently`
  - `raw_body_query_flags_use_first_raw_value`
  - `parse_sse_stream_from_defaults_to_begin`（如已存在同类用例）
- 前端 store：Vitest 单测覆盖 `appendSseEvent` 按 `seq` 去重、`appendSseResponseBody` 幂等 append。

### E2E 测试

- `crates/bifrost-e2e/src/tests/replay_sse.rs`：
  - `replay_sse_live_stream_keeps_tail_events`（活跃流保尾）。
  - `replay_sse_live_stream_keeps_done_event`（OpenAI `[DONE]` 事件）。
- `e2e-tests/tests/test_sse_frames.sh`：curl 走真实 mock upstream，断言 Body 与 Messages 内容一致。
- `e2e-tests/mock_servers/sse_echo_server.py`：作为可控 upstream，同时被上述两组测试复用。

### 真实场景测试 human_tests

- `human_tests/proxy-websocket-sse.md`：
  - TC-PWS-SSE-01：curl 命中 openai 兼容 upstream，Web 详情页首屏 Messages 与 Body 一致，无 `/frames` 请求。
  - TC-PWS-SSE-02：Body 面板向下滚动到底，等待新事件，Messages 面板同步新增。
  - TC-PWS-SSE-03：关闭 EventSource 后重新打开详情页，走本地 parse 路径，事件顺序不变。
- 关联 `human_tests/api-traffic.md` 的 TC-ATR-24：SSE detail live 流不丢尾部事件。

### 覆盖率与项目校验

- 前端：`pnpm --filter web test -- useTrafficStore`（涵盖 store 单测）。
- 后端：`cargo test -p bifrost-admin sse_stream`、`cargo test -p bifrost-e2e replay_sse_live_stream`。
- 全量：`cargo test --workspace --all-features`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 本机遵守 no-local-coverage 约定，不跑 `make coverage` / `make coverage-unit`，依赖远端 CI。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：Body 与 Messages 是否同源，活跃/结束状态是否都覆盖。
- 复核 diff：`handlers/traffic.rs`、`useTrafficStore`、TrafficDetail 布局、Replay MessagesPanel 是否都涉及。
- 重点 review：`payload_preview` 旧路径是否有残留写入 Body 的分支；SSE 重连是否会重复 append。
- 复测：前端 store 单测、`replay_sse_live_stream_*`、`test_sse_frames.sh`。

### 第 2 轮

- 修复第 1 轮问题后再跑一遍，同时手工在 Web 打开一个真实 GPT 会话验证。
- 检查 `docs/getting-started.md` 与 `site/` 站点 SSE 描述是否与实现一致。
- 断言 CI 中 `replay_sse_live_stream_*` 在 Windows runner 上仍在超时预算内。

## 风险与决策

- **服务端 SSE stream 何时 EOF**：管理端 SSE 连接在部分平台可能延迟关闭；测试客户端必须按「目标事件数 + 完成标记」自主关闭，参考 `replay_sse` 中 `stop_after_finish` 处理。
- **`payload_preview` 是否彻底移除**：短期保留兜底（服务端仍会填充），前端不再消费；等 traffic v2 存储稳定后再删。
- **重连一致性**：`from=begin` 强制服务端重放全部事件，前端 `seq` 去重保证幂等；如未来引入 `from=since_seq` 需扩展 store 校验。
- **Base64/二进制事件**：SSE 规范只允许 UTF-8，非法字节由服务端替换为 U+FFFD 并原样落 raw；不引入 base64 分支避免复杂化。
