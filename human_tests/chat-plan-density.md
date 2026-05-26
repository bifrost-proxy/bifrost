# Agent Chat Plan 密度与输入框高度测试用例

## 功能模块说明

验证 Agent Chat composer 中的 Plan 面板和输入框高度控制：

- Plan 是辅助信息，展开后必须紧凑展示，不能抢占主消息阅读区域。
- Plan 不展示二级标题行，只保留真正的 todo steps。
- Plan step 使用 todo 状态图标：完成勾选、进行中旋转、未开始空心圆点。
- Plan step 超过 5 条时必须在 step 列表内部滚动。
- 输入框默认只提供 2 行内容高度，随输入扩高，到 7 行上限后内部滚动。
- 输入框 hint 和发送按钮底部留白与顶部输入留白一致，不能出现大块底部空白。
- 输入框 hint 只提示换行方式，不展示 session id。
- Threads 列表详情 tooltip 只在左侧 runner/source 图标上悬浮超过 0.5 秒后出现。
- 亮色和暗色主题下均需保持可读、无重叠、无布局撑出。

## 前置条件

1. 使用当前源码 WebUI。
2. 使用 Playwright mock Agent Chat stream 构造 7 条 plan steps，避免依赖真实模型返回。
3. 页面入口：`/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。

## 测试用例列表

### TC-CPD-01: Plan 展开态紧凑展示

**操作步骤**

1. 打开 Agent Chat 页面。
2. 发送一条消息，使 mock stream 返回 7 条 plan steps。
3. 检查 Plan 面板展开态。

**预期结果**

- Plan header 高度紧凑，标题 `Plan` 与数量 tag 不撑高。
- 不展示 `plan_updated` title / explanation，例如 `CI Watcher` 或 `Density check` 这类二级标题不出现。
- 每条 step 行高约 24px，step 文本字号为 12px，左侧状态图标尺寸约 14px。
- 长 step 文字单行省略，不把单条 step 撑成多行。

### TC-CPD-01b: Plan step 使用 todo 状态图标

**操作步骤**

1. 打开 Agent Chat 页面。
2. 发送一条消息，使 mock stream 返回 completed、in_progress、pending 三种状态。
3. 检查每条 step 左侧状态展示。

**预期结果**

- completed step 左侧显示勾选图标。
- in_progress step 左侧显示旋转 loading 图标。
- pending step 左侧显示空心待办圆点。
- 不再展示 `Completed`、`In Progress`、`Pending` 文字 tag。

### TC-CPD-02: Plan 超过 5 条后内部滚动

**操作步骤**

1. 保持 TC-CPD-01 的 7 条 plan steps。
2. 检查 `agent-chat-plan-list` 的可视高度和滚动状态。
3. 滚动 plan list 到底部。

**预期结果**

- step list 可视高度等于 5 条 step 的高度。
- 第 6、7 条不继续抬高 composer，而是在 step list 内部滚动查看。
- Plan 面板仍被 composer track 包含，不越界、不遮挡消息区。

### TC-CPD-03: 输入框默认两行并扩高到上限

**操作步骤**

1. 打开 Agent Chat 页面且不输入内容。
2. 检查输入框内容区域高度。
3. 输入 10 行文本。
4. 检查输入框高度与滚动状态。

**预期结果**

- 空输入框内容区域为 2 行高度。
- hint 文案为 `Shift + Enter for a new line`，不包含 `Session:` 或 session id。
- hint 与发送按钮距离输入框底部约 8px，与顶部输入 padding 一致。
- 输入多行后输入框随内容扩高。
- 输入 10 行后输入框内容区域停在 7 行高度，并在输入框内部滚动。

### TC-CPD-04: 亮色与暗色主题均可读

**操作步骤**

1. 在亮色主题执行 TC-CPD-01 到 TC-CPD-03。
2. 切换到暗色主题。
3. 重复检查 Plan 面板、step list、输入框和发送按钮位置。

**预期结果**

- 亮色和暗色主题下 Plan 面板、状态图标、step 文字和输入框 hint 均清晰可读。
- 发送按钮仍位于输入框右下角，不遮挡输入文本或 hint。
- Plan 折叠后不显示 step list，重新展开后仍维持 5 条高度上限。

### TC-CPD-05: Threads tooltip 只在图标延迟触发

**操作步骤**

1. 打开带有至少一条 thread 的 Agent Chat 页面。
2. 将鼠标悬浮在线程标题或 meta 文本区域超过 0.5 秒。
3. 将鼠标移到同一行左侧 runner/source 图标上。
4. 在 0.5 秒前和 0.5 秒后分别检查 tooltip 状态。

**预期结果**

- 悬浮在线程行文本区域不会弹出详情 tooltip。
- 悬浮在左侧图标上不足 0.5 秒不会弹出 tooltip。
- 悬浮在左侧图标上超过 0.5 秒才显示包含 Workspace、Runner、Source、State、Created、Duration 的详情 tooltip。

## 清理步骤

1. 关闭为本用例启动的 Playwright/Vite dev server。
2. 不修改用户真实 Bifrost 数据。

## 执行记录

- 2026-05-26: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "keeps plan content compact|thread list scrolls" --reporter=line --timeout=60000`，2 条真实 Chromium UI 验证通过。覆盖 Plan 不展示二级标题、todo 状态图标替代文字 tag、7 条 step 时只显示 5 条高度并内部滚动、输入框默认 2 行且 10 行内容扩展到 7 行后内部滚动、hint 只显示 `Shift + Enter for a new line` 且不包含 session id、hint/发送按钮底部 8px 留白与顶部 padding 对齐、亮色/暗色主题下布局保持稳定、Threads 详情 tooltip 在线程文本区域 hover 0.65 秒不弹出、在左侧 runner/source 图标 hover 0.45 秒不弹出且超过 0.5 秒后显示。测试启动临时 Bifrost 后端时使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`，未修改用户真实数据。
