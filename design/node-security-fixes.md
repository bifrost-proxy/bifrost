# Node.js Security Dependency Fixes

## 背景

Bifrost 主体是 Rust，但仓库内附带多个 Node.js 子项目——Web 控制台（`web/`）、文档站（`site/`）、独立 Sync Server（`packages/bifrost-sync-server/`）以及两个 Puppeteer skill 脚本目录（`.agents/skills/e2e-verify/scripts/`、`.agents/skills/site-cookie-login/scripts/`）。这些 Node 子项目会带入 GitHub Dependabot 中的 npm 漏洞告警。

本方案定义一个可反复执行的“Node 依赖安全刷新流程”：拉全 Dependabot open 告警，逐 manifest 升级或 override，本地二次跑 `pnpm audit` / `npm audit` 兜底，并把回归验证写进 human_tests。目标是让 Dependabot 队列在每次刷新后归零，并保证 Web 控制台、文档站、Sync Server、Puppeteer 脚本这四类真实用户路径继续可用。

## 用户目标验证清单

### 必须实现

- 所有 open 状态的 npm 生态 Dependabot 告警清零。
- `web/`、`site/`、`packages/bifrost-sync-server/` 三个 pnpm workspace 各自 `pnpm audit --audit-level=low` 为 0。
- `.agents/skills/e2e-verify/scripts/`、`.agents/skills/site-cookie-login/scripts/` 两个 npm 目录 `npm audit --audit-level=low` 为 0。
- pnpm workspace 通过 `overrides` 强制推平传递依赖（例如 `dompurify`、`esbuild`），避免 `monaco-editor` 或 Astro 的旧传递版本回落。
- 每次刷新完成后本地 lockfile（`pnpm-lock.yaml` / `package-lock.json`）与 CI GitHub Actions 保持一致。

### 必须不破坏

- Web 控制台的 `pnpm --dir web run lint`、`build`、`test:unit`、`test:ui` 全部通过。
- 文档站 `pnpm --dir site run build` 通过；如果 site 有 `docs:test`，也通过。
- Sync Server `pnpm --dir packages/bifrost-sync-server run build` 与 `test` 通过。
- 两个 Puppeteer skill 脚本 `node scripts/xxx.js --help` 或 skill 自带 smoke 步骤仍可执行。
- Rust 工作区 `cargo test --workspace --all-features` 不受 Node 升级影响；package.json 只影响前端管道。

### 必须真实验证

- 每次刷新在 human_tests/node-security-frontend.md 中留下真实执行日志：audit 输出、lint/build/test 状态。
- 远端 GitHub Actions release 与 CI workflow 全绿。
- 至少在一台本机（macOS 或 Linux）真实跑一次 Web UI 的 Playwright E2E。

## 产品语义

- 用户视角看不到 Node 升级本身，但看得到升级后的 Web UI、文档站、Sync Server 与 Puppeteer skill 继续工作。
- Bifrost 承诺 npm ecosystem 告警的响应窗口按仓库约定，不允许 open 告警长期堆积。
- 升级策略采取“直接依赖升级 + pnpm overrides 覆盖传递依赖”双路径，避免因单一 monaco-editor / astro / vite 传递依赖锁死告警。

## 技术细节

### 涉及 manifest

- `web/package.json` + `web/pnpm-workspace.yaml`：升级 `axios`、`react-router-dom`、`vite`、`@vitejs/plugin-react`、`vitest`、`@playwright/test`；`pnpm.overrides` 覆盖 `dompurify` 到已修复版本，压制来自 `monaco-editor` 的旧版本。
- `site/package.json` + `site/pnpm-workspace.yaml`：刷新 Astro / Starlight lockfile；`pnpm.overrides` 覆盖 `esbuild` 到修复版本。
- `packages/bifrost-sync-server/package.json` + `packages/bifrost-sync-server/pnpm-workspace.yaml`：升级 `js-yaml`；固定 `vite` 安全版本；覆盖 `esbuild` / `vite` 传递依赖；显式声明 `pnpm.onlyBuiltDependencies` 白名单避免任意 post-install 脚本。
- `.agents/skills/e2e-verify/scripts/package.json` + `package-lock.json` + `pnpm-lock.yaml`：升级 `puppeteer` 到已修复版本。
- `.agents/skills/site-cookie-login/scripts/package.json` + `package-lock.json`：同步升级 `puppeteer`。

### 刷新流程

1. `gh api /repos/bifrost-proxy/bifrost/dependabot/alerts?state=open&ecosystem=npm` 拉取告警清单，按 `manifest_path` 分组。
2. 对每个 manifest：
   - 优先直接升级顶层直接依赖到 Dependabot 建议的 fixed version。
   - 若漏洞来自传递依赖，使用 pnpm `overrides`（workspace 内根 `pnpm-workspace.yaml` 或 package `package.json`）钉住安全版本。
   - 对 npm `package-lock.json` 目录使用 `npm audit fix` + 手动覆盖异常。
3. `pnpm install` / `npm ci` 重生 lockfile，本地跑 `pnpm audit --audit-level=low` / `npm audit --audit-level=low` 兜底。
4. 跑 Web UI 与 Sync Server 的构建/测试。
5. 提交前对照 `git diff` 检查 lockfile 是否引入非预期的其它包版本变动。

### CLI + Web + Admin API

Node 升级本身不改 Bifrost CLI / Admin API / Web API 语义。但需要保持：

- Web UI 打包产物大小、路由、组件行为无回归——由 `pnpm --dir web run test:unit` + `test:ui` 覆盖。
- Sync Server 对外 API（登录、rule sync、group sync、health check）与升级前完全一致——由 `pnpm --dir packages/bifrost-sync-server test` 覆盖。

### Sync 边界

Sync Server 是 Node 侧的独立进程，升级 `js-yaml` / `vite` / `esbuild` 只影响其自身构建产物与运行时。Bifrost Rust 侧 Sync client 使用 HTTPS + REST 协议对接，不感知 Sync Server 依赖版本，因此本次升级不涉及 sync 协议改动。

## Phase 1-4

### Phase 1：告警清单与升级计划

- 拉 Dependabot open 告警清单。
- 按 manifest 分组，制定直接升级 vs override 计划。
- 记录每条告警的 fixed version 与影响面。

### Phase 2：Web 与 Sync Server 升级

- 升级 `web/`、`packages/bifrost-sync-server/` 的直接依赖。
- 更新 `pnpm-workspace.yaml` overrides。
- 本地跑 audit、lint、build、test。

### Phase 3：文档站与 Puppeteer skill 升级

- 升级 `site/` Astro / Starlight。
- 升级两个 `.agents/skills/*/scripts` puppeteer。
- 刷 `pnpm-lock.yaml` + `package-lock.json`。

### Phase 4：human_tests + CI 收尾

- `human_tests/node-security-frontend.md` 更新真实执行日志。
- `human_tests/readme.md` 同步 case 索引。
- GitHub Actions 全绿；如有 flaky 步骤记录并重跑。

## 测试方案

### 单元测试

- `pnpm --dir web run test:unit`：Web 控制台 store / API / 组件工具函数。
- `pnpm --dir packages/bifrost-sync-server test`：Sync Server Vitest 单测。
- site 有 `docs:test` 时执行；否则跳过并在 human_tests 中记录原因。

### E2E 测试

- `pnpm --dir web run test:ui`：Playwright 覆盖控制台核心页面（Rules、Traffic、Group、Sync、Admin API console）。
- Playwright 浏览器缺失时先安装或记录环境阻塞，再修复重跑。

### 真实场景测试

- 用例 ID：TC-NSF-01 到 TC-NSF-06，位于 `human_tests/node-security-frontend.md`。
  - TC-NSF-01：`pnpm audit --audit-level=low` 三个 pnpm workspace 全 0。
  - TC-NSF-02：`npm audit --audit-level=low` 两个 puppeteer skill 目录全 0。
  - TC-NSF-03：`pnpm --dir web run lint && build && test:unit`。
  - TC-NSF-04：`pnpm --dir web run test:ui`（Playwright）。
  - TC-NSF-05：`pnpm --dir site run build`。
  - TC-NSF-06：`pnpm --dir packages/bifrost-sync-server run build && test`。
- 附加 TC-NSF-07：两个 puppeteer skill 脚本 `npm audit` + smoke 执行。
- 全部用例必须使用真实终端，禁止跳过或引用旧输出。

### Coverage 门禁

- Rust 侧仍按仓库约定：`make coverage`；本机无法运行时退化为 `make coverage-unit` 并在 human_tests 记录原因。
- Node 侧 audit + build + test 视作 coverage 等价证据。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 Dependabot open 告警清单、每个 manifest 的 diff、pnpm overrides 是否命中传递依赖。
- 跑 audit + Web 最小测试。
- 修复残留漏洞或构建失败。

### 第 2 轮

- 复查最新 diff、lockfile 实际版本、human_tests 索引。
- 复跑受影响测试直到无追加漏洞。
- 若发现新漏洞、构建失败、UI E2E 挂、Sync Server 兼容问题，追加第 3 轮。

## 风险与决策

- **风险**：Astro / Starlight 大版本升级可能引入 Markdown 渲染差异；通过 site build + 抽样访问关键文档页面确认。
- **风险**：`monaco-editor` 更新后 dompurify 仍可能反弹回旧版本，必须靠 `pnpm.overrides` 强制固定。
- **风险**：Puppeteer 大版本会改变 Chromium 下载路径；skill 脚本需要在升级后重新 `npx puppeteer browsers install`，human_tests 步骤记录该操作。
- **决策**：不直接升级 Rust 侧任何依赖，避免与 Node 升级混淆；Node 升级 PR 独立提交、独立 review。
- **决策**：使用 pnpm workspace overrides 而不是 npm resolutions，保持仓库 pnpm-first 约定。
