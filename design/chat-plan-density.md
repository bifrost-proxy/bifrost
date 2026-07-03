# Agent Chat Plan 胶囊密度优化

## 背景

Agent Chat composer 上方原本用 Ant Design `Space` 垂直堆叠 Plan 胶囊、Token/Context HUD 和输入卡片。任务进入 Plan 模式后，`AgentChatPlan` 组件会撑出一整行高度，把 Token/Context HUD、hint 与输入框向下挤，导致：

1. 桌面/窄屏下每次进入 Plan 模式输入区跳动，用户视觉焦点被打断。
2. 长任务名称加上剩余步骤统计会撑到多行，让原本仅两行高的输入卡片被挤到接近半屏。
3. Plan Popover 需要 hover 触发，但 Space 布局让 Popover 相对锚点漂移，命中区不稳定。

本方案把 `AgentChatPlan` 从 `Space` 中拆出，作为 `composerTrack` 的绝对定位浮层，垂直贴在 Token/Context HUD 进度线上方。Token/Context HUD 仍紧贴输入卡片上沿，输入框卡片保持原始两行高度。Plan Popover 相对胶囊向上浮出，hover/focus 时展开完整步骤列表。

## 用户目标验证清单

### 必须实现

- Plan 胶囊在任何 Plan 状态（`Planning / Executing / Done` 等）都不占用 composer 布局垂直空间；输入卡片高度与 Plan 未开启时完全一致。
- Token/Context HUD 位置保持不变，Plan 胶囊悬浮在 HUD 上沿之上，不遮挡 HUD 进度条。
- Plan 胶囊在桌面端 `top: -64px`、窄屏 `top: -60px`，左右对齐 composer track 内边距。
- 长任务名称使用固定最大宽度 + 单行省略，避免撑宽整个 composer。
- Popover 相对胶囊向上浮出，最多显示 5 条步骤并允许纵向滚动。
- 亮色/暗色主题下胶囊背景、文本、border 都可读，不引入硬编码单主题颜色。
- 页面刷新后仍能从 history URL 恢复完成态 `Done N/N` 胶囊。

### 必须不破坏

- 普通消息输入、发送、`/` slash 面板、queue、stop、线程切换、run 恢复行为不变。
- Token/Context HUD 采集、progress 计算、颜色 threshold 不变。
- Playwright 现有 `agent-chat.spec.ts` 中已通过用例继续通过；仅新增或修改与 Plan 布局相关的断言。
- 单主题 style token（例如 `theme.appearance.primary`、`theme.token.colorBgElevated`）继续使用；不新增 dark/light 分支。

### 必须真实验证

- Playwright mock stream 注入 7 条 plan step 时：胶囊悬浮，输入卡片 offsetHeight 与未开启 Plan 时相等。
- Hover Plan 胶囊后 Popover 弹出，包含 5 条可视步骤 + 滚动。
- 亮暗主题 snapshot 下胶囊对比度足够（文本 vs 背景 contrast ≥ 4.5）。
- 用户给定 history URL 打开完成态会话时，胶囊显示 `Done 4/4` 并浮在 Context 进度条上方。

## 产品语义

### Plan 胶囊状态

| 状态 | 文本模板 | 说明 |
|------|----------|------|
| Planning | `Planning...` | 尚未产出步骤列表，显示 spinner |
| Executing | `Step k/N · <当前步骤简称>` | 单行省略 |
| Paused | `Paused at k/N` | 无 spinner |
| Done | `Done N/N` | 灰色文本，保留 5 秒后可折叠 |
| Failed | `Failed at k/N` | 红色 accent |

胶囊本体不承担细节展示，细节通过 Popover 呈现，避免与输入卡片竞争空间。

### 布局层级

```text
<composerTrack> (relative)
  ├── <planPanel>     (absolute, top:-64px, pointer-events:auto)
  │      └── <planCapsule>       (max-width, ellipsis, hover 触发 Popover)
  │              └── <planPopover> (absolute, bottom:100%)
  ├── <tokenHud>      (紧贴 composer 上沿)
  └── <composerCard>  (两行输入 + 发送按钮)
```

`composerTrack` 是 stacking 上下文根；`planPanel` 采用 `pointer-events: none`，`planCapsule` 恢复 `auto`，避免遮挡 HUD 点击。

## 技术细节

### AgentChatSection.tsx

- 从 `Space` 组件中移除 `<AgentChatPlan />`，改为在 `composerTrack` 的第一个子节点里独立渲染。
- Plan 组件通过 props 接收 `planState`、`onExpand`、`isDark` 等；不再受 `Space` 间距影响。
- 保留 Plan 组件卸载动画：状态由 `Done` 变为 `hidden` 时 fade out。

### AgentChatSection.styles.ts

- `planPanel`：`position: absolute; left: 0; right: 0; top: -64px;` 桌面端；媒体查询 `@media (max-width: 720px)` 覆盖 `top: -60px`。
- `planCapsule`：`max-width: 320px; padding: 4px 10px; border-radius: 999px;` 内文本 `text-overflow: ellipsis; white-space: nowrap;`。
- `planPopover`：`position: absolute; bottom: calc(100% + 4px); max-height: 200px; overflow-y: auto;`。
- 复用现有 `tokenHud*` token 与 `composerTrack` 相对宽度，不新增 z-index 层级污染。

### AgentChatPlan.tsx

- 组件本身无需 layout 假设；只对外提供 `size`, `label`, `stepsPreview`。
- `role="status"` + `aria-live="polite"`，Screen reader 可读到步骤变化，但不打断输入焦点。

## API

无后端契约变更。Plan 数据仍复用现有 `run/plan` SSE 事件流：`plan_started / plan_step_delta / plan_step_completed / plan_finished`。

## CLI

无 CLI 变更。

## Web 与 Admin

Web 端唯一变更点已在“技术细节”描述。Admin API 无变更。

## Sync 边界

不涉及。Plan 数据是 run 内瞬态状态，不进入 sync。

## 实现切分

### Phase 1：布局重构

- `AgentChatSection.tsx` 把 Plan 从 Space 中拆出。
- `AgentChatSection.styles.ts` 新增 `planPanel` 绝对定位样式。
- 手动验证 Plan/无 Plan 状态下输入卡片高度一致。

### Phase 2：胶囊细节

- 单行省略 + 最大宽度。
- Popover 上浮，最多 5 条可视 + 滚动。
- Loading / Paused / Done / Failed 状态视觉差异。

### Phase 3：Playwright 与真实校验

- 更新 `agent-chat.spec.ts`：新增 `keeps plan content compact` 用例；使用 mock stream 注入 7 条 plan steps 后断言胶囊悬浮、输入卡片 offsetHeight 稳定。
- 覆盖 hover Popover、亮暗主题、窄屏媒体查询。

### Phase 4：human_tests 与索引

- 更新 `human_tests/chat-plan-density.md` 用例 TC-CPD-01 ~ TC-CPD-04。
- `human_tests/readme.md` 中相应索引行同步。

## 测试方案

### 单元 / 前端测试

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "keeps plan content compact" --reporter=line`

### E2E 测试

Playwright mock Agent Chat SSE 返回 7 条 plan steps 与最终结果：

- 断言 Plan 胶囊 `getBoundingClientRect().bottom` 小于 Token/Context HUD `top`。
- 断言输入卡片 offsetHeight 与 mock 未包含 plan 的 baseline 一致（差值 ≤ 1px 允许亚像素）。
- 断言 hover Plan 胶囊 500ms 内 Popover 出现，`role="listbox"` 内最多 5 条可视。
- 断言 Popover 顶部与胶囊底部间距等于 4px。

### 真实场景测试

- 更新并执行 `human_tests/chat-plan-density.md` 中 TC-CPD-01 ~ TC-CPD-04。
- 使用当前源码启动本地服务，打开用户给定 history URL，确认完成态 `Done 4/4` 胶囊悬浮在 Context 进度条上方且亮暗主题都可读。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "keeps plan content compact" --reporter=line`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD|keeps plan content compact" --reporter=line`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定生效，不运行 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `AgentChatSection.tsx` 渲染层级、`AgentChatSection.styles.ts` 定位常量与媒体查询 breakpoint。
- 执行 `git status --short`、`git diff`。
- 跑 TypeScript 构建和 focused Playwright 用例；发现布局偏移或断言失败立即修复。

### 第 2 轮

- 基于第 1 轮 diff 再次复核 Plan、Token/Context HUD、输入框高度、Popover 和亮暗主题。
- 复跑 focused Playwright + 真实页面检查。
- 若仍发现胶囊遮挡或占位，追加第 3 轮。

## 风险与决策

- 极窄窗口（≤ 360px）下 Plan 胶囊 + Token/Context HUD + 消息底部可能争抢垂直空间；本方案通过窄屏 Playwright 断言与真实页面检查覆盖，实在冲突时降级为把胶囊缩放至更小 padding。
- Popover 在极长 step 列表下滚动可能被移动端 IME 干扰；第一版仅在桌面端 hover 场景验证，移动端保留后续迭代。
- 亮暗主题 contrast 依赖现有 theme token，不新增分支；如后续 UI 库升级导致 token 变化，需要同步更新 planCapsule 颜色。
- 胶囊长文案国际化时若超过 `max-width: 320px`，通过 `text-overflow: ellipsis` 兜底，不做多行 wrap，避免再次撑高。
