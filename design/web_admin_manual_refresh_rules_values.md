# Web Admin Rules / Values 同步策略设计

## 背景

Bifrost Web Admin 早期文档把 `/rules` 与 `/values` 两个页面描述为“页面级首次加载 + 手动刷新”模型：进入页面时通过 HTTP GET 拉一次快照，用户点击 Refresh 按钮后再手动拉一次。

实际实现在 2026-Q2 已经继续演进：`/rules` 与 `/values` 都不再单纯依赖 HTTP，也不再依赖轮询，而是 **首次 HTTP 拉取 + WebSocket `need_values` push 增量** 的混合模型。手动刷新按钮已下线。

本文件用于把这份差异固化为设计文档，避免后续实现或 review 依据过期语义作决策。

## 用户目标验证清单

### 必须实现

- 进入 `/values` 页面时，若本地 `useValuesStore` 为空，立即触发一次 `fetchValues()`（HTTP GET `/api/values`）。
- 进入 `/values` 或 `/rules` 页面后，通过 `pushService.connect({ need_values: true })` 建立 WebSocket 订阅。
- 收到 `values_update` 消息时，由 `applyValuesSnapshot(data.values)` 合并到 store，覆盖既有键值。
- 离开页面（组件 unmount）时，调用 `updateSubscription({ need_values: false })` 并 `disconnectIfIdle()` 释放 WebSocket。
- `/rules` 页面首次进入若规则列表为空，触发一次 `fetchRules()`；后续规则编辑器所需 values 快照由 push 通道供给。
- 全局启动阶段不再周期轮询 `rules` / `values`；不再对 push 通道额外拉取。

### 必须不破坏

- 后端 `/api/rules` / `/api/values` 语义不变。
- WebSocket 协议 `need_values` 字段与 `values_update` 消息名保持稳定。
- 已有 rule share import、values inline 引用、admin API 调用不受影响。
- 其它页面（Traffic、Metrics、Notifications）的订阅字段互不干扰。
- 后端 `crates/bifrost-admin/src/push.rs` 的 `need_values` 广播逻辑保持不变。

### 必须真实验证

- 打开 `/values` 页面，验证有一次 HTTP GET `/api/values` 与随后的 WebSocket `need_values` 订阅。
- 在另一个进程或 CLI 通过 admin API 修改一个 value，验证 push 消息触发 UI 更新，不需要用户刷新页面。
- 离开 `/values` 页面若无其它订阅者，验证 WebSocket 被 `disconnectIfIdle()` 释放。

## 产品语义

### Rules 页面

- 规则列表：仍走按需 HTTP GET；用户新增/修改/删除规则通过既有 admin API 触发本页 store 更新。
- 全局 values 快照：走 push 通道，`RuleEditor` 中引用 `${values.xxx}` 时基于 store 最新快照，不需要额外拉取。

### Values 页面

- 首次进入：`fetchValues()` 做一次 HTTP GET 拉取。
- 后续更新：`pushService` 订阅 `need_values`，`values_update` 消息合并到 store。
- 卸载：`updateSubscription({ need_values: false })` + `disconnectIfIdle()`。
- 手动刷新按钮：已下线。用户如需强制重拉，可以刷新整页。

### 与旧方案的差异

- 旧文档：`Rules` / `Values` 视为“同一种手动刷新模型”。
- 现状：`Rules` 是“按需 GET 规则 + push values”，`Values` 是“首次 GET values + push values”。
- 两个页面都不再周期轮询。
- 后端不需要为这两个页面额外准备 SSE 或长轮询通道。

## 技术细节

### 前端关键源码

- `web/src/pages/Values/index.tsx`
  - `useValuesStore` 提供 `values`、`fetchValues`、`applyValuesSnapshot`。
  - `useEffect` 检测 `values.length === 0` 时触发 `fetchValues()`。
  - `pushService.connect({ need_values: true })` 建立订阅，`onValuesUpdate` 挂 handler。
  - unmount 清理 `updateSubscription({ need_values: false })` + `disconnectIfIdle()`。
- `web/src/pages/Rules/index.tsx`
  - `useRulesStore` 提供 `fetchRules`。
  - `pushService.connect({ need_values: true })` 与 `onValuesUpdate` 保证 RuleEditor 能拿到最新 values 快照。
  - 首次进入若 store 为空调用 `fetchRules()`。
- `web/src/services/pushService.ts`
  - 提供 `connect(subscription)`、`updateSubscription(subscription)`、`disconnectIfIdle()`、`onValuesUpdate(handler)`。

### 后端关键源码

- `crates/bifrost-admin/src/push.rs`
  - `ClientSubscription` 结构包含 `need_values` 字段。
  - `values_update` 通过 `Message::ValuesUpdate` 发出。
- `crates/bifrost-admin/src/handlers/websocket.rs`
  - WebSocket handshake 支持 `need_values` 字段。
- REST 层：`GET /api/values`、`GET /api/rules` 保持既有 handler。

### CLI + Web + Admin API 边界

- CLI：`bifrost value set/get/list` 等命令写入后触发 admin push；本次不变。
- Web：改动集中在 `Values/index.tsx` 与 `Rules/index.tsx`。
- Admin API：HTTP 与 WebSocket 协议均无变更。

## Sync 边界

- Rules / Values 均属于本地或云端规则/变量存储；sync 是它们的持久化通道。
- 本次改动只影响“Web UI 如何获取最新数据”的传输策略，不影响 rule/value 是否参与 sync。
- push 消息 `values_update` 语义与之前保持一致：任何 admin 写入触发广播。

## 实现切分

### Phase 1：Values 页面接入 push（历史 Phase）

- 移除手动 Refresh 按钮。
- 首次进入触发一次 HTTP GET。
- 挂 push 订阅并合并。
- 卸载释放。

### Phase 2：Rules 页面接入 push（历史 Phase）

- 规则列表按需 GET。
- Values 通过 push 通道供 RuleEditor。
- unmount 清理。

### Phase 3：全局层收敛（历史 Phase）

- `useGlobalDataSync()` 移除 rules / values 相关轮询。
- push 通道成为增量唯一来源。

### Phase 4：文档现代化（本次）

- 更新本文件，明确“Rules 混合、Values 混合”语义。
- 更新 human_tests / readme 与相关 e2e 脚本索引。
- 若产品未来选择“纯 push-first（首次不 GET）”，需要单独 Phase 5 设计。

## 测试方案

### 单元测试

- `pnpm --dir web exec vitest run src/stores/useValuesStore`（校验 `applyValuesSnapshot`）
- `pnpm --dir web exec vitest run src/stores/useRulesStore`
- `pnpm --dir web exec tsc --noEmit`

### E2E 测试

- Playwright（涉及页面）：
  - `web/tests/ui/rules-*.spec.ts` 中的既有用例
  - `web/tests/ui/values-*.spec.ts` 中的既有用例
- 后端脚本：
  - `e2e-tests/tests/test_values_admin_api.sh`：真实调用 admin API 写入 value，验证 push 通道能被订阅端收到。
- 关联脚本：`e2e-tests/tests/test_values_admin_api.sh`

### 真实场景测试 human_tests

- `human_tests/api-push.md` 覆盖 push 通道多字段订阅路径。
- 新增或复用 `human_tests/values-*.md`、`human_tests/rules-*.md`（若已有），断言：
  - 打开 `/values` 页面后网络面板出现一次 `GET /api/values`。
  - CLI 修改 value 后，页面不需要刷新即可看到更新。
  - 离开页面若无其它订阅，WebSocket 被关闭。
- 同步更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec playwright test tests/ui`（按 tag/glob 限定）
- `cargo test -p bifrost-admin --lib push`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 按项目 `rust-project-validate` 要求执行；如无法完整跑 `cargo test --workspace --all-features`，记录阻塞与替代验证。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Values 首次 GET、Rules 首次 GET、push 订阅、unmount 清理。
- 复核 diff：`Values/index.tsx`、`Rules/index.tsx`、`pushService`、`useValuesStore`、`useRulesStore`。
- 重点 review：
  - 是否会重复订阅（多次 `connect`）。
  - 组件多次挂载 / 快速切页时 push 通道是否被过早断开。
  - `applyValuesSnapshot` 是否覆盖删除的键（而不是 merge-only）。
- 运行：受影响 vitest + Playwright + `test_values_admin_api.sh`。

### 第 2 轮

- 复审第 1 轮修复后的最新 diff。
- 重点 review：
  - 是否有页面重新引入了周期轮询（regression 风险）。
  - `RuleEditor` 中 values 引用是否稳定命中最新快照。
- 复跑受影响测试；若仍有回归追加第 3 轮直到关闭。

## 风险与决策点

- **纯 push-first 方案**：短期不落地。首次 GET 保留是为了避免 WebSocket 建立前的空白期出现空列表 UX。
- **push 消息可靠性**：若 WebSocket 中途掉线，`pushService` 会重连；断线期间的 values 变更需要通过重连后的一次快照恢复。当前 `values_update` 是全量快照事件，可以自然覆盖。若后续改成增量事件，需要重新评估断线补偿策略。
- **多标签页并发**：多标签页各自订阅 push；后端广播是 fan-out，不会因订阅方多而漏发。
- **可扩展性**：本模型可以推广到其它中低频写、需要 UI 实时反映的资源，例如 auth-status、system-proxy-status。若未来引入相似需求，可参考 `need_values` 的对称设计增加字段而非新增独立通道。
