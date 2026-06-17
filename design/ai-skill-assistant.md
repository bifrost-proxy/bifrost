# WebUI AI Skill Assistant

## 功能模块详细描述

在 WebUI 底部状态栏的版本号按钮右侧增加一个 AI skill 引导入口（`Skill` 按钮）。点击入口在其上方弹出 Ant Design Popover 浮窗，说明如何安装 Bifrost skill 并和 Codex、Claude Code、Trae、Cursor 等 Agent 集成。用户可以一键复制安装命令，也可以点击仓库 `SKILL.md` 链接查看完整能力说明。

交互要求：

- 入口固定渲染在 `StatusBar` 内、版本号按钮右侧，与状态栏其他条目同行展示，不再使用右下角悬浮气泡。
- 入口使用点击触发的 Popover，浮窗 `placement` 为 `topRight`，关闭由 Popover 自身的点击/外部点击控制。
- 浮窗内展示三类经典应用场景：
  - 通过 AI 操作规则增删改查。
  - 流量搜索和问题排查。
  - 多端口独立规则。
- 浮窗内提供可复制命令：`bifrost install-skill -y`，复制成功后通过 `antd` `message` 显示 `Skill install command copied`。
- 浮窗内提供仓库详情链接：`https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。

## 实现逻辑

- 新增 `web/src/components/AiSkillAssistant/` 组件（`index.tsx` + `index.module.css`），职责包含：
  - 通过 `useState` 维护 Popover `open` 状态，由 `onOpenChange` 与触发按钮控制开关。
  - 通过 `theme.useToken()` 派生一组 CSS custom properties（`--ai-skill-text`/`--ai-skill-accent`/`--ai-skill-panel-bg` 等），透传到触发按钮与浮窗根节点。
  - 使用 `copyToClipboard` 复用 `web/src/utils/clipboard.ts` 的复制能力，并以 `antd` `message` 反馈成功/失败。
  - 暴露 `ai-skill-assistant-trigger`、`ai-skill-assistant-panel`、`ai-skill-assistant-copy`、`ai-skill-assistant-skill-link` 等 `data-testid`，供 E2E 测试断言。
- 在 `web/src/components/StatusBar/index.tsx` 的版本号按钮之后挂载组件，与状态栏分隔符并排渲染。
- 样式使用 CSS Modules，并通过 CSS custom properties 承接 Ant Design token，保证亮色和暗色主题下都有可读的背景、边框、文字和阴影。

## 依赖项

- `web/src/components/StatusBar/index.tsx`
- `web/src/utils/clipboard.ts`
- `@ant-design/icons`
- `antd`

## 测试方案

### 单元测试

- 暂未拆出独立纯函数（拖拽、夹取坐标等逻辑已在状态栏方案中移除），无单元测试。

### E2E 测试

- `web/tests/ui/ai-skill-assistant.spec.ts`：
  - 打开 WebUI Traffic 页面后状态栏 `Skill` 入口可见，且和版本号按钮在同一行、位于其右侧；旧的 `ai-skill-assistant-launcher` 悬浮入口不再存在。
  - 点击入口后浮窗展示安装命令 `bifrost install-skill -y`、三类应用场景和仓库 `SKILL.md` 链接，浮窗整体位于入口上方。
  - 点击复制按钮后出现 `Skill install command copied` 提示。
  - 再次点击入口后浮窗关闭。
  - 切换暗色主题后再次打开浮窗，标题与 `SKILL.md` 链接仍可读。

### 真实场景测试

- 维护 `human_tests/webui-ai-skill-assistant.md`：
  - `TC-AISA-01`：状态栏入口与点击浮窗。
  - `TC-AISA-02`：复制安装命令并出现 `Skill install command copied` 提示。
  - `TC-AISA-03`：仓库 `SKILL.md` 链接跳转。
  - `TC-AISA-04`：点击关闭浮窗，确认旧右下角 `ai-skill-assistant-launcher` 不再出现。
  - `TC-AISA-05`：亮色/暗色主题下浮窗可读。
  - `TC-AISA-06`：回归——浮窗锚定在状态栏入口正上方。
- 同步更新 `human_tests/readme.md` 索引。
- 创建或更新用例文档后必须立即逐条执行并记录实际结果。

## 校验要求

- `pnpm --dir web test:ui ai-skill-assistant.spec.ts`
- 真实场景测试逐条执行通过。
- `pnpm --dir web build`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按任务结束要求执行 `rust-project-validate`。

## 文档更新要求

- 维护 `human_tests/webui-ai-skill-assistant.md`（含 TC-AISA-01 ~ TC-AISA-06，覆盖状态栏入口、复制、链接、点击关闭、亮/暗色主题以及浮窗锚定回归）。
- 维护 `human_tests/readme.md` Web UI 索引和总数。
