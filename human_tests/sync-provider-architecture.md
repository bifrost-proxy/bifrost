# Sync Provider Architecture 真实场景测试

## 功能模块说明

本用例用于审查 `design/sync-provider-architecture.md` 是否完整覆盖 Sync Provider 方案。当前阶段是设计文档落地, 不是功能实现; 因此本用例通过静态审查命令和人工检查清单验证方案是否可继续评审和拆解。

## 前置条件

1. 当前工作区位于 Bifrost 仓库根目录。
2. 已存在 `design/sync-provider-architecture.md`。
3. 不启动 Bifrost 服务, 不修改系统代理, 不连接真实远端同步服务。

## 测试用例列表

### TC-SPA-01: 方案覆盖 provider/capability/lane 基础模型

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Provider Type|Provider Instance|Capability|Sync Lane|Capability Matrix" design/sync-provider-architecture.md
   ```
2. 阅读匹配段落。

**预期结果**:

- 文档明确区分 Provider Type 与 Provider Instance。
- 文档包含 capability 能力矩阵。
- 文档包含 `personal_rules`, `group_rules`, `remote_invoke` lane。
- 文档说明多个 provider instance 可以同时 enabled。

### TC-SPA-02: GitHub Gist 边界清晰

**操作步骤**:

1. 执行:
   ```bash
   rg -n "GitHub Gist|Rules Sync|Config Sync|encrypted|Group rules|Remote Invoke|secret gist" design/sync-provider-architecture.md
   ```
2. 阅读 GitHub Gist provider 章节和 capability matrix。

**预期结果**:

- GitHub Gist 支持个人规则同步和基础配置同步。
- GitHub Gist 不支持小组规则。
- GitHub Gist 不支持 Remote Invoke relay。
- 文档要求 GitHub Gist payload 默认客户端加密。
- 文档说明 secret gist 本身不是安全边界。

### TC-SPA-03: ByteDance 内网 provider 自动检测与不覆盖用户选择

**操作步骤**:

1. 执行:
   ```bash
   rg -n "ByteDance|auto|detect|bifrost.example.com|do not override|managed" design/sync-provider-architecture.md
   ```
2. 阅读 ByteDance Internal Provider 章节和 Product Defaults。

**预期结果**:

- 文档指定探测 `https://bifrost.example.com/v4/sso/check`。
- 文档指定 200/401 算可达, DNS/TLS/timeout/5xx 算不可用。
- 文档说明仅当用户尚未选择 provider 时自动推荐或创建 managed provider。
- 文档明确已选 GitHub 或其他 provider 时不能自动覆盖。

### TC-SPA-04: CLI/UI/API 具备可扩展操作面

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Admin API|bifrost sync provider|bifrost sync lane|Provider Catalog|Capability Routing|Implementation Phases" design/sync-provider-architecture.md
   ```
2. 阅读 Admin API, CLI, Web UI, Implementation Phases 章节。

**预期结果**:

- 文档包含 provider catalog/list/add/update/delete/auth/logout API。
- 文档包含 CLI provider 子命令和 lane 子命令。
- 文档包含 UI 的 Sync Targets, Capability Routing, Provider Catalog。
- 文档包含分阶段落地计划。

### TC-SPA-05: 三种产品同步服务独立连接与能力卡片

**操作步骤**:

1. 执行:
   ```bash
   rg -n "ByteDance Internal|Bifrost Cloud|GitHub Gist|Remote Invoke|Rules Sync|Config Sync|Connected services|independent|同时连接" design/sync-provider-architecture.md
   ```
2. 阅读 Product Provider Matrix 和 Web UI 章节。

**预期结果**:

- 文档明确 ByteDance Internal、Bifrost Cloud、GitHub Gist 三种产品同步服务都可独立连接。
- 文档明确三者互不冲突, 可同时处于已登录状态。
- Web UI provider 卡片展示 Remote Invoke、Rules Sync、Config Sync 三类能力。
- GitHub Gist 卡片显示 Rules Sync + Config Sync, Remote Invoke 不支持。

### TC-SPA-06: 首次无登录时弹出同步登录选择

**操作步骤**:

1. 执行:
   ```bash
   rg -n "First-run sign-in modal|no sync provider|zero provider sessions|local-only|ByteDance Internal|Bifrost Cloud|GitHub Gist" design/sync-provider-architecture.md
   ```
2. 阅读 First-run sign-in modal 和 Product Defaults 章节。

**预期结果**:

- 文档要求 Settings Sync 首次打开且没有任何 provider session 时弹出登录选择窗口。
- 登录窗口提供 ByteDance Internal、Bifrost Cloud、GitHub Gist 三种选择。
- 用户可关闭窗口继续本地使用, 不阻断本地代理能力。
- ByteDance 内网可达时只推荐/高亮, 不覆盖用户已选择的其它 provider。

### TC-SPA-07: Remote Invoke 只向合格 provider 注册且支持双注册

**操作步骤**:

1. 执行:
   ```bash
   rg -n "registration = all_eligible|Remote Invoke registration|byte-work|bifrost-cloud|both|get Remote Invoke registration|remote_invoke_relay" design/sync-provider-architecture.md
   ```
2. 阅读 Remote Invoke lane、Bifrost Cloud Provider 和 Web UI 章节。

**预期结果**:

- 文档要求 Remote Invoke 只使用带 `remote_invoke_relay` capability 的 provider。
- ByteDance Internal 和 Bifrost Cloud 都已连接时, 本机服务向两者都注册。
- GitHub Gist、WebDAV、文件型 provider 永不参与 Remote Invoke 注册。
- UI/CLI 需要展示每个合格 provider 的注册状态。

### TC-SPA-08: 基础配置同步范围受控

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Basic config|basic_config|app_allowlist|domain_allowlist|blacklist|does not sync|secrets|Remote Invoke grants" design/sync-provider-architecture.md
   ```
2. 阅读 Basic config lane 和 Product Defaults 中的 Basic config sync scope。

**预期结果**:

- 文档要求基础配置同步只覆盖应用白名单、域名白名单、黑名单/排除列表。
- 文档明确其它 Settings 配置暂不同步。
- 文档明确密码、证书、token、API key、Remote Invoke grant、本机端口和流量历史等敏感/本机状态不参与同步。

### TC-SPA-09: Web UI 设计方案覆盖页面结构和关键交互

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Information architecture|Sync Overview|First-run sign-in modal|Connected Services|Capability Routing|Add Sync Service|setup wizard|Status and error surfaces|Responsive and theme requirements|Test ids" design/sync-provider-architecture.md
   ```
2. 阅读 Web UI 章节。

**预期结果**:

- 文档包含 Settings Sync 的四段页面结构: Overview、Connected Services、Capability Routing、Add Sync Service。
- 文档定义首次无登录弹窗、ByteDance/Bifrost Cloud/GitHub Gist 三种卡片和 dismiss 行为。
- 文档定义三种 provider 卡片状态、能力 badge、Remote Invoke 注册状态和操作按钮。
- 文档定义 Bifrost Cloud、GitHub Gist、ByteDance 三条 setup wizard。
- 文档定义错误、冲突、部分 Remote Invoke 注册失败的展示方式。
- 文档定义响应式、暗色主题、长文本不重叠和稳定 test id 要求。

### TC-SPA-10: Web UI 验证方案覆盖 Playwright 和视觉检查

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Web UI validation|First-run modal|Connected services|Capability routing|partial registration|Bifrost Cloud setup|GitHub Gist setup|Basic config scope|Dark theme|Responsive layout|Visual/DOM checks" design/sync-provider-architecture.md
   ```
2. 阅读 Test Plan 的 Web UI validation 章节。

**预期结果**:

- 文档列出 first-run modal、三 provider 同时连接、capability routing、Remote Invoke 部分注册失败等 Playwright 用例。
- 文档列出 GitHub Gist 和 Bifrost Cloud setup wizard 验证点。
- 文档列出基础配置同步范围的 UI 验证点。
- 文档列出暗色主题、响应式布局、长 URL/gist id 不重叠、tooltip、polling 不覆盖 draft 的 DOM/视觉检查。

### TC-SPA-11: 同步管理页以三种 Provider 卡片作为主交互面

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Supported provider card grid|exactly three|ByteDance Internal|Bifrost Cloud|GitHub Gist|at most three cards|one column|stable order|settings-sync-provider-grid|no WebDAV|Supported provider card grid|Responsive layout" design/sync-provider-architecture.md
   ```
2. 阅读 Web UI 的 Supported provider card grid、Connected Services 和 Web UI validation 章节。

**预期结果**:

- 文档明确 V1 当前只展示 ByteDance Internal、Bifrost Cloud、GitHub Gist 三种同步 provider 卡片。
- 文档明确三张卡片即使未登录也常驻展示, 各自可独立登录、配置、断开和手动同步。
- 文档明确卡片顺序稳定: ByteDance Internal、Bifrost Cloud、GitHub Gist。
- 文档明确桌面/宽平板最多一排三个卡片, 中等宽度可换行为两列加一列, 窄屏降为一列。
- 文档明确 V1 默认 UI 不展示 WebDAV/custom placeholder/第四张空卡。
- 文档明确 Playwright/DOM 验证需要断言三卡数量、顺序、最多三列、窄屏一列和无重叠。

### TC-SPA-12: ByteDance Internal 和 Bifrost Cloud 配置同步服务端改造方案完整

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Server-Side Config Sync Changes|/Users/eden_studio/work/github/bifrost-server-v4|packages/bifrost-sync-server|/v4/config/sync|/v4/capabilities|bifrost_basic_configs|bifrost_server_basic_config|Config Sync must stay disabled|Coordinated release gate" design/sync-provider-architecture.md
   ```
2. 阅读 Server-Side Config Sync Changes、Implementation Phases 和 Test Plan 章节。

**预期结果**:

- 文档明确 ByteDance Internal 服务在 `/Users/eden_studio/work/github/bifrost-server-v4` 改造, 后续需在该目录完成开发并单独提交 PR。
- 文档明确 Bifrost Cloud 服务在当前仓库 `packages/bifrost-sync-server` 改造。
- 文档列出现有代码入口: ByteDance Internal 的 IDL/controller/service/model/db.sql, Bifrost Cloud 的 routes/types/dao/sql/index。
- 文档定义共享 `/v4/config/sync` 协议、`/v4/capabilities` 能力声明和 Basic Config 三个允许 key。
- 文档列出两边需要新增的表、DAO/model、route/controller/service、测试和兼容策略。
- 文档明确如果服务端未返回 `config_sync`, UI/CLI 必须禁用或标记不可用, 不能假设支持。
- 文档明确最终推进需要 Bifrost client/admin、Bifrost Cloud、ByteDance Internal 三个 PR 一起完成。

### TC-SPA-13: 首次无登录弹窗可关闭且 Start 后才进入所选 Provider 登录流程

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Modal interaction state machine|Continue local only|Start|Provider card click|ByteDance Internal|Bifrost Cloud|GitHub Gist|failed auth|Successful login|First-run modal start flow|closing/dismissing the modal never calls provider auth" design/sync-provider-architecture.md
   ```
2. 阅读 Web UI 的 First-run sign-in modal 和 Web UI validation 章节。

**预期结果**:

- 文档明确没有任何同步服务登录时, 打开 Settings Sync 会弹出同步服务选择窗口。
- 文档明确弹窗可以通过 close icon 或 `Continue local only` 关闭, 关闭后停留在 Settings Sync, 不阻断本地代理能力。
- 文档明确关闭弹窗不会触发任何 provider 登录、auth/start 或 setup API。
- 文档明确用户点击 provider card 后只选中一个 provider, 并通过 `Start` 进入后续流程。
- 文档明确 ByteDance Internal 的 `Start` 进入内部 SSO/login prompt, Bifrost Cloud 的 `Start` 进入 URL/capability/login wizard, GitHub Gist 的 `Start` 进入 GitHub gist auth/setup。
- 文档明确失败或取消登录会回到弹窗/选中 provider 状态, 不把 provider 标记为 connected。
- 文档明确 Web UI validation 有对应 Playwright 用例覆盖 dismiss 和 start flow。

### TC-SPA-14: GitHub Gist token 失效时卡片提示重新连接

**操作步骤**:

1. 执行:
   ```bash
   rg -n "GitHub Gist token expiration|GitHub Gist token expired|Reconnect required|New Token|last_error|authorized=false" design/sync-provider-architecture.md
   ```
2. 启动带有失效 `github_gist` session 的 Bifrost 管理端, 或使用单元/E2E 注入同等 `/api/sync/status` 响应。
3. 打开 Settings Sync 页面, 查看 GitHub Gist provider 卡片。

**预期结果**:

- `/api/sync/status` 中 GitHub Gist provider 保留 `connected=true`, 但返回 `authorized=false`, `reason=error`, `last_error` 包含 GitHub token 失效或 gist scope 错误。
- GitHub Gist 卡片不再显示健康 `Connected`; 状态标签显示 `Reconnect required`。
- 卡片内显示 inline error, 文案包含后端 GitHub token 错误。
- 卡片仍显示 `New Token`/token 生成入口、`Reconnect` 和 `Sign Out`, 方便用户换 token 或退出。

### TC-SPA-15: 已连接 provider 不被其他未连接 provider 阻塞

**操作步骤**:

1. 构造 `/api/sync/status`: 至少一个 provider 返回 `connected=true`, `authorized=true`; 另一个 provider 返回 `connected=false` 或 `reason=unreachable/error`。
2. 打开 Settings Sync 页面。
3. 查看顶部全局提示、底部 Sync 状态和各 provider 卡片。

**预期结果**:

- Settings Sync 不显示 `Remote sync is a pluggable capability` 全局提示。
- 底部 Sync 状态不显示 `Local` / `Remote service unreachable, using local rules only`。
- 已连接 provider 卡片仍显示 `Connected` 或 `Synced`, 登录/重连按钮可用。
- 未连接或失败 provider 只在自己的卡片上显示 `Offline`、`Not connected` 或 card-level error, 不影响其他 provider 同步。

### TC-SPA-16: 每个 provider 卡片展示最近成功同步时间

**操作步骤**:

1. 构造 `/api/sync/status`: provider 列表中给不同 provider 设置各自的 `last_sync_at` 和 `last_sync_action`。
2. 打开 Settings Sync 页面。
3. 查看每个 provider 卡片的 `Last sync` 行。

**预期结果**:

- 每个 provider 卡片显示自己的最近成功同步时间和 action。
- 没有成功同步记录的 provider 显示 `Never`。
- 一个 provider 的同步时间不会覆盖到另一个 provider 卡片。

### TC-SPA-17: Bifrost Cloud 无默认 URL 且连接前校验

**操作步骤**:

1. 打开 Settings Sync 页面, 保持 Bifrost Cloud 未配置、未连接。
2. 查看 Bifrost Cloud URL 输入框。
3. 不填写 URL 直接点击 Bifrost Cloud `Sign In`。
4. 填写 `http://custom-sync.example.test`, 再点击 `Sign In`。
5. 填写合法 `https://custom-sync.example.test`, 点击 `Save` 或 `Sign In`。

**预期结果**:

- Bifrost Cloud URL 输入框为空, 只显示示例 placeholder, 不预填官方或默认云服务地址。
- 空 URL 会提示输入自部署 Bifrost Sync Server URL, 且不会调用 `/api/sync/config` 或 `/api/sync/login`。
- 非 HTTPS URL 会提示必须使用 `https://`, 且不会调用 `/api/sync/config` 或 `/api/sync/login`。
- 合法 HTTPS URL 通过基础校验后才保存并进入登录流程。

### TC-SPA-18: Provider 登录/退出状态互不影响

**操作步骤**:

1. 构造 `/api/sync/status`: ByteDance Internal、Bifrost Cloud、GitHub Gist 三个 provider 都返回 `connected=true`。
2. 打开 Settings Sync 页面。
3. 点击 Bifrost Cloud 卡片的 `Sign Out`。
4. 观察实际请求和三张卡片状态。

**预期结果**:

- UI 调用 `/api/sync/providers/bifrost_cloud/logout`, 不调用全局 `/api/sync/logout`。
- Bifrost Cloud 卡片变为未连接或需要登录。
- ByteDance Internal 和 GitHub Gist 卡片仍保持 `Connected`, 账号信息和 `Last sync` 不被清空。
- 任意单张 provider 卡片的登录、重新连接、退出登录都只影响自身 session。

### TC-SPA-19: GitHub Gist 只因语义内容变化才写入

**操作步骤**:

1. 准备 GitHub Gist snapshot: 只改变规则/基础配置的 `create_time`、`update_time`、对象顺序、JSON key 顺序或 allowlist 数组顺序, 保持规则内容和基础配置有效值不变。
2. 触发 GitHub Gist sync 或执行对应单元测试。
3. 观察 sync action 和是否调用 Gist save/PATCH。

**预期结果**:

- 只发生 metadata/order 变化时, GitHub Gist sync action 为 `no_change`。
- 不调用 Gist save/PATCH, 不产生新的 Gist revision。
- 规则比较只关注规则的实际内容 hash。
- 基础配置比较只关注 canonical JSON 内容 hash; `app_allowlist`、`domain_allowlist`、`blacklist` 的数组顺序不影响 hash。

### TC-SPA-20: GitHub Gist 自动同步有冷却和失败退避

**操作步骤**:

1. 在本机确认 `9900` 端口 Bifrost 服务运行:
   ```bash
   lsof -nP -iTCP:9900 -sTCP:LISTEN
   ```
2. 查询 GitHub API rate limit:
   ```bash
   curl -sS -D - https://api.github.com/rate_limit -o /tmp/bifrost-github-rate-limit.json
   ```
3. 连续触发后台 sync tick 或执行 GitHub Gist auto sync cooldown 单元测试。
4. 查看 GitHub Gist provider 状态和日志。

**预期结果**:

- 现场 rate limit 若为 `remaining: 0`, UI 可以显示 provider 级错误, 但不能让其它 provider 变为不可用。
- 后台 GitHub Gist sync 不跟随 `probe_interval_secs=5` 每 5 秒请求 GitHub。
- 成功或 no-change 后有 provider 级冷却; 失败后使用更长退避。
- 手动 `Reconnect` 或新 token 登录仍可立即尝试, 不被旧冷却永久阻塞。

### TC-SPA-21: Last change 与 Last check 分离且服务端 provider no-change 不高频远端写入

**操作步骤**:

1. 在本机确认 `9900` 端口 Bifrost 服务运行:
   ```bash
   lsof -nP -iTCP:9900 -sTCP:LISTEN
   ```
2. 查询 `/api/sync/status`, 记录 ByteDance Internal 或 Bifrost Cloud provider 的 `last_sync_at`、`last_sync_action`、`last_changed_sync_at`、`last_changed_sync_action` 和 `check_interval_secs`。
3. 在不修改规则和基础配置的情况下, 等待短于 `check_interval_secs` 的时间后再次查询 `/api/sync/status`。
4. 打开 Settings Sync 页面, 查看对应 provider 卡片的 `Last change`、`Last check`、`Check interval`。
5. 触发一次本地规则或基础配置真实变化后, 再查询状态。

**预期结果**:

- `Last change` 只表示最近一次真实内容推送或拉取, `no_change` 检查不会推进该时间。
- `Last check` 表示最近一次成功检查/同步尝试, 可以显示 `no change`。
- `Check interval` 展示 provider 后台自动检查频率, 默认服务端 provider 为 `Every 5 min`。
- 未发生规则或基础配置内容变化时, 后台 tick 不会每 5 秒向服务端重复执行完整同步检查或写入。
- 本地真实变化触发的 wakeup 可绕过冷却, 及时执行一次完整同步并推进 `Last change`。

## 清理步骤

本用例以静态审查、mock UI 和本机 9900 只读诊断为主, 不修改系统代理和真实远端数据。若手动创建了临时 GitHub Gist 或测试 token, 需要删除对应 Gist 并撤销 token。

## 执行记录

- 2026-07-06: PASS - 在 `codex/sync-provider-design` worktree 中执行 TC-SPA-01 至 TC-SPA-04 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认设计覆盖多 provider 同启、capability 区分、GitHub Gist rules-only、ByteDance 自动检测、UI/CLI/API 和分阶段落地。
- 2026-07-07: PASS - 执行 TC-SPA-05 至 TC-SPA-08 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认设计新增三种产品同步服务独立连接、首次无登录弹窗、Remote Invoke 双 provider 注册、基础配置同步范围与 CLI/UI 管理入口。
- 2026-07-07: PASS - 执行 TC-SPA-09 至 TC-SPA-10 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认 Web UI 设计方案覆盖页面结构、关键交互、错误/冲突/注册状态、响应式和测试钩子, Web UI 验证方案覆盖 Playwright、暗色主题和不重叠检查。
- 2026-07-07: PASS - 执行 TC-SPA-11 的 `rg` 静态审查命令, 命中三 provider 卡片网格、稳定顺序、最多三列、窄屏一列、V1 不展示 WebDAV/custom placeholder 和 Playwright/DOM 断言要求。
- 2026-07-07: PASS - 执行 TC-SPA-12 的 `rg` 静态审查命令, 命中 ByteDance Internal 与 Bifrost Cloud 两个服务端路径、现有代码入口、`/v4/config/sync`、`/v4/capabilities`、新增表、capability gating 和三 PR 协同发布要求。
- 2026-07-07: PASS - 执行 TC-SPA-13 的 `rg` 静态审查命令, 命中首次无登录弹窗状态机、关闭不触发登录、选择 provider 后点击 Start 才进入 ByteDance/Bifrost Cloud/GitHub Gist 对应登录或 setup 流程、失败/取消回到弹窗状态和 Playwright 覆盖要求。
- 2026-07-07: PASS - 执行 TC-SPA-14 的 `rg` 静态审查命令, 命中 GitHub Gist token 失效卡片级提示、`Reconnect required`、`New Token`、provider `authorized=false` 和 `last_error` 要求。实现阶段执行 `cargo test -p bifrost-sync status_marks_github_gist_session_unauthorized_after_token_error`、`pnpm --dir web test:unit src/pages/Settings/tabs/SyncTab.test.ts`、`e2e-tests/tests/test_sync_github_gist_expired_status_e2e.sh`、`BIFROST_UI_TEST_TARGET_DIR=/Users/eden/work/github/bifrost/target pnpm --dir web test:ui web/tests/ui/admin-settings.spec.ts -g "GitHub Gist token 失效"` 均通过, 确认失效 Gist session 在 API 中保留 `connected=true` 以显示 Reconnect/Sign Out, 同时返回 `authorized=false`, `reason=error`, `last_error`, UI 卡片显示 `Reconnect required` 和 inline GitHub token 错误。
- 2026-07-07: PASS - 执行 TC-SPA-15/TC-SPA-16 的实现验证: `pnpm --dir web test:unit src/pages/Settings/tabs/SyncTab.test.ts` 通过 4/4, 覆盖有任一 provider connected 时隐藏全局 pluggable-sync 提示; `pnpm --dir web run build` 通过, 覆盖 provider `last_sync_at` / `last_sync_action` 类型接入与 Settings/StatusBar 编译; `cargo test -p bifrost-sync` 通过 142/142, 覆盖 provider 级同步状态序列化和既有 sync_once 行为不回退。
- 2026-07-07: PASS - 执行 TC-SPA-17 的实现验证: 后端新增 `status_leaves_unconfigured_bifrost_cloud_url_empty`; UI 新增 `Settings Sync Bifrost Cloud URL 必须先通过基础校验再连接`, 覆盖 Bifrost Cloud 未配置时输入框为空、空 URL/HTTP URL 不调用 config/login、HTTPS URL 才进入保存/登录。
- 2026-07-07: PASS - 执行 TC-SPA-18 的实现验证: 后端新增 provider-scoped `/api/sync/providers/{provider_id}/logout`, `cargo test -p bifrost-sync logout_provider` 通过; Web UI logout 改为卡片级 provider id 调用, `pnpm --dir web test:unit src/pages/Settings/tabs/SyncTab.test.ts` 通过; Playwright mock 用例覆盖点击 Bifrost Cloud `Sign Out` 只请求 `/sync/providers/bifrost_cloud/logout`, ByteDance Internal 和 GitHub Gist 仍保持 connected。
- 2026-07-07: PASS - 执行 TC-SPA-19 的实现验证: `cargo test -p bifrost-sync` 通过 152/152, 覆盖 `github_gist_rule_sync_ignores_remote_metadata_only_changes`, `github_gist_basic_config_sync_ignores_json_array_order`, `encode_snapshot_content_sorts_rules_and_basic_configs_stably`; 人工复核确认规则比较忽略 provider metadata/order, 基础配置使用 canonical JSON hash, Gist snapshot 输出稳定排序。
- 2026-07-07: PASS - 执行 TC-SPA-20 的现场诊断和实现验证: `lsof -nP -iTCP:9900 -sTCP:LISTEN` 确认 PID 59612 的 `bifrost` 正在监听 9900; `curl https://api.github.com/rate_limit` 返回 `x-ratelimit-limit: 60`, `x-ratelimit-remaining: 0`, reset 为 `2026-07-07 17:43:43 CST`; `/api/sync/status` 显示 ByteDance Internal 仍 `authorized=true` 且 last sync `no_change`, GitHub Gist 独立显示 `authorized=false` 和 token/gist scope 错误; 新增 `github_gist_auto_sync_cooldown_skips_until_elapsed` 和 `github_gist_auto_sync_error_uses_longer_backoff` 单元测试并通过。
- 2026-07-07: PASS - 执行 TC-SPA-21 的实现验证: 新增 provider `last_changed_sync_at` / `last_changed_sync_action` / `check_interval_secs`, Web UI 卡片展示 `Last change`、`Last check`、`Check interval`; 服务端 provider 后台完整同步检查成功后进入 5 分钟冷却, 本地规则/配置变更 wakeup 和手动同步可绕过冷却; 基础配置同步 action 与规则同步 action 合并, metadata-only/no-change 不推进 `Last change`。
- 2026-07-07: PASS - 实现阶段在主仓库执行 `cargo check -p bifrost-sync -p bifrost-admin -p bifrost-cli`, `pnpm --dir packages/bifrost-sync-server lint`, `pnpm --dir packages/bifrost-sync-server test`, `pnpm --dir web run build`, `pnpm --dir web test:ui web/tests/ui/admin-settings.spec.ts -g "Provider 卡片"`; 均通过。人工复核确认 Settings Sync 三 provider 卡片、首登弹窗可关闭、Start 触发 ByteDance/Bifrost Cloud 既有登录链路、Bifrost Cloud 配置同步服务端校验和存储测试均覆盖。
- 2026-07-07: PASS - Review/Fix/Test 第 1 轮执行 `cargo test -p bifrost-sync` 发现旧 mock/旧服务未实现 `/v4/config/sync` 时规则同步会被配置同步拖失败; 已修复为旧服务端点缺失或旧格式空 data 时跳过基础配置同步、不影响规则同步。复跑 `cargo test -p bifrost-sync` 通过 121/121, 复跑 `cargo test -p bifrost-cli sync_cmd` 通过。
- 2026-07-07: PASS - Review/Fix/Test 第 2 轮执行 `git diff --check` 覆盖主仓库和 `/Users/eden_studio/work/github/bifrost-server-v4`, 均无 whitespace error; 静态检索确认无残留 `block_on`, Provider 卡片与 `basic_configs` 状态路径均在预期文件中。
- 2026-07-07: PASS - 收尾验证执行 `cargo fmt --all -- --check`, `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build --all-targets --all-features`, 均通过。
- 2026-07-07: PASS - 首次 `cargo test --workspace --all-features` 在 `bifrost-e2e` 固定端口 `127.0.0.1:15093` 被临时占用时失败; 确认端口释放后复跑失败用例 `cargo test -p bifrost-e2e tests::tls_intercept_mode::tests::test_intercept_with_modification -- --exact` 通过, 随后执行 `cargo test --workspace --all-features -- --test-threads=1` 全量通过。
- 2026-07-07: PASS - Web UI 收尾验证执行 `pnpm --dir web test:ui web/tests/ui/admin-settings.spec.ts -g "Settings Sync"`, `pnpm --dir web test:ui web/tests/ui/admin-settings.spec.ts -g "Certificate iOS Configurator"`, `pnpm --dir web test:ui web/tests/ui/admin-settings.spec.ts`, `pnpm --dir web run build`; Sync 6/6、Certificate 1/1、Settings 全文件 34/34 均通过, Web build 通过。期间修复 Settings Sync 旧断言因三张 Provider 卡片导致 `Not signed in` 多实例冲突, 并修复 Certificate 用例被真实 `mobile_devices` push 覆盖 mock 的测试夹具问题。
- 2026-07-07: BLOCKED - ByteDance Internal 仓库 `/Users/eden_studio/work/github/bifrost-server-v4` 已补 `SyncBasicConfig` 实现和脚本测试, 但本机 `pnpm install` 在内部 bnpm registry 下载依赖时因旧 pnpm/Node 下载链路报 `ERR_INVALID_THIS` 阻塞, 未能执行 `pnpm run build`, `pnpm run test:env-sync-optimization`, `pnpm run test:basic-config-sync`。该阻塞需要在可用内部 Node/pnpm/bnpm 环境中复跑。
- 2026-07-07: PASS - CI run `28852425930` 的 coverage gate 发现 `bifrost-sync` 为 `89.91% < 90.00%`; 补充 provider/GitHub Gist error isolation、provider sync meta success/error 和 provider session user update 的单元覆盖。执行 `cargo fmt --all -- --check`, `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-sync provider --lib`, `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-sync --lib` 均通过, 其中 `bifrost-sync` lib tests 为 145/145 PASS。
- 2026-07-07: PASS - CI run `28860976222` 的 coverage gate 发现 `bifrost-sync` 为 `89.58% < 90.00%`; 针对 TC-SPA-21 的 Last change/Last check 拆分逻辑补充 `sync_action_helpers_track_push_pull_and_change_state`、`provider_sync_success_updates_changed_timestamp_only_for_changes`、`status_aggregates_latest_changed_sync_metadata_from_connected_providers`、`server_config_sync_action_combines_with_existing_rule_action` 单元覆盖。执行 `cargo fmt --all -- --check`, `git diff --check`, `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-sync --lib` 均通过, 其中 `bifrost-sync` lib tests 为 158/158 PASS。
