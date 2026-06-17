# Docs Site Generator Completeness

## 功能模块

站点生成器负责把仓库根目录 `docs/` 与 `docs-en/` 下的 Markdown 文档同步到 Astro/Starlight 站点的 `site/src/content/docs/`，再由 GitHub Pages workflow 构建和部署。

本次需求在原有中文站点同步能力基础上新增英文文档体系：

- `README.md` 保留中文首页，并提供 `README.en.md`、`docs-en/README.md` 与站点入口链接。
- `README.en.md` 作为独立英文 README，提供英文快速开始、功能说明和英文文档索引。
- `docs-en/` 作为独立英文文档目录，与 `docs/` 保持同构路径，便于中英文互相切换。
- 文档站点同时生成中文路由和英文 `/en/...` 路由，首页、404、Starlight 文档页均提供英文入口。

## 实现逻辑

1. `site/scripts/docs-sync-lib.mjs` 递归发现 `docs/**/*.md` 与 `docs-en/**/*.md`，忽略隐藏文件。
2. 对中文核心文档保留稳定路由覆盖，例如：
   - `docs/overview.md` -> `getting-started/overview.mdx`
   - `docs/getting-started.md` -> `getting-started/installation.mdx`
   - `docs/rule.md` -> `reference/rule-engine.md`
   - `docs/rules/README.md` -> `reference/rules/index.md`
3. 对英文核心文档生成 `/en` 命名空间下的稳定路由，例如：
   - `docs-en/overview.md` -> `en/getting-started/overview.mdx`
   - `docs-en/getting-started.md` -> `en/getting-started/installation.mdx`
   - `docs-en/rule.md` -> `en/reference/rule-engine.md`
   - `docs-en/rules/README.md` -> `en/reference/rules/index.md`
4. 对未显式覆盖的未来文档使用默认规则：
   - `docs/<name>.md` -> `reference/<name>.md`
   - `docs-en/<name>.md` -> `en/reference/<name>.md`
   - `docs[-en]/<dir>/README.md` -> 对应 `reference/<dir>/index.md`
   - `docs[-en]/<dir>/<name>.md` -> 对应 `reference/<dir>/<name>.md`
5. 生成文件写入中英文来源标记、Starlight frontmatter、`sidebar.label` 和 `sidebar.order`。
6. 生成前清理旧的中英文自动生成页面，避免删除源文档后站点仍残留旧页面。
7. `site/astro.config.mjs` 使用 Starlight `locales` 配置声明 `zh-CN` 与 `en-US`，并配置站点级 alternate 链接。
8. `site/src/pages/index.astro` 在首页导航和 hero CTA 中提供英文文档入口。
9. `site/scripts/verify-docs-sync.mjs` 在站点构建前验证每个 `docs/**/*.md` 与 `docs-en/**/*.md` 都存在对应生成目标，并校验中英文来源标记。
10. `site/scripts/verify-site-links.mjs` 在 Astro 构建后扫描 `site/dist/**/*.html` 的本地 `href` / `src`，任何指向缺失页面或静态资源的站内链接都会让构建失败。
11. `pnpm --dir site run build` 在构建前清理 `site/dist`，避免被删除的中英文 docs 页面残留进部署产物。

## 依赖项

- Node.js >= 22
- pnpm
- Astro / Starlight
- GitHub Pages workflow `.github/workflows/site.yml`

## 测试方案

### 单元测试

- `pnpm --dir site run docs:test`
- 覆盖递归发现中文和英文文档、英文默认路由、英文相对链接重写、旧中英文生成文件清理、生成来源标记和站点内链扫描。

### E2E 测试

- `pnpm --dir site run build`
- 覆盖真实仓库 `docs/` 与 `docs-en/` 当前全量文档、同步校验脚本、历史深链重定向、全站内链扫描和真实 Astro/Starlight 构建产物。

### 真实场景测试

- `human_tests/docs-site-generator.md`（planned, not yet shipped as of 2026-06-16；当前 `human_tests/` 目录下尚未落地该用例文件）
- 覆盖中文 README 到英文 README 的切换、中文 docs 到英文 docs 的切换、站点首页英文入口、英文 `/en/...` 构建产物、英文站点内链、未来新增英文文档自动纳入和清理。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标与当前 diff。
- 检查 `README.md` / `README.en.md` / `docs/README.md` / `docs-en/README.md` 是否提供双语切换入口。
- 检查 `site/scripts/docs-sync-lib.mjs` 是否覆盖 `docs-en/**/*.md` 并维持中文路由兼容。
- 运行 `pnpm --dir site run docs:test`、`pnpm --dir site run docs:sync`、`pnpm --dir site run docs:verify`。

### 第 2 轮

- 复查第 1 轮修复后的 diff、生成文件和测试资产。
- 运行真实站点构建、内链验证和 human_tests 中定义的真实构建路径。
- 如发现漏生成、导航缺失、残留旧文件或部署构建失败，继续追加新一轮。

## 校验要求

- 必须执行 `pnpm --dir site run docs:test`
- 必须执行 `pnpm --dir site run docs:sync`
- 必须执行 `pnpm --dir site run docs:verify`
- 必须执行 `pnpm --dir site run site:verify-links`
- 必须执行 `pnpm --dir site run build`
- 必须执行 `git diff --check`
- 收尾阶段按项目规则执行 rust-project-validate；本次仅修改文档与站点脚本，Rust workspace all-features 可标记为不适用，但需在最终验证矩阵中说明原因。

## 文档更新要求

- 同步更新 `README.md`、新增 `README.en.md`。
- 新增 `docs-en/` 英文文档目录。
- 同步更新 `human_tests/docs-site-generator.md` 和 `human_tests/readme.md`（两份 human_tests 文档 planned, not yet shipped as of 2026-06-16）。
- 若未来调整 docs 路由策略，需要同步更新本文档、E2E 脚本和 human_tests 用例。
