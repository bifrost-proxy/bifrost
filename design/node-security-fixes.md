# Node.js security dependency fixes

## 功能模块说明

本次修复 GitHub Dependabot 中所有 open npm ecosystem 告警，并额外以本地 `pnpm audit` / `npm audit` 兜底当前 Node.js 依赖漏洞。影响范围包括：

- `web/` 管理端前端应用。
- `site/` Astro 文档站点。
- `packages/bifrost-sync-server/` 独立 Sync Server Node 包。
- `.agents/skills/e2e-verify/scripts/` 与 `.agents/skills/site-cookie-login/scripts/` 两个 Puppeteer 脚本目录。

## 实现逻辑

1. 使用 GitHub Dependabot Alerts API 获取 `state=open&ecosystem=npm` 告警清单，按 manifest 分组处理。
2. `web/` 升级直接依赖 `axios`、`react-router-dom`、`vite`、`@vitejs/plugin-react`、`vitest`、`@playwright/test` 等，并通过 `web/pnpm-workspace.yaml` 覆盖 `dompurify` 到安全版本，避免 `monaco-editor` 传递旧版本。
3. `site/` 刷新 Astro/Starlight 锁文件，并通过 `site/pnpm-workspace.yaml` 覆盖 `esbuild` 到安全版本。
4. `packages/bifrost-sync-server/` 升级 `js-yaml`，显式固定 `vite` 安全版本，并通过 `packages/bifrost-sync-server/pnpm-workspace.yaml` 覆盖 `esbuild` / `vite` 与声明 pnpm build-script 白名单。
5. 两个 `.agents/skills/*/scripts` 目录升级 `puppeteer`，刷新 `package-lock.json`；`e2e-verify` 同步刷新 `pnpm-lock.yaml`。

## 依赖项

- Node.js >= 22，与仓库 GitHub Actions 配置一致。
- pnpm 10.x/11.x，按各目录现有 lockfile 执行。
- npm 11.x，用于两个提交 `package-lock.json` 的脚本目录。

## 测试方案

### 单元测试

- `pnpm --dir web run test:unit`：覆盖管理端前端 store、API、组件工具函数。
- `pnpm --dir site run docs:test`：覆盖文档同步脚本。
- `pnpm --dir packages/bifrost-sync-server test`：覆盖 Sync Server TypeScript/Vitest 测试。

### E2E 测试

- `pnpm --dir web run test:ui`：Playwright 覆盖管理端核心页面与前端行为。
- 如 Playwright 依赖或浏览器缺失，先执行对应安装或记录环境阻塞后修复。

### 真实场景测试

- 新增 `human_tests/node-security-frontend.md`。
- 按用例真实执行安全 audit、前端 lint/build/unit/UI、site build、sync server build/test、两个 Puppeteer 脚本 npm audit。

### Coverage 90% 门禁

- 本次不修改 Rust 业务代码；仍按仓库门禁收尾运行 `make coverage`。
- 若 E2E coverage 环境因平台或本地工具缺失不可用，退化为 `make coverage-unit` 并记录原因。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 GitHub Dependabot npm 告警清单、所有修改的 manifest/lockfile、pnpm override 是否生效；运行 audit 与前端最小测试，修复残留漏洞或兼容问题。
- 第 2 轮：复查最新 diff、human_tests 索引、锁文件实际版本和测试输出；复跑受影响测试并确认无需追加轮次。
- 任一轮发现漏洞残留、前端构建失败、UI E2E 失败、Sync Server 兼容失败或文档缺口时，追加后续轮次。

## 校验要求

- `pnpm audit --audit-level=low`：`web/`、`site/`、`packages/bifrost-sync-server/` 均必须 0 漏洞。
- `npm audit --audit-level=low`：两个 `.agents/skills/*/scripts` 目录均必须 0 漏洞。
- `pnpm --dir web run lint`
- `pnpm --dir web run build`
- `pnpm --dir web run test:unit`
- `pnpm --dir web run test:ui`
- `pnpm --dir site run build`
- `pnpm --dir packages/bifrost-sync-server run build`
- `pnpm --dir packages/bifrost-sync-server test`
- `cargo test --workspace --all-features`
- `make coverage`
- 远端 GitHub Actions CI 全绿。

## 文档更新要求

- 更新本设计文档。
- 新增 `human_tests/node-security-frontend.md` 并同步 `human_tests/readme.md` Web UI 测试索引。
