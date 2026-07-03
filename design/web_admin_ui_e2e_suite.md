# Web Admin 完整 E2E UI 用例设计

## 背景

Bifrost Web Admin 早期 E2E 只覆盖少量 JSON 场景，浏览器侧几乎没有系统化用例。用户在 Traffic / Rules / Values / Scripts / Replay / Settings 上会遇到跨页面数据同步、push 收敛、代理请求闭环等复杂交互，缺少稳定 E2E 意味着回归风险高。

截至 2026-06-17，仓库已基于 **Playwright** 落地一整套 Web Admin UI E2E：

- 测试目录：`web/tests/ui/`，按页面拆分成多个 spec：
  - `traffic.spec.ts` / `traffic-push.spec.ts`
  - `admin-rules-values.spec.ts`
  - `admin-scripts.spec.ts`
  - `admin-replay.spec.ts`
  - `admin-settings.spec.ts`
  - `pending-auth-modal.spec.ts`
  - `rules-dynamic-island-global.spec.ts`
  - `notifications.spec.ts` / `breakpoint-ui.spec.ts` / `curl-parse.spec.ts` / `file-access-roots.spec.ts` / `im-gateway-provider.spec.ts` / `videos-tool.spec.ts` / `voice-wake-actions.spec.ts` / `asr-*.spec.ts` / `agent-*.spec.ts` / `ai-skill-assistant.spec.ts`
- 运行器：`web/playwright.config.ts` 通过 `tests/ui/helpers/test-env.ts` 的 `allocateUiTestEnv()` 自动分配 backend/web 端口、临时 `BIFROST_DATA_DIR`、运行目录与 PID 文件；前端由 Playwright `webServer` 拉起 `pnpm exec vite --host 127.0.0.1 --port <WEB_PORT>`。
- 入口脚本：`web/package.json` 暴露 `pnpm test:ui` / `pnpm test:ui:headed`。
- CI：`.github/workflows/ci.yml` 的 `e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell` 等 job 通过 `scripts/ci/run-e2e-shell.sh` 拉起 Playwright，缓存 `~/.cache/ms-playwright`。
- Admin 写入辅助：`web/tests/ui/helpers/admin-helpers.ts` 通过 `/_bifrost/api` 直接 CRUD；`traffic-generator.cjs` 生成 HTTP / SSE / WebSocket / 大 body 请求。
- Global setup / teardown：`web/tests/ui/global-setup.ts` / `global-teardown.ts`。

本文保留原始矩阵作为设计目标和补全清单，具体落地以上述 spec 为准。原文提到的 `.trae/skills/e2e-verify/scripts/scenarios/` 浏览器 JSON 场景路径已经下线，不再作为 ground truth。

## 用户目标验证清单

### 必须实现

- Traffic：代理产生的 HTTP / HTTPS / SSE / WebSocket 请求都能在 UI 列表出现，选中可看 request/response headers/body/状态。
- Rules：UI 创建/编辑/禁用/删除规则后，代理请求行为立即体现规则效果；Traffic 详情可见 matched rule。
- Values：UI CRUD Values 并能在 Rules 引用中生效；push 场景下第二页面写入能自动收敛到当前页。
- Scripts：request/response/decode script CRUD、执行后对代理流量生效、push 同步。
- Replay：Traffic → Replay 保存、group 分组、执行 replay 产生新 Traffic。
- Settings：Proxy / TLS / Performance / Access / Certificate / Pending Authorizations 的展示与写入。
- Push 同步专项：至少覆盖 “第二页面/API 写入 → 当前页面自动收敛” 一类场景。
- 每个场景使用独立临时 `BIFROST_DATA_DIR`、非 9900 端口、临时前端端口、mock upstream；结束必须清理数据。

### 必须不破坏

- 已有 Playwright 用例：`traffic.spec.ts` / `traffic-push.spec.ts` / `admin-rules-values.spec.ts` / `admin-scripts.spec.ts` / `admin-replay.spec.ts` / `admin-settings.spec.ts` / `pending-auth-modal.spec.ts`。
- `allocateUiTestEnv()` 分配端口与目录的语义。
- CI job（`e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell`）的现有配置。
- `admin-helpers.ts` / `traffic-generator.cjs` 的调用契约。

### 必须真实验证

- 三大平台 CI（Linux / macOS / Windows）均能跑通核心 spec。
- 本地 `pnpm test:ui` 可复现；失败 case 能通过 `pnpm test:ui:headed` 观察。
- 单 spec 可独立运行（不依赖其他 spec 的副作用）。

## 产品语义

### 测试分层

三层套件，避免把所有场景塞进一个巨大集合：

- **Core UI 套件**：本地和 CI 都稳定运行。不依赖改系统代理或装证书。使用 `127.0.0.1`、mock server、临时数据目录。覆盖 Traffic / Rules / Values / Scripts / Replay / Settings 只读项与安全配置项。
- **Host Integration 套件**：需要宿主机能力，可能修改系统代理、shell 配置、证书信任状态；默认**不进 CI**。覆盖 System Proxy toggle、CLI Proxy 状态/持久化、Certificate 下载/二维码/安装提示。
- **Push Sync 套件**：验证数据同步模型。使用双页面或 “页面 + 直接 API 写入” 联动。覆盖 Values / Scripts / Replay / Settings 访问控制与性能配置同步。

### 统一前置环境

所有 Core UI 场景由 `web/tests/ui/helpers/test-env.ts` 的 `allocateUiTestEnv()` 分配：

- 独立运行目录：`.bifrost-ui-test-runs/<run-id>/`
- 临时数据目录：`BIFROST_DATA_DIR=.bifrost-ui-test-runs/<run-id>/data`
- 临时代理端口：每次自动分配，不复用 9900
- 临时前端端口：Playwright `webServer` 自动分配
- Mock 上游：`e2e-tests/mock_servers/start_servers.sh`（或 `traffic-generator.cjs` 提供的内嵌 upstream）
- 环境变量：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` / `BIFROST_DISABLE_TRAY=1` / `--no-system-proxy` / `--host 127.0.0.1`

## 核心用例矩阵

下面矩阵按页面 + 场景 ID 组织，作为覆盖目标；具体实现在右侧 spec。落地状态见 [推荐场景拆分](#推荐场景拆分)。

### A. 启动与基础可用性

- UI-BOOT-001 启动链路可用：代理服务、前端 dev server、UI 首页可用；默认跳转 `Traffic`；侧边栏存在 `Traffic / Replay / Rules / Values / Scripts / Settings`；push 连接建立成功。→ 由 `playwright.config.ts` 的 `webServer.url` + 各 spec 的入口 `beforeAll` / `beforeEach` 覆盖。

### B. Traffic 页面

- UI-TRAFFIC-001 HTTP 流量采集与列表展示 → `traffic.spec.ts`
- UI-TRAFFIC-002 详情面板与 body 展示 → `traffic.spec.ts`
- UI-TRAFFIC-003 SSE / WebSocket 状态收敛 → `traffic-push.spec.ts`（含 `traffic-generator.cjs` SSE/WS 场景）
- UI-TRAFFIC-004 删除流量与详情清理 → `traffic.spec.ts`
- UI-TRAFFIC-005 搜索 / 过滤 / 清空 → `traffic.spec.ts`

### C. Rules 页面

- UI-RULES-001 创建规则并对代理流量生效 → `admin-rules-values.spec.ts`
- UI-RULES-002 编辑、禁用、删除规则 → `admin-rules-values.spec.ts`
- UI-RULES-003 规则引用 Values → `admin-rules-values.spec.ts`
- UI-RULES-TREE-01 Rules 列表按 `/` 分组树状展开/折叠 → `admin-rules-values.spec.ts`（约第 867 行）
- UI-RULES-KBD-02 上下键在可见叶子序列内切换 → `admin-rules-values.spec.ts`（约第 912 行）

### D. Values 页面

- UI-VALUES-001 Values CRUD → `admin-rules-values.spec.ts`
- UI-VALUES-002 Values 改动影响 Rules 生效 → `admin-rules-values.spec.ts`
- UI-VALUES-003 Values push 同步 → `admin-rules-values.spec.ts`

### E. Scripts 页面

- UI-SCRIPTS-001 Request Script CRUD → `admin-scripts.spec.ts`
- UI-SCRIPTS-002 Script 对代理请求生效 → `admin-scripts.spec.ts`
- UI-SCRIPTS-003 Response / Decode Script 生效 → `admin-scripts.spec.ts`
- UI-SCRIPTS-004 Scripts push 同步 → `admin-scripts.spec.ts`

### F. Replay 页面

- UI-REPLAY-001 保存请求到集合 → `admin-replay.spec.ts`
- UI-REPLAY-002 分组、移动、删除 → `admin-replay.spec.ts`
- UI-REPLAY-003 执行 replay → `admin-replay.spec.ts`

### G. Settings 页面

- UI-SETTINGS-001 Proxy 基础信息展示 → `admin-settings.spec.ts`
- UI-SETTINGS-002 TLS 配置编辑（含重连提示，参考 `web_admin_tls_whitelist_restart_notice.md`，约第 273 行断言重连文案） → `admin-settings.spec.ts`
- UI-SETTINGS-003 Performance 配置编辑 → `admin-settings.spec.ts`
- UI-SETTINGS-004 Access Control 配置 → `admin-settings.spec.ts`
- UI-SETTINGS-005 Pending Authorizations → `pending-auth-modal.spec.ts`
- UI-SETTINGS-006 Certificate 展示 → `admin-settings.spec.ts`
- UI-SETTINGS-007 System Proxy / CLI Proxy → Host Integration 套件，默认不进 CI（planned, not yet shipped as of 2026-06-17）

### H. Push 同步专项

- UI-PUSH-VALUES：`admin-rules-values.spec.ts` 覆盖 Values push
- UI-PUSH-SCRIPTS：`admin-scripts.spec.ts` 覆盖 Scripts push
- UI-PUSH-TRAFFIC：`traffic-push.spec.ts` 覆盖 Traffic 推送与 Intercept 按钮联动
- UI-PUSH-RULES-ISLAND：`rules-dynamic-island-global.spec.ts` 覆盖 Rules dynamic island 全局提示
- 双标签联动专项：planned, not yet shipped as of 2026-06-17

## 推荐场景拆分

落地状态（2026-06-17）：原计划的 JSON 场景文件已替换为 Playwright spec。下表保留作为设计意图，ground truth 是 `web/tests/ui/*.spec.ts`：

| 原始命名 (planned) | Playwright 实现 |
|---|---|
| `ui-bootstrap-smoke.json` | `playwright.config.ts` webServer + 各 spec 入口 |
| `ui-traffic-http-detail.json` | `traffic.spec.ts` |
| `ui-traffic-streaming-state.json` | `traffic-push.spec.ts` |
| `ui-traffic-search-delete.json` | `traffic.spec.ts` |
| `ui-rules-create-apply.json` | `admin-rules-values.spec.ts` |
| `ui-rules-edit-disable-delete.json` | `admin-rules-values.spec.ts` |
| `ui-values-crud-sync.json` | `admin-rules-values.spec.ts` |
| `ui-values-rules-integration.json` | `admin-rules-values.spec.ts` |
| `ui-scripts-crud.json` | `admin-scripts.spec.ts` |
| `ui-scripts-apply-request-response.json` | `admin-scripts.spec.ts` |
| `ui-replay-group-execute.json` | `admin-replay.spec.ts` |
| `replay-collection-sync.json` | `admin-replay.spec.ts` |
| `replay-execute-traffic.json` | `admin-replay.spec.ts` |
| `replay-history-filters.json` | `admin-replay.spec.ts` |
| `ui-settings-proxy-tls.json` | `admin-settings.spec.ts` |
| `ui-settings-performance-sync.json` | `admin-settings.spec.ts` |
| `ui-settings-access-control.json` | `admin-settings.spec.ts` |
| `ui-settings-pending-authorizations.json` | `pending-auth-modal.spec.ts` |
| `ui-settings-certificate-readonly.json` | `admin-settings.spec.ts` |
| `ui-settings-system-cli-proxy-host.json` | planned, not yet shipped as of 2026-06-17（Host Integration） |

以下辅助能力已被 Playwright helper 取代，不再以 shell/JS 单文件形式落地：

- 流量造数：`web/tests/ui/traffic-generator.cjs`（HTTP / SSE / WebSocket / 大 body）
- Admin 数据写入与断言：`web/tests/ui/helpers/admin-helpers.ts`（rules / values / scripts / replay / settings 通过 `/_bifrost/api` 直接 CRUD）
- 运行环境与端口分配：`web/tests/ui/helpers/test-env.ts` + `global-setup.ts` + `global-teardown.ts`

原计划的 `seed-traffic.sh` / `seed-admin-data.sh` / `assert-admin-state.js` 已确认不再实现。

## 推荐实现顺序

落地状态（2026-06-17）：P0/P1 中绝大多数项已通过 Playwright spec 落地；剩余少量主题（如 System / CLI proxy 切换的 Host Integration、双标签 push 同步专项）仍按 planned 看待。

### P0（已落地）

- `ui-bootstrap-smoke`
- `ui-traffic-http-detail`
- `ui-rules-create-apply`
- `ui-values-crud-sync`
- `ui-scripts-crud`
- `ui-settings-access-control`
- `ui-settings-performance-sync`

### P1（大部分已落地）

- `ui-traffic-streaming-state`
- `ui-traffic-search-delete`
- `ui-values-rules-integration`
- `ui-scripts-apply-request-response`
- `ui-replay-group-execute`
- `ui-settings-pending-authorizations`

### P2（部分 planned）

- `ui-settings-proxy-tls`（已落地）
- `ui-settings-certificate-readonly`（已落地）
- `ui-settings-system-cli-proxy-host`（planned, Host Integration，不进 CI）
- 双标签 push 同步专项（planned）

## 场景编写约束

为了让 UI 场景长期可维护，强制遵守：

- 不依赖中文文案做唯一定位，优先 `role + name`、`data-testid`、icon、稳定 selector、数据区域结构。
- 所有写操作都要有 cleanup（`admin-helpers.ts` 的 CRUD 提供 delete 辅助）。
- 每个场景必须显式记录：前置数据、触发动作、UI 断言、网络断言、清理动作。
- 流量验证同时看两层：upstream mock 收到的真实请求 + Admin UI 中展示的 request/response/status。
- Push 验证至少保留一类“第二页面/API 写入 → 当前页面自动收敛”的场景。
- 每个 spec 使用唯一前缀（例如 `uniqueName("tree-folder")`），避免多次运行相互干扰。

## Admin API 与 CLI

E2E 主要通过 Admin API 直接写入 / 断言状态，不新增 API/CLI 语义。常用端点：

- Rules：`GET/POST/PUT/DELETE /_bifrost/api/rules`、`PUT /_bifrost/api/rules/reorder`
- Values：`GET/POST/PUT/DELETE /_bifrost/api/values`
- Scripts：`GET/POST/PUT/DELETE /_bifrost/api/scripts`
- Replay：`GET/POST/PUT/DELETE /_bifrost/api/replay/{groups,requests}`、`POST /_bifrost/api/replay/execute`
- Settings：`GET/PUT /_bifrost/api/{proxy,tls,performance,whitelist,cli/proxy,certificate}`
- Pending Auth：`GET /_bifrost/api/whitelist/pending`、`POST /_bifrost/api/whitelist/{approve,reject,clear-all}`
- Traffic：`GET /_bifrost/api/traffic`、`DELETE /_bifrost/api/traffic/{id}`、`GET /_bifrost/api/traffic/{id}/{req,res}`

## Sync 边界

- E2E 场景默认关闭自动登录：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 单 spec 不触发 Sync 后端流量，除非明确测试 Sync 场景。
- 与 Sync 相关的登录回填 `auto_sync=true` 由 `bifrost-sync` 单元测试覆盖，不在 UI E2E 中重复。

## Phase 1：基础运行框架

- `web/tests/ui/helpers/test-env.ts`：`allocateUiTestEnv()` 分配端口、目录、PID 文件。
- `global-setup.ts` / `global-teardown.ts`：进程生命周期管理。
- `playwright.config.ts`：`webServer` 拉起 vite dev server。

## Phase 2：Core UI 用例

- 按页面拆分 spec：Traffic / Rules / Values / Scripts / Replay / Settings / Pending Auth。
- `admin-helpers.ts` 提供 CRUD 与断言。
- `traffic-generator.cjs` 提供 HTTP / SSE / WebSocket / 大 body 流量。

## Phase 3：Push Sync 专项

- 通过 `admin-helpers.ts` 后台写入 → 当前页面自动收敛。
- `traffic-push.spec.ts` 覆盖 Traffic 推送与 Intercept 按钮联动。
- `rules-dynamic-island-global.spec.ts` 覆盖 Rules dynamic island。

## Phase 4：Host Integration 与双标签联动

- （planned, not yet shipped as of 2026-06-17）：System Proxy / CLI Proxy toggle 的 Host Integration 场景，默认不进 CI，需在 macOS/Windows 有权限的机器上手工触发。
- （planned）双标签 push 同步专项：使用 Playwright 的 `context.newPage()` 打开两页，验证跨页面即时收敛。

## 测试方案

### 落地 spec 列表

- `web/tests/ui/traffic.spec.ts`
- `web/tests/ui/traffic-push.spec.ts`
- `web/tests/ui/admin-rules-values.spec.ts`（含 tree view / 键盘导航用例）
- `web/tests/ui/admin-scripts.spec.ts`
- `web/tests/ui/admin-replay.spec.ts`
- `web/tests/ui/admin-settings.spec.ts`（含 TLS 白名单重连提示用例）
- `web/tests/ui/pending-auth-modal.spec.ts`
- `web/tests/ui/rules-dynamic-island-global.spec.ts`
- `web/tests/ui/breakpoint-ui.spec.ts` / `curl-parse.spec.ts` / `file-access-roots.spec.ts` / `im-gateway-provider.spec.ts` / `notifications.spec.ts` / `videos-tool.spec.ts` / `voice-wake-actions.spec.ts` / `asr-*.spec.ts` / `agent-*.spec.ts` / `ai-skill-assistant.spec.ts`

### 环境约束

- 独立运行目录：`.bifrost-ui-test-runs/<run-id>/`
- 临时 `BIFROST_DATA_DIR`
- 临时代理端口、临时前端端口，禁止复用 9900
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` / `BIFROST_DISABLE_TRAY=1` / `--no-system-proxy` / `--host 127.0.0.1`

### 运行命令

- 本地：`pnpm -C web test:ui` / `pnpm -C web test:ui:headed`
- CI：`.github/workflows/ci.yml` 中 `e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell` job 通过 `scripts/ci/run-e2e-shell.sh` 拉起。
- 单 spec：`pnpm -C web test:ui -- traffic.spec.ts`。

### 覆盖率与项目校验

- `pnpm -C web lint`
- `pnpm -C web test:ui`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定：不跑 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 Core UI 套件覆盖：Traffic / Rules / Values / Scripts / Replay / Settings / Pending Auth 全部命中。
- 复核 spec 稳定性：`allocateUiTestEnv()` 端口冲突、`data-testid` 是否稳定、`admin-helpers.ts` cleanup 覆盖率。
- 重点 review：spec 之间是否有隐式依赖；push 场景是否会因为 timing 抖动 flaky；Host Integration 场景是否被误纳入 CI。
- 复测：`pnpm test:ui` 全量。

### 第 2 轮

- 复核第 1 轮发现问题的修复；`git status --short` 与 `git diff` 覆盖 web tests、helpers、config。
- 重点 review：双标签联动 planned 场景的可行性；剩余 Host Integration 用例的手工执行清单。
- 复测：CI 三平台 job（`e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell`）；本地 macOS 手测 System/CLI Proxy。

## 文档结论（2026-06-17 刷新）

- 完整 Web Admin UI E2E 已落地为 `web/tests/ui/` 下的 Playwright 套件，并接入 CI 的 `e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell` 等 job。
- 本文上面的矩阵作为覆盖目标对照表使用；具体场景命名、JSON 路径、辅助 shell 脚本均以 Playwright spec 与 `helpers/` 为准。
- 仍按 planned, not yet shipped as of 2026-06-17 看待的剩余项主要集中在：System / CLI Proxy 等 Host Integration 场景，以及双标签 push 同步专项。

## 风险与决策

- **Host Integration 不进 CI**：修改系统代理、装证书、shell 配置属于宿主机副作用，默认在 CI 关闭，避免污染 runner；由人工在 macOS / Windows 上手动跑。
- **Playwright vs 原 JSON 场景**：Playwright 提供更强的可维护性、更好的类型推断和更完整的 CI 集成，因此正式弃用 `.trae/skills/e2e-verify/scripts/scenarios/` 的 JSON 场景。
- **端口/目录冲突**：`allocateUiTestEnv()` 每次分配独立端口与目录，避免多个 spec 并行时冲突；不允许硬编码 9900。
- **flaky 风险**：SSE / WS / push 场景对时间敏感，应通过 explicit wait + 明确 network 断言收敛；不要依赖 `sleep`。
- **spec 增长**：随着功能扩展，spec 会持续增长；应按页面/主题拆分，避免出现单一超大 spec。
