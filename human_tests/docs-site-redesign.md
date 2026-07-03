# Docs Site Redesign 真实场景测试

## 功能模块说明

验证文档站重构已经真实落地：首页由手写静态 HTML 交付，构建期间自动同步 `docs/` 与 `docs-en/`，文档区继续由 Astro/Starlight 生成，最终部署产物中的首页不包含 Astro runtime，并覆盖明暗模式与中英文体验。

## 前置条件

1. 当前工作目录位于仓库根目录。
2. 当前分支为 `codex/docs-site-redesign-plan` 或等效任务分支。
3. 站点依赖已安装：`pnpm --dir site install --frozen-lockfile`。
4. 方案文档存在：`design/docs-site-redesign.md`。
5. 首页源文件存在：`site/home/index.html`、`site/home/styles.css`、`site/home/home.js`。
6. 构建脚本存在：`site/scripts/build-home.mjs`、`site/scripts/verify-home.mjs`、`site/scripts/home-static-lib.mjs`。

## 测试用例列表

### TC-DSR-01 用户目标与技术方案一致

操作步骤：
1. 执行 `rg -n "必须实现|必须不破坏|必须真实验证|必须交付" design/docs-site-redesign.md`。
2. 执行 `rg -n "docs/|docs-en/|build-home|verify-home|docs-en/\\*\\*" design/docs-site-redesign.md`。
3. 检查方案是否明确写出首页为手写 HTML、CSS 和少量原生 JavaScript。
4. 检查方案是否明确写出部署构建每次都自动从 `docs/` 与 `docs-en/` 生成文档区内容。

预期结果：
- 四类目标清单均存在。
- 首页纯 HTML 和非框架生成约束明确。
- 自动同步源文档、英文文档触发 CI、文档区保留 Starlight 均有明确描述。

### TC-DSR-02 首页源文件独立于 Astro

操作步骤：
1. 执行 `test -f site/home/index.html && test -f site/home/styles.css && test -f site/home/home.js`。
2. 执行 `test ! -f site/src/pages/index.astro`。
3. 执行 `rg -n "astro-island|/_astro/|@vite/client|React|Vue" site/home`，预期无匹配。
4. 执行 `rg -n "role=\"tablist\"|aria-selected|data-lang=\"zh\"|A proxy workbench for capturing traffic|%BASE_PATH%" site/home/index.html`。
5. 执行 `rg -n "bifrost start -d" site/home/index.html`。
6. 执行 `! rg -n "bifrost start --no-system-proxy" site/home/index.html site/home/home.js`。
7. 执行 `rg -n "prefers-color-scheme: dark|color-scheme: light dark" site/home/styles.css`。
8. 执行 `rg -n "translations|zh:|setLanguage|navigator.language" site/home/home.js`。

预期结果：
- 首页源文件均存在。
- 旧 Astro 首页不存在。
- `site/home` 不包含 Astro runtime、React 或 Vue 标记。
- 首页包含 base path 占位符、产品首屏文案、可访问 Tab 标记和中英文切换标记。
- 首页首次启动示例为 `bifrost start -d`，不默认推荐 `bifrost start --no-system-proxy`。
- 样式包含暗色模式自适应。
- 原生 JS 包含中英文切换和浏览器中文语言自动适配。

### TC-DSR-03 构建脚本保证每次部署自动同步文档

操作步骤：
1. 执行 `node -e "const p=require('./site/package.json'); console.log(p.scripts.build)"`。
2. 检查 build 脚本顺序是否为：`sync-docs.mjs` -> `verify-docs-sync.mjs` -> `astro build` -> `build-home.mjs` -> `verify-home.mjs` -> `verify-site-links.mjs`。
3. 执行 `rg -n "docs-en/\\*\\*" .github/workflows/site.yml`。
4. 执行 `rg -n "Future English Docs Probe|role=\"tablist\"|/_astro/" e2e-tests/tests/test_site_docs_sync.sh`。

预期结果：
- build 脚本每次部署都会先同步并校验源文档。
- 静态首页在 Astro build 之后覆盖最终 `dist/index.html`。
- 英文文档源变化会触发站点 CI。
- E2E 同时覆盖英文文档同步和静态首页产物。

### TC-DSR-04 站点单元测试通过

操作步骤：
1. 执行 `pnpm --dir site run test`。

预期结果：
- docs sync 单元测试通过。
- static home 单元测试通过。
- 命令退出码为 0。

### TC-DSR-05 完整构建产物为静态首页加同步文档区

操作步骤：
1. 执行 `pnpm --dir site run build`。
2. 执行 `test -f site/dist/index.html`。
3. 执行 `grep -q 'A proxy workbench for capturing traffic' site/dist/index.html`。
4. 执行 `grep -q 'href="/bifrost/docs/"' site/dist/index.html`。
5. 执行 `grep -q 'href="/bifrost/en/reference/"' site/dist/index.html`。
6. 执行 `grep -q 'role="tablist"' site/dist/index.html`。
7. 执行 `grep -q 'data-lang="zh"' site/dist/index.html`。
8. 执行 `grep -q 'bifrost start -d' site/dist/index.html`。
9. 执行 `! grep -q 'bifrost start --no-system-proxy' site/dist/index.html`。
10. 执行 `! grep -q '/_astro/' site/dist/index.html`。
11. 执行 `test -f site/dist/docs/index.html && test -f site/dist/en/reference/index.html`。

预期结果：
- 完整构建成功。
- 首页最终产物来自手写静态源。
- 首页链接使用 GitHub Pages `/bifrost/` base path。
- 首页不包含 Astro runtime 资源。
- 首页包含中英文切换控件。
- 首页默认启动指引使用后台 daemon，不把测试隔离参数作为新用户首选命令。
- 中文和英文文档区产物仍存在。

### TC-DSR-06 docs sync E2E 覆盖未来新增文档

操作步骤：
1. 执行 `bash e2e-tests/tests/test_site_docs_sync.sh`。

预期结果：
- 脚本临时新增中文和英文源文档。
- 新增文档被同步到 `site/src/content/docs/`。
- 新增文档进入 `site/dist`。
- 静态首页断言通过。
- 脚本清理临时源文档、生成文档和 `site/dist`。

### TC-DSR-07 本地静态服务首页体验验证

操作步骤：
1. 如果 `site/dist/index.html` 不存在，先执行 `pnpm --dir site run build`。
2. 用 `/bifrost/` 前缀模拟 GitHub Pages：`rm -rf /tmp/bifrost-site-preview && mkdir -p /tmp/bifrost-site-preview && ln -s "$PWD/site/dist" /tmp/bifrost-site-preview/bifrost`。
3. 启动静态服务：`python3 -m http.server 4174 -d /tmp/bifrost-site-preview`。
4. 在浏览器打开 `http://127.0.0.1:4174/bifrost/`。
5. 检查首屏是否显示 Bifrost 标题、Install 和 Read Docs。
6. 点击 `CLI`、`Web UI`、`Rules` 三个 Tab。
7. 检查每次切换后只有当前 panel 可见，按钮 `aria-selected` 状态同步变化。
8. 检查 CLI 面板首行是 `bifrost start -d`，页面没有出现 `bifrost start --no-system-proxy`。
9. 点击 `中文`，检查首屏、CTA、工作流和开始步骤切换为中文，其中第 2 步为后台启动服务语义。
10. 在浅色和暗色模式下分别截图，观察文本对比度、按钮状态、工作台面板和卡片层级。
11. 打开移动宽度视图，检查标题、CTA、语言切换、工作台预览和下一段内容不重叠。

预期结果：
- 首页首屏可直接理解产品定位。
- 首次体验路径不要求用户理解测试隔离参数或手动配置系统代理边界。
- Tab 交互无需页面跳转。
- 中英文切换不跳页，且中文文案没有撑破卡片或按钮。
- 浅色和暗色模式都有清晰层级，不出现低对比文字。
- 桌面和移动布局无明显重叠、遮挡或横向溢出。

### TC-DSR-08 变更范围与生成内容边界

操作步骤：
1. 执行 `git status --short`。
2. 执行 `git diff --stat`。
3. 执行 `rg -n "2026-06-29 PPE|发布前 PPE header|老 v4 CLI 调 legacy" docs/design/remote-invoke-v5-pop-hardening.md site/src/content/docs/reference/design/remote-invoke-v5-pop-hardening.md`。

预期结果：
- 变更范围只包含站点首页、站点构建脚本、站点 E2E、设计文档、human_tests、workflow path filter，以及由 docs sync 生成的内容漂移。
- `site/src/content/docs/reference/design/remote-invoke-v5-pop-hardening.md` 的差异能在源文档 `docs/design/remote-invoke-v5-pop-hardening.md` 中找到对应内容，证明它是 sync 产物。

## 清理步骤

1. 如果手动启动了 `python3 -m http.server`，测试完成后停止该进程。
2. `e2e-tests/tests/test_site_docs_sync.sh` 会自动清理临时文档和 `site/dist`。
