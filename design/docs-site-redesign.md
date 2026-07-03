# Docs Site Redesign

## 功能模块

文档站现在拆成两个明确的交付表面：

1. 首页：`/` 由 `site/home/index.html`、`site/home/styles.css` 和 `site/home/home.js` 直接维护。构建时由 `site/scripts/build-home.mjs` 复制到 `site/dist/index.html`，不经过 Astro 页面、Starlight layout、React/Vue runtime 或 hydration。
2. 文档区：`/docs/`、`/getting-started/`、`/reference/`、`/en/...` 继续使用现有 Astro + Starlight + Pagefind。文档内容仍然从仓库根目录 `docs/` 与 `docs-en/` 自动同步到 `site/src/content/docs/`。

这次实现的核心目标是：让首页保持极轻、极可控，同时让部署每次都重新同步全部源文档，避免文档站与 `docs/`、`docs-en/` 目录漂移。

## 用户目标验证清单

### 必须实现

- 调研更适合文档站的方案，并明确不替换现有文档区框架。
- 首页不使用框架生成页面，不使用生成式模板方案，采用单独设计的纯 HTML 交付。
- 首页性能优先，首屏不依赖框架运行时、hydration、客户端路由或大型 JS 包。
- 首页体验参考 `https://bigmodel.cn/glm-coding` 的首屏交互、布局节奏和视觉表达，但不照搬素材、文案和品牌。
- 部署构建每次都自动从 `docs/` 与 `docs-en/` 生成文档区内容，保持始终同步。
- 英文文档源 `docs-en/**` 变更必须触发站点 CI 与部署。

### 必须不破坏

- 不改变 `docs/`、`docs-en/` 作为源文档的维护方式。
- 不破坏现有 `/docs/`、`/getting-started/`、`/reference/`、`/en/...` 深链。
- 不降低文档区搜索、侧边栏、代码块、i18n、404 和站内链接验证能力。
- 不让首页视觉资产、脚本或动画拖慢文档区构建。

### 必须真实验证

- 真实浏览器打开参考站，记录可借鉴的首屏、滚动段落和交互模式。
- 执行 `pnpm --dir site run test` 验证文档同步与静态首页脚本单元测试。
- 执行 `pnpm --dir site run build` 验证部署构建链路：sync docs -> verify docs -> Astro docs build -> static home overlay -> home verify -> site links verify。
- 执行 `e2e-tests/tests/test_site_docs_sync.sh` 验证新增中文和英文源文档会被自动同步、构建和链接校验覆盖。
- 用浏览器或本地静态服务打开 `site/dist/index.html`，验证首页布局、Tab 交互和静态资源加载。

### 必须交付

- 更新技术方案文档。
- 实现静态首页源文件、构建脚本、校验脚本和单元测试。
- 更新站点构建脚本、部署触发路径和 docs sync E2E。
- 更新 `human_tests/` 真实场景用例并执行。
- 完成两轮 Review/Fix/Test。

## 任务启动与隔离证据

- 主工作区启动检查：`git status --short --branch` 显示当前位于 `codex/default-global-rule-design`。
- 本任务从 `origin/main` 创建独立 worktree：`../bifrost-docs-site-plan`。
- 方案与实现分支：`codex/docs-site-redesign-plan`。
- 变更只保留在该独立 worktree，不污染主工作区的并行开发改动。

## 现状与落地结果

当前仓库已经具备文档站基础：

- `site/` 使用 Astro + Starlight。
- `site/src/content/docs/` 由 `site/scripts/sync-docs.mjs` 从 `docs/` 与 `docs-en/` 同步生成。
- `site/scripts/verify-docs-sync.mjs` 校验生成文档与源文档一致。
- `site/scripts/verify-site-links.mjs` 校验最终 `site/dist` 内部链接。
- `.github/workflows/site.yml` 使用 GitHub Pages 部署 `site/dist`。

本次已落地的变更：

- 删除旧的 `site/src/pages/index.astro`，避免首页继续由 Astro 生成。
- 新增 `site/home/index.html` 作为首页唯一源文件。
- 新增 `site/home/styles.css`，首页样式不依赖 Starlight 或远程字体。
- 新增 `site/home/home.js`，只负责原生 Tab 切换、键盘可访问性和中英文轻量切换。
- 新增 `site/scripts/build-home.mjs`，在 Astro build 后写入 `site/dist/index.html`，并生成带 hash 的 CSS/JS 资源。
- 新增 `site/scripts/verify-home.mjs` 与 `site/scripts/home-static-lib.mjs`，禁止首页引入 Astro runtime marker，校验 base path、ARIA、中英文切换、图片尺寸和 gzip 预算。
- `site/package.json` 的 `build` 保持部署期自动 sync docs，并在文档区构建后覆盖静态首页。
- `.github/workflows/site.yml` 增加 `docs-en/**` path filter，英文文档变更也会触发站点 CI/部署。
- `e2e-tests/tests/test_site_docs_sync.sh` 增加静态首页断言，并继续验证临时新增中文和英文文档会自动出现在构建产物中。
- `site/src/styles/starlight.css` 将文档区调整为 Vite-like 三栏阅读布局：固定顶部导航、左侧分组 sidebar、中间克制正文宽度、右侧本页目录，并使用绿色主题 accent。

## 外部调研结论

### Astro / Starlight

Astro 适合内容驱动站点，Starlight 提供文档导航、搜索、i18n、SEO、代码高亮和暗色模式。Starlight 默认集成 Pagefind，适合静态文档搜索。

结论：继续用于文档区。替换文档区框架成本高，收益主要不在文档阅读体验。

### Pagefind

Pagefind 对静态 HTML 构建产物生成搜索索引，不需要搜索服务，适合静态文档站。

结论：继续作为文档区搜索方案。首页不加载 Pagefind，避免首屏额外资源。

### Docusaurus

Docusaurus 是 React 静态站点生成器，会生成可见 HTML，但也构建 SPA 和客户端路由体验。

结论：不适合本次目标。它能改善文档生态，但首页“纯 HTML、非框架生成、极致性能”目标会被 React/SPA 运行时拉偏。

### VitePress

VitePress 是 Vite + Vue 文档静态站点生成器，Markdown 和 Vue 扩展体验好。

结论：不建议迁移。它仍是框架生成文档站，当前 Starlight 已满足内容区能力，迁移只会引入 Vue 主题重写成本。

### Vite 中文文档站

2026-07-03 参考 `https://vitejs.cn/guide/` 的文档区布局。该站点的技术体验重点不是首页，而是文档阅读界面：

- 顶部固定导航，搜索、主题切换、语言/版本入口都在同一层级，正文不被营销内容打断。
- 桌面端三栏布局清晰：左侧分组 sidebar，中间较窄正文，右侧本页目录。
- 正文区域保持扁平，不把整篇文章包进大卡片；标题、段落、表格和代码块靠留白与细线分层。
- 当前页面、链接、inline code、focus 状态用品牌色提示，避免大面积彩色背景影响阅读。
- 移动端收起 sidebar，保留移动目录与正文优先阅读。

结论：文档区参考 Vite 文档的布局节奏和信息密度，但继续使用 Astro + Starlight + Pagefind。主题色改为 Bifrost 绿色系，首页仍保持独立纯 HTML 方案，不参考 Vite 首页。

### Cloudflare Pages / GitHub Pages

Cloudflare Pages 支持任意静态 HTML 部署，并可通过 `_headers` 控制响应头；GitHub Pages 当前已接入仓库 workflow，部署简单但响应头控制弱。

结论：短期继续 GitHub Pages，方案中保留 Cloudflare Pages 作为后续性能增强部署目标。

## BigModel GLM Coding 首页体验参考

2026-07-03 通过真实浏览器打开 `https://bigmodel.cn/glm-coding` 观察，提炼以下可借鉴模式：

1. 首屏高度克制：顶部导航只保留关键入口；主体用超大产品名、短副标题和单主 CTA 建立定位。
2. 产品演示前置：首屏下半部露出大尺寸终端或 IDE 演示面板，让用户无需滚动就理解产品与工作流。
3. 轻量 Tab 交互：演示面板提供切换，不跳页，降低理解成本。
4. 视觉语言集中：白底、大字号、少色彩，关键视觉集中在演示面板。
5. 滚动叙事顺序清晰：首屏定位 -> 能力信任带 -> 工作流证据 -> 如何开始。
6. 底部行动路径具体：用编号卡片把开始流程拆成明确动作。

Bifrost 首页吸收了这些节奏，但表达为工程工具场景：

- 首屏标题直接使用 `Bifrost`。
- 首屏直接展示 CLI、Web UI、Rules 三个真实工作流 Tab。
- 第二屏用 Capture、Rewrite、Replay、Automate 解释实际能力。
- “Start” 区域保持四步：安装、后台启动服务、添加规则、回放对比。
- 顶部提供 `EN / 中文` 轻量切换，默认英文 no-JS 可读，中文用户通过 JS 切换或浏览器语言自动进入中文文案。

## 实现架构

```
site/
  home/
    index.html
    styles.css
    home.js
  src/
    pages/
      404.astro
    content/docs/
  scripts/
    build-home.mjs
    verify-home.mjs
    home-static-lib.mjs
    home-static.test.mjs
    sync-docs.mjs
    verify-docs-sync.mjs
    verify-site-links.mjs
```

构建顺序固定为：

```bash
rm -rf dist
node scripts/sync-docs.mjs
node scripts/verify-docs-sync.mjs
astro build
node scripts/build-home.mjs
node scripts/verify-home.mjs
node scripts/verify-site-links.mjs
```

关键约束：

- `sync-docs.mjs` 每次部署构建都执行，确保 `docs/` 和 `docs-en/` 是唯一源。
- `verify-docs-sync.mjs` 在 Astro build 前执行，源文档和生成文档不一致时提前失败。
- `build-home.mjs` 必须在 Astro build 后执行，最终 `dist/index.html` 才是手写 HTML。
- `verify-home.mjs` 必须在 `build-home.mjs` 后执行，防止首页重新引入框架 runtime、缺失 base path 或超出预算。
- `verify-site-links.mjs` 必须最后执行，覆盖首页和文档区的最终链接状态。
- `BASE_PATH` 或 `SITE_URL` 控制 GitHub Pages `/bifrost/` 前缀，首页链接和 hash 资源必须一致使用该前缀。

## 首页内容结构

1. 顶部导航
   - 左侧：Bifrost 字标和 favicon。
   - 右侧：Docs、Install、English、GitHub。
   - 语言：`EN / 中文` segmented control，不跳页、不加载框架。

2. 首屏
   - H1：`Bifrost`
   - 副标题：`A proxy workbench for capturing traffic, rewriting requests, replaying failures...`
   - 主 CTA：Install。
   - 次 CTA：Read Docs。
   - 首屏下半部露出工作台预览，保证桌面和移动端都能看到下一段内容的线索。

3. 原生演示 Tab
   - `CLI`：展示 `bifrost start -d` 和 traffic search；首页不向首次体验用户推荐 `--no-system-proxy`，该参数只保留给测试、CI、沙箱或明确诊断场景。
   - `Web UI`：展示 Traffic、Headers、Body、Replay 等 UI 信息层级。
   - `Rules`：展示规则片段。
   - Tab 使用 `<button role="tab" aria-selected>`；无 JS 时默认显示 CLI 静态面板。

4. 信任带
   - 使用能力标签：Capture、Rewrite、Replay、TLS MITM、Scripts、Desktop。

5. 工作流
   - Capture the real request。
   - Rewrite without rebuilding。
   - Replay the failure。
   - Automate the edge cases。

6. 如何开始
   - 01 Install the CLI。
   - 02 Start the daemon。
   - 03 Add a rule for one target。
   - 04 Replay and compare。

## 视觉方向

- 默认浅色以白色和浅灰为主，不做大面积深色或单一蓝紫渐变。
- 暗色模式通过 `prefers-color-scheme: dark` 自动适配，保持终端/工作台的工程工具质感，避免只反转颜色导致的低对比。
- 首屏标题使用大字号，但文档和工具内页不使用 hero 字号。
- 视觉主角是产品状态，不是装饰图形。
- 渐变只用于按钮和演示面板边缘。
- 卡片半径控制在 8px 以内。
- 不使用生成图片作为首页主视觉。
- 不加载远程字体，使用 system font。
- 文档区参考 Vite 文档布局：正文扁平、三栏阅读、窄正文列、右侧目录和左侧分组导航；绿色只作为 active/link/code/focus 状态，不做大面积绿色铺底。

## 性能预算

首页预算按移动 4G 和冷缓存制定，由 `verify-home.mjs` 自动校验主要静态预算：

| 项目 | 预算 |
| --- | --- |
| HTML | <= 28 KiB gzip |
| CSS | <= 12 KiB gzip |
| JS | <= 4 KiB gzip |
| 首屏图片 | 必须有 width/height |
| 总首屏传输 | <= 150 KiB gzip |
| 第三方请求 | 0 |
| 字体 | system font，不加载远程字体 |
| LCP | 本地 Lighthouse mobile <= 1.8s 作为目标 |
| CLS | <= 0.02 |

## 搜索策略

- 首页不加载 Pagefind。
- 首页顶部入口跳转到 `/docs/`，由 Starlight/Pagefind 接管搜索。
- 后续如确实需要首页 command palette，必须按需加载 Pagefind，用户点击搜索后才请求搜索 bundle。

## 部署策略

短期：

- 保持 `.github/workflows/site.yml` 部署 GitHub Pages。
- `site/package.json` 的 `build` 是部署唯一入口，始终先 sync `docs/` 和 `docs-en/`。
- workflow path filter 包含 `site/**`、`docs/**`、`docs-en/**`、`assets/**`、`package.json`、`pnpm-lock.yaml`。

中期：

- 可增加 Cloudflare Pages 双部署，输出仍为 `site/dist`。
- 在 Cloudflare Pages 使用 `_headers` 设置：
  - HTML: 短缓存或 no-cache。
  - hash 资源: 长缓存 immutable。
  - 安全头: `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`。

## 回滚策略

- 恢复 `site/src/pages/index.astro`，并从 `site/package.json` build 中移除 `build-home.mjs` 和 `verify-home.mjs`。
- 首页手写资产独立在 `site/home/`，回滚不会影响 `docs/` 与 `docs-en/` 同步。
- 若 GitHub Pages 或后续 Cloudflare Pages 部署异常，仍可通过前一个成功的 Pages artifact 回滚。

## 测试方案

### 单元测试

- `pnpm --dir site run home:test`
  - 验证 base path 替换、hash 资源引用、复制输出和 no-framework marker。
  - 验证缺失图片尺寸、缺少中英文切换和框架 marker 会失败。
- `pnpm --dir site run docs:test`
  - 验证文档同步脚本行为。
- `pnpm --dir site run test`
  - 同时执行 docs sync 与静态首页脚本测试。

### E2E 测试

- `bash e2e-tests/tests/test_site_docs_sync.sh`
  - 临时新增中文源文档和英文源文档。
  - 执行 docs sync、docs verify 和完整 site build。
  - 验证新增文档进入 `site/dist`。
  - 验证 `site/dist/index.html` 为静态首页，包含 `bifrost start -d`、`/bifrost/docs/`、`/bifrost/en/reference/`、`role="tablist"`、`data-lang="zh"`，且不包含 `/_astro/` 或默认推荐 `bifrost start --no-system-proxy`。
  - 执行站内链接校验。

### 真实场景测试

- `human_tests/docs-site-redesign.md`
- 覆盖首页纯 HTML 源、构建顺序、自动同步、部署触发路径、静态首页产物、文档区 Vite-like 三栏布局、绿色主题、Tab 交互、中英文、明暗模式和清理要求。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：BigModel 参考、首页纯 HTML、部署期自动同步、独立 worktree、实现交付。
- 执行 `git status --short` 和 `git diff`。
- Review `site/home/*`、`site/scripts/home-static-*`、`site/package.json`、`.github/workflows/site.yml` 和 `e2e-tests/tests/test_site_docs_sync.sh`。
- Review `site/src/styles/starlight.css` 是否只影响文档区、不改变首页纯 HTML 交付。
- 执行 `pnpm --dir site run test`、`pnpm --dir site run build` 和 `bash e2e-tests/tests/test_site_docs_sync.sh`。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 检查 `human_tests/readme.md` 只更新相关索引行，没有全局汇总数字。
- 复核 `site/dist/index.html` 是否来自 `site/home/index.html`，且不包含 Astro runtime marker。
- 复核文档页桌面端是否保留左 sidebar / 中正文 / 右目录，移动端是否无横向溢出。
- 复跑受影响测试，并用本地静态服务做浏览器或 HTTP 验证。

## 校验要求

本次实现阶段必须执行：

- `git status --short --branch`
- `git diff`
- `git diff --check`
- `pnpm --dir site run test`
- `pnpm --dir site run build`
- `bash e2e-tests/tests/test_site_docs_sync.sh`
- 按 `human_tests/docs-site-redesign.md` 逐条执行用例

以下项目不适用：

- Rust 单元测试：未修改 Rust 代码。
- `cargo test --workspace --all-features`：站点静态首页、构建脚本和 CI path filter 变更，不触及 Rust workspace 行为。
- `make coverage`：未修改业务代码，覆盖率门禁不适用。
- `scripts/ci/local-ci.sh`：本次由站点单元测试、站点 build、docs sync E2E 和远端 CI 覆盖，完整 local-ci 成本与收益不匹配。

## 文档更新要求

- 更新本文档。
- 更新 `human_tests/docs-site-redesign.md`。
- 更新 `human_tests/readme.md` 索引。
- 通过 `site/scripts/sync-docs.mjs` 让 `docs/` 与 `docs-en/` 持续生成文档站内容。

## 调研来源

- Astro: https://astro.build/
- Astro Why Astro: https://docs.astro.build/en/concepts/why-astro/
- Astro Starlight: https://starlight.astro.build/
- Starlight Search: https://starlight.astro.build/guides/site-search/
- Pagefind: https://pagefind.app/
- Pagefind docs: https://pagefind.app/docs/
- Docusaurus SSG: https://docusaurus.io/docs/advanced/ssg
- Docusaurus introduction: https://docusaurus.io/docs
- VitePress: https://vitepress.dev/
- VitePress routing: https://vitepress.dev/guide/routing
- Vite Chinese docs reference: https://vitejs.cn/guide/
- Cloudflare Pages headers: https://developers.cloudflare.com/pages/configuration/headers/
- Cloudflare Pages serving pages: https://developers.cloudflare.com/pages/configuration/serving-pages/
- Cloudflare static HTML: https://developers.cloudflare.com/pages/framework-guides/deploy-anything/
- BigModel GLM Coding reference: https://bigmodel.cn/glm-coding
