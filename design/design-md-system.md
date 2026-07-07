# DESIGN.md 设计一致性系统

## 背景

`google-labs-code/design.md` 提供了一种面向 coding agent 的设计系统表达方式：根目录 `DESIGN.md` 用 YAML front matter 存放机器可读 token，用 Markdown 正文解释视觉和交互原则，并通过 `@google/design.md` CLI 校验结构、token 引用、对比度和版本差异。

Bifrost 已经有 WebUI、桌面壳、公开首页、文档站和 agent skill 多个交互面。过去这些约束分散在 `AGENTS.md`、站点 CSS、WebUI Ant Design token 和若干方案文档里。新增 `DESIGN.md` 后，后续产品交互迭代有一个可被 Agent 自动读取、可被 CLI lint 的统一设计契约。

## 用户目标验证清单

### 必须实现

- 学习并吸收 `google-labs-code/design.md` 的文件结构、token schema、CLI lint/diff/export 工作流。
- 在仓库根目录新增 `DESIGN.md`，固化 Bifrost 的颜色、字体、圆角、间距、组件和交互原则。
- 新增仓库级 `.agents/skills/design-md/SKILL.md`，让后续 Agent 在修改 UI、站点、文档视觉和交互时主动读取 `DESIGN.md`。
- 新增根项目 `design:*` scripts 与 `@google/design.md` devDependency，提供稳定的本地校验入口。
- 新增 `human_tests/design-md-system.md` 并更新索引，保证该流程可被真实场景测试复核。

### 必须不破坏

- 不修改现有 WebUI、桌面壳、站点或文档实现行为。
- 不替代 Ant Design token；WebUI 继续以 Ant Design token 和主题算法作为实现主线。
- 不绕过站点部署边界；`site/` 仍是公开首页和文档站唯一源码来源。
- 不污染主 worktree 中已有并行改动。

### 必须真实验证

- `pnpm design:lint` 能成功解析并校验根目录 `DESIGN.md`。
- `pnpm design:spec` 能输出当前工具规范；若当前 npm 包的 `spec` 子命令仍缺 bundled `dist/spec.md`，仓库脚本会回退读取同一包内实际发布的 `dist/linter/spec.md`。
- `test -f .agents/skills/design-md/SKILL.md` 和文本检索能证明 skill 已安装到仓库级 `.agents/skills`。
- `human_tests/design-md-system.md` 的用例创建后立即执行并通过。

### 必须交付

- 提交 `DESIGN.md`、`.agents/skills/design-md/SKILL.md`、`design/design-md-system.md`、`human_tests/design-md-system.md`、`human_tests/readme.md`、`package.json` 和 `pnpm-lock.yaml`。
- 完成两轮 Review/Fix/Test。
- 推送分支并创建 PR，按项目规则看护远端 CI。

## 设计来源

### DESIGN.md 规范

- YAML front matter 是机器可读 token，正文 Markdown 是人类可读设计理由。
- token 支持 `colors`、`typography`、`rounded`、`spacing`、`components`。
- token 引用使用 `{path.to.token}`，例如 `{colors.primary}`。
- CLI 支持 `lint`、`diff`、`export` 和 `spec`；本仓库固定使用无点别名 `designmd`，避免 Windows 对 `.md` bin 名称的解析歧义。

### Bifrost 现有风格

- `site/home/styles.css` 已定义公开首页的主视觉：`#f7f8f4` 背景、`#111816` 墨色文字、`#13a58f` teal、`#3578e5` blue、`#d57926` amber、8px 面板圆角和柔和阴影。
- `web/src/App.tsx` 的 Ant Design ConfigProvider 使用 `colorPrimary: "#1677ff"` 和 `borderRadius: 6`，并通过 light/dark 算法维持双主题。
- `AGENTS.md` 已要求 WebUI 开发同时支持亮色和暗色主题，并优先使用 CSS 变量或 Ant Design token。

## 实现逻辑

### 根目录 `DESIGN.md`

`DESIGN.md` 作为长期设计契约，包含：

- Bifrost 品牌和交互性格：精确、克制、可扫描的代理工作台。
- 颜色 token：primary、secondary、tertiary、accent、neutral、surface、text、border、success、warning、danger。
- 字体 token：display、page-title、section-title、body、caption、code。
- 圆角和间距 token：对齐 WebUI 4/6/8/12px 与站点 1180px 最大宽。
- 组件 token：primary/secondary button、tool button、panel、sidebar active item、status badge。
- Do/Don't：明确 WebUI 不是营销页、必须双主题、避免视觉分叉和过度装饰。

### 仓库级 skill

`.agents/skills/design-md/SKILL.md` 采用标准 skill frontmatter，触发场景覆盖 WebUI、桌面壳、站点、文档视觉、交互 copy、布局和组件样式。Agent 使用时必须先读根目录 `DESIGN.md`，再读本方案文档。

### 校验入口

根 `package.json` 新增：

```json
{
  "design:lint": "designmd lint DESIGN.md",
  "design:spec": "node scripts/design-md-spec.mjs",
  "design:export:tailwind": "designmd export --format json-tailwind DESIGN.md",
  "design:export:css": "designmd export --format css-tailwind DESIGN.md"
}
```

`pnpm-lock.yaml` 锁定 `@google/design.md`，保证不同开发者和 CI 环境使用同一 lint 实现。

## 测试方案

### 单元/静态测试

- `pnpm design:lint`：校验 `DESIGN.md` front matter、section order、token 引用、对比度规则。
- `pnpm design:spec`：确认安装的包可输出规范和规则；当前 `@google/design.md@0.3.0` 的 `designmd spec --rules` 子命令存在 bundled path 缺失问题，仓库脚本会优先尝试官方命令，失败后回退到同包内实际存在的 spec 文件。

### E2E 测试

本次不修改 Rust、代理、WebUI 或站点运行行为，不新增自动化 E2E 脚本。端到端验证由 `@google/design.md` CLI 读取真实仓库 `DESIGN.md`，并由 human_tests 用例覆盖 Agent skill 可发现性和 lint 链路。

### 真实场景测试

新增 `human_tests/design-md-system.md`，覆盖：

- `DESIGN.md` lint 链路。
- 仓库级 skill 安装位置与 frontmatter。
- 后续 UI 任务可发现的读取规则。
- package scripts 与依赖一致性。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：是否学习外部规范并安装进仓库。
- 执行 `git status --short`、`git diff`。
- Review `DESIGN.md` token 引用、skill 描述、package scripts、human_tests 索引。
- 复跑 `pnpm design:lint` 和 human_tests 命令。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff。
- 确认没有修改运行时代码，Rust / coverage / local-ci 不适用的理由成立。
- 复跑 `pnpm design:lint`、`pnpm design:spec`、skill 文件检查。

## 校验要求

- 本次为文档/流程/Agent skill 集成，不修改业务代码，`make coverage` 不适用。
- 本次不修改 Rust，`cargo test --workspace --all-features`、`cargo fmt`、`cargo clippy` 和 `rust-project-validate` 标记不适用；若后续 UI 实现任务触发运行时代码改动，仍按 AGENTS.md 完整执行。
- 提交前必须至少通过 `pnpm design:lint` 与 `human_tests/design-md-system.md` 全部用例。

## 文档更新要求

- `DESIGN.md` 作为根目录设计系统源文件。
- `.agents/skills/design-md/SKILL.md` 作为 Agent 可发现的使用入口。
- `human_tests/design-md-system.md` 与 `human_tests/readme.md` 作为验收索引。
