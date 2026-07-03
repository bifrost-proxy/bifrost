# Docs Site Redesign

## 功能模块

文档站重构分为两个独立表面：

1. 首页：`/` 使用手写静态 HTML、CSS 和极少量原生 JavaScript，不经过 Astro/Starlight/React/Vue 运行时生成。目标是把首屏性能、可读性和转化路径做到极致。
2. 文档区：`/docs/`、`/getting-started/`、`/reference/`、`/en/...` 继续使用现有 Astro + Starlight + Pagefind 体系，保留 Markdown 同步、双语路由、侧边栏、搜索、代码高亮和内链校验。

本方案只设计重构方向，不在本次修改里落地运行时代码。

## 用户目标验证清单

### 必须实现

- 调研当前更好的文档站方案，明确是否替换现有 Starlight。
- 首页不使用框架生成页面，不使用生成式模板方案，采用单独设计的纯 HTML 交付。
- 首页性能优先，首屏不依赖框架运行时、hydration、客户端路由或大型 JS 包。
- 首页体验参考 `https://bigmodel.cn/glm-coding` 的首屏交互、布局节奏和视觉表达，但不能照搬素材、文案和品牌。
- 文档内容区继续保持可维护、可搜索、双语和可自动校验。

### 必须不破坏

- 不改变 `docs/`、`docs-en/` 作为源文档的维护方式。
- 不破坏现有 `/docs/`、`/getting-started/`、`/reference/`、`/en/...` 深链。
- 不降低文档区搜索、侧边栏、代码块、i18n、404 和站内链接验证能力。
- 不让首页视觉资产、脚本或动画拖慢文档区构建。

### 必须真实验证

- 真实浏览器打开参考站，记录可借鉴的首屏、滚动段落和交互模式。
- 用文档用例验证本方案是否覆盖首页、文档区、部署、性能预算、迁移、测试和回滚。

### 必须交付

- 新增技术方案文档。
- 新增 `human_tests/` 方案验证用例并更新索引。
- 完成两轮 Review/Fix/Test。

## 任务启动与隔离证据

- 主工作区启动检查：`git status --short --branch` 显示当前位于 `codex/default-global-rule-design`。
- 本任务按用户要求从 `origin/main` 创建独立 worktree：`../bifrost-docs-site-plan`。
- 方案分支：`codex/docs-site-redesign-plan`。
- 本方案文档、human_tests 用例和索引更新只应保留在该独立 worktree，不污染主工作区的并行开发改动。

## 现状分析

当前仓库已经具备文档站基础：

- `site/` 使用 Astro + Starlight。
- `site/src/pages/index.astro` 是站点首页，当前仍由 Astro 构建链路生成。
- `site/src/content/docs/` 由 `site/scripts/sync-docs.mjs` 从 `docs/` 与 `docs-en/` 同步生成。
- `site/package.json` 中 `build` 会执行 docs sync、docs verify、Astro build 和站内链接验证。
- `.github/workflows/site.yml` 使用 GitHub Pages 部署 `site/dist`。
- `design/docs-site-generator.md` 和 `human_tests/docs-site-generator.md` 已覆盖双语文档同步与构建产物校验。

当前体验问题不是“缺一个文档框架”，而是首页和文档区被放在同一个框架审美与构建心智里：首页视觉像普通站点页面，产品第一眼信号不够强，首屏虽然静态生成但仍依赖 Astro 页面源码和框架产物约束，无法把性能预算、资源加载、动效和 HTML 结构控制到最小闭环。

## 外部调研结论

### Astro / Starlight

官方定位适合内容驱动站点，Astro 默认只发送需要的 JavaScript；Starlight 提供文档导航、搜索、i18n、SEO、代码高亮和暗色模式。Starlight 默认集成 Pagefind，适合静态文档搜索。

结论：继续用于文档区。它已经和当前仓库同步脚本匹配，替换成本高，收益主要不在文档区。

### Pagefind

Pagefind 对任意静态 HTML 构建产物生成搜索索引，不需要搜索服务。官方说明强调大站点低带宽搜索，索引会分块加载。

结论：继续作为文档区搜索方案。首页如需要搜索入口，应跳转到文档区搜索或轻量 command palette，不应在首屏加载 Pagefind。

### Docusaurus

Docusaurus 是 React 静态站点生成器，会生成可见 HTML，但也构建 SPA 和客户端路由体验。

结论：不适合本次目标。它能改善文档生态，但首页“纯 HTML、非框架生成、极致性能”目标会被 React/SPA 运行时拉偏。

### VitePress

VitePress 是 Vite + Vue 文档静态站点生成器，Markdown 和 Vue 扩展体验好。

结论：不建议迁移。它仍是框架生成文档站，当前 Starlight 已满足内容区能力，迁移只会引入 Vue 主题重写成本。

### Cloudflare Pages / GitHub Pages

Cloudflare Pages 支持任意静态 HTML 部署，并可通过 `_headers` 控制响应头；GitHub Pages 当前已接入仓库 workflow，部署简单但响应头控制弱。

结论：短期继续 GitHub Pages，方案中预留 Cloudflare Pages 作为性能增强部署目标。若需要更强缓存、压缩、安全头和边缘回滚，再迁移或双发布。

## BigModel GLM Coding 首页体验参考

2026-07-03 通过真实浏览器打开 `https://bigmodel.cn/glm-coding` 观察，提炼以下可借鉴模式：

1. 首屏高度克制：顶部导航只保留 Logo、文档、控制台和登录；主体用超大产品名、短副标题、单主 CTA 直接建立定位。
2. 产品演示前置：首屏下半部露出一个大尺寸终端/IDE 演示面板，让用户无需滚动就理解产品与工作流。
3. 轻量 Tab 交互：演示面板提供“终端 / IDE”切换，切换不跳页，降低理解成本。
4. 视觉语言集中：白底、大字号、少色彩，关键视觉只用蓝紫渐变面板、真实工具 Logo、价格卡高亮和少量浮动服务入口。
5. 滚动叙事顺序清晰：首屏定位 -> 工具 Logo 信任带 -> 套餐/行动卡 -> 模型能力证据 -> IDE 推荐 -> 如何开始四步。
6. 底部行动路径具体：用编号卡片把开始流程拆成四步，每张卡都有一个明确动作。

Bifrost 首页不能复刻定价/模型营销结构，但可以吸收这种节奏：

- 首屏标题聚焦产品名和类别，不做宽泛口号。
- 首屏直接展示 Bifrost 真实代理工作台或终端命令演示。
- 用 2 到 3 个原生 Tab 切换“抓包 / 改写 / 回放”或“CLI / Desktop / Web UI”。
- 第二屏承接真实协议能力和典型场景，而不是堆满装饰卡。
- “如何开始”保持 3 到 4 步，面向安装、启动、信任证书、打开 Web UI。

## 推荐技术方案

### 总体架构

```
site/
  home/
    index.html
    styles.css
    home.js
    assets/
      bifrost-ui.avif
      terminal-demo.svg
      desktop-preview.avif
  public/
    _headers
    images/
  src/
    pages/
      404.astro
      docs/index.astro
    content/docs/
  scripts/
    build-home.mjs
    verify-home.mjs
    sync-docs.mjs
    verify-docs-sync.mjs
    verify-site-links.mjs
```

- `site/home/index.html` 是首页源文件，直接写完整 HTML，不用 Astro frontmatter、组件、layout 或 MDX。
- `site/home/styles.css` 是首页唯一 CSS，构建时内联 critical CSS 或复制为 `/home.<hash>.css`。
- `site/home/home.js` 只负责可选交互：Tab 切换、复制安装命令、减少动效偏好、简单可访问状态同步。目标 gzip 后小于 4 KiB。
- `site/scripts/build-home.mjs` 把首页源文件复制到 `site/dist/index.html`，在 Astro build 之后执行，覆盖框架生成首页。
- 文档区继续由 Astro/Starlight 输出到 `site/dist/docs/`、`site/dist/getting-started/`、`site/dist/reference/`、`site/dist/en/`。
- `site/scripts/verify-home.mjs` 做静态验收：禁止首页引入框架 runtime，检查资源大小、首屏图片尺寸、ARIA、链接、base path 和 no-JS fallback。

### 构建顺序

`site/package.json` 的未来构建应调整为：

```bash
rm -rf dist
node scripts/sync-docs.mjs
node scripts/verify-docs-sync.mjs
astro build
node scripts/build-home.mjs
node scripts/verify-home.mjs
node scripts/verify-site-links.mjs
```

关键点：

- 首页构建必须在 `astro build` 之后执行，确保最终 `dist/index.html` 是手写 HTML。
- `verify-site-links` 必须在首页覆盖后执行，确保首页链接也被扫描。
- `build-home` 必须支持 GitHub Pages 的 `BASE_PATH`，例如 `/bifrost/`。
- `404.astro` 和 docs redirect 可以继续由 Astro 管理。

### 首页内容结构

1. 顶部导航
   - 左侧：Bifrost 字标。
   - 右侧：Docs、GitHub、Install、Open Web UI。
   - 移动端：不使用重型菜单库，使用原生 `<details>` 或少量 JS 控制。

2. 首屏
   - H1：`Bifrost`
   - 副标题：`A proxy workbench for traffic capture, rewrite, replay, and debugging.`
   - 主 CTA：Install。
   - 次 CTA：Read docs / GitHub。
   - 首屏下半部露出真实产品演示面板，保证桌面和移动端都能看到下一段内容的线索。

3. 原生演示 Tab
   - `CLI`：展示 `bifrost start --no-system-proxy`、`bifrost status`、`bifrost traffic search`。
   - `Web UI`：展示 Traffic、Rules、Replay 三列或真实截图。
   - `Desktop`：展示托盘/证书/系统代理状态。
   - Tab 使用 `<button aria-selected>`，无 JS 时默认显示 CLI 静态面板。

4. 信任带
   - 不使用假 Logo。使用协议和场景标签：HTTP/2、HTTPS MITM、SOCKS5、WebSocket、SSE、Replay、Scripts。

5. 能力证据
   - 用真实工作流分区：Capture、Rewrite、Replay、Automate。
   - 每个分区配一个短代码片段或产品截图，不堆营销形容词。

6. 如何开始
   - 01 Install
   - 02 Start without touching system proxy by default in docs examples
   - 03 Trust certificate only when HTTPS interception is needed
   - 04 Open Web UI and inspect traffic

### 视觉方向

参考 BigModel 的克制首屏，但调整为 Bifrost 的工程工具气质：

- 背景以白色或极浅灰为主，不做大面积深色或单一蓝紫渐变。
- 首屏标题使用极大字号，但文档和工具内页不使用 hero 字号。
- 视觉主角是产品状态，而不是装饰图形：真实 UI 截图、终端输出、协议流动示意。
- 渐变只用于演示面板边缘或主 CTA，不作为整站底色。
- 卡片半径控制在 8px 以内，避免营销页式圆角堆叠。
- 不使用生成图片作为首页主视觉；需要图片时使用真实截图或手工绘制的轻量 SVG/AVIF。

### 性能预算

首页预算按移动 4G 和冷缓存制定：

| 项目 | 预算 |
| --- | --- |
| HTML | <= 28 KiB gzip |
| CSS | <= 12 KiB gzip |
| JS | <= 4 KiB gzip，允许 0 KiB |
| 首屏图片 | <= 80 KiB AVIF/WebP，必须有宽高 |
| 总首屏传输 | <= 150 KiB gzip/br |
| 第三方请求 | 0 |
| 字体 | 使用 system font，不加载远程字体 |
| LCP | 本地 Lighthouse mobile <= 1.8s 作为目标 |
| CLS | <= 0.02 |

### 搜索策略

- 首页不加载 Pagefind。
- 首页顶部搜索入口可以跳转到 `/docs/` 后由 Starlight/Pagefind 接管。
- 后续如确实需要首页 command palette，必须按需加载 Pagefind，用户点击搜索后才请求搜索 bundle。

### 部署策略

短期：

- 保持 `.github/workflows/site.yml` 部署 GitHub Pages。
- 修复 workflow paths，必须包含 `docs-en/**`，否则英文文档变更不会触发站点 CI。
- 继续上传 `site/dist`。

中期：

- 可增加 Cloudflare Pages 双部署，输出仍为 `site/dist`。
- 在 Cloudflare Pages 使用 `_headers` 设置：
  - HTML: 短缓存或 no-cache。
  - hash 资源: 长缓存 immutable。
  - 安全头: `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`。

### 迁移步骤

1. 新增 `site/home/` 手写首页源文件和静态资产。
2. 新增 `build-home.mjs`，先只复制首页并处理 base path。
3. 新增 `verify-home.mjs`：
   - `dist/index.html` 不包含 Astro island、React、Vue、Starlight runtime 标记。
   - 所有 `href` 和 `src` 在当前 `BASE_PATH` 下可解析。
   - 图片有 width/height 或 CSS aspect-ratio。
   - 关键 CTA 存在且可键盘访问。
   - JS/CSS/图片大小不超过预算。
4. 调整 `pnpm --dir site run build` 顺序。
5. 用 Playwright 增加视觉和交互 smoke：
   - 桌面首屏非空，演示 Tab 可切换。
   - 移动首屏标题、CTA、演示面板不重叠。
   - no-JS 模式仍显示核心内容和安装命令。
6. 保留现有 `site/src/pages/index.astro` 一个版本作为迁移缓冲，确认稳定后删除或改成构建失败提示，避免误以为它仍是首页源。
7. 发布后监控 GitHub Pages 或 Cloudflare Pages 产物，确认 `/`、`/docs/`、`/en/reference/`、历史 redirect 都正常。

### 回滚策略

- 回滚只需恢复 `site/package.json` 构建顺序，停止执行 `build-home.mjs`，让 `site/src/pages/index.astro` 重新生成首页。
- 首页手写资产独立在 `site/home/`，不影响 `docs/` 同步和 Starlight 文档区。
- 若 Cloudflare Pages 双部署失败，GitHub Pages 仍可作为主站或备用站。

## 测试方案

### 单元测试

未来实现时新增：

- `node --test site/scripts/build-home.test.mjs`
  - 验证 base path 替换、hash 资源引用、复制输出和 no-framework marker。
- `node --test site/scripts/verify-home.test.mjs`
  - 验证超预算资源失败、缺 CTA 失败、缺 width/height 失败、框架 runtime 误引入失败。

本次只修改设计文档，不修改脚本或业务代码，单元测试不适用。

### E2E 测试

未来实现时新增：

- `e2e-tests/tests/test_site_home_static.sh`
  - 执行 `pnpm --dir site run build`。
  - 检查 `site/dist/index.html` 为手写首页。
  - 检查 Starlight docs 产物仍存在。
- Playwright UI smoke：
  - 桌面和移动首屏截图。
  - 首页 Tab 切换。
  - no-JS fallback。
  - `/docs/` 搜索入口可用。

本次只修改设计文档，自动化 E2E 不适用。

### 真实场景测试

- `human_tests/docs-site-redesign.md`
- 覆盖方案文档完整性、参考站体验吸收、首页纯 HTML 架构、文档区保留、性能预算、部署与回滚。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：调研、BigModel 参考、首页纯 HTML、独立 worktree、方案交付。
- 执行 `git status --short` 和 `git diff`。
- Review `design/docs-site-redesign.md` 是否覆盖现状、选型、架构、性能预算、测试与回滚。
- 执行 `human_tests/docs-site-redesign.md` 中的所有用例。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 检查 `human_tests/readme.md` 只新增相关索引行，没有全局汇总数字。
- 复核调研来源是否为官方资料或真实页面观察。
- 复跑受影响 human_tests 用例和 Markdown 基础检查。

## 校验要求

本次方案文档变更必须执行：

- `git status --short --branch`
- `git diff`
- `git diff --check`
- `test -f design/docs-site-redesign.md`
- `test -f human_tests/docs-site-redesign.md`
- `rg -n "docs-site-redesign" human_tests/readme.md`
- 按 `human_tests/docs-site-redesign.md` 逐条执行用例

以下项目不适用：

- Rust 单元测试：未修改 Rust 代码。
- Web 单元测试：未修改 Web/站点脚本。
- 自动化 E2E：未修改运行时代码或构建脚本。
- `cargo test --workspace --all-features`：文档-only 方案变更。
- `make coverage`：未修改业务代码，覆盖率门禁不适用。
- `scripts/ci/local-ci.sh`：文档-only 方案变更，运行成本与收益不匹配。

## 文档更新要求

- 新增本文档。
- 新增 `human_tests/docs-site-redesign.md`。
- 更新 `human_tests/readme.md` 索引。
- 未来实现阶段必须同步更新 `design/docs-site-generator.md`、`human_tests/docs-site-generator.md`、站点 README 或部署文档。

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
- Cloudflare Pages headers: https://developers.cloudflare.com/pages/configuration/headers/
- Cloudflare Pages serving pages: https://developers.cloudflare.com/pages/configuration/serving-pages/
- Cloudflare static HTML: https://developers.cloudflare.com/pages/framework-guides/deploy-anything/
- BigModel GLM Coding reference: https://bigmodel.cn/glm-coding
