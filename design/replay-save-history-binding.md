# Replay 首次保存后的历史绑定

## 背景

Bifrost Replay 页面允许用户从 Traffic 面板导入一个请求快照，或者在 Replay 内自己拼装一个请求。这些"未保存"的请求默认属于 History 的 `unbound` 范围：所有执行记录都可以看到，但只要用户"保存为模板"（即写入 saved requests），后续执行就会带上 `request_id`，走到该 saved request 的历史桶里。

修复前的用户投诉：从 Traffic 导入请求后，第一次点"保存为模板"，页面没有把当前请求切成"已保存请求上下文"。用户回到 History 面板，仍然是 `unbound` 视图，看不到刚刚执行的记录；即便刷新，也无法自动回到这条模板的历史上下文。这条 UX 问题只在"首次保存"路径里出现，因为已保存请求的重复保存链路本来就有 `historyFilter = { type: request, requestId }` 语义。

本次设计的目标是把首次保存后立即把 UI 状态、历史过滤器、持久化的 `selectedRequestId` 全部对齐到新的 saved request，并在刷新后由持久化状态自动恢复上下文，最终由 Playwright 覆盖 Traffic 导入 → 首次保存 → 执行 → History → 刷新五步链路。

## 用户目标验证清单

### 必须实现

- Traffic 导入到 Replay 的临时请求，在首次"保存为模板"成功后，`currentRequest` 立即被替换为后端返回的 saved request，包含 `id` / `is_saved = true`。
- `historyFilter` 立即切成 `{ type: "request", requestId: savedRequest.id }`。
- `uiState.selectedRequestId` 同步指向新 saved request，`selectedHistoryId` 清空，`historyPage` 归 1。
- 保存成功后立刻调用 `loadRecentHistory`；如果当前 mode 是 `history`，再触发 `loadAllHistory`，让分页历史与模板一致。
- 页面刷新（浏览器 F5 或 Web app reload）后，`selectedRequestId` 通过 Zustand `partialize` 恢复；`Replay/index.tsx` 的 effect 根据它自动 `selectRequest`，回到该模板。
- 首次保存后立即执行的请求，其 History 面板 scope 显示当前模板名称，且执行记录可见。

### 必须不破坏

- 已保存请求的重复保存（rename、move group、编辑后保存）仍能保留 `historyFilter` 和历史列表。
- `saveRequest` 失败时不改变 `currentRequest`、`historyFilter` 和 `selectedRequestId`，避免 UI 处于不一致状态。
- Traffic 导入后的临时请求，如果用户不保存直接执行，History 仍走 `unbound` 视图。
- Replay 中的 Cancel、Delete saved request、切换分组、切回 all history 等既有交互不受影响。
- SSE / WebSocket 长连接以及 recent history 的推流不因过滤器切换发生重复注入。

### 必须真实验证

- Playwright 覆盖：Traffic 导入 → 首次保存 → 执行 → History 查看 → 刷新页面 → History 依旧可见。
- 手工回归至少一次“导入 → 首次保存 → History → 刷新”，确认 `selectedRequestId` 持久化恢复。

## 产品语义

### 三种 Replay history 视图

Replay 的 History 侧栏由 `historyFilter` 驱动，共三种：

- `{ type: "all" }`：所有 replay 执行记录。
- `{ type: "unbound" }`：所有尚未绑定 saved request 的执行记录，用于 Traffic 导入或临时构造请求。
- `{ type: "request", requestId }`：某个 saved request 的执行历史。

Traffic 导入后默认落到 `unbound`。首次保存必须把上下文一次性切到 `request`，避免用户看到"记录消失"的错觉。

### saveRequest 成功后的原子切换

`useReplayStore.saveRequest` 在收到后端 200 之后必须一次性完成以下操作：

1. 把后端返回的 saved request（含新的 `id`、`is_saved: true`、`name`、`group_id` 等）作为新的 `currentRequest`。
2. `historyFilter = { type: "request", requestId: savedRequest.id }`。
3. `uiState.selectedRequestId = savedRequest.id`，`uiState.selectedHistoryId = null`，`uiState.historyPage = 1`。
4. 触发 `loadRecentHistory(nextHistoryFilter)`；如果 `uiState.mode === "history"`，再触发 `loadAllHistory()`。

失败路径必须保留原状态，不能只修改一半。

### 持久化与刷新恢复

Zustand `persist` 的 `partialize` 已经把 `selectedRequestId` 纳入持久化字段。`web/src/pages/Replay/index.tsx` 挂载后由 effect 检查：如果 `savedRequests` 里能命中持久化的 `selectedRequestId`，就调用 `selectRequest`，重放上一次 replay 上下文；如果该 saved request 已经被删掉，`selectRequest` 会 fallback 到 `historyFilter = { type: "all" }` 并清空 `selectedRequestId`。

## 技术细节

### 前端 store：`web/src/stores/useReplayStore.ts`

- `saveRequest`（约 375 行起）：
  - `POST /api/replay/requests`（新建）或 `PUT /api/replay/requests/:id`（更新）。
  - 成功后统一 `set({ currentRequest: savedRequest, historyFilter: nextHistoryFilter, uiState: { ...uiState, selectedRequestId, selectedHistoryId: null, historyPage: 1 } })`（对应实现见 411-425 行）。
  - `await loadRecentHistory(nextHistoryFilter)`；`if (uiState.mode === 'history') await loadAllHistory()`。
- `deleteRequest`（约 835 行）：如果被删除的正是 `historyFilter.requestId` 或 `selectedRequestId`，才 fallback 到 `{ type: "all" }` 并清空 selection，避免误伤其他视图。
- `selectRequest`（约 938 行）：切换 saved request 时同步 `historyFilter`；`is_saved === true` 走 `request`，否则走 `all`。
- `partialize`（1345-1365 行）：`selectedRequestId` 与部分 UI 状态入持久化，避免刷新后 filter 与 selection 不一致。
- Live push（`applyReplayStream`，约 1396 行起）：`historyFilter.type === 'request'` 时只接收 `data.request_id === historyFilter.requestId` 的推流，避免旧 `unbound` 推流污染新模板视图。

### 页面：`web/src/pages/Replay/index.tsx`

- 挂载 effect 读取 `useReplayStore.getState().uiState.selectedRequestId`，尝试 `find` 于 `savedRequests`；命中则 `selectRequest(fullRequest)`。
- Traffic 导入入口通过 `importFromTrafficRecord` action 生成 `{ ...record, is_saved: false }` 的临时 currentRequest，`historyFilter` 归 `unbound`（对应 1050-1053 行）。

### Admin API 依赖

- `POST /api/replay/requests` 与 `PUT /api/replay/requests/:id`：返回新的 saved request，其中 `is_saved: true` 与稳定 `id` 是本设计的必要条件。
- `GET /api/replay/history?filter=request&request_id=...`：为 `loadRecentHistory` / `loadAllHistory` 提供 request 维度的历史条目。
- `POST /api/replay/execute`：执行时携带 `request_id`（如果 `is_saved`），后端写入历史时带该 `request_id`，用于前端 filter 命中。

## CLI + Web + Admin API

- CLI：本次改动为 Web UI 交互修复，不新增或修改 CLI 子命令；`bifrost replay` 相关命令保持不变。
- Web：Replay 页面自身逻辑与状态机改动，Traffic 面板右键 "Send to Replay" 保持既有语义。
- Admin API：无新增字段与新端点。所有必要字段（`id`、`is_saved`、`request_id`）都已存在。

## Sync 边界

- Saved request 已属于 replay 存储，与远端 sync（rule / group / port）无关，本设计不涉及 sync。
- History 记录保持本地存储，只在本机 replay 页面展示；不进入 rule/group sync。
- 分享 URL 不涉及 replay history。

## Phase 1 — Store 首次保存原子切换

- 在 `saveRequest` 的新建路径（无 `savedRequests.find(id)` 命中）里补全 `currentRequest / historyFilter / uiState` 的一次性 set。
- 抽出 `nextHistoryFilter = { type: 'request', requestId: savedRequest.id }` 局部变量，避免二次读 `get()`。
- 单元测试：`useReplayStore.test.ts`（如已有）新增 `saveRequest 首次保存后切换 historyFilter 与 selectedRequestId`。

## Phase 2 — Persist 与 index.tsx 恢复

- `partialize` 中确认包含 `uiState.selectedRequestId`。
- `Replay/index.tsx` 挂载 effect 里加读取 `selectedRequestId → savedRequests.find → selectRequest` 的恢复逻辑，仅在 `selectedRequestId && savedRequests.length` 情况下触发；已存在则不再重复触发。
- 兼容：如果 saved request 已被删除，`selectRequest` 走 fallback 到 `all` 并清空 selection。

## Phase 3 — Playwright E2E

- 新增用例 `从 Traffic 导入后首次保存的模板在执行后可见历史，刷新后仍能恢复`（已落地于 `web/tests/ui/admin-replay.spec.ts:125`）。
- 覆盖：
  - 通过 admin push proxy 触发一条 Traffic 记录。
  - 右键 → Send to Replay。
  - 输入模板名并保存。
  - 点击 Send / Execute。
  - 切到 History tab，断言 scope 显示模板名、列表非空。
  - `page.reload()`，断言 saved request 自动选中、History 仍能显示这条执行。
- 复用 `admin-replay.spec.ts` 里的既有 fixture：本地 mock server、临时 data dir、非 9900 端口、`--no-system-proxy`。

## Phase 4 — 文档与 human_tests

- 无 CLI/README 面向变化；不新增 CLI help。
- `human_tests/webui-replay.md`：更新 `TC-WRP-*` 用例，加入 Traffic 导入 → 首次保存 → 刷新链路的手动验证步骤（对应 `human_tests/webui-replay.md` 中的 Replay 保存与刷新章节）。
- `human_tests/readme.md`：同步用例数量。

## 测试方案

### 单元与集成

- `web/tests/unit/useReplayStore.spec.ts` 或类似：
  - `saveRequest 首次保存后 historyFilter/currentRequest/selectedRequestId 一次性切换`。
  - `saveRequest 失败保持旧状态`。
  - `deleteRequest 删除当前 selected 时 fallback 到 all`。
- 若无对应 unit 文件，直接由 Playwright 覆盖。

### Playwright E2E

- `web/tests/ui/admin-replay.spec.ts`：
  - 现有：`从 Traffic 导入后首次保存的模板在执行后可见历史，刷新后仍能恢复`（line 125）。
  - 现有：`从其他 tab 切到 Replay 时立即加载已保存请求列表`（line 20）作为交叉回归。
  - 现有：`Replay 页面保存请求、创建分组、移动并执行，然后查看历史记录`（line 51）保证已保存请求的原路径未破坏。
  - 现有：`Replay 执行中可以点击 Cancel 中止请求`（line 181）保证 Cancel 不因过滤器变化而卡住。

### 真实场景 human_tests

- `human_tests/webui-replay.md`：
  - TC-WRP-11：Traffic 导入 → 首次保存 → 执行 → History → 刷新，全链路 PASS。
  - TC-WRP-12：首次保存失败（模拟 5xx）时 currentRequest 保持临时态。
- `human_tests/traffic-replay.md`：Traffic 面板右键 Send to Replay 后的首次保存回归。

### 环境要求

- 所有 Playwright 与手工验证：临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 store 变更是否只在"新建 saved request"路径触发过滤器切换，避免影响 rename/move 的静默保存。
- 复核 `partialize` 是否保留 `selectedRequestId`，effect 是否幂等。
- 运行 Playwright 5 条 replay 用例。
- 手工执行 `human_tests/webui-replay.md` 相关用例。

### 第 2 轮

- 基于第 1 轮修复后复查 diff。
- 检查 admin push proxy → replay live push 是否仍把新 saved request 的 request_id 推入 History 面板。
- 复跑失败用例。

## 风险与决策

- 决策：`saveRequest` 中的过滤器切换只在新建路径触发，rename/move 不改 filter。这与用户预期一致（rename 不应打断已经打开的 History）。
- 决策：`selectedRequestId` 进入 `partialize`。虽然多 tab 场景下可能出现选中不一致，但比"每次刷新掉回 unbound"更接近用户预期。
- 风险：Traffic 导入的临时请求首次保存后，如果 saved request 立即被其他 tab 删除，`selectRequest` 的 fallback 需要处理 404。当前实现由 `savedRequests` 本地过滤兜底，未走后端 detail 请求。
- 风险：Live push 过滤逻辑与 filter 切换非同一事务，理论上存在极短窗口把 `unbound` 推流误注入。已通过 `historyFilter.type === 'request' && historyFilter.requestId === data.request_id` 的联合判定收敛。

## 实现现状（截至 2026-07-03）

- `web/src/stores/useReplayStore.ts` 的 `saveRequest` 在保存成功后回写 `currentRequest`，将 `historyFilter` 切换为 `{ type: 'request', requestId: savedRequest.id }`，同步重置 `selectedRequestId`、`selectedHistoryId`、`historyPage`，随后立即调用 `loadRecentHistory`；`uiState.mode === 'history'` 时再触发 `loadAllHistory`。
- 持久化 `partialize` 包含 `selectedRequestId`；刷新后 `web/src/pages/Replay/index.tsx` 挂载 effect 依据它在 `savedRequests` 中命中并调用 `selectRequest`，恢复模板上下文。
- Playwright 用例 `从 Traffic 导入后首次保存的模板在执行后可见历史，刷新后仍能恢复`（`web/tests/ui/admin-replay.spec.ts:125`）覆盖全链路；未删除的其他三条 replay 用例保证既有能力不回归。
- 本设计文档无待落地项。
