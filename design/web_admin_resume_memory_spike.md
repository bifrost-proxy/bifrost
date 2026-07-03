# Web 管理端睡眠恢复与内存放大

## 背景

Web 管理端在长期后台驻留、笔记本合盖 / 断网休眠后回到前台时，历史上会出现两类问题：

1. Push 侧堆积：服务端向所有 WebSocket 无界队列写事件，慢客户端不能及时消费，会撑大内存并拖慢广播路径。
2. 前端补量瞬时压力：页面恢复时同时向 traffic delta / metrics 拉取一大批 backlog，浏览器内存与网络瞬时抖动。

本文档对这两类问题的**已交付防护**与**恢复顺序不变式**做描述。旧文档把已经落地的防护写成「待做事项」，需要更新为「现状 + 剩余优化」的语义。

## 用户目标验证清单

### 必须实现

- WebSocket push 队列有界（`PUSH_CHANNEL_CAPACITY = 64`），慢客户端使用 `try_send` 淘汰而不是无限堆积。
- 服务端支持 `x_client_id` 分桶淘汰重复客户端。
- WebSocket 协议层 Ping/Pong 已启用，保活并检测断开。
- 服务端订阅侧 `pending_ids` 限额为 `MAX_SUBSCRIBED_IDS`，超出截断。
- 前端 `useTrafficStore` 定义 `MAX_PENDING_IDS = 500` / `POLL_MIN_INTERVAL = 200` / `HAS_MORE_BACKOFF_INTERVAL = 500`。
- 页面隐藏（`document.hidden`）时 traffic + metrics push 均暂停，恢复时先恢复 traffic 再恢复 metrics。
- 恢复时重新建立 `/api/push` 连接（而不是复用旧连接状态），且首帧订阅必须包含 `need_traffic` 和 `last_sequence`。

### 必须不破坏

- Overview / metrics / traffic 三路订阅在同一连接上仍可组合。
- CLI 的 push 消费（`bifrost status` TUI）不受前端 visibility 逻辑影响。
- `x_client_id` 依然由前端 sessionStorage 生成并复用。
- 断开 push 期间用户仍能通过 REST 端点主动拉取 traffic detail。

### 必须真实验证

- 手工：合盖 30 分钟后打开页面，2 秒内看到最新流量补齐，不出现浏览器 tab 白屏或崩溃。
- Playwright：模拟 `visibilitychange` hidden→visible，断言 push 连接被销毁并重建、traffic 先于 metrics。
- E2E：使用 `test_traffic_push_e2e.sh` 覆盖 `last_sequence` 的 delta 补量。

## 已实现的防护

### 服务端

- `crates/bifrost-admin/src/push.rs`
  - `PUSH_CHANNEL_CAPACITY = 64`：`let (sender, receiver) = mpsc::channel(PUSH_CHANNEL_CAPACITY);`。
  - `PushClient::send()` 使用 `try_send`，慢客户端自然被淘汰。
  - `MAX_SUBSCRIBED_IDS = 500`：`pending_ids` / traffic id 集合上限。
  - `send_initial_traffic_delta`：拿到 `last_sequence` 后一次性回补差集，避免多轮 pull。
- `crates/bifrost-admin/src/handlers/websocket.rs`
  - Ping/Pong 协议层保活。
  - `x_client_id` 客户端标识与桶级淘汰。
  - `sub.pending_ids.len() > MAX_SUBSCRIBED_IDS` 时截断。

### 前端

- `web/src/stores/useTrafficStore.ts`
  - `MAX_PENDING_IDS = 500`：pending id 集合上限；超出时按 FIFO 丢弃。
  - `POLL_MIN_INTERVAL = 200` / `HAS_MORE_BACKOFF_INTERVAL = 500`：poll 兜底节流。
  - `enablePush()` 内会构造包含 `need_traffic: true` 和 `last_sequence: state.lastSequence || undefined` 的订阅片段。
  - `disablePush()` 显式发送 `need_traffic: false` 并清空 `last_sequence`。
- `web/src/hooks/useGlobalDataSync.ts`
  - 注册 `document.addEventListener('visibilitychange', onVisibilityChange)`。
  - `hidden` 分支：先 `useTrafficStore.disablePush()` 再 `useMetricsStore.disablePush()`，同时 `stopPolling()`。
  - `visible` 分支：先恢复 traffic (`useTrafficStore.enablePush()`)，再恢复 metrics (`useMetricsStore.enablePush({...})`)，若允许则重启 traffic poll。

### 恢复顺序不变式

恢复顺序必须是「traffic 先、metrics 后」。原因：

- WebSocket 首帧订阅决定服务端 `send_initial_traffic_delta` 是否被触发。若前端先只发 `need_metrics` 建连，服务端把这条连接当作纯 metrics 客户端，之后再发 `need_traffic` 就走热订阅路径，不会重放 `last_sequence` 之前的 backlog。
- 反过来先发 `need_traffic` + `last_sequence`，`send_initial_traffic_delta` 会把 backlog 一次性推给客户端。

## 产品语义

- Push 通道是「事件运输层」，服务端不为每个客户端保留长历史；恢复补量依赖客户端上报 `last_sequence`。
- 一旦 `last_sequence` 太旧（超过服务端保留窗口），服务端会返回 snapshot 或回落 REST 全量拉取。
- 前端仅在 `document.hidden === false` 时保持 push 连接，最大程度降低后台耗电与内存。

## 技术细节

### 建连片段（前端）

```ts
pushService.send({
  need_traffic: true,
  last_sequence: state.lastSequence || undefined,
  // 其他资源订阅可同时挂上
});
```

`useTrafficStore.enablePush()` 会真正调用 `pushService.reconnect()` 而不是复用现有 socket 状态，避免其他仍持有 ref 的订阅者把旧连接保活。

### `x_client_id`

`web/src/services/clientId.ts` 在 `sessionStorage['bifrost_x_client_id']` 中生成并存储 UUID；每次建连都作为 query 参数发送，用于服务端桶级淘汰。

### 服务端限流

- `MAX_SUBSCRIBED_IDS`：pending id 集合上限 500。
- `MAX_SETTINGS_SCOPES`：settings scope 上限，防止客户端提交过多 scope 触发放大。
- 每个 client 的 `mpsc::channel(64)` 通道 + `try_send` 即时淘汰。

## Sync 边界

Sync 通道与 push 通道解耦：sync 完成写入后广播 push；后台恢复补量与 sync 无关。

## Phase 1 – 服务端有界队列（已交付）

- `mpsc::channel(PUSH_CHANNEL_CAPACITY)`、`try_send`。
- Ping/Pong。
- `x_client_id` 桶淘汰。

## Phase 2 – 服务端订阅侧限流（已交付）

- `MAX_SUBSCRIBED_IDS = 500`。
- `send_initial_traffic_delta` 支持 `last_sequence` backlog 补量。

## Phase 3 – 前端 traffic store 节流（已交付）

- `MAX_PENDING_IDS`、`POLL_MIN_INTERVAL`、`HAS_MORE_BACKOFF_INTERVAL`。
- `enablePush` / `disablePush` 明确 `need_traffic` 与 `last_sequence` 首帧订阅。

## Phase 4 – 前端 visibility 编排（已交付）

- `useGlobalDataSync` 注册 `visibilitychange`。
- Hidden：disable traffic → disable metrics → stopPolling。
- Visible：enable traffic → enable metrics → startPolling（如未 paused）。

## 仍需谨慎的点（未交付，规划中）

- **`visible_ids` 精细订阅**：只订阅可见 id 集合，进一步降低补量流量。planned, not yet shipped as of 2026-06-17。
- **多 tab 复用单 push 连接**：需要引入 SharedWorker 或 BroadcastChannel。planned, not yet shipped as of 2026-06-17。
- **服务端补量窗口的持久化**：若长时间断开超过内存窗口，客户端仍会退回全量 REST；未来可考虑滚动落 DB 提供更长窗口。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/push.rs`
  - `send_initial_traffic_delta_replays_backlog_from_last_sequence`
  - `try_send_drops_when_channel_full`
  - `x_client_id_bucket_evicts_previous_socket`
- `crates/bifrost-admin/src/handlers/websocket.rs`
  - `pending_ids_truncated_at_max_subscribed_ids`
  - `settings_scopes_truncated_at_max_settings_scopes`

### E2E 测试

- `e2e-tests/tests/test_traffic_push_e2e.sh`
  - 覆盖 `last_sequence` backlog 补量、metrics 交叉订阅、`x_client_id`。
- `e2e-tests/test_utils/ws_channel_limit_probe.js`
  - 探测同 client id 建立多连接时被淘汰的行为。

### Web UI 测试

- `web/tests/ui/traffic-push.spec.ts`
  - 断言 traffic 恢复顺序、`last_sequence` 上报、metrics 后启动。
- （新增）`web/tests/ui/global-visibility-resume.spec.ts`
  - 用 `page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')))` 触发；断言 push 连接销毁与重建顺序。

### 真实场景 human_tests

- `human_tests/api-push.md`
  - TC-APU-08：`x_client_id` 标识 + 桶淘汰。
  - 建议新增 TC-APU-10：合盖恢复补量。
- `human_tests/admin-cross-site-security.md`
  - 中提到的 `x_client_id` 示例仍适用。

启动约束：临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 `useGlobalDataSync` 的 hidden/visible 分支顺序。
- 复核 `enablePush` 是否真的 reconnect，而不是复用旧 socket。
- 复核 `send_initial_traffic_delta` 与 subscription 到达先后关系。

### 第 2 轮

- 长断开场景：断开 30 分钟后 `last_sequence` 超窗口时，服务端行为是否可预测（返回 error / snapshot / 空）。
- Multi-tab：确认多 tab 分别 hidden/visible 时不互相踩到。

## 风险与决策

- **恢复顺序**：traffic 优先是硬约束，改动 `useGlobalDataSync` 需要保持不变。
- **`last_sequence` 缺失**：首次建连或本地被清空时 `last_sequence` 为空，服务端只发未来事件，UI 需主动 REST 首屏。
- **淘汰的可见性**：客户端被淘汰时前端会看到 WebSocket 关闭，通常在 hidden 情况下发生；恢复时按现有 reconnect 逻辑处理。
- **未实现项**：`visible_ids` 与多 tab 共享连接目前不在计划本季度内交付。
