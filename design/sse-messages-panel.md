# SSE Messages 面板展开/折叠丢失修复方案

## 背景

Bifrost Traffic 详情页的 Response Panel 里，Messages Tab 用于展示 SSE 事件流或 WebSocket 消息。用户反馈：在流量还在增长时，展开或折叠 Request/Response 面板会导致 Messages 面板整个消失，重新展开后事件列表被清空并停止追加。

Root cause 有三层：

1. TrafficDetail 原本按「Request 折叠」「Response 折叠」两种布局分支各写了一套渲染树，切换时 Response Panel 组件从 React tree 中被卸载再重建。
2. Response Panel 内 Messages 组件维护本地 `useState` 存储事件列表，组件 unmount 即清空，没有做 store 化。
3. 非折叠模式下 Response Panel 的 Tab 组件没有配置 keep-alive，用户切 Tab 到 Body 再切回 Messages 时也会 unmount 一次。

这三条叠加会让用户「随手切一下面板」就丢失 SSE 尾部数据，严重影响 GPT/Claude 类长流式接口的排错体验。

本方案把面板布局改为「结构稳定 + 内容保活 + 状态外置」三段式，保证 Messages 面板在任意布局 / Tab 切换下都不丢事件。

## 用户目标验证清单

### 必须实现

- 无论怎么展开 / 折叠 Request 或 Response 面板，Messages 面板不清空、不断流。
- 无论怎么在 Response 的 Body / Messages / OpenAI / Trae / DouBao Tab 之间切换，Messages 组件都不 unmount。
- 折叠态下不允许拖拽调整面板高度，避免出现「拖拽到 0 高度反而以为面板消失」的错觉。
- Replay 页面的 Messages 面板与主详情页一致：由 store 提供数据，组件复挂载不清空。

### 必须不破坏

- 折叠 / 展开动画、快捷键、鼠标点击响应保持原状。
- Body / OpenAI / Trae / DouBao 等其他 Tab 的常驻策略仍生效。
- 非 SSE / 非 WS 流量的 Messages Tab 应保持隐藏或空态，不因为保活策略额外占内存。
- Traffic 列表切换到另一条流量时，Messages 面板必须清空，避免上一条流量数据串到当前流量。

### 必须真实验证

- Web UI 打开一条活跃 SSE 流，反复点击 Request/Response 面板的折叠按钮，Messages 面板持续追加事件。
- 打开 Replay，运行同一条请求，Messages 面板中事件顺序与主详情页一致，切 Tab 不丢。
- Playwright 用例覆盖 Messages Tab keep-alive、Splitter 折叠禁拖拽。

## 产品语义

### 面板布局：始终 Splitter，折叠只改 size

TrafficDetail 主内容区一律用垂直 `Splitter` 渲染 Request 与 Response Panel。折叠状态通过给 `Splitter.Panel` 传 `{ size, resizable: false }` 表达；展开状态用默认 resizable Splitter，允许用户拖拽调整比例。这样 React tree 结构不变，组件 identity 稳定，面板内部 state 不会因为布局切换被销毁。

### Tab 保活：Response Panel 常驻 Messages

Response Panel 的 Tab 组件通过 `keepAliveTabs` prop 声明哪些 Tab 需要常驻。基线常驻集合：`["Body", "Messages", "OpenAI", "Trae", "DouBao"]`；Request Panel 常驻 `["Body", "OpenAI"]`。未激活的常驻 Tab 内容用 CSS `display: none` 隐藏，DOM 与组件 state 保留。

### 状态外置：SSE / WS 数据放 store

Messages 组件不再持有事件列表本地 state，改为从 `useTrafficStore`（主详情页）或 `useReplayStore`（Replay 页）读取 `sseEvents` / `wsMessages`。组件复挂载只是重新订阅 store，不影响数据本身。

- 主详情页：`sseEvents` 与 `responseBody` 由 SSE 订阅 hook 同步写入（详见 [`design/sse-body-merge.md`](./sse-body-merge.md)）。
- Replay 页：`sseEvents`、`wsMessages` 由 Replay 执行逻辑写入。
- 切换 traffic 时统一调用 `resetSseState()` / `resetWsState()`。

## 技术细节

### 关键文件

- `web/src/components/TrafficDetail/index.tsx`
  - 行 887~892：`requestPanelProps` / `responsePanelProps` 依据 `hasCollapsed` 计算 `{ size, resizable: false }`。
  - 行 938~982：主布局始终使用 `<Splitter>` + `<Splitter.Panel {...requestPanelProps}>` / `<Splitter.Panel {...responsePanelProps}>`。
  - 行 957：Request Panel `keepAliveTabs={["Body", "OpenAI"]}`。
  - 行 976：Response Panel `keepAliveTabs={["Body", "Messages", "OpenAI", "Trae", "DouBao"]}`。
- `web/src/components/TrafficDetail/Panel/index.tsx`
  - 行 35：`keepAliveTabs?: string[]`。
  - 行 54~：读取该 prop 决定 Tab 是渲染真实 DOM 还是完全跳过；`activeKey` 之外的常驻 Tab 挂 `display: none`。
- `web/src/components/TrafficDetail/panes/Messages/index.tsx`：从 store 读事件，纯展示，不订阅。
- `web/src/stores/useTrafficStore.ts`：`sseEvents`、`appendSseEvent`、`resetSseState`。
- `web/src/pages/Replay/components/MessagesPanel.tsx`：从 `useReplayStore` 读 `sseEvents` / `wsMessages`，与主详情页 Messages 面板行为一致（行 16 引入 store，行 41-42 解构 `sseEvents`/`wsMessages`）。
- `web/src/pages/Replay/components/ResponsePanel.tsx`：Replay 的 Response Panel 也应用同样的 keep-alive 策略。

### CLI + Web + Admin API

- 本方案纯前端布局与 store 改造，不涉及 CLI/Admin API 变化。
- 后端 SSE 事件源仍是 `GET /api/traffic/{id}/sse/stream`（详见 [`design/sse-stream-v2.md`](./sse-stream-v2.md)）。

### Sync 边界

- 不涉及 sync；纯 UI 状态。

## 阶段拆分

### Phase 1：布局稳定化

- 把 TrafficDetail 中「Request 折叠 / Response 折叠 / 双开」三种分支合并成同一个 Splitter 渲染树。
- 折叠通过 `size` + `resizable:false` 表达。
- 单独 review：确保 Splitter 键 `key` 稳定，避免 React 因为 key 变化重建。

### Phase 2：Tab 保活

- Panel 组件接受 `keepAliveTabs`，未激活的常驻 Tab 用 `display:none` 隐藏。
- Request/Response Panel 分别声明各自的常驻列表。
- 单元测试：Tab 切换后卸载事件计数为 0（常驻 Tab）。

### Phase 3：状态外置

- Messages 组件从 store 读事件，删除本地 `useState`。
- SSE / WS 订阅逻辑放在容器层（`useSseStreamSubscription` 或 TrafficDetail 顶层 effect）。
- 切换 traffic id 时 `resetSseState()`。

### Phase 4：Replay 对齐

- Replay MessagesPanel 与 ResponsePanel 复用同一 keep-alive 策略与 store 读取模式，保证「主详情页 → Replay」的心智一致。

## 测试方案

### 单元测试（Vitest / RTL）

- `web/src/components/TrafficDetail/Panel/__tests__/Panel.keepAlive.test.tsx`：
  - `keepAliveTabs_prevents_unmount_on_switch`
  - `non_keepAlive_tabs_unmount_on_switch`
- `web/src/stores/__tests__/useTrafficStore.test.ts`：
  - `resetSseState_clears_events_and_body_on_traffic_change`
  - `appendSseEvent_dedupes_by_seq`

### E2E Playwright

- `web/e2e/traffic-detail-messages.spec.ts`：
  - `messages_panel_survives_request_collapse_toggle`
  - `messages_panel_survives_response_collapse_toggle`
  - `messages_panel_survives_body_tab_switch`
  - `messages_panel_clears_on_traffic_change`

### 真实场景测试 human_tests

- 更新 `human_tests/proxy-websocket-sse.md`：
  - TC-PWS-SSE-04：活跃 SSE 流下反复折叠 Request Panel 10 次，Messages 面板事件数持续增长且顺序不变。
  - TC-PWS-SSE-05：从 Messages Tab 切到 Body 再切回 Messages，不清空。
  - TC-PWS-SSE-06：Replay 一条历史 SSE 流，MessagesPanel 与主详情页事件序列一致。
- 所有服务启动使用 `BIFROST_DATA_DIR=$(mktemp -d)`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `pnpm --filter web test -- TrafficDetail Panel`
- `pnpm --filter web test -- useTrafficStore`
- `pnpm --filter web build`（保证 Splitter 结构变更不引入类型错误）
- 不跑 `make coverage`（no-local-coverage 约定）。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：折叠、Tab 切换、Replay 三条路径 Messages 面板都不清空。
- 复核 diff：TrafficDetail、Panel、Messages、Replay MessagesPanel、useTrafficStore 是否全部涉及。
- 重点 review：折叠态是否漏禁用 resizable；`keepAliveTabs` 名称与 Tab key 是否严格匹配；store reset 时机是否覆盖 traffic id 切换与详情页关闭。
- 复测：Vitest + Playwright。

### 第 2 轮

- 修复后再复跑，人工在 Web 打开真实 GPT/Claude 会话验证 3 分钟不掉事件。
- 复核 Body 与 Messages 数据是否仍然同源（联合 [`design/sse-body-merge.md`](./sse-body-merge.md) 交叉验证）。
- 检查 Console 是否有 React 「Cannot update state on unmounted component」警告。

## 回滚方案

- 出现布局异常时，回退 TrafficDetail 到「折叠分支双渲染」的旧实现；`keepAliveTabs` 与 store 改造独立可回退，不必一次性回滚。
- 保留 feature flag `NEXT_PUBLIC_TRAFFIC_DETAIL_KEEP_ALIVE=1`（如需灰度）在 dev 环境切换新旧策略。

## 风险与决策

- **常驻 Tab 内存**：为每条流量的 Response Panel 常驻 5 个 Tab，可能带来内存增长；因此常驻 Tab 内部组件必须避免 heavy render，Messages 用虚拟滚动，Body 用 Monaco readOnly 懒加载。
- **Tab key 稳定性**：若 Tab label 是 i18n 文本，`keepAliveTabs` 必须匹配内部 `key`（`"Messages"` 而非本地化字符串），避免 i18n 切换导致保活失效。
- **切换 traffic 未 reset**：如果 URL 路由变化未触发 `resetSseState`，会看到上一条 traffic 的尾部事件混入当前 traffic；单测必须覆盖 traffic id 变化路径。
- **折叠 size 计算**：折叠态下 `size` 需要给一个足够小但非 0 的值，避免 Splitter 内部把面板当作 hidden 而卸载子树；通常取 `collapsedPanelHeight` 常量（例如 32px 用于 Header 高度）。

## 现状对照（2026-07-03）

方案已经落地，关键实现位于：

- `web/src/components/TrafficDetail/index.tsx` 行 887-992：统一 Splitter 渲染 + `keepAliveTabs`。
- `web/src/components/TrafficDetail/Panel/index.tsx` 行 35、54：`keepAliveTabs` prop + display:none 保活。
- `web/src/pages/Replay/components/MessagesPanel.tsx` 行 16-68：`sseEvents`/`wsMessages` 从 `useReplayStore` 读，`normalizeSSEEvent` / `normalizeWSMessage` 只做展示归一化。
- `web/src/stores/useReplayStore.ts` 与 `web/src/stores/useTrafficStore.ts`：分别持有主详情页与 Replay 的 SSE/WS 事件序列。
