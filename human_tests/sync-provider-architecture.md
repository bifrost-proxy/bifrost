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
   rg -n "ByteDance|auto|detect|bifrost.bytedance.net|do not override|managed" design/sync-provider-architecture.md
   ```
2. 阅读 ByteDance Internal Provider 章节和 Product Defaults。

**预期结果**:

- 文档指定探测 `https://bifrost.bytedance.net/v4/sso/check`。
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

## 清理步骤

本用例只读文档, 无需清理临时服务或数据目录。

## 执行记录

- 2026-07-06: PASS - 在 `codex/sync-provider-design` worktree 中执行 TC-SPA-01 至 TC-SPA-04 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认设计覆盖多 provider 同启、capability 区分、GitHub Gist rules-only、ByteDance 自动检测、UI/CLI/API 和分阶段落地。
- 2026-07-07: PASS - 执行 TC-SPA-05 至 TC-SPA-08 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认设计新增三种产品同步服务独立连接、首次无登录弹窗、Remote Invoke 双 provider 注册、基础配置同步范围与 CLI/UI 管理入口。
- 2026-07-07: PASS - 执行 TC-SPA-09 至 TC-SPA-10 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认 Web UI 设计方案覆盖页面结构、关键交互、错误/冲突/注册状态、响应式和测试钩子, Web UI 验证方案覆盖 Playwright、暗色主题和不重叠检查。
- 2026-07-07: PASS - 执行 TC-SPA-11 的 `rg` 静态审查命令, 命中三 provider 卡片网格、稳定顺序、最多三列、窄屏一列、V1 不展示 WebDAV/custom placeholder 和 Playwright/DOM 断言要求。
- 2026-07-07: PASS - 执行 TC-SPA-12 的 `rg` 静态审查命令, 命中 ByteDance Internal 与 Bifrost Cloud 两个服务端路径、现有代码入口、`/v4/config/sync`、`/v4/capabilities`、新增表、capability gating 和三 PR 协同发布要求。
