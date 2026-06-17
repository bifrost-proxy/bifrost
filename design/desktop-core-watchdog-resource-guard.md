# 桌面端 Core 保活与资源耗尽防护

## 功能模块详细描述

当前桌面端仅在启动期拉起内嵌 core，运行过程中如果 core 因异常退出、句柄耗尽或监听失活而不可用，桌面壳层仍然留在前台，但代理能力已经失效。这个状态会让用户看到“桌面还在、代理已坏”的半失效行为。

本次修复聚焦三个方向：

- 为桌面端增加运行期 core watchdog，发现 sidecar 退出或健康检查失活后自动恢复。
- 为容易放大文件句柄占用的 app icon 提取链路增加串行化保护，降低资源耗尽场景下的并发放大效应。
- 为 body / ws payload 这类长期持有文件句柄的存储链路补充活跃句柄统计与硬上限，避免长连接场景把 fd 持续拉爆。

## 实现逻辑

### 1. 桌面端运行期 watchdog

- 在 `desktop/src-tauri/src/main.rs` 中新增后台 watchdog（`monitor_desktop_backend()`，由启动期 `bootstrap_desktop_backend()` spawn 出独立线程），按 `BACKEND_WATCHDOG_POLL_INTERVAL`（默认 2s）周期轮询。
- watchdog 周期性检查：
  - 当前 managed child 是否已经退出（`poll_managed_backend_exit()`）
  - 当前 `proxy_port` 的健康探针 `probe_backend_health()` 是否仍然成功
- 若发现 core 异常退出或健康探针失败，则进入 `attempt_backend_recovery()` 统一恢复流程：
  - 标记 `startup_ready=false`
  - 通过 `begin_backend_recovery()` 拿到 `BackendRecoveryGuard`（基于 `BackendState.backend_recovery_in_progress` 原子位），避免并发重复恢复
  - 清理当前 child 句柄（必要时 `terminate_child()`）
  - 复用现有 `ensure_backend_running()` 逻辑重新拉起或接管 healthy backend
  - 恢复成功后更新运行端口、清空错误态，调用 `try_start_native_handoff()` 并记录日志
  - 恢复失败时按 `BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY`（默认 3s）退避后等待下一轮

### 2. 恢复流程约束

- 端口切换导致的显式 restart 与 watchdog 共用同一恢复互斥标记（`backend_recovery_in_progress`，通过 `begin_backend_recovery()` 进入临界区），防止运行期保活与手动重启并发执行。
- 端口切换导致的显式 restart 与 watchdog 共用同一恢复互斥标记，防止运行期保活与手动重启并发执行。
- 恢复失败时只记录错误并等待下一轮重试，不主动把桌面端一起退出。

### 3. app icon 资源保护

- 在真正进入 `extract_app_icon()` 前增加提取锁（`AppIconCache.extract_lock`），并在获得锁后再次检查内存缓存与磁盘缓存。
- 在真正进入 `extract_app_icon()` 前增加提取锁，并在获得锁后再次检查内存缓存与磁盘缓存。
- 这样可以避免高并发 traffic/app icon 请求在缓存尚未命中时，同时触发多路系统图标提取，放大文件句柄占用与系统资源竞争。

### 4. 流式文件 writer 统计与限流

- `BodyStore` 新增活跃 stream writer 计数，并设置默认硬上限。
- 当响应体 / SSE 原始流需要落盘时，若当前活跃 writer 已达到上限，则拒绝继续打开新的文件 writer，并记录告警日志。
- `BodyStreamWriter` 在 `finish()` 和 `drop` 时都会归还占用槽位，避免句柄计数失真。
- `WsPayloadStoreStats` / `BodyStoreStats` 会直接暴露当前活跃 writer 数与上限，沿用已有的 performance / system diagnostics 接口输出，便于快速判断“磁盘文件多”还是“活跃句柄多”。

### 5. 接近上限主动告警

- 新增统一 `resource_alerts` 计算模块，对 body stream writers 和 ws payload writers 复用同一套风险分级逻辑。
- 告警阈值分为两档：
  - `warn`：达到上限的 80% 及以上
  - `critical`：达到上限的 95% 及以上
- `/_bifrost/api/config/performance`、`/_bifrost/api/system/memory` 以及 settings push 的 `performance_config` scope 都会返回统一的 `resource_alerts` 字段。
- `BodyStore` / `WsPayloadStore` 在打开新 writer 后若进入 `warn/critical` 区间，会主动输出 warning 日志，便于离开页面后仍能从日志发现风险。

### 6. Performance 页风险态直观呈现

- Settings 的 Performance tab 在检测到 `resource_alerts.overall_level != ok` 时，会在页内顶部展示汇总告警。
- Body Cache / WebSocket Payloads 两个热点区域会显示 badge 状态，并在接近上限或进入危险区时直接使用 warning / error 色强调当前 writer 占用。
- 这样用户不需要翻 system diagnostics JSON，也能快速发现“文件句柄正在逼近上限”。

## 依赖项

- `desktop/src-tauri/src/main.rs`
- `crates/bifrost-admin/src/app_icon.rs`
- `crates/bifrost-admin/src/body_store.rs`
- `crates/bifrost-admin/src/ws_payload_store.rs`
- `crates/bifrost-admin/src/resource_alerts.rs`
- `crates/bifrost-admin/src/handlers/config.rs`
- `crates/bifrost-admin/src/handlers/system.rs`
- `crates/bifrost-admin/src/push.rs`
- `web/src/api/config.ts`
- `web/src/pages/Settings/tabs/PerformanceTab.tsx`
- `README.md`

## 测试方案（含 e2e）

1. 启动桌面端，确认正常情况下 watchdog 不影响现有启动与 handoff。
2. 人工终止内嵌 core，确认桌面端不会退出，且会自动重新拉起 backend。
3. 构造 backend 健康探针失败场景，确认 watchdog 会记录恢复日志并重试。
4. 构造多路长连接 body / SSE 流，确认 body stream writer 达到上限后会降级并打印告警，而不是继续无限打开文件。
5. 检查 performance / memory diagnostics，确认 body store 与 ws payload store 能返回活跃 writer 数和上限。
6. 构造“接近上限但尚未打满”的场景，确认 `resource_alerts` 会进入 `warn/critical`，并且 Performance 页直接标红。
7. 压测 app icon / traffic 列表场景，确认不会因为并发图标提取快速放大 fd 消耗。
8. 执行与桌面端恢复链路相关的 E2E / targeted test。

## 校验要求（含 rust-project-validate）

- 先执行与本次修复相关的 E2E 或 targeted test
- 再执行 `rust-project-validate` 要求的格式、lint、测试和构建校验

## 文档更新要求

- 若桌面端行为说明新增“运行期自动恢复”表述，需要同步更新 `docs/desktop.md`
- 若 diagnostics / performance 页增加资源告警能力，需要同步更新 `README.md`
