# Docs Site Generator（中英文文档同步）设计方案

## 背景

Bifrost 面向用户的文档站基于 Astro/Starlight（历史）或 VitePress（重构后，详见 `docs-site-redesign.md`）。无论采用哪种前端方案，核心链路都是：仓库根目录 `docs/` 与 `docs-en/` 下的 Markdown 每次构建自动同步到 `site/src/content/docs/`，再由 GitHub Pages workflow 构建部署。

本方案聚焦“中英文双语文档同步生成”的能力：确保用户在 `README.md`、`README.en.md`、`docs/`、`docs-en/` 和站点首页/正文里都能一键切换语言，未来新增文档自动被纳入站点，删除文档不残留旧页面，构建前后有验证脚本保护。

## 用户目标验证清单

### 必须实现

- `docs/`、`docs-en/` 保持同构目录结构，便于翻译对齐与阅读切换。
- 站点同时生成中文根路由与英文 `/en/...` 路由。
- `README.md` 中文首页提供 `README.en.md` 与 `docs-en/README.md` 双语切换入口。
- `README.en.md` 独立英文 README，覆盖英文快速开始、功能说明和英文文档索引。
- 首页与 Starlight/VitePress 文档页均提供英文入口。
- `site/scripts/docs-sync-lib.mjs` 递归发现两侧 `**/*.md`，忽略隐藏文件。
- 已知核心文档使用稳定路由覆盖，未知文档使用默认规则。
- 构建前后有独立 verify 脚本：`verify-docs-sync.mjs`（源→生成一致性）、`verify-site-links.mjs`（产物站内链）。
- 每次构建都清理旧的自动生成中英文页面，避免删除源文档后站点仍残留旧页面。

### 必须不破坏

- 现有中文深链 `/docs/`、`/getting-started/`、`/reference/` 继续可访问。
- 手写页面（非自动生成）不被 sync 脚本覆盖。
- Astro/Starlight `locales` 配置或 VitePress `locales` 配置保持 `zh-CN` 与 `en-US`。
- 生成文件写入中/英文来源标记，便于后续 verify 与人工排查。
- GitHub Pages workflow `.github/workflows/site.yml` 无需为每次新增文档手动修改。

### 必须真实验证

- `pnpm --dir site run docs:test` 覆盖 lib 单测。
- `pnpm --dir site run docs:sync` 全量同步 `docs/` 与 `docs-en/`。
- `pnpm --dir site run docs:verify` 通过。
- `pnpm --dir site run site:verify-links` 通过。
- `pnpm --dir site run build` 完整跑通部署构建链路。
- `bash e2e-tests/tests/test_site_docs_sync.sh` 覆盖新增中/英文 probe 自动纳入。

## 产品语义

### 中英文对齐

`docs/` 是主中文文档目录，`docs-en/` 是主英文文档目录。两者路径同构，翻译时按同名对齐；未翻译文档暂时缺失英文侧文件，verify 脚本可容忍缺失但会输出警告。

### 稳定路由覆盖

核心文档由映射表覆盖，便于历史深链保持稳定：

- `docs/overview.md` → `getting-started/overview.mdx`
- `docs/getting-started.md` → `getting-started/installation.mdx`
- `docs/rule.md` → `reference/rule-engine.md`
- `docs/rules/README.md` → `reference/rules/index.md`
- `docs-en/overview.md` → `en/getting-started/overview.mdx`
- `docs-en/getting-started.md` → `en/getting-started/installation.mdx`
- `docs-en/rule.md` → `en/reference/rule-engine.md`
- `docs-en/rules/README.md` → `en/reference/rules/index.md`

### 默认规则

未覆盖的文档使用默认映射：

- `docs/<name>.md` → `reference/<name>.md`
- `docs-en/<name>.md` → `en/reference/<name>.md`
- `docs[-en]/<dir>/README.md` → 对应 `reference/<dir>/index.md`
- `docs[-en]/<dir>/<name>.md` → 对应 `reference/<dir>/<name>.md`

这样新增文档时不需要修改映射表，仍可自动出现在站点。

### 来源标记与 frontmatter

每个生成文件顶部写入：

- `sidebar.label` 与 `sidebar.order`（Starlight）或对应 VitePress sidebar 项。
- `# This page is automatically synced from \`docs[-en]/xxx.md\``。
- 源路径注释，便于 verify 脚本对照。

## 技术细节

### 关键模块

- `site/scripts/docs-sync-lib.mjs`：递归发现两侧 Markdown，输出 [{ source, target, frontmatter }] 列表；重构后同时被 VitePress `config.mjs` 的 `buildPagesSync()` 复用。
- `site/scripts/sync-docs.mjs`：命令行入口，`pnpm run docs:sync` 调用。
- `site/scripts/verify-docs-sync.mjs`：`pnpm run docs:verify` 调用，检查每个源文档都有对应生成目标、来源标记正确、无孤儿生成文件。
- `site/scripts/verify-site-links.mjs`：`pnpm run site:verify-links` 调用，扫描 `site/dist/**/*.html` 中所有本地 `href`/`src`，任何指向缺失页面或静态资源的站内链接都会让构建失败。
- `site/astro.config.mjs`（Starlight 时代）：`locales` 配置 `zh-CN` 与 `en-US`，站点级 alternate 链接。
- `site/.vitepress/config.mjs`（重构后）：直接复用 `docs-sync-lib.mjs` 输出生成 sidebar；关闭 `themeConfig.i18nRouting` 走显式映射。

### 构建流程（重构后 VitePress 版本，摘自 docs-site-redesign）

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

构建流程中每一步都是可独立回放的，本文档聚焦 `sync-docs` / `verify-docs-sync` / `verify-site-links` 三个环节，其它环节由 `docs-site-redesign.md` 主导。

### 清理旧生成文件

`sync-docs.mjs` 每次执行前枚举现有生成目录 `site/src/content/docs/reference/` 与 `site/src/content/docs/en/`，与本次生成目标集合做差集，删除孤儿文件。verify 脚本再次校验以防漏删。

### Markdown 内链重写

Starlight 时代：使用 `.mdx`，保留原文相对链接，Starlight base path 自动处理。
VitePress 时代：`docs-sync-lib.mjs` 增加 clean URL 重写，避免 build 阶段 dead link；示例：`[规则](rule.md)` → `[规则](/reference/rule-engine)`。

## CLI 与 Admin API

无独立 CLI/Admin API。所有能力通过 pnpm scripts 暴露：

- `pnpm --dir site run docs:sync`
- `pnpm --dir site run docs:verify`
- `pnpm --dir site run docs:test`
- `pnpm --dir site run site:verify-links`
- `pnpm --dir site run build`

`.github/workflows/site.yml` 在部署前依次调用 sync → verify → build → verify-links → deploy。

## Sync / 导入导出边界

文档站是单向消费仓库源，`docs/` 与 `docs-en/` 才是源。任何“反向从站点回写到 docs”的路径都被明确禁止：

- 生成产物 `site/src/content/docs/` 不进入 git（如需检查落盘可临时对比，但不 commit）。
- 用户或 Agent 修改文档只改 `docs/` 或 `docs-en/`，不直接改 `site/src/content/docs/`。

主站部署仓库 `bifrost-proxy/bifrost-proxy.github.io` 只承载产物，禁止手写另一套页面或独立文档入口。

## 实现切分

### Phase 1：中文单语同步

- `docs-sync-lib.mjs` 覆盖中文 `docs/`。
- 稳定路由覆盖 + 默认规则。
- verify-docs-sync 与 verify-site-links 上线。

### Phase 2：英文文档接入

- 新增 `README.en.md`、`docs-en/`。
- `docs-sync-lib.mjs` 递归发现 `docs-en/**/*.md` 并输出 `/en/...` 目标。
- `astro.config.mjs` / VitePress `locales` 声明 `en-US`。
- 首页与 Starlight/VitePress 顶部导航增加英文入口。

### Phase 3：孤儿清理与深链保护

- `sync-docs.mjs` 增加旧生成文件清理。
- `write-redirects.mjs` 为历史深链写入静态 redirect HTML。
- `verify-site-links.mjs` 校验 redirect 目标存在。

### Phase 4：E2E 与 human_tests

- `e2e-tests/tests/test_site_docs_sync.sh` 覆盖临时新增中/英文 probe。
- `human_tests/docs-site-generator.md` 覆盖中英切换、新增文档自动纳入、删除文档清理。

## 测试方案

### 单元测试

`pnpm --dir site run docs:test`，覆盖：

- 递归发现中文和英文文档。
- 英文默认路由生成。
- 英文相对链接重写。
- 旧中英文生成文件清理。
- 生成来源标记正确。
- verify-site-links 对缺失资源报错。

### E2E 测试

- `pnpm --dir site run build`：真实仓库全量 docs 构建。
- `bash e2e-tests/tests/test_site_docs_sync.sh`：临时在 `docs/` 与 `docs-en/` 增加 probe 文档，验证无需改站点配置即可被同步、构建、进入 `dist`，并覆盖首页 AI 文案、`With AI`、`bifrost install-skill`、`bifrost start -d`、禁止默认推荐 `--no-system-proxy`。

### 真实场景测试 human_tests

新增或更新 `human_tests/docs-site-generator.md`：

- TC-DSG-01：当前 `docs/` 与 `docs-en/` 全量同步，生成目录包含 `/en/reference/`、`/en/getting-started/`、`/en/reference/rules/`。
- TC-DSG-02：`README.md` 提供 `[English](README.en.md)` 与 `[English docs](docs-en/README.md)` 入口。
- TC-DSG-03：新增一份中/英文文档后重跑 sync + build，站点自动出现新页面。
- TC-DSG-04：删除源文档后重跑，生成目录不残留旧页面。
- TC-DSG-05：verify-site-links 能检出缺失静态资源。
- TC-DSG-06：站点首页与文档正文均能一键切换中英文。

同步更新 `human_tests/readme.md` 用例数量与说明。

### 覆盖率与项目校验

- `pnpm --dir site run docs:test`
- `pnpm --dir site run docs:sync`
- `pnpm --dir site run docs:verify`
- `pnpm --dir site run site:verify-links`
- `pnpm --dir site run build`
- `git diff --check`
- 本次仅修改文档与站点脚本，Rust workspace all-features 与 `rust-project-validate` 在最终验证矩阵中标记为不适用，需在交付备注中说明原因。

## 边界与非目标

- 本方案不覆盖首页设计与渲染方案（详见 `docs-site-redesign.md`）。
- 本方案不覆盖搜索索引具体实现（Starlight 内置或 VitePress local provider，按前端方案定）。
- 本方案不定义每份文档的写作规范或翻译流程，只保证已存在的中英文源文档能被正确同步与部署。
- 本方案不引入服务端渲染 / 动态数据源；文档站保持纯静态。
- 本方案不引入下拉语言选择器业务组件；语言切换由稳定路由 `/` ↔ `/en/...` + 顶部导航链接完成。

## 与其它设计文档的关系

- `docs-site-redesign.md`：负责首页与文档区前端框架、构建流程编排、主站部署边界；本文档为 sync 层提供稳定契约，两者互不越界。
- `AGENTS.md`：固化“不能在部署仓库维护第二套页面”的开发守则。
- `human_tests/docs-site-generator.md`：真实场景验证入口。
- `.github/workflows/site.yml`：CI/CD 入口，调用本文档定义的 pnpm scripts。

## 常见问题排查

- **新增文档没出现在站点**：检查 `pnpm --dir site run docs:sync` 输出是否覆盖该文件；检查文件是否在 `docs/` 或 `docs-en/` 根下（非隐藏目录）；执行 `pnpm --dir site run docs:verify` 看是否报缺失。
- **删除文档后站点仍有旧页**：确认 `sync-docs.mjs` 的孤儿清理执行；必要时手动删除 `site/src/content/docs/reference/<name>.md` 后重跑同步；再执行 `verify-site-links.mjs` 确认无 dead link。
- **英文页面路由到 `/reference/` 而非 `/en/reference/`**：检查 `docs-sync-lib.mjs` 是否正确识别 `docs-en/` 前缀；检查 frontmatter 中的来源标记是否指向英文源。
- **`verify-site-links.mjs` 报缺失 favicon 或图片**：确认静态资源放在 VitePress 的 `srcDir/public` 目录（重构后：`site/src/content/docs/public/`）；Starlight 时代对应目录不同。
- **GitHub Pages 部署 404**：确认 `BASE_PATH` / `SITE_URL` 与部署目标一致；`/bifrost/` 部署与根站部署使用不同的 base，混用会导致所有链接指向错误路径。

## 已知问题与后续演进

- 稳定路由覆盖表随文档演进增长，需要人工维护；未来考虑在源文档 frontmatter 中显式声明目标路由，减少映射表。
- Verify 脚本目前对中英文缺失一侧不阻塞；若产品阶段要求“英文必须与中文一比一”，可将缺失升级为构建失败。
- Markdown clean URL 重写只处理相对链接；绝对链接、锚点、外部链接不做处理，需要作者自己维护。
- Astro/Starlight → VitePress 迁移期间 sync 层要同时兼容两种目标格式；迁移完成后可以移除 Astro 侧的适配代码。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标与 diff。
- 检查 `README.md` / `README.en.md` / `docs/README.md` / `docs-en/README.md` 是否提供双语切换入口。
- 检查 `docs-sync-lib.mjs` 是否覆盖 `docs-en/**/*.md` 并维持中文路由兼容。
- 运行 `docs:test`、`docs:sync`、`docs:verify`、`site:verify-links`。

### 第 2 轮

- 复查第 1 轮修复后的 diff、生成文件和测试资产。
- 运行真实 `pnpm --dir site run build` 与 `test_site_docs_sync.sh`。
- 若发现漏生成、导航缺失、残留旧文件或部署构建失败，继续追加一轮。

## 依赖项

- Node.js >= 22
- pnpm
- Astro / Starlight（Phase 1-2）或 VitePress（Phase 2+，详见 `docs-site-redesign.md`）
- GitHub Pages workflow `.github/workflows/site.yml`

## 文档更新要求

- 同步更新 `README.md`、新增 `README.en.md`。
- 新增 `docs-en/` 英文文档目录。
- 同步更新 `human_tests/docs-site-generator.md` 和 `human_tests/readme.md`。
- 若未来调整 docs 路由策略，需要同步更新本文档、E2E 脚本和 human_tests 用例。

## 风险与决策点

- 中英文翻译进度不同步：未翻译文档暂时缺失英文侧文件，verify 脚本发出警告但不阻塞构建，避免影响中文文档发布节奏。
- 稳定路由覆盖表是维护成本：核心文档改名时需要同步更新映射，忘记会造成 dead link；verify-site-links 会兜底。
- Astro/Starlight → VitePress 切换：详见 `docs-site-redesign.md`，本文档不重复描述前端框架决策，只保证 sync 层输出契约稳定。
- 主站部署仓库不能维护第二套内容：若发现 `bifrost-proxy.github.io` 出现独立首页或独立文档入口，必须回退到从本仓库构建产物同步。
- GitHub Pages base path 依赖 `SITE_URL` / `BASE_PATH`：切换根站 vs. `/bifrost/` 部署时不要修改源文档，只在构建命令层控制。
