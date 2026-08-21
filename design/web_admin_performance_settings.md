# 管理端 Performance 设置

## 背景

Web 管理端 `Settings → Performance` 面板是唯一集中调节 Bifrost 流量记录、Body 缓冲、断点超时、二进制流量捕获、Badge 注入等运行时性能参数的入口。历史上该面板是一次修改立即 `PUT`，滑条拖拽时会产生大量抖动请求；同时旧文档只覆盖了 `Max Records` 滑条防抖，未包含后续扩展进来的一整组存储/断点/注入开关。

当前实现（验证于 2026-06-17，代码位于 `web/src/pages/Settings/tabs/PerformanceTab.tsx` 与 `web/src/pages/Settings/index.tsx`）已经统一为「本地草稿 + 按字段独立 600ms 防抖 + 失败回滚」模型，Admin API 侧 `/api/config/performance` 与 `/api/config/performance/clear-cache` 也做了范围校验、区间上下限与热应用。该文档在此把当前实际能力做完整重写，替代旧的单交互式说明。

## 用户目标验证清单

### 必须实现

- Performance 面板上的所有字段：滑动/输入时立刻更新本地显示，不出现输入框反弹。
- 每个字段独立防抖 600ms 后向 `PUT /api/config/performance` 提交，只保留最后一次值。
- Admin 侧对每个字段的范围/枚举执行校验，越界请求返回 400 并携带可读错误。
- 服务端应用成功后回传完整 `PerformanceConfigResponse`，前端用其覆写草稿。
- 请求失败时把草稿回滚到最近一次成功的服务端配置，避免出现「UI 已改、后端未改」的错觉。
- `Clear Cache` 按钮通过 `DELETE /api/config/performance/clear-cache` 清理 body / frame / ws_payload 缓存并刷新存储统计。
- 存储统计面板显示 `body_store` / `frame_store` / `ws_payload_store` 的 entry 数与近似字节数。
- 断点超时字段支持 min/max 区间提示，UI 与 Admin 均按 `MIN_BREAKPOINT_TIMEOUT_MS..=MAX_BREAKPOINT_TIMEOUT_MS` 收敛。

### 必须不破坏

- `bifrost config performance show|set` CLI 与 Admin API 语义一致：CLI 走 `/api/config/performance`，字段名保持 `snake_case`。
- `Max Records` 兼容旧默认与旧值范围（`DEFAULT_TRAFFIC_MAX_RECORDS` / `MIN_TRAFFIC_MAX_RECORDS..=MAX_TRAFFIC_MAX_RECORDS`），不因 UI 上限调整而误锁定旧配置。
- Body 缓冲/探针大小、Retention Days、Badge/Binary 开关，热应用到运行中的 `AdminState`（`set_max_body_buffer_size` / `set_max_body_probe_size` / `set_binary_traffic_performance_mode` / `set_breakpoint_timeout`），无需重启 daemon。
- 关闭 `Inject Bifrost Badge` 后，正在下发的 HTML 注入应立即停止；重新开启后新请求恢复注入。
- Total DB Size 相关字段变更后走 `retention_days` / 后台清理线程，不阻塞主线程。

### 必须真实验证

- CLI + Admin API：`bifrost config performance show` 打印字段与 `/api/config/performance` GET 一致。
- Web UI：滑条拖拽期间前端只发起 1 次 PUT（最后一次），失败时草稿回滚。
- Web UI：`Clear Cache` 按钮点击后统计计数明显下降，或返回空 store。
- 真实代理场景：把 `Enable Binary Traffic Capture` 关闭后，二进制帧不再进入 body_store。

## 产品语义

### 「本地草稿 + 分字段防抖」

`Settings/index.tsx` 中：

- `perfDraft: TrafficConfig | null` 与 `breakpointPerfDraft: BreakpointPerformanceConfig | null` 用于承载 UI 上尚未提交的值。
- `perfUpdateTimers = useRef<Record<string, number>>({})` 为每个字段挂一个独立 `window.setTimeout`，key 即字段名（`max_records`、`breakpoint_timeout_ms`、`inject_bifrost_badge` 等）。
- `schedulePerformanceUpdate(key, payload)` 先清掉同 key 的旧计时器，再挂新计时器；`600ms` 到期时执行 `PUT /api/config/performance`。
- 成功：`setPerfDraft(result.traffic); setBreakpointPerfDraft(result.breakpoint);` 把服务端权威结果反写覆盖。
- 失败：`setPerfDraft(performanceConfig.traffic); setBreakpointPerfDraft(performanceConfig.breakpoint);` 回滚到最近一次已知服务端值。
- 组件卸载：遍历 `perfUpdateTimers.current` 清理所有挂起计时器，避免路由切走后再触发陈旧 PUT。

`trafficDraft = perfDraft ?? performanceConfig?.traffic` 是渲染时使用的合并值：草稿存在优先看草稿，否则回退当前服务端配置。

### 字段清单

Performance 面板当前渲染的字段（对应 `PerformanceTab.tsx`）：

| 字段 | UI 上限 / 区间 | 后端字段 | 校验 |
| --- | --- | --- | --- |
| Max Records | UI 上限 100000 | `max_records` | `MIN_TRAFFIC_MAX_RECORDS..=MAX_TRAFFIC_MAX_RECORDS` |
| Max DB Size | 上限 10 GiB | `max_db_size` | 按字节校验 |
| Max Body Inline Size (DB) | 上限 10 MiB | `max_body_inline_size` | 按字节校验 |
| Max Body Buffer Size | 上限 64 MiB | `max_body_buffer_size` | 按字节校验 |
| Max Body Probe Size | 上限 1 MiB | `max_body_probe_size` | 按字节校验；低端相邻刻度文本需保持可读间距，不改变实际刻度值 |
| File Retention Days | 上限 7 天 | `file_retention_days` | `<= 7` |
| Breakpoint Timeout | min/max 区间 | `breakpoint_timeout_ms` | `MIN_BREAKPOINT_TIMEOUT_MS..=MAX_BREAKPOINT_TIMEOUT_MS` |
| Enable Binary Traffic Capture | 开关 | `binary_traffic_performance_mode` | bool |
| Inject Bifrost Badge | 开关 | `inject_bifrost_badge` | bool |
| Storage Stats | 只读 | `body_store` / `frame_store` / `ws_payload_store` | — |
| Clear Cache | 按钮 | 触发 `DELETE /api/config/performance/clear-cache` | — |

历史文档写的 `Max Records = 50000` 为旧值，当前实现和 UI 文案均以 `100000` 为上限，后端仍以 `DEFAULT_TRAFFIC_MAX_RECORDS` 作为默认。

### `Clear Cache` 的边界

`/api/config/performance/clear-cache` 会清空 `body_store` / `frame_store` / `ws_payload_store`，但不会删除已落 DB 的记录。传输中的 body 引用如果仍被 traffic detail 页面拿去渲染，会以 fallback 的方式重新按需拉取（涉及 `traffic.rs` 的 body/frame stream 路径）。

## 技术细节

### 前端

- 文件：`web/src/pages/Settings/tabs/PerformanceTab.tsx`
  - `handleMaxRecordsChange`、`handleBreakpointTimeoutChange`、`handleInjectBifrostBadgeChange` 等回调纯做本地 draft 更新，然后调用 `schedulePerformanceUpdate` 排入防抖队列。
  - 输入组件包括 `InputNumber` / `Slider` / `Switch`，控件 `value` 都绑到 `trafficDraft` 派生值。
- 文件：`web/src/pages/Settings/index.tsx`
  - `useEffect` 首屏 `GET /api/config/performance` 得到 `performanceConfig`，落入 `perfDraft` / `breakpointPerfDraft`。
  - 每次成功刷新 `PerformanceConfig` 后同步 `fetchHistory(3600)` 更新趋势图。
  - `Max Body Probe Size` 的 `0`、`16KB` 与 `64KB` mark 使用 Ant Design `marks` 对象的 label style 做局部排布修正，避免线性 0..1MiB 刻度下低端标签互相覆盖。

### Admin API

- 文件：`crates/bifrost-admin/src/handlers/config.rs`
  - 路由：`/api/config/performance`（GET / PUT）、`/api/config/performance/clear-cache`（DELETE）。
  - GET 使用 `get_performance_config`：从 `ConfigManager` 读；无 config manager 时回落 defaults（`DEFAULT_TRAFFIC_MAX_RECORDS`、`10 * 1024 * 1024`、`64 * 1024`、`file_retention_days = 7`、`binary_traffic_performance_mode = true`、`inject_bifrost_badge = true`、`DEFAULT_BREAKPOINT_TIMEOUT_MS`、`MIN/MAX_BREAKPOINT_TIMEOUT_MS`）。
  - PUT 使用 `update_performance_config`：
    - `file_retention_days > 7` → 400。
    - `max_records` 不在 `MIN..=MAX_TRAFFIC_MAX_RECORDS` → 400 `max_records must be between ...`。
    - `breakpoint_timeout_ms` 不在 `MIN..=MAX_BREAKPOINT_TIMEOUT_MS` → 400 `breakpoint_timeout_ms must be between ...`。
    - 校验通过后依次热应用：`traffic_db_store.set_max_records`、`set_retention_days` / `retention_days`、`state.set_max_body_buffer_size` / `set_max_body_probe_size`、`state.set_binary_traffic_performance_mode`、`state.set_breakpoint_timeout(timeout_ms)`。
    - 最终重新调用 `get_performance_config(state)` 回显。

- 结构体（同文件）：
  - `TrafficPerformanceConfig` / `BreakpointPerformanceConfig` / `PerformanceConfigResponse` / `PerformanceUpdateRequest`。
  - 所有请求字段均为 `Option<T>`，允许前端只发一个字段做 patch 式更新。

### CLI

- `crates/bifrost-cli/src/commands/config/mod.rs` 提供 `bifrost config performance show|set`，走同一 Admin API。
- `crates/bifrost-cli/src/commands/status_tui.rs` 通过 push 通道解析 `settings_update`，其中 `performance` scope 会广播 traffic + breakpoint 变更，TUI 侧使用 `parses_performance_settings_update_from_push_message`（单测）保证解析。

### Push 边界

- 更新性能配置会通过 `settings_update` push 消息推送到订阅了 `settings_scopes=performance` 的客户端（`crates/bifrost-admin/src/push.rs`）。前端 Settings 页面依赖同一订阅刷新非当前编辑字段。

## Sync 边界

- Performance 配置属于本地运行时策略，不进入 sync/import/export。
- 备份/恢复 `config.toml` 时会带上这些字段，但不会推到远端 sync backend。

## Phase 1 – 基础草稿 + 防抖

- 引入 `perfDraft` / `perfUpdateTimers`。
- 每个字段实现独立 `handleXxxChange` + `schedulePerformanceUpdate`。
- 失败回滚。
- 单元测试（前端 Vitest）：模拟连续 5 次滑动只触发 1 次 fetch。

## Phase 2 – Admin API 校验与热应用

- `update_performance_config` 校验区间。
- 热应用 4 类字段（records / body / binary / breakpoint）。
- 单测：`performance_config_uses_defaults_without_config_manager`、`get_performance_config_uses_config_manager_when_present`（`crates/bifrost-admin/src/handlers/config.rs`）。

## Phase 3 – 存储统计与 Clear Cache

- 面板展示 body/frame/ws_payload store 计数与近似字节数。
- `Clear Cache` 按钮触发 `DELETE /api/config/performance/clear-cache`，并回读统计。

## Phase 4 – 面板扩展项

- 加入 `Enable Binary Traffic Capture`、`Inject Bifrost Badge` 开关。
- 增加 `File Retention Days` 与 `Max DB Size`，与 `traffic_db_size_limit.md` 中的清理线程联动。
- 统一 push scope `performance`，让多标签页同步。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/config.rs`
  - `performance_config_uses_defaults_without_config_manager`
  - `get_performance_config_uses_config_manager_when_present`
  - （新增）`update_performance_config_rejects_out_of_range_max_records`
  - （新增）`update_performance_config_rejects_out_of_range_breakpoint_timeout`
  - （新增）`update_performance_config_hot_applies_body_buffer_and_probe`
- `crates/bifrost-cli/src/commands/status_tui.rs::parses_performance_settings_update_from_push_message`

### E2E 测试

已存在或需要覆盖的脚本：

- `e2e-tests/tests/test_performance_config_admin_api.sh` — Admin API GET/PUT/Clear Cache 冒烟。
- `e2e-tests/tests/test_breakpoint_performance_guard.sh` — 断点超时区间热应用。
- `e2e-tests/tests/test_body_cache_sync_cleanup_admin_api.sh` — body 缓存与 sync cleanup 交互。
- `e2e-tests/tests/test_total_size_cleanup_admin_api.sh` — DB 大小/清理线程。
- `e2e-tests/tests/test_traffic_db_e2e.sh` — 覆盖 `max_records` / `file_retention_days` 变更后新写入的旋转行为。
- `e2e-tests/tests/test_badge_injection_e2e.sh` — `inject_bifrost_badge` 开关热应用。

### Web UI 测试

- （新增）`web/tests/ui/settings-performance.spec.ts`
  - 连续滑动 5 次 → 只发 1 次 `PUT /api/config/performance`（拦截网络请求断言）。
  - 服务端注入 400 → 前端草稿回滚到 `performanceConfig.traffic`。
  - `Clear Cache` 点击后 body_store 统计归零或明显下降。
- `web/tests/ui/admin-settings.spec.ts`
  - `Settings Performance 的 Max Body Probe Size 相邻刻度文本不重叠`：用真实 Chromium 在 900px 和 1280px 宽度下量测 `Off`、`16KB`、`64KB`、`256KB`、`1MB` 全部相邻 mark 边界，覆盖亮色和暗色主题。

### 真实场景 human_tests

- 更新 `human_tests/api-config.md`：覆盖 GET/PUT/Clear Cache。
- 更新 `human_tests/badge-hover-panel.md` 与 `human_tests/traffic-cleanup.md`：分别对应 badge 开关与清理策略。

启动约束：均使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 覆盖草稿 + 防抖 + 回滚三个不变式。
- 覆盖 Admin API 全部区间校验错误。
- 覆盖 `Clear Cache` 与存储统计联动。

### 第 2 轮

- 排查是否有直接绕过 `schedulePerformanceUpdate` 的 setter；例如 legacy 组件直接 fetch。
- 排查 push `settings_update` 与本地 draft 竞争覆盖问题：本地 draft 存在时 push 到达是否会覆盖用户正在输入的值。当前实现在 draft 存在时选择「让服务端回显覆盖」，实现上要求前端在 blur 或 timer 完成后清空 draft。

## 风险与决策

- Draft 覆盖时机：目前设计选择「PUT 成功即回写」；如果用户在防抖窗口内继续拖拽，会由下一次 timer 覆盖，可接受。
- 高频拖拽下的 4xx：如果用户把值拖到区间外，Admin 返回 400，前端会回滚到上次成功值，导致视觉「反弹」。这是符合语义的行为，不再做 UI 侧预校验，避免与 Admin 校验分裂。
- Body/DB store 统计代价：统计走 in-memory 计数，无锁开销可忽略。
- `Clear Cache` 与正在渲染的 traffic detail：清理后 detail 会重新按需从 DB / stream 加载，会有短暂延迟。
