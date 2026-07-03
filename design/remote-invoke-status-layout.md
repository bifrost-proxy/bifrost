# Remote Invoke 状态区布局优化

> 实施状态（2026-04-28 首版，2026-06-22 / 2026-07-03 复核）：`Remote Status` 卡片已把 `Connection Status` 与 `Discovery Mode` 合并到左列同一张卡片，`SSH Key` 卡片保持右列独立，`Active Calls` 已经从状态卡片移除，`Grants` 列表按 `auth_method` 展示 `SSH key` / `Pair code` 标签。所有 test-id 与 admin-settings.spec.ts 的断言均已合入 `main`。

## 背景

`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx` 承担 Remote Invoke 的所有设置入口。旧布局把 `Connection Status`、`Discovery Mode`、`SSH Key` 拆成三张独立卡片。桌面宽屏（`md` 以上）下：

- 左下会出现明显空白区，`SSH Key` 卡片高度大于左列两张卡片之和，视觉断层严重；
- `Connection Status` 同时展示 relay 状态、Sync Account 与 “Active Calls” 计数，而底部的 `Recent Calls` 表格已经带 caller / 状态 / 时间等更完整信息，导致同一个数据两处出现，扫读时容易割裂；
- 未登录 Sync 时的登录提示使用硬编码浅黄色背景，暗色主题下明显跳色，无障碍表现差；
- `Grants` 列表只显示授权本身，不区分是通过 `SSH key` 还是 `Pair code` 连接进来的，运维排查抓不到连接类型。

## 用户目标验证清单

### 必须实现

- 左列合并为一张 `Remote Status` 卡片，纵向分为 `Connection` 与 `Discovery` 两段。
- 右列 `SSH Key` 卡片保持独立，通过 `height: 100%` 与左列同高。
- `Remote Status` 卡片移除 `Active Calls` 计数，避免与 `Recent Calls` 重复。
- 未登录 Sync 时的登录提示背景/边框/文字色使用 Ant Design 主题 token（`colorWarningBg` / `colorWarningBorder` / `colorWarningText` / `colorWarning`），暗色主题自适应。
- `Grants` 列表读取 grant `auth_method` 字段，展示 `SSH key` / `Pair code`；缺失时按 `ssh_key_fingerprint`/`ssh_key_id` 兜底。
- 卡片与区块暴露稳定 `data-testid`，Playwright 可锚定。

### 必须不破坏

- Remote Invoke Tab 切换、发现模式操作、SSH Key 创建/轮换/撤销、file access 策略保存、Grants revoke 全部功能不变。
- Recent Calls 表格中 caller 信息（`by <caller>` 与详情弹窗中的 `Caller` 字段）保持不变。
- 深色/浅色主题下的整体信息层级与颜色对比度保持一致。

### 必须真实验证

- Web UI E2E 覆盖合并卡片、SSH 卡片、`Enter Discovery Mode` 按钮、Grants 双标签、未登录 + 暗色主题登录提示。
- 真人回归覆盖宽屏下无空白块、窄屏下能垂直滚动、暗色主题登录提示可读。

## 产品语义

`Remote Status` 是 Remote Invoke 面板的核心状态入口，用户在这里判断本机 target 是否连上 relay、发现模式是否开启，以及是否需要登录 Sync；`Recent Calls` 是运行时观测入口；`Grants` 是授权管理入口。三段职责必须清晰不重叠：

- 状态卡片只放 relay / instance / device / sync 等静态状态；
- 正在执行 / 已执行调用统一由 `Recent Calls` 承接；
- 授权连接方式由 `Grants` 直接展示。

## 技术细节

### 布局重组

- 外层保留 `Row gutter=16`（`RemoteInvokeTab.tsx:1841` 附近）。
- 左列：单张 `Card` `data-testid="settings-remote-invoke-status-card"`：
  - 上半区 `data-testid="settings-remote-invoke-connection-section"`，渲染 `Connection Status`；
  - 下半区 `data-testid="settings-remote-invoke-discovery-section"`，渲染 `Discovery Mode` 与 `Enter Discovery Mode` 按钮、`pair_code` 展示。
- 右列：`Card` `data-testid="settings-remote-invoke-ssh-card"`（`RemoteInvokeTab.tsx:2188`），保留 SSH Key 创建/轮换/撤销/文件访问策略等区块。
- 两列容器与卡片均设置 `height: 100%`，实现 `md` 及以上宽度等高。

### 未登录 Sync 提示

- 判定 `syncReady = !!syncStatus?.has_session`（`RemoteInvokeTab.tsx:844`）。
- 未登录时展示锁定提示块（`RemoteInvokeTab.tsx:1873-1899`），样式使用：
  - `background: token.colorWarningBg`
  - `border: 1px solid token.colorWarningBorder`
  - 图标色 `token.colorWarning`
  - 文案色 `token.colorWarningText`
- Ant Design theme algorithm 切换到 dark 时自动映射到暗色告警配色，不再出现硬编码浅黄块。

### Grants 连接方式标签

- 读取 grant 对象上的 `auth_method`（`RemoteInvokeTab.tsx:712`）：
  - `ssh_publickey` → `SSH key`（tooltip: `Connected with SSH key`）；
  - `pair_code` / `null` / `''` → `Pair code`；
  - 其它未知值原样展示，tooltip `auth_method: ${method}`；
  - fallback：`method` 缺失但存在 `ssh_key_fingerprint` 或 `ssh_key_id` 时按 `SSH key` 兜底。
- `Grants` 卡片挂 `data-testid="settings-remote-invoke-grants-card"`（`RemoteInvokeTab.tsx:2646`）。

### Recent Calls

- 保留 caller `by <caller>` 展示与详情弹窗中的 `Caller` 字段（`RemoteInvokeTab.tsx:2805` 附近）。
- 不再在 `Remote Status` 中重复展示 `Active Calls` 计数。

### `Create SSH key` 弹窗文案

将两段独立提示合并为一条集中提示：

- 同一时间只支持一个 active SSH key；
- SSH grant 不受时间型 TTL 控制，只有 rotate 或 revoke 才会失效。

## CLI

本设计文档不引入 CLI 命令改动。相关能力（`bifrost remote *`）继续沿用现有语义。

## Admin API

- 复用现有 `/api/sync/status`（`has_session`）、`/api/remote-invoke/status`、`/api/remote-invoke/grants` 等端点，不新增字段。
- `auth_method` 字段在 grant API 已有实现（见 `packages/bifrost-sync-server/src/remote-invoke/service.ts`），前端仅消费。

## Sync 边界

- 状态卡片的 `has_session` 由 sync client 提供，仅影响 UI 可用性；不写回 sync 服务。
- Grants 列表基于本地缓存 + relay 事件驱动，不通过 sync 广播。
- 该 UI 改动不影响 sync 数据结构或迁移策略。

## Phase 拆分

- Phase 1：布局合并、`Active Calls` 移除、`data-testid` 注入、`height:100%` 等高、`Grants` `auth_method` 兜底逻辑。
- Phase 2：未登录 Sync 登录提示切换到 theme token，暗色主题回归。
- Phase 3：`admin-settings.spec.ts` 补齐 UI E2E 断言，包含暗色主题 + `has_session=false` 场景。
- Phase 4：`human_tests/remote-invoke.md` 与 `human_tests/readme.md` 更新用例说明。

## 测试方案

### 单元测试

本次改动为纯布局重组与样式 token 替换，无独立业务函数；不新增前端单元测试。

### E2E 测试

`web/tests/ui/admin-settings.spec.ts`：

- `Settings Remote Invoke 状态卡片合并 + SSH 卡片独立` 用例（`admin-settings.spec.ts:1999`）：
  - `page.getByTestId("settings-remote-invoke-status-card")` 可见；
  - `settings-remote-invoke-connection-section` 与 `settings-remote-invoke-discovery-section` 同时存在（`spec.ts:2001-2004`）；
  - `settings-remote-invoke-ssh-card` 独立存在（`spec.ts:2006`）；
  - `Connection Status` 不再包含 `Active Calls` 文案；
  - `Enter Discovery Mode` 按钮仍可点击。
- `Settings Remote Invoke Grants 展示 SSH key 与 Pair code 连接方式`（`admin-settings.spec.ts:2025`）：
  - `settings-remote-invoke-grants-card` 可见（`spec.ts:2080`）；
  - `SSH key` 与 `Pair code` 文案同时出现（`spec.ts:2082`、`spec.ts:2084`）。
- 暗色主题 + 未登录 Sync 回归（`admin-settings.spec.ts:2093-2178`）：
  - `localStorage` 预置 `bifrost-theme=dark`；
  - mock `/api/sync/status` 返回 `has_session:false`；
  - `html[data-theme="dark"]` 生效（`spec.ts:2118`）；
  - 登录提示可见，且提示块背景色/边框色不等于旧的浅黄色硬编码值。

### human_tests

`human_tests/remote-invoke.md`：新增或扩展布局回归用例，验证：

- 左侧状态卡片已合并；
- `Remote Status` 卡片不再展示 `Active Calls`；
- `Recent Calls` 中仍保留 caller 信息；
- `Grants` 列表能看出每条 grant 是 SSH key 连接还是 pair code 连接；
- 宽屏下左右卡片首屏占满，无明显孤立空白块；
- Discovery Mode 操作区仍可正常识别与点击；
- 未登录 Sync 服务时，暗色主题下登录提示与卡片背景协调，提示文案和 Sync Server 地址可读；
- `Create SSH key` 弹窗只显示合并后的单条说明文案。

同步刷新 `human_tests/readme.md` 用例索引。

### 校验命令

- `pnpm --filter @bifrost/web lint`
- `pnpm --filter @bifrost/web test:ui -- admin-settings`
- 按 `human_tests/remote-invoke.md` 手工回归上述用例

本机 no-local-coverage，Web coverage 交由 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：合并卡片、去掉 `Active Calls`、theme token 化、Grants 双标签。
- 复核 diff：`RemoteInvokeTab.tsx` 与 `admin-settings.spec.ts` 是否成套；是否有 `Active Calls` 残留；是否有硬编码颜色残留。
- 复测：Playwright 相关用例，暗色主题回归。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 视觉走查（宽/窄屏、light/dark）；确认 `Recent Calls` 详情弹窗 `Caller` 字段仍存在。
- 复跑 `pnpm --filter @bifrost/web test:ui -- admin-settings`。

## 风险与决策

- 合并卡片后，`Discovery Mode` 权重相对下降，需要通过标题分隔线 + 稳定的 test-id 保证可访问性。
- Ant Design theme token 覆盖硬编码颜色属于渐进式改造；如后续 token 命名调整需要同步刷新。
- `auth_method` 依赖后端已经返回该字段；老 relay 未升级时按 `ssh_key_fingerprint` 兜底。
- 若产品未来重新拆分 `Connection` / `Discovery`，需要保留现有 test-id 以避免破坏 E2E。
