# Docs Site Redesign 真实场景测试

## 功能模块说明

验证文档站重构已经真实落地：首页由手写静态 HTML 交付并强调 AI 时代一站式代理方案，文档区使用 VitePress，构建期间自动同步 `docs/` 与 `docs-en/`，最终部署产物中的首页不包含 VitePress/Vue runtime，并覆盖明暗模式、移动端、中英文体验和截图观察。

文档区布局参考 Vite / Vite 中文文档站的三栏阅读体验，但主题使用 Bifrost 绿色系；首页不参考 Vite 首页，保持独立产品首屏。

## 前置条件

1. 当前工作目录位于仓库根目录。
2. 当前分支为 `codex/docs-site-redesign-plan` 或等效任务分支。
3. 站点依赖已安装：`pnpm --dir site install --frozen-lockfile`。
4. 方案文档存在：`design/docs-site-redesign.md`。
5. 首页源文件存在：`site/home/index.html`、`site/home/styles.css`、`site/home/home.js`。
6. VitePress 配置存在：`site/.vitepress/config.mjs`、`site/.vitepress/theme/style.css`。
7. 构建脚本存在：`site/scripts/build-home.mjs`、`site/scripts/verify-home.mjs`、`site/scripts/write-redirects.mjs`、`site/scripts/home-static-lib.mjs`。

## 测试用例列表

### TC-DSR-01 用户目标与技术方案一致

操作步骤：

1. 执行 `rg -n "必须实现|必须不破坏|必须真实验证|必须交付" design/docs-site-redesign.md`。
2. 执行 `rg -n "VitePress|docs/|docs-en/|build-home|verify-home|With AI|Coding Agent" design/docs-site-redesign.md`。
3. 检查方案是否明确写出首页为手写 HTML、CSS 和少量原生 JavaScript。
4. 检查方案是否明确写出部署构建每次都自动从 `docs/` 与 `docs-en/` 生成文档区内容。

预期结果：

- 四类目标清单均存在。
- 首页纯 HTML 和非框架生成约束明确。
- 文档区切到 VitePress、自动同步源文档、AI 首页和 With AI tab 均有明确描述。

执行结果：

- 已执行，通过。

### TC-DSR-02 首页源文件独立于 VitePress

操作步骤：

1. 执行 `test -f site/home/index.html && test -f site/home/styles.css && test -f site/home/home.js`。
2. 执行 `test ! -f site/src/pages/index.astro`。
3. 执行 `rg -n "astro-island|/_astro/|@vite/client|vitepress-theme-appearance|VPNav|React|Vue" site/home`，预期无匹配。
4. 执行 `rg -n "role=\"tablist\"|aria-selected|data-lang=\"zh\"|AI-era proxy|With AI|bifrost install-skill|Coding Agent|%BASE_PATH%" site/home/index.html`。
5. 执行 `rg -n "bifrost start -d" site/home/index.html`。
6. 执行 `! rg -n "bifrost start --no-system-proxy" site/home/index.html site/home/home.js`。
7. 执行 `rg -n "prefers-color-scheme: dark|color-scheme: light dark" site/home/styles.css`。
8. 执行 `rg -n "translations|zh:|setLanguage|navigator.language" site/home/home.js`。

预期结果：

- 首页源文件均存在。
- 旧 Astro 首页不存在。
- `site/home` 不包含 Astro、VitePress、React 或 Vue runtime 标记。
- 首页包含 base path 占位符、AI 产品首屏文案、With AI tab、skill 安装示例、可访问 Tab 标记和中英文切换标记。
- 首页首次启动示例为 `bifrost start -d`，不默认推荐 `bifrost start --no-system-proxy`。
- 样式包含暗色模式自适应。
- 原生 JS 包含中英文切换和浏览器中文语言自动适配。

执行结果：

- 已执行，通过。

### TC-DSR-03 构建脚本保证每次部署自动同步文档

操作步骤：

1. 执行 `node -e "const p=require('./site/package.json'); console.log(p.scripts.build)"`。
2. 检查 build 脚本顺序是否为：`sync-docs.mjs` -> `verify-docs-sync.mjs` -> `vitepress build .` -> `write-redirects.mjs` -> `build-home.mjs` -> `verify-home.mjs` -> `verify-site-links.mjs`。
3. 执行 `rg -n "Future English Docs Probe|With AI|bifrost install-skill|/_astro/" e2e-tests/tests/test_site_docs_sync.sh`。
4. 执行 `rg -n "srcDir|outDir|cleanUrls|locales|i18nRouting|sidebar|search" site/.vitepress/config.mjs`。

预期结果：

- build 脚本每次部署都会先同步并校验源文档。
- VitePress 构建后写 legacy redirect，再覆盖静态首页。
- E2E 同时覆盖英文文档同步、VitePress 构建和静态首页 AI 产物。
- VitePress 配置包含 source dir、output dir、clean URL、i18n、sidebar 和搜索配置。

执行结果：

- 已执行，通过。

### TC-DSR-04 文档区 VitePress 阅读布局

操作步骤：

1. 执行 `rg -n "defineConfig|vitepress|srcDir|themeConfig|sidebar|local" site/.vitepress/config.mjs`。
2. 执行 `rg -n -- "--vp-c-brand-1: #0f9f7e|--vp-c-brand-1: #8ff3dd|--vp-layout-max-width|VPSidebar|VPDocAsideOutline|content-container" site/.vitepress/theme/style.css`。
3. 执行 `rg -n "VitePress|Vite 中文文档|三栏阅读|首页仍保持单独设计" design/docs-site-redesign.md`。
4. 检查 `site/home/index.html` 和 `site/home/styles.css` 没有因参考 Vite 文档区而新增 VitePress、Vue 或 Vite 首页相关代码。

预期结果：

- 技术方案明确文档区采用 VitePress 并参考 Vite 中文文档站，首页不参考 Vite 首页。
- 文档区主题包含固定正文宽度、左 sidebar、中正文、右目录的覆盖点。
- 文档区主题以绿色 accent 为主，但背景仍为白/深灰阅读底色，不是一整片绿色。
- 首页仍是独立纯 HTML 源，未引入文档区框架或 Vite 首页样式。

执行结果：

- 已执行，通过。

### TC-DSR-05 站点单元测试通过

操作步骤：

1. 执行 `pnpm --dir site run test`。

预期结果：

- docs sync 单元测试通过。
- static home 单元测试通过。
- 命令退出码为 0。

执行结果：

- 已执行，通过，8 个测试通过。

### TC-DSR-06 完整构建产物为静态首页加同步文档区

操作步骤：

1. 执行 `pnpm --dir site run build`。
2. 执行 `test -f site/dist/index.html`。
3. 执行 `grep -q 'A one-stop proxy solution for the AI era' site/dist/index.html`。
4. 执行 `grep -q 'With AI' site/dist/index.html`。
5. 执行 `grep -q 'bifrost install-skill' site/dist/index.html`。
6. 执行 `grep -q 'href="/bifrost/docs/"' site/dist/index.html`。
7. 执行 `grep -q 'href="/bifrost/en/reference/"' site/dist/index.html`。
8. 执行 `grep -q 'role="tablist"' site/dist/index.html`。
9. 执行 `grep -q 'data-lang="zh"' site/dist/index.html`。
10. 执行 `grep -q 'bifrost start -d' site/dist/index.html`。
11. 执行 `! grep -q 'bifrost start --no-system-proxy' site/dist/index.html`。
12. 执行 `! grep -q '/_astro/' site/dist/index.html`。
13. 执行 `! grep -q 'vitepress-theme-appearance' site/dist/index.html`。
14. 执行 `test -f site/dist/docs/index.html && test -f site/dist/en/reference/index.html`。

预期结果：

- 完整构建成功。
- 首页最终产物来自手写静态源。
- 首页链接使用 GitHub Pages `/bifrost/` base path。
- 首页不包含 Astro 或 VitePress runtime 资源。
- 首页包含 AI 定位、With AI tab、skill 安装示例和中英文切换控件。
- 首页默认启动指引使用后台 daemon，不把测试隔离参数作为新用户首选命令。
- 中文和英文 VitePress 文档区产物存在。

执行结果：

- 已执行，通过。

### TC-DSR-07 docs sync E2E 覆盖未来新增文档

操作步骤：

1. 执行 `bash e2e-tests/tests/test_site_docs_sync.sh`。

预期结果：

- 脚本临时新增中文和英文源文档。
- 新增文档被同步到 `site/src/content/docs/`。
- 新增文档进入 `site/dist`。
- VitePress 构建、静态首页断言和站内链接校验通过。
- 脚本清理临时源文档、生成文档和 `site/dist`。

执行结果：

- 已执行，通过。

### TC-DSR-08 本地预览首页体验验证

操作步骤：

1. 如果 `site/dist/index.html` 不存在，先执行 `pnpm --dir site run build`。
2. 启动 VitePress preview：`pnpm --dir site exec vitepress preview . --host 127.0.0.1 --port 4177`。
3. 在浏览器打开 `http://127.0.0.1:4177/bifrost/`。
4. 检查首屏是否显示 Bifrost 标题、Install 和 Read Docs。
5. 点击 `With AI` tab。
6. 检查 `With AI` 面板包含 `bifrost install-skill`、`bifrost traffic search --include headers,body ...` 和 Agent case。
7. 检查 CLI 面板首行是 `bifrost start -d`，页面没有出现 `bifrost start --no-system-proxy`。
8. 点击 `中文`，检查首屏、CTA、工作流和开始步骤切换为中文。
9. 在桌面和移动端分别截图，观察文本对比度、按钮状态、工作台面板和卡片层级。

预期结果：

- 首页首屏可直接理解 AI 时代一站式代理方案和 Coding Agent 配合方式。
- 首次体验路径不要求用户理解测试隔离参数或手动配置系统代理边界。
- With AI 面板完整显示，不裁切结果卡。
- Tab 交互无需页面跳转。
- 中英文切换不跳页，且中文文案没有撑破卡片或按钮。
- 桌面和移动布局无明显重叠、遮挡或横向溢出。

执行结果：

- 已执行，通过。
- 截图：`/tmp/bifrost-home-vitepress-overlay-desktop.png`、`/tmp/bifrost-home-vitepress-overlay.png`、`/tmp/bifrost-home-vitepress-mobile.png`。
- 执行中发现 With AI 面板固定高度裁切结果卡，已修复并复测通过。

### TC-DSR-09 本地预览文档区体验验证

操作步骤：

1. 如果 `site/dist/getting-started/overview.html` 不存在，先执行 `pnpm --dir site run build`。
2. 启动 VitePress preview：`pnpm --dir site exec vitepress preview . --host 127.0.0.1 --port 4177`。
3. 在浏览器打开 `http://127.0.0.1:4177/bifrost/getting-started/overview`。
4. 在桌面宽度检查顶部导航、左侧 sidebar、中间正文、右侧本页目录同时可见。
5. 检查当前 sidebar 项、正文链接、inline code 和右侧目录风格使用绿色 accent，而不是蓝紫默认主题。
6. 切换暗色模式，检查正文、sidebar、代码块和右侧目录对比度。
7. 打开 `http://127.0.0.1:4177/bifrost/en/getting-started/overview`，检查英文正文、英文 sidebar 和英文 `html lang`。
8. 打开移动宽度视图，检查菜单按钮、正文优先显示、无横向溢出；点击移动菜单，确认 sidebar 项可见。

预期结果：

- 文档区阅读布局接近 Vite 文档站的信息结构：固定导航、左侧分组导航、中间正文、右侧本页目录。
- Bifrost 绿色主题与 Vite 默认主题有明显区分。
- 首页仍保持本次自定义纯 HTML 风格，文档区样式没有污染首页。
- 中英文路由均可打开，语言入口不生成错误拼接路径。
- 桌面、暗色和移动布局无明显重叠、遮挡或横向溢出。

执行结果：

- 已执行，通过。
- 截图：`/tmp/bifrost-docs-vitepress-desktop.png`、`/tmp/bifrost-docs-vitepress-dark.png`、`/tmp/bifrost-docs-vitepress-english.png`、`/tmp/bifrost-docs-vitepress-mobile.png`。
- 执行中发现 Python 简单静态服务不支持 VitePress clean URL，改用 VitePress preview 验证真实文档路由。

### TC-DSR-10 变更范围与生成内容边界

操作步骤：

1. 执行 `git status --short`。
2. 执行 `git diff --stat`。
3. 执行 `rg -n "2026-06-29 PPE|发布前 PPE header|老 v4 CLI 调 legacy" docs/design/remote-invoke-v5-pop-hardening.md site/src/content/docs/reference/design/remote-invoke-v5-pop-hardening.md`。

预期结果：

- 变更范围只包含站点首页、VitePress 配置与主题、站点构建脚本、站点 E2E、设计文档、human_tests，以及由 docs sync 生成的内容漂移。
- `site/src/content/docs/reference/design/remote-invoke-v5-pop-hardening.md` 的差异能在源文档 `docs/design/remote-invoke-v5-pop-hardening.md` 中找到对应内容，证明它是 sync 产物。

执行结果：

- 已执行第 1、2 步，范围符合预期。
- 第 3 步将在提交前 Review/Fix/Test 轮次中复查。

## 清理步骤

1. 如果手动启动了 `vitepress preview` 或 `python3 -m http.server`，测试完成后停止该进程。
2. `e2e-tests/tests/test_site_docs_sync.sh` 会自动清理临时文档和 `site/dist`。
