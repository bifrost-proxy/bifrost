# Web Admin AI Agent Chat Threads

## 功能模块说明

Agent Chat 右侧 Threads 列表用于展示 `/_bifrost/api/im-gateway/agent/sessions/all` 返回的会话摘要。历史会话数量变多时，原实现会把所有线程按钮一次性挂到 DOM，导致 Agent Chat 首屏和滚动性能下降。

本次优化让 Threads 列表默认只开放最近 20 条，用户滚动到底部后可点击 Load more，每次追加 20 条。同时列表内部使用虚拟滚动，只渲染当前可视窗口附近的线程行，避免大量历史会话造成页面卡顿。

## 实现逻辑

- 保持后端 `sessions/all` API 不变，前端仍接收完整摘要数组并沿用 `dedupeThreads()` 的排序与去重。
- `AgentThreadListCard` 维护本地 `visibleLimit`，默认值为 20。
- 当线程数量变化时，`visibleLimit` 不低于 20，且不超过当前线程总数；如果当前选中线程位于 20 条之后，提升 `visibleLimit` 到可覆盖该选中线程，保证深链和已选会话可见。
- 实际渲染数组为 `threads.slice(0, visibleLimit)`。
- 使用 `@tanstack/react-virtual` 绑定 Threads 容器，按估算行高生成虚拟行，仅渲染可见行与少量 overscan。
- 列表底部在 `visibleLimit < threads.length` 时展示 Load more 按钮；每次点击将 `visibleLimit` 增加 20。
- 右键删除菜单、选中态、running 状态点、来源/Runner 标识、亮暗主题颜色继续使用现有 `theme token` 与样式对象。

## 依赖项

- 复用 WebUI 现有依赖 `@tanstack/react-virtual`。
- 不新增后端 API、不新增 npm 依赖。

## 测试方案

### 单元测试

本次改动主要是 React 组件交互，现有仓库未引入 React Testing Library。单元级覆盖通过 TypeScript 编译、ESLint 和 Playwright DOM 断言兜底；不新增纯函数单测。

### E2E 测试

- 新增 `web/tests/ui/agent-chat-threads.spec.ts`，构造超过 40 条线程的 mock `sessions/all`。
- 验证默认展示加载计数为 `20 / N`。
- 验证点击 Load more 后加载计数增加到 `40 / N`。
- 验证虚拟列表 DOM 中的线程行数量显著小于数据总量。
- 验证滚动到底部后 Load more 可见并可继续追加。

### 真实场景测试

- 更新 `human_tests/im-gateway-agent.md`，新增 Agent Chat Threads 大量历史性能回归用例。
- 同步更新 `human_tests/readme.md` 中 IM Gateway Agent 的用例数和说明。
- 按用例真实打开 Agent Chat 页面，验证默认 20 条、Load more 步进、虚拟滚动和亮暗主题可读性。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认 20 条、每次追加 20 条、虚拟滚动。
- 检查 `AgentChatSection.panels.tsx` 是否保留选中态、右键删除、running 点和主题 token。
- 执行 `git status --short`、`git diff`。
- 运行 Web 相关最小测试：`pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "thread"`，以及 `pnpm --dir web exec tsc --noEmit`。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、human_tests 索引和 E2E 断言覆盖。
- 复跑受影响 Playwright 用例和 Web 静态检查。
- 若发现交互或文档缺口，继续追加第 3 轮直到关闭。

## 校验要求

- 先按 `e2e-test` 技能完成相关 UI E2E 和 human_tests。
- 收尾前按 `rust-project-validate` 技能执行格式、lint、相关构建/测试，并至少运行一次 `cargo test --workspace --all-features`；如果因时间或环境无法完成，需要记录阻塞与风险。

## 文档更新要求

- 更新 `human_tests/im-gateway-agent.md`。
- 更新 `human_tests/readme.md` 的 IM Gateway Agent 行。
- 本次不改变 README、CLI help、API 协议或配置项。
