# System Proxy Reliability and Diagnostics

## 状态

已收敛为可执行技术方案。本文定义下一轮 system proxy 可靠性、wake 后检查、结构化诊断与现场取证能力的落地规格。

### 实施进度（截至 2026-06-17）

已落地：

- `bifrost-power::power_notifications::PowerNotificationWatcher` 已实现 macOS IOKit power notification 监听（`crates/bifrost-power/src/power_notifications.rs`）。
- `bifrost-cli` 已新增 `system-proxy lifecycle-helper` 子命令（`crates/bifrost-cli/src/commands/system_proxy.rs::run_system_proxy_lifecycle_helper`），覆盖 parent-pid + start-time 监控、SIGTERM/SIGINT/SIGHUP 处理、`PowerEvent::SystemHasPoweredOn` 后调用 `reconcile_system_proxy_after_power_wake`（line 825）与 `cleanup_or_restart_managed_runtime`（line 728）。
- `bifrost-admin` 在 enable/disable 路径上接入 lifecycle helper（`crates/bifrost-admin/src/state.rs::SystemProxyLifecycleHelperState` line 62 与 `crates/bifrost-admin/src/handlers/proxy.rs` 的 start/stop helper）。
- `bifrost-core::SystemProxyManager` 已提供 `managed_target_has_live_listener`（`system_proxy.rs` line 787）/ `last_runtime_target_has_live_listener`（line 796）/ `recover_from_crash` 等 guarded restore 基础能力。
- `bifrost-core::system_proxy_recovery` 提供 `is_retryable_recovery_error` / `is_network_services_not_ready_error` / `retry_with_policy`（`system_proxy_recovery.rs` line 21/31/58），对应 network services 未 ready 的 retry 安全网。
- LaunchDaemon one-shot cleanup（`system_proxy_launchd`）保留并继续走 startup recovery（`system_proxy_launchd.rs` line 357/371 使用两次 live listener 检查）。

尚未落地（本轮按 `design/runtime-stability-hardening.md` 实施）：

- `bifrost-core::system_proxy_owner_state` 模块与 `system_proxy_owner_state.json` 文件（含 `runtime_start_mode` / `restartable_runtime` / `helper_pid` / `helper_last_heartbeat_at` / `wake_watcher_status` / `last_*` 字段）。
- `bifrost-core::system_proxy_events` 模块、`logs/system_proxy_events.jsonl` 结构化事件、`.system_proxy_diagnostics.lock` 独立诊断锁、10 MiB rotation。
- 抽出的共享决策函数 `reconcile_system_proxy_after_wake(trigger, ...)` 与 `WakeReconcileOutcome` 枚举（当前 wake 路径以 `reconcile_system_proxy_after_power_wake` + `cleanup_or_restart_managed_runtime` 内联实现，未提供具名共享 API）。
- `bifrost-core::managed_runtime_restart::restart_managed_runtime_before_restore` 显式 API 与 `RuntimeRestartOutcome` 枚举。
- helper 5 秒 heartbeat 写入、`helper_heartbeat_stale` / `helper_heartbeat_recovered` 事件、主进程 watchdog 基于 heartbeat 的检查。
- `bifrost status` 系统代理诊断摘要新增字段（lifecycle helper、wake watcher、last reconcile/cleanup、ManagedRuntimeDead 等）。
- WebUI Settings/StatusBar 风险诊断模型。
- `bifrost doctor system-proxy` 命令、`collection_manifest.json` 与 diagnostic bundle。
- Request error aggregation 中 `network_stack_unready_summary` 5 分钟聚合窗口。
- Windows 平台上的 owner state / event log / parent-death restart-before-restore 端到端验证矩阵。

本节的“已落地”只代表 wake / lifecycle-helper 链路；下面的技术细节章节列出目标规格，代码 vs 文档 gap 以本节为准。

## 背景

Bifrost 启用系统代理后，如果主进程异常退出、macOS 合盖或休眠唤醒后网络服务暂不可读、cleanup 路径在网络栈未 ready 时过早退出，system proxy 可能仍指向已经不可达的 Bifrost listener。用户表现是“整个系统没有网络”。

现有 `design/system-proxy.md` 已覆盖 ownership、crash recovery、lifecycle helper、LaunchDaemon one-shot cleanup、runtime target fallback、PID + start_time 身份判定、macOS network services readiness retry。本文在此基础上补齐四类能力：

1. helper 内 IOKit sleep/wake watcher，唤醒后主动检查系统代理。
2. owner state 与 lifecycle event log，持久记录谁在托管、谁做过恢复、为什么跳过。
3. `status` / WebUI 的风险诊断，直接提示 listener dead、helper missing、external owner。
4. `doctor system-proxy` 诊断包，一次性收集现场并输出摘要。

## 用户目标验证清单

### 必须实现

- 系统代理由 Bifrost 托管时，能从落盘状态回答 owner / runtime / helper / wake watcher / launchd / 最近 cleanup/reconcile 结果。
- macOS sleep/wake 后，运行中的 helper 收到 IOKit power notification 并触发一次受 ownership 保护的 system proxy 检查。
- Bifrost listener dead 且系统代理仍指向 Bifrost target 时，尽快 guarded restore。
- listener dead 但上次 runtime 是可托管 daemon/desktop 模式时，优先尝试自动重启主进程；只有重启失败/超时/不允许重启时才恢复或关闭系统代理。
- Bifrost listener alive 但网络栈未 ready 时，不误恢复、不破坏当前代理。
- 用户反馈类似问题时，可通过 `doctor system-proxy` 获取诊断 bundle，不需要人工拼多天日志。
- 不误伤 Surge / Clash / VPN / 手动配置的外部代理。

### 必须不破坏

- 现有 ownership、crash recovery、lifecycle helper、LaunchDaemon one-shot cleanup、network services readiness retry。
- HTTP / HTTPS / SOCKS 转发、规则匹配、TLS 解包逻辑。
- 前台 CLI 调试进程异常退出的语义：默认只恢复/关闭系统代理，不静默拉起新的主进程。
- Windows 上不引入 IOKit / CFRunLoop / LaunchDaemon / networksetup / macOS unified log 依赖。

### 必须真实验证

- helper 崩溃兜底：`kill -9` 主进程后 helper 恢复 system proxy。
- wake watcher：macOS 合盖 → 唤醒 → 触发 reconcile；listener 不同状态触发不同 outcome。
- 可托管 runtime 自动重启：daemon / desktop 模式下 listener dead → 主进程重启成功后不误关系统代理。
- doctor bundle：一次执行能生成 zip，包含摘要、`scutil --proxy`、`networksetup` snapshot、runtime/proxy/owner state、event log、cleanup daemon 日志。

## 最终决策

| 决策 | 结论 |
| --- | --- |
| wake notification 放置位置 | 放在 lifecycle helper 内，不放在主进程内 |
| 主进程角色 | 保留 shutdown/listener-exit restore、wake-gap reconcile 兜底、status/WebUI 展示 |
| helper 角色 | parent-death cleanup、IOKit wake watcher、helper heartbeat、wake 后 guarded reconcile |
| LaunchDaemon 角色 | 继续 one-shot，只负责 boot/bootstrap/kickstart 后残留 cleanup |
| restore 安全边界 | 只恢复或关闭明确匹配 Bifrost managed target / last runtime target 的系统代理 |
| wake 后 listener alive 但网络未 ready | 记录 `network_stack_unready_summary`，不恢复系统代理 |
| watcher 失败 | 不停止 helper，降级到主进程 wake-gap、LaunchDaemon 与 startup recovery |
| 诊断数据 | 写入 `system_proxy_owner_state.json` 与 `logs/system_proxy_events.jsonl` |
| 诊断状态并发 | owner state 与 event log 使用独立 `.system_proxy_diagnostics.lock` 与原子写，不复用 system proxy 写锁；等待上限 2 秒，超时不得阻塞启动、restart 或恢复 |
| 心跳落点 | helper heartbeat 只更新 owner state，不按 5 秒频率写 JSONL |
| 主进程已死但代理残留 | 对 daemon/desktop 等可托管运行时优先尝试自动重启；重启失败或不可托管时才 guarded restore/disable |
| Windows 边界 | Windows 落地跨平台诊断、helper parent-death cleanup、可托管 runtime restart-before-restore、status/doctor；不实现 IOKit watcher、LaunchDaemon、networksetup readiness |

## 非目标

- 不阻止 macOS sleep（合盖是 forced sleep，应用只能收到通知并尽快 ack）。
- 不接管所有网络可用性问题（上游网络、DNS、Wi-Fi 未 ready 不等同于 Bifrost listener 故障）。
- 不改变 HTTP/HTTPS/SOCKS 转发、规则匹配、TLS 解包逻辑。
- 不把 LaunchDaemon `loaded` 当成 cleanup-daemon 常驻进程；one-shot 退出是正常状态。
- 不在前台 CLI 调试进程异常退出后静默拉起新的主进程；前台模式默认只做恢复/关闭系统代理。

## 总体架构

```text
Layer 0  主进程 graceful restore
         - SIGTERM/SIGINT/SIGHUP
         - listener task 异常退出

Layer 1  lifecycle helper
         - 父进程异常退出 cleanup
         - macOS IOKit wake watcher
         - 可托管运行时 restart-before-restore
         - helper heartbeat

Layer 2  主进程 wake-gap reconcile
         - watcher/helper 不可用时兜底
         - 保留现有 scheduler gap 机制

Layer 3  LaunchDaemon one-shot cleanup
         - boot/bootstrap/kickstart 后检查上次残留

Layer 4  startup recovery
         - 下一次 Bifrost 启动前同步清理 stale system proxy
```

核心原则：

- 所有写系统代理的路径必须持有 `.system_proxy.lock`。
- 所有 restore/disable 必须先确认当前系统代理 target 属于 Bifrost。
- 所有 cleanup/reconcile 决策必须写 lifecycle event。
- helper 与主进程共用同一套 wake reconcile 决策函数，只是 trigger 不同。
- 诊断状态写入不得获取 `.system_proxy.lock`，只能只读现有 ownership；独立 diagnostics lock 也必须有界等待，避免 heartbeat/event 写入阻塞恢复。

## 平台能力矩阵

| 能力 | macOS | Windows | Linux/其他 |
| --- | --- | --- | --- |
| 系统代理读写 | 支持 | 支持 | 不支持 |
| ownership / crash recovery | 支持 | 支持 | 不启用 |
| lifecycle helper | 支持 | 支持（独立进程组） | 不启动 |
| helper parent-death cleanup | 支持 | 支持 | 不支持 |
| helper heartbeat | 支持 | 支持 | 不支持 |
| 可托管运行时自动重启 | 支持 | 支持 parent-death | 不支持 |
| owner state | 支持 | 支持 | 只读 |
| lifecycle event log | 支持 | 支持 | 可写 unsupported/no-op |
| IOKit wake watcher | 支持 | 不支持（`wake_watcher_status=unsupported`） | 不支持 |
| scheduler wake-gap reconcile | 支持 | 不新增 | 不支持 |
| LaunchDaemon one-shot cleanup | 支持 | 不支持 | 不支持 |
| macOS network services readiness retry | 支持 | 不适用 | 不适用 |
| status 诊断 | 完整 helper/watcher/launchd/networksetup | helper/system proxy；watcher unsupported | system proxy unsupported |
| doctor system-proxy | 完整 bundle | Windows bundle，跳过 macOS-only | unsupported summary |

Windows 非回归约束：

- 不引入 IOKit、CFRunLoop、LaunchDaemon、`networksetup`、macOS unified log 依赖。
- 不改变现有 `SystemProxyManager` Windows implementation。
- Windows helper 只做 parent-death cleanup、可托管 runtime restart-before-restore、heartbeat、owner state/event 写入，不做 sleep/wake watcher。
- `wake_watcher_status=unsupported` 是正常状态，不触发 warning，不 restart helper。
- Windows `doctor system-proxy` 必须跳过 macOS-only 采集项，并在 `collection_manifest.json` 中标记 `not_applicable`。

## 产品语义

### wake reconcile 决策

`reconcile_system_proxy_after_wake(trigger, data_dir, runtime_target)` 只在 macOS 启用；Windows 不接入 scheduler gap / power notification wake reconcile。

trigger 枚举：`power_notification` / `scheduler_gap`。

`WakeReconcileOutcome`：

- `KeptManagedProxy`
- `RestartedRuntime`
- `RestartSkipped`
- `RestoredDeadListener`
- `DisabledStaleProxy`
- `SkippedExternalOwner`
- `Retrying`
- `Failed`

决策流程：

1. 读取 `proxy_state.json` / `proxy_backup.json` / `runtime.json`。
2. 读取当前 macOS per-service Web/Secure Web proxy。
3. network services 不可读或返回空 service list：`Retrying`，`reason=network_services_unready`，保留 state，后台 retry，不退回 scutil 聚合状态做 destructive cleanup。
4. 当前系统代理不指向 Bifrost target：`SkippedExternalOwner`。
5. 指向 Bifrost target 且 listener alive：`KeptManagedProxy`。
6. 指向 Bifrost target 且 listener dead：
   - 若 `runtime_start_mode ∈ {daemon, desktop}` 且 `restartable_runtime=true`：先 `RestartedRuntime`；成功后再次探活；失败或超时降级为 `RestartSkipped` → `RestoredDeadListener`。
   - 前台或不可托管 runtime：直接 `RestoredDeadListener`。
7. proxy_state 与实际系统代理 target 不一致：`DisabledStaleProxy`（关代理但保留 backup 供下次启动使用）。

## 技术细节

### `bifrost-power::power_notifications`

封装 macOS-only IOKit power notification（`crates/bifrost-power/src/power_notifications.rs`）：

- FFI：`IORegisterForSystemPower` / `IODeregisterForSystemPower` / `IONotificationPortGetRunLoopSource` / `IONotificationPortDestroy` / `CFRunLoopAddSource` / `CFRunLoopRemoveSource` / `CFRunLoopRun` / `CFRunLoopStop` / `IOAllowPowerChange`。
- `spawn_power_event_watcher(sender) -> Result<PowerWatcherHandle>`：在独立线程运行 CFRunLoop，线程名 `bifrost-system-proxy-power-events`。
- callback 只做两件事：对需要 ack 的 sleep message 调用 `IOAllowPowerChange`；将 power event 投递到 channel。
- callback 禁止直接执行：`networksetup` / listener probe / `SystemProxyManager::recover_from_crash` / 文件锁等待。

消息处理：

- `kIOMessageCanSystemSleep`：立即 ack、写 `system_can_sleep` event，不阻止 idle sleep。
- `kIOMessageSystemWillSleep`：立即 ack、写 `system_will_sleep` event。
- `kIOMessageSystemWillPowerOn`：写 `system_will_power_on` event。
- `kIOMessageSystemHasPoweredOn`：写 `system_has_powered_on` event，worker 线程 debounce 后触发 reconcile。

`PowerWatcherHandle::drop`：注销 IOKit notification / 移除 CFRunLoop source / `CFRunLoopStop` / join watcher thread，超时 2 秒后写 warn 并继续 cleanup。

### `bifrost-core::system_proxy_owner_state`（planned）

文件：`system_proxy_owner_state.json`。字段：

```json
{
  "schema_version": 1,
  "runtime_id": "REQ-...",
  "pid": 12345,
  "started_at_ms": 1780000000000,
  "version": "0.0.xx",
  "binary_path": "/path/to/bifrost",
  "data_dir": "/path/to/data",
  "runtime_start_mode": "foreground|daemon|desktop|unknown",
  "restartable_runtime": false,
  "listener_addr": "127.0.0.1:9900",
  "enabled_source": "cli|admin_api|webui|config|recovery",
  "enabled_at": "2026-06-09T04:00:05Z",
  "clean_shutdown": false,
  "helper_pid": 12346,
  "helper_started_at_ms": 1780000000100,
  "helper_last_heartbeat_at": "2026-06-09T04:01:05Z",
  "wake_watcher_status": "running|failed|unsupported|disabled",
  "wake_watcher_last_event_at": "2026-06-09T04:01:20Z",
  "wake_watcher_last_reconcile_at": "2026-06-09T04:01:22Z",
  "launchd_installed": true,
  "launchd_loaded": true,
  "last_reconcile_at": "2026-06-09T04:02:05Z",
  "last_reconcile_trigger": "periodic|scheduler_gap|power_notification|startup|shutdown|launchd",
  "last_reconcile_result": "success|skipped_external_owner|network_unready|failed",
  "last_cleanup_attempt_at": "2026-06-09T04:03:05Z",
  "last_cleanup_result": "success|retrying|skipped_external_owner|failed",
  "last_runtime_restart_attempt_at": null,
  "last_runtime_restart_result": "not_attempted|started|succeeded|failed|skipped_not_restartable|skipped_clean_stop",
  "last_error": null
}
```

写入规则：

- CLI/daemon start 写基础信息；前台 `bifrost start` 写 `foreground` + `restartable_runtime=false`；`--daemon` / Desktop 写 `daemon|desktop` + `restartable_runtime=true`。
- Admin API / WebUI / CLI 启用系统代理成功后写 `enabled_source` 与 listener target。
- helper 启动后写 `helper_pid / helper_started_at_ms`；heartbeat 每 5 秒更新 `helper_last_heartbeat_at`。
- watcher 启动、失败、停止、收到 wake event 时更新 `wake_watcher_*`。
- periodic reconcile / scheduler wake-gap / power notification reconcile / shutdown restore / launchd cleanup 更新最近 attempt/result。
- clean shutdown 完成 restore 后写 `clean_shutdown=true`，helper / watcher 状态置 disabled。

并发与权限：

- 独立诊断锁 `.system_proxy_diagnostics.lock` 只保护 owner state 与 event log，不得包住 `networksetup`。
- 写入采用 read-merge-atomic-write：持锁 → 读 → merge 本次字段 → 临时文件 → fsync → rename。
- macOS root LaunchDaemon 与普通用户进程均可能写；锁与日志权限沿用 `.system_proxy.lock` 的 nofollow + fd chmod 思路，避免 symlink 跟随。

### `bifrost-core::system_proxy_events`（planned）

文件：`logs/system_proxy_events.jsonl`。事件示例：

```json
{
  "ts": "2026-06-09T04:00:05.931867Z",
  "schema_version": 1,
  "event": "wake_notification_reconcile_completed",
  "trigger": "power_notification",
  "runtime_id": "REQ-...",
  "pid": 12345,
  "helper_pid": 12346,
  "version": "0.0.xx",
  "data_dir": "/path/to/data",
  "current_proxy": { "enabled": true, "host": "127.0.0.1", "port": 9900, "managed_by_bifrost": true },
  "expected_proxy": { "host": "127.0.0.1", "port": 9900 },
  "listener_alive": true,
  "network_services_readable": true,
  "decision": "keep_managed_proxy",
  "error": null,
  "duration_ms": 42
}
```

必须支持事件：`startup_state_snapshot` / `startup_unclean_previous_runtime` / `system_proxy_enable_requested|applied` / `system_proxy_disable_requested|applied` / `helper_start_requested|started` / `helper_heartbeat_stale|recovered` / `helper_missing` / `helper_restart_requested` / `launchd_status_checked|install_requested` / `wake_notification_watcher_started|failed|stopped` / `system_can_sleep|will_sleep|will_power_on|has_powered_on` / `wake_notification_reconcile_started|completed|skipped_listener_alive|restarted_runtime|restored_dead_listener` / `runtime_restart_considered|skipped|started|succeeded|failed` / `wake_gap_detected` / `reconcile_started|completed` / `cleanup_started|retrying|restored|disabled_stale_proxy|skipped_external_owner|failed` / `network_stack_unready_summary`。

写入要求：

- append-only JSONL，单行；先序列化完整 JSON 再持锁 append。
- 单条写入失败不影响主流程，但在主日志打 warn。
- 不记录 URL path / cookie / Authorization / body / rules / values / scripts；host/port 可记录。
- 单文件超过 10 MiB 后 rotate 为 `system_proxy_events.jsonl.1`，最多保留 3 个历史文件。
- doctor bundle 默认只导出 `--since` 时间范围内事件；无时间字段或解析失败的行放入 `diagnostics/event_parse_errors.jsonl`。
- helper 心跳不按 5 秒频率 append JSONL；只 append helper 状态跃迁。

### 当前 wake 路径（已落地内联版）

`crates/bifrost-cli/src/commands/system_proxy.rs`：

- `reconcile_system_proxy_after_power_wake(...)`（line 825）：guarded 决策，命中 `cleanup_or_restart_managed_runtime`（line 728）。
- `cleanup_or_restart_managed_runtime(data_dir)`：调用 `SystemProxyManager::managed_target_has_live_listener` / `last_runtime_target_has_live_listener` / `recover_from_crash`，并透过 `system_proxy_recovery::retry_with_policy` 处理 `network services 未 ready` 场景。
- helper 主循环：line 1015 附近，在 `PowerEvent::SystemHasPoweredOn` 后调用 `reconcile_system_proxy_after_power_wake`。

`crates/bifrost-admin/src/state.rs::SystemProxyLifecycleHelperState`（line 62-335）+ `crates/bifrost-cli/src/commands/start.rs` line 2032 / 3212：

- Admin API / WebUI enable 成功 → `SystemProxyLifecycleHelperState::ensure_started()`。
- start.rs 在启动时未启用 system proxy 的进程也持有该 state，保证运行中打开系统代理时才启动 helper。
- Drop（state.rs line 326）负责 helper 生命周期结束。

## CLI / Web / Admin API

### CLI（已落地）

- `bifrost system-proxy status`：显示 owner / enabled / target / managed_by_bifrost / lifecycle helper / launchd status（macOS）。
- `bifrost system-proxy enable` / `disable`：正常入口，确保 helper 启动/停止。
- `bifrost system-proxy cleanup-daemon`：LaunchDaemon 入口，one-shot recover。
- `bifrost system-proxy lifecycle-helper --parent-pid <pid>`：helper 入口（内部）。

### CLI（planned）

- `bifrost doctor system-proxy [--since=<duration>] [--output=<path>]`：收集诊断 bundle（zip），包含摘要 / `scutil --proxy` / `networksetup` snapshot / runtime/proxy/owner state / `system_proxy_events.jsonl` / cleanup daemon 日志 / `collection_manifest.json`。默认剔除 URL 路径、cookie、Authorization。
- `bifrost status` 系统代理段增补：helper 状态、wake watcher、last reconcile / cleanup、runtime restart。

### Web UI（planned）

- Settings → System Proxy：显示 owner state 摘要、wake watcher status、helper 状态、needs-upgrade reason。
- Status Bar：ManagedRuntimeDead 时高亮，链到 `doctor system-proxy` 说明。

### Admin API（planned）

- `GET /_bifrost/api/system-proxy/diagnostics`：返回 owner state + 最近 N 条 event。
- `POST /_bifrost/api/system-proxy/doctor`：触发 doctor bundle 生成任务。

## Sync 边界

- owner state、event log、helper 状态、wake watcher 状态、launchd status 都是本机 macOS-only 概念，不通过 Sync 分发。
- Sync 分发的是规则、Values、TLS 配置等；系统代理运行时状态不进入 Sync。

## Phase 1 - 4

### Phase 1：owner state + event log 基线（planned）

- 引入 `system_proxy_owner_state.json` 与 `logs/system_proxy_events.jsonl`。
- 独立诊断锁与原子写。
- helper heartbeat + startup snapshot event。

### Phase 2：wake watcher 与 reconcile 抽象（planned）

- 抽出 `reconcile_system_proxy_after_wake(trigger, ...)` 与 `WakeReconcileOutcome`。
- helper 内接入 IOKit watcher（已实现）+ 决策抽象（planned）。
- 主进程 wake-gap 兜底。

### Phase 3：runtime restart-before-restore（planned）

- `restart_managed_runtime_before_restore` API 与 `RuntimeRestartOutcome`。
- daemon / desktop 模式下 listener dead → 重启主进程。
- 前台模式不重启。

### Phase 4：status / doctor / WebUI（planned）

- `bifrost status` 补 helper / watcher / last reconcile。
- `bifrost doctor system-proxy` bundle。
- WebUI Settings / StatusBar 风险模型。

## 测试方案

### 单元测试（已落地）

- `crates/bifrost-core/src/system_proxy_recovery.rs::tests`（line 110/114/122/125）：`is_retryable_recovery_error` / `is_network_services_not_ready_error` 分类。
- `crates/bifrost-core/src/system_proxy_launchd.rs::tests`（line 1217-1219）：networksetup 错误 / 空 services / 状态文件损坏分类。
- `crates/bifrost-core/src/system_proxy.rs::tests`（line 3542 / 3565）：`last_runtime_target_has_live_listener_detects_runtime_port` / `last_runtime_target_has_live_listener_resolves_localhost`。
- 待补：owner state serde、event JSON schema、`reconcile_system_proxy_after_wake` outcome 表格。

### E2E（已落地）

- `e2e-tests/tests/test_system_proxy_e2e.sh`：helper 崩溃兜底、cleanup-daemon one-shot、外部代理保留、running-enable helper 启动。
- 待补：wake watcher 触发、runtime restart-before-restore、owner state/event 断言、doctor bundle。

### human_tests

`human_tests/cli-system-proxy.md`：

- `TC-CSP-16/17/18`：LaunchDaemon 安装 / 升级 / running-enable helper。
- `TC-CSP-31`（planned）：合盖 → 唤醒 → helper 触发 wake reconcile。
- `TC-CSP-32`（planned）：合盖 → 唤醒 → listener dead 且 daemon 模式，主进程被 helper 重启。
- retry 错误分类：`is_retryable_recovery_error` 覆盖 networksetup 暂不可用、空网络服务、临时 IO 错误；不可重试为解析失败、状态文件损坏（cli-system-proxy.md line 719 附近）。
- 平台矩阵：ubuntu-latest / windows-latest / macos-latest `cargo test --workspace --all-features`（line 750）。

### 收尾校验

- `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo test --workspace --all-features`。
- 最后 `rust-project-validate`。

## Review / Fix / Test 闭环方案

### 第 1 轮

- 复核决策矩阵：wake watcher 归属 helper、restart-before-restore 归属 helper、doctor bundle 归属 CLI。
- Review：guarded restore 边界、外部代理 ownership、诊断锁独立、心跳不 spam JSONL、Windows 边界 stub。
- 复测：`system_proxy_recovery::tests` / `system_proxy_launchd::tests` / `system_proxy::tests` 全跑；`test_system_proxy_e2e.sh`；helper 崩溃兜底手测。

### 第 2 轮

- 复核第 1 轮修复；`git status --short` 干净。
- Review：新增字段兼容、event 敏感字段脱敏、doctor bundle 只在 `--since` 窗口内取事件。
- 复测：三平台 `cargo test`；macOS 合盖唤醒手测；`TC-CSP-16/17/18/31/32`（planned 项标注 not yet shipped）。

## 风险与决策

- IOKit callback 若阻塞会拖垮 CFRunLoop：callback 只 ack + 投递 channel，重活交给 worker。
- helper heartbeat 与 event log 争抢锁：独立诊断锁与 owner state 的合并写解决。
- doctor bundle 敏感信息泄漏：不采集 URL 路径 / cookie / Authorization / body / rules / values / scripts；default `--since` 缩小窗口。
- wake watcher 失败降级：状态标 `wake_watcher_status=failed`，主进程 wake-gap 补位；不停止 helper。
- Windows 覆盖：本次不引入 IOKit / LaunchDaemon；`wake_watcher_status=unsupported` 是正常态；`doctor` 跳过 macOS-only 项。
- 可托管 runtime 重启对前台模式的安全防护：前台不重启，避免与调试者显式停止冲突。
- schema 演进：owner state 与 event 使用 `schema_version=1`；破坏性变更需要递增并同步 doctor bundle 解析。
- 回滚：若 wake watcher 在真实机器上引起过多误清理，可通过 `BIFROST_DISABLE_POWER_WATCHER=1` 关闭；helper 仍保留 parent-death cleanup。
