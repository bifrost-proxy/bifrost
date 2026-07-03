# HTTPS Interception 状态可见性

## 背景

Bifrost 的全局 HTTPS Interception 是高影响开关: 一旦启用,代理会对所有匹配的 CONNECT/SOCKS TLS 流量做 MITM,解密后再转发,直接影响用户隐私、性能和 TLS 兼容性。用户在切换项目、临时开启拦截调试后经常忘记关闭,导致后续所有 HTTPS 流量都被解密。

本设计要求 Web UI 状态栏、Tray 菜单和 Admin API 一致地暴露 TLS 全局拦截状态,让用户在任何前端上都能一眼看到当前 TLS 拦截是 Full On / Scoped / Off,并能直接跳入设置调整。为了避免刷新延迟造成误导,Tray 顶部图标和 macOS 系统状态标题只保留 System Proxy 状态,不再展示 TLS 角标。

## 用户目标验证清单

### 必须实现

- Web UI 底部状态栏必须实时展示 `TLS: Full On` / `TLS: Scoped` / `TLS: Off` / `TLS: Unknown` 四态,并订阅 `settings_update: tls_config` push,不做轮询。
- 全局 `enable_tls_interception=true` 时,状态栏文字与圆点必须有可见的强调 (脉冲/跳动动画或强色),暗色主题也可辨识。
- 完全关闭且无 include list 时展示 `Off`;关闭但 `intercept_include`/`app_intercept_include`/`ip_intercept_include` 任一非空时展示 `Scoped`。
- 点击状态栏 TLS 区域可跳转 `Settings?tab=tls`,支持键盘 focus 与 Enter。
- Tray 下拉菜单顶部提供 `System Proxy: On/Off` 与 `TLS Interception: On/Off` 两个独立可切换行,读取 Admin API `/api/config/tls` 与 `/api/proxy/system` 的 snapshot。
- Tray 顶部图标/macOS 状态栏标题不展示 TLS 状态角标,避免频繁刷新带来的误导。

### 必须不破坏

- Admin API `/api/config/tls` 现有字段 (`enable_tls_interception`、`intercept_include`、`app_intercept_include`、`ip_intercept_include`) 与 push channel 保持向后兼容。
- Tray 的 `System Proxy` 单行勾选行为、快捷菜单结构、其它菜单项(quit/open dashboard 等)保持不变。
- Web UI 其它状态指示 (proxy port、data dir、update available、runner state) 不受影响。
- 无 push 场景(SSE 未连接、离线时)有 fallback 到最近一次拉取或 `Unknown`。

### 必须真实验证

- 使用真实 Chrome 打开 Web UI,启用 `enable_tls_interception` 后状态栏出现 `Full On` 且圆点脉冲。
- 使用 tray 菜单直接切换 TLS Interception, Web UI 状态栏在 2s 内同步为新值,不需要手动刷新。
- 亮/暗主题、macOS/Linux tray 都需人工验证一次可读性。

## 产品语义

`getTlsInterceptionIndicator` 只根据 `TlsConfig` snapshot 派生四态,不引入 pending/error 中间态:

- `enable_tls_interception === true` → `{ state: "full", text: "Full On" }`。
- 否则如果 `intercept_include.length + app_intercept_include.length + ip_intercept_include.length > 0` → `{ state: "limited", text: "Scoped" }`。
- 否则 → `{ state: "limited", text: "Off" }`。
- config 未加载 → `{ state: "unknown", text: "Unknown" }`。

状态栏语义: `state=full` 代表所有 TLS 流量都会被解密,是最强调状态; `limited` 代表仅少量域名或应用会被解密; `unknown` 代表还没拿到 config snapshot。

## 技术细节

### Web UI

- 派生逻辑: `web/src/components/StatusBar/statusIndicators.ts:getTlsInterceptionIndicator`,输出 `{active, text, detail, state}`。
- 状态栏渲染: `web/src/components/StatusBar/index.tsx`,读取 `useTlsConfigStore` 提供的最近 snapshot,根据 `state` 挂 `data-state` 属性,CSS 根据 `data-state=full` 加脉冲动画。
- Store: `web/src/stores/useTlsConfigStore.ts`,首次挂载调用 `/api/config/tls` 拉取,随后订阅 `pushService` 的 `settings_update` channel 中 `tls_config` 事件,更新到最新 snapshot。
- 类型: `web/src/api/config.ts` 的 `TlsConfig` 包含 `enable_tls_interception`、`intercept_include`、`app_intercept_include`、`ip_intercept_include`。
- 单元测试: `web/src/components/StatusBar/statusIndicators.test.ts` 覆盖 four states + null。
- Playwright: `web/tests/ui/admin-settings.spec.ts` mock `/api/config/tls` 三种 payload 断言状态栏文本、`data-state`、点击跳转。

### Tray

- Menu builder: `crates/bifrost-cli/src/commands/tray/menu.rs`。
  - `System Proxy: On/Off` 与 `TLS Interception: On/Off` 独立顶层行。
  - Tray icon title 只包含 System Proxy state,不 append TLS。
- Runtime: `crates/bifrost-cli/src/commands/tray/tray.rs`。
  - `TrayRuntime` 保存 `enable_tls_interception: bool` snapshot。
  - 后台 poller 每次 tick 拉取 `/api/config/tls`,更新 snapshot 并 rebuild menu。
  - 点击 `TLS Interception` 触发 `PUT /api/config/tls` 反转 `enable_tls_interception`,并使用 `pending action` 锁避免并发切换。
- 单元测试: `crates/bifrost-cli/src/commands/tray/tray_tests.rs` 覆盖 menu 构造、pending 抑制、System Proxy vs TLS 独立性。

### Admin API

- `GET /api/config/tls` 返回完整 `TlsConfig`,包含 `enable_tls_interception` 与三个 include list。
- `PUT /api/config/tls` 支持切换 `enable_tls_interception`,变更后通过 `SharedPushManager` 广播 `settings_update: tls_config`,Web UI/Tray 同步刷新。
- 见 `crates/bifrost-admin/src/handlers/config.rs` 与 `crates/bifrost-admin/ADMIN_API.md`。

## CLI + Web + Admin API

| 入口 | 命令/路径 | 展示位置 |
| --- | --- | --- |
| CLI | `bifrost status --format json` | `tls.enable_tls_interception`、`tls.intercept_include`、`tls.app_intercept_include`、`tls.ip_intercept_include` |
| CLI | `bifrost config tls show` (若支持) | 与 admin API 同源 |
| Tray | menu | System Proxy / TLS Interception 两行,顶部图标不含 TLS |
| Web UI | 底部状态栏 | Full On / Scoped / Off / Unknown |
| Admin | `GET /api/config/tls`, `PUT /api/config/tls` | snapshot 与切换 |

## Sync 边界

- `enable_tls_interception` 属于本机 TLS config,不通过 API Sync 上行远端,避免多设备互相污染 MITM 状态。
- Tray/Web UI push channel 仅在本机 `SharedPushManager` 广播,不跨设备。
- 已存在的 `unified_config` 存储层负责持久化,重启后按 disk snapshot 恢复。

## Phase 1: Web UI 状态派生与订阅

- 实现 `getTlsInterceptionIndicator`。
- `useTlsConfigStore` 初次拉取 + push 订阅。
- 状态栏挂载 `TlsInterceptionIndicator`。
- 单元测试与初步 Playwright。

## Phase 2: Tray 菜单

- Menu builder 拆分 System Proxy 与 TLS 两行,tray 图标标题不再包含 TLS。
- Runtime poller 与切换动作,pending 抑制。
- Rust 单元测试覆盖菜单结构与 pending 逻辑。

## Phase 3: 动画与主题

- 状态栏 CSS 按 `data-state` 变化,亮/暗主题分别检查。
- 支持 `prefers-reduced-motion` 时动画退化为静态强调。

## Phase 4: 文档与真实场景

- 更新 `human_tests/tls-interception-status-indicators.md`。
- 更新 `human_tests/readme.md` 索引行。
- 若 tray 菜单结构改动需更新 CLI 相关 human_tests。

## 测试方案

### 单元测试

- `web/src/components/StatusBar/statusIndicators.test.ts`: 覆盖 full/scoped/off/unknown 派生。
- `crates/bifrost-cli/src/commands/tray/tray_tests.rs`: 覆盖 System Proxy 与 TLS 独立性、pending 锁、tray 顶部标题不含 TLS。
- `crates/bifrost-cli/src/commands/tray/menu.rs` 内嵌单测: 覆盖菜单标题构造。
- `crates/bifrost-admin/src/handlers/config.rs`: TLS get/update 覆盖。

### E2E 测试

- `web/tests/ui/admin-settings.spec.ts`: mock 三种 payload,断言状态栏、动画 class、跳转入口。
- `e2e-tests/tests/test_tls_intercept_mode_api.sh`: 通过 Admin API 切换 `enable_tls_interception`,验证 GET 返回一致。
- `crates/bifrost-e2e/src/tests/tls_switch_test.rs`: Rust E2E 切换与响应。

### 真实场景 (human_tests)

`human_tests/tls-interception-status-indicators.md`:

- TC-TLSI-01: Web 状态栏亮色主题,`Full On` 动画可见,点击跳设置。
- TC-TLSI-02: 暗色主题下 `Full On`/`Scoped`/`Off` 三态文字与圆点对比度可读。
- TC-TLSI-03: Tray 顶部图标 title 不含 TLS,菜单展示 System Proxy 与 TLS Interception 独立行。
- TC-TLSI-04: Tray 点击 TLS Interception 切换后 Web UI 状态栏 2s 内同步。
- TC-TLSI-05: `intercept_include` 非空但 `enable_tls_interception=false` → 状态栏 `Scoped`。

配套 `human_tests/readme.md` 索引行更新,`human_tests/statusbar-proxy-popover.md` 与 `human_tests/cli-system-proxy.md` 中 tray 描述保持一致。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 Web 派生逻辑、push 订阅、tray snapshot、tray 顶部标题不含 TLS、System Proxy 与 TLS 两行结构。
- 复测 statusIndicators 单元、tray 单元、`test_tls_intercept_mode_api.sh`、Playwright admin-settings。

### 第 2 轮

- 基于最新 diff 复查主题颜色、动画降级、菜单文案、human_tests/readme 索引、未触碰的既有 ASR/其它 status 改动。
- 检查 `TrayRuntime` 在切换失败(admin 401/500)时是否回滚 snapshot 并显示 "TLS interception toggle failed" 提示。
- 复跑相关单元测试与 Playwright。

## 校验要求

- 先执行本模块 Playwright 与 Rust 单元。
- 再执行 `rust-project-validate`(交由 CI 或用户手动)。
- 无 no-local-coverage 例外时按 CI 常规覆盖率跑,本地约定禁跑 `make coverage`。

## 风险与决策

- 决策: Tray 图标不展示 TLS 角标。原因: tray icon 由 OS 缓存,更新有延迟,若用户已在 Web UI 切换但 tray 尚未刷新,会产生 "两地状态不一致" 错觉,反而增加沟通成本。将 TLS 状态收敛到菜单内文字。
- 风险: `settings_update` push 通道拥塞可能延迟状态更新。缓解: 状态栏保留短周期 fallback 拉取(仅在 push 中断 >30s 时补一次)。
- 风险: `Scoped` 语义可能被理解为"部分域名已被拦截",但 include list 只是"允许 MITM 的白名单",实际是否拦截仍受规则和请求命中影响。文案 detail 明确 "N domain, app, or IP rule(s) can still enable HTTPS interception",避免歧义。
- 风险: 未来若增加 `intercept_exclude` list,状态派生需要考虑 exclude-only 场景。当前设计以 include-only 计数,后续扩展时补测试。
