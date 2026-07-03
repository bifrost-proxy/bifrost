# WebUI AI Skill Assistant 设计方案

## 背景

Bifrost 的核心能力（规则增删改查、流量搜索、多端口独立规则、远程调用等）已经通过 SKILL.md 暴露给 Codex、Claude Code、Trae、Cursor 等外部 Agent，但用户在 WebUI 中并没有明显入口感知“Bifrost 是可以被 AI 直接操作的”。旧版本尝试用右下角悬浮气泡（`ai-skill-assistant-launcher`）承载引导，气泡在流量表格、Traffic Detail 弹层、Agent Runs 页面上会遮挡内容，并且拖拽逻辑与主界面点击存在冲突。

第二版设计把 Skill 引导入口收敛到 WebUI 底部状态栏的固定位置：紧贴版本号按钮右侧展示一个 `Skill` 按钮，点击后在按钮上方展开 Ant Design Popover，介绍安装命令、经典应用场景、以及仓库 SKILL.md 链接。这样既保留了引导入口的可发现性，又不会遮挡任何主内容，也不再需要拖拽夹取坐标逻辑。

## 用户目标验证清单

### 必须实现

- WebUI 底部状态栏在版本号按钮之后展示 `Skill` 入口，与状态栏其他条目同行渲染。
- 点击 `Skill` 入口在其上方弹出 Popover，`placement=topRight`，`arrow=false`。
- 浮窗展示：安装命令 `bifrost install-skill -y`、Copy 按钮、三类经典应用场景、仓库 SKILL.md 链接。
- Copy 按钮通过 `copyToClipboard` 复制安装命令，成功后展示 `Skill install command copied` 提示。
- 仓库详情链接指向 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`，在新页面打开。
- 亮色和暗色主题下浮窗内容全部可读，`AI Skill 加速 Bifrost 操作`、`SKILL.md` 链接、经典应用场景在两种主题下都清晰。
- 组件通过 `theme.useToken()` 派生 CSS 变量，透传到触发按钮和 Popover 根节点，随主题切换自动更新。

### 必须不破坏

- 状态栏其他条目（版本号按钮、Traffic/Agent 状态徽标、主题切换）位置和交互不变。
- Traffic、Rules、Agent 等主内容不被浮窗遮挡；旧右下角 `ai-skill-assistant-launcher` 完全移除，不再渲染。
- 页面其他 antd Popover / Modal 的层级和事件行为不受影响，Popover 外部点击关闭仍然可用。
- WebUI 构建（`pnpm --dir web build`）与 Playwright global setup 启动流程不变。

### 必须真实验证

- 真实浏览器打开 `http://127.0.0.1:<web_port>/_bifrost/traffic`，Skill 入口位于版本号按钮右侧同一行，点击后浮窗出现在入口正上方。
- 真实执行 Copy 按钮，剪贴板内容为 `bifrost install-skill -y`，页面出现成功提示。
- 真实点击 SKILL.md 链接，新页面打开仓库文档。
- 真实切换暗色主题后再次打开浮窗，标题与链接可读。
- 真实执行 Playwright `ai-skill-assistant.spec.ts`，包括“状态栏 topRight 锚定回归”和暗色主题回归。

## 产品语义

### Skill 入口是 WebUI 常驻状态栏一等入口，不是可拖拽悬浮气泡

Skill 入口固定挂在 `StatusBar` 版本号按钮之后，与状态栏分隔符并排渲染，位置在浏览器右下角状态栏内部（不是页面右下角悬浮层）。这样有三点收益：

1. 不遮挡 Traffic / Rules / Agent 等主内容，不与 Traffic Detail 抽屉、Agent Runs 页面冲突。
2. 视觉锚点固定，用户学习一次即知“Skill 在版本号旁边”。
3. 不需要拖拽夹取坐标逻辑，也不需要持久化位置状态。

Popover 使用点击触发（`trigger="click"`），关闭由 antd Popover 的 `onOpenChange` 与外部点击控制。`arrow=false` 让浮窗底边贴近入口顶端，`overlayInnerStyle={{ padding: 0, borderRadius: 8 }}` 让内部自行控制圆角与内边距。

### 浮窗只承担“告知 + 一键复制 + 深度文档跳转”

浮窗内不承载安装进度、状态检测、Skill 版本号等运行时状态：Bifrost skill 的安装、更新、卸载走 CLI `bifrost install-skill` 命令，WebUI 只是引导入口。浮窗只包含以下三块：

- Header：机器人图标 + `AI Skill 加速 Bifrost 操作` 标题 + 副标题“安装后让强大的 Agent 直接理解代理、规则、流量和远程能力。”
- Command Row：`bifrost install-skill -y` + Copy 按钮。
- Scenario List：三类经典应用场景，每条一行 icon + 文案。
- Footer：一句提示“安装后可在 Agent 中直接调用 Bifrost 能力。” + SKILL.md 链接按钮。

### 主题响应通过 CSS 变量而不是硬编码颜色

组件通过 `theme.useToken()` 读取 `colorText`、`colorTextSecondary`、`colorBorderSecondary`、`colorPrimary`、`colorPrimaryBg`、`colorBgElevated`、`colorFillQuaternary`、`boxShadow` 等 token，并在同一 `useMemo` 里派生 CSS 变量：

```ts
const cssVariables = useMemo(
  () =>
    ({
      "--ai-skill-text": token.colorText,
      "--ai-skill-muted": token.colorTextSecondary,
      "--ai-skill-border": token.colorBorderSecondary,
      "--ai-skill-accent": token.colorPrimary,
      "--ai-skill-accent-bg": token.colorPrimaryBg,
      "--ai-skill-panel-bg": token.colorBgElevated,
      "--ai-skill-command-bg": token.colorFillQuaternary,
      "--ai-skill-panel-shadow": token.boxShadow,
    }) as CSSProperties,
  [token],
);
```

CSS Module (`index.module.css`) 只从这些变量取色，不引入 hardcoded 十六进制。主题切换时 `theme.useToken()` 更新，Popover 内部与状态栏按钮同步换色。

## 技术细节

### 组件结构

新增目录 `web/src/components/AiSkillAssistant/`：

- `index.tsx`：默认导出 `AiSkillAssistant` 组件；内部持有 `open` 状态；渲染触发按钮 + Popover content。
- `index.module.css`：状态栏按钮、Popover 面板、命令行、场景列表、footer 的 CSS Module。

关键常量：

```ts
const INSTALL_COMMAND = "bifrost install-skill -y";
const SKILL_DOC_URL = "https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md";
```

关键 data-testid：

- `ai-skill-assistant-trigger`：状态栏触发按钮，供 Playwright 定位。
- `ai-skill-assistant-panel`：Popover 面板根节点。
- `ai-skill-assistant-copy`：Copy 按钮。
- `ai-skill-assistant-skill-link`：SKILL.md 链接按钮。

已废弃 testid：`ai-skill-assistant-launcher`（旧右下角悬浮气泡入口）。Playwright 断言 `toHaveCount(0)` 保证不再回归。

### 状态栏挂载

在 `web/src/components/StatusBar/index.tsx` 内，在版本号按钮（`statusbar-version-button`）之后挂载 `<AiSkillAssistant />`。挂载点保持与其他状态栏条目相同 flex 布局，使用同一 divider 样式，确保 baseline 对齐。

### 复制流程

复用 `web/src/utils/clipboard.ts` 中的 `copyToClipboard(text: string): Promise<boolean>`，成功时通过 `antd` `message.success` 展示 `Skill install command copied`，失败时 `message.error("Failed to copy skill install command")`。不新增 clipboard polyfill，不改变权限策略。

### Popover 布局

```tsx
<Popover
  content={content}
  trigger="click"
  placement="topRight"
  arrow={false}
  open={open}
  onOpenChange={setOpen}
  overlayInnerStyle={{ padding: 0, borderRadius: 8 }}
>
  <button
    type="button"
    className={styles.statusButton}
    data-testid="ai-skill-assistant-trigger"
    aria-label="Open Bifrost AI skill guide"
    style={cssVariables}
  >
    <RobotOutlined />
    <span>Skill</span>
  </button>
</Popover>
```

`placement=topRight` + `arrow=false` 让浮窗底边贴入口顶部；Playwright 用 `panelBox.y + panelBox.height <= triggerBox.y + 2` 断言这一点。

## CLI 与 Admin API 边界

本方案不改动 CLI 与 Admin API：

- CLI：安装 skill 仍走 `bifrost install-skill -y`；不新增 skill 相关子命令，不改动 `install-skill.rs`。
- Admin API：不新增接口。Skill 的展示完全静态化，Popover 内容硬编码在前端。
- Sync：与规则/Group 同步无关，不涉及 sync 边界。

## Sync / 导入导出 / 分享边界

Skill 引导入口不参与规则 sync、Group sync、rule share URL 或规则导入导出。分享同一台设备上的 Bifrost skill 由 SKILL.md 与 CLI 承担，本组件只是入口引导。

## 实现切分

### Phase 1：组件与状态栏挂载

- 新增 `web/src/components/AiSkillAssistant/` 组件。
- 在 `StatusBar/index.tsx` 中版本号按钮之后挂载组件。
- 移除旧右下角 `ai-skill-assistant-launcher` 悬浮组件与拖拽逻辑。

### Phase 2：主题与交互抛光

- 通过 `theme.useToken()` 派生 CSS 变量，确保亮/暗主题都可读。
- Copy / SKILL.md 链接 / 场景列表交互对齐设计。
- 确认 Popover `topRight` + `arrow=false` 锚定符合期望。

### Phase 3：测试与文档

- 新增 Playwright `web/tests/ui/ai-skill-assistant.spec.ts` 覆盖入口位置、浮窗锚定、Copy、SKILL.md 链接、暗色主题。
- 新增 `human_tests/webui-ai-skill-assistant.md` 覆盖 TC-AISA-01 ~ TC-AISA-06。
- 更新 `human_tests/readme.md` 索引和 Web UI 总数。

### Phase 4：回归与观察

- 逐条执行 human_tests，记录真实执行结果（含真实 dev server 3000/8800 端口的入口位置量测）。
- 复跑 `pnpm --dir web test:ui ai-skill-assistant.spec.ts`。
- 观察 Traffic / Rules / Agent 页面是否有回归。

## 测试方案

### 单元测试

组件内部没有可独立测试的纯函数（拖拽夹取坐标等旧逻辑已经移除），组件行为通过 E2E 覆盖。若后续引入独立 clipboard helper 或 scenario 数据源，再补 `web/src/components/AiSkillAssistant/index.test.tsx`。

### E2E 测试

`web/tests/ui/ai-skill-assistant.spec.ts` 覆盖两条用例：

- `AI skill assistant opens from the status bar next to version`：
  - Traffic 页面下 `ai-skill-assistant-launcher` count 为 0。
  - `ai-skill-assistant-trigger` 与 `statusbar-version-button` 同一 y 轴，且触发按钮 x > 版本号按钮 x。
  - 点击触发按钮后 `ai-skill-assistant-panel` 可见，包含 `bifrost install-skill -y`、三类场景文案、不包含“拖拽气泡”。
  - 面板底部 y + height <= trigger.y + 2（浮窗在入口上方）。
  - `ai-skill-assistant-skill-link` href 精确为 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。
  - 点击 `ai-skill-assistant-copy` 后 toast `Skill install command copied`。
  - 再次点击 trigger 后 panel 隐藏。
- `AI skill assistant status bar popover remains readable in dark theme`：
  - 打开 Rules 页面 → 点击 `theme-toggle` → 点击 trigger。
  - Panel 可见，包含 `AI Skill 加速 Bifrost 操作` 与 `SKILL.md`。

### 真实场景测试 human_tests

`human_tests/webui-ai-skill-assistant.md` 保持 TC-AISA-01 ~ TC-AISA-06 六条用例：

- TC-AISA-01：状态栏入口与点击浮窗，同时验证旧悬浮气泡不再存在。
- TC-AISA-02：复制安装命令并出现 `Skill install command copied` 提示。
- TC-AISA-03：仓库 SKILL.md 链接跳转到 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。
- TC-AISA-04：再次点击 trigger 关闭浮窗，确认旧 `ai-skill-assistant-launcher` 不再出现，不包含“拖拽气泡”文案。
- TC-AISA-05：亮色/暗色主题下浮窗均可读，主题切换后入口仍可点击。
- TC-AISA-06：回归——浮窗底边锚定在状态栏入口上方；Copy 按钮可点击并出现成功提示。

`human_tests/readme.md` 的 Web UI 索引与总数必须同步刷新。所有 UI 类用例都必须真实打开浏览器，禁止只跑 Playwright 就当作 human_tests 已完成；执行时使用临时数据目录、非 9900 端口、`--no-system-proxy` 启动 Bifrost。

### 覆盖率与项目校验

- `pnpm --dir web test:ui ai-skill-assistant.spec.ts`
- `pnpm --dir web build`
- `pnpm --dir web lint`（如 workspace 已启用）
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按任务结束要求执行 `rust-project-validate`。

本机存在 no-local-coverage 约定时不运行 `make coverage`；交付时说明覆盖率本地豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：入口位置、浮窗锚定、Copy、SKILL.md 链接、主题响应、旧悬浮气泡下线。
- 复核 diff：`git status --short` / `git diff` 是否覆盖 `AiSkillAssistant/` 组件、`StatusBar` 挂载点、Playwright 用例、human_tests、readme 索引。
- 重点 review：Popover `placement`/`arrow` 是否正确；`copyToClipboard` 失败分支是否有错误提示；CSS 变量是否只从 `theme.useToken()` 派生。
- 复测：Playwright `ai-skill-assistant.spec.ts`；真实打开 dev server 手动验证入口位置。

### 第 2 轮

- 复核第 1 轮修复：入口 bbox 是否真的在版本号右侧且 y 对齐；panel bbox 是否在 trigger 上方；旧 launcher 是否被彻底移除。
- 再次执行 `git status --short` / `git diff`，检查是否有遗漏的旧组件文件、CSS Module 或 testid。
- 复测：Playwright 用例 + human_tests 逐条执行；至少一次真实亮/暗主题切换验证。

## 风险与决策点

- Popover `arrow=false` 是有意选择，用于让浮窗底边贴入口顶端；如果后续设计要求 arrow，需要重新校准 Playwright 中 `panelBox.y + panelBox.height <= triggerBox.y + 2` 断言的容忍值。
- 复制依赖 `navigator.clipboard`：在非 HTTPS / 非 localhost 场景下失败，`copyToClipboard` 会回退到 fallback（若已实现），仍失败时通过 `message.error` 明确提示。
- 主题变量：如果未来 antd 版本变更 token 名，需要同步更新 `useMemo` 内派生；变量命名统一使用 `--ai-skill-*` 前缀以便定位。
- Skill 安装命令：如果后续 CLI 改动了 `bifrost install-skill` 参数，需要同步刷新 `INSTALL_COMMAND` 常量和 Popover 文案，并同步 human_tests。
- 无障碍：`aria-label="Open Bifrost AI skill guide"` 已挂在按钮上；后续若增加键盘快捷键，需要在 Popover 内提供跳过策略。
