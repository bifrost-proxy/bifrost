# Docs Site Redesign 设计方案

## 背景

Bifrost 文档站承担两类完全不同的表面：**首页**（`/`）需要强定位、可视化产品价值，追求首屏性能与内容表达完全可控；**文档区**（`/docs/`、`/getting-started/`、`/reference/`、`/en/...`）需要三栏阅读结构、搜索、暗色模式、i18n、深链稳定。

旧站将两者混在 Astro/Starlight 一套页面生成体系里，首页也需要引入 Starlight/Vue runtime，首屏性能与设计控制粒度都受限。本次重构决策：

- **文档区**：切到 VitePress，和 Vite / Vite 中文文档站底层保持一致，直接享受成熟的三栏布局、本地搜索、i18n。
- **首页**：不使用任何前端框架生成，采用手写纯 HTML / CSS / 原生 JS；构建后 overlay 覆盖到 `site/dist/index.html`。

主题色使用 Bifrost 绿色系，避免照搬 Vite 首页。首页强调“AI 时代的一站式代理解决方案”，突出与 Coding Agent / skill 工作流的配合。

## 用户目标验证清单

### 必须实现

- 文档区技术栈迁移到 VitePress。
- 首页不使用框架生成页面，不使用生成式模板方案，采用单独设计的纯 HTML / CSS / 原生 JS。
- 首页强调“AI 时代的一站式代理解决方案”，并突出与 Coding Agent / skill 工作流的配合。
- 首页右侧预览 Tab 使用 `CLI`、`With AI`、`Rules`；`With AI` 展示 skill 安装、抓取真实流量、让 Agent 实现技能的 case。
- 部署构建每次自动从 `docs/` 与 `docs-en/` 生成文档区内容，保持始终同步。
- 文档区布局参考 Vite 文档三栏结构，但主题色使用 Bifrost 绿色系。
- 首页首次体验指引使用 `bifrost start -d`，不默认推荐 `--no-system-proxy`。

### 必须不破坏

- 不改变 `docs/`、`docs-en/` 作为源文档的维护方式。
- 不破坏现有 `/docs/`、`/getting-started/`、`/reference/`、`/en/...` 深链。
- 不让首页引入 VitePress / Vue runtime、hydration 或客户端路由。
- 不让 `--no-system-proxy` 出现在普通用户手册或首页默认建议中；它只属于测试/CI 场景。
- 不降低文档区的搜索、侧边栏、代码块、i18n、404、站内链接验证能力。

### 必须真实验证

- `pnpm --dir site run test`：docs sync、站点链接、首页静态脚本单测。
- `pnpm --dir site run build`：sync docs → verify docs → VitePress build → legacy redirects → static home overlay → home verify → site links verify。
- `bash e2e-tests/tests/test_site_docs_sync.sh`：新增中英文源文档自动同步、构建、进入链接校验；覆盖首页 AI 文案与命令建议。
- 本地 `pnpm --dir site exec vitepress preview . --host 127.0.0.1 --port 4177` 打开 `/bifrost/`、中文文档、英文文档、暗色模式、移动端并截图观察 UI。

### 必须交付

- 更新技术方案文档。
- 实现 VitePress 配置、主题、文档同步脚本、重定向脚本和首页 AI 定位。
- 更新站点 E2E、human_tests 与索引。
- 完成两轮 Review/Fix/Test。
- 提交、推送、更新 PR 并看护远端 CI。

## 任务启动与隔离证据

- 主工作区存在并行开发风险，本任务使用独立 worktree：`../bifrost-docs-site-plan`。
- 方案与实现分支：`codex/docs-site-redesign-plan`。
- 任务启动检查命令：`git status --short --branch`。

## 产品语义

### 首页：AI 时代代理工具

首页要在几秒钟内让访问者理解：

- Bifrost 是 AI 时代的一站式代理解决方案。
- 面向 Coding Agent / skill 工作流深度适配，能被 Agent 直接调用抓包、观察、复现。
- 提供 CLI、规则、AI 联动三种主要视角，通过右侧可交互 Tab 展示实际用法。

首页参考 GLM Coding 首屏的克制节奏：顶部导航仅保留关键入口，主体使用超大产品名 + 短副标题 + 单主 CTA + 右侧演示面板；轻量 Tab 交互不跳页。Bifrost 用绿色调、绿色主色 CTA 建立与 Vite 差异化视觉。

### 文档区：Vite-like 阅读体验

- 顶部导航保留 Home、Docs、Reference、GitHub、语言切换。
- 左侧分组 sidebar 由 `docs-sync-lib.mjs` 输出驱动。
- 中间正文，Bifrost 绿色主题变量覆盖 VitePress 默认色。
- 右侧目录（TOC）自动生成。
- 顶部搜索使用 VitePress local provider。
- 明暗模式切换。

### i18n 显式映射

VitePress 默认 `i18nRouting` 会按当前路径同构映射语言链接，但 Bifrost 中英文源文档存在显式映射（例如 `docs/rule.md` → `reference/rule-engine`，`docs-en/rule.md` → `en/reference/rule-engine`），需要关闭默认行为，语言入口使用固定可用路径。

## 技术细节

### VitePress 配置

- `site/.vitepress/config.mjs`：
  - `srcDir: "src/content/docs"`。
  - `outDir: "dist"`。
  - `cleanUrls: true`。
  - `base` 由 `BASE_PATH` 或 `SITE_URL` 推导，默认适配 GitHub Pages `/bifrost/`。
  - 直接复用 `site/scripts/docs-sync-lib.mjs` 的 `buildPagesSync()` 输出中/英文 sidebar。
  - `themeConfig.search.provider = "local"`。
  - `themeConfig.i18nRouting = false`。
  - `themeConfig.logoLink` 指向当前 `basePath` 且设置 `target: "_self"`，让文档区左上角品牌 Logo 使用浏览器原生整页导航回到站点首页，避免 VitePress SPA router 把静态首页当作文档页处理并显示 404。
- `site/.vitepress/theme/style.css`：覆盖 VitePress 主题变量与布局细节，使用 Bifrost 绿色主题。
- 静态资源放在 `site/src/content/docs/public/`，符合 VitePress `srcDir/public` 复制规则。

### 首页

- `site/home/index.html` 是唯一首页源文件。
- 定位：`AI-era proxy / Coding Agent ready`；中文 `AI 时代代理方案 / 适配 Coding Agent`。
- 右侧预览 Tab：
  - `CLI`：`bifrost start -d` + `bifrost traffic search`。
  - `With AI`：`bifrost install-skill`、`bifrost traffic search --include headers,body ...`、以及“帮我抓取登录接口，然后实现一个可复用 Coding Agent 会话刷新技能”的 case。
  - `Rules`：展示典型规则片段。
- `site/home/home.js`：原生 Tab 切换、键盘可访问性、中英文轻量切换。
- `site/scripts/home-static-lib.mjs`：为首页生成带 hash 的 CSS/JS，校验 base path、ARIA、中英文、图片尺寸、gzip 预算、forbidden runtime marker（禁止出现任何 `vite-*` / `starlight-*` marker）。

### 自动同步

- `site/scripts/sync-docs.mjs` 每次构建执行。
- `site/scripts/verify-docs-sync.mjs` 在 `vitepress build` 前执行，源文档和生成文档不一致时提前失败。
- `site/scripts/docs-sync-lib.mjs` 负责发现 `docs/` 与 `docs-en/` 下所有 Markdown，并生成 VitePress `.md` 内容；README 与已知页面映射到稳定路由。
- Markdown 内部链接按 VitePress clean URL 重写。

### 重定向与 404

- `site/scripts/write-redirects.mjs` 为旧路径写入静态 redirect HTML，例如 `reference/getting-started/cli-quick-start/index.html -> /bifrost/getting-started/cli-quick-start`。
- 根站部署（`BASE_PATH=/`）时，`site/scripts/write-redirects.mjs` 还会生成 `bifrost/index.html -> /`，主动覆盖历史 GitHub Pages `/bifrost/` 入口，避免旧发布产物继续以另一套首页、canonical、资源路径和文档入口被搜索引擎或用户访问。
- 子路径部署（`BASE_PATH=/bifrost/`）时不能生成上述 redirect，因为 `/bifrost/` 本身就是该部署形态的真实首页。
- VitePress 生成 `404.html`。

### 根站部署边界

公开根站 `https://bifrost-proxy.github.io/` 必须直接使用本仓库 `site/` 构建出的同一套首页与文档产物，禁止独立跳转壳或独立设计。

部署流程：

```bash
SITE_URL=https://bifrost-proxy.github.io/ BASE_PATH=/ pnpm run site:build
rsync -a --delete site/dist/ ../bifrost-proxy.github.io/ \
  --exclude .git --exclude .github --exclude README.md
```

推送 `bifrost-proxy.github.io` 后跟进 `Pages` 与 `pages-build-deployment` runs，线上验证 `https://bifrost-proxy.github.io/`。

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
- 根站构建产物必须包含 `site/dist/bifrost/index.html`，并且该文件只能作为 noindex redirect 指向 `/`；禁止在 `bifrost/` 下保留第二套首页。

## CLI 与 Admin API

无独立 CLI/Admin API。所有能力通过 pnpm scripts + `.github/workflows/site.yml` 暴露：

- `pnpm --dir site run test`
- `pnpm --dir site run build`
- `pnpm run site:build`（根站变体）

`.github/workflows/site.yml` 在 PR 和 main push 上完整构建校验 `site/dist`。非 PR 事件仍会使用 GitHub Pages 权限发布主仓库项目站，但上传的 artifact 只包含 `site/project-pages-redirect-dist/index.html` 这个 noindex redirect，目标是 `https://bifrost-proxy.github.io/`。公开入口统一由 `bifrost-proxy/bifrost-proxy.github.io` 仓库承载，主仓库项目站只作为历史 `/bifrost/` tombstone，避免继续与根站分叉。

## Sync / 导入导出边界

- `docs/`、`docs-en/` 是唯一文档源，生成产物不 commit。
- 主站部署仓库 `bifrost-proxy/bifrost-proxy.github.io` 只承载 GitHub Pages 产物，禁止手写独立首页、独立语言切换、独立导航、独立 SEO 文案或独立文档入口。
- `AGENTS.md` 必须固化上述边界，任何 Agent 触发主站更新时都能直接从开发手册判断先改源、再构建、再同步。

## 外部调研结论

### VitePress

VitePress 官方 Getting Started：通过 `.vitepress/config.*` 配置，源文件 Markdown，支持 `vitepress dev/build/preview`。Site Config：`base` 用于 GitHub Pages 这类子路径部署，`srcDir` 与 `outDir` 可配置。i18n 文档：内置 `locales` 配置。

**结论**：文档区采用 VitePress，与 Vite 文档站底层一致。

### Vite / Vite 中文文档

Vite 仓库 `docs/` 使用 `.vitepress` 配置；线上 `vitejs.cn/guide/` 与 `cn.vite.dev/guide/` 页面包含 VitePress runtime、默认主题组件、local search 与 VitePress 样式类。

**结论**：文档区布局参考 Vite 文档的阅读体验：顶部导航、左侧分组 sidebar、中间正文、右侧目录、搜索和明暗模式；Bifrost 主题改为绿色，不照搬 Vite 首页。

### BigModel GLM Coding 首页

2026-07-03 通过真实浏览器打开 `https://bigmodel.cn/glm-coding` 观察：

- 首屏高度克制：顶部导航仅关键入口。
- 主体用超大产品名、短副标题和单主 CTA 建立定位。
- 首屏右侧放可交互演示面板，让用户无需滚动就理解产品工作流。
- 轻量 Tab 交互不跳页。
- 白底、大字号、少色彩，关键视觉集中在演示面板。

**结论**：Bifrost 首页吸收节奏，但表达为 AI 时代代理工具；左侧强调一站式代理与 Coding Agent，右侧 `With AI` 展示 skill 工作流。

## 实现切分

### Phase 1：VitePress 迁移基础

- 引入 `site/.vitepress/config.mjs` 与主题目录。
- `docs-sync-lib.mjs` 输出改造为 VitePress 兼容 sidebar / clean URL。
- 首页 overlay 脚本 `build-home.mjs`、`verify-home.mjs`、`home-static-lib.mjs`。
- 关闭 `i18nRouting` 走显式映射。

### Phase 2：首页重设计

- 手写 `site/home/index.html`、`styles.css`、`home.js`。
- 三个预览 Tab（CLI / With AI / Rules）。
- Forbidden runtime marker 校验：禁止 `vite-*`、`starlight-*`、`data-hydrate*`。

### Phase 3：Legacy 深链

- `write-redirects.mjs` 覆盖历史深链。
- `verify-site-links.mjs` 校验 redirect 目标存在。

### Phase 4：真实回归

- E2E `test_site_docs_sync.sh` 增加首页文案与命令建议断言。
- Human tests 覆盖桌面、暗色、移动端、中英文与截图观察。

## 测试方案

### 单元测试

- `pnpm --dir site run test`
- 覆盖 docs source discovery、Markdown link rewrite、stale generated docs cleanup、VitePress clean URL link verification、home static build 与 forbidden marker 校验。

### E2E

- `bash e2e-tests/tests/test_site_docs_sync.sh`
- 临时新增中/英文 probe，验证无需改站点配置即可被同步、构建、进入 `dist`。
- 覆盖首页 AI 文案、`With AI`、`bifrost install-skill`、`bifrost start -d`、禁止默认推荐 `--no-system-proxy`。

### Human Tests

- `human_tests/docs-site-redesign.md`
- 覆盖：TC-DSR-00 主站部署边界、首页纯 HTML、VitePress build、文档区 Vite-like 布局、本地 preview、中/英文、明暗模式、移动端、截图观察。

## 已执行验证

- `pnpm --dir site run test`：通过，8 个测试通过。
- `pnpm --dir site run build`：通过，VitePress build、redirect、home verify、site links verify 全部通过。
- `bash e2e-tests/tests/test_site_docs_sync.sh`：通过，临时新增中英文文档均进入同步与构建。
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

- 复查最新 diff、浏览器 UI 与截图。
- 运行：`pnpm --dir site run build`，本地 `vitepress preview` 浏览器验证。
- 已发现并修复：
  - With AI 面板在真实桌面截图中最后一条结果卡被固定高度裁切。
  - Python 静态服务不支持 VitePress clean URL，改用 `vitepress preview` 验证真实文档路由。

## 残余风险

- VitePress `cleanUrls` 在不同静态服务上表现依赖服务端 fallback。GitHub Pages 与 VitePress preview 支持目标路由；本地手写 Python server 仅适合验证 `/bifrost/` 首页，不适合作为 clean URL 文档页验证服务。
- 本次未改变源文档内容，只同步生成站点内容；源文档若新增大量特殊 Markdown/Vue 语法，仍需要 VitePress build 作为最终门禁。
- 首页手写 CSS/JS 长期维护成本高：任何 UI 改动都要重新跑 `verify-home.mjs`，覆盖 base path、ARIA、gzip 预算、forbidden marker。
- 根站部署仓库若有人误改内容会导致线上偏离源；`AGENTS.md` 与 human_tests 都必须固化“不能在部署仓库维护第二套页面”这一约束。

## 参考资料

- VitePress Getting Started: https://vitepress.dev/guide/getting-started
- VitePress Site Config: https://vitepress.dev/reference/site-config
- VitePress i18n: https://vitepress.dev/guide/i18n
- Vite docs source: https://github.com/vitejs/vite/tree/main/docs
- VitePress config in Vite repo: https://github.com/vitejs/vite/blob/main/docs/.vitepress/config.ts
- Vite 中文文档: https://vitejs.cn/guide/
- BigModel GLM Coding: https://bigmodel.cn/glm-coding
