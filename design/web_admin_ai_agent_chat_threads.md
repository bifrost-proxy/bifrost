# Web Admin AI Agent Chat Threads 列表懒加载与虚拟滚动设计

## 背景

Agent Chat 页面右侧 Threads 列表用于展示历史会话摘要，数据来自 `/_bifrost/api/im-gateway/agent/sessions/all`。当账号累积了大量历史会话（数百到数千条）后，旧实现有两个明显问题：

1. 一次性把所有 `SessionSummary` 渲染成 `AgentThreadListCard` 内部 DOM 行，Agent Chat 页面首屏 mount 时间显著变长。
2. 滚动列表期间由于每个线程行都是常驻 DOM 节点，浏览器 layout / paint 成本随线程总数线性上升，尤其在低配 Mac 或远端浏览器场景下卡顿明显。

本次优化在保持后端摘要接口不变的前提下：

- 默认只把最近 20 条线程加入 UI 数组。
- 提供 `Load more` 按钮，每次追加 20 条。
- 无论 UI 数组多长，实际渲染都通过 `@tanstack/react-virtual` 只挂载可见窗口的行 DOM。
- 若当前选中会话位于 20 条之后（例如深链或右键跳转），自动把窗口撑到覆盖它。

该优化落地于 `web/src/pages/AI/AgentChatSection.panels.tsx`，与 `AgentChatSection.tsx` 主视图协作。

## 用户目标验证清单

### 必须实现

- Agent Chat 首次打开时 Threads 列表默认最多渲染 20 行。
- 列表底部固定入口显示 `Showing X of N`，并提供 `Load more` 按钮，每次点击后可见数增加 20，直到覆盖所有线程为止。
- 使用虚拟滚动，滚动位置离开的行必须被回收，可见窗口 + 少量 overscan 之外不保留 DOM。
- 后端 `sessions/all` 语义保持不变；前端仍复用 `dedupeThreads()` 做去重和排序。
- 如果当前选中线程位于第 20 条之后，`visibleLimit` 必须自动提升到能覆盖它，避免深链或恢复上次会话时 UI 卡在“Showing 20 of N”状态。
- 亮/暗主题、右键删除菜单、running 状态点、来源 / Runner 标识、Selected 高亮态、Escape 关闭菜单等既有交互全部保留。

### 必须不破坏

- 不改变后端 `agent/sessions/all` 或 `agent/sessions/{id}` 的响应结构。
- 不改变 `useAgentSessionStore` 的 store 语义与订阅方式。
- 不改变 push 通道对 `session_updated` / `session_deleted` 的语义。
- 不引入新的运行时 npm 依赖（`@tanstack/react-virtual` 已在 web 包中被 `VirtualTrafficTable`、`useVirtualMessageList`、`SseMessageList` 等复用）。
- 不改变 Agent Chat 侧栏的宽度、折叠交互、Runner 切换菜单等既有布局。

### 必须真实验证

- Playwright：注入 55 条 mock 会话，验证初始计数 `Showing 20 of 55` 与 Load more 步进。
- Playwright：断言当前 DOM 中 `agent-chat-thread-virtual-row` 数量显著小于总条数（虚拟化生效）。
- 真实浏览器：在 dev 或 preview 构建下打开 Agent Chat，验证 Load more、选中态、右键删除、Runner 标识、running 点。

## 产品语义

### 分批加载 vs 一次全量

Threads 列表本身是一个 **只加不减** 的 UI 窗口：

- 初始 `visibleLimit = 20`。
- 用户点 Load more 时 `visibleLimit += 20`，上限为 `threads.length`。
- 每次线程数据刷新（例如 push 通道新增会话、删除会话）都会重新计算 `visibleLimit`：
  - 不低于常量 `INITIAL_THREAD_LOAD_COUNT = 20`。
  - 不高于 `threads.length`。
  - 如果 `selectedThreadId` 不在 `threads.slice(0, visibleLimit)` 中，`visibleLimit` 被撑到能覆盖它。

### 虚拟滚动窗口

即便 `visibleLimit = 500`，DOM 中的线程行也只有当前 scroll 窗口 + overscan 几十个节点：

- Virtualizer 通过 `AgentChatSection.panels.tsx:206` 的 `useVirtualizer({ count: visibleThreads.length, estimateSize: () => THREAD_ROW_ESTIMATE_SIZE })` 构建。
- `THREAD_ROW_ESTIMATE_SIZE = 68` 行高估算基于线程行高度实测；实际由 `measureElement` 校准。
- 虚拟行外层容器带 `data-testid="agent-chat-thread-virtual-space"`，滚动容器带 `data-testid="agent-chat-thread-list"`，方便 E2E 断言。

### 与选中态的关系

`selectedThreadId` 是唯一有权力“撑大” `visibleLimit` 的外部信号。这样保证：

- 从 URL 深链或从 sidebar 点击某个历史线程时，即使它位于 200 条之外，UI 也会自动展开到覆盖它。
- 但普通新增线程（例如 push 通道推送 `session_created`）不会绕过 20 条的懒加载策略。

## 技术细节

### 关键源码位置

- `web/src/pages/AI/AgentChatSection.panels.tsx`
  - `INITIAL_THREAD_LOAD_COUNT = 20`
  - `THREAD_LOAD_INCREMENT = 20`
  - `THREAD_ROW_ESTIMATE_SIZE = 68`
  - `useState<number>(INITIAL_THREAD_LOAD_COUNT)` 保存 `visibleLimit`
  - `useEffect` 根据 `threads.length` 和 `selectedThreadId` 归一化 `visibleLimit`
  - `useMemo` 计算 `visibleThreads = threads.slice(0, Math.min(visibleLimit, threads.length))`
  - `useVirtualizer` 挂到 Threads 容器
  - `Load more` 按钮在 `visibleLimit < threads.length` 时可见，点击后 `setVisibleLimit(current => Math.min(threads.length, Math.max(current, INITIAL_THREAD_LOAD_COUNT) + THREAD_LOAD_INCREMENT))`
- `web/src/pages/AI/AgentChatSection.tsx`：拉起 `AgentThreadListCard`，注入 `threads`、`selectedThreadId`、`onSelect`、`onDelete`、`runnerBySession`。
- `web/src/pages/AI/AgentChatSection.helpers.tsx`：`dedupeThreads()` 依然是唯一去重排序入口。

### 关键 data-testid

- `agent-chat-thread-list` 滚动容器
- `agent-chat-thread-virtual-space` 虚拟总高容器
- `agent-chat-thread-virtual-row` 单行
- `agent-chat-thread-item` 行内可点击区域
- `agent-chat-thread-delete` / `agent-chat-thread-delete-confirm` / `agent-chat-thread-delete-cancel`
- `agent-chat-thread-load-count` 底部计数
- `agent-chat-thread-load-more` 加载更多按钮
- `agent-chat-thread-runner-mark` 来源 / Runner 标识
- `agent-chat-threads-collapse` 侧栏折叠按钮

E2E 用例通过这些 testid 定位，保持后端 push / API 无关。

### 主题与样式

- 沿用 `useThemeStyles()` 计算的 `theme token`：`styles.threadRow`、`styles.threadRowSelected`、`styles.runningDot` 等。
- 不引入硬编码颜色。
- 亮暗主题、Selected/Active 状态、右键菜单的 hover 与 disabled 态由现有 style object 供给。

### 后端 API 与 push

- `GET /_bifrost/api/im-gateway/agent/sessions/all` 保持一次返回全部摘要；本次改动没有引入分页参数。
  - 相关 handler：`crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`
- push 通道保留：`session_updated`、`session_deleted`、`session_created`。
- 不新增字段、不改变枚举、不改变订阅协议。

### CLI + Web + Admin API 边界

- CLI：无相关命令改动。`bifrost cli` 侧的 agent 相关子命令不涉及 Threads 列表渲染。
- Web：改动全部集中在 `AgentChatSection.panels.tsx` 的 `AgentThreadListCard`。
- Admin API：仅使用既有 `sessions/all`，不新增端点、不新增查询参数。

## Sync 边界

- Threads 列表懒加载纯本地 UI 策略，不参与 rule/values sync。
- 不涉及远端配置存储，不影响 `bifrost sync` 状态。
- 不修改 `notification`、`values`、`rules` 相关 push 订阅。

## 实现切分

### Phase 1：常量与状态

- 引入 `INITIAL_THREAD_LOAD_COUNT`、`THREAD_LOAD_INCREMENT`、`THREAD_ROW_ESTIMATE_SIZE` 常量。
- `AgentThreadListCard` 引入 `visibleLimit` 状态与归一化 effect。
- 底部渲染 `Showing X of N` 与 `Load more` 按钮。

### Phase 2：虚拟滚动接线

- 引入 `useVirtualizer`，将 `visibleThreads` 交给虚拟器。
- 虚拟行容器套现有线程行 JSX。
- 保留 running 点、Selected 态、右键菜单、Runner 标识等所有交互。

### Phase 3：选中态自动扩容

- `useEffect` 监听 `selectedThreadId` 与 `threads`，当 selected 位置超过当前 `visibleLimit` 时提升上限。
- 覆盖深链场景。

### Phase 4：测试与 human_tests

- 新增 `web/tests/ui/agent-chat-threads.spec.ts` Playwright 用例：
  - `AI Agent Chat thread list loads in batches and virtualizes rows`
- 更新 `human_tests/im-gateway-agent.md` 增加 Agent Chat Threads 大量历史性能回归用例。
- 更新 `human_tests/readme.md` 中 IM Gateway Agent 行的用例数与说明。

## 测试方案

### 单元测试

本次改动主要是 React 组件交互，仓库未引入 React Testing Library。单元级覆盖由 TypeScript 编译 + ESLint + Playwright DOM 断言兜底。

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec eslint src/pages/AI`

### E2E 测试

- `web/tests/ui/agent-chat-threads.spec.ts::AI Agent Chat thread list loads in batches and virtualizes rows`
  - 通过 `page.route()` mock `sessions/all` 返回 55 条摘要。
  - 断言 `agent-chat-thread-load-count` 文本为 `Showing 20 of 55`。
  - 断言 `agent-chat-thread-load-more` 可见。
  - 断言 `agent-chat-thread-virtual-row` 数量远小于 55（虚拟化生效，实际取决于容器高度）。
  - 点击 `Load more` 后计数变为 `Showing 40 of 55`，再点一次覆盖全部并使 `Load more` 消失。
- 相关 Playwright 文件：
  - `web/tests/ui/agent-chat-threads.spec.ts`
  - `web/tests/ui/agent-chat.spec.ts`
- 关联的 e2e 后端脚本（用于支撑 push 与 sessions 语义未破坏）：
  - `e2e-tests/tests/test_agent_history_pagination_api.sh`
  - `e2e-tests/tests/test_agent_run_timeline_channel_unification.sh`
  - `e2e-tests/tests/test_agent_session_stale_running_reconciliation.sh`
  - `e2e-tests/tests/test_im_gateway_external_runner_delayed_final_state.sh`

### 真实场景测试 human_tests

- `human_tests/im-gateway-agent.md`：Agent Chat Threads 大量历史性能回归用例。
- `human_tests/readme.md`：同步 IM Gateway Agent 分组的用例数与说明。
- 关联用例：`human_tests/agent-chat-history-pagination.md`、`human_tests/agent-session-persistence.md`。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec playwright test tests/ui/agent-chat-threads.spec.ts`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "thread"`
- 按需触发 `rust-project-validate`。若因资源约束无法完整 `cargo test --workspace --all-features`，需要在 PR 记录中说明并附带 `cargo check` 与相关 crate 单测证据。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认 20 条、Load more 步进、虚拟滚动、选中态撑大窗口。
- 逐行 review `AgentChatSection.panels.tsx` 中 `visibleLimit` 归一化 effect 是否覆盖：
  - 线程列表首次填充
  - push 通道追加新线程
  - 删除线程后 `threads.length` 降低
  - `selectedThreadId` 变化
- 检查 `useVirtualizer` 的 `count`、`estimateSize`、`overscan` 是否合理。
- 检查 running 点、右键菜单、Selected 态、Runner 标识是否在虚拟行内正确出现。
- 复跑 Playwright 用例与 `tsc --noEmit`。

### 第 2 轮

- 复审第 1 轮修改后的 diff、human_tests 索引和 E2E 断言覆盖。
- 重点看 push 通道新增线程时 `Showing X of N` 中 N 是否实时反映，且不会把 X 强行拉到覆盖新增会话。
- 复跑受影响 Playwright 用例。
- 如仍有交互或文档缺口，追加第 3 轮直到关闭。

## 风险与决策点

- **20 条阈值是否可调**：第一版硬编码常量，若后续需要按屏幕高度自适应，可拆出配置项。
- **是否需要“Load previous”方向**：当前 `sessions/all` 一次返回全部，UI 只需 append。若未来后端改为服务端分页，需要重新设计上下双向加载。
- **虚拟行高估算不准**：行内可能出现两行标题、Runner 标识等变高元素。当前使用 `measureElement`（Virtualizer 默认支持）自动校准，若发现滚动跳动，需要调整 `estimateSize` 或引入 `dynamicSize` 校准策略。
- **无 React Testing Library**：仓库暂不引入。所有交互靠 Playwright 断言，若后续要加逻辑单测需先评估依赖影响。
