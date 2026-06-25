# Agent Chat Plan Density

## 功能模块说明

Agent Chat composer 的 Plan 胶囊用于提示当前任务进度。它属于辅助状态信息，不能占用输入框卡片的正文布局空间，也不能把输入内容、hint 或发送按钮向下挤。

本轮优化将 Plan 胶囊放到 composer track 的绝对定位浮层中，垂直位置位于 Token/Context HUD 进度线之上；Token/Context HUD 仍贴近输入卡片上沿，输入框卡片内部保持默认两行高度。

## 实现逻辑

1. `AgentChatSection.tsx` 将 `AgentChatPlan` 从 Ant Design `Space` 的垂直布局中移出，直接作为 `composerTrack` 的子浮层渲染。
2. `AgentChatSection.styles.ts` 中 `planPanel` 改为绝对定位，左右对齐 composer track 内边距，桌面端位于 `top: -64px`，窄屏位于 `top: -60px`。
3. `planCapsule` 保持固定最大宽度和单行省略，避免长任务名称撑宽页面。
4. `planPopover` 继续相对胶囊向上浮出，hover/focus 时展示完整步骤列表。

## 依赖项

- `web/src/pages/AI/AgentChatSection.tsx`
- `web/src/pages/AI/AgentChatSection.styles.ts`
- `web/tests/ui/agent-chat.spec.ts`
- `human_tests/chat-plan-density.md`

## 测试方案

### 单元/前端测试

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "keeps plan content compact" --reporter=line`

### E2E 测试

- 使用 Playwright mock Agent Chat stream 返回 7 条 plan steps。
- 断言 Plan 胶囊浮在 Token/Context HUD 上方。
- 断言输入框保持正常顶部 padding，胶囊不占用输入卡片空间。
- 断言 hover 后详情浮层仍在胶囊上方并保持 5 条可视高度。

### 真实场景测试

- 更新并执行 `human_tests/chat-plan-density.md` 中 TC-CPD-01 到 TC-CPD-04。
- 使用当前源码本地服务打开用户给定 history URL，确认完成态 `Done 4/4` 胶囊悬浮在 Context 进度条上方。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标、`AgentChatSection.tsx` 渲染层级、`AgentChatSection.styles.ts` 定位和 Playwright 布局断言。
- 执行 `git status --short`、`git diff`。
- 运行 TypeScript 构建和 focused Playwright 用例；发现布局偏移或测试失败立即修复。

### 第 2 轮

- 基于第 1 轮后的最新 diff 再次复核 Plan、Token/Context HUD、输入框高度、popover 和亮暗主题。
- 复跑 focused Playwright 用例和真实页面检查。
- 若仍发现胶囊遮挡或占位，继续追加 Review/Fix/Test 轮次。

## 校验要求

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "keeps plan content compact" --reporter=line`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD|keeps plan content compact" --reporter=line`
- `cargo test --workspace --all-features`
- `make coverage`
- rust-project-validate

## 文档更新要求

- 更新 `human_tests/chat-plan-density.md` 的 Plan 胶囊位置预期和执行记录。
- `human_tests/readme.md` 只维护相关模块索引行，不新增全局汇总数字。

## 残余风险

- 极窄窗口下 Plan 胶囊、Token/Context HUD 和消息底部可能争抢垂直空间；通过窄屏 Playwright 断言和真实页面检查覆盖。
