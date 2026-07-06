# Desktop Install Docs / Homepage Apps 真实场景测试

## 功能模块说明

验证 README、安装手册和官网首页已经补齐桌面端 App 安装说明；官网首屏右侧工作台提供 `Apps` tab，用户可直接点击平台卡片下载最新桌面安装包，不需要跳转到 GitHub Releases 页面手动找资产。验证部署边界：主仓库提交源码和文档，`bifrost-proxy.github.io` 仓库只同步 `site/dist/` 发布产物。

## 前置条件

1. 当前工作目录位于 Bifrost 主仓库任务分支。
2. 已执行 `pnpm --dir site run build`。
3. 本地预览服务可用：`pnpm --dir site exec vitepress preview . --host 127.0.0.1 --port 4177`。
4. 网络可以访问 GitHub Release 静态下载 URL 和用户提供的 Windows/macOS 图标地址。

## 测试用例列表

### TC-DID-01 README 和安装手册包含桌面端安装路径

操作步骤：

1. 执行 `rg -n "桌面端|Desktop App|Install the Desktop App|Install the desktop app|aarch64-apple-darwin|x86_64-apple-darwin|x86_64-pc-windows-msvc|aarch64-pc-windows-msvc" README.md README.en.md docs/getting-started.md docs-en/getting-started.md docs/desktop.md docs-en/desktop.md`。
2. 检查 README 和 README.en.md 是否说明从 GitHub Releases 选择 `.dmg` / `.msi` 的基本路径。
3. 检查 `docs/getting-started.md` 和 `docs-en/getting-started.md` 是否把 CLI 与 Desktop 两条安装路线并列。
4. 检查 `docs/desktop.md` 和 `docs-en/desktop.md` 是否覆盖 macOS Apple Silicon、macOS Intel、Windows x64 和 Windows ARM64。

预期结果：

- README、英文 README、中文安装手册、英文安装手册和 desktop 手册均能搜索到桌面端安装说明。
- macOS Apple Silicon 与 Intel 区分清楚，Windows x64 与 ARM64 区分清楚。

执行结果：

- 已执行，通过。

### TC-DID-02 首页 Hero 不保留单独 Desktop App 按钮

操作步骤：

1. 执行 `pnpm --dir site run build`。
2. 执行 `curl -sS http://127.0.0.1:4177/bifrost/ | rg -n 'class="hero-actions"|href="#install"|href="/bifrost/docs/"|href="https://github.com/bifrost-proxy/bifrost"|href="#desktop"'`。
3. 检查首屏 Hero CTA 是否只保留安装、文档和 GitHub 入口，且不再出现 `href="#desktop"`。

预期结果：

- 首页 Hero 区不再出现单独 `Desktop App` / `桌面端 App` 按钮。
- 桌面端下载入口统一收敛到右侧工作台的 `Apps` tab。

执行结果：

- 已执行，通过，Hero CTA 只保留安装、文档和 GitHub 入口。

### TC-DID-03 Apps tab 顺序和卡片排序符合首屏设计

操作步骤：

1. 执行 `curl -sS http://127.0.0.1:4177/bifrost/ | perl -0ne 'while(/aria-controls="(panel-[^"]+)"/g){print "$1\n"}'`。
2. 执行 `curl -sS http://127.0.0.1:4177/bifrost/ | perl -0ne 'while(/data-download-target="([^"]+)"/g){print "$1\n"}'`。
3. 在浏览器打开 `http://127.0.0.1:4177/bifrost/`，点击右侧工作台 `Apps` tab。
4. 检查第一排是否为 `macOS Apple Silicon`、`macOS Intel`。
5. 检查第二排是否为 `Windows`、`Windows ARM64`。

预期结果：

- tab 顺序为 `panel-cli`、`panel-apps`、`panel-web`、`panel-rules`。
- 下载卡片顺序为 `mac-arm`、`mac-intel`、`win-x64`、`win-arm`。
- Apps 面板使用用户提供的 macOS 与 Windows 图标。

执行结果：

- 已执行，通过。

### TC-DID-04 Apps 点击下载使用静态发布命名规则

操作步骤：

1. 执行 `node scripts/sync-site-downloads.mjs`。
2. 执行 `node -e 'const m=require("./site/desktop-downloads.json"); console.log(m.tag, Object.values(m.assets).join("\\n"))'`。
3. 执行 `rg -n "desktop-downloads-data|bifrost-desktop-v0.0.141-aarch64-apple-darwin.dmg|data-download-target" site/home site/dist/index.html site/dist/assets/home.*.js`。
4. 执行 `! rg -n "api.github.com/repos/bifrost-proxy/bifrost/releases/latest|browser_download_url" site/home site/.vitepress/theme site/dist/index.html site/dist/assets -g "*.js"`。

预期结果：

- `site/desktop-downloads.json` 使用当前发布版本生成四类桌面安装包文件名。
- 首页 HTML 内嵌静态下载 metadata，首页 JS 只按命名规则拼 release asset 直链。
- 首页和文档产物中不包含运行时 GitHub Releases API 请求。

执行结果：

- 已执行，通过，当前静态版本为 `v0.0.141`，四个资产直链均按命名规则生成，页面产物不再包含 GitHub Releases API 请求。

### TC-DID-05 安装手册页提供直接下载按钮

操作步骤：

1. 执行 `curl -sS http://127.0.0.1:4177/bifrost/getting-started/installation | rg -n "直接下载最新版|desktop-download-card|data-vp-download-target"`。
2. 执行 `curl -sS http://127.0.0.1:4177/bifrost/getting-started/installation | perl -0ne 'while(/data-vp-download-target="([^"]+)"/g){print "$1\n"}'`。
3. 执行 `rg -n "bifrost-desktop-v0.0.141-aarch64-apple-darwin.dmg|data-vp-download-target" site/.vitepress/theme/index.js site/dist/assets -g "*.js"`，并检查 `site/dist/getting-started/installation.html` 与 `site/dist/en/getting-started/installation.html`。
4. 在浏览器打开 `http://127.0.0.1:4177/bifrost/getting-started/installation`，检查 `安装桌面端 App` 段落下是否展示四个下载按钮。

预期结果：

- 安装手册不再要求用户进入 GitHub Releases 手动展开 `Assets` 查找桌面端安装包。
- 下载按钮顺序为 `mac-arm`、`mac-intel`、`win-x64`、`win-arm`。
- VitePress 主题脚本会读取静态 manifest 并把按钮 href 替换为 release asset 直链。

执行结果：

- 已执行，通过。

### TC-DID-06 部署边界与产物同步

操作步骤：

1. 在主仓库执行 `SITE_URL=https://bifrost-proxy.github.io/ BASE_PATH=/ pnpm --dir site run build`。
2. 执行 `test -f site/dist/index.html && test -f site/dist/docs/index.html && test -f site/dist/getting-started/desktop.html && test -f site/dist/en/getting-started/desktop.html && test -f site/dist/og-image.png`。
3. 将 `site/dist/` 同步到 `bifrost-proxy.github.io` 仓库 checkout。
4. 在部署仓库执行 `git status --short` 和 `git diff --stat`。
5. 检查部署仓库变更是否为构建产物，不直接手写另一套首页。

预期结果：

- 主仓库保留首页和文档源码。
- 部署仓库只承载根路径发布产物。
- 根路径构建后 `href` 使用 `/docs/`、`/getting-started/...` 等根站路径。

执行结果：

- 已执行，通过。已使用根路径构建同步到 `bifrost-proxy.github.io` checkout，部署仓库仅包含 `site/dist/` 产物变更；根站 HTML 不含 `/bifrost` 资源前缀。

## 清理步骤

1. 停止本地 preview 进程。
2. 如果创建临时检查文件，删除临时文件。
3. 保留 `site/dist/` 作为部署仓库同步来源。
