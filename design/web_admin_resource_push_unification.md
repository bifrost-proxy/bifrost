# Web Admin 资源 Push 通道统一

## 背景

Bifrost Web 管理端历史上每个「非流量」资源都各自轮询：Values、Scripts、Replay、Settings 各自定时 `GET`，规则页面还额外拉自己的列表。这带来 3 个问题：多标签重复拉取、页面切换时短暂看到旧数据、后端修改后不能立即传达到 UI。

自 2026 年上半年起，Admin API 引入 `/api/push` WebSocket，通过一个连接同时承载 traffic delta、metrics、以及一批资源快照/增量。本文档在此把当前**真正落地**的能力做完整梳理：多数资源已经 push-first，但并不是「所有请求都消失了」——首屏 REST 拉取和详情按需 GET 仍在，push 负责状态收敛与失效通知。

## 用户目标验证清单

### 必须实现

- 一个 push 连接可以同时订阅 `traffic` / `overview` / `metrics` / `values` / `scripts` / `replay saved requests` / `replay groups` / `settings scopes`。
- 客户端可以在同一 WebSocket 上通过发送 JSON 订阅片段动态开关任意一路能力，无需重建连接。
- 服务端首次收到 `need_*` 时下发一份全量快照（initial snapshot），随后按事件推送增量。
- 客户端断线重连后能拿回最新快照，不丢事件。
- 单个客户端由 `x_client_id` 参数标识，可以做去重与淘汰。

### 必须不破坏

- 老的 REST 端点保留可用：`GET /api/values`、`/api/scripts`、`/api/replay/*`、`/api/config/*`、`/api/rules` 仍工作。
- 前端在 push 通道不可用时能优雅回退到定期 REST 拉取（degrade path）。
- Rules 列表仍走 REST（`GET /api/rules`），push 只用于变量/脚本增量。
- Settings 按 scope 订阅：只订阅当前页面用到的 scope，避免跨模块串扰。

### 必须真实验证

- Playwright：多标签同时打开 Values 页面，一个标签修改后另一个立即刷新，且只有 1 条 WebSocket 建连。
- CLI：`bifrost status` 的 TUI push 通道可以同时接收 `settings_update` + `metrics`。
- E2E：`test_traffic_push_e2e.sh` 覆盖 traffic delta 的初次快照与增量。
- E2E：`test_values_hot_reload.sh` 与 `test_scripts_admin_api.sh` 覆盖 values / scripts push 触发。

## 产品语义

### 「push-first + REST fallback」而不是「纯 push」

不同资源接入 push 的深度不同：

| 资源 | 首屏来源 | 增量来源 | 备注 |
| --- | --- | --- | --- |
| Values | REST + push 快照 | `values_update` push | 前端偏好订阅 |
| Scripts | REST + push 快照 | `scripts_update` push | 与 values 同 |
| Replay saved requests | push 快照 | `replay_saved_requests_update` push | 详情/发送仍走 REST |
| Replay groups | push 快照 | `replay_groups_update` push | — |
| Settings（scope 化） | REST + push 快照 | `settings_update` push（按 scope） | 支持 `performance`、`agent`、`sync` 等 scope |
| Rules 列表 | REST | 无独立 push（复用 values push 做变量补全和校验） | 大文件按需读 |
| Traffic | push delta + REST detail | `traffic_delta` push | 保持 |
| Metrics | push interval | `metrics` push | `metrics_interval_ms` |
| Breakpoint 状态 | push | `breakpoint_settings_updated` push | 独立事件 |

也就是说：文档不能写「所有 GET 都消失」，Rules 列表和详情、Replay 详情、Values 单条编辑仍走 REST，push 负责失效通知与快照同步。

### `x_client_id` 与订阅片段

- 建连 URL：`ws://host/_bifrost/api/push?x_client_id=<id>&need_overview=true&need_values=true&need_scripts=true&settings_scopes=performance,agent&metrics_interval_ms=500`。
- 建连后仍可通过 text frame 发送 JSON `{ "need_values": true }` 或 `{ "settings_scopes": ["performance"] }` 追加订阅，服务端会：
  - 更新 `PushSubscription` 字段。
  - 对新增的 `need_*` 或新增的 `settings_scopes` 立即下发对应快照（`send_initial_traffic_delta` / `build_settings_update` / `build_values_update` 等）。
- 前端 `web/src/services/pushService.ts` 使用同一 client 处理所有资源事件（`case 'values_update'` / `case 'scripts_update'` / `case 'settings_update'` / `case 'replay_saved_requests_update'` / `case 'replay_groups_update'` / `case 'breakpoint_settings_updated'`）。
- 每个 client 由 `x_client_id` 做桶级淘汰，防止同一浏览器多刷时无限堆积连接。

## 技术细节

### 服务端

- 文件：`crates/bifrost-admin/src/push.rs`
  - `PushMessage` 枚举承载所有事件：`ValuesUpdate` / `ScriptsUpdate` / `SettingsUpdate` / `ReplaySavedRequestsUpdate` / `ReplayGroupsUpdate` / `BreakpointSettingsUpdated`。
  - `PushSubscription { need_values, need_scripts, need_replay_saved_requests, need_replay_groups, settings_scopes: Vec<String>, ... }`。
  - `build_values_update` / `build_scripts_update` / `build_replay_saved_requests_data` / `build_replay_groups_data` / `build_settings_update(scope)` 构造快照。
  - `PUSH_CHANNEL_CAPACITY = 64` 有界队列 + `try_send` 淘汰慢客户端。
  - `MAX_SUBSCRIBED_IDS = 500` 限制 pending id 集合。
- 文件：`crates/bifrost-admin/src/handlers/websocket.rs`
  - 建连处理 query string 中的 `need_*` / `settings_scopes` / `x_client_id`。
  - 支持热订阅：`match key { "need_values" => ..., "need_scripts" => ..., "need_replay_saved_requests" => ..., "need_replay_groups" => ..., "settings_scopes" if !value.is_empty() => ... }`。
  - `settings_scopes` 超过 `MAX_SETTINGS_SCOPES` 时截断并 dedup。
  - `sub.pending_ids.len() > MAX_SUBSCRIBED_IDS` 时截断保护。

### 前端

- 文件：`web/src/services/pushService.ts`
  - `Subscription` 类型：`need_values` / `need_scripts` / `need_replay_saved_requests` / `need_replay_groups` / `settings_scopes` / `need_overview` / `need_metrics` / `metrics_interval_ms`。
  - `switch (msg.type)` 分发到各 store。
- Store：
  - `web/src/stores/useValuesStore.ts` — 接 `values_update`。
  - `web/src/stores/useScriptsStore.ts` — 接 `scripts_update`。
  - `web/src/stores/useReplayStore.ts` — 接 `replay_saved_requests_update` / `replay_groups_update`（保留 `x_client_id` 用于 send 请求）。
  - `web/src/stores/useSettingsStore.ts`（或分散的 Settings 页 hook）— 接 `settings_update`，按 scope 匹配。
  - `web/src/stores/useTrafficStore.ts` — 接 `traffic_delta` / `overview`。

### CLI TUI

- `crates/bifrost-cli/src/commands/status_tui.rs` 用同一 push URL：`ws://.../api/push?need_metrics=true&need_values=true&need_scripts=true&settings_scopes=performance,agent,sync&metrics_interval_ms=...`。
- 单测 `parses_performance_settings_update_from_push_message` 保证 CLI 侧能解析 `settings_update`。

## Sync 边界

Push 通道只是「资源变更事件的传输层」，本身不参与 Sync 后端；同步逻辑在 `sync/*` 模块内部完成后，通过 `PushMessage::ValuesUpdate` 等触发 UI 刷新。Sync 侧写入本地存储后，各 handler 会调用 `PushManager` 的 `broadcast_*` 广播增量。

## Phase 1 – 通道基座

- WebSocket `/api/push` + `x_client_id` + 有界队列。
- 支持 `need_overview` / `need_metrics` / `traffic_delta`。

## Phase 2 – 资源 push 扩展

- 增加 `values` / `scripts` / `replay saved requests` / `replay groups` 订阅位。
- 增加 initial snapshot 生成器（`build_*_data`）。
- 增加 `broadcast_*` 广播 API 供 handler 侧调用。

## Phase 3 – Settings scope 化

- `settings_scopes` 支持 `performance` / `agent` / `sync` / `certificate` 等分片订阅。
- `build_settings_update(scope)` 按 scope 构造。
- `MAX_SETTINGS_SCOPES` 限流。

## Phase 4 – 前端 store 收敛与 CLI 复用

- 各 store 迁移到 push-first；REST 保留为 fallback + 首屏 warm-up。
- CLI TUI 复用同一 push 通道，避免独立轮询。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/push.rs`
  - `build_replay_saved_requests_data_works_with_empty_store`
  - `build_replay_groups_data_works_with_empty_store`
  - `broadcast_values_sends_values_update`（覆盖 `PushMessage::ValuesUpdate` 分发）
  - `broadcast_settings_scope_filters`（`settings_scopes` 命中/未命中的分发）
- `crates/bifrost-cli/src/commands/status_tui.rs::parses_performance_settings_update_from_push_message`

### E2E 测试

- `e2e-tests/tests/test_traffic_push_e2e.sh` — traffic delta / overview / metrics 快照与订阅。
- `e2e-tests/tests/test_values_hot_reload.sh` — Values 修改经 push 反映到订阅端。
- `e2e-tests/tests/test_scripts_admin_api.sh` — Scripts CRUD + push。
- `e2e-tests/tests/test_replay_rules.sh`、`test_replay_body_decode.sh`、`test_replay_websocket_frames.sh` — replay 相关，间接覆盖 replay push。
- `e2e-tests/test_utils/ws_channel_limit_probe.js` — `x_client_id` 淘汰与 `MAX_SUBSCRIBED_IDS` 保护。

### Web UI 测试

- （新增或扩展）`web/tests/ui/push-multi-resource.spec.ts`
  - 同时订阅 `need_values` + `need_scripts` + `settings_scopes=performance`。
  - 在另一标签修改 value → 立刻收到 `values_update`。
  - 修改 script → 收到 `scripts_update`。
  - 修改 performance scope 设置 → 收到 `settings_update`，其他 scope 无事件。

### 真实场景 human_tests

- `human_tests/api-push.md` — TC-APU-08 `x_client_id`、TC-APU-* 覆盖各资源订阅。
- `human_tests/admin-cross-site-security.md` 中 `x_client_id` 的示例仍适用。

## Review/Fix/Test 闭环

### 第 1 轮

- Push 事件与 REST 一致：所有 `broadcast_*` 都在对应 handler 写完存储后触发。
- Rules 页面确认仍使用 REST 拉列表，未误接 push。

### 第 2 轮

- 端到端做 push 断线重连测试：断线后重连是否会重发全量。
- 覆盖 `settings_scopes` 从「多 scope」缩减到「单 scope」时，服务端是否清理旧订阅，避免继续下发多余 scope。

## 风险与决策

- **Rules 列表不完全 push 化**：规则文件较大，首屏 REST 拉列表摘要更省内存；未来若做全量增量，需要针对大文件规则做行级 diff。
- **Replay 详情按需 GET**：保留 REST 拉详情，push 仅做失效通知。
- **多 tab 复用连接**：目前每 tab 一个连接，靠 `x_client_id` 桶淘汰；跨 tab 共享是可选优化，暂未落地。
- **`visible_ids` 精细订阅**：`traffic` 侧可能进一步只订阅可见 id 集合以省流量，属于后续优化，非当前范围。
