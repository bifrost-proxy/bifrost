# WebUI AI Skill Assistant

## 功能模块详细描述

在 WebUI 全局布局中增加一个可拖拽的 AI skill 引导入口。入口默认显示在右下角，使用跳动动画吸引注意；鼠标悬浮时展示浮窗，说明如何安装 Bifrost skill 并和 Codex、Claude Code、Trae、Cursor 等 Agent 集成。用户可以一键复制安装命令，也可以点击仓库 `SKILL.md` 链接查看完整能力说明。

交互要求：

- 默认固定在右下角，不遮挡主操作区域和底部状态栏。
- 用户可拖拽入口到任意视口内位置，刷新后保留拖拽位置。
- 鼠标悬浮入口或浮窗时展示说明浮窗，移出后关闭。
- 鼠标从入口移动到浮窗时允许短暂离开 hover 区域，关闭动作延迟约 450ms，避免经过入口与浮窗间隙时来不及操作。
- 点击跳动入口本身隐藏入口，当前页面会话内不再展示，避免影响用户继续操作。
- 浮窗内展示三类经典应用场景：
  - 通过 AI 操作规则增删改查。
  - 流量搜索和问题排查。
  - 多端口独立规则。
- 浮窗内提供可复制命令：`bifrost install-skill -y`。
- 浮窗内提供仓库详情链接：`https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。

## 实现逻辑

- 新增 `web/src/components/AiSkillAssistant/` 组件，职责包含：
  - 维护浮窗 hover 开关。
  - 维护 hover 关闭延迟，离开入口或浮窗后等待短时间再关闭，重新进入时取消关闭计时。
  - 维护当前会话隐藏状态。
  - 维护拖拽坐标，并通过 `localStorage` 持久化。
  - 使用 `copyToClipboard` 复用现有复制能力。
- 在 `web/src/components/Layout/index.tsx` 的全局布局末尾挂载组件，使 Network、Rules、Traffic、Settings 等主页面一致展示。
- 样式使用 CSS Modules，并通过 CSS custom properties 承接 Ant Design token，保证亮色和暗色主题下都有可读的背景、边框、文字和阴影。
- 拖拽坐标通过 `clampSkillAssistantPosition` 限制在视口内，避免入口被拖出屏幕。

## 依赖项

- `web/src/components/Layout/index.tsx`
- `web/src/utils/clipboard.ts`
- `web/src/stores/useThemeStore.ts`
- `@ant-design/icons`
- `antd`

## 测试方案

### 单元测试

- `clampSkillAssistantPosition`：验证负坐标、超出右下边界、正常坐标都会被限制在视口可见范围内。
- `isSkillAssistantDrag`：验证微小点击抖动不被判定为拖拽，真实拖动会被判定为拖拽。

### E2E 测试

- 新增 `web/tests/ui/ai-skill-assistant.spec.ts`：
  - 打开 WebUI 主页面后全局入口可见。
  - 悬浮后浮窗展示安装命令、三类应用场景和仓库 `SKILL.md` 链接。
  - 点击复制按钮后出现复制成功提示。
  - 拖拽入口后坐标改变且入口仍可见。
  - 点击入口后入口消失。
  - 鼠标经过入口与浮窗之间的间隙时，浮窗不会立即消失，用户仍能移动到浮窗并点击复制。

### 真实场景测试

- 新增 `human_tests/webui-ai-skill-assistant.md`：
  - `TC-AISA-01`：入口默认展示与 hover 浮窗。
  - `TC-AISA-02`：复制安装命令。
  - `TC-AISA-03`：仓库 `SKILL.md` 链接跳转。
  - `TC-AISA-04`：拖拽位置与点击隐藏。
  - `TC-AISA-05`：亮色/暗色主题下浮窗可读。
- 同步更新 `human_tests/readme.md` 索引。
- 创建或更新用例文档后必须立即逐条执行并记录实际结果。

## 校验要求

- `pnpm --dir web test:unit -- AiSkillAssistant.test.ts`
- `pnpm --dir web test:ui ai-skill-assistant.spec.ts`
- 真实场景测试逐条执行通过。
- `pnpm --dir web build`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按任务结束要求执行 `rust-project-validate`。

## 文档更新要求

- 新增 `human_tests/webui-ai-skill-assistant.md`。
- 更新 `human_tests/readme.md` Web UI 索引和总数。
