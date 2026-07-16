# Desktop Runtime Port Switch

## 背景

Bifrost 桌面端默认代理端口是 9900。用户会在多种场景下修改端口：

- 端口被别的服务（DevTools proxy、Charles、其它 dev server）占用；
- 与团队约定统一在某个端口对外提供服务；
- 需要在同一台机器同时跑两个 Bifrost 实例做对比测试；
- 用户临时想切到规范的 8080 / 8888 端口方便别的工具连接。

早期实现是“改配置 + 重启 sidecar”：桌面壳层写 `desktop-config.json`，kill 内嵌 core，再按新端口重新拉起。这个做法带来的问题：

- 每次切换要等 20 秒左右的 backend bootstrap；
- 现有 TLS/WS/长连接被切断；
- traffic 表被写入短暂断层，push 通道要重连；
- 用户在 Settings 页操作时窗口有明显的“断开-恢复”感受。

当前实现把顺序倒过来：优先让 core 自身热切监听端口（`PUT /_bifrost/api/config/server` + `PortRebindManager`），只有 core 明确无法 rebind、或者响应格式不识别、或者健康探测失败时才回退到桌面壳层的整进程重启。前端在两条路径上都能感知实际生效端口。

本文覆盖端口切换的用户语义、桌面壳层协议、后端 rebind 机制、失败回退、前端连接恢复以及 UI 文案边界。相关文档：`desktop-launcher-startup.md`、`desktop-startup-observability.md`、`desktop-core-watchdog-resource-guard.md`、`crates/bifrost-admin/ADMIN_API.md`。

## 用户目标验证清单

### 必须实现

- 用户在 Settings → Proxy 中把端口从 X 改成 Y，桌面壳层调用 `update_desktop_proxy_port(Y)`。
- 若 X == Y：直接返回当前 runtime，不做任何切换。
- 否则先尝试后端运行时 rebind：`PUT http://127.0.0.1:X/_bifrost/api/config/server { "port": Y }`。
  - 成功（返回 `{expected_port, actual_port}` 结构）：以 rebind 结果为准，更新 `desktop-config.json`、`state.expected_port`、`state.port`。
  - 后端返回的是普通 server config JSON（说明该 core 不支持 rebind）：进入 `RestartRequired` 分支，走 `restart_backend_on_port`。
  - 响应既不是 rebind 结果也不是 server config：用 `wait_for_rebound_backend_port` 在 `Y..Y+MAX_PORT_INCREMENT_ATTEMPTS` 上做最多 ~2s 健康探测；若某个端口 healthy 则视为 rebind 成功，否则报错。
- 目标端口被占用：由 core 侧 `PortRebindManager` 决定实际端口（可能顺延），桌面壳层以 `actual_port` 为准。
- 桌面窗口不整体退出；handoff 已完成的 host window 与前端保持存在。
- 前端连接恢复：
  - 更新 `expectedProxyPort` / `proxyPort`；
  - 对新端口做 admin 健康轮询；
  - 刷新 overview / proxy / cli-proxy / system-proxy 状态；
  - 断开旧 push 通道、按新端口重连；
  - 缓存最近可用的 `invoke` 与窗口句柄，降低切换窗口期误报。

### 必须不破坏

- 主端口 `bifrost port` 命令、临时端口绑定语义。
- Backend watchdog（`monitor_desktop_backend`）：切换过程中 watchdog 不能把切换视为异常并触发 recovery。使用 `backend_recovery_in_progress` guard 独占。
- `desktop-config.json` 的持久化字段结构 `{ proxy_port: u16 }`。
- Admin API 安全模型（仍要 loopback）。
- `desktop-bootstrap.log`、sidecar stdout/stderr 日志文件持续写入。
- CLI `bifrost start --port` 语义与默认值。

### 必须真实验证

- 修改端口后 Settings 页与主界面立即使用新端口。
- 切换过程中 host window 不刷新、不重启，前端路由保持。
- 目标端口被占用时能顺延到下一个可用端口，`proxyPort` != `expectedProxyPort` 时 UI 展示两个值。
- 后端返回普通 server config（老 core）时进入重启回退，最终仍能到达新端口。
- 后端崩溃或响应超时时 rebind 失败返回错误，前端弹提示，桌面端不进入 zombie 状态。
- 切换后一段时间 watchdog 保持沉默（不误报）。

## 产品语义

### 切换意图与最终态

- 用户输入的目标端口称为 `expected_port`。
- 实际生效端口称为 `actual_port`，可能等于或大于 `expected_port`（rebind manager 可能顺延）。
- 桌面壳层持久化 `expected_port`（`desktop-config.json`），下一次启动仍从这个偏好开始。
- 桌面 runtime 同时展示两个值，前端负责区分。

### 首选路径：core 内部 rebind

- 桌面壳层通过 `PUT http://{BACKEND_ADMIN_HOST}:{current_port}/_bifrost/api/config/server` 发送 `{ "port": expected_port }`。
- Admin handler `update_server_config` 识别 `port` 字段：
  - 若 `port == 0` 返回 400；
  - 若 `port_rebind_manager` 为空返回 503（当前 runtime 不支持 rebind）；
  - 否则 `manager.rebind_port(port).await`，成功后返回 `UpdateServerPortResponse { expected_port, actual_port }`。
- `PortRebindManager` 通过 mpsc + oneshot 将请求转给 proxy 主循环，代理层重建 listener 到目标端口（顺延时更新 `actual_port`）。

### 回退路径：桌面壳层整进程重启

`restart_backend_on_port` 分支只在以下情况触发：

- Admin 明确返回普通 `server` config JSON（老版 core 没实现 rebind endpoint 的 `port` 字段）；
- 响应结构完全无法解析且健康探测也失败。

执行动作：

1. `begin_backend_recovery` 抢占 recovery guard，避免 watchdog 并发重启。
2. `state.startup_ready = false` + 清 `startup_error`。
3. Kill 当前 managed child（如果还持有）。
4. 同步调用 `stop_backend_with_binary`（`<embedded core> stop`）保证 daemon runtime marker 被清理。
5. `wait_for_backend_shutdown(current_port, 3s)` 等旧端口断开；stop helper 失败或超时后旧端口仍健康时立即返回错误，禁止继续启动 replacement core。
6. `launch_backend_on_available_port(expected_port)`：从 `expected_port` 开始，最多 `MAX_PORT_INCREMENT_ATTEMPTS = 64` 次端口顺延。
7. 成功后写回 `state.child`、`state.startup_ready = true`。

### 健康探测兜底

Admin 响应无法用 `parse_port_update_response` 或 `is_server_config_response` 识别时，桌面壳层不直接把切换视为失败：

```rust
let actual_port = wait_for_rebound_backend_port(expected_port, Duration::from_secs(2))?;
Ok(BackendPortTransition::Rebound(DesktopPortUpdateResponse {
    expected_port,
    actual_port,
}))
```

`wait_for_rebound_backend_port` 在 `Y..=Y+64` 循环 probe，直到 healthy 或超时。这解决了 core 未来输出结构再演进时前后端错位导致误报的问题。

### Watchdog 与 rebind 的隔离

- `monitor_desktop_backend` 每 `BACKEND_WATCHDOG_POLL_INTERVAL = 2s` 检查 managed child 是否退出、`probe_backend_health(current_port)` 是否 healthy。
- `restart_backend_on_port` 和 watchdog 恢复都使用同一个 `backend_recovery_in_progress` `AtomicBool` guard；同时只允许一个 recovery 流。
- Rebind 成功时不 kill child，因此 watchdog 不会 poll 出 exit；`state.port` 已经更新到新值，`probe_backend_health(new_port)` 也能通过。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `#[tauri::command] update_desktop_proxy_port(state, port)` 用户入口。
  - `request_backend_port_transition(current_port, expected_port)`：调用 admin rebind API + 兜底解析。
  - `parse_port_update_response` / `is_server_config_response`：JSON 判别。
  - `wait_for_rebound_backend_port(expected_port, timeout)`：健康探测兜底。
  - `restart_backend_on_port(state, current_port, expected_port)`：整进程回退。
  - `save_desktop_config(&config_path, &DesktopConfig { proxy_port })`：持久化。
  - `begin_backend_recovery(&state)`：`AtomicBool` swap guard。
  - `BackendPortTransition { Rebound, RestartRequired }` 枚举。
- `crates/bifrost-admin/src/port_rebind.rs`
  - `PortRebindManager::rebind_port(expected_port) -> PortRebindResponse`
  - `PortRebindManager::channel(buffer)`
- `crates/bifrost-admin/src/handlers/config.rs`
  - `update_server_config` → 解析 body 中 `port` 字段 → 调 manager。
  - `UpdateServerConfigRequest` / `UpdateServerPortResponse`。
- `crates/bifrost-proxy/src/server.rs`：主循环消费 `PortRebindRequest`，重建 listener。
- `web/src/desktop/tauri.ts`
  - `invokeDesktop("update_desktop_proxy_port", { port })`。
  - `DesktopRuntimeInfo` 类型。
  - 缓存最近可用 `invoke` 与窗口句柄。
- `web/src/runtime.ts` / `web/src/stores/useDesktopCoreStore.ts`：runtime 状态、reconnect。
- `web/src/pages/Settings/tabs/ProxyTab.tsx`：Settings UI 与文案。

## 请求 / 响应契约

### 请求

```http
PUT http://127.0.0.1:{current_port}/_bifrost/api/config/server
Content-Type: application/json

{ "port": 9901 }
```

### 响应

- Rebound（新版 core）：

```json
{
  "expected_port": 9901,
  "actual_port": 9901
}
```

或 camelCase 兼容：

```json
{
  "expectedPort": 9901,
  "actualPort": 9902
}
```

`parse_port_update_response` 通过 serde `alias` 同时接受 snake / camel。

- Restart Required（老版 core）：

```json
{
  "timeout_secs": 30,
  "http1_max_header_size": 65536,
  "http2_max_header_list_size": 262144,
  "websocket_handshake_max_header_size": 65536
}
```

`is_server_config_response` 检查所有字段 > 0 才认定。

- 未知结构：进入 `wait_for_rebound_backend_port` 健康兜底。

## 状态流

```
update_desktop_proxy_port(port)
├── if expected_port == port → return current runtime
├── current_port ← state.port
├── request_backend_port_transition(current_port, port)
│   ├── PUT /_bifrost/api/config/server { port }
│   ├── success body?
│   │   ├── parse_port_update_response → Rebound(runtime)
│   │   ├── is_server_config_response  → RestartRequired
│   │   └── else → wait_for_rebound_backend_port → Rebound|Err
│   └── non-2xx → Err
├── Rebound(runtime) → 使用 runtime
├── RestartRequired  → restart_backend_on_port(state, current_port, port) → runtime
├── save_desktop_config({ proxy_port: port })
├── state.expected_port ← port
├── state.port ← runtime.actual_port
└── return DesktopRuntimeInfo { expected, actual, startupReady, startupError }
```

## 依赖项

- `desktop/src-tauri/src/main.rs`
- `web/src/desktop/tauri.ts`
- `web/src/runtime.ts`
- `web/src/stores/useDesktopCoreStore.ts`
- `web/src/pages/Settings/tabs/ProxyTab.tsx`
- `crates/bifrost-admin/src/handlers/config.rs`
- `crates/bifrost-admin/src/port_rebind.rs`
- `crates/bifrost-proxy/src/server.rs`

## CLI / 环境变量 / Web / Admin API 表面

- 无新 CLI；用户只通过桌面 Settings 页面切换端口。CLI 侧 `bifrost start --port` / `bifrost port` 保持独立。
- 无新环境变量；`BIFROST_DATA_DIR` 影响 daemon runtime marker 定位。
- Tauri invoke：`update_desktop_proxy_port(port: u16) -> Result<DesktopRuntimeInfo, String>` 与 `get_desktop_runtime() -> Result<DesktopRuntimeInfo, String>`。
- Admin：`PUT /_bifrost/api/config/server`：body `port` 字段专用于 rebind，其它字段（`timeout_secs`、`http1_max_header_size` 等）走既有 server config 更新，两类互斥。

## Sync 边界

- 端口是本机运行时选择，不通过 sync。
- `desktop-config.json` 本地文件，随 `BIFROST_DATA_DIR` 走。

## 实现切分

### Phase 1：Admin rebind 通道（已完成）

- 新增 `PortRebindManager` + `PortRebindRequest`。
- `update_server_config` 识别 `port` 字段并转给 manager。
- Proxy 主循环消费请求，重建 listener。
- Admin 单元/集成测试覆盖 rebind 成功、port=0、manager 不存在等分支。

### Phase 2：桌面壳层优先热切（已完成）

- `update_desktop_proxy_port` 走 rebind API。
- `parse_port_update_response` + `is_server_config_response` + `wait_for_rebound_backend_port` 三段判别。
- 幂等 short-circuit：相同端口直接返回当前 runtime。

### Phase 3：桌面壳层重启兜底（已完成）

- `restart_backend_on_port` 走 `terminate_managed_backend` + `stop_backend_before_restart` + `launch_backend_on_available_port`；前两步必须成功且旧端口必须确认下线，否则 fail-closed。
- `begin_backend_recovery` guard 与 watchdog 共享。

### Phase 4：前端 UI 与 store 收敛（已完成）

- Settings 卡片描述改成 rebind 语义。
- Store 更新端口后触发状态刷新与 push 重连。
- `web/src/desktop/tauri.ts` 缓存 `invoke` 与窗口句柄。
- Apply 按钮文案仍为 “Apply & Restart”，属尚未完全统一的表述，非阻塞（保留描述以便未来收敛）。

### Phase 5：文档 & 人工测试维护

- `crates/bifrost-admin/ADMIN_API.md` 以 rebind 为准记录 `port` 字段行为。
- 本文覆盖端到端语义。
- Human_tests 覆盖 rebind / restart / port 顺延 / watchdog 无误报四种场景。

## 测试方案

### 单元测试

- `desktop/src-tauri/src/main.rs::tests`
  - `parses_snake_case_port_update_response`
  - `parses_camel_case_port_update_response`
  - `detects_legacy_server_config_response`
  - `backend_recovery_guard_prevents_parallel_recovery`
  - `poll_managed_backend_exit_reports_exited_child`
- `crates/bifrost-admin/src/handlers/config.rs`：
  - `update_server_config` 分支覆盖 (`port` 字段成功 / `port_rebind_manager` 为空 / `port == 0` / 非 port 字段的原逻辑)。
- `crates/bifrost-admin/src/port_rebind.rs`：
  - `rebind_port_returns_error_when_manager_dropped`
  - `rebind_port_forwards_expected_port_to_receiver`

### E2E / 真实场景（`human_tests/desktop-runtime-port-switch.md`）

- TC-DRP-01：Rebind 成功路径：Settings 输入新端口 → runtime 更新 → 手机连接新端口成功。
- TC-DRP-02：目标端口占用 → 顺延到下一可用端口；UI 展示 `expected != actual`。
- TC-DRP-03：老版 core（模拟返回普通 server config）→ 走 `restart_backend_on_port`；backend 停 → 起，新端口 healthy。
- TC-DRP-04：Admin 响应超时 / 无法解析 → 走 `wait_for_rebound_backend_port`；`healthy` 时视为成功；始终 unhealthy 时报错，桌面进入错误提示但不退出。
- TC-DRP-05：Rebind 期间 watchdog 保持沉默；`desktop-bootstrap.log` 无 recovery reason。
- TC-DRP-06：相同端口 (X == Y) short-circuit 不触发 admin 请求。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-desktop --tests`
- `cargo test -p bifrost-admin port_rebind config`
- `rust-project-validate`
- 本机 no-local-coverage。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标：热切优先、重启兜底、端口顺延、UI 感知实际端口、watchdog 不误报。
- 复核 diff：Admin handler / port_rebind / desktop shell 三处闭环；前端 store / Settings / tauri.ts 状态同步。
- 重点 review：short-circuit 是否会漏改 desktop-config；`restart_backend_on_port` 在 stop 失败时是否仍尝试 launch；`wait_for_rebound_backend_port` 的顺延范围是否与 `MAX_PORT_INCREMENT_ATTEMPTS` 一致；watchdog `backend_recovery_in_progress` guard 是否覆盖 restart 分支。
- 复测：单元测试 + rebind / restart / port busy 三个场景真实操作。

### 第 2 轮

- 复核第 1 轮问题修复。
- 检查 `git status --short`、`git diff`、human_tests 索引。
- 重点 review：错误路径下 `state.expected_port` 与 `state.port` 不被误更新；`startup_error` 是否被正确清理。
- 复测：Apply 后前端 push 通道断开重连；system proxy / cli proxy 请求命中新端口。

## 风险与决策点

- Admin `PUT /_bifrost/api/config/server` 复用 body：`port` 字段与其它 server config 字段互斥，避免歧义。
- Rebind 顺延后 `actual_port != expected_port`：桌面壳层持久化 `expected_port`；下次启动 `launch_backend_on_available_port(expected_port)` 会再次尝试首选端口。
- 老 core（无 `port_rebind_manager`）返回 503 vs 返回普通 server config：桌面壳层对两种都能处理。503 会走 `!response.status().is_success()` 报错分支；普通 server config 走 RestartRequired。若未来老 core 完全下线可简化。
- `wait_for_rebound_backend_port` 使用与 `MAX_PORT_INCREMENT_ATTEMPTS` 一致的顺延范围，避免探测口径不一致。
- Settings 按钮文案 “Apply & Restart” 属历史用词，热切时并不真的 restart；短期保留是为了避免用户以为“只是热切没有真的应用”，长期应改成 “Apply”。
- Rebind 期间前端 push / SSE 通道会在切换窗口有短暂断开：靠 `useDesktopCoreStore` 与缓存的 invoke/window 句柄回补，仍可能出现瞬时白屏，可接受。
- Rebind 只影响 admin 侧代理监听端口，不改变 admin API 自身的端口（admin API 与代理共用同一端口）：因此 rebind 后 admin URL 也必须更新，桌面壳层用 `state.port` 组装后续 API URL。
