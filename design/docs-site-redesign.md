# Docs Site Redesign

## 功能模块

文档站拆成两个明确表面：

1. 首页：`/` 由 `site/home/index.html`、`site/home/styles.css` 和 `site/home/home.js` 手写维护。构建后由 `site/scripts/build-home.mjs` 覆盖到 `site/dist/index.html`，首页不经过 VitePress 页面生成，也不加载 Vue/VitePress runtime。
2. 文档区：`/docs/`、`/getting-started/`、`/reference/`、`/en/...` 使用 VitePress。文档内容每次构建都从仓库根目录 `docs/` 与 `docs-en/` 同步到 `site/src/content/docs/`。

当前决策是把文档区技术栈切到 VitePress，和 Vite / Vite 中文文档站的底层文档技术方案保持一致；首页仍保持单独设计的纯 HTML 方案，以便首屏性能和内容表达完全可控。

## 用户目标验证清单

### 必须实现

- 文档区技术栈迁移到 VitePress。
- 首页不使用框架生成页面，不使用生成式模板方案，采用单独设计的纯 HTML / CSS / 原生 JS。
- 首页强调“AI 时代的一站式代理解决方案”，并突出与 Coding Agent / skill 工作流的配合。
- 首页右侧预览 Tab 使用 `CLI`、`With AI`、`Rules`，`With AI` 展示 skill 安装、抓取真实流量和让 Agent 实现技能的 case。
- 部署构建每次自动从 `docs/` 与 `docs-en/` 生成文档区内容，保持始终同步。
- 文档区布局参考 Vite 文档的三栏阅读结构，但主题色使用 Bifrost 绿色系。
- 首页首次体验指引使用 `bifrost start -d`，不默认推荐 `--no-system-proxy`。

### 必须不破坏

- 不改变 `docs/`、`docs-en/` 作为源文档的维护方式。
- 不破坏现有 `/docs/`、`/getting-started/`、`/reference/`、`/en/...` 深链。
- 不让首页引入 VitePress/Vue runtime、hydration 或客户端路由。
- 不让测试/CI 场景使用的 `--no-system-proxy` 成为普通用户手册或首页默认建议。
- 不降低文档区搜索、侧边栏、代码块、i18n、404、站内链接验证能力。

### 必须真实验证

- 执行 `pnpm --dir site run test` 验证 docs sync、站点链接和首页静态脚本单元测试。
- 执行 `pnpm --dir site run build` 验证部署构建链路：sync docs -> verify docs -> VitePress build -> legacy redirects -> static home overlay -> home verify -> site links verify。
- 执行 `bash e2e-tests/tests/test_site_docs_sync.sh` 验证新增中文和英文源文档会自动同步、构建和进入链接校验。
- 使用本地 `vitepress preview` 打开 `/bifrost/`、中文文档、英文文档、暗色模式和移动端并截图观察 UI。

### 必须交付

- 更新技术方案文档。
- 实现 VitePress 配置、主题、文档同步脚本、重定向脚本和首页 AI 定位。
- 更新站点 E2E、human_tests 和索引。
- 完成两轮 Review/Fix/Test。
- 提交、推送、更新 PR 并看护远端 CI。

## 任务启动与隔离证据

- 主工作区存在并行开发风险，本任务使用独立 worktree：`../bifrost-docs-site-plan`。
- 方案与实现分支：`codex/docs-site-redesign-plan`。
- 任务启动检查命令：`git status --short --branch`。

## 当前实现

### VitePress 文档区

- `site/.vitepress/config.mjs` 是 VitePress 入口配置。
- `srcDir: "src/content/docs"`，`outDir: "dist"`，`cleanUrls: true`，`base` 由 `BASE_PATH` 或 `SITE_URL` 推导，默认适配 GitHub Pages `/bifrost/`。
- `.vitepress/config.mjs` 直接复用 `site/scripts/docs-sync-lib.mjs` 的 `buildPagesSync()`，从同步页列表生成中文和英文 sidebar。
- `themeConfig.search.provider = "local"` 使用 VitePress 本地搜索。
- `themeConfig.i18nRouting = false`，避免 VitePress 对当前路径做简单同构语言映射；本项目中英文源文档有显式映射，语言入口使用固定可用路径。
- `site/.vitepress/theme/style.css` 覆盖 VitePress 主题变量和布局细节，使用 Bifrost 绿色主题。
- 静态资源放在 `site/src/content/docs/public/`，符合 VitePress 在 `srcDir/public` 复制静态资源的规则。

### 首页

- `site/home/index.html` 是唯一首页源文件。
- 首页定位：`AI-era proxy / Coding Agent ready` 与中文 `AI 时代代理方案 / 适配 Coding Agent`。
- 右侧预览 Tab：
  - `CLI`：展示 `bifrost start -d` 和 traffic search。
  - `With AI`：展示 `bifrost install-skill`、`bifrost traffic search --include headers,body ...` 和“帮我抓取登录接口，然后实现一个可复用 Coding Agent 会话刷新技能”的 case。
  - `Rules`：展示规则片段。
- `site/home/home.js` 只负责原生 Tab 切换、键盘可访问性和中英文轻量切换。
- `site/scripts/home-static-lib.mjs` 为首页生成带 hash 的 CSS/JS，并校验 base path、ARIA、中英文、图片尺寸、gzip 预算和 forbidden runtime marker。

### 自动同步

- `site/scripts/sync-docs.mjs` 每次构建都执行。
- `site/scripts/verify-docs-sync.mjs` 在 VitePress build 前执行，源文档和生成文档不一致时提前失败。
- `site/scripts/docs-sync-lib.mjs` 负责发现 `docs/` 与 `docs-en/` 下所有 Markdown，并生成 VitePress `.md` 内容。
- README 入口和已知页面映射到稳定路由，例如 `docs/overview.md -> getting-started/overview.md`、`docs-en/README.md -> en/reference/index.md`。
- Markdown 内部链接按 VitePress clean URL 重写，避免 build 阶段 dead link。

### 重定向与 404

- `site/scripts/write-redirects.mjs` 为旧路径写入静态 redirect HTML，例如 `reference/getting-started/cli-quick-start/index.html -> /bifrost/getting-started/cli-quick-start`。
- VitePress 生成 `404.html`，根站 `https://bifrost-proxy.github.io/` 仍可通过 GitHub Pages 仓库配置指向 `/bifrost/` 或依靠站点首页入口。

## 外部调研结论

### VitePress

官方 Getting Started 说明 VitePress 通过 `.vitepress/config.*` 配置，源文件是 Markdown，支持 `vitepress dev/build/preview`。官方 Site Config 说明 `base` 需要用于 GitHub Pages 这类子路径部署，`srcDir` 和 `outDir` 可配置。官方 i18n 文档说明 VitePress 内置 `locales` 配置。

结论：文档区采用 VitePress。它与 Vite 文档站的底层方案一致，适合当前“文档区参考 Vite 布局，首页独立纯 HTML”的目标。

### Vite / Vite 中文文档

Vite 仓库的 `docs/` 使用 `.vitepress` 配置。线上 `vitejs.cn/guide/` 和 `cn.vite.dev/guide/` 的页面资源包含 VitePress runtime、默认主题组件、local search 与 VitePress 样式类，说明其文档区是 VitePress 方案。

结论：文档区布局参考 Vite 文档的阅读体验：顶部导航、左侧分组 sidebar、中间正文、右侧目录、搜索和明暗模式；Bifrost 主题改为绿色，不照搬 Vite 首页。

### BigModel GLM Coding 首页

2026-07-03 通过真实浏览器打开 `https://bigmodel.cn/glm-coding` 观察，提炼以下可借鉴模式：

- 首屏高度克制：顶部导航只保留关键入口。
- 主体用超大产品名、短副标题和单主 CTA 建立定位。
- 首屏右侧放可交互演示面板，让用户无需滚动就理解产品工作流。
- 轻量 Tab 交互不跳页，降低理解成本。
- 白底、大字号、少色彩，关键视觉集中在演示面板。

Bifrost 首页吸收这些节奏，但表达为 AI 时代代理工具：左侧强调一站式代理和 Coding Agent，右侧 `With AI` 展示 skill 工作流。

## 构建流程

```bash
rm -rf dist
node scripts/sync-docs.mjs
node scripts/verify-docs-sync.mjs
vitepress build .
node scripts/write-redirects.mjs
node scripts/build-home.mjs
node scripts/verify-home.mjs
node scripts/verify-site-links.mjs
```

关键约束：

- `build-home.mjs` 必须在 `vitepress build` 后执行，最终 `dist/index.html` 才是手写首页。
- `verify-home.mjs` 必须在 `build-home.mjs` 后执行，防止首页重新引入 framework marker、缺失 base path 或超出预算。
- `verify-site-links.mjs` 必须最后执行，覆盖首页、VitePress 文档区和 redirect HTML 的最终链接状态。

## 测试方案

### 单元测试

- `pnpm --dir site run test`
- 覆盖 docs source discovery、Markdown link rewrite、stale generated docs cleanup、VitePress clean URL link verification、home static build 和 forbidden marker 校验。

### E2E

- `bash e2e-tests/tests/test_site_docs_sync.sh`
- 临时新增中文和英文 docs probe，验证无需改站点配置即可被同步、构建、进入 `dist`，并验证首页 AI 文案、`With AI`、`bifrost install-skill`、`bifrost start -d` 和禁止默认推荐 `--no-system-proxy`。

### Human Tests

- `human_tests/docs-site-redesign.md`
- 覆盖首页纯 HTML、VitePress build、文档区 Vite-like 布局、本地 preview、中文/英文、明暗模式、移动端和截图观察。

## 已执行验证

- `pnpm --dir site run test`：通过，8 个测试通过。
- `pnpm --dir site run build`：通过，VitePress build、redirect、home verify、site links verify 全部通过。
- `bash e2e-tests/tests/test_site_docs_sync.sh`：通过，临时新增中文和英文文档均进入同步和构建链路。
- 本地服务：
  - `pnpm --dir site exec vitepress preview . --host 127.0.0.1 --port 4177`
  - 首页：`http://127.0.0.1:4177/bifrost/`
  - 中文文档：`http://127.0.0.1:4177/bifrost/getting-started/overview`
  - 英文文档：`http://127.0.0.1:4177/bifrost/en/getting-started/overview`
- 截图证据：
  - `/tmp/bifrost-home-vitepress-overlay-desktop.png`
  - `/tmp/bifrost-home-vitepress-overlay.png`
  - `/tmp/bifrost-docs-vitepress-desktop.png`
  - `/tmp/bifrost-docs-vitepress-dark.png`
  - `/tmp/bifrost-docs-vitepress-english.png`
  - `/tmp/bifrost-docs-vitepress-mobile.png`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：VitePress 迁移、首页 AI 定位、With AI tab、docs sync、`bifrost start -d`。
- Review 范围：`site/.vitepress/`、`site/home/`、`site/scripts/`、E2E、design、human_tests。
- 运行：`pnpm --dir site run test`、`pnpm --dir site run build`、`bash e2e-tests/tests/test_site_docs_sync.sh`。
- 已发现并修复：
  - Markdown 相对链接按旧页面路径计算，VitePress dead link 检查失败。
  - VitePress clean URL 与首页链接尾斜杠不一致。
  - VitePress `srcDir` 下 public 资源位置不对，首页 favicon 校验失败。
  - VitePress 默认 i18n 对应页推断生成错误语言链接。

### 第 2 轮

- 复查最新 diff、浏览器 UI 和截图。
- 运行：`pnpm --dir site run build`，本地 `vitepress preview` 浏览器验证。
- 已发现并修复：
  - With AI 面板在真实桌面截图中最后一条结果卡被固定高度裁切。
  - Python 静态服务不支持 VitePress clean URL，改用 `vitepress preview` 验证真实文档路由。

## 残余风险

- VitePress `cleanUrls` 在不同静态服务上表现依赖服务端 fallback。GitHub Pages 和 VitePress preview 支持目标路由；本地手写 Python server 仅适合验证 `/bifrost/` 首页，不适合作为 clean URL 文档页验证服务。
- 本次未改变源文档内容，只同步生成站点内容；源文档若新增大量特殊 Markdown/Vue 语法，仍需要 VitePress build 作为最终门禁。

## 参考资料

- VitePress Getting Started: https://vitepress.dev/guide/getting-started
- VitePress Site Config: https://vitepress.dev/reference/site-config
- VitePress i18n: https://vitepress.dev/guide/i18n
- Vite docs source: https://github.com/vitejs/vite/tree/main/docs
- VitePress config in Vite repo: https://github.com/vitejs/vite/blob/main/docs/.vitepress/config.ts
- Vite 中文文档: https://vitejs.cn/guide/
- BigModel GLM Coding: https://bigmodel.cn/glm-coding
