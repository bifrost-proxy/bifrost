# Docs Site Generator Completeness

## 功能模块

站点生成器负责把仓库根目录 `docs/` 下的 Markdown 文档同步到 Astro/Starlight 站点的 `site/src/content/docs/`，再由 GitHub Pages workflow 构建和部署。

本次修复目标是消除手写页面清单导致的覆盖缺口：当前 `docs/` 下所有 `.md` 文件必须生成站点页面；未来新增的 `docs/**/*.md` 文件也必须在无需修改生成器清单的情况下自动进入构建产物和导航。

## 实现逻辑

1. `site/scripts/docs-sync-lib.mjs` 递归发现 `docs/**/*.md`，忽略隐藏文件。
2. 对已有核心文档保留稳定路由覆盖，例如：
   - `docs/overview.md` -> `getting-started/overview.mdx`
   - `docs/getting-started.md` -> `getting-started/installation.mdx`
   - `docs/rule.md` -> `reference/rule-engine.md`
   - `docs/rules/README.md` -> `reference/rules/index.md`
3. 对未显式覆盖的未来文档使用默认规则：
   - `docs/<name>.md` -> `reference/<name>.md`
   - `docs/<dir>/README.md` -> `reference/<dir>/index.md`
   - `docs/<dir>/<name>.md` -> `reference/<dir>/<name>.md`
4. 生成文件写入来源标记、Starlight frontmatter、`sidebar.label` 和 `sidebar.order`。
5. 生成前清理旧的自动生成页面，避免删除已移除的 docs 文档后站点仍残留旧页面。
6. `site/astro.config.mjs` 使用 Starlight `autogenerate` 目录导航，让新增页面自动进入侧边栏。
7. `site/scripts/verify-docs-sync.mjs` 在站点构建前验证每个 `docs/**/*.md` 都存在对应生成目标，并校验来源标记。

## 依赖项

- Node.js >= 22
- Astro / Starlight
- GitHub Pages workflow `.github/workflows/site.yml`

## 测试方案

### 单元测试

- `pnpm --dir site run docs:test`
- 覆盖递归发现、未来新增文档默认路由、README 映射、相对链接重写、旧生成文件清理和生成来源标记。

### E2E 测试

- `bash e2e-tests/tests/test_site_docs_sync.sh`
- 覆盖真实仓库 `docs/` 当前全量文档、临时新增未来文档、同步校验脚本和真实 `pnpm --dir site run build` 构建产物。

### 真实场景测试

- `human_tests/docs-site-generator.md`
- 覆盖当前文档完整性、未来新增文档自动纳入、部署构建校验和清理后无残留。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标与当前 diff。
- 检查 `site/scripts/docs-sync-lib.mjs` 是否仍包含手写全量页面清单依赖。
- 检查 `site/astro.config.mjs` 是否通过目录自动导航覆盖新增文档。
- 运行 `pnpm --dir site run docs:test`、`pnpm --dir site run docs:sync`、`pnpm --dir site run docs:verify`。

### 第 2 轮

- 复查第 1 轮修复后的 diff、生成文件和测试资产。
- 运行 E2E 与 human_tests 中定义的真实构建路径。
- 如发现漏生成、导航缺失、残留旧文件或部署构建失败，继续追加新一轮。

## 校验要求

- 必须执行 `pnpm --dir site run docs:test`
- 必须执行 `pnpm --dir site run docs:verify`
- 必须执行 `pnpm --dir site run build`
- 必须执行 `bash e2e-tests/tests/test_site_docs_sync.sh`
- 收尾阶段按项目规则执行 rust-project-validate；若 Rust 相关检查因本次仅改站点文档工具不适用，需在最终验证矩阵中说明。
- 项目规则要求的 `cargo test --workspace --all-features` 仍需至少执行一次或明确记录阻塞原因。

## 文档更新要求

- 同步更新 `human_tests/docs-site-generator.md` 和 `human_tests/readme.md`。
- 若未来调整 docs 路由策略，需要同步更新本文档、E2E 脚本和 human_tests 用例。
